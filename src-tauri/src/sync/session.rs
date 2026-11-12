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
use super::SyncReport;
use crate::db;
use crate::error::{AppError, AppResult};
use rusqlite::Connection;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
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

    /// The original HTTP generation can cross the blocking boundary by value;
    /// never reacquire a new session to authorize a late result.
    pub(super) fn ensure_generation(conn: &Connection, generation: i64) -> AppResult<()> {
        if db::sync_generation(conn)? == generation {
            Ok(())
        } else {
            Err(Self::stale_error())
        }
    }
}

/// 便捷复核（自持短锁）：HTTP 批次之间调用，返回 false = 会话已失效。
pub(super) async fn session_is_current(db: &Arc<Mutex<Connection>>, session: &SyncSession) -> bool {
    let generation = session.generation;
    crate::state::database_work(db.clone(), move |conn| {
        SyncSession::ensure_generation(conn, generation)
    })
    .await
    .is_ok()
}

/* ============================================================
W3-1：会话按「账号身份 + 代际」缓存复用（每次同步/即时推送免重复认证）
============================================================

形态说明：缓存是**进程级单槽**（应用同一时刻只有一个活动账号；代际推进即整
体失效），挂在 sync::session 的静态上——与 PULL_LOCK / PUSH_LOCK /
QUEUE_EVENT_TARGET 同类（进程唯一、与 AppState 同生命周期）。未注册（集成
测试不调用 [`init_session_cache`]）时查找恒 miss、写入恒 no-op，引擎行为与
无缓存时代逐字一致。

不变量：
- 命中需要**身份（account_key）+ 代际 + TTL** 三者同时匹配。代际由账号提交
  事务推进（含仅改密码），身份比对是防测试库间同代际串号的第二道键；
- TTL 10 分钟只为限制「服务端侧吊销（改密）后的失效窗口」：GR ClientLogin
  token 在改密前长效、Fever api_key 是凭据的静态 md5 派生（均不随时间过期）；
- 401/认证类失败回落：调用方（phases/push/subscriptions 的推送·拉取入口）
  在「租约来自缓存」且本轮出现认证类失败时，经 [`reauth_after_auth_failure`]
  剔除缓存会话、重建并**只重试一次**（重试租约 from_cache=false，不再触发）。
*/

/// 会话缓存 TTL。依据：GR ClientLogin token 改密前长效、Fever api_key 静态
/// ——token 不会因时间过期，TTL 只限制「服务端侧吊销后旧会话继续失败」的
/// 窗口（配合 401 回落，实际远小于此值）；10 分钟 ≈ 一次典型同步间隔量级。
const SESSION_CACHE_TTL: Duration = Duration::from_secs(10 * 60);

/// 会话缓存条目（单槽）。身份+代际是命中的双键；Arc 共享让 action token
/// 等 client 内惰性缓存跨同步轮存活。
struct CachedSession {
    identity: String,
    generation: i64,
    created_at: Instant,
    session: Arc<SyncSession>,
}

static SESSION_CACHE: OnceLock<std::sync::Mutex<Option<CachedSession>>> = OnceLock::new();

/// TTL 判定（纯函数，单测锁定）：age < TTL 才复用；恰满 TTL 即视为过期。
fn cache_entry_fresh(age: Duration) -> bool {
    age < SESSION_CACHE_TTL
}

/// 注册会话缓存（lib.rs setup 时调用一次；幂等）。未注册时查找/写入全部
/// no-op——集成测试不注册，引擎行为与无缓存时代完全一致。
pub fn init_session_cache() {
    let _ = SESSION_CACHE.set(std::sync::Mutex::new(None));
}

/// 清空会话缓存（`#[doc(hidden)]`：仅供集成测试在用例间复位，非公开 API）。
#[doc(hidden)]
pub fn clear_session_cache_for_tests() {
    if let Some(cache) = SESSION_CACHE.get() {
        let mut slot = cache.lock().expect("session cache poisoned");
        *slot = None;
    }
}

/// 命中：身份+代际匹配且未过期；过期条目就地剔除（视为 miss）。
fn cache_lookup(identity: &str, generation: i64) -> Option<Arc<SyncSession>> {
    let cache = SESSION_CACHE.get()?;
    let mut slot = cache.lock().expect("session cache poisoned");
    let hit = match slot.as_ref() {
        Some(entry) if entry.identity == identity && entry.generation == generation => {
            cache_entry_fresh(entry.created_at.elapsed()).then(|| entry.session.clone())
        }
        _ => None,
    };
    if hit.is_none() && slot.is_some() {
        // miss（身份/代际失配或过 TTL）：单槽让位，下次 acquire 重建。
        *slot = None;
    }
    hit
}

