//! 同步引擎三期改造的端到端测试：
//! ① 即时状态推送（push_states_now）：只推不拉、read 广播副本、队列清空
//! ② 分步同步（feeds_phase / states_phase）：两阶段独立可跑、锁不跨 await
//! ③ 旧变更收敛：changed_at 早于增量游标的远端已读，full 对账路径能追上
//!    （light 路径追不上——分层设计：快路径便宜、慢路径彻底）
//! 运行：cargo test --test sync_phases_e2e -- --ignored --nocapture

mod mock_greader;

use app_lib::db;
use app_lib::sync;
use mock_greader::MockGReader;
use std::sync::Arc;
use tokio::sync::Mutex;

async fn setup(name: &str) -> (Arc<Mutex<rusqlite::Connection>>, reqwest::Client, Arc<MockGReader>) {
    let server = MockGReader::start().await.expect("start mock server");
    let tmp = std::env::temp_dir().join(format!("fluxreader_phases_{name}.db"));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).expect("open db");
    db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
    db::set_setting(&conn, "greader_username", "test").unwrap();
    db::set_setting(&conn, "greader_password", "test-token").unwrap();
    let http = app_lib::ingestion::build_client(10);
    (Arc::new(Mutex::new(conn)), http, server)
}

/// 本地造一篇直连文章 + 远端 mock 加同 URL entry（绑定回填的匹配目标）。
/// 返回 (本地文章 id, 远端 entry id)。
async fn seed_local_article(
    db: &Arc<Mutex<rusqlite::Connection>>,
    server: &MockGReader,
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
    let mf_id = server.add_entry_ret(10, url, "Remote version", false, false);
    (aid, mf_id)
}

#[tokio::test]
#[ignore = "spins a local mock server"]
async fn instant_push_only_pushes_and_drains_queue() {
    let (db, http, server) = setup("instant").await;
    let (aid, _mf_id) = seed_local_article(&db, &server, "http://127.0.0.1:8765/post/1").await;

    // 先跑一次 full states（含绑定回填），拿到 remote_id 绑定
    sync::states_phase(&db, &http, true).await.expect("bind phase");
    {
        let conn = db.lock().await;
        let bound: Option<i64> = conn
            .query_row("SELECT remote_id FROM articles WHERE id = ?1", [aid], |r| r.get(0))
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
    let updates = mock_greader::status_updates_map(&server);
    let (conn, ) = (db.lock().await,);
    let mf_id: i64 = conn
        .query_row("SELECT remote_id FROM articles WHERE id = ?1", [aid], |r| r.get(0))
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
    let dup_id = server.add_entry_ret(11, "http://127.0.0.1:8765/post/1", "Dup copy", false, false);
    // states 增量轮把副本记账到 remote_dup_ids
    sync::states_phase(&db, &http, true).await.expect("record dup");

    // 本地标读 → 即时推送必须广播到副本 entry
    {
        let conn = db.lock().await;
        db::set_read(&conn, aid, true).unwrap();
        db::enqueue_sync(&conn, Some(aid), None, "read", None).unwrap();
    }
    sync::push_states_now(&db, &http).await;
    let updates = mock_greader::status_updates_map(&server);
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
            .query_row("SELECT remote_id FROM feeds WHERE feed_url = 'http://127.0.0.1:8765/local_feed.xml'", [], |r| r.get(0))
            .ok()
            .flatten();
        assert_eq!(bound, Some(10), "feed bound to remote id 10");
    }

    // states 阶段独立可跑：文章完成绑定
    sync::states_phase(&db, &http, true).await.expect("states phase");
    let (conn, ) = (db.lock().await,);
    let bound: Option<i64> = conn
        .query_row("SELECT remote_id FROM articles WHERE id = ?1", [aid], |r| r.get(0))
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
            e.read = true;
            e.changed_at = (chrono::Utc::now() - chrono::Duration::hours(1)).timestamp();
        }
    }

    // light 增量：此前 changed_after 增量拉不到这条旧变更（changed_at 早于游标），
    // 导致「Miniflux 已读但本地未读」漂移。修复后 light 同步末尾有未读状态精确
    // 对账（GET /v1/entries/ids?status=unread），即使增量漏掉也能收敛。
    sync::sync_light(&db, &http).await.expect("light sync");
    {
        let conn = db.lock().await;
        let is_read: bool = conn
            .query_row("SELECT is_read FROM articles WHERE id = ?1", [aid], |r| r.get::<_, i64>(0).map(|v| v != 0))
            .unwrap();
        assert!(is_read, "light sync must converge the stale remote read via unread-id reconcile");
    }

    // full 对账：全量条目结果里直接应用状态 → 收敛
    sync::states_phase(&db, &http, true).await.expect("full reconcile");
    let (conn, ) = (db.lock().await,);
    let is_read: bool = conn
        .query_row("SELECT is_read FROM articles WHERE id = ?1", [aid], |r| r.get::<_, i64>(0).map(|v| v != 0))
        .unwrap();
    assert!(is_read, "full reconcile must converge the stale remote read");
}

