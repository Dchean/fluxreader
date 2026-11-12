//! OPT-009 阶段 B1：订阅 outbox 生产执行器 + GReader 严格 wire 端到端回归。
//!
//! 用真实 loopback mock 服务端 + 独立文件库驱动生产路径（feeds_phase →
//! subscription_outbox 执行器 / record_feed_edit / opml_import），断言对象是
//! **实际 HTTP 请求表单**与**持久队列状态**，不用 UI 回显当远端真值：
//!
//! 1. quick_add 成功 → 绑定写失败（真实 SQLite RAISE(ABORT) 注入）→ 队项保留
//!    remote_created、重启后重试且**不重复 quick_add**、最终绑定+挂载收敛出队；
//! 2. 分类挂载失败 → 队项保留、重试收敛；重试不重复 quick_add；
//! 3. 移动分类 wire：`a`=目标 label + `r`=旧 label，**其他 label 保留**；
//!    改名只发 `t` 不发分类参数；
//! 4. 旧 HTTP 成功不得确认（prune）在途期间新入队的编辑意图（响应屏障分离）；
//! 5. 跨账号同 URL：B 的本地动作不得删除 A 的远端已建（remote_created）意图，
//!    执行器只发当前账号 key 的队项；
//! 6. Fever 不支持订阅写回 → 队项进 local_only 终态（保留展示、不重试），
//!    不向远端发任何订阅写请求；
//! 7. quick_add 响应在途原地更新目标 → 挂载必须发送重读后的最新目标；
//! 8. 挂载响应在途：helper 级（原地更新）与**生产命令级**（edit 行替换订阅行）
//!    两条路径都不得被旧成功清掉在途新意图。
//!
//! B1-R3（复审返工）增补：
//! 9. 未发送 A→B→C 折叠保留链起点（wire `a=C,r=A`、远端只留最终 label）；
//! 10. 在途移动成功 → 替代行移除基准重定为已发送目标（远端不残留中间 label）；
//! 11. OPML 导入走类型化 subscribe（无 legacy 队项），quick_add / 挂载失败后
//!     重试收敛、只创建一次远端订阅、分类目标不丢；
//! 12. 异账号暂停意图不保护本账号的 pull 删除对账；
//! 13. 保护集合读取失败 fail-closed（零删除 + 错误可见，恢复后收敛）。
//!
//! 运行：cargo test --test opt009_phaseb_e2e

mod common;
mod mock_greader;

use app_lib::db;
use app_lib::sync;
use mock_greader::{last_subscription_edit_form, MockGReader, MockSubscription};
use std::sync::Arc;
use tokio::sync::Mutex;

/// 账号指纹的手工拼装：与生产约定同构（协议 + 规范化服务器根 + 用户名；
/// `http://127.0.0.1:<port>` 无尾斜杠/无路径，规范化后原样）。
fn acct_key(protocol: &str, server: &MockGReader, user: &str) -> String {
    format!("{protocol}|{}|{user}", server.url())
}

fn open_with_credentials(server: &MockGReader, protocol: &str, user: &str) -> rusqlite::Connection {
    let tmp = common::unique_db_path("opt009b_setup");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();
    db::set_setting(&conn, "sync_protocol", protocol).unwrap();
    db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
    db::set_setting(&conn, "greader_username", user).unwrap();
    db::set_setting(&conn, "greader_password", "test-token").unwrap();
    conn
}

fn seed_bound_feed(
    conn: &rusqlite::Connection,
    url: &str,
    folder_name: &str,
    remote_id: i64,
) -> (i64, i64) {
    let folder = db::create_folder(conn, folder_name, "article").unwrap();
    let feed = db::insert_feed(
        conn,
        url,
        None,
        "Bound Feed",
        None,
        folder,
        "inherit",
        true,
        false,
    )
    .unwrap();
    db::set_feed_remote_id(conn, feed, remote_id).unwrap();
    (folder, feed)
}

fn quickadd_count(server: &MockGReader, url: &str) -> usize {
    server
        .subscribed_urls
        .lock()
        .unwrap()
        .iter()
        .filter(|u| u.as_str() == url)
        .count()
}

fn form_has(form: &[(String, String)], k: &str, v: &str) -> bool {
    form.iter().any(|(fk, fv)| fk == k && fv == v)
}

/* ============================================================
1. quick_add 成功 → 绑定失败 → 重启重试（不得重复 quick_add）
============================================================ */

#[tokio::test]
async fn subscribe_bind_failure_survives_restart_without_second_quickadd() {
    let server = MockGReader::start().await.expect("start mock");
    server.subscriptions.lock().unwrap().clear();
    let tmp = common::unique_db_path("opt009b_bind");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = "http://127.0.0.1:1/opt009-bind.xml";
    let key = acct_key("greader", &server, "test");

    {
        let conn = db::open(&tmp).unwrap();
        db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
        db::set_setting(&conn, "greader_username", "test").unwrap();
        db::set_setting(&conn, "greader_password", "test-token").unwrap();
        let folder = db::create_folder(&conn, "分类A", "article").unwrap();
        db::insert_feed(
            &conn,
            url,
            None,
            "Bind Feed",
            None,
            folder,
            "inherit",
            true,
            false,
        )
        .unwrap();
        // 绑定写失败注入：真实 SQLite 约束失败路径（RAISE(ABORT)），非测试钩子。
        conn.execute_batch(
            "CREATE TRIGGER test_fail_bind BEFORE UPDATE OF remote_id ON feeds
             WHEN NEW.remote_id IS NOT NULL BEGIN
                 SELECT RAISE(ABORT, 'simulated bind failure');
             END;",
        )
        .unwrap();
        let gen = db::sync_generation(&conn).unwrap();
        db::enqueue_subscribe(&conn, gen, &key, url, None, Some("分类A")).unwrap();
    }

    // 第一轮：quick_add 成功、绑定写失败 → 队项保留且已推进 remote_created
    let dbm = Arc::new(Mutex::new(db::open(&tmp).unwrap()));
    let report = sync::feeds_phase(&dbm, &http).await.expect("feeds phase 1");
    {
        let conn = dbm.lock().await;
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(
            items.len(),
            1,
            "绑定失败必须保留队项（不立即 done/prune）: {report:?}"
        );
        assert_eq!(
            items[0].stage, "remote_created",
            "quick_add 成功必须先推进 remote_created（重启后不得重复 quick_add）"
        );
        assert!(items[0].remote_id.is_some(), "remote_id 必须已记录");
        assert!(items[0].attempts >= 1, "绑定失败必须累计 attempts");
        assert!(
            items[0].last_error.is_some(),
            "绑定失败必须记录 last_error（UI 重试统计来源）"
        );
        let bound: Option<i64> = conn
            .query_row(
                "SELECT remote_id FROM feeds WHERE feed_url = ?1",
                [url],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bound, None, "绑定写失败：本地 remote_id 不得写入");
        conn.execute_batch("DROP TRIGGER test_fail_bind;").unwrap();
    }
    assert_eq!(
        quickadd_requests(&server),
        1,
        "第一轮只允许一次 quick_add（request-level）"
    );
    assert_eq!(
        quickadd_count(&server, url),
        1,
        "第一轮只允许创建一次远端订阅（created-count）"
    );

    // 重启（丢连接重开同一文件库）→ 重试必须走 remote_created 分支
    drop(dbm);
    let dbm = Arc::new(Mutex::new(db::open(&tmp).unwrap()));
    sync::feeds_phase(&dbm, &http).await.expect("feeds phase 2");
    {
        let conn = dbm.lock().await;
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "绑定+挂载完整确认后必须精确出队"
        );
        let bound: Option<i64> = conn
            .query_row(
                "SELECT remote_id FROM feeds WHERE feed_url = ?1",
                [url],
                |r| r.get(0),
            )
            .unwrap();
        assert!(bound.is_some(), "重启重试必须完成绑定");
        let form = last_subscription_edit_form(&server);
        assert!(
            form_has(&form, "a", "user/-/label/分类A"),
            "目标分类挂载必须保留并最终推达，实际 {form:?}"
        );
    }
    assert_eq!(
        quickadd_requests(&server),
        1,
        "重启重试不得重复发送 quick_add（request-level；mock 幂等化后 created-count 发现不了重复请求）"
    );
    assert_eq!(
        quickadd_count(&server, url),
        1,
        "远端只创建一次（created-count）"
    );
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
2. 分类挂载失败 → 队项保留 → 重试收敛（不重复 quick_add）
============================================================ */

#[tokio::test]
async fn subscribe_mount_failure_keeps_row_then_converges() {
    let server = MockGReader::start().await.expect("start mock");
    server.subscriptions.lock().unwrap().clear();
    let tmp = common::unique_db_path("opt009b_mount");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = "http://127.0.0.1:1/opt009-mount.xml";
    let key = acct_key("greader", &server, "test");

    {
        let conn = db::open(&tmp).unwrap();
        db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
        db::set_setting(&conn, "greader_username", "test").unwrap();
        db::set_setting(&conn, "greader_password", "test-token").unwrap();
        let folder = db::create_folder(&conn, "目标分类", "article").unwrap();
        db::insert_feed(
            &conn,
            url,
            None,
            "Mount Feed",
            None,
            folder,
            "inherit",
            true,
            false,
        )
        .unwrap();
        let gen = db::sync_generation(&conn).unwrap();
        db::enqueue_subscribe(&conn, gen, &key, url, None, Some("目标分类")).unwrap();
    }

    let dbm = Arc::new(Mutex::new(db::open(&tmp).unwrap()));
    server.set_fail_subscription_edit(true);
    let report = sync::feeds_phase(&dbm, &http).await.expect("feeds phase 1");
    {
        let conn = dbm.lock().await;
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1, "挂载失败必须保留队项: {report:?}");
        assert_eq!(items[0].stage, "remote_created");
        assert!(items[0].attempts >= 1, "挂载失败必须累计 attempts");
        let bound: Option<i64> = conn
            .query_row(
                "SELECT remote_id FROM feeds WHERE feed_url = ?1",
                [url],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            bound.is_some(),
            "绑定与挂载分离：绑定已完成（不得因挂载失败回滚）"
        );
    }

    server.set_fail_subscription_edit(false);
    sync::feeds_phase(&dbm, &http).await.expect("feeds phase 2");
    {
        let conn = dbm.lock().await;
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "挂载重试成功后完整意图确认出队"
        );
        let form = last_subscription_edit_form(&server);
        assert!(
            form_has(&form, "ac", "edit") && form_has(&form, "a", "user/-/label/目标分类"),
            "重试必须补发分类挂载，实际 {form:?}"
        );
        assert!(
            form.iter().all(|(k, _)| k != "r"),
            "新增订阅无旧 label，不得发出 r=，实际 {form:?}"
        );
    }
    assert_eq!(
        quickadd_requests(&server),
        1,
        "挂载重试不得重复发送 quick_add（request-level）"
    );
    assert_eq!(
        quickadd_count(&server, url),
        1,
        "远端只创建一次（created-count）"
    );
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
3. 移动分类 wire：a=目标 + r=旧，其他 label 保留；改名只发 t
============================================================ */

#[tokio::test]
async fn move_sends_add_and_remove_labels_preserving_others() {
    let server = MockGReader::start().await.expect("start mock");
    let tmp = common::unique_db_path("opt009b_move");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = format!("{}/opt009-move.xml", server.url());
    let key = acct_key("greader", &server, "test");

    {
        // 远端订阅已在两个分类：旧分类（本次要移除）+ 保留分类（不得被动）
        *server.subscriptions.lock().unwrap() = vec![MockSubscription {
            id: "feed/10".into(),
            title: "Move Feed".into(),
            url: url.clone(),
            html_url: None,
            categories: vec![
                ("旧分类".into(), "folder".into()),
                ("保留分类".into(), "folder".into()),
            ],
        }];
    }

    let dbm = {
        let conn = db::open(&tmp).unwrap();
        db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
        db::set_setting(&conn, "greader_username", "test").unwrap();
        db::set_setting(&conn, "greader_password", "test-token").unwrap();
        seed_bound_feed(&conn, &url, "旧分类", 10);
        db::create_folder(&conn, "新分类", "article").unwrap();
        Arc::new(Mutex::new(conn))
    };

    // ① 移动：a=目标 + r=旧（只发 a= 是追加分类，不是移动）
    {
        let conn = dbm.lock().await;
        let gen = db::sync_generation(&conn).unwrap();
        db::enqueue_edit(
            &conn,
            gen,
            &key,
            &url,
            10,
            None,
            Some("新分类"),
            Some("旧分类"),
        )
        .unwrap();
    }
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (move)");
    {
        let form = last_subscription_edit_form(&server);
        assert!(
            form_has(&form, "ac", "edit") && form_has(&form, "s", "feed/10"),
            "移动必须发 ac=edit s=feed/10，实际 {form:?}"
        );
        assert!(
            form_has(&form, "a", "user/-/label/新分类"),
            "移动必须发 a=目标 label，实际 {form:?}"
        );
        assert!(
            form_has(&form, "r", "user/-/label/旧分类"),
            "移动必须发 r=旧 label（移除原分类），实际 {form:?}"
        );
        let cats = server
            .subscriptions
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.id == "feed/10")
            .unwrap()
            .categories
            .clone();
        let labels: Vec<&str> = cats.iter().map(|(l, _)| l.as_str()).collect();
        assert!(labels.contains(&"新分类"), "远端应加入目标分类: {labels:?}");
        assert!(
            !labels.contains(&"旧分类"),
            "远端应从原分类消失（真实移动语义）: {labels:?}"
        );
        assert!(
            labels.contains(&"保留分类"),
            "非目标 label 必须保留（r 只移除旧 label）: {labels:?}"
        );
        let conn = dbm.lock().await;
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "移动确认后精确出队"
        );
    }

    // ② 改名：只发 t=，不得夹带分类参数
    {
        let conn = dbm.lock().await;
        let gen = db::sync_generation(&conn).unwrap();
        db::enqueue_edit(&conn, gen, &key, &url, 10, Some("新标题"), None, None).unwrap();
    }
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (rename)");
    {
        let form = last_subscription_edit_form(&server);
        assert!(
            form_has(&form, "t", "新标题"),
            "改名必须发 t=新标题，实际 {form:?}"
        );
        assert!(
            form.iter().all(|(k, _)| k != "a" && k != "r"),
            "改名不得夹带分类参数（a/r），实际 {form:?}"
        );
        let labels: Vec<String> = server
            .subscriptions
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.id == "feed/10")
            .unwrap()
            .categories
            .iter()
            .map(|(l, _)| l.clone())
            .collect();
        assert!(
            labels.contains(&"新分类".to_string()) && labels.contains(&"保留分类".to_string()),
            "改名不得改变分类归属: {labels:?}"
        );
    }
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
4. 旧 HTTP 成功不得确认后续新 intent（响应在途 + 本地新编辑）
============================================================ */

#[tokio::test]
async fn stale_edit_response_does_not_confirm_newer_intent() {
    let server = MockGReader::start().await.expect("start mock");
    let tmp = common::unique_db_path("opt009b_stale");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = format!("{}/opt009-stale.xml", server.url());
    let key = acct_key("greader", &server, "test");

    *server.subscriptions.lock().unwrap() = vec![MockSubscription {
        id: "feed/10".into(),
        title: "Stale Feed".into(),
        url: url.clone(),
        html_url: None,
        categories: vec![("旧分类".into(), "folder".into())],
    }];

    let dbm = {
        let conn = db::open(&tmp).unwrap();
        db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
        db::set_setting(&conn, "greader_username", "test").unwrap();
        db::set_setting(&conn, "greader_password", "test-token").unwrap();
        seed_bound_feed(&conn, &url, "旧分类", 10);
        let gen = db::sync_generation(&conn).unwrap();
        db::enqueue_edit(&conn, gen, &key, &url, 10, Some("第一版"), None, None).unwrap();
        Arc::new(Mutex::new(conn))
    };

    let old_id = {
        let conn = dbm.lock().await;
        db::pending_outbox(&conn).unwrap()[0].id
    };

    // 旧请求的响应已算定但挂起（服务端状态已变更、响应未写回）
    let barrier = server.arm_subscription_edit_barrier("feed/10");
    let db2 = dbm.clone();
    let http2 = http.clone();
    let handle = tokio::spawn(async move { sync::feeds_phase(&db2, &http2).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), barrier.arrived)
        .await
        .expect("执行器必须在期限内发出 edit 请求（网络屏障）")
        .expect("barrier arrived");

    // 在途期间本地产生新意图（同 URL 替换 → 新 id）
    let new_id = {
        let conn = dbm.lock().await;
        let gen = db::sync_generation(&conn).unwrap();
        db::enqueue_edit(&conn, gen, &key, &url, 10, Some("第二版"), None, None).unwrap();
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1, "替换后同 URL 只留最新意图");
        assert_ne!(items[0].id, old_id, "替换必须换新 id（不复用）");
        items[0].id
    };

    barrier.release.send(()).unwrap();
    let _ = handle
        .await
        .expect("join feeds_phase")
        .expect("feeds phase (stale response)");

    {
        let conn = dbm.lock().await;
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(
            items.len(),
            1,
            "旧 HTTP 成功不得确认（prune）在途期间入队的新意图"
        );
        assert_eq!(items[0].id, new_id);
        assert_eq!(items[0].desired_title.as_deref(), Some("第二版"));
    }

    // 下一轮推送新目标并出队
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (new intent)");
    {
        let form = last_subscription_edit_form(&server);
        assert!(
            form_has(&form, "t", "第二版"),
            "新意图必须最终推达，实际 {form:?}"
        );
        let conn = dbm.lock().await;
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "新意图确认后出队"
        );
    }
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
5. 跨账号同 URL：身份切换清旧账号队项（2026-11-02 语义），B 的意图独立收敛
============================================================ */

