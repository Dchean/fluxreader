//! sync 的 session 子模块（OPT-006 / F05、F06）：同步会话与代际守卫。
//!
//! ## 问题
//!
//! 同步流程是「短锁读凭据/映射 → 锁外 HTTP → 再持锁落库」。断开或切换账号
//! 只清理**当时**的数据库，对已经发出的请求没有失效机制——旧响应随后返回，
//! 再次持锁就把旧账号的数据（订阅/绑定/游标/队列确认）写回本地。
//!
//! ## 机制
//!
//! `sync_generation`（settings，每库独立）在账号提交（`sync_save`）与断开
//! （`sync_disconnect`）的**同一事务**里 +1。每个同步会话 [`SyncSession`] 在
//! 同一个短 DB 临界区里同时捕获「凭据 + 代际」，此后每个 HTTP→DB 回写边界
//! （端点缓存、推送计划/确认/失败标记、建分类/绑定/删除、正文合并、状态对账、
//! 两类游标推进）都必须在**同一次持锁内**复核代际仍相等，否则丢弃本轮结果。
//!
//! 纪律：
//! - HTTP 绝不跨 DB 锁；每次代际复核都是一个新的短临界区。
//! - 代际是**每库身份**，不是全局原子计数——两个测试库互不影响。
//! - 会话不携带、也不日志输出密码（凭据只在构建 client 的一次性路径里使用）。
//! - 已发出的请求不能事后撤回：守卫只阻止**本地回写与后续请求**，不宣称能
//!   取消服务端（见任务卡「已发给旧账号的请求不能被事后撤回」）。
// Note: 会话/代际边界与配置应用语义 — 见 .agents/notes/implemented/architecture/2026-10-08-账号会话与配置应用边界.md

use super::credentials::{read_credentials, Backend, ClientBuildFailure};
use crate::db;
use crate::error::{AppError, AppResult};
use rusqlite::Connection;
use std::sync::Arc;
use tokio::sync::Mutex;

/// 一次同步会话：代际 + 已完成认证的后端 client。
/// 代际不等于「凭据内容」——同账号仅改密码也会推进代际，使旧请求失效
/// （设计边界：token 更新可换代际但保留同账号数据）。
pub(super) struct SyncSession {
    /// 捕获时的库代际；所有回写边界据此复核。
    pub(super) generation: i64,
    /// 已认证客户端（GReader / Fever）。
    pub(super) client: Backend,
}

impl SyncSession {
    /// 代际失配的可见错误（调用方据此丢弃本轮结果、不落任何库）。
    pub(super) fn stale_error() -> AppError {
        AppError::new(
            "staleSession",
            "同步会话已失效（账号配置已在其他窗口变更），本轮结果已丢弃",
        )
    }

    /// 同一次持锁内复核代际是否仍匹配（调用方须已持 DB 锁）。
    pub(super) fn is_current(&self, conn: &Connection) -> AppResult<bool> {
        Ok(db::sync_generation(conn)? == self.generation)
    }

    /// 同上，失配即 `Err(staleSession)`。
    pub(super) fn ensure_current(&self, conn: &Connection) -> AppResult<()> {
        if self.is_current(conn)? {
            Ok(())
        } else {
            Err(Self::stale_error())
        }
    }
}

/// 便捷复核（自持短锁）：HTTP 批次之间调用，返回 false = 会话已失效。
pub(super) async fn session_is_current(db: &Arc<Mutex<Connection>>, session: &SyncSession) -> bool {
    let conn = db.lock().await;
    session.is_current(&conn).unwrap_or(false)
}

