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
    /// 正文是否已被全文提取覆盖（手动按钮/自动模式共用状态源；来自 contents）
    pub fulltext_extracted: bool,
    /// OPT-008B 第二阶段：单一正文所有权——条目引用的 contents 行与其修订号。
    /// 前端以 (content_id, content_revision) 判定正文失效（snippet 未变也失效）。
    pub content_id: Option<i64>,
    pub content_revision: i64,
    /// OPT-013 B1：AI 产物缓存签名（生成时输入身份的 sha256；存量/未生成行 NULL）。
    /// 前端以「产物非空 && 签名为空」呈「未标记版本」；命中判定在命令层做。
    pub ai_summary_sig: Option<String>,
    pub translated_sig: Option<String>,
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
    /// OPT-008B 第二阶段：正文身份/修订（前端失效依据）
    pub content_id: Option<i64>,
    pub content_revision: i64,
    /// OPT-013 B1：AI 产物缓存签名（同 ArticleRow；轻量行也返回——卡片即时显示
    /// 摘要的「未标记版本」判定依赖它）
    pub ai_summary_sig: Option<String>,
    pub translated_sig: Option<String>,
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

/// 详情/水合查询列（OPT-008B 第二阶段：正文只从 contents 取；条目级字段从 a 取）。
/// 尾部两列为 content_id/content_revision（前端修订失效依据）；
/// OPT-013 B1：再追加两列 AI 产物缓存签名。
const ARTICLE_COLS: &str = "a.id, a.feed_id, a.url, a.title, a.author, a.summary, c.content_html, a.image_url, a.enclosure_url, a.enclosure_mime, a.duration_sec, a.ai_summary, a.translated_content, a.source, a.published_at, a.is_read, a.is_starred, COALESCE(c.fulltext_extracted, 0), a.content_id, COALESCE(c.revision, 0), a.ai_summary_sig, a.translated_sig";

/// 详情/水合查询的 FROM（单一正文所有权 JOIN）。
const ARTICLE_FROM: &str = " FROM articles a LEFT JOIN contents c ON c.id = a.content_id";

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
        content_id: r.get(18)?,
        content_revision: r.get(19)?,
        ai_summary_sig: r.get(20)?,
        translated_sig: r.get(21)?,
    })
}

fn article_list_item(r: &rusqlite::Row<'_>) -> rusqlite::Result<ArticleListItem> {
    // 列顺序依赖 list_articles 的 SELECT；当 with_content=false 时中段四列为 NULL/0，
    // 尾部四列（content_id/content_revision/AI 产物签名）两种模式都返回。
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
        content_id: r.get(18)?,
        content_revision: r.get(19)?,
        ai_summary_sig: r.get(20)?,
        translated_sig: r.get(21)?,
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

/* ============================================================
OPT-008B 第一阶段：smartDedup 的显示聚合谓词

智能去重不再是入库闸门（条目是订阅源/远端 ID 的独立实体，跨源同 URL 各留
一行；见 upsert_article_with_feed 注释），而是**查询呈现层的聚合策略**：

- 代表选择 = 同组（`url_norm` 相等）中 id 最小的行；组内 ID 稳定 ⇒ 代表稳定，
  分页/锚定不因开关切换抖动之外的因素漂移；
- 组在**当前查询 scope 与当前视图筛选的交集**内形成——scope（feed/folder/
  layout）与视图筛选（未读/收藏/今天）都是「该视图自己的集合」的一部分：
  单源视图永不被其它源的副本吞掉、不同布局各自可达、未读视图不会因代表
  已读而整体消失（不得漏项）；
- 谓词必须与外部查询的 WHERE 逐字同构并进入 SQL 本体（而不是分页后再数组
  去重——那会跳过页、hidden 集合随页边界变化）：列表、`article_index`、
  搜索、`mark_all_read`、`list_unread_ids_scoped`、`feed_counts`、
  `scope_counts` 全部消费同一生成器 `dedup_exists_clause` 或其等价常量。
============================================================ */

/// smartDedup 开关（app_settings JSON，默认关 = 关闭去重展示，全部条目可见）。
/// 读取失败按关闭处理（显示层默认展示全部，不隐藏任何条目）；不是入库语义。
fn smart_dedup_on(conn: &Connection) -> bool {
    crate::db::app_settings_bool(conn, "smartDedup", false)
}

/// 显示聚合谓词生成器：`NOT EXISTS (同组更低 id 的行在**同一 scope 与视图**
/// 内)`。返回 (SQL 片段, 绑定值)；总是以 `a` 为外部行别名，内部以 `b` 为
/// 同组行别名。scope 谓词与视图谓词由调用方逐项声明（必须与外部 WHERE 同构）。
///
/// - `feed`：限定同 feed（`b.feed_id = a.feed_id`，无需额外绑定）；
/// - `folder`/`layout`：与外部同构的标量子查询（独立别名 f3/f4/fo4，避免与
///   外部查询的同名片段相互遮蔽；值各绑定一次）；
/// - `unread_only`/`starred_only`/`today_only`：视图筛选，必须在组内参与
///   代表选择（否则未读视图里「代表已读」会把仍有未读副本的组整体藏掉）。
#[derive(Default, Clone, Copy)]
struct DedupScope<'a> {
    feed: bool,
    folder: Option<i64>,
    layout: Option<&'a str>,
    unread_only: bool,
    starred_only: bool,
    today_only: bool,
    /// 与 `scope_clause` 的 since 同构的边界（unix ms；闭区间用 >= 与外部一致）。
    since_ms: Option<i64>,
}

fn dedup_exists_clause(scope: DedupScope) -> (String, Vec<rusqlite::types::Value>) {
    let mut cond = String::from("b.url_norm = a.url_norm AND b.id < a.id");
    let mut params: Vec<rusqlite::types::Value> = Vec::new();
    if scope.feed {
        cond.push_str(" AND b.feed_id = a.feed_id");
    }
    if let Some(folder) = scope.folder {
        cond.push_str(" AND (SELECT folder_id FROM feeds f3 WHERE f3.id = b.feed_id) = ?");
        params.push(folder.into());
    }
    if let Some(layout) = scope.layout {
        cond.push_str(
            " AND (SELECT CASE WHEN f4.layout != 'inherit' THEN f4.layout ELSE fo4.layout END \
             FROM feeds f4 JOIN folders fo4 ON f4.folder_id = fo4.id WHERE f4.id = b.feed_id) = ?",
        );
        params.push(layout.to_string().into());
    }
    if scope.unread_only {
        cond.push_str(" AND b.is_read = 0");
    }
    if scope.starred_only {
        cond.push_str(" AND b.is_starred = 1");
    }
    if scope.today_only {
        cond.push_str(" AND date(b.published_at, 'localtime') = date('now', 'localtime')");
    }
    if let Some(ms) = scope.since_ms {
        cond.push_str(" AND datetime(b.published_at) >= datetime(?, 'unixepoch')");
        params.push((ms / 1000).into());
    }
    (
        format!("NOT EXISTS (SELECT 1 FROM articles b WHERE {cond})"),
        params,
    )
}

/// 由 `ArticleQuery` 派生显示聚合 scope（与 `article_where` 的筛选逐项同构）。
fn dedup_scope_of(q: &ArticleQuery) -> DedupScope<'_> {
    DedupScope {
        feed: q.feed_id.is_some(),
        folder: q.folder_id,
        layout: q.layout.as_deref(),
        unread_only: q.only_unread,
        starred_only: q.only_starred,
        today_only: q.only_today,
        since_ms: None,
    }
}

