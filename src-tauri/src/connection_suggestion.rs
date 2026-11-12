//! Non-secret connection suggestions; no Gist/configuration-sync transport.
use crate::{
    db,
    error::{AppError, AppResult},
};
use serde::{Deserialize, Serialize};
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
pub struct ConnectionConfig {
    pub sync_protocol: Option<String>,
    pub greader_endpoint: Option<String>,
    pub greader_username: Option<String>,
}

impl ConnectionConfig {
    /// 是否含任一非空字段（空建议不落库）。
    pub fn has_any_value(&self) -> bool {
        [
            &self.sync_protocol,
            &self.greader_endpoint,
            &self.greader_username,
        ]
        .into_iter()
        .any(|v| v.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false))
    }
}

/// 配置导入的待确认连接建议（settings，非敏感：协议/地址/用户名，**无密码**）。
/// 导入只写这条建议，不改活动凭据/绑定；激活必须经 sync_save 专用保存流程
/// （重新输入凭据 + 携带本建议的 version 做 CAS），提交成功后同一事务消费本键。
/// 内容改变或清空后重导递增 `version`；同值重复导入沿用同一建议 token。
// Note: 导入不直接应用连接配置、激活 CAS 与版本语义 — 见 .agents/notes/implemented/architecture/2026-10-08-账号会话与配置应用边界.md
pub(crate) const PENDING_CONNECTION_KEY: &str = "pending_connection_config";

/// 待确认建议的独立发行序号（settings，非敏感）。
/// R2 ③：token 由**持久单调 serial** 发行，清空建议（放弃/已消费）**只清内容、
/// 不重置发行器**——否则「清空后重导」会从 1 重新发行，与历史在途 token 撞车
/// （ABA：旧 token 命中新建议）。serial 只增不减（i64→u64 checked_add，溢出 Err）。
pub(crate) const PENDING_SERIAL_KEY: &str = "pending_connection_serial";

/// 待确认连接建议的落库形态：版本号 + 非敏感连接字段。
/// 旧版本（无 `version` 字段的平铺 JSON）反序列化为 version 0（兼容，不代表"从未导入"）。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PendingSuggestion {
    /// 每次导入写入递增；激活提交用它与当前值做 CAS。
    #[serde(default)]
    pub version: u64,
    #[serde(flatten)]
    pub connection: ConnectionConfig,
}

/// 读待确认连接建议（空串 = 无建议；损坏 JSON → 显式错误，不静默当无）。
pub fn read_pending_suggestion(
    conn: &rusqlite::Connection,
) -> AppResult<Option<PendingSuggestion>> {
    let Some(raw) = db::get_setting(conn, PENDING_CONNECTION_KEY)? else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    serde_json::from_str::<PendingSuggestion>(trimmed)
        .map(Some)
        .map_err(|e| {
            AppError::internal(format!(
                "待确认连接建议损坏（{PENDING_CONNECTION_KEY}）: {e}"
            ))
        })
}

/// 读待确认建议的发行序号（缺失 = 0 从未发行；现存但非法/负/溢出 = Err，
/// 不按 0 处理——那会让 serial 回退、重新发行历史 token）。
fn read_pending_serial(conn: &rusqlite::Connection) -> AppResult<u64> {
    let Some(raw) = db::get_setting(conn, PENDING_SERIAL_KEY)? else {
        return Ok(0);
    };
    raw.trim().parse::<u64>().map_err(|_| {
        AppError::internal(format!(
            "待确认建议发行序号损坏（{PENDING_SERIAL_KEY}={raw:?}）：拒绝继续，请排查/修复"
        ))
    })
}

/// 写待确认连接建议（导入事务内调用），返回本次 token（版本号）：
/// - **同值重复导入 = ignore**：不换 token（内容一致，旧 token 消费的是同一建议）；
/// - 换值 / 清空后重导 = 由独立单调 serial 发行**新 token**（绝不回退复用，
///   即使 `pending_connection_config` 被清空过——防 ABA）；
/// - serial 与内容写由调用方的导入事务捆在一起（`&tx`）：一起提交/一起回滚；
///   `checked_add` 溢出 Err（不饱和复用），serial 先写、内容后写——中途失败只留
///   「已用号段」的空洞，不会出现同号两建议。
pub fn store_pending_connection(
    conn: &rusqlite::Connection,
    cc: &ConnectionConfig,
) -> AppResult<u64> {
    if let Some(existing) = read_pending_suggestion(conn)? {
        if existing.connection == *cc {
            return Ok(existing.version); // 同值 ignore：不刷新 token
        }
    }
    let version = read_pending_serial(conn)?.checked_add(1).ok_or_else(|| {
        AppError::internal(format!(
            "待确认建议发行序号溢出（{PENDING_SERIAL_KEY} 已达 u64::MAX）：拒绝继续"
        ))
    })?;
    db::set_setting(conn, PENDING_SERIAL_KEY, &version.to_string())?;
    let suggestion = PendingSuggestion {
        version,
        connection: cc.clone(),
    };
    db::set_setting(
        conn,
        PENDING_CONNECTION_KEY,
        &serde_json::to_string(&suggestion)?,
    )?;
    Ok(version)
}

/// 清除待确认连接建议（仅专用激活提交 CAS 命中 / 用户放弃时调用）。
/// R2 ③：**只清内容，不触碰发行 serial**——历史 token 永不复用（防 ABA）。
pub fn clear_pending_connection(conn: &rusqlite::Connection) -> AppResult<()> {
    db::set_setting(conn, PENDING_CONNECTION_KEY, "")
}
