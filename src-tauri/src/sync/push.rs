// Note: 队列状态变化要主动发事件；认证失败与网络失败必须分类上报 — 见 .agents/notes/implemented/feature/2026-10-06-同步四态与失败可见性.md
//! sync 的 push 子模块（TASK-045 从 sync.rs 按既有章节拆分）。

use super::*;
use crate::db;
use crate::error::{AppError, AppResult};
use chrono::Utc;
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::Mutex;

/// 全局推送互斥：同一时刻只允许一个推送在飞（防抖即时推送 vs 后台自动
/// 同步 vs 手动同步并发）。exec_push 成功后按 queue_id prune——并发时 A
/// 可能 prune 掉 B 正在推的项；更糟的是收藏 toggle 非幂等，交错执行会把
/// 星标状态翻转两次。串行化后两场景只会先后重推同一状态（幂等），无害。
pub(super) static PUSH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 待推送动作的锁内快照：HTTP 执行所需的全部信息。
pub(super) struct PushPlan {
    /// (队列 id, action, entry ids)——read 广播副本展开后
    pub(super) status: Vec<PushStatus>,
    /// (队列 id, entry id)——收藏切换（star/unstar 语义，Google Reader 无 toggle）
    pub(super) stars: Vec<(i64, i64, bool)>,
    /// 本轮 plan_push 写回了多少条无身份/已绑定分类。调用方据此在队列没有
    /// 可推项时也发 `sync-queue-changed`（分类计数本身已变化）。
    pub(super) reclassified: usize,
}

impl PushPlan {
    /// states 同一轮的 pull 后补推只处理新绑定/新入队项；已经在本轮首推尝试过的
    /// queue id 不得再次发送（尤其 Fever toggle）。
    // Note: 同轮重试只补新绑定项，attempted 集合跨首推/补推保留 — 见
    // .agents/notes/implemented/bug-fix/2026-11-03-状态写回ACK验证与条目三态.md
    pub(super) fn remove_attempted(&mut self, attempted: &HashSet<i64>) {
        self.status
            .retain(|status| !attempted.contains(&status.queue_id));
        self.stars
            .retain(|(queue_id, _, _)| !attempted.contains(queue_id));
    }
}

pub(super) struct PushStatus {
    queue_id: i64,
    action: String,
    entry_ids: Vec<i64>,
}