/// 原「换号暂停、回原账号恢复」语义已被生命周期终点取代（2026-11-02）：
/// 切换账号时旧账号的队项（含 remote_created 在途窗口）被整体清账，B 的本地
/// 动作只服务 B 自己的意图。保留的断言面：A 的远端 id 不得被发给 B、B 的
/// subscribe 全程只创建一次。判别力：若清理漏掉 remote_created 窗口即红。
#[tokio::test]
async fn account_switch_abandons_old_account_intent_and_b_intents_stay_scoped() {
    let server = MockGReader::start().await.expect("start mock");
    server.subscriptions.lock().unwrap().clear();
    let tmp = common::unique_db_path("opt009b_cross");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = "http://127.0.0.1:1/opt009-cross.xml";
    let key_a = acct_key("greader", &server, "user-a");
    let key_b = acct_key("greader", &server, "user-b");
    let dbm = Arc::new(Mutex::new(open_with_credentials(
        &server, "greader", "user-a",
    )));

    // 账号 A：subscribe 已推进到 remote_created（远端已建、本地绑定未完成）
    let id_a = {
        let conn = dbm.lock().await;
        let folder = db::create_folder(&conn, "分类", "article").unwrap();
        db::insert_feed(
            &conn,
            url,
            None,
            "Cross Feed",
            None,
            folder,
            "inherit",
            true,
            false,
        )
        .unwrap();
        let gen = db::sync_generation(&conn).unwrap();
        db::enqueue_subscribe(&conn, gen, &key_a, url, None, None).unwrap();
        let id = db::pending_outbox(&conn).unwrap()[0].id;
        assert!(
            db::advance_subscribe_stage(&conn, id, gen, 42).unwrap(),
            "前置：A 的 subscribe 推进 remote_created"
        );
        id
    };

    // 切到账号 B（同服务器不同用户 = 身份变化；本地 feed 保留，远端数据 purge）
    {
        let mut conn = dbm.lock().await;
        app_lib::commands::save_account_for_test(
            &mut conn,
            "greader",
            &server.url(),
            "user-b",
            "pw-b",
            &server.url(),
        )
        .expect("switch account");
    }
    let gen_b = {
        let conn = dbm.lock().await;
        db::sync_generation(&conn).unwrap()
    };

    // B 的本地新增：同 URL 入队 subscribe —— A 的行已在切换时清账，表内只剩 B
    {
        let conn = dbm.lock().await;
        db::enqueue_subscribe(&conn, gen_b, &key_b, url, None, None).unwrap();
        let items = db::pending_outbox(&conn).unwrap();
        assert!(
            items.iter().all(|i| i.id != id_a),
            "身份切换已清账：A 的 remote_created 队项不再跨换号存活: {items:?}"
        );
        assert!(
            items.iter().all(|i| i.account_key == key_b),
            "切换后表内只剩 B 的队项: {items:?}"
        );
    }
    // B 的退订不得借用/删除 A 的目标（结转按 key 屏障 + 删除按 key 限定）。
    // R1 起 B 会有**自己**的退订意图（证据 = 上一步 B 自己的 pending subscribe；
    // 该行 id 未知、留待执行器用幂等 quick_add-by-URL 解析），但绝不携带 A 的 42，
    // 也绝不删除 A 的行——这正是 key 屏障要保证的两件事。
    {
        let conn = dbm.lock().await;
        let queued = db::enqueue_unsubscribe(&conn, gen_b, &key_b, url, None, None).unwrap();
        assert!(queued, "B 有自己的创建证据：应写入 B 自己的退订意图");
        let items = db::pending_outbox(&conn).unwrap();
        assert!(
            items.iter().all(|i| i.account_key == key_b),
            "B 的退订入队不产生异账号行（入队级 key 屏障保持）"
        );
        let b_row = items
            .iter()
            .find(|i| i.account_key == key_b)
            .expect("B 自己的退订意图");
        assert_eq!(b_row.action, "unsubscribe");
        assert_eq!(
            b_row.remote_id, None,
            "不得借用 A 的 remote_id（跨账号屏障）"
        );
    }
    // 恢复 B 的 subscribe 意图，跑执行器：只发 B 的队项
    {
        let conn = dbm.lock().await;
        db::enqueue_subscribe(&conn, gen_b, &key_b, url, None, None).unwrap();
    }
    let report = sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (B)");
    {
        let conn = dbm.lock().await;
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(
            items.len(),
            0,
            "B 的 subscribe 应完成出队，A 的行已在切换时清账: {items:?}（report {report:?}）"
        );
    }
    // A 的远端 id 从未出现在任何请求中
    let edits = mock_greader::subscription_edit_actions(&server);
    assert!(
        edits.iter().all(|(_, s)| !s.contains("feed/42")),
        "A 的 remote_id 不得被发给 B: {edits:?}"
    );
    assert_eq!(
        quickadd_requests(&server),
        1,
        "只有 B 的 subscribe 发出一次 quick_add 请求（request-level）"
    );
    assert_eq!(
        quickadd_count(&server, url),
        1,
        "远端只创建一次（created-count）"
    );
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
R3-1. 未发送的连续移动折叠：必须保留「远端实际持有」的旧 label
============================================================ */

/// OPT-009 阶段 B1-R3（复审 P2-1）：两次移动都在**未发送窗口**内完成时，
/// 合并后的队项必须仍带链起点（远端实际持有的旧分类）——
/// A→B 未发送再 B→C，最终 wire 必须是 a=C + r=A；只发 r=B 时远端的 A 永不移除。
/// 走生产命令 `record_feed_edit`（非直接写 db 原语），断言实际 HTTP 表单与
/// mock 的最终成员标签。
/// 判别力：合并按「新操作的 prev」覆盖链起点时，r= 变成 B分类、远端保留 A分类。
#[tokio::test]
async fn move_chain_unsent_folds_to_remote_old_label() {
    let server = MockGReader::start().await.expect("start mock");
    let tmp = common::unique_db_path("opt009b_chain");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = format!("{}/opt009-chain.xml", server.url());

    // 远端当前在「A分类」（另有不得被触碰的「保留分类」）
    *server.subscriptions.lock().unwrap() = vec![MockSubscription {
        id: "feed/10".into(),
        title: "Chain Feed".into(),
        url: url.clone(),
        html_url: None,
        categories: vec![
            ("A分类".into(), "folder".into()),
            ("保留分类".into(), "folder".into()),
        ],
    }];

    let (dbm, feed_id, folder_b, folder_c) = {
        let conn = db::open(&tmp).unwrap();
        db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
        db::set_setting(&conn, "greader_username", "test").unwrap();
        db::set_setting(&conn, "greader_password", "test-token").unwrap();
        let (_folder_a, feed) = seed_bound_feed(&conn, &url, "A分类", 10);
        let folder_b = db::create_folder(&conn, "B分类", "article").unwrap();
        let folder_c = db::create_folder(&conn, "C分类", "article").unwrap();
        (Arc::new(Mutex::new(conn)), feed, folder_b, folder_c)
    };

    // 生产命令路径：A→B 与 B→C 两次移动都发生在未发送窗口内
    {
        let conn = dbm.lock().await;
        let push1 = app_lib::commands::record_feed_edit(
            &conn,
            feed_id,
            None,
            Some(folder_b),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(push1.is_some(), "已绑定远端：编辑必须有推送目标");
        let push2 = app_lib::commands::record_feed_edit(
            &conn,
            feed_id,
            None,
            Some(folder_c),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(push2.is_some());
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1, "同账号同 URL 合并为一行");
        assert_eq!(items[0].action, "edit");
        assert_eq!(items[0].desired_folder.as_deref(), Some("C分类"));
        assert_eq!(
            items[0].prev_folder.as_deref(),
            Some("A分类"),
            "未发送目标折叠后必须保留实际远端应移除的旧 label（链起点）: {items:?}"
        );
    }

    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (move chain)");
    {
        let form = last_subscription_edit_form(&server);
        assert!(
            form_has(&form, "a", "user/-/label/C分类"),
            "必须发送最终目标 label，实际 {form:?}"
        );
        assert!(
            form_has(&form, "r", "user/-/label/A分类"),
            "必须移除远端实际持有的旧 label（r=B 时远端 A 永不移除），实际 {form:?}"
        );
        let labels: Vec<String> = server
            .subscriptions
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.id == "feed/10")
            .expect("远端订阅仍在")
            .categories
            .iter()
            .map(|(l, _)| l.clone())
            .collect();
        assert!(
            labels.contains(&"C分类".to_string()),
            "远端必须加入最终目标: {labels:?}"
        );
        assert!(
            !labels.contains(&"A分类".to_string()),
            "远端链起点旧分类必须被移除: {labels:?}"
        );
        assert!(
            !labels.contains(&"B分类".to_string()),
            "从未发送到远端的中间目标不得出现: {labels:?}"
        );
        assert!(
            labels.contains(&"保留分类".to_string()),
            "非目标 label 不得被触碰: {labels:?}"
        );
        let conn = dbm.lock().await;
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "发送并确认后精确出队"
        );
    }
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
R3-2. 移动响应在途 + 新移动：发送成功必须把替代行的移除基准重定为已发送目标
============================================================ */

/// OPT-009 阶段 B1-R3（复审 P2-1 的配套）：A→B 的移动 HTTP 已成功（远端已处于 B）
/// 时，在途期间入队的 B→C 替代行若仍按「链起点 A」移除，远端会残留 B。
/// 生产语义：**发送成功 = 远端当前 label 即发送目标**——替代行的移除基准必须
/// 重定为该目标（执行器在确认落空后修复），最终远端只留 C。
/// 判别力：合并保留链起点但不做发送后修复时，远端最终为 {B分类, C分类}。
#[tokio::test]
async fn in_flight_move_success_rebases_successor_removal() {
    let server = MockGReader::start().await.expect("start mock");
    let tmp = common::unique_db_path("opt009b_rebase");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = format!("{}/opt009-rebase.xml", server.url());

    *server.subscriptions.lock().unwrap() = vec![MockSubscription {
        id: "feed/10".into(),
        title: "Rebase Feed".into(),
        url: url.clone(),
        html_url: None,
        categories: vec![
            ("A分类".into(), "folder".into()),
            ("保留分类".into(), "folder".into()),
        ],
    }];

    let (dbm, feed_id, folder_b, folder_c) = {
        let conn = db::open(&tmp).unwrap();
        db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
        db::set_setting(&conn, "greader_username", "test").unwrap();
        db::set_setting(&conn, "greader_password", "test-token").unwrap();
        let (_folder_a, feed) = seed_bound_feed(&conn, &url, "A分类", 10);
        let folder_b = db::create_folder(&conn, "B分类", "article").unwrap();
        let folder_c = db::create_folder(&conn, "C分类", "article").unwrap();
        (Arc::new(Mutex::new(conn)), feed, folder_b, folder_c)
    };

    // 未发送窗口入队 A→B
    {
        let conn = dbm.lock().await;
        app_lib::commands::record_feed_edit(
            &conn,
            feed_id,
            None,
            Some(folder_b),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
    }

    // A→B 的响应在途（服务端已接受）
    let barrier = server.arm_subscription_edit_barrier("feed/10");
    let db2 = dbm.clone();
    let http2 = http.clone();
    let handle = tokio::spawn(async move { sync::feeds_phase(&db2, &http2).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), barrier.arrived)
        .await
        .expect("执行器必须在期限内发出移动请求（网络屏障）")
        .expect("barrier arrived");

    // 在途期间生产命令再移动 B→C：合并必须保留链起点 A（未发送部分），
    // 而发送成功后执行器必须把「远端当前 label」重定为已发送的 B。
    {
        let conn = dbm.lock().await;
        app_lib::commands::record_feed_edit(
            &conn,
            feed_id,
            None,
            Some(folder_c),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1, "同 URL 合并为一行");
        assert_eq!(items[0].desired_folder.as_deref(), Some("C分类"));
        assert_eq!(
            items[0].prev_folder.as_deref(),
            Some("A分类"),
            "合并时刻的链起点仍是 A（发送结果未知）: {items:?}"
        );
    }

    barrier.release.send(()).unwrap();
    let _ = handle
        .await
        .expect("join feeds_phase")
        .expect("feeds phase (in-flight move)");

    {
        let form = last_subscription_edit_form(&server);
        assert!(
            form_has(&form, "a", "user/-/label/B分类")
                && form_has(&form, "r", "user/-/label/A分类"),
            "本轮必须按发送时目标移动（a=B, r=A），实际 {form:?}"
        );
        let conn = dbm.lock().await;
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1, "旧成功不得清掉在途新意图");
        assert_eq!(items[0].desired_folder.as_deref(), Some("C分类"));
        assert_eq!(
            items[0].prev_folder.as_deref(),
            Some("B分类"),
            "发送成功后远端当前 label 是已发送目标：替代行的移除基准必须重定为 B，实际 {items:?}"
        );
    }

    // 下一轮：B→C 推达后远端只应剩 C（B 被移除）
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (rebased successor)");
    {
        let labels: Vec<String> = server
            .subscriptions
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.id == "feed/10")
            .expect("远端订阅仍在")
            .categories
            .iter()
            .map(|(l, _)| l.clone())
            .collect();
        assert!(
            labels.contains(&"C分类".to_string()),
            "远端必须加入最终目标: {labels:?}"
        );
        assert!(
            !labels.contains(&"B分类".to_string()),
            "已发送成功的中间 label 必须被移除（否则远端长期残留）: {labels:?}"
        );
        assert!(
            !labels.contains(&"A分类".to_string()),
            "链起点也已移除: {labels:?}"
        );
        assert!(
            labels.contains(&"保留分类".to_string()),
            "非目标 label 不得被触碰: {labels:?}"
        );
        let conn = dbm.lock().await;
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "收敛后精确出队"
        );
    }
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
R3-3. 异账号暂停意图不得保护本账号的远端删除对账
============================================================ */

/// OPT-009 阶段 B1-R3（复审 P2-3）：pull 的删除对账保护集合只允许**当前同步
/// 账号**的 pending 意图参与。账号 A 留下的同 URL 暂停意图（换号不执行）不得
/// 阻止账号 B 按自己的远端快照移除该源——否则 B 侧永久残留已退订订阅。
/// 判别力：保护查询不限定 account_key 时，本用例的源不会被删除（removed=0）。
#[tokio::test]
async fn paused_other_account_intent_does_not_block_pull_removal() {
    let server = MockGReader::start().await.expect("start mock");
    server.subscriptions.lock().unwrap().clear(); // B 的远端已不含该 URL
    let tmp = common::unique_db_path("opt009b_acct_protect");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = format!("{}/opt009-acct-protect.xml", server.url());
    let key_a = acct_key("greader", &server, "user-a");

    let mut conn = open_with_credentials(&server, "greader", "user-a");
    // 切到账号 B（身份变化）。2026-11-02 起换号即清账——异账号行只能以
    // 「升级前旧库存」的形态存在，这里直写模拟该形态，锁定「拉取保护只算
    // 当前账号」的读取面（清账只发生在账号提交内核）。
    app_lib::commands::save_account_for_test(
        &mut conn,
        "greader",
        &server.url(),
        "user-b",
        "pw-b",
        &server.url(),
    )
    .expect("switch account");
    let key_gen = db::sync_generation(&conn).unwrap();
    db::enqueue_subscribe(&conn, key_gen, &key_a, &url, None, None).unwrap();
    let id_a = db::pending_outbox(&conn).unwrap()[0].id;
    assert!(
        db::advance_subscribe_stage(&conn, id_a, key_gen, 7).unwrap(),
        "前置：A 的残留意图推进 remote_created"
    );
    // B 自己的服务端来源订阅：B 已在远端退订（远端列表不含它）
    let feed_id = {
        let folder = db::create_folder(&conn, "B分类", "article").unwrap();
        let feed = db::insert_feed_origin(
            &conn,
            &url,
            None,
            "B Remote Feed",
            None,
            folder,
            "inherit",
            true,
            false,
            "remote",
        )
        .unwrap();
        db::set_feed_remote_id(&conn, feed, 10).unwrap();
        feed
    };
    let dbm = Arc::new(Mutex::new(conn));

    let report = sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (account B)");
    {
        let conn = dbm.lock().await;
        assert_eq!(
            report.removed_feeds, 1,
            "B 的权威快照必须生效：异账号暂停意图不得保护本账号的删除对账: {report:?}"
        );
        let left: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM feeds WHERE active=1 AND id = ?1",
                [feed_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(left, 0, "B 的 remote-origin 源必须按远端事实删除");
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(
            items.len(),
            1,
            "异账号残留行不参与 B 的执行，也不被 B 的轮次清理（清账只在账号提交内核）: {items:?}"
        );
        assert_eq!(items[0].id, id_a);
        assert_eq!(items[0].account_key, key_a);
        assert_eq!(items[0].stage, "remote_created");
    }
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
R3-4. 保护查询失败必须 fail-closed（不得误删本地订阅）
============================================================ */

/// OPT-009 阶段 B1-R3（复审 P3-4）：pull 的删除对账读取「本地未推送保护集合」
/// 失败时，状态是**未知**而不是空集合——必须记同步错误并跳过本轮删除对账。
/// 故障注入：把 subscription_outbox 改名（真实 SQLITE_ERROR：no such table），
/// 保护查询的 prepare 必然失败。断言本轮零删除、错误可见；表恢复后重跑收敛。
/// 判别力：把查询错误吞成空集合的实现会删除该源（removed=1）。
#[tokio::test]
async fn protection_read_failure_does_not_delete_remote_feed() {
    let server = MockGReader::start().await.expect("start mock");
    server.subscriptions.lock().unwrap().clear(); // 远端已不含该 URL（删除对账本应命中）
    let tmp = common::unique_db_path("opt009b_protect_fault");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = format!("{}/opt009-protect-fault.xml", server.url());

    let dbm = {
        let conn = db::open(&tmp).unwrap();
        db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
        db::set_setting(&conn, "greader_username", "test").unwrap();
        db::set_setting(&conn, "greader_password", "test-token").unwrap();
        let folder = db::create_folder(&conn, "分类", "article").unwrap();
        let feed = db::insert_feed_origin(
            &conn,
            &url,
            None,
            "Remote Feed",
            None,
            folder,
            "inherit",
            true,
            false,
            "remote",
        )
        .unwrap();
        db::set_feed_remote_id(&conn, feed, 10).unwrap();
        // 故障注入：保护查询的表不可读（真实 schema 级失败）
        conn.execute_batch("ALTER TABLE subscription_outbox RENAME TO subscription_outbox_fault;")
            .unwrap();
        assert!(
            db::pending_outbox(&conn).is_err(),
            "前置：改名后保护查询必须真实失败（schema 级故障注入生效）"
        );
        Arc::new(Mutex::new(conn))
    };
    let feed_id = {
        let conn = dbm.lock().await;
        conn.query_row::<i64, _, _>(
            "SELECT id FROM feeds WHERE active=1 AND feed_url = ?1",
            [&url],
            |r| r.get(0),
        )
        .unwrap()
    };
    let report = sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (protection read failure)");
    {
        let conn = dbm.lock().await;
        assert_eq!(
            report.removed_feeds, 0,
            "保护集合读取失败 = 状态未知：必须 fail-closed（不得按空集合删除）: {report:?}"
        );
        let left: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM feeds WHERE active=1 AND id = ?1",
                [feed_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(left, 1, "状态未知时本地订阅必须原样");
        assert!(
            report.errors.iter().any(|e| e.contains("保护")),
            "必须记录可见的同步错误: {report:?}"
        );
        // 恢复 schema → 下一轮必须收敛（故障不是永久冻结）
        conn.execute_batch("ALTER TABLE subscription_outbox_fault RENAME TO subscription_outbox;")
            .unwrap();
    }
    let report2 = sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (recovered)");
    assert_eq!(report2.removed_feeds, 1, "故障恢复后必须收敛: {report2:?}");
    {
        let conn = dbm.lock().await;
        let left: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM feeds WHERE active=1 AND id = ?1",
                [feed_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(left, 0, "恢复后按远端事实删除");
    }
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
R3-5. OPML 导入：本地新增与类型化订阅意图同一事务，故障后重试收敛
============================================================ */

/// 最小 OPML 文档：逐条给出 (url, title, folder)。
fn opml_doc(feeds: &[(&str, &str, Option<&str>)]) -> String {
    let mut body = String::new();
    for (url, title, folder) in feeds {
        match folder {
            Some(f) => body.push_str(&format!(
                r#"<outline text="{f}"><outline type="rss" text="{title}" xmlUrl="{url}"/></outline>"#
            )),
            None => body.push_str(&format!(
                r#"<outline type="rss" text="{title}" xmlUrl="{url}"/>"#
            )),
        }
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><opml version="1.0"><head><title>t</title></head><body>{body}</body></opml>"#
    )
}

/// 实际发出（含失败注入）的 quick_add 请求数——请求日志计数，不受 mock 是否
/// 命中既有订阅影响。
fn quickadd_requests(server: &MockGReader) -> usize {
    server
        .request_log()
        .iter()
        .filter(|r| r.ends_with("/subscription/quickadd"))
        .count()
}

/// OPT-009 阶段 B1-R3（复审 P2-2）：OPML 导入的本地新增与**类型化 subscribe
/// 意图**必须同一事务落库；不得再写 legacy `sync_queue.add_feed`（否则
/// push_feeds 在 quick_add 成功后剪队，绑定/挂载失败的任务就永久丢失）。
/// 故障注入走生产 OPML 命令 + 真实 loopback：挂载失败后重试必须收敛、
/// 不重复 quick_add、不丢分类目标。
/// 判别力：legacy 路径下挂载失败时队项已被剪掉（outbox 为空即红）。
#[tokio::test]
async fn opml_import_mount_failure_keeps_folder_target_then_converges() {
    let server = MockGReader::start().await.expect("start mock");
    let tmp = common::unique_db_path("opt009b_opml_mount");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = format!("{}/opml-mount.xml", server.url());
    // 远端已有该订阅（幂等 quick_add 返回既有 id）——便于断言远端成员标签
    server
        .existing_feed_urls
        .lock()
        .unwrap()
        .push((url.clone(), 30));
    server.subscriptions.lock().unwrap().push(MockSubscription {
        id: "feed/30".into(),
        title: "OPML 标题".into(),
        url: url.clone(),
        html_url: None,
        categories: vec![],
    });

    let dbm = {
        let conn = open_with_credentials(&server, "greader", "test");
        let report = app_lib::commands::opml_import_for_test(
            &conn,
            &opml_doc(&[(url.as_str(), "OPML 标题", Some("导入分类"))]),
        )
        .expect("opml import");
        assert_eq!((report.imported, report.skipped), (1, 0));
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1, "OPML 导入必须写入类型化订阅意图: {items:?}");
        assert_eq!(items[0].action, "subscribe");
        assert_eq!(
            items[0].desired_folder.as_deref(),
            Some("导入分类"),
            "导入目录必须成为远端目标"
        );
        let legacy: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_queue WHERE action = 'add_feed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            legacy, 0,
            "不得再写 legacy add_feed（否则与执行器双重推送，且 quick_add 后即被剪掉）"
        );
        let (title, folder): (String, String) = conn
            .query_row(
                "SELECT f.title, fo.name FROM feeds f JOIN folders fo ON fo.id = f.folder_id
                  WHERE f.feed_url = ?1",
                [&url],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(title, "OPML 标题", "本地标题目标必须保留");
        assert_eq!(folder, "导入分类", "本地目录目标必须保留");
        Arc::new(Mutex::new(conn))
    };

    // 故障注入：分类挂载失败（legacy push_feeds 在 quick_add 成功时就已剪队）
    server.set_fail_subscription_edit(true);
    let report = sync::feeds_phase(&dbm, &http).await.expect("feeds phase 1");
    {
        let conn = dbm.lock().await;
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(
            items.len(),
            1,
            "挂载失败必须保留持久意图（legacy 路径此时队项已被剪掉）: {report:?}"
        );
        assert_eq!(items[0].stage, "remote_created");
        assert_eq!(
            items[0].desired_folder.as_deref(),
            Some("导入分类"),
            "分类目标不得丢失"
        );
    }
    server.set_fail_subscription_edit(false);
    sync::feeds_phase(&dbm, &http).await.expect("feeds phase 2");
    {
        let form = last_subscription_edit_form(&server);
        assert!(
            form_has(&form, "a", "user/-/label/导入分类"),
            "重试必须补挂导入分类，实际 {form:?}"
        );
        let labels: Vec<String> = server
            .subscriptions
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.id == "feed/30")
            .expect("远端订阅仍在")
            .categories
            .iter()
            .map(|(l, _)| l.clone())
            .collect();
        assert!(
            labels.contains(&"导入分类".to_string()),
            "远端成员必须包含导入分类: {labels:?}"
        );
        let conn = dbm.lock().await;
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "收敛后精确出队"
        );
        let bound: Option<i64> = conn
            .query_row(
                "SELECT remote_id FROM feeds WHERE feed_url = ?1",
                [&url],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bound, Some(30), "绑定必须完成");
    }
    assert_eq!(
        quickadd_requests(&server),
        0,
        "远端已持有同 URL 订阅：列表证据直接解析，全程零 quick_add"
    );
    let _ = std::fs::remove_file(&tmp);
}

/// OPT-009 阶段 B1-R3（复审 P2-2）：quick_add 失败时类型化意图保留、下一轮
/// **只创建一次**远端订阅并完成挂载收敛；全程不得出现 legacy add_feed 队项。
#[tokio::test]
async fn opml_import_quickadd_failure_retries_without_duplicate_remote_subscription() {
    let server = MockGReader::start().await.expect("start mock");
    server.subscriptions.lock().unwrap().clear();
    let tmp = common::unique_db_path("opt009b_opml_quickadd");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = format!("{}/opml-quickadd.xml", server.url());

    let dbm = {
        let conn = open_with_credentials(&server, "greader", "test");
        let report = app_lib::commands::opml_import_for_test(
            &conn,
            &opml_doc(&[(url.as_str(), "QuickAdd 标题", Some("收藏夹"))]),
        )
        .expect("opml import");
        assert_eq!(report.imported, 1);
        Arc::new(Mutex::new(conn))
    };

    server.set_fail_quick_add(true);
    let report = sync::feeds_phase(&dbm, &http).await.expect("feeds phase 1");
    {
        let conn = dbm.lock().await;
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1, "quick_add 失败必须保留意图: {report:?}");
        assert_eq!(items[0].stage, "pending", "远端未创建：不得推进阶段");
        assert!(items[0].attempts >= 1, "失败必须累计 attempts");
        assert!(items[0].last_error.is_some(), "失败必须记录 last_error");
        let legacy: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_queue WHERE action = 'add_feed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(legacy, 0, "全程不得回落到 legacy 队列");
    }
    assert_eq!(quickadd_requests(&server), 1);

    server.set_fail_quick_add(false);
    sync::feeds_phase(&dbm, &http).await.expect("feeds phase 2");
    {
        let conn = dbm.lock().await;
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "重试成功后完整确认出队"
        );
    }
    assert_eq!(
        quickadd_count(&server, url.as_str()),
        1,
        "远端订阅只创建一次（失败轮未创建，重试轮创建一次）"
    );
    assert_eq!(
        quickadd_requests(&server),
        2,
        "两次尝试：一次失败 + 一次成功"
    );
    let form = last_subscription_edit_form(&server);
    assert!(
        form_has(&form, "a", "user/-/label/收藏夹"),
        "最终挂载必须带分类目标，实际 {form:?}"
    );
    let _ = std::fs::remove_file(&tmp);
}

