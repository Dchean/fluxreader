//! 后台刷新调度器：定时醒来 → 查"到期"源 → 限并发抓取 → 事件通知前端。
//!
//! 设置实时读取（app_settings JSON 的 autoRefresh/refreshInterval/
//! fetchConcurrency/smartDedup），改设置无需重启即生效（下一个 tick
//! 最多 60s 后跟上）。抓取走 ingestion::refresh_feed_staged 三段式
//! 管线：HTTP 在锁外执行，写库时短暂持锁——并发真正并行。

use crate::state::AppState;
use rusqlite::Connection;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex as StdMutex, Weak};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

/// 调度循环的醒来节奏。每 tick 只跑一条便宜的索引查询，没到期的源直接返回。
const TICK: Duration = Duration::from_secs(60);

/// 并发抓取上限默认值：4（个人规模订阅数下兼顾速度与源站压力）。
/// 用户可在设置页 1–16 调整（fetchConcurrency）。
const DEFAULT_CONCURRENCY: usize = 4;
pub const MAX_CONCURRENCY: usize = 16;

/// 一次刷新批次的完整汇总。`refreshed_feed_urls` 只含本轮 `Applied` 的规范化
/// feed URL，用于跨批次短期去重；在途跳过、代际取代与失败都不进入 footprint。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RefreshBatchReport {
    new_articles: usize,
    failed_feeds: usize,
    refreshed_feed_urls: Vec<String>,
}

/// watch 通道需要可复制的错误载荷；`AppError` 本身不 Clone，故只搬运 wire 字段。
#[derive(Debug, Clone)]
struct SharedRefreshError {
    code: String,
    message: String,
}

impl From<crate::error::AppError> for SharedRefreshError {
    fn from(error: crate::error::AppError) -> Self {
        Self {
            code: error.code,
            message: error.message,
        }
    }
}

impl From<SharedRefreshError> for crate::error::AppError {
    fn from(error: SharedRefreshError) -> Self {
        crate::error::AppError::new(&error.code, error.message)
    }
}

type FlightResult = Result<RefreshBatchReport, SharedRefreshError>;

struct FullRefreshFlight {
    result: tokio::sync::watch::Sender<Option<FlightResult>>,
}

impl FullRefreshFlight {
    async fn wait(&self) -> crate::error::AppResult<RefreshBatchReport> {
        let mut rx = self.result.subscribe();
        loop {
            if let Some(value) = rx.borrow().clone() {
                return value.map_err(Into::into);
            }
            if rx.changed().await.is_err() {
                return Err(crate::error::AppError::internal(
                    "刷新任务结果通道在发布前关闭",
                ));
            }
        }
    }
}

/// flight 的生命周期必须由后台任务本身持有：调用方 future 被取消时，任务仍会
/// 完成并发布结果；任务 panic 时 Drop 守卫也会发布失败并移除映射，避免后续
/// 调用永久等待一个没有领跑者的 flight。
struct FlightCompletionGuard {
    key: usize,
    flight: Arc<FullRefreshFlight>,
    finished: bool,
}

impl FlightCompletionGuard {
    fn new(key: usize, flight: Arc<FullRefreshFlight>) -> Self {
        Self {
            key,
            flight,
            finished: false,
        }
    }

    fn publish(&self, outcome: FlightResult) {
        self.flight.result.send_replace(Some(outcome));
        let mut flights = FULL_REFRESH_FLIGHTS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if flights
            .get(&self.key)
            .is_some_and(|current| Arc::ptr_eq(current, &self.flight))
        {
            flights.remove(&self.key);
        }
    }

    fn finish(mut self, outcome: FlightResult) {
        self.publish(outcome);
        self.finished = true;
    }
}

impl Drop for FlightCompletionGuard {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let reason = if std::thread::panicking() {
            "刷新任务 panic，批次未完成"
        } else {
            "刷新任务被取消，批次未完成"
        };
        self.publish(Err(SharedRefreshError::from(
            crate::error::AppError::internal(reason),
        )));
    }
}

/// 手动/托盘全刷的 single-flight：同一数据库同时只有一个领跑者，后到调用
/// 订阅同一 watch 并复用结果，不再各自创建 Semaphore(fetchConcurrency)。
// Note: 全刷 single-flight、共享批次闸门与 hybrid 源集合 — 见 .agents/notes/implemented/architecture/2026-11-04-刷新编排与源名分域.md
static FULL_REFRESH_FLIGHTS: LazyLock<StdMutex<HashMap<usize, Arc<FullRefreshFlight>>>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));

/// 所有批次刷新（手动全刷、托盘全刷、调度器到期批）共享的串行闸门。
/// 同一数据库同一时刻只有一批在跑；不同批次类型不合并结果（到期批只抓 due，
/// 手动批抓全部/本地全部），后者等前者结束后按自己的选择集执行。
static REFRESH_BATCH_GATES: LazyLock<StdMutex<HashMap<usize, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));

/// 跨批次自动去重 footprint 的存活时间。只有调度 due 批读取它；手动/托盘
/// 全刷是用户的显式刷新请求，必须绕过 footprint 重新抓取。90 秒用于压制
/// 调度器与其它自动入口的短窗口重复，不压制用户的显式动作。
const RECENT_REFRESH_TTL: Duration = Duration::from_secs(90);

struct RecentRefreshes {
    /// Weak 防止测试/重开数据库时指针地址复用而继承另一个库的 footprint。
    db: Weak<tokio::sync::Mutex<Connection>>,
    /// 键是规范化 feed URL，不是 SQLite rowid：删除最大 id 后新源可能复用同一
    /// rowid，不能因此继承旧源的“刚刷新”状态。
    feeds: HashMap<String, Instant>,
}

static RECENT_REFRESHES: LazyLock<StdMutex<HashMap<usize, RecentRefreshes>>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));

#[cfg(test)]
static PANIC_NEXT_FULL_REFRESH: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// 托盘菜单项由 setup 注册；设置页改 syncMode 时经 command 就地更新文案，
/// 避免原生菜单在运行期继续显示「全部」却实际跳过服务端来源。
static TRAY_REFRESH_ITEM: LazyLock<StdMutex<Option<tauri::menu::MenuItem<tauri::Wry>>>> =
    LazyLock::new(|| StdMutex::new(None));

#[cfg(test)]
static LAST_TRAY_REFRESH_MODE: LazyLock<StdMutex<Option<String>>> =
    LazyLock::new(|| StdMutex::new(None));

#[cfg(test)]
type TrayTextWriter = fn(&str);

#[cfg(test)]
static TRAY_TEXT_WRITER: LazyLock<StdMutex<Option<TrayTextWriter>>> =
    LazyLock::new(|| StdMutex::new(None));

pub fn refresh_all_menu_label(sync_mode: &str) -> &'static str {
    if sync_mode == "hybrid" {
        "刷新本地订阅源"
    } else {
        "刷新全部订阅源"
    }
}

pub fn register_tray_refresh_item(item: tauri::menu::MenuItem<tauri::Wry>) {
    *TRAY_REFRESH_ITEM.lock().unwrap_or_else(|e| e.into_inner()) = Some(item);
}

pub fn set_tray_refresh_mode(sync_mode: &str) {
    #[cfg(test)]
    {
        *LAST_TRAY_REFRESH_MODE
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(sync_mode.to_string());
    }
    let item = TRAY_REFRESH_ITEM.lock().unwrap_or_else(|e| e.into_inner());
    // 单元测试只验证「配置应用触发了模式同步」，不链接真实 Tauri 菜单实现：
    // 测试 exe 默认加载 comctl32 v5，而该实现依赖 v6 的 TaskDialogIndirect，
    // 会在测试启动前报 STATUS_ENTRYPOINT_NOT_FOUND。
    #[cfg(not(test))]
    {
        if let Some(item) = item.as_ref() {
            if let Err(e) = item.set_text(refresh_all_menu_label(sync_mode)) {
                log::warn!("scheduler: 更新托盘刷新文案失败: {e}");
            }
        }
    }
    #[cfg(test)]
    {
        if let Some(writer) = *TRAY_TEXT_WRITER.lock().unwrap_or_else(|e| e.into_inner()) {
            writer(refresh_all_menu_label(sync_mode));
        }
        drop(item);
    }
}

#[cfg(test)]
pub(crate) fn take_last_tray_refresh_mode_for_test() -> Option<String> {
    LAST_TRAY_REFRESH_MODE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
}

#[cfg(test)]
pub(crate) fn set_tray_text_writer_for_test(writer: Option<TrayTextWriter>) {
    *TRAY_TEXT_WRITER.lock().unwrap_or_else(|e| e.into_inner()) = writer;
}

fn db_key(db: &Arc<tokio::sync::Mutex<Connection>>) -> usize {
    Arc::as_ptr(db) as usize
}