/// 组装列表查询 SQL + 绑定（`list_articles` 的生产字节；测试对同一产物跑
/// EXPLAIN QUERY PLAN，见 `list_query_plan_*`）。
/// `dedup` = smartDedup 显示聚合是否开启（由 `list_articles` 从设置解析后传入，
/// 测试可显式控制）。
fn list_articles_sql(q: &ArticleQuery, dedup: bool) -> (String, Vec<rusqlite::types::Value>) {
    // 正文列只在 with_content 时 SELECT（社交/通知布局需要），其余布局保持轻量查询。
    // 尾部两列（content_id/content_revision）两种模式都返回——前端修订失效依据。
    let content_cols = if q.with_content {
        "a.url, c.content_html, a.translated_content, COALESCE(c.fulltext_extracted, 0)"
    } else {
        "NULL, NULL, NULL, 0"
    };
    let mut sql = format!(
        "SELECT a.id, a.feed_id, a.title, a.author,
                COALESCE(NULLIF(a.summary, ''), substr(COALESCE(c.body_text, ''), 1, 280)) AS snippet,
                a.image_url, a.enclosure_url, a.enclosure_mime, a.duration_sec,
                a.ai_summary, a.source, a.published_at, a.is_read, a.is_starred,
                {content_cols}, a.content_id, COALESCE(c.revision, 0), a.ai_summary_sig, a.translated_sig
         FROM articles a LEFT JOIN contents c ON c.id = a.content_id",
    );
    // 值全部走绑定参数（占位符序号即绑定顺序），条件文本只拼固定字符串
    let (mut where_clauses, mut params) = article_where(q, dedup);
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
        where_clauses.push(predicate.to_string());
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
    // OPT-008B：显示聚合开关在查询入口解析一次（设置读取失败按关闭处理——显示
    // 层默认展示全部条目，不隐藏任何数据）。
    let (sql, params) = list_articles_sql(q, smart_dedup_on(conn));
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
/// 共用，保证「绝对位置」与「列表顺序」口径一致；OPT-008B 起 smartDedup 显示
/// 聚合谓词也在同一处追加，两处口径天然同构）。
fn article_where(q: &ArticleQuery, dedup: bool) -> (Vec<String>, Vec<rusqlite::types::Value>) {
    let mut where_clauses: Vec<String> = vec![];
    let mut params: Vec<rusqlite::types::Value> = Vec::new();
    if let Some(fid) = q.feed_id {
        where_clauses.push("a.feed_id = ?".to_string());
        params.push(fid.into());
    }
    if let Some(folder) = q.folder_id {
        // M-5：标量子查询形态（见 FOLDER_FILTER_SQL）——IN 子查询会让规划器
        // 改走 feed 索引并对结果回退 TEMP B-TREE 排序；标量形态可由
        // idx_articles_published 有序驱动（实测见 tmp/task-099/explain-query-plan.md）。
        where_clauses.push(FOLDER_FILTER_SQL.to_string());
        params.push(folder.into());
    }
    if q.only_unread {
        where_clauses.push("a.is_read = 0".to_string());
    }
    if q.only_starred {
        where_clauses.push("a.is_starred = 1".to_string());
    }
    if q.only_today {
        // TASK-064 N5：两侧统一本地时区——published_at 存 RFC3339（常带 offset），
        // SQLite 的 date() 对带 offset 值归一到 UTC，直接 date(a.published_at) 会
        // 与 date('now','localtime') 错位：非本地时区用户本地凌晨（+08:00 的
        // 00:00-08:00）发布的文章不进「今天」视图。
        where_clauses
            .push("date(a.published_at, 'localtime') = date('now', 'localtime')".to_string());
    }
    if let Some(l) = &q.layout {
        // TASK-094：列表查询的布局维度与 mark_all_read / list_unread_ids_scoped
        // 同一段谓词（见 LAYOUT_FILTER_SQL，一处定义三处共用）。
        // 外层 FROM 只有 articles（别名 a），片段里的 feed_id 无歧义地解析到它；
        // f2/fo2 是子查询自己的别名，不受影响。M-5 起为标量形态，绑定一次。
        where_clauses.push(LAYOUT_FILTER_SQL.to_string());
        params.push(l.clone().into());
    }
    if dedup {
        // OPT-008B：显示聚合（见 dedup_exists_clause 文件段注释）——谓词与
        // 上述 scope/视图条件逐项同构，追加在最后，绑定顺序紧随其后。
        let (clause, dedup_params) = dedup_exists_clause(dedup_scope_of(q));
        where_clauses.push(clause);
        params.extend(dedup_params);
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
    let (sql, params) = article_index_sql(q, article_id, smart_dedup_on(conn));
    let pos = conn
        .prepare(&sql)?
        .query_row(rusqlite::params_from_iter(params), |r| r.get::<_, i64>(0))
        .optional()?;
    Ok(pos)
}

/// 组装 article_index 的窗口 SQL + 绑定（生产字节；测试对同一产物跑
/// EXPLAIN QUERY PLAN，见 `list_query_plan_*`）。
fn article_index_sql(
    q: &ArticleQuery,
    article_id: i64,
    dedup: bool,
) -> (String, Vec<rusqlite::types::Value>) {
    let (where_clauses, mut params) = article_where(q, dedup);
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
            &format!("SELECT {ARTICLE_COLS}{ARTICLE_FROM} WHERE a.id = ?1"),
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
    let sql = format!("SELECT {ARTICLE_COLS}{ARTICLE_FROM} WHERE a.id IN ({placeholders})");
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

/// 全文搜索：标题/正文/摘要/AI 摘要/AI 翻译的 LIKE 子串匹配（词间 AND，按
/// 发布时间倒序，返回与列表页同构的轻量行），并由 v29 的 trigram 候选索引
/// 收缩扫描行数。
///
/// 为什么结果是 LIKE 语义：unicode61 分词器把连续中文整段当作一个 token，
/// 搜「科技」永远匹配不到正文里的「科技公司新闻」——对中文用户形同虚设；
/// 且 FTS5 查询语法字符（"*()" 等）需要额外转义，`node.js`、`C++` 这类词
/// 的前缀/精确语义也很反直觉。LIKE 的"包含这个子串"对任意语言/任意字符
/// 都确定。AI 字段纳入命中后（SRH-2），v3 起的 `articles_fts`（unicode61）
/// 已无 MATCH 消费者，v29（OPT-017B）将其删除并以本索引替代——它是被搜索
/// 真实消费的**候选面**。
///
/// 候选索引（`articles_trigram_idx`，migrations v29）契约：
/// - **只收缩不判定**：≥3 字符的词生成 `"abc" AND "bcd"` 形式的引用三元组
///   MATCH 作为 `a.id IN (...)` 候选；词短于 3 字符（如 2 字中文）无法成
///   三元组，该词不加候选（整查退回纯 LIKE）。结果集始终由下方 LIKE 条件
///   逐行验证——与实现前行为（冻结参考实现回归）逐字等值：id 集合/顺序/
///   去重/字段覆盖、AI 字段命中、中英文、无结果、`%`/`_`/`\` 转义。
/// - 索引面（表 + 四触发器）缺失、或 MATCH 报错（影子表损坏等）时**回落
///   LIKE** 并记日志；回落不依赖候选（回归覆盖"删索引/坏索引"两组）。
/// - 陈旧边界：候选是超集，陈旧只会**多**出候选（多出的行被 LIKE 拒绝）；
///   漏行只可能出现在"触发器被手工拆除"的库——该状态无法经应用产生
///   （v29 一并创建索引与触发器），不承诺修复。
///
/// 匹配字段：标题 + 正文纯文本 + 摘要 + AI 摘要 + AI 翻译；LIKE 通配符 %/_
/// 按字面转义。该字段集与 v29 候选索引文档串的列集**必须同步维护**
/// （migrations v29 的 lower(...) 连接列缺一即造成 LIKE 命中的假阴性）。
pub fn search_articles(
    conn: &Connection,
    query: &str,
    limit: i64,
) -> AppResult<Vec<ArticleListItem>> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let raw_terms: Vec<&str> = q.split_whitespace().filter(|t| !t.is_empty()).collect();
    if raw_terms.is_empty() {
        return Ok(Vec::new());
    }
    let terms: Vec<String> = raw_terms.iter().map(|t| like_escape(t)).collect();
    let candidate_matches: Vec<String> = if search_candidates_ready(conn) {
        raw_terms
            .iter()
            .filter_map(|t| trigram_candidate_match(t))
            .collect()
    } else {
        Vec::new()
    };
    if candidate_matches.is_empty() {
        return run_search(conn, &terms, &[], limit);
    }
    match run_search(conn, &terms, &candidate_matches, limit) {
        Ok(items) => Ok(items),
        Err(err) => {
            // 候选只是加速面，结果契约以 LIKE 为准：索引损坏/畸形时回落。
            log::warn!("搜索候选索引不可用，回落 LIKE-only：{err}");
            run_search(conn, &terms, &[], limit)
        }
    }
}

/// LIKE 通配符按字面转义（SQL 侧 `ESCAPE '\'`）。
fn like_escape(term: &str) -> String {
    term.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// 候选索引的 SQL 面是否齐备（表 + 四个维护触发器，migrations v29 一并创建）。
/// 缺失即不使用候选，不在此处报错——搜索承诺在任何 schema 状态下仍以 LIKE
/// 给出正确结果。
fn search_candidates_ready(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE \
         (type = 'table' AND name = 'articles_trigram_idx') OR \
         (type = 'trigger' AND name IN ('trg_articles_trigram_ai','trg_articles_trigram_ad','trg_articles_trigram_au','trg_contents_trigram_au'))",
        [],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n == 5)
    .unwrap_or(false)
}

/// 单词语 → 候选 MATCH 表达式：ASCII 折叠（SQLite `lower()` 只折叠 ASCII，
/// 与 LIKE 的大小写语义逐字一致）后取全部引用三元组，各自加引号（`"` 双写
/// 转义）并以 AND 连接。语义为"这些三元组都出现"的**超集**（不要求连续），
/// 由 LIKE 条件收紧。词短于 3 字符返回 None（该词不加候选）。三元组只取前
/// 64 个：截取只会放宽候选（仍是超集），杜绝超长词把 MATCH 表达式撑爆。
fn trigram_candidate_match(term: &str) -> Option<String> {
    let folded = term.to_ascii_lowercase();
    let chars: Vec<char> = folded.chars().collect();
    if chars.len() < 3 {
        return None;
    }
    let parts: Vec<String> = chars
        .windows(3)
        .take(64)
        .map(|w| {
            let tri: String = w.iter().collect();
            format!("\"{}\"", tri.replace('"', "\"\""))
        })
        .collect();
    Some(parts.join(" AND "))
}

/// [`search_articles`] 的 SQL 装配与执行内核：`candidate_matches` 为空 = 纯
/// LIKE（与实现前 SQL 同构）；非空 = 追加候选过滤。拆出为独立函数只为让测试
/// 能对生成的 SQL 做 EXPLAIN 计划断言；生产调用入口只有 [`search_articles`]。
fn run_search(
    conn: &Connection,
    terms: &[String],
    candidate_matches: &[String],
    limit: i64,
) -> AppResult<Vec<ArticleListItem>> {
    let (sql, bind_values) = build_search_sql(terms, candidate_matches, smart_dedup_on(conn));
    // P3[8]（REQ-104）：LIMIT 此前用字符串插值 `LIMIT {limit}`。i64 本身无注入风险，
    // 但**负数在 SQLite 里表示「不限制」**，调用方一处笔误就会把整个库倒出来；
    // 且它不是绑定参数、不受参数检查保护。改为绑定参数并把上界收敛：
    // 非正数 → 回落安全默认（搜索场景没有「不限制」语义），并设上限防误用。
    let limit: i64 = if limit <= 0 { 100 } else { limit.min(1000) };
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params_from_iter(
            bind_values
                .iter()
                .map(|v| v as &dyn rusqlite::ToSql)
                .chain(std::iter::once(&limit as &dyn rusqlite::ToSql)),
        ),
        article_list_item,
    )?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 装配搜索 SQL 与绑定值（LIMIT 为末尾占位符 `?{n}`，值由 [run_search] 绑定）。
/// `dedup_on` = smartDedup；`terms` 为已 LIKE 转义的词表。
fn build_search_sql(
    terms: &[String],
    candidate_matches: &[String],
    dedup_on: bool,
) -> (String, Vec<rusqlite::types::Value>) {
    // 每个词一组 (title/body/summary/ai_summary/translated LIKE)，词间 AND。
    // 占位符逐个编号（?N 是编号引用，跨词组复用会错位），ESCAPE 声明 SQLite 的
    // LIKE 通配符转义字符 '\'。同一组条件既用于外部匹配（别名 a + 正文别名 c），
    // 也用于显示聚合的内层匹配集（别名 b + 正文别名 bc）——见下方 R1 搜索去重说明。
    // OPT-008B 第二阶段：正文纯文本来自 contents（单一正文所有权）。
    let term_conditions = |alias: &str,
                           content_alias: &str,
                           start_idx: usize|
     -> (Vec<String>, usize) {
        let mut parts = Vec::with_capacity(terms.len());
        let mut idx = start_idx;
        for _ in terms.iter() {
            let b = idx;
            parts.push(format!(
                "({a}.title LIKE ?{b} ESCAPE '\\' OR {ac}.body_text LIKE ?{c} ESCAPE '\\' OR COALESCE({a}.summary,'') LIKE ?{d} ESCAPE '\\' OR COALESCE({a}.ai_summary,'') LIKE ?{e} ESCAPE '\\' OR COALESCE({a}.translated_content,'') LIKE ?{f} ESCAPE '\\')",
                a = alias,
                ac = content_alias,
                b = b,
                c = b + 1,
                d = b + 2,
                e = b + 3,
                f = b + 4,
            ));
            idx += 5;
        }
        (parts, idx)
    };
    let (where_parts, next_idx) = term_conditions("a", "c", 1);
    // 候选过滤：编号接在外层 LIKE（?1..?5n）之后。
    // 装配顺序 = 文本次序 = 绑定次序：外层 LIKE → 候选 MATCH → 去重内层 LIKE → LIMIT。
    let mut candidate_sql = String::new();
    let mut candidate_args: Vec<rusqlite::types::Value> = Vec::new();
    for (i, m) in candidate_matches.iter().enumerate() {
        candidate_sql.push_str(&format!(
            " AND a.id IN (SELECT rowid FROM articles_trigram_idx WHERE articles_trigram_idx MATCH ?{})",
            next_idx + i
        ));
        candidate_args.push(m.clone().into());
    }
    let next_idx = next_idx + candidate_matches.len();
    let mut like_args: Vec<String> = Vec::with_capacity(terms.len() * 5);
    for t in terms.iter() {
        let pat = format!("%{t}%");
        for _ in 0..5 {
            like_args.push(pat.clone());
        }
    }
    /* OPT-008B R1（主控源码发现）：搜索的显示去重必须在**关键词匹配集合内**
    选代表——内层 EXISTS 带上与外部逐字相同的 LIKE 条件（别名 b），否则
    「同 URL 组里小 id 副本不含词、大 id 副本含词」时含词副本会被无词代表
    隐藏，搜索漏匹配。命中的代表可在列表 scope 锚定（实测由回归锁定），
    不做「关闭搜索去重」也不做「limit 后数组过滤」。 */
    let (dedup_sql, params): (String, Vec<rusqlite::types::Value>) = if dedup_on {
        let (inner_parts, _) = term_conditions("b", "bc", next_idx);
        // 内层参数与外部同值同序，追加一遍（词 ×5）
        let mut extra: Vec<rusqlite::types::Value> = Vec::with_capacity(terms.len() * 5);
        for t in terms.iter() {
            let pat = format!("%{t}%");
            for _ in 0..5 {
                extra.push(pat.clone().into());
            }
        }
        (
            format!(
                " AND NOT EXISTS (SELECT 1 FROM articles b LEFT JOIN contents bc ON bc.id = b.content_id \
                 WHERE b.url_norm = a.url_norm AND b.id < a.id AND {})",
                inner_parts.join(" AND ")
            ),
            extra,
        )
    } else {
        (String::new(), Vec::new())
    };
    let limit_idx = like_args.len() + candidate_args.len() + params.len() + 1;
    // 候选存在时用 INDEXED BY 固定上表访问路径：规划器若把 IN(子查询) 反转为
    // 「FTS 驱动的半连接 + TEMP B-TREE FOR ORDER BY」，命中稠密的长词会丢掉
    // 排序索引的 LIMIT 提前停止（实测 20k×8KiB：3.6ms → 118.8ms；稀疏词两
    // 计划都 <1ms）。INDEXED BY 名缺失（理论上的畸形库）会 prepare 报错 →
    // search_articles 回落 LIKE-only（回落 SQL 不带 INDEXED BY）。
    let from_clause = if candidate_matches.is_empty() {
        "articles a LEFT JOIN contents c ON c.id = a.content_id"
    } else {
        "articles a INDEXED BY idx_articles_published_id LEFT JOIN contents c ON c.id = a.content_id"
    };
    let sql = format!(
        "SELECT a.id, a.feed_id, a.title, a.author,
                COALESCE(NULLIF(a.summary, ''), substr(COALESCE(c.body_text, ''), 1, 280)) AS snippet,
                a.image_url, a.enclosure_url, a.enclosure_mime, a.duration_sec,
                a.ai_summary, a.source, a.published_at, a.is_read, a.is_starred,
                NULL, NULL, NULL, 0, a.content_id, COALESCE(c.revision, 0), a.ai_summary_sig, a.translated_sig
         FROM {from_clause}
         WHERE {where}{candidate_sql}{dedup_sql}
         ORDER BY a.published_at DESC
         LIMIT ?{limit_idx}",
        from_clause = from_clause,
        where = where_parts.join(" AND "),
        candidate_sql = candidate_sql,
        dedup_sql = dedup_sql,
        limit_idx = limit_idx
    );
    // 绑定顺序 = 文本次序：外层 LIKE → 候选 MATCH → 去重内层 LIKE → LIMIT。
    let mut bind_values: Vec<rusqlite::types::Value> =
        like_args.into_iter().map(Into::into).collect();
    bind_values.extend(candidate_args);
    bind_values.extend(params);
    (sql, bind_values)
}

/// 清空去重墓碑（smartDedup 开→关 瞬间调用：用户想让重复文章回来）。
///
/// OPT-008B 第一阶段：内容去重墓碑不再是入库闸门（入库不再跨源丢弃同 URL
/// 条目——见 `upsert_article_with_feed` 的函数注释），本函数保留为**有限兼容**
/// 入口（开关关闭路径仍调用；`deduped_urls` 表结构不删，008A 迁移已清空历史
/// 墓碑）。清理幂等。
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
        // OPT-008B 第二阶段：清理按**条目行**生效（原年龄范围不变）——共享 contents
        // 的兄弟条目不受影响（不误清年轻兄弟）；被清条目 ai_epoch+1，使网络在途的
        // 迟到 AI 提交被守卫拒绝（不复活已清产物）。正文保留（重新打开可再生成）。
        ai_cleared = tx.execute(
            &format!(
                "UPDATE articles SET ai_summary = NULL, translated_content = NULL,
                        ai_summary_sig = NULL, translated_sig = NULL, ai_epoch = ai_epoch + 1
                 WHERE (ai_summary IS NOT NULL OR translated_content IS NOT NULL)
                   AND datetime(published_at) < {cutoff}"
            ),
            [],
        )?;
        // OPT-013 B2：清 AI 缓存必须同时带走分段进度（清除使签名失效的
        // 持久化侧落实——片段绑定签名，留着会让"已清除"的内容被恢复复用）。
        tx.execute(
            &format!(
                "DELETE FROM ai_segments WHERE article_id IN (
                     SELECT id FROM articles WHERE datetime(published_at) < {cutoff})"
            ),
            [],
        )?;
        // 搜索候选索引由 trg_articles_trigram_au（UPDATE OF ai_summary/translated_content）
        // 同步；旧 articles_fts 已在 v29 删除（本语句不再是「FTS 维护点」）
    }
    tx.commit()?;
    Ok((deleted, ai_cleared))
}

/// OPT-013 B3：按「条目 × 产物」清除 AI 缓存（真实消费者「清除」入口的持久化侧）。
///
/// 契约（B3 定版「交付 2：三入口真实接线」）：
/// - 清产物列与签名列（同一条 UPDATE，无「新产物 + 旧签名」半写形态）；
///   `ai_epoch + 1` 推进清除代次，使网络在途的迟到提交被
///   [`set_article_ai_fields_checked`] 拒绝——清除后不得被旧请求复活；
/// - 删除该条目该产物在**所有签名**下的持久化片段（后台恢复记录）：清除后
///   「继续」不得复用任何旧片段（零恢复）；
/// - 只动指定产物的列：清 translation 不牵连 summary（反之亦然）。
///
/// 返回 `false` = 条目不存在（调用方如实报 not_found，不静默成功）。
/// Note: 清除语义与三入口 — 见 .agents/notes/implemented/architecture/2026-10-08-AI完成信号与缓存提交.md
/// 末节「OPT-013 B3 追加（2026-10-15）」。
pub fn clear_article_ai_product(
    conn: &mut Connection,
    article_id: i64,
    product: &str,
) -> AppResult<bool> {
    let tx = conn.transaction()?;
    let affected = match product {
        "summary" => tx.execute(
            "UPDATE articles SET ai_summary = NULL, ai_summary_sig = NULL,
                    ai_epoch = ai_epoch + 1
             WHERE id = ?1",
            params![article_id],
        )?,
        "translation" => tx.execute(
            "UPDATE articles SET translated_content = NULL, translated_sig = NULL,
                    ai_epoch = ai_epoch + 1
             WHERE id = ?1",
            params![article_id],
        )?,
        _ => {
            return Err(crate::error::AppError::new(
                "validate",
                "product 必须是 summary 或 translation",
            ))
        }
    };
    if affected == 0 {
        return Ok(false);
    }
    tx.execute(
        "DELETE FROM ai_segments WHERE article_id = ?1 AND product = ?2",
        params![article_id, product],
    )?;
    tx.commit()?;
    Ok(true)
}

/* ============================================================
写入与状态：文章入库（含智能去重）、已读/收藏、AI 字段、未读数
============================================================ */

/// 抓取/同步入库（带 feed_id）：guid 冲突或同 feed 同 URL 命中时仅更新内容字段
/// （正文/图片/enclosure/来源），已读/收藏/AI 产物等用户状态字段不动 —— 直连
/// 重抓到已读文章时不会"复活"它。匹配键用规范化 URL（url_norm）：跟踪参数/
/// www./m./协议/尾斜杠/AMP 差异不再产生同源重复文章。
///
/// OPT-008B 第一阶段（条目身份）：**入库不再跨源丢弃同 URL 条目**——条目是
/// 订阅源/远端 ID 的独立实体，智能去重从「入库丢身份」改为查询呈现层的聚合
/// 策略（见本文件 dedup 谓词；列表/计数/search/keyset/markAll 同一口径）。
/// `dedup` 参数保留为调用方接线兼容（scheduler/commands 仍传设置值），本函数
/// 不再消费它；旧 `deduped_urls` 内容墓碑表与 [`clear_dedup_tombstones`] 保留
/// 为有限兼容（008A 迁移已清空历史墓碑），但不再是入库闸门。
///
/// R3：本函数是 [`upsert_article_with_feed_tracked`] 的旧形状包装（只关心
/// id/是否新插入的调用方保持零改动）；需要「既有行是否实际改过可见字段」的
/// 调用方（sync 合并 / F11 数据变化标记）用 tracked 版本。
pub fn upsert_article_with_feed(
    conn: &Connection,
    feed_id: i64,
    a: &NewArticle,
    dedup: bool,
) -> AppResult<(i64, bool)> {
    let o = upsert_article_with_feed_tracked(conn, feed_id, a, dedup)?;
    Ok((o.id, o.inserted))
}

/// `upsert_article_with_feed` 的富结果（R3）：在 id/inserted 之外，报告既有行
/// UPDATE 是否**实际**改变了可见字段值（title/url/author/summary/正文/body/
/// 封面/enclosure/duration/published）。判定用 UPDATE 前后值比对（空串与 NULL
/// 归一为同值）——SQLite 的 `changes()` 对同值 UPDATE 也计命中行，不能当变化。
#[derive(Debug, PartialEq, Eq)]
pub struct ArticleUpsert {
    pub id: i64,
    pub inserted: bool,
    pub content_changed: bool,
}

