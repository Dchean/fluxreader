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

/// 批量标读的**真实逻辑**（命令与测试共用，与 [`apply_mark_all_read`] 同一手法）。
///
/// 审查 FINDING TASK-084-F1 指出：上一版把这段循环**抄**进测试体，导致测试从不调用
/// `set_read_bulk`——对真实命令体做变异（跳过入队/绕过 record_read_state）测试仍全绿，
/// 即那份「等价性证据」是装饰性的。现在把循环抽成本函数，命令与测试都调它，
/// 变异真实命令路径即可让测试失败（并已被实测验证）。
pub fn apply_read_bulk(conn: &rusqlite::Connection, ids: &[i64], read: bool) -> AppResult<()> {
    for id in ids {
        record_read_state(conn, *id, read)?;
    }
    Ok(())
}

/// 批量标读：一次 IPC 处理整批 id（AUDIT P3[F4]）。
///
/// 背景：前端 `markEntriesReadBulk` 此前对每个 id 各发一次 `invoke('set_read')`，
/// 「全部已读」/滚动标读传入几百个 id 时就是几百次 IPC 往返。本命令把它收敛为一次：
/// 整批在**同一次持锁**内写完，锁外只调度一次推送。
///
/// 语义与逐条路径严格一致（这是本命令的唯一契约，由 [`apply_read_bulk`] 承载并单测锁定）：
///   · 每个 id 都经 [`record_read_state`] —— 本地 `is_read` 写入 + `sync_queue`
///     入队（action 名 `read`/`unread`）与逐条完全相同；
///   · 未配置同步后端时同样入队、由推送段静默跳过（A-5 语义）；
///   · 空 ids 安全返回。
/// 粒度差异：整批共用一次锁与一次 push 调度。注意 `record_read_state` 内部**逐条提交**
/// （无外层事务），故「不写半批」只对内存态成立——中途失败时先前 id 已落库并已入队。
/// 这是有意保留的：给整批包事务会改变 `record_read_state` 的既有语义（见本卡 non_goals）。
#[tauri::command]
pub async fn set_read_bulk(state: State<'_, AppState>, ids: Vec<i64>, read: bool) -> AppResult<()> {
    if ids.is_empty() {
        return Ok(());
    }
    {
        let conn = state.db.lock().await;
        apply_read_bulk(&conn, &ids, read)?;
    }
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


#[cfg(test)]
mod bulk_read_tests {
    use super::*;
    use rusqlite::Connection;
    use crate::db::{create_folder, insert_feed, upsert_article_with_feed, MIGRATIONS, NewArticle};

    fn seeded(n: usize) -> (Connection, Vec<i64>) {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let f = create_folder(&conn, "F", "article").unwrap();
        let feed = insert_feed(
            &conn, "http://a.example/feed", None, "feed", None, f, "inherit", false, false,
        )
        .unwrap();
        let mut ids = Vec::new();
        for i in 0..n {
            let a = NewArticle {
                guid: format!("g{i}"),
                url: Some(format!("http://a.example/{i}")),
                title: format!("t{i}"),
                author: None,
                summary: None,
                content_html: None,
                body_text: "b".into(),
                image_url: None,
                enclosure_url: None,
                enclosure_mime: None,
                duration_sec: None,
                published_at: None,
                source: "direct".into(),
            };
            let (id, _) = upsert_article_with_feed(&conn, feed, &a, false).unwrap();
            ids.push(id);
        }
        (conn, ids)
    }

    fn count_read(c: &Connection) -> i64 {
        c.query_row("SELECT COUNT(*) FROM articles WHERE is_read = 1", [], |r| r.get(0))
            .unwrap()
    }

    fn queued(c: &Connection) -> Vec<(String, Option<i64>)> {
        let mut stmt = c
            .prepare("SELECT action, article_id FROM sync_queue ORDER BY action, article_id")
            .unwrap();
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?)))
            .unwrap();
        rows.collect::<Result<Vec<_>, _>>().unwrap()
    }

    /// AUDIT P3[F4]（TASK-084）：批量标读必须与逐条标读**语义等价**。
    ///
    /// **本次审查修复（FINDING TASK-084-F1）**：上一版把批量循环**抄**进测试体，
    /// 于是测试从不调用真实代码路径——对 `set_read_bulk` 做变异（跳过入队/绕过
    /// record_read_state）测试仍全绿，那份「等价性证据」是装饰性的。
    /// 现在批量侧调用的是**命令实际调用的同一个函数** [`apply_read_bulk`]，
    /// 对它的任何改动都会被本用例捕获（已用变异实测验证）。
    #[test]
    fn apply_read_bulk_is_equivalent_to_per_item_read_state() {
        // 路径 A：逐条（= 修前前端的行为：每个 id 一次 set_read → record_read_state）
        let (conn_a, ids_a) = seeded(5);
        for id in &ids_a {
            record_read_state(&conn_a, *id, true).unwrap();
        }

        // 路径 B：批量 —— 调用 set_read_bulk 内部真正使用的 apply_read_bulk
        let (conn_b, ids_b) = seeded(5);
        apply_read_bulk(&conn_b, &ids_b, true).unwrap();

        assert_eq!(count_read(&conn_a), 5, "逐条路径：5 条应已读");
        assert_eq!(count_read(&conn_b), 5, "批量路径：5 条应已读");
        assert_eq!(
            count_read(&conn_a),
            count_read(&conn_b),
            "批量与逐条的本地已读结果必须一致"
        );

        let qa = queued(&conn_a);
        let qb = queued(&conn_b);
        assert_eq!(qa.len(), 5, "逐条路径：应入队 5 条 read");
        assert_eq!(
            qa, qb,
            "批量与逐条的 sync_queue 队项（action + article_id 集合）必须完全一致"
        );
        assert!(qa.iter().all(|(a, _)| a == "read"), "action 名应保持 read");
        assert_eq!(qb.len(), 5, "批量路径必须逐条入队（绕过 enqueue 即失败）");
    }

    /// 反向保护：批量路径**不得**绕过入队（若只写 is_read 而不入队，本用例必须失败）。
    /// 这是对 F1 的直接回归锚——它钉住「批量路径也要产生 sync_queue 行」这一契约。
    #[test]
    fn apply_read_bulk_enqueues_for_every_id() {
        let (conn, ids) = seeded(3);
        apply_read_bulk(&conn, &ids, true).unwrap();

        assert_eq!(count_read(&conn), 3, "3 条都应已读");
        let q = queued(&conn);
        assert_eq!(q.len(), 3, "每个 id 都必须产生一条队项（跳过入队即失败）");
        let queued_ids: Vec<i64> = q.iter().filter_map(|(_, id)| *id).collect();
        let mut want: Vec<i64> = ids.clone();
        want.sort();
        let mut got = queued_ids.clone();
        got.sort();
        assert_eq!(got, want, "入队的 id 集合必须与传入的 ids 完全一致");
    }

    /// 空 ids：不得写库、不得入队、不得报错。
    #[test]
    fn apply_read_bulk_empty_is_noop() {
        let (conn, _) = seeded(2);
        apply_read_bulk(&conn, &[], true).unwrap();
        assert_eq!(count_read(&conn), 0, "空 ids 不应标读任何条目");
        assert_eq!(queued(&conn).len(), 0, "空 ids 不应入队");
    }

    /// 方向不得写死：read=false 同样要入队 `unread`。
    /// （注：`enqueue_sync` 对同文章正反方向做互斥合并，见 db/sync_queue.rs:26-32，
    ///  故 read→unread 后只剩最后那条 `unread`；这里断言该真实合并语义。）
    #[test]
    fn apply_read_bulk_false_queues_unread_actions() {
        let (conn, ids) = seeded(2);
        let id = ids[0];
        record_read_state(&conn, id, true).unwrap();
        apply_read_bulk(&conn, &[id], false).unwrap();

        let is_read: i64 = conn
            .query_row("SELECT is_read FROM articles WHERE id = ?1", [id], |r| r.get(0))
            .unwrap();
        assert_eq!(is_read, 0, "read=false 必须把文章标回未读");

        let actions: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT action FROM sync_queue WHERE article_id = ?1 ORDER BY id")
                .unwrap();
            let rows = stmt.query_map([id], |r| r.get::<_, String>(0)).unwrap();
            rows.collect::<Result<Vec<_>, _>>().unwrap()
        };
        assert_eq!(
            actions,
            vec!["unread"],
            "同文章反向入队应互斥合并，只留最新的 unread（不得留下 read+unread 两条）"
        );
    }
}