fn batch_gate(db: &Arc<tokio::sync::Mutex<Connection>>) -> Arc<tokio::sync::Mutex<()>> {
    let key = db_key(db);
    let mut gates = REFRESH_BATCH_GATES
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    gates
        .entry(key)
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

fn prune_recent_refreshes(refreshes: &mut HashMap<usize, RecentRefreshes>, now: Instant) {
    refreshes.retain(|_, entry| {
        if entry.db.strong_count() == 0 {
            return false;
        }
        entry.feeds.retain(|_, refreshed_at| {
            now.saturating_duration_since(*refreshed_at) < RECENT_REFRESH_TTL
        });
        !entry.feeds.is_empty()
    });
}

fn filter_recently_refreshed(
    db: &Arc<tokio::sync::Mutex<Connection>>,
    selected: Vec<crate::db::FeedRefreshCandidate>,
) -> Vec<crate::db::FeedRefreshCandidate> {
    if selected.is_empty() {
        return selected;
    }
    let key = db_key(db);
    let now = Instant::now();
    let mut refreshes = RECENT_REFRESHES.lock().unwrap_or_else(|e| e.into_inner());
    prune_recent_refreshes(&mut refreshes, now);
    let Some(entry) = refreshes.get(&key) else {
        return selected;
    };
    selected
        .into_iter()
        .filter(|candidate| !entry.feeds.contains_key(&candidate.normalized_url))
        .collect()
}

fn record_recent_refreshes(
    db: &Arc<tokio::sync::Mutex<Connection>>,
    refreshed_feed_urls: &[String],
) {
    if refreshed_feed_urls.is_empty() {
        return;
    }
    let key = db_key(db);
    let now = Instant::now();
    let mut refreshes = RECENT_REFRESHES.lock().unwrap_or_else(|e| e.into_inner());
    prune_recent_refreshes(&mut refreshes, now);
    let entry = refreshes.entry(key).or_insert_with(|| RecentRefreshes {
        db: Arc::downgrade(db),
        feeds: HashMap::new(),
    });
    for feed_url in refreshed_feed_urls {
        entry.feeds.insert(feed_url.clone(), now);
    }
}

/// 从 app_settings JSON 里读 autoRefresh / refreshInterval / smartDedup /
/// fetchConcurrency。async 版：在调度循环（tokio worker）里调用。
///
/// 注（TASK-069 审查 F3）：这里保留一次 raw 聚合读取，是因为 refreshInterval 与
/// fetchConcurrency 是**数值型**（带区间夹取），N-硬2 明确将数值读取推迟到后续任务，
/// 布尔助手不覆盖该语义。已收口的纯布尔读取仍走 app_settings_bool/app_settings_str。
///
/// OPT-011：refreshInterval 的解析收口到 read_refresh_interval（5–720 整数，
/// 与前端校验/UI 滑杆同口径；整数值小数如 30.0 等价接受，30.5 拒绝）。
async fn read_refresh_config(
    db: &Arc<tokio::sync::Mutex<rusqlite::Connection>>,
) -> (bool, i64, bool, usize) {
    let conn = db.lock().await;
    let raw = crate::db::get_setting(&conn, "app_settings").ok().flatten();
    let mut enabled = true;
    let mut interval = 30i64;
    let mut dedup = false;
    let mut concurrency = DEFAULT_CONCURRENCY;
    if let Some(json) = raw
        .as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
    {
        if let Some(v) = json.get("autoRefresh").and_then(|v| v.as_bool()) {
            enabled = v;
        }
        if let Some(v) = read_refresh_interval(&json) {
            interval = v;
        }
        if let Some(v) = json.get("smartDedup").and_then(|v| v.as_bool()) {
            dedup = v;
        }
        if let Some(v) = json.get("fetchConcurrency").and_then(|v| v.as_i64()) {
            if (1..=MAX_CONCURRENCY as i64).contains(&v) {
                concurrency = v as usize;
            }
        }
    }
    (enabled, interval, dedup, concurrency)
}

/// refreshInterval 的数值读取单点（OPT-011）：只接受 5–720 的**整数**。
/// serde_json 里 `30` 解析为 i64、`30.0`/`30.5` 为 f64——整数值的 f64（30.0）
/// 等价接受；非整数小数（30.5）与越界值一律 None（调用方回落默认），
/// 与前端 settingsValidation（区间 + Number.isInteger）逐字同口径，
/// 不会出现「客户端收下小数、Rust 静默回默认」的双侧分叉。
fn read_refresh_interval(v: &serde_json::Value) -> Option<i64> {
    let raw = v.get("refreshInterval")?;
    let as_int = raw
        .as_i64()
        .or_else(|| raw.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64))?;
    (5..=720).contains(&as_int).then_some(as_int)
}

/// 同步模式（app_settings.syncMode，UI 词条「本机抓取 / 跟随服务端」）：
/// - `hybrid` 跟随服务端：后台、手动与托盘刷新都跳过 origin='remote' 源
///   （内容走服务端同步；手动入口文案为「刷新本地订阅源」）
/// - `direct` 本机抓取（默认/未配置）：全部源直连抓取（旧行为）
///
/// 锁内读（调用方持 conn）。
fn read_sync_mode_conn(conn: &rusqlite::Connection) -> String {
    /* TASK-068：收口为类型化助手（默认值语义不变） */
    crate::db::app_settings_str(conn, "syncMode", "direct")
}

/// 手动/托盘全刷入口（忽略到期时间与自动去重 footprint）。并发重复调用复用
/// 同一 in-flight 结果；一次批次完成后的下一次显式全刷会重新抓取。
/// 同步模式语义与后台一致：direct 抓全部来源；hybrid 排除 origin='remote'
///（保留 local 及其它非 remote 来源；服务端来源由同步提供，避免
/// direct/remote 两份文章）。
pub async fn refresh_all(
    db: &Arc<tokio::sync::Mutex<rusqlite::Connection>>,
    http: &reqwest::Client,
) -> crate::error::AppResult<(usize, usize)> {
    let key = db_key(db);
    let (flight, leader) = {
        let mut flights = FULL_REFRESH_FLIGHTS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match flights.get(&key) {
            Some(existing) => (existing.clone(), false),
            None => {
                let (tx, _) = tokio::sync::watch::channel(None);
                let created = Arc::new(FullRefreshFlight { result: tx });
                flights.insert(key, created.clone());
                (created, true)
            }
        }
    };
    if !leader {
        let report = flight.wait().await?;
        return Ok((report.new_articles, report.failed_feeds));
    }

    let db = db.clone();
    let http = http.clone();
    let guard = FlightCompletionGuard::new(key, flight.clone());
    tokio::spawn(async move {
        let report = run_full_refresh(&db, &http).await;
        guard.finish(report.map_err(Into::into));
    });
    let report = flight.wait().await?;
    Ok((report.new_articles, report.failed_feeds))
}

async fn run_full_refresh(
    db: &Arc<tokio::sync::Mutex<rusqlite::Connection>>,
    http: &reqwest::Client,
) -> crate::error::AppResult<RefreshBatchReport> {
    #[cfg(test)]
    if PANIC_NEXT_FULL_REFRESH.swap(false, std::sync::atomic::Ordering::SeqCst) {
        panic!("injected full refresh panic");
    }
    let gate = batch_gate(db);
    let _permit = gate.lock().await;
    // TASK-064 N3：smartDedup 照常生效——此前写死 false，开智能去重的用户手动
    // 全刷会放行跨源同文（文章翻倍且不可逆）。模式只决定源集合，不解除去重。
    let (_, _, dedup, concurrency) = read_refresh_config(db).await;
    let report =
        refresh_feeds_inner_with_concurrency(db, http, None, dedup, concurrency, true).await?;
    record_recent_refreshes(db, &report.refreshed_feed_urls);
    Ok(report)
}

/// 抓取所有到期源（后台调度入口，自动使用成功 footprint 去重；并发上限 =
/// 设置 fetchConcurrency，默认 4）。
/// HTTP 在锁外执行（refresh_feed_staged），写库时短暂持锁。
/// 返回批次报告；数据库查询失败向上返回，不能被伪装成空批成功。
/// 同步模式作用点：hybrid（跟随服务端）→ 到期查询跳过 origin='remote' 的源
/// （服务端源内容由后端同步提供）；direct → 全部源照常直连（旧行为）。
async fn refresh_due_feeds(
    db: &Arc<tokio::sync::Mutex<rusqlite::Connection>>,
    http: &reqwest::Client,
) -> crate::error::AppResult<RefreshBatchReport> {
    let gate = batch_gate(db);
    let _permit = gate.lock().await;
    let (_, interval_min, dedup, concurrency) = read_refresh_config(db).await;
    let report = refresh_feeds_inner_with_concurrency(
        db,
        http,
        Some(interval_min),
        dedup,
        concurrency,
        false,
    )
    .await?;
    record_recent_refreshes(db, &report.refreshed_feed_urls);
    Ok(report)
}

/// 集成测试入口：与调度循环使用同一到期批实现（含共享闸门、footprint 与查询错误
/// 上抛），不启动 8 秒延迟的循环。
#[doc(hidden)]
pub async fn refresh_due_once(
    db: &Arc<tokio::sync::Mutex<rusqlite::Connection>>,
    http: &reqwest::Client,
) -> crate::error::AppResult<(usize, usize)> {
    let report = refresh_due_feeds(db, http).await?;
    Ok((report.new_articles, report.failed_feeds))
}

