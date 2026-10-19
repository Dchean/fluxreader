//! OPT-013 B2：AI 分段产物的持久化（长文翻译的恢复依据）。
//!
//! 契约（定版 §3 / Note「OPT-013 B2 追加」）：
//! - 每个**完成**片段独立落库并绑定同一缓存签名；签名 = B1 的
//!   `ai::ai_cache_signature`（含 `SEGMENT_POLICY_VERSION` 与整篇正文哈希），
//!   同一签名下的片段序列由输入确定性决定，因此只需 (签名, 序号) 定位；
//! - 同段重试 = UPSERT 覆盖（主键去重）——失败片段重试不追加重复文本；
//! - 只有全部片段完成才由命令层写整篇产物；本模块只负责片段本身；
//! - 清除（cache_cleanup scope='ai'）与条目删除都必须带走片段（见
//!   articles::cleanup_cache 与 v28 的 FK 级联）。

use super::*;
use rusqlite::params;

/// 片段产物类型标识（与 commands 层 `AiKind::id()` 的 translation 逐字一致）。
pub const AI_PRODUCT_TRANSLATION: &str = "translation";

/// 写入/覆盖一个完成片段（同签名同序号幂等——重试覆盖而非追加）。
///
/// OPT-013 B3：带**清除代次前提**——`expected_epoch` 与条目当前 `ai_epoch`
/// 不符（生成期间用户执行了清除）时拒绝写入（返回 `false`，0 行变更）。否则
/// 「清除在生成在途时发生」会被迟到请求在清除**之后**重新写入同签名片段，
/// 使「清除后零恢复」被破坏（读段只按签名，清除不改变签名）。
pub fn upsert_ai_segment(
    conn: &Connection,
    article_id: i64,
    product: &str,
    signature: &str,
    segment_index: i64,
    content: &str,
    expected_epoch: i64,
) -> AppResult<bool> {
    let n = conn.execute(
        "INSERT INTO ai_segments (article_id, product, signature, segment_index, content)
         SELECT ?1, ?2, ?3, ?4, ?5
         WHERE EXISTS (SELECT 1 FROM articles WHERE id = ?1 AND ai_epoch = ?6)
         ON CONFLICT(article_id, product, signature, segment_index)
         DO UPDATE SET content = excluded.content, created_at = datetime('now')",
        params![
            article_id,
            product,
            signature,
            segment_index,
            content,
            expected_epoch
        ],
    )?;
    Ok(n > 0)
}

/// 读某签名下已完成的片段（按序号升序）。
pub fn list_ai_segments(
    conn: &Connection,
    article_id: i64,
    product: &str,
    signature: &str,
) -> AppResult<Vec<(i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT segment_index, content FROM ai_segments
         WHERE article_id = ?1 AND product = ?2 AND signature = ?3
         ORDER BY segment_index ASC",
    )?;
    let rows = stmt.query_map(params![article_id, product, signature], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 片段计划统计：`(当前签名已完成数, 其他签名的旧片段数)`。
///
/// 「其他签名的旧片段数」> 0 表示该条目存在**因输入变化而作废**的进度——
/// 命令层据此在重生成时如实告知（前端最小提示），也用于成功后清理。
pub fn ai_segment_stats(
    conn: &Connection,
    article_id: i64,
    product: &str,
    signature: &str,
) -> AppResult<(usize, usize)> {
    let mut stmt = conn.prepare(
        "SELECT signature, COUNT(*) FROM ai_segments
         WHERE article_id = ?1 AND product = ?2
         GROUP BY signature",
    )?;
    let rows = stmt.query_map(params![article_id, product], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    })?;
    let mut completed = 0usize;
    let mut discarded = 0usize;
    for row in rows {
        let (sig, count) = row?;
        let count = count.max(0) as usize;
        if sig == signature {
            completed += count;
        } else {
            discarded += count;
        }
    }
    Ok((completed, discarded))
}

