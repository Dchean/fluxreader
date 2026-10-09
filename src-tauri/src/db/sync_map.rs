use super::*;
use rusqlite::params;

/// 按 Miniflux entry id 找本地条目（Pull 状态合并的匹配键之一）
pub fn article_by_remote_id(conn: &Connection, remote_id: i64) -> AppResult<Option<i64>> {
    let id = conn
        .query_row(
            "SELECT id FROM articles WHERE remote_id = ?1",
            params![remote_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(id)
}

/* ============================================================
Pull 合并的批量预取映射 —— 消除 N+1

同步对账（pull_entries）里，原先对每个远端 entry 逐条查询
（url_norm 匹配 / remote_id 匹配 / 同源判定 / pending 集合 / feed 绑定），
首次同步上千条 = 数千次 SQLite 查询。这里一次性把全部映射查进内存，
循环内改为 HashMap/HashSet 查找（O(1)），把「数千次查询」压成「5 次批量查询」。

TASK-070（REQ-104）：被本映射取代的那批逐条查询函数已删除
（article_id_by_url / article_matches_remote_feed / article_has_pending_sync /
feed_by_remote_id / set_folder_remote_id），生产零调用。
============================================================ */

/// Pull 合并所需的全部匹配映射（一次批量预取）。
pub struct SyncMatchMaps {
    /// 规范化 URL（url_norm）→ article id
    pub url_to_id: HashMap<String, i64>,
    /// article id → remote_id（None 表示未绑定）
    pub id_to_mf_id: HashMap<i64, Option<i64>>,
    /// article id → (文章 remote_id, 所属 feed 的 remote_id)
    /// （同源判定的输入：生产据此推导 same_feed_trusted）
    pub id_to_mf_pair: HashMap<i64, (Option<i64>, Option<i64>)>,
    /// 有「已入队未推送」读/收藏变更的 article id 集合
    pub pending_ids: HashSet<i64>,
    /// feed remote_id → feed id
    pub feed_mf_to_id: HashMap<i64, i64>,
    /// article remote_id → article id
    pub mf_id_to_article: HashMap<i64, i64>,
}

/// 一次批量查询构建 Pull 合并所需的全部匹配映射。
pub fn sync_match_maps(conn: &Connection) -> AppResult<SyncMatchMaps> {
    // 1. url_norm → id
    let mut url_to_id = HashMap::new();
    {
        // ORDER BY id：与规范化 URL 匹配的既有口径一致——
        // 同 URL 多篇时保留 id 最小者（or_insert 保留首见，首见即最小 id）
        let mut stmt = conn
            .prepare("SELECT url_norm, id FROM articles WHERE url_norm IS NOT NULL ORDER BY id")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        for row in rows {
            let (url_norm, id) = row?;
            url_to_id.entry(url_norm).or_insert(id);
        }
    }

    // 2. id → remote_id（含 None 的也要，用 Option 区分「未绑定」与「不存在」）
    let mut id_to_mf_id: HashMap<i64, Option<i64>> = HashMap::new();
    // 3. id → (文章 mf_id, feed mf_id)
    let mut id_to_mf_pair: HashMap<i64, (Option<i64>, Option<i64>)> = HashMap::new();
    // 6. mf_id → article id（仅已绑定的）
    let mut mf_id_to_article: HashMap<i64, i64> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT a.id, a.remote_id, f.remote_id FROM articles a
             JOIN feeds f ON f.id = a.feed_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<i64>>(1)?,
                r.get::<_, Option<i64>>(2)?,
            ))
        })?;
        for row in rows {
            let (id, mf_id, feed_mf_id) = row?;
            id_to_mf_id.insert(id, mf_id);
            id_to_mf_pair.insert(id, (mf_id, feed_mf_id));
            if let Some(mf) = mf_id {
                mf_id_to_article.insert(mf, id);
            }
        }
    }

    // 4. pending sync 的 article id 集合
    let mut pending_ids = HashSet::new();
    {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT article_id FROM sync_queue WHERE action IN ('read','unread','star','unstar')",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
        for row in rows {
            pending_ids.insert(row?);
        }
    }

    // 5. feed mf_id → feed id
    let mut feed_mf_to_id = HashMap::new();
    {
        let mut stmt =
            conn.prepare("SELECT remote_id, id FROM feeds WHERE remote_id IS NOT NULL")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
        for row in rows {
            let (mf_id, id) = row?;
            feed_mf_to_id.insert(mf_id, id);
        }
    }

    Ok(SyncMatchMaps {
        url_to_id,
        id_to_mf_id,
        id_to_mf_pair,
        pending_ids,
        feed_mf_to_id,
        mf_id_to_article,
    })
}

