//! 统一错误类型：IPC 序列化友好（code + message），避免把 Rust 错误栈泄给前端。

use serde::Serialize;
use std::error::Error as _;

#[derive(Debug, Serialize)]
pub struct AppError {
    pub code: String,
    pub message: String,
}

impl AppError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.to_string(), message: message.into() }
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
