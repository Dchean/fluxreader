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

/// ④ 核心回归：origin='miniflux' 源的历史文章（published_at 早于增量游标）
/// 必须被全量拉取并对齐状态。此前用 after=last_sync_ts（按 published_at 过滤）
/// 会漏掉发布时间早于游标的历史文章——这是「跟随服务端」模式下客户端文章数
/// 少于 Miniflux、且已读状态对不齐的根因。
#[tokio::test]
#[ignore = "spins a local mock server"]
async fn miniflux_origin_pulls_historical_entries_regardless_of_published_at() {
    let (db, http, server) = setup("history_pull").await;

    // 先 feeds 阶段：把远端 feed 11（remote-only）拉到本地（origin='miniflux'）
    sync::feeds_phase(&db, &http).await.expect("feeds phase");

    // 服务端 feed 11 加一条「历史文章」：published_at 拨到 3 天前（早于增量游标），
    // 状态为 read（Miniflux 端已读）。
    let old_ts = (chrono::Utc::now() - chrono::Duration::days(3)).to_rfc3339();
    server.add_entry_with_published(
        11,
        "http://example.com/remote-only/post/history-1",
        "Historical read article",
        "read",
        false,
        old_ts,
    );

    // 把本地增量游标拨到「现在」（模拟：上次同步已推进到当前，之后服务端才补入
    // 了这条 published_at 更早的历史文章）。旧实现用 after=now 会漏掉它。
    {
        let conn = db.lock().await;
        db::set_last_sync_ts(&conn, chrono::Utc::now().timestamp()).unwrap();
    }

    // 后台轻量同步（full=false）——必须拉入历史文章并对齐 read 状态
    sync::sync_light(&db, &http).await.expect("light sync");

    {
        let conn = db.lock().await;
        let (count, is_read, source): (i64, bool, String) = conn
            .query_row(
                "SELECT COUNT(*), COALESCE(MAX(is_read), 0), COALESCE(MAX(source), '')
                 FROM articles WHERE url = 'http://example.com/remote-only/post/history-1'",
                [],
                |r| Ok((r.get(0)?, r.get::<_, i64>(1).unwrap_or(0) != 0, r.get::<_, String>(2).unwrap_or_default())),
            )
            .unwrap();
        assert_eq!(count, 1, "历史文章（published_at 早于游标）必须被拉入本地");
        assert_eq!(source, "miniflux");
        assert!(is_read, "Miniflux 端已读的历史文章，客户端必须对齐为已读");
    }
}

/// ⑤ 核心回归：本地已绑定文章「未读」，Miniflux 端已读且 changed_at 早于增量
/// 游标（手机很久前标读）——light 同步（full=false）必须通过未读 id 精确对账
/// 收敛为已读。此前只靠 changed_after 增量，会永久漏掉这条旧变更，导致
/// 「Miniflux 已读但本地未读」与未读数漂移。
#[tokio::test]
#[ignore = "spins a local mock server"]
async fn light_sync_converges_stale_remote_read_via_unread_ids() {
    let (db, http, server) = setup("stale_read").await;

    // 本地直连 feed 绑定远端 feed 10，造一篇未读文章并绑定 entry
    let (aid, mf_id) = {
        let conn = db.lock().await;
        let folder = db::create_folder(&conn, "F", "article").unwrap();
        let feed = db::insert_feed(&conn, "http://127.0.0.1:8765/local_feed.xml", None, "Local", None, folder, "inherit", true, false).unwrap();
        db::set_feed_miniflux_id(&conn, feed, 10).unwrap();
        let mf_id = server.add_entry_ret(10, "http://127.0.0.1:8765/post/stale", "Stale read", "unread", false);
        let a = db::NewArticle {
            guid: "g-stale".into(),
            url: Some("http://127.0.0.1:8765/post/stale".into()),
            title: "Stale read".into(),
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
        // 本地保持未读（is_read=0）
        (aid, mf_id)
    };

    // 手机端很久以前标读：entry 状态 read，changed_at 拨回 1 小时前（早于游标）
    {
        let mut es = server.entries.lock().unwrap();
        if let Some(e) = es.iter_mut().find(|e| e.id == mf_id) {
            e.status = "read".into();
            e.changed_at = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        }
    }

    // light 同步（full=false）：changed_after 增量拉不到这条旧变更，但未读 id
    // 对账必须收敛——本地应从「未读」变为「已读」。
    sync::sync_light(&db, &http).await.expect("light sync");

    {
        let conn = db.lock().await;
        let is_read: bool = conn
            .query_row("SELECT is_read FROM articles WHERE id = ?1", [aid], |r| r.get::<_, i64>(0).map(|v| v != 0))
            .unwrap();
        assert!(is_read, "light sync must converge stale remote read (本地未读 → Miniflux 已读)");
    }
}