/// 真实服务端对重复 URL 的 quickadd 语义（2026-10-31 审计对齐，mock 默认）：
/// FreshRSS 返回 200 + numResults=0（无 streamId，freshrss-greader.php
/// quickadd() 的 catch 分支）、Miniflux 返回 500（feed_already_exists，
/// handler.go quickAddHandler → JSONServerError）。
/// 场景是真实世界的列表滞后窗口：远端已持有该订阅（existing 注册表）但
/// 订阅列表尚未列出——产品发送 quickadd 被拒后，两种形态都必须按「未创建」
/// 处理：意图保留（Retry + last_error）、零重复远端创建，等列表证据出现后
/// 收敛（见 quickadd_response_lost_across_restart_converges_via_list_evidence）。
/// 判别力（旧 mock 语义）：重复 quickadd 返回既有 id → 产品误判成功绑定。
#[tokio::test]
async fn quickadd_duplicate_url_is_rejected_by_real_servers_and_intent_is_kept() {
    for (name, mode, miniflux_500) in [
        (
            "freshrss",
            mock_greader::QuickaddDuplicate::FreshRssNumResultsZero,
            false,
        ),
        (
            "miniflux",
            mock_greader::QuickaddDuplicate::Miniflux500,
            true,
        ),
    ] {
        let server = MockGReader::start().await.expect("start mock");
        server.set_quickadd_duplicate(mode);
        let http = app_lib::ingestion::build_client(10);
        let url = format!("{}/opt009-dup-{name}.xml", server.url());
        let key = acct_key("greader", &server, "test");

        // 远端已持有该订阅，但订阅列表滞后（清空预设列表，模拟窗口）
        server
            .existing_feed_urls
            .lock()
            .unwrap()
            .push((url.clone(), 40));
        server.subscriptions.lock().unwrap().clear();

        // 客户端级：锁定两种真实响应形态——FreshRSS 200 不携带 streamId、
        // Miniflux 是传输级错误。
        let client = app_lib::greader::GReaderClient::new(&server.url(), "mock/abc", http.clone());
        let resp = client.quick_add(&url).await;
        if miniflux_500 {
            assert!(
                resp.is_err(),
                "[{name}] Miniflux 形态必须是错误（feed_already_exists 500）"
            );
        } else {
            let resp = resp.expect("[{name}] FreshRSS 形态是 200，不是传输错误");
            assert_eq!(resp.num_results, 0, "[{name}] numResults=0");
            assert!(resp.stream_id.is_none(), "[{name}] 不携带 streamId");
        }

        // 执行器级：列表滞后窗口内发送 quickadd 被拒 → Retry 保留意图
        let tmp = common::unique_db_path(&format!("opt009b_dup_{name}"));
        let _ = std::fs::remove_file(&tmp);

        let dbm = {
            let conn = db::open(&tmp).unwrap();
            db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
            db::set_setting(&conn, "greader_username", "test").unwrap();
            db::set_setting(&conn, "greader_password", "test-token").unwrap();
            let folder = db::create_folder(&conn, "分类", "article").unwrap();
            db::insert_feed(
                &conn, &url, None, "Dup", None, folder, "inherit", true, false,
            )
            .unwrap();
            let gen = db::sync_generation(&conn).unwrap();
            db::enqueue_subscribe(&conn, gen, &key, &url, None, None).unwrap();
            Arc::new(Mutex::new(conn))
        };

        sync::feeds_phase(&dbm, &http).await.expect("feeds phase");
        {
            let conn = dbm.lock().await;
            let items = db::pending_outbox(&conn).unwrap();
            assert_eq!(
                items.len(),
                1,
                "[{name}] 被拒的订阅意图必须保留（Retry）: {items:?}"
            );
            assert!(
                items[0].attempts >= 1,
                "[{name}] 失败必须累计 attempts: {items:?}"
            );
            assert!(
                items[0].last_error.is_some(),
                "[{name}] 失败必须记录 last_error: {items:?}"
            );
            assert_eq!(
                items[0].remote_id, None,
                "[{name}] 被拒不产生已验证 id: {items:?}"
            );
        }
        assert!(
            server.created_feeds.lock().unwrap().is_empty(),
            "[{name}] 重复 URL 不得再创建远端订阅"
        );
        assert_eq!(
            quickadd_count(&server, &url),
            0,
            "[{name}] 重复 URL 不得创建新远端订阅（created-count 为 0）"
        );
        assert_eq!(
            quickadd_requests(&server),
            2,
            "[{name}] 客户端形态探测 + 执行器各一次 quick_add"
        );
        let _ = std::fs::remove_file(&tmp);
    }
}

/// B1（F2）核心时序：quick_add 已被服务端执行（创建+登记）、响应以 500 丢失，
/// 且服务端列表尚未列出该订阅（崩溃窗口）——pending 行不得盲目重发 quick_add
/// （真实服务端会拒绝重复 URL 且无法推进阶段）；恢复轮以订阅列表证据解析出
/// 已创建的 feed，直接绑定收敛，零重复订阅、零二次 quick_add。
/// 判别力（修前实现：重启后盲发 quick_add）：重复 URL 被拒 → 永久 Retry，
/// 第二次 quick_add 请求与收敛失败即红。
#[tokio::test]
async fn quickadd_response_lost_across_restart_converges_via_list_evidence() {
    let server = MockGReader::start().await.expect("start mock");
    let tmp = common::unique_db_path("opt009b_crash_window");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = format!("{}/opt009-crash-window.xml", server.url());
    let key = acct_key("greader", &server, "test");

    let dbm = {
        // 两轮共用同一文件库：round 2 重开 tmp 即「崩溃后重启」。
        let conn = db::open(&tmp).unwrap();
        db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
        db::set_setting(&conn, "greader_username", "test").unwrap();
        db::set_setting(&conn, "greader_password", "test-token").unwrap();
        let folder = db::create_folder(&conn, "分类", "article").unwrap();
        db::insert_feed(
            &conn,
            &url,
            None,
            "Crash Window",
            None,
            folder,
            "inherit",
            true,
            false,
        )
        .unwrap();
        let gen = db::sync_generation(&conn).unwrap();
        db::enqueue_subscribe(&conn, gen, &key, &url, None, Some("分类")).unwrap();
        Arc::new(Mutex::new(conn))
    };

    // 崩溃窗口：quick_add 创建后响应丢失；列表尚未列出该订阅（服务端列表滞后）
    // → 解析未见 → 保留意图、无 remote_id、零二次 quick_add。
    server.set_quickadd_applied_then_fail(true);
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (crash window)");
    let remote_id = {
        let id = server
            .existing_feed_urls
            .lock()
            .unwrap()
            .iter()
            .find(|(u, _)| u == &url)
            .expect("quick_add 创建必须登记既有 id")
            .1;
        // 真实服务端的订阅列表会列出这条已创建的订阅（供恢复轮发现）
        server.subscriptions.lock().unwrap().push(MockSubscription {
            id: format!("feed/{id}"),
            title: "Crash Window".into(),
            url: url.clone(),
            html_url: None,
            categories: vec![],
        });
        id
    };
    {
        let conn = dbm.lock().await;
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1, "崩溃窗口后意图必须保留: {items:?}");
        assert_eq!(items[0].remote_id, None, "响应未处理：无已验证 id");
        assert!(items[0].attempts >= 1, "失败必须累计 attempts");
    }
    // 重启（重开同一文件库）后列表已列出该订阅：列表证据解析 → 绑定收敛，
    // 不重发 quick_add、不新建重复订阅。
    server.set_quickadd_applied_then_fail(false);
    let dbm = Arc::new(Mutex::new(db::open(&tmp).unwrap()));
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (recovered)");
    {
        let conn = dbm.lock().await;
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "列表证据解析后精确出队"
        );
        let bound: Option<i64> = conn
            .query_row(
                "SELECT remote_id FROM feeds WHERE feed_url = ?1",
                [&url],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bound, Some(remote_id), "必须绑定到已创建的远端订阅");
        let dup: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM feeds WHERE active=1 AND feed_url = ?1",
                [&url],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dup, 1, "不得产生重复本地 feed 行");
    }
    assert_eq!(
        quickadd_requests(&server),
        1,
        "恢复全程只有崩溃前那一次 quick_add（不得重发）"
    );
    assert_eq!(quickadd_count(&server, &url), 1, "远端只创建一次");
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
B2-1. 在途移动成功 + 后续换代：按旧请求账号与已接受目标重定后继
============================================================ */

/// OPT-009 阶段 B2（发现 1）：A→B 的移动 HTTP 已成功、同 URL 的后继移动 B→C 已
/// 排队，随后**数据库代际变化**（同账号改密码/重连）——执行器的确认阶段会因
/// 代际失配被跳过，但「远端已接受 B」是账号作用域事实：必须按旧请求的 account
/// key 与已接受目标把后继行的移除基准重定为 B，否则下一轮发 r=链起点 A，远端
/// 长期残留 B。异账号同 URL 的队项一个字节都不动。
/// 判别力（旧实现 = 代际守卫直接 return）：后继行 prev 停在 A分类、远端最终残留 B分类。
#[tokio::test]
async fn inflight_move_success_rebase_survives_generation_bump() {
    let server = MockGReader::start().await.expect("start mock");
    let tmp = common::unique_db_path("opt009b_b2_gen");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = format!("{}/opt009-b2-gen.xml", server.url());
    let key = acct_key("greader", &server, "test");

    *server.subscriptions.lock().unwrap() = vec![MockSubscription {
        id: "feed/10".into(),
        title: "Gen Feed".into(),
        url: url.clone(),
        html_url: None,
        categories: vec![
            ("A分类".into(), "folder".into()),
            ("保留分类".into(), "folder".into()),
        ],
    }];

    let (dbm, feed_id, folder_b, folder_c) = {
        let conn = db::open(&tmp).unwrap();
        db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
        db::set_setting(&conn, "greader_username", "test").unwrap();
        db::set_setting(&conn, "greader_password", "test-token").unwrap();
        let (_folder_a, feed) = seed_bound_feed(&conn, &url, "A分类", 10);
        let folder_b = db::create_folder(&conn, "B分类", "article").unwrap();
        let folder_c = db::create_folder(&conn, "C分类", "article").unwrap();
        // 异账号同 URL 的暂停队项：本轮及重定都不得触碰
        db::enqueue_edit(
            &conn,
            7,
            "greader|http://other.example|other",
            &url,
            10,
            None,
            Some("别的目标"),
            Some("别的旧分类"),
        )
        .unwrap();
        (Arc::new(Mutex::new(conn)), feed, folder_b, folder_c)
    };

    // 未发送窗口入队 A→B
    {
        let conn = dbm.lock().await;
        app_lib::commands::record_feed_edit(
            &conn,
            feed_id,
            None,
            Some(folder_b),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
    }

    // A→B 的响应在途（服务端已接受）
    let barrier = server.arm_subscription_edit_barrier("feed/10");
    let db2 = dbm.clone();
    let http2 = http.clone();
    let handle = tokio::spawn(async move { sync::feeds_phase(&db2, &http2).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), barrier.arrived)
        .await
        .expect("执行器必须在期限内发出移动请求（网络屏障）")
        .expect("barrier arrived");

    // 在途：后继移动 B→C + **代际推进**（同账号改密码/重连；确认阶段将失配）
    let old_generation = {
        let conn = dbm.lock().await;
        app_lib::commands::record_feed_edit(
            &conn,
            feed_id,
            None,
            Some(folder_c),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let gen = db::sync_generation(&conn).unwrap();
        let new_gen = db::bump_sync_generation(&conn).unwrap();
        assert!(new_gen > gen, "前置：代际必须推进");
        gen
    };
    assert_eq!(old_generation, 0);

    barrier.release.send(()).unwrap();
    let _ = handle
        .await
        .expect("join feeds_phase")
        .expect("feeds phase (in-flight move, generation bump)");

    {
        let conn = dbm.lock().await;
        let items = db::pending_outbox(&conn).unwrap();
        // 本账号的后继行：prev 已按**已接受目标** B 重定（不受代际失配影响）
        let mine = items
            .iter()
            .find(|i| i.account_key == key)
            .expect("本账号后继行仍在队");
        assert_eq!(mine.desired_folder.as_deref(), Some("C分类"));
        assert_eq!(
            mine.prev_folder.as_deref(),
            Some("B分类"),
            "换代后仍需按旧请求已接受目标重定后继移除基准: {items:?}"
        );
        assert_eq!(
            mine.account_generation, 0,
            "确认/重绑不在代际失配时落库（行保留原代际，下轮重绑）"
        );
        // 异账号队项原样
        let other = items
            .iter()
            .find(|i| i.account_key.contains("other.example"))
            .expect("异账号行保留");
        assert_eq!(other.desired_folder.as_deref(), Some("别的目标"));
        assert_eq!(
            other.prev_folder.as_deref(),
            Some("别的旧分类"),
            "异账号队项不得被本账号发送结果改写"
        );
    }

    // 回原账号重试：新一轮把本账号行重绑到新代际并按 r=B 收敛
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (retry after bump)");
    {
        let labels: Vec<String> = server
            .subscriptions
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.id == "feed/10")
            .expect("远端订阅仍在")
            .categories
            .iter()
            .map(|(l, _)| l.clone())
            .collect();
        assert!(
            labels.contains(&"C分类".to_string()),
            "远端必须加入最终目标: {labels:?}"
        );
        assert!(
            !labels.contains(&"B分类".to_string()),
            "已接受但被后继取代的中间 label 必须移除（换代后也不得残留）: {labels:?}"
        );
        assert!(
            !labels.contains(&"A分类".to_string()),
            "链起点已移除: {labels:?}"
        );
        assert!(
            labels.contains(&"保留分类".to_string()),
            "非目标 label 不得被触碰: {labels:?}"
        );
        let conn = dbm.lock().await;
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1, "本账号行出队，异账号行暂停: {items:?}");
        assert!(items[0].account_key.contains("other.example"));
    }
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
B2-2. 普通删除：typed 退订意图持久化，故障/重启后重试收敛
============================================================ */

/// OPT-009 阶段 B2（发现 2）：删除订阅 = 墓碑 + 本地删除 + **typed 退订意图**同一
/// 事务；退订失败/断网/重启后由执行器按队项重试（不再是一次性 best-effort）；
/// 远端确认已不含该 URL 后墓碑才收敛清除。
/// 判别力（旧实现 = best-effort 一次 + 无队项）：删后立刻断言的「队项在」与
/// 「重启后重试」必红。
#[tokio::test]
async fn deleted_feed_unsubscribe_is_durable_across_outage_and_restart() {
    let server = MockGReader::start().await.expect("start mock");
    let tmp = common::unique_db_path("opt009b_b2_unsub");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = format!("{}/opt009-b2-unsub.xml", server.url());

    let (dbm, feed_id) = {
        let conn = db::open(&tmp).unwrap();
        db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
        db::set_setting(&conn, "greader_username", "test").unwrap();
        db::set_setting(&conn, "greader_password", "test-token").unwrap();
        *server.subscriptions.lock().unwrap() = vec![MockSubscription {
            id: "feed/10".into(),
            title: "Delete Feed".into(),
            url: url.clone(),
            html_url: None,
            categories: vec![("分类".into(), "folder".into())],
        }];
        let (_folder, feed) = seed_bound_feed(&conn, &url, "分类", 10);
        (Arc::new(Mutex::new(conn)), feed)
    };

    // 删除（生产命令逻辑）：同一事务落墓碑 + 本地删除 + typed 退订意图
    {
        let conn = dbm.lock().await;
        let target = app_lib::commands::record_feed_deletion(&conn, feed_id)
            .expect("record deletion")
            .expect("已绑定远端且已配置：应返回待退订目标");
        assert_eq!(target.0, 10);
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(
            items.len(),
            1,
            "删除必须持久化 typed 退订意图（离线/重启可重试）: {items:?}"
        );
        assert_eq!(items[0].action, "unsubscribe");
        assert_eq!(items[0].remote_id, Some(10));
        assert!(
            db::feed_tombstones(&conn)
                .unwrap()
                .iter()
                .any(|u| u.contains("opt009-b2-unsub.xml")),
            "删除墓碑必须落下（防 pull 复活）"
        );
    }

    // ① 断网/退订失败：队项保留（attempts/last_error）、墓碑在、不复活
    server.set_fail_subscription_edit(true);
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (outage)");
    {
        let conn = dbm.lock().await;
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1, "退订失败必须保留队项");
        assert_eq!(items[0].action, "unsubscribe");
        assert!(items[0].attempts >= 1, "失败必须累计 attempts");
        assert!(items[0].last_error.is_some(), "失败必须记录 last_error");
        let left: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM feeds WHERE active=1 AND id = ?1",
                [feed_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(left, 0, "本地删除已生效");
        let revived: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM feeds WHERE active=1 AND feed_url = ?1",
                [&url],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(revived, 0, "远端仍列出时不得复活（墓碑生效）");
    }

    // ② 重启（丢连接重开同一文件库）→ 恢复网络 → 收敛
    drop(dbm);
    let dbm = Arc::new(Mutex::new(db::open(&tmp).unwrap()));
    server.set_fail_subscription_edit(false);
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (restart retry)");
    {
        let conn = dbm.lock().await;
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "退订重试成功后精确出队"
        );
        assert!(
            db::feed_tombstones(&conn).unwrap().is_empty(),
            "远端列表确认已不含后墓碑收敛清除"
        );
    }
    assert!(
        mock_greader::subscription_edit_actions(&server)
            .iter()
            .any(|(ac, s)| ac == "unsubscribe" && s.contains("feed/10")),
        "重启重试必须真的发出 ac=unsubscribe"
    );
    assert!(
        server
            .subscriptions
            .lock()
            .unwrap()
            .iter()
            .all(|s| s.id != "feed/10"),
        "远端订阅已移除"
    );
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
B2-3. 分类改名：逐源 a=新 + r=旧；部分失败可重试收敛
============================================================ */

/// OPT-009 阶段 B2（发现 3）：非空分类改名对**每个成员订阅**表达远端移动
/// （`a=` 新 label + `r=` 旧 label，其它标签保留——不发明 rename-tag/disable-tag）；
/// 本地改名、成员 outbox 与旧 label 墓碑同事务。同批请求部分失败时成功项精确
/// 出队、失败项保留重试，最终全部收敛。
/// 判别力（旧实现 = 只写墓碑改名）：无任何 a=/r= 请求、远端成员不动。
#[tokio::test]
async fn folder_rename_emits_per_member_move_and_converges_after_partial_failure() {
    let server = MockGReader::start().await.expect("start mock");
    let tmp = common::unique_db_path("opt009b_b2_rename");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url1 = format!("{}/opt009-b2-ren-1.xml", server.url());
    let url2 = format!("{}/opt009-b2-ren-2.xml", server.url());

    *server.subscriptions.lock().unwrap() = vec![
        MockSubscription {
            id: "feed/10".into(),
            title: "Rename A".into(),
            url: url1.clone(),
            html_url: None,
            categories: vec![
                ("旧分类".into(), "folder".into()),
                ("保留分类".into(), "folder".into()),
            ],
        },
        MockSubscription {
            id: "feed/20".into(),
            title: "Rename B".into(),
            url: url2.clone(),
            html_url: None,
            categories: vec![("旧分类".into(), "folder".into())],
        },
    ];

    let (dbm, folder_id) = {
        let conn = db::open(&tmp).unwrap();
        db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
        db::set_setting(&conn, "greader_username", "test").unwrap();
        db::set_setting(&conn, "greader_password", "test-token").unwrap();
        let (folder, _f1) = seed_bound_feed(&conn, &url1, "旧分类", 10);
        let f2 = db::insert_feed(
            &conn, &url2, None, "Rename B", None, folder, "inherit", true, false,
        )
        .unwrap();
        db::set_feed_remote_id(&conn, f2, 20).unwrap();
        (Arc::new(Mutex::new(conn)), folder)
    };

    // 生产命令：改名（本地 + 墓碑 + 逐成员意图同事务）
    {
        let conn = dbm.lock().await;
        app_lib::commands::record_folder_rename(&conn, folder_id, "新分类").expect("rename");
        let renamed: String = conn
            .query_row("SELECT name FROM folders WHERE id = ?1", [folder_id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(renamed, "新分类", "本地改名已生效");
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 2, "每个成员一条移动意图: {items:?}");
        for item in &items {
            assert_eq!(item.action, "edit");
            assert_eq!(item.desired_folder.as_deref(), Some("新分类"));
            assert_eq!(
                item.prev_folder.as_deref(),
                Some("旧分类"),
                "必须表达旧 label 移除（不是追加）: {items:?}"
            );
        }
        assert!(
            db::folder_tombstones(&conn)
                .unwrap()
                .iter()
                .any(|t| t.label == "旧分类"),
            "旧 label 墓碑必须落下（防 pull 复活旧目录）"
        );
    }

    // 部分失败注入：feed/20 的移动 500，feed/10 正常
    server.set_fail_subscription_edit_for(Some("feed/20"));
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (partial failure)");
    {
        let conn = dbm.lock().await;
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1, "失败项保留、成功项出队: {items:?}");
        assert_eq!(items[0].remote_id, Some(20));
        assert!(items[0].attempts >= 1);
    }
    {
        let labels10: Vec<String> = server
            .subscriptions
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.id == "feed/10")
            .unwrap()
            .categories
            .iter()
            .map(|(l, _)| l.clone())
            .collect();
        assert!(
            labels10.contains(&"新分类".to_string()) && !labels10.contains(&"旧分类".to_string()),
            "成功项远端已完成移动: {labels10:?}"
        );
        assert!(
            labels10.contains(&"保留分类".to_string()),
            "非目标 label 保留: {labels10:?}"
        );
        let labels20: Vec<String> = server
            .subscriptions
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.id == "feed/20")
            .unwrap()
            .categories
            .iter()
            .map(|(l, _)| l.clone())
            .collect();
        assert!(
            labels20.contains(&"旧分类".to_string()),
            "失败项远端未动（无假成功）: {labels20:?}"
        );
    }

    // 恢复后重试：失败项收敛，队列清空
    server.set_fail_subscription_edit_for(None);
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (retry)");
    {
        let conn = dbm.lock().await;
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "重试后全部出队"
        );
    }
    {
        let labels20: Vec<String> = server
            .subscriptions
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.id == "feed/20")
            .unwrap()
            .categories
            .iter()
            .map(|(l, _)| l.clone())
            .collect();
        assert!(
            labels20.contains(&"新分类".to_string()) && !labels20.contains(&"旧分类".to_string()),
            "重试后远端完成移动: {labels20:?}"
        );
    }
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
B2-3b. 分类删除：逐成员持久退订；级联中途 DB 故障全回滚
============================================================ */

/// OPT-009 阶段 B2（发现 2/3）：删除分类在级联前取成员与远端目标快照，为每个
/// **实际远端成员**建立独立可恢复退订意图；本地目录/订阅/条目按既有确认弹窗
/// 契约一并删除。断言真实退订请求、远端成员消失与本地不复活。
#[tokio::test]
async fn folder_delete_creates_per_member_unsubscribe_intents_and_converges() {
    let server = MockGReader::start().await.expect("start mock");
    let tmp = common::unique_db_path("opt009b_b2_fdel");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url1 = format!("{}/opt009-b2-fdel-1.xml", server.url());
    let url2 = format!("{}/opt009-b2-fdel-2.xml", server.url());

    *server.subscriptions.lock().unwrap() = vec![
        MockSubscription {
            id: "feed/10".into(),
            title: "Del A".into(),
            url: url1.clone(),
            html_url: None,
            categories: vec![("待删分类".into(), "folder".into())],
        },
        MockSubscription {
            id: "feed/20".into(),
            title: "Del B".into(),
            url: url2.clone(),
            html_url: None,
            categories: vec![("待删分类".into(), "folder".into())],
        },
    ];

    let (dbm, folder_id) = {
        let conn = db::open(&tmp).unwrap();
        db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
        db::set_setting(&conn, "greader_username", "test").unwrap();
        db::set_setting(&conn, "greader_password", "test-token").unwrap();
        let (folder, _f1) = seed_bound_feed(&conn, &url1, "待删分类", 10);
        let f2 = db::insert_feed(
            &conn, &url2, None, "Del B", None, folder, "inherit", true, false,
        )
        .unwrap();
        db::set_feed_remote_id(&conn, f2, 20).unwrap();
        (Arc::new(Mutex::new(conn)), folder)
    };

    {
        let conn = dbm.lock().await;
        app_lib::commands::record_folder_delete(&conn, folder_id).expect("delete folder");
        // 目录与订阅本地删除（确认弹窗契约）
        let folders: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM folders WHERE id = ?1",
                [folder_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(folders, 0, "目录已删除");
        let feeds: i64 = conn
            .query_row("SELECT COUNT(*) FROM feeds WHERE active=1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(feeds, 0, "成员订阅随目录删除");
        // 逐成员独立退订意图（各自带远端目标）
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 2, "每个实际远端成员一条退订意图: {items:?}");
        let mut ids: Vec<Option<i64>> = items.iter().map(|i| i.remote_id).collect();
        ids.sort();
        assert_eq!(ids, vec![Some(10), Some(20)]);
        assert!(items.iter().all(|i| i.action == "unsubscribe"));
        // 墓碑：目录 + 每个成员 URL
        assert!(db::folder_tombstones(&conn)
            .unwrap()
            .iter()
            .any(|t| t.label == "待删分类"));
        assert_eq!(db::feed_tombstones(&conn).unwrap().len(), 2);
    }

    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (folder delete)");
    {
        let conn = dbm.lock().await;
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "逐成员退订确认后出队"
        );
        assert_eq!(
            db::feed_tombstones(&conn).unwrap().len(),
            0,
            "远端确认已不列成员后收敛回收"
        );
    }
    let subs = server.subscriptions.lock().unwrap().clone();
    assert!(
        subs.iter().all(|s| s.id != "feed/10" && s.id != "feed/20"),
        "远端两个成员都已退订: {subs:?}"
    );
    let _ = std::fs::remove_file(&tmp);
}

