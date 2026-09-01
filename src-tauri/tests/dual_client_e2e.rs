//! URL 规范化（去重精确化）+ 双端读意图双向流的回归测试。
//!
//! 双端场景（Read You 安卓 + FluxReader 桌面共用一个 Miniflux）：
//! - 手机上读的是某源副本 entry → 桌面必须跟随已读（读到哪算读）
//! - 跨源副本的「未读」态不得复活桌面已读文章
//! - 桌面标读 → 广播到全部同文副本 entry（手机另一源的副本也已读）

use app_lib::db;

mod mock_miniflux;

fn temp_db(name: &str) -> std::path::PathBuf {
    let tmp = std::env::temp_dir().join(format!(
        "fluxreader_dual_{name}_{}.db",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&tmp);
    tmp
}

fn article(url: &str, guid: &str) -> db::NewArticle {
    db::NewArticle {
        guid: guid.into(),
        url: Some(url.into()),
        title: format!("T-{guid}"),
        author: None,
        summary: None,
        content_html: Some("<p>c</p>".into()),
        body_text: "c".into(),
        image_url: None,
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: Some(chrono::Utc::now().to_rfc3339()),
        source: "direct".into(),
    }
}

/* ============================================================
   URL 规范化矩阵
   ============================================================ */

#[test]
fn normalize_url_collapses_known_variants() {
    let n = |u: &str| db::normalize_url(u);
    // 跟踪参数
    assert_eq!(
        n("https://example.com/a?utm_source=x&utm_medium=rss&id=5"),
        n("http://example.com/a?id=5"),
        "utm params stripped, https==http"
    );
    // www./m. 前缀 + 尾斜杠
    assert_eq!(n("https://www.example.com/a/"), n("http://example.com/a"), "www + trailing slash");
    assert_eq!(n("https://m.example.com/a"), n("http://example.com/a"), "mobile prefix");
    // AMP 页
    assert_eq!(n("https://example.com/amp/post/1"), n("http://example.com/post/1"), "amp prefix");
    // 锚点
    assert_eq!(n("https://example.com/a#comments"), n("http://example.com/a"), "fragment dropped");
    // 非跟踪参数保留（可能定位不同文章）
    assert_ne!(n("http://example.com/p?id=1"), n("http://example.com/p?id=2"), "meaningful params kept");
    // 路径不同 → 不同文章
    assert_ne!(n("http://example.com/a"), n("http://example.com/b"), "different paths stay distinct");
}

#[test]
fn dedup_uses_normalized_url() {
    let (conn, tmp) = {
        let tmp = temp_db("norm");
        let conn = db::open(&tmp).expect("open db");
        db::create_folder(&conn, "F", "article").unwrap();
        (conn, tmp)
    };
    db::insert_feed(&conn, "http://x/1.xml", None, "F1", None, 1, "inherit", false, false).unwrap();
    db::insert_feed(&conn, "http://x/2.xml", None, "F2", None, 1, "inherit", false, false).unwrap();

    // feed A：带 utm 的 URL；feed B：干净 URL + www + 尾斜杠 → 同一篇
    let (_, n1) = db::upsert_article_with_feed(
        &conn, 1,
        &article("https://example.com/story/?utm_source=feed_a&utm_campaign=daily", "ga"),
        true,
    ).unwrap();
    assert!(n1, "first variant ingested");
    let (_, n2) = db::upsert_article_with_feed(
        &conn, 2,
        &article("http://www.example.com/story/", "gb"),
        true,
    ).unwrap();
    assert!(!n2, "dressed-up duplicate must be caught by normalized key");

    // 原始 url 保留（打开源网页用原始链接）
    let orig: String = conn
        .query_row("SELECT url FROM articles WHERE guid = 'ga'", [], |r| r.get(0))
        .unwrap();
    assert!(orig.contains("utm_source=feed_a"), "original URL preserved for open-in-browser");

    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
   双端读意图双向流（mock 服务端端到端）
   ============================================================ */

#[tokio::test]
async fn read_intent_flows_both_ways_across_feeds() {
    let server = mock_miniflux::MockMiniflux::start().await.expect("start mock");
    let tmp = temp_db("flow");
    let conn = db::open(&tmp).expect("open db");
    let db = std::sync::Arc::new(tokio::sync::Mutex::new(conn));

    // 服务端先建条目（mock 自动分配 entry id），本地文章绑定到真实 id
    // （准备段：锁内操作完毕后 drop，sync_now 内部会再拿锁——同一任务
    //   里长持 guard 跨 sync 会自死锁）
    let (xid, _own_id, copy_id) = {
        let conn = db.lock().await;
        let own_id = server.add_entry_ret(10, "http://x/story", "Own copy", "read", false);
        let copy_id = server.add_entry_ret(20, "http://x/story", "Other feed copy", "unread", false);

        db::create_folder(&conn, "F", "article").unwrap();
        db::insert_feed(&conn, "http://x/a.xml", None, "FA", None, 1, "inherit", false, false).unwrap();
        db::set_feed_miniflux_id(&conn, 1, 10).unwrap();
        let (xid, _) = db::upsert_article_with_feed(&conn, 1, &article("http://x/story", "gx"), false).unwrap();
        db::set_article_miniflux_id(&conn, xid, own_id).unwrap();
        conn.execute("UPDATE articles SET is_read = 1 WHERE id = ?1", [xid]).unwrap();

        db::set_setting(&conn, "miniflux_endpoint", &server.url()).unwrap();
        db::set_setting(&conn, "miniflux_token", "t").unwrap();
        (xid, own_id, copy_id)
    };
    let http = app_lib::ingestion::build_client(10);

    // 第一轮：副本 entry 被记账（不写状态）
    let r1 = app_lib::sync::sync_now(&db, &http).await.expect("sync 1");
    {
        let conn = db.lock().await;
        assert!(r1.errors.is_empty(), "no errors: {:?}", r1.errors);
        let dups = db::article_dup_entries(&conn, xid).unwrap();
        assert!(dups.contains(&copy_id), "cross-feed copy recorded as dup entry (got {dups:?})");
        let still_read: bool = conn
            .query_row("SELECT is_read FROM articles WHERE id = ?1", [xid], |r| r.get::<_, i64>(0).map(|v| v != 0))
            .unwrap();
        assert!(still_read, "unread copy must not resurrect the read article");
    }

    // ---- 手机端读了副本（copy entry → read）：桌面必须跟随 ----
    server.entries.lock().unwrap().iter_mut().for_each(|e| {
        if e.id == copy_id { e.status = "read".into(); }
    });
    let _ = app_lib::sync::sync_now(&db, &http).await.expect("sync 2");
    {
        let conn = db.lock().await;
        let still_read2: bool = conn
            .query_row("SELECT is_read FROM articles WHERE id = ?1", [xid], |r| r.get::<_, i64>(0).map(|v| v != 0))
            .unwrap();
        assert!(still_read2, "reading on the phone (any copy) keeps desktop read");
    }

    // ---- 桌面端标读：广播到副本 entry ----
    let (yid, y_own, y_copy) = {
        let conn = db.lock().await;
        let y_own = server.add_entry_ret(10, "http://x/story-y", "Y own", "unread", false);
        let y_copy = server.add_entry_ret(20, "http://x/story-y", "Y copy", "unread", false);
        let (yid, _) = db::upsert_article_with_feed(&conn, 1, &article("http://x/story-y", "gy"), false).unwrap();
        db::set_article_miniflux_id(&conn, yid, y_own).unwrap();
        (yid, y_own, y_copy)
    };
    // 先同步让 y_copy 被记账为副本
    let _ = app_lib::sync::sync_now(&db, &http).await.expect("sync 3");
    {
        let conn = db.lock().await;
        let dups_y = db::article_dup_entries(&conn, yid).unwrap();
        assert!(dups_y.contains(&y_copy), "Y's copy recorded (got {dups_y:?})");
        // 桌面读 Y → 入队 → 同步 → 推送应同时标读 own 和 copy
        db::set_read(&conn, yid, true).unwrap();
        db::enqueue_sync(&conn, Some(yid), None, "read", None).unwrap();
    }
    let _ = app_lib::sync::sync_now(&db, &http).await.expect("sync 4");
    {
        let conn = db.lock().await;
        let _ = conn; // 断言走 mock 状态，无需 DB
    }
    let updates = mock_miniflux::status_updates_map(&server);
    assert_eq!(updates.get(&y_own).map(|s| s.as_str()), Some("read"), "own entry pushed read");
    assert_eq!(updates.get(&y_copy).map(|s| s.as_str()), Some("read"), "copy entry broadcast read (phone sees it read)");

    let _ = std::fs::remove_file(&tmp);
    println!("=== DUAL-CLIENT READ FLOW PASS ===");
}

/* ============================================================
   待推保护（防乒乓）
   ============================================================ */

#[tokio::test]
async fn pending_local_change_wins_over_stale_remote() {
    let server = mock_miniflux::MockMiniflux::start().await.expect("start mock");
    let tmp = temp_db("pending");
    let conn = db::open(&tmp).expect("open db");

    db::create_folder(&conn, "F", "article").unwrap();
    db::insert_feed(&conn, "http://x/a.xml", None, "FA", None, 1, "inherit", false, false).unwrap();
    db::set_feed_miniflux_id(&conn, 1, 10).unwrap();
    let own_id = server.add_entry_ret(10, "http://x/p", "P", "read", false); // 服务端：已读
    let (aid, _) = db::upsert_article_with_feed(&conn, 1, &article("http://x/p", "gp"), false).unwrap();
    db::set_article_miniflux_id(&conn, aid, own_id).unwrap();

    db::set_setting(&conn, "miniflux_endpoint", &server.url()).unwrap();
    db::set_setting(&conn, "miniflux_token", "t").unwrap();
    let http = app_lib::ingestion::build_client(10);

    // 桌面刚标未读（已入队未推送），服务端还是旧已读态
    db::set_read(&conn, aid, false).unwrap();
    db::enqueue_sync(&conn, Some(aid), None, "unread", None).unwrap();
    drop(conn);

    let db = std::sync::Arc::new(tokio::sync::Mutex::new(
        db::open(&tmp).expect("reopen db"),
    ));
    let _ = app_lib::sync::sync_now(&db, &http).await.expect("sync");
    let conn = db.lock().await;
    let final_read: bool = conn
        .query_row("SELECT is_read FROM articles WHERE id = ?1", [aid], |r| r.get::<_, i64>(0).map(|v| v != 0))
        .unwrap();
    assert!(!final_read, "local unread must survive the sync round-trip");
    let updates = mock_miniflux::status_updates_map(&server);
    assert_eq!(updates.get(&own_id).map(|s| s.as_str()), Some("unread"), "unread pushed to server");

    let _ = std::fs::remove_file(&tmp);
}