/// P0-5 回归：空库（0 分类）直接 feeds_phase → 无分类归属的远端订阅必须落入
/// 「未分类」分类（无则建），不得回落不存在的 folder_id=1 导致订阅静默丢失。
#[tokio::test]
#[ignore = "spins a local mock server"]
async fn pull_feeds_without_folder_uses_uncategorized() {
    let (db, http, server) = setup("uncategorized").await;
    // 造一个无分类归属的远端订阅（空 categories）
    {
        let mut subs = server.subscriptions.lock().unwrap();
        subs.push(mock_greader::MockSubscription {
            id: "feed/77".into(),
            title: "No Category Feed".into(),
            url: "http://example.com/no-category.xml".into(),
            html_url: None,
            categories: vec![],
        });
    }

    // 空库直接 feeds 阶段
    let report = sync::feeds_phase(&db, &http).await.expect("feeds phase");
    assert!(!report.errors.iter().any(|e| e.contains("入库失败")), "无分类订阅不得报错: {:?}", report.errors);

    {
        let conn = db.lock().await;
        // 远端订阅已入库
        let feed_id: Option<i64> = conn
            .query_row(
                "SELECT id FROM feeds WHERE feed_url = 'http://example.com/no-category.xml'",
                [],
                |r| r.get(0),
            )
            .ok();
        assert!(feed_id.is_some(), "无分类订阅必须入库");
        // 落到了「未分类」分类
        let folder_name: Option<String> = conn
            .query_row(
                "SELECT fo.name FROM feeds f JOIN folders fo ON f.folder_id = fo.id WHERE f.id = ?1",
                [feed_id.unwrap()],
                |r| r.get(0),
            )
            .ok();
        assert_eq!(folder_name.as_deref(), Some("未分类"), "无分类订阅落到「未分类」");
    }
}

/// 不推进游标，本地收藏/已读状态与 `sync_last_sync` 均不变。
/// 修复前：`unwrap_or_default()` 把失败当空集合，本地收藏会被全部取消。
#[tokio::test]
#[ignore = "spins a local mock server"]
async fn authority_set_failure_aborts_reconcile_without_changes() {
    let (db, http, server) = setup("authset_fail").await;
    let (aid, mf_id) = seed_local_article(&db, &server, "http://127.0.0.1:8765/post/1").await;

    // 绑定 + 让本地收藏该文章（远端也收藏，避免误伤）
    sync::states_phase(&db, &http, true).await.expect("bind phase");
    {
        let conn = db.lock().await;
        db::set_starred(&conn, aid, true).unwrap();
    }
    // 远端收藏同一条
    {
        let mut es = server.entries.lock().unwrap();
        if let Some(e) = es.iter_mut().find(|e| e.id == mf_id) {
            e.starred = true;
        }
    }
    // 记录失败前游标
    let before_ts = {
        let conn = db.lock().await;
        db::last_sync_ts(&conn).unwrap_or(0)
    };

    // 触发权威集合失败 → 轻量同步应中止对账
    *server.fail_authority_sets.lock().unwrap() = true;
    let report = sync::sync_light(&db, &http).await.expect("light sync returns Ok with aborted flag");
    assert!(report.aborted, "权威集合失败必须标记 aborted");
    assert!(!report.errors.is_empty(), "必须有错误进入 errors");

    // 本地状态不变（收藏仍为 1，未读不受影响）
    {
        let conn = db.lock().await;
        let (is_read, is_starred): (i64, i64) = conn
            .query_row("SELECT is_read, is_starred FROM articles WHERE id = ?1", [aid], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!(is_starred, 1, "本地收藏不被失败的空集合回滚");
        assert_eq!(is_read, 0, "本地已读状态不被失败的空集合回滚");
        // 游标不推进
        let after_ts = db::last_sync_ts(&conn).unwrap_or(0);
        assert_eq!(after_ts, before_ts, "失败轮不得推进同步游标");
    }
}

/// 同一 I-SYNC-1 语义：恢复后端后，再次轻量同步能正常对账收敛，
/// 证明中止只是「本轮」，不是永久损坏（失败不推进游标，下次重试）。
#[tokio::test]
#[ignore = "spins a local mock server"]
async fn authority_set_recovers_after_transient_failure() {
    let (db, http, server) = setup("authset_recover").await;
    let (aid, mf_id) = seed_local_article(&db, &server, "http://127.0.0.1:8765/post/1").await;
    sync::states_phase(&db, &http, true).await.expect("bind phase");

    // 远端标读（changed_at 拨回 1 小时前，走权威集合对账才收敛）
    {
        let mut es = server.entries.lock().unwrap();
        if let Some(e) = es.iter_mut().find(|e| e.id == mf_id) {
            e.read = true;
            e.changed_at = (chrono::Utc::now() - chrono::Duration::hours(1)).timestamp();
        }
    }

    // 先失败一轮
    *server.fail_authority_sets.lock().unwrap() = true;
    let r = sync::sync_light(&db, &http).await.unwrap();
    assert!(r.aborted);
    {
        let conn = db.lock().await;
        let is_read: i64 = conn.query_row("SELECT is_read FROM articles WHERE id = ?1", [aid], |r| r.get(0)).unwrap();
        assert_eq!(is_read, 0, "失败轮不误标读");
    }

    // 恢复后端 → 正常对账，远端已读收敛到本地
    *server.fail_authority_sets.lock().unwrap() = false;
    let r2 = sync::sync_light(&db, &http).await.unwrap();
    assert!(!r2.aborted, "恢复后同步不应中止");
    {
        let conn = db.lock().await;
        let is_read: i64 = conn.query_row("SELECT is_read FROM articles WHERE id = ?1", [aid], |r| r.get(0)).unwrap();
        assert_eq!(is_read, 1, "恢复后权威对账收敛远端已读");
    }
}

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
    server.add_entry_ret(10, "http://127.0.0.1:8765/missing/post/999", "Remote-only entry", false, false);

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
