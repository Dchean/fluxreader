//! REQ-002/003 双向同步缺口的复现测试（TASK-031）。
//!
//! 这些测试**故意失败语义下的当前实现**：每个用例复现一个用户报告的同步缺口，
//! 断言"当前实现确实不回传/会回滚"。修复任务转正时，把对应断言反转为期望行为
//! 并去掉 #[ignore]。缺口定位与修复设计见 .workflow-kit/docs/FINDINGS-SYNC-GAP.md。
//!
//! 运行：cargo test --test sync_gap_repro_e2e -- --ignored --nocapture

mod mock_greader;

use app_lib::db;
use app_lib::sync;
use mock_greader::{subscription_edit_actions, MockGReader};
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
    let tmp = std::env::temp_dir().join(format!(
        "fluxreader_gap_{}_{}_{}.db",
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
    (Arc::new(Mutex::new(conn)), http, server)
}

/// 本地造一篇直连文章（URL 与 mock feed 10 的 entry 对齐，供绑定回填）。
async fn seed_local_article(
    db: &Arc<Mutex<rusqlite::Connection>>,
    server: &MockGReader,
    url: &str,
    read: bool,
) -> i64 {
    // 远端造同 URL entry（绑定回填的匹配目标——mock entries 默认为空）
    server.add_entry(10, url, "Remote Entry", "unread", false);
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
    let aid = db::upsert_article_with_feed(&conn, feed_id, &a, false)
        .unwrap()
        .0;
    if read {
        db::set_read(&conn, aid, true).unwrap();
    }
    aid
}

/// A-1 修复后的期望行为（原复现测试转正）：删除已绑定远端的订阅会 best-effort
/// 退订（GReader），并写入删除墓碑——下次 pull 不得按远端订阅列表复活已删订阅。
#[tokio::test]
async fn deleted_feed_stays_deleted_and_unsubscribes() {
    let (db, http, server) = setup("gap_delete").await;
    let feed_id = {
        let conn = db.lock().await;
        let folder_id = db::create_folder(&conn, "测试分类", "article").unwrap();
        db::insert_feed(
            &conn,
            "http://127.0.0.1:8765/local_feed.xml", // 与 mock feed 10 同 URL（远端订阅存在）
            None,
            "Local Direct Feed",
            None,
            folder_id,
            "inherit",
            true,
            false,
        )
        .unwrap()
    };
    // 绑定远端：feeds_phase 按 URL 匹配写入 remote_id=10
    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase (bind)");
    {
        let conn = db.lock().await;
        let bound: Option<i64> = conn
            .query_row(
                "SELECT remote_id FROM feeds WHERE id = ?1",
                [feed_id],
                |r| r.get(0),
            )
            .ok()
            .flatten();
        assert_eq!(bound, Some(10), "前置条件：订阅已绑定远端 feed 10");
    }

    // 删除（命令层真实逻辑）：写墓碑 + 删除本地 + 返回待退订的远端 id
    let unsubscribe = {
        let conn = db.lock().await;
        app_lib::commands::record_feed_deletion(&conn, feed_id).expect("record deletion")
    };
    assert!(
        matches!(unsubscribe, Some((10, _))),
        "已绑定远端且同步已配置时应返回待退订目标"
    );

    // ① 删除后立即同步：远端订阅列表仍含 feed/10（列表滞后），墓碑必须阻止复活
    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase right after delete");
    let revived = {
        let conn = db.lock().await;
        conn.query_row(
            "SELECT COUNT(*) FROM feeds WHERE feed_url = 'http://127.0.0.1:8765/local_feed.xml'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    };
    assert_eq!(revived, 0, "已删除订阅不得被 pull 复活（墓碑生效）");

    // ② 退订远端（best-effort，GReader）：成功后退订墓碑解除
    let (remote_id, feed_url) = unsubscribe.expect("unsubscribe target");
    assert!(
        sync::unsubscribe_remote(&db, &http, remote_id, &feed_url).await,
        "退订应成功（mock 支持 ac=unsubscribe）"
    );
    assert!(
        subscription_edit_actions(&server)
            .iter()
            .any(|(ac, s)| ac == "unsubscribe" && s.contains("feed/10")),
        "远端应收到 ac=unsubscribe"
    );
    {
        let conn = db.lock().await;
        assert!(
            db::feed_tombstones(&conn).unwrap().is_empty(),
            "退订成功（远端确认）后墓碑应清除"
        );
    }

    // ③ 墓碑已清、远端已移除该订阅：再次同步仍不复活
    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase after unsubscribe");
    let existed = {
        let conn = db.lock().await;
        conn.query_row(
            "SELECT COUNT(*) FROM feeds WHERE feed_url = 'http://127.0.0.1:8765/local_feed.xml'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    };
    assert_eq!(existed, 0, "退订后同步不得复活（远端已不再列出该订阅）");
}

/// A-2（REQ-002）：订阅改名不回传远端（edit_subscription 已实现但从未接线）。
/// 当前实现：update_feed 只更新本地 DB，不产生任何远端调用；双端标题永久分歧。
/// 修复后期望：收到 ac=edit 的 subscription/edit 且 t=新标题，断言反转为「已推送」。
#[tokio::test]
#[ignore = "REQ-002 repro: 订阅改名不回传远端（TASK-031 定位，修复任务转正）"]
async fn feed_rename_never_reaches_backend() {
    let (db, http, _server) = setup("gap_rename").await;
    let (conn,) = (db.lock().await,);
    let folder_id = db::create_folder(&conn, "测试分类", "article").unwrap();
    let feed_id = db::insert_feed(
        &conn,
        "http://127.0.0.1:8765/local_feed.xml",
        None,
        "Old Title",
        None,
        folder_id,
        "inherit",
        true,
        false,
    )
    .unwrap();
    drop(conn);

    // 本地改名（对应 commands::update_feed 的实际行为：仅 db::update_feed）
    {
        let conn = db.lock().await;
        db::update_feed(
            &conn,
            feed_id,
            Some("Brand New Title"),
            None,
            None,
            None,
            None,
        )
        .unwrap();
    }

    // 下一次 feeds 同步：本地没有 add_feed 队列项，push 无事可做
    sync::feeds_phase(&db, &http).await.expect("feeds phase");

    let edits = subscription_edit_actions(&_server);
    assert!(
        !edits
            .iter()
            .any(|(ac, s)| ac == "edit" && s.contains("feed/10")),
        "缺口复现：改名单独发生时未向远端发送 subscription/edit"
    );
    assert!(
        !edits.iter().any(|(ac, _)| ac == "edit"),
        "缺口复现：整个同步过程未发送任何编辑动作"
    );
}

/// A-5 修复后的期望行为（原复现测试转正）：离线（未配置凭据）期间的已读变更
/// 现在也会入队（commands::record_read_state），连接后 states_phase 推送段补推。
#[tokio::test]
async fn offline_read_change_pushed_after_connect() {
    // "离线"阶段：未配置任何凭据
    let server = MockGReader::start().await.expect("start mock server");
    let tmp = std::env::temp_dir().join(format!(
        "fluxreader_gap_offline_{}_{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&tmp);
    let db = Arc::new(Mutex::new(db::open(&tmp).expect("open db")));
    let http = app_lib::ingestion::build_client(10);

    // 离线阶段：走命令层真实逻辑（record_read_state = set_read + 入队）
    let aid = seed_local_article(&db, &server, "http://127.0.0.1:8765/post/1", false).await;
    {
        let conn = db.lock().await;
        app_lib::commands::record_read_state(&conn, aid, true).expect("record read");
    }
    let queued: i64 = {
        let conn = db.lock().await;
        conn.query_row("SELECT COUNT(*) FROM sync_queue", [], |r| r.get(0))
            .unwrap()
    };
    assert!(queued >= 1, "离线变更也应写入同步队列（A-5）");

    // "连接"：配置凭据后做全量同步（绑定回填）——首推时条目可能尚未绑定，
    // 队列保留；第二次 states_phase 在绑定完成后补推
    {
        let conn = db.lock().await;
        db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
        db::set_setting(&conn, "greader_username", "test").unwrap();
        db::set_setting(&conn, "greader_password", "test-token").unwrap();
    }
    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase (bind feed)");
    sync::states_phase(&db, &http, true)
        .await
        .expect("states phase (bind article)");

    // 连接后的补推：推送段消费队列（即时推送路径与 states_phase 推送段共用 plan_push）
    sync::push_states_now(&db, &http).await;

    // 修复期望：离线变更被补推到远端
    let updates = server.status_updates.lock().unwrap();
    assert!(
        updates.iter().any(|(_eid, st)| *st == "read"),
        "修复期望：离线已读变更在连接后被补推（收到 edit-tag read）"
    );
    let remains: i64 = {
        let conn = db.lock().await;
        conn.query_row("SELECT COUNT(*) FROM sync_queue", [], |r| r.get(0))
            .unwrap()
    };
    drop(updates);
    assert_eq!(remains, 0, "补推完成后队列应清空");
}

/// C-1 修复后的期望行为：GReader 权威状态集合拉取失败时跳过对账，
/// 绝不把"拉取失败"当"空集合"清空本地收藏。
#[tokio::test]
async fn reconcile_skipped_when_state_fetch_fails() {
    let (db, http, server) = setup("gap_c1").await;
    let (aid, _rid) = {
        let conn = db.lock().await;
        let folder_id = db::create_folder(&conn, "测试分类", "article").unwrap();
        let feed_id = db::insert_feed(
            &conn,
            "http://127.0.0.1:8765/local_feed.xml",
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
            guid: "guid-c1".into(),
            url: Some("http://127.0.0.1:8765/post/1".into()),
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
        (aid, ())
    };

    // 首次全量同步：绑定 feed 与文章
    sync::feeds_phase(&db, &http).await.expect("feeds phase");
    sync::states_phase(&db, &http, true)
        .await
        .expect("states phase 1");

    // 本地收藏（走命令层真实逻辑，入队）
    {
        let conn = db.lock().await;
        app_lib::commands::record_star_state(&conn, aid, true).expect("record star");
    }
    // 推送收藏成功后，注入状态集合拉取故障
    sync::states_phase(&db, &http, true)
        .await
        .expect("states phase 2 (push star)");
    server
        .fail_stream_ids
        .store(true, std::sync::atomic::Ordering::SeqCst);

    // 轻量同步触发对账：read/starred 集合拉取失败 → 必须跳过对账
    let report = sync::states_phase(&db, &http, false)
        .await
        .expect("states phase 3");
    assert!(
        report.errors.iter().any(|e| e.contains("状态对账跳过")),
        "修复期望：对账因集合拉取失败被跳过并记录 errors"
    );

    // 本地收藏保持：绝不被"空集合对账"静默取消
    let conn = db.lock().await;
    let starred: i64 = conn
        .query_row(
            "SELECT is_starred FROM articles WHERE id = ?1",
            [aid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(starred, 1, "修复期望：拉取失败时本地收藏不被清空");
}
