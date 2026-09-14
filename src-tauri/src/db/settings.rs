use super::*;
use rusqlite::params;

pub fn get_setting(conn: &Connection, key: &str) -> AppResult<Option<String>> {
    let v = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![key],
            |r| r.get(0),
        )
        .optional()?;
    // 敏感键读时解密（SEC-2）；历史明文无前缀则原样返回（兼容）
    Ok(v.map(|raw: String| {
        if crate::credentials::is_sensitive_key(key) {
            crate::credentials::decrypt_secret(&raw)
        } else {
            raw
        }
    }))
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> AppResult<()> {
    // 敏感键写时加密（SEC-2）：DPAPI 加密后落库，读 DB 不见明文
    let stored = if crate::credentials::is_sensitive_key(key) {
        crate::credentials::encrypt_secret(value)
    } else {
        value.to_string()
    };
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, stored],
    )?;
    Ok(())
}

/* ============================================================
后端同步 —— id 映射 + 离线变更队列（Google Reader / Fever）
============================================================ */
