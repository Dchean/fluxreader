//! 同步引擎三期改造的端到端测试：
//! ① 即时状态推送（push_states_now）：只推不拉、read 广播副本、队列清空
//! ② 分步同步（feeds_phase / states_phase）：两阶段独立可跑、锁不跨 await
//! ③ 旧变更收敛：changed_at 早于增量游标的远端已读，full 对账路径能追上
//!    （light 路径追不上——分层设计：快路径便宜、慢路径彻底）
//! 运行：cargo test --test sync_phases_e2e -- --ignored --nocapture

mod common;
mod mock_greader;

use app_lib::db;
use app_lib::sync;
use mock_greader::MockGReader;
use std::sync::Arc;
use tokio::sync::Mutex;

async fn setup(
    name: &str,
) -> (
    Arc<Mutex<rusqlite::Connection>>,
    reqwest::Client,
    Arc<MockGReader>,
) {
    let server = MockGReader::start().await.expect("start mock server");
    let tmp = common::unique_db_path(&format!("phases_{name}"));
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
///
/// OPT-008B：**feed 绑定是本人份匹配的前置**——URL 兜底现在只在同一订阅源内
/// 选行（跨源同 URL 各自独立成条目），未绑定 feed 的远端条目不再借全局 URL
/// 映射改写本地行。本夹具直接声明 feed 10 已绑定，聚焦 states 合并。
/// 生产边界（勿以「恒 feeds→states」论证）：`sync_now` 串联 feeds→states；
/// `sync_light`/后台自动同步只跑 states——纯后台路径不会先行绑定 feed，
/// 未绑定远端 feed 的条目停留在有限兼容分支（本地无对应源行，用户不可见）。
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
    db::set_feed_remote_id(&conn, feed_id, 10).unwrap();
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
    let aid = db::upsert_article_with_feed(&conn, feed_id, &a, false)
        .unwrap()
        .0;
    drop(conn);
    let mf_id = server.add_entry_ret(10, url, "Remote version", false, false);
    (aid, mf_id)
}