/// OPT-009 阶段 B2（发现 2/3 的原子性）：分类级联删除中途 DB 故障必须**整体回滚**
/// ——目录、成员订阅、目录/成员墓碑、逐成员退订意图一起撤销（修前墓碑与删除不在
/// 同一事务，故障后留下半套状态）。
/// 注入点：第 2 个成员的退订意图 INSERT 处 RAISE(ABORT)（真实 SQLITE_CONSTRAINT）。
#[tokio::test]
async fn folder_delete_midway_failure_rolls_back_cascade_and_intents() {
    let tmp = common::unique_db_path("opt009b_b2_fdel_rb");
    let _ = std::fs::remove_file(&tmp);
    let url1 = "http://127.0.0.1:1/opt009-b2-rb-1.xml";
    let url2 = "http://127.0.0.1:1/opt009-b2-rb-2.xml";

    let conn = db::open(&tmp).unwrap();
    db::set_setting(&conn, "greader_endpoint", "http://127.0.0.1:1").unwrap();
    db::set_setting(&conn, "greader_username", "test").unwrap();
    db::set_setting(&conn, "greader_password", "test-token").unwrap();
    let (folder_id, _f1) = seed_bound_feed(&conn, url1, "待删分类", 10);
    let f2 = db::insert_feed(
        &conn, url2, None, "Del B", None, folder_id, "inherit", true, false,
    )
    .unwrap();
    db::set_feed_remote_id(&conn, f2, 20).unwrap();
    conn.execute_batch(&format!(
        "CREATE TRIGGER test_fail_second_unsub BEFORE INSERT ON subscription_outbox
         WHEN NEW.feed_url = '{url2}' BEGIN
             SELECT RAISE(ABORT, 'simulated midway failure');
         END;"
    ))
    .unwrap();

    let err = app_lib::commands::record_folder_delete(&conn, folder_id)
        .expect_err("注入的约束失败必须让整单失败");
    assert!(
        err.message.contains("simulated midway failure"),
        "必须是注入的失败现场: {err}"
    );

    let folders: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM folders WHERE id = ?1",
            [folder_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(folders, 1, "故障后目录必须原样（不得半删）");
    let feeds: i64 = conn
        .query_row("SELECT COUNT(*) FROM feeds WHERE active=1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(feeds, 2, "故障后成员订阅必须原样");
    assert!(
        db::folder_tombstones(&conn).unwrap().is_empty(),
        "事务内的目录墓碑必须回滚"
    );
    assert!(
        db::feed_tombstones(&conn).unwrap().is_empty(),
        "事务内的成员墓碑必须回滚"
    );
    assert!(
        db::pending_outbox(&conn).unwrap().is_empty(),
        "事务内的退订意图必须回滚"
    );
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
B2-4. 分类墓碑：读取失败关闭 + 跨账号隔离
============================================================ */

/// OPT-009 阶段 B2（发现 4）：分类墓碑读取失败 = 状态未知，必须记同步错误并
/// 跳过受影响的分类/订阅导入（不得按空集合继续建旧分类、也不得提交依赖该映射
/// 的 feed 状态）。恢复后下一轮收敛，证明不是永久冻结。
/// 判别力（旧实现 = `unwrap_or_default()` 当空表）：坏墓碑时远端 Default 目录
/// 与订阅会被照常导入（不复活保护失效）。
#[tokio::test]
async fn folder_tombstone_read_failure_skips_import_instead_of_reviving() {
    let server = MockGReader::start().await.expect("start mock");
    let tmp = common::unique_db_path("opt009b_b2_tomb_fail");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);

    let dbm = {
        let conn = open_with_credentials(&server, "greader", "test");
        // 故障注入：墓碑内容损坏（读取必然失败）
        db::set_setting(&conn, "folder_tombstones_v2", "{ not json").unwrap();
        assert!(
            db::folder_tombstones(&conn).is_err(),
            "前置：坏内容必须让墓碑读取真实失败"
        );
        Arc::new(Mutex::new(conn))
    };

    let report = sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (broken tombstone)");
    {
        let conn = dbm.lock().await;
        assert!(
            report.errors.iter().any(|e| e.contains("分类墓碑")),
            "必须记录可见同步错误: {report:?}"
        );
        let folders: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM folders WHERE name = 'Default'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            folders, 0,
            "状态未知时不得按远端 label 建目录（会复活已删分类）"
        );
        let feeds: i64 = conn
            .query_row("SELECT COUNT(*) FROM feeds WHERE active=1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            feeds, 0,
            "依赖 label→folder 映射的订阅导入必须一并跳过（不提交该状态）"
        );
        // 失败关闭：原值保留待重试
        assert_eq!(
            db::get_setting(&conn, "folder_tombstones_v2")
                .unwrap()
                .as_deref(),
            Some("{ not json")
        );
    }

    // 修复墓碑后下一轮收敛：远端目录与订阅照常导入
    {
        let conn = dbm.lock().await;
        db::set_setting(&conn, "folder_tombstones_v2", "[]").unwrap();
    }
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (recovered)");
    {
        let conn = dbm.lock().await;
        let folders: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM folders WHERE name = 'Default'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(folders, 1, "故障恢复后分类导入收敛");
        let feeds: i64 = conn
            .query_row("SELECT COUNT(*) FROM feeds WHERE active=1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(feeds, 2, "故障恢复后远端订阅（mock 两条）导入收敛");
    }
    let _ = std::fs::remove_file(&tmp);
}

/// OPT-009 阶段 B2（发现 4，跨账号隔离）：A 删除的「同名分类」墓碑不得压制 B 的
/// 远端同名分类；B 的 pull 也不得回收 A 的墓碑（回 A 后仍防复活）。
/// 判别力（旧实现 = 无账号归属的裸 label 列表）：B 的同名分类被 A 的墓碑抑制，
/// 且 B 的 pull 把 A 的墓碑当 stale 清掉。
#[tokio::test]
async fn folder_tombstone_is_account_scoped_across_switch() {
    let server = MockGReader::start().await.expect("start mock");
    let tmp = common::unique_db_path("opt009b_b2_tomb_acct");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);

    let mut conn = open_with_credentials(&server, "greader", "user-a");
    let key_a = acct_key("greader", &server, "user-a");
    let key_b = acct_key("greader", &server, "user-b");
    // A：删除本地同名分类 → 墓碑归属 A
    {
        let folder = db::create_folder(&conn, "同名分类", "article").unwrap();
        app_lib::commands::record_folder_delete(&conn, folder).expect("delete folder A");
        let ts = db::folder_tombstones(&conn).unwrap();
        assert_eq!(ts.len(), 1);
        assert_eq!(ts[0].label, "同名分类");
        assert_eq!(ts[0].account_key, key_a, "墓碑必须归属写入账号");
    }
    // 切到 B（身份变化：清理旧账号远端数据，但分类墓碑按 key 保留）
    app_lib::commands::save_account_for_test(
        &mut conn,
        "greader",
        &server.url(),
        "user-b",
        "pw-b",
        &server.url(),
    )
    .expect("switch account");
    let dbm = Arc::new(Mutex::new(conn));

    // B 的远端 tag/list 含「同名分类」→ 必须照常导入（不被 A 的墓碑抑制）
    server.folders.lock().unwrap().push("同名分类".into());
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (account B)");
    {
        let conn = dbm.lock().await;
        let imported: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM folders WHERE name = '同名分类'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            imported, 1,
            "异账号墓碑不得抑制本账号同名分类导入（B2-4 跨账号隔离）"
        );
        let ts = db::folder_tombstones(&conn).unwrap();
        assert_eq!(ts.len(), 1, "异账号墓碑不得被本账号 pull 回收: {ts:?}");
        assert_eq!(ts[0].account_key, key_a);
    }

    // 回 A：把本地同名分类删掉（模拟 A 本地已无该目录），A 的墓碑必须继续压制
    // 远端旧 label 的复活；A 的 pull 也不得回收 A 自己的墓碑。
    {
        let mut conn = dbm.lock().await;
        let existing: Option<i64> = conn
            .query_row(
                "SELECT id FROM folders WHERE name = '同名分类'",
                [],
                |r| r.get(0),
            )
            .ok();
        if let Some(fid) = existing {
            db::delete_folder(&conn, fid).expect("清掉 B 轮导入的同名分类，聚焦 A 的墓碑语义");
        }
        app_lib::commands::save_account_for_test(
            &mut conn,
            "greader",
            &server.url(),
            "user-a",
            "pw-a",
            &server.url(),
        )
        .expect("switch back to A");
    }
    let dbm_a = dbm.clone();
    sync::feeds_phase(&dbm_a, &http)
        .await
        .expect("feeds phase (account A again)");
    {
        let conn = dbm_a.lock().await;
        let dup: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM folders WHERE name = '同名分类'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dup, 0, "回 A 后墓碑仍压制旧 label（不得复活）");
        let ts = db::folder_tombstones(&conn).unwrap();
        assert!(ts
            .iter()
            .any(|t| t.label == "同名分类" && t.account_key == key_a));
        assert!(
            !ts.iter().any(|t| t.account_key == key_b),
            "B 从未删除分类：其 pull 不得凭空写入墓碑，也不得删除 A 的: {ts:?}"
        );
    }
    let _ = key_b;
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
B2-5. OPML 标题：desired_title + 真实 t= wire
============================================================ */

/// OPT-009 阶段 B2（发现 5）：导入的非空标题进入 typed subscribe 的
/// `desired_title`，quick_add 后用真实 `t=` 写回远端；空标题不产生标题目标
/// （不伪造远端标题）。重复 URL/已有 feed 仍按既有语义跳过——不得用被跳过的
/// 那一条的标题覆盖既有目标，也不得回落 legacy add_feed。
/// 判别力（旧实现 = 标题不进队项）：wire 无 `t=`、mock 远端标题不变。
#[tokio::test]
async fn opml_import_title_lands_on_remote_wire_and_mock() {
    let server = MockGReader::start().await.expect("start mock");
    let tmp = common::unique_db_path("opt009b_b2_opml_title");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url_titled = format!("{}/opt009-b2-title.xml", server.url());
    let url_untitled = format!("{}/opt009-b2-untitled.xml", server.url());

    // 远端已有两个订阅（幂等 quick_add 返回既有 id）
    server
        .existing_feed_urls
        .lock()
        .unwrap()
        .push((url_titled.clone(), 30));
    server
        .existing_feed_urls
        .lock()
        .unwrap()
        .push((url_untitled.clone(), 31));
    *server.subscriptions.lock().unwrap() = vec![
        MockSubscription {
            id: "feed/30".into(),
            title: "服务端发现标题".into(),
            url: url_titled.clone(),
            html_url: None,
            categories: vec![],
        },
        MockSubscription {
            id: "feed/31".into(),
            title: "服务端发现标题2".into(),
            url: url_untitled.clone(),
            html_url: None,
            categories: vec![],
        },
    ];

    let dbm = {
        let conn = open_with_credentials(&server, "greader", "test");
        let report = app_lib::commands::opml_import_for_test(
            &conn,
            &opml_doc(&[
                (url_titled.as_str(), "我的标题", Some("导入目录")),
                // 空标题：不产生 t= 目标
                (url_untitled.as_str(), "", None),
            ]),
        )
        .expect("opml import");
        assert_eq!((report.imported, report.skipped), (2, 0));
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 2, "两条导入各自一条 typed 意图: {items:?}");
        let titled = items
            .iter()
            .find(|i| i.feed_url == url_titled)
            .expect("带标题的意图");
        assert_eq!(titled.desired_title.as_deref(), Some("我的标题"));
        assert_eq!(titled.desired_folder.as_deref(), Some("导入目录"));
        let untitled = items
            .iter()
            .find(|i| i.feed_url == url_untitled)
            .expect("无标题的意图");
        assert_eq!(
            untitled.desired_title, None,
            "无可用标题（解析回退成 URL）不得伪造 t= 目标"
        );
        let legacy: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_queue WHERE action = 'add_feed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(legacy, 0, "不得双投 legacy add_feed");
        Arc::new(Mutex::new(conn))
    };

    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (opml titles)");
    {
        let conn = dbm.lock().await;
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "挂载成功后精确出队"
        );
    }
    let subs = server.subscriptions.lock().unwrap().clone();
    let titled_remote = subs.iter().find(|s| s.id == "feed/30").unwrap();
    assert_eq!(
        titled_remote.title, "我的标题",
        "非空导入标题必须用真实 t= 写回远端"
    );
    let labels: Vec<String> = titled_remote
        .categories
        .iter()
        .map(|(l, _)| l.clone())
        .collect();
    assert!(
        labels.contains(&"导入目录".to_string()),
        "分类目标必须随挂载写回: {labels:?}"
    );
    let untitled_remote = subs.iter().find(|s| s.id == "feed/31").unwrap();
    assert_eq!(
        untitled_remote.title, "服务端发现标题2",
        "空标题不得改写远端标题"
    );

    // 重复 URL / 已有 feed：再次导入同名 URL → 跳过，既有目标不被覆盖
    {
        let conn = dbm.lock().await;
        let report = app_lib::commands::opml_import_for_test(
            &conn,
            &opml_doc(&[(url_titled.as_str(), "另一个标题", Some("另一个目录"))]),
        )
        .expect("re-import");
        assert_eq!((report.imported, report.skipped), (0, 1), "已有源必须跳过");
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "跳过不得产生新意图、也不得覆盖既有目标"
        );
    }
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
R1-1. 删除与在途 quick_add 竞态：未知远端目标也必须落可恢复退订意图
============================================================ */

/// OPT-009B2 复审 P2（返工 R1）：pending subscribe 的 `quick_add` 已发出、响应
/// 在途时删除该源——旧实现 `enqueue_unsubscribe` 找不到可继承 id 就**不写行**，
/// 在途成功后的 `adopt_remote_id_for_pending` 也无替代行可填，远端订阅成为孤儿。
/// 修后：删除时同账号同 URL 仍有 pending 队项 = 创建证据 → 写 `remote_id IS NULL`
/// 的退订意图；响应在本地、同 key 同代际返回时由 adopt 结转已验证 id；下一轮按
/// id 退订；墓碑只在 pull 确认远端已不含后回收。
#[tokio::test]
async fn delete_while_quickadd_inflight_keeps_durable_unsubscribe() {
    let server = MockGReader::start().await.expect("start mock");
    let tmp = common::unique_db_path("opt009b_r1_del_inflight");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = format!("{}/opt009-r1-del.xml", server.url());
    let key = acct_key("greader", &server, "test");

    let (dbm, feed_id) = {
        let conn = open_with_credentials(&server, "greader", "test");
        let folder = db::create_folder(&conn, "分类", "article").unwrap();
        let feed = db::insert_feed(
            &conn,
            &url,
            None,
            "Inflight Del",
            None,
            folder,
            "inherit",
            true,
            false,
        )
        .unwrap();
        let gen = db::sync_generation(&conn).unwrap();
        db::enqueue_subscribe(&conn, gen, &key, &url, None, Some("分类")).unwrap();
        (Arc::new(Mutex::new(conn)), feed)
    };

    // quick_add 响应在途（服务端已创建）
    let barrier = server.arm_quickadd_barrier("opt009-r1-del");
    let db2 = dbm.clone();
    let http2 = http.clone();
    let handle = tokio::spawn(async move { sync::feeds_phase(&db2, &http2).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), barrier.arrived)
        .await
        .expect("执行器必须在期限内发出 quick_add（网络屏障）")
        .expect("barrier arrived");

    // 服务端已创建（幂等登记给出 id）→ 也放进远端订阅列表：孤儿现场
    let remote_id = {
        let id = server
            .existing_feed_urls
            .lock()
            .unwrap()
            .iter()
            .find(|(u, _)| u == &url)
            .expect("quick_add 创建必须幂等登记既有 id")
            .1;
        server.subscriptions.lock().unwrap().push(MockSubscription {
            id: format!("feed/{id}"),
            title: "Inflight Del".into(),
            url: url.clone(),
            html_url: None,
            categories: vec![("分类".into(), "folder".into())],
        });
        id
    };

    // 在途期间删除（生产命令逻辑；本地尚无 remote_id）
    {
        let conn = dbm.lock().await;
        let target =
            app_lib::commands::record_feed_deletion(&conn, feed_id).expect("record deletion");
        assert!(
            target.is_none(),
            "本地尚无 remote_id：命令不返回即时推送目标（可恢复意图在队列里）"
        );
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(
            items.len(),
            1,
            "删除必须落下可恢复退订意图（旧实现此处为空 → 孤儿）: {items:?}"
        );
        assert_eq!(items[0].action, "unsubscribe");
        assert_eq!(
            items[0].remote_id, None,
            "id 此刻不可知（响应在途），由执行器幂等解析或 adopt 结转: {items:?}"
        );
        let left: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM feeds WHERE active=1 AND id = ?1",
                [feed_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(left, 0, "本地删除必须已生效（与意图同一事务）");
        assert!(
            db::feed_tombstones(&conn)
                .unwrap()
                .iter()
                .any(|u| u.contains("opt009-r1-del")),
            "删除墓碑必须已落"
        );
    }

    barrier.release.send(()).unwrap();
    let _ = handle
        .await
        .expect("join feeds_phase")
        .expect("feeds phase (in-flight delete)");

    // 响应在本地、同 key 同代际 → adopt 把已验证 id 补给退订意图
    {
        let conn = dbm.lock().await;
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1, "本轮仍只应有退订意图: {items:?}");
        assert_eq!(
            items[0].remote_id,
            Some(remote_id),
            "在途创建成功的已验证 id 必须结转给退订意图: {items:?}"
        );
        assert!(
            db::feed_tombstones(&conn)
                .unwrap()
                .iter()
                .any(|u| u.contains("opt009-r1-del")),
            "远端列表仍含该 URL：墓碑不得被提前回收"
        );
        let revived: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM feeds WHERE active=1 AND feed_url = ?1",
                [&url],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(revived, 0, "墓碑必须挡住 pull 复活");
    }

    // 下一轮：按结转 id 退订 → 远端移除 → pull 确认后墓碑收敛
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (retry)");
    {
        assert!(
            mock_greader::subscription_edit_actions(&server)
                .iter()
                .any(|(ac, s)| ac == "unsubscribe" && s == &format!("feed/{remote_id}")),
            "重试必须按已验证 id 发出 ac=unsubscribe"
        );
        assert!(
            server
                .subscriptions
                .lock()
                .unwrap()
                .iter()
                .all(|s| s.id != format!("feed/{remote_id}")),
            "远端订阅已移除（孤儿收敛）"
        );
        let conn = dbm.lock().await;
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "退订确认后精确出队"
        );
        assert!(
            db::feed_tombstones(&conn).unwrap().is_empty(),
            "pull 确认远端已不含该 URL 后墓碑才回收"
        );
        let revived: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM feeds WHERE active=1 AND feed_url = ?1",
                [&url],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(revived, 0, "收敛后仍不得复活");
    }
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
R1-2. 分类级联删除与在途 quick_add 竞态：成员同样必须可恢复
============================================================ */

