//! 后台刷新调度器：定时醒来 → 查"到期"源 → 限并发抓取 → 事件通知前端。
//!
//! 设置实时读取（app_settings JSON 的 autoRefresh/refreshInterval/
//! fetchConcurrency/smartDedup），改设置无需重启即生效（下一个 tick
//! 最多 60s 后跟上）。抓取走 ingestion::refresh_feed_staged 三段式
//! 管线：HTTP 在锁外执行，写库时短暂持锁——并发真正并行。

use crate::state::AppState;
use rusqlite::Connection;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// 调度循环的醒来节奏。每 tick 只跑一条便宜的索引查询，没到期的源直接返回。
const TICK: Duration = Duration::from_secs(60);

/// 并发抓取上限默认值：4（个人规模订阅数下兼顾速度与源站压力）。
/// 用户可在设置页 1–16 调整（fetchConcurrency）。
const DEFAULT_CONCURRENCY: usize = 4;
pub const MAX_CONCURRENCY: usize = 16;

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
/// - `hybrid` 跟随服务端：后台刷新跳过 Miniflux 源（内容走服务端同步）
/// - `direct` 本机抓取（默认/未配置）：全部源直连抓取（旧行为）
///
/// 锁内读（调用方持 conn）。
fn read_sync_mode_conn(conn: &rusqlite::Connection) -> String {
    /* TASK-068：收口为类型化助手（默认值语义不变） */
    crate::db::app_settings_str(conn, "syncMode", "direct")
}

/// 全量刷新所有源（托盘「刷新全部订阅」与手动全刷入口，忽略到期时间）。
/// 手动动作始终包含 Miniflux 源——用户显式点了刷新就是要全部内容
/// （同步模式只影响后台定时行为，不拦用户显式动作）。
pub async fn refresh_all(
    db: &Arc<tokio::sync::Mutex<rusqlite::Connection>>,
    http: &reqwest::Client,
) -> (usize, usize) {
    // TASK-064 N3：smartDedup 照常生效——此前写死 false，开智能去重的用户手动
    // 全刷会放行跨源同文（文章翻倍且不可逆）。手动语义只解除「到期时间与同步
    // 模式对 remote 源的跳过」，不解除去重。
    let (_, _, dedup, concurrency) = read_refresh_config(db).await;
    refresh_feeds_inner_with_concurrency(db, http, None, dedup, concurrency).await
}
/// 抓取所有到期源（后台调度入口，并发上限 = 设置 fetchConcurrency，默认 4）。
/// HTTP 在锁外执行（refresh_feed_staged），写库时短暂持锁。
/// 返回 (新增条数, 失败源数)。
/// 同步模式作用点：hybrid（跟随服务端）→ 到期查询跳过 origin='remote' 的源
/// （服务端源内容由后端同步提供）；direct → 全部源照常直连（旧行为）。
async fn refresh_due_feeds(
    db: &Arc<tokio::sync::Mutex<rusqlite::Connection>>,
    http: &reqwest::Client,
) -> (usize, usize) {
    let (_, interval_min, dedup, concurrency) = read_refresh_config(db).await;
    refresh_feeds_inner_with_concurrency(db, http, Some(interval_min), dedup, concurrency).await
}

/// 抓取实现：`Some(interval_min)` 只抓到期源（模式过滤在查询内做），
/// `None` 全量（手动语义，始终含 Miniflux 源）；`dedup` 透传给入库管线
/// （TASK-064 N3：两条路径都读设置，不再对手动全量写死 false）。
/// 并发上限取设置值（全量入口同样尊重 fetchConcurrency）。
async fn refresh_feeds_inner_with_concurrency(
    db: &Arc<tokio::sync::Mutex<rusqlite::Connection>>,
    http: &reqwest::Client,
    due_filter: Option<i64>,
    dedup: bool,
    concurrency: usize,
) -> (usize, usize) {
    use tokio::sync::Semaphore;
    let due: Vec<i64> = {
        let conn = db.lock().await;
        match due_filter {
            // 调度路径：模式判定在锁内一次完成（读 settings + 查询同临界区）
            Some(interval_min) => {
                let include_remote = read_sync_mode_conn(&conn) != "hybrid";
                crate::db::feeds_due_for_refresh(&conn, interval_min, include_remote)
            }
            // 手动全量：hybrid（跟随服务端）模式下跳过 origin='remote' 源——
            // 服务端源的内容由后端同步提供，直连抓取会产生 source='direct'
            // 文章与已有的 source='miniflux' 文章重复（guid 不同 + 智能去重默认关），
            // 导致文章翻倍、状态错乱、未读数对不齐。direct 模式则全部直连（旧行为）。
            None => {
                let include_remote = read_sync_mode_conn(&conn) != "hybrid";
                crate::db::feeds_all_ids(&conn, include_remote)
            }
        }
        .unwrap_or_else(|e| {
            log::warn!("scheduler: query feeds failed: {e}");
            Vec::new()
        })
    };
    if due.is_empty() {
        return (0, 0);
    }
    log::info!(
        "scheduler: {} feed(s) due, concurrency={concurrency}",
        due.len()
    );

    let sem = Arc::new(Semaphore::new(concurrency));
    let mut handles = Vec::with_capacity(due.len());
    for id in due {
        let sem = sem.clone();
        let db = db.clone();
        let http = http.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire_owned().await;
            match crate::ingestion::refresh_feed_staged(&db, &http, id, dedup).await {
                Ok(n) => (n, 0),
                Err(_) => (0, 1),
            }
        }));
    }
    let mut new_articles = 0;
    let mut failed = 0;
    for h in handles {
        if let Ok((n, f)) = h.await {
            new_articles += n;
            failed += f;
        }
    }
    (new_articles, failed)
}