/// exec_push 的结果按可落库语义分组：
/// - done：写请求成功且读回目标态一致，可精确出队；
/// - retry：写失败或读回失败/不一致，保留队列并进入 retry_wait；
/// - target_missing：保留给未来能提供权威删除证据的端点；当前空/短响应不会
///   产生该分类。
#[derive(Default)]
pub(super) struct PushOutcome {
    pub(super) done: Vec<i64>,
    pub(super) failures: Vec<PushFailure>,
    pub(super) warnings: Vec<String>,
    pub(super) attempted: HashSet<i64>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PushFailureClass {
    Push,
    Verification,
    TargetMissing,
}

pub(super) struct PushFailure {
    pub(super) queue_id: i64,
    pub(super) summary: String,
    pub(super) class: PushFailureClass,
}

impl PushOutcome {
    pub(super) fn failures_for(&self, class: PushFailureClass) -> Vec<(i64, String)> {
        self.failures
            .iter()
            .filter(|failure| failure.class == class)
            .map(|failure| (failure.queue_id, failure.summary.clone()))
            .collect()
    }
}

#[derive(Clone, Copy)]
enum VerifyField {
    Read,
    Star,
}

impl VerifyField {
    fn state(self, item: &crate::greader::ItemContent) -> crate::greader::RemoteState {
        match self {
            Self::Read => item.read_state(),
            Self::Star => item.starred_state(),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Read => "已读",
            Self::Star => "收藏",
        }
    }
}

/// 锁内：解析 sync_queue → 推送计划。
///
/// 无 remote_id 的条目**不是静默跳过**：按同 feed 是否已绑定写回
/// `local_only` / `bindable_retry`、原因与错误类别，保留整行等待后续绑定。
/// 有 remote_id 的 `local_only` / `bindable_retry`（升级遗留或触发器缺失）会
/// 回到 `bound_pending` 并正常进入计划；`failed` / `expired_unconfirmed` 不自动
/// 重试。身份查询错误一律上抛，禁止 `.ok().flatten()` 把 DB 失败当成未绑定。
pub(super) fn plan_push(conn: &Connection) -> AppResult<PushPlan> {
    let items = db::take_sync_queue(conn)?;
    let mut plan = PushPlan {
        status: Vec::new(),
        stars: Vec::new(),
        reclassified: 0,
    };
    for item in items {
        if item.kind != "article_state" {
            continue; // add_feed 由 push_feeds 阶段消费；其它历史动作仅保留可见
        }
        let Some(article_id) = item.article_id else {
            if item.status.as_str() != db::SYNC_QUEUE_STATUS_LOCAL_ONLY
                || item.unbound_reason.as_deref() != Some("no_article_identity")
            {
                db::mark_queue_identity_waiting(
                    conn,
                    item.id,
                    db::SYNC_QUEUE_STATUS_LOCAL_ONLY,
                    "no_article_identity",
                )?;
                plan.reclassified += 1;
            }
            continue;
        };
        let Some((article_remote_id, feed_remote_id)) =
            db::article_queue_identity(conn, article_id)?
        else {
            return Err(crate::error::AppError::internal(format!(
                "同步队列项 {} 指向不存在的文章 {article_id}",
                item.id
            )));
        };
        let Some(remote_id) = article_remote_id else {
            if item.status.as_str() == "failed"
                || item.status.as_str() == db::STATUS_EXPIRED_UNCONFIRMED
            {
                continue;
            }
            let (status, reason) = if feed_remote_id.is_some() {
                (db::STATUS_BINDABLE_RETRY, "awaiting_remote_identity")
            } else {
                (db::SYNC_QUEUE_STATUS_LOCAL_ONLY, "no_remote_identity")
            };
            if item.status.as_str() != status || item.unbound_reason.as_deref() != Some(reason) {
                db::mark_queue_identity_waiting(conn, item.id, status, reason)?;
                plan.reclassified += 1;
            }
            continue;
        };
        if item.status.as_str() == "failed"
            || item.status.as_str() == db::STATUS_EXPIRED_UNCONFIRMED
        {
            continue;
        }
        if item.status.as_str() != db::STATUS_BOUND_PENDING
            && item.status.as_str() != db::STATUS_RETRY_WAIT
        {
            db::mark_queue_bound_pending(conn, item.id)?;
            plan.reclassified += 1;
        }
        match item.action.as_str() {
            // 已读广播：绑定的 entry + 全部同文副本 entry 一并标读
            // （双端场景：Read You 不去重，手机上另一源的副本也要已读，
            // 否则手机读完这篇、那个源里又冒出来一篇未读的"同一篇"）
            // TASK-112 政策：本分支是「同文副本读状态传播 = read 广播保持现状」
            // 的消费点（政策定义与 DEC-refactor-roadmap-20261005 第 6 条原文见
            // conflict_policy.rs 头注；unread/star/unstar 不广播）。
            // OPT-008B：目标集由 db::read_broadcast_targets 单点给出（同文兄弟
            // 条目的 remote_id + 旧 CSV 记账的有限兼容），不再只靠 remote_dup_ids
            // ——跨源同 URL 现在是各自独立的持久条目，广播按统一选择规则展开。
            // OPT-008B R1（初审 P3-4 / 主控 P2）：广播目标查询失败必须**透传**
            // ——`unwrap_or_default` 会把「同文兄弟 + 旧 CSV 记账」静默当空，
            // 读传播无声缺失且原 read 队项会被 prune（本卡明确禁止「失败当空」）。
            // 错误经 plan_push 上抛：push_states_now/ states 推送段据此中止本轮，
            // 队列保留、下轮重试。
            "read" => {
                let mut ids = vec![remote_id];
                for dup in db::read_broadcast_targets(conn, article_id)? {
                    if dup != remote_id {
                        ids.push(dup);
                    }
                }
                plan.status.push(PushStatus {
                    queue_id: item.id,
                    action: "read".into(),
                    entry_ids: ids,
                });
            }
            "unread" => plan.status.push(PushStatus {
                queue_id: item.id,
                action: "unread".into(),
                entry_ids: vec![remote_id],
            }),
            "star" => plan.stars.push((item.id, remote_id, true)),
            "unstar" => plan.stars.push((item.id, remote_id, false)),
            _ => {}
        }
    }
    Ok(plan)
}

/// 读回后的单目标判定。只有「目标存在且状态字段明确等于目标态」才算确认；
/// 缺条/短响应的结果是无证据，不是 target_missing。
// Note: 200 空/短/缺 items 响应一律 inconclusive — 见
// .agents/notes/implemented/bug-fix/2026-11-03-状态写回ACK验证与条目三态.md
#[derive(Debug)]
enum TargetCheck {
    Confirmed,
    Inconclusive(String),
}

#[derive(Debug)]
enum QueueCheck {
    Confirmed,
    Partial(String),
    Inconclusive(String),
}

struct ReadbackStates {
    states: HashMap<i64, crate::greader::ItemContent>,
    complete: bool,
}

fn check_target(
    states: &ReadbackStates,
    entry_id: i64,
    field: VerifyField,
    desired: bool,
) -> TargetCheck {
    let Some(item) = states.states.get(&entry_id) else {
        let reason = if states.complete {
            "读回响应完整但没有目标；协议没有提供可证明目标已删除的信号"
        } else {
            "读回响应为空或短于请求，无法区分目标删除与短响应"
        };
        return TargetCheck::Inconclusive(format!("verification/inconclusive: {reason}"));
    };
    match field.state(item) {
        crate::greader::RemoteState::Missing => TargetCheck::Inconclusive(format!(
            "entry {entry_id} 读回缺少{}状态字段，无法确认目标态",
            field.label()
        )),
        state if state == crate::greader::RemoteState::from_bool(desired) => TargetCheck::Confirmed,
        _ => TargetCheck::Inconclusive(format!(
            "entry {entry_id} 读回{}状态与目标 {} 不一致",
            field.label(),
            desired
        )),
    }
}

/// 主目标决定队列是否结算；广播副本只作为 partial/warning。副本缺失/未确认
/// 不得让整行永久失败，也不能触发每轮重发全部副本。
// Note: 广播聚合以 entry_ids[0] 为主目标，副本只记 partial — 见
// .agents/notes/implemented/bug-fix/2026-11-03-状态写回ACK验证与条目三态.md
fn verify_queue_status(
    status: &PushStatus,
    states: &ReadbackStates,
    field: VerifyField,
    desired: bool,
) -> QueueCheck {
    let Some(primary) = status.entry_ids.first() else {
        return QueueCheck::Inconclusive("状态推送计划没有主目标".into());
    };
    match check_target(states, *primary, field, desired) {
        TargetCheck::Confirmed => {
            let secondary: Vec<String> = status
                .entry_ids
                .iter()
                .skip(1)
                .filter_map(
                    |entry_id| match check_target(states, *entry_id, field, desired) {
                        TargetCheck::Confirmed => None,
                        TargetCheck::Inconclusive(message) => Some(format!(
                            "广播副本 entry {entry_id} 未确认（不影响主目标结算）: {message}"
                        )),
                    },
                )
                .collect();
            if secondary.is_empty() {
                QueueCheck::Confirmed
            } else {
                QueueCheck::Partial(secondary.join("; "))
            }
        }
        TargetCheck::Inconclusive(message) => QueueCheck::Inconclusive(message),
    }
}

async fn readback_states(session: &SyncSession, ids: &[i64]) -> AppResult<ReadbackStates> {
    // 与两份服务端实现的单次 items 窗口上限对齐；分块后任一块失败仍整次
    // 读回失败（retry_wait），不会把被服务端截断的 id 误判成 target_missing。
    const VERIFICATION_CHUNK: usize = 50;
    let mut states = HashMap::new();
    let mut complete = true;
    for chunk in ids.chunks(VERIFICATION_CHUNK) {
        let read = session.client.items_for_state_verification(chunk).await?;
        complete &= read.complete;
        for item in read.items {
            let Some(id) = crate::greader::parse_item_id(&item.id) else {
                return Err(crate::error::AppError::new(
                    "protocol",
                    format!("状态读回响应含无法解析的条目 id: {:?}", item.id),
                ));
            };
            states.insert(id, item);
        }
    }
    Ok(ReadbackStates { states, complete })
}

fn record_failure(
    outcome: &mut PushOutcome,
    report: &mut SyncReport,
    queue_id: i64,
    summary: String,
    class: PushFailureClass,
) {
    report.errors.push(summary.clone());
    outcome.failures.push(PushFailure {
        queue_id,
        summary,
        class,
    });
}

fn record_warning(outcome: &mut PushOutcome, report: &mut SyncReport, summary: String) {
    report.errors.push(summary.clone());
    outcome.warnings.push(summary);
}

fn apply_readback(
    outcome: &mut PushOutcome,
    report: &mut SyncReport,
    statuses: &[&PushStatus],
    field: VerifyField,
    desired: bool,
    states: &ReadbackStates,
    verified_targets: &mut usize,
) {
    for status in statuses {
        match verify_queue_status(status, states, field, desired) {
            QueueCheck::Confirmed => {
                *verified_targets += status.entry_ids.len();
                outcome.done.push(status.queue_id);
            }
            QueueCheck::Partial(message) => {
                *verified_targets += 1;
                outcome.done.push(status.queue_id);
                record_warning(outcome, report, format!("状态写回验证 partial：{message}"));
            }
            QueueCheck::Inconclusive(message) => record_failure(
                outcome,
                report,
                status.queue_id,
                format!("状态写回后读回未确认（inconclusive）: {message}"),
                PushFailureClass::Verification,
            ),
        }
    }
}

/// 锁外：执行推送计划并逐目标读回确认。
///
/// HTTP 成功只表示服务端收到请求；GReader `edit-tag` / Fever `mark=item` 对
/// 不存在目标也可能返回 200/OK。只有读回目标存在且三态字段等于 desired 的
/// 队项才进入 `done`。读回失败/字段缺失/状态相反/短响应进入 `retry_wait`
/// 的 verification/inconclusive；当前协议没有可从空/短响应证明 target_missing
/// 的端点，缺条绝不升级成终态。本函数保持无业务 DB 访问，由调用方落库。
///
/// OPT-006：每个新批次发送前与会话返回后复核代际；旧账号结果不得落回本地。
pub(super) async fn exec_push(
    db: &Arc<Mutex<Connection>>,
    session: &SyncSession,
    plan: &PushPlan,
    report: &mut SyncReport,
) -> PushOutcome {
    let mut outcome = PushOutcome::default();
    let mut verified_targets = 0usize;

    // read/unread 聚合批量（Google Reader edit-tag 单请求可携带全部 id + tag）。
    for action in ["read", "unread"] {
        let statuses: Vec<&PushStatus> =
            plan.status.iter().filter(|s| s.action == action).collect();
        if statuses.is_empty() {
            continue;
        }
        let mut seen = std::collections::HashSet::new();
        let ids: Vec<i64> = statuses
            .iter()
            .flat_map(|s| s.entry_ids.iter().copied())
            .filter(|id| seen.insert(*id))
            .collect();
        for status in &statuses {
            outcome.attempted.insert(status.queue_id);
        }
        if !session_is_current(db, session).await {
            log::warn!("sync: 推送批次发送前检测到账号代际变更，停止后续推送（{action}）");
            return outcome;
        }
        let result = if action == "read" {
            session.client.mark_read(&ids).await
        } else {
            session.client.mark_unread(&ids).await
        };
        if !session_is_current(db, session).await {
            log::warn!("sync: 推送响应返回时账号代际已变更，丢弃该批次确认（{action}）");
            return outcome;
        }
        match result {
            Ok(()) => {
                let states = match readback_states(session, &ids).await {
                    Ok(states) => states,
                    Err(e) => {
                        if !session_is_current(db, session).await {
                            return outcome;
                        }
                        report.note_client_error(&e);
                        let summary =
                            format!("{action} 写回后读回失败（服务端已 ACK，目标态未确认）: {e}");
                        for status in &statuses {
                            record_failure(
                                &mut outcome,
                                report,
                                status.queue_id,
                                summary.clone(),
                                PushFailureClass::Verification,
                            );
                        }
                        continue;
                    }
                };
                if !session_is_current(db, session).await {
                    log::warn!("sync: 状态读回返回时账号代际已变更，丢弃该批次确认（{action}）");
                    return outcome;
                }
                apply_readback(
                    &mut outcome,
                    report,
                    &statuses,
                    VerifyField::Read,
                    action == "read",
                    &states,
                    &mut verified_targets,
                );
            }
            Err(e) => {
                report.note_client_error(&e);
                let summary = format!("状态推送失败: {e}");
                for status in &statuses {
                    record_failure(
                        &mut outcome,
                        report,
                        status.queue_id,
                        summary.clone(),
                        PushFailureClass::Push,
                    );
                }
            }
        }
    }

    // 收藏：star/unstar 逐条发送并逐条读回。GReader 是绝对 add/remove；Fever
    // 在 api_version<4 时先探测再发送，避免 toggle 重发翻反。
    for (qid, remote_id, want_star) in &plan.stars {
        outcome.attempted.insert(*qid);
        if !session_is_current(db, session).await {
            log::warn!("sync: 推送批次发送前检测到账号代际变更，停止后续推送（star）");
            return outcome;
        }
        let result = match &session.client {
            Backend::GReader(_) => {
                let r = if *want_star {
                    session.client.mark_starred(&[*remote_id]).await
                } else {
                    session.client.mark_unstarred(&[*remote_id]).await
                };
                r.map(|_| true)
            }
            Backend::Fever(c) => c.push_star_state(*remote_id, *want_star).await,
        };
        if !session_is_current(db, session).await {
            log::warn!("sync: 推送响应返回时账号代际已变更，丢弃该批次确认（star）");
            return outcome;
        }
        match result {
            Ok(_sent) => {
                let states = match readback_states(session, &[*remote_id]).await {
                    Ok(states) => states,
                    Err(e) => {
                        if !session_is_current(db, session).await {
                            return outcome;
                        }
                        report.note_client_error(&e);
                        record_failure(
                            &mut outcome,
                            report,
                            *qid,
                            format!(
                                "收藏写回后读回失败（entry {remote_id}，服务端已 ACK，目标态未确认）: {e}"
                            ),
                            PushFailureClass::Verification,
                        );
                        continue;
                    }
                };
                if !session_is_current(db, session).await {
                    log::warn!("sync: 收藏读回返回时账号代际已变更，丢弃该批次确认（star）");
                    return outcome;
                }
                match check_target(&states, *remote_id, VerifyField::Star, *want_star) {
                    TargetCheck::Confirmed => {
                        verified_targets += 1;
                        outcome.done.push(*qid);
                    }
                    TargetCheck::Inconclusive(message) => record_failure(
                        &mut outcome,
                        report,
                        *qid,
                        format!("收藏写回后读回未确认（inconclusive）: {message}"),
                        PushFailureClass::Verification,
                    ),
                }
            }
            Err(e) => {
                report.note_client_error(&e);
                let summary = format!("收藏同步失败: entry {remote_id}: {e}");
                record_failure(
                    &mut outcome,
                    report,
                    *qid,
                    summary,
                    match e.code.as_str() {
                        "target_missing" => PushFailureClass::TargetMissing,
                        "verification" => PushFailureClass::Verification,
                        _ => PushFailureClass::Push,
                    },
                );
            }
        }
    }
    report.pushed_states += verified_targets;
    outcome
}

/// 即时状态推送：只推 sync_queue（read/unread/star/unstar + 副本广播），
/// 不做任何 pull。set_read/set_starred 变更后 ~1s 内到达服务端。
/// 失败静默（队列保留，下轮同步重推）——后台同步不打扰用户。
/// 队列保留期（A-8）：超过该天数仍无远端绑定的状态项视为无法自动收敛，
/// 转为 `expired_unconfirmed` 并保留本地意图（不再物理删除）。
const QUEUE_RETENTION_DAYS: i64 = 30;

/// 老化处置（A-8 修订）：到期无绑定项转 `expired_unconfirmed`，不再删除用户
/// 意图；本地文章 read/star 值原样保留。结果计入 report 便于诊断。
/// TASK-124：返回转态条数（>0 = 队列分类统计已变化，调用方据此发事件）。
pub(super) fn age_stale_queue(conn: &Connection, report: &mut SyncReport) -> usize {
    // 与 sync_queue.created_at 同格式（SQLite datetime('now')：UTC 无时区后缀）
    let cutoff = (Utc::now() - chrono::Duration::days(QUEUE_RETENTION_DAYS))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    match db::prune_stale_unbound(conn, &cutoff) {
        Ok(n) if n > 0 => {
            report.errors.push(format!(
                "队列老化：{n} 条超过 {QUEUE_RETENTION_DAYS} 天仍未绑定远端的本地状态转为 expired_unconfirmed（本地 read/star 保留，不再自动推送）"
            ));
            n
        }
        Ok(_) => 0,
        Err(e) => {
            report.errors.push(format!("队列老化转态失败: {e}"));
            0
        }
    }
}

/* ============================================================
TASK-124（审计 P2-6①②）：sync-queue-changed 事件总线
============================================================ */

/// 事件目标注册点：lib.rs setup 时注册一次。为什么是注册表而不是 AppHandle
/// 参数传递——push_states_now / states_phase 的 `(db, http)` 签名被 tests/ 的
/// 大量二参调用锁定（sync_phases_e2e / sync_e2e / pull_cursor_e2e 等），加参
/// 会波及不可修改的集成测试面；注册表 set-once、集成测试不注册 → 所有发射点
/// no-op（既有测试零影响）。AppHandle: Clone+Send+Sync，OnceLock 静态安全。
static QUEUE_EVENT_TARGET: std::sync::OnceLock<tauri::AppHandle> = std::sync::OnceLock::new();

/// setup 时注册事件目标（lib.rs；幂等：重复注册以首次为准）。
pub fn init_queue_event_target(app: tauri::AppHandle) {
    let _ = QUEUE_EVENT_TARGET.set(app);
}

/// 即时推送的基础设施错误（计划构建/失败标记/出队写库）走独立事件，前端
/// 转成带重试入口的错误；普通协议失败仍由队列 `last_error` 展示，不在此重复。
pub(super) fn emit_sync_error(message: &str) {
    crate::diagnostics::record(0, "push", "error", Some(&AppError::internal("")));
    let Some(app) = QUEUE_EVENT_TARGET.get() else {
        return;
    };
    use tauri::Emitter;
    if app
        .emit(
            "sync-error",
            serde_json::json!({ "stage": "states", "message": message }),
        )
        .is_err()
    {
        crate::diagnostics::record(0, "push", "event_failed", None);
    }
}

/// build_client 失败摘要（TASK-124）：区分「认证失败」与「网络/端点失败」。
/// - Fever 认证错误 code="auth"（fever.rs AUTH_FAILED_MSG）；
/// - GReader 凭据错误都经 ClientLogin 报出（greader.rs login_at：401/403/400 →
///   "ClientLogin → {status}"；BadAuthentication → "ClientLogin 失败：{code}"）；
/// - 其余（端点 404 / DNS / 超时等）透传实际错误，不冒充认证失败。
///
/// 摘要进队列 last_error → pill「· 部分失败」与摘要卡错误行；截断由
/// mark_push_blocked 统一承担（200 字符）。
pub fn push_block_summary(err: &crate::error::AppError) -> String {
    let msg = err.message.trim();
    if err.code == "auth" || msg.contains("ClientLogin") {
        format!("认证失败：{msg}")
    } else {
        format!("推送被阻塞（网络/端点失败）：{msg}")
    }
}

/// 队列状态变化通知：发轻量 Tauri 事件 `sync-queue-changed`
/// （payload = 生命周期分类统计，经 db::queue_changed_payload 单点）。
///
/// 锁纪律：调用方必须在 **DB 锁外** 调用——本函数内部自己短持锁读统计
/// （sync_queue_stats 单查询，<1ms 量级；参照 schedule_state_push 的锁外调度先例，
/// 不在持锁临界区内做 emit）。
///
/// 静默规则（审计「区分未配置/认证失败/网络失败」的「未配置」半边）：
/// - 未配置同步（无凭据）→ 不发事件不报警，前端 pill 靠既有「本地模式 · 直连抓取」
///   分支（不误报、不刷状态）；
/// - 事件目标未注册（集成测试环境）→ no-op。
pub async fn notify_queue_changed(db: &Arc<Mutex<Connection>>) {
    let Some(app) = QUEUE_EVENT_TARGET.get() else {
        return;
    };
    let payload = {
        let conn = db.lock().await;
        // OPT-014 R1：读取/解密失败按「已配置」的显示口径处理（Err ≠ 未配置）——
        // 让等待/失败统计照常发给前端（错误原因由推送路径的 last_error 呈现）；
        // 只有确证未配置才静默（不误报、不刷状态）。
        let configured = match crate::sync::read_credentials(&conn) {
            Ok(None) => false,
            Ok(Some(_)) => true,
            Err(e) => {
                log::warn!("sync: 凭据读取失败，队列状态按已配置口径上报: {e}");
                true
            }
        };
        match db::sync_queue_stats(&conn) {
            Ok(stats) => db::queue_changed_payload(&stats, configured),
            Err(e) => {
                log::warn!("sync: 读取队列统计失败，跳过本轮 sync-queue-changed: {e}");
                emit_sync_error(&format!("同步队列状态读取失败，请重试：{e}"));
                None
            }
        }
    };
    if let Some(p) = payload {
        use tauri::Emitter;
        if app.emit("sync-queue-changed", p).is_err() {
            crate::diagnostics::record(0, "push", "event_failed", None);
        }
    }
}

/// 客户端构建失败（认证/端点/网络）的统一处置（push_states_now 与 states_phase
/// 共用，TASK-124）：全部现存队项计一次失败尝试 + 记录摘要（审计②此前
/// attempts/last_error 对这类失败永不记录），确有标记行时发 sync-queue-changed
/// ——pill「· 部分失败」与摘要卡错误行即时可见。持锁段内只做 DB 标记，
/// 事件在锁外发（notify_queue_changed 内部自持短锁）。
pub(super) async fn record_push_block(db: &Arc<Mutex<Connection>>, err: &crate::error::AppError) {
    let summary = push_block_summary(err);
    log::warn!("sync: 状态推送被阻塞（客户端构建失败）: {summary}");
    let marked = {
        let conn = db.lock().await;
        db::mark_push_blocked(&conn, &summary)
    };
    match marked {
        Ok(n) if n > 0 => notify_queue_changed(db).await,
        Ok(_) => {}
        Err(e) => {
            let message = format!("状态推送阻塞标记落库失败: {e}");
            log::warn!("sync: {message}");
            emit_sync_error(&message);
        }
    }
}

pub async fn push_states_now(db: &Arc<Mutex<Connection>>, http: &reqwest::Client) {
    let runtime = runtime::for_db(db);
    let db = db.clone();
    let http = http.clone();
    let _ = runtime
        .run(
            "push",
            async move { push_states_now_inner(&db, &http).await },
        )
        .await;
}

async fn push_states_now_inner(
    db: &Arc<Mutex<Connection>>,
    http: &reqwest::Client,
) -> AppResult<SyncReport> {
    runtime::check_generation(db).await?;
    let lease = match acquire_session(db, http).await {
        Ok(c) => c,
        // 未配置：静默返回（队列保留，连接后补推）——不发事件不报警（A-5 语义不变）
        Err(ClientBuildFailure::NotConfigured) => {
            return Err(AppError::new("notConnected", "未配置同步服务"))
        }
        // OPT-006：构建期间账号被切换——丢弃本轮（不记录阻塞标记）
        Err(ClientBuildFailure::Stale) => {
            log::info!("sync: 即时推送会话已失效（账号配置变更），丢弃本轮");
            return Err(SyncSession::stale_error());
        }
        // R1：坏代际 = 可见协议错误，不记录推送阻塞（与队列无关）
        Err(ClientBuildFailure::CorruptGeneration(e)) => {
            log::warn!("sync: 即时推送因同步代际损坏中止: {e}");
            return Err(e);
        }
        // TASK-124：认证/端点/网络失败——记录阻塞标记 + 发事件（此前在此提前返回，
        // 用户在 UI 上永远看不到「为什么一直不同步」，审计 P2-6②）
        Err(ClientBuildFailure::Failed { err, .. }) => {
            record_push_block(db, &err).await;
            return Err(err);
        }
    };
    runtime::accept_generation(lease.session.generation)?;
    let mut report = push_states_with_session(db, &lease.session).await?;
    // W3-1：401/认证类失败回落——即时推送命中缓存会话且出现认证类失败时，
    // 剔除缓存重认证并只重试一次（重试租约 from_cache=false，不再触发回落）。
    if lease.from_cache && report_has_auth_failure(&report) {
        if let Some(fresh) = reauth_after_auth_failure(db, http, &lease).await {
            runtime::accept_generation(fresh.generation)?;
            report = push_states_with_session(db, &fresh).await?;
        }
    }
    Ok(report)
}

/// 即时推送的会话版本（W3-1 拆分）：串行化与记账段原样搬移，返回本轮是否
/// 出现认证类失败（供会话缓存的 401 回落判定；失败项仍按既有口径落库标记）。
async fn push_states_with_session(
    db: &Arc<Mutex<Connection>>,
    session: &SyncSession,
) -> AppResult<SyncReport> {
    // 串行化：与 states_phase/feeds_phase 的推送段互斥（见 PUSH_LOCK 注释）
    let _guard = PUSH_LOCK.lock().await;
    let planned = {
        let conn = db.lock().await;
        // OPT-006：计划构建是回写前边界（age_stale_queue 会写库）——代际失配即弃。
        if session.ensure_current(&conn).is_err() {
            log::info!("sync: 即时推送计划构建时账号代际已变更，丢弃本轮");
            return Err(SyncSession::stale_error());
        }
        let mut report = SyncReport::default();
        let stale = age_stale_queue(&conn, &mut report);
        match plan_push(&conn) {
            Ok(p) => Ok((p, report, stale)),
            Err(e) => Err(e),
        }
    };
    let (plan, mut report, stale) = match planned {
        Ok(v) => v,
        Err(e) => {
            let message = format!("状态推送计划构建失败（队列保留）: {e}");
            log::warn!("sync: {message}");
            emit_sync_error(&message);
            return Err(AppError::internal(message));
        }
    };
    if plan.status.is_empty() && plan.stars.is_empty() {
        // 无可推项：老化转状或 plan_push 的无身份分类改变了统计时仍要发事件。
        if stale > 0 || plan.reclassified > 0 {
            notify_queue_changed(db).await;
        }
        return Ok(report);
    }
    let outcome = exec_push(db, session, &plan, &mut report).await;
    let mut infra_errors: Vec<String> = Vec::new();
    {
        let conn = db.lock().await;
        // OPT-006：确认/失败标记/剪除都是回写边界——账号已变更时旧队项已清空，
        // 不得把旧批次的记账落回（更不得动新账号的队项）。
        if session.ensure_current(&conn).is_err() {
            log::info!("sync: 即时推送回写时账号代际已变更，丢弃确认与标记");
            return Err(SyncSession::stale_error());
        }
        // TASK-116：失败项落库标记（attempts+1 / last_error）——即时推送失败此前
        // 只进日志，用户在 UI 上永远看不到「部分失败」。标记失败不影响队列保留。
        let retry_push = outcome.failures_for(PushFailureClass::Push);
        let retry_verification = outcome.failures_for(PushFailureClass::Verification);
        let target_missing = outcome.failures_for(PushFailureClass::TargetMissing);
        if !retry_push.is_empty() {
            if let Err(e) = db::mark_push_failed(&conn, &retry_push) {
                let message = format!("队列推送失败标记落库失败: {e}");
                log::warn!("sync: {message}");
                report.errors.push(message.clone());
                infra_errors.push(message);
            }
        }
        if !retry_verification.is_empty() {
            if let Err(e) = db::mark_push_verification_failed(&conn, &retry_verification) {
                let message = format!("队列失败标记落库失败: {e}");
                log::warn!("sync: {message}");
                report.errors.push(message.clone());
                infra_errors.push(message);
            }
        }
        if !target_missing.is_empty() {
            if let Err(e) = db::mark_queue_target_missing(&conn, &target_missing) {
                let message = format!("target_missing 标记落库失败: {e}");
                log::warn!("sync: {message}");
                report.errors.push(message.clone());
                infra_errors.push(message);
            }
        }
        if !outcome.done.is_empty() {
            if let Err(e) = db::prune_sync(&conn, &outcome.done) {
                let message = format!("已推送队项出队失败: {e}");
                log::warn!("sync: {message}");
                report.errors.push(message.clone());
                infra_errors.push(message);
            }
        }
    }
    let mut emitted_target_missing = std::collections::HashSet::new();
    for failure in outcome
        .failures
        .iter()
        .filter(|failure| failure.class == PushFailureClass::TargetMissing)
    {
        if emitted_target_missing.insert(failure.summary.as_str()) {
            emit_sync_error(&failure.summary);
        }
    }
    for warning in &outcome.warnings {
        emit_sync_error(warning);
    }
    for message in &infra_errors {
        emit_sync_error(message);
    }
    // TASK-124：推送确认/失败后发事件——成功出队 waiting 降（pill 恢复「后端已
    // 同步」），失败标记 failed/last_error 升（「· 部分失败」）。统计读在锁外
    // 短临界区；本轮统计无变化（不可能到这里，done/failed 至少一者非空）除外。
    notify_queue_changed(db).await;
    if !report.errors.is_empty() {
        log::info!("sync: 即时推送失败（队列保留待重推）: {:?}", report.errors);
    } else {
        log::info!("sync: 即时推送 {} 项状态", report.pushed_states);
    }
    Ok(report)
}

/* ============================================================
TASK-124 单元测试：build_client 失败摘要的认证/网络区分
（cargo test 由 CI 执行，DEC-local-cargo-gate-20261005）
============================================================ */
#[cfg(test)]
mod t124_tests {
    use super::*;

