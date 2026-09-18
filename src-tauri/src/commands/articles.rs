//! commands 的 articles 领域子模块（TASK-044 从 commands.rs 按既有章节拆分，纯搬运）。

use super::{read_dedup_flag, schedule_state_push};
use crate::db;
use crate::error::AppResult;
use crate::ingestion;
use crate::state::AppState;
use serde::Deserialize;
use tauri::State;

/* ============================================================
Articles
============================================================ */

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArticleListArgs {
    pub feed_id: Option<i64>,
    pub folder_id: Option<i64>,
    pub only_unread: Option<bool>,
    pub only_starred: Option<bool>,
    pub only_today: Option<bool>,
    pub newest_first: Option<bool>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub with_content: Option<bool>,
}

#[tauri::command]
pub async fn list_articles(
    state: State<'_, AppState>,
    args: ArticleListArgs,
) -> AppResult<Vec<db::ArticleListItem>> {
    let conn = state.db.lock().await;
    db::list_articles(&conn, &article_query(&args))
}

/// 计算某篇文章在当前筛选排序下的绝对位置（0 起）——供前端「搜索/深层打开
/// 文章后只加载目标页」的双向分页锚定。
#[tauri::command]
pub async fn article_index(
    state: State<'_, AppState>,
    args: ArticleListArgs,
    article_id: i64,
) -> AppResult<Option<i64>> {
    let conn = state.db.lock().await;
    db::article_index(&conn, &article_query(&args), article_id)
}

/// 从反序列化的列表参数构建 db::ArticleQuery（list_articles 与 article_index 共用）。
fn article_query(args: &ArticleListArgs) -> db::ArticleQuery {
    db::ArticleQuery {
        feed_id: args.feed_id,
        folder_id: args.folder_id,
        only_unread: args.only_unread.unwrap_or(false),
        only_starred: args.only_starred.unwrap_or(false),
        only_today: args.only_today.unwrap_or(false),
        newest_first: args.newest_first.unwrap_or(true),
        limit: args.limit.unwrap_or(500),
        offset: args.offset.unwrap_or(0),
        with_content: args.with_content.unwrap_or(false),
    }
}

#[tauri::command]
pub async fn get_article(state: State<'_, AppState>, id: i64) -> AppResult<Option<db::ArticleRow>> {
    let conn = state.db.lock().await;
    db::get_article(&conn, id)
}

#[tauri::command]
pub async fn get_articles(
    state: State<'_, AppState>,
    ids: Vec<i64>,
) -> AppResult<Vec<db::ArticleRow>> {
    let conn = state.db.lock().await;
    db::get_articles(&conn, &ids)
}

/// 全文搜索（FTS5）：标题/正文/作者/AI 摘要/翻译
#[tauri::command]
pub async fn search_articles(
    state: State<'_, AppState>,
    query: String,
    limit: Option<i64>,
) -> AppResult<Vec<db::ArticleListItem>> {
    let conn = state.db.lock().await;
    db::search_articles(&conn, &query, limit.unwrap_or(50))
}

/// 记录已读状态并写入同步队列（命令与测试共用的真实逻辑）。
/// A-5：无论是否已配置同步都入队——离线期间的变更保留持久化待推记录，
/// 连接后由 states_phase 推送段/即时推送补推。此前仅在 sync_configured 时
/// 入队，离线变更永不补推且可能被远端对账覆盖。
pub fn record_read_state(conn: &rusqlite::Connection, id: i64, read: bool) -> AppResult<()> {
    db::set_read(conn, id, read)?;
    db::enqueue_sync(
        conn,
        Some(id),
        None,
        if read { "read" } else { "unread" },
        None,
    )
}

/// 同 [`record_read_state`]：收藏状态。
pub fn record_star_state(conn: &rusqlite::Connection, id: i64, starred: bool) -> AppResult<()> {
    db::set_starred(conn, id, starred)?;
    db::enqueue_sync(
        conn,
        Some(id),
        None,
        if starred { "star" } else { "unstar" },
        None,
    )
}

#[tauri::command]
pub async fn set_read(state: State<'_, AppState>, id: i64, read: bool) -> AppResult<()> {
    {
        let conn = state.db.lock().await;
        record_read_state(&conn, id, read)?;
    }
    // 锁外调度即时推送（防抖合批，~1s 内到达服务端）；未配置时 push_states_now
    // 静默返回，队列项留待连接后的同步补推
    schedule_state_push(&state);
    Ok(())
}

