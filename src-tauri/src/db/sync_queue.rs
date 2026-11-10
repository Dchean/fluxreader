// Note: 「远端已确认」不可累计溯源，不得虚构总数 — 见 .agents/notes/implemented/feature/2026-10-06-同步四态与失败可见性.md
// Note: 队列生命周期、local-only 保留与 waiting 口径 — 见 .agents/notes/implemented/architecture/2026-11-03-同步队列生命周期与本地意图.md
use super::*;
use rusqlite::params;
use rusqlite::OptionalExtension;
use serde::Serialize;

/// 离线变更队列条目
#[derive(Debug, Serialize)]
pub struct SyncQueueItem {
    pub id: i64,
    pub article_id: Option<i64>,
    pub feed_url: Option<String>,
    pub action: String,
    pub payload: Option<String>,
    /// 入队时间（SQLite datetime('now')，UTC 'YYYY-MM-DD HH:MM:SS'）。
    /// A-8：无远端绑定项的老化转态依据（不是删除依据）。
    pub created_at: String,
    /// article_state | legacy_add_feed | legacy_other
    pub kind: String,
    /// local_only | bindable_retry | bound_pending | retry_wait | failed |
    /// expired_unconfirmed
    pub status: String,
    pub unbound_reason: Option<String>,
}

pub const STATUS_LOCAL_ONLY: &str = "local_only";
pub const STATUS_BINDABLE_RETRY: &str = "bindable_retry";
pub const STATUS_BOUND_PENDING: &str = "bound_pending";
pub const STATUS_RETRY_WAIT: &str = "retry_wait";
pub const STATUS_FAILED: &str = "failed";
pub const STATUS_EXPIRED_UNCONFIRMED: &str = "expired_unconfirmed";

/// 本地变更入队（已读/收藏等）。同一条目同向的旧记录先删，避免重复推送。
pub fn enqueue_sync(
    conn: &Connection,
    article_id: Option<i64>,
    feed_url: Option<&str>,
    action: &str,
    payload: Option<&str>,
) -> AppResult<()> {
    let (kind, field, desired_state, status, unbound_reason) =
        if matches!(action, "read" | "unread" | "star" | "unstar") {
            let field = if matches!(action, "read" | "unread") {
                "read"
            } else {
                "star"
            };
            let desired_state = if matches!(action, "read" | "star") {
                1
            } else {
                0
            };
            let (status, reason) = match article_id {
                Some(aid) => {
                    let Some((article_remote_id, feed_remote_id)) =
                        article_queue_identity(conn, aid)?
                    else {
                        return Err(crate::error::AppError::internal(format!(
                            "同步入队失败：文章 {aid} 不存在"
                        )));
                    };
                    if article_remote_id.is_some() {
                        (STATUS_BOUND_PENDING, None)
                    } else if feed_remote_id.is_some() {
                        (STATUS_BINDABLE_RETRY, Some("awaiting_remote_identity"))
                    } else {
                        (STATUS_LOCAL_ONLY, Some("no_remote_identity"))
                    }
                }
                None => (STATUS_LOCAL_ONLY, Some("no_article_identity")),
            };
            (
                "article_state",
                Some(field),
                Some(desired_state),
                status,
                reason,
            )
        } else if action == "add_feed" {
            let (status, reason) = if feed_url.is_some_and(|url| !url.trim().is_empty()) {
                (STATUS_BOUND_PENDING, None)
            } else {
                (STATUS_LOCAL_ONLY, Some("no_feed_url"))
            };
            ("legacy_add_feed", None, None, status, reason)
        } else {
            (
                "legacy_other",
                None,
                None,
                STATUS_LOCAL_ONLY,
                Some("unsupported_legacy_action"),
            )
        };
    let last_error_class = match status {
        STATUS_LOCAL_ONLY | STATUS_BINDABLE_RETRY => Some("unbound"),
        _ => None,
    };

    // 同 article 的 read/unread 或 star/unstar 只保留最新一条；feed 级行不合并。
    if let Some(aid) = article_id {
        conn.execute(
            "DELETE FROM sync_queue WHERE article_id = ?1 AND action IN (?2, ?3)",
            params![aid, opposite_action(action), action],
        )?;
    }
    // created_at 用 SQLite datetime('now')（UTC，'YYYY-MM-DD HH:MM:SS'）——与建表
    // 默认值及历史数据同格式，老化比较才能按字符串正确排序
    conn.execute(
        "INSERT INTO sync_queue
            (article_id, feed_url, action, payload, created_at, kind, field,
             desired_state, status, updated_at, unbound_reason, last_error_class)
         VALUES (?1, ?2, ?3, ?4, datetime('now'), ?5, ?6, ?7, ?8,
                 datetime('now'), ?9, ?10)",
        params![
            article_id,
            feed_url,
            action,
            payload,
            kind,
            field,
            desired_state,
            status,
            unbound_reason,
            last_error_class
        ],
    )?;
    Ok(())
}

