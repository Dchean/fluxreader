//! sync 的 push 子模块（TASK-045 从 sync.rs 按既有章节拆分）。

use super::*;
use crate::db;
use crate::error::AppResult;
use chrono::Utc;
use rusqlite::Connection;
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
}

pub(super) struct PushStatus {
    queue_id: i64,
    action: String,
    entry_ids: Vec<i64>,
}

/// 锁内：解析 sync_queue → 推送计划。
/// 条目未绑定 entry 的跳过（保留在队列，Pull 的绑定回填会补上，直接丢弃
/// 会让"已读"在服务端永久丢失）。
pub(super) fn plan_push(conn: &Connection) -> AppResult<PushPlan> {
    let items = db::take_sync_queue(conn)?;
    let mut plan = PushPlan {
        status: Vec::new(),
        stars: Vec::new(),
    };
    for item in items {
        let Some(article_id) = item.article_id else {
            continue; // feed 级动作（add_feed）在 push_feeds 阶段处理
        };
        let remote_id = db::get_article_remote_id(conn, article_id).ok().flatten();
        let Some(remote_id) = remote_id else {
            continue;
        };
        match item.action.as_str() {
            // 已读广播：绑定的 entry + 记账的全部同文副本 entry 一并标读
            // （双端场景：Read You 不去重，手机上另一源的副本也要已读，
            // 否则手机读完这篇、那个源里又冒出来一篇未读的"同一篇"）
            // TASK-112 政策：本分支是「同文副本读状态传播 = read 广播保持现状」
            // 的消费点（政策定义与 DEC-refactor-roadmap-20261005 第 6 条原文见
            // conflict_policy.rs 头注；unread/star/unstar 不广播）。
            "read" => {
                let mut ids = vec![remote_id];
                for dup in db::article_dup_entries(conn, article_id).unwrap_or_default() {
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

/// 锁外：执行推送计划。返回 (成功清除的队列 id, 失败项 (队列 id, 错误摘要))。
/// 失败项保留在队列（天然重试），由调用方落库标记 attempts/last_error
/// （TASK-116 四态展示；db::mark_push_failed）——本函数保持无 DB 访问，
/// 与「HTTP 全在锁外、DB 读写锁内短临界区」的锁纪律一致。
/// read 广播聚合为单请求：任一 entry 失败则该组全部 queue id 同记一条摘要。
pub(super) async fn exec_push(
    client: &Backend,
    plan: &PushPlan,
    report: &mut SyncReport,
) -> (Vec<i64>, Vec<(i64, String)>) {
    let mut done: Vec<i64> = Vec::new();
    let mut failed: Vec<(i64, String)> = Vec::new();
    // read/unread 聚合批量（Google Reader edit-tag 单请求可携带全部 id + tag）
    for action in ["read", "unread"] {
        let ids: Vec<i64> = plan
            .status
            .iter()
            .filter(|s| s.action == action)
            .flat_map(|s| s.entry_ids.iter().copied())
            .collect::<Vec<_>>();
        if ids.is_empty() {
            continue;
        }
        let result = if action == "read" {
            client.mark_read(&ids).await
        } else {
            client.mark_unread(&ids).await
        };
        match result {
            Ok(()) => {
                report.pushed_states += ids.len();
                done.extend(
                    plan.status
                        .iter()
                        .filter(|s| s.action == action)
                        .map(|s| s.queue_id),
                );
            }
            Err(e) => {
                // 聚合报告与 per-item 摘要同文：errors 口径逐字保持修前形态
                let summary = format!("状态推送失败: {e}");
                report.errors.push(summary.clone());
                failed.extend(
                    plan.status
                        .iter()
                        .filter(|s| s.action == action)
                        .map(|s| (s.queue_id, summary.clone())),
                );
            }
        }
    }
    // 收藏：star/unstar（Google Reader 有明确的 add/remove 语义，非 toggle）
    for (qid, remote_id, want_star) in &plan.stars {
        let result = if *want_star {
            client.mark_starred(&[*remote_id]).await
        } else {
            client.mark_unstarred(&[*remote_id]).await
        };
        match result {
            Ok(()) => {
                report.pushed_states += 1;
                done.push(*qid);
            }
            Err(e) => {
                let summary = format!("收藏同步失败: entry {remote_id}: {e}");
                report.errors.push(summary.clone());
                failed.push((*qid, summary));
            }
        }
    }
    (done, failed)
}

/// 即时状态推送：只推 sync_queue（read/unread/star/unstar + 副本广播），
/// 不做任何 pull。set_read/set_starred 变更后 ~1s 内到达服务端。
/// 失败静默（队列保留，下轮同步重推）——后台同步不打扰用户。
/// 队列保留期（A-8）：超过该天数仍无远端绑定的状态项视为无法收敛，清理并记录。
const QUEUE_RETENTION_DAYS: i64 = 30;

/// 老化清理（A-8）：无远端绑定的状态队列项此前会永久滞留（plan_push 跳过但
/// 保留、pending 保护长期存在、队列无 TTL）。清理结果计入 report 便于诊断。
/// TASK-124：返回清理条数（>0 = 队列统计已变化，调用方据此发 sync-queue-changed）。
pub(super) fn age_stale_queue(conn: &Connection, report: &mut SyncReport) -> usize {
    // 与 sync_queue.created_at 同格式（SQLite datetime('now')：UTC 无时区后缀）
    let cutoff = (Utc::now() - chrono::Duration::days(QUEUE_RETENTION_DAYS))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    match db::prune_stale_unbound(conn, &cutoff) {
        Ok(n) if n > 0 => {
            report.errors.push(format!(
                "队列老化：清理 {n} 条超过 {QUEUE_RETENTION_DAYS} 天仍未绑定远端的状态变更（本地状态保留，但不再尝试推送）"
            ));
            n
        }
        Ok(_) => 0,
        Err(e) => {
            report.errors.push(format!("队列老化清理失败: {e}"));
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
/// （payload = `{waiting, failed, last_error}`，经 db::queue_changed_payload 单点）。
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
        let configured = crate::sync::read_credentials(&conn).is_some();
        db::sync_queue_stats(&conn)
            .ok()
            .and_then(|s| db::queue_changed_payload(&s, configured))
    };
    if let Some(p) = payload {
        use tauri::Emitter;
        let _ = app.emit("sync-queue-changed", p);
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
        Err(e) => log::warn!("sync: 阻塞标记落库失败: {e}"),
    }
}

pub async fn push_states_now(db: &Arc<Mutex<Connection>>, http: &reqwest::Client) {
    let client = match build_client(db, http).await {
        Ok(c) => c,
        // 未配置：静默返回（队列保留，连接后补推）——不发事件不报警（A-5 语义不变）
        Err(ClientBuildFailure::NotConfigured) => return,
        // TASK-124：认证/端点/网络失败——记录阻塞标记 + 发事件（此前在此提前返回，
        // 用户在 UI 上永远看不到「为什么一直不同步」，审计 P2-6②）
        Err(ClientBuildFailure::Failed(e)) => {
            record_push_block(db, &e).await;
            return;
        }
    };
    // 串行化：与 states_phase/feeds_phase 的推送段互斥（见 PUSH_LOCK 注释）
    let _guard = PUSH_LOCK.lock().await;
    let (plan, mut report, stale) = {
        let conn = db.lock().await;
        let mut report = SyncReport::default();
        let stale = age_stale_queue(&conn, &mut report);
        match plan_push(&conn) {
            Ok(p) => (p, report, stale),
            Err(e) => {
                log::warn!("sync: 读队列失败: {e}");
                return;
            }
        }
    };
    if plan.status.is_empty() && plan.stars.is_empty() {
        // 无可推项：仅老化清理改变了统计时发事件（waiting 已下降）
        if stale > 0 {
            notify_queue_changed(db).await;
        }
        return;
    }
    let (done, failed) = exec_push(&client, &plan, &mut report).await;
    {
        let conn = db.lock().await;
        // TASK-116：失败项落库标记（attempts+1 / last_error）——即时推送失败此前
        // 只进日志，用户在 UI 上永远看不到「部分失败」。标记失败不影响队列保留。
        if !failed.is_empty() {
            if let Err(e) = db::mark_push_failed(&conn, &failed) {
                log::warn!("sync: 推送失败标记落库失败: {e}");
            }
        }
        if !done.is_empty() {
            if let Err(e) = db::prune_sync(&conn, &done) {
                log::warn!("sync: 清队列失败: {e}");
            }
        }
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
