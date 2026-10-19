//! sync 的 phases 子模块（TASK-045 从 sync.rs 按既有章节拆分）。

use super::*;
use crate::db;
use crate::error::{AppError, AppResult};
use crate::fever;
use crate::greader::GReaderClient;
use chrono::Utc;
use rusqlite::Connection;
use std::sync::Arc;
use tokio::sync::Mutex;

/// 条目 pull 全局串行：cursor/history 的失败恢复依赖本轮快照不被另一 pull
/// 交错改写。锁只覆盖同步编排，数据库锁仍按既有短临界区获取。
static PULL_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// feeds 阶段（订阅层）：push_feeds + pull_feeds。秒级，首连先跑这段。
/// 锁纪律：HTTP 全在锁外；DB 读写在锁内短临界区完成。
/// TASK-124：feeds 阶段的构建失败不记录不发事件（记录/呈现职责在 states 段与
/// 即时推送，见 push.rs record_push_block）——「未配置」与「失败」对订阅层同步
/// 的既有行为一致（都是中止），两类失败映射回同一 notConnected 错误。
/// OPT-006：会话代际失配（账号在构建期间被切换）→ staleSession 错误，不落库。
pub async fn feeds_phase(
    db: &Arc<Mutex<Connection>>,
    http: &reqwest::Client,
) -> AppResult<SyncReport> {
    let session = build_session(db, http).await.map_err(|e| match e {
        ClientBuildFailure::Stale => SyncSession::stale_error(),
        // R1：坏代际是可见协议错误（原样上报，不伪装 notConnected）
        ClientBuildFailure::CorruptGeneration(err) => err,
        _ => AppError::new(
            "notConnected",
            "未配置同步后端（Google Reader / Fever 凭据）",
        ),
    })?;
    feeds_phase_in_session(db, &session).await
}

/// feeds 阶段的会话版本（sync_local_feeds 用）：要求当前代际等于
/// `expected_generation`——入队与推送之间账号被切换时不得把本地订阅推给新账号
/// （首连推送必须经用户确认，见 SyncTab 的 firstConnect 流程）。
pub async fn feeds_phase_at(
    db: &Arc<Mutex<Connection>>,
    http: &reqwest::Client,
    expected_generation: i64,
) -> AppResult<SyncReport> {
    let session = build_session(db, http).await.map_err(|e| match e {
        ClientBuildFailure::Stale => SyncSession::stale_error(),
        ClientBuildFailure::CorruptGeneration(err) => err,
        _ => AppError::new(
            "notConnected",
            "未配置同步后端（Google Reader / Fever 凭据）",
        ),
    })?;
    if session.generation != expected_generation {
        return Err(SyncSession::stale_error());
    }
    feeds_phase_in_session(db, &session).await
}

async fn feeds_phase_in_session(
    db: &Arc<Mutex<Connection>>,
    session: &SyncSession,
) -> AppResult<SyncReport> {
    let mut report = SyncReport::default();
    push_feeds(db, session, &mut report).await;
    // OPT-009 阶段 B：类型化订阅 outbox（新增/编辑/退订）的生产执行器。
    // 在 pull 之前执行：本轮入队的订阅先推远端并完成本地绑定，随后的 pull
    // 以远端列表做对账（两类路径都会绑定，绑定写幂等）。
    push_subscription_outbox(db, session, &mut report).await;
    pull_feeds(db, session, &mut report).await;
    Ok(report)
}

async fn begin_subscription_discovery(
    db: &Arc<Mutex<Connection>>,
    session: &SyncSession,
) -> AppResult<bool> {
    let conn = db.lock().await;
    session.ensure_current(&conn)?;
    let account = super::current_account(&conn)?.ok_or_else(|| {
        AppError::new(
            "notConnected",
            "订阅发现缺少当前同步账号身份，本轮不发起远端请求",
        )
    })?;
    let now = Utc::now().timestamp();
    if !db::zero_binding_discovery_due(&conn, &account.key, now)? {
        return Ok(false);
    }
    db::mark_zero_binding_discovery_attempt(&conn, &account.key, now)?;
    Ok(true)
}

