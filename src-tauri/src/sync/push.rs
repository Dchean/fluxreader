// Note: 队列状态变化要主动发事件；认证失败与网络失败必须分类上报 — 见 .agents/notes/implemented/feature/2026-11-13-前端交互契约.md
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
    /// 每条持久队项只包含一个真实远端目标。
    pub(super) status: Vec<PushStatus>,
    /// (队列 id, entry id)——收藏切换（star/unstar 语义，Google Reader 无 toggle）
    pub(super) stars: Vec<(i64, i64, bool)>,
    /// The content-origin proof captured with each queue item. Source lifecycle
    /// and binding changes invalidate both an unsent request and a late ACK.
    bindings: HashMap<i64, db::VerifiedRemoteBinding>,
    /// 本轮 plan_push 写回了多少条无身份/已绑定分类。调用方据此在队列没有
    /// 可推项时也发 `sync-queue-changed`（分类计数本身已变化）。
    pub(super) reclassified: usize,
}

impl PushPlan {
    /// Per-write proof is scoped to this batch, never cached on the session.
    fn request_guard(
        &self,
        database: &Arc<Mutex<Connection>>,
        session: &SyncSession,
        queue_ids: impl IntoIterator<Item = i64>,
    ) -> crate::greader::RequestGuard {
        let bindings: Vec<_> = queue_ids
            .into_iter()
            .map(|id| self.bindings.get(&id).cloned())
            .collect();
        let weak_db = Arc::downgrade(database);
        let generation = session.generation;
        Arc::new(move || {
            let weak_db = weak_db.clone();
            let bindings = bindings.clone();
            Box::pin(async move {
                let database = weak_db.upgrade().ok_or_else(SyncSession::stale_error)?;
                crate::state::database_work(database, move |conn| {
                    if db::sync_generation(conn)? != generation {
                        return Err(SyncSession::stale_error());
                    }
                    for binding in &bindings {
                        let Some(binding) = binding else {
                            return Err(AppError::new(
                                "staleBinding",
                                "发送前条目来源凭据失效，队列保留",
                            ));
                        };
                        if db::verified_remote_binding(conn, binding.article_id)?.as_ref()
                            != Some(binding)
                        {
                            return Err(AppError::new(
                                "staleBinding",
                                "发送前条目来源凭据失效，队列保留",
                            ));
                        }
                    }
                    Ok(())
                })
                .await
            })
        })
    }

    fn binding_is_current(&self, conn: &Connection, queue_id: i64) -> AppResult<bool> {
        let Some(binding) = self.bindings.get(&queue_id) else {
            return Ok(false);
        };
        Ok(db::verified_remote_binding(conn, binding.article_id)?.as_ref() == Some(binding))
    }

    /// states 同一轮的 pull 后补推只处理新绑定/新入队项；已经在本轮首推尝试过的
    /// queue id 不得再次发送（尤其 Fever toggle）。
    // Note: 同轮重试只补新绑定项，attempted 集合跨首推/补推保留 — 见
    // .agents/notes/implemented/architecture/2026-11-13-同步写回与状态一致性.md
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
    entry_id: i64,
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
    /// Call under the same DB lock as the ACK/failure commit. In particular a
    /// stale failure must not change an invalidated queue item back to retry_wait.
    pub(super) fn discard_stale(
        &mut self,
        conn: &Connection,
        plan: &PushPlan,
        report: &mut SyncReport,
    ) -> AppResult<()> {
        let mut current = HashSet::new();
        for queue_id in self
            .done
            .iter()
            .copied()
            .chain(self.failures.iter().map(|f| f.queue_id))
        {
            if plan.binding_is_current(conn, queue_id)? {
                current.insert(queue_id);
            }
        }
        let previous_done = self.done.len();
        self.done.retain(|id| current.contains(id));
        self.failures
            .retain(|failure| current.contains(&failure.queue_id));
        report.pushed_states = report
            .pushed_states
            .saturating_sub(previous_done - self.done.len());
        Ok(())
    }

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
        bindings: HashMap::new(),
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
        let Some(binding) = db::verified_remote_binding(conn, article_id)? else {
            return Err(AppError::internal("队列身份与远端来源凭据不一致"));
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
            // Every source-local group member already owns its own durable queue item.
            // ACK confirms only that exact remote identity; no URL-derived extra targets.
            "read" => plan.status.push(PushStatus {
                queue_id: item.id,
                action: "read".into(),
                entry_id: remote_id,
            }),
            "unread" => plan.status.push(PushStatus {
                queue_id: item.id,
                action: "unread".into(),
                entry_id: remote_id,
            }),
            "star" => plan.stars.push((item.id, remote_id, true)),
            "unstar" => plan.stars.push((item.id, remote_id, false)),
            _ => {}
        }
        plan.bindings.insert(item.id, binding);
    }
    Ok(plan)
}

