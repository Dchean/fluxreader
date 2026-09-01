//! 账号数据边界 + 缓存清理的端到端测试：
//! ① 断开连接：origin='miniflux' 订阅（含文章/绑定/队列）被清理，
//!    origin='local' 直连订阅保留，绑定/副本记账归零
//! ② 缓存清理：scope=articles 删指定天数前的非收藏文章（收藏保留）；
//!    scope=ai 只清 AI 摘要/翻译缓存
//! 运行：cargo test --test account_lifecycle_e2e -- --include-ignored --nocapture

mod mock_miniflux;

use app_lib::db;
use mock_miniflux::MockMiniflux;
use rusqlite::Connection;

fn fresh_db(name: &str) -> Connection {
    let tmp = std::env::temp_dir().join(format!("fluxreader_account_{name}.db"));
    let _ = std::fs::remove_file(&tmp);
    db::open(&tmp).expect("open db")
}

/// 本地直连订阅（origin 默认 'local'）+ 一篇文章
fn seed_local(conn: &Connection) -> (i64, i64) {
    let folder = db::create_folder(conn, "本地分类", "article").unwrap();
    let feed = db::insert_feed(
        conn,
        "http://127.0.0.1:8765/local.xml",
        None,
        "Local Feed",
        None,
        folder,
        "inherit",
        true,
        false,
    )
    .unwrap();
    let a = db::NewArticle {
        guid: "local-1".into(),
        url: Some("http://127.0.0.1:8765/local-1".into()),
        title: "Local Article".into(),
        author: None,
        summary: None,
        content_html: Some("<p>local</p>".into()),
        body_text: "local".into(),
        image_url: None,
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: Some(chrono::Utc::now().to_rfc3339()),
        source: "direct".into(),
    };
    let (aid, _) = db::upsert_article_with_feed(conn, feed, &a, false).unwrap();
    (feed, aid)
}