/// 抓取实现：`Some(interval_min)` 只抓到期源（模式过滤在查询内做），
/// `None` 为手动/托盘全量（direct 含全部来源，hybrid 排除 origin='remote'）；
/// `dedup` 透传给入库管线
/// （TASK-064 N3：两条路径都读设置，不再对手动全量写死 false）。
/// `force=true` 仅用于用户显式全刷，绕过 footprint；due 批传 false。
/// 并发上限取设置值（全量入口同样尊重 fetchConcurrency）。
async fn refresh_feeds_inner_with_concurrency(
    db: &Arc<tokio::sync::Mutex<rusqlite::Connection>>,
    http: &reqwest::Client,
    due_filter: Option<i64>,
    dedup: bool,
    concurrency: usize,
    force: bool,
) -> crate::error::AppResult<RefreshBatchReport> {
    use tokio::sync::Semaphore;
    let runtime = crate::sync::runtime::for_db(db);
    runtime.ensure_running()?;
    let selected: Vec<crate::db::FeedRefreshCandidate> = {
        let conn = db.lock().await;
        match due_filter {
            // 调度路径：模式判定在锁内一次完成（读 settings + 查询同临界区）
            Some(interval_min) => {
                let include_remote = read_sync_mode_conn(&conn) != "hybrid";
                crate::db::feeds_due_for_refresh_candidates(&conn, interval_min, include_remote)
            }
            // 手动/托盘全量：hybrid（跟随服务端）排除 origin='remote'，保留
            // local 及其它非 remote 来源——服务端源内容由后端同步提供，直连抓取
            // 会产生 source='direct' 文章与已有 remote 文章重复（guid 不同 +
            // 智能去重默认关），导致文章翻倍、状态错乱、未读数对不齐。
            // direct 模式则全部直连（旧行为）。
            None => {
                let include_remote = read_sync_mode_conn(&conn) != "hybrid";
                crate::db::feeds_all_candidates(&conn, include_remote)
            }
        }?
    };
    // 手动/托盘是用户的显式刷新请求，必须绕过短期 footprint；调度 due 批才是
    // 自动去重入口。并发 manual 仍由 full-refresh flight 复用同一结果。
    let selected = if force {
        selected
    } else {
        filter_recently_refreshed(db, selected)
    };
    if selected.is_empty() {
        return Ok(RefreshBatchReport::default());
    }
    log::info!(
        "scheduler: {} feed(s) selected, concurrency={concurrency}",
        selected.len()
    );

    let sem = Arc::new(Semaphore::new(concurrency));
    let mut handles = Vec::with_capacity(selected.len());
    for candidate in selected {
        let sem = sem.clone();
        let db = db.clone();
        let http = http.clone();
        let runtime = runtime.clone();
        handles.push(tokio::spawn(async move {
            let _permit = tokio::select! {
                biased;
                _ = runtime.wait_for_stop() => return Err(crate::sync::runtime::cancelled()),
                permit = sem.acquire_owned() => permit,
            };
            runtime.ensure_running()?;
            // Note: 退出信号同时关闭排队抓取与在途异步链，flight/CAS 保持原契约 — 见 .agents/notes/implemented/architecture/2026-11-05-同步运行生命周期与诊断.md
            tokio::select! {
                biased;
                _ = runtime.wait_for_stop() => Err(crate::sync::runtime::cancelled()),
                result = crate::ingestion::refresh_feed_staged(&db, &http, candidate.id, dedup) => {
                    result.map(|outcome| (candidate.id, outcome))
                },
            }
        }));
    }
    let mut report = RefreshBatchReport::default();
    for h in handles {
        absorb_refresh_worker(h, &mut report).await;
    }
    runtime.ensure_running()?;
    Ok(report)
}

async fn absorb_refresh_worker(
    handle: tokio::task::JoinHandle<
        crate::error::AppResult<(i64, crate::ingestion::FeedRefreshOutcome)>,
    >,
    report: &mut RefreshBatchReport,
) {
    match handle.await {
        Ok(Ok((feed_id, outcome))) => match outcome {
            crate::ingestion::FeedRefreshOutcome::Applied {
                new_articles,
                feed_url,
            } => {
                report.new_articles += new_articles;
                report
                    .refreshed_feed_urls
                    .push(crate::db::normalize_url(&feed_url));
            }
            crate::ingestion::FeedRefreshOutcome::SkippedInFlight => {
                log::debug!("scheduler: feed {feed_id} 已有刷新在途，批次不记账");
            }
            crate::ingestion::FeedRefreshOutcome::Superseded => {
                log::debug!("scheduler: feed {feed_id} 的写回已被更新代际取代，不记账");
            }
        },
        Ok(Err(error)) => {
            log::warn!("scheduler: 源刷新失败: {error}");
            report.failed_feeds += 1;
        }
        Err(error) => {
            log::error!("scheduler: 源刷新任务异常结束: {error}");
            report.failed_feeds += 1;
        }
    }
}

/// 启动后台调度循环。启动后等 8s 再跑第一轮（避开启动期的 UI 抢锁）。
pub fn spawn_scheduler(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        // 只 clone Send 的部件（Arc<Mutex> + Client），不跨 await 持有 tauri State
        let db = app.state::<AppState>().db.clone();
        let http = app.state::<AppState>().http.clone();
        let runtime = crate::sync::runtime::for_db(&db);
        tokio::select! {
            _ = runtime.wait_for_stop() => return,
            _ = tokio::time::sleep(Duration::from_secs(8)) => {},
        }
        loop {
            if runtime.is_stopped() {
                return;
            }
            let (enabled, _interval, _dedup, _concurrency) = read_refresh_config(&db).await;
            if enabled {
                match refresh_due_feeds(&db, &http).await {
                    Ok(report) if report.new_articles > 0 || report.failed_feeds > 0 => {
                        let _ = app.emit(
                            "feeds-updated",
                            serde_json::json!({
                                "new_articles": report.new_articles,
                                "failed_feeds": report.failed_feeds
                            }),
                        );
                        // 新文章系统通知（notifyOnNewArticles 开关，默认关；
                        // 窗口隐藏/失焦时才发——正在看应用时不打扰）
                        if report.new_articles > 0 && should_notify(&db, &app).await {
                            notify_new_articles(&app, report.new_articles);
                        }
                    }
                    Ok(_) => {}
                    Err(error) => {
                        log::error!("scheduler: 到期源查询失败，本轮跳过: {error}");
                        crate::diagnostics::record(0, "refresh", "error", Some(&error));
                        let _ = app.emit("sync-error", serde_json::json!({
                            "source": "refresh", "message": format!("后台刷新失败：{}", error.message)
                        }));
                    }
                }
            }
            // 后端后台自动同步（autoSync 开关，默认开）：
            // 到期才跑轻量同步（push 队列 + changed_after 增量 pull）
            auto_sync_backend(&db, &http, &app).await;
            tokio::select! {
                _ = runtime.wait_for_stop() => return,
                _ = tokio::time::sleep(TICK) => {},
            }
        }
    });
}

/// 后端自动同步：读 autoSync（默认开）与刷新间隔，
/// 到期（now - last_sync ≥ refreshInterval 分钟）时跑轻量同步。
/// 失败经同步活动终态上报；下个 tick 仍会因 last_sync 未推进而重试。
/// 状态被拉平后发 feeds-updated——前端列表/未读计数与 DB 不再脱节
/// （pull 改变了 is_read 但用户无感知的"静默漂移"问题）。
async fn auto_sync_backend(
    db: &Arc<tokio::sync::Mutex<Connection>>,
    http: &reqwest::Client,
    app: &AppHandle,
) {
    run_background_sync(db, http, app, false).await;
}

struct BackendSyncPlan {
    normal_due: bool,
    discovery_due: bool,
    outbox_pending: bool,
}

