//! 统一错误类型：IPC 序列化友好（code + message），避免把 Rust 错误栈泄给前端。

use serde::Serialize;
use std::error::Error as _;

#[derive(Debug, Serialize)]
pub struct AppError {
    pub code: String,
    pub message: String,
    /// 认证失败结构化标记（会话缓存 401/403 回落的判据）。构造点：greader.rs /
    /// fever.rs 的 401/403 状态码错误与 Fever api_key 失败（[`AppError::auth`] /
    /// [`AppError::auth_if`]）。不进 IPC 序列化、不影响 Display——对外形状
    /// 逐字不变；回落判定读标记不读文案（URL 类错误伪造「→ 401」文案的
    /// 串匹配口子就此关闭）。
    #[serde(skip)]
    is_auth: bool,
}

impl AppError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            is_auth: false,
        }
    }

    /// 认证失败标记（无条件）：Fever `auth != 1` 等**本身就是认证失败**的
    /// 构造点用。
    pub fn auth(mut self) -> Self {
        self.is_auth = true;
        self
    }

    /// 按条件打认证标记：HTTP 状态码构造点配 [`AppError::is_auth_status`] 用。
    pub fn auth_if(mut self, is_auth: bool) -> Self {
        self.is_auth = is_auth;
        self
    }

    /// HTTP 状态码是否属认证类失败（401/403）——构造点打标记的统一判据。
    pub fn is_auth_status(status: u16) -> bool {
        status == 401 || status == 403
    }

    /// 是否认证类失败（会话回落判据；Display 与序列化形状不受影响）。
    pub fn is_auth_failure(&self) -> bool {
        self.is_auth
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new("internal", message)
    }

    pub fn network(message: impl Into<String>) -> Self {
        Self::new("network", message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new("notFound", message)
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for AppError {}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        AppError::new("db", e.to_string())
    }
}

impl From<rusqlite_migration::Error> for AppError {
    fn from(e: rusqlite_migration::Error) -> Self {
        AppError::new("migration", e.to_string())
    }
}

impl From<reqwest::Error> for AppError {
    fn from(e: reqwest::Error) -> Self {
        // 携带底层原因链（DNS/TLS/超时），同步中心排查抓取失败需要根因
        let mut msg = e.to_string();
        let mut src = e.source();
        while let Some(s) = src {
            msg.push_str(" ← ");
            msg.push_str(&s.to_string());
            src = s.source();
        }
        AppError::network(msg)
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::new("json", e.to_string())
    }
}

impl From<feed_rs::parser::ParseFeedError> for AppError {
    fn from(e: feed_rs::parser::ParseFeedError) -> Self {
        AppError::new("parse", e.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod auth_marker_tests {
    use super::*;

    /// 401 回落结构化（2026-11-02）：标记只由构造点设置；Display 与 IPC
    /// 序列化形状不受标记影响（前端契约与错误文案逐字不变）。
    #[test]
    fn auth_marker_is_structural_and_invisible_on_wire() {
        // 无条件构造器（Fever api_key 失败形态）
        let fever = AppError::new("auth", "Fever 认证失败（api_key 不正确）").auth();
        assert!(fever.is_auth_failure());

        // 条件构造器：401/403 打标，其它状态码与普通网络错误不打
        assert!(AppError::network("GET /x → 401 Unauthorized")
            .auth_if(AppError::is_auth_status(401))
            .is_auth_failure());
        assert!(AppError::network("POST /y → 403 Forbidden")
            .auth_if(AppError::is_auth_status(403))
            .is_auth_failure());
        assert!(!AppError::is_auth_status(400));
        assert!(!AppError::is_auth_status(500));
        assert!(!AppError::network("GET /x → 500 Internal Server Error")
            .auth_if(AppError::is_auth_status(500))
            .is_auth_failure());
        assert!(!AppError::network("连接超时").is_auth_failure());

        // Display 与序列化形状逐字不变
        let marked = AppError::network("GET /x → 401 Unauthorized").auth();
        assert_eq!(marked.to_string(), "[network] GET /x → 401 Unauthorized");
        let wire = serde_json::to_value(&marked).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({"code": "network", "message": "GET /x → 401 Unauthorized"}),
            "is_auth 不进 IPC 序列化（前端契约不变）"
        );
    }
}