/// states 阶段（状态+条目层）：push 队列 + pull entries。
/// `full=true` 含绑定回填+全量对账（手动同步/首连）；false 只做增量（后台自动同步）。
pub async fn states_phase(
    db: &Arc<Mutex<Connection>>,
    http: &reqwest::Client,
    full: bool,
) -> AppResult<SyncReport> {
    let session = match build_session(db, http).await {
        Ok(c) => c,
        Err(ClientBuildFailure::NotConfigured) => {
            return Err(AppError::new(
                "notConnected",
                "未配置同步后端（Google Reader / Fever 凭据）",
            ));
        }
        // OPT-006：构建期间账号被切换——旧会话作废，不记录阻塞标记（那会标记
        // 到新账号的队列上），直接丢弃本轮。
        Err(ClientBuildFailure::Stale) => return Err(SyncSession::stale_error()),
        // R1：坏代际 = 可见协议错误，不记录推送阻塞（与队列无关）。
        Err(ClientBuildFailure::CorruptGeneration(err)) => return Err(err),
        // TASK-124（审计 P2-6②）：有凭据但认证/端点/网络失败——记录阻塞标记
        // （attempts/last_error 此前对这类失败永不记录）+ 发 sync-queue-changed。
        // 返回错误保持既有 notConnected 形态：手动同步的「未连接静默跳过」前端
        // 语义（isNotConnectedError）不因本卡改变；可见性由 pill/摘要卡承担。
        Err(ClientBuildFailure::Failed(e)) => {
            record_push_block(db, &e).await;
            return Err(AppError::new(
                "notConnected",
                "未配置同步后端（Google Reader / Fever 凭据）",
            ));
        }
    };
    let mut report = SyncReport::default();
    // OPT-019 P2-1：只读订阅发现与“是否有绑定”无关，只由独立节流键控制。
    // 该路径绝不执行 push_feeds/subscription outbox，因此不会自动推送本地
    // 订阅或绕过首连确认；发现时间也不写条目 last_sync_ts。
    let discovery_attempted = if !full {
        match begin_subscription_discovery(db, &session).await {
            Ok(due) => due,
            Err(e) => {
                report
                    .errors
                    .push(format!("订阅发现节流检查失败，本轮不发起远端请求: {e}"));
                return Ok(report);
            }
        }
    } else {
        false
    };
    if discovery_attempted {
        pull_feeds(db, &session, &mut report).await;
    } else if !full {
        log::debug!("sync: 订阅发现尚未到期，本轮跳过只读订阅拉取");
    }
    // 推送段进 PUSH_LOCK（与 push_states_now/feeds_phase 的推送互斥，防 prune 竞态）
    {
        let _guard = PUSH_LOCK.lock().await;
        let (plan, stale) = {
            let conn = db.lock().await;
            // OPT-006：计划构建即 HTTP→DB 边界（age_stale_queue 是写）——代际失配
            // 直接丢弃本轮，不发出任何请求。
            session.ensure_current(&conn)?;
            let stale = age_stale_queue(&conn, &mut report);
            (plan_push(&conn)?, stale)
        };
        let (done, failed) = exec_push(db, &session, &plan, &mut report).await;
        {
            let conn = db.lock().await;
            // OPT-006：推送确认/失败标记/剪除都是回写边界——账号已变更时旧队项
            // 已随 purge 清空，且新账号可能复用编号语义，一律不得落库。
            session.ensure_current(&conn)?;
            // TASK-116：失败项落库标记（attempts+1 / last_error，四态展示口径）
            if !failed.is_empty() {
                if let Err(err) = db::mark_push_failed(&conn, &failed) {
                    log::warn!("sync: 推送失败标记落库失败（states 首次 push）: {err}");
                }
            }
            // P3[1]：剪除失败此前静默 → 队项残留会被重复推送。改为 warn。
            if !done.is_empty() {
                if let Err(err) = db::prune_sync(&conn, &done) {
                    log::warn!("sync: 剪除已推送队项失败（states 首次 push）: {err}");
                }
            }
        }
        // TASK-124：推送确认/失败后发事件（同 push_states_now 口径：成功出队
        // waiting 降、失败标记 failed/last_error 升；仅老化清理也发——waiting 已降）。
        // 统计读在锁外短临界区（notify_queue_changed 内部自持短锁）。
        if !done.is_empty() || !failed.is_empty() || stale > 0 {
            notify_queue_changed(db).await;
        }
    }
    {
        let _pull_guard = PULL_LOCK.lock().await;
        pull_entries(db, &session, &mut report, full).await?;
        if report.binding_incomplete {
            let retry_discovery = if discovery_attempted {
                true
            } else {
                match begin_subscription_discovery(db, &session).await {
                    Ok(due) => due,
                    Err(e) => {
                        report.errors.push(format!(
                            "条目绑定不完整后的订阅发现检查失败，本轮不重试: {e}"
                        ));
                        false
                    }
                }
            };
            if retry_discovery {
                pull_feeds(db, &session, &mut report).await;
                // 只重试一次；若仍不完整，report 保留错误供下一节流窗口处理。
                pull_entries(db, &session, &mut report, full).await?;
            }
        }
    }
    // pull 后补推：pull 会为「本地已读但后端刚抓取成功的文章」绑定
    // remote_id（此前未绑定，push 段跳过）。绑定后再推一次，把它们的
    // pending read/star 推到后端——否则这些文章要等下一轮同步才同步状态，
    // 其他客户端会看到「本地已读、后端仍未读」。二次 push 幂等（队列已空则无操作）。
    {
        let _guard = PUSH_LOCK.lock().await;
        let (plan, stale) = {
            let conn = db.lock().await;
            session.ensure_current(&conn)?;
            let stale = age_stale_queue(&conn, &mut report);
            (plan_push(&conn)?, stale)
        };
        if !plan.status.is_empty() || !plan.stars.is_empty() {
            let (done, failed) = exec_push(db, &session, &plan, &mut report).await;
            {
                let conn = db.lock().await;
                // TASK-116：失败项落库标记（pull 后补推段，同首次 push 段口径）
                // OPT-006：同首次 push 段——回写前复核代际。
                session.ensure_current(&conn)?;
                if !failed.is_empty() {
                    if let Err(err) = db::mark_push_failed(&conn, &failed) {
                        log::warn!("sync: 推送失败标记落库失败（states 补推）: {err}");
                    }
                }
                // P3[1]：同上（pull 后补推段）。
                if !done.is_empty() {
                    if let Err(err) = db::prune_sync(&conn, &done) {
                        log::warn!("sync: 剪除已推送队项失败（states 补推）: {err}");
                    }
                }
            }
            // TASK-124：补推段同口径发事件。
            if !done.is_empty() || !failed.is_empty() || stale > 0 {
                notify_queue_changed(db).await;
            }
        } else if stale > 0 {
            // TASK-124：补推段无可推项，但老化清理改变了统计 → 仍发事件。
            notify_queue_changed(db).await;
        }
    }
    Ok(report)
}