/// 文章及其所属 feed 的远端身份快照。`None` 表示文章不存在；`(None, None)`
/// 表示文章存在、自身与 feed 都没有远端绑定。
pub fn article_queue_identity(
    conn: &Connection,
    article_id: i64,
) -> AppResult<Option<(Option<i64>, Option<i64>)>> {
    let row: Option<(Option<i64>, Option<i64>)> = conn
        .query_row(
            "SELECT a.remote_id, f.remote_id
               FROM articles a
               JOIN feeds f ON f.id = a.feed_id
              WHERE a.id = ?1",
            params![article_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(row)
}

/// read↔unread / star↔unstar 的反向动作（入队去重用）
fn opposite_action(action: &str) -> &str {
    match action {
        "read" => "unread",
        "unread" => "read",
        "star" => "unstar",
        "unstar" => "star",
        _ => "",
    }
}

/// 取出全部待推送条目（不删除；成功后由 prune_sync 清除）
pub fn take_sync_queue(conn: &Connection) -> AppResult<Vec<SyncQueueItem>> {
    let mut stmt = conn.prepare(
        "SELECT id, article_id, feed_url, action, payload, created_at, kind, status,
                unbound_reason
           FROM sync_queue ORDER BY id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(SyncQueueItem {
            id: r.get(0)?,
            article_id: r.get(1)?,
            feed_url: r.get(2)?,
            action: r.get(3)?,
            payload: r.get(4)?,
            created_at: r.get(5)?,
            kind: r.get(6)?,
            status: r.get(7)?,
            unbound_reason: r.get(8)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 清除已成功推送的队列条目（远端确认 = 按 id 精确出队）。
/// 精确确认的前提是**操作 id 在删除后不复用**：推送计划取快照后即释放 DB
/// 锁，网络往返只由 PUSH_LOCK 串行保护，而本地入队不经过 PUSH_LOCK——窗口内
/// 复用 id 会让旧计划返回后按 id prune 误删新的用户意图（审计 F01；v18 起
/// sync_queue 为 AUTOINCREMENT，见 db/migrations.rs 的 v18 迁移）。
// Note: 操作 id 不复用是旧推送计划按 id 精确确认的前提 — 见 .agents/notes/implemented/bug-fix/2026-09-18-状态写入事务化与对账守卫.md
pub fn prune_sync(conn: &Connection, ids: &[i64]) -> AppResult<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let mut stmt = conn.prepare(&format!(
        "SELECT article_id FROM sync_queue WHERE id IN ({placeholders}) AND article_id IS NOT NULL"
    ))?;
    let articles = stmt
        .query_map(rusqlite::params_from_iter(ids.iter()), |r| {
            r.get::<_, i64>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    conn.execute(
        &format!("DELETE FROM sync_queue WHERE id IN ({placeholders})"),
        rusqlite::params_from_iter(ids.iter()),
    )?;
    super::articles::prune_retained_confirmed(conn, &articles)?;
    Ok(())
}

/// 清掉历史版本残留的 remove_feed 僵尸队项（该动作从未被 sync 消费；现行
/// 删除语义为「本地删除不推远端」，见 delete_feed 命令）。返回清除条数。
pub fn purge_remove_feed_zombies(conn: &Connection) -> AppResult<usize> {
    let n = conn.execute("DELETE FROM sync_queue WHERE action = 'remove_feed'", [])?;
    Ok(n)
}

/// 老化处置（A-8 修订）：超过保留期、仍无远端绑定的文章状态意图转为
/// `expired_unconfirmed`，**不物理删除**。本地文章 read/star 值与队列行都保留，
/// 用户仍能看到「已停止自动重试、本地状态仍在」；add_feed 由 feeds 阶段处理。
/// `cutoff` 与 created_at 同为 'YYYY-MM-DD HH:MM:SS'（UTC）。返回转态条数。
pub fn prune_stale_unbound(conn: &Connection, cutoff: &str) -> AppResult<usize> {
    let n = conn.execute(
        "UPDATE sync_queue
            SET status = 'expired_unconfirmed',
                unbound_reason = COALESCE(unbound_reason, 'expired_unconfirmed'),
                last_error_class = 'expired_unconfirmed',
                updated_at = datetime('now')
          WHERE created_at < ?1
            AND kind = 'article_state'
            AND status IN ('local_only', 'bindable_retry')
            AND (article_id IS NULL
                 OR article_id IN (SELECT id FROM articles WHERE remote_id IS NULL))",
        params![cutoff],
    )?;
    Ok(n)
}

#[derive(Debug, Default, Serialize)]
pub struct ExpiredRetryResult {
    pub requeued: usize,
    pub bound_pending: usize,
    pub bindable_retry: usize,
    pub local_only: usize,
    pub remaining_expired: usize,
}

// Note: 恢复复核摘要代际、按当前本地目标创建新操作身份 — 见 .agents/notes/implemented/bug-fix/2026-11-05-GR权威集合回补与过期意图恢复.md
pub fn retry_expired_intents(
    conn: &mut Connection,
    expected_account_generation: i64,
) -> AppResult<ExpiredRetryResult> {
    let tx = conn.transaction()?;
    if sync_generation(&tx)? != expected_account_generation {
        return Err(crate::error::AppError::new(
            "staleAccount",
            "同步账号已变更，本次未重试过期项；请刷新后重试",
        ));
    }
    let candidates: Vec<(i64, String, bool)> = {
        let mut stmt = tx.prepare(
            "SELECT DISTINCT a.id, q.field,
                    CASE q.field WHEN 'read' THEN a.is_read ELSE a.is_starred END
               FROM sync_queue q JOIN articles a ON a.id = q.article_id
              WHERE q.kind = 'article_state' AND q.status = 'expired_unconfirmed'
                AND q.field IN ('read', 'star')
              ORDER BY a.id, q.field",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let mut result = ExpiredRetryResult::default();
    for (article_id, field, desired) in candidates {
        let action = match (field.as_str(), desired) {
            ("read", true) => "read",
            ("read", false) => "unread",
            (_, true) => "star",
            (_, false) => "unstar",
        };
        enqueue_sync(&tx, Some(article_id), None, action, None)?;
        let status: String = tx.query_row(
            "SELECT status FROM sync_queue WHERE article_id = ?1 AND action = ?2",
            params![article_id, action],
            |row| row.get(0),
        )?;
        match status.as_str() {
            STATUS_BOUND_PENDING => result.bound_pending += 1,
            STATUS_BINDABLE_RETRY => result.bindable_retry += 1,
            _ => result.local_only += 1,
        }
        result.requeued += 1;
    }
    result.remaining_expired = tx.query_row(
        "SELECT COUNT(*) FROM sync_queue WHERE status = 'expired_unconfirmed'",
        [],
        |row| row.get(0),
    )?;
    tx.commit()?;
    Ok(result)
}

/* ============================================================
TASK-116（同步四态展示）：失败标记 + 队列统计
============================================================ */

/// last_error 摘要的最大长度（字符数）。错误摘要进 UI「部分失败（最新错误）」
/// 单行展示，超长只留开头——错误尾巴通常是冗余的堆栈/URL 细节。
const LAST_ERROR_MAX_CHARS: usize = 200;

/// 错误摘要截断：按字符（码点）截断保证多字节安全，截断后以「…」示意。
/// sync_queue 与订阅 outbox（db/outbox.rs）共用同一口径，故对 db 子模块可见。
pub(super) fn truncate_last_error(err: &str) -> String {
    if err.chars().count() <= LAST_ERROR_MAX_CHARS {
        return err.to_string();
    }
    let head: String = err.chars().take(LAST_ERROR_MAX_CHARS).collect();
    format!("{head}…")
}

/// 推送失败项落库标记（TASK-116）：attempts +1、状态转 retry_wait 并记录错误摘要。
/// 队列行保留（失败 = 天然重试语义，见 sync/push.rs）；成功路径不走这里——
/// 成功项由 prune_sync 整行删除，attempts/last_error 随之消失（stats 归零）。
/// 状态变更重新入队（enqueue_sync 删旧插新）也会重置为未失败。
pub fn mark_push_failed(conn: &Connection, failed: &[(i64, String)]) -> AppResult<()> {
    if failed.is_empty() {
        return Ok(());
    }
    let mut stmt = conn.prepare(
        "UPDATE sync_queue
            SET attempts = attempts + 1,
                last_error = ?1,
                last_error_class = 'push',
                status = CASE
                           WHEN status IN ('bound_pending', 'retry_wait') THEN 'retry_wait'
                           ELSE status
                         END,
                updated_at = datetime('now')
          WHERE id = ?2",
    )?;
    for (id, summary) in failed {
        stmt.execute(params![truncate_last_error(summary), id])?;
    }
    Ok(())
}

/// 写请求已被服务端 ACK、但状态读回失败/不一致：保留队列并进入 `retry_wait`。
///
/// 这类失败不是「没发出去」，所以不能伪装成成功，也不能直接 `prune_sync`；
/// 下一轮会重新按目标态收敛（GReader 是绝对 tag 语义，Fever 收藏先探测再发送）。
pub fn mark_push_verification_failed(conn: &Connection, failed: &[(i64, String)]) -> AppResult<()> {
    if failed.is_empty() {
        return Ok(());
    }
    let mut stmt = conn.prepare(
        "UPDATE sync_queue
            SET attempts = attempts + 1,
                last_error = ?1,
                last_error_class = 'verification',
                status = 'retry_wait',
                updated_at = datetime('now')
          WHERE id = ?2
            AND status NOT IN ('failed', 'expired_unconfirmed')",
    )?;
    for (id, summary) in failed {
        stmt.execute(params![truncate_last_error(summary), id])?;
    }
    Ok(())
}

/// 可证明目标条目不存在/已删除时使用：终态 `failed` + `target_missing`，保留
/// 本地读/藏意图供用户处理。当前 GReader/Fever 读回端点没有权威删除信号，
/// 200 空/短响应不得调用本入口；它保留给未来端点或明确的协议错误分类。
// Note: 该入口只接受可证明的目标不存在；空/短响应走 verification/inconclusive — 见
// .agents/notes/implemented/bug-fix/2026-11-03-状态写回ACK验证与条目三态.md
pub fn mark_queue_target_missing(conn: &Connection, failed: &[(i64, String)]) -> AppResult<()> {
    if failed.is_empty() {
        return Ok(());
    }
    let mut stmt = conn.prepare(
        "UPDATE sync_queue
            SET attempts = attempts + 1,
                last_error = ?1,
                last_error_class = 'target_missing',
                status = 'failed',
                unbound_reason = NULL,
                updated_at = datetime('now')
          WHERE id = ?2
            AND status NOT IN ('expired_unconfirmed')",
    )?;
    for (id, summary) in failed {
        stmt.execute(params![truncate_last_error(summary), id])?;
    }
    Ok(())
}

/// 手动/全量同步的显式重试入口：把 `target_missing` 终态重新放回可推送状态。
///
/// 自动同步不会调用；只有用户主动触发的 full states 阶段才重试，避免后台
/// 对已删除目标无限刷请求。保留 attempts，清掉上次摘要，失败后重新落
/// `failed/target_missing`。
pub fn retry_target_missing(conn: &Connection) -> AppResult<usize> {
    let n = conn.execute(
        "UPDATE sync_queue
            SET status = 'bound_pending',
                last_error = NULL,
                last_error_class = NULL,
                updated_at = datetime('now')
          WHERE status = 'failed'
            AND last_error_class = 'target_missing'",
        [],
    )?;
    Ok(n)
}

/// 推送阻塞标记（TASK-124，审计 P2-6②）：后端客户端构建失败（认证/端点解析/
/// 网络不可达）时，本轮推送**未进 exec_push**——此前 attempts/last_error 对这类
/// 失败永不记录，用户在 UI 上永远看不到「为什么一直不同步」。现在把**全部可推送
/// 队项**计一次失败尝试（attempts+1）并记录摘要——与 mark_push_failed 同口径
/// （attempts 累加、摘要按码点截断 200 字符）；摘要带「认证失败」/实际错误文案，
/// 由 sync::push_block_summary 分类。空队列为 no-op，返回标记行数（调用方据
/// n>0 决定是否发 sync-queue-changed——统计真变了才发，X3 不制造噪音）。
pub fn mark_push_blocked(conn: &Connection, summary: &str) -> AppResult<usize> {
    let summary = truncate_last_error(summary);
    let n = conn.execute(
        "UPDATE sync_queue
            SET attempts = attempts + 1,
                last_error = ?1,
                last_error_class = 'session_blocked',
                status = 'retry_wait',
                updated_at = datetime('now')
          WHERE status IN ('bound_pending', 'retry_wait')",
        params![summary],
    )?;
    Ok(n)
}

/// plan_push 的显式分类写回：没有远端身份的意图不静默跳过，而是把状态、
/// 原因与错误类别落库。`last_error` 只记录真实推送失败，分类本身既不递增
/// attempts，也不污染最近错误摘要。
pub fn mark_queue_identity_waiting(
    conn: &Connection,
    id: i64,
    status: &str,
    reason: &str,
) -> AppResult<()> {
    conn.execute(
        "UPDATE sync_queue
            SET status = ?1,
                unbound_reason = ?2,
                last_error = NULL,
                last_error_class = 'unbound',
                updated_at = datetime('now')
          WHERE id = ?3",
        params![status, reason, id],
    )?;
    Ok(())
}

/// plan_push 发现原无身份项已获得 remote_id（例如 pull 绑定完成后）时，
/// 将状态恢复到可推送。正常绑定由 articles.remote_id UPDATE 触发器先行完成；
/// 此函数同时覆盖升级后遗留的脏状态与直接 SQL 改写的边界。
pub fn mark_queue_bound_pending(conn: &Connection, id: i64) -> AppResult<()> {
    conn.execute(
        "UPDATE sync_queue
            SET status = 'bound_pending',
                unbound_reason = NULL,
                last_error = NULL,
                last_error_class = NULL,
                updated_at = datetime('now')
          WHERE id = ?1",
        params![id],
    )?;
    Ok(())
}

/// 队列统计（生命周期口径）：
/// - waiting = 真正可推送且尚未确认的 bound_pending + retry_wait；
/// - failed  = 终态 failed 行数；retry_wait 单独计数，前端可合并展示「部分失败」；
/// - local_only / bindable_retry / bound_pending / retry_wait / expired_unconfirmed
///   分别暴露，前端不必再从总行数猜测「仅本机/等待身份/过期」；
/// - last_error = retry_wait/failed 中最近一次失败摘要（按 updated_at，再按 id）。
#[derive(Debug, Serialize)]
pub struct SyncQueueStats {
    pub account_generation: i64,
    pub waiting: usize,
    pub failed: usize,
    pub local_only: usize,
    pub bindable_retry: usize,
    pub bound_pending: usize,
    pub retry_wait: usize,
    pub expired_unconfirmed: usize,
    pub last_error: Option<String>,
}

pub fn sync_queue_stats(conn: &Connection) -> AppResult<SyncQueueStats> {
    let (
        waiting,
        failed,
        local_only,
        bindable_retry,
        bound_pending,
        retry_wait,
        expired_unconfirmed,
        last_error,
    ): (i64, i64, i64, i64, i64, i64, i64, Option<String>) = conn.query_row(
        "SELECT (SELECT COUNT(*) FROM sync_queue
                  WHERE status IN ('bound_pending', 'retry_wait')),
                (SELECT COUNT(*) FROM sync_queue WHERE status = 'failed'),
                (SELECT COUNT(*) FROM sync_queue WHERE status = 'local_only'),
                (SELECT COUNT(*) FROM sync_queue WHERE status = 'bindable_retry'),
                (SELECT COUNT(*) FROM sync_queue WHERE status = 'bound_pending'),
                (SELECT COUNT(*) FROM sync_queue WHERE status = 'retry_wait'),
                (SELECT COUNT(*) FROM sync_queue WHERE status = 'expired_unconfirmed'),
                (SELECT last_error FROM sync_queue
                  WHERE status IN ('retry_wait', 'failed') AND last_error IS NOT NULL
                  ORDER BY updated_at DESC, id DESC LIMIT 1)",
        [],
        |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
            ))
        },
    )?;
    Ok(SyncQueueStats {
        account_generation: sync_generation(conn)?,
        waiting: waiting as usize,
        failed: failed as usize,
        local_only: local_only as usize,
        bindable_retry: bindable_retry as usize,
        bound_pending: bound_pending as usize,
        retry_wait: retry_wait as usize,
        expired_unconfirmed: expired_unconfirmed as usize,
        last_error,
    })
}

/* ============================================================
TASK-124（同步状态变化事件）：sync-queue-changed 事件 payload 单点
============================================================ */

/// `sync-queue-changed` 事件 payload（纯函数，wire 形态单点 + 可测）：
/// `{waiting, failed, local_only, ..., last_error}` 与 [`SyncQueueStats`] /
/// 前端 `SyncQueueStats`（src/lib/api.ts）三方同形。configured=false → None：
/// 未配置同步静默不发事件
/// （审计「区分未配置/认证失败/网络失败」的「未配置」半边——前端 pill 靠既有
/// 「本地模式 · 直连抓取」分支，不误报不刷状态）。
pub fn queue_changed_payload(
    stats: &SyncQueueStats,
    configured: bool,
) -> Option<serde_json::Value> {
    if !configured {
        return None;
    }
    Some(serde_json::json!({
        "account_generation": stats.account_generation,
        "waiting": stats.waiting,
        "failed": stats.failed,
        "local_only": stats.local_only,
        "bindable_retry": stats.bindable_retry,
        "bound_pending": stats.bound_pending,
        "retry_wait": stats.retry_wait,
        "expired_unconfirmed": stats.expired_unconfirmed,
        "last_error": stats.last_error,
    }))
}

/* ============================================================
文件级测试助手（#[cfg(test)]）：t116/t124 两个测试模块共用。
TASK-124 修复轮 R1：`attempts_of` 原私有定义在 `mod t116_tests` 内，
兄弟模块 `mod t124_tests` 引用它报 E0425（Rust 模块私有性：兄弟模块的
私有项互不可见）。上移到文件级后两模块经 `use super::*` 解析同一份。
============================================================ */
#[cfg(test)]
fn attempts_of(c: &Connection, id: i64) -> i64 {
    c.query_row(
        "SELECT attempts FROM sync_queue WHERE id = ?1",
        params![id],
        |r| r.get(0),
    )
    .unwrap()
}

#[cfg(test)]
fn seed_article(c: &Connection, feed_bound: bool, article_bound: bool) -> i64 {
    let seq: i64 = c
        .query_row("SELECT COALESCE(MAX(id), 0) + 1 FROM feeds", [], |r| {
            r.get(0)
        })
        .unwrap();
    c.execute(
        "INSERT INTO feeds (feed_url, title) VALUES (?1, '测试源')",
        params![format!("https://feed-{seq}.example/rss")],
    )
    .unwrap();
    let feed_id = c.last_insert_rowid();
    if feed_bound {
        c.execute(
            "UPDATE feeds SET remote_id = ?1 WHERE id = ?2",
            params![10_000 + feed_id, feed_id],
        )
        .unwrap();
    }
    c.execute(
        "INSERT INTO articles (feed_id, guid, title, is_read, is_starred)
         VALUES (?1, ?2, '测试文章', 1, 0)",
        params![feed_id, format!("guid-{feed_id}")],
    )
    .unwrap();
    let article_id = c.last_insert_rowid();
    if article_bound {
        c.execute(
            "UPDATE articles SET remote_id = ?1 WHERE id = ?2",
            params![20_000 + article_id, article_id],
        )
        .unwrap();
    }
    article_id
}

/* ============================================================
TASK-116 单元测试：失败标记落库 / 成功 prune 归零 / stats 口径
（cargo test 由 CI 执行，DEC-local-cargo-gate-20261005）
============================================================ */
#[cfg(test)]
mod t116_tests {
    use super::*;

    fn conn() -> Connection {
        let mut c = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut c).unwrap();
        c
    }

    fn last_error_of(c: &Connection, id: i64) -> Option<String> {
        c.query_row(
            "SELECT last_error FROM sync_queue WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .unwrap()
    }

    /// (t116-r1) 失败标记：attempts +1 / last_error 摘要落库，只标记失败 id；
    /// 重复失败累加不覆盖计数；超长摘要按码点截断。判别力：attempts 恒等写死
    /// （不 +1）、标记全队列、或 last_error 不落库的实现必红。
    #[test]
    fn mark_push_failed_records_attempts_and_error() {
        let c = conn();
        let aid_a = seed_article(&c, true, true);
        let aid_b = seed_article(&c, true, true);
        enqueue_sync(&c, Some(aid_a), None, "read", None).unwrap();
        enqueue_sync(&c, Some(aid_b), None, "star", None).unwrap();
        let items = take_sync_queue(&c).unwrap();
        assert_eq!(items.len(), 2, "前置：两条队项就位");
        let (id_a, id_b) = (items[0].id, items[1].id);

        // 首次失败：只标记 id_a
        mark_push_failed(&c, &[(id_a, "状态推送失败: HTTP 500".into())]).unwrap();
        assert_eq!(attempts_of(&c, id_a), 1, "失败一次 attempts=1");
        assert_eq!(
            c.query_row("SELECT status FROM sync_queue WHERE id = ?1", [id_a], |r| r
                .get::<_, String>(0))
                .unwrap(),
            STATUS_RETRY_WAIT,
            "失败可重试项进入 retry_wait"
        );
        assert_eq!(
            last_error_of(&c, id_a).as_deref(),
            Some("状态推送失败: HTTP 500"),
            "错误摘要落库"
        );
        assert_eq!(attempts_of(&c, id_b), 0, "未失败的队项不得被标记");
        assert_eq!(last_error_of(&c, id_b), None);

        // 再次失败：累加（不重置），摘要更新为最新一次
        mark_push_failed(&c, &[(id_a, "状态推送失败: timeout".into())]).unwrap();
        assert_eq!(attempts_of(&c, id_a), 2, "重复失败 attempts 累加");
        assert_eq!(
            last_error_of(&c, id_a).as_deref(),
            Some("状态推送失败: timeout"),
            "摘要更新为最近一次"
        );

        // 超长摘要截断：200 字符 + 「…」，按码点截（多字节安全）
        let long = "错".repeat(300);
        mark_push_failed(&c, &[(id_b, long)]).unwrap();
        let stored = last_error_of(&c, id_b).unwrap();
        assert_eq!(
            stored.chars().count(),
            LAST_ERROR_MAX_CHARS + 1,
            "截断到 200 字符 + 「…」"
        );
        assert!(stored.ends_with('…'));
        assert!(stored.starts_with('错'));
        // 空列表 no-op（生产调用方以 is_empty 守卫，这里锁死该语义）
        mark_push_failed(&c, &[]).unwrap();
        assert_eq!(attempts_of(&c, id_a), 2);
    }

    /// (t116-r2) 成功路径：成功项 prune 出队后 stats 归零；同轮失败项不受影响。
    /// 判别力：prune 语义被改成「成功也留队」或「清空全表」时必红（远端确认 =
    /// 出队、失败保留 = 重试，两条口径同时锁死）。
    #[test]
    fn successful_prune_zeros_stats_failed_rows_survive() {
        let c = conn();
        let aid_a = seed_article(&c, true, true);
        let aid_b = seed_article(&c, true, true);
        enqueue_sync(&c, Some(aid_a), None, "read", None).unwrap();
        enqueue_sync(&c, Some(aid_b), None, "unread", None).unwrap();
        let items = take_sync_queue(&c).unwrap();
        let (id_a, id_b) = (items[0].id, items[1].id);

        // 混合轮：id_a 成功、id_b 失败
        mark_push_failed(&c, &[(id_b, "状态推送失败: HTTP 503".into())]).unwrap();
        prune_sync(&c, &[id_a]).unwrap();
        let s = sync_queue_stats(&c).unwrap();
        assert_eq!(s.waiting, 1, "成功项已出队，失败项保留");
        assert_eq!(
            (s.retry_wait, s.failed),
            (1, 0),
            "可重试失败进入 retry_wait，终态 failed 仍为 0"
        );
        assert_eq!(s.last_error.as_deref(), Some("状态推送失败: HTTP 503"));

        // 重试成功：失败项也出队 → stats 全归零（X3：无队列无失败不制造噪音）
        prune_sync(&c, &[id_b]).unwrap();
        let s = sync_queue_stats(&c).unwrap();
        assert_eq!(
            (s.waiting, s.failed, s.retry_wait, s.last_error.is_none()),
            (0, 0, 0, true)
        );
    }

    /// (t116-r3) stats 口径：waiting=可推送未确认项（bound/retry）、
    /// failed=终态失败、retry_wait 单独计数；last_error 取 updated_at 最新者
    /// （同秒再按 id 最大）；空队列零值。
    #[test]
    fn sync_queue_stats_counts_waiting_failed_and_latest_error() {
        let c = conn();
        let empty = sync_queue_stats(&c).unwrap();
        assert_eq!(
            (empty.waiting, empty.failed, empty.last_error.is_none()),
            (0, 0, true),
            "空队列 = 零值（新装用户无噪音）"
        );

        let aid_a = seed_article(&c, true, true);
        let aid_b = seed_article(&c, true, true);
        let aid_c = seed_article(&c, true, true);
        enqueue_sync(&c, Some(aid_a), None, "read", None).unwrap();
        enqueue_sync(&c, Some(aid_b), None, "star", None).unwrap();
        enqueue_sync(&c, Some(aid_c), None, "unstar", None).unwrap();
        let items = take_sync_queue(&c).unwrap();
        let (id_a, id_b, id_c) = (items[0].id, items[1].id, items[2].id);

        // 只标记最早一条：last_error = 它自己的摘要
        mark_push_failed(&c, &[(id_a, "第一条失败".into())]).unwrap();
        let s = sync_queue_stats(&c).unwrap();
        assert_eq!((s.waiting, s.retry_wait, s.failed), (3, 1, 0));
        assert_eq!(s.last_error.as_deref(), Some("第一条失败"));

        // 再标记最新一条：last_error 切到 updated_at 最新者（同秒按 id 最大）
        mark_push_failed(&c, &[(id_c, "第三条失败".into())]).unwrap();
        let s = sync_queue_stats(&c).unwrap();
        assert_eq!(
            (s.waiting, s.retry_wait, s.failed),
            (3, 2, 0),
            "waiting 含可重试项；failed 只数终态失败"
        );
        assert_eq!(
            s.last_error.as_deref(),
            Some("第三条失败"),
            "last_error 取最近更新（updated_at，再按 id）的失败行"
        );
        assert_eq!(attempts_of(&c, id_b), 0, "中间行未受影响");
    }

    /// 较早入队的行后失败时，last_error 必须按 updated_at 取真正最近的一次；
    /// 仍按 id 最大会让较小 id 的新错误被较大 id 的旧错误遮住。
    #[test]
    fn last_error_prefers_later_failure_over_larger_queue_id() {
        let c = conn();
        let aid_a = seed_article(&c, true, true);
        let aid_b = seed_article(&c, true, true);
        enqueue_sync(&c, Some(aid_a), None, "read", None).unwrap();
        enqueue_sync(&c, Some(aid_b), None, "read", None).unwrap();
        let items = take_sync_queue(&c).unwrap();
        let (id_a, id_b) = (items[0].id, items[1].id);

        mark_push_failed(&c, &[(id_b, "旧错误（较大 id）".into())]).unwrap();
        mark_push_failed(&c, &[(id_a, "新错误（较小 id）".into())]).unwrap();
        c.execute(
            "UPDATE sync_queue
                SET updated_at = CASE id
                                   WHEN ?1 THEN '2026-01-01 00:00:00'
                                   ELSE '2026-01-02 00:00:00'
                                 END",
            [id_b],
        )
        .unwrap();

        assert_eq!(
            sync_queue_stats(&c).unwrap().last_error.as_deref(),
            Some("新错误（较小 id）"),
            "真实最近失败时间优先于队列 id 大小"
        );
    }
}

/* ============================================================
TASK-124 单元测试：事件 payload 正确性 / 推送阻塞标记
（cargo test 由 CI 执行，DEC-local-cargo-gate-20261005）
============================================================ */
#[cfg(test)]
mod t124_tests {
    use super::*;

    fn conn() -> Connection {
        let mut c = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut c).unwrap();
        c
    }

    /// (t124-r1) 事件 payload 正确性（探针 P8 链路的 Rust 半边）：队列变更后
    /// stats 查询即得最新分类计数——入队 → waiting=1 且 failed=0（无 last_error）；
    /// 失败标记 → retry_wait=1 + 摘要；成功出队 → 全归零。
    /// 未配置（configured=false）→ None（静默不发事件）。
    /// 判别力：payload 字段名/口径漂移（如 failed 误用全队行数、last_error 丢失）
    /// 或未配置误发事件的实现必红。
    #[test]
    fn queue_changed_payload_reflects_stats_after_mutation() {
        let c = conn();

        // 未配置静默：无论队列什么状态，configured=false 一律 None
        let s = sync_queue_stats(&c).unwrap();
        assert!(queue_changed_payload(&s, false).is_none(), "未配置不发事件");

        // 空队列（已配置）：零值 payload（新装用户无噪音）
        let p = queue_changed_payload(&sync_queue_stats(&c).unwrap(), true).unwrap();
        assert_eq!(p["waiting"].as_u64(), Some(0));
        assert_eq!(p["failed"].as_u64(), Some(0));
        assert!(p["last_error"].is_null());

        // 探针 P8 本体（Rust 半边）：本地标读入队 → waiting=1（「等待同步 1 条」的 N）
        let aid = seed_article(&c, true, true);
        enqueue_sync(&c, Some(aid), None, "read", None).unwrap();
        let p = queue_changed_payload(&sync_queue_stats(&c).unwrap(), true).unwrap();
        assert_eq!(p["waiting"].as_u64(), Some(1));
        assert_eq!(p["failed"].as_u64(), Some(0));
        assert!(p["last_error"].is_null());

        // 推送失败标记 → retry_wait=1 + last_error 摘要（「· 部分失败」的依据）
        let id = take_sync_queue(&c).unwrap().remove(0).id;
        mark_push_failed(&c, &[(id, "状态推送失败: HTTP 503".into())]).unwrap();
        let p = queue_changed_payload(&sync_queue_stats(&c).unwrap(), true).unwrap();
        assert_eq!(p["waiting"].as_u64(), Some(1));
        assert_eq!(p["retry_wait"].as_u64(), Some(1));
        assert_eq!(p["failed"].as_u64(), Some(0));
        assert_eq!(p["last_error"], "状态推送失败: HTTP 503");

        // 推送成功出队 → 恢复「后端已同步」（全归零、last_error 清失）
        prune_sync(&c, &[id]).unwrap();
        let p = queue_changed_payload(&sync_queue_stats(&c).unwrap(), true).unwrap();
        assert_eq!(p["waiting"].as_u64(), Some(0));
        assert_eq!(p["failed"].as_u64(), Some(0));
        assert!(p["last_error"].is_null());
    }

    /// (t124-r3) 推送阻塞标记（审计 P2-6②）：客户端构建失败时全部可推送队项
    /// attempts+1、转 retry_wait 并记录 last_error；local_only/expired 不触碰。
    /// 空队列为 no-op；重复阻塞累加（与
    /// exec_push 失败的 mark_push_failed 同口径——每次被阻塞的推送都是一次失败尝试）。
    #[test]
    fn mark_push_blocked_records_all_rows_and_accumulates() {
        let c = conn();

        // 空队列 no-op：返回 0（调用方据此不发事件）
        assert_eq!(mark_push_blocked(&c, "认证失败：x").unwrap(), 0);
        assert_eq!(sync_queue_stats(&c).unwrap().waiting, 0);

        let aid_a = seed_article(&c, true, true);
        let aid_b = seed_article(&c, true, true);
        enqueue_sync(&c, Some(aid_a), None, "read", None).unwrap();
        enqueue_sync(&c, Some(aid_b), None, "star", None).unwrap();
        let items = take_sync_queue(&c).unwrap();
        let (id_a, id_b) = (items[0].id, items[1].id);

        // 首次阻塞：全部可推送队项标记（绑定失败时它们确实推不出去）
        assert_eq!(
            mark_push_blocked(&c, "认证失败：ClientLogin → 401").unwrap(),
            2
        );
        let s = sync_queue_stats(&c).unwrap();
        assert_eq!(
            (s.waiting, s.retry_wait, s.failed),
            (2, 2, 0),
            "认证失败后两项进入 retry_wait，仍计入 waiting"
        );
        assert_eq!(
            s.last_error.as_deref(),
            Some("认证失败：ClientLogin → 401"),
            "阻塞摘要落库（认证失败文案）"
        );
        assert_eq!(attempts_of(&c, id_a), 1);
        assert_eq!(attempts_of(&c, id_b), 1);

        // 重复阻塞：attempts 累加（不重置），摘要更新为最新一次
        mark_push_blocked(&c, "认证失败：密码已变更").unwrap();
        assert_eq!(attempts_of(&c, id_a), 2, "重复阻塞 attempts 累加");
        assert_eq!(
            sync_queue_stats(&c).unwrap().last_error.as_deref(),
            Some("认证失败：密码已变更")
        );

        // 新入队行是未失败的新行：retry_wait 是失败可见计数，不继承旧行标记
        let aid_c = seed_article(&c, true, true);
        enqueue_sync(&c, Some(aid_c), None, "read", None).unwrap();
        let s = sync_queue_stats(&c).unwrap();
        assert_eq!(s.waiting, 3, "新文章独立入队：追加一行");
        assert_eq!(s.retry_wait, 2, "新行未失败，retry_wait 保持原两行");
        assert_eq!(s.failed, 0);
    }
}

/* ============================================================
同步队列生命周期：不静默删除、分类可见、绑定后再驱动
============================================================ */
#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    fn conn() -> Connection {
        let mut c = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut c).unwrap();
        c
    }

    fn status_of(c: &Connection, id: i64) -> String {
        c.query_row("SELECT status FROM sync_queue WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap()
    }

    #[test]
    fn expired_recovery_uses_current_targets_new_ids_and_identity_classes() {
        let mut connection = conn();
        let bound = seed_article(&connection, true, false);
        let bindable = seed_article(&connection, true, false);
        let local = seed_article(&connection, false, false);
        for article_id in [bound, bindable, local] {
            enqueue_sync(&connection, Some(article_id), None, "read", None).unwrap();
        }
        enqueue_sync(&connection, Some(bound), None, "star", None).unwrap();
        let old_ids: Vec<i64> = take_sync_queue(&connection)
            .unwrap()
            .iter()
            .map(|item| item.id)
            .collect();
        connection
            .execute("UPDATE sync_queue SET status = 'expired_unconfirmed'", [])
            .unwrap();
        crate::db::set_article_remote_id(&connection, bound, 88_001).unwrap();
        crate::db::set_read(&connection, bound, false).unwrap();
        crate::db::set_starred(&connection, bound, false).unwrap();
        let versions = crate::db::article_field_versions(&connection, bound).unwrap();
        let result = retry_expired_intents(&mut connection, 0).unwrap();
        assert_eq!(
            (
                result.requeued,
                result.bound_pending,
                result.bindable_retry,
                result.local_only
            ),
            (4, 2, 1, 1)
        );
        assert_eq!(result.remaining_expired, 0);
        let items = take_sync_queue(&connection).unwrap();
        assert!(items.iter().all(|item| !old_ids.contains(&item.id)));
        assert!(items
            .iter()
            .any(|item| item.article_id == Some(bound) && item.action == "unread"));
        assert!(items
            .iter()
            .any(|item| item.article_id == Some(bound) && item.action == "unstar"));
        assert_eq!(
            crate::db::article_field_versions(&connection, bound).unwrap(),
            versions
        );
        prune_sync(&connection, &old_ids).unwrap();
        assert_eq!(
            take_sync_queue(&connection).unwrap().len(),
            4,
            "旧确认不能删除新操作"
        );
        assert_eq!(
            retry_expired_intents(&mut connection, 0).unwrap().requeued,
            0,
            "重复操作幂等"
        );
        assert_eq!(sync_queue_stats(&connection).unwrap().account_generation, 0);
    }

    #[test]
    fn expired_recovery_stale_generation_and_missing_article_keep_intents() {
        let mut connection = conn();
        let article_id = seed_article(&connection, false, false);
        enqueue_sync(&connection, Some(article_id), None, "read", None).unwrap();
        connection
            .execute("UPDATE sync_queue SET status = 'expired_unconfirmed'", [])
            .unwrap();
        connection.execute("INSERT INTO sync_queue (article_id, action, kind, field, desired_state, status) VALUES (NULL, 'star', 'article_state', 'star', 1, 'expired_unconfirmed')", []).unwrap();
        let old_ids: Vec<i64> = take_sync_queue(&connection)
            .unwrap()
            .iter()
            .map(|item| item.id)
            .collect();
        crate::db::bump_sync_generation(&connection).unwrap();
        let error = retry_expired_intents(&mut connection, 0).unwrap_err();
        assert_eq!(error.code, "staleAccount");
        assert_eq!(
            take_sync_queue(&connection)
                .unwrap()
                .iter()
                .map(|item| item.id)
                .collect::<Vec<_>>(),
            old_ids
        );
        let result = retry_expired_intents(&mut connection, 1).unwrap();
        assert_eq!(
            (result.requeued, result.local_only, result.remaining_expired),
            (1, 1, 1)
        );
        assert_eq!(
            retry_expired_intents(&mut connection, 1)
                .unwrap()
                .remaining_expired,
            1
        );
    }

    #[test]
    fn expired_recovery_insert_failure_rolls_back_and_bad_generation_is_visible() {
        let mut connection = conn();
        let article_id = seed_article(&connection, true, false);
        enqueue_sync(&connection, Some(article_id), None, "read", None).unwrap();
        connection
            .execute("UPDATE sync_queue SET status = 'expired_unconfirmed'", [])
            .unwrap();
        let before = take_sync_queue(&connection).unwrap()[0].id;
        connection.execute_batch("CREATE TRIGGER reject_retry BEFORE INSERT ON sync_queue BEGIN SELECT RAISE(ABORT, 'retry blocked'); END;").unwrap();
        assert!(retry_expired_intents(&mut connection, 0).is_err());
        assert_eq!(take_sync_queue(&connection).unwrap()[0].id, before);
        assert_eq!(status_of(&connection, before), STATUS_EXPIRED_UNCONFIRMED);
        crate::db::set_setting(&connection, "sync_generation", "broken").unwrap();
        assert!(sync_queue_stats(&connection).is_err());
        assert!(retry_expired_intents(&mut connection, 0).is_err());
    }

    /// 44 条「feed 已绑定、文章尚无 remote_id」的 read 意图：不删、不计 waiting；
    /// 31 天后转 expired_unconfirmed，队列行与文章 read/star 均保留。
    #[test]
    fn unbound_intents_survive_expiry_and_keep_article_state() {
        let c = conn();
        let mut ids = Vec::new();
        for _ in 0..44 {
            let aid = seed_article(&c, true, false);
            enqueue_sync(&c, Some(aid), None, "read", None).unwrap();
            ids.push(aid);
        }
        c.execute("UPDATE articles SET is_starred = 1", []).unwrap();

        let before = sync_queue_stats(&c).unwrap();
        assert_eq!(
            (
                before.waiting,
                before.bindable_retry,
                before.local_only,
                before.expired_unconfirmed
            ),
            (0, 44, 0, 0),
            "等待同源身份的项不进 waiting，但必须单独可见"
        );

        let old = (chrono::Utc::now() - chrono::Duration::days(31))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        c.execute("UPDATE sync_queue SET created_at = ?1", [old])
            .unwrap();
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(30))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        assert_eq!(
            prune_stale_unbound(&c, &cutoff).unwrap(),
            44,
            "到期只转态，不删除"
        );

        let rows: i64 = c
            .query_row("SELECT COUNT(*) FROM sync_queue", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 44, "所有用户意图都保留");
        let statuses: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM sync_queue WHERE status = 'expired_unconfirmed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(statuses, 44);
        let after = sync_queue_stats(&c).unwrap();
        assert_eq!(
            (
                after.waiting,
                after.bindable_retry,
                after.expired_unconfirmed
            ),
            (0, 0, 44),
            "过期未确认项仍可见，但不再冒充远端等待"
        );

        let state: (i64, i64, i64) = c
            .query_row(
                "SELECT COUNT(*), SUM(is_read), SUM(is_starred) FROM articles",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(state, (44, 44, 44), "老化不得回滚或删除本地 read/star");
        assert_eq!(ids.len(), 44);
    }

    /// 绑定到真实 remote_id 后由 articles UPDATE 触发器单点恢复 pending；
    /// 不依赖 URL/标题/时间，也不在绑定函数里再造第二套匹配规则。
    #[test]
    fn article_remote_binding_revives_only_its_own_queue_intent() {
        let c = conn();
        let aid = seed_article(&c, true, false);
        let other = seed_article(&c, true, false);
        enqueue_sync(&c, Some(aid), None, "read", None).unwrap();
        enqueue_sync(&c, Some(other), None, "read", None).unwrap();
        let qid: i64 = c
            .query_row(
                "SELECT id FROM sync_queue WHERE article_id = ?1",
                [aid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status_of(&c, qid), STATUS_BINDABLE_RETRY);

        assert!(crate::db::set_article_remote_id(&c, aid, 991_001).unwrap());
        assert_eq!(
            status_of(&c, qid),
            STATUS_BOUND_PENDING,
            "同源确认绑定后回到可推送状态"
        );
        let other_status: String = c
            .query_row(
                "SELECT status FROM sync_queue WHERE article_id = ?1",
                [other],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            other_status, STATUS_BINDABLE_RETRY,
            "绑定一篇不得猜测或连带另一篇"
        );
        let stats = sync_queue_stats(&c).unwrap();
        assert_eq!((stats.waiting, stats.bindable_retry), (1, 1));
    }

    #[test]
    fn local_only_is_visible_but_not_waiting_or_session_failed() {
        let c = conn();
        let aid = seed_article(&c, false, false);
        enqueue_sync(&c, Some(aid), None, "read", None).unwrap();
        let stats = sync_queue_stats(&c).unwrap();
        assert_eq!((stats.waiting, stats.local_only, stats.failed), (0, 1, 0));
        let payload = queue_changed_payload(&stats, true).unwrap();
        assert_eq!(payload["waiting"].as_u64(), Some(0));
        assert_eq!(payload["local_only"].as_u64(), Some(1));
        assert_eq!(payload["expired_unconfirmed"].as_u64(), Some(0));
        assert_eq!(
            mark_push_blocked(&c, "认证失败：不应标记 local-only").unwrap(),
            0,
            "认证失败只标记真正可推送项"
        );
        let attempts = attempts_of(
            &c,
            c.query_row("SELECT id FROM sync_queue", [], |r| r.get(0))
                .unwrap(),
        );
        assert_eq!(attempts, 0, "local-only 未发出网络请求，attempts 保持 0");
    }

    #[test]
    fn terminal_failed_is_visible_but_not_counted_as_waiting() {
        let c = conn();
        let aid = seed_article(&c, true, true);
        enqueue_sync(&c, Some(aid), None, "read", None).unwrap();
        c.execute(
            "UPDATE sync_queue
                SET status = 'failed', last_error = '远端目标不存在'
              WHERE article_id = ?1",
            [aid],
        )
        .unwrap();
        let stats = sync_queue_stats(&c).unwrap();
        assert_eq!(
            (stats.waiting, stats.failed, stats.retry_wait),
            (0, 1, 0),
            "终态 failed 必须可见，但不再冒充可推送等待"
        );
        assert_eq!(stats.last_error.as_deref(), Some("远端目标不存在"));
    }
}

/* ============================================================
OPT-001（F01）单元测试：操作身份不复用（v17 复现锚 + v18 AUTOINCREMENT）
（cargo test 由 CI 执行，DEC-local-cargo-gate-20261005）
============================================================ */
#[cfg(test)]
mod opt001_tests {
    use super::*;

    /// (opt001-u1) 替换入队不复用操作 id（验收伪码的单元版）：
    /// ① v17 修前锚——替换入队复用被删行的最大 ROWID，旧计划按 id 确认会
    ///    误删新的用户意图；② v18（AUTOINCREMENT）同一序列——新 id 必不同、
    ///    旧 id 的确认不伤新项，清空后序列继续走高不回落。断言只验证
    ///    「不复用」的实际行为，不绑定 sqlite_sequence 表的实现细节。
    /// 判别力：v18 退回普通 INTEGER PRIMARY KEY 时，star/unstar 段必红
    /// （复用使 assert_ne 失败、prune 旧 id 直接删掉新行）。
    #[test]
    fn queue_identity_not_reused_across_replacement() {
        let mut c = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut c).unwrap();
        c.execute_batch("INSERT INTO feeds (feed_url,title) VALUES('https://f.example/rss','F'); INSERT INTO articles(feed_id,guid,title) VALUES(1,'g1','t');").unwrap();
        enqueue_sync(&c, Some(1), None, "read", None).unwrap();
        let basis = take_sync_queue(&c).unwrap().remove(0).id;
        assert_eq!(basis, 1, "前置：升级时现存最大 id = 1（v17 复用结果）");
        prune_sync(&c, &[basis]).unwrap();

        // ---- ② v18：同一序列不得复用；空队列后序列继续走高 ----
        // （sequence 由 SQLite 随拷贝/RENAME 自动维护，不手工重置）
        enqueue_sync(&c, Some(1), None, "star", None).unwrap();
        let star = take_sync_queue(&c).unwrap().remove(0).id;
        assert!(
            star > basis,
            "升级后新项 id 必须高于已提交最大 id（不复用被删行）"
        );

        enqueue_sync(&c, Some(1), None, "unstar", None).unwrap();
        let unstar = take_sync_queue(&c).unwrap().remove(0).id;
        assert_ne!(unstar, star, "v18：替换入队必须换新 id（不复用被删行）");
        prune_sync(&c, &[star]).unwrap();
        let q = take_sync_queue(&c).unwrap();
        assert_eq!(q.len(), 1, "旧 id 的确认不得删掉新用户意图");
        assert_eq!(q[0].id, unstar);
        assert_eq!(q[0].action, "unstar");

        // unstar 残留先清场：read/unread 入队不会清除 unstar，保留会让下一段
        // remove(0) 拿到旧行（清场后重新断言队列空，再进入 read 场景）
        prune_sync(&c, &[unstar]).unwrap();
        assert!(take_sync_queue(&c).unwrap().is_empty(), "unstar 场景清场");

        // read→unread 同断言（队列已空也不回落复用）
        enqueue_sync(&c, Some(1), None, "read", None).unwrap();
        let read = take_sync_queue(&c).unwrap().remove(0).id;
        assert!(read > unstar, "清空后新项 id 继续走高，不回落复用");
        enqueue_sync(&c, Some(1), None, "unread", None).unwrap();
        let unread = take_sync_queue(&c).unwrap().remove(0).id;
        assert!(unread > read, "替换入队 id 严格递增");
        prune_sync(&c, &[read]).unwrap();
        let q = take_sync_queue(&c).unwrap();
        assert_eq!(
            (q.len(), q[0].id, q[0].action.as_str()),
            (1, unread, "unread"),
            "旧 read 计划的确认不得伤及新的 unread 意图"
        );
    }
}
