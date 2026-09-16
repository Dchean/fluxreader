use super::*;
use rusqlite::params;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct ArticleRow {
    pub id: i64,
    pub feed_id: i64,
    pub url: Option<String>,
    pub title: String,
    pub author: Option<String>,
    pub snippet: String,
    pub content_html: Option<String>,
    pub image_url: Option<String>,
    pub enclosure_url: Option<String>,
    pub enclosure_mime: Option<String>,
    pub duration_sec: Option<i64>,
    pub ai_summary: Option<String>,
    pub translated_content: Option<String>,
    pub source: String,
    pub published_at: Option<String>,
    pub is_read: bool,
    pub is_starred: bool,
    /// 正文是否已被全文提取覆盖（手动按钮/自动模式共用状态源）
    pub fulltext_extracted: bool,
}

/// 列表页条目（轻量：默认不含正文 HTML，snippet 截断；with_content 时附带正文，
/// 供社交/通知布局直接渲染，免去逐篇 get_article 水合的 IPC 洪峰与「加载正文」等待）。
#[derive(Debug, Serialize)]
pub struct ArticleListItem {
    pub id: i64,
    pub feed_id: i64,
    pub title: String,
    pub author: Option<String>,
    pub snippet: String,
    pub image_url: Option<String>,
    pub enclosure_url: Option<String>,
    pub enclosure_mime: Option<String>,
    pub duration_sec: Option<i64>,
    pub ai_summary: Option<String>,
    pub source: String,
    pub published_at: Option<String>,
    pub is_read: bool,
    pub is_starred: bool,
    /* with_content=true 时填充（否则 None） */
    pub url: Option<String>,
    pub content_html: Option<String>,
    pub translated_content: Option<String>,
    pub fulltext_extracted: bool,
}

/* ============================================================
Folders
============================================================ */

/// 抓取管线产出的新条目（source 由抓取层决定）
#[derive(Debug)]
pub struct NewArticle {
    pub guid: String,
    pub url: Option<String>,
    pub title: String,
    pub author: Option<String>,
    pub summary: Option<String>,
    pub content_html: Option<String>,
    pub body_text: String,
    pub image_url: Option<String>,
    pub enclosure_url: Option<String>,
    pub enclosure_mime: Option<String>,
    pub duration_sec: Option<i64>,
    pub published_at: Option<String>,
    pub source: String,
}

const ARTICLE_COLS: &str = "id, feed_id, url, title, author, summary, content_html, image_url, enclosure_url, enclosure_mime, duration_sec, ai_summary, translated_content, source, published_at, is_read, is_starred, fulltext_extracted";

fn article_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ArticleRow> {
    Ok(ArticleRow {
        id: r.get(0)?,
        feed_id: r.get(1)?,
        url: r.get(2)?,
        title: r.get(3)?,
        author: r.get(4)?,
        snippet: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
        content_html: r.get(6)?,
        image_url: r.get(7)?,
        enclosure_url: r.get(8)?,
        enclosure_mime: r.get(9)?,
        duration_sec: r.get(10)?,
        ai_summary: r.get(11)?,
        translated_content: r.get(12)?,
        source: r.get(13)?,
        published_at: r.get(14)?,
        is_read: r.get::<_, i64>(15)? != 0,
        is_starred: r.get::<_, i64>(16)? != 0,
        fulltext_extracted: r.get::<_, i64>(17)? != 0,
    })
}

fn article_list_item(r: &rusqlite::Row<'_>) -> rusqlite::Result<ArticleListItem> {
    // 列顺序依赖 list_articles 的 SELECT；当 with_content=false 时尾部四列为 NULL/0。
    Ok(ArticleListItem {
        id: r.get(0)?,
        feed_id: r.get(1)?,
        title: r.get(2)?,
        author: r.get(3)?,
        snippet: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
        image_url: r.get(5)?,
        enclosure_url: r.get(6)?,
        enclosure_mime: r.get(7)?,
        duration_sec: r.get(8)?,
        ai_summary: r.get(9)?,
        source: r.get(10)?,
        published_at: r.get(11)?,
        is_read: r.get::<_, i64>(12)? != 0,
        is_starred: r.get::<_, i64>(13)? != 0,
        url: r.get(14)?,
        content_html: r.get(15)?,
        translated_content: r.get(16)?,
        fulltext_extracted: r.get::<_, i64>(17)? != 0,
    })
}