/// 绑定 Miniflux entry id（Pull 时首次见到该条目）
pub fn set_article_remote_id(conn: &Connection, id: i64, remote_id: i64) -> AppResult<()> {
    conn.execute(
        "UPDATE articles SET remote_id = ?1 WHERE id = ?2",
        params![remote_id, id],
    )?;
    Ok(())
}

/// 记账服务端同文副本 entry（跨源同 URL 的另一条 entry）。
/// 幂等：已在列表中不重复；上限 16 个防脏数据撑爆字段。
pub fn add_article_dup_entry(conn: &Connection, id: i64, dup_entry_id: i64) -> AppResult<()> {
    if dup_entry_id <= 0 {
        return Ok(());
    }
    let cur: String = conn
        .query_row(
            "SELECT COALESCE(remote_dup_ids, '') FROM articles WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or_default();
    let ids: Vec<i64> = cur
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    if ids.contains(&dup_entry_id) || ids.len() >= 16 {
        return Ok(());
    }
    let mut next = ids;
    next.push(dup_entry_id);
    let joined = next
        .iter()
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(",");
    conn.execute(
        "UPDATE articles SET remote_dup_ids = ?1 WHERE id = ?2",
        params![joined, id],
    )?;
    Ok(())
}

/// 读某文章的副本 entry 列表（广播已读/收藏用）
pub fn article_dup_entries(conn: &Connection, id: i64) -> AppResult<Vec<i64>> {
    let cur: Option<String> = conn
        .query_row(
            "SELECT remote_dup_ids FROM articles WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(cur
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect())
}

/* ============================================================
同步代际（OPT-006 / F05、F06；内部 settings，无 schema 迁移）

每个数据库独立的 `sync_generation`：账号配置提交（sync_save）与断开
（sync_disconnect）在同一事务里 +1；同步会话在同一个短 DB 临界区里
捕获「凭据 + 代际」，此后每个 HTTP→DB 回写边界在**同一次持锁内**复核
代际——不等 = 账号已变更，旧响应一律不落库（不复活旧绑定/游标/队列）。
代际缺失/非法按 0 处理（旧库首次升级即 0，第一次账号提交推进到 1）。
============================================================ */

const SYNC_GENERATION_KEY: &str = "sync_generation";

/// 读当前同步代际（每库独立）。
///
/// R2 三态收紧：**只有「键缺失」= 0**（旧库未初始化，第一次账号提交推进到 1）；
/// 键存在但为空串/空白/非法/负数/溢出一律 `Err`——「有键但不可解析」是状态损坏，
/// 按 0 会与旧代际 0 的会话假匹配，也绝不能在 `bump` 里被顺手覆写掩盖。
pub fn sync_generation(conn: &Connection) -> AppResult<i64> {
    let Some(raw) = get_setting(conn, SYNC_GENERATION_KEY)? else {
        return Ok(0);
    };
    match raw.trim().parse::<i64>() {
        Ok(v) if v >= 0 => Ok(v),
        _ => Err(crate::error::AppError::new(
            "protocol",
            format!("同步代际损坏（{SYNC_GENERATION_KEY}={raw:?}）：拒绝继续，请排查/修复后重试"),
        )),
    }
}

/// 推进代际并返回新值（调用方须已处于账号提交事务内）。
/// R2：`checked_add`——到 i64::MAX 时 Err（不饱和复用同一代际），且不改写现场。
pub fn bump_sync_generation(conn: &Connection) -> AppResult<i64> {
    let next = sync_generation(conn)?.checked_add(1).ok_or_else(|| {
        crate::error::AppError::new(
            "protocol",
            format!("同步代际溢出（{SYNC_GENERATION_KEY} 已达 i64::MAX）：拒绝继续"),
        )
    })?;
    set_setting(conn, SYNC_GENERATION_KEY, &next.to_string())?;
    Ok(next)
}

/// 记录上次同步时间戳（Pull 增量游标，unix 秒）
pub fn last_sync_ts(conn: &Connection) -> AppResult<i64> {
    let v: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'sync_last_sync'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    Ok(v.and_then(|s| s.parse().ok()).unwrap_or(0))
}

pub fn set_last_sync_ts(conn: &Connection, ts: i64) -> AppResult<()> {
    set_setting(conn, "sync_last_sync", &ts.to_string())
}

/// Fever 协议增量游标：上次同步拉到的最大条目 id（`since_id` 分页用）。
pub fn last_sync_entry_id(conn: &Connection) -> AppResult<i64> {
    let v: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'sync_last_entry_id'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    Ok(v.and_then(|s| s.parse().ok()).unwrap_or(0))
}

pub fn set_last_sync_entry_id(conn: &Connection, id: i64) -> AppResult<()> {
    set_setting(conn, "sync_last_entry_id", &id.to_string())
}

/* ============================================================
Fever 历史状态（OPT-005 R2；内部 settings，无 schema 迁移）

显式三态（`fever_history_state` 键，单键原子承载状态+游标）：
- 缺失/空串                       = Unknown：**绝不当完成**（旧库只有 since、
  或从未初始化）；同步据此启动回溯（从顶部）。
- `pending:<max_id>`（1..=i64::MAX）= Pending：历史未完成，下一次 `items&max_id`
  从该游标继续；页事务把页数据与它同 commit。
- `complete`                       = Complete：历史已取尽。
其余取值 = **损坏 → Err**（显式报错，不擅自当完成、不覆盖现场）。

与 `sync_last_entry_id`（只由连续成功范围推进）互不越权：续取旧历史不推动
增量游标；初始化 Pending 成功先于任何更大的 since 落库。
============================================================ */

const FEVER_HISTORY_STATE_KEY: &str = "fever_history_state";
const FEVER_HISTORY_STATE_COMPLETE: &str = "complete";

/// Fever 历史状态（见上方三态语义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeverHistoryState {
    /// 键缺失/空：状态未知，**绝不当完成**。
    Unknown,
    /// 未完成：下一次 max_id 从该游标继续。
    Pending(i64),
    /// 已取尽。
    Complete,
}

