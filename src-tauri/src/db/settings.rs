use super::*;
use rusqlite::params;

pub fn get_setting(conn: &Connection, key: &str) -> AppResult<Option<String>> {
    let v: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![key],
            |r| r.get(0),
        )
        .optional()?;
    match v {
        // 敏感键读时解密（SEC-2）；历史明文无前缀则原样返回（兼容）。
        // OPT-014 / F22：密文损坏/DPAPI 失败返回可见错误——绝不把 `dpapi:`
        // 原串当已解密值交给调用方（那会把密文当密码发给服务端）。
        Some(raw) if crate::credentials::is_sensitive_key(key) => {
            Ok(Some(crate::credentials::decrypt_secret(&raw)?))
        }
        Some(raw) => Ok(Some(raw)),
        None => Ok(None),
    }
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> AppResult<()> {
    set_setting_with(conn, key, value, crate::credentials::encrypt_secret)
}

/// 写入核心（可注入加密器；同模块单测用它模拟加密失败；账号提交事务内核
/// 经 `db::set_setting_with` 复用它把敏感写入并入同一事务）。
///
/// OPT-014 / F22 失败关闭：敏感键加密失败必须整体 Err——SQL 尚未执行，
/// 旧值原样保留，任何路径都不落明文。注入助手不是 IPC 命令：生产命令面
/// 无加密后端参数，webview 没有可操控的后门。
///
/// Note: 失败关闭边界见 .agents/notes/implemented/architecture/2026-10-08-凭据失败关闭与受控更新检查.md
pub(crate) fn set_setting_with(
    conn: &Connection,
    key: &str,
    value: &str,
    encrypt: fn(&str) -> AppResult<String>,
) -> AppResult<()> {
    // 敏感键写时加密（SEC-2）：DPAPI 加密后落库，读 DB 不见明文
    let stored = if crate::credentials::is_sensitive_key(key) {
        encrypt(value)?
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

    /// OPT-014 / F22：加密失败必须失败关闭——保存返回 Err、旧值原样、缺行不插入
    /// （任何路径都不落明文）。
    #[test]
    fn set_setting_propagates_encrypt_failure_and_leaves_db_untouched() {
        let conn = conn();
        set_setting(&conn, "greader_password", "old-valid-secret").unwrap();
        let before: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key='greader_password'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        let err = set_setting_with(
            &conn,
            "greader_password",
            "new-secret-must-not-land",
            |_| {
                Err(crate::error::AppError::new(
                    "credentialEncrypt",
                    "注入的加密失败",
                ))
            },
        )
        .expect_err("加密失败必须返回 Err，而不是静默写入");
        assert_eq!(err.code, "credentialEncrypt");
        let after: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key='greader_password'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(before, after, "失败后旧值必须原样保留");
        assert!(
            !after.contains("new-secret-must-not-land"),
            "DB 不得出现待保存明文"
        );

        // 缺行场景：失败不得插入任何行（尤其明文）
        let err2 = set_setting_with(&conn, "miniflux_token", "also-must-not-land", |_| {
            Err(crate::error::AppError::new(
                "credentialEncrypt",
                "注入的加密失败",
            ))
        });
        assert!(err2.is_err());
        let exists: Option<String> = conn
            .query_row(
                "SELECT value FROM settings WHERE key='miniflux_token'",
                [],
                |r| r.get(0),
            )
            .optional()
            .unwrap();
        assert!(exists.is_none(), "失败不得写入新行");

        // 对照：同轴注入成功加密器时正常写入（不误伤保存路径）
        set_setting_with(&conn, "miniflux_token", "next-token", |v| {
            Ok(format!("dpapi:test[{v}]"))
        })
        .unwrap();
        let stored: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key='miniflux_token'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, "dpapi:test[next-token]");
    }
}