/// 列表查询参数：feed 范围 + 视图筛选 + 排序 + 分页。
#[derive(Debug, Clone)]
pub struct ArticleQuery {
    pub feed_id: Option<i64>,
    pub folder_id: Option<i64>,
    pub only_unread: bool,
    pub only_starred: bool,
    pub only_today: bool,
    pub newest_first: bool,
    pub limit: i64,
    pub offset: i64,
    /// 附带正文 HTML（社交/通知布局直接渲染，免逐篇水合）
    pub with_content: bool,
}

/// 列表条目（含 body_text 截断生成的 snippet；with_content 时附带正文）
pub fn list_articles(conn: &Connection, q: &ArticleQuery) -> AppResult<Vec<ArticleListItem>> {
    // 正文列只在 with_content 时 SELECT（社交/通知布局需要），其余布局保持轻量查询。
    // 尾部四列顺序与 article_list_item 的索引 14..17 严格对应。
    let content_cols = if q.with_content {
        "a.url, a.content_html, a.translated_content, a.fulltext_extracted"
    } else {
        "NULL, NULL, NULL, 0"
    };
    let mut sql = format!(
        "SELECT a.id, a.feed_id, a.title, a.author,
                COALESCE(NULLIF(a.summary, ''), substr(a.body_text, 1, 280)) AS snippet,
                a.image_url, a.enclosure_url, a.enclosure_mime, a.duration_sec,
                a.ai_summary, a.source, a.published_at, a.is_read, a.is_starred,
                {content_cols}
         FROM articles a",
    );
    // 值全部走绑定参数（占位符序号即绑定顺序），条件文本只拼固定字符串
    let (where_clauses, mut params) = article_where(q);
    if !where_clauses.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&where_clauses.join(" AND "));
    }
    sql.push_str(if q.newest_first {
        " ORDER BY COALESCE(a.published_at, a.fetched_at) DESC LIMIT ? OFFSET ?"
    } else {
        " ORDER BY COALESCE(a.published_at, a.fetched_at) ASC LIMIT ? OFFSET ?"
    });
    params.push(q.limit.into());
    params.push(q.offset.into());

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(params), article_list_item)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 构建列表查询的 WHERE 条件 + 绑定参数（`list_articles` 与 `article_index`
/// 共用，保证「绝对位置」与「列表顺序」口径一致）。
fn article_where(q: &ArticleQuery) -> (Vec<&'static str>, Vec<rusqlite::types::Value>) {
    let mut where_clauses: Vec<&'static str> = vec![];
    let mut params: Vec<rusqlite::types::Value> = Vec::new();
    if let Some(fid) = q.feed_id {
        where_clauses.push("a.feed_id = ?");
        params.push(fid.into());
    }
    if let Some(folder) = q.folder_id {
        where_clauses.push("a.feed_id IN (SELECT id FROM feeds WHERE folder_id = ?)");
        params.push(folder.into());
    }
    if q.only_unread {
        where_clauses.push("a.is_read = 0");
    }
    if q.only_starred {
        where_clauses.push("a.is_starred = 1");
    }
    if q.only_today {
        where_clauses.push("date(a.published_at) = date('now', 'localtime')");
    }
    (where_clauses, params)
}

