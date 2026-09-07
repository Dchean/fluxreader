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
async fn read_refresh_config(db: &Arc<tokio::sync::Mutex<rusqlite::Connection>>) -> (bool, i64, bool, usize) {
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
        if let Some(v) = json.get("refreshInterval").and_then(|v| v.as_i64()) {
            if (5..=720).contains(&v) {
                interval = v;
            }
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

/// 同步模式（app_settings.syncMode，UI 词条「本机抓取 / 跟随服务端」）：
/// - `hybrid` 跟随服务端：后台刷新跳过 Miniflux 源（内容走服务端同步）
/// - `direct` 本机抓取（默认/未配置）：全部源直连抓取（旧行为）
///
/// 锁内读（调用方持 conn）。
fn read_sync_mode_conn(conn: &rusqlite::Connection) -> String {
    crate::db::get_setting(conn, "app_settings")
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("syncMode").and_then(|m| m.as_str()).map(String::from))
        .unwrap_or_else(|| "direct".into())
}

/// 全量刷新所有源（托盘「刷新全部订阅」与手动全刷入口，忽略到期时间）。
/// 手动动作始终包含 Miniflux 源——用户显式点了刷新就是要全部内容
/// （同步模式只影响后台定时行为，不拦用户显式动作）。
pub async fn refresh_all(
    db: &Arc<tokio::sync::Mutex<rusqlite::Connection>>,
    http: &reqwest::Client,
) -> (usize, usize) {
    let concurrency = read_refresh_config(db).await.3;
    refresh_feeds_inner_with_concurrency(db, http, None, concurrency).await
}
/// 抓取所有到期源（后台调度入口，并发上限 = 设置 fetchConcurrency，默认 4）。
/// HTTP 在锁外执行（refresh_feed_staged），写库时短暂持锁。
/// 返回 (新增条数, 失败源数)。
/// 同步模式作用点：hybrid（跟随服务端）→ 到期查询跳过 origin='miniflux' 的源
/// （服务端源内容由 Miniflux 同步提供）；direct → 全部源照常直连（旧行为）。
async fn refresh_due_feeds(
    db: &Arc<tokio::sync::Mutex<rusqlite::Connection>>,
    http: &reqwest::Client,
) -> (usize, usize) {
    let (_, interval_min, dedup, concurrency) = read_refresh_config(db).await;
    refresh_feeds_inner_with_concurrency(db, http, Some((interval_min, dedup)), concurrency).await
}

/// 抓取实现：`Some((interval, dedup))` 只抓到期源（模式过滤在查询内做），
/// `None` 全量（手动语义，始终含 Miniflux 源）。
/// 并发上限取设置值（全量入口同样尊重 fetchConcurrency）。
async fn refresh_feeds_inner_with_concurrency(
    db: &Arc<tokio::sync::Mutex<rusqlite::Connection>>,
    http: &reqwest::Client,
    due_filter: Option<(i64, bool)>,
    concurrency: usize,
) -> (usize, usize) {
    use tokio::sync::Semaphore;
    let dedup = due_filter.map(|(_, d)| d).unwrap_or(false);
    let due: Vec<i64> = {
        let conn = db.lock().await;
        match due_filter {
            // 调度路径：模式判定在锁内一次完成（读 settings + 查询同临界区）
            Some((interval_min, _)) => {
                let include_miniflux = read_sync_mode_conn(&conn) != "hybrid";
                crate::db::feeds_due_for_refresh(&conn, interval_min, include_miniflux)
            }
            // 手动全量：hybrid（跟随服务端）模式下跳过 origin='miniflux' 源——
            // 服务端源的内容由 Miniflux 同步提供，直连抓取会产生 source='direct'
            // 文章与已有的 source='miniflux' 文章重复（guid 不同 + 智能去重默认关），
            // 导致文章翻倍、状态错乱、未读数对不齐。direct 模式则全部直连（旧行为）。
            None => {
                let include_miniflux = read_sync_mode_conn(&conn) != "hybrid";
                crate::db::feeds_all_ids(&conn, include_miniflux)
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
    log::info!("scheduler: {} feed(s) due, concurrency={concurrency}", due.len());

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
            // Miniflux 后台自动同步（autoSyncMiniflux 开关，默认开）：
            // 到期才跑轻量同步（push 队列 + changed_after 增量 pull）
            auto_sync_miniflux(&db, &http, &app).await;
            tokio::time::sleep(TICK).await;
        }
    });
}

/// Miniflux 自动同步：读 autoSyncMiniflux（默认开）与刷新间隔，
/// 到期（now - last_sync ≥ refreshInterval 分钟）时跑轻量同步。
/// 失败静默（log 记录），下个 tick 仍会因 last_sync 未推进而重试。
/// 状态被拉平后发 feeds-updated——前端列表/未读计数与 DB 不再脱节
/// （pull 改变了 is_read 但用户无感知的"静默漂移"问题）。
async fn auto_sync_miniflux(
    db: &Arc<tokio::sync::Mutex<Connection>>,
    http: &reqwest::Client,
    app: &AppHandle,
) {
    let (on, interval_min, connected, last_sync) = {
        let conn = db.lock().await;
        let raw = crate::db::get_setting(&conn, "app_settings")
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
        let on = raw.as_ref()
            .and_then(|v| v.get("autoSyncMiniflux").and_then(|b| b.as_bool()))
            .unwrap_or(true);
        if !on {
            return;
        }
        let interval = raw
            .as_ref()
            .and_then(|v| v.get("refreshInterval").and_then(|i| i.as_i64()))
            .filter(|i| (5..=720).contains(i))
            .unwrap_or(30);
        let connected = crate::sync::read_credentials(&conn).is_some();
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
    log::info!("scheduler: Miniflux 自动同步开始（间隔 {interval_min} 分钟到期）");
    let _ = app.emit("sync-running", serde_json::json!({ "source": "auto" }));
    match crate::sync::sync_light(db, http).await {
        Ok(r) => {
            log::info!(
                "scheduler: Miniflux 自动同步完成：推 {}/拉 {} 项，{} 错误",
                r.pushed_states, r.pulled_entries, r.errors.len()
            );
            // 拉平了状态（或推空但有 pending 修正）→ 通知前端重载
            if r.pulled_entries > 0 {
                let _ = app.emit(
                    "feeds-updated",
                    serde_json::json!({ "new_articles": 0, "failed_feeds": 0 }),
                );
            }
        }
        Err(e) => log::warn!("scheduler: Miniflux 自动同步失败: {e}"),
    }
    let _ = app.emit("sync-idle", ());
}

/// 通知开关开启 且 主窗口不可见（最小化到托盘/失焦）。
async fn should_notify(db: &Arc<tokio::sync::Mutex<rusqlite::Connection>>, app: &AppHandle) -> bool {
    let on = {
        let conn = db.lock().await;
        crate::db::get_setting(&conn, "app_settings")
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v.get("notifyOnNewArticles").and_then(|b| b.as_bool()))
            .unwrap_or(false)
    };
    if !on {
        return false;
    }
    !app
        .get_webview_window("main")
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

/// 每轮最多补全的封面数（防一次性扫全库 + 轰炸源站）。
const COVER_BACKFILL_BATCH: i64 = 20;
/// 封面补全并发上限（低优先级，比 feed 抓取的默认 4 更低，避免抢带宽）。
const COVER_BACKFILL_CONCURRENCY: usize = 2;
/// 封面补全循环间隔：60s 醒一次，每轮最多处理一批，处理完下一批等下轮。
const COVER_BACKFILL_TICK: Duration = Duration::from_secs(60);

/// 封面后台补全循环：摘要型 RSS（少数派等）不带 media 字段，正文也没有图，
/// 列表卡片无封面。此循环对「无封面 + 有原文 URL 的直连文章」抓文章页
/// og:image 补封面（复用 extraction::lead_image），低优先级、限并发、失败
/// 负缓存（同一 URL 本进程不重试，避免反复轰炸源站）。
///
/// 只处理 direct 源：Miniflux 源入库时已用正文第一图兜底，无需再抓文章页。
pub fn spawn_cover_backfill(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        // 启动后等 45s 再跑首轮：避开启动期 UI 抢锁 + 让 feed 抓取先落库，
        // 否则首轮查到的是「还没有文章的库」，白跑一轮。
        tokio::time::sleep(Duration::from_secs(45)).await;
        let db = app.state::<AppState>().db.clone();
        let http = app.state::<AppState>().http.clone();
        let tried = std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashSet::<String>::new()));
        loop {
            // 取一批无封面文章（url, 已尝试过的不再取）
            let targets: Vec<(i64, String)> = {
                let conn = db.lock().await;
                let all = crate::db::articles_without_cover(&conn, COVER_BACKFILL_BATCH)
                    .unwrap_or_default();
                let tried_guard = tried.lock().await;
                all.into_iter()
                    .filter(|(_, url)| !tried_guard.contains(url))
                    .collect()
            };
            if targets.is_empty() {
                tokio::time::sleep(COVER_BACKFILL_TICK).await;
                continue;
            }

            let sem = Arc::new(tokio::sync::Semaphore::new(COVER_BACKFILL_CONCURRENCY));
            let mut handles = Vec::with_capacity(targets.len());
            for (aid, url) in targets {
                let sem = sem.clone();
                let db = db.clone();
                let http = http.clone();
                let tried = tried.clone();
                handles.push(tokio::spawn(async move {
                    let _permit = sem.acquire_owned().await;
                    match backfill_cover_once(&http, &db, &tried, aid, &url).await {
                        Ok(true) => Some(aid),
                        _ => None,
                    }
                }));
            }
            let mut filled = 0usize;
            for h in handles {
                if let Ok(Some(_)) = h.await {
                    filled += 1;
                }
            }
            if filled > 0 {
                log::info!("scheduler: 封面补全 {} 篇", filled);
                // 封面变化 → 通知前端重载（列表卡片封面即时补上）
                let _ = app.emit("feeds-updated", serde_json::json!({ "new_articles": 0, "failed_feeds": 0 }));
            }
            tokio::time::sleep(COVER_BACKFILL_TICK).await;
        }
    });
}

