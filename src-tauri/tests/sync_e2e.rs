//! Miniflux 同步引擎端到端测试：mock 服务端 + 临时数据库。
//! 覆盖：连接测试、URL 碰撞合并（§4.4）、远端订阅拉取、状态推送（已读/收藏）、
//! 远端新条目经 pull 落库为 source='miniflux'。
//! 注（TASK-070）：本条原先写作「直连失败源的兜底拉取」——该因果不成立：
//! 条目落库为 source='miniflux' 取决于远端源/条目的匹配与绑定，而不是把源标记为
//! 抓取失败（审查者已用变异证实：删掉失败标记调用，本套件仍全绿且条目照常入库）。
//! 另注：feeds.fetch_failed 在 Rust 侧没有读取方；抓取退避由 fail_count /
//! next_retry_at 承担（db/feeds.rs 的 set_feed_fetch_state 写入、
//! feeds_due_for_refresh 按 next_retry_at 过滤），该列现存用途是下发前端做失败标记。
//! 运行：cargo test --test sync_e2e -- --ignored --nocapture

mod mock_greader;

use app_lib::db;
use app_lib::sync;
use mock_greader::MockGReader;

#[tokio::test]
async fn miniflux_sync_end_to_end() {
    let server = MockGReader::start().await.expect("start mock server");

    let tmp = std::env::temp_dir().join(format!(
        "fluxreader_sync_e2e_{}_{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).expect("open db");
    let db = std::sync::Arc::new(tokio::sync::Mutex::new(conn));
    let conn = db.lock().await;

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
    // 本地条目（直连抓取产物，remote_id 未绑定）
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
    let (local_article_id, _) =
        db::upsert_article_with_feed(&conn, local_feed_id, &local_entry, false).unwrap();

    // 远端同 URL 条目（已读 + 收藏状态 —— Miniflux 是状态权威）
    server.add_entry(
        10,
        "http://127.0.0.1:8765/post/1",
        "Remote version of same article",
        "read",
        true,
    );
    // 远端另一条目（属于远端 feed 11，本地尚无对应源）。
    // TASK-070：此处原写「走兜底路径不涉及」，把本条与 Miniflux 兜底拉取挂钩——
    // 该因果不成立（见文件头注）。本条实际由 pull 阶段按「远端条目 + 同轮拉到的
    // 远端源」入库为 source='miniflux'；本文件后续断言即以此为据。
    server.add_entry(
        11,
        "http://example.com/only-remote",
        "Remote only article",
        "unread",
        false,
    );

    // ---------- ① 连接 + 全量同步 ----------
    db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
    db::set_setting(&conn, "greader_username", "test").unwrap();
    db::set_setting(&conn, "greader_password", "test-token").unwrap();

    let http = app_lib::ingestion::build_client(10);
    drop(conn);
    let report = sync::sync_now(&db, &http)
        .await
        .expect("sync should succeed");
    let conn = db.lock().await;
    println!("sync report: pushed_states={} pushed_feeds={} pulled_feeds={} pulled_entries={} merged={}",
        report.pushed_states, report.pushed_feeds, report.pulled_feeds, report.pulled_entries, report.merged_states);

    // URL 碰撞合并：本地 feed 绑定了远端 feed id 10
    let bound: Option<i64> = conn
        .query_row(
            "SELECT remote_id FROM feeds WHERE id = ?1",
            [local_feed_id],
            |r| r.get(0),
        )
        .ok()
        .flatten();
    assert_eq!(bound, Some(10), "local feed must bind remote feed id 10");

    // 远端独有 feed 拉到本地（挂在远端分类对应的本地 folder）
    let remote_only: Option<(i64, i64)> = conn
        .query_row(
            "SELECT id, remote_id FROM feeds WHERE feed_url = 'http://example.com/remote-only.xml'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    assert!(remote_only.is_some(), "remote-only feed pulled to local");
    assert_eq!(remote_only.unwrap().1, 11);

    // 远端分类建到本地
    let remote_cat: Option<i64> = conn
        .query_row(
            "SELECT id FROM folders WHERE name = 'Remote Cat'",
            [],
            |r| r.get(0),
        )
        .ok();
    assert!(remote_cat.is_some(), "remote category created locally");

    // 本地条目与远端条目 URL 匹配 → 状态合并（Miniflux 权威：已读+收藏）
    let merged: (bool, bool, Option<i64>) = conn
        .query_row(
            "SELECT is_read, is_starred, remote_id FROM articles WHERE id = ?1",
            [local_article_id],
            |r| {
                Ok((
                    r.get::<_, i64>(0)? != 0,
                    r.get::<_, i64>(1)? != 0,
                    r.get(2)?,
                ))
            },
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

    drop(conn);
    let report2 = sync::sync_now(&db, &http).await.expect("second sync");
    let conn = db.lock().await;
    println!("push report: pushed_states={}", report2.pushed_states);

    // 远端收到 unread 状态更新（entry id 即绑定的 remote_id）
    let mf_id = merged.2.unwrap();
    let updates = mock_greader::status_updates_map(&server);
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

    // ---------- ③ 已绑定源的远端新条目经 pull 落库（source='miniflux'） ----------
    // 把 local_feed 标记为直连失败（贴近真实场景）+ 远端加一条本地没有的条目。
    // 注意：触发落库的是「该源已绑定 remote_id」，不是失败标记本身（见文件头说明）。
    db::set_feed_fetch_state(
        &conn,
        local_feed_id,
        true,
        Some("connection refused"),
        None,
        None,
    )
    .unwrap();
    server.add_entry(
        10,
        "http://127.0.0.1:8765/new-fallback-entry",
        "Fallback Entry From Miniflux",
        "unread",
        false,
    );
    drop(conn);

    let report3 = sync::sync_now(&db, &http).await.expect("third sync");
    let conn = db.lock().await;
    println!(
        "third sync report: pulled_entries={}",
        report3.pulled_entries
    );

    let ingested: Option<(String, String)> = conn
        .query_row(
            "SELECT title, source FROM articles WHERE url = 'http://127.0.0.1:8765/new-fallback-entry'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let fb = ingested.expect("remote entry on a bound feed must be ingested");
    assert_eq!(
        fb.1, "miniflux",
        "bound-feed remote entry must be stored with source='miniflux'"
    );
    assert!(fb.0.contains("Fallback"));

    // 队列清空
    let queue_left = db::take_sync_queue(&conn).unwrap().len();
    assert_eq!(
        queue_left, 0,
        "sync queue must be drained after successful push"
    );

    // ---------- ④ 复活防护：跨源同 URL entry 不得覆盖本地状态 ----------
    // 场景：本地文章（feed 10 的 entry）已读；服务端另一源（feed 11）
    // 也有同 URL 的 entry 且未读。同步后本地必须仍是已读——
    // 跨源 entry 无权写状态（旧版会按 URL 兜底把未读覆盖回来）
    // 注：此处的「兜底」指 URL 匹配兜底，与 Miniflux 兜底拉取无关
    // 注意：② 推过 unread（mock 真实回写），own entry 现在服务端是 unread；
    // 未读合并只认绑定 entry 是合法语义，所以先把 own 恢复 read（手机读过）
    conn.execute(
        "UPDATE articles SET is_read = 1 WHERE id = ?1",
        [local_article_id],
    )
    .unwrap();
    drop(conn);
    {
        let mut es = server.entries.lock().unwrap();
        if let Some(e) = es.iter_mut().find(|e| e.id == mf_id) {
            e.read = true;
        }
    }
    // 模拟跨源同 URL entry（feed 11 = remote-only feed，未读态）
    let cross_url = "http://127.0.0.1:8765/post/1"; // 与本地文章同 URL
    server.add_entry(
        11,
        cross_url,
        "Cross-feed duplicate of read article",
        "unread",
        false,
    );

    let _report4 = sync::sync_now(&db, &http)
        .await
        .expect("fourth sync (cross-feed guard)");
    let conn = db.lock().await;

    let (still_read, binding): (bool, i64) = conn
        .query_row(
            "SELECT is_read, COALESCE(remote_id, -1) FROM articles WHERE id = ?1",
            [local_article_id],
            |r| Ok((r.get::<_, i64>(0)? != 0, r.get(1)?)),
        )
        .unwrap();
    assert!(
        still_read,
        "cross-feed unread entry must NOT resurrect the read article"
    );
    assert_eq!(
        binding, mf_id,
        "binding must stay on the original (own-feed) entry"
    );

    // 同源 entry（feed 10）状态变化仍正常合并（防护不影响正常路径）
    // —— 通过 mock 更新既有 entry 状态为 unread 再同步，本地应跟随
    {
        let mut es = server.entries.lock().unwrap();
        if let Some(e) = es.iter_mut().find(|e| e.id == mf_id) {
            e.read = false;
        }
    }
    drop(conn);
    let _report5 = sync::sync_now(&db, &http)
        .await
        .expect("fifth sync (same-feed still merges)");
    let conn = db.lock().await;
    let now_read: bool = conn
        .query_row(
            "SELECT is_read FROM articles WHERE id = ?1",
            [local_article_id],
            |r| r.get::<_, i64>(0).map(|v| v != 0),
        )
        .unwrap();
    assert!(
        !now_read,
        "own-feed entry status change still merges (guard doesn't break normal path)"
    );

    // 连接测试
    let (msg, username, resolved_base) =
        sync::test_connection("greader", &server.url(), "mockuser", "mockpass", &http)
            .await
            .unwrap();
    assert!(
        msg.contains("mockuser"),
        "test_connection returns username: {msg}"
    );
    assert_eq!(username, "mockuser");
    assert_eq!(
        resolved_base,
        server.url(),
        "Miniflux 形态下解析结果应就是站点根（首个候选命中）"
    );

    let _ = std::fs::remove_file(&tmp);
    println!("=== SYNC E2E PASS ===");
}

/* ============================================================
P3-11（TASK-074，DEC-req104-p3-11-remote-unsub-20260920）：
远端退订 → 本地同步删除（含防误删与 pending 保护）

修复前：pull_feeds 只有 upsert、没有删除分支，远端退掉的订阅在本地永久残留。
修复后：满足「origin='remote' 且 remote_id 已绑定」且「规范化 URL 不在本轮远端
订阅列表」且「队列里没有该 URL 的未推送变更」三条的源，才在本地删除。
本地直连源（origin='local'）与未绑定源一律保留。
=========================================================== */

async fn setup_remote_unsub(name: &str) -> (
    std::sync::Arc<tokio::sync::Mutex<rusqlite::Connection>>,
    reqwest::Client,
    std::sync::Arc<MockGReader>,
) {
    let server = MockGReader::start().await.expect("start mock server");
    let tmp = std::env::temp_dir().join(format!(
        "fluxreader_unsub_{}_{}_{}.db",
        name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).expect("open db");
    db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
    db::set_setting(&conn, "greader_username", "test").unwrap();
    db::set_setting(&conn, "greader_password", "test-token").unwrap();
    let http = app_lib::ingestion::build_client(10);
    (
        std::sync::Arc::new(tokio::sync::Mutex::new(conn)),
        http,
        server,
    )
}

fn feed_id_by_url(conn: &rusqlite::Connection, url: &str) -> Option<i64> {
    conn.query_row(
        "SELECT id FROM feeds WHERE feed_url = ?1",
        rusqlite::params![url],
        |r| r.get(0),
    )
    .ok()
}

fn feed_origin(conn: &rusqlite::Connection, id: i64) -> Option<String> {
    conn.query_row("SELECT origin FROM feeds WHERE id = ?1", [id], |r| r.get(0))
        .ok()
}

/// B① 远端退订的「服务端来源」订阅必须在本地删除，且下一轮 pull 不复活。
#[tokio::test]
async fn remote_unsubscribe_removes_local_remote_feed() {
    let (db, http, server) = setup_remote_unsub("removes").await;

    // 第一轮：把远端 feed 11（remote-only）拉到本地 → origin='remote' + 绑定 remote_id
    sync::feeds_phase(&db, &http).await.expect("first pull");
    let feed_id = {
        let conn = db.lock().await;
        let id = feed_id_by_url(&conn, "http://example.com/remote-only.xml")
            .expect("remote-only feed pulled to local");
        assert_eq!(
            feed_origin(&conn, id).as_deref(),
            Some("remote"),
            "pulled remote feed must be origin='remote'"
        );
        id
    };

    // 服务端退订 feed 11
    server.remove_remote_subscription("feed/11");
    assert!(
        !server.remote_subscription_ids().contains(&"feed/11".to_string()),
        "remote no longer lists feed 11"
    );

    // 第二轮：本地必须同步删除（修复前：残留）
    let report = sync::feeds_phase(&db, &http).await.expect("second pull");
    {
        let conn = db.lock().await;
        assert!(
            feed_id_by_url(&conn, "http://example.com/remote-only.xml").is_none(),
            "remote-unsubscribed feed must be deleted locally (was id {feed_id})"
        );
    }
    assert_eq!(
        report.removed_feeds, 1,
        "report must count the locally removed subscription"
    );

    // 第三轮：不应复活（无墓碑也能保持删除，因为远端确实不再列出）
    sync::feeds_phase(&db, &http).await.expect("third pull");
    {
        let conn = db.lock().await;
        assert!(
            feed_id_by_url(&conn, "http://example.com/remote-only.xml").is_none(),
            "removed feed must not come back on the next pull"
        );
    }
}

/// B② 本地直连源（origin='local'）即便在远端消失也不得被删除。
#[tokio::test]
async fn remote_unsubscribe_keeps_local_origin_feed() {
    let (db, http, server) = setup_remote_unsub("keeps_local").await;

    // 本地直连添加一个与远端 feed 10 同 URL 的源，并绑定远端 id（URL 碰撞合并的结果）
    {
        let conn = db.lock().await;
        let folder = db::create_folder(&conn, "本地分类", "article").unwrap();
        let fid = db::insert_feed(
            &conn,
            "http://127.0.0.1:8765/local_feed.xml",
            None,
            "Local Direct Feed",
            None,
            folder,
            "inherit",
            false,
            false,
        )
        .unwrap();
        db::set_feed_remote_id(&conn, fid, 10).unwrap();
        assert_eq!(feed_origin(&conn, fid).as_deref(), Some("local"));
    }

    // 服务端退掉 feed 10
    server.remove_remote_subscription("feed/10");
    sync::feeds_phase(&db, &http).await.expect("pull after unsub");

    let conn = db.lock().await;
    assert!(
        feed_id_by_url(&conn, "http://127.0.0.1:8765/local_feed.xml").is_some(),
        "origin='local' feed must survive a remote unsubscribe (data-loss guard)"
    );
}

/// B③ 队列里有未推送变更的源不得被远端快照删除（pending 保护）。
#[tokio::test]
async fn remote_unsubscribe_keeps_feed_with_pending_queue_item() {
    let (db, http, server) = setup_remote_unsub("pending").await;

    // 先正常拉取 feed 11 到本地（origin='remote'、已绑定）
    sync::feeds_phase(&db, &http).await.expect("first pull");
    {
        let conn = db.lock().await;
        assert!(
            feed_id_by_url(&conn, "http://example.com/remote-only.xml").is_some(),
            "remote-only feed pulled to local"
        );
        // 模拟「本地刚改名、尚未推送」：入队一条该 URL 的变更
        db::enqueue_sync(
            &conn,
            None,
            Some("http://example.com/remote-only.xml"),
            "add_feed",
            Some(r#"{"title":"改名未推送"}"#),
        )
        .unwrap();
    }

    // 服务端退订该源，并让本轮的 push 失败 —— 队项因此保留在 sync_queue
    // （feeds_phase 先 push 再 pull；push 成功会把队项 prune 掉，那样 pending
    //  保护就无从验证。注入失败正是「本地变更尚未回传」的真实形态。）
    server.remove_remote_subscription("feed/11");
    server.set_fail_quick_add(true);
    let report = sync::feeds_phase(&db, &http).await.expect("pull after unsub");
    server.set_fail_quick_add(false);

    let conn = db.lock().await;
    // 前置：队项确实还在（否则本用例没有验证到保护逻辑）
    let queued: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sync_queue WHERE feed_url = ?1",
            rusqlite::params!["http://example.com/remote-only.xml"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        queued, 1,
        "precondition: the unsent queue item must still be present"
    );
    assert!(
        feed_id_by_url(&conn, "http://example.com/remote-only.xml").is_some(),
        "feed with an unsent queued change must NOT be deleted by the remote snapshot"
    );
    assert_eq!(
        report.removed_feeds, 0,
        "pending-protected feed must not be counted as removed"
    );
}

/// B②' 未绑定 remote_id 的源（origin='remote' 但 remote_id 为空）不得被删除。
#[tokio::test]
async fn remote_unsubscribe_keeps_unbound_feed() {
    let (db, http, server) = setup_remote_unsub("unbound").await;

    {
        let conn = db.lock().await;
        let folder = db::create_folder(&conn, "分类", "article").unwrap();
        db::insert_feed_origin(
            &conn,
            "http://example.com/never-listed.xml",
            None,
            "Unbound Remote",
            None,
            folder,
            "inherit",
            false,
            false,
            "remote",
        )
        .unwrap();
    }

    server.remove_remote_subscription("feed/11");
    sync::feeds_phase(&db, &http).await.expect("pull");

    let conn = db.lock().await;
    assert!(
        feed_id_by_url(&conn, "http://example.com/never-listed.xml").is_some(),
        "remote feed without a bound remote_id must not be deleted"
    );
}

/// B③' **article 级**未推送队项也算 pending：源与其文章都不得被删除。
///
/// 独立审查 FINDING（TASK-075）：原实现只查 `sync_queue.feed_url`，而
/// commands/articles.rs 的 record_read_state / record_star_state / mark_all_read
/// 入队的行是 `article_id = Some(id), feed_url = None`。只查 feed_url 时，离线期间
/// 「标星/已读但未推送」的源会被远端退订连源带文章一起删除，未推送状态也被静默丢弃。
#[tokio::test]
async fn remote_unsubscribe_keeps_feed_with_article_scoped_pending() {
    let (db, http, server) = setup_remote_unsub("pending_article").await;

    // 第一轮：远端 feed 11 拉到本地（origin='remote' + 已绑定）
    sync::feeds_phase(&db, &http).await.expect("first pull");

    let (article_id, feed_id) = {
        let conn = db.lock().await;
        let fid = feed_id_by_url(&conn, "http://example.com/remote-only.xml")
            .expect("remote-only feed pulled to local");
        let a = db::NewArticle {
            guid: "guid-article-scoped-pending".into(),
            url: Some("http://example.com/post/pending".into()),
            title: "Offline starred".into(),
            author: None,
            summary: None,
            content_html: None,
            body_text: "pending".into(),
            image_url: None,
            enclosure_url: None,
            enclosure_mime: None,
            duration_sec: None,
            published_at: None,
            source: "direct".into(),
        };
        let (aid, _) = db::upsert_article_with_feed(&conn, fid, &a, false).unwrap();
        // 与 commands/articles.rs::record_star_state 逐字同形：article_id=Some, feed_url=None
        db::enqueue_sync(&conn, Some(aid), None, "star", None).unwrap();
        (aid, fid)
    };

    // 前置断言：队项确实存在，且确实是 feed_url IS NULL 的 article 级行
    // （否则本用例验证不到「article 级」这一路径）
    {
        let conn = db.lock().await;
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_queue
                 WHERE article_id = ?1 AND action = 'star' AND feed_url IS NULL",
                rusqlite::params![article_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "precondition: an unsent article-scoped queue row must exist");
    }

    // 服务端退订该源
    server.remove_remote_subscription("feed/11");
    let report = sync::feeds_phase(&db, &http).await.expect("pull after unsub");

    let conn = db.lock().await;
    assert_eq!(
        feed_id_by_url(&conn, "http://example.com/remote-only.xml"),
        Some(feed_id),
        "feed with an unsent ARTICLE-scoped change must NOT be deleted"
    );
    let article_alive: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM articles WHERE id = ?1",
            rusqlite::params![article_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(article_alive, 1, "its article must survive too");
    let still_queued: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sync_queue WHERE article_id = ?1 AND action = 'star'",
            rusqlite::params![article_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(still_queued, 1, "the unsent state must not be silently dropped");
    assert_eq!(
        report.removed_feeds, 0,
        "pending-protected feed must not be counted as removed"
    );
}