/// 清掉其他签名的旧片段（整篇产物成功提交后调用）：当前签名是唯一可恢复的
/// 进度，其余签名的片段因输入变化已无复用价值。返回删除行数。
pub fn delete_stale_ai_segments(
    conn: &Connection,
    article_id: i64,
    product: &str,
    signature: &str,
) -> AppResult<usize> {
    let n = conn.execute(
        "DELETE FROM ai_segments
         WHERE article_id = ?1 AND product = ?2 AND signature != ?3",
        params![article_id, product, signature],
    )?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrations::MIGRATIONS;

    fn mem_db() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO folders (id, name, layout) VALUES (1, 'F', 'article')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO feeds (id, folder_id, feed_url, title) VALUES (1, 1, 'https://a.example/rss', 'A')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO articles (id, feed_id, guid, title) VALUES (1, 1, 'g1', '标题')",
            [],
        )
        .unwrap();
        conn
    }

    #[test]
    fn upsert_is_idempotent_and_replace_not_append() {
        let conn = mem_db();
        upsert_ai_segment(&conn, 1, AI_PRODUCT_TRANSLATION, "sig-a", 0, "第一段", 0).unwrap();
        upsert_ai_segment(
            &conn,
            1,
            AI_PRODUCT_TRANSLATION,
            "sig-a",
            0,
            "第一段（重试覆盖）",
            0,
        )
        .unwrap();
        upsert_ai_segment(&conn, 1, AI_PRODUCT_TRANSLATION, "sig-a", 1, "第二段", 0).unwrap();
        let rows = list_ai_segments(&conn, 1, AI_PRODUCT_TRANSLATION, "sig-a").unwrap();
        assert_eq!(
            rows,
            vec![
                (0, "第一段（重试覆盖）".to_string()),
                (1, "第二段".to_string())
            ],
            "同段重试必须覆盖而非追加"
        );
    }

    /// OPT-013 B3：清除代次前提——epoch 不符（清除在生成在途时发生）必须拒绝
    /// 写入（返回 false、0 行），否则迟到请求会在清除之后复活同签名片段。
    #[test]
    fn upsert_rejected_when_epoch_advanced() {
        let conn = mem_db();
        assert!(
            !upsert_ai_segment(&conn, 1, AI_PRODUCT_TRANSLATION, "sig-a", 0, "迟到段", 1).unwrap(),
            "代次不符必须拒绝写入"
        );
        assert!(
            list_ai_segments(&conn, 1, AI_PRODUCT_TRANSLATION, "sig-a")
                .unwrap()
                .is_empty(),
            "拒绝写入不得留下片段"
        );
        assert!(
            upsert_ai_segment(&conn, 1, AI_PRODUCT_TRANSLATION, "sig-a", 0, "正常段", 0).unwrap(),
            "代次相符照常写入"
        );
    }

    #[test]
    fn stats_split_current_and_discarded_signatures() {
        let conn = mem_db();
        upsert_ai_segment(&conn, 1, AI_PRODUCT_TRANSLATION, "sig-a", 0, "旧一", 0).unwrap();
        upsert_ai_segment(&conn, 1, AI_PRODUCT_TRANSLATION, "sig-a", 1, "旧二", 0).unwrap();
        upsert_ai_segment(&conn, 1, AI_PRODUCT_TRANSLATION, "sig-b", 0, "新一", 0).unwrap();
        assert_eq!(
            ai_segment_stats(&conn, 1, AI_PRODUCT_TRANSLATION, "sig-b").unwrap(),
            (1, 2),
            "当前签名完成数与其他签名作废数分别统计"
        );
        assert_eq!(
            ai_segment_stats(&conn, 1, AI_PRODUCT_TRANSLATION, "sig-c").unwrap(),
            (0, 3)
        );
        let removed = delete_stale_ai_segments(&conn, 1, AI_PRODUCT_TRANSLATION, "sig-b").unwrap();
        assert_eq!(removed, 2);
        assert_eq!(
            list_ai_segments(&conn, 1, AI_PRODUCT_TRANSLATION, "sig-b")
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn deleting_article_cascades_segments() {
        let conn = mem_db();
        upsert_ai_segment(&conn, 1, AI_PRODUCT_TRANSLATION, "sig-a", 0, "段", 0).unwrap();
        conn.execute("DELETE FROM articles WHERE id = 1", [])
            .unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM ai_segments", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "条目删除必须带走片段（FK 级联）");
    }
}