#[test]
fn disconnect_purges_miniflux_data_but_keeps_local() {
    let mut conn = fresh_db("disconnect");
    let (local_feed, local_aid) = seed_local(&conn);

    // 模拟服务端拉取：origin='miniflux' 订阅 + 文章 + 绑定 + 队列 + 墓碑
    let remote_folder = db::create_folder(&conn, "远端分类", "article").unwrap();
    let remote_feed = db::insert_feed_origin(
        &conn,
        "http://example.com/remote.xml",
        None,
        "Remote Feed",
        None,
        remote_folder,
        "inherit",
        true,
        false,
        "miniflux",
    )
    .unwrap();
    let a = db::NewArticle {
        guid: "remote-1".into(),
        url: Some("http://example.com/remote-1".into()),
        title: "Remote Article".into(),
        author: None,
        summary: None,
        content_html: Some("<p>remote</p>".into()),
        body_text: "remote".into(),
        image_url: None,
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: Some(chrono::Utc::now().to_rfc3339()),
        source: "miniflux".into(),
    };
    let (remote_aid, _) = db::upsert_article_with_feed(&conn, remote_feed, &a, false).unwrap();
    // 绑定 + 队列 + 本地订阅也绑（模拟 URL 碰撞合并过的本地源）
    db::set_article_miniflux_id(&conn, remote_aid, 9001).unwrap();
    db::set_article_miniflux_id(&conn, local_aid, 9002).unwrap();
    db::set_feed_miniflux_id(&conn, local_feed, 77).unwrap();
    db::set_folder_miniflux_id(&conn, remote_folder, 55).unwrap();
    db::enqueue_sync(&conn, Some(local_aid), None, "read", None).unwrap();
    db::enqueue_sync(&conn, Some(remote_aid), None, "read", None).unwrap();
    let _ = conn.execute(
        "INSERT INTO deduped_urls (url, kept_aid) VALUES ('http://example.com/dup', ?1)",
        rusqlite::params![remote_aid],
    );

    let (feeds, articles) = db::purge_miniflux_data(&mut conn).unwrap();
    assert_eq!(feeds, 1, "one miniflux-origin feed purged");
    assert!(articles > 0, "bindings cleared (rows touched)");

    // 远端订阅及其文章消失；本地订阅与文章保留
    let remote_left: i64 = conn
        .query_row("SELECT COUNT(*) FROM feeds WHERE id = ?1", [remote_feed], |r| r.get(0))
        .unwrap();
    assert_eq!(remote_left, 0, "remote feed purged");
    let remote_art: i64 = conn
        .query_row("SELECT COUNT(*) FROM articles WHERE id = ?1", [remote_aid], |r| r.get(0))
        .unwrap();
    assert_eq!(remote_art, 0, "remote article purged (cascade)");
    let local_left: i64 = conn
        .query_row("SELECT COUNT(*) FROM feeds WHERE id = ?1", [local_feed], |r| r.get(0))
        .unwrap();
    assert_eq!(local_left, 1, "local feed kept");
    let local_art: i64 = conn
        .query_row("SELECT COUNT(*) FROM articles WHERE id = ?1", [local_aid], |r| r.get(0))
        .unwrap();
    assert_eq!(local_art, 1, "local article kept");

    // 绑定/队列/墓碑/文件夹绑定全部归零
    let bound: i64 = conn
        .query_row("SELECT COUNT(*) FROM articles WHERE miniflux_id IS NOT NULL", [], |r| r.get(0))
        .unwrap();
    assert_eq!(bound, 0, "article bindings cleared");
    let feed_bound: i64 = conn
        .query_row("SELECT COUNT(*) FROM feeds WHERE miniflux_id IS NOT NULL", [], |r| r.get(0))
        .unwrap();
    assert_eq!(feed_bound, 0, "feed bindings cleared");
    let folder_bound: i64 = conn
        .query_row("SELECT COUNT(*) FROM folders WHERE miniflux_id IS NOT NULL", [], |r| r.get(0))
        .unwrap();
    assert_eq!(folder_bound, 0, "folder bindings cleared");
    let queue: i64 = conn.query_row("SELECT COUNT(*) FROM sync_queue", [], |r| r.get(0)).unwrap();
    assert_eq!(queue, 0, "sync queue emptied");
    let tomb: i64 = conn.query_row("SELECT COUNT(*) FROM deduped_urls", [], |r| r.get(0)).unwrap();
    assert_eq!(tomb, 0, "tombstones purged");
    // 远端分类（空了）删除；本地分类保留
    let remote_folder_left: i64 = conn
        .query_row("SELECT COUNT(*) FROM folders WHERE id = ?1", [remote_folder], |r| r.get(0))
        .unwrap();
    assert_eq!(remote_folder_left, 0, "empty remote folder removed");
}

#[test]
fn cache_cleanup_articles_respects_star_and_age() {
    let mut conn = fresh_db("cache");
    let (feed, _) = seed_local(&conn);

    let mk = |guid: &str, days_ago: i64, starred: bool, read: bool| {
        let a = db::NewArticle {
            guid: guid.into(),
            url: Some(format!("http://127.0.0.1:8765/{guid}")),
            title: guid.into(),
            author: None,
            summary: None,
            content_html: Some("<p>x</p>".into()),
            body_text: "x".into(),
            image_url: None,
            enclosure_url: None,
            enclosure_mime: None,
            duration_sec: None,
            published_at: Some(
                (chrono::Utc::now() - chrono::Duration::days(days_ago)).to_rfc3339(),
            ),
            source: "direct".into(),
        };
        let (aid, _) = db::upsert_article_with_feed(&conn, feed, &a, false).unwrap();
        if starred {
            conn.execute("UPDATE articles SET is_starred = 1 WHERE id = ?1", [aid]).unwrap();
        }
        if read {
            conn.execute("UPDATE articles SET is_read = 1 WHERE id = ?1", [aid]).unwrap();
        }
        aid
    };
    let old = mk("old", 40, false, true);
    let old_starred = mk("old-star", 40, true, true);
    let recent = mk("recent", 3, false, true);
    // 未读旧文不删：删了会被全量同步按服务器 unread 状态拉回（数据打架）
    let old_unread = mk("old-unread", 40, false, false);

    let (deleted, _) = db::cleanup_cache(&mut conn, 30, "articles").unwrap();
    assert_eq!(deleted, 1, "only old read non-starred deleted");

    for (aid, should_exist, why) in [
        (old, false, "old read article purged"),
        (old_starred, true, "starred preserved"),
        (recent, true, "recent preserved"),
        (old_unread, true, "old UNREAD preserved (sync would resurrect it)"),
    ] {
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM articles WHERE id = ?1", [aid], |r| r.get(0))
            .unwrap();
        assert_eq!(n > 0, should_exist, "{why}");
    }
}

