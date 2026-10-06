use super::*;
use rusqlite::params;
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
    /// A-8：无远端绑定项的老化清理依据。
    pub created_at: String,
}

/// 本地变更入队（已读/收藏等）。同一条目同向的旧记录先删，避免重复推送。
pub fn enqueue_sync(
    conn: &Connection,
    article_id: Option<i64>,
    feed_url: Option<&str>,
    action: &str,
    payload: Option<&str>,
) -> AppResult<()> {
    // 同 article+action 只保留最新一条（read/unread 视为同向互斥，直接覆盖）
    if let Some(aid) = article_id {
        conn.execute(
            "DELETE FROM sync_queue WHERE article_id = ?1 AND action IN (?2, ?3)",
            params![aid, opposite_action(action), action],
        )?;
    }
    // created_at 用 SQLite datetime('now')（UTC，'YYYY-MM-DD HH:MM:SS'）——与建表
    // 默认值及历史数据同格式，老化比较才能按字符串正确排序
    conn.execute(
        "INSERT INTO sync_queue (article_id, feed_url, action, payload, created_at)
         VALUES (?1, ?2, ?3, ?4, datetime('now'))",
        params![article_id, feed_url, action, payload],
    )?;
    Ok(())
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
        "SELECT id, article_id, feed_url, action, payload, created_at FROM sync_queue ORDER BY id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(SyncQueueItem {
            id: r.get(0)?,
            article_id: r.get(1)?,
            feed_url: r.get(2)?,
            action: r.get(3)?,
            payload: r.get(4)?,
            created_at: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 清除已成功推送的队列条目
pub fn prune_sync(conn: &Connection, ids: &[i64]) -> AppResult<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    conn.execute(
        &format!("DELETE FROM sync_queue WHERE id IN ({placeholders})"),
        rusqlite::params_from_iter(ids.iter()),
    )?;
    Ok(())
}

/// 清掉历史版本残留的 remove_feed 僵尸队项（该动作从未被 sync 消费；现行
/// 删除语义为「本地删除不推远端」，见 delete_feed 命令）。返回清除条数。
pub fn purge_remove_feed_zombies(conn: &Connection) -> AppResult<usize> {
    let n = conn.execute("DELETE FROM sync_queue WHERE action = 'remove_feed'", [])?;
    Ok(n)
}

/// 老化清理（A-8）：删除超过保留期、且仍无远端绑定的状态队列项。
/// 只清理状态类动作（read/unread/star/unstar）——add_feed 由 feeds 阶段处理。
/// `cutoff` 与 created_at 同为 'YYYY-MM-DD HH:MM:SS'（UTC）。返回清理条数。
pub fn prune_stale_unbound(conn: &Connection, cutoff: &str) -> AppResult<usize> {
    let n = conn.execute(
        "DELETE FROM sync_queue
          WHERE created_at < ?1
            AND action IN ('read', 'unread', 'star', 'unstar')
            AND (article_id IS NULL
                 OR article_id IN (SELECT id FROM articles WHERE remote_id IS NULL))",
        params![cutoff],
    )?;
    Ok(n)
}

/* ============================================================
TASK-116（同步四态展示）：失败标记 + 队列统计
============================================================ */

/// last_error 摘要的最大长度（字符数）。错误摘要进 UI「部分失败（最新错误）」
/// 单行展示，超长只留开头——错误尾巴通常是冗余的堆栈/URL 细节。
const LAST_ERROR_MAX_CHARS: usize = 200;

/// 错误摘要截断：按字符（码点）截断保证多字节安全，截断后以「…」示意。
fn truncate_last_error(err: &str) -> String {
    if err.chars().count() <= LAST_ERROR_MAX_CHARS {
        return err.to_string();
    }
    let head: String = err.chars().take(LAST_ERROR_MAX_CHARS).collect();
    format!("{head}…")
}

/// 推送失败项落库标记（TASK-116）：attempts +1 并记录错误摘要。
/// 队列行保留（失败 = 天然重试语义，见 sync/push.rs）；成功路径不走这里——
/// 成功项由 prune_sync 整行删除，attempts/last_error 随之消失（stats 归零）。
/// 状态变更重新入队（enqueue_sync 删旧插新）也会重置为未失败。
pub fn mark_push_failed(conn: &Connection, failed: &[(i64, String)]) -> AppResult<()> {
    if failed.is_empty() {
        return Ok(());
    }
    let mut stmt = conn
        .prepare("UPDATE sync_queue SET attempts = attempts + 1, last_error = ?1 WHERE id = ?2")?;
    for (id, summary) in failed {
        stmt.execute(params![truncate_last_error(summary), id])?;
    }
    Ok(())
}

