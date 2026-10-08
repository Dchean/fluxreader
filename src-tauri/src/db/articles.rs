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
读取与检索：行/列表模型、分页游标、搜索、按 URL 去重查询
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
    /// 布局过滤（TASK-094 / REQ-107）：与前端 resolveFeedLayout 同口径
    /// （feed 级覆盖 → 分类兜底，见 [`LAYOUT_FILTER_SQL`]）。
    /// None = 不按布局过滤（既有行为逐字不变）。
    pub layout: Option<String>,
    /// TASK-117：keyset 续拉游标锚——上一页最后一行的 (published_at 原文, id)。
    /// **两者同时给出**才启用 keyset 谓词（缺一回落 OFFSET 既有语义：成对约束
    /// 避免半截游标的歧义；前端续拉恒成对发送）。article_index 不消费本字段
    /// （锚定求的是全筛选集里的绝对位置，与续拉起点无关），谓词只在
    /// [`list_articles_sql`] 追加、不进 [`article_where`]。
    pub last_published: Option<String>,
    pub last_id: Option<i64>,
}

/// 列表排序表达式（M-5 / REQ-108；TASK-117 起补 `a.id` 决胜）——
/// `list_articles` 与 `article_index` 的窗口排序共用同一常量，保证「绝对位置」
/// 与「列表顺序」同口径。published_at 非空不变量由 v14 迁移的触发器 + 写入兜底
/// 固化（见 db/migrations.rs），使本表达式与旧 `COALESCE(published_at, fetched_at)`
/// 逐行等价（由 migrations 的排序等价测试锁定）。
///
// Note: 分页谓词必须与 ORDER BY 逐字同构（裸字符串比较，成对给出游标） — 见 .agents/notes/implemented/architecture/2026-10-05-查询与分页契约.md
/// TASK-117（审计 P1-1）：keyset 分页要求排序键是**全序**——published_at 是
/// 秒级粒度的 TEXT，同秒文章此前顺序不定（SQLite 对相等键保持扫描序，非确定），
/// keyset 游标会因此重复/漏行。补 `a.id`（INTEGER PRIMARY KEY = rowid）决胜后
/// 全序成立。索引有序驱动（免 TEMP B-TREE）由 v17 迁移的
/// idx_articles_published_id (published_at, id) 承接（ASC 声明可正向供 ASC+id ASC、
/// 反向供 DESC+id DESC；原 idx_articles_published 的 DESC 声明带隐式 rowid ASC 尾巴，
/// 反向扫描给不出 id DESC，实测 EXPLAIN 见 tmp/audit-20261007 后的本卡探查）；
/// feed/unread 等带等值前缀的查询仍由 idx_articles_feed_published /
/// idx_articles_read_published 反向扫描有序驱动（计划断言见本文件测试）。
pub(crate) const PUBLISHED_ORDER_DESC: &str = "a.published_at DESC, a.id DESC";
pub(crate) const PUBLISHED_ORDER_ASC: &str = "a.published_at ASC, a.id ASC";

/// TASK-117：keyset 续拉谓词（DESC：严格排在游标**之前**）。占位符按出现次序
/// 绑定（last_published 两次 + last_id 一次），外部字符串 last_published 只经
/// 绑定参数进入 SQL，无任何拼接（注入面为零，由 `list_articles_sql` 的组装
/// 方式保证——条件文本只来自本文件常量）。
/// 比较必须是**裸字符串**比较（与 [`PUBLISHED_ORDER_DESC`] 同一表达式口径）：
/// keyset 正确性的根基是「谓词的『游标之后』≡ 排序的『下一行』」——若谓词用
/// datetime() 规范化而排序仍按原文（或反过来），混合时区/格式形态下行会在两套
/// 序之间错位（跳行/重复）。datetime() 包列还会让索引无法有序驱动（TEMP B-TREE，
/// 破坏 M-5 计划断言）。混合 offset 形态的时间语义由 v15 归一 + 生产写入恒
/// RFC3339 保证；即便存在历史混排，字符串序也是**确定的全序**，游标在同一序里
/// 续拉就不重不漏（分页正确性与时间语义正交）。
const KEYSET_PREDICATE_DESC: &str = "(a.published_at < ? OR (a.published_at = ? AND a.id < ?))";
/// 同 [`KEYSET_PREDICATE_DESC`]，ASC 方向（严格排在游标**之后**）。
const KEYSET_PREDICATE_ASC: &str = "(a.published_at > ? OR (a.published_at = ? AND a.id > ?))";

/// 组装列表查询 SQL + 绑定（`list_articles` 的生产字节；测试对同一产物跑
/// EXPLAIN QUERY PLAN，见 `list_query_plan_*`）。
fn list_articles_sql(q: &ArticleQuery) -> (String, Vec<rusqlite::types::Value>) {
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
    let (mut where_clauses, mut params) = article_where(q);
    // TASK-121：R1 修复——keyset 谓词追加处 `where_clauses.push`（本函数内，紧随
    // 其后）需可变绑定，漏 mut 即 CI rust job 的 error[E0596]
    // （lib 编译失败即止，tests/ 目标从未编译）。
    // TASK-117：keyset 续拉——游标锚成对给出时追加「严格排在游标之后」的谓词
    // （方向随排序翻转），并停用 OFFSET 语义（offset 参数保留绑定、调用方传 0）。
    // last_published 是外部字符串，只经绑定参数进入 SQL（见 KEYSET_PREDICATE_* 注释）。
    if let (Some(last_published), Some(last_id)) = (&q.last_published, q.last_id) {
        let (predicate, dir_params): (&'static str, Vec<rusqlite::types::Value>) = if q.newest_first
        {
            (
                KEYSET_PREDICATE_DESC,
                vec![
                    last_published.clone().into(),
                    last_published.clone().into(),
                    last_id.into(),
                ],
            )
        } else {
            (
                KEYSET_PREDICATE_ASC,
                vec![
                    last_published.clone().into(),
                    last_published.clone().into(),
                    last_id.into(),
                ],
            )
        };
        where_clauses.push(predicate);
        params.extend(dir_params);
    }
    if !where_clauses.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&where_clauses.join(" AND "));
    }
    let order = if q.newest_first {
        PUBLISHED_ORDER_DESC
    } else {
        PUBLISHED_ORDER_ASC
    };
    sql.push_str(&format!(" ORDER BY {order} LIMIT ? OFFSET ?"));
    // P3-4（自检 2026-09-29）：limit 夹取——负数在 SQLite 的 LIMIT 语义是
    // 「不限制」，调用方一处笔误就会把整个库倒出来（search_articles 的 P3[8]
    // 同款修法，见本文件 :367 附近）。负 → 0 行；合法大值原样放行，不设上限
    // （TASK-112 注释修正：前端 ARTICLES_PAGE_SIZE=500，见 src/store/internals.ts
    // ——TASK-110 起真分页；100000 是废除前 reloadFilteredEntries 的「近似全集」
    // 旧字面量，本处旧注释引用有误）。
    params.push(q.limit.max(0).into());
    // TASK-117：OFFSET 保留（兼容既有调用方），但前端列表续拉已改走 keyset
    // 谓词（last_published/last_id 成对给出时 OFFSET 恒为 0——两种游标不同时
    // 生效，见上方 keyset 组装处）。
    params.push(q.offset.into());
    (sql, params)
}

/// 列表条目（含 body_text 截断生成的 snippet；with_content 时附带正文）
pub fn list_articles(conn: &Connection, q: &ArticleQuery) -> AppResult<Vec<ArticleListItem>> {
    let (sql, params) = list_articles_sql(q);
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(params), article_list_item)?;
    let mut items = rows.collect::<Result<Vec<_>, _>>()?;
    // F07 读边界：社交/通知布局直接渲染这些字段；存量行可能从未经净化写入。
    // with_content=false 时正文列为 NULL（元数据列表），不产生清洗成本。
    if q.with_content {
        for item in &mut items {
            sanitize_renderable_fields(
                item.url.as_deref(),
                &mut item.content_html,
                &mut item.translated_content,
            );
        }
    }
    Ok(items)
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
        // M-5：标量子查询形态（见 FOLDER_FILTER_SQL）——IN 子查询会让规划器
        // 改走 feed 索引并对结果回退 TEMP B-TREE 排序；标量形态可由
        // idx_articles_published 有序驱动（实测见 tmp/task-099/explain-query-plan.md）。
        where_clauses.push(FOLDER_FILTER_SQL);
        params.push(folder.into());
    }
    if q.only_unread {
        where_clauses.push("a.is_read = 0");
    }
    if q.only_starred {
        where_clauses.push("a.is_starred = 1");
    }
    if q.only_today {
        // TASK-064 N5：两侧统一本地时区——published_at 存 RFC3339（常带 offset），
        // SQLite 的 date() 对带 offset 值归一到 UTC，直接 date(a.published_at) 会
        // 与 date('now','localtime') 错位：非 UTC 时区用户本地凌晨（+08:00 的
        // 00:00-08:00）发布的文章不进「今天」视图。
        where_clauses.push("date(a.published_at, 'localtime') = date('now', 'localtime')");
    }
    if let Some(l) = &q.layout {
        // TASK-094：列表查询的布局维度与 mark_all_read / list_unread_ids_scoped
        // 同一段谓词（见 LAYOUT_FILTER_SQL，一处定义三处共用）。
        // 外层 FROM 只有 articles（别名 a），片段里的 feed_id 无歧义地解析到它；
        // f2/fo2 是子查询自己的别名，不受影响。M-5 起为标量形态，绑定一次。
        where_clauses.push(LAYOUT_FILTER_SQL);
        params.push(l.clone().into());
    }
    (where_clauses, params)
}

