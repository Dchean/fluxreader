//! sync 的 credentials 子模块（TASK-045 从 sync.rs 按既有章节拆分）。

use crate::db;
use crate::error::{AppError, AppResult};
use crate::fever;
use crate::greader::{self, GReaderClient};
use rusqlite::Connection;
use std::sync::Arc;
use tokio::sync::Mutex;

/// 后端凭据：协议 + endpoint + username + password。
/// username/password 是 Miniflux「集成」页单独配置的凭据（Google Reader 与
/// Fever 共用同一套集成凭据，非 Miniflux 账号密码）。
/// 协议从 settings 键 `sync_protocol` 读取（"greader" | "fever"，默认 "greader"）。
pub fn read_credentials(conn: &Connection) -> Option<(String, String, String, String)> {
    let protocol = db::get_setting(conn, "sync_protocol")
        .ok()
        .flatten()
        .filter(|p| p == "fever" || p == "greader")
        .unwrap_or_else(|| "greader".to_string());
    let endpoint = db::get_setting(conn, "greader_endpoint").ok().flatten()?;
    let username = db::get_setting(conn, "greader_username").ok().flatten()?;
    let password = db::get_setting(conn, "greader_password").ok().flatten()?;
    if endpoint.trim().is_empty() || username.trim().is_empty() || password.trim().is_empty() {
        return None;
    }
    Some((protocol, endpoint, username, password))
}

/// 协议无关后端客户端（Google Reader / Fever）。
/// sync 引擎只依赖这个枚举的统一方法，协议差异封装在内部。
pub enum Backend {
    GReader(GReaderClient),
    Fever(fever::FeverClient),
}

impl Backend {
    pub(super) async fn subscriptions(&self) -> AppResult<Vec<greader::Subscription>> {
        match self {
            Backend::GReader(c) => c.subscriptions().await,
            Backend::Fever(c) => c.subscriptions().await,
        }
    }

    /// 订阅编辑（改名 `t` / 移动分类 `a`）：GReader 走 ac=edit；
    /// Fever 协议无订阅编辑端点，视为已完成（本地已生效、无从推送）。
    pub(super) async fn edit_subscription(
        &self,
        remote_id: i64,
        title: Option<&str>,
        dest_label: Option<&str>,
    ) -> AppResult<()> {
        match self {
            Backend::GReader(c) => c.edit_subscription(remote_id, title, dest_label).await,
            Backend::Fever(_) => Ok(()),
        }
    }

    pub(super) async fn tags(&self) -> AppResult<Vec<greader::TagRef>> {
        match self {
            Backend::GReader(c) => c.tags().await,
            Backend::Fever(c) => c.tags().await,
        }
    }

    pub(super) async fn mark_read(&self, ids: &[i64]) -> AppResult<()> {
        match self {
            Backend::GReader(c) => c.mark_read(ids).await,
            Backend::Fever(c) => c.mark_read(ids).await,
        }
    }

    pub(super) async fn mark_unread(&self, ids: &[i64]) -> AppResult<()> {
        match self {
            Backend::GReader(c) => c.mark_unread(ids).await,
            Backend::Fever(c) => c.mark_unread(ids).await,
        }
    }

    pub(super) async fn mark_starred(&self, ids: &[i64]) -> AppResult<()> {
        match self {
            Backend::GReader(c) => c.mark_starred(ids).await,
            Backend::Fever(c) => c.mark_starred(ids).await,
        }
    }

    pub(super) async fn mark_unstarred(&self, ids: &[i64]) -> AppResult<()> {
        match self {
            Backend::GReader(c) => c.mark_unstarred(ids).await,
            Backend::Fever(c) => c.mark_unstarred(ids).await,
        }
    }

    /// 订阅新源。Fever 协议无写订阅端点，降级为明确错误。
    pub(super) async fn quick_add(&self, url: &str) -> AppResult<greader::QuickAddResponse> {
        match self {
            Backend::GReader(c) => c.quick_add(url).await,
            Backend::Fever(_) => Err(AppError::new(
                "unsupported",
                "Fever 协议不支持添加订阅，请在 Miniflux Web 端添加后重新同步",
            )),
        }
    }
}

/// 锁内读凭据 → 锁外按协议构建 client。
pub(super) async fn build_client(
    db: &Arc<Mutex<Connection>>,
    http: &reqwest::Client,
) -> Option<Backend> {
    let (protocol, endpoint, username, password) = {
        let conn = db.lock().await;
        read_credentials(&conn)?
    };
    match protocol.as_str() {
        "fever" => {
            let client = fever::FeverClient::new(&endpoint, &username, &password, http.clone());
            match client.verify().await {
                Ok(()) => Some(Backend::Fever(client)),
                Err(e) => {
                    log::warn!("sync: Fever 认证失败: {e}");
                    None
                }
            }
        }
        _ => match GReaderClient::login(&endpoint, &username, &password, http.clone()).await {
            Ok(c) => Some(Backend::GReader(c)),
            Err(e) => {
                log::warn!("sync: ClientLogin 失败: {e}");
                None
            }
        },
    }
}