#[tokio::test]
async fn instant_push_only_pushes_and_drains_queue() {
    let (db, http, server) = setup("instant").await;
    let (aid, _mf_id) = seed_local_article(&db, &server, "http://127.0.0.1:8765/post/1").await;

    // 先跑一次 full states（含绑定回填），拿到 remote_id 绑定
    sync::states_phase(&db, &http, true)
        .await
        .expect("bind phase");
    {
        let conn = db.lock().await;
        let bound: Option<i64> = conn
            .query_row("SELECT remote_id FROM articles WHERE id = ?1", [aid], |r| {
                r.get(0)
            })
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
    let (conn,) = (db.lock().await,);
    let mf_id: i64 = conn
        .query_row("SELECT remote_id FROM articles WHERE id = ?1", [aid], |r| {
            r.get(0)
        })
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
async fn instant_push_read_broadcasts_dup_entries() {
    let (db, http, server) = setup("broadcast").await;
    let (aid, _mf_id) = seed_local_article(&db, &server, "http://127.0.0.1:8765/post/1").await;
    sync::states_phase(&db, &http, true)
        .await
        .expect("bind phase");

    // 跨源副本 entry（feed 11 同 URL——手机端另一源的副本）
    let dup_id = server.add_entry_ret(11, "http://127.0.0.1:8765/post/1", "Dup copy", false, false);
    // states 增量轮把副本记账到 remote_dup_ids
    sync::states_phase(&db, &http, true)
        .await
        .expect("record dup");

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
async fn feeds_and_states_phases_run_independently() {
    let (db, http, _server) = setup("phases").await;
    let (aid, _mf) = seed_local_article(&db, &_server, "http://127.0.0.1:8765/post/1").await;

    // feeds 阶段独立可跑：本地 feed 绑定远端 feed 10
    sync::feeds_phase(&db, &http).await.expect("feeds phase");
    {
        let conn = db.lock().await;
        let bound: Option<i64> = conn
            .query_row("SELECT remote_id FROM feeds WHERE feed_url = 'http://127.0.0.1:8765/local_feed.xml'", [], |r| r.get(0))
            .ok()
            .flatten();
        assert_eq!(bound, Some(10), "feed bound to remote id 10");
    }

    // states 阶段独立可跑：文章完成绑定
    sync::states_phase(&db, &http, true)
        .await
        .expect("states phase");
    let (conn,) = (db.lock().await,);
    let bound: Option<i64> = conn
        .query_row("SELECT remote_id FROM articles WHERE id = ?1", [aid], |r| {
            r.get(0)
        })
        .ok()
        .flatten();
    assert!(bound.is_some(), "article bound in states phase");
}

/// 未读数漂移根因回归：远端 changed_at 早于增量游标（手机很久前标读），
/// light 增量拉不到，full 对账必须收敛。
#[tokio::test]
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
            .query_row("SELECT is_read FROM articles WHERE id = ?1", [aid], |r| {
                r.get::<_, i64>(0).map(|v| v != 0)
            })
            .unwrap();
        assert!(
            is_read,
            "light sync must converge the stale remote read via unread-id reconcile"
        );
    }

    // full 对账：全量条目结果里直接应用状态 → 收敛
    sync::states_phase(&db, &http, true)
        .await
        .expect("full reconcile");
    let (conn,) = (db.lock().await,);
    let is_read: bool = conn
        .query_row("SELECT is_read FROM articles WHERE id = ?1", [aid], |r| {
            r.get::<_, i64>(0).map(|v| v != 0)
        })
        .unwrap();
    assert!(
        is_read,
        "full reconcile must converge the stale remote read"
    );
}

/// 本地直连源漏抓的条目（本地文章数 < 远端）→ full 同步对比拉取补齐。
/// 根因：Miniflux 协议时期 pull_entries 只对「完全失败」的源做兜底拉取
/// （0ba940f 协议切换时该兜底随 Miniflux 专用路径一并移除），正常直连源
/// 若 feed 只提供摘要/漏了几条，本地永久缺失。现在 full 对账会对已绑定源
/// 的远端条目逐一 upsert 补齐（幂等，不重复、不覆盖已有正文/已读）。
#[tokio::test]
async fn full_reconcile_backfills_missing_local_entries() {
    let (db, http, server) = setup("backfill").await;
    // 本地造一篇直连文章 + 绑定远端 feed 10
    let (aid, _mf) = seed_local_article(&db, &server, "http://127.0.0.1:8765/post/1").await;
    // feeds 阶段绑定本地 feed → 远端 feed 10
    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase bind");

    // 远端 feed 10 加一条本地完全没有的条目（模拟直连源漏抓）
    server.add_entry_ret(
        10,
        "http://127.0.0.1:8765/missing/post/999",
        "Remote-only entry",
        false,
        false,
    );

    // full 同步：绑定回填 + 对比拉取补齐缺失条目
    sync::states_phase(&db, &http, true)
        .await
        .expect("full reconcile");

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
        // 来源应标记为 miniflux（full 对账从同步后端拉回的条目；该值是与 direct 相对的来源标记，非「兜底」）
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
        conn.query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
            .unwrap()
    };
    sync::states_phase(&db, &http, true)
        .await
        .expect("second full reconcile");
    let after: i64 = {
        let conn = db.lock().await;
        conn.query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(before, after, "二次 full 同步不应重复入库（幂等）");

    // ---- 补强（TASK-054 P4）：对账补拉时，本地「已入队未推送」的已读不得被
    // 远端陈旧未读覆盖。之前这个测试完全没有 pending 数据，所以「pending 查询
    // 去掉 read/unread 动作过滤」的变异无法被观察到（TASK-054 捕获性验证结论）。
    //
    // 构造：edit-tag 返回 500 → 推送失败、队列保留（pending 命中守卫）；
    // 远端条目保持未读（服务端没有应用我们的标记）。守卫消费的是
    // db/sync_map.rs:91 的共享 pending 查询——该查询的变异由本段捕获。
    server.set_fail_edit_tag(true);
    {
        let conn = db.lock().await;
        db::set_read(&conn, aid, true).unwrap();
        db::enqueue_sync(&conn, Some(aid), None, "read", None).unwrap();
    }
    let report = sync::states_phase(&db, &http, true)
        .await
        .expect("full reconcile with pending read");
    assert!(
        !report.errors.is_empty(),
        "前置条件：edit-tag 注入的失败应体现在同步报告里"
    );
    {
        let conn = db.lock().await;
        // 队列必须还在（推送确实失败了，本地变更仍待推）——pending 是本段的前提
        let queued: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_queue WHERE article_id = ?1 AND action = 'read'",
                [aid],
                |r| r.get(0),
            )
            .unwrap();
        assert!(queued > 0, "推送失败后队列应保留（pending 前提）");
        let is_read: bool = conn
            .query_row("SELECT is_read FROM articles WHERE id = ?1", [aid], |r| {
                r.get::<_, i64>(0).map(|v| v != 0)
            })
            .unwrap();
        assert!(
            is_read,
            "pending 守卫必须生效：本地已读不得被陈旧的远端未读覆盖"
        );
    }
}