/// 计算某篇文章在当前筛选排序下的绝对位置（0 起）。
/// 用窗口函数 ROW_NUMBER() OVER (ORDER BY ...) - 1 求位置，供前端「搜索/深层
/// 打开文章后只加载目标那一页」的双向分页锚定——无需从头拉全量。
/// 排序与 list_articles 完全同口径（COALESCE(published_at, fetched_at)）。
pub fn article_index(
    conn: &Connection,
    q: &ArticleQuery,
    article_id: i64,
) -> AppResult<Option<i64>> {
    let (where_clauses, mut params) = article_where(q);
    let order = if q.newest_first {
        "COALESCE(a.published_at, a.fetched_at) DESC"
    } else {
        "COALESCE(a.published_at, a.fetched_at) ASC"
    };
    let where_sql = if where_clauses.is_empty() {
        "1=1".to_string()
    } else {
        where_clauses.join(" AND ")
    };
    let sql = format!(
        "SELECT pos FROM (
             SELECT a.id AS aid,
                    ROW_NUMBER() OVER (ORDER BY {order}) - 1 AS pos
             FROM articles a
             WHERE {where_sql}
         ) WHERE aid = ?",
        order = order,
        where_sql = where_sql,
    );
    params.push(article_id.into());
    let pos = conn
        .prepare(&sql)?
        .query_row(rusqlite::params_from_iter(params), |r| r.get::<_, i64>(0))
        .optional()?;
    Ok(pos)
}

pub fn get_article(conn: &Connection, id: i64) -> AppResult<Option<ArticleRow>> {
    let row = conn
        .query_row(
            &format!("SELECT {ARTICLE_COLS} FROM articles WHERE id = ?1"),
            params![id],
            article_row,
        )
        .optional()?;
    Ok(row)
}

/// 批量拉取文章详情（正文水合专用）：一次查询返回多篇完整行，
/// 避免前端逐篇 get_article 造成 IPC 洪峰 + 每篇一次 set 的 O(n) 重渲染。
/// IN 子句占位符按 id 数量动态展开（id 列表来自受控的 ids 参数，非用户输入拼接）。
pub fn get_articles(conn: &Connection, ids: &[i64]) -> AppResult<Vec<ArticleRow>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = vec!["?"; ids.len()].join(",");
    let sql = format!("SELECT {ARTICLE_COLS} FROM articles WHERE id IN ({placeholders})");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(ids.iter()), article_row)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 全文搜索：LIKE 子串匹配，命中按发布时间倒序，返回与列表页同构的轻量行。
///
/// 不用 FTS5 MATCH 的原因：unicode61 分词器把连续中文整段当作一个 token，
/// 搜「科技」永远匹配不到正文里的「科技公司新闻」——对中文用户形同虚设；
/// 且 FTS 查询语法字符（"*()" 等）需要额外转义，`node.js`、`C++` 这类词
/// 的前缀/精确语义也很反直觉。个人库规模（≤ 数千篇）LIKE 全表扫毫秒级
/// 完成，语义对任意语言/任意字符都正确（就是"包含这个子串"）。
///
/// 多关键词 AND（所有词都要命中，与主流阅读器一致）；匹配字段：标题 +
/// 正文纯文本 + 摘要 + AI 摘要 + AI 翻译；LIKE 通配符 %/_ 按字面转义。
/// 注：AI 字段纳入命中后（SRH-2），`articles_fts` FTS5 表（unicode61 分词对中文
/// 不友好，见迁移注释）不再作为搜索入口，仅保留触发器同步作历史遗留。
pub fn search_articles(
    conn: &Connection,
    query: &str,
    limit: i64,
) -> AppResult<Vec<ArticleListItem>> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let terms: Vec<String> = q
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .map(|t| {
            t.replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        })
        .collect();
    if terms.is_empty() {
        return Ok(Vec::new());
    }

    // 每个词一组 (title/body/summary/ai_summary/translated LIKE)，词间 AND。
    // 占位符逐个编号（?N 是编号引用，跨词组复用 ?1..?5 会错位），
    // ESCAPE 声明 SQLite 的 LIKE 通配符转义字符 '\'。
    let mut where_parts = Vec::with_capacity(terms.len());
    let mut like_args: Vec<String> = Vec::with_capacity(terms.len() * 5);
    for (i, t) in terms.iter().enumerate() {
        let pat = format!("%{t}%");
        let base = i * 5 + 1;
        where_parts.push(format!(
            "(a.title LIKE ?{b} ESCAPE '\\' OR a.body_text LIKE ?{c} ESCAPE '\\' OR COALESCE(a.summary,'') LIKE ?{d} ESCAPE '\\' OR COALESCE(a.ai_summary,'') LIKE ?{e} ESCAPE '\\' OR COALESCE(a.translated_content,'') LIKE ?{f} ESCAPE '\\')",
            b = base,
            c = base + 1,
            d = base + 2,
            e = base + 3,
            f = base + 4,
        ));
        like_args.push(pat.clone());
        like_args.push(pat.clone());
        like_args.push(pat.clone());
        like_args.push(pat.clone());
        like_args.push(pat);
    }
    let sql = format!(
        "SELECT a.id, a.feed_id, a.title, a.author,
                COALESCE(NULLIF(a.summary, ''), substr(a.body_text, 1, 280)) AS snippet,
                a.image_url, a.enclosure_url, a.enclosure_mime, a.duration_sec,
                a.ai_summary, a.source, a.published_at, a.is_read, a.is_starred,
                NULL, NULL, NULL, 0
         FROM articles a
         WHERE {}
         ORDER BY a.published_at DESC
         LIMIT {limit}",
        where_parts.join(" AND ")
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(like_args), article_list_item)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 抓取管线写入（带 feed_id）：guid 冲突时仅更新内容字段（正文/图片/enclosure/来源），
/// 已读/收藏/AI 产物等用户状态字段不动 —— 直连重抓到已读文章时不会"复活"它。
/// 同 URL 已有文章 → 返回其 id（去重判定 + 墓碑 kept_aid 记账共用）。
/// 匹配键用规范化 URL（url_norm）：跟踪参数/www./m./协议/尾斜杠/AMP 差异
/// 不再产生重复文章。
fn existing_article_with_url(conn: &Connection, url: &str) -> AppResult<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT id FROM articles WHERE url_norm = ?1 ORDER BY id LIMIT 1",
            params![normalize_url(url)],
            |r| r.get(0),
        )
        .optional()?)
}

