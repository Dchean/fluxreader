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

/// TASK-068：app_settings JSON 的类型化读取助手（收口此前 6+ 处复制的
/// get_setting → from_str → v.get 模板）。get_setting 失败 / JSON 解析失败 /
/// 字段缺失 / 类型不符一律返回 default，与各调用点的既有默认值语义一致。
pub fn app_settings_bool(conn: &Connection, key: &str, default: bool) -> bool {
    get_setting(conn, "app_settings")
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get(key).and_then(|b| b.as_bool()))
        .unwrap_or(default)
}

/// 同上，字符串字段（如 syncMode）。
pub fn app_settings_str(conn: &Connection, key: &str, default: &str) -> String {
    get_setting(conn, "app_settings")
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get(key).and_then(|m| m.as_str()).map(String::from))
        .unwrap_or_else(|| default.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::MIGRATIONS;
    use rusqlite::Connection;

    fn conn() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        conn
    }

    #[test]
    fn app_settings_helpers_return_defaults_when_unset_or_broken() {
        let conn = conn();
        // 未设置：默认值
        assert!(!app_settings_bool(&conn, "smartDedup", false));
        assert_eq!(app_settings_str(&conn, "syncMode", "direct"), "direct");
        // 坏 JSON：默认值（不 panic）
        set_setting(&conn, "app_settings", "{broken").unwrap();
        assert!(app_settings_bool(&conn, "closeToTray", true));
        assert_eq!(app_settings_str(&conn, "syncMode", "direct"), "direct");
    }

    #[test]
    fn app_settings_helpers_read_present_values() {
        let conn = conn();
        set_setting(
            &conn,
            "app_settings",
            r#"{"smartDedup":true,"syncMode":"hybrid","n":1}"#,
        )
        .unwrap();
        assert!(app_settings_bool(&conn, "smartDedup", false));
        assert_eq!(app_settings_str(&conn, "syncMode", "direct"), "hybrid");
        // 类型不符（数字当布尔）→ 默认
        assert!(app_settings_bool(&conn, "n", true));
        assert!(!app_settings_bool(&conn, "missing", false));
    }
}