#[tauri::command]
pub async fn set_starred(state: State<'_, AppState>, id: i64, starred: bool) -> AppResult<()> {
    {
        let conn = state.db.lock().await;
        record_star_state(&conn, id, starred)?;
    }
    schedule_state_push(&state);
    Ok(())
}

#[tauri::command]
pub async fn mark_all_read(
    state: State<'_, AppState>,
    feed_id: Option<i64>,
    folder_id: Option<i64>,
    starred_only: Option<bool>,
    since_ms: Option<i64>,
) -> AppResult<usize> {
    let starred_only = starred_only.unwrap_or(false);
    let n = {
        let conn = state.db.lock().await;
        apply_mark_all_read(&conn, feed_id, folder_id, starred_only, since_ms)?
    };
    // 锁外调度即时推送；未配置时 push_states_now 内 build_client 返回 None 而
    // 静默返回，队列项留待连接后的同步补推（与 set_read/set_starred 一致）
    schedule_state_push(&state);
    Ok(n)
}

/// 「全部已读」的标读与入队（命令与测试共用的真实逻辑）。
///
/// 入队口径（A-5，TASK-053 对齐）：**无论是否已配置同步后端都入队**——与
/// [`record_read_state`] / [`record_star_state`] 完全同语义。此前这里以
/// `sync_configured` 作为入队前置条件，造成「有的状态变更离线会入队、有的不会」
/// 的不一致：离线期间的「全部已读」不写待推队列，连接后首次全量对账按远端状态
/// 把本地已读翻回未读（用户现象：「刚标的已读自己变回去了」）。
///
/// 推送侧无需改动：未配置时 `push_states_now` / `states_phase` 经 `build_client`
/// 拿不到 client 而静默跳过（队列保留，连接后补推），这正是 A-5 建立的
/// 「无论是否 configured 都入队，推送段在未配置时静默跳过」语义。
///
/// 返回实际标读条数。入队集合由 [`db::list_unread_ids_scoped`] 在标读**前**收集，
/// 与 [`db::mark_all_read`] 同口径（F8）——标读后再查 `is_read = 0` 会得到空集，
/// 「全部已读」将永远不推送（历史 bug）。
pub fn apply_mark_all_read(
    conn: &rusqlite::Connection,
    feed_id: Option<i64>,
    folder_id: Option<i64>,
    starred_only: bool,
    since_ms: Option<i64>,
) -> AppResult<usize> {
    let ids = db::list_unread_ids_scoped(conn, feed_id, folder_id, starred_only, since_ms)?;
    let n = db::mark_all_read(conn, feed_id, folder_id, starred_only, since_ms)?;
    // A-5：无论是否已配置同步都入队。逐条入队（量级可控：个人订阅日常几十条）
    for id in ids {
        db::enqueue_sync(conn, Some(id), None, "read", None)?;
    }
    Ok(n)
}

#[tauri::command]
pub async fn feed_counts(state: State<'_, AppState>) -> AppResult<Vec<db::FeedCounts>> {
    let conn = state.db.lock().await;
    db::feed_counts(&conn)
}
/* ============================================================
刷新（直连抓取）
============================================================ */

/// 刷新单个订阅源（直连）。三段式：锁内取条件头 → 锁外 HTTP+解析 → 锁内落库。
/// 与并发管线共用 refresh_feed_staged，网络 IO 不占数据库写锁。
#[tauri::command]
pub async fn refresh_feed(state: State<'_, AppState>, feed_id: i64) -> AppResult<usize> {
    let db = state.db.clone();
    let client = state.http.clone();
    let dedup = {
        let conn = db.lock().await;
        read_dedup_flag(&conn)
    };
    ingestion::refresh_feed_staged(&db, &client, feed_id, dedup).await
}

/// 刷新全部订阅源（直连，并发上限 = 设置 fetchConcurrency，默认 4）。
/// 复用调度器的三段式管线：HTTP 锁外并行，写库短暂持锁。
#[tauri::command]
pub async fn refresh_all_feeds(state: State<'_, AppState>) -> AppResult<RefreshSummary> {
    let db = state.db.clone();
    let http = state.http.clone();
    let (n, f) = crate::scheduler::refresh_all(&db, &http).await;
    Ok(RefreshSummary {
        new_articles: n,
        failed_feeds: f,
    })
}

#[derive(serde::Serialize, Default)]
pub struct RefreshSummary {
    pub new_articles: usize,
    pub failed_feeds: usize,
}