/// 完整同步（全量路径）：feeds 阶段 + states 阶段（full 对账）串联。
pub async fn sync_now(
    db: &Arc<Mutex<Connection>>,
    http: &reqwest::Client,
) -> AppResult<SyncReport> {
    let mut report = feeds_phase(db, http).await?;
    let states = states_phase(db, http, true).await?;
    report.pushed_states = states.pushed_states;
    report.pulled_entries = states.pulled_entries;
    // R3：条目层实际变化标记必须跨阶段汇聚（缺了它，sync_now 路径上正文/
    // 绑定/对账的真实变化会被漏发事件）。旧计数保持既有形状不动（兼容）。
    report.entries_data_changed |= states.entries_data_changed;
    report.errors.extend(states.errors);
    Ok(report)
}

/// 轻量同步（后台自动调度）：push 队列 + 增量 pull。
pub async fn sync_light(
    db: &Arc<Mutex<Connection>>,
    http: &reqwest::Client,
) -> AppResult<SyncReport> {
    states_phase(db, http, false).await
}

/// 仅由自动调度器使用的订阅发现路径：节流到期时只读 `pull_feeds`。若本轮
/// 新增了远端 feed 绑定，再拉一次条目状态；不执行订阅推送/outbox。
pub async fn sync_subscription_discovery(
    db: &Arc<Mutex<Connection>>,
    http: &reqwest::Client,
) -> AppResult<SyncReport> {
    let session = match build_session(db, http).await {
        Ok(c) => c,
        Err(ClientBuildFailure::NotConfigured) => {
            return Err(AppError::new(
                "notConnected",
                "未配置同步后端（Google Reader / Fever 凭据）",
            ));
        }
        Err(ClientBuildFailure::Stale) => return Err(SyncSession::stale_error()),
        Err(ClientBuildFailure::CorruptGeneration(err)) => return Err(err),
        Err(ClientBuildFailure::Failed(e)) => {
            record_push_block(db, &e).await;
            return Err(AppError::new(
                "notConnected",
                "未配置同步后端（Google Reader / Fever 凭据）",
            ));
        }
    };
    let mut report = SyncReport::default();
    if !begin_subscription_discovery(db, &session).await? {
        return Ok(report);
    }
    let before = {
        let conn = db.lock().await;
        session.ensure_current(&conn)?;
        db::remote_feed_binding_count(&conn)?
    };
    pull_feeds(db, &session, &mut report).await;
    let after = {
        let conn = db.lock().await;
        session.ensure_current(&conn)?;
        db::remote_feed_binding_count(&conn)?
    };
    if after > before {
        let _pull_guard = PULL_LOCK.lock().await;
        pull_entries(db, &session, &mut report, false).await?;
        if report.binding_incomplete {
            // 新订阅在首次发现与条目读取之间才变为可见时，再发现并重试一次。
            pull_feeds(db, &session, &mut report).await;
            pull_entries(db, &session, &mut report, false).await?;
        }
    }
    Ok(report)
}