/// 计算某篇文章在当前筛选排序下的绝对位置（0 起）。
/// 用窗口函数 ROW_NUMBER() OVER (ORDER BY ...) - 1 求位置，供前端「搜索/深层
/// 打开文章后只加载目标那一页」的双向分页锚定——无需从头拉全量。
/// 排序与 list_articles 完全同口径（[`PUBLISHED_ORDER_DESC`] / [`PUBLISHED_ORDER_ASC`]，
/// M-5：纯 published_at + TASK-117 起共用的 a.id 决胜，与旧 COALESCE 口径逐行等价；
/// ROW_NUMBER 的窗口 ORDER 与列表 ORDER 共用同一常量，锚定位置与列表顺序天然同序，
/// 并列 published_at 由 id 决胜后窗口序确定）。
/// 注意：本函数不消费 [`ArticleQuery::last_published`] / [`ArticleQuery::last_id`]
/// （keyset 游标只作用于 [`list_articles`]）——锚定求的是**全筛选集**里的绝对位置。
pub fn article_index(
    conn: &Connection,
    q: &ArticleQuery,
    article_id: i64,
) -> AppResult<Option<i64>> {
    let (sql, params) = article_index_sql(q, article_id);
    let pos = conn
        .prepare(&sql)?
        .query_row(rusqlite::params_from_iter(params), |r| r.get::<_, i64>(0))
        .optional()?;
    Ok(pos)
}