async fn backend_sync_plan(
    db: &Arc<tokio::sync::Mutex<Connection>>,
    focus: bool,
) -> crate::error::AppResult<Option<BackendSyncPlan>> {
    let now = chrono::Utc::now().timestamp();
    let (on, interval_min, connected, last_sync, has_binding, discovery_due, outbox_pending) = {
        let conn = db.lock().await;
        let settings = crate::db::get_setting(&conn, "app_settings")?
            .map(|raw| serde_json::from_str::<serde_json::Value>(&raw))
            .transpose()?;
        let on = settings
            .as_ref()
            .and_then(|value| value.get("autoSync"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
        if !on {
            return Ok(None);
        }
        // refreshInterval 是数值型（带区间夹取），N-硬2 明确推迟到后续任务；
        // 仅在 autoSync 开启时才需要解析，故放在早退之后（避免无谓的二次读取）。
        // OPT-011：解析收口到 read_refresh_interval（5–720 整数，与刷新调度同一口径）。
        let interval = settings
            .as_ref()
            .and_then(read_refresh_interval)
            .unwrap_or(30);
        let (connected, account_key) = match crate::sync::current_account(&conn)? {
            Some(ctx) => (true, Some(ctx.key)),
            None => (false, None),
        };
        if !connected {
            return Ok(None);
        }
        let last = crate::db::last_sync_ts(&conn)?;
        let has_binding = crate::db::has_remote_feed_binding(&conn)?;
        let discovery_due = match account_key.as_deref() {
            Some(key) => crate::db::zero_binding_discovery_due(&conn, key, now)?,
            None => false,
        };
        // A-P2-2：订阅 outbox 是否有待推行（本地查询，零网络成本）。有 pending
        // 才值得构建会话执行写回；无 pending 不发任何请求，后台同步保持便宜。
        // key 屏障与执行器一致；读取失败由活动终态呈现，不把未知当无待办。
        let outbox_pending = match account_key.as_deref() {
            Some(key) => crate::db::outbox_has_pending(&conn, key)?,
            None => false,
        };
        (
            on,
            interval,
            connected,
            last,
            has_binding,
            discovery_due,
            outbox_pending,
        )
    };
    if !on || !connected {
        return Ok(None);
    }
    let normal_due =
        focus || (has_binding && backend_sync_due(now, interval_min, true, last_sync, false));
    if !normal_due && !discovery_due && !outbox_pending {
        return Ok(None);
    }
    Ok(Some(BackendSyncPlan {
        normal_due,
        discovery_due,
        outbox_pending,
    }))
}

async fn run_background_sync(
    db: &Arc<tokio::sync::Mutex<Connection>>,
    http: &reqwest::Client,
    app: &AppHandle,
    focus: bool,
) {
    let runtime = crate::sync::runtime::for_db(db);
    if runtime.is_stopped() || matches!(backend_sync_plan(db, focus).await, Ok(None)) {
        return;
    }
    let db = db.clone();
    let http = http.clone();
    let app = app.clone();
    let source = if focus { "focus" } else { "auto" };
    let _ = runtime
        .run(source, async move {
            let Some(plan) = backend_sync_plan(&db, focus).await? else {
                return Ok(crate::sync::SyncReport::default());
            };
            let mut report = crate::sync::SyncReport::default();
            // A-P2-2：outbox 有 pending 时先执行订阅写回。执行器自带 PUSH_LOCK 串行化
            // 与账号 key/代际屏障（见 push_subscription_outbox）；构建失败的落账由
            // try_push 内的 record_outbox_session_block 承担（A-P2-6）。排在 light/
            // discovery 之前：先收敛本地意图，再做拉取对账（与 feeds_phase 的 push →
            // pull 次序一致）。
            // Note: 订阅 outbox 接入后台自动调度 — 见 .agents/notes/implemented/architecture/2026-10-14-订阅写回outbox执行器.md
            if plan.outbox_pending {
                crate::sync::runtime::stage("feeds");
                report.absorb(crate::sync::push_subscription_changes(&db, &http).await?);
            }
            crate::sync::runtime::stage("states");
            let result = if plan.normal_due {
                crate::sync::sync_light(&db, &http).await
            } else if plan.discovery_due {
                crate::sync::sync_subscription_discovery(&db, &http).await
            } else {
                Ok(crate::sync::SyncReport::default())
            };
            match result {
                Ok(r) => {
                    log::info!(
                        "scheduler: 后端自动同步完成：推 {}/拉 {} 项、对账 {} 项，{} 错误",
                        r.pushed_states,
                        r.pulled_entries,
                        r.merged_states,
                        r.errors.len()
                    );
                    /* OPT-007（F11 / R3 / R4 / R5）：数据变化事件按**实际变更集**驱动——
                    判定单点在 SyncReport::has_data_change：条目层真实变化
                    （entries_data_changed：新插入 / 正文·绑定真实改变 / 读藏值实际改变，
                    每个成功变更点即时合并）或 merged_states（本消费路径上为 states 阶段
                    对账值条件写命中行——真实变化；订阅阶段的无条件绑定计数不在
                    sync_light 路径上，见 mod.rs 注释）或订阅层增删（pulled_feeds /
                    removed_feeds）。pulled_entries 是「处理过/已接受」计数（守卫通过但
                    值未变也计数），不冒充变化。空变化（只有推送确认/错误）不发，避免
                    无谓 reload 抖动。payload 保持既有字段（new_articles 固定 0 为保留
                    策略），前端零改动。 */
                    report.absorb(r);
                }
                Err(error) => {
                    report.note_client_error(&error);
                    report.errors.push(error.to_string());
                }
            }
            if report.has_data_change()
                && app
                    .emit(
                        "feeds-updated",
                        serde_json::json!({
                            "new_articles": 0,
                            "failed_feeds": 0,
                            "pulled_entries": report.pulled_entries,
                            "merged_states": report.merged_states
                        }),
                    )
                    .is_err()
            {
                report
                    .errors
                    .push("同步已写入本机，但界面更新通知失败，请重试刷新".into());
            }
            Ok(report)
        })
        .await;
}

/// 有绑定时沿用条目游标间隔；零绑定时改用独立发现节流，避免 `last_sync_ts=0`
/// 令 scheduler 每个 TICK 都发起认证/订阅请求。
fn backend_sync_due(
    now: i64,
    interval_min: i64,
    has_binding: bool,
    last_sync: i64,
    discovery_due: bool,
) -> bool {
    if has_binding {
        now - last_sync >= interval_min * 60
    } else {
        discovery_due
    }
}

/* ============================================================
W3-3：窗口聚焦触发的 light 同步（60s 节流；30 分钟周期判据零改动）
============================================================ */

/// 聚焦触发的最小间隔（秒）：距上次触发不足 60s 直接跳过。
/// 依据：回窗口即看到服务端新状态（同步即得性）与代价（每次 light 全链
/// 的往返）的折中——连续 alt-tab 不重复发起；60s 与调度 TICK 同量级，
/// 不引入新节拍。聚焦只约束自身触发，不影响 refresh/同步调度循环的节奏。
const FOCUS_SYNC_THROTTLE_SECS: i64 = 60;

/// 上次聚焦触发时刻（unix 秒；0 = 从未触发）。事件回调内 CAS 认领窗口，
/// 并发焦点事件只有一个胜出——节流判定与计时更新是同一原子操作，无竞态。
static FOCUS_LAST_TRIGGER: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// 节流判据（纯函数，单测锁定）：now 距上次触发 ≥ 60s 才允许再次触发。
fn focus_sync_due(now: i64, last_trigger: i64) -> bool {
    now.saturating_sub(last_trigger) >= FOCUS_SYNC_THROTTLE_SECS
}

/// 窗口获得焦点（Focused(true)）时的同步入口（lib.rs on_window_event 调用）。
/// 认领节流窗口后异步执行：前置判定（autoSync 开启 + 有可用凭据，与
/// auto_sync_backend 同源）→ outbox pending 写回（有 pending 才发请求）→
/// sync_light 全链。节流窗口按「焦点事件」计：认领后即使前置判定不通过
/// 也不回滚（换取无竞察的单一触发，代价只是启用 autoSync 后首窗最多顺延 60s）。
pub fn on_window_focused(app: AppHandle) {
    use std::sync::atomic::Ordering;
    let now = chrono::Utc::now().timestamp();
    let last = FOCUS_LAST_TRIGGER.load(Ordering::Relaxed);
    if !focus_sync_due(now, last) {
        return;
    }
    if FOCUS_LAST_TRIGGER
        .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return; // 并发焦点事件已被认领，跳过
    }
    tauri::async_runtime::spawn(async move {
        focus_triggered_sync(&app).await;
    });
}

/// 聚焦同步执行体：与 auto_sync_backend 的 normal_due 分支同链
/// （outbox pending 检查 → 订阅写回 → sync_light → 数据变化事件），
/// 但不做 30 分钟/发现节流判定——聚焦只受自身 60s 节流约束。周期同步的
/// 到期判定、发现节流与调度循环节奏零改动。
async fn focus_triggered_sync(app: &AppHandle) {
    let db = app.state::<AppState>().db.clone();
    let http = app.state::<AppState>().http.clone();
    run_background_sync(&db, &http, app, true).await;
}

/// 通知开关开启 且 主窗口不可见（最小化到托盘/失焦）。
async fn should_notify(
    db: &Arc<tokio::sync::Mutex<rusqlite::Connection>>,
    app: &AppHandle,
) -> bool {
    let on = {
        let conn = db.lock().await;
        /* TASK-069：notifyOnNewArticles 布尔读取收口为类型化助手（默认值语义不变） */
        crate::db::app_settings_bool(&conn, "notifyOnNewArticles", false)
    };
    if !on {
        return false;
    }
    !app.get_webview_window("main")
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false)
}