/// 写入：构建成功才缓存；单槽直接替换。
fn cache_store(identity: String, generation: i64, session: &Arc<SyncSession>) {
    let Some(cache) = SESSION_CACHE.get() else {
        return;
    };
    *cache.lock().expect("session cache poisoned") = Some(CachedSession {
        identity,
        generation,
        created_at: Instant::now(),
        session: session.clone(),
    });
}

/// 剔除：仅当槽内是**同一租约会话实例**（Arc::ptr_eq）的条目时清除。
/// 键（身份+代际）只保证「同账号」：并发双入口持有同键租约都见认证失败时，
/// 先到者剔除并重建的新会话不得被后到者的剔除误杀（误杀后果仅多一次
/// ClientLogin，无正确性影响，但仍属无谓挤兑——W3 审计 P3-2）。
fn cache_evict(identity: &str, generation: i64, session: &Arc<SyncSession>) {
    let Some(cache) = SESSION_CACHE.get() else {
        return;
    };
    let mut slot = cache.lock().expect("session cache poisoned");
    let matches = slot.as_ref().is_some_and(|e| {
        e.identity == identity && e.generation == generation && Arc::ptr_eq(&e.session, session)
    });
    if matches {
        *slot = None;
    }
}

/// 一次同步的会话租约：会话本体 + 缓存键（401 回落剔除用）+ 是否命中缓存
/// （401 回落只在命中缓存的租约上触发——全新构建的会话刚认证过，无回落意义）。
pub(super) struct SessionLease {
    pub(super) session: Arc<SyncSession>,
    pub(super) identity: String,
    pub(super) generation: i64,
    pub(super) from_cache: bool,
}

/// 会话获取（同步/即时推送入口统一走这里）：
/// 锁内快照「凭据身份 + 代际」→ 命中缓存零网络返回 → 未命中走既有
/// [`build_session`] 全链（端点缓存/代际复核语义不变）→ 构建后按**构建时点**
/// 的身份快照写缓存（期间换号则本租约照常返回，ensure_current 会判 stale，
/// 缓存不写脏条目）。
///
/// 失败映射与 build_session 逐字同型（NotConfigured / Failed{快照} /
/// Stale / CorruptGeneration），调用方既有 match 原样保留。
pub(super) async fn acquire_session(
    db: &Arc<Mutex<Connection>>,
    http: &reqwest::Client,
) -> Result<SessionLease, ClientBuildFailure> {
    let snapshot = crate::state::database_work(db.clone(), |conn| {
        Ok((|| {
            let credentials = match read_credentials(conn) {
                Ok(Some(c)) => c,
                Ok(None) => return Err(ClientBuildFailure::NotConfigured),
                // 读取/解密失败：失败会话身份无从得知，快照置空（同 build_session）。
                Err(e) => {
                    return Err(ClientBuildFailure::Failed {
                        err: e,
                        endpoint: String::new(),
                        username: String::new(),
                    })
                }
            };
            let generation = match db::sync_generation(conn) {
                Ok(g) => g,
                Err(e) => return Err(ClientBuildFailure::CorruptGeneration(e)),
            };
            Ok((credentials, generation))
        })())
    })
    .await
    .map_err(ClientBuildFailure::CorruptGeneration)??;
    let ((protocol, endpoint, username, _password), generation) = snapshot;
    let identity = super::account_key(&protocol, &endpoint, &username);
    if let Some(session) = cache_lookup(&identity, generation) {
        return Ok(SessionLease {
            session,
            identity,
            generation,
            from_cache: true,
        });
    }
    let session = Arc::new(build_session(db, http).await?);
    // 构建后重读身份/代际：与 session.generation 一致才写缓存。期间换号
    // （代际已推进）时本租约返回但缓存不落——调用方的 ensure_current 会以
    // staleSession 丢弃本轮，下次 acquire 按新账号重建。
    let fallback_identity = identity.clone();
    let built_generation = session.generation;
    let (lease_identity, lease_generation, cacheable) =
        crate::state::database_work(db.clone(), move |conn| {
            Ok(match (read_credentials(conn), db::sync_generation(conn)) {
                (Ok(Some(c)), Ok(g)) => (
                    super::account_key(&c.0, &c.1, &c.2),
                    g,
                    g == built_generation,
                ),
                _ => (fallback_identity, generation, false),
            })
        })
        .await
        .map_err(ClientBuildFailure::CorruptGeneration)?;
    if cacheable {
        cache_store(lease_identity.clone(), lease_generation, &session);
    }
    Ok(SessionLease {
        session,
        identity: lease_identity,
        generation: lease_generation,
        from_cache: false,
    })
}

