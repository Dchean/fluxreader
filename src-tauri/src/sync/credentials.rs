//! sync 的 credentials 子模块（TASK-045 从 sync.rs 按既有章节拆分）。

use crate::db;
use crate::error::{AppError, AppResult};
use crate::fever;
use crate::greader::{self, GReaderClient};
use rusqlite::Connection;

/// 后端凭据：协议 + endpoint + username + password。
/// username/password 是 Miniflux「集成」页单独配置的凭据（Google Reader 与
/// Fever 共用同一套集成凭据，非 Miniflux 账号密码）。
/// 协议从 settings 键 `sync_protocol` 读取（"greader" | "fever"，默认 "greader"）。
///
/// **三态语义（OPT-014 R1）**：
/// - `Ok(Some(..))` 已配置；
/// - `Ok(None)` 未配置（键缺失或凭据字段为空）；
/// - `Err(e)` **读取/解密失败**（如密码密文损坏）——调用方不得把它当「未配置/
///   首次连接」继续写新账号或发请求；必须按可见失败处理（失败关闭）。
pub fn read_credentials(conn: &Connection) -> AppResult<Option<(String, String, String, String)>> {
    let protocol = db::get_setting(conn, "sync_protocol")?
        .filter(|p| p == "fever" || p == "greader")
        .unwrap_or_else(|| "greader".to_string());
    let Some(endpoint) = db::get_setting(conn, "greader_endpoint")? else {
        return Ok(None);
    };
    let Some(username) = db::get_setting(conn, "greader_username")? else {
        return Ok(None);
    };
    let Some(password) = db::get_setting(conn, "greader_password")? else {
        return Ok(None);
    };
    if endpoint.trim().is_empty() || username.trim().is_empty() || password.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some((protocol, endpoint, username, password)))
}

/// 规范化「服务器根」身份：scheme + host + **有效端口** + basepath（host 小写、
/// 去尾斜杠）。只归并**等价形态**：省略端口与显式默认端口（http:80 / https:443）
/// 视为同一 server；同 host 不同端口、不同 basepath 都是不同 server。
/// 解析失败（如缺 scheme 的裸串）退回整体小写 + 去尾斜杠——保守等价。
/// （OPT-006 身份判定与 OPT-009 账号指纹共用同一实现，不得各写一份。）
pub fn normalized_server_root(endpoint: &str) -> String {
    let trimmed = endpoint.trim().trim_end_matches('/');
    match url::Url::parse(trimmed) {
        Ok(u) => {
            let host = u.host_str().unwrap_or("").to_ascii_lowercase();
            // port_or_known_default：未显式端口返回协议默认端口，显式默认端口
            // 与之相等 → 两种写法归并；非默认端口原样保留。
            let port = u
                .port_or_known_default()
                .map(|p| format!(":{p}"))
                .unwrap_or_default();
            let path = u.path().trim_end_matches('/');
            format!(
                "{}://{}{}{}",
                u.scheme().to_ascii_lowercase(),
                host,
                port,
                path
            )
        }
        Err(_) => trimmed.to_ascii_lowercase(),
    }
}

/// 账号指纹（OPT-009）：协议 + 规范化服务器根 + 用户名。**持久身份，跨代际
/// 不变**——订阅 outbox 行以此做账号屏障：key 匹配才可发，key 不匹配暂停。
pub fn account_key(protocol: &str, endpoint: &str, username: &str) -> String {
    format!("{protocol}|{}|{username}", normalized_server_root(endpoint))
}

/// 当前账号身份 + 库代际的同一快照（锁内读取）。未配置 → Ok(None)；
/// 凭据读取/解密失败 → Err（失败关闭，不得当未配置）。
pub struct AccountContext {
    pub key: String,
    pub generation: i64,
}

pub fn current_account(conn: &Connection) -> AppResult<Option<AccountContext>> {
    let Some((protocol, endpoint, username, _)) = read_credentials(conn)? else {
        return Ok(None);
    };
    Ok(Some(AccountContext {
        key: account_key(&protocol, &endpoint, &username),
        generation: db::sync_generation(conn)?,
    }))
}