/// Windows toast：新文章到达。
fn notify_new_articles(app: &AppHandle, count: usize) {
    use tauri_plugin_notification::NotificationExt;
    let _ = app
        .notification()
        .builder()
        .title("FluxReader 新文章")
        .body(format!("后台刷新抓到 {count} 篇新文章，点击查看"))
        .show();
}

/* ============================================================
封面后台补全（og:image 兜底）
============================================================ */

// Note: 候选覆盖面（全来源）、请求预算与退出终止语义 — 见
// .agents/notes/implemented/feature/2026-10-16-封面补全覆盖面与预算边界.md

/// 每轮最多补全的封面数（防一次性扫全库 + 轰炸源站）。
const COVER_BACKFILL_BATCH: i64 = 20;
/// 封面补全并发上限（低优先级，比 feed 抓取的默认 4 更低，避免抢带宽）。
const COVER_BACKFILL_CONCURRENCY: usize = 2;
/// 封面补全循环间隔：60s 醒一次，每轮最多处理一批，处理完下一批等下轮。
const COVER_BACKFILL_TICK: Duration = Duration::from_secs(60);
/// 单请求（抓文章页）超时。
const COVER_BACKFILL_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// 进程内封面 URL 结果缓存上限（OPT-017D；成功与失败共用容量）。
///
/// 语义：尝试成功记有限成功图（同URL后继条目复用），无图/抓取失败记10分钟
/// 负缓存；DB 写失败与终止信号中断不算「已尝试」。
/// 缓存写满按插入序逐出最旧 URL；失败在 TTL 到期或被逐出后可重试，每轮请求量由
/// [`COVER_BACKFILL_BATCH`] / [`COVER_BACKFILL_CONCURRENCY`] / 扫描页数约束，
/// 不存在对单个 URL 的逐轮无间隔重试。此上限是条目数预算，不是严格字节预算。
pub const COVER_BACKFILL_ATTEMPT_CACHE_MAX: usize = 1024;
const COVER_FAILURE_TTL: Duration = Duration::from_secs(10 * 60);

enum CachedCover {
    Failed(Instant),
    Image(String),
}

/// 进程内有界URL结果缓存；成功保留可复用图片，失败按TTL恢复机会。
pub struct CoverAttemptCache {
    set: HashMap<String, CachedCover>,
    order: std::collections::VecDeque<String>,
    cap: usize,
}

impl Default for CoverAttemptCache {
    fn default() -> Self {
        Self::new()
    }
}

impl CoverAttemptCache {
    /// 生产默认上限 [`COVER_BACKFILL_ATTEMPT_CACHE_MAX`]。
    pub fn new() -> Self {
        Self::with_capacity(COVER_BACKFILL_ATTEMPT_CACHE_MAX)
    }

    /// 指定上限（测试用小容量验证逐出；cap=0 表示不缓存）。
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            set: HashMap::new(),
            order: std::collections::VecDeque::new(),
            cap,
        }
    }

    pub fn contains(&self, url: &str) -> bool {
        self.contains_at(url, Instant::now())
    }

    fn contains_at(&self, url: &str, now: Instant) -> bool {
        match self.set.get(url) {
            Some(CachedCover::Image(_)) => true,
            Some(CachedCover::Failed(at)) => now.saturating_duration_since(*at) < COVER_FAILURE_TTL,
            None => false,
        }
    }

    fn image(&self, url: &str) -> Option<String> {
        match self.set.get(url) {
            Some(CachedCover::Image(image)) => Some(image.clone()),
            _ => None,
        }
    }

    fn remember(&mut self, url: String, value: CachedCover) {
        let exists = self.set.insert(url.clone(), value).is_some();
        if !exists {
            self.order.push_back(url);
        }
        while self.order.len() > self.cap {
            if let Some(oldest) = self.order.pop_front() {
                self.set.remove(&oldest);
            }
        }
    }

    /// 记入一次失败完成时刻；重复不改变逐出顺序，TTL从最近实际失败计算。
    pub fn insert(&mut self, url: String) {
        self.remember(url, CachedCover::Failed(Instant::now()));
    }

    pub fn len(&self) -> usize {
        self.set.len()
    }

    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }
}

/// 封面补全的进程内状态（**跨轮持久**）。
///
/// - `attempts`：结果缓存；成功图可复用，失败10分钟内跳过（见 [`CoverAttemptCache`]）。
/// - `scan_offset`：候选扫描游标（行偏移）。每轮从上次结束处继续扫描，按
///   **已扫描行数**推进（负缓存命中 / 同 URL 重复 / 批次已满都算扫描过），
///   扫到候选集末尾（读到空页）回绕到 0。
///
/// 游标的存在性理由（R1-P2-2「窗口冻结」）：候选队列里「抓过但拿不到图」的行
/// 不会被移除，负缓存命中只跳过不推进旧实现的每轮固定 [0, 80) 窗口；当窗口内
/// 80 行全部处于「已尝试且仍无封面」状态时，`picked` 恒空 → 不产生新请求 →
/// 负缓存不增长也不逐出 → 更老的候选在本进程内冻结到重启。游标推进 + 回绕
/// 保证候选集在有限轮数内被全覆盖；不落盘（进程内即可）。
pub struct CoverBackfillState {
    attempts: CoverAttemptCache,
    scan_offset: i64,
}

impl Default for CoverBackfillState {
    fn default() -> Self {
        Self::new()
    }
}

impl CoverBackfillState {
    pub fn new() -> Self {
        Self::with_capacity(COVER_BACKFILL_ATTEMPT_CACHE_MAX)
    }

    /// 指定负缓存上限（测试用小容量验证逐出/预置）。
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            attempts: CoverAttemptCache::with_capacity(cap),
            scan_offset: 0,
        }
    }

    pub fn attempts(&self) -> &CoverAttemptCache {
        &self.attempts
    }

    pub fn attempts_mut(&mut self) -> &mut CoverAttemptCache {
        &mut self.attempts
    }

    /// 当前扫描游标（行偏移）。
    pub fn scan_offset(&self) -> i64 {
        self.scan_offset
    }

    /// 同一成功图被前端确认失效后撤掉可复用值，并保留短期失败屏障。
    /// 与数据库精确 CAS 同一命令接线；后来不同图URL的迟到失败不影响新缓存。
    pub fn report_broken(&mut self, page_url: &str, image_url: &str) {
        if self
            .attempts
            .image(page_url)
            .as_deref()
            .map_or(true, |cached| cached == image_url)
        {
            self.attempts.insert(page_url.to_string());
        }
    }
}

/// 可复制的终止信号（应用退出 / 测试注入）：`trigger` 后 `is_triggered` 恒真、
/// `wait` 立即返回。封面补全循环在等待与请求两处消费它——收到信号即停止发起
/// 新请求，已发出的请求被 select 取消；`cover_backfill_round` 会等待全部
/// 子任务返回，不残留后台任务。
#[derive(Clone)]
pub struct ShutdownToken {
    tx: tokio::sync::watch::Sender<bool>,
    /// 保活一个接收端：watch 通道在所有接收端 drop 后进入 closed 状态，
    /// `send` 会失败且值不更新——触发信号就丢了。
    _rx: tokio::sync::watch::Receiver<bool>,
}

impl Default for ShutdownToken {
    fn default() -> Self {
        Self::new()
    }
}

impl ShutdownToken {
    pub fn new() -> Self {
        let (tx, rx) = tokio::sync::watch::channel(false);
        Self { tx, _rx: rx }
    }

    pub fn trigger(&self) {
        // 通道有保活接收端（`_rx`），send 不会因 closed 失败；仍防御性忽略结果。
        let _ = self.tx.send(true);
    }

    pub fn is_triggered(&self) -> bool {
        *self.tx.borrow()
    }

    /// 触发后立即返回；否则等到 `trigger` 被调用（或发送端关闭）。
    pub async fn wait(&self) {
        let mut rx = self.tx.subscribe();
        if *rx.borrow() {
            return;
        }
        let _ = rx.changed().await;
    }
}
/// 单轮扫描候选的页数上限（每页 COVER_BACKFILL_BATCH 条）。
///
/// REQ-106①（封面补全饥饿）：候选按 published_at DESC 排序，页面抓不到
/// og:image 的条目会**留在队列里**（没有封面可写），因此只看第一页会让窗口
/// 停在同一批最新条目上——真实库实测：最新 20 条候选全部落在同一域名
/// （实测 403），更老的可用候选永不被尝试。多扫几页让窗口推进；上限则是
/// 约束单轮开销（一页一条索引查询，最多 4 页 = 80 条候选的扫描成本）。
const COVER_BACKFILL_SCAN_PAGES: usize = 4;

