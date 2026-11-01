//! OPT-009 B3 集成：订阅写回回执（当前账号限定）、能力矩阵与 outbox 生命周期
//! 走**生产账号生命周期**（`save_account_for_test` / `disconnect_for_test` =
//! 与 `sync_save` / `sync_disconnect` 同一个提交内核）+ 独立文件库，
//! 证明账号限定与身份切换清账在真实换号/断开/重连路径下成立，而不是只在内存
//! 单测里成立。
//!
//! 覆盖：
//! 1. 未连接：能力矩阵 = 未连接态（三态其一）；回执全零（无当前账号）；
//! 2. GReader 接入后走生产删除路径（`record_feed_deletion`）→ 持久退订意图落库，
//!    回执 pending 可见；能力矩阵 = GReader 态；
//! 3. 换号到 Fever 账号 B：A 的暂停队项**不得混入** B 的回执（全库口径会红），
//!    能力矩阵 = Fever 态（逐动作仅本地）；
//! 4. 回原账号 A：旧队项**不复活**（身份切换清账 = 生命周期终点，2026-11-02
//!    起放弃「换号暂停、回原账号恢复」语义）；
//! 5. 生命周期链：A 重新入队 → 断开（行保留）→ 同身份重连（行仍在、恢复可见）
//!    → 换用户名重连（旧行清账）。
//!
//! Note: 同身份保留、身份切换清账 — 见 .agents/notes/implemented/bug-fix/2026-11-02-订阅outbox生命周期终点.md
//!
//! 运行：cargo test --test opt009_phaseb3_e2e

mod common;

use app_lib::commands;
use app_lib::db;

fn open_conn(tag: &str) -> rusqlite::Connection {
    let tmp = common::unique_db_path(tag);
    let _ = std::fs::remove_file(&tmp);
    db::open(&tmp).unwrap()
}

#[test]
fn b3_receipts_capabilities_and_outbox_lifecycle_across_account_lifecycle() {
    let mut conn = open_conn("opt009b3_e2e");

    // 1) 未连接：能力矩阵 = 未连接态；回执全零（没有当前账号，计数无归属）
    let caps = commands::subscription_capabilities_for_test(&conn).unwrap();
    assert!(!caps.connected, "未连接：connected=false");
    assert!(caps.protocol.is_none(), "未连接：protocol=null");
    let s = commands::subscription_outbox_stats_for_test(&conn).unwrap();
    assert_eq!(
        (s.pending, s.failed, s.local_only),
        (0, 0, 0),
        "未连接：回执全零（无当前账号）"
    );

    // 2) GReader 账号 A：删除一个已绑定源 → 生产路径落持久退订意图
    commands::save_account_for_test(
        &mut conn,
        "greader",
        "http://a.example",
        "user-a",
        "pw-a",
        "http://a.example",
    )
    .unwrap();
    let folder = db::create_folder(&conn, "分类A", "article").unwrap();
    let feed = db::insert_feed_origin(
        &conn,
        "http://a.example/feed.xml",
        None,
        "A 源",
        None,
        folder,
        "inherit",
        true,
        false,
        "local",
    )
    .unwrap();
    db::set_feed_remote_id(&conn, feed, 42).unwrap();
    commands::record_feed_deletion(&conn, feed).unwrap();

    let s_a = commands::subscription_outbox_stats_for_test(&conn).unwrap();
    assert_eq!(s_a.pending, 1, "删除落持久退订意图，回执必须可见");
    let caps_a = commands::subscription_capabilities_for_test(&conn).unwrap();
    assert!(caps_a.connected);
    assert_eq!(caps_a.protocol.as_deref(), Some("greader"));
    assert_eq!(
        caps_a.unsubscribe.support,
        commands::SubscriptionWriteSupport::Supported,
        "GReader 退订为远端写回（Supported）"
    );

    // 3) 换号到 Fever 账号 B：身份切换清账 —— A 的队项物理删除（不再暂停保留）
    commands::save_account_for_test(
        &mut conn,
        "fever",
        "http://b.example",
        "user-b",
        "pw-b",
        "http://b.example",
    )
    .unwrap();
    let s_b = commands::subscription_outbox_stats_for_test(&conn).unwrap();
    assert_eq!(
        (s_b.pending, s_b.failed, s_b.local_only),
        (0, 0, 0),
        "异账号暂停队项不得混入当前账号回执（旧全库口径实现会红）"
    );
    let caps_b = commands::subscription_capabilities_for_test(&conn).unwrap();
    assert_eq!(caps_b.protocol.as_deref(), Some("fever"));
    assert_eq!(
        (caps_b.subscribe.support, caps_b.unsubscribe.support),
        (
            commands::SubscriptionWriteSupport::LocalOnly,
            commands::SubscriptionWriteSupport::LocalOnly
        ),
        "Fever：无写订阅端点 → 仅本地（不冒充远端完成）"
    );

    // 4) 回原账号 A：清账即生命周期终点，旧队项不复活
    commands::save_account_for_test(
        &mut conn,
        "greader",
        "http://a.example",
        "user-a",
        "pw-a",
        "http://a.example",
    )
    .unwrap();
    let s_again = commands::subscription_outbox_stats_for_test(&conn).unwrap();
    assert_eq!(
        s_again.pending, 0,
        "身份切换已清账：回原账号不复活旧队项（2026-11-02 语义）"
    );
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM subscription_outbox", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total, 0, "切换时已物理删除，表内无旧行");

    // 5) 生命周期链：入队 → 断开保留 → 同身份重连保留 → 换用户名重连清账
    let key_a = app_lib::sync::account_key("greader", "http://a.example", "user-a");
    let gen = db::sync_generation(&conn).unwrap();
    db::enqueue_subscribe(
        &conn,
        gen,
        &key_a,
        "https://lifecycle.example/rss",
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        commands::subscription_outbox_stats_for_test(&conn)
            .unwrap()
            .pending,
        1,
        "同身份在场：入队即可见"
    );

    // 断开：生命周期终点只在身份切换，断开保留行（同身份重连仍可发）
    commands::disconnect_for_test(&mut conn).unwrap();
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM subscription_outbox", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total, 1, "断开保留队项（不碰 outbox）");

    // 同身份重连：行保留且恢复可见
    commands::save_account_for_test(
        &mut conn,
        "greader",
        "http://a.example",
        "user-a",
        "pw-a",
        "http://a.example",
    )
    .unwrap();
    assert_eq!(
        commands::subscription_outbox_stats_for_test(&conn)
            .unwrap()
            .pending,
        1,
        "同身份重连：行保留且恢复可见"
    );

    // 换用户名重连（同协议同端点 = 新身份）：旧行清账
    commands::disconnect_for_test(&mut conn).unwrap();
    commands::save_account_for_test(
        &mut conn,
        "greader",
        "http://a.example",
        "user-a2",
        "pw-a2",
        "http://a.example",
    )
    .unwrap();
    let s_a2 = commands::subscription_outbox_stats_for_test(&conn).unwrap();
    assert_eq!(s_a2.pending, 0, "换用户名重连：旧行已清账");
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM subscription_outbox", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total, 0, "换身份重连后表内不再有旧账号行");
}
