//! sync 的 phases 子模块（TASK-045 从 sync.rs 按既有章节拆分）。

use super::*;
use crate::db;
use crate::error::{AppError, AppResult};
use crate::fever;
use crate::greader::GReaderClient;
use chrono::Utc;
use rusqlite::Connection;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Mutex;

/// 条目 pull 全局串行：cursor/history 的失败恢复依赖本轮快照不被另一 pull
/// 交错改写。锁只覆盖同步编排，数据库锁仍按既有短临界区获取。
static PULL_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 定向 feeds 阶段的推送范围（sync_local_feeds 专用）。
/// `None` 仍表示正常全量同步；空集合是合法的定向范围——不得回退成全量。
#[derive(Debug, Default, Clone)]
pub struct FeedsPushScope {
    pub legacy_add_queue_ids: HashSet<i64>,
    pub subscription_outbox_ids: HashSet<i64>,
}

/// feeds 阶段（订阅层）：push_feeds + pull_feeds。秒级，首连先跑这段。
/// 锁纪律：HTTP 全在锁外；DB 读写在锁内短临界区完成。
/// 构建失败保留真实错误，且为已有订阅 outbox 行落账；仅未配置返回 notConnected。
/// OPT-006：会话代际失配（账号在构建期间被切换）→ staleSession 错误，不落库。
pub async fn feeds_phase(
    db: &Arc<Mutex<Connection>>,
    http: &reqwest::Client,
) -> AppResult<SyncReport> {
    let runtime = runtime::for_db(db);
    let db = db.clone();
    let http = http.clone();
    runtime
        .run("feeds", async move { feeds_phase_inner(&db, &http).await })
        .await
}

async fn feeds_phase_inner(
    db: &Arc<Mutex<Connection>>,
    http: &reqwest::Client,
) -> AppResult<SyncReport> {
    runtime::check_generation(db).await?;
    let lease = match acquire_session(db, http).await {
        Ok(l) => l,
        Err(ClientBuildFailure::Stale) => return Err(SyncSession::stale_error()),
        // R1：坏代际是可见协议错误（原样上报，不伪装 notConnected）
        Err(ClientBuildFailure::CorruptGeneration(err)) => return Err(err),
        // A-P2-6：订阅 outbox 的 pending 行同样被本轮构建失败阻塞，落账可见。
        Err(ClientBuildFailure::Failed {
            err,
            endpoint,
            username,
        }) => {
            record_outbox_session_block(db, &err, &endpoint, &username).await;
            return Err(err);
        }
        Err(ClientBuildFailure::NotConfigured) => {
            return Err(AppError::new(
                "notConnected",
                "未配置同步后端（Google Reader / Fever 凭据）",
            ));
        }
    };
    runtime::accept_generation(lease.session.generation)?;
    let mut report = feeds_phase_in_session(db, &lease.session).await?;
    // W3-1：401/认证类失败回落——命中缓存的会话可能已被服务端吊销，剔除后
    // 重认证并只重试一次（重试租约 from_cache=false，不再触发回落）。
    if lease.from_cache && report_has_auth_failure(&report) {
        if let Some(fresh) = reauth_after_auth_failure(db, http, &lease).await {
            runtime::accept_generation(fresh.generation)?;
            report = feeds_phase_in_session(db, &fresh).await?;
        }
    }
    Ok(report)
}

/// feeds 阶段的会话版本（sync_local_feeds 用）：要求当前代际等于
/// `expected_generation`——入队与推送之间账号被切换时不得把本地订阅推给新账号
/// （首连推送必须经用户确认，见 SyncTab 的 firstConnect 流程）。
pub async fn feeds_phase_at(
    db: &Arc<Mutex<Connection>>,
    http: &reqwest::Client,
    expected_generation: i64,
) -> AppResult<SyncReport> {
    feeds_phase_at_scoped(db, http, expected_generation, None).await
}