/// 队列统计（四态展示口径，TASK-116）：
/// - waiting = 队列现存行数（含 add_feed；「等待同步 N 条」的 N）；
/// - failed  = attempts > 0 的行数（「部分失败」的 N；是 waiting 的子集）；
/// - last_error = 最近一次失败摘要（按队列 id 最大者——入队互斥合并会删旧插
///   新，id 单调递增；失败标记不删除行，故 id 最大 ≡ 最晚入队，无独立失败
///   时间戳列也够用）。
#[derive(Debug, Serialize)]
pub struct SyncQueueStats {
    pub waiting: usize,
    pub failed: usize,
    pub last_error: Option<String>,
}

pub fn sync_queue_stats(conn: &Connection) -> AppResult<SyncQueueStats> {
    let (waiting, failed, last_error): (i64, i64, Option<String>) = conn.query_row(
        "SELECT (SELECT COUNT(*) FROM sync_queue),
                (SELECT COUNT(*) FROM sync_queue WHERE attempts > 0),
                (SELECT last_error FROM sync_queue
                  WHERE attempts > 0 AND last_error IS NOT NULL
                  ORDER BY id DESC LIMIT 1)",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    Ok(SyncQueueStats {
        waiting: waiting as usize,
        failed: failed as usize,
        last_error,
    })
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

    fn attempts_of(c: &Connection, id: i64) -> i64 {
        c.query_row(
            "SELECT attempts FROM sync_queue WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .unwrap()
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
        enqueue_sync(&c, None, None, "read", None).unwrap();
        enqueue_sync(&c, None, None, "star", None).unwrap();
        let items = take_sync_queue(&c).unwrap();
        assert_eq!(items.len(), 2, "前置：两条队项就位");
        let (id_a, id_b) = (items[0].id, items[1].id);

        // 首次失败：只标记 id_a
        mark_push_failed(&c, &[(id_a, "状态推送失败: HTTP 500".into())]).unwrap();
        assert_eq!(attempts_of(&c, id_a), 1, "失败一次 attempts=1");
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
        enqueue_sync(&c, None, None, "read", None).unwrap();
        enqueue_sync(&c, None, None, "unread", None).unwrap();
        let items = take_sync_queue(&c).unwrap();
        let (id_a, id_b) = (items[0].id, items[1].id);

        // 混合轮：id_a 成功、id_b 失败
        mark_push_failed(&c, &[(id_b, "状态推送失败: HTTP 503".into())]).unwrap();
        prune_sync(&c, &[id_a]).unwrap();
        let s = sync_queue_stats(&c).unwrap();
        assert_eq!(s.waiting, 1, "成功项已出队，失败项保留");
        assert_eq!(s.failed, 1, "失败标记随保留行留存");
        assert_eq!(s.last_error.as_deref(), Some("状态推送失败: HTTP 503"));

        // 重试成功：失败项也出队 → stats 全归零（X3：无队列无失败不制造噪音）
        prune_sync(&c, &[id_b]).unwrap();
        let s = sync_queue_stats(&c).unwrap();
        assert_eq!((s.waiting, s.failed, s.last_error.is_none()), (0, 0, true));
    }

    /// (t116-r3) stats 口径：waiting=全部行、failed=attempts>0 子集、
    /// last_error 取 id 最大（最晚入队）者；空队列为零值。
    #[test]
    fn sync_queue_stats_counts_waiting_failed_and_latest_error() {
        let c = conn();
        let empty = sync_queue_stats(&c).unwrap();
        assert_eq!(
            (empty.waiting, empty.failed, empty.last_error.is_none()),
            (0, 0, true),
            "空队列 = 零值（新装用户无噪音）"
        );

        enqueue_sync(&c, None, None, "read", None).unwrap();
        enqueue_sync(&c, None, None, "star", None).unwrap();
        enqueue_sync(&c, None, None, "unstar", None).unwrap();
        let items = take_sync_queue(&c).unwrap();
        let (id_a, id_b, id_c) = (items[0].id, items[1].id, items[2].id);

        // 只标记最早一条：last_error = 它自己的摘要
        mark_push_failed(&c, &[(id_a, "第一条失败".into())]).unwrap();
        let s = sync_queue_stats(&c).unwrap();
        assert_eq!(s.waiting, 3);
        assert_eq!(s.failed, 1);
        assert_eq!(s.last_error.as_deref(), Some("第一条失败"));

        // 再标记最晚一条：last_error 切到 id 最大者（最近口径）
        mark_push_failed(&c, &[(id_c, "第三条失败".into())]).unwrap();
        let s = sync_queue_stats(&c).unwrap();
        assert_eq!(s.waiting, 3, "waiting=队列全部行（含未失败）");
        assert_eq!(s.failed, 2, "failed=attempts>0 的子集");
        assert_eq!(
            s.last_error.as_deref(),
            Some("第三条失败"),
            "last_error 取 id 最大（最晚入队）的失败行"
        );
        assert_eq!(attempts_of(&c, id_b), 0, "中间行未受影响");
    }
}