/// 单篇文章封面补全：抓文章页 → lead_image 抽 og:image → 落库（幂等 COALESCE）。
/// 返回 true=补到了封面；任何失败（网络/无 og:image/超时）记负缓存后返回 false。
async fn backfill_cover_once(
    http: &reqwest::Client,
    db: &Arc<tokio::sync::Mutex<Connection>>,
    tried: &Arc<tokio::sync::Mutex<std::collections::HashSet<String>>>,
    aid: i64,
    url: &str,
) -> Result<bool, ()> {
    // 负缓存：本进程已尝试过（失败/无图）的 URL 不再重试
    {
        let mut g = tried.lock().await;
        if g.contains(url) {
            return Ok(false);
        }
        g.insert(url.to_string());
    }
    // 拉文章页（30s 超时；只取 og:image，不必等整页正文）
    let resp = match http.get(url).timeout(std::time::Duration::from_secs(30)).send().await {
        Ok(r) if r.status().is_success() => r,
        _ => return Ok(false),
    };
    let html = match resp.text().await {
        Ok(h) => h,
        Err(_) => return Ok(false),
    };
    // lead_image 是纯同步（scraper）返回 Option<String>，spawn_blocking 里跑避免阻塞 async worker
    let base = url.to_string();
    let image = match tokio::task::spawn_blocking(move || crate::extraction::lead_image(&html, &base)).await {
        Ok(Some(img)) => img,
        _ => return Ok(false),
    };
    // 落库（幂等：已有封面不覆盖）
    let conn = db.lock().await;
    let n = conn
        .execute(
            "UPDATE articles SET image_url = COALESCE(image_url, ?1) WHERE id = ?2",
            rusqlite::params![image, aid],
        )
        .unwrap_or(0);
    Ok(n > 0)
}