/// 既有行可见字段快照（R3 前后比对用；空串与 NULL 归一——两者无可见差异）。
#[derive(Debug, PartialEq, Eq)]
struct VisibleFields {
    url: Option<String>,
    title: Option<String>,
    author: Option<String>,
    summary: Option<String>,
    content_html: Option<String>,
    body_text: Option<String>,
    image_url: Option<String>,
    enclosure_url: Option<String>,
    enclosure_mime: Option<String>,
    duration_sec: Option<i64>,
    published_at: Option<String>,
    /// OPT-008B 第二阶段：正文身份（重指/分叉在内容变化判定内，不只看字节相等）
    content_id: Option<i64>,
}

/// 读既有行的可见字段快照；读取失败透传 Err（不得吞成「无变化」）。
/// 文本字段经 `null_if_empty` 归一（空串 ≡ NULL，见 db.rs）。
/// OPT-008B 第二阶段：正文两列从 contents JOIN 读取（单一正文所有权）。
fn visible_fields(conn: &Connection, id: i64) -> AppResult<VisibleFields> {
    let f = conn.query_row(
        "SELECT a.url, a.title, a.author, a.summary, c.content_html, c.body_text, a.image_url,
                a.enclosure_url, a.enclosure_mime, a.duration_sec, a.published_at, a.content_id
         FROM articles a LEFT JOIN contents c ON c.id = a.content_id WHERE a.id = ?1",
        params![id],
        |r| {
            Ok(VisibleFields {
                url: null_if_empty(r.get(0)?),
                title: null_if_empty(r.get(1)?),
                author: null_if_empty(r.get(2)?),
                summary: null_if_empty(r.get(3)?),
                content_html: null_if_empty(r.get(4)?),
                body_text: null_if_empty(r.get(5)?),
                image_url: null_if_empty(r.get(6)?),
                enclosure_url: null_if_empty(r.get(7)?),
                enclosure_mime: null_if_empty(r.get(8)?),
                duration_sec: r.get(9)?,
                published_at: null_if_empty(r.get(10)?),
                content_id: r.get(11)?,
            })
        },
    )?;
    Ok(f)
}

/// R3 核心：返回富结果的 upsert。匹配逻辑与旧入口完全同源（本函数即原实现，
/// 旧入口为薄包装）——禁止在别处再写第二份 URL/guid 匹配。
///
/// OPT-008B：`dedup` 参数保留为接线兼容但不再决定入库（跨源同 URL 一律独立
/// 成行）；同源匹配（guid → 同 feed url_norm）保持原语义不变。
pub fn upsert_article_with_feed_tracked(
    conn: &Connection,
    feed_id: i64,
    a: &NewArticle,
    dedup: bool,
) -> AppResult<ArticleUpsert> {
    // 兼容参数：智能去重的入库闸门已移除（见函数注释与
    // upsert_article_with_feed 的 OPT-008B 说明）。
    let _ = dedup;
    let existing: Option<(i64, bool)> = conn
        .query_row(
            "SELECT a.id, COALESCE(c.fulltext_extracted, 0)
             FROM articles a LEFT JOIN contents c ON c.id = a.content_id
             WHERE a.feed_id = ?1 AND a.guid = ?2",
            params![feed_id, a.guid],
            |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0)),
        )
        .optional()?;
    if let Some((id, ft_extracted)) = existing {
        // R3：既有行先抓可见字段快照，UPDATE 后复读比对（同值 replay →
        // content_changed=false；不能拿 changes() 冒充变化）。
        let before = visible_fields(conn, id)?;
        refresh_existing_article(conn, id, a, ft_extracted, true)?;
        Ok(ArticleUpsert {
            id,
            inserted: false,
            content_changed: before != visible_fields(conn, id)?,
        })
    } else if let Some(url) = a.url.as_deref().filter(|u| !u.is_empty()) {
        // 同 feed 内按规范化 URL 兜底去重：direct 抓取与 Miniflux 同步的 guid 不同
        // （direct 用原始 guid，Miniflux 用 `miniflux-{id}`），但 URL 相同是同一篇。
        // 命中已有文章时只更新内容（正文/标题/封面/附件），**保留 source 与状态**
        // （is_read/is_starred/remote_id）——抓取只负责内容、同步只负责状态，
        // 避免「切换本地抓取后数量翻倍」与「状态被抓取覆盖回未读」。
        let by_url: Option<(i64, bool)> = conn
            .query_row(
                "SELECT a.id, COALESCE(c.fulltext_extracted, 0)
                 FROM articles a LEFT JOIN contents c ON c.id = a.content_id
                 WHERE a.feed_id = ?1 AND a.url_norm = ?2 ORDER BY a.id LIMIT 1",
                params![feed_id, normalize_url(url)],
                |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0)),
            )
            .optional()?;
        if let Some((id, ft_extracted)) = by_url {
            // R3：同 guid 分支——前后快照比对给出 content_changed。
            let before = visible_fields(conn, id)?;
            refresh_existing_article(conn, id, a, ft_extracted, false)?;
            Ok(ArticleUpsert {
                id,
                inserted: false,
                content_changed: before != visible_fields(conn, id)?,
            })
        } else {
            let (id, inserted) = insert_new_article(conn, feed_id, a)?;
            Ok(ArticleUpsert {
                id,
                inserted,
                content_changed: false,
            })
        }
    } else {
        let (id, inserted) = insert_new_article(conn, feed_id, a)?;
        Ok(ArticleUpsert {
            id,
            inserted,
            content_changed: false,
        })
    }
}

/// 既有行内容刷新（guid 命中与 URL 命中两分支共用；R3 全文保护、正文冻结与
/// 「空串 ≡ NULL」归一由本函数承担，调用方负责前后快照比对）。
/// `include_url=true` 时同语句刷新 url/url_norm（guid 命中分支保留「原始 url
/// 只在有值时覆盖」的既有语义）；URL 命中分支不触碰 url（避免把已有原始链接
/// 换成远端饰词形态）。
fn refresh_existing_article(
    conn: &Connection,
    id: i64,
    a: &NewArticle,
    _ft_extracted: bool,
    include_url: bool,
) -> AppResult<()> {
    /// 条目当前指向的内容快照（正文保护判定用）：
    /// (content_id, extraction_source, fulltext_extracted)。正文本身不进快照——
    /// 保护时只经 contents::repair_extracted_body_if_mismatch 就地修复。
    type CurrentContent = (Option<i64>, String, i64);
    // OPT-008B 第二阶段：**正文保护上移到 contents**——条目当前指向 readability
    // 提取行（source='readability' 且 fulltext_extracted=1）时，RSS 刷新不得覆盖/
    // 重指该内容（既定保护，旧 articles.fulltext_extracted 语义等价迁移）；否则
    // 确保 RSS 变体行（内容等价则复用同一行——跨源同文共享的目标路径）并按需把
    // 条目重指过去（FTS 触发器随 content_id 更新：每个引用条目的索引都保持正确）。
    let cur: Option<CurrentContent> = conn
        .query_row(
            "SELECT a.content_id, COALESCE(c.extraction_source, ''), COALESCE(c.fulltext_extracted, 0)
             FROM articles a LEFT JOIN contents c ON c.id = a.content_id WHERE a.id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)?)),
        )
        .optional()?;
    let (cur_id, cur_source, cur_extracted) = cur.unwrap_or((None, String::new(), 0));
    let protected = cur_source == super::contents::SOURCE_READABILITY && cur_extracted != 0;
    if protected {
        // OPT-003 R1（历史错配修复，二期迁移到此保持同一用户保证）：flag1 行的
        // body_text 与保留 HTML 不同源（旧版只写 HTML+flag 留下的错配）时按 HTML
        // 重算修复；修复随 revision+1 暴露（前端修订失效）并经内容触发器更新 FTS。
        // R3：修复单点收敛到 contents::repair_extracted_body_if_mismatch（与
        // sync_map 回填路径共用同一语义）。
        if let Some(cid) = cur_id {
            super::contents::repair_extracted_body_if_mismatch(conn, cid)?;
        }
    }
    if !protected && super::contents::content_non_empty(a.content_html.as_deref(), &a.body_text) {
        let input = super::contents::ContentInput {
            content_html: a.content_html.clone(),
            body_text: a.body_text.clone(),
            extraction_source: super::contents::SOURCE_RSS.to_string(),
            fulltext_extracted: false,
            url_norm: a.url.as_deref().map(normalize_url),
        };
        /* R3（内容重指故障原子性）：ensure_content 的 INSERT 与条目重指/旧行回收
        必须同生共死——UPDATE 失败（外键/触发器等）时整体回滚，不留无引用
        新行；SAVEPOINT 兼容调用方已有事务（sync 入库路径）。 */
        super::contents::with_write_savepoint(conn, "contents_refresh_repoint", || {
            let (new_id, _rev, _created) = super::contents::ensure_content(conn, &input, Some(id))?;
            if Some(new_id) != cur_id {
                conn.execute(
                    "UPDATE articles SET content_id = ?1 WHERE id = ?2",
                    params![new_id, id],
                )?;
                /* 重指后旧行若无引用：回收（「最后引用才回收」——否则每次内容替换都会
                留下孤儿行；触发器只在条目删除时兜底）。 */
                if let Some(old_id) = cur_id {
                    conn.execute(
                        "DELETE FROM contents WHERE id = ?1
                         AND NOT EXISTS (SELECT 1 FROM articles a WHERE a.content_id = ?1)",
                        params![old_id],
                    )?;
                }
            }
            Ok(())
        })?;
    }
    // 条目级字段（正文列已不在 articles；其余同旧语义：非空才覆盖/COALESCE 保护）。
    // guid 与规范化 URL 两条命中路径同口径。
    if include_url {
        conn.execute(
            "UPDATE articles SET
                url = COALESCE(?2, url),
                url_norm = CASE WHEN ?2 IS NOT NULL AND ?2 != '' THEN ?11 ELSE url_norm END,
                title = ?3,
                author = COALESCE(?4, author),
                summary = COALESCE(?5, summary),
                image_url = COALESCE(?6, image_url),
                enclosure_url = COALESCE(?7, enclosure_url),
                enclosure_mime = COALESCE(?8, enclosure_mime),
                duration_sec = COALESCE(?9, duration_sec),
                published_at = COALESCE(?10, published_at)
             WHERE id = ?1",
            params![
                id,
                a.url,
                a.title,
                a.author,
                a.summary,
                a.image_url,
                a.enclosure_url,
                a.enclosure_mime,
                a.duration_sec,
                a.published_at,
                a.url.as_deref().map(normalize_url),
            ],
        )?;
    } else {
        // OPT-003 全文保护：guid 变化但规范化 URL 相同的重放分支，已提取全文的
        // 条目同样保持指向提取内容，只用 RSS 更新元数据。
        conn.execute(
            "UPDATE articles SET
                title = ?1,
                author = COALESCE(?2, author),
                summary = COALESCE(?3, summary),
                image_url = COALESCE(?4, image_url),
                enclosure_url = COALESCE(?5, enclosure_url),
                enclosure_mime = COALESCE(?6, enclosure_mime),
                duration_sec = COALESCE(?7, duration_sec),
                published_at = COALESCE(?8, published_at)
             WHERE id = ?9",
            params![
                a.title,
                a.author,
                a.summary,
                a.image_url,
                a.enclosure_url,
                a.enclosure_mime,
                a.duration_sec,
                a.published_at,
                id,
            ],
        )?;
    }
    Ok(())
}

/// 同步条目入库（OPT-008B R1）：**精确协议 entry 的独立身份**入库。
///
/// 与 [`upsert_article_with_feed_tracked`] 的唯一差别在 URL 兜底守卫：
/// 同 feed 同 `url_norm` 的候选行**已绑定另一个精确 entry（remote_id != eid）**
/// 时不得复用/改写该行——精确协议 ID 必须保有自己身份（各自绑定/读藏），本
/// entry 插入独立新行（guid = remote-{eid}）。候选未绑定（直连行首绑 /
/// 快照外竞态行）时保持「URL 稳定复用」的既有语义（直连 guid 变化、URL 一致
/// → 同一行）。
///
/// 依赖 `remote_id` 唯一约束：调用前该 eid 必未绑定任何行（同 eid 已绑定的行
/// 走精确命中路径，不到这里）；否则会撞 UNIQUE 并透传错误（宁可失败可见，
/// 不做静默改写）。
pub fn upsert_remote_article(
    conn: &Connection,
    feed_id: i64,
    a: &NewArticle,
    eid: Option<i64>,
) -> AppResult<ArticleUpsert> {
    let existing: Option<(i64, bool)> = conn
        .query_row(
            "SELECT a.id, COALESCE(c.fulltext_extracted, 0)
             FROM articles a LEFT JOIN contents c ON c.id = a.content_id
             WHERE a.feed_id = ?1 AND a.guid = ?2",
            params![feed_id, a.guid],
            |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0)),
        )
        .optional()?;
    if let Some((id, ft_extracted)) = existing {
        let before = visible_fields(conn, id)?;
        refresh_existing_article(conn, id, a, ft_extracted, true)?;
        return Ok(ArticleUpsert {
            id,
            inserted: false,
            content_changed: before != visible_fields(conn, id)?,
        });
    }
    if let Some(url) = a.url.as_deref().filter(|u| !u.is_empty()) {
        let by_url: Option<(i64, bool, Option<i64>)> = conn
            .query_row(
                "SELECT a.id, COALESCE(c.fulltext_extracted, 0), a.remote_id
                 FROM articles a LEFT JOIN contents c ON c.id = a.content_id
                 WHERE a.feed_id = ?1 AND a.url_norm = ?2 ORDER BY a.id LIMIT 1",
                params![feed_id, normalize_url(url)],
                |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0, r.get(2)?)),
            )
            .optional()?;
        if let Some((id, ft_extracted, bound)) = by_url {
            if bound.is_none() || bound == eid {
                let before = visible_fields(conn, id)?;
                refresh_existing_article(conn, id, a, ft_extracted, false)?;
                return Ok(ArticleUpsert {
                    id,
                    inserted: false,
                    content_changed: before != visible_fields(conn, id)?,
                });
            }
            // 候选行属于另一个精确 entry：不复用——独立新行（下方 insert）。
        }
    }
    let (id, inserted) = insert_new_article(conn, feed_id, a)?;
    Ok(ArticleUpsert {
        id,
        inserted,
        content_changed: false,
    })
}

/// 插入新文章（upsert_article_with_feed 的兜底路径）。
/// OPT-008B 第二阶段：先插条目行（content_id 暂空）再确保内容行（空内容按新
/// 条目 id 走私有身份——不聚合、不误并），最后回填 content_id（FTS 触发器随
/// content_id 更新写索引）。
/// R3（故障原子性）：三步在**同一 SAVEPOINT** 内——任一步失败整体回滚，极端
/// 崩溃窗口不再留下「content_id NULL 且 FTS 正文为空」的半插入条目。
fn insert_new_article(conn: &Connection, feed_id: i64, a: &NewArticle) -> AppResult<(i64, bool)> {
    super::contents::with_write_savepoint(conn, "contents_insert_article", || {
        // M-5：published_at 写入兜底——程序路径不允许产生 NULL（v14 触发器再兜住
        // 裸 SQL 路径），使「published_at 非空」不变量成立，列表排序可退化为纯
        // ORDER BY published_at（与旧 COALESCE 口径逐行等价，见 migrations v14 注释）。
        conn.execute(
            "INSERT INTO articles
                (feed_id, guid, url, url_norm, title, author, summary, image_url,
                 enclosure_url, enclosure_mime, duration_sec, published_at, source)
             VALUES (?1, ?2, ?3, ?13, ?4, ?5, ?6, ?7, ?8, ?9, ?10, COALESCE(?11, datetime('now')), ?12)",
            params![
                feed_id,
                a.guid,
                a.url,
                a.title,
                a.author,
                a.summary,
                a.image_url,
                a.enclosure_url,
                a.enclosure_mime,
                a.duration_sec,
                a.published_at,
                a.source,
                a.url.as_deref().map(normalize_url)
            ],
        )?;
        let id = conn.last_insert_rowid();
        let input = super::contents::ContentInput {
            content_html: a.content_html.clone(),
            body_text: a.body_text.clone(),
            extraction_source: super::contents::SOURCE_RSS.to_string(),
            fulltext_extracted: false,
            url_norm: a.url.as_deref().map(normalize_url),
        };
        let (content_id, _rev, _created) = super::contents::ensure_content(conn, &input, Some(id))?;
        conn.execute(
            "UPDATE articles SET content_id = ?1 WHERE id = ?2",
            params![content_id, id],
        )?;
        Ok((id, true))
    })
}