/// 测试连接（设置页「测试连接」按钮）。
/// 按协议分派：Google Reader 走 ClientLogin，Fever 走 `api_key` 认证。
/// 返回 (展示消息, 用户名, 解析出的 API 根)——用户名供 sync_save 落库做账号显示，
/// API 根供其写入解析缓存（TASK-059：后续同步不必重复探测）。
pub async fn test_connection(
    protocol: &str,
    endpoint: &str,
    username: &str,
    password: &str,
    http: &reqwest::Client,
) -> AppResult<(String, String, String)> {
    if endpoint.trim().is_empty() || username.trim().is_empty() || password.trim().is_empty() {
        return Err(AppError::new(
            "notConnected",
            "请先填写 Endpoint、用户名和密码",
        ));
    }
    // 两个协议都要先**解析端点**（用户只填域名时自动适配），再用解析出的地址拉订阅。
    let (subs, base) = match protocol {
        "fever" => {
            let client = fever::FeverClient::new(endpoint, username, password, http.clone())
                .resolve()
                .await?;
            (
                client.subscriptions().await?.len(),
                client.resolved_base().to_string(),
            )
        }
        _ => {
            let client = GReaderClient::login(endpoint, username, password, http.clone()).await?;
            (
                client.subscriptions().await?.len(),
                client.resolved_base().to_string(),
            )
        }
    };
    Ok((
        format!("已连接：{username}（{subs} 个订阅）"),
        username.to_string(),
        base,
    ))
}