/// build_client 失败的三种形态（TASK-124，审计 P2-6②；OPT-006 增 Stale）。
/// 此前 build_client 返回 Option<Backend>，把「未配置」与「配置了但认证/端点/
/// 网络失败」都压成 None——调用方无从区分，后者（attempts/last_error）永不记录。
pub(super) enum ClientBuildFailure {
    /// 无凭据或凭据为空：未配置同步。调用方静默跳过（A-5 既有语义：
    /// 队列保留，连接后补推；不发事件不报警）。
    NotConfigured,
    /// 有凭据但构建失败（认证被拒 / 端点解析不到 / 网络不可达）。
    /// 带实际错误——调用方据此记录阻塞标记并发 sync-queue-changed。
    Failed(AppError),
    /// OPT-006：登录/探测期间账号配置已变更（代际失配）。既非身份错误也不该
    /// 记录阻塞标记——调用方直接丢弃本轮（不落库、不发请求）。
    Stale,
    /// OPT-006 R1：代际键**现存但非法/负数/溢出**——可见的协议错误：不得按 0
    /// 与旧会话假匹配，也不得记录推送阻塞标记（与队列无关）；调用方原样上报。
    CorruptGeneration(AppError),
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

    /// 协议能力声明（OPT-009）：是否支持订阅写回（新增/改名/移动/退订）。
    /// GReader 有 subscription/edit 端点；Fever 协议没有写订阅端点——调用方
    /// **必须**据此把队项声明为 local-only，不得用 Ok 冒充远端完成。
    pub(super) fn supports_subscription_write(&self) -> bool {
        matches!(self, Backend::GReader(_))
    }

    /// 订阅编辑（改名 `t` / 移动分类 `a`+`r`）：GReader 走 ac=edit。
    /// Fever 协议无订阅编辑端点——返回**明确错误**（此前返回 Ok(()) 会冒充
    /// 「远端已完成」，用户以为改到了远端）；执行器在调用前用能力声明拦下，
    /// 此错误只作为绕过能力门时的最后防线。
    pub(super) async fn edit_subscription(
        &self,
        remote_id: i64,
        title: Option<&str>,
        dest_label: Option<&str>,
        remove_label: Option<&str>,
    ) -> AppResult<()> {
        match self {
            Backend::GReader(c) => {
                c.edit_subscription(remote_id, title, dest_label, remove_label)
                    .await
            }
            Backend::Fever(_) => Err(AppError::new(
                "unsupported",
                "Fever 协议不支持订阅编辑（本地已生效，远端不宣称完成）",
            )),
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

    /// 退订远端订阅：仅 GReader 有端点；Fever 明确错误（不冒充完成）。
    pub(super) async fn unsubscribe(&self, feed_numeric_id: i64) -> AppResult<()> {
        match self {
            Backend::GReader(c) => c.unsubscribe(feed_numeric_id).await,
            Backend::Fever(_) => Err(AppError::new(
                "unsupported",
                "Fever 协议不支持退订订阅（本地已生效，远端不宣称完成）",
            )),
        }
    }
}

/// 锁内读凭据 → 锁外按协议构建 client。
///
/// OPT-006：原 `build_client` 的构建逻辑已收编进 [`super::session::build_session`]
/// ——会话在同一短临界区捕获代际，并在写端点缓存前复核代际（HTTP→DB 回写边界）。
/// 订阅编辑/退订等不需要会话身份的路径经 `session::build_session` 取 `client`。
#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::MIGRATIONS.to_latest(&mut conn).unwrap();
        conn
    }

    /// R1 P2 三态语义：未配置 → Ok(None)；完整 → Ok(Some)；**损坏密文 → Err**
    /// （不得吞成 None 让调用方当「首次连接」继续写新账号）。
    #[test]
    fn read_credentials_distinguishes_not_configured_from_read_error() {
        let conn = conn();
        assert!(matches!(read_credentials(&conn), Ok(None)), "空库 = 未配置");

        db::set_setting(&conn, "greader_endpoint", "http://example.com").unwrap();
        db::set_setting(&conn, "greader_username", "u").unwrap();
        assert!(
            matches!(read_credentials(&conn), Ok(None)),
            "缺密码 = 未配置"
        );

        db::set_setting(&conn, "greader_password", "pw").unwrap();
        let c = read_credentials(&conn).unwrap().unwrap();
        assert_eq!(
            (c.1.as_str(), c.2.as_str(), c.3.as_str()),
            ("http://example.com", "u", "pw")
        );

        // 损坏密文（裸 SQL 写入，绕过写入加密）：Err 而非 Ok(None)
        conn.execute(
            "UPDATE settings SET value = 'dpapi:@@corrupt@@' WHERE key = 'greader_password'",
            [],
        )
        .unwrap();
        let err = read_credentials(&conn).expect_err("损坏密文必须返回可见错误");
        assert!(
            err.code == "credentialCorrupt" || err.code == "credentialDecrypt",
            "错误码应指向解密失败：{err}"
        );
    }
}