/// 启动后台调度循环。启动后等 8s 再跑第一轮（避开启动期的 UI 抢锁）。
pub fn spawn_scheduler(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(8)).await;
        // 只 clone Send 的部件（Arc<Mutex> + Client），不跨 await 持有 tauri State
        let db = app.state::<AppState>().db.clone();
        let http = app.state::<AppState>().http.clone();
        loop {
            let (enabled, _interval, _dedup, _concurrency) = read_refresh_config(&db).await;
            if enabled {
                let (new_articles, failed) = refresh_due_feeds(&db, &http).await;
                if new_articles > 0 || failed > 0 {
                    let _ = app.emit(
                        "feeds-updated",
                        serde_json::json!({ "new_articles": new_articles, "failed_feeds": failed }),
                    );
                    // 新文章系统通知（notifyOnNewArticles 开关，默认关；
                    // 窗口隐藏/失焦时才发——正在看应用时不打扰）
                    if new_articles > 0 && should_notify(&db, &app).await {
                        notify_new_articles(&app, new_articles);
                    }
                }
            }
            // 后端后台自动同步（autoSync 开关，默认开）：
            // 到期才跑轻量同步（push 队列 + changed_after 增量 pull）
            auto_sync_backend(&db, &http, &app).await;
            tokio::time::sleep(TICK).await;
        }
    });
}

/// 后端自动同步：读 autoSync（默认开）与刷新间隔，
/// 到期（now - last_sync ≥ refreshInterval 分钟）时跑轻量同步。
/// 失败静默（log 记录），下个 tick 仍会因 last_sync 未推进而重试。
/// 状态被拉平后发 feeds-updated——前端列表/未读计数与 DB 不再脱节
/// （pull 改变了 is_read 但用户无感知的"静默漂移"问题）。
async fn auto_sync_backend(
    db: &Arc<tokio::sync::Mutex<Connection>>,
    http: &reqwest::Client,
    app: &AppHandle,
) {
    let (on, interval_min, connected, last_sync) = {
        let conn = db.lock().await;
        /* TASK-068/069：autoSync 布尔读取收口为类型化助手（默认值语义不变） */
        let on = crate::db::app_settings_bool(&conn, "autoSync", true);
        if !on {
            return;
        }
        // refreshInterval 是数值型（带区间夹取），N-硬2 明确推迟到后续任务；
        // 仅在 autoSync 开启时才需要解析，故放在早退之后（避免无谓的二次读取）。
        // OPT-011：解析收口到 read_refresh_interval（5–720 整数，与刷新调度同一口径）。
        let interval = crate::db::get_setting(&conn, "app_settings")
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .as_ref()
            .and_then(read_refresh_interval)
            .unwrap_or(30);
        // OPT-014 R1：凭据读取/解密失败（密文损坏）不等于「未配置」——
        // 记警告并跳过本轮自动同步（不发任何请求；可见失败由推送路径的
        // last_error 呈现），不得静默当作离线。
        let connected = match crate::sync::read_credentials(&conn) {
            Ok(Some(_)) => true,
            Ok(None) => false,
            Err(e) => {
                log::warn!("scheduler: 凭据读取失败，跳过本轮后端自动同步: {e}");
                false
            }
        };
        let last = crate::db::last_sync_ts(&conn).unwrap_or(0);
        (on, interval, connected, last)
    };
    if !on || !connected {
        return;
    }
    let now = chrono::Utc::now().timestamp();
    if now - last_sync < interval_min * 60 {
        return;
    }
    log::info!("scheduler: 后端自动同步开始（间隔 {interval_min} 分钟到期）");
    let _ = app.emit("sync-running", serde_json::json!({ "source": "auto" }));
    match crate::sync::sync_light(db, http).await {
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
            if r.has_data_change() {
                let _ = app.emit(
                    "feeds-updated",
                    serde_json::json!({
                        "new_articles": 0,
                        "failed_feeds": 0,
                        "pulled_entries": r.pulled_entries,
                        "merged_states": r.merged_states
                    }),
                );
            }
        }
        Err(e) => log::warn!("scheduler: 后端自动同步失败: {e}"),
    }
    let _ = app.emit("sync-idle", ());
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
/// 进程内「已尝试 URL」负缓存上限（OPT-017D）。
///
/// 语义：一次尝试**完整结束后**（成功 / 无 og:image / 抓取失败）URL 记入负缓存，
/// 本进程内不再重试；DB 写失败与终止信号中断不算「已尝试」。
/// 缓存有上限：写满后按插入序逐出最旧的 URL——逐出只把「本进程永不重试」放宽为
/// 「至少要再排 CAP 条其它尝试之后才可能被重试」，每轮请求量仍由
/// [`COVER_BACKFILL_BATCH`] / [`COVER_BACKFILL_CONCURRENCY`] / 扫描页数约束，
/// 不存在对单个 URL 的无界重试。上限本身约束内存（约 100B/条 × 1024 ≈ 100KB）。
pub const COVER_BACKFILL_ATTEMPT_CACHE_MAX: usize = 1024;