/// OPT-009B2 复审 P2（返工 R1，分类路径）：成员 subscribe 的 quick_add 在途时
/// 删除其所在分类——级联事务必须同时得到「成员退订意图（未知 id）+ 目录/成员
/// 墓碑 + 本地级联删除」；在途成功结转 id 后重试退订，成员不随目录复活。
#[tokio::test]
async fn folder_delete_while_quickadd_inflight_keeps_member_unsubscribe() {
    let server = MockGReader::start().await.expect("start mock");
    let tmp = common::unique_db_path("opt009b_r1_fdel_inflight");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = format!("{}/opt009-r1-fdel.xml", server.url());
    let key = acct_key("greader", &server, "test");

    let (dbm, folder_id) = {
        let conn = open_with_credentials(&server, "greader", "test");
        let folder = db::create_folder(&conn, "待删分类", "article").unwrap();
        db::insert_feed(
            &conn, &url, None, "Member", None, folder, "inherit", true, false,
        )
        .unwrap();
        let gen = db::sync_generation(&conn).unwrap();
        db::enqueue_subscribe(&conn, gen, &key, &url, None, Some("待删分类")).unwrap();
        (Arc::new(Mutex::new(conn)), folder)
    };
    // 远端 tag/list 也列出该分类（墓碑在远端仍列出时必须保留）
    server.folders.lock().unwrap().push("待删分类".into());

    let barrier = server.arm_quickadd_barrier("opt009-r1-fdel");
    let db2 = dbm.clone();
    let http2 = http.clone();
    let handle = tokio::spawn(async move { sync::feeds_phase(&db2, &http2).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), barrier.arrived)
        .await
        .expect("执行器必须在期限内发出 quick_add（网络屏障）")
        .expect("barrier arrived");
    let remote_id = {
        let id = server
            .existing_feed_urls
            .lock()
            .unwrap()
            .iter()
            .find(|(u, _)| u == &url)
            .expect("quick_add 创建必须幂等登记既有 id")
            .1;
        server.subscriptions.lock().unwrap().push(MockSubscription {
            id: format!("feed/{id}"),
            title: "Member".into(),
            url: url.clone(),
            html_url: None,
            categories: vec![("待删分类".into(), "folder".into())],
        });
        id
    };

    // 在途期间删除分类（生产命令逻辑；成员本地尚无 remote_id）
    {
        let conn = dbm.lock().await;
        app_lib::commands::record_folder_delete(&conn, folder_id).expect("delete folder");
        let folders: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM folders WHERE id = ?1",
                [folder_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(folders, 0, "目录已级联删除");
        let feeds: i64 = conn
            .query_row("SELECT COUNT(*) FROM feeds WHERE active=1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(feeds, 0, "成员订阅随目录删除");
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(
            items.len(),
            1,
            "成员必须留下可恢复退订意图（旧实现为空 → 孤儿）: {items:?}"
        );
        assert_eq!(items[0].action, "unsubscribe");
        assert_eq!(items[0].remote_id, None);
        assert!(
            db::folder_tombstones(&conn)
                .unwrap()
                .iter()
                .any(|t| t.label == "待删分类"),
            "目录墓碑已落"
        );
        assert!(
            db::feed_tombstones(&conn)
                .unwrap()
                .iter()
                .any(|u| u.contains("opt009-r1-fdel")),
            "成员墓碑已落"
        );
    }

    barrier.release.send(()).unwrap();
    let _ = handle
        .await
        .expect("join feeds_phase")
        .expect("feeds phase (in-flight folder delete)");
    {
        let conn = dbm.lock().await;
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(
            items[0].remote_id,
            Some(remote_id),
            "在途成功必须结转成员 id"
        );
    }

    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (retry)");
    {
        assert!(
            mock_greader::subscription_edit_actions(&server)
                .iter()
                .any(|(ac, s)| ac == "unsubscribe" && s == &format!("feed/{remote_id}")),
            "重试必须发出成员退订"
        );
        assert!(
            server
                .subscriptions
                .lock()
                .unwrap()
                .iter()
                .all(|s| s.id != format!("feed/{remote_id}")),
            "远端成员已移除"
        );
        let conn = dbm.lock().await;
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "退订确认后出队"
        );
        let folder: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM folders WHERE name = '待删分类'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(folder, 0, "目录不得复活");
        let feed: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM feeds WHERE active=1 AND feed_url = ?1",
                [&url],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(feed, 0, "成员不得随目录复活");
        assert!(
            db::folder_tombstones(&conn)
                .unwrap()
                .iter()
                .any(|t| t.label == "待删分类"),
            "远端 tag/list 仍列出该 label：目录墓碑必须保留"
        );
    }
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
R1-3. 普通删除的事务失败：本地删除与墓碑一起回滚
============================================================ */

/// OPT-009B2 复审 P2（返工 R1）：普通删除单事务的第二段（typed 退订意图 INSERT）
/// 失败时，本地删除与墓碑必须一起回滚（不得留下「源已删、无意图」的半套状态）。
/// 注入点：真实 SQLite RAISE(ABORT)。
#[tokio::test]
async fn record_feed_deletion_outbox_failure_rolls_back_local_and_tombstone() {
    let tmp = common::unique_db_path("opt009b_r1_del_rb");
    let _ = std::fs::remove_file(&tmp);
    let url = "http://127.0.0.1:1/opt009-r1-rb.xml";

    let conn = db::open(&tmp).unwrap();
    db::set_setting(&conn, "greader_endpoint", "http://127.0.0.1:1").unwrap();
    db::set_setting(&conn, "greader_username", "test").unwrap();
    db::set_setting(&conn, "greader_password", "test-token").unwrap();
    let (_folder, feed) = seed_bound_feed(&conn, url, "分类", 10);
    conn.execute_batch(&format!(
        "CREATE TRIGGER test_fail_unsub BEFORE INSERT ON subscription_outbox
         WHEN NEW.feed_url = '{url}' BEGIN
             SELECT RAISE(ABORT, 'simulated outbox failure');
         END;"
    ))
    .unwrap();

    let err = app_lib::commands::record_feed_deletion(&conn, feed)
        .expect_err("注入的队列入队失败必须让整单失败");
    assert!(
        err.message.contains("simulated outbox failure"),
        "必须是注入的失败现场: {err}"
    );

    let feeds: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM feeds WHERE active=1 AND id = ?1",
            [feed],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(feeds, 1, "故障后本地订阅必须原样（不得半删）");
    assert!(
        db::feed_tombstones(&conn).unwrap().is_empty(),
        "事务内的删除墓碑必须一起回滚"
    );
    assert!(
        db::pending_outbox(&conn).unwrap().is_empty(),
        "事务内的退订意图必须一起回滚"
    );
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
R1-4. 在途 quick_add + 换号：清账后在途响应零写入，远端孤儿如实保留
============================================================ */

/// OPT-009B2 复审 P2（返工 R1，账号边界）+ 2026-11-02 生命周期终点：在途
/// quick_add 的响应在**换号（已清账）之后**才返回——行已不存在，代际守卫与
/// adopt 守卫叠加保证零写入；新账号轮次零订阅写请求；旧语义的「切回原账号
/// 幂等解析收敛」已放弃，远端孤儿如实保留（由用户手动清理）。
#[tokio::test]
async fn inflight_quickadd_response_after_account_switch_writes_nothing_after_cleanup() {
    let server = MockGReader::start().await.expect("start mock");
    let tmp = common::unique_db_path("opt009b_r1_switch");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = format!("{}/opt009-r1-switch.xml", server.url());
    let key_a = acct_key("greader", &server, "user-a");

    let conn = open_with_credentials(&server, "greader", "user-a");
    let feed_id = {
        let folder = db::create_folder(&conn, "分类", "article").unwrap();
        let feed = db::insert_feed(
            &conn, &url, None, "Switch", None, folder, "inherit", true, false,
        )
        .unwrap();
        let gen = db::sync_generation(&conn).unwrap();
        db::enqueue_subscribe(&conn, gen, &key_a, &url, None, Some("分类")).unwrap();
        feed
    };
    let dbm = Arc::new(Mutex::new(conn));

    // A 的 quick_add 在途
    let barrier = server.arm_quickadd_barrier("opt009-r1-switch");
    let db2 = dbm.clone();
    let http2 = http.clone();
    let handle = tokio::spawn(async move { sync::feeds_phase(&db2, &http2).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), barrier.arrived)
        .await
        .expect("执行器必须在期限内发出 quick_add（网络屏障）")
        .expect("barrier arrived");
    let remote_id = {
        let id = server
            .existing_feed_urls
            .lock()
            .unwrap()
            .iter()
            .find(|(u, _)| u == &url)
            .expect("quick_add 创建必须幂等登记既有 id")
            .1;
        server.subscriptions.lock().unwrap().push(MockSubscription {
            id: format!("feed/{id}"),
            title: "Switch".into(),
            url: url.clone(),
            html_url: None,
            categories: vec![("分类".into(), "folder".into())],
        });
        id
    };

    // 在途期间：删除本地源 → 意图（key=A、id 未知）；随后换到账号 B
    {
        let mut conn = dbm.lock().await;
        app_lib::commands::record_feed_deletion(&conn, feed_id).expect("record deletion");
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].account_key, key_a);
        assert_eq!(items[0].remote_id, None, "adopt 尚未发生");
        app_lib::commands::save_account_for_test(
            &mut conn,
            "greader",
            &server.url(),
            "user-b",
            "pw-b",
            &server.url(),
        )
        .expect("switch to B");
    }

    barrier.release.send(()).unwrap();
    let _ = handle
        .await
        .expect("join feeds_phase")
        .expect("feeds phase (in-flight, switched)");
    {
        let conn = dbm.lock().await;
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM subscription_outbox", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 0, "换号已清账：在途响应不得写回任何行（含 adopt）");
    }
    assert!(
        mock_greader::subscription_edit_actions(&server)
            .iter()
            .all(|(ac, _)| ac != "unsubscribe"),
        "释放旧响应本身不得发出任何退订（换号后旧会话不写库不发请求）"
    );

    // B 的轮次：不得看到/发送 A 的意图与 id
    server.clear_request_log();
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (account B)");
    {
        let log = server.request_log();
        assert!(
            !log.iter().any(|r| r.contains("subscription/edit")),
            "B 的轮次不得发出任何订阅编辑/退订请求: {log:?}"
        );
        let conn = dbm.lock().await;
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM subscription_outbox", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 0, "B 轮次后仍无任何队项");
    }

    // 切回 A：旧语义「幂等解析收敛」已放弃——零退订请求、远端孤儿保留
    {
        let mut conn = dbm.lock().await;
        app_lib::commands::save_account_for_test(
            &mut conn,
            "greader",
            &server.url(),
            "user-a",
            "pw-a",
            &server.url(),
        )
        .expect("switch back to A");
    }
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (back to A)");
    {
        assert!(
            mock_greader::subscription_edit_actions(&server)
                .iter()
                .all(|(ac, _)| ac != "unsubscribe"),
            "清账后没有任何退订意图可执行（远端孤儿由用户手动清理）"
        );
        assert_eq!(
            quickadd_count(&server, url.as_str()),
            1,
            "全程只有 A 在途那一次 quick_add（无新意图可重发）"
        );
        assert!(
            server
                .subscriptions
                .lock()
                .unwrap()
                .iter()
                .any(|s| s.id == format!("feed/{remote_id}")),
            "语义放弃的代价：远端孤儿订阅如实保留（本用例锁定该形态）"
        );
        let conn = dbm.lock().await;
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "A 回场后也无队项（清账即生命周期终点）"
        );
    }
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
R1-5. 移动成功后删除 + 重加：已接受的 label 必须被移除（F-1）
============================================================ */

/// OPT-009B2 复审 F-1（返工 R1）：A→B 的移动 HTTP 已成功（远端已在 B），在途期间
/// 用户删除该源并重新添加同一 URL——后继行从 unsubscribe 变成新的 **subscribe**。
/// 旧实现的重定只认 edit 行，重加后的挂载只发 `a=`，远端残留 B。
/// R2 复审（P3-A）后时序语义：删除时把被替换移动行的**链起点** A 快照进退订行
/// （移动尚未确认，重定的正确基准只有执行器知道），重加继承 A；移动的 2xx 返回时
/// 执行器把替代 subscribe 行的移除基准**重定为已接受目标 B**——本用例因此判别
/// 「重定确实发生」（终态 wire `a=新 + r=B`、远端不残留任何中间 label）。
#[tokio::test]
async fn move_success_then_delete_readd_removes_accepted_label() {
    let server = MockGReader::start().await.expect("start mock");
    let tmp = common::unique_db_path("opt009b_r1_readd");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = format!("{}/opt009-r1-readd.xml", server.url());
    let key = acct_key("greader", &server, "test");

    *server.subscriptions.lock().unwrap() = vec![MockSubscription {
        id: "feed/10".into(),
        title: "Readd".into(),
        url: url.clone(),
        html_url: None,
        categories: vec![
            ("A分类".into(), "folder".into()),
            ("保留分类".into(), "folder".into()),
        ],
    }];
    server
        .existing_feed_urls
        .lock()
        .unwrap()
        .push((url.clone(), 10));

    let (dbm, feed_id) = {
        let conn = open_with_credentials(&server, "greader", "test");
        let (folder_a, feed) = seed_bound_feed(&conn, &url, "A分类", 10);
        assert_eq!(folder_a, 1);
        db::create_folder(&conn, "B分类", "article").unwrap();
        db::create_folder(&conn, "新分类", "article").unwrap();
        let gen = db::sync_generation(&conn).unwrap();
        assert!(gen == 0);
        let _ = &key;
        (Arc::new(Mutex::new(conn)), feed)
    };

    // 未发送窗口入队 A→B，然后让它的响应在途（服务端已应用）
    let barrier = server.arm_subscription_edit_barrier("feed/10");
    {
        let conn = dbm.lock().await;
        let folder_b = db::list_folders(&conn)
            .unwrap()
            .into_iter()
            .find(|f| f.name == "B分类")
            .expect("B分类存在")
            .id;
        app_lib::commands::record_feed_edit(
            &conn,
            feed_id,
            None,
            Some(folder_b),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
    }
    let db2 = dbm.clone();
    let http2 = http.clone();
    let handle = tokio::spawn(async move { sync::feeds_phase(&db2, &http2).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), barrier.arrived)
        .await
        .expect("执行器必须在期限内发出移动请求（网络屏障）")
        .expect("barrier arrived");

    // 在途期间：删除该源 → 重加同一 URL（生产 OPML 导入路径）
    {
        let conn = dbm.lock().await;
        app_lib::commands::record_feed_deletion(&conn, feed_id).expect("delete in flight");
        let report = app_lib::commands::opml_import_for_test(
            &conn,
            &opml_doc(&[(url.as_str(), "重加标题", Some("新分类"))]),
        )
        .expect("re-add same url");
        assert_eq!(
            (report.imported, report.skipped),
            (1, 0),
            "重加必须导入新源"
        );
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1, "同 URL 只留最新意图: {items:?}");
        assert_eq!(items[0].action, "subscribe", "后继是重加的 subscribe 行");
        assert_eq!(items[0].desired_folder.as_deref(), Some("新分类"));
        assert_eq!(
            items[0].prev_folder.as_deref(),
            Some("A分类"),
            "重加行继承被替换退订行的**链起点**（本地已 B、但移动未确认，远端仍持 A；\
             移动成功响应返回后由执行器重定为已接受目标 B）: {items:?}"
        );
    }

    barrier.release.send(()).unwrap();
    let _ = handle
        .await
        .expect("join feeds_phase")
        .expect("feeds phase (in-flight move, deleted+readded)");
    {
        let conn = dbm.lock().await;
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].action, "subscribe");
        assert_eq!(
            items[0].prev_folder.as_deref(),
            Some("B分类"),
            "移动 2xx 返回后执行器必须把替代行的移除基准重定为已接受目标（链起点 A → B；\
             否则挂载会发 r=A、远端残留 B）: {items:?}"
        );
    }

    // 重试轮：挂载 a=新分类 + r=B分类，远端只剩新目标与无关标签
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (readd retry)");
    {
        let form = mock_greader::last_subscription_edit_form(&server);
        assert!(
            form_has(&form, "a", "user/-/label/新分类")
                && form_has(&form, "r", "user/-/label/B分类"),
            "重加挂载必须 a=新 + r=B（移除已接受 label），实际 {form:?}"
        );
        let labels: Vec<String> = server
            .subscriptions
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.id == "feed/10")
            .expect("远端订阅仍在")
            .categories
            .iter()
            .map(|(l, _)| l.clone())
            .collect();
        assert!(
            labels.contains(&"新分类".to_string()),
            "远端必须加入重加的目标分类: {labels:?}"
        );
        assert!(
            !labels.contains(&"B分类".to_string()),
            "已接受但被删除/重加取代的中间 label 必须移除: {labels:?}"
        );
        assert!(
            !labels.contains(&"A分类".to_string()),
            "链起点也已移除: {labels:?}"
        );
        assert!(
            labels.contains(&"保留分类".to_string()),
            "无关 label 必须保留: {labels:?}"
        );
        let conn = dbm.lock().await;
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "重加挂载确认后出队"
        );
        let bound: Option<i64> = conn
            .query_row(
                "SELECT remote_id FROM feeds WHERE feed_url = ?1",
                [&url],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bound, Some(10), "重加的源必须绑定到既有远端订阅（幂等）");
    }
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
R1-6. remote_created 窗口成员的分类改名：真实挂载 wire（a=新, r=旧）
============================================================ */

/// OPT-009B2 复审缺口（返工 R1）：分类改名作用于「subscribe 行已是 remote_created、
/// 但本地 feed 尚未拿到 remote_id」的成员时，必须经生产命令与执行器实际发出
/// `a=` 新 label + `r=` 旧 label（原语级单测不足以证明 wire），且不得重发 quick_add。
#[tokio::test]
async fn folder_rename_remote_created_member_mounts_move_wire() {
    let server = MockGReader::start().await.expect("start mock");
    let tmp = common::unique_db_path("opt009b_r1_rc_rename");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = format!("{}/opt009-r1-rc.xml", server.url());
    let key = acct_key("greader", &server, "test");

    *server.subscriptions.lock().unwrap() = vec![MockSubscription {
        id: "feed/30".into(),
        title: "RC Member".into(),
        url: url.clone(),
        html_url: None,
        categories: vec![
            ("旧分类".into(), "folder".into()),
            ("保留分类".into(), "folder".into()),
        ],
    }];

    let (dbm, folder_id) = {
        let conn = open_with_credentials(&server, "greader", "test");
        let folder = db::create_folder(&conn, "旧分类", "article").unwrap();
        let _feed = db::insert_feed(
            &conn,
            &url,
            None,
            "RC Member",
            None,
            folder,
            "inherit",
            true,
            false,
        )
        .unwrap();
        let gen = db::sync_generation(&conn).unwrap();
        db::enqueue_subscribe(&conn, gen, &key, &url, None, Some("旧分类")).unwrap();
        let row = db::pending_outbox(&conn).unwrap()[0].id;
        assert!(
            db::advance_subscribe_stage(&conn, row, gen, 30).unwrap(),
            "前置：subscribe 行推进 remote_created（远端已建、本地未绑定）"
        );
        let bound: Option<i64> = conn
            .query_row(
                "SELECT remote_id FROM feeds WHERE feed_url = ?1",
                [&url],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bound, None, "前置：本地 feed 尚无 remote_id");
        (Arc::new(Mutex::new(conn)), folder)
    };

    // 生产命令：分类改名（成员走 remote_created 窗口的 desired/prev 合并）
    {
        let conn = dbm.lock().await;
        app_lib::commands::record_folder_rename(&conn, folder_id, "新分类").expect("rename");
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(
            items.len(),
            1,
            "改名不得换行（原地并入订阅目标）: {items:?}"
        );
        assert_eq!(items[0].action, "subscribe");
        assert_eq!(items[0].desired_folder.as_deref(), Some("新分类"));
        assert_eq!(
            items[0].prev_folder.as_deref(),
            Some("旧分类"),
            "旧 label 必须随目标并入（挂载用 r= 移除）"
        );
    }

    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (rename remote_created member)");
    {
        let form = mock_greader::last_subscription_edit_form(&server);
        assert!(
            form_has(&form, "a", "user/-/label/新分类")
                && form_has(&form, "r", "user/-/label/旧分类"),
            "remote_created 成员挂载必须实际发出 a=新 + r=旧，实际 {form:?}"
        );
        assert_eq!(
            quickadd_requests(&server),
            0,
            "remote_created 行不得重发 quick_add"
        );
        let labels: Vec<String> = server
            .subscriptions
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.id == "feed/30")
            .expect("远端订阅仍在")
            .categories
            .iter()
            .map(|(l, _)| l.clone())
            .collect();
        assert!(
            labels.contains(&"新分类".to_string()) && !labels.contains(&"旧分类".to_string()),
            "远端成员必须完成移动: {labels:?}"
        );
        assert!(
            labels.contains(&"保留分类".to_string()),
            "无关 label 必须保留: {labels:?}"
        );
        let conn = dbm.lock().await;
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "挂载确认后出队"
        );
        let bound: Option<i64> = conn
            .query_row(
                "SELECT remote_id FROM feeds WHERE feed_url = ?1",
                [&url],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bound, Some(30), "绑定完成");
    }
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
7. quick_add 响应在途期间原地更新目标：必须发送重读后的最新目标
============================================================ */

/// B1-R2（复审 P2）：执行器不得使用计划快照里的 desired_title/desired_folder
/// 发送挂载请求。quick_add 响应在途期间 record_feed_edit 经
/// `update_pending_subscribe_desired` 把同一行 A→B（同 id），执行器必须先重读
/// 当前行再发送：请求带 B、且不得带 A。
/// 判别力：修前发送的是快照 A（表单无 t=、a=A分类）。
#[tokio::test]
async fn quickadd_window_edit_sends_latest_target_not_plan_snapshot() {
    let server = MockGReader::start().await.expect("start mock");
    server.subscriptions.lock().unwrap().clear();
    let tmp = common::unique_db_path("opt009b_qa_window");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = "http://127.0.0.1:1/opt009-qa-window.xml";
    let key = acct_key("greader", &server, "test");

    let (dbm, feed_id, folder_b) = {
        let conn = db::open(&tmp).unwrap();
        db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
        db::set_setting(&conn, "greader_username", "test").unwrap();
        db::set_setting(&conn, "greader_password", "test-token").unwrap();
        let folder_a = db::create_folder(&conn, "A分类", "article").unwrap();
        let folder_b = db::create_folder(&conn, "B分类", "article").unwrap();
        let feed = db::insert_feed(
            &conn, url, None, "A标题", None, folder_a, "inherit", true, false,
        )
        .unwrap();
        let gen = db::sync_generation(&conn).unwrap();
        // 计划快照：desired = A（标题未设置、分类 A分类）
        db::enqueue_subscribe(&conn, gen, &key, url, None, Some("A分类")).unwrap();
        (Arc::new(Mutex::new(conn)), feed, folder_b)
    };

    // quick_add 响应在途：服务端已创建订阅、响应未写回
    let barrier = server.arm_quickadd_barrier("opt009-qa-window");
    let db2 = dbm.clone();
    let http2 = http.clone();
    let handle = tokio::spawn(async move { sync::feeds_phase(&db2, &http2).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), barrier.arrived)
        .await
        .expect("执行器必须在期限内发出 quick_add（网络屏障）")
        .expect("barrier arrived");

    // 在途期间本地编辑（生产命令路径；remote_id=None → 原地并入 pending subscribe 行）
    {
        let conn = dbm.lock().await;
        let push = app_lib::commands::record_feed_edit(
            &conn,
            feed_id,
            Some("B标题"),
            Some(folder_b),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(
            push.is_none(),
            "remote_created 窗口无直接推送目标（并入订阅行）"
        );
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1, "原地更新不得替换行（同 id 合并）");
        assert_eq!(items[0].desired_title.as_deref(), Some("B标题"));
        assert_eq!(items[0].desired_folder.as_deref(), Some("B分类"));
    }

    barrier.release.send(()).unwrap();
    let _ = handle
        .await
        .expect("join feeds_phase")
        .expect("feeds phase (quickadd window)");

    {
        let form = last_subscription_edit_form(&server);
        assert!(
            form_has(&form, "t", "B标题"),
            "挂载必须发送重读后的最新标题，实际 {form:?}"
        );
        assert!(
            form_has(&form, "a", "user/-/label/B分类"),
            "挂载必须发送重读后的最新目标分类，实际 {form:?}"
        );
        assert!(
            !form
                .iter()
                .any(|(k, v)| (k == "a" && v == "user/-/label/A分类")
                    || (k == "t" && v == "A标题")),
            "计划快照的旧目标不得被发送，实际 {form:?}"
        );
    }
    {
        let conn = dbm.lock().await;
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "最新目标发送完整确认后精确出队"
        );
        let bound: Option<i64> = conn
            .query_row(
                "SELECT remote_id FROM feeds WHERE feed_url = ?1",
                [url],
                |r| r.get(0),
            )
            .unwrap();
        assert!(bound.is_some(), "绑定必须完成");
    }
    assert_eq!(
        quickadd_requests(&server),
        1,
        "只有一次 quick_add 请求（request-level）"
    );
    assert_eq!(
        quickadd_count(&server, url),
        1,
        "远端只创建一次（created-count）"
    );
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
8. 挂载响应在途期间原地更新目标：旧成功不得 prune 未发送的新目标
============================================================ */

/// B1-R2：确认（prune）必须保证「最新目标已被发送」。挂载请求发出后、
/// 响应在途期间同一行被原地更新（A→B，同 id/代际），旧成功返回时不得按
/// id 出队——否则 B 意图静默丢失。队项必须保留并在下一轮发送 B。
/// 判别力：仅按 (id, generation) prune 时，本用例「行仍在且 desired=B」必红。
#[tokio::test]
async fn mount_window_in_place_update_is_not_pruned_by_old_target() {
    let server = MockGReader::start().await.expect("start mock");
    server.subscriptions.lock().unwrap().clear();
    let tmp = common::unique_db_path("opt009b_mount_window");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = "http://127.0.0.1:1/opt009-mount-window.xml";
    let key = acct_key("greader", &server, "test");

    let dbm = {
        let conn = db::open(&tmp).unwrap();
        db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
        db::set_setting(&conn, "greader_username", "test").unwrap();
        db::set_setting(&conn, "greader_password", "test-token").unwrap();
        let folder_a = db::create_folder(&conn, "A分类", "article").unwrap();
        db::insert_feed(
            &conn,
            url,
            None,
            "挂载窗口",
            None,
            folder_a,
            "inherit",
            true,
            false,
        )
        .unwrap();
        let gen = db::sync_generation(&conn).unwrap();
        db::enqueue_subscribe(&conn, gen, &key, url, None, Some("A分类")).unwrap();
        Arc::new(Mutex::new(conn))
    };

    // 第一轮：挂载（A）响应在途
    let barrier = server.arm_subscription_edit_barrier("feed/");
    let db2 = dbm.clone();
    let http2 = http.clone();
    let handle = tokio::spawn(async move { sync::feeds_phase(&db2, &http2).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), barrier.arrived)
        .await
        .expect("执行器必须在期限内发出挂载（网络屏障）")
        .expect("barrier arrived");

    // 在途期间原地更新同一行目标 A→B（同 id；模拟 record_feed_edit 的合并语义
    // 在更晚时序发生——确认必须能识别「发送后目标已变」）
    {
        let conn = dbm.lock().await;
        let updated =
            db::update_pending_subscribe_desired(&conn, &key, url, None, Some("B分类"), None)
                .unwrap();
        assert!(updated, "前置：原地更新必须命中 pending subscribe 行");
    }

    barrier.release.send(()).unwrap();
    let _ = handle
        .await
        .expect("join feeds_phase")
        .expect("feeds phase (mount window)");
    {
        let form = last_subscription_edit_form(&server);
        assert!(
            form_has(&form, "a", "user/-/label/A分类"),
            "本轮 HTTP 已按发送时目标发出（A），实际 {form:?}"
        );
        let conn = dbm.lock().await;
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(
            items.len(),
            1,
            "发送后目标已被更新：旧成功不得确认（prune）未发送的新目标"
        );
        assert_eq!(items[0].desired_folder.as_deref(), Some("B分类"));
    }

    // 下一轮发送最新目标并出队（不重复 quick_add）
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (latest target)");
    {
        let form = last_subscription_edit_form(&server);
        assert!(
            form_has(&form, "a", "user/-/label/B分类"),
            "下一轮必须发送最新目标，实际 {form:?}"
        );
        let conn = dbm.lock().await;
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "最新目标确认后精确出队"
        );
    }
    assert_eq!(
        quickadd_requests(&server),
        1,
        "重试不得重复发送 quick_add（行已 remote_created；request-level）"
    );
    assert_eq!(
        quickadd_count(&server, url),
        1,
        "远端只创建一次（created-count）"
    );
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
8b. 挂载响应在途 + 生产命令编辑：绑定完成后的编辑走 edit 行，旧挂载成功不得清掉它
============================================================ */