/// 封面后台补全循环：摘要型 RSS（少数派等）不带 media 字段，正文也没有图，
/// 列表卡片无封面。此循环对「无封面 + 有原文 URL 的文章」（直连与各服务端
/// 协议后端同列——正文首图兜底后仍无图的缺口集合）抓文章页 og:image 补封面
/// （复用 extraction::lead_image），低优先级、限并发、单请求超时、失败
/// 结果缓存（同一 URL 失败10分钟内不重试，成功图直接复用，避免反复请求源站）。
///
/// 生命周期（OPT-017D）：`shutdown` 触发后循环立即退出（等待期与轮内两处
/// 都消费信号）；轮内在途请求被取消、全部子任务随 round 返回结束——应用
/// 退出不残留任务。
pub fn spawn_cover_backfill(app: AppHandle, shutdown: ShutdownToken) {
    tauri::async_runtime::spawn(async move {
        // 启动后等 45s 再跑首轮：避开启动期 UI 抢锁 + 让 feed 抓取先落库，
        // 否则首轮查到的是「还没有文章的库」，白跑一轮。等待期可被终止信号打断。
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(45)) => {}
            _ = shutdown.wait() => {
                log::info!("scheduler: 封面补全在启动等待期收到终止信号，退出");
                return;
            }
        }
        let db = app.state::<AppState>().db.clone();
        let http = app.state::<AppState>().http.clone();
        let state = app.state::<AppState>().cover_backfill.clone();
        loop {
            if shutdown.is_triggered() {
                log::info!("scheduler: 封面补全循环退出（收到终止信号）");
                return;
            }
            let filled = cover_backfill_round(&db, &http, &state, &shutdown).await;
            if filled > 0 {
                log::info!("scheduler: 封面补全 {} 篇", filled);
                // 封面变化 → 通知前端重载（列表卡片即时补上封面）
                let _ = app.emit(
                    "feeds-updated",
                    serde_json::json!({ "new_articles": 0, "failed_feeds": 0 }),
                );
            }
            tokio::select! {
                _ = tokio::time::sleep(COVER_BACKFILL_TICK) => {}
                _ = shutdown.wait() => {
                    log::info!("scheduler: 封面补全循环退出（收到终止信号）");
                    return;
                }
            }
        }
    });
}

/// 单轮封面补全（可测试入口）：从跨轮扫描游标处分页扫描候选、跳过未过期失败，
/// 复用成功图或并发抓取最多 COVER_BACKFILL_BATCH 篇，返回本轮补图条目数。
///
/// 窗口推进（REQ-106①）：候选队列里「抓过但没图」的条目不会被移除，若每轮
/// 只看 ORDER BY published_at DESC 的第一页，最新一批全部拿不到图时更老的
/// 候选在本进程内永远轮不到。这里按 (LIMIT, OFFSET) 往后翻页并跳过 tried，
/// 用 COVER_BACKFILL_SCAN_PAGES 约束单轮扫描量。
// Note: 窗口推进与负缓存时序的原决策 — 见 .agents/notes/implemented/bug-fix/2026-09-23-封面补全的候选窗口饥饿.md
/// 游标推进（R1-P2-2）：offset 从 [`CoverBackfillState::scan_offset`] 继续、
/// 每轮按已扫描行数前移、读到空页回绕到 0——负缓存命中的行同样推进，避免
/// 「窗口内全部已尝试」时逐轮重扫同一窗、更老候选冻结到重启。行因补全成功
/// 离开队列会让 offset 语义滑动（可能同轮略过部分行），回绕保证后续轮仍覆盖。
///
/// `shutdown`：终止信号（OPT-017D）。触发后本轮不再发起新请求；已在途的请求
/// 被取消，全部子任务随函数返回结束（不残留）。
pub async fn cover_backfill_round(
    db: &Arc<tokio::sync::Mutex<Connection>>,
    http: &reqwest::Client,
    state: &Arc<tokio::sync::Mutex<CoverBackfillState>>,
    shutdown: &ShutdownToken,
) -> usize {
    // 终止信号已触发（应用退出）：本轮一个新请求都不发。
    if shutdown.is_triggered() {
        log::debug!("scheduler: 封面补全已收到终止信号，本轮跳过");
        return 0;
    }
    // 目标按URL分组，同轮同URL只请求一次；窗外/后新增同URL候选在后续扫描中
    // 复用已缓存的成功图，不为了填全部同URL条目做额外全库扫描。
    let targets: Vec<(String, Vec<i64>)> = {
        let conn = db.lock().await;
        let mut state = state.lock().await;
        let mut picked: Vec<(String, Vec<i64>)> = Vec::new();
        let mut picked_urls: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        // 从上次游标继续；读到空页 = 到达候选集末尾 → 回绕。
        let mut offset = state.scan_offset;
        for _ in 0..COVER_BACKFILL_SCAN_PAGES {
            let page = match crate::db::articles_without_cover(&conn, COVER_BACKFILL_BATCH, offset)
            {
                Ok(page) => page,
                Err(e) => {
                    // 读失败不得压成空集合后静默当作「没有候选」：记录后提前结束
                    // 扫描（已选出的目标照常处理）；未尝试的条目不记负缓存，
                    // 下一轮读恢复后仍会重试。游标停在本轮已扫描处，不跳段。
                    log::warn!(
                        "scheduler: 封面补全候选查询失败（本轮提前结束扫描，未处理条目不算已处理；下轮重试）：{e}"
                    );
                    break;
                }
            };
            if page.is_empty() {
                // 候选集末尾：回绕到 0，让后续轮次从头覆盖（行集合会随补全成功
                // /新文章入库漂移，回绕保证「有限轮数内全覆盖」而非单趟快照）。
                offset = 0;
                break;
            }
            offset += page.len() as i64;
            for (aid, url) in page {
                if state.attempts().contains(&url) && state.attempts().image(&url).is_none() {
                    // 负缓存命中：跳过请求，但该行已计入 offset 推进（下方统一写回）。
                    continue;
                }
                match picked_urls.get(&url) {
                    // 同 URL 的第二篇及其后：只登记条目，不占用批量额度（不额外发请求）
                    Some(index) => picked[*index].1.push(aid),
                    None => {
                        if picked.len() >= COVER_BACKFILL_BATCH as usize {
                            continue;
                        }
                        picked_urls.insert(url.clone(), picked.len());
                        picked.push((url, vec![aid]));
                    }
                }
            }
            if picked.len() >= COVER_BACKFILL_BATCH as usize {
                break;
            }
        }
        // 游标按已扫描行数写回（负缓存命中 / 重复 URL / 批次已满都算扫描过）。
        state.scan_offset = offset;
        picked
    };
    if targets.is_empty() {
        return 0;
    }

    let sem = Arc::new(tokio::sync::Semaphore::new(COVER_BACKFILL_CONCURRENCY));
    let mut handles = Vec::with_capacity(targets.len());
    for (url, aids) in targets {
        let sem = sem.clone();
        let db = db.clone();
        let http = http.clone();
        let state = state.clone();
        let shutdown = shutdown.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire_owned().await;
            backfill_cover_once(&http, &db, &state, &shutdown, &aids, &url).await
        }));
    }
    // 等待全部子任务结束（含被终止信号取消的）——函数返回即不残留后台任务。
    let mut filled = 0usize;
    for h in handles {
        match h.await {
            Ok(CoverAttempt::Settled(n)) | Ok(CoverAttempt::Filled(n)) => filled += n,
            Ok(CoverAttempt::Cancelled) | Ok(CoverAttempt::DbWriteFailed) => {}
            // 子任务 panic（join 失败）不得静默跳过：该 URL 未记入负缓存，
            // 下一轮会重新尝试；记错误便于诊断。
            Err(e) => log::error!(
                "scheduler: 封面补全子任务异常结束（join 失败，本轮该 URL 未处理，下轮重试）：{e}"
            ),
        }
    }
    filled
}

/// 单次尝试的结局（决定是否记入负缓存、是否计入补全数）。
enum CoverAttempt {
    /// 页面无图或抓取失败，记10分钟失败缓存；n保留既有回执形态。
    Settled(usize),
    Filled(usize),
    /// 终止信号中断了这次尝试（未完成，不记负缓存）。
    Cancelled,
    /// 数据库写入失败：不记负缓存（那不是「已处理」），下轮重试；已记错误日志。
    DbWriteFailed,
}

/// 单个 URL 的封面补全：抓文章页 → lead_image 抽 og:image → 为组内每个条目落库。
/// 返回本次尝试的结局。
///
/// 负缓存时序（REQ-106①）：URL 只在**一次尝试结束后**才记入 tried。此前是在
/// 发请求前就记，于是「已尝试」包含了「还没发出请求」的条目——配合不推进的
/// 候选窗口，最新一批全失败后更老的候选在本进程内既不会被尝试也不会被清出
/// tried，形成饥饿。写库失败与终止中断不属于「尝试结束」，不记。
async fn backfill_cover_once(
    http: &reqwest::Client,
    db: &Arc<tokio::sync::Mutex<Connection>>,
    state: &Arc<tokio::sync::Mutex<CoverBackfillState>>,
    shutdown: &ShutdownToken,
    aids: &[i64],
    url: &str,
) -> CoverAttempt {
    // 终止信号：不再发起新请求。
    if shutdown.is_triggered() {
        return CoverAttempt::Cancelled;
    }
    // 成功可直接填扫描到的候选；失败TTL内直接返回，不续期失败时钟。
    {
        let cache = state.lock().await;
        if let Some(image) = cache.attempts().image(url) {
            drop(cache);
            return fill_cover(db, state, aids, url, &image).await;
        }
        if cache.attempts().contains(url) {
            return CoverAttempt::Settled(0);
        }
    }
    let outcome = attempt_cover(http, db, state, shutdown, aids, url).await;
    // 实际完成失败后才写入失败时刻，成功图在写库同锁内记入。
    if matches!(outcome, CoverAttempt::Settled(_)) {
        state.lock().await.attempts_mut().insert(url.to_string());
    }
    outcome
}