/// 读 Fever 历史状态；损坏值返回显式错误（调用方不得继续、不得覆盖）。
// Note: Unknown/Pending/Complete 与 checkpoint 同 commit 的语义 — 见 .agents/notes/implemented/architecture/2026-10-08-Fever身份与历史回溯.md
pub fn fever_history_state(conn: &Connection) -> AppResult<FeverHistoryState> {
    let raw = get_setting(conn, FEVER_HISTORY_STATE_KEY)?;
    let Some(raw) = raw else {
        return Ok(FeverHistoryState::Unknown);
    };
    let t = raw.trim();
    if t.is_empty() {
        return Ok(FeverHistoryState::Unknown);
    }
    if t == FEVER_HISTORY_STATE_COMPLETE {
        return Ok(FeverHistoryState::Complete);
    }
    if let Some(rest) = t.strip_prefix("pending:") {
        if let Ok(v) = rest.trim().parse::<i64>() {
            if v >= 1 {
                return Ok(FeverHistoryState::Pending(v));
            }
        }
    }
    Err(crate::error::AppError::new(
        "protocol",
        format!(
            "Fever 历史状态损坏（{FEVER_HISTORY_STATE_KEY}={raw:?}）：不擅自当完成，请排查/修复"
        ),
    ))
}

/// 写入 Pending（未完成）状态与续取游标。
pub fn set_fever_history_pending(conn: &Connection, max_id: i64) -> AppResult<()> {
    set_setting(conn, FEVER_HISTORY_STATE_KEY, &format!("pending:{max_id}"))
}