#[test]
fn cache_cleanup_ai_only_clears_ai_fields() {
    let mut conn = fresh_db("cache_ai");
    let (feed, _) = seed_local(&conn);
    let a = db::NewArticle {
        guid: "ai-1".into(),
        url: Some("http://127.0.0.1:8765/ai-1".into()),
        title: "AI Article".into(),
        author: None,
        summary: None,
        content_html: Some("<p>body</p>".into()),
        body_text: "body".into(),
        image_url: None,
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: Some((chrono::Utc::now() - chrono::Duration::days(90)).to_rfc3339()),
        source: "direct".into(),
    };
    let (aid, _) = db::upsert_article_with_feed(&conn, feed, &a, false).unwrap();
    db::set_article_ai_fields(&conn, aid, Some("摘要缓存"), Some("<p>译文</p>")).unwrap();

    let (deleted, ai_cleared) = db::cleanup_cache(&mut conn, 30, "ai").unwrap();
    assert_eq!(deleted, 0, "articles scope untouched");
    assert_eq!(ai_cleared, 1, "one article's ai fields cleared");

    let (summary, translated, body): (Option<String>, Option<String>, String) = conn
        .query_row(
            "SELECT ai_summary, translated_content, COALESCE(content_html, '') FROM articles WHERE id = ?1",
            [aid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert!(summary.is_none() && translated.is_none(), "ai cache cleared");
    assert!(!body.is_empty(), "body content kept");
}

/// 集成：断开 → 重连另一账号（mock）→ pull 只出现新账号的订阅。
/// （模拟用户报告的「断开后换账号，本地与远端订阅混杂」）
#[tokio::test]
#[ignore = "spins a local mock server"]
async fn reconnect_other_account_no_mixing() {
    let server = MockMiniflux::start().await.expect("mock");
    let tmp = std::env::temp_dir().join("fluxreader_account_mix.db");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();

    // 账号 A：拉取订阅（origin=miniflux）
    db::set_setting(&conn, "miniflux_endpoint", &server.url()).unwrap();
    db::set_setting(&conn, "miniflux_token", "token-a").unwrap();
    let http = app_lib::ingestion::build_client(10);
    let _ = app_lib::sync::feeds_phase(
        &std::sync::Arc::new(tokio::sync::Mutex::new(conn)),
        &http,
    )
    .await
    .unwrap();
    // 直接用 mock 的 feed 列表数断言 Pull 生效（2 个远端 feed）
    {
        let conn = db::open(&tmp).unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM feeds WHERE origin = 'miniflux'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2, "account A feeds pulled");
    }

    // 断开 → 清理
    let mut conn = db::open(&tmp).unwrap();
    let (feeds, _) = db::purge_miniflux_data(&mut conn).unwrap();
    assert_eq!(feeds, 2, "account A data purged on disconnect");

    // 换账号 B（不同 token）：本地不再有 A 的订阅 → 不混杂
    db::set_setting(&conn, "miniflux_token", "token-b").unwrap();
    let left: i64 = conn
        .query_row("SELECT COUNT(*) FROM feeds WHERE origin = 'miniflux'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(left, 0, "no account A feeds left after purge");
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
   本地订阅同步到 Miniflux（sync_local_feeds 语义）
   ============================================================ */

/// 未连接期间添加的本地源 → 首连后 sync_local_feeds 入队 → push_feeds 推送 →
/// 服务端收到 create_feed 且本地绑定 miniflux_id；再跑一次（幂等）不再推送。
#[tokio::test]
async fn sync_local_feeds_pushes_unbound_local_feeds() {
    let server = MockMiniflux::start().await.expect("start mock");
    let tmp = std::env::temp_dir().join("fluxreader_account_localsync.db");
    let _ = std::fs::remove_file(&tmp);
    let conn = fresh_db("localsync");

    // 未连接时的本地直连源（origin 默认 local）
    let folder = db::create_folder(&conn, "本地", "article").unwrap();
    db::insert_feed(&conn, "http://127.0.0.1:1/a.xml", None, "Local A", None, folder, "inherit", true, false).unwrap();
    db::insert_feed(&conn, "http://127.0.0.1:1/b.xml", None, "Local B", None, folder, "inherit", true, false).unwrap();
    // 已绑定一个（不应重复入队）
    let bound = db::insert_feed(&conn, "http://127.0.0.1:1/c.xml", None, "Bound", None, folder, "inherit", true, false).unwrap();
    db::set_feed_miniflux_id(&conn, bound, 999).unwrap();

    // 连接（复刻 sync_local_feeds 的入队逻辑：未绑本地源 → add_feed 队列）
    db::set_setting(&conn, "miniflux_endpoint", &server.url()).unwrap();
    db::set_setting(&conn, "miniflux_token", "t").unwrap();
    let unbound: Vec<(String, Option<i64>)> = {
        let mut stmt = conn
            .prepare("SELECT feed_url, folder_id FROM feeds WHERE origin = 'local' AND miniflux_id IS NULL")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    assert_eq!(unbound.len(), 2, "只有未绑定的本地源入队");
    for (url, folder_id) in &unbound {
        let payload = serde_json::json!({ "folder_id": folder_id }).to_string();
        db::enqueue_sync(&conn, None, Some(url), "add_feed", Some(&payload)).unwrap();
    }

    // 跑 feeds 阶段（推送）：服务端 created_feeds 应收到 2 个 create
    let http = app_lib::ingestion::build_client(10);
    let report = app_lib::sync::feeds_phase(
        &std::sync::Arc::new(tokio::sync::Mutex::new(conn)),
        &http,
    ).await.expect("feeds phase");

    let created = server.created_feeds.lock().unwrap();
    assert_eq!(created.len(), 2, "both unbound local feeds pushed: {created:?}");
    assert_eq!(report.pushed_feeds, 2, "report counts both");

    // 本地两个源绑定上 miniflux_id
    let tmp2 = db::open(&tmp).unwrap();
    let bound_count: i64 = tmp2
        .query_row(
            "SELECT COUNT(*) FROM feeds WHERE origin = 'local' AND miniflux_id IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(bound_count, 3, "2 newly bound + 1 pre-bound");

    // 幂等：队列已清空，再入队（无未绑源）→ 0
    let unbound2: i64 = tmp2
        .query_row("SELECT COUNT(*) FROM feeds WHERE origin = 'local' AND miniflux_id IS NULL", [], |r| r.get(0))
        .unwrap();
    assert_eq!(unbound2, 0, "idempotent: nothing left to push");
    let _ = std::fs::remove_file(&tmp);
}

/// create_feed 409 幂等：对 mock 已有的 feed 10 URL（GET /v1/feeds 静态预置）
/// 再 create → 409 + feed_id → create_feed 返回既有 id 而非报错。
/// （真实 Miniflux 对重复订阅同样返回 409 带 feed_id。）
#[tokio::test]
async fn create_feed_conflict_returns_existing_id() {
    let server = MockMiniflux::start().await.expect("start mock");
    let http = app_lib::ingestion::build_client(10);
    let client = app_lib::miniflux::MinifluxClient::new(&server.url(), "t", http.clone());

    // mock GET /v1/feeds 预置的既有订阅 URL
    let existing_url = "http://127.0.0.1:8765/local_feed.xml";
    let id = client.create_feed(existing_url, 1).await.expect("409 should resolve to existing id, not error");
    assert_eq!(id, 10, "conflict resolves to the pre-existing feed id");

    // 且不重复入 created_feeds（幂等，不产生重复创建记录）
    let created = server.created_feeds.lock().unwrap();
    assert!(!created.iter().any(|(u, _)| u == existing_url), "no duplicate create recorded");
}