/// 读回后的单目标判定。只有「目标存在且状态字段明确等于目标态」才算确认；
/// 缺条/短响应的结果是无证据，不是 target_missing。
// Note: 200 空/短/缺 items 响应一律 inconclusive — 见
// .agents/notes/implemented/architecture/2026-11-13-同步写回与状态一致性.md
#[derive(Debug)]
enum TargetCheck {
    Confirmed,
    Inconclusive(String),
}

#[derive(Debug)]
enum QueueCheck {
    Confirmed,
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

/// 每个持久队项单独确认自己的远端目标；组内其他目标保留各自重试状态。
fn verify_queue_status(
    status: &PushStatus,
    states: &ReadbackStates,
    field: VerifyField,
    desired: bool,
) -> QueueCheck {
    match check_target(states, status.entry_id, field, desired) {
        TargetCheck::Confirmed => QueueCheck::Confirmed,
        TargetCheck::Inconclusive(message) => QueueCheck::Inconclusive(message),
    }
}

async fn readback_states(
    database: &Arc<Mutex<Connection>>,
    session: &SyncSession,
    plan: &PushPlan,
    ids: &[i64],
) -> AppResult<ReadbackStates> {
    // 与两份服务端实现的单次 items 窗口上限对齐；分块后任一块失败仍整次
    // 读回失败（retry_wait），不会把被服务端截断的 id 误判成 target_missing。
    const VERIFICATION_CHUNK: usize = 50;
    let mut states = HashMap::new();
    let mut complete = true;
    for chunk in ids.chunks(VERIFICATION_CHUNK) {
        let ids = chunk.to_vec();
        let bindings: Vec<_> = plan
            .bindings
            .values()
            .filter(|binding| ids.contains(&binding.remote_id))
            .cloned()
            .collect();
        let generation = session.generation;
        let current = crate::state::database_work(database.clone(), move |conn| {
            SyncSession::ensure_generation(conn, generation)?;
            let mut current = Vec::new();
            for id in &ids {
                for binding in &bindings {
                    if binding.remote_id == *id
                        && db::verified_remote_binding(conn, binding.article_id)?.as_ref()
                            == Some(binding)
                    {
                        current.push(*id);
                        break;
                    }
                }
            }
            Ok(current)
        })
        .await?;
        complete &= current.len() == chunk.len();
        if current.is_empty() {
            continue;
        }
        let read = session
            .client
            .items_for_state_verification(&current)
            .await?;
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

async fn current_push_items(
    database: &Arc<Mutex<Connection>>,
    session: &SyncSession,
    plan: &PushPlan,
    queue_ids: impl IntoIterator<Item = i64>,
    report: &mut SyncReport,
) -> Option<HashSet<i64>> {
    let bindings: Vec<_> = queue_ids
        .into_iter()
        .map(|id| (id, plan.bindings.get(&id).cloned()))
        .collect();
    let generation = session.generation;
    let result = crate::state::database_work(database.clone(), move |conn| {
        if SyncSession::ensure_generation(conn, generation).is_err() {
            return Ok(None);
        }
        let mut current = HashSet::new();
        for (queue_id, binding) in bindings {
            if let Some(binding) = binding {
                if db::verified_remote_binding(conn, binding.article_id)?.as_ref() == Some(&binding)
                {
                    current.insert(queue_id);
                }
            }
        }
        Ok(Some(current))
    })
    .await;
    match result {
        Ok(current) => current,
        Err(error) => {
            report
                .errors
                .push(format!("推送来源凭据复核失败（队列保留）: {error}"));
            None
        }
    }
}

fn record_failure(
    outcome: &mut PushOutcome,
    report: &mut SyncReport,
    queue_id: i64,
    summary: String,
    class: PushFailureClass,
    is_auth: bool,
) {
    report.push_classified_error(summary.clone(), is_auth);
    outcome.failures.push(PushFailure {
        queue_id,
        summary,
        class,
    });
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
                *verified_targets += 1;
                outcome.done.push(status.queue_id);
            }
            QueueCheck::Inconclusive(message) => record_failure(
                outcome,
                report,
                status.queue_id,
                format!("状态写回后读回未确认（inconclusive）: {message}"),
                PushFailureClass::Verification,
                false,
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
/// 的端点，缺条绝不升级成终态。每次 HTTP 前复核身份，由调用方在同一
/// DB 临界区再次复核来源凭据并落库。
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
        let mut statuses: Vec<&PushStatus> =
            plan.status.iter().filter(|s| s.action == action).collect();
        let Some(current) = current_push_items(
            db,
            session,
            plan,
            statuses.iter().map(|s| s.queue_id),
            report,
        )
        .await
        else {
            return outcome;
        };
        statuses.retain(|status| current.contains(&status.queue_id));
        if statuses.is_empty() {
            continue;
        }
        let mut seen = std::collections::HashSet::new();
        let mut ids: Vec<i64> = statuses
            .iter()
            .map(|s| s.entry_id)
            .filter(|id| seen.insert(*id))
            .collect();
        if !session_is_current(db, session).await {
            log::warn!("sync: 推送批次发送前检测到账号代际变更，停止后续推送（{action}）");
            return outcome;
        }
        let result = if matches!(session.client, Backend::Fever(_)) {
            // Fever already uses one mark HTTP per ID. Recheck between writes,
            // but preserve the existing 50-ID readback chunks after all marks.
            // IDs skipped before a write never enter this batch's ACK domain.
            let mut sent = HashSet::new();
            let mut result = Ok(());
            for id in &ids {
                let Some(current) = current_push_items(
                    db,
                    session,
                    plan,
                    statuses
                        .iter()
                        .filter(|s| s.entry_id == *id)
                        .map(|s| s.queue_id),
                    report,
                )
                .await
                else {
                    return outcome;
                };
                if current.is_empty() {
                    continue;
                }
                outcome.attempted.extend(current);
                sent.insert(*id);
                result = if action == "read" {
                    session.client.mark_read(&[*id]).await
                } else {
                    session.client.mark_unread(&[*id]).await
                };
                if result.is_err() {
                    break;
                }
            }
            statuses.retain(|status| sent.contains(&status.entry_id));
            ids.retain(|id| sent.contains(id));
            result
        } else {
            outcome
                .attempted
                .extend(statuses.iter().map(|status| status.queue_id));
            let Backend::GReader(client) = &session.client else {
                unreachable!("Fever handled above")
            };
            let guard =
                plan.request_guard(db, session, statuses.iter().map(|status| status.queue_id));
            let read = [crate::greader::tags::READ];
            let (add, remove): (&[&str], &[&str]) = if action == "read" {
                (&read, &[])
            } else {
                (&[], &read)
            };
            client.edit_tag_checked(&ids, add, remove, guard).await
        };
        if statuses.is_empty() {
            continue;
        }
        if !session_is_current(db, session).await {
            log::warn!("sync: 推送响应返回时账号代际已变更，丢弃该批次确认（{action}）");
            return outcome;
        }
        match result {
            Ok(()) => {
                let states = match readback_states(db, session, plan, &ids).await {
                    Ok(states) => states,
                    Err(e) => {
                        if !session_is_current(db, session).await {
                            return outcome;
                        }
                        let summary =
                            format!("{action} 写回后读回失败（服务端已 ACK，目标态未确认）: {e}");
                        for status in &statuses {
                            record_failure(
                                &mut outcome,
                                report,
                                status.queue_id,
                                summary.clone(),
                                PushFailureClass::Verification,
                                e.is_auth_failure(),
                            );
                        }
                        continue;
                    }
                };
                if !session_is_current(db, session).await {
                    log::warn!("sync: 状态读回返回时账号代际已变更，丢弃该批次确认（{action}）");
                    return outcome;
                }
                let Some(current) = current_push_items(
                    db,
                    session,
                    plan,
                    statuses.iter().map(|s| s.queue_id),
                    report,
                )
                .await
                else {
                    return outcome;
                };
                statuses.retain(|status| current.contains(&status.queue_id));
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
                let summary = format!("状态推送失败: {e}");
                for status in &statuses {
                    record_failure(
                        &mut outcome,
                        report,
                        status.queue_id,
                        summary.clone(),
                        PushFailureClass::Push,
                        e.is_auth_failure(),
                    );
                }
            }
        }
    }

    // 收藏：star/unstar 逐条发送并逐条读回。GReader 是绝对 add/remove；Fever
    // 在 api_version<4 时先探测再发送，避免 toggle 重发翻反。
    for (qid, remote_id, want_star) in &plan.stars {
        let Some(current) = current_push_items(db, session, plan, [*qid], report).await else {
            return outcome;
        };
        if !current.contains(qid) {
            continue;
        }
        outcome.attempted.insert(*qid);
        if !session_is_current(db, session).await {
            log::warn!("sync: 推送批次发送前检测到账号代际变更，停止后续推送（star）");
            return outcome;
        }
        let result = match &session.client {
            Backend::GReader(client) => {
                let guard = plan.request_guard(db, session, [*qid]);
                let star = [crate::greader::tags::STARRED];
                let (add, remove): (&[&str], &[&str]) = if *want_star {
                    (&star, &[])
                } else {
                    (&[], &star)
                };
                let r = client
                    .edit_tag_checked(&[*remote_id], add, remove, guard)
                    .await;
                r.map(|_| true)
            }
            Backend::Fever(c) => match c.star_state_needs_write(*remote_id, *want_star).await {
                Ok(false) => Ok(false),
                Ok(true) => {
                    let Some(current) = current_push_items(db, session, plan, [*qid], report).await
                    else {
                        return outcome;
                    };
                    if !current.contains(qid) {
                        continue;
                    }
                    if *want_star {
                        c.mark_starred(&[*remote_id]).await.map(|()| true)
                    } else {
                        c.mark_unstarred(&[*remote_id]).await.map(|()| true)
                    }
                }
                Err(error) => Err(error),
            },
        };
        if !session_is_current(db, session).await {
            log::warn!("sync: 推送响应返回时账号代际已变更，丢弃该批次确认（star）");
            return outcome;
        }
        match result {
            Ok(_sent) => {
                let states = match readback_states(db, session, plan, &[*remote_id]).await {
                    Ok(states) => states,
                    Err(e) => {
                        if !session_is_current(db, session).await {
                            return outcome;
                        }
                        record_failure(
                            &mut outcome,
                            report,
                            *qid,
                            format!(
                                "收藏写回后读回失败（entry {remote_id}，服务端已 ACK，目标态未确认）: {e}"
                            ),
                            PushFailureClass::Verification,
                            e.is_auth_failure(),
                        );
                        continue;
                    }
                };
                if !session_is_current(db, session).await {
                    log::warn!("sync: 收藏读回返回时账号代际已变更，丢弃该批次确认（star）");
                    return outcome;
                }
                let Some(current) = current_push_items(db, session, plan, [*qid], report).await
                else {
                    return outcome;
                };
                if !current.contains(qid) {
                    continue;
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
                        false,
                    ),
                }
            }
            Err(e) => {
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
                    e.is_auth_failure(),
                );
            }
        }
    }
    report.pushed_states += verified_targets;
    outcome
}