/// 组装 article_index 的窗口 SQL + 绑定（生产字节；测试对同一产物跑
/// EXPLAIN QUERY PLAN，见 `list_query_plan_*`）。
fn article_index_sql(q: &ArticleQuery, article_id: i64) -> (String, Vec<rusqlite::types::Value>) {
    let (where_clauses, mut params) = article_where(q);
    let order = if q.newest_first {
        PUBLISHED_ORDER_DESC
    } else {
        PUBLISHED_ORDER_ASC
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
    (sql, params)
}

// Note: 可渲染正文/译文的净化在读取边界与写入路径同源（同一 sanitize()） — 见 .agents/notes/implemented/architecture/2026-10-08-可渲染正文安全边界.md
/// 读边界的可渲染字段净化（F07）：存量行可能含未清洗 HTML（历史回填绕过、
/// 历史 AI 产物、旧库迁移），详情与携带正文的列表行返回前统一过生产
/// [`crate::sanitize::sanitize`]；基址取该行 url（相对链接与写入口径一致地重写
/// 为绝对）。None/空串原样保留；只读元数据的列表不携带正文列，不经过本函数。
fn sanitize_renderable_fields(
    url: Option<&str>,
    content_html: &mut Option<String>,
    translated_content: &mut Option<String>,
) {
    if let Some(html) = content_html.as_deref() {
        if !html.is_empty() {
            *content_html = Some(crate::sanitize::sanitize(html, url));
        }
    }
    if let Some(tr) = translated_content.as_deref() {
        if !tr.is_empty() {
            *translated_content = Some(crate::sanitize::sanitize(tr, url));
        }
    }
}

pub fn get_article(conn: &Connection, id: i64) -> AppResult<Option<ArticleRow>> {
    let mut row = conn
        .query_row(
            &format!("SELECT {ARTICLE_COLS} FROM articles WHERE id = ?1"),
            params![id],
            article_row,
        )
        .optional()?;
    if let Some(r) = row.as_mut() {
        sanitize_renderable_fields(
            r.url.as_deref(),
            &mut r.content_html,
            &mut r.translated_content,
        );
    }
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
    let mut articles = rows.collect::<Result<Vec<_>, _>>()?;
    for a in &mut articles {
        sanitize_renderable_fields(
            a.url.as_deref(),
            &mut a.content_html,
            &mut a.translated_content,
        );
    }
    Ok(articles)
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
    // P3[8]（REQ-104）：LIMIT 此前用字符串插值 `LIMIT {limit}`。i64 本身无注入风险，
    // 但**负数在 SQLite 里表示「不限制」**，调用方一处笔误就会把整个库倒出来；
    // 且它不是绑定参数、不受参数检查保护。改为绑定参数并把上界收敛：
    // 非正数 → 回落安全默认（搜索场景没有「不限制」语义），并设上限防误用。
    let limit: i64 = if limit <= 0 { 100 } else { limit.min(1000) };
    let sql = format!(
        "SELECT a.id, a.feed_id, a.title, a.author,
                COALESCE(NULLIF(a.summary, ''), substr(a.body_text, 1, 280)) AS snippet,
                a.image_url, a.enclosure_url, a.enclosure_mime, a.duration_sec,
                a.ai_summary, a.source, a.published_at, a.is_read, a.is_starred,
                NULL, NULL, NULL, 0
         FROM articles a
         WHERE {}
         ORDER BY a.published_at DESC
         LIMIT ?{limit_idx}",
        where_parts.join(" AND "),
        limit_idx = like_args.len() + 1
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params_from_iter(
            like_args
                .iter()
                .map(|s| s as &dyn rusqlite::ToSql)
                .chain(std::iter::once(&limit as &dyn rusqlite::ToSql)),
        ),
        article_list_item,
    )?;
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
///
/// 独立调用入口：自开事务。需要与调用方其它写入同一短事务（账号切换/断开
/// 的「全有全无」）时改用 [`purge_remote_data_in`]，避免嵌套 BEGIN。
pub fn purge_remote_data(conn: &mut Connection) -> AppResult<(usize, usize)> {
    let tx = conn.transaction()?;
    let r = purge_remote_data_in(&tx)?;
    tx.commit()?;
    Ok(r)
}

/// [`purge_remote_data`] 的事务内核（OPT-014 R1）：在**调用方已有事务**内执行
/// 全部清理步骤，不自行 BEGIN/COMMIT——与调用方的其它写入同生共死。
/// SQL 语义与独立入口完全一致（同一份代码）。
pub(crate) fn purge_remote_data_in(tx: &rusqlite::Transaction<'_>) -> AppResult<(usize, usize)> {
    // P3[4]（REQ-104）：先记下「属于服务端的分类」，供第 5 步只删这些空目录。
    // 此前第 5 步的 SQL 是「删所有无成员的目录」，注释却写「Pull 建的」——
    // 于是**用户自建的空目录会被一起删掉**（用户手动建了目录、还没往里放订阅，
    // 断开一次连接就没了）。这里改用可判定的归属信号：
    //   ① 仍带着远端绑定的目录（folders.remote_id 非空，pull 建远端分类时写入）；
    //   ② 其成员订阅属于服务端的目录（origin='remote'，第 1 步会把这些订阅删掉）。
    // 两者都取不到的用户自建空目录一律保留。
    let remote_folder_ids: Vec<i64> = {
        let mut stmt = tx.prepare(
            "SELECT id FROM folders
             WHERE remote_id IS NOT NULL
                OR id IN (SELECT DISTINCT folder_id FROM feeds
                          WHERE folder_id IS NOT NULL AND origin = 'remote')",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
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
    // 5. 只删「服务端来源且已空」的目录：用户自建的空目录保留（P3[4]）。
    for id in remote_folder_ids {
        tx.execute(
            "DELETE FROM folders
             WHERE id = ?1
               AND id NOT IN (SELECT DISTINCT folder_id FROM feeds WHERE folder_id IS NOT NULL)",
            [id],
        )?;
    }
    Ok((feeds, articles))
}

/// 缓存清理：删除指定天数之前的文章（含 FTS/队列级联）与/或 AI 产物。
/// 保留项：收藏文章永不清（用户显式标过星）；**未读文章永不清**——删除后
/// 下一次全量同步会按服务器状态重新拉回（Miniflux 端仍是 unread），数据
/// 打架等于白删；scope='ai' 只清 AI 摘要与翻译缓存（正文保留，重新打开
/// 可再生成）。返回 (删文章数, 清 AI 字段数)。
pub fn cleanup_cache(conn: &mut Connection, days: i64, scope: &str) -> AppResult<(usize, usize)> {
    let tx = conn.transaction()?;
    // P3[5]（REQ-104）：cutoff 此前是 datetime('now','-N days','localtime')，而 published_at
    // 按 RFC3339 存（多为 UTC）。两者时区不一致 → 在 UTC+8 等时区 cutoff 被推后 8 小时，
    // **会把「只差一小会儿才到 N 天」的文章也删掉**（用户设 7 天，实际删掉 6 天 16 小时的）。
    // 改为：两侧都过 datetime() 归一到同一时基（datetime() 会把带 offset 的 RFC3339 转成
    // UTC 字符串），不再混入 localtime。
    // P3-3（自检 2026-09-29）：ai 分支对齐同一口径——此前 `published_at < cutoff` 是
    // 裸字符串比较，RFC3339 带 offset 的行（'T' > ' '）在 cutoff 当天漏清，边界差
    // 几小时。见 v15 迁移前混排格式的同款字符串比较问题（migrations.rs v15 注释）。
    let cutoff = format!("datetime('now', '-{days} days')");
    let (mut deleted, mut ai_cleared) = (0usize, 0usize);
    if scope == "articles" {
        deleted = tx.execute(
            &format!(
                "DELETE FROM articles
                 WHERE datetime(published_at) < {cutoff}
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
                   AND datetime(published_at) < {cutoff}"
            ),
            [],
        )?;
        // FTS 触发器同步（UPDATE 触发 articles_au 已处理）
    }
    tx.commit()?;
    Ok((deleted, ai_cleared))
}

/* ============================================================
写入与状态：文章入库（含智能去重）、已读/收藏、AI 字段、未读数
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
    // M-5：published_at 写入兜底——程序路径不允许产生 NULL（v14 触发器再兜住
    // 裸 SQL 路径），使「published_at 非空」不变量成立，列表排序可退化为纯
    // ORDER BY published_at（与旧 COALESCE 口径逐行等价，见 migrations v14 注释）。
    conn.execute(
        "INSERT INTO articles
            (feed_id, guid, url, url_norm, title, author, summary, content_html, body_text, image_url,
             enclosure_url, enclosure_mime, duration_sec, published_at, source)
         VALUES (?1, ?2, ?3, ?15, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, COALESCE(?13, datetime('now')), ?14)",
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

/* ============================================================
TASK-108：状态写入 + 同步入队 的复合事务（同生共死，REQ-002）
============================================================ */

// Note: 状态写入与同步入队必须同事务；批量路径全有全无 — 见 .agents/notes/implemented/bug-fix/2026-09-18-状态写入事务化与对账守卫.md
/// 已读状态「写入 + 入队」的事务内核（TASK-108）：调用方负责事务边界——
/// 单条入口 [`set_read_with_enqueue`] 与批量入口 [`set_read_bulk_with_enqueue`]
/// 各自开事务后调用本函数。语句顺序与参数同 commands 层既有逐条提交形态
/// 完全一致（先写状态后入队，action 名 `read`/`unread`），事务化只改变失败时
/// 的原子性（任一步失败整体回滚），成功路径逐列语义不变。
fn set_read_enqueue_in_tx(conn: &Connection, id: i64, read: bool) -> AppResult<()> {
    set_read(conn, id, read)?;
    enqueue_sync(
        conn,
        Some(id),
        None,
        if read { "read" } else { "unread" },
        None,
    )
}

/// 收藏版事务内核，同 [`set_read_enqueue_in_tx`]（action 名 `star`/`unstar`）。
fn set_starred_enqueue_in_tx(conn: &Connection, id: i64, starred: bool) -> AppResult<()> {
    set_starred(conn, id, starred)?;
    enqueue_sync(
        conn,
        Some(id),
        None,
        if starred { "star" } else { "unstar" },
        None,
    )
}

/// 单条已读状态 + 同步入队（TASK-108，REQ-002）：复合操作包进**单一事务**——
/// 此前 commands 层先 `set_read` 再 `enqueue_sync` 无外层事务，入队一步失败会
/// 留下「本地状态已改但没有待同步记录」的孤儿变更：离线期间该变更永不补推，
/// 且可能被远端对账覆盖（AUDIT-20261005-core-consistency 点名缺口）。
/// 事务口径与 [`mark_all_read_with_enqueue`] 一致：`unchecked_transaction`
/// （&Connection 可用，单连接 + 互斥锁的既有约束）+ 显式 commit；出错时
/// 事务对象 drop 自动回滚，「状态写入与队列项同生共死」由故障注入测试锁定。
pub fn set_read_with_enqueue(conn: &Connection, id: i64, read: bool) -> AppResult<()> {
    let tx = conn.unchecked_transaction()?;
    set_read_enqueue_in_tx(&tx, id, read)?;
    tx.commit()?;
    Ok(())
}

/// 单条收藏状态 + 同步入队，同 [`set_read_with_enqueue`]（TASK-108）。
pub fn set_starred_with_enqueue(conn: &Connection, id: i64, starred: bool) -> AppResult<()> {
    let tx = conn.unchecked_transaction()?;
    set_starred_enqueue_in_tx(&tx, id, starred)?;
    tx.commit()?;
    Ok(())
}

/// 批量已读 + 逐条入队（TASK-108）：整批**单事务、全有全无**——任一 id 的
/// 状态写入或入队失败，先前 id 已在事务内完成的写入与队项一并回滚，不存在
/// 部分成功（修前逐条自动提交，中途失败会留下半批已落库）。ids 逐条走
/// [`set_read_enqueue_in_tx`]（与 [`set_read_with_enqueue`] 同一内核语句），
/// 同文章正反方向的互斥合并语义与逐条入队一致；空 ids 不开事务直接返回。
pub fn set_read_bulk_with_enqueue(conn: &Connection, ids: &[i64], read: bool) -> AppResult<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let tx = conn.unchecked_transaction()?;
    for id in ids {
        set_read_enqueue_in_tx(&tx, *id, read)?;
    }
    tx.commit()?;
    Ok(())
}

/// 查询「无封面 + 有原文 URL + 直连来源」的文章 id（封面后台补全用）。
/// 摘要型 RSS（少数派等）不带 media 字段，封面只能从文章页 og:image 拿；
/// 这里只取直连源（source='direct'）的条目——Miniflux 源在入库时已用
/// 正文第一图兜底，无需再抓文章页。limit 限制单轮批处理量（避免启动时
/// 一次性扫全库 + 轰炸源站）。
///
/// offset 供调用方**推进候选窗口**（REQ-106①）：补全每轮只看最新 limit 条，
/// 若这些条目的文章页全部拿不到 og:image，更老的候选就永远轮不到。按
/// (limit, offset) 分页扫描可让调用方跳过本进程已尝试过的条目继续往后找。
pub fn articles_without_cover(
    conn: &Connection,
    limit: i64,
    offset: i64,
) -> AppResult<Vec<(i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT id, url FROM articles
         WHERE (image_url IS NULL OR image_url = '')
           AND url IS NOT NULL AND url != ''
           AND source = 'direct'
         ORDER BY published_at DESC
         LIMIT ?1 OFFSET ?2",
    )?;
    let rows = stmt.query_map(params![limit, offset], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 清除一个已失效的直连封面，只有上报 URL 仍与库中当前值完全一致时才生效。
///
/// source='direct' 是必要边界：Miniflux 文章不进入本地封面补全队列，不能因为
/// 图片加载失败被清成空值后永久失去封面。
pub fn clear_article_cover_if_matches(
    conn: &Connection,
    article_id: i64,
    url: &str,
) -> AppResult<usize> {
    Ok(conn.execute(
        "UPDATE articles
         SET image_url = NULL
         WHERE id = ?1
           AND image_url = ?2
           AND image_url IS NOT NULL
           AND image_url != ''
           AND source = 'direct'",
        params![article_id, url],
    )?)
}

/// 布局过滤谓词（TASK-094 收口：**一处定义，三处共用**）：feed 级 layout 覆盖 →
/// 分类 layout 兜底，与前端 resolveFeedLayout 等价。列表查询（article_where →
/// list_articles / article_index）、[`mark_all_read`] 与 `list_unread_ids_scoped`
/// （db/sync_map.rs）三处拼接同一段 SQL，杜绝第四份拷贝。
///
/// M-5：谓词用标量子查询形态（原 `feed_id IN (SELECT ...)` 的等价改写）——
/// IN 子查询会驱动规划器先扫 feeds 再按源取行，排序回退 TEMP B-TREE；
/// 标量形态可由 idx_articles_published 有序驱动（实测见
/// tmp/task-099/explain-query-plan.md）。语义逐行等价：feed 的有效布局
/// （自身覆盖，否则所属分类）等于目标布局 ⇔ 原 IN 判定；无分类的 feed
/// 两侧都不命中（原 JOIN 为内连接）。
/// 常量本体不含前导 ` AND `（article_where 走子句 join；其余两处拼接时自带）；
/// 列名 `feed_id` 不限定表名——三处消费方的目标表都只有 articles（同旧写法）。
/// 占位符 ? 绑定一次（布局值）。
pub(crate) const LAYOUT_FILTER_SQL: &str = "(SELECT CASE WHEN f2.layout != 'inherit' THEN f2.layout ELSE fo2.layout END FROM feeds f2 JOIN folders fo2 ON f2.folder_id = fo2.id WHERE f2.id = feed_id) = ?";

/// 分类过滤谓词（M-5）：标量子查询形态（原 `feed_id IN (SELECT id FROM feeds
/// WHERE folder_id = ?)` 的等价改写），理由同 [`LAYOUT_FILTER_SQL`]——使列表
/// 查询可由 idx_articles_published 有序驱动。语义逐行等价：feed 的归属分类
/// 等于目标分类 ⇔ 其 id 在原子查询集合中；无分类（NULL）两侧都不命中。
pub(crate) const FOLDER_FILTER_SQL: &str =
    "(SELECT folder_id FROM feeds f2 WHERE f2.id = feed_id) = ?";

/// 范围过滤子句 + 绑定（feed → folder → layout → 收藏 → 时间），
/// [`mark_all_read`]、[`mark_all_read_with_enqueue`] 与 `list_unread_ids_scoped`
/// （db/sync_map.rs）三处共用，保证「实际标读集合」与「入队集合」永远同口径（F8）。
/// 返回的子句自带前导 ` AND `，占位符按序对应返回的绑定。
pub(crate) fn scope_clause(
    feed_id: Option<i64>,
    folder_id: Option<i64>,
    starred_only: bool,
    since_ms: Option<i64>,
    layout: Option<&str>,
) -> (String, Vec<Box<dyn rusqlite::ToSql>>) {
    let mut sql = String::new();
    let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(fid) = feed_id {
        sql.push_str(" AND feed_id = ?");
        binds.push(Box::new(fid));
    }
    if let Some(folder) = folder_id {
        sql.push_str(" AND ");
        sql.push_str(FOLDER_FILTER_SQL);
        binds.push(Box::new(folder));
    }
    if let Some(l) = layout {
        // 布局谓词与列表查询同源（LAYOUT_FILTER_SQL，TASK-094 收口）
        sql.push_str(" AND ");
        sql.push_str(LAYOUT_FILTER_SQL);
        binds.push(Box::new(l.to_string()));
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
    (sql, binds)
}

/// 全部已读：作用于当前筛选范围（feed/folder/all），与前端「全部已读」按钮语义一致
pub fn mark_all_read(
    conn: &Connection,
    feed_id: Option<i64>,
    folder_id: Option<i64>,
    starred_only: bool,
    since_ms: Option<i64>,
    layout: Option<&str>,
) -> AppResult<usize> {
    let (clause, binds) = scope_clause(feed_id, folder_id, starred_only, since_ms, layout);
    let sql = format!("UPDATE articles SET is_read = 1 WHERE is_read = 0{clause}");
    let refs: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
    let n = conn.execute(&sql, refs.as_slice())?;
    Ok(n)
}

/// 「全部已读」集合化（REQ-108 M-9）：一个事务内完成 入队 + 标读，语句数常数化。
///
/// 旧实现 = 1 SELECT（list_unread_ids_scoped 收集未读 id）+ 1 UPDATE + 每条 id
/// 一次 enqueue_sync（DELETE + INSERT）= 2N+2 条语句，且命令全程持库锁。
/// 这里固定 3 条语句：
///   ① 批量删除范围内条目的 read/unread 旧队项（与逐条 enqueue 的「同向互斥
///      覆盖」语义一致：每个 id 至多保留一条最新 read 队项）；
///   ② `INSERT ... SELECT` 为范围内全部未读条目入 `read` 队项（feed_url/payload
///      为 NULL，与 `enqueue_sync(conn, Some(id), None, "read", None)` 逐列相同）；
///   ③ 单条 UPDATE 标读，返回受影响行数。
/// ①② 在 ③ 之前求值 `is_read = 0`，入队集合与旧实现「标读前收集」的集合逐行
/// 相同（F8，由等价性测试锁定）；三条语句同事务——旧实现逐条提交，中途失败会
/// 留下半批已标读，事务化只会更保守（不多不少）。
/// created_at 用语句级 `datetime('now')`（旧逐条可能跨秒；老化/补推只做时间序
/// 比较，语义不受影响）。语句数不随 N 增长的断言见 auth_probe 探针测试。
pub fn mark_all_read_with_enqueue(
    conn: &Connection,
    feed_id: Option<i64>,
    folder_id: Option<i64>,
    starred_only: bool,
    since_ms: Option<i64>,
    layout: Option<&str>,
) -> AppResult<usize> {
    let (clause, binds) = scope_clause(feed_id, folder_id, starred_only, since_ms, layout);
    let refs: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
    let tx = conn.unchecked_transaction()?;
    let del = format!(
        "DELETE FROM sync_queue WHERE action IN ('read', 'unread')
           AND article_id IN (SELECT id FROM articles WHERE is_read = 0{clause})"
    );
    tx.execute(&del, refs.as_slice())?;
    let ins = format!(
        "INSERT INTO sync_queue (article_id, feed_url, action, payload, created_at)
         SELECT id, NULL, 'read', NULL, datetime('now')
           FROM articles WHERE is_read = 0{clause}"
    );
    tx.execute(&ins, refs.as_slice())?;
    let upd = format!("UPDATE articles SET is_read = 1 WHERE is_read = 0{clause}");
    let n = tx.execute(&upd, refs.as_slice())?;
    tx.commit()?;
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
    // TASK-064 N5：today 判定与 only_today 同口径（本地时区两侧对齐），理由见
    // list_articles 的 where 构建处。
    let mut stmt = conn.prepare(
        "SELECT feed_id, COUNT(*), SUM(is_read = 0), SUM(is_starred = 1),
                SUM(date(published_at, 'localtime') = date('now', 'localtime'))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{create_folder, insert_feed, upsert_article_with_feed, MIGRATIONS};
    use chrono::{Local, TimeZone};
    use rusqlite::Connection;

    fn conn() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        conn
    }

    fn na(guid: &str, published_at: Option<String>) -> NewArticle {
        NewArticle {
            guid: guid.into(),
            url: Some(format!("https://e.example/{guid}")),
            title: "t".into(),
            author: None,
            summary: None,
            content_html: None,
            body_text: "b".into(),
            image_url: None,
            enclosure_url: None,
            enclosure_mime: None,
            duration_sec: None,
            published_at,
            source: "direct".into(),
        }
    }

    /// N5：「今天」判定两侧时区必须一致。published_at 存 RFC3339（带 offset），
    /// SQLite 的 date() 对带 offset 值归一到 UTC——修前 date(a.published_at)
    /// （UTC 日期）对比 date('now','localtime')（本地日期），非 UTC 时区的本地
    /// 凌晨文章（+08:00 的 00:00-08:00 → UTC 前一日）不进「今天」。
    /// 用本地今天 01:00 构造确定性形态；判定力依赖主机时区非 UTC（本项目环境
    /// +08:00；UTC 主机上修前同样命中——不误报，只是失去判别力）。
    #[test]
    fn early_morning_local_article_counts_as_today() {
        let conn = conn();
        let fid = create_folder(&conn, "F", "article").unwrap();
        let feed = insert_feed(
            &conn,
            "https://f.example/rss",
            None,
            "f",
            None,
            fid,
            "inherit",
            true,
            false,
        )
        .unwrap();

        let today = Local::now().date_naive();
        let one_am_local = Local
            .from_local_datetime(&today.and_hms_opt(1, 0, 0).unwrap())
            .single()
            .expect("01:00 local exists");
        let yesterday_1am = Local
            .from_local_datetime(
                &(today - chrono::Duration::days(1))
                    .and_hms_opt(1, 0, 0)
                    .unwrap(),
            )
            .single()
            .expect("yesterday 01:00 local exists");

        upsert_article_with_feed(
            &conn,
            feed,
            &na("today-1am", Some(one_am_local.to_rfc3339())),
            false,
        )
        .unwrap();
        upsert_article_with_feed(
            &conn,
            feed,
            &na("yesterday-1am", Some(yesterday_1am.to_rfc3339())),
            false,
        )
        .unwrap();

        let counts = feed_counts(&conn).unwrap();
        let today_count = counts
            .iter()
            .find(|c| c.feed_id == feed)
            .map(|c| c.today)
            .unwrap_or(0);
        assert_eq!(
            today_count, 1,
            "本地今天 01:00 的文章必须计入 today（N5 修前为 0）"
        );

        let q = ArticleQuery {
            feed_id: Some(feed),
            folder_id: None,
            only_unread: false,
            only_starred: false,
            only_today: true,
            newest_first: true,
            limit: 100,
            offset: 0,
            with_content: false,
            layout: None,
            last_published: None,
            last_id: None,
        };
        let ids: Vec<i64> = list_articles(&conn, &q)
            .unwrap()
            .into_iter()
            .map(|a| a.id)
            .collect();
        assert_eq!(
            ids.len(),
            1,
            "only_today 必须只含本地今天 01:00 的文章（N5 修前为 0，昨天的不计入）"
        );
    }

    /* ---------- TASK-094（REQ-107）：列表查询的布局维度 ---------- */

    /// 三布局夹具：
    ///   · folder_article(layout=article)：feed_override(layout=image，feed 级覆盖)
    ///     与 feed_inherit(layout=inherit → 兜底 article)；
    ///   · folder_podcast(layout=podcast)：feed_pod(layout=inherit → 兜底 podcast)。
    /// 返回 (conn, feed_override, feed_inherit, feed_pod, 覆盖源文章, 继承源文章, 播客源文章)。
    fn seed_layout_fixture() -> (Connection, i64, i64, i64, Vec<i64>, Vec<i64>, Vec<i64>) {
        let conn = conn();
        let folder_article = create_folder(&conn, "图文", "article").unwrap();
        let folder_podcast = create_folder(&conn, "播客", "podcast").unwrap();
        let feed_override = insert_feed(
            &conn,
            "https://ov.example/rss",
            None,
            "覆盖源",
            None,
            folder_article,
            "image",
            false,
            false,
        )
        .unwrap();
        let feed_inherit = insert_feed(
            &conn,
            "https://inh.example/rss",
            None,
            "继承源",
            None,
            folder_article,
            "inherit",
            false,
            false,
        )
        .unwrap();
        let feed_pod = insert_feed(
            &conn,
            "https://pod.example/rss",
            None,
            "播客源",
            None,
            folder_podcast,
            "inherit",
            false,
            false,
        )
        .unwrap();
        let plant = |feed: i64, tag: &str, n: usize| -> Vec<i64> {
            (0..n)
                .map(|i| {
                    let a = na(
                        &format!("{tag}{i}"),
                        Some(format!("2026-01-01T00:00:{:02}Z", i)),
                    );
                    upsert_article_with_feed(&conn, feed, &a, false).unwrap().0
                })
                .collect()
        };
        let ids_override = plant(feed_override, "ov", 3);
        let ids_inherit = plant(feed_inherit, "inh", 2);
        let ids_pod = plant(feed_pod, "pod", 4);
        (
            conn,
            feed_override,
            feed_inherit,
            feed_pod,
            ids_override,
            ids_inherit,
            ids_pod,
        )
    }

    fn layout_q(layout: Option<&str>) -> ArticleQuery {
        ArticleQuery {
            feed_id: None,
            folder_id: None,
            only_unread: false,
            only_starred: false,
            only_today: false,
            newest_first: true,
            limit: 500,
            offset: 0,
            with_content: false,
            layout: layout.map(str::to_string),
            last_published: None,
            last_id: None,
        }
    }

    fn listed_ids(conn: &Connection, q: &ArticleQuery) -> Vec<i64> {
        list_articles(conn, q)
            .unwrap()
            .into_iter()
            .map(|a| a.id)
            .collect()
    }

    fn sorted(mut v: Vec<i64>) -> Vec<i64> {
        v.sort();
        v
    }

    /// 情形一：feed 级 layout 覆盖优先——layout=image 只含覆盖源；
    /// 该源在 layout=article 的查询里被排除（不被分类布局吞并）。
    #[test]
    fn list_articles_layout_feed_override() {
        let (conn, _fo, _fi, _fp, ids_override, ids_inherit, _ip) = seed_layout_fixture();
        assert_eq!(
            sorted(listed_ids(&conn, &layout_q(Some("image")))),
            sorted(ids_override.clone()),
            "layout=image 必须恰好等于 feed 级覆盖为 image 的源（修前：查询无布局维度，返回全部）"
        );
        let article_ids = listed_ids(&conn, &layout_q(Some("article")));
        assert_eq!(
            sorted(article_ids.clone()),
            sorted(ids_inherit),
            "layout=article 只含分类兜底为 article 的源"
        );
        assert!(
            !article_ids.iter().any(|id| ids_override.contains(id)),
            "feed 级覆盖必须优先于分类布局：覆盖源不得混进 article 列表"
        );
    }

    /// 情形二：分类兜底——inherit 源跟随分类 layout；feed 级覆盖不被兜底吞并。
    #[test]
    fn list_articles_layout_folder_fallback() {
        let (conn, feed_override, _fi, _fp, _io, _ii, ids_pod) = seed_layout_fixture();
        assert_eq!(
            sorted(listed_ids(&conn, &layout_q(Some("podcast")))),
            sorted(ids_pod),
            "layout=podcast 必须兜底命中 inherit 源所在分类的 podcast 布局"
        );
        // feed 级覆盖 + 布局过滤可组合：覆盖源（image）在 layout=podcast 查询下为空集
        let mut q = layout_q(Some("podcast"));
        q.feed_id = Some(feed_override);
        assert!(
            listed_ids(&conn, &q).is_empty(),
            "覆盖源（image）在 layout=podcast 查询下必须为空（feed 级覆盖不被分类兜底覆盖）"
        );
    }

    /// 情形三：layout=None 行为逐字不变——不做任何布局过滤。
    #[test]
    fn list_articles_layout_none_returns_all() {
        let (conn, _fo, _fi, _fp, ids_override, ids_inherit, ids_pod) = seed_layout_fixture();
        let mut want = ids_override;
        want.extend(&ids_inherit);
        want.extend(&ids_pod);
        assert_eq!(
            sorted(listed_ids(&conn, &layout_q(None))),
            sorted(want),
            "layout=None 必须返回全部布局的条目（既有行为不变）"
        );
        // None 时查询不带布局子查询：对照「某布局过滤」结果数 < None 结果数
        let image_count = listed_ids(&conn, &layout_q(Some("image"))).len();
        let all_count = listed_ids(&conn, &layout_q(None)).len();
        assert!(
            image_count < all_count,
            "夹具应跨布局（image {image_count} < 全部 {all_count}），否则本用例无判别力"
        );
    }

    /// 情形四：article_index 与按布局过滤后的 list_articles 位置对齐——
    /// 「绝对位置」必须与「该布局列表顺序」同口径，否则锚定分页会打开错文章。
    #[test]
    fn article_index_aligns_with_layout_filtered_list() {
        let (conn, _fo, _fi, feed_pod, _io, _ii, _ip) = seed_layout_fixture();
        let q = layout_q(Some("image"));
        let all = list_articles(&conn, &q).unwrap();
        assert_eq!(all.len(), 3, "夹具：image 布局 3 篇");
        for (pos, row) in all.iter().enumerate() {
            assert_eq!(
                article_index(&conn, &q, row.id).unwrap(),
                Some(pos as i64),
                "layout 过滤下文章 {} 的绝对位置必须等于列表位置 {}",
                row.id,
                pos
            );
        }
        // 不属于该布局的文章：过滤后的序列里没有它的位置
        let pod_article: i64 = conn
            .query_row(
                "SELECT id FROM articles WHERE feed_id = ?1 LIMIT 1",
                [feed_pod],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            article_index(&conn, &q, pod_article).unwrap(),
            None,
            "布局外文章在过滤后的序列中无位置（修前会返回全局位置，错配列表）"
        );
        // 从位置取页：offset=1 的首条必须是位置 1 的文章（锚定分页的前提）
        let target = all[1].id;
        let page = list_articles(
            &conn,
            &ArticleQuery {
                offset: 1,
                ..q.clone()
            },
        )
        .unwrap();
        assert_eq!(page[0].id, target);
    }
    /* ---------- P3[4]：purge_remote_data 必须保住用户自建的空目录 ---------- */

    /// 用户自建空目录（无订阅、无远端绑定）在断开连接后必须保留。
    /// 修前 SQL 是「删所有无成员目录」，该目录会被一起删掉。
    #[test]
    fn purge_remote_data_keeps_user_created_empty_folder() {
        let mut conn = conn();
        let user_folder = create_folder(&conn, "我的空分类", "article").unwrap();

        let (feeds, _) = purge_remote_data(&mut conn).unwrap();

        assert_eq!(feeds, 0, "没有服务端订阅可删");
        let left: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM folders WHERE id = ?1",
                [user_folder],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(left, 1, "用户自建的空目录必须保留（P3[4] 修前会被误删）");
    }

    /// 服务端来源的空目录（其成员是 origin='remote' 订阅）仍应被清掉。
    #[test]
    fn purge_remote_data_removes_emptied_remote_folder() {
        let mut conn = conn();
        let remote_folder = create_folder(&conn, "远端分类", "article").unwrap();
        insert_feed_origin(
            &conn,
            "https://r.example/feed",
            None,
            "R",
            None,
            remote_folder,
            "inherit",
            false,
            false,
            "remote",
        )
        .unwrap();

        let (feeds, _) = purge_remote_data(&mut conn).unwrap();

        assert_eq!(feeds, 1, "服务端订阅被删");
        let left: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM folders WHERE id = ?1",
                [remote_folder],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(left, 0, "清空后的服务端分类应删除");
    }

    /// 用户自建但有订阅的目录当然也要保留（回归保护，避免修 4 时误伤）。
    #[test]
    fn purge_remote_data_keeps_user_folder_with_local_feed() {
        let mut conn = conn();
        let folder = create_folder(&conn, "本地分类", "article").unwrap();
        insert_feed(
            &conn,
            "https://l.example/feed",
            None,
            "L",
            None,
            folder,
            "inherit",
            false,
            false,
        )
        .unwrap();

        purge_remote_data(&mut conn).unwrap();

        let left: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM folders WHERE id = ?1",
                [folder],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(left, 1, "本地订阅所属目录必须保留");
    }

    /* ---------- P3[5]：cleanup_cache 的 cutoff 不得因时区混用而多删 ---------- */

    /// cutoff 必须与 published_at 处于同一时基（P3[5]）。**实测语义**（非推测）：
    ///
    /// · published_at 由 `to_rfc3339()` 产生，形如 `2026-09-14T18:45:20+12:00`（含 `T` 与偏移）；
    /// · 修前 cutoff = `datetime('now','-N days','localtime')`，形如 `2026-09-14 16:45:20`
    ///   （含空格、本地墙上时间）；
    /// · 两者做**裸文本比较**：第 11 个字符处 `'T'(0x54) > ' '(0x20)`，故当**日期部分相同**时
    /// OPT-014 R1：事务内核 `purge_remote_data_in` 不自行提交——在调用方事务里
    /// 执行后回滚，一切原样（证明可与账号提交同事务，且无嵌套 BEGIN）。
    #[test]
    fn purge_kernel_rolls_back_with_caller_transaction() {
        let mut conn = conn();
        let folder = create_folder(&conn, "远端分类", "article").unwrap();
        insert_feed_origin(
            &conn,
            "https://r.example/feed",
            None,
            "R",
            None,
            folder,
            "inherit",
            false,
            false,
            "remote",
        )
        .unwrap();

        {
            let tx = conn.transaction().unwrap();
            let (feeds, _) = purge_remote_data_in(&tx).unwrap();
            assert_eq!(feeds, 1, "内核在事务内可见清理结果");
            // 不 commit：模拟调用方后续步骤失败（如凭据加密 Err）
        }

        let left: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM feeds WHERE origin = 'remote'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(left, 1, "调用方回滚后服务端订阅必须原样");
    }

    ///   文章恒被判定为「不够旧」；加上 localtime 把阈值整体挪动，实际判定退化为按**日期**
    ///   粗比、且随本机时区漂移。
    ///
    /// 构造（跨时区稳定）：文章真实瞬时 = now-7d-2h（**确实超过 7 天，应当被删**），
    /// 但以 `+12:00` 存储 → 其墙上日期 ≥ 阈值日期 → 修前**漏删**（该清的文章留在库里）；
    /// 修后经 `datetime()` 归一为真实瞬时 → 正确删除。
    #[test]
    fn cleanup_cache_cutoff_is_timezone_normalised() {
        let mut conn = conn();
        let folder = create_folder(&conn, "F", "article").unwrap();
        let feed = insert_feed(
            &conn,
            "https://c.example/feed",
            None,
            "C",
            None,
            folder,
            "inherit",
            false,
            false,
        )
        .unwrap();

        // 真实瞬时：7 天 2 小时前（确实超过 7 天阈值）
        let inst = chrono::Utc::now() - chrono::Duration::hours(7 * 24 + 2);
        // 以 +12:00 偏移存储：墙上时间比 UTC 快 12 小时，日期部分因此不早于阈值日期
        let offset = chrono::FixedOffset::east_opt(12 * 3600).unwrap();
        let stored = inst
            .with_timezone(&offset)
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
        assert!(
            stored.ends_with("+12:00"),
            "本用例须用 +12:00 偏移，实际: {stored}"
        );

        let (aid, _) =
            upsert_article_with_feed(&conn, feed, &na("tz-edge", Some(stored)), false).unwrap();
        set_read(&conn, aid, true).unwrap();

        let (deleted, _) = cleanup_cache(&mut conn, 7, "articles").unwrap();

        assert_eq!(
            deleted, 1,
            "真实瞬时已超 7 天的文章必须被清理；修前因 cutoff 时区混用 + 文本比较会漏删（P3[5]）"
        );
        let left: i64 = conn
            .query_row("SELECT COUNT(*) FROM articles WHERE id = ?1", [aid], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(left, 0, "文章应已被清理");
    }

    /// 明显年轻于阈值的文章（本机时区无关）绝不能被删——基本防误删回归。
    #[test]
    fn cleanup_cache_does_not_delete_recent_articles() {
        let mut conn = conn();
        let folder = create_folder(&conn, "F", "article").unwrap();
        let feed = insert_feed(
            &conn,
            "https://c.example/feed",
            None,
            "C",
            None,
            folder,
            "inherit",
            false,
            false,
        )
        .unwrap();

        let recent = chrono::Utc::now() - chrono::Duration::days(1);
        let (aid, _) =
            upsert_article_with_feed(&conn, feed, &na("recent", Some(recent.to_rfc3339())), false)
                .unwrap();
        set_read(&conn, aid, true).unwrap();

        let (deleted, _) = cleanup_cache(&mut conn, 7, "articles").unwrap();

        assert_eq!(deleted, 0, "1 天前的文章绝不能被 7 天阈值删除");
    }

    /// 真正超过 N 天的已读文章仍应被删（确认修复没有把功能关掉）。
    #[test]
    fn cleanup_cache_still_deletes_articles_older_than_cutoff() {
        let mut conn = conn();
        let folder = create_folder(&conn, "F", "article").unwrap();
        let feed = insert_feed(
            &conn,
            "https://c.example/feed",
            None,
            "C",
            None,
            folder,
            "inherit",
            false,
            false,
        )
        .unwrap();

        let old = chrono::Utc::now() - chrono::Duration::days(9);
        let (aid, _) =
            upsert_article_with_feed(&conn, feed, &na("old", Some(old.to_rfc3339())), false)
                .unwrap();
        set_read(&conn, aid, true).unwrap();

        let (deleted, _) = cleanup_cache(&mut conn, 7, "articles").unwrap();

        assert_eq!(deleted, 1, "9 天前的已读未收藏文章应被清理");
    }

    /* ---------- P3[8]：search_articles 的 LIMIT 必须是绑定参数且有上界 ---------- */

    /// 造一篇标题含 needle 的文章（search_articles 会 LIKE title/body/summary/ai/translated）。
    fn na_titled(guid: &str, title: &str) -> NewArticle {
        let mut a = na(guid, None);
        a.title = title.to_string();
        a
    }

    /// 负数 LIMIT 在 SQLite 里意为「不限制」，此前会直接插值进 SQL。
    /// 修后应回落安全默认值，且**不得报错**。
    #[test]
    fn search_articles_negative_limit_is_clamped() {
        let conn = conn();
        let folder = create_folder(&conn, "F", "article").unwrap();
        let feed = insert_feed(
            &conn,
            "https://s.example/feed",
            None,
            "S",
            None,
            folder,
            "inherit",
            false,
            false,
        )
        .unwrap();
        for i in 0..5 {
            upsert_article_with_feed(&conn, feed, &na_titled(&format!("hit-{i}"), "hit"), false)
                .unwrap();
        }
        let all = search_articles(&conn, "hit", -1).unwrap();
        assert_eq!(
            all.len(),
            5,
            "负数 LIMIT 应回落默认上限（5 条命中全部返回，且不报错）"
        );
    }

    /// 正数 LIMIT 必须真正生效（绑定参数后仍限定行数）。
    #[test]
    fn search_articles_respects_limit() {
        let conn = conn();
        let folder = create_folder(&conn, "F", "article").unwrap();
        let feed = insert_feed(
            &conn,
            "https://s.example/feed",
            None,
            "S",
            None,
            folder,
            "inherit",
            false,
            false,
        )
        .unwrap();
        for i in 0..5 {
            upsert_article_with_feed(&conn, feed, &na_titled(&format!("hit-{i}"), "hit"), false)
                .unwrap();
        }
        let limited = search_articles(&conn, "hit", 2).unwrap();
        assert_eq!(
            limited.len(),
            2,
            "LIMIT 2 必须只返回 2 条（绑定参数后仍生效）"
        );
    }

    /* ============================================================
    REQ-108（M-5 / M-9）：计划断言与集合化等价性
    ============================================================ */

    /// 大规模夹具：3 分类 × 10 源 × 300 条 = 3000 篇，published_at 递增无并列；
    /// 未读/收藏混合。直接 SQL 批量插入（upsert 逐条太慢）。
    fn seed_plan_fixture() -> Connection {
        let conn = conn();
        let tx = conn.unchecked_transaction().unwrap();
        for f in 1..=3i64 {
            tx.execute(
                "INSERT INTO folders (name, layout) VALUES (?1, 'article')",
                params![format!("分类{f}")],
            )
            .unwrap();
        }
        for fid in 1..=10i64 {
            tx.execute(
                "INSERT INTO feeds (feed_url, title, folder_id) VALUES (?1, ?2, ?3)",
                params![
                    format!("https://f{fid}.example/rss"),
                    format!("源{fid}"),
                    (fid - 1) % 3 + 1
                ],
            )
            .unwrap();
        }
        for fid in 1..=10i64 {
            for i in 0..300i64 {
                tx.execute(
                    "INSERT INTO articles (feed_id, guid, title, published_at, is_read, is_starred)
                     VALUES (?1, ?2, 't', datetime(?3, 'unixepoch'), ?4, ?5)",
                    params![
                        fid,
                        format!("g-{fid}-{i}"),
                        1_800_000_000 - i * 60 - fid,
                        (i % 3 != 0) as i64,
                        (i % 7 == 0) as i64,
                    ],
                )
                .unwrap();
            }
        }
        tx.commit().unwrap();
        conn
    }

    fn explain(conn: &Connection, sql: &str, p: &[rusqlite::types::Value]) -> AppResult<String> {
        let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        let rows = stmt.query_map(rusqlite::params_from_iter(p.iter()), |r| {
            r.get::<_, String>(3)
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?.join(" | "))
    }

    #[allow(clippy::too_many_arguments)]
    fn q(
        feed_id: Option<i64>,
        folder_id: Option<i64>,
        only_unread: bool,
        layout: Option<&str>,
    ) -> ArticleQuery {
        ArticleQuery {
            feed_id,
            folder_id,
            only_unread,
            only_starred: false,
            only_today: false,
            newest_first: true,
            limit: 500,
            offset: 0,
            with_content: false,
            layout: layout.map(|s| s.to_string()),
            last_published: None,
            last_id: None,
        }
    }

    /// M-5 验收①：列表查询（生产 SQL 字节）在「全部 / 按源 / 按分类」三个验收
    /// 变体（外加未读视图与布局过滤两个真实主路径）上必须走索引且无
    /// TEMP B-TREE 排序；article_index 的窗口排序同口径。
    #[test]
    fn list_query_plan_is_index_ordered_without_temp_btree() {
        let conn = seed_plan_fixture();
        let cases: Vec<(&str, ArticleQuery)> = vec![
            ("全部", q(None, None, false, None)),
            ("按源", q(Some(1), None, false, None)),
            ("按分类", q(None, Some(1), false, None)),
            ("未读", q(None, None, true, None)),
            ("布局", q(None, None, false, Some("image"))),
        ];
        for (name, query) in &cases {
            let (sql, p) = list_articles_sql(query);
            let plan = explain(&conn, &sql, &p).unwrap();
            assert!(
                !plan.contains("USE TEMP B-TREE"),
                "{name} 仍有 TEMP B-TREE 排序：\n{plan}"
            );
            assert!(
                plan.contains("USING INDEX"),
                "{name} 未走索引（SCAN 不带索引）：\n{plan}"
            );
        }
        let (sql, p) = article_index_sql(&q(None, None, false, None), 1);
        let plan = explain(&conn, &sql, &p).unwrap();
        assert!(
            !plan.contains("USE TEMP B-TREE"),
            "article_index 窗口仍有 TEMP B-TREE：\n{plan}"
        );
    }

    /// 等价性夹具：2 分类 × 4 源 × 12 条，布局覆盖/继承混合（含无分类源），
    /// 未读/收藏混合，并预置既有 sync_queue 行（read/unread/star/add_feed），
    /// 覆盖「同向互斥覆盖」「范围外队项保留」「add_feed 不受影响」三个分支。
    fn seed_equiv_fixture() -> Connection {
        let conn = conn();
        let tx = conn.unchecked_transaction().unwrap();
        tx.execute(
            "INSERT INTO folders (name, layout) VALUES ('F0', 'image')",
            [],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO folders (name, layout) VALUES ('F1', 'inherit')",
            [],
        )
        .unwrap();
        // f1：F0+继承（有效布局 image）；f2：F0+podcast 覆盖；f3：F1+继承（article）；
        // f4：无分类+gallery（分类/布局过滤都不命中）
        let feeds: [(&str, i64, &str); 4] = [
            ("https://f1.example/rss", 1, "inherit"),
            ("https://f2.example/rss", 1, "podcast"),
            ("https://f3.example/rss", 2, "inherit"),
            ("https://f4.example/rss", 0, "gallery"),
        ];
        for (idx, (url, folder, layout)) in feeds.iter().enumerate() {
            tx.execute(
                "INSERT INTO feeds (feed_url, title, folder_id, layout) VALUES (?1, ?2, ?3, ?4)",
                params![
                    url,
                    format!("源{}", idx + 1),
                    if *folder == 0 {
                        Option::<i64>::None
                    } else {
                        Some(*folder)
                    },
                    layout
                ],
            )
            .unwrap();
        }
        for fid in 1..=4i64 {
            for i in 0..12i64 {
                tx.execute(
                    "INSERT INTO articles (feed_id, guid, title, published_at, is_read, is_starred)
                     VALUES (?1, ?2, 't', datetime(?3, 'unixepoch'), ?4, ?5)",
                    params![
                        fid,
                        format!("g-{fid}-{i}"),
                        1_800_000_000 - i * 3600 - fid,
                        (i % 3 != 0) as i64,
                        (i % 5 == 0) as i64,
                    ],
                )
                .unwrap();
            }
        }
        // 预置队列：范围内外的 read/unread/star 与 add_feed
        tx.execute_batch(
            "INSERT INTO sync_queue (article_id, action) VALUES (1, 'unread'), (2, 'read'), (3, 'star'),
                (20, 'read'), (40, 'unread');
             INSERT INTO sync_queue (feed_url, action, payload)
                VALUES ('https://f3.example/rss', 'add_feed', '{\"folder_id\":2}');",
        )
        .unwrap();
        tx.commit().unwrap();
        conn
    }

    /// 等价性快照：(id, is_read) 列表 + 队列行集（忽略 created_at）+ feed 计数。
    type EquivSnapshot = (
        Vec<(i64, i64)>,
        Vec<(i64, String)>,
        Vec<(i64, i64, i64, i64, i64)>,
    );
    /// 等价性范围用例：(名称, feed_id, folder_id, starred_only, since_ms, layout)。
    type ScopeCase = (
        &'static str,
        Option<i64>,
        Option<i64>,
        bool,
        Option<i64>,
        Option<&'static str>,
    );

    fn equiv_snapshot(conn: &Connection) -> EquivSnapshot {
        let mut reads: Vec<(i64, i64)> = {
            let mut stmt = conn
                .prepare("SELECT id, is_read FROM articles ORDER BY id")
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        reads.sort();
        let mut queue: Vec<(i64, String)> = {
            let mut stmt = conn
                .prepare("SELECT article_id, action FROM sync_queue WHERE article_id IS NOT NULL ORDER BY article_id, action")
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        queue.sort();
        let counts: Vec<(i64, i64, i64, i64, i64)> = feed_counts(conn)
            .unwrap()
            .into_iter()
            .map(|c| (c.feed_id, c.total, c.unread, c.starred, c.today))
            .collect();
        (reads, queue, counts)
    }

    /// 参考旧实现（apply_mark_all_read 修前形态，逐字）：标读前收集 id →
    /// 单条 UPDATE → 逐 id enqueue_sync。本测试用它作等价性基准（修前语义）。
    fn reference_mark_all_read(
        conn: &Connection,
        feed_id: Option<i64>,
        folder_id: Option<i64>,
        starred_only: bool,
        since_ms: Option<i64>,
        layout: Option<&str>,
    ) -> AppResult<usize> {
        let ids = list_unread_ids_scoped(conn, feed_id, folder_id, starred_only, since_ms, layout)?;
        let n = mark_all_read(conn, feed_id, folder_id, starred_only, since_ms, layout)?;
        for id in ids {
            enqueue_sync(conn, Some(id), None, "read", None)?;
        }
        Ok(n)
    }

    /// M-9 验收③：同一夹具上，新集合化实现与参考旧实现在 all/feed/folder/
    /// layout/starred/since（含组合）各范围下产生完全相同的 is_read、
    /// sync_queue 行集（忽略 created_at）与 feed 计数；返回值相同。
    #[test]
    fn mark_all_read_with_enqueue_matches_reference_for_all_scopes() {
        let scopes: Vec<ScopeCase> = vec![
            ("全部", None, None, false, None, None),
            ("按源 f1", Some(1), None, false, None, None),
            ("按分类 F0", None, Some(1), false, None, None),
            ("布局 podcast", None, None, false, None, Some("podcast")),
            (
                "布局 image(分类兜底)",
                None,
                None,
                false,
                None,
                Some("image"),
            ),
            ("仅收藏", None, None, true, None, None),
            (
                "仅今天(零命中)",
                None,
                None,
                false,
                Some(1_900_000_000_000),
                None,
            ),
            ("源+收藏", Some(1), None, true, None, None),
            ("无分类源(不命中)", None, None, false, None, Some("gallery")),
        ];
        for (name, feed_id, folder_id, starred_only, since_ms, layout) in scopes {
            let a = seed_equiv_fixture();
            let na =
                reference_mark_all_read(&a, feed_id, folder_id, starred_only, since_ms, layout)
                    .unwrap();
            let sa = equiv_snapshot(&a);
            let b = seed_equiv_fixture();
            let nb =
                mark_all_read_with_enqueue(&b, feed_id, folder_id, starred_only, since_ms, layout)
                    .unwrap();
            let sb = equiv_snapshot(&b);
            assert_eq!(na, nb, "{name}：返回条数必须一致");
            assert_eq!(sa, sb, "{name}：is_read / 队列行集 / feed 计数必须逐项一致");
        }
    }

    /// M-9 验收③（判别力）：旧实现的事件数随 N 线性增长、新实现恒定——
    /// 若把集合化回退成逐 id 循环，本测试的恒定断言立即变红。
    #[test]
    fn mark_all_read_statement_events_do_not_scale_with_n() {
        // N=1：1 源 1 篇未读；N=200：1 源 200 篇未读（各自独立夹具，同一代码路径）
        let run = |n: i64| -> (usize, usize) {
            let build = || -> (Connection, i64) {
                let c = conn();
                let f = insert_feed(
                    &c,
                    "https://n.example/rss",
                    None,
                    "N",
                    None,
                    create_folder(&c, "F", "article").unwrap(),
                    "inherit",
                    false,
                    false,
                )
                .unwrap();
                for i in 0..n {
                    upsert_article_with_feed(&c, f, &na(&format!("g{i}"), None), false).unwrap();
                }
                (c, f)
            };
            let (ca, fa) = build();
            let (out_ref, ev_ref) = crate::db::with_count(&ca, None, || {
                reference_mark_all_read(&ca, Some(fa), None, false, None, None).unwrap()
            });
            let (cb, fb) = build();
            let (out_new, ev_new) = crate::db::with_count(&cb, None, || {
                mark_all_read_with_enqueue(&cb, Some(fb), None, false, None, None).unwrap()
            });
            assert_eq!(out_ref, n as usize, "参考实现必须标读全部 {n} 条");
            assert_eq!(out_new, n as usize, "新实现必须标读全部 {n} 条");
            (ev_ref, ev_new)
        };
        let (ref1, new1) = run(1);
        let (ref200, new200) = run(200);
        assert!(
            ref200 > ref1 * 10,
            "参考旧实现的事件数必须随 N 增长（ref1={ref1}, ref200={ref200}），否则本测试无判别力"
        );
        assert_eq!(
            new1, new200,
            "新实现语句数必须与 N 无关（new1={new1}, new200={new200}）"
        );
    }

    /// P3-3（自检 2026-09-29）：ai 分支与 articles 分支同口径——published_at 必须
    /// 过 datetime() 归一后再与 cutoff 比较。RFC3339 带 offset 的行（'T' > ' ' 的
    /// 字符串序恒大于 'YYYY-MM-DD HH:MM:SS' 形态的 cutoff）在 cutoff 当天漏清。
    /// 取 cutoff（now - 7 天）再前推 4 小时的时刻，折成 +08:00 offset 的 RFC3339：
    /// UTC 真值早于 cutoff（修后必清），而字符串序恒晚于 cutoff（修前必不清）——
    /// 两个方向都与运行当日时刻无关，测试确定性成立。
    #[test]
    fn cleanup_cache_ai_scope_normalizes_rfc3339_offset_before_cutoff() {
        let mut conn = conn();
        let fid = create_folder(&conn, "F", "article").unwrap();
        let feed = insert_feed(
            &conn,
            "https://f.example/rss",
            None,
            "f",
            None,
            fid,
            "inherit",
            true,
            false,
        )
        .unwrap();

        let offset8 = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
        let cutoff_utc = chrono::Utc::now() - chrono::Duration::days(7);
        // 边界行：cutoff 前 4 小时，写成 +08:00 offset 的 RFC3339（现行写入形态之一）
        let boundary = (cutoff_utc - chrono::Duration::hours(4)).with_timezone(&offset8);
        // 控制组①：RFC3339 UTC 且远新于 cutoff → 永不该清
        let fresh = (chrono::Utc::now() - chrono::Duration::days(1)).to_rfc3339();
        // 控制组②：legacy 空格格式（v12/v14 回填形态）且早于 cutoff → 两代口径都清
        let legacy = (cutoff_utc - chrono::Duration::hours(1))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();

        for (guid, published_at) in [
            ("offset-stale", boundary.to_rfc3339()),
            ("utc-fresh", fresh),
            ("legacy-stale", legacy),
        ] {
            let (aid, _) =
                upsert_article_with_feed(&conn, feed, &na(guid, Some(published_at)), false)
                    .unwrap();
            set_article_ai_fields(&conn, aid, Some("s"), None).unwrap();
        }

        // 判别力锚：边界行在修前的裸字符串比较下确实不满足 < cutoff
        let old_hit: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM articles WHERE guid = 'offset-stale'
                  AND published_at < datetime('now', '-7 days')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            old_hit, 0,
            "修前裸字符串比较漏掉 offset 边界行（否则本测试无判别力）"
        );

        let (deleted, ai_cleared) = cleanup_cache(&mut conn, 7, "ai").unwrap();
        assert_eq!(deleted, 0, "ai scope 不删文章");
        assert_eq!(
            ai_cleared, 2,
            "offset 边界行与 legacy 行必须被清（修前为 1，漏掉 offset 行）"
        );
        let kept: Option<String> = conn
            .query_row(
                "SELECT ai_summary FROM articles WHERE guid = 'utc-fresh'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kept.as_deref(), Some("s"), "新于 cutoff 的行不得被清");
    }

    /// P3-4（自检 2026-09-29）：limit 夹取——负数在 SQLite 的 LIMIT 语义是
    /// 「不限制」，必须收敛为 0 行；正常分页与前端合法大值（100000）不受影响。
    #[test]
    fn list_articles_clamps_negative_limit_to_zero_rows() {
        let conn = conn();
        let fid = create_folder(&conn, "F", "article").unwrap();
        let feed = insert_feed(
            &conn,
            "https://f.example/rss",
            None,
            "f",
            None,
            fid,
            "inherit",
            true,
            false,
        )
        .unwrap();
        for i in 0..3 {
            upsert_article_with_feed(
                &conn,
                feed,
                &na(
                    &format!("g{i}"),
                    Some(format!("2026-01-01T00:00:0{i}+00:00")),
                ),
                false,
            )
            .unwrap();
        }
        let q = |limit: i64| ArticleQuery {
            feed_id: Some(feed),
            folder_id: None,
            only_unread: false,
            only_starred: false,
            only_today: false,
            newest_first: true,
            limit,
            offset: 0,
            with_content: false,
            layout: None,
            last_published: None,
            last_id: None,
        };
        assert!(
            list_articles(&conn, &q(-1)).unwrap().is_empty(),
            "负 limit 必须收敛为 0 行（修前 = 不限制，整库倒出）"
        );
        assert!(
            list_articles(&conn, &q(i64::MIN)).unwrap().is_empty(),
            "极小值同样夹取"
        );
        assert_eq!(
            list_articles(&conn, &q(0)).unwrap().len(),
            0,
            "limit=0 → 0 行（既有语义锚）"
        );
        assert_eq!(
            list_articles(&conn, &q(2)).unwrap().len(),
            2,
            "正常分页不受影响"
        );
        assert_eq!(
            list_articles(&conn, &q(100000)).unwrap().len(),
            3,
            "前端合法大值（ARTICLES_PAGE_SIZE=100000）必须原样放行"
        );
    }

    /* ============================================================
    TASK-117（审计 P1-1）：keyset 分页谓词——可变集合上连续翻页不丢不重
    ============================================================ */

    /// 夹具：12 篇文章，published_at 按 i/3 分 4 组（组内**同秒并列**），
    /// id 随插入递增。预期全序：DESC = (published_at DESC, id DESC)——
    /// 同秒组内按 id 降序；ASC 反向。
    fn seed_keyset_fixture() -> (Connection, i64, Vec<i64>) {
        let conn = conn();
        let fid = create_folder(&conn, "F", "article").unwrap();
        let feed = insert_feed(
            &conn,
            "https://k.example/feed",
            None,
            "k",
            None,
            fid,
            "inherit",
            true,
            false,
        )
        .unwrap();
        let mut ids = Vec::new();
        for i in 0..12 {
            let group = i / 3;
            let (aid, _) = upsert_article_with_feed(
                &conn,
                feed,
                &na(
                    &format!("k{i}"),
                    Some(format!("2026-01-01T00:{:02}:00+00:00", group * 10)),
                ),
                false,
            )
            .unwrap();
            ids.push(aid);
        }
        (conn, feed, ids)
    }

    fn keyset_q(feed: i64, newest_first: bool, only_unread: bool) -> ArticleQuery {
        ArticleQuery {
            feed_id: Some(feed),
            folder_id: None,
            only_unread,
            only_starred: false,
            only_today: false,
            newest_first,
            limit: 0,
            offset: 0,
            with_content: false,
            layout: None,
            last_published: None,
            last_id: None,
        }
    }

    /// 全序期望（DESC）：published_at 降序，同秒组内 id 降序。
    fn expected_desc_order(ids: &[i64]) -> Vec<i64> {
        let mut keyed: Vec<(String, i64)> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| (format!("2026-01-01T00:{:02}:00+00:00", (i / 3) * 10), *id))
            .collect();
        keyed.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
        keyed.into_iter().map(|(_, id)| id).collect()
    }

    /// 全序期望（ASC）：published_at 升序，同秒组内 id 升序。
    fn expected_asc_order(ids: &[i64]) -> Vec<i64> {
        let mut keyed: Vec<(String, i64)> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| (format!("2026-01-01T00:{:02}:00+00:00", (i / 3) * 10), *id))
            .collect();
        keyed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        keyed.into_iter().map(|(_, id)| id).collect()
    }

    /// keyset 连续翻页走完全集：每页 limit=page，锚 = 上一页最后一行，
    /// OFFSET 恒 0（前端续拉形态）。
    fn walk_by_keyset(conn: &Connection, q: &ArticleQuery, page: i64) -> Vec<i64> {
        let mut cursor = q.clone();
        cursor.limit = page;
        cursor.offset = 0;
        let mut out = Vec::new();
        loop {
            let rows = list_articles(conn, &cursor).unwrap();
            let n = rows.len() as i64;
            out.extend(rows.iter().map(|r| r.id));
            if n < page {
                break;
            }
            let last = rows.last().unwrap();
            cursor.last_published = last.published_at.clone();
            cursor.last_id = Some(last.id);
        }
        out
    }

    /// OFFSET 连续翻页走完全集（不可变集合上的参照实现）。
    fn walk_by_offset(conn: &Connection, q: &ArticleQuery, page: i64) -> Vec<i64> {
        let mut cursor = q.clone();
        cursor.limit = page;
        let mut out = Vec::new();
        loop {
            let rows = list_articles(conn, &cursor).unwrap();
            let n = rows.len() as i64;
            out.extend(rows.iter().map(|r| r.id));
            if n < page {
                break;
            }
            cursor.offset += page;
        }
        out
    }

    /// keyset 谓词正确性①：DESC/ASC 两方向、同秒并列由 id 决胜、
    /// 与 OFFSET 路径在不可变集合上逐页等价（keyset 全集 == offset 全集）。
    /// 这同时锁定「首页一致」：两路径的第一页来自同一序（offset 0 / 无谓词）。
    #[test]
    fn keyset_pagination_matches_offset_and_resolves_ties() {
        let (conn, feed, ids) = seed_keyset_fixture();

        let desc = keyset_q(feed, true, false);
        let keyset_desc = walk_by_keyset(&conn, &desc, 5);
        let offset_desc = walk_by_offset(&conn, &desc, 5);
        assert_eq!(
            keyset_desc,
            expected_desc_order(&ids),
            "DESC keyset 全序：published_at 降序 + 同秒组 id 降序（不重不漏走完全集）"
        );
        assert_eq!(
            keyset_desc, offset_desc,
            "同集合同排序下 keyset 翻页必须与 OFFSET 翻页逐页等价（DESC）"
        );

        let asc = keyset_q(feed, false, false);
        let keyset_asc = walk_by_keyset(&conn, &asc, 5);
        let offset_asc = walk_by_offset(&conn, &asc, 5);
        assert_eq!(
            keyset_asc,
            expected_asc_order(&ids),
            "ASC keyset 全序：published_at 升序 + 同秒组 id 升序"
        );
        assert_eq!(
            keyset_asc, offset_asc,
            "同集合同排序下 keyset 翻页必须与 OFFSET 翻页逐页等价（ASC）"
        );
    }

    /// keyset 谓词正确性②（审计 P1 探针场景的本体）：可变筛选集合
    /// （WHERE is_read=0）上读掉一页后续拉——keyset 必须返回**剩余集合**的
    /// 下一页（OFFSET 语义在此会跳过 (页大小) 行并假 exhausted）。
    #[test]
    fn keyset_continuation_tracks_mutable_unread_collection() {
        let (conn, feed, _) = seed_keyset_fixture();
        let page = 5i64;
        let unread = keyset_q(feed, true, true);
        // 预期全序（未读视图与全集合同序：全部行未读）
        let expected = expected_desc_order(&seed_ids(&conn));
        let mut seen: Vec<i64> = Vec::new();

        let mut cursor = unread.clone();
        cursor.limit = page;
        loop {
            let rows = list_articles(&conn, &cursor).unwrap();
            let fetched = rows.len();
            let page_ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
            // 操作序列：读掉当前页（真实「打开即标读」的批量形态）
            for id in &page_ids {
                set_read(&conn, *id, true).unwrap();
            }
            seen.extend(&page_ids);
            if fetched < page as usize {
                break;
            }
            let last = rows.last().unwrap();
            cursor.last_published = last.published_at.clone();
            cursor.last_id = Some(last.id);
        }

        assert_eq!(
            seen, expected,
            "可变集合上连续「读一页→续拉」必须不重不漏走完全集"
        );
    }

    /// keyset 谓词正确性③：同秒并列组跨页边界——锚行的同秒兄弟（published_at
    /// 相等、id 更小）必须由 `published_at = ? AND a.id < ?` 分支在本页续上，
    /// 不得因 `published_at < ?` 单臂漏行（页大小刻意与组大小互质：5 vs 3）。
    #[test]
    fn keyset_tie_group_spans_page_boundary_without_gap() {
        let (conn, feed, ids) = seed_keyset_fixture();
        let q = keyset_q(feed, true, false);
        let page = 5i64;

        let mut cursor = q.clone();
        cursor.limit = page;
        cursor.offset = 0;
        let first = list_articles(&conn, &cursor).unwrap();
        assert_eq!(first.len(), 5, "夹具自检：满页");
        let anchor = first.last().unwrap();
        // 锚行所属同秒组的兄弟（id 更小）必然一部分已被本页包含、一部分没有：
        // 断言锚的 published_at 与首屏某行相同（同秒组跨页的前提成立）
        assert!(
            first
                .iter()
                .any(|r| r.published_at == anchor.published_at && r.id != anchor.id),
            "夹具自检：锚行的同秒兄弟必须部分落在首屏（组跨页边界）"
        );

        cursor.last_published = anchor.published_at.clone();
        cursor.last_id = Some(anchor.id);
        let second = list_articles(&conn, &cursor).unwrap();
        let second_ids: Vec<i64> = second.iter().map(|r| r.id).collect();
        // 第二页 = 全序中严格排在锚之后的 5 行；其中必须含锚的同秒兄弟（id < 锚 id 且同秒）
        let want = expected_desc_order(&ids);
        let anchor_pos = want.iter().position(|id| *id == anchor.id).unwrap();
        assert_eq!(
            second_ids,
            want[anchor_pos + 1..anchor_pos + 6],
            "keyset 第二页必须恰好是全序中锚之后的 5 行（同秒兄弟由 id 决胜臂续上）"
        );
        assert!(
            second
                .iter()
                .any(|r| r.published_at == anchor.published_at && r.id < anchor.id),
            "第二页必须含锚行的同秒兄弟（= 分支生效），否则谓词退化为纯 < 比较"
        );
    }

    /// 夹具辅助：从库中按插入序取回 ids（expected_* 依赖插入序与分组的关系）。
    fn seed_ids(conn: &Connection) -> Vec<i64> {
        let mut stmt = conn.prepare("SELECT id FROM articles ORDER BY id").unwrap();
        stmt.query_map([], |r| r.get::<_, i64>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }
}