/* ============================================================
OPT-007（F09/F10）：本地读/藏状态写入的**字段版本**内核

pull 合并以「pull 起点版本快照 + 当前 DB 版本 + 该字段 pending」为守卫决定
是否接受远端字段（sync/conflict_policy.rs 的 field_accepts_remote）。要让
该守卫成立，本地每一次读/藏写入都必须在**同一 SQL 事务**里把对应版本 +1：

- 单条 / 批量 / mark_all_read 三组入口都经这里递增（checked 增长：到
  i64::MAX 时 Err，绝不饱和——饱和会让「版本相等」把本地新写入误判成
  「拉取期间无变化」，旧网络快照即可回滚用户刚下的意图）；
- 远端合并**不经过**本内核：写侧用 db/sync_map.rs 的带版本条件更新
  （WHERE 版本 = 快照）二次校验，版本不符即 0 行跳过该字段；
- read / star 两列独立递增：读的本地写入不会压住无冲突的收藏合并，反之亦然。
============================================================ */

/// 把指定版本列 +1（调用方须已处于包含状态写入的事务/语句内）。
/// 文章不存在时保持旧语义（纯 SET 0 行、不报错）——不在这里引入新的错误面。
fn bump_article_version(conn: &Connection, id: i64, column: &str) -> AppResult<()> {
    debug_assert!(column == "read_version" || column == "star_version");
    let current: Option<i64> = conn
        .query_row(
            &format!("SELECT {column} FROM articles WHERE id = ?1"),
            params![id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(current) = current else {
        return Ok(());
    };
    let next = current.checked_add(1).ok_or_else(|| {
        crate::error::AppError::new(
            "protocol",
            format!("{column} 溢出（文章 id={id} 已达 i64::MAX）：拒绝本地状态写入"),
        )
    })?;
    let n = conn.execute(
        &format!("UPDATE articles SET {column} = ?1 WHERE id = ?2 AND {column} = ?3"),
        params![next, id, current],
    )?;
    if n == 0 {
        return Err(crate::error::AppError::new(
            "db",
            format!("{column} 递增失败（文章 id={id} 版本在写入窗口内变化）"),
        ));
    }
    Ok(())
}

pub fn set_read(conn: &Connection, id: i64, read: bool) -> AppResult<()> {
    bump_article_version(conn, id, "read_version")?;
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

/* ============================================================
OPT-008B 第二阶段：异步产物提交守卫（全文提取 / AI 生成）

网络期间不持 DB 锁：发起前读 (content_id, revision)（AI 另加 ai_epoch 清除代次）
作为提交前提；落地时在**单条 UPDATE 的 WHERE 内**逐项核对——迟到提交不得写到
已删除/已分叉/已替换的条目或内容上，也不得复活清理后的旧产物。
`(content_id, contents.revision)` 即交付给 013 复用的稳定内容版本接口。
============================================================ */

/// AI 提交前提（生成前捕获；提交时校验）。
///
/// OPT-013 B1：加入 `title` —— 签名输入含标题，提交时必须与生成时同标题，
/// 否则会出现「新产物 + 旧签名」（标题变化后产物是旧标题下生成的）。正文输入
/// 由 (content_id, revision) 覆盖（内容的每次更新都推进 revision）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiCommitToken {
    pub ai_epoch: i64,
    pub content_id: Option<i64>,
    pub content_revision: i64,
    pub title: String,
}

/// 读某条目的提交前提（不存在返回 None）。
pub fn article_ai_commit_token(conn: &Connection, id: i64) -> AppResult<Option<AiCommitToken>> {
    let t = conn
        .query_row(
            "SELECT a.ai_epoch, a.content_id, COALESCE(c.revision, 0), a.title
             FROM articles a LEFT JOIN contents c ON c.id = a.content_id
             WHERE a.id = ?1",
            params![id],
            |r| {
                Ok(AiCommitToken {
                    ai_epoch: r.get(0)?,
                    content_id: r.get(1)?,
                    content_revision: r.get(2)?,
                    title: r.get(3)?,
                })
            },
        )
        .optional()?;
    Ok(t)
}

/// AI 产物提交载荷：**产物与缓存签名成对**——两者必须在同一条 UPDATE 内写入，
/// 不允许出现「新产物 + 旧签名」或「新签名 + 旧产物」（OPT-013 B1 定版 §1）。
///
/// Note: 提交前提与签名身份 — 见 .agents/notes/implemented/architecture/2026-10-08-AI完成信号与缓存提交.md
/// 末节「OPT-013 B1 追加（2026-10-15）」。
#[derive(Debug, Clone, Copy)]
pub enum AiProduct<'a> {
    Summary { text: &'a str, signature: &'a str },
    Translation { text: &'a str, signature: &'a str },
}

/// 带前提校验的 AI 产物落库：任一前提不符（条目已删/已清/内容已替换/已分叉/
/// 标题已变）返回 `false`（调用方如实提示"内容已更新，已丢弃过期产物"，不写入）。
/// `AiProduct` 的签名与产物同一条 UPDATE 落库——部分失败整体失败，无半写形态。
pub fn set_article_ai_fields_checked(
    conn: &Connection,
    id: i64,
    expected: &AiCommitToken,
    product: AiProduct<'_>,
) -> AppResult<bool> {
    // WHERE 即「签名前提」的复核：ai_epoch（清除代次）、content_id/revision
    // （正文身份与修订——正文哈希由修订唯一决定）、title（签名输入之一）。
    let n = match product {
        AiProduct::Summary { text, signature } => conn.execute(
            "UPDATE articles SET
                ai_summary = ?1,
                ai_summary_sig = ?2
             WHERE id = ?3
               AND ai_epoch = ?4
               AND COALESCE(content_id, -1) = COALESCE(?5, -1)
               AND COALESCE((SELECT revision FROM contents WHERE id = content_id), 0) = ?6
               AND title = ?7",
            params![
                text,
                signature,
                id,
                expected.ai_epoch,
                expected.content_id,
                expected.content_revision,
                expected.title
            ],
        )?,
        AiProduct::Translation { text, signature } => conn.execute(
            "UPDATE articles SET
                translated_content = ?1,
                translated_sig = ?2
             WHERE id = ?3
               AND ai_epoch = ?4
               AND COALESCE(content_id, -1) = COALESCE(?5, -1)
               AND COALESCE((SELECT revision FROM contents WHERE id = content_id), 0) = ?6
               AND title = ?7",
            params![
                text,
                signature,
                id,
                expected.ai_epoch,
                expected.content_id,
                expected.content_revision,
                expected.title
            ],
        )?,
    };
    Ok(n > 0)
}

/// 全文提取的提交前提（发起前捕获：正文原文 + 内容身份/修订）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentCommitToken {
    pub content_id: Option<i64>,
    pub content_revision: i64,
}