/// 清空去重墓碑（smartDedup 开→关 瞬间调用：用户想让重复文章回来）
pub fn clear_dedup_tombstones(conn: &Connection) -> AppResult<usize> {
    let n = conn.execute("DELETE FROM deduped_urls", [])?;
    Ok(n)
}

/* ============================================================
账号数据边界 / 缓存清理
============================================================ */

/// 断开连接时清理服务端来源的数据：删 origin='remote' 的订阅（级联清
/// 其文章/绑定/队列/墓碑），清空本地条目上的 Miniflux 绑定与副本记账、
/// folders/feeds 的 remote_id 残留。用户直连订阅（origin='local'）保留。
/// 空目录（pull 建的、没了成员）一并删除。
pub fn purge_remote_data(conn: &mut Connection) -> AppResult<(usize, usize)> {
    let tx = conn.transaction()?;
    // 0. 先恢复「原本是本地直连添加、后被 hybrid 模式转为服务端来源」的订阅
    //    ——这类源在断开连接时应保留（回到纯本地直连），而非随服务端数据删除。
    tx.execute(
        "UPDATE feeds SET origin = 'local', origin_was_local = 0 WHERE origin_was_local = 1",
        [],
    )?;
    // 1. 服务端来源订阅（级联：articles → sync_queue / deduped_urls 墓碑 / FTS 触发器）
    let feeds = tx.execute("DELETE FROM feeds WHERE origin = 'remote'", [])?;
    // 2. 本地直连条目上的绑定/副本/已读态全部回归纯本地
    let articles = tx.execute(
        "UPDATE articles SET remote_id = NULL, remote_dup_ids = ''",
        [],
    )?;
    // 3. 本地直连源/分类的 remote_id 绑定清除
    tx.execute("UPDATE feeds SET remote_id = NULL", [])?;
    tx.execute("UPDATE folders SET remote_id = NULL", [])?;
    // 4. 清空待推队列（推给这个账号的变更不再有意义）
    tx.execute("DELETE FROM sync_queue", [])?;
    // 5. 空目录（Pull 建的远端分类，删完成员后空了）——保留用户建的非空目录
    tx.execute(
        "DELETE FROM folders WHERE id NOT IN (SELECT DISTINCT folder_id FROM feeds WHERE folder_id IS NOT NULL)",
        [],
    )?;
    tx.commit()?;
    Ok((feeds, articles))
}