/* ============================================================
B2：GR read/starred 权威集合的 contents 回补（未绑定行收敛）
============================================================ */

/// 夹具：feeds_phase 绑定 mock feed 10 → 本地源；直连入库一行（URL 给定）；
/// 远端加一条「已读、published 早于游标」的条目——它不在 light 增量窗口
/// （窗口列 published 严格大于），只能经 read 权威集合的回补通道可达。
/// 返回 (直连行 id, 远端 entry id, 直连行所在本地 feed id)。
async fn seed_backfill_fixture(
    db: &Arc<Mutex<rusqlite::Connection>>,
    server: &MockGReader,
    http: &reqwest::Client,
    entry_url: &str,
    direct_url: &str,
    t0: i64,
) -> (i64, i64, i64) {
    sync::feeds_phase(db, http).await.expect("feeds phase");
    let eid = server.add_entry_with_times(
        10,
        entry_url,
        "Backfill entry",
        true, // read 权威集合成员
        false,
        t0 - 100, // published 早于游标：不在 light 增量窗口
        t0 + 10,
    );
    let (aid, feed_id) = {
        let conn = db.lock().await;
        let feed_id: i64 = conn
            .query_row("SELECT id FROM feeds WHERE remote_id = 10", [], |r| {
                r.get(0)
            })
            .unwrap();
        let a = db::NewArticle {
            guid: format!("direct-{direct_url}"),
            url: Some(direct_url.into()),
            title: "Direct article".into(),
            author: None,
            summary: None,
            content_html: Some("<p>direct body</p>".into()),
            body_text: "direct body".into(),
            image_url: None,
            enclosure_url: None,
            enclosure_mime: None,
            duration_sec: None,
            published_at: Some("2026-01-01T00:00:00+00:00".into()),
            source: "direct".into(),
        };
        let (aid, inserted) = db::upsert_article_with_feed(&conn, feed_id, &a, false).unwrap();
        assert!(inserted, "直连行必须新插入");
        (aid, feed_id)
    };
    {
        let conn = db.lock().await;
        db::set_last_sync_ts(&conn, t0).unwrap();
    }
    (aid, eid, feed_id)
}

