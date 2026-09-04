//! 同步引擎三期改造的端到端测试：
//! ① 即时状态推送（push_states_now）：只推不拉、read 广播副本、队列清空
//! ② 分步同步（feeds_phase / states_phase）：两阶段独立可跑、锁不跨 await
//! ③ 旧变更收敛：changed_at 早于增量游标的远端已读，full 对账路径能追上
//!    （light 路径追不上——分层设计：快路径便宜、慢路径彻底）
//! 运行：cargo test --test sync_phases_e2e -- --ignored --nocapture

mod mock_miniflux;

use app_lib::db;
use app_lib::sync;
use mock_miniflux::MockMiniflux;
use std::sync::Arc;
use tokio::sync::Mutex;

async fn setup(name: &str) -> (Arc<Mutex<rusqlite::Connection>>, reqwest::Client, Arc<MockMiniflux>) {
    let server = MockMiniflux::start().await.expect("start mock server");
    let tmp = std::env::temp_dir().join(format!("fluxreader_phases_{name}.db"));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).expect("open db");
    db::set_setting(&conn, "miniflux_endpoint", &server.url()).unwrap();
    db::set_setting(&conn, "miniflux_token", "test-token").unwrap();
    let http = app_lib::ingestion::build_client(10);
    (Arc::new(Mutex::new(conn)), http, server)
}