/// OPT-009 阶段 B1-R3（复审 P3-5）：上一条 `mount_window_*` 直接调 db 原语，
/// 只证明 SQL 的判别力；本用例走**生产命令路径**（record_feed_edit，与 Tauri
/// `update_feed` 同一实现）+ loopback 响应屏障，验证真实接口语义与队列最终状态：
/// - 挂载 HTTP 在途期间（绑定已写入）用户移动分类 → 生产路径生成 **edit 行**
///   （订阅行被替换，新 id），携带实际远端旧 label；
/// - 旧挂载成功按订阅 id 确认必须落空：在途新意图不得被清掉；
/// - 下一轮 edit 行以真实移动语义推达（a=新目标 + r=旧 label），远端只剩新分类。
#[tokio::test]
async fn mount_window_production_edit_survives_old_mount_success() {
    let server = MockGReader::start().await.expect("start mock");
    let tmp = common::unique_db_path("opt009b_mount_prod");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = format!("{}/opt009-mount-prod.xml", server.url());
    let key = acct_key("greader", &server, "test");

    // 远端已有该订阅（quick_add 幂等返回既有 id；挂载/移动作用于真实远端成员）
    server
        .existing_feed_urls
        .lock()
        .unwrap()
        .push((url.clone(), 10));
    *server.subscriptions.lock().unwrap() = vec![MockSubscription {
        id: "feed/10".into(),
        title: "挂载窗口（生产路径）".into(),
        url: url.clone(),
        html_url: None,
        categories: vec![("A分类".into(), "folder".into())],
    }];

    let (dbm, feed_id, folder_b) = {
        let conn = db::open(&tmp).unwrap();
        db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
        db::set_setting(&conn, "greader_username", "test").unwrap();
        db::set_setting(&conn, "greader_password", "test-token").unwrap();
        let folder_a = db::create_folder(&conn, "A分类", "article").unwrap();
        let folder_b = db::create_folder(&conn, "B分类", "article").unwrap();
        let feed = db::insert_feed(
            &conn,
            &url,
            None,
            "挂载窗口（生产路径）",
            None,
            folder_a,
            "inherit",
            true,
            false,
        )
        .unwrap();
        let gen = db::sync_generation(&conn).unwrap();
        db::enqueue_subscribe(&conn, gen, &key, &url, None, Some("A分类")).unwrap();
        (Arc::new(Mutex::new(conn)), feed, folder_b)
    };

    // 第一轮：挂载（A分类）响应在途
    let barrier = server.arm_subscription_edit_barrier("feed/");
    let db2 = dbm.clone();
    let http2 = http.clone();
    let handle = tokio::spawn(async move { sync::feeds_phase(&db2, &http2).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), barrier.arrived)
        .await
        .expect("执行器必须在期限内发出挂载（网络屏障）")
        .expect("barrier arrived");

    // 在途期间：生产命令移动 A→B（此时绑定已完成 → 远端 id 可见 → edit 行）
    {
        let conn = dbm.lock().await;
        let push = app_lib::commands::record_feed_edit(
            &conn,
            feed_id,
            None,
            Some(folder_b),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(push.is_some(), "绑定已完成：生产编辑必须携带远端推送目标");
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1, "同 URL 只保留最新意图");
        assert_eq!(
            items[0].action, "edit",
            "绑定完成后的编辑走 edit 行（订阅行被替换）: {items:?}"
        );
        assert_eq!(items[0].desired_folder.as_deref(), Some("B分类"));
        assert_eq!(
            items[0].prev_folder.as_deref(),
            Some("A分类"),
            "移动必须携带远端实际持有的旧 label"
        );
    }

    barrier.release.send(()).unwrap();
    let _ = handle
        .await
        .expect("join feeds_phase")
        .expect("feeds phase (mount window, production edit)");
    {
        let form = last_subscription_edit_form(&server);
        assert!(
            form_has(&form, "a", "user/-/label/A分类"),
            "本轮 HTTP 按发送时目标发出（A），实际 {form:?}"
        );
        let conn = dbm.lock().await;
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(
            items.len(),
            1,
            "旧挂载成功不得清掉在途新意图（接口语义：按订阅 id 精确确认）: {items:?}"
        );
        assert_eq!(items[0].action, "edit");
        assert_eq!(items[0].desired_folder.as_deref(), Some("B分类"));
        let bound: Option<i64> = conn
            .query_row(
                "SELECT remote_id FROM feeds WHERE id = ?1",
                [feed_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bound, Some(10), "绑定在本轮已完成");
    }

    // 下一轮：edit 行以真实移动语义推达
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (latest move)");
    {
        let form = last_subscription_edit_form(&server);
        assert!(
            form_has(&form, "a", "user/-/label/B分类"),
            "下一轮必须发送最新目标，实际 {form:?}"
        );
        let labels: Vec<String> = server
            .subscriptions
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.id == "feed/10")
            .expect("远端订阅仍在")
            .categories
            .iter()
            .map(|(l, _)| l.clone())
            .collect();
        assert!(
            labels.contains(&"B分类".to_string()) && !labels.contains(&"A分类".to_string()),
            "远端成员必须只剩新分类（真实移动语义）: {labels:?}"
        );
        let conn = dbm.lock().await;
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "最新目标确认后精确出队"
        );
    }
    assert_eq!(
        quickadd_requests(&server),
        0,
        "远端已持有该订阅：列表证据直接解析，全程零 quick_add"
    );
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
6. Fever：订阅写回不支持 → local_only 终态，不向远端发写请求
============================================================ */
#[tokio::test]
async fn fever_subscription_intents_are_local_only_without_remote_writes() {
    let server = MockGReader::start().await.expect("start mock");
    server.subscriptions.lock().unwrap().clear();
    let tmp = common::unique_db_path("opt009b_fever");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = "http://127.0.0.1:1/opt009-fever.xml";
    let key = acct_key("fever", &server, "test");

    let dbm = {
        let conn = open_with_credentials(&server, "fever", "test");
        let folder = db::create_folder(&conn, "分类", "article").unwrap();
        db::insert_feed(
            &conn,
            url,
            None,
            "Fever Feed",
            None,
            folder,
            "inherit",
            true,
            false,
        )
        .unwrap();
        let gen = db::sync_generation(&conn).unwrap();
        db::enqueue_subscribe(&conn, gen, &key, url, None, Some("分类")).unwrap();
        Arc::new(Mutex::new(conn))
    };

    server.clear_request_log();
    sync::feeds_phase(&dbm, &http).await.expect("feeds phase");

    {
        let conn = dbm.lock().await;
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "Fever 不支持写订阅：不得以 pending 冒充可重试的成功路径"
        );
        // OPT-009 B3：统计改为按当前账号限定（本用例的队项用的是 Fever 账号 key）
        let stats = db::subscription_outbox_stats(&conn, &key).unwrap();
        assert_eq!(
            (stats.pending, stats.local_only),
            (0, 1),
            "必须落 local_only 终态（保留展示、不重试、不冒充远端完成）"
        );
    }
    let log = server.request_log();
    assert!(
        !log.iter().any(|r| r.contains("subscription/quickadd")),
        "Fever 不得发出 quick_add（无写订阅端点）: {log:?}"
    );
    assert!(
        !log.iter().any(|r| r.contains("subscription/edit")),
        "Fever 不得发出 subscription/edit: {log:?}"
    );

    // 再跑一轮：终态不得被重试、不得发请求
    server.clear_request_log();
    sync::feeds_phase(&dbm, &http).await.expect("feeds phase 2");
    let log2 = server.request_log();
    assert!(
        !log2.iter().any(|r| r.contains("subscription/")),
        "local_only 是终态，不得下轮重试: {log2:?}"
    );
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
R2-1. 全局分类墓碑：单个账号的 pull 不得回收（生产 pull 跨 A/B 回归）
============================================================ */

/// OPT-009B2 复审 R2-1（P2）：空 `account_key` 的墓碑是**全局**墓碑（纯本地写入
/// 或 v24 迁移时无账号；按契约对任一同步账号生效）。一个账号的远端缺席不能证明
/// 其它/未来账号都缺席——单个账号的 pull 不得回收全局墓碑（回收只限归属该账号
/// 且远端确认缺席的条目），否则 A 的一次 pull 会让 B 之后把同名分类导入复活。
/// 判别力（旧实现：空 key 视同当前账号的活动墓碑）：A 的 pull 后全局墓碑消失、
/// B 的 pull 把「全局已删分类」按远端 label 导入——下面两条断言必红。
#[tokio::test]
async fn global_folder_tombstone_survives_single_account_pull_and_suppresses_other_account() {
    let server = MockGReader::start().await.expect("start mock");
    let tmp = common::unique_db_path("opt009b_r2_global_tomb");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);

    let conn = open_with_credentials(&server, "greader", "user-a");
    // 纯本地现场写入的全局墓碑（无账号归属：本地删除意图不得因之后连接账号而失效）
    db::add_folder_tombstone(&conn, "", "全局已删分类").unwrap();
    // 对照面：A 自己的墓碑（远端不含 → 归属账号的 pull 允许按既有判据回收）
    let key_a = acct_key("greader", &server, "user-a");
    db::add_folder_tombstone(&conn, &key_a, "A专属已删分类").unwrap();
    let dbm = Arc::new(Mutex::new(conn));

    // 远端 tag/list 不含这两个 label（mock 默认 folders 只有 Default / Remote Cat）
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (A, labels absent)");
    {
        let conn = dbm.lock().await;
        let ts = db::folder_tombstones(&conn).unwrap();
        assert!(
            ts.iter()
                .any(|t| t.label == "全局已删分类" && t.account_key.is_empty()),
            "单账号 pull 不得回收全局（空 key）墓碑: {ts:?}"
        );
        assert!(
            !ts.iter().any(|t| t.label == "A专属已删分类"),
            "账号专属墓碑：远端确认缺席后按归属回收（既有行为保持）: {ts:?}"
        );
    }

    // 切到 B：B 的远端**列出**同名「全局已删分类」→ 全局墓碑必须继续压制导入，
    // 且 B 的 pull 不得回收它（契约：对所有账号生效，直到显式解除）。
    {
        let mut conn = dbm.lock().await;
        app_lib::commands::save_account_for_test(
            &mut conn,
            "greader",
            &server.url(),
            "user-b",
            "pw-b",
            &server.url(),
        )
        .expect("switch account");
    }
    server.folders.lock().unwrap().push("全局已删分类".into());
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (B)");
    {
        let conn = dbm.lock().await;
        let imported: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM folders WHERE name = '全局已删分类'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            imported, 0,
            "全局墓碑对任何账号生效：B 不得按远端 label 导入该分类（旧实现此处为 1）"
        );
        let ts = db::folder_tombstones(&conn).unwrap();
        assert!(
            ts.iter()
                .any(|t| t.label == "全局已删分类" && t.account_key.is_empty()),
            "B 的 pull 同样不得回收全局墓碑: {ts:?}"
        );
    }
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
R2-2. 未知 id 退订：列表发现失败不得确认；解析目标持久化；恢复后收敛
============================================================ */

/// OPT-009B2 复审 R2-2（P2，B1 契约）：`remote_id` 未知的退订意图（删除撞上在途
/// quick_add）在**订阅列表读取失败**时不得发送任何退订、也不得确认出队——列表是
/// 唯一可靠证据源（真实服务端拒绝重复 URL，恢复路径零 quick_add）。修复后的契约：
/// 1. 列表读失败 = 重复目标发现未完成 → 保留 pending 队项、本轮零发送；
/// 2. 重启（重开同一文件库）后同样只靠列表证据，等待期不发任何写请求；
/// 3. 列表恢复 → 按列表证据退订全部同 URL id，全部 2xx 才精确出队。
/// 判别力（旧实现：列表失败仍发解析退订并确认）：第一轮若有任何退订请求即红。
#[tokio::test]
async fn unsubscribe_recovery_list_failure_keeps_intent_without_sending() {
    let server = MockGReader::start().await.expect("start mock");
    let tmp = common::unique_db_path("opt009b_r2_unsub_list");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = format!("{}/opt009-r2-unsub.xml", server.url());
    let key = acct_key("greader", &server, "test");

    // 远端已有一条同 URL 订阅（删除撞上在途 quick_add 的历史现场）
    *server.subscriptions.lock().unwrap() = vec![MockSubscription {
        id: "feed/40".into(),
        title: "Existing Twin".into(),
        url: url.clone(),
        html_url: None,
        categories: vec![("原分类".into(), "folder".into())],
    }];
    // 列表暂时不可用：重复目标发现失败
    server.set_fail_subscription_list(true);

    let (dbm, feed_id) = {
        let conn = db::open(&tmp).unwrap();
        db::set_setting(&conn, "sync_protocol", "greader").unwrap();
        db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
        db::set_setting(&conn, "greader_username", "test").unwrap();
        db::set_setting(&conn, "greader_password", "test-token").unwrap();
        let folder = db::create_folder(&conn, "分类", "article").unwrap();
        let feed = db::insert_feed(
            &conn,
            &url,
            None,
            "Unsub Recover",
            None,
            folder,
            "inherit",
            true,
            false,
        )
        .unwrap();
        let gen = db::sync_generation(&conn).unwrap();
        // 创建证据：同账号同 URL 的 pending subscribe（quick_add 已发出、响应丢失）
        db::enqueue_subscribe(&conn, gen, &key, &url, None, Some("分类")).unwrap();
        (Arc::new(Mutex::new(conn)), feed)
    };
    {
        let conn = dbm.lock().await;
        app_lib::commands::record_feed_deletion(&conn, feed_id).expect("record deletion");
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1, "删除必须落可恢复退订意图: {items:?}");
        assert_eq!(items[0].action, "unsubscribe");
        assert_eq!(items[0].remote_id, None, "id 此刻不可知（响应丢失）");
    }

    // 第一轮：列表失败 = 重复目标发现未完成 → 队项保留、本轮零发送。
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (list down)");
    {
        let conn = dbm.lock().await;
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(
            items.len(),
            1,
            "列表失败 = 重复目标发现未完成：队项必须保留（旧实现此处为空 → 意图丢失）: {items:?}"
        );
        assert_eq!(items[0].action, "unsubscribe");
        assert_eq!(
            items[0].remote_id, None,
            "解析结果写独立持久字段，不冒充「入队时已知的远端绑定」"
        );
        assert_eq!(
            items[0].resolved_remote_id, None,
            "列表失败时无可解析目标（零 quick_add，不得用服务端副作用伪造 id）: {items:?}"
        );
        assert!(items[0].attempts >= 1, "失败必须累计 attempts");
        assert!(
            items[0]
                .last_error
                .as_deref()
                .is_some_and(|e| e.contains("重复目标发现未完成")),
            "失败摘要必须说明发现未完成: {items:?}"
        );
        assert!(
            db::feed_tombstones(&conn)
                .unwrap()
                .iter()
                .any(|u| u.contains("opt009-r2-unsub")),
            "墓碑继续挡复活"
        );
    }
    assert!(
        mock_greader::subscription_edit_actions(&server)
            .iter()
            .all(|(ac, _)| ac != "unsubscribe"),
        "列表失败时不得发送任何半套退订"
    );
    assert!(
        server
            .subscriptions
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.id == "feed/40"),
        "原订阅必须原样（旧实现只退新 id 便确认，此处已漏删）"
    );
    assert_eq!(
        quickadd_requests(&server),
        0,
        "恢复路径零 quick_add：列表是唯一证据源（真实服务端拒绝重复 URL）"
    );

    // 第二轮：重启（丢连接重开同一文件库）+ 列表仍不可用 → 仍然零发送、意图原样。
    drop(dbm);
    let dbm = Arc::new(Mutex::new(db::open(&tmp).unwrap()));
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (restart, list still down)");
    {
        let conn = dbm.lock().await;
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1, "重启后意图原样");
        assert_eq!(items[0].resolved_remote_id, None);
    }
    assert_eq!(
        quickadd_requests(&server),
        0,
        "列表长期失败也不得改用 quick_add 试探"
    );
    assert!(
        mock_greader::subscription_edit_actions(&server)
            .iter()
            .all(|(ac, _)| ac != "unsubscribe"),
        "重启轮同样零发送"
    );

    // 第三轮：列表恢复 → 按列表证据退订同 URL 全部 id（feed/40），精确出队；
    // 墓碑在 pull 确认后回收，本地不复活。
    server.set_fail_subscription_list(false);
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (list recovered)");
    {
        let actions = mock_greader::subscription_edit_actions(&server);
        assert!(
            actions
                .iter()
                .any(|(ac, s)| ac == "unsubscribe" && s == "feed/40"),
            "必须按列表证据退订 feed/40: {actions:?}"
        );
        assert!(
            server
                .subscriptions
                .lock()
                .unwrap()
                .iter()
                .all(|s| s.url != url),
            "远端不得残留任何同 URL 订阅"
        );
        let conn = dbm.lock().await;
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "全部退订 2xx 后才精确出队"
        );
        assert!(
            db::feed_tombstones(&conn).unwrap().is_empty(),
            "pull 确认远端已不含该 URL 后墓碑才回收"
        );
        let revived: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM feeds WHERE active=1 AND feed_url = ?1",
                [&url],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(revived, 0, "收敛后仍不得复活");
    }
    assert_eq!(quickadd_requests(&server), 0, "恢复全程零 quick_add");
    let _ = std::fs::remove_file(&tmp);
}

/// OPT-009B2 复审 R2-2（P2，账号边界；B1 契约）+ 2026-11-02 生命周期终点：
/// A 的列表证据解析落库后换到 B——换号清账，已持久化的解析结果随行消亡；
/// B 的轮次零订阅写请求；切回 A 后零退订、远端孤儿如实保留，全程零 quick_add。
#[tokio::test]
async fn account_switch_clears_unsubscribe_resolution_and_orphan_persists() {
    let server = MockGReader::start().await.expect("start mock");
    let tmp = common::unique_db_path("opt009b_r2_unsub_switch");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = format!("{}/opt009-r2-switch.xml", server.url());
    let key_a = acct_key("greader", &server, "user-a");

    // A 的远端已有该订阅（列表证据会解析出这个 id）
    *server.subscriptions.lock().unwrap() = vec![MockSubscription {
        id: "feed/40".into(),
        title: "Scoped".into(),
        url: url.clone(),
        html_url: None,
        categories: vec![],
    }];
    server
        .existing_feed_urls
        .lock()
        .unwrap()
        .push((url.clone(), 40));
    // 第一轮让退订发送失败（订阅编辑注入 500）：解析已持久化、意图保留
    server.set_fail_subscription_edit(true);

    let conn = open_with_credentials(&server, "greader", "user-a");
    let feed_id = {
        let folder = db::create_folder(&conn, "分类", "article").unwrap();
        let feed = db::insert_feed(
            &conn, &url, None, "Scoped", None, folder, "inherit", true, false,
        )
        .unwrap();
        let gen = db::sync_generation(&conn).unwrap();
        db::enqueue_subscribe(&conn, gen, &key_a, &url, None, Some("分类")).unwrap();
        feed
    };
    let dbm = Arc::new(Mutex::new(conn));
    {
        let conn = dbm.lock().await;
        app_lib::commands::record_feed_deletion(&conn, feed_id).expect("record deletion");
    }

    // A 的第一轮：列表证据解析 feed/40 并持久化；退订发送被注入拒绝 → 保留意图
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (A, list down)");
    {
        let conn = dbm.lock().await;
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].account_key, key_a);
        assert_eq!(
            items[0].resolved_remote_id,
            Some(40),
            "A 的解析结果必须持久化: {items:?}"
        );
    }
    assert_eq!(
        quickadd_requests(&server),
        0,
        "恢复解析走列表证据，零 quick_add"
    );

    // 换到 B：换号清账（A 的行与已持久化解析结果一并消亡），B 的轮次零订阅写请求
    {
        let mut conn = dbm.lock().await;
        app_lib::commands::save_account_for_test(
            &mut conn,
            "greader",
            &server.url(),
            "user-b",
            "pw-b",
            &server.url(),
        )
        .expect("switch to B");
    }
    server.clear_request_log();
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (B)");
    {
        let log = server.request_log();
        assert!(
            !log.iter()
                .any(|r| r.contains("subscription/edit") || r.contains("subscription/quickadd")),
            "B 的轮次不得发出 A 的订阅写请求（list 读取是 pull 正常行为）: {log:?}"
        );
        let conn = dbm.lock().await;
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM subscription_outbox", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 0, "换号清账：已持久化的解析结果不跨换号存活");
    }

    // 切回 A、解除注入 → 零退订、远端孤儿保留；全程零 quick_add
    {
        let mut conn = dbm.lock().await;
        app_lib::commands::save_account_for_test(
            &mut conn,
            "greader",
            &server.url(),
            "user-a",
            "pw-a",
            &server.url(),
        )
        .expect("switch back to A");
    }
    server.set_fail_subscription_edit(false);
    server.clear_request_log();
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (back to A)");
    {
        assert!(
            mock_greader::subscription_edit_actions(&server)
                .iter()
                .all(|(ac, _)| ac != "unsubscribe"),
            "清账后没有任何退订意图可执行（远端孤儿由用户手动清理）"
        );
        assert!(
            server
                .subscriptions
                .lock()
                .unwrap()
                .iter()
                .any(|s| s.id == "feed/40"),
            "语义放弃的代价：远端孤儿订阅如实保留（本用例锁定该形态）"
        );
        let conn = dbm.lock().await;
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "A 回场后也无队项（清账即生命周期终点）"
        );
    }
    assert_eq!(
        quickadd_requests(&server),
        0,
        "全程零 quick_add（无新意图可重发）"
    );
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
R2-3. 移动确认出队后删除再快速重加：远端旧 label 必须持久结转
============================================================ */