/// 定向版本：只把 `scope` 指定的 legacy add_feed 队项与订阅 outbox 项交给
/// 实际执行器。传 `Some(空集合)` 也必须是不推送任何项，而不是回退全量。
pub async fn feeds_phase_at_scoped(
    db: &Arc<Mutex<Connection>>,
    http: &reqwest::Client,
    expected_generation: i64,
    scope: Option<&FeedsPushScope>,
) -> AppResult<SyncReport> {
    let runtime = runtime::for_db(db);
    let db = db.clone();
    let http = http.clone();
    let scope = scope.cloned();
    runtime
        .run("feeds", async move {
            feeds_phase_at_scoped_inner(&db, &http, expected_generation, scope).await
        })
        .await
}

async fn feeds_phase_at_scoped_inner(
    db: &Arc<Mutex<Connection>>,
    http: &reqwest::Client,
    expected_generation: i64,
    scope: Option<FeedsPushScope>,
) -> AppResult<SyncReport> {
    runtime::check_generation(db).await?;
    let lease = match acquire_session(db, http).await {
        Ok(l) => l,
        Err(ClientBuildFailure::Stale) => return Err(SyncSession::stale_error()),
        Err(ClientBuildFailure::CorruptGeneration(err)) => return Err(err),
        // A-P2-6：同 feeds_phase——构建失败为订阅 outbox pending 行落账。
        Err(ClientBuildFailure::Failed {
            err,
            endpoint,
            username,
        }) => {
            record_outbox_session_block(db, &err, &endpoint, &username).await;
            return Err(err);
        }
        Err(ClientBuildFailure::NotConfigured) => {
            return Err(AppError::new(
                "notConnected",
                "未配置同步后端（Google Reader / Fever 凭据）",
            ));
        }
    };
    if lease.session.generation != expected_generation {
        return Err(SyncSession::stale_error());
    }
    runtime::accept_generation(lease.session.generation)?;
    let mut report = feeds_phase_in_session_scoped(db, &lease.session, scope.as_ref()).await?;
    // W3-1：同 feeds_phase 的 401 回落；重试前复核代际仍是调用方捕获的期望值
    // （期间换号则维持首轮结果，不向新账号重放旧意图）。
    if lease.from_cache && report_has_auth_failure(&report) {
        if let Some(fresh) = reauth_after_auth_failure(db, http, &lease).await {
            if fresh.generation == expected_generation {
                report = feeds_phase_in_session_scoped(db, &fresh, scope.as_ref()).await?;
            }
        }
    }
    Ok(report)
}

async fn feeds_phase_in_session(
    db: &Arc<Mutex<Connection>>,
    session: &SyncSession,
) -> AppResult<SyncReport> {
    feeds_phase_in_session_scoped(db, session, None).await
}

async fn feeds_phase_in_session_scoped(
    db: &Arc<Mutex<Connection>>,
    session: &SyncSession,
    scope: Option<&FeedsPushScope>,
) -> AppResult<SyncReport> {
    let mut report = SyncReport::default();
    push_feeds(
        db,
        session,
        &mut report,
        scope.map(|scope| &scope.legacy_add_queue_ids),
    )
    .await;
    // OPT-009 阶段 B：类型化订阅 outbox（新增/编辑/退订）的生产执行器。
    // 在 pull 之前执行：本轮入队的订阅先推远端并完成本地绑定，随后的 pull
    // 以远端列表做对账（两类路径都会绑定，绑定写幂等）。
    push_subscription_outbox(
        db,
        session,
        &mut report,
        scope.map(|scope| &scope.subscription_outbox_ids),
    )
    .await;
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
    let runtime = runtime::for_db(db);
    let db = db.clone();
    let http = http.clone();
    runtime
        .run("states", async move {
            states_phase_inner(&db, &http, full).await
        })
        .await
}