/// 门禁 a（B2 回补核心症状）：直连入库、URL 可归一匹配、从未被列举窗口绑定
/// 的行，经 light 同步从 read 权威集合收敛为已读并绑定同一行——修前该行
/// 永久漂移（reconcile 对不在映射里的 id 无行可写）。
#[tokio::test]
async fn light_sync_backfills_read_collection_row_missing_from_listing_window() {
    const T0: i64 = 1_700_000_000;
    let (db, http, server) = setup("gr_backfill_read").await;
    let (_aid, eid, _feed) = seed_backfill_fixture(
        &db,
        &server,
        &http,
        "http://127.0.0.1:8765/backfill/a",
        "http://127.0.0.1:8765/backfill/a",
        T0,
    )
    .await;

    let r = sync::states_phase(&db, &http, false).await.expect("light");
    assert!(r.errors.is_empty(), "light 轮不应有错误：{:?}", r.errors);
    {
        let conn = db.lock().await;
        let (count, is_read, remote): (i64, i64, Option<i64>) = conn
            .query_row(
                "SELECT COUNT(*), MAX(is_read), MAX(remote_id) FROM articles WHERE url = ?1",
                rusqlite::params!["http://127.0.0.1:8765/backfill/a"],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(count, 1, "回补必须命中既有行，不得另开新行");
        assert_eq!(is_read, 1, "read 权威集合的未绑定行必须收敛为已读");
        assert_eq!(remote, Some(eid), "回补行必须绑定远端 entry id");
        assert!(
            db::last_sync_ts(&conn).unwrap() > T0,
            "回补成功轮的游标照常推进"
        );
    }
}

/// 门禁 b：URL 归一化不匹配的直连行不得被误绑/误标——回补走 ③ 分支独立
/// upsert 新行（各自绑定/读藏），旧行为不变。
#[tokio::test]
async fn light_sync_backfill_does_not_bind_url_mismatched_rows() {
    const T0: i64 = 1_700_000_000;
    let (db, http, server) = setup("gr_backfill_mismatch").await;
    let (aid, eid, _feed) = seed_backfill_fixture(
        &db,
        &server,
        &http,
        "http://127.0.0.1:8765/backfill/b",
        "http://127.0.0.1:8765/direct/own-b",
        T0,
    )
    .await;

    let r = sync::states_phase(&db, &http, false).await.expect("light");
    assert!(r.errors.is_empty(), "light 轮不应有错误：{:?}", r.errors);
    {
        let conn = db.lock().await;
        // 直连行：保持未读、未绑定、URL 不被改写
        let (is_read, remote): (i64, Option<i64>) = conn
            .query_row(
                "SELECT is_read, remote_id FROM articles WHERE id = ?1",
                [aid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(is_read, 0, "URL 不匹配的直连行不得被误标已读");
        assert_eq!(remote, None, "URL 不匹配的直连行不得被误绑");
        // 回补条目：独立新行（source='miniflux'），自己的读状态照常落地
        let (count, new_read, new_remote, source): (i64, i64, Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT COUNT(*), MAX(is_read), MAX(remote_id), MAX(source) FROM articles WHERE url = ?1",
                rusqlite::params!["http://127.0.0.1:8765/backfill/b"],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(count, 1, "回补条目独立成行（③ 分支行为不变）");
        assert_eq!(new_read, 1);
        assert_eq!(new_remote, Some(eid));
        assert_eq!(source.as_deref(), Some("miniflux"));
    }
}

/// 门禁 c：contents 回补请求失败 → 该侧跳过对账、直连行状态守恒（不误标）、
/// 游标保留；下一轮恢复后收敛。
#[tokio::test]
async fn gr_backfill_failure_skips_reconcile_side_and_keeps_cursor() {
    const T0: i64 = 1_700_000_000;
    let (db, http, server) = setup("gr_backfill_failure").await;
    let (aid, _eid, _feed) = seed_backfill_fixture(
        &db,
        &server,
        &http,
        "http://127.0.0.1:8765/backfill/c",
        "http://127.0.0.1:8765/backfill/c",
        T0,
    )
    .await;

    server.set_fail_item_contents(true);
    let r1 = sync::states_phase(&db, &http, false).await.expect("light");
    assert!(!r1.errors.is_empty(), "回补失败必须可见：{:?}", r1.errors);
    {
        let conn = db.lock().await;
        let (is_read, remote): (i64, Option<i64>) = conn
            .query_row(
                "SELECT is_read, remote_id FROM articles WHERE id = ?1",
                [aid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(is_read, 0, "回补失败侧跳过对账：状态必须守恒");
        assert_eq!(remote, None, "回补失败轮不得留下半程绑定");
        assert_eq!(
            db::last_sync_ts(&conn).unwrap(),
            T0,
            "回补失败必须保留游标（fail-closed）"
        );
    }

    server.set_fail_item_contents(false);
    let r2 = sync::states_phase(&db, &http, false)
        .await
        .expect("light retry");
    assert!(r2.errors.is_empty(), "恢复轮不应有错误：{:?}", r2.errors);
    {
        let conn = db.lock().await;
        let is_read: i64 = conn
            .query_row("SELECT is_read FROM articles WHERE id = ?1", [aid], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(is_read, 1, "下一轮回补恢复后必须收敛");
        assert!(db::last_sync_ts(&conn).unwrap() > T0, "恢复轮游标照常推进");
    }
}

/* ============================================================
B2：Fever with_ids 回补的本地正文预过滤（请求计数）
============================================================ */

/// 门禁（B2 Fever 预过滤）：已有正文的条目不再出现在 with_ids 请求里；
/// 服务端空正文的条目（首轮已拉、id ≤ 游标、本地无正文）仍回补。修前
/// unread∪saved 全集每轮重拉（审计实测：2 万条库最坏 403 次串行往返/轮）。
/// 空正文夹具用 `add_entry_full(content="")`：空内容走条目私有 contents 行，
/// 不会连带清掉 e1-e3 共享的正文行（同内容按内容哈希共享行）。
#[tokio::test]
async fn fever_with_ids_backfill_skips_entries_with_local_body() {
    let server = MockGReader::start().await.expect("start mock server");
    let tmp = common::unique_db_path("phases_fever_prefilter");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).expect("open db");
    db::set_setting(&conn, "sync_protocol", "fever").unwrap();
    db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
    db::set_setting(&conn, "greader_username", "user-f").unwrap();
    db::set_setting(&conn, "greader_password", "pw-f").unwrap();
    server.set_fever_serve_entries(true);
    let db = Arc::new(Mutex::new(conn));
    let http = app_lib::ingestion::build_client(10);

    sync::feeds_phase(&db, &http)
        .await
        .expect("fever feeds phase");
    let e1 = server.add_entry_ret(10, "http://127.0.0.1:8765/fever/p1", "P1", false, false);
    let e2 = server.add_entry_ret(10, "http://127.0.0.1:8765/fever/p2", "P2", false, false);
    let e3 = server.add_entry_ret(10, "http://127.0.0.1:8765/fever/p3", "P3", false, false);
    let e4 = server.add_entry_full(
        10,
        "http://127.0.0.1:8765/fever/p4",
        "P4",
        false,
        false,
        "", // 服务端空正文：本地拉入后仍无正文，每轮留在 with_ids 通道
        Vec::new(),
    );

    // 首轮 full：历史回溯取尽四条；e1-e3 有正文，e4 空正文
    let r1 = sync::states_phase(&db, &http, true)
        .await
        .expect("fever full");
    assert!(
        r1.errors.is_empty(),
        "fever full 轮不应有错误：{:?}",
        r1.errors
    );

    let log_before = server.fever_query_log().len();
    let r2 = sync::states_phase(&db, &http, false)
        .await
        .expect("fever light");
    assert!(
        r2.errors.is_empty(),
        "fever light 轮不应有错误：{:?}",
        r2.errors
    );

    let mut requested: Vec<i64> = Vec::new();
    for q in server.fever_query_log().into_iter().skip(log_before) {
        if let Some(pos) = q.find("with_ids=") {
            let csv = q[pos + "with_ids=".len()..]
                .split('&')
                .next()
                .unwrap_or_default();
            requested.extend(csv.split(',').filter_map(|s| s.parse::<i64>().ok()));
        }
    }
    requested.sort_unstable();
    requested.dedup();
    assert_eq!(
        requested,
        vec![e4],
        "with_ids 必须只回补无正文的 e4；已有正文的 e1-e3 不得重拉（审计 F1 固定成本）"
    );
    {
        let conn = db.lock().await;
        let bound: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM articles WHERE url = ?1 AND remote_id = ?2",
                rusqlite::params!["http://127.0.0.1:8765/fever/p4", e4],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bound, 1, "e4 经 with_ids 回补后保持绑定");
    }
}