/// OPT-009B2 复审 R2-3（P3，先证时序）：移动已成功并确认出队后删除源、且在
/// unsubscribe **尚未被服务端确认**时快速重加同 URL——`enqueue_subscribe` 会替换
/// 退订意图；若原订阅仍在远端，新 quick_add 返回旧 ID，挂载只 `a=` 会永久残留
/// 旧 label。修后：删除时把本地当前分类 label 作为**远端移除基准**快照进退订行；
/// 重加替换退订行时持久继承 → 挂载发 `a=目标 + r=旧`，其它 label 保留。
/// 判别力（旧实现：重加行 prev 为空、挂载只 a=）：重加行的 prev 断言与远端
/// 「B分类 必须消失」断言必红。
#[tokio::test]
async fn readd_after_confirmed_move_and_delete_carries_remote_old_label() {
    let server = MockGReader::start().await.expect("start mock");
    let tmp = common::unique_db_path("opt009b_r2_readd");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = format!("{}/opt009-r2-readd.xml", server.url());

    *server.subscriptions.lock().unwrap() = vec![MockSubscription {
        id: "feed/10".into(),
        title: "Readd After Confirm".into(),
        url: url.clone(),
        html_url: None,
        categories: vec![
            ("A分类".into(), "folder".into()),
            ("保留分类".into(), "folder".into()),
        ],
    }];
    server
        .existing_feed_urls
        .lock()
        .unwrap()
        .push((url.clone(), 10));

    let (dbm, feed_id) = {
        let conn = open_with_credentials(&server, "greader", "test");
        let (_folder_a, feed) = seed_bound_feed(&conn, &url, "A分类", 10);
        db::create_folder(&conn, "B分类", "article").unwrap();
        db::create_folder(&conn, "新分类", "article").unwrap();
        (Arc::new(Mutex::new(conn)), feed)
    };

    // ① 移动 A→B（生产命令 + 执行器）：成功并确认出队——本时序的前提
    {
        let conn = dbm.lock().await;
        let folder_b = db::list_folders(&conn)
            .unwrap()
            .into_iter()
            .find(|f| f.name == "B分类")
            .expect("B分类存在")
            .id;
        app_lib::commands::record_feed_edit(
            &conn,
            feed_id,
            None,
            Some(folder_b),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
    }
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (move)");
    {
        let conn = dbm.lock().await;
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "时序前提：移动必须已确认出队"
        );
    }
    {
        let labels: Vec<String> = server
            .subscriptions
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.id == "feed/10")
            .expect("远端订阅仍在")
            .categories
            .iter()
            .map(|(l, _)| l.clone())
            .collect();
        assert!(
            labels.contains(&"B分类".to_string()) && !labels.contains(&"A分类".to_string()),
            "时序前提：远端已接受 B: {labels:?}"
        );
    }

    // ② 删除（unsubscribe 仍在 pending：不跑执行器）——必须快照远端当前 label
    {
        let conn = dbm.lock().await;
        app_lib::commands::record_feed_deletion(&conn, feed_id).expect("record deletion");
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1, "退订意图已落: {items:?}");
        assert_eq!(items[0].action, "unsubscribe");
        assert_eq!(
            items[0].prev_folder.as_deref(),
            Some("B分类"),
            "删除必须快照「远端当前 label」的移除基准（重加后继需要它）: {items:?}"
        );
    }

    // ③ 快速重加：替换 pending 退订行 → 必须持久继承移除基准
    {
        let conn = dbm.lock().await;
        let report = app_lib::commands::opml_import_for_test(
            &conn,
            &opml_doc(&[(url.as_str(), "重加标题", Some("新分类"))]),
        )
        .expect("re-add");
        assert_eq!((report.imported, report.skipped), (1, 0));
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1, "同 URL 只留最新意图: {items:?}");
        assert_eq!(items[0].action, "subscribe");
        assert_eq!(
            items[0].prev_folder.as_deref(),
            Some("B分类"),
            "重加行必须继承被替换退订行的移除基准（否则挂载只 a= 残留 B）: {items:?}"
        );
    }

    // ④ 执行器：quick_add 命中既有远端订阅（id 10）→ 挂载 a=新分类 + r=B分类
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (readd)");
    {
        let form = last_subscription_edit_form(&server);
        assert!(
            form_has(&form, "a", "user/-/label/新分类")
                && form_has(&form, "r", "user/-/label/B分类"),
            "重加挂载必须 a=新 + r=B（移除远端已接受的旧 label），实际 {form:?}"
        );
        let labels: Vec<String> = server
            .subscriptions
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.id == "feed/10")
            .expect("远端订阅仍在")
            .categories
            .iter()
            .map(|(l, _)| l.clone())
            .collect();
        assert!(
            labels.contains(&"新分类".to_string()),
            "远端必须加入重加的目标分类: {labels:?}"
        );
        assert!(
            !labels.contains(&"B分类".to_string()),
            "远端旧 label（已接受移动的中间值）必须被移除: {labels:?}"
        );
        assert!(
            !labels.contains(&"A分类".to_string()),
            "更早的链起点也不得复活: {labels:?}"
        );
        assert!(
            labels.contains(&"保留分类".to_string()),
            "无关 label 必须保留: {labels:?}"
        );
        let conn = dbm.lock().await;
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "挂载确认后精确出队"
        );
        let bound: Option<i64> = conn
            .query_row(
                "SELECT remote_id FROM feeds WHERE feed_url = ?1",
                [&url],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bound, Some(10), "重加源绑定既有远端订阅（幂等）");
    }
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
结转退订事务分支（2026-11-01 事务补口的直达 e2e）：subscribe 行已
remote_created、本地 feed 消失、无替代行 → DELETE pending（同账号同 URL
subscribe 行）/ DELETE add_feed（兼容队列行）/ INSERT 退订意图三写同事务。
此前既有用例只覆盖相邻的 adopt 结转分支（响应在途形态），本节直达
exec_subscribe_item 的 Ok(None) 结转路径。
============================================================ */

/// 直达分支夹具：remote_created 订阅行（远端 id 夹具给定 42，执行器不再发
/// quick_add）+ 同 URL 旧 add_feed 兼容队行（DELETE add_feed 的观察对象）+
/// 本地无该 feed（feed_id_by_url_normalized → None 的结转路径）。
/// `fail_subscription_list` 置位让 push_feeds 的旧队列段按「列表不可用」保留
/// add_feed 行（不 quickadd、不建本地源、不剪除），排除其对夹具的干扰。
async fn setup_carryover_fixture(
    url: &str,
) -> (
    Arc<Mutex<rusqlite::Connection>>,
    Arc<MockGReader>,
    reqwest::Client,
    String,
) {
    let server = MockGReader::start().await.expect("start mock");
    server.set_fail_subscription_list(true);
    let key = acct_key("greader", &server, "test");
    let conn = open_with_credentials(&server, "greader", "test");
    let gen = db::sync_generation(&conn).unwrap();
    conn.execute(
        "INSERT INTO subscription_outbox
            (account_generation, account_key, action, feed_url, remote_id, stage, status)
         VALUES (?1, ?2, 'subscribe', ?3, 42, 'remote_created', 'pending')",
        rusqlite::params![gen, key, url],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sync_queue (feed_url, action, payload) VALUES (?1, 'add_feed', ?2)",
        rusqlite::params![url, r#"{"folder_id": 1}"#],
    )
    .unwrap();
    let http = app_lib::ingestion::build_client(10);
    (Arc::new(Mutex::new(conn)), server, http, key)
}

/// 注意：本 helper 是**同步**函数、只消费调用方已持有的 `&Connection`——
/// 2026-11-02 CI 根因修复：曾作 async helper 内部再 `db.lock().await`，
/// 被两个结转用例在**已持锁的断言临界区**里调用，tokio Mutex 不可重入，
/// 同任务自我死锁（CI Run tests 挂死/硬超时的真凶）。
fn add_feed_queue_count(conn: &rusqlite::Connection, url: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM sync_queue WHERE action = 'add_feed' AND feed_url = ?1",
        [url],
        |r| r.get(0),
    )
    .unwrap()
}

/// 成功路径：结转退订三写原子提交——subscribe 行被退订意图替换（携带结转
/// rid=42）、旧 add_feed 兼容队项随同删除；remote_created 行不得再发 quick_add。
/// 用例硬超时（2026-11-02 CI 加固）：结转用例全链路是毫秒级回环请求 +
/// build_client(10) 单请求超时；120s 只可能在「某段等待永不满足」时触发。
/// 挂起形态在此转为带诊断的红色失败，Run tests 步骤不再可能被本套件永久占住。
const CARRYOVER_TEST_HARD_LIMIT: std::time::Duration = std::time::Duration::from_secs(120);

async fn run_carryover_bounded(name: &str, case: impl std::future::Future<Output = ()>) {
    if tokio::time::timeout(CARRYOVER_TEST_HARD_LIMIT, case)
        .await
        .is_err()
    {
        panic!(
            "opt009 结转退订用例 {name} 硬超时（{CARRYOVER_TEST_HARD_LIMIT:?}）：存在未在时限内收敛的等待"
        );
    }
}

#[tokio::test]
async fn carryover_unsubscribe_commits_all_three_writes_with_rid() {
    run_carryover_bounded(
        "commits_all_three_writes_with_rid",
        carryover_unsubscribe_commits_all_three_writes_with_rid_case(),
    )
    .await;
}

async fn carryover_unsubscribe_commits_all_three_writes_with_rid_case() {
    let url = "http://127.0.0.1:1/opt009-r12-carry.xml";
    let (db, server, http, key) = setup_carryover_fixture(url).await;

    sync::feeds_phase(&db, &http).await.expect("feeds phase");
    {
        let conn = db.lock().await;
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1, "只应剩退订意图: {items:?}");
        assert_eq!(
            items[0].action, "unsubscribe",
            "subscribe 行必须被退订意图替换（同账号同 URL DELETE）"
        );
        assert_eq!(
            items[0].remote_id,
            Some(42),
            "退订意图必须携带结转的远端 id: {items:?}"
        );
        assert_eq!(items[0].feed_url, url);
        assert_eq!(items[0].account_key, key, "退订意图必须归属同一账号");
        assert_eq!(
            add_feed_queue_count(&conn, url),
            0,
            "结转退订必须清除同 URL 旧 add_feed 队项（同事务 DELETE）"
        );
        assert_eq!(
            quickadd_count(&server, url),
            0,
            "remote_created 行已携带远端 id，不得再发 quick_add"
        );
    }
}

/// 原子性：INSERT 退订失败（真实 SQLite RAISE(ABORT) 注入）→ 三写整体回滚——
/// subscribe 行保全（stage/remote_id 原样）、被 DELETE 的旧 add_feed 队项恢复、
/// 无退订意图残留；解除注入后重试收敛（同一事务语义的正反两面）。
#[tokio::test]
async fn carryover_unsubscribe_insert_failure_rolls_back_all_three_writes() {
    run_carryover_bounded(
        "insert_failure_rolls_back_all_three_writes",
        carryover_unsubscribe_insert_failure_rolls_back_all_three_writes_case(),
    )
    .await;
}

async fn carryover_unsubscribe_insert_failure_rolls_back_all_three_writes_case() {
    let url = "http://127.0.0.1:1/opt009-r12-rollback.xml";
    let (db, _server, http, _key) = setup_carryover_fixture(url).await;

    {
        let conn = db.lock().await;
        conn.execute(
            "CREATE TRIGGER fail_carryover_unsubscribe BEFORE INSERT ON subscription_outbox
             WHEN NEW.action = 'unsubscribe'
             BEGIN SELECT RAISE(ABORT, 'injected unsubscribe insert failure'); END;",
            [],
        )
        .unwrap();
    }

    let _report = sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase (trigger 注入下仍须正常返回：结转失败是软失败)");
    {
        let conn = db.lock().await;
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1, "回滚后只应剩原 subscribe 行: {items:?}");
        assert_eq!(
            items[0].action, "subscribe",
            "INSERT 失败必须回滚：退订意图不得出现"
        );
        assert_eq!(items[0].stage, "remote_created", "回滚后行阶段原样");
        assert_eq!(items[0].remote_id, Some(42), "回滚后行远端 id 原样");
        assert_eq!(
            add_feed_queue_count(&conn, url),
            1,
            "回滚必须恢复被 DELETE 的旧 add_feed 队项"
        );
        conn.execute("DROP TRIGGER fail_carryover_unsubscribe", [])
            .unwrap();
    }

    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase (retry)");
    {
        let conn = db.lock().await;
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1, "重试后只应剩退订意图: {items:?}");
        assert_eq!(items[0].action, "unsubscribe");
        assert_eq!(items[0].remote_id, Some(42), "重试结转的远端 id 不变");
        assert_eq!(
            add_feed_queue_count(&conn, url),
            0,
            "解除注入后三写原子提交"
        );
    }
}

/* ============================================================
8c. 挂载成功 + 在途生产编辑：替代行移除基准重定为已接受目标（A-P2-3①+②）
============================================================ */

/// A-P2-3①+② 联动回归。场景：订阅行经未绑定窗口移动折叠为
/// (desired=B, prev=A)（链起点 A），绑定后挂载（a=B, r=A）响应在途期间
/// 生产命令移动 B→C → 生成替代 edit 行。
/// - ①替代行必须保留链起点 A（挂载尚未成功，不能被调用方 prev=B 覆盖）；
/// - ②挂载成功的 2xx =「远端当下持有 B」的账号作用域事实，④b 必须把替代行
///   的移除基准重定为 B（修前 sent_moves 只收 edit 行，替代行按链起点 A 发
///   r= 是 no-op，远端残留 B）。
/// 判别力：修前②时远端最终为 {B, C}；修后远端只剩 C。
/// 前置：远端**已有**该订阅（feed/10，无分类）——mock 替身的 quickadd 创建
/// 不进入订阅列表/模型，需要断言远端 label 效果的用例必须显式种子（与
/// `mount_window_production_edit_*` 同款前置），否则 edit 对模型是 no-op。
#[tokio::test]
async fn mount_window_success_rebases_replacement_removal_basis() {
    let server = MockGReader::start().await.expect("start mock");
    let tmp = common::unique_db_path("opt009b_mount_rebase");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = format!("{}/opt009-mount-rebase.xml", server.url());
    let key = acct_key("greader", &server, "test");
    // 远端已有该订阅（feed/10，无分类）：mock 替身的 quickadd 创建不进入订阅
    // 列表/模型（既有契约——需要断言远端 label 效果的用例一律显式种子，见
    // mount_window_production_edit_* 同款前置），挂载/移动的真实远端效果由此可断言。
    server
        .existing_feed_urls
        .lock()
        .unwrap()
        .push((url.clone(), 10));
    *server.subscriptions.lock().unwrap() = vec![MockSubscription {
        id: "feed/10".into(),
        title: "挂载重定".into(),
        url: url.clone(),
        html_url: None,
        categories: vec![],
    }];

    let (dbm, feed_id, folder_c) = {
        let conn = db::open(&tmp).unwrap();
        db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
        db::set_setting(&conn, "greader_username", "test").unwrap();
        db::set_setting(&conn, "greader_password", "test-token").unwrap();
        let folder_a = db::create_folder(&conn, "A分类", "article").unwrap();
        let folder_b = db::create_folder(&conn, "B分类", "article").unwrap();
        let folder_c = db::create_folder(&conn, "C分类", "article").unwrap();
        let feed = db::insert_feed(
            &conn,
            &url,
            None,
            "挂载重定",
            None,
            folder_a,
            "inherit",
            true,
            false,
        )
        .unwrap();
        let gen = db::sync_generation(&conn).unwrap();
        db::enqueue_subscribe(&conn, gen, &key, &url, None, Some("A分类")).unwrap();
        // 未绑定窗口移动 A→B：折叠为 (desired=B, prev=A)（链起点首值）
        let push = app_lib::commands::record_feed_edit(
            &conn,
            feed,
            None,
            Some(folder_b),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(push.is_none(), "未绑定窗口：编辑并入订阅行，无直接推送目标");
        {
            let items = db::pending_outbox(&conn).unwrap();
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].action, "subscribe");
            assert_eq!(items[0].desired_folder.as_deref(), Some("B分类"));
            assert_eq!(
                items[0].prev_folder.as_deref(),
                Some("A分类"),
                "前置：未绑定窗口移动的链起点"
            );
        }
        (Arc::new(Mutex::new(conn)), feed, folder_c)
    };

    // 第一轮：quick_add → 绑定 → 挂载（a=B, r=A）响应在途
    let barrier = server.arm_subscription_edit_barrier("feed/");
    let db2 = dbm.clone();
    let http2 = http.clone();
    let handle = tokio::spawn(async move { sync::feeds_phase(&db2, &http2).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), barrier.arrived)
        .await
        .expect("执行器必须在期限内发出挂载（网络屏障）")
        .expect("barrier arrived");

    // 在途期间：绑定已完成 → 生产命令移动 B→C 生成替代 edit 行。
    // A-P2-3①：subscribe 行的链起点 A 保留为替代行移除基准。
    {
        let conn = dbm.lock().await;
        let push = app_lib::commands::record_feed_edit(
            &conn,
            feed_id,
            None,
            Some(folder_c),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(push.is_some(), "绑定已完成：生产编辑必须携带远端推送目标");
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1, "同 URL 只保留最新意图");
        assert_eq!(items[0].action, "edit", "绑定完成后的编辑走 edit 行");
        assert_eq!(items[0].desired_folder.as_deref(), Some("C分类"));
        assert_eq!(
            items[0].prev_folder.as_deref(),
            Some("A分类"),
            "A-P2-3①：替代行必须保留 subscribe 行的链起点（挂载尚未成功）: {items:?}"
        );
    }

    barrier.release.send(()).unwrap();
    let _ = handle
        .await
        .expect("join feeds_phase")
        .expect("feeds phase (mount window rebase)");
    {
        let form = last_subscription_edit_form(&server);
        assert!(
            form_has(&form, "a", "user/-/label/B分类")
                && form_has(&form, "r", "user/-/label/A分类"),
            "挂载按发送时目标发出（a=B, r=A），实际 {form:?}"
        );
        let conn = dbm.lock().await;
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1, "旧挂载成功不得确认在途新意图");
        assert_eq!(
            items[0].prev_folder.as_deref(),
            Some("B分类"),
            "A-P2-3②：挂载成功的 2xx = 远端当下持有 B，替代行移除基准重定为 B: {items:?}"
        );
    }

    // 第二轮：替代行以重定后的基准推达（a=C, r=B），远端只剩 C
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (converge)");
    {
        let form = last_subscription_edit_form(&server);
        assert!(
            form_has(&form, "a", "user/-/label/C分类")
                && form_has(&form, "r", "user/-/label/B分类"),
            "替代行必须移除已接受的 B（修前 r=A 是 no-op，B 残留），实际 {form:?}"
        );
        let labels: Vec<String> = server
            .subscriptions
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.url == url)
            .expect("远端订阅仍在")
            .categories
            .iter()
            .map(|(l, _)| l.clone())
            .collect();
        assert!(
            labels.contains(&"C分类".to_string())
                && !labels.contains(&"B分类".to_string())
                && !labels.contains(&"A分类".to_string()),
            "远端必须只剩最终分类（无中间 label 残留）: {labels:?}"
        );
        let conn = dbm.lock().await;
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "替代行确认后精确出队"
        );
    }
    assert_eq!(
        quickadd_requests(&server),
        0,
        "远端已持有该订阅（种子）：列表证据直接解析，全程零 quick_add"
    );
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
9. legacy add_feed：挂载失败保留队项（pull 不回灌既有源分类，剪除即永久缺口）
============================================================ */