async fn states_phase_inner(
    db: &Arc<Mutex<Connection>>,
    http: &reqwest::Client,
    full: bool,
) -> AppResult<SyncReport> {
    runtime::check_generation(db).await?;
    let lease = match acquire_session(db, http).await {
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
        // 没有队项时也必须把真实失败传给整轮结果，不能冒充未配置。
        Err(ClientBuildFailure::Failed { err, .. }) => {
            record_push_block(db, &err).await;
            return Err(err);
        }
    };
    runtime::accept_generation(lease.session.generation)?;
    let discovery_attempted = if !full {
        match begin_subscription_discovery(db, &lease.session).await {
            Ok(due) => due,
            Err(e) => {
                let mut report = SyncReport::default();
                report
                    .errors
                    .push(format!("订阅发现节流检查失败，本轮不发起远端请求: {e}"));
                return Ok(report);
            }
        }
    } else {
        false
    };
    let mut report = states_phase_in_session(db, &lease.session, full, discovery_attempted).await?;
    // W3-1：401/认证类失败回落（只在命中缓存的租约上触发；重试一次）。
    if lease.from_cache && report_has_auth_failure(&report) {
        if let Some(fresh) = reauth_after_auth_failure(db, http, &lease).await {
            runtime::accept_generation(fresh.generation)?;
            report = states_phase_in_session(db, &fresh, full, discovery_attempted).await?;
        }
    }
    Ok(report)
}