/// 单次抓取与落库（不含负缓存读写）：返回本次尝试的结局。
async fn attempt_cover(
    http: &reqwest::Client,
    db: &Arc<tokio::sync::Mutex<Connection>>,
    state: &Arc<tokio::sync::Mutex<CoverBackfillState>>,
    shutdown: &ShutdownToken,
    aids: &[i64],
    url: &str,
) -> CoverAttempt {
    // 拉文章页（单请求超时；只取 og:image，不必等整页正文）。
    // 请求与读 body 两处都 select 终止信号：退出时不留下未完成的请求。
    let request = http.get(url).timeout(COVER_BACKFILL_REQUEST_TIMEOUT);
    let resp = tokio::select! {
        biased;
        _ = shutdown.wait() => return CoverAttempt::Cancelled,
        r = request.send() => r,
    };
    let resp = match resp {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            log::debug!("scheduler: 封面补全抓取失败 {url}: HTTP {}", r.status());
            return CoverAttempt::Settled(0);
        }
        Err(e) => {
            log::debug!("scheduler: 封面补全抓取失败 {url}: {e}");
            return CoverAttempt::Settled(0);
        }
    };
    let base = resp.url().to_string();
    let html = tokio::select! {
        biased;
        _ = shutdown.wait() => return CoverAttempt::Cancelled,
        h = crate::ingestion::read_text_capped(resp, crate::ingestion::MAX_BODY_BYTES) => h,
    };
    let html = match html {
        Ok(h) => h,
        Err(e) => {
            log::debug!("scheduler: 封面补全读取页面失败 {url}: {e}");
            return CoverAttempt::Settled(0);
        }
    };
    // lead_image 是纯同步（scraper）返回 Option<String>，spawn_blocking 里跑避免阻塞 async worker
    let image = match tokio::task::spawn_blocking(move || {
        crate::extraction::lead_image(&html, &base)
    })
    .await
    {
        Ok(Some(img)) => img,
        Ok(None) => {
            log::debug!("scheduler: 封面补全页面无 og:image {url}");
            return CoverAttempt::Settled(0);
        }
        Err(e) => {
            log::debug!("scheduler: 封面补全解析任务失败 {url}: {e}");
            return CoverAttempt::Settled(0);
        }
    };
    // 落库（幂等：已有封面不覆盖）。NULLIF 把空串也视为「没有封面」——候选查询
    // （image_url IS NULL OR image_url = ''）把空串计入队列，而 COALESCE 只判 NULL，
    // 空串行会一直留在队列里且每轮都被算作「已补全」。
    fill_cover(db, state, aids, url, &image).await
}

async fn fill_cover(
    db: &Arc<tokio::sync::Mutex<Connection>>,
    state: &Arc<tokio::sync::Mutex<CoverBackfillState>>,
    aids: &[i64],
    url: &str,
    image: &str,
) -> CoverAttempt {
    let conn = db.lock().await;
    let mut cache = state.lock().await;
    if cache.attempts().contains(url) && cache.attempts().image(url).is_none() {
        // 请求在途期间失败上报已清掉该图；不能把刚失败的结果重新填回。
        return CoverAttempt::Cancelled;
    }
    let mut filled = 0;
    for aid in aids {
        match conn.execute(
            "UPDATE articles SET image_url = ?1 WHERE id = ?2 AND url = ?3 AND (image_url IS NULL OR image_url = '')",
            rusqlite::params![image, aid, url],
        ) {
            Ok(n) => filled += n,
            Err(e) => {
                // 写失败不得压成 0 后把 URL 当作已处理：不记负缓存，下轮重试。
                log::error!("scheduler: 封面补全写库失败（不记已尝试，下轮重试）{url}: {e}");
                return CoverAttempt::DbWriteFailed;
            }
        }
    }
    cache
        .attempts_mut()
        .remember(url.to_string(), CachedCover::Image(image.to_string()));
    CoverAttempt::Filled(filled)
}

/* ============================================================
OPT-011：refreshInterval 读取口径单元测试（5–720 整数；与前端同区间）
============================================================ */
#[cfg(test)]
mod refresh_interval_tests {
    use super::{
        backend_sync_due, focus_sync_due, read_refresh_interval, FOCUS_SYNC_THROTTLE_SECS,
    };
    use serde_json::json;

    /// (opt011-int) 区间边界与整数口径：5 / 720 接受；4 / 721 拒绝；
    /// JSON 整数（30）与整数值小数（300.0）等价接受；非整数小数（30.5）
    /// 拒绝（不悄悄回默认——调用方以 None 走默认，与前端 Number.isInteger
    /// 的拒绝口径一致）；缺失/非数值拒绝。
    #[test]
    fn refresh_interval_accepts_only_integers_within_range() {
        assert_eq!(
            read_refresh_interval(&json!({"refreshInterval": 5})),
            Some(5)
        );
        assert_eq!(
            read_refresh_interval(&json!({"refreshInterval": 300})),
            Some(300)
        );
        assert_eq!(
            read_refresh_interval(&json!({"refreshInterval": 720})),
            Some(720)
        );
        assert_eq!(
            read_refresh_interval(&json!({"refreshInterval": 300.0})),
            Some(300),
            "整数值小数与 int 等价（JSON 里 300.0 解析为 f64）"
        );
        assert_eq!(
            read_refresh_interval(&json!({"refreshInterval": 30.5})),
            None,
            "非整数小数必须拒绝（修前 as_i64() 静默 None → 回默认，与 UI 值分叉）"
        );
        assert_eq!(read_refresh_interval(&json!({"refreshInterval": 4})), None);
        assert_eq!(
            read_refresh_interval(&json!({"refreshInterval": 721})),
            None
        );
        assert_eq!(
            read_refresh_interval(&json!({"refreshInterval": "30"})),
            None
        );
        assert_eq!(read_refresh_interval(&json!({})), None);
    }

    #[test]
    fn zero_binding_gate_uses_discovery_throttle_not_last_sync_zero() {
        let now = 1_700_000_000;
        assert!(
            !backend_sync_due(now, 30, false, 0, false),
            "零绑定时 last_sync=0 不得让 scheduler 每个 TICK 启动"
        );
        assert!(
            backend_sync_due(now, 30, false, 0, true),
            "独立发现节流到期后仍应允许自动发现"
        );
        assert!(
            !backend_sync_due(now, 30, true, now - 60, false),
            "有绑定时继续按条目游标间隔节流"
        );
        assert!(backend_sync_due(now, 30, true, now - 30 * 60, false));
    }

    /// W3-3：聚焦节流判据（纯函数锁定）——距上次触发 <60s 跳过，≥60s 允许；
    /// last=0（从未触发）不受负窗口影响恒允许。判别力：把比较方向写反
    /// （窗口内反而触发）或用 `>` 边界（恰好 60s 被跳过）的实现会红。
    #[test]
    fn focus_sync_due_throttles_within_window() {
        let last = 1_700_000_000;
        assert!(!focus_sync_due(last + 59, last), "窗口内必须跳过");
        assert!(focus_sync_due(last + FOCUS_SYNC_THROTTLE_SECS, last));
        assert!(focus_sync_due(last + FOCUS_SYNC_THROTTLE_SECS + 1, last));
        assert!(
            focus_sync_due(last, 0),
            "从未触发（last=0）不得被负窗口卡住"
        );
        assert_eq!(FOCUS_SYNC_THROTTLE_SECS, 60, "节流常量变更需同步注释");
    }
}

#[cfg(test)]
mod backend_plan_tests {
    use super::*;

    fn configured_db() -> Arc<tokio::sync::Mutex<Connection>> {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::MIGRATIONS.to_latest(&mut conn).unwrap();
        for (key, value) in [
            ("greader_endpoint", "http://127.0.0.1:9"),
            ("greader_username", "runtime-test"),
            ("greader_password", "test-token"),
        ] {
            crate::db::set_setting(&conn, key, value).unwrap();
        }
        Arc::new(tokio::sync::Mutex::new(conn))
    }