/// 缓存清理：删除指定天数之前的文章（含 FTS/队列级联）与/或 AI 产物。
/// 保留项：收藏文章永不清（用户显式标过星）；**未读文章永不清**——删除后
/// 下一次全量同步会按服务器状态重新拉回（Miniflux 端仍是 unread），数据
/// 打架等于白删；scope='ai' 只清 AI 摘要与翻译缓存（正文保留，重新打开
/// 可再生成）。返回 (删文章数, 清 AI 字段数)。
pub fn cleanup_cache(conn: &mut Connection, days: i64, scope: &str) -> AppResult<(usize, usize)> {
    let tx = conn.transaction()?;
    let cutoff = format!("datetime('now', '-{days} days', 'localtime')");
    let (mut deleted, mut ai_cleared) = (0usize, 0usize);
    if scope == "articles" {
        deleted = tx.execute(
            &format!(
                "DELETE FROM articles
                 WHERE published_at < {cutoff}
                   AND is_read = 1
                   AND is_starred = 0
                   AND id NOT IN (SELECT article_id FROM sync_queue WHERE article_id IS NOT NULL)"
            ),
            [],
        )?;
        // 墓碑指向被删文章的清掉（kept_aid 级联已处理，这里兜底空墓碑）
        tx.execute(
            "DELETE FROM deduped_urls WHERE kept_aid NOT IN (SELECT id FROM articles)",
            [],
        )?;
    } else if scope == "ai" {
        ai_cleared = tx.execute(
            &format!(
                "UPDATE articles SET ai_summary = NULL, translated_content = NULL
                 WHERE (ai_summary IS NOT NULL OR translated_content IS NOT NULL)
                   AND published_at < {cutoff}"
            ),
            [],
        )?;
        // FTS 触发器同步（UPDATE 触发 articles_au 已处理）
    }
    tx.commit()?;
    Ok((deleted, ai_cleared))
}

/* ============================================================
URL 规范化（去重匹配键）
============================================================ */

