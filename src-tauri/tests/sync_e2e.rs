//! Miniflux 同步引擎端到端测试：mock 服务端 + 临时数据库。
//! 覆盖：连接测试、URL 碰撞合并（§4.4）、远端订阅拉取、状态推送（已读/收藏）、
//! 直连失败源的兜底拉取（source='miniflux'）。
//! 运行：cargo test --test sync_e2e -- --ignored --nocapture

mod mock_miniflux;

use app_lib::db;
use app_lib::sync;
use mock_miniflux::MockMiniflux;

#[tokio::test]
#[ignore = "spins a local mock server"]
async fn miniflux_sync_end_to_end() {
    let server = MockMiniflux::start().await.expect("start mock server");

    let tmp = std::env::temp_dir().join("fluxreader_sync_e2e.db");
    let _ = std::fs::remove_file(&tmp);
    let mut conn = db::open(&tmp).expect("open db");

    // ---------- 场景准备：本地状态 ----------
    // 本地分类 + 一个直连添加的 feed（URL 与远端 feed 10 碰撞）
    let folder_id = db::create_folder(&conn, "技术开发", "article").unwrap();
    let local_feed_id = db::insert_feed(
        &conn,
        "http://127.0.0.1:8765/local_feed.xml", // 与 mock 的 feed 10 同 URL
        None,
        "Local Direct Feed",
        None,
        folder_id,
        "inherit",
        true,
        false,
    )
    .unwrap();
    // 本地条目（直连抓取产物，miniflux_id 未绑定）
    let local_entry = db::NewArticle {
        guid: "guid-local-1".into(),
        url: Some("http://127.0.0.1:8765/post/1".into()),
        title: "Local Article".into(),
        author: Some("Local Author".into()),
        summary: None,
        content_html: Some("<p>local content</p>".into()),
        body_text: "local content".into(),
        image_url: None,
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: Some(chrono::Utc::now().to_rfc3339()),
        source: "direct".into(),
    };
    let (local_article_id, _) = db::upsert_article_with_feed(&conn, local_feed_id, &local_entry, false).unwrap();

    // 远端同 URL 条目（已读 + 收藏状态 —— Miniflux 是状态权威）
    server.add_entry(10, "http://127.0.0.1:8765/post/1", "Remote version of same article", "read", true);
    // 远端另一条目（本地没有 —— 走兜底路径不涉及，状态 Pull 也不该建新条目，因为无 URL 匹配）
    server.add_entry(11, "http://example.com/only-remote", "Remote only article", "unread", false);

    // ---------- ① 连接 + 全量同步 ----------
    db::set_setting(&conn, "miniflux_endpoint", &server.url()).unwrap();
    db::set_setting(&conn, "miniflux_token", "test-token").unwrap();

    let http = app_lib::ingestion::build_client(10);
    let report = sync::sync_now(&mut conn, &http).await.expect("sync should succeed");
    println!("sync report: pushed_states={} pushed_feeds={} pulled_feeds={} pulled_entries={} merged={} fallback={}",
        report.pushed_states, report.pushed_feeds, report.pulled_feeds, report.pulled_entries, report.merged_states, report.fallback_entries);

    // URL 碰撞合并：本地 feed 绑定了远端 feed id 10
    let bound: Option<i64> = conn
        .query_row("SELECT miniflux_id FROM feeds WHERE id = ?1", [local_feed_id], |r| r.get(0))
        .ok()
        .flatten();
    assert_eq!(bound, Some(10), "local feed must bind remote feed id 10");

    // 远端独有 feed 拉到本地（挂在远端分类对应的本地 folder）
    let remote_only: Option<(i64, i64)> = conn
        .query_row(
            "SELECT id, miniflux_id FROM feeds WHERE feed_url = 'http://example.com/remote-only.xml'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    assert!(remote_only.is_some(), "remote-only feed pulled to local");
    assert_eq!(remote_only.unwrap().1, 11);

    // 远端分类建到本地
    let remote_cat: Option<i64> = conn
        .query_row("SELECT id FROM folders WHERE name = 'Remote Cat'", [], |r| r.get(0))
        .ok();
    assert!(remote_cat.is_some(), "remote category created locally");

    // 本地条目与远端条目 URL 匹配 → 状态合并（Miniflux 权威：已读+收藏）
    let merged: (bool, bool, Option<i64>) = conn
        .query_row(
            "SELECT is_read, is_starred, miniflux_id FROM articles WHERE id = ?1",
            [local_article_id],
            |r| Ok((r.get::<_, i64>(0)? != 0, r.get::<_, i64>(1)? != 0, r.get(2)?)),
        )
        .unwrap();
    assert!(merged.0, "read state pulled from miniflux");
    assert!(merged.1, "starred state pulled from miniflux");
    assert!(merged.2.is_some(), "article bound to miniflux entry id");

    // ---------- ② 状态推送：本地改未读 → push 到远端 ----------
    db::set_read(&conn, local_article_id, false).unwrap();
    db::enqueue_sync(&conn, Some(local_article_id), None, "unread", None).unwrap();
    // 收藏切换
    db::set_starred(&conn, local_article_id, false).unwrap();
    db::enqueue_sync(&conn, Some(local_article_id), None, "unstar", None).unwrap();

    let report2 = sync::sync_now(&mut conn, &http).await.expect("second sync");
    println!("push report: pushed_states={}", report2.pushed_states);

    // 远端收到 unread 状态更新（entry id 即绑定的 miniflux_id）
    let mf_id = merged.2.unwrap();
    let updates = mock_miniflux::status_updates_map(&server);
    assert_eq!(
        updates.get(&mf_id).map(|s| s.as_str()),
        Some("unread"),
        "unread status must be pushed to remote"
    );
    // 收藏取消推送到远端
    assert!(
        server.bookmark_toggles.lock().unwrap().contains(&mf_id),
        "bookmark toggle must be pushed"
    );

    // ---------- ③ 兜底：直连失败的源从 Miniflux 拉条目 ----------
    // 把 local_feed 标记为直连失败 + 绑定远端 feed，远端加一条本地没有的条目
    db::set_feed_fetch_state(&conn, local_feed_id, true, Some("connection refused"), None, None).unwrap();
    server.add_entry(10, "http://127.0.0.1:8765/new-fallback-entry", "Fallback Entry From Miniflux", "unread", false);

    let report3 = sync::sync_now(&mut conn, &http).await.expect("third sync");
    println!("fallback report: fallback_entries={}", report3.fallback_entries);

    let fallback: Option<(String, String)> = conn
        .query_row(
            "SELECT title, source FROM articles WHERE url = 'http://127.0.0.1:8765/new-fallback-entry'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let fb = fallback.expect("fallback entry ingested from miniflux");
    assert_eq!(fb.1, "miniflux", "fallback entry source must be 'miniflux'");
    assert!(fb.0.contains("Fallback"));

    // 队列清空
    let queue_left = db::take_sync_queue(&conn).unwrap().len();
    assert_eq!(queue_left, 0, "sync queue must be drained after successful push");

    // ---------- ④ 复活防护：跨源同 URL entry 不得覆盖本地状态 ----------
    // 场景：本地文章（feed 10 的 entry）已读；服务端另一源（feed 11）
    // 也有同 URL 的 entry 且未读。同步后本地必须仍是已读——
    // 跨源 entry 无权写状态（旧版会按 URL 兜底把未读覆盖回来）
    // 注意：② 推过 unread（mock 真实回写），own entry 现在服务端是 unread；
    // 未读合并只认绑定 entry 是合法语义，所以先把 own 恢复 read（手机读过）
    conn.execute("UPDATE articles SET is_read = 1 WHERE id = ?1", [local_article_id]).unwrap();
    {
        let mut es = server.entries.lock().unwrap();
        if let Some(e) = es.iter_mut().find(|e| e.id == mf_id) {
            e.status = "read".into();
        }
    }
    // 模拟跨源同 URL entry（feed 11 = remote-only feed，未读态）
    let cross_url = "http://127.0.0.1:8765/post/1"; // 与本地文章同 URL
    server.add_entry(11, cross_url, "Cross-feed duplicate of read article", "unread", false);

    let _report4 = sync::sync_now(&mut conn, &http).await.expect("fourth sync (cross-feed guard)");

    let (still_read, binding): (bool, i64) = conn
        .query_row(
            "SELECT is_read, COALESCE(miniflux_id, -1) FROM articles WHERE id = ?1",
            [local_article_id],
            |r| Ok((r.get::<_, i64>(0)? != 0, r.get(1)?)),
        )
        .unwrap();
    assert!(still_read, "cross-feed unread entry must NOT resurrect the read article");
    assert_eq!(binding, mf_id, "binding must stay on the original (own-feed) entry");

    // 同源 entry（feed 10）状态变化仍正常合并（防护不影响正常路径）
    // —— 通过 mock 更新既有 entry 状态为 unread 再同步，本地应跟随
    {
        let mut es = server.entries.lock().unwrap();
        if let Some(e) = es.iter_mut().find(|e| e.id == mf_id) {
            e.status = "unread".into();
        }
    }
    let _report5 = sync::sync_now(&mut conn, &http).await.expect("fifth sync (same-feed still merges)");
    let now_read: bool = conn
        .query_row("SELECT is_read FROM articles WHERE id = ?1", [local_article_id], |r| r.get::<_, i64>(0).map(|v| v != 0))
        .unwrap();
    assert!(!now_read, "own-feed entry status change still merges (guard doesn't break normal path)");

    // 连接测试
    let msg = sync::test_connection(&server.url(), "test-token", &http).await.unwrap();
    assert!(msg.contains("mockuser"), "test_connection returns username: {msg}");

    let _ = std::fs::remove_file(&tmp);
    println!("=== SYNC E2E PASS ===");
}
