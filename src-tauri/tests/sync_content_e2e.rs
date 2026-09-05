//! 同步内容回归测试（针对「跟随服务端」模式下的文章数量与状态对齐 bug）：
//! ① origin='miniflux' 源的新条目必须通过 Miniflux 拉取（此前只拉 fetch_failed
//!    源，服务端源既不直连抓取、又从不补内容 → 文章数永久少于服务端）。
//! ② upsert_miniflux_entry 的状态覆盖必须尊重本地待推变更（防乒乓）。
//! ③ article_id_by_url 用规范化 URL 匹配，同文不同饰不重复入库。
//! 运行：cargo test --test sync_content_e2e -- --ignored --nocapture

mod mock_miniflux;

use app_lib::db;
use app_lib::sync;
use mock_miniflux::MockMiniflux;
use std::sync::Arc;
use tokio::sync::Mutex;

async fn setup(name: &str) -> (Arc<Mutex<rusqlite::Connection>>, reqwest::Client, Arc<MockMiniflux>) {
    let server = MockMiniflux::start().await.expect("start mock server");
    let tmp = std::env::temp_dir().join(format!("fluxreader_sync_content_{name}.db"));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).expect("open db");
    db::set_setting(&conn, "miniflux_endpoint", &server.url()).unwrap();
    db::set_setting(&conn, "miniflux_token", "test-token").unwrap();
    let http = app_lib::ingestion::build_client(10);
    (Arc::new(Mutex::new(conn)), http, server)
}

/// ① 服务端来源（origin='miniflux'）源：后台增量同步（full=false）必须拉取新条目。
#[tokio::test]
#[ignore = "spins a local mock server"]
async fn miniflux_origin_feed_pulls_new_entries_in_light_sync() {
    let (db, http, server) = setup("origin_pull").await;

    // 先 feeds 阶段：把远端 feed 11（remote-only）拉到本地（origin='miniflux'）
    sync::feeds_phase(&db, &http).await.expect("feeds phase");
    {
        let conn = db.lock().await;
        let origin: String = conn
            .query_row(
                "SELECT origin FROM feeds WHERE feed_url = 'http://example.com/remote-only.xml'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(origin, "miniflux", "remote feed must be origin='miniflux'");
    }

    // 服务端 feed 11 加一条新条目（本地完全没有）
    server.add_entry_ret(11, "http://example.com/remote-only/post/new-1", "New from server", "unread", false);

    // 后台轻量同步（full=false）——旧实现只拉 fetch_failed 源，会漏掉这条
    sync::sync_light(&db, &http).await.expect("light sync");

    {
        let conn = db.lock().await;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM articles WHERE url = 'http://example.com/remote-only/post/new-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "light sync must pull new entries for origin='miniflux' feeds");
        let source: String = conn
            .query_row(
                "SELECT source FROM articles WHERE url = 'http://example.com/remote-only/post/new-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(source, "miniflux");
    }
}

/// ② 本地待推变更保护：upsert_miniflux_entry 不得用远端旧状态覆盖本地未推的已读。
#[tokio::test]
#[ignore = "spins a local mock server"]
async fn pending_local_read_wins_over_stale_remote_in_upsert() {
    let (db, http, server) = setup("pending_upsert").await;

    // 服务端 feed 10 已有该条目（unread），拿到真实 entry id
    let mf_id = server.add_entry_ret(10, "http://127.0.0.1:8765/post/pending", "Pending", "unread", false);

    // 本地直连 feed 绑定远端 feed 10，造一篇未读文章并绑定该 entry
    let aid = {
        let conn = db.lock().await;
        let folder = db::create_folder(&conn, "F", "article").unwrap();
        let feed = db::insert_feed(&conn, "http://127.0.0.1:8765/local_feed.xml", None, "Local", None, folder, "inherit", true, false).unwrap();
        db::set_feed_miniflux_id(&conn, feed, 10).unwrap();
        let a = db::NewArticle {
            guid: "g-pending".into(),
            url: Some("http://127.0.0.1:8765/post/pending".into()),
            title: "Pending".into(),
            author: None,
            summary: None,
            content_html: Some("<p>x</p>".into()),
            body_text: "x".into(),
            image_url: None,
            enclosure_url: None,
            enclosure_mime: None,
            duration_sec: None,
            published_at: Some(chrono::Utc::now().to_rfc3339()),
            source: "direct".into(),
        };
        let (aid, _) = db::upsert_article_with_feed(&conn, feed, &a, false).unwrap();
        db::set_article_miniflux_id(&conn, aid, mf_id).unwrap();
        // 本地标读并入队（待推），服务端仍是 unread
        db::set_read(&conn, aid, true).unwrap();
        db::enqueue_sync(&conn, Some(aid), None, "read", None).unwrap();
        aid
    };

    // 直接跑 states 阶段（full）：push 阶段会把 read 推上去（mock 回写 read），
    // 随后 pull 阶段全量条目里该 entry 已是 read。关键验证：本地已读不被覆盖。
    let _ = sync::states_phase(&db, &http, true).await.expect("states phase");

    let conn = db.lock().await;
    let is_read: bool = conn
        .query_row("SELECT is_read FROM articles WHERE id = ?1", [aid], |r| r.get::<_, i64>(0).map(|v| v != 0))
        .unwrap();
    assert!(is_read, "local read must survive (no ping-pong)");
}

/// ③ 规范化 URL 匹配：同文不同饰（https/http + 尾斜杠）不重复入库。
#[test]
fn article_id_by_url_uses_normalized_match() {
    let tmp = std::env::temp_dir().join("fluxreader_sync_content_norm.db");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();
    let folder = db::create_folder(&conn, "F", "article").unwrap();
    let feed = db::insert_feed(&conn, "http://x/f.xml", None, "F", None, folder, "inherit", false, false).unwrap();
    let a = db::NewArticle {
        guid: "g-norm".into(),
        url: Some("https://example.com/story/".into()),
        title: "T".into(),
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
    };
    db::upsert_article_with_feed(&conn, feed, &a, false).unwrap();

    // 远端同文但 http + 无尾斜杠 → 必须匹配到同一篇
    let matched = db::article_id_by_url(&conn, "http://example.com/story").unwrap();
    assert!(matched.is_some(), "normalized URL must match the local article (https://example.com/story/ vs http://example.com/story)");

    let _ = std::fs::remove_file(&tmp);
}