/// dedup=true 时同 URL 文章跨源去重（智能去重：同一新闻被多个源推送只留首个）。
/// 丢弃时写 deduped_urls 墓碑：记录保留了哪篇；墓碑在（开关未关闭过）时，
/// 同 URL 的后续重放（guid 稳定 → 每轮刷新都会再来）持续被拦，
/// 直到用户关闭智能去重（清墓碑，重复文章按用户意图重新入库）。
pub fn upsert_article_with_feed(
    conn: &Connection,
    feed_id: i64,
    a: &NewArticle,
    dedup: bool,
) -> AppResult<(i64, bool)> {
    // 智能去重：规范化 URL 已存在于任一源 → 跳过（返回非新增，计数不膨胀）
    if dedup {
        if let Some(url) = a.url.as_deref().filter(|u| !u.is_empty()) {
            let norm = normalize_url(url);
            if let Some(kept_aid) = existing_article_with_url(conn, url)? {
                // 墓碑记账（INSERT OR REPLACE：重放时刷新 kept_aid/kept_at；
                // 键用规范化 URL——重放时参数饰词可能不同，规范化后才对得上）
                let _ = conn.execute(
                    "INSERT OR REPLACE INTO deduped_urls (url, kept_aid) VALUES (?1, ?2)",
                    params![norm, kept_aid],
                );
                return Ok((0, false));
            }
            // 无现存文章但墓碑在（保留的那篇已被用户删掉）：同 URL 仍拦。
            // 否则删掉一篇 → 下轮刷新同 URL 立即回来，删除形同虚设。
            let tombstoned: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM deduped_urls WHERE url = ?1)",
                params![norm],
                |r| r.get(0),
            )?;
            if tombstoned {
                return Ok((0, false));
            }
        }
    }
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM articles WHERE feed_id = ?1 AND guid = ?2",
            params![feed_id, a.guid],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = existing {
        conn.execute(
            "UPDATE articles SET
                url = COALESCE(?2, url),
                url_norm = CASE WHEN ?2 IS NOT NULL AND ?2 != '' THEN ?13 ELSE url_norm END,
                title = ?3,
                author = COALESCE(?4, author),
                summary = COALESCE(?5, summary),
                content_html = COALESCE(?6, content_html),
                body_text = CASE WHEN ?7 != '' THEN ?7 ELSE body_text END,
                image_url = COALESCE(?8, image_url),
                enclosure_url = COALESCE(?9, enclosure_url),
                enclosure_mime = COALESCE(?10, enclosure_mime),
                duration_sec = COALESCE(?11, duration_sec),
                published_at = COALESCE(?12, published_at)
             WHERE id = ?1",
            params![
                id,
                a.url,
                a.title,
                a.author,
                a.summary,
                a.content_html,
                a.body_text,
                a.image_url,
                a.enclosure_url,
                a.enclosure_mime,
                a.duration_sec,
                a.published_at,
                a.url.as_deref().map(normalize_url)
            ],
        )?;
        Ok((id, false))
    } else if let Some(url) = a.url.as_deref().filter(|u| !u.is_empty()) {
        // 同 feed 内按规范化 URL 兜底去重：direct 抓取与 Miniflux 同步的 guid 不同
        // （direct 用原始 guid，Miniflux 用 `miniflux-{id}`），但 URL 相同是同一篇。
        // 命中已有文章时只更新内容（正文/标题/封面/附件），**保留 source 与状态**
        // （is_read/is_starred/remote_id）——抓取只负责内容、同步只负责状态，
        // 避免「切换本地抓取后数量翻倍」与「状态被抓取覆盖回未读」。
        let by_url: Option<i64> = conn
            .query_row(
                "SELECT id FROM articles WHERE feed_id = ?1 AND url_norm = ?2 ORDER BY id LIMIT 1",
                params![feed_id, normalize_url(url)],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = by_url {
            conn.execute(
                "UPDATE articles SET
                    title = ?1,
                    author = COALESCE(?2, author),
                    summary = COALESCE(?3, summary),
                    content_html = COALESCE(?4, content_html),
                    body_text = CASE WHEN ?5 != '' THEN ?5 ELSE body_text END,
                    image_url = COALESCE(?6, image_url),
                    enclosure_url = COALESCE(?7, enclosure_url),
                    enclosure_mime = COALESCE(?8, enclosure_mime),
                    duration_sec = COALESCE(?9, duration_sec),
                    published_at = COALESCE(?10, published_at)
                 WHERE id = ?11",
                params![
                    a.title,
                    a.author,
                    a.summary,
                    a.content_html,
                    a.body_text,
                    a.image_url,
                    a.enclosure_url,
                    a.enclosure_mime,
                    a.duration_sec,
                    a.published_at,
                    id,
                ],
            )?;
            Ok((id, false))
        } else {
            insert_new_article(conn, feed_id, a)
        }
    } else {
        insert_new_article(conn, feed_id, a)
    }
}

/// 插入新文章（upsert_article_with_feed 的兜底路径）。
fn insert_new_article(conn: &Connection, feed_id: i64, a: &NewArticle) -> AppResult<(i64, bool)> {
    conn.execute(
        "INSERT INTO articles
            (feed_id, guid, url, url_norm, title, author, summary, content_html, body_text, image_url,
             enclosure_url, enclosure_mime, duration_sec, published_at, source)
         VALUES (?1, ?2, ?3, ?15, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            feed_id,
            a.guid,
            a.url,
            a.title,
            a.author,
            a.summary,
            a.content_html,
            a.body_text,
            a.image_url,
            a.enclosure_url,
            a.enclosure_mime,
            a.duration_sec,
            a.published_at,
            a.source,
            a.url.as_deref().map(normalize_url)
        ],
    )?;
    Ok((conn.last_insert_rowid(), true))
}

pub fn set_read(conn: &Connection, id: i64, read: bool) -> AppResult<()> {
    conn.execute(
        "UPDATE articles SET is_read = ?1 WHERE id = ?2",
        params![read as i64, id],
    )?;
    Ok(())
}

