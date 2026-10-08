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

// Note: rename_all 只作用于命令形参名，不作用于形参内部对象；字段名与前端 payload 须逐字一致 — 见 .agents/notes/implemented/bug-fix/2026-09-22-命令入参序列化契约.md
#[derive(Deserialize)]
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
    /// 布局过滤（TASK-094 / REQ-107，可选）：与前端 ContentLayoutType 同词表
    /// （article/social/image/podcast/notification）。省略 = 不过滤（既有调用
    /// 逐字不变）；过滤口径与 mark_all_read 同源（feed 级覆盖 → 分类兜底）。
    pub layout: Option<String>,
    /// TASK-117：keyset 续拉游标锚——上一页最后一行的 (published_at 原文, id)。
    /// wire 键为 snake_case（与 feed_id 等既有键同一命名口径）。两者**成对**给出
    /// 才启用 keyset 谓词；省略 = 既有 OFFSET 语义（首屏/锚定路径不带）。
    /// last_published 是外部字符串，后端只经绑定参数进入 SQL（注入面为零）。
    pub last_published: Option<String>,
    pub last_id: Option<i64>,
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
        layout: args.layout.clone(),
        last_published: args.last_published.clone(),
        last_id: args.last_id,
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

/// 图片位上报当前封面失效；仅在 URL 未被其它更新替换时清空，避免误删新封面。
#[tauri::command]
pub async fn report_broken_cover(
    state: State<'_, AppState>,
    article_id: i64,
    url: String,
) -> AppResult<bool> {
    let conn = state.db.lock().await;
    Ok(db::clear_article_cover_if_matches(&conn, article_id, &url)? > 0)
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
/// TASK-108：复合操作下沉为 [`db::set_read_with_enqueue`] **单事务**——状态写入
/// 与入队同生共死：入队步失败时状态写入一并回滚。修前无外层事务，入队失败会
/// 留下「本地已改但无待推记录」的孤儿变更（离线永不补推，且可能被远端对账覆盖）。
pub fn record_read_state(conn: &rusqlite::Connection, id: i64, read: bool) -> AppResult<()> {
    db::set_read_with_enqueue(conn, id, read)
}

/// 同 [`record_read_state`]：收藏状态（TASK-108 单事务化，同上）。
pub fn record_star_state(conn: &rusqlite::Connection, id: i64, starred: bool) -> AppResult<()> {
    db::set_starred_with_enqueue(conn, id, starred)
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
    // TASK-124（审计 P2-6①，探针 P8 本体）：本地事务已提交 → 立即发
    // sync-queue-changed，pill 即时「等待同步 N 条」（不等 800ms 推送往返）。
    // 锁外发（notify_queue_changed 内部自持短锁读统计）；未配置时内部静默。
    crate::sync::notify_queue_changed(&state.db).await;
    Ok(())
}

/// 批量标读的**真实逻辑**（命令与测试共用，与 [`apply_mark_all_read`] 同一手法）。
///
/// 审查 FINDING TASK-084-F1 指出：上一版把这段循环**抄**进测试体，导致测试从不调用
/// `set_read_bulk`——对真实命令体做变异（跳过入队/绕过 record_read_state）测试仍全绿，
/// 即那份「等价性证据」是装饰性的。现在把循环抽成本函数，命令与测试都调它，
/// 变异真实命令路径即可让测试失败（并已被实测验证）。
///
/// TASK-108：整批下沉为 [`db::set_read_bulk_with_enqueue`]——**单事务全有全无**，
/// 逐 id 走与 [`record_read_state`] 完全相同的内核语句（状态写入 + 入队），语义
/// 与单条路径严格一致；中途任一 id 失败整批回滚，不再有部分成功（修前逐条
/// 自动提交，中途失败会留下半批已落库+已入队）。
pub fn apply_read_bulk(conn: &rusqlite::Connection, ids: &[i64], read: bool) -> AppResult<()> {
    db::set_read_bulk_with_enqueue(conn, ids, read)
}

/// 批量标读：一次 IPC 处理整批 id（AUDIT P3[F4]）。
///
/// 背景：前端 `markEntriesReadBulk` 此前对每个 id 各发一次 `invoke('set_read')`，
/// 「全部已读」/滚动标读传入几百个 id 时就是几百次 IPC 往返。本命令把它收敛为一次：
/// 整批在**同一次持锁**内写完，锁外只调度一次推送。
///
/// 语义契约（由 [`apply_read_bulk`] 承载并单测锁定，与逐条路径严格一致）：
///   · 每个 id 都走与 [`record_read_state`] 相同的内核语句——本地 `is_read` 写入 +
///     `sync_queue` 入队（action 名 `read`/`unread`）；
///   · 未配置同步后端时同样入队、由推送段静默跳过（A-5 语义）；
///   · 空 ids 安全返回；
///   · TASK-108：整批**单事务、全有全无**——任一 id 的写入或入队失败整批回滚，
///     不存在部分成功（修前逐条提交，「不写半批」只对内存态成立）。
/// 粒度差异：整批共用一次锁、一次事务与一次 push 调度。
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
    // TASK-124：整批单事务提交后发 sync-queue-changed（锁外，同 set_read 口径）
    crate::sync::notify_queue_changed(&state.db).await;
    Ok(())
}

