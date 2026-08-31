//! SQLite 数据层：schema + 迁移 + 类型化数据访问。
//!
//! 所有 SQL 集中在此；命令层只调用类型化函数，不写裸 SQL。
//! 迁移为追加式：已发布的迁移不可修改，只能新增 M::up。

use rusqlite::{params, Connection, OptionalExtension};
use rusqlite_migration::{Migrations, M};
use serde::Serialize;
use std::path::Path;
use std::sync::LazyLock;

use crate::error::AppResult;

/* ============================================================
   Schema —— 对应实施方案 §3
   folders/feeds 增 layout/auto_summary/auto_translate 列；
   articles 增 source 列（'direct' | 'miniflux'）+ fetch_failed 源级状态。
   ============================================================ */

static MIGRATIONS: LazyLock<Migrations> = LazyLock::new(|| {
    Migrations::new(vec![
    M::up(r#"
        CREATE TABLE folders (
            id            INTEGER PRIMARY KEY,
            name          TEXT NOT NULL,
            position      INTEGER NOT NULL DEFAULT 0,
            layout        TEXT NOT NULL DEFAULT 'article',
            auto_summary  INTEGER NOT NULL DEFAULT 1,
            auto_translate INTEGER NOT NULL DEFAULT 0,
            collapsed     INTEGER NOT NULL DEFAULT 0,
            created_at    TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE feeds (
            id              INTEGER PRIMARY KEY,
            feed_url        TEXT NOT NULL UNIQUE,
            site_url        TEXT,
            title           TEXT NOT NULL,
            favicon_url     TEXT,
            folder_id       INTEGER REFERENCES folders(id) ON DELETE CASCADE,
            layout          TEXT NOT NULL DEFAULT 'inherit',
            auto_summary    INTEGER NOT NULL DEFAULT 1,
            auto_translate  INTEGER NOT NULL DEFAULT 0,
            etag            TEXT,
            last_modified   TEXT,
            last_fetched_at TEXT,
            fetch_error     TEXT,
            fetch_failed    INTEGER NOT NULL DEFAULT 0,
            created_at      TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE articles (
            id            INTEGER PRIMARY KEY,
            feed_id       INTEGER NOT NULL REFERENCES feeds(id) ON DELETE CASCADE,
            guid          TEXT NOT NULL,
            url           TEXT,
            title         TEXT NOT NULL,
            author        TEXT,
            summary       TEXT,
            content_html  TEXT,
            body_text     TEXT NOT NULL DEFAULT '',
            image_url     TEXT,
            enclosure_url TEXT,
            enclosure_mime TEXT,
            duration_sec  INTEGER,
            ai_summary    TEXT,
            translated_content TEXT,
            source        TEXT NOT NULL DEFAULT 'direct',
            published_at  TEXT,
            fetched_at    TEXT NOT NULL DEFAULT (datetime('now')),
            is_read       INTEGER NOT NULL DEFAULT 0,
            is_starred    INTEGER NOT NULL DEFAULT 0,
            UNIQUE(feed_id, guid)
        );

        CREATE INDEX idx_articles_feed      ON articles(feed_id);
        CREATE INDEX idx_articles_published ON articles(published_at DESC);
        CREATE INDEX idx_articles_unread    ON articles(is_read) WHERE is_read = 0;

        CREATE TABLE settings (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
    "#),
    // Miniflux 同步支持 —— 条目/源/分类的 Miniflux id 映射 + 离线变更队列
    M::up(r#"
        ALTER TABLE articles ADD COLUMN miniflux_id INTEGER;
        CREATE UNIQUE INDEX idx_articles_miniflux_id ON articles(miniflux_id) WHERE miniflux_id IS NOT NULL;

        ALTER TABLE feeds ADD COLUMN miniflux_id INTEGER;

        ALTER TABLE folders ADD COLUMN miniflux_id INTEGER;

        CREATE TABLE sync_queue (
            id          INTEGER PRIMARY KEY,
            article_id  INTEGER REFERENCES articles(id) ON DELETE CASCADE,
            feed_url    TEXT,
            action      TEXT NOT NULL,  -- 'read' | 'unread' | 'star' | 'unstar' | 'add_feed' | 'remove_feed'
            payload     TEXT,           -- JSON：add_feed 的 title/folder 等附加信息
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );
    "#),
    // FTS5 全文索引：标题/正文纯文本/作者/AI 摘要/翻译。触发器保持与 articles 同步，
    // user_version=3。unicode61 分词器：中文按字、英文按词，个人规模足够（无需 ICU）。
    M::up(r#"
        CREATE VIRTUAL TABLE articles_fts USING fts5(
            title, body_text, author, ai_summary, translated_content,
            content='articles', content_rowid='id',
            tokenize='unicode61'
        );

        INSERT INTO articles_fts(rowid, title, body_text, author, ai_summary, translated_content)
            SELECT id, title, body_text, COALESCE(author, ''), COALESCE(ai_summary, ''), COALESCE(translated_content, '')
            FROM articles;

        CREATE TRIGGER articles_ai AFTER INSERT ON articles BEGIN
            INSERT INTO articles_fts(rowid, title, body_text, author, ai_summary, translated_content)
            VALUES (new.id, new.title, new.body_text, COALESCE(new.author, ''),
                    COALESCE(new.ai_summary, ''), COALESCE(new.translated_content, ''));
        END;
        CREATE TRIGGER articles_ad AFTER DELETE ON articles BEGIN
            INSERT INTO articles_fts(articles_fts, rowid, title, body_text, author, ai_summary, translated_content)
            VALUES ('delete', old.id, old.title, old.body_text, COALESCE(old.author, ''),
                    COALESCE(old.ai_summary, ''), COALESCE(old.translated_content, ''));
        END;
        CREATE TRIGGER articles_au AFTER UPDATE ON articles BEGIN
            INSERT INTO articles_fts(articles_fts, rowid, title, body_text, author, ai_summary, translated_content)
            VALUES ('delete', old.id, old.title, old.body_text, COALESCE(old.author, ''),
                    COALESCE(old.ai_summary, ''), COALESCE(old.translated_content, ''));
            INSERT INTO articles_fts(rowid, title, body_text, author, ai_summary, translated_content)
            VALUES (new.id, new.title, new.body_text, COALESCE(new.author, ''),
                    COALESCE(new.ai_summary, ''), COALESCE(new.translated_content, ''));
        END;
    "#),
    // 后台刷新调度：失败计数 + 下次重试时间（指数退避 5min→30min→2h）。
    // user_version=4。
    M::up(r#"
        ALTER TABLE feeds ADD COLUMN fail_count INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE feeds ADD COLUMN next_retry_at TEXT;
    "#),
    // 全文提取标志：1 = 正文已被 Readability 全文覆盖（工具栏按钮状态与
    // 设置「自动全文」共用此标志，重启不丢）。user_version=5。
    M::up("ALTER TABLE articles ADD COLUMN fulltext_extracted INTEGER NOT NULL DEFAULT 0;"),
    // 后续迁移在此追加（M::up），已发布的不可改
    ])
});

/// 打开数据库并应用迁移。WAL 模式 + foreign_keys + busy_timeout。
pub fn open(path: &Path) -> AppResult<Connection> {
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    MIGRATIONS.to_latest(&mut conn)?;
    Ok(conn)
}

/* ============================================================
   行类型（前端 IPC 契约）—— 与 src/types.ts 保持同构
   ============================================================ */

#[derive(Debug, Serialize)]
pub struct FolderRow {
    pub id: i64,
    pub name: String,
    pub layout: String,
    pub auto_summary: bool,
    pub auto_translate: bool,
    pub collapsed: bool,
}

#[derive(Debug, Serialize)]
pub struct FeedRow {
    pub id: i64,
    pub folder_id: i64,
    pub feed_url: String,
    pub site_url: Option<String>,
    pub title: String,
    pub favicon_url: Option<String>,
    pub layout: String,
    pub auto_summary: bool,
    pub auto_translate: bool,
    pub fetch_failed: bool,
    pub fetch_error: Option<String>,
    pub last_fetched_at: Option<String>,
}

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

/// 列表页条目（轻量：不含正文 HTML，snippet 截断）
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
}

/* ============================================================
   Folders
   ============================================================ */

pub fn list_folders(conn: &Connection) -> AppResult<Vec<FolderRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, layout, auto_summary, auto_translate, collapsed
         FROM folders ORDER BY position, id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(FolderRow {
            id: r.get(0)?,
            name: r.get(1)?,
            layout: r.get(2)?,
            auto_summary: r.get::<_, i64>(3)? != 0,
            auto_translate: r.get::<_, i64>(4)? != 0,
            collapsed: r.get::<_, i64>(5)? != 0,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn create_folder(conn: &Connection, name: &str, layout: &str) -> AppResult<i64> {
    let next_pos: i64 = conn
        .query_row("SELECT COALESCE(MAX(position), -1) + 1 FROM folders", [], |r| r.get(0))
        .unwrap_or(0);
    conn.execute(
        "INSERT INTO folders (name, position, layout) VALUES (?1, ?2, ?3)",
        params![name, next_pos, layout],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn rename_folder(conn: &Connection, id: i64, name: &str) -> AppResult<()> {
    conn.execute("UPDATE folders SET name = ?1 WHERE id = ?2", params![name, id])?;
    Ok(())
}

pub fn delete_folder(conn: &Connection, id: i64) -> AppResult<()> {
    conn.execute("DELETE FROM folders WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn update_folder_layout(conn: &Connection, id: i64, layout: &str) -> AppResult<()> {
    conn.execute("UPDATE folders SET layout = ?1 WHERE id = ?2", params![layout, id])?;
    Ok(())
}

pub fn set_folder_collapsed(conn: &Connection, id: i64, collapsed: bool) -> AppResult<()> {
    conn.execute(
        "UPDATE folders SET collapsed = ?1 WHERE id = ?2",
        params![collapsed as i64, id],
    )?;
    Ok(())
}

pub fn set_folder_ai_flags(conn: &Connection, id: i64, summary: bool, translate: bool) -> AppResult<()> {
    conn.execute(
        "UPDATE folders SET auto_summary = ?1, auto_translate = ?2 WHERE id = ?3",
        params![summary as i64, translate as i64, id],
    )?;
    Ok(())
}

/* ============================================================
   Feeds
   ============================================================ */

const FEED_COLS: &str = "id, folder_id, feed_url, site_url, title, favicon_url, layout, auto_summary, auto_translate, fetch_failed, fetch_error, last_fetched_at";

fn feed_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<FeedRow> {
    Ok(FeedRow {
        id: r.get(0)?,
        folder_id: r.get(1)?,
        feed_url: r.get(2)?,
        site_url: r.get(3)?,
        title: r.get(4)?,
        favicon_url: r.get(5)?,
        layout: r.get(6)?,
        auto_summary: r.get::<_, i64>(7)? != 0,
        auto_translate: r.get::<_, i64>(8)? != 0,
        fetch_failed: r.get::<_, i64>(9)? != 0,
        fetch_error: r.get(10)?,
        last_fetched_at: r.get(11)?,
    })
}

pub fn list_feeds(conn: &Connection) -> AppResult<Vec<FeedRow>> {
    let mut stmt = conn.prepare(&format!("SELECT {FEED_COLS} FROM feeds ORDER BY id"))?;
    let rows = stmt.query_map([], feed_row)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn find_feed_by_url(conn: &Connection, url: &str) -> AppResult<Option<i64>> {
    let id = conn
        .query_row("SELECT id FROM feeds WHERE feed_url = ?1", params![url], |r| r.get(0))
        .optional()?;
    Ok(id)
}

pub fn insert_feed(
    conn: &Connection,
    feed_url: &str,
    site_url: Option<&str>,
    title: &str,
    favicon_url: Option<&str>,
    folder_id: i64,
    layout: &str,
    auto_summary: bool,
    auto_translate: bool,
) -> AppResult<i64> {
    conn.execute(
        "INSERT INTO feeds (feed_url, site_url, title, favicon_url, folder_id, layout, auto_summary, auto_translate)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![feed_url, site_url, title, favicon_url, folder_id, layout, auto_summary as i64, auto_translate as i64],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn delete_feed(conn: &Connection, id: i64) -> AppResult<()> {
    conn.execute("DELETE FROM feeds WHERE id = ?1", params![id])?;
    Ok(())
}

/// 刷新结果落库：成功清零失败计数并立即可再抓；失败则递增计数并按
/// 指数退避（5min→30min→2h 封顶）推迟下次尝试。
pub fn set_feed_fetch_state(
    conn: &Connection,
    id: i64,
    failed: bool,
    error: Option<&str>,
    etag: Option<&str>,
    last_modified: Option<&str>,
) -> AppResult<()> {
    conn.execute(
        "UPDATE feeds SET
            fetch_failed = ?1,
            fetch_error = ?2,
            etag = ?3,
            last_modified = ?4,
            last_fetched_at = datetime('now'),
            fail_count = CASE WHEN ?1 THEN fail_count + 1 ELSE 0 END,
            next_retry_at = CASE
                WHEN ?1 THEN datetime('now', '+' || (CASE fail_count
                    WHEN 0 THEN 5 WHEN 1 THEN 5 WHEN 2 THEN 30 ELSE 120 END) || ' minutes')
                ELSE NULL END
         WHERE id = ?5",
        params![failed as i64, error, etag, last_modified, id],
    )?;
    Ok(())
}

/// 调度器取"到期"的源：超过全局间隔未抓 且 不在退避窗口内。
/// last_fetched_at 为 NULL（从未抓过）的源立即视为到期。
pub fn feeds_due_for_refresh(conn: &Connection, interval_min: i64) -> AppResult<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM feeds
         WHERE last_fetched_at IS NULL
            OR ( (julianday('now') - julianday(last_fetched_at)) * 1440.0 >= ?1
                 AND (next_retry_at IS NULL OR julianday('now') >= julianday(next_retry_at)) )",
    )?;
    let rows = stmt.query_map(params![interval_min], |r| r.get(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn set_feed_title_and_icon(conn: &Connection, id: i64, title: Option<&str>, favicon: Option<&str>, site_url: Option<&str>) -> AppResult<()> {
    let title = title.filter(|t| !t.trim().is_empty());
    let favicon = favicon.filter(|f| !f.trim().is_empty());
    // 只覆盖非空值：用户手动重命名的标题不被下一次抓取冲掉
    conn.execute(
        "UPDATE feeds SET
            title = COALESCE(?1, title),
            favicon_url = COALESCE(?2, favicon_url),
            site_url = COALESCE(?3, site_url)
         WHERE id = ?4",
        params![title, favicon, site_url, id],
    )?;
    Ok(())
}

pub fn update_feed_layout(conn: &Connection, id: i64, layout: &str) -> AppResult<()> {
    conn.execute("UPDATE feeds SET layout = ?1 WHERE id = ?2", params![layout, id])?;
    Ok(())
}

pub fn set_feed_ai_flags(conn: &Connection, id: i64, summary: bool, translate: bool) -> AppResult<()> {
    conn.execute(
        "UPDATE feeds SET auto_summary = ?1, auto_translate = ?2 WHERE id = ?3",
        params![summary as i64, translate as i64, id],
    )?;
    Ok(())
}

pub fn move_feed(conn: &Connection, id: i64, folder_id: i64) -> AppResult<()> {
    conn.execute(
        "UPDATE feeds SET folder_id = ?1 WHERE id = ?2",
        params![folder_id, id],
    )?;
    Ok(())
}

/// 直连失败的源（供 Miniflux 兜底路径查询）
#[allow(dead_code)] // Miniflux 兜底路径使用
pub fn feeds_fetch_failed(conn: &Connection) -> AppResult<Vec<FeedRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {FEED_COLS} FROM feeds WHERE fetch_failed = 1 ORDER BY id"
    ))?;
    let rows = stmt.query_map([], feed_row)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/* ============================================================
   Articles
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
    })
}

/// 列表查询参数：feed 范围 + 视图筛选 + 排序。
pub struct ArticleQuery {
    pub feed_id: Option<i64>,
    pub folder_id: Option<i64>,
    pub only_unread: bool,
    pub only_starred: bool,
    pub only_today: bool,
    pub newest_first: bool,
    pub limit: i64,
}

/// 列表条目（含 body_text 截断生成的 snippet）
pub fn list_articles(conn: &Connection, q: &ArticleQuery) -> AppResult<Vec<ArticleListItem>> {
    let mut sql = String::from(
        "SELECT a.id, a.feed_id, a.title, a.author,
                COALESCE(NULLIF(a.summary, ''), substr(a.body_text, 1, 280)) AS snippet,
                a.image_url, a.enclosure_url, a.enclosure_mime, a.duration_sec,
                a.ai_summary, a.source, a.published_at, a.is_read, a.is_starred
         FROM articles a",
    );
    // 值全部走绑定参数（占位符序号即绑定顺序），条件文本只拼固定字符串
    let mut where_clauses: Vec<&str> = vec![];
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
    if !where_clauses.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&where_clauses.join(" AND "));
    }
    sql.push_str(if q.newest_first {
        " ORDER BY COALESCE(a.published_at, a.fetched_at) DESC LIMIT ?"
    } else {
        " ORDER BY COALESCE(a.published_at, a.fetched_at) ASC LIMIT ?"
    });
    params.push(q.limit.into());

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(params), article_list_item)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
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

/// 全文搜索：FTS5 MATCH，命中按相关度（bm25）排序，返回与列表页同构的轻量行。
/// 查询词走 OR 连接（多关键词任一命中即返回）；空关键词返回空。
pub fn search_articles(conn: &Connection, query: &str, limit: i64) -> AppResult<Vec<ArticleListItem>> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    /* FTS5 语法字符（双引号/括号/星号等）会破坏 MATCH 表达式：
       逐词包裹双引号转义（"词"），任意词命中即可。 */
    let terms: Vec<String> = q
        .split_whitespace()
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect();
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let match_expr = terms.join(" OR ");

    let sql = format!(
        "SELECT a.id, a.feed_id, a.title, a.author,
                COALESCE(NULLIF(a.summary, ''), substr(a.body_text, 1, 280)) AS snippet,
                a.image_url, a.enclosure_url, a.enclosure_mime, a.duration_sec,
                a.ai_summary, a.source, a.published_at, a.is_read, a.is_starred
         FROM articles_fts f
         JOIN articles a ON a.id = f.rowid
         WHERE articles_fts MATCH ?
         ORDER BY bm25(articles_fts)
         LIMIT {limit}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![match_expr], article_list_item)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 抓取管线写入（带 feed_id）：guid 冲突时仅更新内容字段（正文/图片/enclosure/来源），
/// 已读/收藏/AI 产物等用户状态字段不动 —— 直连重抓到已读文章时不会"复活"它。
/// dedup=true 时同 URL 文章跨源去重（智能去重：同一新闻被多个源推送只留首个）。
pub fn upsert_article_with_feed(
    conn: &Connection,
    feed_id: i64,
    a: &NewArticle,
    dedup: bool,
) -> AppResult<(i64, bool)> {
    // 智能去重：URL 已存在于任一源 → 跳过（返回非新增，计数不膨胀）
    if dedup {
        if let Some(url) = a.url.as_deref().filter(|u| !u.is_empty()) {
            let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM articles WHERE url = ?1)",
                params![url],
                |r| r.get(0),
            )?;
            if exists {
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
                a.published_at
            ],
        )?;
        Ok((id, false))
    } else {
        conn.execute(
            "INSERT INTO articles
                (feed_id, guid, url, title, author, summary, content_html, body_text, image_url,
                 enclosure_url, enclosure_mime, duration_sec, published_at, source)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
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
                a.source
            ],
        )?;
        Ok((conn.last_insert_rowid(), true))
    }
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

/// 全部已读：作用于当前筛选范围（feed/folder/all），与前端「全部已读」按钮语义一致
pub fn mark_all_read(
    conn: &Connection,
    feed_id: Option<i64>,
    folder_id: Option<i64>,
) -> AppResult<usize> {
    let mut sql = String::from("UPDATE articles SET is_read = 1 WHERE is_read = 0");
    if let Some(fid) = feed_id {
        sql.push_str(&format!(" AND feed_id = {fid}"));
    }
    if let Some(folder) = folder_id {
        sql.push_str(&format!(
            " AND feed_id IN (SELECT id FROM feeds WHERE folder_id = {folder})"
        ));
    }
    let n = conn.execute(&sql, [])?;
    Ok(n)
}

/// 条目计数（侧边栏角标）：按 feed/分类聚合，含未读/收藏细分
#[derive(Debug, Serialize)]
pub struct FeedCounts {
    pub feed_id: i64,
    pub total: i64,
    pub unread: i64,
    pub starred: i64,
}

pub fn feed_counts(conn: &Connection) -> AppResult<Vec<FeedCounts>> {
    let mut stmt = conn.prepare(
        "SELECT feed_id, COUNT(*), SUM(is_read = 0), SUM(is_starred = 1)
         FROM articles GROUP BY feed_id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(FeedCounts {
            feed_id: r.get(0)?,
            total: r.get(1)?,
            unread: r.get::<_, Option<i64>>(2)?.unwrap_or(0),
            starred: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/* ============================================================
   Settings（键值对，Miniflux Endpoint/Token 等后续接这里）
   ============================================================ */

pub fn get_setting(conn: &Connection, key: &str) -> AppResult<Option<String>> {
    let v = conn
        .query_row("SELECT value FROM settings WHERE key = ?1", params![key], |r| {
            r.get(0)
        })
        .optional()?;
    Ok(v)
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> AppResult<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/* ============================================================
   Miniflux 同步 —— id 映射 + 离线变更队列
   ============================================================ */

/// 离线变更队列条目
#[derive(Debug, Serialize)]
pub struct SyncQueueItem {
    pub id: i64,
    pub article_id: Option<i64>,
    pub feed_url: Option<String>,
    pub action: String,
    pub payload: Option<String>,
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
    conn.execute(
        "INSERT INTO sync_queue (article_id, feed_url, action, payload) VALUES (?1, ?2, ?3, ?4)",
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
        "SELECT id, article_id, feed_url, action, payload FROM sync_queue ORDER BY id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(SyncQueueItem {
            id: r.get(0)?,
            article_id: r.get(1)?,
            feed_url: r.get(2)?,
            action: r.get(3)?,
            payload: r.get(4)?,
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

/// 按 Miniflux entry id 找本地条目（Pull 状态合并的匹配键之一）
pub fn article_by_miniflux_id(conn: &Connection, miniflux_id: i64) -> AppResult<Option<i64>> {
    let id = conn
        .query_row(
            "SELECT id FROM articles WHERE miniflux_id = ?1",
            params![miniflux_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(id)
}

/// 按 URL 找本地条目（Pull 合并的兜底匹配键）
pub fn article_id_by_url(conn: &Connection, url: &str) -> AppResult<Option<i64>> {
    let id = conn
        .query_row(
            "SELECT id FROM articles WHERE url = ?1 ORDER BY id LIMIT 1",
            params![url],
            |r| r.get(0),
        )
        .optional()?;
    Ok(id)
}

/// 绑定 Miniflux entry id（Pull 时首次见到该条目）
pub fn set_article_miniflux_id(conn: &Connection, id: i64, miniflux_id: i64) -> AppResult<()> {
    conn.execute(
        "UPDATE articles SET miniflux_id = ?1 WHERE id = ?2",
        params![miniflux_id, id],
    )?;
    Ok(())
}

/// 记录上次同步时间戳（Pull 增量游标，unix 秒）
pub fn last_sync_ts(conn: &Connection) -> AppResult<i64> {
    let v: Option<String> = conn
        .query_row("SELECT value FROM settings WHERE key = 'miniflux_last_sync'", [], |r| r.get(0))
        .optional()?;
    Ok(v.and_then(|s| s.parse().ok()).unwrap_or(0))
}

pub fn set_last_sync_ts(conn: &Connection, ts: i64) -> AppResult<()> {
    set_setting(conn, "miniflux_last_sync", &ts.to_string())
}

/// feeds/folders 的 miniflux_id 绑定
pub fn set_feed_miniflux_id(conn: &Connection, feed_id: i64, miniflux_id: i64) -> AppResult<()> {
    conn.execute(
        "UPDATE feeds SET miniflux_id = ?1 WHERE id = ?2",
        params![miniflux_id, feed_id],
    )?;
    Ok(())
}

pub fn set_folder_miniflux_id(conn: &Connection, folder_id: i64, miniflux_id: i64) -> AppResult<()> {
    conn.execute(
        "UPDATE folders SET miniflux_id = ?1 WHERE id = ?2",
        params![miniflux_id, folder_id],
    )?;
    Ok(())
}

/// 按 Miniflux feed id 找本地 feed
pub fn feed_by_miniflux_id(conn: &Connection, miniflux_id: i64) -> AppResult<Option<i64>> {
    let id = conn
        .query_row(
            "SELECT id FROM feeds WHERE miniflux_id = ?1",
            params![miniflux_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(id)
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

#[cfg(test)]
mod dedup_tests {
    use super::*;

    fn new_article(url: &str, guid: &str) -> NewArticle {
        NewArticle {
            guid: guid.into(),
            url: Some(url.into()),
            title: "t".into(),
            author: None,
            summary: None,
            content_html: None,
            body_text: "b".into(),
            image_url: None,
            enclosure_url: None,
            enclosure_mime: None,
            duration_sec: None,
            published_at: None,
            source: "direct".into(),
        }
    }

    /// 智能去重：同 URL 跨源只留首个；不同 URL 互不影响；同源 guid 冲突仍走更新。
    #[test]
    fn smart_dedup_blocks_cross_feed_same_url() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let f1 = create_folder(&conn, "A", "article").unwrap();
        let f2 = create_folder(&conn, "B", "article").unwrap();
        let feed1 = insert_feed(&conn, "https://x.example/f1", None, "f1", None, f1, "inherit", true, false).unwrap();
        let feed2 = insert_feed(&conn, "https://x.example/f2", None, "f2", None, f2, "inherit", true, false).unwrap();

        // feed1 首个入库
        let (id1, new1) = upsert_article_with_feed(&conn, feed1, &new_article("https://n.example/a", "g1"), true).unwrap();
        assert!(new1);

        // feed2 推来同 URL（不同 guid）→ dedup 拦截
        let (_, new2) = upsert_article_with_feed(&conn, feed2, &new_article("https://n.example/a", "g2"), true).unwrap();
        assert!(!new2, "same URL cross-feed must be blocked by dedup");

        // feed2 不同 URL → 正常入库
        let (_, new3) = upsert_article_with_feed(&conn, feed2, &new_article("https://n.example/b", "g3"), true).unwrap();
        assert!(new3);

        // dedup 关闭时同 URL 也会入库（保持既有行为）
        let (_, new4) = upsert_article_with_feed(&conn, feed2, &new_article("https://n.example/a", "g4"), false).unwrap();
        assert!(new4, "dedup off must not block");

        // 同源 guid 冲突 → 更新而非插入（was_new=false）
        let (_, new5) = upsert_article_with_feed(&conn, feed1, &new_article("https://n.example/a", "g1"), true).unwrap();
        assert!(!new5);
        let _ = id1;
    }
}