/// states 阶段的会话版本（W3-1 拆分：会话由 acquire 提供，401 回落重放同一体）。
/// `discovery_attempted` 沿用首轮的只读订阅发现判定——发现尝试已在首轮记账，
/// 重试时不得重复节流判定，也不得因节流窗口错过重拉。
async fn states_phase_in_session(
    db: &Arc<Mutex<Connection>>,
    session: &SyncSession,
    full: bool,
    discovery_attempted: bool,
) -> AppResult<SyncReport> {
    let mut report = SyncReport::default();
    if discovery_attempted {
        pull_feeds(db, session, &mut report).await;
    } else if !full {
        log::debug!("sync: 订阅发现尚未到期，本轮跳过只读订阅拉取");
    }
    // 同一 states 轮的首推尝试集合：pull 后补推只消费新绑定/新入队项，
    // 不把本轮刚进入 retry_wait 的行（尤其 Fever toggle）再发一次。
    let mut attempted_this_round = std::collections::HashSet::new();
    // 推送段进 PUSH_LOCK（与 push_states_now/feeds_phase 的推送互斥，防 prune 竞态）
    {
        let _guard = PUSH_LOCK.lock().await;
        let (plan, stale) = {
            let conn = db.lock().await;
            // OPT-006：计划构建即 HTTP→DB 边界（age_stale_queue 是写）——代际失配
            // 直接丢弃本轮，不发出任何请求。
            session.ensure_current(&conn)?;
            let stale = age_stale_queue(&conn, &mut report);
            let retried = if full {
                db::retry_target_missing(&conn)?
            } else {
                0
            };
            let mut plan = plan_push(&conn)?;
            plan.reclassified += retried;
            (plan, stale)
        };
        let outcome = exec_push(db, session, &plan, &mut report).await;
        attempted_this_round.extend(outcome.attempted.iter().copied());
        let retry_push = outcome.failures_for(PushFailureClass::Push);
        let retry_verification = outcome.failures_for(PushFailureClass::Verification);
        let target_missing = outcome.failures_for(PushFailureClass::TargetMissing);
        {
            let conn = db.lock().await;
            // OPT-006：推送确认/失败标记/剪除都是回写边界——账号已变更时旧队项
            // 已随 purge 清空，且新账号可能复用编号语义，一律不得落库。
            session.ensure_current(&conn)?;
            // TASK-116：失败项落库标记（attempts+1 / last_error，四态展示口径）
            if !retry_push.is_empty() {
                if let Err(err) = db::mark_push_failed(&conn, &retry_push) {
                    log::warn!("sync: 推送失败标记落库失败（states 首次 push）: {err}");
                    report
                        .errors
                        .push(format!("队列失败标记落库失败（states 首次 push）: {err}"));
                }
            }
            if !retry_verification.is_empty() {
                if let Err(err) = db::mark_push_verification_failed(&conn, &retry_verification) {
                    log::warn!("sync: 状态读回失败标记落库失败（states 首次 push）: {err}");
                    report.errors.push(format!(
                        "状态读回失败标记落库失败（states 首次 push）: {err}"
                    ));
                }
            }
            if !target_missing.is_empty() {
                if let Err(err) = db::mark_queue_target_missing(&conn, &target_missing) {
                    log::warn!("sync: target_missing 标记落库失败（states 首次 push）: {err}");
                    report.errors.push(format!(
                        "target_missing 标记落库失败（states 首次 push）: {err}"
                    ));
                }
            }
            // P3[1]：剪除失败此前静默 → 队项残留会被重复推送。改为 warn。
            if !outcome.done.is_empty() {
                if let Err(err) = db::prune_sync(&conn, &outcome.done) {
                    log::warn!("sync: 剪除已推送队项失败（states 首次 push）: {err}");
                    report
                        .errors
                        .push(format!("已推送队项出队失败（states 首次 push）: {err}"));
                }
            }
        }
        // TASK-124：推送确认/失败后发事件（成功出队 waiting 降、失败转
        // retry_wait 并更新 last_error；无身份分类与老化转态也发）。
        // 统计读在锁外短临界区（notify_queue_changed 内部自持短锁）。
        if !outcome.done.is_empty()
            || !outcome.failures.is_empty()
            || stale > 0
            || plan.reclassified > 0
        {
            notify_queue_changed(db).await;
        }
    }
    {
        let _pull_guard = PULL_LOCK.lock().await;
        pull_entries(db, session, &mut report, full).await?;
        if report.binding_incomplete {
            let retry_discovery = if discovery_attempted {
                true
            } else {
                match begin_subscription_discovery(db, session).await {
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
                pull_feeds(db, session, &mut report).await;
                // 只重试一次；若仍不完整，report 保留错误供下一节流窗口处理。
                pull_entries(db, session, &mut report, full).await?;
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
            let mut plan = plan_push(&conn)?;
            plan.remove_attempted(&attempted_this_round);
            (plan, stale)
        };
        if !plan.status.is_empty() || !plan.stars.is_empty() {
            let outcome = exec_push(db, session, &plan, &mut report).await;
            let retry_push = outcome.failures_for(PushFailureClass::Push);
            let retry_verification = outcome.failures_for(PushFailureClass::Verification);
            let target_missing = outcome.failures_for(PushFailureClass::TargetMissing);
            {
                let conn = db.lock().await;
                // TASK-116：失败项落库标记（pull 后补推段，同首次 push 段口径）
                // OPT-006：同首次 push 段——回写前复核代际。
                session.ensure_current(&conn)?;
                if !retry_push.is_empty() {
                    if let Err(err) = db::mark_push_failed(&conn, &retry_push) {
                        log::warn!("sync: 推送失败标记落库失败（states 补推）: {err}");
                        report
                            .errors
                            .push(format!("队列失败标记落库失败（states 补推）: {err}"));
                    }
                }
                if !retry_verification.is_empty() {
                    if let Err(err) = db::mark_push_verification_failed(&conn, &retry_verification)
                    {
                        log::warn!("sync: 状态读回失败标记落库失败（states 补推）: {err}");
                        report
                            .errors
                            .push(format!("状态读回失败标记落库失败（states 补推）: {err}"));
                    }
                }
                if !target_missing.is_empty() {
                    if let Err(err) = db::mark_queue_target_missing(&conn, &target_missing) {
                        log::warn!("sync: target_missing 标记落库失败（states 补推）: {err}");
                        report
                            .errors
                            .push(format!("target_missing 标记落库失败（states 补推）: {err}"));
                    }
                }
                // P3[1]：同上（pull 后补推段）。
                if !outcome.done.is_empty() {
                    if let Err(err) = db::prune_sync(&conn, &outcome.done) {
                        log::warn!("sync: 剪除已推送队项失败（states 补推）: {err}");
                        report
                            .errors
                            .push(format!("已推送队项出队失败（states 补推）: {err}"));
                    }
                }
            }
            // TASK-124：补推段同口径发事件（含分类写回）。
            if !outcome.done.is_empty()
                || !outcome.failures.is_empty()
                || stale > 0
                || plan.reclassified > 0
            {
                notify_queue_changed(db).await;
            }
        } else if stale > 0 || plan.reclassified > 0 {
            // TASK-124：补推段无可推项，但分类/老化转态改变了统计 → 仍发事件。
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
    let runtime = runtime::for_db(db);
    let db = db.clone();
    let http = http.clone();
    runtime
        .run("manual", async move { sync_now_inner(&db, &http).await })
        .await
}

async fn sync_now_inner(
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
    let runtime = runtime::for_db(db);
    let db = db.clone();
    let http = http.clone();
    runtime
        .run("discovery", async move {
            sync_subscription_discovery_inner(&db, &http).await
        })
        .await
}

async fn sync_subscription_discovery_inner(
    db: &Arc<Mutex<Connection>>,
    http: &reqwest::Client,
) -> AppResult<SyncReport> {
    runtime::check_generation(db).await?;
    let lease = match acquire_session(db, http).await {
        Ok(c) => c,
        Err(ClientBuildFailure::NotConfigured) => {
            return Err(AppError::new(
                "notConnected",
                "未配置同步后端（Google Reader / Fever 凭据）",
            ));
        }
        Err(ClientBuildFailure::Stale) => return Err(SyncSession::stale_error()),
        Err(ClientBuildFailure::CorruptGeneration(err)) => return Err(err),
        Err(ClientBuildFailure::Failed { err, .. }) => {
            record_push_block(db, &err).await;
            return Err(err);
        }
    };
    runtime::accept_generation(lease.session.generation)?;
    let mut report = SyncReport::default();
    if !begin_subscription_discovery(db, &lease.session).await? {
        return Ok(report);
    }
    discovery_pull_in_session(db, &lease.session, &mut report).await?;
    // W3-1：401/认证类失败回落。发现尝试已在首轮记账，重试直接重放发现体
    // （不做节流判定——避免「认证失败撞上发现窗口」后整个发现窗口被吞）。
    if lease.from_cache && report_has_auth_failure(&report) {
        if let Some(fresh) = reauth_after_auth_failure(db, http, &lease).await {
            runtime::accept_generation(fresh.generation)?;
            let mut retry_report = SyncReport::default();
            discovery_pull_in_session(db, &fresh, &mut retry_report).await?;
            report = retry_report;
        }
    }
    Ok(report)
}

/// 订阅发现体的会话版本：绑定计数前后比对 → pull_feeds →（有新增绑定）
/// PULL_LOCK 下拉条目 + 绑定不完整重试一次。
async fn discovery_pull_in_session(
    db: &Arc<Mutex<Connection>>,
    session: &SyncSession,
    report: &mut SyncReport,
) -> AppResult<()> {
    let before = {
        let conn = db.lock().await;
        session.ensure_current(&conn)?;
        db::remote_feed_binding_count(&conn)?
    };
    pull_feeds(db, session, report).await;
    let after = {
        let conn = db.lock().await;
        session.ensure_current(&conn)?;
        db::remote_feed_binding_count(&conn)?
    };
    if after > before {
        let _pull_guard = PULL_LOCK.lock().await;
        pull_entries(db, session, report, false).await?;
        if report.binding_incomplete {
            // 新订阅在首次发现与条目读取之间才变为可见时，再发现并重试一次。
            pull_feeds(db, session, report).await;
            pull_entries(db, session, report, false).await?;
        }
    }
    Ok(())
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