#[tauri::command]
pub async fn set_starred(state: State<'_, AppState>, id: i64, starred: bool) -> AppResult<()> {
    {
        let conn = state.db.lock().await;
        record_star_state(&conn, id, starred)?;
    }
    schedule_state_push(&state);
    // TASK-124：收藏入队事务提交后发 sync-queue-changed（锁外，同 set_read 口径）
    crate::sync::notify_queue_changed(&state.db).await;
    Ok(())
}

#[tauri::command]
pub async fn mark_all_read(
    state: State<'_, AppState>,
    feed_id: Option<i64>,
    folder_id: Option<i64>,
    starred_only: Option<bool>,
    since_ms: Option<i64>,
    layout: Option<String>,
) -> AppResult<usize> {
    let starred_only = starred_only.unwrap_or(false);
    let n = {
        let conn = state.db.lock().await;
        apply_mark_all_read(
            &conn,
            feed_id,
            folder_id,
            starred_only,
            since_ms,
            layout.as_deref(),
        )?
    };
    // 锁外调度即时推送；未配置时 push_states_now 内 build_client 返回 None 而
    // 静默返回，队列项留待连接后的同步补推（与 set_read/set_starred 一致）
    schedule_state_push(&state);
    // TASK-124：「全部已读」集合入队单事务提交后发 sync-queue-changed（锁外）——
    // 一次 mark_all_read 入队 N 条，事件只发一次（统计已聚合）。
    crate::sync::notify_queue_changed(&state.db).await;
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
/// 返回实际标读条数。REQ-108 M-9：整条链路已在 [`db::mark_all_read_with_enqueue`]
/// 集合化（一个事务内 3 条语句，语句数不随未读条目数增长；旧实现为
/// 1 SELECT + 1 UPDATE + 每 id 一次 DELETE+INSERT = 2N+2 条且全程持库锁），
/// 入队集合仍按标读**前**的 `is_read = 0` 求值——标读后再查会得到空集，
/// 「全部已读」将永远不推送（历史 bug，F8 口径由等价性测试锁定）。
pub fn apply_mark_all_read(
    conn: &rusqlite::Connection,
    feed_id: Option<i64>,
    folder_id: Option<i64>,
    starred_only: bool,
    since_ms: Option<i64>,
    layout: Option<&str>,
) -> AppResult<usize> {
    db::mark_all_read_with_enqueue(conn, feed_id, folder_id, starred_only, since_ms, layout)
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
    use crate::db::{create_folder, insert_feed, upsert_article_with_feed, NewArticle, MIGRATIONS};
    use rusqlite::Connection;

    fn seeded(n: usize) -> (Connection, Vec<i64>) {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let f = create_folder(&conn, "F", "article").unwrap();
        let feed = insert_feed(
            &conn,
            "http://a.example/feed",
            None,
            "feed",
            None,
            f,
            "inherit",
            false,
            false,
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
        c.query_row("SELECT COUNT(*) FROM articles WHERE is_read = 1", [], |r| {
            r.get(0)
        })
        .unwrap()
    }

    fn count_starred(c: &Connection) -> i64 {
        c.query_row(
            "SELECT COUNT(*) FROM articles WHERE is_starred = 1",
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn queued(c: &Connection) -> Vec<(String, Option<i64>)> {
        let mut stmt = c
            .prepare("SELECT action, article_id FROM sync_queue ORDER BY action, article_id")
            .unwrap();
        let rows = stmt
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?))
            })
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
            .query_row("SELECT is_read FROM articles WHERE id = ?1", [id], |r| {
                r.get(0)
            })
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

    /* ---------- TASK-108：状态写入 + 入队 同生共死（故障注入） ---------- */

    /// 故障注入手法（TASK-108）：在 sync_queue 上挂 `BEFORE INSERT` 触发器
    /// `RAISE(ABORT)`——纯 SQL 层注入，不打桩、不引依赖、不改生产语义；失败
    /// 发生在真实 `enqueue_sync` 的 INSERT 语句上，此时同一事务内的状态写入
    /// UPDATE 已经执行，断言整体回滚。
    fn inject_enqueue_failure_all(c: &Connection) {
        c.execute_batch(
            "CREATE TRIGGER inject_enqueue_fail BEFORE INSERT ON sync_queue
             BEGIN SELECT RAISE(ABORT, 'injected: enqueue step failed'); END",
        )
        .unwrap();
    }

    /// TASK-108 验收③（单条·读）：入队步失败 → 已读状态不落库。
    /// 修前 record_read_state 无外层事务：set_read 已自动提交，入队失败留下
    /// 「已读但无待推记录」的孤儿变更（离线永不补推）。修后必须整体回滚。
    #[test]
    fn record_read_state_enqueue_failure_rolls_back_read_write() {
        let (conn, ids) = seeded(1);
        let id = ids[0];
        inject_enqueue_failure_all(&conn);

        let err = record_read_state(&conn, id, true).unwrap_err();
        assert!(
            err.to_string().contains("injected: enqueue step failed"),
            "失败必须来自注入的入队步（而非状态写入步）：{err}"
        );
        assert_eq!(
            count_read(&conn),
            0,
            "入队失败时 is_read 不得落库（事务回滚，同生共死）"
        );
        assert!(queued(&conn).is_empty(), "失败的队项也不得残留");
    }

    /// TASK-108 验收③（单条·藏）：同上，收藏状态。
    #[test]
    fn record_star_state_enqueue_failure_rolls_back_star_write() {
        let (conn, ids) = seeded(1);
        let id = ids[0];
        inject_enqueue_failure_all(&conn);

        let err = record_star_state(&conn, id, true).unwrap_err();
        assert!(
            err.to_string().contains("injected: enqueue step failed"),
            "失败必须来自注入的入队步（而非状态写入步）：{err}"
        );
        assert_eq!(
            count_starred(&conn),
            0,
            "入队失败时 is_starred 不得落库（事务回滚，同生共死）"
        );
        assert!(queued(&conn).is_empty(), "失败的队项也不得残留");
    }

    /// TASK-108 验收②：bulk **全有全无**——中途 id 失败时，先前 id 已在事务内
    /// 完成的写入与入队一并回滚。WHEN 子句只在最后一篇的入队上触发失败：若实现
    /// 退回逐条自动提交（修前形态），前两篇会部分成功落库，本用例必红。
    #[test]
    fn apply_read_bulk_is_all_or_nothing_on_mid_batch_enqueue_failure() {
        let (conn, ids) = seeded(3);
        let bad = ids[2]; // 最后一篇注入失败：前两篇已写入，若非单事务即部分成功
                          // 触发器 WHEN 里的 id 是夹具自产的 i64，直接内插进 DDL（SQLite 触发器
                          // 程序内不允许绑定参数）
        conn.execute_batch(&format!(
            "CREATE TRIGGER inject_enqueue_fail_mid BEFORE INSERT ON sync_queue
             WHEN NEW.article_id = {bad}
             BEGIN SELECT RAISE(ABORT, 'injected: mid-batch enqueue failure'); END"
        ))
        .unwrap();

        let err = apply_read_bulk(&conn, &ids, true).unwrap_err();
        assert!(
            err.to_string()
                .contains("injected: mid-batch enqueue failure"),
            "失败必须来自注入点（bad = {bad}）：{err}"
        );
        assert_eq!(
            count_read(&conn),
            0,
            "任一 id 失败必须整批回滚：先前 id 不得落库（无部分成功）"
        );
        assert_eq!(queued(&conn).len(), 0, "先前 id 的队项也必须一并回滚");
    }

    /// TASK-108 补充锚：record_star_state 成功路径（此前无直接单测），锁定
    /// 「写状态 + 入队」复合语义未被事务化改写，反向合并语义与逐条入队一致。
    #[test]
    fn record_star_state_success_writes_state_and_enqueues() {
        let (conn, ids) = seeded(2);
        record_star_state(&conn, ids[0], true).unwrap();
        assert_eq!(count_starred(&conn), 1, "收藏应落库");
        assert_eq!(
            queued(&conn),
            vec![("star".to_string(), Some(ids[0]))],
            "应入队一条 star"
        );

        record_star_state(&conn, ids[0], false).unwrap();
        assert_eq!(count_starred(&conn), 0, "取消收藏应落库");
        assert_eq!(
            queued(&conn),
            vec![("unstar".to_string(), Some(ids[0]))],
            "同文章 star→unstar 互斥合并，只留最新的 unstar"
        );
    }

    /// P0-1: ArticleListArgs 反序列化必须原生支持 snake_case（与前端契约对齐）
    #[test]
    fn article_list_args_deserializes_snake_case() {
        let json_payload = serde_json::json!({
            "feed_id": 10,
            "folder_id": 2,
            "only_unread": true,
            "only_starred": false,
            "only_today": true,
            "newest_first": false,
            "limit": 50,
            "offset": 100,
            "with_content": true,
            "layout": "image",
            "last_published": "2026-01-01T00:00:00+08:00",
            "last_id": 42,
        });

        let args: ArticleListArgs = serde_json::from_value(json_payload).unwrap();
        assert_eq!(args.feed_id, Some(10));
        assert_eq!(args.folder_id, Some(2));
        assert_eq!(args.only_unread, Some(true));
        assert_eq!(args.only_starred, Some(false));
        assert_eq!(args.only_today, Some(true));
        assert_eq!(args.newest_first, Some(false));
        assert_eq!(args.limit, Some(50));
        assert_eq!(args.offset, Some(100));
        assert_eq!(args.with_content, Some(true));
        // TASK-094：可选 layout（snake_case 键名与前端一致）；缺省 = None
        assert_eq!(args.layout.as_deref(), Some("image"));
        // TASK-117：keyset 游标锚（snake_case wire 键，原文透传不重格式化）
        assert_eq!(
            args.last_published.as_deref(),
            Some("2026-01-01T00:00:00+08:00")
        );
        assert_eq!(args.last_id, Some(42));
        let without: ArticleListArgs =
            serde_json::from_value(serde_json::json!({ "limit": 1 })).unwrap();
        assert_eq!(without.layout, None, "layout 缺省必须是 None（不过滤）");
        assert_eq!(
            without.last_published, None,
            "last_published 缺省必须是 None（走既有 OFFSET 语义）"
        );
        assert_eq!(without.last_id, None, "last_id 缺省必须是 None");
    }
}