/// 本地造一篇直连文章 + 远端 mock 加同 URL entry（绑定回填的匹配目标）。
/// 返回 (本地文章 id, 远端 entry id)。
async fn seed_local_article(
    db: &Arc<Mutex<rusqlite::Connection>>,
    server: &MockMiniflux,
    url: &str,
) -> (i64, i64) {
    let conn = db.lock().await;
    let folder_id = db::create_folder(&conn, "测试分类", "article").unwrap();
    let feed_id = db::insert_feed(
        &conn,
        "http://127.0.0.1:8765/local_feed.xml", // 与 mock feed 10 同 URL
        None,
        "Local Direct Feed",
        None,
        folder_id,
        "inherit",
        true,
        false,
    )
    .unwrap();
    let a = db::NewArticle {
        guid: format!("guid-{url}"),
        url: Some(url.into()),
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
    let aid = db::upsert_article_with_feed(&conn, feed_id, &a, false).unwrap().0;
    drop(conn);
    let mf_id = server.add_entry_ret(10, url, "Remote version", "unread", false);
    (aid, mf_id)
}

#[tokio::test]
#[ignore = "spins a local mock server"]
async fn instant_push_only_pushes_and_drains_queue() {
    let (db, http, server) = setup("instant").await;
    let (aid, _mf_id) = seed_local_article(&db, &server, "http://127.0.0.1:8765/post/1").await;

    // 先跑一次 full states（含绑定回填），拿到 miniflux_id 绑定
    sync::states_phase(&db, &http, true).await.expect("bind phase");
    {
        let conn = db.lock().await;
        let bound: Option<i64> = conn
            .query_row("SELECT miniflux_id FROM articles WHERE id = ?1", [aid], |r| r.get(0))
            .ok()
            .flatten();
        assert!(bound.is_some(), "article bound after states_phase(full)");
    }

    // 本地标读（入队但不跑全量同步——只走即时推送）
    {
        let conn = db.lock().await;
        db::set_read(&conn, aid, true).unwrap();
        db::enqueue_sync(&conn, Some(aid), None, "read", None).unwrap();
    }
    sync::push_states_now(&db, &http).await;

    // 远端收到 read
    let updates = mock_miniflux::status_updates_map(&server);
    let (conn, ) = (db.lock().await,);
    let mf_id: i64 = conn
        .query_row("SELECT miniflux_id FROM articles WHERE id = ?1", [aid], |r| r.get(0))
        .unwrap();
    assert_eq!(
        updates.get(&mf_id).map(|s| s.as_str()),
        Some("read"),
        "instant push must reach the server"
    );
    // 队列清空（推送成功 → prune）
    let left = db::take_sync_queue(&conn).unwrap().len();
    assert_eq!(left, 0, "queue drained after successful instant push");
}

#[tokio::test]
#[ignore = "spins a local mock server"]
async fn instant_push_read_broadcasts_dup_entries() {
    let (db, http, server) = setup("broadcast").await;
    let (aid, _mf_id) = seed_local_article(&db, &server, "http://127.0.0.1:8765/post/1").await;
    sync::states_phase(&db, &http, true).await.expect("bind phase");

    // 跨源副本 entry（feed 11 同 URL——手机端另一源的副本）
    let dup_id = server.add_entry_ret(11, "http://127.0.0.1:8765/post/1", "Dup copy", "unread", false);
    // states 增量轮把副本记账到 miniflux_dup_ids
    sync::states_phase(&db, &http, true).await.expect("record dup");

    // 本地标读 → 即时推送必须广播到副本 entry
    {
        let conn = db.lock().await;
        db::set_read(&conn, aid, true).unwrap();
        db::enqueue_sync(&conn, Some(aid), None, "read", None).unwrap();
    }
    sync::push_states_now(&db, &http).await;
    let updates = mock_miniflux::status_updates_map(&server);
    assert_eq!(
        updates.get(&dup_id).map(|s| s.as_str()),
        Some("read"),
        "read must broadcast to the cross-feed dup entry"
    );
}

#[tokio::test]
#[ignore = "spins a local mock server"]
async fn feeds_and_states_phases_run_independently() {
    let (db, http, _server) = setup("phases").await;
    let (aid, _mf) = seed_local_article(&db, &_server, "http://127.0.0.1:8765/post/1").await;

    // feeds 阶段独立可跑：本地 feed 绑定远端 feed 10
    let r1 = sync::feeds_phase(&db, &http).await.expect("feeds phase");
    assert!(r1.merged_states >= 1, "local feed merged with remote");
    {
        let conn = db.lock().await;
        let bound: Option<i64> = conn
            .query_row("SELECT miniflux_id FROM feeds WHERE feed_url = 'http://127.0.0.1:8765/local_feed.xml'", [], |r| r.get(0))
            .ok()
            .flatten();
        assert_eq!(bound, Some(10), "feed bound to remote id 10");
    }

    // states 阶段独立可跑：文章完成绑定
    sync::states_phase(&db, &http, true).await.expect("states phase");
    let (conn, ) = (db.lock().await,);
    let bound: Option<i64> = conn
        .query_row("SELECT miniflux_id FROM articles WHERE id = ?1", [aid], |r| r.get(0))
        .ok()
        .flatten();
    assert!(bound.is_some(), "article bound in states phase");
}

/// 未读数漂移根因回归：远端 changed_at 早于增量游标（手机很久前标读），
/// light 增量拉不到，full 对账必须收敛。
#[tokio::test]
#[ignore = "spins a local mock server"]
async fn stale_remote_read_converges_via_full_reconcile() {
    let (db, http, server) = setup("stale").await;
    let (aid, mf_id) = seed_local_article(&db, &server, "http://127.0.0.1:8765/post/1").await;

    // 绑定
    sync::states_phase(&db, &http, true).await.expect("bind");

    // 模拟"很久以前手机标读"：entry 状态 read，changed_at 拨回 1 小时前
    {
        let mut es = server.entries.lock().unwrap();
        if let Some(e) = es.iter_mut().find(|e| e.id == mf_id) {
            e.status = "read".into();
            e.changed_at = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        }
    }

    // light 增量（changed_after = last_sync ≈ now）：拉不到这条旧变更
    sync::sync_light(&db, &http).await.expect("light sync");
    {
        let conn = db.lock().await;
        let is_read: bool = conn
            .query_row("SELECT is_read FROM articles WHERE id = ?1", [aid], |r| r.get::<_, i64>(0).map(|v| v != 0))
            .unwrap();
        assert!(!is_read, "light sync must not see the stale change (by design)");
    }

    // full 对账：全量条目结果里直接应用状态 → 收敛
    sync::states_phase(&db, &http, true).await.expect("full reconcile");
    let (conn, ) = (db.lock().await,);
    let is_read: bool = conn
        .query_row("SELECT is_read FROM articles WHERE id = ?1", [aid], |r| r.get::<_, i64>(0).map(|v| v != 0))
        .unwrap();
    assert!(is_read, "full reconcile must converge the stale remote read");
}

/// 本地直连源漏抓的条目（本地文章数 < 远端）→ full 同步对比拉取补齐。
/// 根因：此前 pull_entries 只对「完全失败」的源做 Miniflux 兜底，正常直连源
/// 若 feed 只提供摘要/漏了几条，本地永久缺失。现在 full 对账会对已绑定源
/// 的远端条目逐一 upsert 补齐（幂等，不重复、不覆盖已有正文/已读）。
#[tokio::test]
#[ignore = "spins a local mock server"]
async fn full_reconcile_backfills_missing_local_entries() {
    let (db, http, server) = setup("backfill").await;
    // 本地造一篇直连文章 + 绑定远端 feed 10
    let (_aid, _mf) = seed_local_article(&db, &server, "http://127.0.0.1:8765/post/1").await;
    // feeds 阶段绑定本地 feed → 远端 feed 10
    sync::feeds_phase(&db, &http).await.expect("feeds phase bind");

    // 远端 feed 10 加一条本地完全没有的条目（模拟直连源漏抓）
    server.add_entry_ret(10, "http://127.0.0.1:8765/missing/post/999", "Remote-only entry", "unread", false);

    // full 同步：绑定回填 + 对比拉取补齐缺失条目
    sync::states_phase(&db, &http, true).await.expect("full reconcile");

    {
        let conn = db.lock().await;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM articles WHERE url = 'http://127.0.0.1:8765/missing/post/999'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "远程独有条目应被对比拉取补齐到本地");
        // 来源应标记为 miniflux（兜底补齐）
        let source: String = conn
            .query_row(
                "SELECT source FROM articles WHERE url = 'http://127.0.0.1:8765/missing/post/999'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(source, "miniflux", "补齐的条目来源应为 miniflux");
    }

    // 幂等：再跑一次 full 同步，不应重复入库
    let before: i64 = {
        let conn = db.lock().await;
        conn.query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0)).unwrap()
    };
    sync::states_phase(&db, &http, true).await.expect("second full reconcile");
    let after: i64 = {
        let conn = db.lock().await;
        conn.query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0)).unwrap()
    };
    assert_eq!(before, after, "二次 full 同步不应重复入库（幂等）");
}