/// 锁内读凭据 + 捕获代际 → 锁外按协议构建 client → 再锁内复核代际后才写
/// 端点解析缓存。
///
/// **端点解析结果走缓存（TASK-059）**：`build_session` 在每次同步
/// （feeds/states/订阅/推送）都会被调用，若不缓存就会**每轮都重复探测**。
/// 命中缓存时直接使用上次解析出的 API 根（候选收敛为唯一地址，不再发探测请求）；
/// 未命中才探测，并把结果落库——落库前必须复核代际，否则旧账号登录期间结束后
/// 会把旧端点写进缓存（F06 的一个回写边界）。
///
/// 失败两种形态（TASK-124）：未配置（静默跳过）/ 构建失败（记录阻塞）+ 新增
/// 代际失配（`Stale`：既不是身份问题也不记录阻塞，调用方直接丢弃本轮）。
pub(super) async fn build_session(
    db: &Arc<Mutex<Connection>>,
    http: &reqwest::Client,
) -> Result<SyncSession, ClientBuildFailure> {
    let (protocol, endpoint, username, password, generation) = {
        let conn = db.lock().await;
        // OPT-014 R1：读取/解密失败不是「未配置」——按可重试失败上报。
        let credentials = match read_credentials(&conn) {
            Ok(Some(c)) => c,
            Ok(None) => return Err(ClientBuildFailure::NotConfigured),
            Err(e) => return Err(ClientBuildFailure::Failed(e)),
        };
        // 同一次持锁内捕获代际（与凭据同源快照）。R1：现存非法代际是**可见的
        // 协议错误**（不按 0 与会话假匹配），单列 CorruptGeneration 交给调用方。
        let generation = match db::sync_generation(&conn) {
            Ok(g) => g,
            Err(e) => return Err(ClientBuildFailure::CorruptGeneration(e)),
        };
        (
            credentials.0,
            credentials.1,
            credentials.2,
            credentials.3,
            generation,
        )
    };
    let cached = {
        let conn = db.lock().await;
        crate::endpoint_resolve::cached_base(&conn, &protocol, &endpoint)
    };

    // 两条路径都产出「已认证 + 端点已确定」的客户端：
    // 命中缓存 ⇒ 候选收敛为唯一地址，这一次请求只做认证（不探测）；
    // 未命中 ⇒ 客户端自己探测，探测成功即已认证，无需再登录一次。
    let (backend, resolved) = match protocol.as_str() {
        "fever" => {
            let probe =
                crate::fever::FeverClient::new(&endpoint, &username, &password, http.clone());
            let client = match &cached {
                Some(base) => {
                    let client = probe.at_resolved(base);
                    match client.verify().await {
                        Ok(()) => client,
                        Err(e) => {
                            log::warn!("sync: Fever 认证失败: {e}");
                            return Err(ClientBuildFailure::Failed(e));
                        }
                    }
                }
                None => match probe.resolve().await {
                    Ok(client) => client,
                    Err(e) => {
                        log::warn!("sync: Fever 端点解析/认证失败: {e}");
                        return Err(ClientBuildFailure::Failed(e));
                    }
                },
            };
            let base = client.resolved_base().to_string();
            (Backend::Fever(client), base)
        }
        _ => {
            let logged_in = match &cached {
                Some(base) => {
                    crate::greader::GReaderClient::login_resolved(
                        base,
                        &username,
                        &password,
                        http.clone(),
                    )
                    .await
                }
                None => {
                    crate::greader::GReaderClient::login(
                        &endpoint,
                        &username,
                        &password,
                        http.clone(),
                    )
                    .await
                }
            };
            match logged_in {
                Ok(client) => {
                    let base = client.resolved_base().to_string();
                    (Backend::GReader(client), base)
                }
                Err(e) => {
                    log::warn!("sync: ClientLogin 失败: {e}");
                    return Err(ClientBuildFailure::Failed(e));
                }
            }
        }
    };

    // 登录/探测期间账号可能已被切换：只有代际仍匹配才允许落缓存与返回会话。
    {
        let conn = db.lock().await;
        let current = match db::sync_generation(&conn) {
            Ok(g) => g,
            Err(e) => return Err(ClientBuildFailure::CorruptGeneration(e)),
        };
        if current != generation {
            log::warn!(
                "sync: 构建客户端期间账号配置已变更（代际 {generation} → {current}），丢弃本次会话"
            );
            return Err(ClientBuildFailure::Stale);
        }
        // 只有「这一轮真的探测过」才落库；命中缓存时无需重复写。
        if cached.is_none() {
            if let Err(e) =
                crate::endpoint_resolve::remember_base(&conn, &protocol, &endpoint, &resolved)
            {
                log::warn!("sync: 端点解析结果落库失败（不影响本次同步）: {e}");
            }
        }
    }
    Ok(SyncSession {
        generation,
        client: backend,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::MIGRATIONS.to_latest(&mut conn).unwrap();
        conn
    }

    /// OPT-006 守卫单元锁定：代际推进即旧会话失效；每库独立（两个库互不影响）。
    #[test]
    fn session_guard_flips_on_generation_bump_and_is_per_database() {
        let conn_a = conn();
        let conn_b = conn();
        assert_eq!(db::sync_generation(&conn_a).unwrap(), 0, "初始代际为 0");

        let session = SyncSession {
            generation: 0,
            // 守卫测试不触网：用 GReader 空客户端占位即可（不调用其方法）。
            client: Backend::GReader(crate::greader::GReaderClient::new(
                "http://127.0.0.1:1",
                "t",
                reqwest::Client::new(),
            )),
        };
        assert!(session.is_current(&conn_a).unwrap());
        session.ensure_current(&conn_a).unwrap();

        // 另一个库推进代际不影响本库
        db::bump_sync_generation(&conn_b).unwrap();
        assert!(session.is_current(&conn_a).unwrap(), "每库代际独立");
        assert_eq!(db::sync_generation(&conn_b).unwrap(), 1);

        // 本库推进 → 旧会话失效，错误码明确
        db::bump_sync_generation(&conn_a).unwrap();
        assert!(!session.is_current(&conn_a).unwrap());
        let err = session
            .ensure_current(&conn_a)
            .expect_err("代际变化必须失效");
        assert_eq!(err.code, "staleSession");
    }

    /// R2 ②：代际**只有缺失**按 0（旧库未初始化）；现存空串/空白也是 Err——
    /// 「有键但不可解析」属于状态损坏，按 0 会与旧代际 0 的会话假匹配。
    /// bump 用 checked_add：到 i64::MAX 时 Err 而非饱和复用，且不改写现场。
    #[test]
    fn generation_missing_is_zero_and_corrupt_is_error() {
        let conn = conn();
        assert_eq!(db::sync_generation(&conn).unwrap(), 0, "缺失 = 0");

        for bad in ["", "  ", "not-a-number", "-1", "1.5", "9223372036854775808"] {
            db::set_setting(&conn, "sync_generation", bad).unwrap();
            let err =
                db::sync_generation(&conn).expect_err("现存不可解析代际必须 Err（不得按 0 处理）");
            assert_eq!(err.code, "protocol", "{bad:?} 应返回协议错误");
            assert!(err.message.contains("sync_generation"), "{err}");
            // 损坏现场不得被 bump 掩盖成 1：推进同样必须 Err 且不改写
            assert!(db::bump_sync_generation(&conn).is_err());
            let raw = db::get_setting(&conn, "sync_generation").unwrap();
            assert_eq!(raw.as_deref(), Some(bad), "损坏值不得被覆写");
        }

        // 合法边界：i64::MAX 可读；推进必须 checked_add 溢出 Err（不是饱和复用）
        db::set_setting(&conn, "sync_generation", &i64::MAX.to_string()).unwrap();
        assert_eq!(db::sync_generation(&conn).unwrap(), i64::MAX);
        let overflow = db::bump_sync_generation(&conn).expect_err("溢出必须 Err，不得饱和");
        assert_eq!(overflow.code, "protocol");
        assert_eq!(
            db::get_setting(&conn, "sync_generation")
                .unwrap()
                .as_deref(),
            Some(i64::MAX.to_string().as_str()),
            "溢出失败不得改写代际"
        );

        // 对照：缺失态推进正常 +1
        let mut fresh = Connection::open_in_memory().unwrap();
        crate::db::MIGRATIONS.to_latest(&mut fresh).unwrap();
        assert_eq!(db::bump_sync_generation(&fresh).unwrap(), 1);
        assert_eq!(db::bump_sync_generation(&fresh).unwrap(), 2);
    }
}