/// 401/认证类失败的回落（只重试一次）：剔除该会话的缓存条目 → 重新认证
/// 构建会话。重建失败如实记日志并返回 None（调用方保留首轮既有结果——
/// fail-closed 语义已保留游标/队列，不额外制造阻塞标记）。
pub(super) async fn reauth_after_auth_failure(
    db: &Arc<Mutex<Connection>>,
    http: &reqwest::Client,
    lease: &SessionLease,
) -> Option<Arc<SyncSession>> {
    cache_evict(&lease.identity, lease.generation, &lease.session);
    log::warn!("sync: 同步收到认证类失败，已剔除缓存会话并重认证重试一次");
    match acquire_session(db, http).await {
        Ok(fresh) => Some(fresh.session),
        Err(ClientBuildFailure::NotConfigured) => {
            log::warn!("sync: 重认证时凭据已不存在，保留本轮既有结果");
            None
        }
        Err(ClientBuildFailure::Stale) => {
            log::warn!("sync: 重认证期间账号配置已变更，保留本轮既有结果");
            None
        }
        Err(ClientBuildFailure::CorruptGeneration(e)) => {
            log::warn!("sync: 重认证时代际损坏，保留本轮既有结果: {e}");
            None
        }
        Err(ClientBuildFailure::Failed { err, .. }) => {
            log::warn!("sync: 重认证失败，保留本轮既有结果: {err}");
            None
        }
    }
}