    /// (t124-r2) 认证失败区分（审计「区分未配置/认证失败/网络失败」）：
    /// - Fever auth code（fever.rs AUTH_FAILED_MSG）→「认证失败」；
    /// - GReader 凭据错误（greader.rs login_at 实际文案：ClientLogin → 401 /
    ///   ClientLogin 失败：BadAuthentication）→「认证失败」；
    /// - 网络/端点失败（reqwest DNS/超时、端点 404 文案）→ 透传实际错误且
    ///   **不冒充认证失败**。
    ///
    /// 判别力：分类丢失（全部归为网络错误）时认证格子必红；无中生有
    /// （网络错误被标成认证）时网络格子必红。
    #[test]
    fn push_block_summary_distinguishes_auth_from_network() {
        // Fever：code=auth（fever.rs AUTH_FAILED_MSG 形态）
        let fever_auth = crate::error::AppError::new(
            "auth",
            "Fever 认证失败（api_key 不正确；FreshRSS 请使用个人设置里的「API 密码」）",
        );
        assert!(push_block_summary(&fever_auth).starts_with("认证失败："));
        assert!(push_block_summary(&fever_auth).contains("api_key 不正确"));

        // GReader：路径存在但凭据被拒（实际文案，code 为 network）
        let greader_401 = crate::error::AppError::network(
            "ClientLogin → 401 Unauthorized（已定位 API：https://example.com/api/greader.php）",
        );
        assert!(
            push_block_summary(&greader_401).starts_with("认证失败："),
            "GReader 凭据错误经 ClientLogin 报出，必须归入认证失败"
        );

        let greader_bad_auth =
            crate::error::AppError::network("ClientLogin 失败：BadAuthentication");
        assert!(push_block_summary(&greader_bad_auth).starts_with("认证失败："));

        // 网络/端点失败：透传实际错误，不冒充认证失败
        let dns = crate::error::AppError::network(
            "error sending request for url (https://example.com/) ← dns error: lookup failed",
        );
        let s = push_block_summary(&dns);
        assert!(
            s.starts_with("推送被阻塞（网络/端点失败）："),
            "非认证失败不得带「认证失败」文案"
        );
        assert!(s.contains("dns error"), "网络失败必须带实际错误摘要");
        assert!(!s.contains("认证失败"));

        // 端点 404（greader.rs login 文案，无 ClientLogin 字样）→ 网络/端点桶
        let endpoint_404 = crate::error::AppError::network(
            "在该地址下找不到 GReader API（HTTP 404，已尝试：https://example.com）。请确认域名是否正确",
        );
        let s = push_block_summary(&endpoint_404);
        assert!(s.starts_with("推送被阻塞（网络/端点失败）："));
        assert!(s.contains("HTTP 404"));
        assert!(!s.contains("认证失败"));
    }
}

#[cfg(test)]
mod queue_plan_tests {
    use super::*;