/// AI 产物落库（缓存）：完成后写入，重复打开不重算。
/// UPDATE 触发器自动同步 FTS 索引（ai_summary/translated_content 可被搜索）。
pub fn set_article_ai_fields(
    conn: &Connection,
    id: i64,
    ai_summary: Option<&str>,
    translated_content: Option<&str>,
) -> AppResult<()> {
    conn.execute(
        "UPDATE articles SET
            ai_summary = COALESCE(?1, ai_summary),
            translated_content = COALESCE(?2, translated_content)
         WHERE id = ?3",
        params![ai_summary, translated_content, id],
    )?;
    Ok(())
}

pub fn set_starred(conn: &Connection, id: i64, starred: bool) -> AppResult<()> {
    conn.execute(
        "UPDATE articles SET is_starred = ?1 WHERE id = ?2",
        params![starred as i64, id],
    )?;
    Ok(())
}

/// 查询「无封面 + 有原文 URL + 直连来源」的文章 id（封面后台补全用）。
/// 摘要型 RSS（少数派等）不带 media 字段，封面只能从文章页 og:image 拿；
/// 这里只取直连源（source='direct'）的条目——Miniflux 源在入库时已用
/// 正文第一图兜底，无需再抓文章页。limit 限制单轮批处理量（避免启动时
/// 一次性扫全库 + 轰炸源站）。
pub fn articles_without_cover(conn: &Connection, limit: i64) -> AppResult<Vec<(i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT id, url FROM articles
         WHERE (image_url IS NULL OR image_url = '')
           AND url IS NOT NULL AND url != ''
           AND source = 'direct'
         ORDER BY published_at DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 全部已读：作用于当前筛选范围（feed/folder/all），与前端「全部已读」按钮语义一致
pub fn mark_all_read(
    conn: &Connection,
    feed_id: Option<i64>,
    folder_id: Option<i64>,
    starred_only: bool,
    since_ms: Option<i64>,
) -> AppResult<usize> {
    let mut sql = String::from("UPDATE articles SET is_read = 1 WHERE is_read = 0");
    let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(fid) = feed_id {
        sql.push_str(" AND feed_id = ?");
        binds.push(Box::new(fid));
    }
    if let Some(folder) = folder_id {
        sql.push_str(" AND feed_id IN (SELECT id FROM feeds WHERE folder_id = ?)");
        binds.push(Box::new(folder));
    }
    // F8：视图口径过滤——收藏视图只影响收藏文章；今天视图只影响当日文章
    // （since_ms 由前端按本地日界计算传入，datetime() 统一归一化后再比较）
    if starred_only {
        sql.push_str(" AND is_starred = 1");
    }
    if let Some(ms) = since_ms {
        sql.push_str(" AND datetime(published_at) >= datetime(?, 'unixepoch')");
        binds.push(Box::new(ms / 1000));
    }
    let refs: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
    let n = conn.execute(&sql, refs.as_slice())?;
    Ok(n)
}

/// 条目计数（侧边栏角标）：按 feed 聚合，含未读/收藏/今日细分。
/// 前端据此聚合分类/视图的精确计数——不依赖文章列表的分页 limit，
/// 保证数字准确（「全部/未读数字被 limit 截断」的根因修复）。
#[derive(Debug, Serialize)]
pub struct FeedCounts {
    pub feed_id: i64,
    pub total: i64,
    pub unread: i64,
    pub starred: i64,
    pub today: i64,
}

pub fn feed_counts(conn: &Connection) -> AppResult<Vec<FeedCounts>> {
    let mut stmt = conn.prepare(
        "SELECT feed_id, COUNT(*), SUM(is_read = 0), SUM(is_starred = 1),
                SUM(date(published_at) = date('now', 'localtime'))
         FROM articles GROUP BY feed_id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(FeedCounts {
            feed_id: r.get(0)?,
            total: r.get(1)?,
            unread: r.get::<_, Option<i64>>(2)?.unwrap_or(0),
            starred: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
            today: r.get::<_, Option<i64>>(4)?.unwrap_or(0),
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/* ============================================================
Settings（键值对：同步 Endpoint/凭据、AI/同步相关配置）
============================================================ */
