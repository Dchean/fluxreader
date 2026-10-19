//! 凭据失败关闭集成测试（OPT-014 / F22）：真实 DPAPI 加密 + 损坏密文负例。
//!
//! 行为契约：
//! - 加密成功时落库为 `dpapi:` 密文（持久化值不含测试明文），读取往返还原；
//! - 损坏密文（非法 base64 / 非 DPAPI 密文）读取返回可见错误；
//! - legacy 明文读取兼容保持（迁移的前置条件）；
//! - Windows DPAPI 迁移升级 + 幂等。
//!
//! 运行：cargo test --test credential_safety_e2e
//! 说明：失败注入（加密器模拟失败/迁移失败回滚）已按 review R1 收进
//! `credentials.rs` / `db/settings.rs` / `commands/sync.rs` 的模块单测——库外
//! 不存在任何可注入加密后端的 pub 入口（非 IPC 后门也无 Rust 公共面）。
//! DPAPI 真实路径只在 Windows 存在，CI rust job 固定在 windows-latest。

use app_lib::db;

mod common;

/// 打开一个迁移到最新的空临时库（进程内唯一名，随测试隔离）。
fn fresh_conn(name: &str) -> rusqlite::Connection {
    let tmp = common::unique_db_path(name);
    let _ = std::fs::remove_file(&tmp);
    db::open(&tmp).expect("open temp db")
}

/// 读 settings 原始存库值（绕过解密层，断言「DB 里到底存了什么」）。
fn raw_value(conn: &rusqlite::Connection, key: &str) -> Option<String> {
    use rusqlite::OptionalExtension;
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
        r.get(0)
    })
    .optional()
    .expect("query settings")
}

/// 明文注入读取兼容：无 `dpapi:` 前缀的旧值必须原样读回（迁移前置条件）。
#[test]
fn legacy_plaintext_reads_still_work() {
    let conn = fresh_conn("cred_legacy_compat");
    conn.execute(
        "INSERT INTO settings (key, value) VALUES ('greader_password', 'legacy-plain-password')",
        [],
    )
    .unwrap();
    assert_eq!(
        db::get_setting(&conn, "greader_password").unwrap().unwrap(),
        "legacy-plain-password",
        "无 dpapi: 前缀的旧明文必须原样读取（迁移兼容口径）"
    );
}

/// 损坏密文：非法 base64 → 可见 Err（不把 `dpapi:` 原串当密码返回）。
#[test]
fn invalid_base64_ciphertext_read_is_error() {
    let conn = fresh_conn("cred_corrupt_b64");
    conn.execute(
        "INSERT INTO settings (key, value) VALUES ('greader_password', 'dpapi:@@not-base64@@')",
        [],
    )
    .unwrap();
    let err = db::get_setting(&conn, "greader_password")
        .expect_err("损坏密文读取必须返回可见错误，不能把 dpapi: 原串当密码");
    // Windows 上先过 base64 解析（credentialCorrupt）；非 Windows 无解密能力（credentialDecrypt）
    #[cfg(windows)]
    assert_eq!(err.code, "credentialCorrupt");
    #[cfg(not(windows))]
    assert_eq!(err.code, "credentialDecrypt");
    assert!(
        !err.message.contains("@@not-base64@@"),
        "错误不得回显密文原文：{err}"
    );
}

/// 合法 base64 但不是 DPAPI 密文：读取必须拒绝（Windows CryptUnprotectData / 非 Windows 无能力）。
#[test]
fn non_dpapi_ciphertext_read_is_error() {
    let conn = fresh_conn("cred_corrupt_cipher");
    conn.execute(
        "INSERT INTO settings (key, value) VALUES ('greader_password', 'dpapi:bm90LWEtZHBhcGktY2lwaGVy')",
        [],
    )
    .unwrap();
    let err = db::get_setting(&conn, "greader_password").expect_err("非 DPAPI 密文必须拒绝");
    assert_eq!(err.code, "credentialDecrypt");
}

/// 真实 DPAPI 写入往返：落库是 `dpapi:` 密文且不含测试明文，读取还原。
#[cfg(windows)]
#[test]
fn real_dpapi_roundtrip_keeps_secret_out_of_plaintext_at_rest() {
    let conn = fresh_conn("cred_dpapi_roundtrip");
    let secret = r#"{"backend":"gist","token":"mock-pat-not-a-real-credential"}"#;
    db::set_setting(&conn, "config_sync_credentials", secret).unwrap();

    let stored = raw_value(&conn, "config_sync_credentials").expect("落库值应存在");
    assert!(
        stored.starts_with("dpapi:"),
        "落库必须是 DPAPI 密文：{stored}"
    );
    assert!(
        !stored.contains("mock-pat-not-a-real-credential"),
        "持久化值不得包含明文"
    );
    assert_eq!(
        db::get_setting(&conn, "config_sync_credentials")
            .unwrap()
            .unwrap(),
        secret,
        "读取必须还原明文（往返成功）"
    );
}

/// 真实 DPAPI 迁移：明文升级为密文，迁移后读取还原，幂等。
#[cfg(windows)]
#[test]
fn real_dpapi_legacy_migration_upgrades_and_is_idempotent() {
    let conn = fresh_conn("cred_migrate_real");
    conn.execute(
        "INSERT INTO settings (key, value) VALUES ('miniflux_token', 'legacy-token-plain')",
        [],
    )
    .unwrap();

    assert_eq!(
        app_lib::credentials::migrate_legacy_plaintext(&conn).unwrap(),
        1
    );
    let stored = raw_value(&conn, "miniflux_token").unwrap();
    assert!(stored.starts_with("dpapi:"));
    assert!(!stored.contains("legacy-token-plain"));
    assert_eq!(
        db::get_setting(&conn, "miniflux_token").unwrap().unwrap(),
        "legacy-token-plain"
    );
    assert_eq!(
        app_lib::credentials::migrate_legacy_plaintext(&conn).unwrap(),
        0,
        "已升级后幂等，不重复计数"
    );
}