/// 历史已取尽：写 Complete（只有在真正走完空页后允许）。
pub fn set_fever_history_complete(conn: &Connection) -> AppResult<()> {
    set_setting(conn, FEVER_HISTORY_STATE_KEY, FEVER_HISTORY_STATE_COMPLETE)
}

/// 重置历史状态为 Unknown（OPT-006：断开/换号同一事务内调用）。
/// 写空串而非删除行：`fever_history_state` 读侧把空串当 Unknown（见三态语义），
/// 显式写空让「刚被账号事件重置」与「从未初始化」在同步行为上一致（都启动回溯），
/// 同时保留一行可审计记录。
pub fn reset_fever_history_state(conn: &Connection) -> AppResult<()> {
    set_setting(conn, FEVER_HISTORY_STATE_KEY, "")
}

/// feeds 的 remote_id 绑定
pub fn set_feed_remote_id(conn: &Connection, feed_id: i64, remote_id: i64) -> AppResult<()> {
    conn.execute(
        "UPDATE feeds SET remote_id = ?1 WHERE id = ?2",
        params![remote_id, feed_id],
    )?;
    Ok(())
}

/// 按 URL 找本地 feed（首次同步的碰撞检测键）
pub fn feed_id_by_url(conn: &Connection, feed_url: &str) -> AppResult<Option<i64>> {
    let id = conn
        .query_row(
            "SELECT id FROM feeds WHERE feed_url = ?1",
            params![feed_url],
            |r| r.get(0),
        )
        .optional()?;
    Ok(id)
}

/// 按 URL 检查 feed 是否存在（OPML 导入去重用）
pub fn feed_exists_by_url(conn: &Connection, feed_url: &str) -> AppResult<bool> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM feeds WHERE feed_url = ?1)",
        params![feed_url],
        |r| r.get(0),
    )?;
    Ok(exists)
}

/// 按规范化 URL 找本地 feed（与条目侧的规范化匹配同口径）。
/// 用于 pull_feeds 与 Miniflux 的 feed_url 碰撞匹配：Miniflux 返回的 URL 与
/// 本地直连添加时的 URL 常有协议/www./尾斜杠/跟踪参数差异，精确匹配会漏判成
/// 新订阅 → 同一订阅出现两个本地 feed（文章翻倍、状态分裂、数量与未读数
/// 对不齐的根因）。feeds 数量少（个人订阅几十个），内存规范化匹配即可。
pub fn feed_id_by_url_normalized(conn: &Connection, feed_url: &str) -> AppResult<Option<i64>> {
    let norm = normalize_url(feed_url);
    let feeds = list_feeds(conn)?;
    Ok(feeds
        .iter()
        .find(|f| normalize_url(&f.feed_url) == norm)
        .map(|f| f.id))
}

/* ============================================================
commands.rs SQL 抽取层（TASK-017）
============================================================ */

/// 确保「未分类」folder 存在：已有则返回其 id，无则创建后返回。
/// 用于 add_feed 未指定分类时的兜底逻辑。
pub fn ensure_uncategorized_folder(conn: &Connection) -> AppResult<i64> {
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM folders WHERE name = '未分类' ORDER BY id LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    match existing {
        Some(fid) => Ok(fid),
        None => create_folder(conn, "未分类", "article"),
    }
}

/// 检查 folder 是否存在（update_feed 参数校验用）
pub fn folder_exists(conn: &Connection, folder_id: i64) -> AppResult<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM folders WHERE id = ?1",
        params![folder_id],
        |r| r.get(0),
    )?;
    Ok(count > 0)
}

