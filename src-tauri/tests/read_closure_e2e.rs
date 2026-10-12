//! OPT-008B 第一阶段 R1 返工：读传播的**本地半边**（read 广播 = 同组原子读+版本+入队）
//! 与「全部已读真的清空范围」的真实隔离库回归。
//!
//! 返工依据（主控裁决）：
//! - 显示去重不意味着操作只改代表：`mark_all_read_with_enqueue` 3 源同 URL 3 未读
//!   的探针（tmp/optimization-20261008/OPT-008B/opt008b_controller_probe.rs）证明
//!   只改代表时 changed=1/read=1/queued=1，另外 2 行仍未读；离线/纯直连无法靠
//!   远端收敛，范围未清空。
//! - 读政策本地半边：同组（同 `url_norm`）read 原子更新相应本地条目版本与队列
//!   （与推送 read 广播同政策）；unread/star 只当前条目。
//!
//! 这些用例先在旧实现下红、修复后绿（red-first）。

use app_lib::db;

mod common;
mod mock_greader;

fn setup(name: &str) -> rusqlite::Connection {
    let tmp = common::unique_db_path(&format!("read_closure_{name}"));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();
    db::set_setting(&conn, "app_settings", r#"{"smartDedup":true}"#).unwrap();
    conn
}

fn seed_same_url_rows(conn: &rusqlite::Connection, n: i64) -> Vec<i64> {
    let folder = db::create_folder(conn, "probe", "article").unwrap();
    let mut ids = Vec::new();
    for i in 0..n {
        let feed = db::insert_feed(
            conn,
            &format!("https://probe.invalid/feed/{i}"),
            None,
            "probe",
            None,
            folder,
            "inherit",
            false,
            false,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO articles(feed_id,guid,url,url_norm,title,published_at,is_read)
             VALUES(?1,?2,'https://probe.invalid/shared','https://probe.invalid/shared','probe','2026-10-12T00:00:00Z',0)",
            rusqlite::params![feed, format!("guid-{i}")],
        )
        .unwrap();
        ids.push(conn.last_insert_rowid());
    }
    ids
}

fn read_queued(conn: &rusqlite::Connection) -> Vec<i64> {
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT article_id FROM sync_queue WHERE action = 'read' ORDER BY article_id",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |r| r.get::<_, i64>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    rows
}

fn unread_ids(conn: &rusqlite::Connection) -> Vec<i64> {
    let mut stmt = conn
        .prepare("SELECT id FROM articles WHERE is_read = 0 ORDER BY id")
        .unwrap();
    let rows = stmt
        .query_map([], |r| r.get::<_, i64>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    rows
}

/// 主控探针本体（SQL 级）：3 源同 URL 3 未读，markAll 后：
/// - 3 行全部已读（不依赖远端同步收敛）；
/// - 每一行都有自己的一条 read 队项（changed 行与入队行一致）；
/// - 该范围未读集合为空、显示计数 unread == 0。
#[test]
fn mark_all_read_clears_all_scoped_copies_atomically() {
    let conn = setup("markall");
    let ids = seed_same_url_rows(&conn, 3);

    let changed = db::mark_all_read_with_enqueue(&conn, None, None, false, None, None).unwrap();
    let read: i64 = conn
        .query_row("SELECT COUNT(*) FROM articles WHERE is_read = 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    let queued = read_queued(&conn);
    let unguarded: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM articles a WHERE is_read = 1
             AND NOT EXISTS (SELECT 1 FROM sync_queue q WHERE q.article_id = a.id AND q.action = 'read')",
            [],
            |r| r.get(0),
        )
        .unwrap();

    assert_eq!(
        read, 3,
        "markAll 必须清空范围内全部未读副本（不等待远端收敛）"
    );
    assert_eq!(unguarded, 0, "每个被标读行都必须有自己的 read 队项");
    assert_eq!(
        changed, 3,
        "changed 应为真实改变的行数（3 行全部从未读变已读）"
    );
    let mut want = ids.clone();
    want.sort_unstable();
    assert_eq!(queued, want, "三行各自入队 read");
    assert!(unread_ids(&conn).is_empty(), "范围未读集合真的为空");

    let counts = db::scope_counts(&conn, None).unwrap();
    assert_eq!(counts.all.unread, 0, "显示计数 unread 必须为 0");
}

/// 范围闭包：scope 内命中组的**跨 scope 兄弟**按 read 广播政策一并标读+入队
/// （跨布局/跨源不缩减）；scope 外不相关条目不动。
#[test]
fn mark_all_read_broadcasts_to_same_group_copies_outside_scope() {
    let conn = setup("scope_closure");
    let folder = db::create_folder(&conn, "F", "article").unwrap();
    // scope 内：feed1（article 布局）一行
    let feed1 = db::insert_feed(
        &conn,
        "https://probe.invalid/f1",
        None,
        "f1",
        None,
        folder,
        "inherit",
        false,
        false,
    )
    .unwrap();
    // 同组兄弟：feed2（social 布局，scope 外）
    let feed2 = db::insert_feed(
        &conn,
        "https://probe.invalid/f2",
        None,
        "f2",
        None,
        folder,
        "social",
        false,
        false,
    )
    .unwrap();
    // 无关条目：另一 URL 且**在 scope 外**（social 布局，article scope 不含）
    let feed3 = db::insert_feed(
        &conn,
        "https://probe.invalid/f3",
        None,
        "f3",
        None,
        folder,
        "social",
        false,
        false,
    )
    .unwrap();
    let mk = |feed: i64, guid: &str, url: &str| {
        conn.execute(
            "INSERT INTO articles(feed_id,guid,url,url_norm,title,published_at,is_read)
             VALUES(?1,?2,?3,?3,'t','2026-10-12T00:00:00Z',0)",
            rusqlite::params![feed, guid, url],
        )
        .unwrap();
        conn.last_insert_rowid()
    };
    let in_scope = mk(feed1, "a", "https://probe.invalid/u");
    let sibling = mk(feed2, "b", "https://probe.invalid/u");
    let unrelated = mk(feed3, "c", "https://probe.invalid/other");

    let changed =
        db::mark_all_read_with_enqueue(&conn, None, None, false, None, Some("article")).unwrap();

    let states: Vec<(i64, i64)> = conn
        .prepare("SELECT id, is_read FROM articles ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let by_id: std::collections::HashMap<i64, i64> = states.into_iter().collect();
    assert_eq!(by_id[&in_scope], 1, "scope 内行使已读");
    assert_eq!(by_id[&sibling], 1, "同组跨 scope 兄弟按 read 广播政策标读");
    assert_eq!(by_id[&unrelated], 0, "不相关条目不得被波及");
    assert_eq!(changed, 2, "changed = 真实改变行数（scope 行 + 广播兄弟）");
    let queued = read_queued(&conn);
    assert!(
        queued.contains(&in_scope) && queued.contains(&sibling) && !queued.contains(&unrelated),
        "入队集合 = 改变集合（含广播兄弟，不含无关行）: {queued:?}"
    );
}

/// 单条「标读 + 入队」（用户入口）同样持本地广播半边：同组未读副本一并读+版本+入队；
/// 取消标读（unread）只当前条目；收藏只当前条目。
#[test]
fn single_read_with_enqueue_broadcasts_group_but_unread_and_star_stay_current() {
    let conn = setup("single");
    let ids = seed_same_url_rows(&conn, 2);
    let (a, b) = (ids[0], ids[1]);

    db::set_read_with_enqueue(&conn, a, true).unwrap();
    let read_state = |id: i64| -> i64 {
        conn.query_row("SELECT is_read FROM articles WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap()
    };
    assert_eq!(read_state(a), 1);
    assert_eq!(read_state(b), 1, "同组未读副本必须一并标读（本地广播半边）");
    let queued = read_queued(&conn);
    assert!(
        queued.contains(&a) && queued.contains(&b),
        "同组两行各自入队 read: {queued:?}"
    );
    let versions: Vec<(i64, i64)> = conn
        .prepare("SELECT id, read_version FROM articles ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let vmap: std::collections::HashMap<i64, i64> = versions.into_iter().collect();
    assert!(vmap[&b] >= 1, "副本的读字段版本同样递增（原子意图）");

    // unread（取消标读）只当前条目
    db::set_read_with_enqueue(&conn, a, false).unwrap();
    assert_eq!(read_state(a), 0);
    assert_eq!(read_state(b), 1, "unread 不得广播给同组兄弟");

    // star 只当前条目
    db::set_starred_with_enqueue(&conn, a, true).unwrap();
    let star_state = |id: i64| -> i64 {
        conn.query_row("SELECT is_starred FROM articles WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap()
    };
    assert_eq!(star_state(a), 1);
    assert_eq!(star_state(b), 0, "收藏不得广播给同组兄弟");
}

/// 批量标读同口径：整批同事务、同组闭包全部读+入队；不相关 URL 不动。
#[test]
fn bulk_read_with_enqueue_applies_group_closure() {
    let conn = setup("bulk");
    let ids = seed_same_url_rows(&conn, 3);
    let folder = db::create_folder(&conn, "G", "article").unwrap();
    let other_feed = db::insert_feed(
        &conn,
        "https://probe.invalid/other",
        None,
        "o",
        None,
        folder,
        "inherit",
        false,
        false,
    )
    .unwrap();
    conn.execute(
        "INSERT INTO articles(feed_id,guid,url,url_norm,title,published_at,is_read)
         VALUES(?1,'og','https://probe.invalid/other','https://probe.invalid/other','t','2026-10-12T00:00:00Z',0)",
        [other_feed],
    )
    .unwrap();
    let other = conn.last_insert_rowid();

    db::set_read_bulk_with_enqueue(&conn, &[ids[0]], true).unwrap();

    let reads: Vec<(i64, i64)> = conn
        .prepare("SELECT id, is_read FROM articles ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let m: std::collections::HashMap<i64, i64> = reads.into_iter().collect();
    for id in &ids {
        assert_eq!(m[id], 1, "组内副本 {id} 必须标读");
    }
    assert_eq!(m[&other], 0, "不同 URL 不得被波及");
    let queued = read_queued(&conn);
    for id in &ids {
        assert!(queued.contains(id), "组内副本 {id} 必须入队");
    }
    assert!(!queued.contains(&other));
}

/// 故障注入：读广播目标查询失败（remote_dup_ids 被写成 BLOB 使 String 读取失败）
/// 时，推送计划必须整体失败、**保留队列意图**——不得 `unwrap_or_default` 当成
/// 「没有副本」继续推送并 prune（那会把原 read 意图一并清掉）。
#[tokio::test]
async fn push_plan_error_keeps_queue_instead_of_dropping_broadcast_targets() {
    use app_lib::sync;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    let server = mock_greader::MockGReader::start().await.unwrap();
    let tmp = common::unique_db_path("read_closure_push_fail");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();
    db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
    db::set_setting(&conn, "greader_username", "test").unwrap();
    db::set_setting(&conn, "greader_password", "test-token").unwrap();
    let folder = db::create_folder(&conn, "F", "article").unwrap();
    let feed = db::insert_feed(
        &conn,
        "http://127.0.0.1:8765/local_feed.xml",
        None,
        "L",
        None,
        folder,
        "inherit",
        false,
        false,
    )
    .unwrap();
    db::set_feed_remote_id(&conn, feed, 10).unwrap();
    let (aid, _) = db::upsert_article_with_feed(
        &conn,
        feed,
        &db::NewArticle {
            guid: "g1".into(),
            url: Some("http://127.0.0.1:8765/post/1".into()),
            title: "t".into(),
            author: None,
            summary: None,
            content_html: None,
            body_text: "b".into(),
            image_url: None,
            enclosure_url: None,
            enclosure_mime: None,
            duration_sec: None,
            published_at: Some(chrono::Utc::now().to_rfc3339()),
            source: "direct".into(),
        },
        false,
    )
    .unwrap();
    let eid = server.add_entry_ret(10, "http://127.0.0.1:8765/post/1", "t", false, false);
    db::set_article_remote_id(&conn, aid, eid).unwrap();
    db::set_read(&conn, aid, true).unwrap();
    db::enqueue_sync(&conn, Some(aid), None, "read", None).unwrap();
    // 注入：remote_dup_ids 读取失败（BLOB → String 读取错误）
    conn.execute(
        "UPDATE articles SET remote_dup_ids = x'00' WHERE id = ?1",
        [aid],
    )
    .unwrap();

    let dbm = Arc::new(Mutex::new(conn));
    let http = app_lib::ingestion::build_client(10);
    // push_states_now 失败静默（日志）但**不得**清队列：错误路径必须在计划阶段中止。
    sync::push_states_now(&dbm, &http).await;

    let conn = dbm.lock().await;
    let queued: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sync_queue WHERE action = 'read'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        queued, 1,
        "计划阶段失败必须保留 read 队项（不得 prune 原意图）"
    );
    drop(conn);
    let updates = mock_greader::status_updates_map(&server);
    assert!(
        updates.is_empty(),
        "计划失败不得发出任何部分推送: {updates:?}"
    );
    let _ = std::fs::remove_file(&tmp);
}
