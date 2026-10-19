//! OPT-001（审计 F01）：可靠 outbox 操作身份端到端回归。
//!
//! 场景（与任务卡验收伪码同构）：
//!   ① 取旧队列快照 → 本地新操作入队（read→unread / star→unstar）→ 旧推送
//!      计划按旧 id 确认（prune_sync）→ 新用户意图必须仍在队列；
//!   ② 队列清空后再入队、关闭重开数据库后，操作 id 不复用。
//!
//! 修前（v17 sync_queue 为普通 INTEGER PRIMARY KEY）两个场景都会翻车：替换
//! 入队复用被删行空出的最大 ROWID，旧计划的按 id 确认会误删新的用户意图。
//! v18 起 sync_queue 为 AUTOINCREMENT（sqlite_sequence 由 SQLite 自动维护，
//! 见 src/db/migrations.rs），本文件用生产 db API 锁定该契约。
//!
//! 运行：cargo test --test outbox_identity_e2e

mod common;
mod mock_greader;

use app_lib::db;
use app_lib::sync;
use mock_greader::MockGReader;
use std::sync::Arc;
use tokio::sync::Mutex;

/// 生产 open 路径建库 + 一篇本地文章（FK 已开，队项可绑定 article）。
fn setup(name: &str) -> (rusqlite::Connection, std::path::PathBuf, i64) {
    let tmp = common::unique_db_path(&format!("outbox_{name}"));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).expect("open db");
    conn.execute_batch(
        "INSERT INTO feeds (feed_url, title) VALUES ('https://f.example/rss', 'F');",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO articles (feed_id, guid, title) VALUES (1, 'g1', 't')",
        [],
    )
    .unwrap();
    let aid = conn.last_insert_rowid();
    (conn, tmp, aid)
}

/// 取当前队列里唯一的待推项（每步都断言恰好一条，防止误读旧行）。
fn sole_item(conn: &rusqlite::Connection) -> db::SyncQueueItem {
    let mut q = db::take_sync_queue(conn).unwrap();
    assert_eq!(q.len(), 1, "本步必须恰好一条待推项");
    q.remove(0)
}