/// 即时状态推送：只推 sync_queue（read/unread/star/unstar 的精确远端目标），
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

/// Publish committed flags immediately, including inside a manual content round.
/// Waiting for that round to finish would reintroduce the slow-body UI coupling.
pub(super) fn notify_state_data_changed(report: &mut SyncReport) {
    let Some(app) = QUEUE_EVENT_TARGET.get() else {
        return;
    };
    if !report.has_data_change() {
        return;
    }
    use tauri::Emitter;
    if app
        .emit(
            "feeds-updated",
            serde_json::json!({
                "new_articles": 0, "failed_feeds": 0,
                "pulled_entries": report.pulled_entries, "merged_states": report.merged_states,
            }),
        )
        .is_err()
    {
        report
            .errors
            .push("状态已写入本机，但界面更新通知失败，请重试刷新".into());
    }
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
    let payload = crate::state::database_work(db.clone(), |conn| {
        // OPT-014 R1：读取/解密失败按「已配置」的显示口径处理（Err ≠ 未配置）——
        // 让等待/失败统计照常发给前端（错误原因由推送路径的 last_error 呈现）；
        // 只有确证未配置才静默（不误报、不刷状态）。
        let configured = match crate::sync::read_credentials(conn) {
            Ok(None) => false,
            Ok(Some(_)) => true,
            Err(e) => {
                log::warn!("sync: 凭据读取失败，队列状态按已配置口径上报: {e}");
                true
            }
        };
        Ok(db::queue_changed_payload(
            &db::sync_queue_stats(conn)?,
            configured,
        ))
    })
    .await;
    let payload = match payload {
        Ok(payload) => payload,
        Err(e) => {
            log::warn!("sync: 读取队列统计失败，跳过本轮 sync-queue-changed: {e}");
            emit_sync_error(&format!("同步队列状态读取失败，请重试：{e}"));
            None
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
    let marked = crate::state::database_work(db.clone(), move |conn| {
        db::mark_push_blocked(conn, &summary)
    })
    .await;
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
        .run_in(runtime::SyncLane::Push, "push", async move {
            push_states_now_inner(&db, &http).await
        })
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
            let mut retry_report = push_states_with_session(db, &fresh).await?;
            retry_report.preserve_changes_from(&report);
            report = retry_report;
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
    let generation = session.generation;
    let planned = crate::state::database_work(db.clone(), move |conn| {
        let _writer = db::writer_guard(conn)?;
        // OPT-006：计划构建是回写前边界（age_stale_queue 会写库）——代际失配即弃。
        if SyncSession::ensure_generation(conn, generation).is_err() {
            log::info!("sync: 即时推送计划构建时账号代际已变更，丢弃本轮");
            return Err(SyncSession::stale_error());
        }
        let mut report = SyncReport::default();
        let stale = age_stale_queue(conn, &mut report);
        Ok(match plan_push(conn) {
            Ok(p) => Ok((p, report, stale)),
            Err(e) => Err(e),
        })
    })
    .await?;
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
    let mut outcome = exec_push(db, session, &plan, &mut report).await;
    let (report, outcome, infra_errors) = crate::state::database_work(db.clone(), move |conn| {
        let _writer = db::writer_guard(conn)?;
        let mut infra_errors: Vec<String> = Vec::new();
        // OPT-006：确认/失败标记/剪除都是回写边界——账号已变更时旧队项已清空，
        // 不得把旧批次的记账落回（更不得动新账号的队项）。
        if SyncSession::ensure_generation(conn, generation).is_err() {
            log::info!("sync: 即时推送回写时账号代际已变更，丢弃确认与标记");
            return Err(SyncSession::stale_error());
        }
        outcome.discard_stale(conn, &plan, &mut report)?;
        // TASK-116：失败项落库标记（attempts+1 / last_error）——即时推送失败此前
        // 只进日志，用户在 UI 上永远看不到「部分失败」。标记失败不影响队列保留。
        let retry_push = outcome.failures_for(PushFailureClass::Push);
        let retry_verification = outcome.failures_for(PushFailureClass::Verification);
        let target_missing = outcome.failures_for(PushFailureClass::TargetMissing);
        if !retry_push.is_empty() {
            if let Err(e) = db::mark_push_failed(conn, &retry_push) {
                let message = format!("队列推送失败标记落库失败: {e}");
                log::warn!("sync: {message}");
                report.errors.push(message.clone());
                infra_errors.push(message);
            }
        }
        if !retry_verification.is_empty() {
            if let Err(e) = db::mark_push_verification_failed(conn, &retry_verification) {
                let message = format!("队列失败标记落库失败: {e}");
                log::warn!("sync: {message}");
                report.errors.push(message.clone());
                infra_errors.push(message);
            }
        }
        if !target_missing.is_empty() {
            if let Err(e) = db::mark_queue_target_missing(conn, &target_missing) {
                let message = format!("target_missing 标记落库失败: {e}");
                log::warn!("sync: {message}");
                report.errors.push(message.clone());
                infra_errors.push(message);
            }
        }
        if !outcome.done.is_empty() {
            if let Err(e) = db::prune_sync(conn, &outcome.done) {
                let message = format!("已推送队项出队失败: {e}");
                log::warn!("sync: {message}");
                report.errors.push(message.clone());
                infra_errors.push(message);
            }
        }
        Ok((report, outcome, infra_errors))
    })
    .await?;
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

    /// A previously content-proven binding enters the outgoing plan.
    #[test]
    fn bound_article_still_enters_push_plan() {
        let c = conn();
        c.execute_batch(
            "INSERT INTO feeds (feed_url, title, remote_id)
             VALUES ('https://plan.example/rss', 'P', 10);
             INSERT INTO articles (feed_id, guid, title, remote_id, remote_origin_feed_id)
             VALUES (1, 'g-bound', 'P', 101, 10);
             INSERT INTO sync_queue (article_id, action)
             VALUES (1, 'read');",
        )
        .unwrap();
        let plan = plan_push(&c).unwrap();
        assert_eq!(plan.status.len(), 1, "绑定项必须进入推送计划");
        assert_eq!(plan.status[0].entry_id, 101);
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