/// 进程内「已尝试 URL」负缓存（带容量上限，逐出最旧）。
pub struct CoverAttemptCache {
    set: std::collections::HashSet<String>,
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
            set: std::collections::HashSet::new(),
            order: std::collections::VecDeque::new(),
            cap,
        }
    }

    pub fn contains(&self, url: &str) -> bool {
        self.set.contains(url)
    }

    /// 记入一次「已尝试」。重复记入不改变顺序；超出上限时逐出最旧。
    pub fn insert(&mut self, url: String) {
        if !self.set.insert(url.clone()) {
            return;
        }
        self.order.push_back(url);
        while self.order.len() > self.cap {
            if let Some(oldest) = self.order.pop_front() {
                self.set.remove(&oldest);
            }
        }
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
/// - `attempts`：负缓存，一次尝试完整结束后记入（见 [`CoverAttemptCache`]）。
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
/// 负缓存（同一 URL 本进程不重试，避免反复轰炸源站）。
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
        let state = Arc::new(tokio::sync::Mutex::new(CoverBackfillState::new()));
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

/// 单轮封面补全（可测试入口）：从**持久扫描游标**处分页扫描候选、跳过本进程
/// 已尝试过的 URL，并发抓取最多 COVER_BACKFILL_BATCH 篇，返回本轮补到封面的篇数。
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
    // 目标按 URL 分组：同一 URL 一轮只请求一次，但**所有**指向该 URL 的条目都要
    // 写入封面（不同源可能收录同一篇原文；只填第一篇会让同 URL 的其它条目永久空缺，
    // 且该 URL 已进负缓存、本进程内不会再轮到它们）。
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
                if state.attempts().contains(&url) {
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
            Ok(CoverAttempt::Settled(n)) => filled += n,
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
    /// 尝试完整结束（成功写库 n 条 / 页面无 og:image / 抓取失败）——
    /// 结果对本进程是确定的，记入负缓存，本进程内不再重复尝试。
    Settled(usize),
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
    // 负缓存：本进程已尝试过（失败/无图）的 URL 不再重试。
    // 这里返回 Settled(0) 会重复记入同一 URL——幂等且不改变插入序。
    if state.lock().await.attempts().contains(url) {
        return CoverAttempt::Settled(0);
    }
    let outcome = attempt_cover(http, db, shutdown, aids, url).await;
    // 成功 / 无图 / 抓取失败都算「本进程已尝试」——但必须发生在尝试**之后**。
    if matches!(outcome, CoverAttempt::Settled(_)) {
        state.lock().await.attempts_mut().insert(url.to_string());
    }
    outcome
}

/// 单次抓取与落库（不含负缓存读写）：返回本次尝试的结局。
async fn attempt_cover(
    http: &reqwest::Client,
    db: &Arc<tokio::sync::Mutex<Connection>>,
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
    let html = tokio::select! {
        biased;
        _ = shutdown.wait() => return CoverAttempt::Cancelled,
        h = resp.text() => h,
    };
    let html = match html {
        Ok(h) => h,
        Err(e) => {
            log::debug!("scheduler: 封面补全读取页面失败 {url}: {e}");
            return CoverAttempt::Settled(0);
        }
    };
    // lead_image 是纯同步（scraper）返回 Option<String>，spawn_blocking 里跑避免阻塞 async worker
    let base = url.to_string();
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
    let conn = db.lock().await;
    let mut filled = 0;
    for aid in aids {
        match conn.execute(
            "UPDATE articles SET image_url = COALESCE(NULLIF(image_url, ''), ?1) WHERE id = ?2",
            rusqlite::params![image, aid],
        ) {
            Ok(n) => filled += n,
            Err(e) => {
                // 写失败不得压成 0 后把 URL 当作已处理：不记负缓存，下轮重试。
                log::error!("scheduler: 封面补全写库失败（不记已尝试，下轮重试）{url}: {e}");
                return CoverAttempt::DbWriteFailed;
            }
        }
    }
    CoverAttempt::Settled(filled)
}

/* ============================================================
OPT-011：refreshInterval 读取口径单元测试（5–720 整数；与前端同区间）
============================================================ */
#[cfg(test)]
mod refresh_interval_tests {
    use super::read_refresh_interval;
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
}

/* ============================================================
OPT-017D：封面补全负缓存（上限与逐出）单元测试
============================================================ */
#[cfg(test)]
mod cover_attempt_cache_tests {
    use super::{CoverAttemptCache, COVER_BACKFILL_ATTEMPT_CACHE_MAX};

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