pub fn article_content_commit_token(
    conn: &Connection,
    id: i64,
) -> AppResult<Option<(ContentCommitToken, String)>> {
    let row = conn
        .query_row(
            "SELECT a.content_id, COALESCE(c.revision, 0), COALESCE(c.content_html, '')
             FROM articles a LEFT JOIN contents c ON c.id = a.content_id
             WHERE a.id = ?1",
            params![id],
            |r| {
                Ok((
                    ContentCommitToken {
                        content_id: r.get(0)?,
                        content_revision: r.get(1)?,
                    },
                    r.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    Ok(row)
}

/// 全文提取落库（带前提校验）：提取结果是 `readability` 来源的内容变体——
/// 保证存在 (readability, 提取 HTML, 同源纯文本) 的 contents 行（无则建、有则
/// 复用，多条目提取同一网页即共享），条目重指该行；旧行若无引用即回收。
/// 前提不符（期间内容被替换/RSS 重指/条目被删）→ `None`，迟到提取不落库。
///
/// 成功返回**提交后的实际内容身份/修订** `(content_id, revision)`（R3：随
/// ExtractOutcome 回传给前端落记录——下一次 reload 不再把刚提取的正文按
/// “修订变化”清空重取；也供调用方核对提交结果与 DB 一致）。
/// R3（故障原子性）：ensure_content 的 INSERT、条目重指、旧行回收在同一
/// SAVEPOINT 内——重指失败整体回滚，不留无引用新行（所有回收点都是引用驱动的，
/// 半提交孤儿不会有任何后期清理入口）。
pub fn replace_article_content_checked(
    conn: &Connection,
    article_id: i64,
    expected: &ContentCommitToken,
    new_html: &str,
) -> AppResult<Option<(i64, i64)>> {
    let now = article_content_commit_token(conn, article_id)?;
    let Some((current, _html)) = now else {
        return Ok(None); // 条目已删除：迟到提取丢弃
    };
    if current != *expected {
        return Ok(None); // 内容已被替换/重指：迟到提取丢弃（不得写分叉）
    }
    let body_text = crate::sanitize::html_to_text(new_html);
    let input = super::contents::ContentInput {
        content_html: Some(crate::sanitize::sanitize(new_html, None)),
        body_text,
        extraction_source: super::contents::SOURCE_READABILITY.to_string(),
        fulltext_extracted: true,
        url_norm: None,
    };
    let committed = super::contents::with_write_savepoint(conn, "contents_extract_commit", || {
        let (new_content_id, rev, _created) =
            super::contents::ensure_content(conn, &input, Some(article_id))?;
        if Some(new_content_id) != current.content_id {
            conn.execute(
                "UPDATE articles SET content_id = ?1 WHERE id = ?2",
                params![new_content_id, article_id],
            )?;
            // 旧行若已无引用：回收（条目级 delete 触发器之外的显式路径）。
            if let Some(old_id) = current.content_id {
                conn.execute(
                    "DELETE FROM contents WHERE id = ?1
                     AND NOT EXISTS (SELECT 1 FROM articles a WHERE a.content_id = ?1)",
                    params![old_id],
                )?;
            }
        }
        Ok((new_content_id, rev))
    })?;
    Ok(Some(committed))
}

pub fn set_starred(conn: &Connection, id: i64, starred: bool) -> AppResult<()> {
    bump_article_version(conn, id, "star_version")?;
    conn.execute(
        "UPDATE articles SET is_starred = ?1 WHERE id = ?2",
        params![starred as i64, id],
    )?;
    Ok(())
}

/* ============================================================
TASK-108：状态写入 + 同步入队 的复合事务（同生共死，REQ-002）
============================================================ */

/* OPT-008B R2（N1）：读写入回执——读闭包（主行 ∪ 同 url_norm 未读副本）一次
可能翻转多行，命令层把**真实翻转行**（操作前未读）回执给前端；前端据此让
受影响条目/视图缓存/按源计数本地收敛，不依赖全量 reload（也无需计数对账
覆盖条目；布局无关的源键可按回执逐行扣减）。
语义边界：`id` = 翻转行 id，`feed_id` = 该行所属源（回执行与持久层同源，
前端不做二次查询）；wire 形态为 `[{id, feed_id}]`。 */
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct ReadWriteReceipt {
    pub id: i64,
    pub feed_id: i64,
}

// Note: 状态写入与同步入队必须同事务；批量路径全有全无 — 见 .agents/notes/implemented/bug-fix/2026-09-18-状态写入事务化与对账守卫.md
/// 已读状态「写入 + 入队」的事务内核（TASK-108）：调用方负责事务边界——
/// 单条入口 [`set_read_with_enqueue`] 与批量入口 [`set_read_bulk_with_enqueue`]
/// 各自开事务后调用本函数。语句顺序与参数同 commands 层既有逐条提交形态
/// 完全一致（先写状态后入队，action 名 `read`/`unread`），事务化只改变失败时
/// 的原子性（任一步失败整体回滚），成功路径逐列语义不变。
fn set_read_enqueue_in_tx(
    conn: &Connection,
    id: i64,
    read: bool,
) -> AppResult<Vec<ReadWriteReceipt>> {
    if read {
        // OPT-008B R1（读广播的本地半边）：用户标读入口 = 当前行 ∪ 同 `url_norm`
        // 未读副本——全部标读 + 版本递增 + 入队 `read`，与推送侧 read 广播
        // （同组 remote_id 展开）同政策、同集合。unread 不广播（下方直通单行）。
        // 副本只在「未读」时处理（已读副本无变化可记，避免版本噪音与重复入队）；
        // 主行始终按既有语义写入（即使已读，仍表达一次本地意图）。
        // R2（N1）：返回**真实翻转行**回执（操作前未读的行），供前端把被闭包
        // 影响但不在被点位置上的条目与计数本地收敛（无需全量 reload）。
        let mut receipts = Vec::new();
        for (target, feed_id, was_unread) in read_targets_with_state(conn, id)? {
            set_read(conn, target, true)?;
            enqueue_sync(conn, Some(target), None, "read", None)?;
            if was_unread {
                receipts.push(ReadWriteReceipt {
                    id: target,
                    feed_id,
                });
            }
        }
        return Ok(receipts);
    }
    let was_read: bool = conn
        .query_row(
            "SELECT is_read FROM articles WHERE id = ?1",
            rusqlite::params![id],
            |r| Ok(r.get::<_, i64>(0)? != 0),
        )
        .optional()?
        .unwrap_or(false);
    let feed_id: i64 = conn
        .query_row(
            "SELECT feed_id FROM articles WHERE id = ?1",
            rusqlite::params![id],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(-1);
    set_read(conn, id, false)?;
    enqueue_sync(conn, Some(id), None, "unread", None)?;
    Ok(if was_read && feed_id >= 0 {
        vec![ReadWriteReceipt { id, feed_id }]
    } else {
        Vec::new()
    })
}

// Note: 读闭包查询计划由 v30 部分索引 `idx_articles_url_norm_unread`
// （`(url_norm) WHERE is_read = 0`）支撑（等值前缀 + 免排序覆盖扫描）；索引缺失
// 或损坏时回落旧计划（`idx_articles_unread` 全未读扫描），结果集不变 — 见
// docs/performance.md「OPT-017G」与 .agents/notes/implemented/architecture/
// 2026-10-12-订阅条目与共享内容身份.md 的「OPT-017G 追加」。
/// 读闭包目标查询：同 `url_norm` 且未读的兄弟行（排除自身），按 id 升序。
/// 提取为常量使 EXPLAIN 计划回归测试与生产**同字节**（`opt017g_read_closure_tests`
/// 直接复用本常量）——测试不得另抄一份 SQL。
pub(crate) const READ_CLOSURE_SQL: &str = "SELECT id FROM articles
             WHERE id != ?1 AND url_norm = ?2 AND is_read = 0
             ORDER BY id";

/// 读目标行随状态：`(id, feed_id, 操作前是否未读)`。读闭包 = 主行 + 同组未读副本。
fn read_targets_with_state(conn: &Connection, id: i64) -> AppResult<Vec<(i64, i64, bool)>> {
    let norm: Option<String> = conn
        .query_row(
            "SELECT url_norm FROM articles WHERE id = ?1",
            rusqlite::params![id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    let mut targets: Vec<i64> = vec![id];
    if let Some(norm) = norm {
        let mut stmt = conn.prepare(READ_CLOSURE_SQL)?;
        let rows = stmt.query_map(rusqlite::params![id, norm], |r| r.get::<_, i64>(0))?;
        targets.extend(rows.collect::<Result<Vec<_>, _>>()?);
    }
    let mut out = Vec::with_capacity(targets.len());
    for tid in targets {
        let (feed_id, is_read): (i64, i64) = conn.query_row(
            "SELECT feed_id, is_read FROM articles WHERE id = ?1",
            rusqlite::params![tid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        out.push((tid, feed_id, is_read == 0));
    }
    Ok(out)
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
/// R2（N1）：返回本次**真实翻转行**回执（含被读闭包一并标读的同组副本）。
pub fn set_read_with_enqueue(
    conn: &Connection,
    id: i64,
    read: bool,
) -> AppResult<Vec<ReadWriteReceipt>> {
    let tx = conn.unchecked_transaction()?;
    let receipts = set_read_enqueue_in_tx(&tx, id, read)?;
    tx.commit()?;
    Ok(receipts)
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
/// R2（N1）：返回整批**真实翻转行**回执（去重后按 id 升序，含闭包副本）。
pub fn set_read_bulk_with_enqueue(
    conn: &Connection,
    ids: &[i64],
    read: bool,
) -> AppResult<Vec<ReadWriteReceipt>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let tx = conn.unchecked_transaction()?;
    let mut seen = std::collections::HashSet::new();
    let mut receipts = Vec::new();
    for id in ids {
        for r in set_read_enqueue_in_tx(&tx, *id, read)? {
            if seen.insert(r.id) {
                receipts.push(r);
            }
        }
    }
    tx.commit()?;
    receipts.sort_by_key(|r| r.id);
    Ok(receipts)
}

/// 查询「无封面 + 有原文 URL」的文章 id（封面后台补全用），来源不限。
/// 摘要型 RSS（少数派等）不带 media 字段、服务端文章正文也可能没有图片，封面
/// 只能从文章页 og:image 拿；limit 限制单轮批处理量（避免启动时一次性扫全库 +
/// 轰炸源站），offset 供调用方推进候选窗口（REQ-106①）。
///
/// OPT-017D（覆盖面）：此前只选 `source='direct'`，理由是「Miniflux 源入库时已
/// 用正文第一图兜底」——但该兜底只覆盖**正文里有图**的服务端文章；正文无图的
/// 服务端文章此前既没有封面、又永不进入补全队列，形成永久缺口（真实库实测见
/// 归属 Note）。旧理由中「本地补的封面会被同步覆盖回空」也不成立：两处封面
/// 写入都不会把本地封面清成空——[`crate::db::backfill_article_content`] 是
/// `COALESCE(旧值, 新值)` 严格只填空；`refresh_existing_article` 是
/// `COALESCE(新值, 旧值)`（远端给出非空新图会覆盖本地值，NULL 保留本地值，
/// R1-P2-1 修正）。因此候选不再按 source 过滤。
///
/// 边界保留：失效上报通道 [`clear_article_cover_if_matches`] 仍只清
/// `source='direct'` 的封面（保守语义，OPT-017D 明确不改）——服务端文章本地
/// 补上的封面若失效，走前端占位而不回清。
// Note: 覆盖面扩展与请求预算的完整理由 — 见 .agents/notes/implemented/feature/2026-10-16-封面补全覆盖面与预算边界.md
pub fn articles_without_cover(
    conn: &Connection,
    limit: i64,
    offset: i64,
) -> AppResult<Vec<(i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT id, url FROM articles
         WHERE (image_url IS NULL OR image_url = '')
           AND url IS NOT NULL AND url != ''
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
/// source='direct' 是**有意保留的保守边界**（OPT-017D 明确不改）：服务端文章的
/// 封面字段不由客户端失效通道改写。注意它不再等价于「服务端文章没有本地补的
/// 封面」——候选查询已扩展到全来源（见 [`articles_without_cover`]），服务端文章
/// 本地补上的封面若失效，前端走失败占位，不会被清空重试。
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

/// 读目标集合（OPT-008B R1）：范围未读行 ∪ 这些行的同 `url_norm` 组全部行。
///
/// 政策（主控裁决）：**显示去重不意味着操作只改代表**——read 传播的本地半边
/// 与推送广播同政策，同组 read 原子更新相应本地条目版本与队列；
/// unread/star 只当前条目（不经过本谓词）。
/// 谓词形如 `((1=1{scope}) OR url_norm IN (SELECT url_norm FROM articles
/// WHERE is_read = 0{scope}))`，外层语句另加 `is_read = 0`：seed 是范围未读、
/// 闭包是同组全部行，最终只有「未读且在读目标集合内」的行被改（幂等，重放
/// 不产生新变化）。两条 scope 绑定按文本序重复一次。
fn read_target_clause(
    feed_id: Option<i64>,
    folder_id: Option<i64>,
    starred_only: bool,
    since_ms: Option<i64>,
    layout: Option<&str>,
) -> (String, Vec<Box<dyn rusqlite::ToSql>>) {
    let (scope, mut binds) = scope_clause(feed_id, folder_id, starred_only, since_ms, layout);
    let (_, inner_binds) = scope_clause(feed_id, folder_id, starred_only, since_ms, layout);
    let clause = format!(
        "((1=1{scope}) OR (url_norm IS NOT NULL AND url_norm IN \
         (SELECT url_norm FROM articles WHERE is_read = 0{scope} AND url_norm IS NOT NULL)))"
    );
    binds.extend(inner_binds);
    (clause, binds)
}

/// 读目标集合的 id 列表（与 [`mark_all_read_with_enqueue`] 的 WHERE 同源；
/// 供等价性测试与「标读前收集」参照实现消费）。
pub fn list_read_target_ids_scoped(
    conn: &Connection,
    feed_id: Option<i64>,
    folder_id: Option<i64>,
    starred_only: bool,
    since_ms: Option<i64>,
    layout: Option<&str>,
) -> AppResult<Vec<i64>> {
    let (clause, binds) = read_target_clause(feed_id, folder_id, starred_only, since_ms, layout);
    let sql = format!("SELECT a.id FROM articles a WHERE a.is_read = 0 AND {clause} ORDER BY a.id");
    let refs: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(refs.as_slice(), |r| r.get(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 全部已读：作用于当前筛选范围（feed/folder/all），与前端「全部已读」按钮语义一致。
/// OPT-007：同步递增 read_version（该路径同样是「本地标读」入口，字段版本必须
/// 覆盖全部本地写入；checked 增长——范围内任一行已达 i64::MAX 时整条拒绝）。
/// OPT-008B R1：作用于**读目标集合**（范围未读 ∪ 同组闭包）——「全部已读」
/// 之后该范围未读真的为空，不依赖推送/拉取的远端收敛（direct 模式同样成立）。
pub fn mark_all_read(
    conn: &Connection,
    feed_id: Option<i64>,
    folder_id: Option<i64>,
    starred_only: bool,
    since_ms: Option<i64>,
    layout: Option<&str>,
) -> AppResult<usize> {
    let (clause, binds) = read_target_clause(feed_id, folder_id, starred_only, since_ms, layout);
    let refs: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
    ensure_read_version_can_grow(conn, &format!(" AND {clause}"), refs.as_slice())?;
    let sql = format!(
        "UPDATE articles AS a SET is_read = 1, read_version = read_version + 1 \
         WHERE a.is_read = 0 AND {clause}"
    );
    let n = conn.execute(&sql, refs.as_slice())?;
    Ok(n)
}

/// OPT-007：读目标集合内是否出现 read_version 饱和行（= i64::MAX 且将被本次标读）。
/// 饱和即 Err——拒绝饱和复用同一版本（否则「版本相等」判定失效；见 bump 内核注释）。
/// 检查与 UPDATE 在调用方同一事务内（mark_all_read_with_enqueue）或在单条语句
/// 之前（mark_all_read），单连接互斥下无并发缝隙。
fn ensure_read_version_can_grow(
    conn: &Connection,
    clause: &str,
    refs: &[&dyn rusqlite::ToSql],
) -> AppResult<()> {
    let sql = format!(
        "SELECT COUNT(*) FROM articles a WHERE a.is_read = 0{clause} AND a.read_version = {max}",
        max = i64::MAX
    );
    let saturated: i64 = conn.query_row(&sql, refs, |r| r.get(0))?;
    if saturated > 0 {
        return Err(crate::error::AppError::new(
            "protocol",
            format!("read_version 溢出（{saturated} 行已达 i64::MAX）：拒绝标读，版本不得饱和复用"),
        ));
    }
    Ok(())
}

/// 「全部已读」集合化（REQ-108 M-9）：一个事务内完成 入队 + 标读，语句数常数化。
///
/// 旧实现 = 1 SELECT（list_unread_ids_scoped 收集未读 id）+ 1 UPDATE + 每条 id
/// 一次 enqueue_sync（DELETE + INSERT）= 2N+2 条语句，且命令全程持库锁。
/// 这里固定 4 条语句（OPT-007 起补一条 read_version 饱和预检）：
///   ① 批量删除读目标行条目的 read/unread 旧队项（与逐条 enqueue 的「同向互斥
///      覆盖」语义一致：每个 id 至多保留一条最新 read 队项）；
///   ② `INSERT ... SELECT` 为读目标集合内的未读条目入 `read` 队项（feed_url/payload
///      为 NULL，与 `enqueue_sync(conn, Some(id), None, "read", None)` 逐列相同）；
///   ③ read_version 饱和预检（目标集合内有行到 i64::MAX 即整条拒绝，见 bump 内核）；
///   ④ 单条 UPDATE 标读并同语句递增 read_version，返回受影响行数。
/// ①② 与 ④ 共用同一 [`read_target_clause`]（同一事务内求值两次即冻结集合——
/// 单连接互斥、无并发写入缝隙），「入队集合 = 标读集合 = 读目标集合」逐行相同
/// （F8 + R1 闭包，由等价性测试与主控探针回归锁定）；语句全在同一事务。
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
    let (clause, binds) = read_target_clause(feed_id, folder_id, starred_only, since_ms, layout);
    let refs: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
    let tx = conn.unchecked_transaction()?;
    let del = format!(
        "DELETE FROM sync_queue WHERE action IN ('read', 'unread')
           AND article_id IN
             (SELECT a.id FROM articles a WHERE a.is_read = 0 AND {clause})"
    );
    tx.execute(&del, refs.as_slice())?;
    let ins = format!(
        "INSERT INTO sync_queue (article_id, feed_url, action, payload, created_at)
         SELECT a.id, NULL, 'read', NULL, datetime('now')
           FROM articles a WHERE a.is_read = 0 AND {clause}"
    );
    tx.execute(&ins, refs.as_slice())?;
    // OPT-007：标读与 read_version 递增同语句同事务——「全部已读」也是本地
    // 标读入口，字段版本必须与状态写入原子一致（checked 增长，饱和拒绝）。
    ensure_read_version_can_grow(&tx, &format!(" AND {clause}"), refs.as_slice())?;
    let upd = format!(
        "UPDATE articles AS a SET is_read = 1, read_version = read_version + 1 \
         WHERE a.is_read = 0 AND {clause}"
    );
    let n = tx.execute(&upd, refs.as_slice())?;
    tx.commit()?;
    Ok(n)
}

/// 条目计数（侧边栏角标）：按 feed 聚合，含未读/收藏/今日细分。
/// 前端据此聚合分类/视图的精确计数——不依赖文章列表的分页 limit，
/// 保证数字准确（「全部/未读数字被 limit 截断」的根因修复）。
/// OPT-008B：smartDedup 开启时按**源 scope 内**的显示聚合计数（与「该源自己的
/// 列表」逐行一致；单源视图不被其它源的副本吞掉——「不能隐藏当前源」）。
/// 跨源聚合数字（全部/分类）用 [`scope_counts`] 取得同口径精确值。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FeedCounts {
    pub feed_id: i64,
    pub total: i64,
    pub unread: i64,
    pub starred: i64,
    pub today: i64,
}

pub fn feed_counts(conn: &Connection) -> AppResult<Vec<FeedCounts>> {
    feed_counts_with_dedup(conn, smart_dedup_on(conn))
}

/// [`feed_counts`] 的显式开关内核（R2/N2：`counts_snapshot` 在同一开关值下
/// 同时计算源计数与 scope 计数——两个数字来自同一次读取、同一口径）。
fn feed_counts_with_dedup(conn: &Connection, dedup: bool) -> AppResult<Vec<FeedCounts>> {
    // TASK-064 N5：today 判定与 only_today 同口径（本地时区两侧对齐），理由见
    // list_articles 的 where 构建处。
    let base_sql = "SELECT a.feed_id, {total}, {unread}, {starred}, {today}
         FROM articles a GROUP BY a.feed_id";
    let sql = if dedup {
        // 源 scope 显示聚合：b.feed_id = a.feed_id（代表在源内选）。
        let ex = |scope: DedupScope| dedup_exists_clause(scope).0;
        let total = ex(DedupScope {
            feed: true,
            ..Default::default()
        });
        let unread = ex(DedupScope {
            feed: true,
            unread_only: true,
            ..Default::default()
        });
        let starred = ex(DedupScope {
            feed: true,
            starred_only: true,
            ..Default::default()
        });
        let today = ex(DedupScope {
            feed: true,
            today_only: true,
            ..Default::default()
        });
        format!(
            "SELECT a.feed_id, SUM({total}), SUM(a.is_read = 0 AND {unread}),
                    SUM(a.is_starred = 1 AND {starred}),
                    SUM(date(a.published_at, 'localtime') = date('now', 'localtime') AND {today})
             FROM articles a GROUP BY a.feed_id"
        )
    } else {
        base_sql
            .replace("{total}", "COUNT(*)")
            .replace("{unread}", "SUM(a.is_read = 0)")
            .replace("{starred}", "SUM(a.is_starred = 1)")
            .replace(
                "{today}",
                "SUM(date(a.published_at, 'localtime') = date('now', 'localtime'))",
            )
    };
    let mut stmt = conn.prepare(&sql)?;
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
OPT-008B 第一阶段：scope 精确计数（前端聚合数字的同口径来源）

`feed_counts` 是「每源自己的 scope」计数（与源自己的列表一致）；侧边栏
「全部 / 分类」角的数字是**另一个 scope 的显示集合**——跨源同 URL 的两行在
「全部」scope 里只算 1（代表），在各自源的 scope 里各算 1。要让每个数字都与
「点进去看到的列表」逐行一致，聚合数字必须按各自的 scope 计算，不能拿每源
数字相加（层级去重的计数不可由子集数字重建）。本函数用与
列表/搜索/markAll 同一谓词族在 SQL 内计算三个层级的计数。
============================================================ */

/// 单个 scope（全部/某分类/某源）的显示集合计数。
#[derive(Debug, Serialize, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScopeCounts {
    pub total: i64,
    pub unread: i64,
    pub starred: i64,
    pub today: i64,
}

/// 带 scope id 的计数行（folders/feeds 数组元素；scope_id = folder id / feed id）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScopedCounts {
    pub scope_id: i64,
    pub total: i64,
    pub unread: i64,
    pub starred: i64,
    pub today: i64,
}

/// `scope_counts` 的结果：全部 + 每分类 + 每源（当前布局维度过滤后）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScopeCountsDetail {
    pub all: ScopeCounts,
    pub folders: Vec<ScopedCounts>,
    pub feeds: Vec<ScopedCounts>,
}

#[derive(Clone, Copy, PartialEq)]
enum ScopeLevel {
    All,
    Folder,
    Feed,
}

#[derive(Clone, Copy)]
struct ScopedCountsRow {
    scope_id: i64,
    total: i64,
    unread: i64,
    starred: i64,
    today: i64,
}

fn counts_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ScopedCountsRow> {
    Ok(ScopedCountsRow {
        scope_id: r.get(0)?,
        total: r.get(1)?,
        unread: r.get::<_, Option<i64>>(2)?.unwrap_or(0),
        starred: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
        today: r.get::<_, Option<i64>>(4)?.unwrap_or(0),
    })
}

/// 生成某层级 scope 的计数 SQL + 绑定。
/// 占位符次序 = SQL 文本次序：SELECT 列表里的 4 个 EXISTS（各在 layout 给定时
/// 绑一次）在前，外层 WHERE 的 layout 绑定在后。
fn scope_counts_sql(
    level: ScopeLevel,
    dedup: bool,
    layout: Option<&str>,
) -> (String, Vec<rusqlite::types::Value>) {
    let b_layout = layout.is_some();
    let b_scope = match level {
        ScopeLevel::All => "",
        ScopeLevel::Feed => " AND b.feed_id = a.feed_id",
        ScopeLevel::Folder => " AND fb.folder_id = fo.id",
    };
    let b_layout_cond = if b_layout {
        " AND (CASE WHEN fb.layout != 'inherit' THEN fb.layout \
           ELSE (SELECT fo5.layout FROM folders fo5 WHERE fo5.id = fb.folder_id) END) = ?"
    } else {
        ""
    };
    let exists = |view: &str| {
        format!(
            "NOT EXISTS (SELECT 1 FROM articles b JOIN feeds fb ON fb.id = b.feed_id \
             WHERE b.url_norm = a.url_norm AND b.id < a.id{b_scope}{b_layout_cond}{view})"
        )
    };
    let (total, unread, starred, today) = if dedup {
        (
            format!("SUM({})", exists("")),
            format!("SUM(a.is_read = 0 AND {})", exists(" AND b.is_read = 0")),
            format!(
                "SUM(a.is_starred = 1 AND {})",
                exists(" AND b.is_starred = 1")
            ),
            format!(
                "SUM(date(a.published_at, 'localtime') = date('now', 'localtime') AND {})",
                exists(" AND date(b.published_at, 'localtime') = date('now', 'localtime')")
            ),
        )
    } else {
        (
            "COUNT(*)".to_string(),
            "SUM(a.is_read = 0)".to_string(),
            "SUM(a.is_starred = 1)".to_string(),
            "SUM(date(a.published_at, 'localtime') = date('now', 'localtime'))".to_string(),
        )
    };
    let (head, group, outer_where) = match level {
        ScopeLevel::All => (
            format!("SELECT 0, {total}, {unread}, {starred}, {today} FROM articles a JOIN feeds fa ON fa.id = a.feed_id"),
            String::new(),
            if b_layout {
                " WHERE (CASE WHEN fa.layout != 'inherit' THEN fa.layout ELSE fo.layout END) = ?"
            } else {
                ""
            },
        ),
        ScopeLevel::Feed => (
            format!("SELECT a.feed_id, {total}, {unread}, {starred}, {today} FROM articles a JOIN feeds fa ON fa.id = a.feed_id"),
            " GROUP BY a.feed_id".to_string(),
            if b_layout {
                " WHERE (CASE WHEN fa.layout != 'inherit' THEN fa.layout ELSE fo.layout END) = ?"
            } else {
                ""
            },
        ),
        ScopeLevel::Folder => (
            format!("SELECT fo.id, {total}, {unread}, {starred}, {today} FROM articles a JOIN feeds fa ON fa.id = a.feed_id JOIN folders fo ON fo.id = fa.folder_id"),
            " GROUP BY fo.id".to_string(),
            if b_layout {
                " WHERE (CASE WHEN fa.layout != 'inherit' THEN fa.layout ELSE fo.layout END) = ?"
            } else {
                ""
            },
        ),
    };
    // All/Feed + layout：外层需要 fo 参与布局解析；Folder 恒有 fo。
    let join_fo = if b_layout && level != ScopeLevel::Folder {
        " JOIN folders fo ON fo.id = fa.folder_id"
    } else {
        ""
    };
    let sql = format!("{head}{join_fo}{outer_where}{group}");
    // 绑定：先 SELECT 列表里 4 个 b_layout 占位（仅 dedup 开启时存在），
    // 再外层 layout（SQL 文本次序）。
    let mut params: Vec<rusqlite::types::Value> = Vec::new();
    if let Some(l) = layout {
        if dedup {
            for _ in 0..4 {
                params.push(l.to_string().into());
            }
        }
        params.push(l.to_string().into());
    }
    (sql, params)
}

fn scope_counts_level(
    conn: &Connection,
    level: ScopeLevel,
    layout: Option<&str>,
    dedup: bool,
) -> AppResult<Vec<ScopedCountsRow>> {
    let (sql, params) = scope_counts_sql(level, dedup, layout);
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(params), counts_from_row)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 计算「全部 / 每分类 / 每源」三个层级的显示集合计数（可选布局过滤，
/// 与列表查询同一布局解析：feed 级覆盖 → 分类兜底）。
/// 结果供前端侧边栏聚合数字使用，保证「数字 == 点进去看到的列表条数」。
pub fn scope_counts(conn: &Connection, layout: Option<&str>) -> AppResult<ScopeCountsDetail> {
    scope_counts_with_dedup(conn, layout, smart_dedup_on(conn))
}

/// [`scope_counts`] 的显式开关内核（R2/N2：与 [`feed_counts_with_dedup`] 同一
/// 开关值、同一次读取内计算——`counts_snapshot` 的一致性来源）。
fn scope_counts_with_dedup(
    conn: &Connection,
    layout: Option<&str>,
    dedup: bool,
) -> AppResult<ScopeCountsDetail> {
    let all_rows = scope_counts_level(conn, ScopeLevel::All, layout, dedup)?;
    let all_row = all_rows.first().copied().unwrap_or(ScopedCountsRow {
        scope_id: 0,
        total: 0,
        unread: 0,
        starred: 0,
        today: 0,
    });
    let folders = scope_counts_level(conn, ScopeLevel::Folder, layout, dedup)?
        .into_iter()
        .map(|r| ScopedCounts {
            scope_id: r.scope_id,
            total: r.total,
            unread: r.unread,
            starred: r.starred,
            today: r.today,
        })
        .collect();
    let feeds = scope_counts_level(conn, ScopeLevel::Feed, layout, dedup)?
        .into_iter()
        .map(|r| ScopedCounts {
            scope_id: r.scope_id,
            total: r.total,
            unread: r.unread,
            starred: r.starred,
            today: r.today,
        })
        .collect();
    Ok(ScopeCountsDetail {
        all: ScopeCounts {
            total: all_row.total,
            unread: all_row.unread,
            starred: all_row.starred,
            today: all_row.today,
        },
        folders,
        feeds,
    })
}

/// 计数组合快照（OPT-008B R2 / N2 + P3-4）：**同一开关值、同一读取时刻**下的
/// 源计数（布局无关）与 scope 精确计数（可按布局过滤）。调用方在同一 DB 锁内
/// 消费本函数，源键与聚合键不可能来自两个时刻——替代前端两次独立 IPC 的
/// 「源键 vs 聚合键不同时刻」混合快照窗口。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CountsSnapshot {
    pub feeds: Vec<FeedCounts>,
    pub scope: ScopeCountsDetail,
}

/// 读一次开关、算两份计数（同一连接 = 同一 SQLite 快照）。
pub fn counts_snapshot(conn: &Connection, layout: Option<&str>) -> AppResult<CountsSnapshot> {
    let dedup = smart_dedup_on(conn);
    let feeds = feed_counts_with_dedup(conn, dedup)?;
    let scope = scope_counts_with_dedup(conn, layout, dedup)?;
    Ok(CountsSnapshot { feeds, scope })
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

    /* ---------- P3[5]：cleanup_cache 的 cutoff 不得因时区混用而多删 ---------- */

    /// cutoff 必须与 published_at 处于同一时基（P3[5]）。**实测语义**（非推测）：
    ///
    /// · published_at 由 `to_rfc3339()` 产生，形如 `2026-09-14T18:45:20+12:00`（含 `T` 与偏移）；
    /// · 修前 cutoff = `datetime('now','-N days','localtime')`，形如 `2026-09-14 16:45:20`
    ///   （含空格、本地墙上时间）；
    /// · 两者做**裸文本比较**：第 11 个字符处 `'T'(0x54) > ' '(0x20)`，故当**日期部分相同**时
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
            let (sql, p) = list_articles_sql(query, false);
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
        let (sql, p) = article_index_sql(&q(None, None, false, None), 1, false);
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

    /// 参考旧实现（apply_mark_all_read 修前形态的参照）：标读前收集
    /// **读目标集合**（范围未读 ∪ 同组闭包，OPT-008B R1）→ 单条 UPDATE →
    /// 逐 id enqueue_sync。与生产入口共用同一目标谓词（list_read_target_ids_scoped），
    /// 等价性测试据此锁定「入队集合 = 标读集合 = 目标集合」。
    fn reference_mark_all_read(
        conn: &Connection,
        feed_id: Option<i64>,
        folder_id: Option<i64>,
        starred_only: bool,
        since_ms: Option<i64>,
        layout: Option<&str>,
    ) -> AppResult<usize> {
        let ids =
            list_read_target_ids_scoped(conn, feed_id, folder_id, starred_only, since_ms, layout)?;
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

    /* ============================================================
    OPT-007（F09/F10）：字段版本递增覆盖全部本地入口 + checked 增长
    ============================================================ */

    fn versions(conn: &Connection, id: i64) -> (i64, i64) {
        conn.query_row(
            "SELECT read_version, star_version FROM articles WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    }

    /// 判别力：set_read/set_starred（单条内核）、set_read_bulk_with_enqueue
    /// （批量）、mark_all_read_with_enqueue / mark_all_read（全部已读）任一漏
    /// 递增时对应断言红；read 写入误递增 star_version（或反向）时隔离断言红。
    #[test]
    fn opt007_local_status_paths_bump_field_versions_in_isolation() {
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
        let a1 = upsert_article_with_feed(&conn, feed, &na("v1", None), false)
            .unwrap()
            .0;
        let a2 = upsert_article_with_feed(&conn, feed, &na("v2", None), false)
            .unwrap()
            .0;
        let a3 = upsert_article_with_feed(&conn, feed, &na("v3", None), false)
            .unwrap()
            .0;
        assert_eq!(versions(&conn, a1), (0, 0), "新入库版本为 0");

        // 单条读：只动 read_version；单条藏：只动 star_version
        set_read(&conn, a1, true).unwrap();
        assert_eq!(versions(&conn, a1), (1, 0), "set_read 递增 read_version");
        set_starred(&conn, a1, true).unwrap();
        assert_eq!(
            versions(&conn, a1),
            (1, 1),
            "set_starred 递增 star_version（独立列）"
        );

        // 复合入口（with_enqueue）逐条递增，且与入队同事务
        set_read_with_enqueue(&conn, a1, false).unwrap();
        assert_eq!(versions(&conn, a1).0, 2, "set_read_with_enqueue 递增");

        // 批量入口逐 id 递增
        set_read_bulk_with_enqueue(&conn, &[a2, a3], true).unwrap();
        assert_eq!(versions(&conn, a2).0, 1, "批量标读递增 a2");
        assert_eq!(versions(&conn, a3).0, 1, "批量标读递增 a3");

        // mark_all_read_with_enqueue：范围内未读行逐行递增
        let n = mark_all_read_with_enqueue(&conn, None, None, false, None, None).unwrap();
        assert!(n >= 1, "范围标读至少命中一行");
        assert_eq!(
            versions(&conn, a1),
            (3, 1),
            "全部已读递增 read_version 且不触碰 star_version"
        );

        // 对照入口 mark_all_read 同口径
        set_read(&conn, a1, false).unwrap();
        let before = versions(&conn, a1).0;
        mark_all_read(&conn, None, None, false, None, None).unwrap();
        assert_eq!(
            versions(&conn, a1).0,
            before + 1,
            "mark_all_read 参照入口同样递增（两入口口径一致）"
        );
    }

    /// checked 增长：版本达 i64::MAX 时本地写入必须 Err（拒绝饱和复用同一版本
    /// ——饱和会让远端合并的「版本相等」判定把新本地意图误判成无变化）。
    /// 单条与批量（mark_all_read*）两条内核都锁死。
    #[test]
    fn opt007_version_growth_is_checked_not_saturating() {
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
        let a = upsert_article_with_feed(&conn, feed, &na("max", None), false)
            .unwrap()
            .0;
        conn.execute(
            "UPDATE articles SET read_version = ?1 WHERE id = ?2",
            params![i64::MAX, a],
        )
        .unwrap();

        // 单条：Err，且现场不被改写（版本仍是 MAX、is_read 不落地）
        let err = set_read(&conn, a, true).expect_err("read_version 饱和必须拒绝");
        assert_eq!(err.code, "protocol");
        assert_eq!(versions(&conn, a).0, i64::MAX, "失败不得改写版本现场");
        let is_read: i64 = conn
            .query_row("SELECT is_read FROM articles WHERE id = ?1", [a], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(is_read, 0, "饱和拒绝时状态写入不得落地");

        // 全部已读：整条 Err（事务内先预检），不产生半批
        let err = mark_all_read_with_enqueue(&conn, None, None, false, None, None)
            .expect_err("范围内饱和行必须让整条标读失败");
        assert_eq!(err.code, "protocol");
        let err2 = mark_all_read(&conn, None, None, false, None, None).expect_err("对照入口同口径");
        assert_eq!(err2.code, "protocol");
        let queued: i64 = conn
            .query_row("SELECT COUNT(*) FROM sync_queue", [], |r| r.get(0))
            .unwrap();
        assert_eq!(queued, 0, "饱和失败时入队同样回滚（无半批）");
    }

    /// R3：tracked upsert 的 `content_changed` 只报**实际前后变化**——
    /// 新插入靠 inserted 表态（content_changed=false）；同值 replay / 空串
    /// 归一（NULL ≡ ''）不得误报；真实字段变化必须报。旧形状入口保持兼容。
    #[test]
    fn tracked_upsert_reports_only_real_content_changes() {
        let conn = conn();
        let fid = create_folder(&conn, "R3", "article").unwrap();
        let feed = insert_feed(
            &conn,
            "https://r3.example/rss",
            None,
            "r3",
            None,
            fid,
            "inherit",
            false,
            false,
        )
        .unwrap();

        let a = na("r3-tracked", Some("2026-01-01T00:00:00+00:00".into()));
        let o1 = upsert_article_with_feed_tracked(&conn, feed, &a, false).unwrap();
        assert!(o1.inserted, "首次必须 report inserted");
        assert!(!o1.content_changed, "新插入的内容变化由 inserted 承担");

        // 同值 replay：不报变化
        let o2 = upsert_article_with_feed_tracked(&conn, feed, &a, false).unwrap();
        assert_eq!(o2.id, o1.id, "guid 命中同一行");
        assert!(
            !o2.inserted && !o2.content_changed,
            "同值 replay 不得报变化"
        );

        // 真实变化：标题改写
        let mut b = na("r3-tracked", Some("2026-01-01T00:00:00+00:00".into()));
        b.title = "changed title".into();
        let o3 = upsert_article_with_feed_tracked(&conn, feed, &b, false).unwrap();
        assert!(
            !o3.inserted && o3.content_changed,
            "标题真实改变必须报 content_changed"
        );

        // 空串 ≡ NULL：author 由 NULL 写成 ''（COALESCE('',...) 命中）→ 归一后无可见差异
        let mut c = na("r3-tracked", Some("2026-01-01T00:00:00+00:00".into()));
        c.title = "changed title".into();
        c.author = Some(String::new());
        let o4 = upsert_article_with_feed_tracked(&conn, feed, &c, false).unwrap();
        assert!(
            !o4.content_changed,
            "空串与 NULL 归一为同值，不构成可见变化"
        );

        // 旧形状包装（id/inserted）保持兼容
        let (id, inserted) = upsert_article_with_feed(&conn, feed, &b, false).unwrap();
        assert_eq!(id, o1.id);
        assert!(!inserted);
    }
}

/* ============================================================
OPT-017B：搜索候选索引等值与回落回归。
（先红后绿：本模块在实现 v29/候选路径前运行于 HEAD 上，trigram 对象不存在。）
============================================================ */
#[cfg(test)]
mod opt017b_search_tests {
    use super::*;
    use crate::db::{
        create_folder, insert_feed, set_article_ai_fields, upsert_article_with_feed, MIGRATIONS,
    };
    use rusqlite::Connection;

    fn conn() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        conn
    }

    fn trigram_objects_present(conn: &Connection) -> bool {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE (type = 'table' AND name = 'articles_trigram_idx') \
                 OR (type = 'trigger' AND name IN ('trg_articles_trigram_ai','trg_articles_trigram_ad','trg_articles_trigram_au','trg_contents_trigram_au'))",
                [],
                |r| r.get(0),
            )
            .unwrap();
        n == 5
    }

    fn drop_trigram_objects(conn: &Connection) {
        conn.execute_batch(
            "DROP TRIGGER IF EXISTS trg_articles_trigram_ai;
             DROP TRIGGER IF EXISTS trg_articles_trigram_ad;
             DROP TRIGGER IF EXISTS trg_articles_trigram_au;
             DROP TRIGGER IF EXISTS trg_contents_trigram_au;
             DROP TABLE IF EXISTS articles_trigram_idx;",
        )
        .unwrap();
    }

    /// 冻结的 LIKE-only 参考实现：逐字复制自 OPT-017B 之前的 `search_articles`
    /// （HEAD 956cd59）。仅作测试差分基准，不参与生产路径；不得随手改动——
    /// 它代表「实现前行为」，改动即失去等值判据。（`rustfmt::skip` = 保持
    /// 冻结字节，格式化只允许发生在生产实现侧。）
    #[rustfmt::skip]
    #[allow(clippy::too_many_lines)]
    fn reference_search_like_only(
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

        let term_conditions = |alias: &str,
                               content_alias: &str,
                               start_idx: usize|
         -> (Vec<String>, usize) {
            let mut parts = Vec::with_capacity(terms.len());
            let mut idx = start_idx;
            for _ in terms.iter() {
                let b = idx;
                parts.push(format!(
                    "({a}.title LIKE ?{b} ESCAPE '\\' OR {ac}.body_text LIKE ?{c} ESCAPE '\\' OR COALESCE({a}.summary,'') LIKE ?{d} ESCAPE '\\' OR COALESCE({a}.ai_summary,'') LIKE ?{e} ESCAPE '\\' OR COALESCE({a}.translated_content,'') LIKE ?{f} ESCAPE '\\')",
                    a = alias,
                    ac = content_alias,
                    b = b,
                    c = b + 1,
                    d = b + 2,
                    e = b + 3,
                    f = b + 4,
                ));
                idx += 5;
            }
            (parts, idx)
        };
        let (where_parts, next_idx) = term_conditions("a", "c", 1);
        let mut like_args: Vec<String> = Vec::with_capacity(terms.len() * 5);
        for t in terms.iter() {
            let pat = format!("%{t}%");
            for _ in 0..5 {
                like_args.push(pat.clone());
            }
        }
        let limit: i64 = if limit <= 0 { 100 } else { limit.min(1000) };
        let (dedup_sql, mut params): (String, Vec<rusqlite::types::Value>) = if smart_dedup_on(conn) {
            let (inner_parts, _) = term_conditions("b", "bc", next_idx);
            let mut extra: Vec<rusqlite::types::Value> = Vec::with_capacity(terms.len() * 5);
            for t in terms.iter() {
                let pat = format!("%{t}%");
                for _ in 0..5 {
                    extra.push(pat.clone().into());
                }
            }
            (
                format!(
                    " AND NOT EXISTS (SELECT 1 FROM articles b LEFT JOIN contents bc ON bc.id = b.content_id \
                     WHERE b.url_norm = a.url_norm AND b.id < a.id AND {})",
                    inner_parts.join(" AND ")
                ),
                extra,
            )
        } else {
            (String::new(), Vec::new())
        };
        let limit_idx = like_args.len() + params.len() + 1;
        let sql = format!(
            "SELECT a.id, a.feed_id, a.title, a.author,
                    COALESCE(NULLIF(a.summary, ''), substr(COALESCE(c.body_text, ''), 1, 280)) AS snippet,
                    a.image_url, a.enclosure_url, a.enclosure_mime, a.duration_sec,
                    a.ai_summary, a.source, a.published_at, a.is_read, a.is_starred,
                    NULL, NULL, NULL, 0, a.content_id, COALESCE(c.revision, 0), a.ai_summary_sig, a.translated_sig
             FROM articles a LEFT JOIN contents c ON c.id = a.content_id
             WHERE {}{dedup_sql}
             ORDER BY a.published_at DESC
             LIMIT ?{limit_idx}",
            where_parts.join(" AND "),
            limit_idx = limit_idx
        );
        params.splice(0..0, like_args.into_iter().map(Into::into));
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(
            rusqlite::params_from_iter(
                params
                    .iter()
                    .map(|v| v as &dyn rusqlite::ToSql)
                    .chain(std::iter::once(&limit as &dyn rusqlite::ToSql)),
            ),
            article_list_item,
        )?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    fn na_full(
        guid: &str,
        url: &str,
        title: &str,
        body: &str,
        summary: Option<&str>,
        published: &str,
    ) -> NewArticle {
        NewArticle {
            guid: guid.into(),
            url: Some(url.into()),
            title: title.into(),
            author: None,
            summary: summary.map(str::to_string),
            content_html: Some(format!("<p>{body}</p>")),
            body_text: body.into(),
            image_url: None,
            enclosure_url: None,
            enclosure_mime: None,
            duration_sec: None,
            published_at: Some(published.to_string()),
            source: "direct".into(),
        }
    }

    /// 等值夹具矩阵：中文 2 字/4 字、英文（含大小写）、中英混合、AI 字段命中
    /// （ai_summary/translated 各自独占）、summary 独占、无结果词、`%`/`_`/`\`
    /// 转义词（含干扰项）、并列发布时间、去重组（小 id 不含词、大 id 含词）。
    fn build_matrix_fixture(conn: &Connection) {
        let folder = create_folder(conn, "搜索矩阵", "article").unwrap();
        let feed = insert_feed(
            conn,
            "https://s.example/rss",
            None,
            "搜索源",
            None,
            folder,
            "inherit",
            false,
            false,
        )
        .unwrap();
        let add = |guid: &str,
                   url: &str,
                   title: &str,
                   body: &str,
                   summary: Option<&str>,
                   published: &str|
         -> i64 {
            upsert_article_with_feed(
                conn,
                feed,
                &na_full(guid, url, title, body, summary, published),
                false,
            )
            .unwrap()
            .0
        };
        let _ = add(
            "m1",
            "https://s.example/m1",
            "科技前沿速递",
            "本文讲深度学习与神经网络。",
            None,
            "2026-01-01T00:01:00+00:00",
        );
        let _ = add(
            "m2",
            "https://s.example/m2",
            "Weekly HTML Digest",
            "The quick brown fox jumps.",
            None,
            "2026-01-01T00:02:00+00:00",
        );
        let _ = add(
            "m3",
            "https://s.example/m3",
            "podcast模型周刊",
            "中英mixed内容与AI模型训练。",
            None,
            "2026-01-01T00:03:00+00:00",
        );
        let m4 = add(
            "m4",
            "https://s.example/m4",
            "普通标题四",
            "普通正文四。",
            None,
            "2026-01-01T00:04:00+00:00",
        );
        let m5 = add(
            "m5",
            "https://s.example/m5",
            "普通标题五",
            "普通正文五。",
            None,
            "2026-01-01T00:05:00+00:00",
        );
        let _ = add(
            "m6",
            "https://s.example/m6",
            "esc标题六",
            "路径 C:\\temp 与 50_off 与 100%折扣。",
            Some("摘要词条：独家"),
            "2026-01-01T00:06:00+00:00",
        );
        // 干扰项：50Xoff 不应被 "50_off" 命中；C:Xtemp 不应被 "C:\\temp" 命中
        let _ = add(
            "m11",
            "https://s.example/m11",
            "干扰标题十一",
            "50Xoff 与 C:Xtemp 干扰项。",
            None,
            "2026-01-01T00:11:00+00:00",
        );
        // AI 字段独占命中
        set_article_ai_fields(conn, m4, Some("模型蒸馏技术"), None).unwrap();
        set_article_ai_fields(conn, m5, None, Some("量子纠缠实验报告")).unwrap();
        // 并列发布时间（同值）：两种实现的并列顺序必须逐字一致
        let _ = add(
            "m7",
            "https://s.example/m7",
            "并列标题甲",
            "并列词验证甲。",
            None,
            "2026-01-01T00:20:00+00:00",
        );
        let _ = add(
            "m8",
            "https://s.example/m8",
            "并列标题乙",
            "并列词验证乙。",
            None,
            "2026-01-01T00:20:00+00:00",
        );
        // 去重组：同 URL；较小 id 副本不含词、较大 id 副本含词（R1：含词副本必须可被搜到）
        let _ = add(
            "dup-a",
            "https://s.example/dedup",
            "重复组第一条",
            "无关键词的组内小 id 副本。",
            None,
            "2026-01-01T00:30:00+00:00",
        );
        let _ = add(
            "dup-b",
            "https://s.example/dedup",
            "重复组第二条",
            "该副本包含 独特去重词 供搜索。",
            None,
            "2026-01-01T00:31:00+00:00",
        );
    }

    const MATRIX: &[(&str, i64)] = &[
        // 中文 2 字（无 trigram → 必须回落 LIKE 且命中正确）
        ("科技", 50),
        ("独家", 50),
        ("模型", 50),
        // 中文 4 字（候选路径）
        ("深度学习", 50),
        ("神经网络", 50),
        ("模型蒸馏", 50),
        ("量子纠缠", 50),
        ("摘要词条", 50),
        // 英文（大小写不敏感）、3 字、多词 AND
        ("html", 50),
        ("HTML", 50),
        ("quick", 50),
        ("fox", 50),
        ("quick brown", 50),
        // 中英混合
        ("AI", 50),
        ("AI模型", 50),
        ("mixed", 50),
        ("科技 深度学习", 50),
        // 转义字符（%/_/\\）
        ("100%", 50),
        ("50_off", 50),
        ("C:\\temp", 50),
        // 去重与并列
        ("独特去重词", 50),
        ("并列词", 50),
        // 无结果 / 词际 AND 落空
        ("罅隙龃龉甪端", 50),
        ("quick 不存在", 50),
        // 空/空白查询
        ("", 50),
        ("   ", 50),
        // LIMIT 收敛
        ("普通", 1),
        ("普通", 50),
        ("普通", -1),
        ("普通", 100000),
    ];

    fn assert_matrix_equal(conn: &Connection, dedup: bool) {
        for (q, limit) in MATRIX {
            let got = serde_json::to_value(search_articles(conn, q, *limit).unwrap()).unwrap();
            let want =
                serde_json::to_value(reference_search_like_only(conn, q, *limit).unwrap()).unwrap();
            assert_eq!(
                got, want,
                "查询 {q:?} limit={limit}（dedup={dedup}）必须与冻结 LIKE 参考实现逐字等值（含 id 序列/顺序/全字段）"
            );
        }
    }

    /// 反例 1：搜索等值矩阵 —— 候选索引就位后，结果集与实现前逐字一致；
    /// 去重开/关两种设置下都必须等值（去重内层 EXISTS 复用 LIKE 条件不受影响）。
    #[test]
    fn search_matches_frozen_like_reference_across_matrix() {
        let conn = conn();
        build_matrix_fixture(&conn);
        assert!(
            trigram_objects_present(&conn),
            "红：v29 未实现——候选索引对象不存在"
        );
        assert_matrix_equal(&conn, false);
        // 打开显示去重后重跑全矩阵
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES ('app_settings', '{\"smartDedup\":true}')",
            [],
        )
        .unwrap();
        assert_matrix_equal(&conn, true);
    }

    /// 反例 4（索引缺失回落）：删除候选索引对象后，search_articles 必须回落
    /// LIKE-only 且结果与参考实现逐字一致（不得返回空/报错）。
    #[test]
    fn search_falls_back_and_stays_equal_when_index_missing() {
        let conn = conn();
        build_matrix_fixture(&conn);
        assert!(
            trigram_objects_present(&conn),
            "红：v29 未实现——候选索引对象不存在"
        );
        drop_trigram_objects(&conn);
        assert!(!trigram_objects_present(&conn));
        assert_matrix_equal(&conn, false);
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES ('app_settings', '{\"smartDedup\":true}')",
            [],
        )
        .unwrap();
        assert_matrix_equal(&conn, true);
    }

    /// 反例 4（索引损坏回落）：影子表被移除后 MATCH 查询会报错——必须捕获、
    /// 记日志并回落 LIKE-only，结果仍与参考实现逐字一致。
    #[test]
    fn search_falls_back_and_stays_equal_when_index_shadow_corrupt() {
        let conn = conn();
        build_matrix_fixture(&conn);
        assert!(
            trigram_objects_present(&conn),
            "红：v29 未实现——候选索引对象不存在"
        );
        conn.execute_batch("DROP TABLE articles_trigram_idx_data;")
            .unwrap();
        assert_matrix_equal(&conn, false);
    }

    fn explain_plan(conn: &Connection, sql: &str, params_n: usize) -> Vec<String> {
        let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        // EXPLAIN 也要求参数个数匹配：以 NULL 占位绑定。
        let args = vec![rusqlite::types::Value::Null; params_n];
        stmt.query_map(rusqlite::params_from_iter(args.iter()), |r| {
            r.get::<_, String>(3)
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
    }

    /// 机制取证（不依赖耗时）：候选子句形状、3 字符阈值、混合词只加长词候选、
    /// 生成 SQL 的计划必须使用候选索引并保持 published 序索引扫描。
    #[test]
    fn candidate_clause_shape_plan_and_short_term_fallback() {
        let conn = conn();
        build_matrix_fixture(&conn);

        // 词形：≥3 字符 → Some（ASCII 折叠 + 引号双写）；<3 字符 → None
        assert_eq!(
            trigram_candidate_match("科技"),
            None,
            "2 字中文无三元组 → 该词必须回落 LIKE"
        );
        assert_eq!(trigram_candidate_match("AI"), None);
        assert_eq!(
            trigram_candidate_match("深度学习"),
            Some("\"深度学\" AND \"度学习\"".to_string())
        );
        assert_eq!(
            trigram_candidate_match("HTML"),
            Some("\"htm\" AND \"tml\"".to_string()),
            "ASCII 折叠必须与 LIKE（lower 只折 ASCII）语义一致"
        );
        assert_eq!(
            trigram_candidate_match("a\"b\"c"),
            Some("\"a\"\"b\" AND \"\"\"b\"\"\" AND \"b\"\"c\"".to_string()),
            "内嵌引号必须双写转义"
        );

        // 生成 SQL：长词带候选子句 + INDEXED BY；短词纯 LIKE 无候选
        let long_cand = vec![trigram_candidate_match("深度学习").unwrap()];
        let (sql_long, long_bind) = build_search_sql(&[like_escape("深度学习")], &long_cand, false);
        assert!(sql_long.contains("a.id IN (SELECT rowid FROM articles_trigram_idx"));
        assert!(sql_long.contains("INDEXED BY idx_articles_published_id"));
        let (sql_short, short_bind) = build_search_sql(&[like_escape("科技")], &[], false);
        assert!(!sql_short.contains("articles_trigram_idx"));
        assert!(!sql_short.contains("INDEXED BY"));

        // 混合词（2 字 + 4 字）：只对长词加一条候选子句
        let mixed = vec![like_escape("科技"), like_escape("深度学习")];
        let (sql_mixed, _) = build_search_sql(&mixed, &long_cand, false);
        assert_eq!(
            sql_mixed
                .matches("IN (SELECT rowid FROM articles_trigram_idx")
                .count(),
            1,
            "混合长度词族只能为长词加候选"
        );

        // 计划：候选查询命中 trigram 表 + published 序索引扫描（免整体排序）
        let plan = explain_plan(&conn, &sql_long, long_bind.len() + 1);
        assert!(
            plan.iter().any(|s| s.contains("articles_trigram_idx")),
            "候选查询计划必须引用候选索引：{plan:?}"
        );
        assert!(
            plan.iter().any(|s| s.contains("idx_articles_published_id")),
            "候选查询必须保持 published 序索引扫描：{plan:?}"
        );
        assert!(
            !plan.iter().any(|s| s.contains("TEMP B-TREE FOR ORDER BY")),
            "候选过滤不得引入整体排序：{plan:?}"
        );
        let plan_short = explain_plan(&conn, &sql_short, short_bind.len() + 1);
        assert!(
            !plan_short
                .iter()
                .any(|s| s.contains("articles_trigram_idx")),
            "短词查询不得引用候选索引：{plan_short:?}"
        );
    }
}

/* ============================================================
OPT-017G：读闭包查询规划与批量标读成本回归。
（先红后绿：本模块在实现 v30 部分索引 `idx_articles_url_norm_unread` 前运行于
未迁移库上——「计划引用新索引」断言为红，语义等值组恒为守卫、以变异证明有效。）
============================================================ */
#[cfg(test)]
mod opt017g_read_closure_tests {
    use super::*;
    use crate::db::{create_folder, insert_feed, scope_counts, MIGRATIONS};
    use rusqlite::Connection;

    const NEW_INDEX: &str = "idx_articles_url_norm_unread";

    fn conn() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        conn
    }

    fn explain_plan(conn: &Connection, sql: &str, params_n: usize) -> Vec<String> {
        let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        let args = vec![rusqlite::types::Value::Null; params_n];
        stmt.query_map(rusqlite::params_from_iter(args.iter()), |r| {
            r.get::<_, String>(3)
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
    }

    fn index_present(conn: &Connection, name: &str) -> bool {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
                rusqlite::params![name],
                |r| r.get(0),
            )
            .unwrap();
        n == 1
    }

    struct Fixture {
        all: Vec<i64>,
        /// 同文组一：A1 未读 / A2 已读+收藏 / A3 未读（跨两个源）
        g1: [i64; 3],
        /// 同文组二：B1/B2 未读
        g2: [i64; 2],
        unique: i64,
        null_norm: i64,
    }

    /// 夹具：同 url_norm 跨源组（读传播主场）、独立组、独条、NULL url_norm。
    /// 另补 200 条唯一 url_norm 底噪行，保证规划器有统计空间选择索引
    /// （小表整表扫描可能压过索引，与生产形态不符）。
    fn seed_fixture(conn: &Connection) -> Fixture {
        let folder = create_folder(conn, "读闭包夹具", "article").unwrap();
        let mk_feed = |suffix: &str| -> i64 {
            insert_feed(
                conn,
                &format!("https://probe.invalid/{suffix}"),
                None,
                suffix,
                None,
                folder,
                "inherit",
                false,
                false,
            )
            .unwrap()
        };
        let feed_a = mk_feed("a");
        let feed_b = mk_feed("b");
        let mk = |feed: i64, guid: &str, norm: Option<&str>, read: bool, star: bool| -> i64 {
            conn.execute(
                "INSERT INTO articles(feed_id,guid,url,url_norm,title,published_at,is_read,is_starred)
                 VALUES(?1,?2,?3,?4,'t','2026-10-16T00:00:00Z',?5,?6)",
                rusqlite::params![feed, guid, norm, norm, read as i64, star as i64],
            )
            .unwrap();
            conn.last_insert_rowid()
        };
        let a1 = mk(
            feed_a,
            "g1-a1",
            Some("https://probe.invalid/same-a"),
            false,
            false,
        );
        let a2 = mk(
            feed_b,
            "g1-a2",
            Some("https://probe.invalid/same-a"),
            true,
            true,
        );
        let a3 = mk(
            feed_a,
            "g1-a3",
            Some("https://probe.invalid/same-a"),
            false,
            false,
        );
        let b1 = mk(
            feed_a,
            "g2-b1",
            Some("https://probe.invalid/same-b"),
            false,
            false,
        );
        let b2 = mk(
            feed_b,
            "g2-b2",
            Some("https://probe.invalid/same-b"),
            false,
            false,
        );
        let unique = mk(
            feed_a,
            "uniq-1",
            Some("https://probe.invalid/uniq-1"),
            false,
            false,
        );
        let null_norm = mk(feed_a, "null-1", None, false, false);
        for i in 0..200 {
            let norm = format!("https://probe.invalid/noise/{i}");
            conn.execute(
                "INSERT INTO articles(feed_id,guid,url,url_norm,title,published_at,is_read,is_starred)
                 VALUES(?1,?2,?3,?3,'t','2026-10-16T00:00:00Z',?4,0)",
                rusqlite::params![feed_a, format!("noise-{i}"), norm, (i % 3 == 0) as i64],
            )
            .unwrap();
        }
        Fixture {
            all: vec![a1, a2, a3, b1, b2, unique, null_norm],
            g1: [a1, a2, a3],
            g2: [b1, b2],
            unique,
            null_norm,
        }
    }

    fn closure_rowset(conn: &Connection, id: i64) -> Vec<i64> {
        let norm: Option<String> = conn
            .query_row(
                "SELECT url_norm FROM articles WHERE id = ?1",
                rusqlite::params![id],
                |r| r.get::<_, Option<String>>(0),
            )
            .unwrap();
        let Some(norm) = norm else {
            return Vec::new();
        };
        let mut stmt = conn.prepare(READ_CLOSURE_SQL).unwrap();
        stmt.query_map(rusqlite::params![id, norm], |r| r.get::<_, i64>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    fn state_snapshot(conn: &Connection) -> serde_json::Value {
        let rows: Vec<(i64, i64, i64, i64)> = {
            let mut stmt = conn
                .prepare("SELECT id, is_read, read_version, is_starred FROM articles ORDER BY id")
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        let queue: Vec<(i64, String)> = {
            let mut stmt = conn
                .prepare("SELECT article_id, action FROM sync_queue ORDER BY article_id, action")
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        serde_json::json!({"articles": rows, "queue": queue})
    }

    /// 反例 1（先红后绿）：读闭包查询在最新 schema 下必须由 v30 部分索引
    /// `idx_articles_url_norm_unread` 驱动（等值前缀 + 免排序覆盖扫描），
    /// 不得再扫全未读行，也不得引入临时排序。
    #[test]
    fn closure_plan_uses_partial_url_norm_unread_index() {
        let conn = conn();
        let _fx = seed_fixture(&conn);
        assert!(
            index_present(&conn, NEW_INDEX),
            "v30 必须在最新 schema 建立 {NEW_INDEX}"
        );
        let plan = explain_plan(&conn, READ_CLOSURE_SQL, 2);
        let plan_ref = explain_plan(&conn, READ_CLOSURE_SQL, 2);
        assert_eq!(plan, plan_ref, "同库同查询计划必须稳定");
        assert!(
            plan.iter().any(|s| s.contains(NEW_INDEX)),
            "闭包计划必须引用 {NEW_INDEX}：{plan:?}"
        );
        assert!(
            !plan.iter().any(|s| s.contains("TEMP B-TREE")),
            "闭包计划不得有临时排序（索引序即 ORDER BY id）：{plan:?}"
        );
        assert!(
            !plan.iter().any(|s| s.contains("idx_articles_unread")),
            "闭包计划不得再落到全未读扫描索引：{plan:?}"
        );
    }

    /// 反例 3（缺失回落）：删掉新索引后——
    /// 计划回落旧现场（idx_articles_unread 全未读扫描），但闭包**结果集**与
    /// 生产状态迁移保持逐字正确（回落只变慢、不变义）。
    #[test]
    fn fallback_when_index_dropped_keeps_results_and_transitions() {
        let conn = conn();
        let fx = seed_fixture(&conn);
        let plan_with = explain_plan(&conn, READ_CLOSURE_SQL, 2);
        assert!(
            plan_with.iter().any(|s| s.contains(NEW_INDEX)),
            "前置：索引在场时计划必须引用新索引：{plan_with:?}"
        );
        let before: Vec<Vec<i64>> = fx.all.iter().map(|id| closure_rowset(&conn, *id)).collect();

        // 索引缺失回落：与生产「库被手工降级/异常现场」同形态（普通 B 树索引无
        // 应用级损坏探测；缺失即规划回落，损坏由 SQLite 报错——本测试覆盖缺失。）
        conn.execute(&format!("DROP INDEX {NEW_INDEX}"), [])
            .unwrap();
        let plan_without = explain_plan(&conn, READ_CLOSURE_SQL, 2);
        assert!(
            !plan_without.iter().any(|s| s.contains(NEW_INDEX)),
            "删索引后计划不得再引用 {NEW_INDEX}：{plan_without:?}"
        );
        assert!(
            plan_without
                .iter()
                .any(|s| s.contains("idx_articles_unread")),
            "删索引后必须回落旧未读扫描计划：{plan_without:?}"
        );
        let after: Vec<Vec<i64>> = fx.all.iter().map(|id| closure_rowset(&conn, *id)).collect();
        assert_eq!(before, after, "索引缺失前后闭包结果集必须逐字一致");

        // 生产入口在回落现场仍然语义正确：A3 标读传播到未读兄弟 A1
        let receipts = set_read_bulk_with_enqueue(&conn, &[fx.g1[2]], true).unwrap();
        let ids: Vec<i64> = receipts.iter().map(|r| r.id).collect();
        let want: Vec<i64> = vec![fx.g1[0], fx.g1[2]];
        assert_eq!(ids, want, "回落现场传播集合与回执必须与索引在场一致");
        let read_state = |id: i64| -> i64 {
            conn.query_row("SELECT is_read FROM articles WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap()
        };
        assert_eq!(read_state(fx.g1[0]), 1);
        assert_eq!(read_state(fx.g1[2]), 1);
        assert_eq!(read_state(fx.g1[1]), 1, "已读兄弟不受影响");
    }

    /// 反例 2（语义等值）：同一夹具双库——库 1 带新索引、库 2 删新索引——
    /// 完整操作序列（批量标读/单条标读/取消标读/收藏）后：
    /// 逐项闭包结果集、回执行、逐行状态（read/read_version/star）与队列、计数快照
    /// 全部逐字一致；收藏保持独立（不随读传播）。
    #[test]
    fn state_transitions_equal_with_and_without_index() {
        let db1 = conn();
        let db2 = conn();
        let fx1 = seed_fixture(&db1);
        let fx2 = seed_fixture(&db2);
        assert_eq!(fx1.all, fx2.all, "双库夹具 id 必须一致");
        assert!(index_present(&db1, NEW_INDEX) && index_present(&db2, NEW_INDEX));
        db2.execute(&format!("DROP INDEX {NEW_INDEX}"), []).unwrap();
        assert!(!index_present(&db2, NEW_INDEX));

        for id in &fx1.all {
            assert_eq!(
                closure_rowset(&db1, *id),
                closure_rowset(&db2, *id),
                "库1/库2 闭包结果集必须一致（id={id}）"
            );
        }

        let mut receipts1: Vec<ReadWriteReceipt> = Vec::new();
        let mut receipts2: Vec<ReadWriteReceipt> = Vec::new();
        // 1) 批量标读触发组内传播（A3→A1）
        receipts1.extend(set_read_bulk_with_enqueue(&db1, &[fx1.g1[2]], true).unwrap());
        receipts2.extend(set_read_bulk_with_enqueue(&db2, &[fx2.g1[2]], true).unwrap());
        // 2) 单条标读已读行（无传播、仅主行入队）
        receipts1.extend(set_read_with_enqueue(&db1, fx1.g1[0], true).unwrap());
        receipts2.extend(set_read_with_enqueue(&db2, fx2.g1[0], true).unwrap());
        // 3) 取消标读（unread 不广播；操作前已读 → 回执含本行）
        receipts1.extend(set_read_with_enqueue(&db1, fx1.g1[1], false).unwrap());
        receipts2.extend(set_read_with_enqueue(&db2, fx2.g1[1], false).unwrap());
        // 4) 收藏独立（不随读传播）
        set_starred_with_enqueue(&db1, fx1.g2[0], true).unwrap();
        set_starred_with_enqueue(&db2, fx2.g2[0], true).unwrap();
        // 5) 批量取消标读
        receipts1.extend(set_read_bulk_with_enqueue(&db1, &fx1.g2, false).unwrap());
        receipts2.extend(set_read_bulk_with_enqueue(&db2, &fx2.g2, false).unwrap());
        // 6) NULL url_norm 条目（无闭包查询、主行直通）
        receipts1.extend(set_read_with_enqueue(&db1, fx1.null_norm, true).unwrap());
        receipts2.extend(set_read_with_enqueue(&db2, fx2.null_norm, true).unwrap());
        // 7) 独条
        receipts1.extend(set_read_with_enqueue(&db1, fx1.unique, true).unwrap());
        receipts2.extend(set_read_with_enqueue(&db2, fx2.unique, true).unwrap());

        assert_eq!(receipts1, receipts2, "回执行必须逐字一致");
        assert_eq!(
            state_snapshot(&db1),
            state_snapshot(&db2),
            "逐行状态（read/read_version/star）与队列必须逐字一致"
        );
        assert_eq!(
            scope_counts(&db1, None).unwrap().all.unread,
            scope_counts(&db2, None).unwrap().all.unread,
            "计数快照 unread 必须一致"
        );

        // 冻结期望（手工推演，独立于实现）：回执集合、行态与队列。
        // 任何「闭包多收/少收、传播错位」的语义变异都会在这里翻红。
        let feed_a = conn_feed_of(&db1, fx1.g1[0]);
        let feed_b = conn_feed_of(&db1, fx1.g1[1]);
        let want_receipts = vec![
            ReadWriteReceipt {
                id: fx1.g1[0],
                feed_id: feed_a,
            }, // 步骤 1 传播
            ReadWriteReceipt {
                id: fx1.g1[2],
                feed_id: feed_a,
            }, // 步骤 1 主行
            ReadWriteReceipt {
                id: fx1.g1[1],
                feed_id: feed_b,
            }, // 步骤 3 取消标读（操作前已读）
            ReadWriteReceipt {
                id: fx1.null_norm,
                feed_id: feed_a,
            }, // 步骤 6
            ReadWriteReceipt {
                id: fx1.unique,
                feed_id: feed_a,
            }, // 步骤 7
        ];
        assert_eq!(receipts1, want_receipts, "回执集合必须等于冻结期望");
        assert_eq!(
            receipts2, want_receipts,
            "（无索引库）回执集合必须等于冻结期望"
        );

        // 行态冻结：(is_read, read_version, is_starred)
        // A1：步骤 1 传播读 + 步骤 2 主行重写 → 版本 2；
        // A2：步骤 3 取消标读（版本 +1）；收藏保持 1；
        // A3：步骤 1 主行 → 版本 1；B1/B2：步骤 5 取消标读（同值写入仍推进版本）；
        // N1/U1：步骤 6/7 各 +1。任何越界标读（如把已读兄弟再次写入）都会改版本。
        let want_states: Vec<(i64, i64, i64, i64)> = vec![
            (fx1.g1[0], 1, 2, 0),
            (fx1.g1[1], 0, 1, 1),
            (fx1.g1[2], 1, 1, 0),
            (fx1.g2[0], 0, 1, 1),
            (fx1.g2[1], 0, 1, 0),
            (fx1.null_norm, 1, 1, 0),
            (fx1.unique, 1, 1, 0),
        ];
        let state_of = |conn: &Connection, id: i64| -> (i64, i64, i64) {
            conn.query_row(
                "SELECT is_read, read_version, is_starred FROM articles WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap()
        };
        for (id, read, ver, star) in &want_states {
            assert_eq!(
                state_of(&db1, *id),
                (*read, *ver, *star),
                "库1 行态必须等于冻结期望（id={id}）"
            );
            assert_eq!(
                state_of(&db2, *id),
                (*read, *ver, *star),
                "库2 行态必须等于冻结期望（id={id}）"
            );
        }

        // 队列冻结（article_id, action 升序）：
        // A1 read（步骤 1/2）；A2 unread（步骤 3 覆盖步骤 1 不存在的 read）；
        // A3 read；B1 star + unread（收藏与取消标读各自保留）；B2 unread；
        // N1 read；U1 read。已读兄弟不得有额外队项。
        let want_queue: Vec<(i64, String)> = vec![
            (fx1.g1[0], "read".into()),
            (fx1.g1[1], "unread".into()),
            (fx1.g1[2], "read".into()),
            (fx1.g2[0], "star".into()),
            (fx1.g2[0], "unread".into()),
            (fx1.g2[1], "unread".into()),
            (fx1.unique, "read".into()),
            (fx1.null_norm, "read".into()),
        ];
        let queue_of = |conn: &Connection| -> Vec<(i64, String)> {
            let mut stmt = conn
                .prepare("SELECT article_id, action FROM sync_queue ORDER BY article_id, action")
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(queue_of(&db1), want_queue, "库1 队列必须等于冻结期望");
        assert_eq!(queue_of(&db2), want_queue, "库2 队列必须等于冻结期望");
    }

    fn conn_feed_of(conn: &Connection, id: i64) -> i64 {
        conn.query_row("SELECT feed_id FROM articles WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap()
    }
}
