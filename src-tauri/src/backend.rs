//! 服务端类型与协议解析（阶段 0 最小版）。
//!
//! 阶段 1 会迁入 `sync/backend.rs` 并扩充探测（probe）/能力（caps）/会话缓存；
//! 本阶段只放「决定 API base 推导规则」的最小集合，让 FreshRSS 与 Miniflux
//! 都能连上（G1/F1 路径、U1 文案）。

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// 服务端类型：决定 API base 推导规则与 UI 提示文案。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServerKind {
    Miniflux,
    FreshRss,
    Custom,
}

impl ServerKind {
    pub fn parse(s: &str) -> ServerKind {
        match s {
            "freshrss" => ServerKind::FreshRss,
            "custom" => ServerKind::Custom,
            _ => ServerKind::Miniflux,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ServerKind::Miniflux => "miniflux",
            ServerKind::FreshRss => "freshrss",
            ServerKind::Custom => "custom",
        }
    }
}

/// 同步协议：Google Reader / Fever。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    GReader,
    Fever,
}

impl Protocol {
    pub fn parse(s: &str) -> Protocol {
        match s {
            "fever" => Protocol::Fever,
            _ => Protocol::GReader,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Protocol::GReader => "greader",
            Protocol::Fever => "fever",
        }
    }
}

/// 推导 API base（用户填根地址 → 实际 API 根路径）。
pub fn resolve_api_base(kind: ServerKind, protocol: Protocol, root: &str) -> String {
    let root = root.trim_end_matches('/');
    match (kind, protocol) {
        (ServerKind::Miniflux, Protocol::GReader) => root.to_string(),
        (ServerKind::Miniflux, Protocol::Fever) => format!("{root}/fever/"),
        (ServerKind::FreshRss, Protocol::GReader) => format!("{root}/api/greader.php"),
        (ServerKind::FreshRss, Protocol::Fever) => format!("{root}/api/fever.php"),
        (ServerKind::Custom, _) => root.to_string(),
    }
}

/// 完整同步配置（凭据 + 类型 + 协议 + 推导后的 API base）。
#[derive(Debug, Clone)]
pub struct SyncConfig {
    pub kind: ServerKind,
    pub protocol: Protocol,
    pub root: String,
    pub api_base: String,
    pub username: String,
    pub password: String,
}

/// 从 settings 键读取同步配置；缺任一必要键返回 None（视为未配置）。
pub fn read_config(conn: &Connection) -> Option<SyncConfig> {
    let kind = ServerKind::parse(&db_get(conn, "sync_server_kind")?.unwrap_or_default());
    let protocol = Protocol::parse(&db_get(conn, "sync_protocol")?.unwrap_or_default());
    let root = db_get(conn, "greader_endpoint")??;
    let username = db_get(conn, "greader_username")??;
    let password = db_get(conn, "greader_password")??;
    if root.trim().is_empty() || username.trim().is_empty() || password.trim().is_empty() {
        return None;
    }
    let api_base = resolve_api_base(kind, protocol, &root);
    Some(SyncConfig {
        kind,
        protocol,
        root,
        api_base,
        username,
        password,
    })
}

/// 读取 settings 键（经 db::get_setting，敏感键自动解密）；查询失败视为缺失。
fn db_get(conn: &Connection, key: &str) -> Option<Option<String>> {
    crate::db::get_setting(conn, key).ok()
}