/// 列出指定范围内的未读文章 id（既有的范围筛选一致性测试仍消费；
/// 「全部已读」的入队已由 [`super::articles::mark_all_read_with_enqueue`] 集合化，
/// 不再经本函数逐 id 往返）。
/// feed_id/folder_id 为 None 时查全部未读。范围子句与 db::mark_all_read 同源
/// （scope_clause，F8/TASK-094 收口）——两处口径永远一致。
pub fn list_unread_ids_scoped(
    conn: &Connection,
    feed_id: Option<i64>,
    folder_id: Option<i64>,
    starred_only: bool,
    since_ms: Option<i64>,
    layout: Option<&str>,
) -> AppResult<Vec<i64>> {
    let (clause, binds) =
        super::articles::scope_clause(feed_id, folder_id, starred_only, since_ms, layout);
    let sql = format!("SELECT id FROM articles WHERE is_read = 0{clause}");
    let refs: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(refs.as_slice(), |r| r.get(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 获取文章 URL（extract_fulltext 取原文网页地址用）
pub fn get_article_url(conn: &Connection, article_id: i64) -> AppResult<Option<String>> {
    let url = conn
        .query_row(
            "SELECT url FROM articles WHERE id = ?1",
            params![article_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    Ok(url)
}

/// 获取文章正文 HTML（extract_fulltext 退化判定用）
pub fn get_article_content_html(conn: &Connection, article_id: i64) -> AppResult<String> {
    let html: String = conn
        .query_row(
            "SELECT COALESCE(content_html, '') FROM articles WHERE id = ?1",
            params![article_id],
            |r| r.get(0),
        )
        .unwrap_or_default();
    Ok(html)
}

/// 更新文章全文提取结果：覆盖 content_html、置提取标志，并按同一来源重写
/// body_text（R1：纯文本列/搜索/摘要必须与提取正文同源，不得残留旧 RSS 文本——
/// 旧实现只写 HTML+flag，后续刷新保护会冻结「HTML=全文、body_text=旧 RSS」的错配）。
pub fn update_article_fulltext(
    conn: &Connection,
    article_id: i64,
    content_html: &str,
    extracted: bool,
) -> AppResult<()> {
    let body_text = crate::sanitize::html_to_text(content_html);
    conn.execute(
        "UPDATE articles SET content_html = ?1, body_text = ?2, fulltext_extracted = ?3 WHERE id = ?4",
        params![content_html, body_text, extracted as i64, article_id],
    )?;
    Ok(())
}

/// 更新文章封面（仅在现有封面为空时）：extract_fulltext 头图兜底用
pub fn update_article_image_if_empty(
    conn: &Connection,
    article_id: i64,
    image_url: &str,
) -> AppResult<()> {
    conn.execute(
        "UPDATE articles SET image_url = COALESCE(image_url, ?1) WHERE id = ?2",
        params![image_url, article_id],
    )?;
    Ok(())
}

/// 导出全部 feeds 附带 folder 名（OPML 导出用）
pub fn export_feeds_with_folders(
    conn: &Connection,
) -> AppResult<Vec<(String, String, Option<String>)>> {
    let mut stmt = conn.prepare(
        "SELECT f.title, f.feed_url, fo.name
         FROM feeds f LEFT JOIN folders fo ON f.folder_id = fo.id
         ORDER BY fo.name, f.title",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
        ))
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 统计未绑定的本地源数量（sync_save 首连判定用）
pub fn count_unbound_local_feeds(conn: &Connection) -> AppResult<i64> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM feeds WHERE origin = 'local' AND remote_id IS NULL",
        [],
        |r| r.get(0),
    )?;
    Ok(count)
}

/// 列出未绑定的本地源（sync_local_feeds 推送用）
pub fn list_unbound_local_feeds(conn: &Connection) -> AppResult<Vec<(i64, String, Option<i64>)>> {
    let mut stmt = conn.prepare(
        "SELECT f.id, f.feed_url, f.folder_id FROM feeds f
         WHERE f.origin = 'local' AND f.remote_id IS NULL",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 获取文章摘要用数据（ai_summarize 用）：(title, body_text, ai_summary)
pub fn get_article_for_summary(
    conn: &Connection,
    article_id: i64,
) -> AppResult<Option<(String, String, Option<String>)>> {
    let row = conn
        .query_row(
            "SELECT title, COALESCE(body_text, ''), ai_summary FROM articles WHERE id = ?1",
            params![article_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()?;
    Ok(row)
}

/// 获取文章翻译用数据（ai_translate 用）：(title, content_html, translated_content)
pub fn get_article_for_translation(
    conn: &Connection,
    article_id: i64,
) -> AppResult<Option<(String, String, Option<String>)>> {
    let row = conn
        .query_row(
            "SELECT title, COALESCE(content_html, ''), translated_content FROM articles WHERE id = ?1",
            params![article_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()?;
    Ok(row)
}

/* ============================================================
sync.rs SQL 抽取层（TASK-022）
============================================================ */

/// 获取文章的 remote_id（Push 段检查绑定用）
pub fn get_article_remote_id(conn: &Connection, article_id: i64) -> AppResult<Option<i64>> {
    let remote_id = conn
        .query_row(
            "SELECT remote_id FROM articles WHERE id = ?1",
            params![article_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(remote_id.flatten())
}

/// 按名称查询 folder id（Pull 段分类匹配用）
pub fn find_folder_by_name(conn: &Connection, name: &str) -> AppResult<Option<i64>> {
    let id = conn
        .query_row(
            "SELECT id FROM folders WHERE name = ?1",
            params![name],
            |r| r.get(0),
        )
        .optional()?;
    Ok(id)
}

/// 获取第一个 folder id（Pull 段兜底分类用）
///
/// TASK-070：本函数零生产调用（生产已改用 ensure_uncategorized_folder），
/// 但其唯一引用是 crate 内测试 db/sync_extraction_tests.rs（不在本任务
/// allowed_paths 内），故按范围约束保留，待后续任务连同该测试一并处置。
pub fn get_first_folder_id(conn: &Connection) -> AppResult<Option<i64>> {
    let id = conn
        .query_row("SELECT id FROM folders LIMIT 1", [], |r| r.get(0))
        .optional()?;
    Ok(id)
}

/// 更新 feed 标题和站点 URL（仅当本地标题等于 feed_url 时，Pull 段远端标题回填用）
pub fn update_feed_title_if_empty(
    conn: &Connection,
    feed_id: i64,
    title: &str,
    site_url: Option<&str>,
) -> AppResult<()> {
    conn.execute(
        "UPDATE feeds SET
            title = CASE WHEN title = feed_url THEN ?1 ELSE title END,
            site_url = COALESCE(site_url, ?2)
         WHERE id = ?3",
        params![title, site_url, feed_id],
    )?;
    Ok(())
}

/// 同步专用：无条件更新文章已读和收藏状态（Pull 段状态合并用）
pub fn sync_set_article_status(
    conn: &Connection,
    article_id: i64,
    is_read: bool,
    is_starred: bool,
) -> AppResult<()> {
    conn.execute(
        "UPDATE articles SET is_read = ?1, is_starred = ?2 WHERE id = ?3",
        params![is_read as i64, is_starred as i64, article_id],
    )?;
    Ok(())
}

/// 同步专用：标记文章为已读（仅当未读时），返回影响行数（Pull 段对账计数用）
pub fn sync_mark_read_if_unread(conn: &Connection, article_id: i64) -> AppResult<usize> {
    let n = conn.execute(
        "UPDATE articles SET is_read = 1 WHERE id = ?1 AND is_read = 0",
        params![article_id],
    )?;
    Ok(n)
}

/// 同步专用：标记文章为未读（仅当已读时），返回影响行数
pub fn sync_mark_unread_if_read(conn: &Connection, article_id: i64) -> AppResult<usize> {
    let n = conn.execute(
        "UPDATE articles SET is_read = 0 WHERE id = ?1 AND is_read = 1",
        params![article_id],
    )?;
    Ok(n)
}

/// 同步专用：标记文章为收藏（仅当未收藏时），返回影响行数
pub fn sync_mark_starred_if_unstarred(conn: &Connection, article_id: i64) -> AppResult<usize> {
    let n = conn.execute(
        "UPDATE articles SET is_starred = 1 WHERE id = ?1 AND is_starred = 0",
        params![article_id],
    )?;
    Ok(n)
}

/// 同步专用：取消文章收藏（仅当已收藏时），返回影响行数
pub fn sync_mark_unstarred_if_starred(conn: &Connection, article_id: i64) -> AppResult<usize> {
    let n = conn.execute(
        "UPDATE articles SET is_starred = 0 WHERE id = ?1 AND is_starred = 1",
        params![article_id],
    )?;
    Ok(n)
}

/// 同步专用：回填文章内容字段（本地为空才补，Pull 段正文兜底用）。
///
/// F07：补全的 HTML 必须与首次入库走同一净化边界——此前原始 HTML 直接补入空
/// content_html，绕过了 sanitizer 并一路抵达 dangerouslySetInnerHTML。
/// 基址取该文章的 url（与抓取入库的相对链接重写口径一致）；DB 错误正常上抛。
// Note: 可渲染 HTML 的净化口径只有 sanitize()（写入口与读边界共用） — 见 .agents/notes/implemented/architecture/2026-10-08-可渲染正文安全边界.md
pub fn backfill_article_content(
    conn: &Connection,
    article_id: i64,
    content_html: &str,
    body_text: &str,
    image_url: Option<&str>,
    enclosure_url: Option<&str>,
    enclosure_mime: Option<&str>,
) -> AppResult<()> {
    let base: Option<String> = conn
        .query_row(
            "SELECT url FROM articles WHERE id = ?1",
            params![article_id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let safe_html = crate::sanitize::sanitize(content_html, base.as_deref());
    conn.execute(
        "UPDATE articles SET
            content_html = CASE WHEN COALESCE(content_html, '') = '' THEN ?1 ELSE content_html END,
            body_text = CASE WHEN body_text = '' THEN ?2 ELSE body_text END,
            image_url = COALESCE(image_url, ?3),
            enclosure_url = COALESCE(enclosure_url, ?4),
            enclosure_mime = COALESCE(enclosure_mime, ?5)
         WHERE id = ?6",
        params![
            safe_html,
            body_text,
            image_url,
            enclosure_url,
            enclosure_mime,
            article_id
        ],
    )?;
    Ok(())
}

/* ============================================================
测试模块（TASK-017 追加）
============================================================ */

#[cfg(test)]
mod fever_checkpoint_tests {
    use super::*;
    use crate::db::MIGRATIONS;

    fn conn() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        conn
    }

    /// 三态往返与边界（i64::MAX 合法的 Pending）。
    #[test]
    fn fever_history_state_roundtrip() {
        let conn = conn();
        assert_eq!(
            fever_history_state(&conn).unwrap(),
            FeverHistoryState::Unknown,
            "缺省 = Unknown（绝不当完成）"
        );
        set_fever_history_pending(&conn, 76).unwrap();
        assert_eq!(
            fever_history_state(&conn).unwrap(),
            FeverHistoryState::Pending(76)
        );
        set_fever_history_pending(&conn, i64::MAX).unwrap();
        assert_eq!(
            fever_history_state(&conn).unwrap(),
            FeverHistoryState::Pending(i64::MAX)
        );
        set_fever_history_complete(&conn).unwrap();
        assert_eq!(
            fever_history_state(&conn).unwrap(),
            FeverHistoryState::Complete
        );
        set_setting(&conn, "fever_history_state", "   ").unwrap();
        assert_eq!(
            fever_history_state(&conn).unwrap(),
            FeverHistoryState::Unknown,
            "空白视为缺失 = Unknown"
        );
    }

    /// 损坏值必须显式报错（不得当完成、不得静默 reset）。
    #[test]
    fn fever_history_state_corrupt_is_error() {
        let conn = conn();
        for bad in [
            "not-a-state",
            "pending:",
            "pending:0",
            "pending:-9",
            "pending:abc",
            "completed",
            "PENDING:5",
        ] {
            set_setting(&conn, "fever_history_state", bad).unwrap();
            let err = fever_history_state(&conn).expect_err("损坏状态必须报错");
            assert!(
                err.to_string().contains("fever_history_state"),
                "{bad:?} 的错误应带键名：{err}"
            );
        }
    }
}