/// ① read→unread：旧快照在网络往返后确认，不得误删新的 unread 意图。
#[test]
fn read_to_unread_replacement_survives_stale_confirmation() {
    let (conn, tmp, aid) = setup("read_unread");

    // 旧队列快照：本地标读入队（push 计划此刻取到 items[0] = 本快照）
    db::set_read_with_enqueue(&conn, aid, true).unwrap();
    let old = sole_item(&conn);
    assert_eq!(old.action, "read");

    // 网络期间用户又标为未读：同一进程内旧行被删、新操作入队
    db::set_read_with_enqueue(&conn, aid, false).unwrap();
    let fresh = sole_item(&conn);
    assert_eq!(fresh.action, "unread");
    assert_ne!(fresh.id, old.id, "替换入队必须换新 id（v17 会复用旧 id）");

    // 旧计划返回后按旧 id 确认：只应清掉旧操作，新意图必须存活
    db::prune_sync(&conn, &[old.id]).unwrap();
    let after = sole_item(&conn);
    assert_eq!(after.id, fresh.id, "旧 id 的确认不得删掉新的 unread 意图");
    assert_eq!(after.action, "unread");

    // 本地状态与队列语义一致（未读）
    let is_read: i64 = conn
        .query_row("SELECT is_read FROM articles WHERE id = ?1", [aid], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(is_read, 0, "本地已读状态必须为未读");
    drop(conn);
    std::fs::remove_file(&tmp).expect("清理临时库失败");
}

/// ① star→unstar：同一断言链的收藏半边（toggle 语义非幂等，旧确认误删
/// 会让「取消收藏」在服务端永远不生效）。
#[test]
fn star_to_unstar_replacement_survives_stale_confirmation() {
    let (conn, tmp, aid) = setup("star_unstar");

    db::set_starred_with_enqueue(&conn, aid, true).unwrap();
    let old = sole_item(&conn);
    assert_eq!(old.action, "star");

    db::set_starred_with_enqueue(&conn, aid, false).unwrap();
    let fresh = sole_item(&conn);
    assert_eq!(fresh.action, "unstar");
    assert_ne!(fresh.id, old.id, "替换入队必须换新 id（v17 会复用旧 id）");

    db::prune_sync(&conn, &[old.id]).unwrap();
    let after = sole_item(&conn);
    assert_eq!(after.id, fresh.id, "旧 id 的确认不得删掉新的 unstar 意图");
    assert_eq!(after.action, "unstar");

    let is_starred: i64 = conn
        .query_row(
            "SELECT is_starred FROM articles WHERE id = ?1",
            [aid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(is_starred, 0, "本地收藏状态必须为未收藏");
    drop(conn);
    std::fs::remove_file(&tmp).expect("清理临时库失败");
}

/// ② 空队列清空后再入队 / 关闭重开数据库后，id 均不复用（序列持久）。
#[test]
fn ids_not_reused_after_queue_drained_or_db_reopened() {
    let (conn, tmp, aid) = setup("drain_reopen");

    // 先制造一轮替换，拿到两个已提交 id
    db::set_starred_with_enqueue(&conn, aid, true).unwrap();
    let first = sole_item(&conn).id;
    db::set_starred_with_enqueue(&conn, aid, false).unwrap();
    let second = sole_item(&conn).id;
    assert_ne!(first, second, "替换入队已换新 id（前置）");

    // 远端确认成功 → 队列清空
    db::prune_sync(&conn, &[second]).unwrap();
    assert!(db::take_sync_queue(&conn).unwrap().is_empty(), "队列已清空");

    // 空队列再入队：id 必须高于曾用过的最大 id，不得从头复用
    db::set_read_with_enqueue(&conn, aid, true).unwrap();
    let third = sole_item(&conn).id;
    assert!(
        third > second && third > first,
        "清空后新项 id 不得复用已提交过的 id（v17 空表会从 1 重来）"
    );

    // 关闭重开：sqlite_sequence 持久，跨连接/重启同样不复用
    drop(conn);
    let conn = db::open(&tmp).expect("reopen db");
    db::set_read_with_enqueue(&conn, aid, false).unwrap();
    let fourth = sole_item(&conn).id;
    assert!(fourth > third, "重开后替换入队仍不得复用（id 持续走高）");
    drop(conn);
    std::fs::remove_file(&tmp).expect("清理临时库失败");
}

/// ③ OPT-018 A-2：未发送的文章级 `sync_queue` 行在关闭并重开 DB 后仍存在，
/// 且仍能被真实推送计划消费。
///
/// 判别力：断言只读重开后的连接（不依赖进程内缓存/上一连接的内存状态）；
/// 若入队未落盘、重开即丢行，或 `plan_push` 只消费进程内快照，本用例必红。
/// 推送走生产 `push_states_now`（锁内 `plan_push` → mock edit-tag → 成功出队），
/// 远端状态与队列清空双向验证。
#[tokio::test]
async fn unsent_sync_queue_row_survives_reopen_and_is_consumed_by_push() {
    let server = MockGReader::start().await.expect("start mock server");
    let tmp = common::unique_db_path("outbox_restart_push");
    let _guard = common::TempDbGuard::new(tmp.clone());
    let _ = std::fs::remove_file(&tmp);

    let remote_id;
    let aid;
    {
        let conn = db::open(&tmp).expect("open db");
        db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
        db::set_setting(&conn, "greader_username", "test").unwrap();
        db::set_setting(&conn, "greader_password", "test-token").unwrap();
        conn.execute_batch(
            "INSERT INTO feeds (feed_url, title) VALUES ('https://f.example/rss', 'F');",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO articles (feed_id, guid, title, url)
             VALUES (1, 'g-restart', 't', 'https://f.example/post/restart')",
            [],
        )
        .unwrap();
        aid = conn.last_insert_rowid();
        remote_id = server.add_entry_ret(
            10,
            "https://f.example/post/restart",
            "Remote restart",
            false,
            false,
        );
        assert!(
            db::set_article_remote_id(&conn, aid, remote_id).unwrap(),
            "前置：本地条目已绑定远端 id"
        );

        // 本地标读：入队但**不发送**，随后立即关闭连接。
        db::set_read_with_enqueue(&conn, aid, true).unwrap();
        let queued = db::take_sync_queue(&conn).unwrap();
        assert_eq!(queued.len(), 1, "前置：关闭前恰有一条未发送队项");
        assert_eq!(queued[0].article_id, Some(aid));
        assert_eq!(queued[0].action, "read");
    } // conn 在此 drop：SQLite 连接关闭（模拟应用退出）

    // 重开同一路径：队项必须来自磁盘，而不是上一连接的内存状态。
    let reopened = db::open(&tmp).expect("reopen db");
    let persisted = db::take_sync_queue(&reopened).unwrap();
    assert_eq!(
        persisted.len(),
        1,
        "未发送的 sync_queue 行必须在重开后仍存在"
    );
    assert_eq!(persisted[0].article_id, Some(aid));
    assert_eq!(persisted[0].action, "read");
    assert_eq!(
        db::sync_queue_stats(&reopened).unwrap().waiting,
        1,
        "重开后统计口径同样看到未发送项"
    );
    assert!(
        mock_greader::status_updates_map(&server).is_empty(),
        "前置：重开前远端从未收到该状态"
    );

    // 真实推送：plan_push 从重开后的连接读队列，成功后才 prune。
    let dbm = Arc::new(Mutex::new(reopened));
    let http = app_lib::ingestion::build_client(10);
    sync::push_states_now(&dbm, &http).await;

    {
        let conn = dbm.lock().await;
        assert!(
            db::take_sync_queue(&conn).unwrap().is_empty(),
            "推送计划必须消费重开后读到的队项（成功后精确出队）"
        );
    }
    assert_eq!(
        mock_greader::status_updates_map(&server)
            .get(&remote_id)
            .map(String::as_str),
        Some("read"),
        "重开后的队项必须真正到达远端"
    );
}