/// 报告级认证失败检测：pull/push 的认证失败按 fail-closed 语义进
/// `report.errors` 而非 Err（不中止本轮），401 回落因此在报告层检测。
/// 判据是**结构化标记**（`AppError::auth`/`auth_if`——greader/fever 的 401/403
/// 状态码构造点与 Fever api_key 失败打标），由各 HTTP 捕获点经
/// `SyncReport::note_client_error` 汇入 `report.auth_failure`；不再串匹配错误
/// 文案（URL 类错误可伪造「→ 401」文案的口子就此关闭，纯网络失败无误报）。
pub(super) fn report_has_auth_failure(report: &SyncReport) -> bool {
    report.auth_failure
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
    let (protocol, endpoint, username, password, generation) =
        crate::state::database_work(db.clone(), |conn| {
            Ok((|| {
                // OPT-014 R1：读取/解密失败不是「未配置」——按可重试失败上报。
                let credentials = match read_credentials(conn) {
                    Ok(Some(c)) => c,
                    Ok(None) => return Err(ClientBuildFailure::NotConfigured),
                    // 解密失败：失败会话的身份（端点/用户名）无从得知，如实留空
                    Err(e) => {
                        return Err(ClientBuildFailure::Failed {
                            err: e,
                            endpoint: String::new(),
                            username: String::new(),
                        })
                    }
                };
                // 同一次持锁内捕获代际（与凭据同源快照）。R1：现存非法代际是**可见的
                // 协议错误**（不按 0 与会话假匹配），单列 CorruptGeneration 交给调用方。
                let generation = match db::sync_generation(conn) {
                    Ok(g) => g,
                    Err(e) => return Err(ClientBuildFailure::CorruptGeneration(e)),
                };
                Ok((
                    credentials.0,
                    credentials.1,
                    credentials.2,
                    credentials.3,
                    generation,
                ))
            })())
        })
        .await
        .map_err(ClientBuildFailure::CorruptGeneration)??;
    let cache_protocol = protocol.clone();
    let cache_endpoint = endpoint.clone();
    let cached = crate::state::database_work(db.clone(), move |conn| {
        Ok(crate::endpoint_resolve::cached_base(
            conn,
            &cache_protocol,
            &cache_endpoint,
        ))
    })
    .await
    .map_err(ClientBuildFailure::CorruptGeneration)?;

    // 两条路径都产出「已认证 + 端点已确定」的客户端：
    // 命中缓存 ⇒ 候选收敛为唯一地址，这一次请求只做认证（不探测）；
    // 未命中 ⇒ 客户端自己探测，探测成功即已认证，无需再登录一次。
    let (backend, resolved) = match protocol.as_str() {
        "fever" => {
            let weak_db = Arc::downgrade(db);
            let probe =
                crate::fever::FeverClient::new(&endpoint, &username, &password, http.clone())
                    .with_request_guard(Arc::new(move || {
                        let weak_db = weak_db.clone();
                        Box::pin(async move {
                            let db = weak_db.upgrade().ok_or_else(SyncSession::stale_error)?;
                            crate::state::database_work(db, move |conn| {
                                SyncSession::ensure_generation(conn, generation)
                            })
                            .await
                        })
                    }));
            let client = match &cached {
                Some(base) => {
                    let client = probe.at_resolved(base);
                    match client.verify().await {
                        Ok(()) => client,
                        Err(e) if e.code == "staleSession" => {
                            return Err(ClientBuildFailure::Stale)
                        }
                        Err(e) => {
                            log::warn!("sync: Fever 认证失败: {e}");
                            return Err(ClientBuildFailure::Failed {
                                err: e,
                                endpoint: endpoint.clone(),
                                username: username.clone(),
                            });
                        }
                    }
                }
                None => match probe.resolve().await {
                    Ok(client) => client,
                    Err(e) if e.code == "staleSession" => return Err(ClientBuildFailure::Stale),
                    Err(e) => {
                        log::warn!("sync: Fever 端点解析/认证失败: {e}");
                        return Err(ClientBuildFailure::Failed {
                            err: e,
                            endpoint: endpoint.clone(),
                            username: username.clone(),
                        });
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
                    let weak_db = Arc::downgrade(db);
                    let client = client.with_request_guard(Arc::new(move || {
                        let weak_db = weak_db.clone();
                        Box::pin(async move {
                            let db = weak_db.upgrade().ok_or_else(SyncSession::stale_error)?;
                            crate::state::database_work(db, move |conn| {
                                SyncSession::ensure_generation(conn, generation)
                            })
                            .await
                        })
                    }));
                    (Backend::GReader(client), base)
                }
                Err(e) => {
                    log::warn!("sync: ClientLogin 失败: {e}");
                    return Err(ClientBuildFailure::Failed {
                        err: e,
                        endpoint: endpoint.clone(),
                        username: username.clone(),
                    });
                }
            }
        }
    };

    // 登录/探测期间账号可能已被切换：只有代际仍匹配才允许落缓存与返回会话。
    crate::state::database_work(db.clone(), move |conn| {
        let _writer = db::writer_guard(conn)?;
        let current = db::sync_generation(conn)?;
        if current != generation {
            log::warn!(
                "sync: 构建客户端期间账号配置已变更（代际 {generation} → {current}），丢弃本次会话"
            );
            return Err(SyncSession::stale_error());
        }
        // 只有「这一轮真的探测过」才落库；命中缓存时无需重复写。
        if cached.is_none() {
            if let Err(e) =
                crate::endpoint_resolve::remember_base(conn, &protocol, &endpoint, &resolved)
            {
                log::warn!("sync: 端点解析结果落库失败（不影响本次同步）: {e}");
            }
        }
        Ok(())
    })
    .await
    .map_err(|error| {
        if error.code == "staleSession" {
            ClientBuildFailure::Stale
        } else {
            ClientBuildFailure::CorruptGeneration(error)
        }
    })?;
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
        SyncSession::ensure_generation(&conn_a, session.generation).unwrap();

        // 另一个库推进代际不影响本库
        db::bump_sync_generation(&conn_b).unwrap();
        SyncSession::ensure_generation(&conn_a, session.generation).expect("每库代际独立");
        assert_eq!(db::sync_generation(&conn_b).unwrap(), 1);

        // 本库推进 → 旧会话失效，错误码明确
        db::bump_sync_generation(&conn_a).unwrap();
        let err = SyncSession::ensure_generation(&conn_a, session.generation)
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

    /// W3-1：TTL 边界纯函数锁定——age < TTL 复用；恰满 TTL 即过期。
    /// 判别力：把判定翻转成 `<=` 或把 TTL 当毫秒用的实现会红。
    #[test]
    fn cache_entry_fresh_boundary() {
        assert!(cache_entry_fresh(Duration::from_secs(0)));
        assert!(cache_entry_fresh(
            SESSION_CACHE_TTL - Duration::from_secs(1)
        ));
        assert!(
            !cache_entry_fresh(SESSION_CACHE_TTL),
            "恰满 TTL 必须视为过期（不为相等留活口）"
        );
        assert!(cache_entry_fresh(Duration::from_secs(599)));
        assert!(!cache_entry_fresh(Duration::from_secs(601)));
    }

    /// W3-1（2026-11-02 结构化）：报告级检测只认错误构造点的认证标记——
    /// 文案相同但未标记的错误（如 URL 路径恰好含「→ 401」）不得触发；
    /// 401/403/Fever api_key 三类构造点的标记经 note_client_error 汇入都必须
    /// 生效；错误文案本身照旧进 errors（IPC 面不变）。
    #[test]
    fn report_has_auth_failure_reads_structured_marker_not_text() {
        let mut report = SyncReport::default();
        assert!(!report_has_auth_failure(&report));
        // 串匹配口子已关闭：文案含「→ 401」但未标记 → 不触发
        report
            .errors
            .push("拉取条目正文失败: [network] GET /x → 500".into());
        report
            .errors
            .push("拉取条目 id 失败: [network] GET /x → 401 Unauthorized".into());
        assert!(!report_has_auth_failure(&report));

        // GReader 401 构造点标记（生产形态：AppError::network(..).auth_if(..)）+
        // 捕获点 note_client_error 登记 → 触发；错误文案照旧进 errors
        let greader_401 = crate::error::AppError::network(
            "GET /reader/api/0/stream/items/ids → 401 Unauthorized",
        )
        .auth_if(crate::error::AppError::is_auth_status(401));
        report.note_client_error(&greader_401);
        report
            .errors
            .push(format!("拉取条目 id 失败: {greader_401}"));
        assert!(
            report.errors.iter().any(|e| e.contains("→ 401")),
            "错误文案照旧进 errors（IPC 面不变）: {:?}",
            report.errors
        );
        assert!(report_has_auth_failure(&report));

        // Fever api_key 构造点（无条件 auth 标记）与 403 构造点同样生效
        let fever_auth = crate::error::AppError::new(
            "auth",
            "Fever 认证失败（api_key 不正确；FreshRSS 请使用个人设置里的「API 密码」）",
        )
        .auth();
        let mut fever_report = SyncReport::default();
        fever_report.note_client_error(&fever_auth);
        fever_report.errors.push(format!(
            "状态对账跳过：Fever 状态集合拉取失败（{fever_auth}）"
        ));
        assert!(report_has_auth_failure(&fever_report));

        let greader_403 = crate::error::AppError::network("GET /x → 403 Forbidden")
            .auth_if(crate::error::AppError::is_auth_status(403));
        let mut case_403 = SyncReport::default();
        case_403.note_client_error(&greader_403);
        case_403.errors.push(format!("拉取订阅失败: {greader_403}"));
        assert!(report_has_auth_failure(&case_403));

        // 纯网络失败（无标记）不触发（note_client_error 是 no-op）
        let plain_network = crate::error::AppError::network("GET /x → 500 Internal Server Error");
        let mut plain = SyncReport::default();
        plain.note_client_error(&plain_network);
        plain
            .errors
            .push(format!("拉取条目 id 失败: {plain_network}"));
        assert!(!report_has_auth_failure(&plain));
    }

    /// W3-1：剔除只命中同一账号条目——期间换号重建的新条目不受影响。
    /// （不触网：构造条目不调用 client 方法。）
    #[test]
    fn cache_evict_only_matching_identity() {
        init_session_cache();
        clear_session_cache_for_tests();
        let session = Arc::new(SyncSession {
            generation: 3,
            client: Backend::GReader(crate::greader::GReaderClient::new(
                "http://127.0.0.1:1",
                "t",
                reqwest::Client::new(),
            )),
        });
        cache_store("greader|http://a.example|user-a".into(), 3, &session);
        // 异账号剔除：no-op，条目仍在
        cache_evict("greader|http://a.example|user-b", 3, &session);
        assert!(cache_lookup("greader|http://a.example|user-a", 3).is_some());
        // 同账号异代际剔除：no-op（代际推进后的新会话不得被旧键误杀）
        cache_evict("greader|http://a.example|user-a", 2, &session);
        assert!(cache_lookup("greader|http://a.example|user-a", 3).is_some());
        // 同键但**异会话实例**（并发双入口各持同键租约的挤兑形态）：
        // 后到者的剔除不得误杀先到者重建并写入的新会话（P3-2）
        let other = Arc::new(SyncSession {
            generation: 3,
            client: Backend::GReader(crate::greader::GReaderClient::new(
                "http://127.0.0.1:1",
                "t2",
                reqwest::Client::new(),
            )),
        });
        cache_evict("greader|http://a.example|user-a", 3, &other);
        assert!(
            cache_lookup("greader|http://a.example|user-a", 3).is_some(),
            "同键异实例剔除不得误杀槽内会话"
        );
        // 同键同实例（自己租约）剔除：条目消失
        cache_evict("greader|http://a.example|user-a", 3, &session);
        assert!(cache_lookup("greader|http://a.example|user-a", 3).is_none());
        clear_session_cache_for_tests();
    }
}