/// 审计 R2-P3（2026-11-01）：push_feeds 的分类挂载失败此前只记 report.errors、
/// 队项照常剪除，而 pull 对既有源不回灌远端分类 → 远端订阅永久留在默认分类
/// 且无收敛通道。修后：挂载失败不进 done（不剪除，保留重试）；下一轮经列表
/// 证据解析既有 id（零 quick_add）只重试挂载，收敛后照常出队。
#[tokio::test]
async fn push_feeds_mount_failure_keeps_legacy_queue_item_until_converges() {
    let server = MockGReader::start().await.expect("start mock");
    let tmp = common::unique_db_path("opt009b_legacy_mount");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = format!("{}/opt009-legacy-mount.xml", server.url());

    // 远端已有该订阅（feed/20，无分类）：列表证据直接解析，不走 quick_add
    server
        .existing_feed_urls
        .lock()
        .unwrap()
        .push((url.clone(), 20));
    *server.subscriptions.lock().unwrap() = vec![MockSubscription {
        id: "feed/20".into(),
        title: "legacy 挂载".into(),
        url: url.clone(),
        html_url: None,
        categories: vec![],
    }];

    let dbm = {
        let conn = db::open(&tmp).unwrap();
        db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
        db::set_setting(&conn, "greader_username", "test").unwrap();
        db::set_setting(&conn, "greader_password", "test-token").unwrap();
        let folder_a = db::create_folder(&conn, "A分类", "article").unwrap();
        db::insert_feed(
            &conn,
            &url,
            None,
            "legacy 挂载",
            None,
            folder_a,
            "inherit",
            true,
            false,
        )
        .unwrap();
        // legacy add_feed 队项（payload 携带目标分类，与历史版本同形）
        db::enqueue_sync(
            &conn,
            None,
            Some(&url),
            "add_feed",
            Some(&format!(r#"{{"folder_id":{folder_a}}}"#)),
        )
        .unwrap();
        Arc::new(Mutex::new(conn))
    };

    // 第一轮：挂载失败注入 → 队项必须保留（否则差距无收敛通道）
    server.set_fail_subscription_edit_for(Some("feed/20"));
    let report = sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (mount fail)");
    server.set_fail_subscription_edit_for(None);
    assert!(
        report.errors.iter().any(|e| e.contains("挂载分类")),
        "挂载失败必须可见: {:?}",
        report.errors
    );
    {
        let conn = dbm.lock().await;
        let queued: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_queue WHERE feed_url = ?1 AND action = 'add_feed'",
                [&url],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            queued, 1,
            "挂载失败不得剪除队项（pull 不回灌既有源分类，剪除即永久缺口）"
        );
        let (attempts, last_error): (i64, Option<String>) = conn
            .query_row(
                "SELECT attempts, last_error FROM sync_queue
                  WHERE feed_url = ?1 AND action = 'add_feed'",
                [&url],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            attempts, 1,
            "保留的 legacy 队项必须落账失败尝试（attempts 恒 0 则失败不可见，W2 复审 P3-3）"
        );
        assert!(
            last_error
                .as_deref()
                .unwrap_or_default()
                .contains("挂载分类"),
            "last_error 必须携带挂载失败摘要: {last_error:?}"
        );
        let labels: Vec<String> = server.subscriptions.lock().unwrap()[0]
            .categories
            .iter()
            .map(|(l, _)| l.clone())
            .collect();
        assert!(labels.is_empty(), "远端仍未挂载分类: {labels:?}");
    }

    // 第二轮：注入清除 → 只重试挂载（零 quick_add）→ 收敛出队
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (converge)");
    {
        let conn = dbm.lock().await;
        let queued: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_queue WHERE feed_url = ?1 AND action = 'add_feed'",
                [&url],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(queued, 0, "挂载成功后队项照常出队");
    }
    assert_eq!(
        quickadd_requests(&server),
        0,
        "重试经列表证据解析既有订阅，全程零 quick_add"
    );
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
9b. legacy add_feed：本地 URL 变体与队列字符串仅规范化后相等 →
    quick_add 后绑定必须命中本地源
============================================================ */

/// 审计 P3（2026-11-02）：push_feeds 的绑定查找原用 `feed_id_by_url` 精确
/// 匹配且 `.ok()` 吞 DB 错误——队列携带的 URL 与本地入库字符串常有尾斜杠/
/// www. 差异，quick_add 成功却永不绑定（该源在 pull 侧被当未绑定源重复
/// 处理）。修后走 `feed_id_by_url_normalized`（与 pull_feeds、OPML 去重
/// 同口径），DB 错误落 report.errors 且队项保留。
#[tokio::test]
async fn push_feeds_binds_local_feed_via_normalized_url_variant() {
    let server = MockGReader::start().await.expect("start mock");
    let tmp = common::unique_db_path("opt009b_legacy_bind_norm");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = format!("{}/opt009-legacy-bind.xml", server.url());
    // 本地入库带尾斜杠变体；legacy 队项（与 quick_add、列表证据）携带原字符串
    let local_url = format!("{url}/");

    // 清空远端订阅列表：列表证据解析不到 → 必须走 quick_add 新建
    *server.subscriptions.lock().unwrap() = vec![];

    let dbm = {
        let conn = db::open(&tmp).unwrap();
        db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
        db::set_setting(&conn, "greader_username", "test").unwrap();
        db::set_setting(&conn, "greader_password", "test-token").unwrap();
        let folder = db::create_folder(&conn, "B分类", "article").unwrap();
        db::insert_feed(
            &conn,
            &local_url,
            None,
            "规范化绑定",
            None,
            folder,
            "inherit",
            true,
            false,
        )
        .unwrap();
        db::enqueue_sync(&conn, None, Some(&url), "add_feed", None).unwrap();
        Arc::new(Mutex::new(conn))
    };

    let report = sync::feeds_phase(&dbm, &http).await.expect("feeds phase");
    assert!(
        report.errors.is_empty(),
        "正常收敛不得有错误: {:?}",
        report.errors
    );
    {
        let conn = dbm.lock().await;
        let queued: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_queue WHERE feed_url = ?1 AND action = 'add_feed'",
                [&url],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            queued, 0,
            "quick_add 成功且无分类挂载：队项照常出队: {report:?}"
        );
        let remote_registered = server
            .existing_feed_urls
            .lock()
            .unwrap()
            .iter()
            .find(|(u, _)| u == &url)
            .map(|(_, id)| *id)
            .expect("quick_add 必须在远端登记新订阅");
        let (bound, dupes): (Option<i64>, i64) = conn
            .query_row(
                "SELECT remote_id, COUNT(*) FROM feeds WHERE feed_url = ?1",
                [&local_url],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(dupes, 1, "不得因绑定漏判新建重复本地源");
        assert_eq!(
            bound,
            Some(remote_registered),
            "绑定查找必须按规范化 URL 命中本地尾斜杠变体"
        );
    }
    assert_eq!(
        quickadd_requests(&server),
        1,
        "远端无此订阅：恰好一次 quick_add"
    );
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
10. 会话构建失败落账（A-P2-6）：pending 行 attempts/last_error 可见
============================================================ */

/// try_push_subscription_outbox 的 `if let Ok(session)` 此前静默吞掉构建失败：
/// 认证/端点/网络失败时 UI 只能显示「等待」。修后对齐推送侧 TASK-124：
/// 有凭据但构建失败 → 当前账号全部 pending 行 attempts+1、last_error 记摘要；
/// 异账号行不触碰；队项全部保留（失败是重试不是放弃）。
#[tokio::test]
async fn try_push_records_session_block_on_build_failure() {
    let tmp = common::unique_db_path("opt009b_session_block");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    // 死地址：本机回环无监听，会话构建立即失败（不触外网）
    let dead = "http://127.0.0.1:1";

    let dbm = {
        let conn = db::open(&tmp).unwrap();
        db::set_setting(&conn, "greader_endpoint", dead).unwrap();
        db::set_setting(&conn, "greader_username", "test").unwrap();
        db::set_setting(&conn, "greader_password", "test-token").unwrap();
        let key = format!("greader|{dead}|test");
        let gen = db::sync_generation(&conn).unwrap();
        db::enqueue_subscribe(&conn, gen, &key, "https://f.example/rss", None, None).unwrap();
        db::enqueue_subscribe(
            &conn,
            gen,
            "other-acct",
            "https://g.example/rss",
            None,
            None,
        )
        .unwrap();
        Arc::new(Mutex::new(conn))
    };

    sync::try_push_subscription_outbox(&dbm, &http).await;

    let conn = dbm.lock().await;
    let items = db::pending_outbox(&conn).unwrap();
    assert_eq!(items.len(), 2, "队项全部保留（失败是重试不是放弃）");
    let mine = items
        .iter()
        .find(|i| i.account_key.starts_with("greader|"))
        .unwrap();
    assert_eq!(mine.attempts, 1, "构建失败必须落账（此前静默吞掉）");
    assert!(
        mine.last_error
            .as_deref()
            .unwrap_or_default()
            .contains("订阅写回"),
        "last_error 携带订阅写回摘要: {mine:?}"
    );
    let other = items
        .iter()
        .find(|i| i.account_key == "other-acct")
        .unwrap();
    assert_eq!(other.attempts, 0, "异账号行不触碰");
    assert!(other.last_error.is_none());
}

/// OPT-009B2 复审 R2-3（对照时序）：unsubscribe **已被服务端确认并出队**后再重加
/// ——被替换的退订行已不存在，重加行不得凭空携带移除基准（无 r=），远端只留新
/// 目标；两段时序的差异被分别断言（上一条对应「未确认被替换」）。
#[tokio::test]
async fn readd_after_completed_unsubscribe_sends_no_stale_removal_baseline() {
    let server = MockGReader::start().await.expect("start mock");
    let tmp = common::unique_db_path("opt009b_r2_readd_done");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = format!("{}/opt009-r2-readd-done.xml", server.url());

    // 前置：移动已确认后的状态（远端在 B分类；本地已绑定 10 且在 B分类）
    *server.subscriptions.lock().unwrap() = vec![MockSubscription {
        id: "feed/10".into(),
        title: "Readd Done".into(),
        url: url.clone(),
        html_url: None,
        categories: vec![
            ("B分类".into(), "folder".into()),
            ("保留分类".into(), "folder".into()),
        ],
    }];
    server
        .existing_feed_urls
        .lock()
        .unwrap()
        .push((url.clone(), 10));

    let (dbm, feed_id) = {
        let conn = open_with_credentials(&server, "greader", "test");
        let (_f, feed) = seed_bound_feed(&conn, &url, "B分类", 10);
        db::create_folder(&conn, "新分类", "article").unwrap();
        (Arc::new(Mutex::new(conn)), feed)
    };

    // ① 删除并让执行器**完成**退订（服务端确认、队项出队）
    {
        let conn = dbm.lock().await;
        app_lib::commands::record_feed_deletion(&conn, feed_id).expect("record deletion");
    }
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (unsubscribe)");
    {
        let conn = dbm.lock().await;
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "退订必须已确认出队"
        );
        assert!(
            db::feed_tombstones(&conn).unwrap().is_empty(),
            "pull 确认远端已不含后墓碑回收"
        );
    }
    assert!(
        server
            .subscriptions
            .lock()
            .unwrap()
            .iter()
            .all(|s| s.id != "feed/10"),
        "远端退订已生效"
    );

    // ② 重加：服务端已无该订阅（退订后 quick_add 会新建）——不得携带任何移除基准
    server
        .existing_feed_urls
        .lock()
        .unwrap()
        .retain(|(u, _)| u != &url);
    {
        let conn = dbm.lock().await;
        let report = app_lib::commands::opml_import_for_test(
            &conn,
            &opml_doc(&[(url.as_str(), "重加标题", Some("新分类"))]),
        )
        .expect("re-add");
        assert_eq!((report.imported, report.skipped), (1, 0));
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].action, "subscribe");
        assert_eq!(
            items[0].prev_folder, None,
            "退订已确认出队：重加行不得凭空携带移除基准: {items:?}"
        );
    }
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (readd)");
    {
        let form = last_subscription_edit_form(&server);
        assert!(
            form_has(&form, "a", "user/-/label/新分类"),
            "重加挂载必须发 a=目标，实际 {form:?}"
        );
        assert!(
            form.iter().all(|(k, _)| k != "r"),
            "退订已确认：不得发送任何残留的移除基准（r=），实际 {form:?}"
        );
        assert!(
            form_has(&form, "s", "feed/101"),
            "退订生效后 quick_add 必须新建订阅（不是旧 id），实际 {form:?}"
        );
        let conn = dbm.lock().await;
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "挂载确认后出队"
        );
    }
    let _ = std::fs::remove_file(&tmp);
}

/// OPT-009B2 复审 R2-3（在途被替换的变体）：unsubscribe 请求已发出、响应在途时
/// 重加——替换不得丢失退订意图/移除基准，旧响应的 2xx 也不得确认新意图；
/// 下一轮收敛（mock 在响应写回前已应用退订状态，故远端此时已无该订阅，
/// 重加 quick_add 新建；携带的 r= 在新订阅上是 no-op——本用例只断言收敛，
/// 不把「不存在的 label 移除恒为成功」写成契约）。
#[tokio::test]
async fn readd_while_unsubscribe_inflight_keeps_replacement_converging() {
    let server = MockGReader::start().await.expect("start mock");
    let tmp = common::unique_db_path("opt009b_r2_readd_inflight");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = format!("{}/opt009-r2-readd-inflight.xml", server.url());

    *server.subscriptions.lock().unwrap() = vec![MockSubscription {
        id: "feed/10".into(),
        title: "Readd Inflight".into(),
        url: url.clone(),
        html_url: None,
        categories: vec![
            ("B分类".into(), "folder".into()),
            ("保留分类".into(), "folder".into()),
        ],
    }];
    server
        .existing_feed_urls
        .lock()
        .unwrap()
        .push((url.clone(), 10));

    let (dbm, feed_id) = {
        let conn = open_with_credentials(&server, "greader", "test");
        let (_f, feed) = seed_bound_feed(&conn, &url, "B分类", 10);
        db::create_folder(&conn, "新分类", "article").unwrap();
        (Arc::new(Mutex::new(conn)), feed)
    };

    // 删除（退订 pending）
    {
        let conn = dbm.lock().await;
        app_lib::commands::record_feed_deletion(&conn, feed_id).expect("record deletion");
    }

    // 退订请求在途（服务端状态已变更、响应未写回）
    let barrier = server.arm_subscription_edit_barrier("feed/10");
    let db2 = dbm.clone();
    let http2 = http.clone();
    let handle = tokio::spawn(async move { sync::feeds_phase(&db2, &http2).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), barrier.arrived)
        .await
        .expect("执行器必须在期限内发出退订（网络屏障）")
        .expect("barrier arrived");

    // 在途期间：重加同 URL（替换 pending 退订行）
    {
        let conn = dbm.lock().await;
        let report = app_lib::commands::opml_import_for_test(
            &conn,
            &opml_doc(&[(url.as_str(), "重加标题", Some("新分类"))]),
        )
        .expect("re-add in flight");
        assert_eq!((report.imported, report.skipped), (1, 0));
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1, "同 URL 只留最新意图: {items:?}");
        assert_eq!(items[0].action, "subscribe");
        assert_eq!(
            items[0].prev_folder.as_deref(),
            Some("B分类"),
            "在途退订被替换时同样必须继承移除基准: {items:?}"
        );
    }
    // 模型：退订已生效（mock 状态在响应写回前已变更）→ 服务端不再持有该订阅；
    // 注册表同步移除，使重加的 quick_add 走「新建」语义。
    server
        .existing_feed_urls
        .lock()
        .unwrap()
        .retain(|(u, _)| u != &url);

    barrier.release.send(()).unwrap();
    let _ = handle
        .await
        .expect("join feeds_phase")
        .expect("feeds phase (in-flight unsubscribe, replaced)");
    {
        let conn = dbm.lock().await;
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(
            items.len(),
            1,
            "旧退订的 2xx 不得确认（prune）在途替换出的新意图: {items:?}"
        );
        assert_eq!(items[0].action, "subscribe");
    }
    assert!(
        server
            .subscriptions
            .lock()
            .unwrap()
            .iter()
            .all(|s| s.id != "feed/10"),
        "远端退订已生效"
    );

    // 下一轮：重加收敛（quick_add 新建 → 挂载 a=新分类 → 出队）
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (readd after in-flight)");
    {
        let form = last_subscription_edit_form(&server);
        assert!(
            form_has(&form, "a", "user/-/label/新分类"),
            "重加挂载必须发 a=目标，实际 {form:?}"
        );
        assert!(
            !form.iter().any(|(k, v)| k == "s" && v == "feed/10"),
            "不得对已被服务端移除的旧 id 发送移动请求，实际 {form:?}"
        );
        let conn = dbm.lock().await;
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "重加收敛后出队"
        );
    }
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
R2 复审 P2-C. 退订目标被外部删除（FreshRSS 400）不得永久 Retry
============================================================ */

/// OPT-009B2 R2 复审 P2-C（B1 契约）：已持久化的解析目标被**外部删除**后，FreshRSS 的
/// `subscriptionEdit` 对不存在 id 走 `badRequest()` → HTTP 400（pinned 上游
/// `freshrss-greader.php:122-128` 的 400 形态 + `:468-471` 的
/// `!($feedId > 0 && deleteFeed($feedId))` 分支）。旧实现任一 Err 立即 Retry：
/// 队项永久卡住、attempts 无界增长，列表恢复后同 URL 的其它 id 也永不被退订。
/// 修后：400/404 = 目标已不存在（退订意图已满足，非致命）——本轮继续退订其它
/// 同 URL 目标并精确出队；5xx/网络错误仍是 Retry。
/// B1 起解析走列表证据（零 quick_add）：第一轮以订阅编辑注入 500 让「解析已
/// 持久化、发送失败」的意图保留到第二轮。
/// 判别力（旧实现）：第二轮后队项仍为 1、feed/50 未被退订——下面断言必红。
#[tokio::test]
async fn unsubscribe_recovery_missing_resolved_target_converges_instead_of_stuck_retry() {
    let server = MockGReader::start().await.expect("start mock");
    let tmp = common::unique_db_path("opt009b_r2_missing");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = format!("{}/opt009-r2-missing.xml", server.url());
    let key = acct_key("greader", &server, "test");

    // 远端同 URL 两条（历史遗留的重复项）：feed/40 稍后被外部删除，feed/50 保留
    *server.subscriptions.lock().unwrap() = vec![
        MockSubscription {
            id: "feed/40".into(),
            title: "Twin A".into(),
            url: url.clone(),
            html_url: None,
            categories: vec![],
        },
        MockSubscription {
            id: "feed/50".into(),
            title: "Twin B".into(),
            url: url.clone(),
            html_url: None,
            categories: vec![],
        },
    ];
    server
        .existing_feed_urls
        .lock()
        .unwrap()
        .push((url.clone(), 40));
    // 第一轮让退订发送失败（订阅编辑注入 500）：解析已持久化、意图保留
    server.set_fail_subscription_edit(true);

    let (dbm, feed_id) = {
        let conn = db::open(&tmp).unwrap();
        db::set_setting(&conn, "sync_protocol", "greader").unwrap();
        db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
        db::set_setting(&conn, "greader_username", "test").unwrap();
        db::set_setting(&conn, "greader_password", "test-token").unwrap();
        let folder = db::create_folder(&conn, "分类", "article").unwrap();
        let feed = db::insert_feed(
            &conn, &url, None, "Missing", None, folder, "inherit", true, false,
        )
        .unwrap();
        let gen = db::sync_generation(&conn).unwrap();
        // 创建证据：同账号同 URL 的 pending subscribe（quick_add 已发出、响应丢失）
        db::enqueue_subscribe(&conn, gen, &key, &url, None, Some("分类")).unwrap();
        (Arc::new(Mutex::new(conn)), feed)
    };
    {
        let conn = dbm.lock().await;
        app_lib::commands::record_feed_deletion(&conn, feed_id).expect("record deletion");
    }

    // 第一轮：列表证据解析 feed/40 并持久化；退订发送被注入拒绝 → 保留意图
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (round 1)");
    {
        let conn = dbm.lock().await;
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1, "前置：可恢复意图保留");
        assert_eq!(
            items[0].resolved_remote_id,
            Some(40),
            "前置：列表证据解析目标已持久化: {items:?}"
        );
        assert!(items[0].attempts >= 1, "前置：发送失败累计 attempts");
        assert_eq!(
            quickadd_requests(&server),
            0,
            "恢复解析零 quick_add（列表是唯一证据源）"
        );
    }
    assert!(
        mock_greader::subscription_edit_actions(&server)
            .iter()
            .all(|(ac, _)| ac != "unsubscribe"),
        "前置：发现未完成时不发送退订"
    );

    // 外部删除 feed/40（其它客户端/Web UI）→ 列表恢复 + FreshRSS 语义注入：
    // 退订不存在的 id 返回 400
    server
        .subscriptions
        .lock()
        .unwrap()
        .retain(|s| s.id != "feed/40");
    server
        .existing_feed_urls
        .lock()
        .unwrap()
        .retain(|(u, _)| u != &url);
    server.set_fail_subscription_edit(false);
    server.set_unsubscribe_unknown_id_returns_400(true);

    // 第二轮：40 已被外部删除 → 400 必须视为「已不存在」而非永久 Retry；
    // 同 URL 的 feed/50 仍要被退订，随后整条意图精确出队。
    let report = sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (missing target)");
    {
        let actions = mock_greader::subscription_edit_actions(&server);
        assert!(
            actions
                .iter()
                .any(|(ac, s)| ac == "unsubscribe" && s == "feed/40"),
            "必须尝试过 40 的退订: {actions:?}"
        );
        assert!(
            actions
                .iter()
                .any(|(ac, s)| ac == "unsubscribe" && s == "feed/50"),
            "40 的 400 不得阻断同 URL 其它目标的退订（旧实现此处永久 Retry）: \
             {actions:?} / report {report:?}"
        );
        assert!(
            server
                .subscriptions
                .lock()
                .unwrap()
                .iter()
                .all(|s| s.url != url),
            "远端不得残留同 URL 订阅"
        );
        let conn = dbm.lock().await;
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "目标不存在 = 意图已满足：收敛出队（旧实现永久 Retry）: {report:?}"
        );
        assert!(
            db::feed_tombstones(&conn).unwrap().is_empty(),
            "pull 确认远端已不含该 URL 后墓碑回收"
        );
        let revived: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM feeds WHERE active=1 AND feed_url = ?1",
                [&url],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(revived, 0, "收敛后仍不得复活");
    }
    // 收敛后不得再有退订请求（无残留意图反复重试）
    server.clear_request_log();
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (idle)");
    assert!(
        !server
            .request_log()
            .iter()
            .any(|r| r.contains("subscription/edit")),
        "收敛后不得再发送任何订阅编辑/退订请求"
    );
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
R2 复审 P3-A. 移动未发送 + 删除 + 快速重加：链起点必须成为移除基准
============================================================ */

/// OPT-009B2 R2 复审 P3-A：移动 A→B **未发送**（edit 行 prev=A、远端仍持 A）时
/// 删除并快速重加——删除侧若只快照本地 label（B）会得到错误的移除基准：挂载发
/// `a=新,r=B` 是 no-op，远端 A 残留。修后：`enqueue_unsubscribe` 读被替换 pending
/// 行的**链起点** `prev_folder`（A，链起点优先于调用方快照），重加继承它 →
/// 挂载发 `a=新分类 + r=A分类`，远端只剩新目标与无关标签。
/// 判别力（旧实现 = 只用调用方快照 B）：退订行/重加行的 prev 为 B、远端残留 A —— 必红。
#[tokio::test]
async fn unsent_move_then_delete_and_readd_removes_remote_old_label() {
    let server = MockGReader::start().await.expect("start mock");
    let tmp = common::unique_db_path("opt009b_r2_unsent");
    let _ = std::fs::remove_file(&tmp);
    let http = app_lib::ingestion::build_client(10);
    let url = format!("{}/opt009-r2-unsent.xml", server.url());

    // 远端订阅在 A分类（「保留分类」同在）；注册表保证 quick_add 幂等返回 feed/10
    *server.subscriptions.lock().unwrap() = vec![MockSubscription {
        id: "feed/10".into(),
        title: "Unsent Move".into(),
        url: url.clone(),
        html_url: None,
        categories: vec![
            ("A分类".into(), "folder".into()),
            ("保留分类".into(), "folder".into()),
        ],
    }];
    server
        .existing_feed_urls
        .lock()
        .unwrap()
        .push((url.clone(), 10));

    let (dbm, feed_id) = {
        let conn = open_with_credentials(&server, "greader", "test");
        let (_f, feed) = seed_bound_feed(&conn, &url, "A分类", 10);
        db::create_folder(&conn, "B分类", "article").unwrap();
        db::create_folder(&conn, "新分类", "article").unwrap();
        (Arc::new(Mutex::new(conn)), feed)
    };

    // ① 移动 A→B 入队但**不跑执行器**（未发送：远端仍是 A）
    {
        let conn = dbm.lock().await;
        let folder_b = db::list_folders(&conn)
            .unwrap()
            .into_iter()
            .find(|f| f.name == "B分类")
            .expect("B分类存在")
            .id;
        app_lib::commands::record_feed_edit(
            &conn,
            feed_id,
            None,
            Some(folder_b),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1, "前置：未发送的移动意图在队: {items:?}");
        assert_eq!(items[0].action, "edit");
        assert_eq!(
            items[0].prev_folder.as_deref(),
            Some("A分类"),
            "前置：链起点 = 远端当前 label"
        );
        assert_eq!(items[0].remote_id, Some(10), "前置：edit 行带远端 id");
    }

    // ② 删除：移除基准必须取链起点 A（不得用本地已生效的 B）
    {
        let conn = dbm.lock().await;
        app_lib::commands::record_feed_deletion(&conn, feed_id).expect("record deletion");
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].action, "unsubscribe");
        assert_eq!(
            items[0].prev_folder.as_deref(),
            Some("A分类"),
            "链起点优先：移动未发送，远端持有的仍是 A（旧实现取本地 label B → r=B 是 no-op）: {items:?}"
        );
    }

    // ③ 快速重加（退订尚未确认）：继承链起点
    {
        let conn = dbm.lock().await;
        let report = app_lib::commands::opml_import_for_test(
            &conn,
            &opml_doc(&[(url.as_str(), "重加标题", Some("新分类"))]),
        )
        .expect("re-add");
        assert_eq!((report.imported, report.skipped), (1, 0));
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1, "同 URL 只留最新意图: {items:?}");
        assert_eq!(items[0].action, "subscribe");
        assert_eq!(
            items[0].prev_folder.as_deref(),
            Some("A分类"),
            "重加行必须继承链起点: {items:?}"
        );
    }

    // ④ 执行器：quick_add 命中既有 feed/10 → 挂载 a=新分类 + r=A分类
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (readd)");
    {
        let form = last_subscription_edit_form(&server);
        assert!(
            form_has(&form, "a", "user/-/label/新分类")
                && form_has(&form, "r", "user/-/label/A分类"),
            "重加挂载必须 a=新 + r=A（链起点；旧实现发 r=B 是 no-op），实际 {form:?}"
        );
        let labels: Vec<String> = server
            .subscriptions
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.id == "feed/10")
            .expect("远端订阅仍在")
            .categories
            .iter()
            .map(|(l, _)| l.clone())
            .collect();
        assert!(
            labels.contains(&"新分类".to_string()),
            "远端必须加入重加的目标分类: {labels:?}"
        );
        assert!(
            !labels.contains(&"A分类".to_string()),
            "未发送移动的链起点必须被移除（旧实现残留 A）: {labels:?}"
        );
        assert!(
            !labels.contains(&"B分类".to_string()),
            "从未发送的目标不得出现在远端: {labels:?}"
        );
        assert!(
            labels.contains(&"保留分类".to_string()),
            "无关 label 必须保留: {labels:?}"
        );
        let conn = dbm.lock().await;
        assert!(
            db::pending_outbox(&conn).unwrap().is_empty(),
            "挂载确认后精确出队"
        );
        let bound: Option<i64> = conn
            .query_row(
                "SELECT remote_id FROM feeds WHERE feed_url = ?1",
                [&url],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bound, Some(10), "重加源绑定既有远端订阅（幂等）");
    }
    let _ = std::fs::remove_file(&tmp);
}