    fn conn() -> Connection {
        let mut c = Connection::open_in_memory().unwrap();
        db::MIGRATIONS.to_latest(&mut c).unwrap();
        c
    }

    fn seed(c: &Connection, feed_remote: Option<i64>, article_remote: Option<i64>) -> i64 {
        c.execute(
            "INSERT INTO feeds (feed_url, title, remote_id)
             VALUES ('https://plan.example/rss', 'P', ?1)",
            rusqlite::params![feed_remote],
        )
        .unwrap();
        c.execute(
            "INSERT INTO articles (feed_id, guid, title, remote_id)
             VALUES (1, 'g-plan', 'P', ?1)",
            rusqlite::params![article_remote],
        )
        .unwrap();
        c.last_insert_rowid()
    }

    /// 旧形态直接 INSERT 得到默认 bound_pending；真实无 remote_id 时 plan_push
    /// 必须显式转 bindable_retry（同 feed 已绑定），不得静默 continue。
    #[test]
    fn unbound_article_is_reclassified_not_silently_skipped() {
        let c = conn();
        let aid = seed(&c, Some(10), None);
        c.execute(
            "INSERT INTO sync_queue (article_id, action) VALUES (?1, 'read')",
            rusqlite::params![aid],
        )
        .unwrap();
        let plan = plan_push(&c).unwrap();
        assert!(plan.status.is_empty() && plan.stars.is_empty());
        assert_eq!(plan.reclassified, 1, "分类写回必须被调用方看见");
        let (status, reason, last_error, last_error_class): (
            String,
            Option<String>,
            Option<String>,
            Option<String>,
        ) = c
            .query_row(
                "SELECT status, unbound_reason, last_error, last_error_class
                   FROM sync_queue
                  WHERE article_id = ?1",
                rusqlite::params![aid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(status, db::STATUS_BINDABLE_RETRY);
        assert_eq!(reason.as_deref(), Some("awaiting_remote_identity"));
        assert!(
            last_error.is_none(),
            "无身份分类不是网络失败，不得污染真实 last_error"
        );
        assert_eq!(last_error_class.as_deref(), Some("unbound"));
        assert_eq!(
            db::sync_queue_stats(&c).unwrap().waiting,
            0,
            "等待身份的项不得计入远端等待"
        );
    }

    /// 有 remote_id 的队项保持原行为：进入推送计划，且不被无身份分类逻辑改写。
    #[test]
    fn bound_article_still_enters_push_plan() {
        let c = conn();
        c.execute_batch(
            "INSERT INTO feeds (feed_url, title, remote_id)
             VALUES ('https://plan.example/rss', 'P', 10);
             INSERT INTO articles (feed_id, guid, title, remote_id)
             VALUES (1, 'g-bound', 'P', 101);
             INSERT INTO sync_queue (article_id, action)
             VALUES (1, 'read');",
        )
        .unwrap();
        let plan = plan_push(&c).unwrap();
        assert_eq!(plan.status.len(), 1, "绑定项必须进入推送计划");
        assert_eq!(plan.status[0].entry_ids, vec![101]);
        assert_eq!(plan.reclassified, 0);
        let status: String = c
            .query_row("SELECT status FROM sync_queue", [], |r| r.get(0))
            .unwrap();
        assert_eq!(status, db::STATUS_BOUND_PENDING);
        assert_eq!(db::sync_queue_stats(&c).unwrap().waiting, 1);
    }

    #[test]
    fn terminal_and_expired_rows_are_not_replanned() {
        for terminal in ["failed", db::STATUS_EXPIRED_UNCONFIRMED] {
            let c = conn();
            let aid = seed(&c, Some(10), Some(101));
            c.execute(
                "INSERT INTO sync_queue (article_id, action) VALUES (?1, 'read')",
                rusqlite::params![aid],
            )
            .unwrap();
            c.execute(
                "UPDATE sync_queue SET status = ?1, last_error = ?2",
                rusqlite::params![terminal, format!("{terminal}: 已停止自动推送")],
            )
            .unwrap();

            let plan = plan_push(&c).unwrap();
            assert!(
                plan.status.is_empty() && plan.stars.is_empty(),
                "{terminal} 不得重新进入推送计划"
            );
            assert_eq!(plan.reclassified, 0, "终态/过期状态不得被静默改写");
            let stored: String = c
                .query_row("SELECT status FROM sync_queue", [], |r| r.get(0))
                .unwrap();
            assert_eq!(stored, terminal);
        }
    }
}
