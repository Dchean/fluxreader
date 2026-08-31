//! 后台刷新调度器：定时醒来 → 查"到期"源 → 限并发抓取 → 事件通知前端。
//!
//! 设置实时读取（app_settings JSON 的 autoRefresh/refreshInterval），改设置
//! 无需重启即生效（下一个 tick 最多 60s 后跟上）。抓取与 `refresh_all_feeds`
//! 命令共用 `ingestion::refresh_feed` 管线，退避状态由 db 层统一维护。

use crate::state::AppState;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// 调度循环的醒来节奏。每 tick 只跑一条便宜的索引查询，没到期的源直接返回。
const TICK: Duration = Duration::from_secs(60);

/// 并发抓取上限：4（个人规模订阅数下兼顾速度与源站压力）。
const CONCURRENCY: usize = 4;

/// 从 app_settings JSON 里读 autoRefresh / refreshInterval / smartDedup。
/// async 版：在调度循环（tokio worker）里调用。
async fn read_refresh_config(db: &Arc<tokio::sync::Mutex<rusqlite::Connection>>) -> (bool, i64, bool) {
    let conn = db.lock().await;
    let raw = crate::db::get_setting(&conn, "app_settings").ok().flatten();
    let mut enabled = true;
    let mut interval = 30i64;
    let mut dedup = false;
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
    }
    (enabled, interval, dedup)
}

/// 全量刷新所有源（托盘「刷新全部订阅」入口，忽略到期时间）。
pub async fn refresh_all(
    db: &Arc<tokio::sync::Mutex<rusqlite::Connection>>,
    http: &reqwest::Client,
) -> (usize, usize) {
    refresh_feeds_inner(db, http, None).await
}

/// 抓取所有到期源（Semaphore 4 并发）。HTTP 在锁外执行，写库时短暂持锁。
/// 返回 (新增条数, 失败源数)。
async fn refresh_due_feeds(
    db: &Arc<tokio::sync::Mutex<rusqlite::Connection>>,
    http: &reqwest::Client,
) -> (usize, usize) {
    let (_, interval_min, dedup) = read_refresh_config(db).await;
    refresh_feeds_inner(db, http, Some((interval_min, dedup))).await
}

/// 抓取实现：Some((interval, dedup)) 只抓到期源，None 全量。
async fn refresh_feeds_inner(
    db: &Arc<tokio::sync::Mutex<rusqlite::Connection>>,
    http: &reqwest::Client,
    due_filter: Option<(i64, bool)>,
) -> (usize, usize) {
    use tokio::sync::Semaphore;
    let dedup = due_filter.map(|(_, d)| d).unwrap_or(false);
    let due: Vec<i64> = {
        let conn = db.lock().await;
        match due_filter {
            Some((interval_min, _)) => crate::db::feeds_due_for_refresh(&conn, interval_min),
            None => crate::db::feeds_all_ids(&conn),
        }
        .unwrap_or_else(|e| {
            log::warn!("scheduler: query feeds failed: {e}");
            Vec::new()
        })
    };
    if due.is_empty() {
        return (0, 0);
    }
    log::info!("scheduler: {} feed(s) due", due.len());

    let sem = Arc::new(Semaphore::new(CONCURRENCY));
    let mut handles = Vec::with_capacity(due.len());
    for id in due {
        let sem = sem.clone();
        let db = db.clone();
        let http = http.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire_owned().await;
            let mut conn = db.lock().await;
            match crate::ingestion::refresh_feed(&mut conn, &http, id, dedup).await {
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
            let (enabled, _interval, _dedup) = read_refresh_config(&db).await;
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
            tokio::time::sleep(TICK).await;
        }
    });
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
