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

/// 锁外：执行推送计划。返回成功清除的队列 id（失败项保留 → 天然重试）。
pub(super) async fn exec_push(
    client: &Backend,
    plan: &PushPlan,
    report: &mut SyncReport,
) -> Vec<i64> {
    let mut done: Vec<i64> = Vec::new();
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
            Err(e) => report.errors.push(format!("状态推送失败: {e}")),
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
            Err(e) => report
                .errors
                .push(format!("收藏同步失败: entry {remote_id}: {e}")),
        }
    }
    done
}

/// 即时状态推送：只推 sync_queue（read/unread/star/unstar + 副本广播），
/// 不做任何 pull。set_read/set_starred 变更后 ~1s 内到达服务端。
/// 失败静默（队列保留，下轮同步重推）——后台同步不打扰用户。
/// 队列保留期（A-8）：超过该天数仍无远端绑定的状态项视为无法收敛，清理并记录。
const QUEUE_RETENTION_DAYS: i64 = 30;

/// 老化清理（A-8）：无远端绑定的状态队列项此前会永久滞留（plan_push 跳过但
/// 保留、pending 保护长期存在、队列无 TTL）。清理结果计入 report 便于诊断。
pub(super) fn age_stale_queue(conn: &Connection, report: &mut SyncReport) {
    // 与 sync_queue.created_at 同格式（SQLite datetime('now')：UTC 无时区后缀）
    let cutoff = (Utc::now() - chrono::Duration::days(QUEUE_RETENTION_DAYS))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    match db::prune_stale_unbound(conn, &cutoff) {
        Ok(n) if n > 0 => report.errors.push(format!(
            "队列老化：清理 {n} 条超过 {QUEUE_RETENTION_DAYS} 天仍未绑定远端的状态变更（本地状态保留，但不再尝试推送）"
        )),
        Ok(_) => {}
        Err(e) => report.errors.push(format!("队列老化清理失败: {e}")),
    }
}

pub async fn push_states_now(db: &Arc<Mutex<Connection>>, http: &reqwest::Client) {
    let Some(client) = build_client(db, http).await else {
        return;
    };
    // 串行化：与 states_phase/feeds_phase 的推送段互斥（见 PUSH_LOCK 注释）
    let _guard = PUSH_LOCK.lock().await;
    let (plan, mut report) = {
        let conn = db.lock().await;
        let mut report = SyncReport::default();
        age_stale_queue(&conn, &mut report);
        match plan_push(&conn) {
            Ok(p) => (p, report),
            Err(e) => {
                log::warn!("sync: 读队列失败: {e}");
                return;
            }
        }
    };
    if plan.status.is_empty() && plan.stars.is_empty() {
        return;
    }
    let done = exec_push(&client, &plan, &mut report).await;
    if !done.is_empty() {
        let conn = db.lock().await;
        if let Err(e) = db::prune_sync(&conn, &done) {
            log::warn!("sync: 清队列失败: {e}");
        }
    }
    if !report.errors.is_empty() {
        log::info!("sync: 即时推送失败（队列保留待重推）: {:?}", report.errors);
    } else {
        log::info!("sync: 即时推送 {} 项状态", report.pushed_states);
    }
}
