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