    #[tokio::test]
    async fn disabled_or_unconfigured_background_sync_remains_a_quiet_skip() {
        let db = configured_db();
        {
            let conn = db.lock().await;
            crate::db::set_setting(&conn, "app_settings", r#"{"autoSync":false}"#).unwrap();
            conn.execute(
                "UPDATE settings SET value='dpapi:@@corrupt@@' WHERE key='greader_password'",
                [],
            )
            .unwrap();
        }
        assert!(backend_sync_plan(&db, false).await.unwrap().is_none());
        {
            let conn = db.lock().await;
            crate::db::set_setting(&conn, "app_settings", r#"{"autoSync":true}"#).unwrap();
            conn.execute("DELETE FROM settings WHERE key='greader_endpoint'", [])
                .unwrap();
        }
        assert!(backend_sync_plan(&db, true).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn background_plan_read_failures_publish_error_instead_of_offline_or_empty_success() {
        for (sql, expected) in [
            (
                "INSERT INTO settings(key,value) VALUES('app_settings','{broken')",
                "json",
            ),
            (
                "UPDATE settings SET value='dpapi:@@corrupt@@' WHERE key='greader_password'",
                "credentialCorrupt",
            ),
            (
                "INSERT INTO settings(key,value) VALUES('sync_last_sync',X'00')",
                "db",
            ),
            ("ALTER TABLE feeds RENAME TO missing_feeds", "db"),
            ("DROP TABLE subscription_outbox", "db"),
        ] {
            for source in ["auto", "focus"] {
                let db = configured_db();
                db.lock().await.execute_batch(sql).unwrap();
                let events = Arc::new(StdMutex::new(Vec::new()));
                let captured = events.clone();
                let runtime = crate::sync::runtime::SyncRuntime::new(move |event| {
                    captured.lock().unwrap().push(event);
                });
                let error = runtime
                    .run(source, async move {
                        backend_sync_plan(&db, source == "focus").await?;
                        Ok(())
                    })
                    .await
                    .unwrap_err();
                assert_eq!(error.code, expected, "{source}: {sql}");
                let events = events.lock().unwrap();
                assert_eq!(events.len(), 2);
                assert_eq!(events[1].outcome.as_deref(), Some("error"));
            }
        }
    }
}

#[cfg(test)]
mod refresh_worker_tests {
    use super::{absorb_refresh_worker, refresh_all, RefreshBatchReport};
    use crate::ingestion::FeedRefreshOutcome;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn panicked_feed_worker_is_counted_as_failed() {
        let handle = tokio::spawn(async {
            panic!("injected refresh worker panic");
        });
        let mut report = RefreshBatchReport::default();
        absorb_refresh_worker(handle, &mut report).await;
        assert_eq!(report.failed_feeds, 1);
        assert_eq!(report.new_articles, 0);
        assert!(
            report.refreshed_feed_urls.is_empty(),
            "panic 的源不得进入成功 footprint"
        );
    }

    #[tokio::test]
    async fn only_applied_outcome_enters_footprint() {
        for outcome in [
            FeedRefreshOutcome::SkippedInFlight,
            FeedRefreshOutcome::Superseded,
        ] {
            let handle = tokio::spawn(async move { Ok::<_, crate::error::AppError>((1, outcome)) });
            let mut report = RefreshBatchReport::default();
            absorb_refresh_worker(handle, &mut report).await;
            assert_eq!(report.new_articles, 0);
            assert_eq!(report.failed_feeds, 0);
            assert!(
                report.refreshed_feed_urls.is_empty(),
                "只有 Applied 才可进入 footprint"
            );
        }

        let raw_url = "https://Example.com/feed?utm_source=skip#fragment";
        let handle = tokio::spawn(async move {
            Ok::<_, crate::error::AppError>((
                7,
                FeedRefreshOutcome::Applied {
                    new_articles: 2,
                    feed_url: raw_url.into(),
                },
            ))
        });
        let mut report = RefreshBatchReport::default();
        absorb_refresh_worker(handle, &mut report).await;
        assert_eq!(report.new_articles, 2);
        assert_eq!(report.failed_feeds, 0);
        assert_eq!(
            report.refreshed_feed_urls,
            vec![crate::db::normalize_url(raw_url)]
        );
    }

    #[tokio::test]
    async fn panicked_full_refresh_publishes_error_and_clears_flight() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::MIGRATIONS.to_latest(&mut conn).unwrap();
        let db = Arc::new(tokio::sync::Mutex::new(conn));
        let client = crate::ingestion::build_client(1);

        super::PANIC_NEXT_FULL_REFRESH.store(true, Ordering::SeqCst);
        let error = refresh_all(&db, &client)
            .await
            .expect_err("后台 flight panic 必须通过 watch 返回错误");
        assert_eq!(error.code, "internal");
        assert!(
            error.message.contains("panic"),
            "错误必须保留 panic 清理语义: {error}"
        );

        let second = tokio::time::timeout(Duration::from_secs(2), refresh_all(&db, &client))
            .await
            .expect("flight 必须已清理，后续调用不得等待旧结果")
            .expect("旧 flight 清理后，新一轮空批次应正常完成");
        assert_eq!(second, (0, 0));
    }
}

/* ============================================================
OPT-017D：封面补全负缓存（上限与逐出）单元测试
============================================================ */
#[cfg(test)]
mod cover_attempt_cache_tests {
    use super::{
        CachedCover, CoverAttemptCache, CoverBackfillState, COVER_BACKFILL_ATTEMPT_CACHE_MAX,
        COVER_FAILURE_TTL,
    };

    #[test]
    fn negative_cache_expires_and_success_report_does_not_poison_new_image() {
        let now = std::time::Instant::now();
        let mut cache = CoverAttemptCache::with_capacity(2);
        cache.remember("failed".into(), CachedCover::Failed(now));
        assert!(cache.contains_at(
            "failed",
            now + COVER_FAILURE_TTL - std::time::Duration::from_millis(1)
        ));
        assert!(!cache.contains_at("failed", now + COVER_FAILURE_TTL));
        cache.remember("good".into(), CachedCover::Image("new.jpg".into()));
        assert!(cache.contains_at("good", now + COVER_FAILURE_TTL * 100));
        assert_eq!(cache.image("good").as_deref(), Some("new.jpg"));
        let mut state = CoverBackfillState::new();
        state
            .attempts
            .remember("page".into(), CachedCover::Image("new.jpg".into()));
        state.report_broken("page", "old.jpg");
        assert_eq!(state.attempts.image("page").as_deref(), Some("new.jpg"));
        state.report_broken("page", "new.jpg");
        assert!(state.attempts.image("page").is_none());
        assert!(state.attempts.contains("page"));
    }

    #[tokio::test]
    async fn expired_failure_retries_real_http_and_fills_after_recovery() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/article", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_write_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut bytes = [0; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let n = stream.read(&mut bytes).unwrap();
                if n == 0 {
                    return;
                }
                request.extend_from_slice(&bytes[..n]);
            }
            let body = "<meta property=\"og:image\" content=\"https://example.com/recovered.jpg\">";
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let mut connection = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::MIGRATIONS.to_latest(&mut connection).unwrap();
        connection.execute_batch("INSERT INTO folders(id,name) VALUES(1,'F'); INSERT INTO feeds(id,folder_id,feed_url,title) VALUES(1,1,'https://example.com/rss','F');").unwrap();
        connection
            .execute(
                "INSERT INTO articles(id,feed_id,guid,title,url) VALUES(1,1,'g','T',?1)",
                [&url],
            )
            .unwrap();
        let db = std::sync::Arc::new(tokio::sync::Mutex::new(connection));
        let state = std::sync::Arc::new(tokio::sync::Mutex::new(CoverBackfillState::new()));
        state.lock().await.attempts_mut().remember(
            url.clone(),
            CachedCover::Failed(std::time::Instant::now() - COVER_FAILURE_TTL),
        );
        let http = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap();
        assert_eq!(
            super::cover_backfill_round(&db, &http, &state, &super::ShutdownToken::new()).await,
            1
        );
        server.join().unwrap();
        let image: String = db
            .lock()
            .await
            .query_row("SELECT image_url FROM articles WHERE id=1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(image, "https://example.com/recovered.jpg");
    }

    /// 上限之外必须逐出最旧（插入序）；重复记入不得改变顺序或触发逐出；
    /// 默认构造必须用到生产上限常量而不是 0（防「容量为 0 悄悄禁掉缓存」）。
    #[test]
    fn attempt_cache_evicts_oldest_beyond_capacity() {
        let mut cache = CoverAttemptCache::with_capacity(2);
        cache.insert("a".into());
        cache.insert("b".into());
        cache.insert("c".into());
        assert_eq!(cache.len(), 2, "容量 2 的缓存不得超过上限");
        assert!(!cache.contains("a"), "最旧的 a 必须被逐出");
        assert!(cache.contains("b") && cache.contains("c"));

        cache.insert("b".into());
        assert_eq!(cache.len(), 2, "重复记入不得增长（也不得逐出他人）");
        assert!(cache.contains("b") && cache.contains("c"));

        assert_eq!(
            COVER_BACKFILL_ATTEMPT_CACHE_MAX, 1024,
            "生产上限值变更需同步文档与测试"
        );
        let mut default_cache = CoverAttemptCache::new();
        default_cache.insert("x".into());
        assert!(
            default_cache.contains("x"),
            "默认构造必须真的缓存（上限不得为 0）"
        );
    }
}
