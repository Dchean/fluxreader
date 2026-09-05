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

pub(crate) static MIGRATIONS: LazyLock<Migrations> = LazyLock::new(|| {
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
            auto_summary    INTEGER NOT NULL DEFAULT 0,
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
            action      TEXT NOT NULL,  -- 'read' | 'unread' | 'star' | 'unstar' | 'add_feed'（'remove_feed' 为历史遗留，已废弃：本地删除不推远端）
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
    // 智能去重墓碑：被丢弃的同 URL 文章记下「保留了哪篇」，关闭去重时
    // 清空墓碑（尊重用户想让重复文章回来的意图）。墓碑存在期间，任何抓取
    // 轮次重放同 URL 都直接跳过——否则 feed B 的 guid 稳定，每轮刷新都会
    // 把被去重的那篇重新插进来（关开关→重影的真正来源）。
    // url 列存规范化匹配键（v7 起）。user_version=6。
    M::up(r#"
        CREATE TABLE deduped_urls (
            url     TEXT PRIMARY KEY,
            kept_aid INTEGER NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
            kept_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
    "#),
    // 去重精确化：url_norm = URL 规范化匹配键（剥跟踪参数/www./m./尾斜杠/
    // AMP/锚点，https→http 统一），原始 url 保留用于「打开源网页」。
    // 同一篇被多个源用不同饰词引用时也能正确去重。
    // miniflux_dup_ids：服务端同文副本 entry 记账（逗号分隔）——双端场景
    // （Read You + FluxReader 共用 Miniflux）下，桌面端的已读/收藏变更
    // 广播到全部副本，手机上任意副本的已读也能被桌面正确跟随。
    // user_version=7。
    M::up(r#"
        ALTER TABLE articles ADD COLUMN url_norm TEXT;
        ALTER TABLE articles ADD COLUMN miniflux_dup_ids TEXT NOT NULL DEFAULT '';
        CREATE INDEX idx_articles_url_norm ON articles(url_norm);
        UPDATE articles SET url_norm = lower(url) WHERE url IS NOT NULL AND url != '';
    "#),
    // 后续迁移在此追加（M::up），已发布的不可改
    // 账号数据边界：feeds.origin 标记订阅来源（'local' 用户直连添加 |
    // 'miniflux' 从服务端拉取）。断开连接时删 miniflux 来源的订阅（级联
    // 清掉其文章/绑定/队列），本地直连订阅保留——换账号登录不会混杂两份
    // 订阅列表。user_version=8。
    M::up(r#"
        ALTER TABLE feeds ADD COLUMN origin TEXT NOT NULL DEFAULT 'local';
    "#),
    ])
});

/// 打开数据库并应用迁移。WAL 模式 + foreign_keys + busy_timeout。
pub fn open(path: &Path) -> AppResult<Connection> {
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    let prev_version = conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?;
    MIGRATIONS.to_latest(&mut conn)?;
    // v7 的 SQL 回填只是 lower(url) 占位；Rust 端 normalize_url 才是完整
    // 规范化（剥跟踪参数/www./AMP/锚点）。从 v6 及以下升级的库补一次精确回填
    // （v7 SQL 已建列，逐行 UPDATE 即可；新装库无行，零成本跳过）
    if prev_version > 0 && prev_version < 7 {
        backfill_url_norm(&conn)?;
    }
    // 启动迁移：历史明文敏感凭据升级为 DPAPI 密文（SEC-2）。幂等。
    let _ = crate::credentials::migrate_legacy_plaintext(&conn)?;
    Ok(conn)
}

/// 逐行用 normalize_url 重算 url_norm（v6→v7 升级路径）
fn backfill_url_norm(conn: &Connection) -> AppResult<()> {
    let rows: Vec<(i64, Option<String>)> = {
        let mut stmt =
            conn.prepare("SELECT id, url FROM articles WHERE url IS NOT NULL AND url != ''")?;
        let it = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        it.collect::<Result<Vec<_>, _>>()?
    };
    for (id, url) in rows {
        if let Some(u) = url {
            let _ = conn.execute(
                "UPDATE articles SET url_norm = ?1 WHERE id = ?2",
                params![normalize_url(&u), id],
            );
        }
    }
    Ok(())
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

#[derive(Debug, Clone, Serialize)]
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

/// 新增订阅源。参数即 feeds 表列（URL/站点/标题/分类/布局/AI 开关）——
/// 结构与表一一对应，收窄成结构体反而隔着一层，接受 9 参。
#[allow(clippy::too_many_arguments)]
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
    insert_feed_origin(conn, feed_url, site_url, title, favicon_url, folder_id, layout, auto_summary, auto_translate, "local")
}

/// 同 insert_feed，带来源标记（'local' 用户直连添加 | 'miniflux' 服务端拉取）。
/// 断开连接按此列清理服务端来源订阅（换账号不混杂）。
#[allow(clippy::too_many_arguments)]
pub fn insert_feed_origin(
    conn: &Connection,
    feed_url: &str,
    site_url: Option<&str>,
    title: &str,
    favicon_url: Option<&str>,
    folder_id: i64,
    layout: &str,
    auto_summary: bool,
    auto_translate: bool,
    origin: &str,
) -> AppResult<i64> {
    conn.execute(
        "INSERT INTO feeds (feed_url, site_url, title, favicon_url, folder_id, layout, auto_summary, auto_translate, origin)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![feed_url, site_url, title, favicon_url, folder_id, layout, auto_summary as i64, auto_translate as i64, origin],
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
/// 到期源 id。`include_miniflux = false`（跟随服务端同步模式）时跳过
/// origin='miniflux' 的源——服务端源的内容由 Miniflux 同步提供，
/// 直连抓取会与服务端状态产生两份不一致的真相。
pub fn feeds_due_for_refresh(
    conn: &Connection,
    interval_min: i64,
    include_miniflux: bool,
) -> AppResult<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM feeds
         WHERE (last_fetched_at IS NULL
            OR ( (julianday('now') - julianday(last_fetched_at)) * 1440.0 >= ?1
                 AND (next_retry_at IS NULL OR julianday('now') >= julianday(next_retry_at)) ))
           AND (?2 OR origin != 'miniflux')",
    )?;
    let rows = stmt.query_map(params![interval_min, include_miniflux], |r| r.get(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 全部源 id（托盘「刷新全部订阅」与手动全刷入口，忽略到期与退避）。
/// 手动入口始终包含 Miniflux 源（用户显式动作 = 要全部内容）。
pub fn feeds_all_ids(conn: &Connection, include_miniflux: bool) -> AppResult<Vec<i64>> {
    let mut stmt = conn.prepare("SELECT id FROM feeds WHERE (?1 OR origin != 'miniflux')")?;
    let rows = stmt.query_map(params![include_miniflux], |r| r.get(0))?;
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

/// 编辑源一次性落库：标题 / 所属分类 / 布局 / AI 开关（单语句，避免多次往返）
pub fn update_feed(
    conn: &Connection,
    id: i64,
    title: Option<&str>,
    folder_id: Option<i64>,
    layout: Option<&str>,
    auto_summary: Option<bool>,
    auto_translate: Option<bool>,
) -> AppResult<()> {
    let title = title.map(str::trim).filter(|t| !t.is_empty());
    // 空标题回退原值（不置 NULL——源名必填）
    conn.execute(
        "UPDATE feeds SET
            title = COALESCE(?1, title),
            folder_id = COALESCE(?2, folder_id),
            layout = COALESCE(?3, layout),
            auto_summary = COALESCE(?4, auto_summary),
            auto_translate = COALESCE(?5, auto_translate)
         WHERE id = ?6",
        params![
            title,
            folder_id,
            layout,
            auto_summary.map(|b| b as i64),
            auto_translate.map(|b| b as i64),
            id
        ],
    )?;
    Ok(())
}

pub fn set_feed_ai_flags(conn: &Connection, id: i64, summary: bool, translate: bool) -> AppResult<()> {
    conn.execute(
        "UPDATE feeds SET auto_summary = ?1, auto_translate = ?2 WHERE id = ?3",
        params![summary as i64, translate as i64, id],
    )?;
    Ok(())
}

/// 直连失败的源（供 Miniflux 兜底路径查询）
pub fn feeds_fetch_failed(conn: &Connection) -> AppResult<Vec<FeedRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {FEED_COLS} FROM feeds WHERE fetch_failed = 1 ORDER BY id"
    ))?;
    let rows = stmt.query_map([], feed_row)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 服务端来源（origin='miniflux'）的源：内容完全由 Miniflux 提供，本地不直连
/// 抓取。同步时**全量拉取**其条目（幂等 upsert），保证数量与状态与 Miniflux
/// 完全对齐——用 `after`（published_at）增量会漏掉发布时间早于游标的历史文章。
/// 仅返回已绑定 miniflux_id 的源。
pub fn feeds_origin_miniflux(conn: &Connection) -> AppResult<Vec<FeedRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {FEED_COLS} FROM feeds
         WHERE origin = 'miniflux' AND miniflux_id IS NOT NULL
         ORDER BY id"
    ))?;
    let rows = stmt.query_map([], feed_row)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 直连失败（fetch_failed=1）且已绑定 miniflux_id 的源：直连失败走 Miniflux
/// 兜底，增量拉取（after=上次同步时间）补直连漏掉的条目。
pub fn feeds_fetch_failed_bound(conn: &Connection) -> AppResult<Vec<FeedRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {FEED_COLS} FROM feeds
         WHERE fetch_failed = 1 AND miniflux_id IS NOT NULL
         ORDER BY id"
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
pub fn search_articles(conn: &Connection, query: &str, limit: i64) -> AppResult<Vec<ArticleListItem>> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let terms: Vec<String> = q
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .map(|t| t.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"))
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
                a.ai_summary, a.source, a.published_at, a.is_read, a.is_starred
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

/// 断开连接时清理服务端来源的数据：删 origin='miniflux' 的订阅（级联清
/// 其文章/绑定/队列/墓碑），清空本地条目上的 Miniflux 绑定与副本记账、
/// folders/feeds 的 miniflux_id 残留。用户直连订阅（origin='local'）保留。
/// 空目录（pull 建的、没了成员）一并删除。
pub fn purge_miniflux_data(conn: &mut Connection) -> AppResult<(usize, usize)> {
    let tx = conn.transaction()?;
    // 1. 服务端来源订阅（级联：articles → sync_queue / deduped_urls 墓碑 / FTS 触发器）
    let feeds = tx.execute("DELETE FROM feeds WHERE origin = 'miniflux'", [])?;
    // 2. 本地直连条目上的绑定/副本/已读态全部回归纯本地
    let articles = tx.execute(
        "UPDATE articles SET miniflux_id = NULL, miniflux_dup_ids = ''",
        [],
    )?;
    // 3. 本地直连源/分类的 miniflux_id 绑定清除
    tx.execute("UPDATE feeds SET miniflux_id = NULL", [])?;
    tx.execute("UPDATE folders SET miniflux_id = NULL", [])?;
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
    let cutoff = format!(
        "datetime('now', '-{days} days', 'localtime')"
    );
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
        tx.execute("DELETE FROM deduped_urls WHERE kept_aid NOT IN (SELECT id FROM articles)", [])?;
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

/// 已知跟踪/统计参数（utm 系 + 各家统计 SDK）。剥掉后不影响定位同一篇文章。
const TRACKING_PARAMS: &[&str] = &[
    "utm_source", "utm_medium", "utm_campaign", "utm_term", "utm_content",
    "utm_id", "utm_name", "utm_cid", "utm_reader", "utm_social",
    "gclid", "gclsrc", "dclid", "gbraid", "wbraid",           // Google Ads
    "fbclid", "fb_action_ids", "fb_action_types", "fb_source", // Facebook
    "igshid", "igsh",                                          // Instagram
    "twclid", "t", "s",                                        // X/Twitter（t/s 短链跳转带参）
    "mc_cid", "mc_eid",                                        // Mailchimp
    "ref", "ref_src", "ref_url", "referrer",                   // 引荐来源
    "spm_id", "scm", "share_token", "nsfrom", "nstoken",       // 国内生态（掘金/微信/知乎）
    "share_source", "tt_from", "group_id", "web_chapter_id",
];

/// URL 规范化为去重匹配键：同文不同饰（跟踪参数/协议/www./m./尾斜杠/AMP）
/// 归一为一个键。失败（非 URL 形态）返回原串小写——匹配键退化但可用。
/// 规则从宽到严排序：只做「无损压缩」，绝不合并可能不同的文章。
pub fn normalize_url(url: &str) -> String {
    let Ok(mut u) = url::Url::parse(url.trim()) else {
        return url.trim().to_lowercase();
    };
    // https 统一（http 降级为同一篇；其他 scheme 保留原样区分）
    if u.scheme() == "https" {
        let _ = u.set_scheme("http");
    }
    // 规整 host：www./m./mobile. 前缀剥掉（多数站点移动/桌面同文）
    if let Some(host) = u.host_str() {
        let trimmed = host
            .strip_prefix("www.")
            .or_else(|| host.strip_prefix("m."))
            .or_else(|| host.strip_prefix("mobile."));
        if let Some(t) = trimmed {
            let port = u.port().map(|p| format!(":{p}")).unwrap_or_default();
            let _ = u.set_host(Some(&format!("{t}{port}")));
        }
    }
    // 跟踪参数剥离
    let filtered: Vec<(String, String)> = u
        .query_pairs()
        .filter(|(k, _)| {
            let k = k.to_lowercase();
            !TRACKING_PARAMS.contains(&k.as_str())
        })
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if filtered.is_empty() {
        u.set_query(None);
    } else {
        let mut q = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in &filtered {
            q.append_pair(k, v);
        }
        u.set_query(Some(&q.finish()));
    }
    // 尾斜杠归一（/a/ 与 /a 同文）；AMP 页归一（/amp/x → /x）
    let mut path = u.path().trim_end_matches('/').to_string();
    if let Some(rest) = path.strip_prefix("/amp") {
        if rest.is_empty() || rest.starts_with('/') {
            path = rest.to_string();
        }
    }
    u.set_path(&path);
    // fragment 无定位意义（纯锚点），丢弃
    u.set_fragment(None);
    u.to_string()
}

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
    } else {
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
    // 敏感键读时解密（SEC-2）；历史明文无前缀则原样返回（兼容）
    Ok(v.map(|raw: String| {
        if crate::credentials::is_sensitive_key(key) {
            crate::credentials::decrypt_secret(&raw)
        } else {
            raw
        }
    }))
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> AppResult<()> {
    // 敏感键写时加密（SEC-2）：DPAPI 加密后落库，读 DB 不见明文
    let stored = if crate::credentials::is_sensitive_key(key) {
        crate::credentials::encrypt_secret(value)
    } else {
        value.to_string()
    };
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, stored],
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

/// 清掉历史版本残留的 remove_feed 僵尸队项（该动作从未被 sync 消费；现行
/// 删除语义为「本地删除不推远端」，见 delete_feed 命令）。返回清除条数。
pub fn purge_remove_feed_zombies(conn: &Connection) -> AppResult<usize> {
    let n = conn.execute("DELETE FROM sync_queue WHERE action = 'remove_feed'", [])?;
    Ok(n)
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

/// URL 兜底匹配的安全校验：本地文章（aid）与远端 entry（mf_entry_id 所属
/// feed mf_feed_id）是否同一订阅源。同源 → 服务端说的是同一篇，可合并状态；
/// 跨源 → URL 碰巧相同但属于另一个订阅的 entry，只有已绑定的那条才有权
/// 写状态（防止未读状态从服务端另一条同 URL entry 复活已读文章）。
/// 判定依据：aid 已绑定的 miniflux_id 所属远端 feed（feeds.miniflux_id）
/// 与远端 entry 的 feed_id 一致，或 aid 尚未绑定（首见，允许建立绑定）。
pub fn article_matches_remote_feed(
    conn: &Connection,
    aid: i64,
    mf_feed_id: i64,
) -> AppResult<bool> {
    let bound: Option<(Option<i64>, Option<i64>)> = conn
        .query_row(
            "SELECT a.miniflux_id, f.miniflux_id FROM articles a
             JOIN feeds f ON f.id = a.feed_id WHERE a.id = ?1",
            params![aid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    match bound {
        // 文章或 feed 已不存在 → 不匹配（保守）
        None => Ok(false),
        // 文章已绑定 entry：entry 必须属于同一（远端）feed 才可信
        Some((Some(_entry_id), feed_mf)) => Ok(feed_mf == Some(mf_feed_id)),
        // 未绑定：首见，允许（绑定回填/兜底合并的正常路径）
        Some((None, _)) => Ok(true),
    }
}

/// 按 URL 找本地条目（Pull 合并的兜底匹配键）。
/// 用规范化 URL（url_norm）匹配：Miniflux 返回的条目 URL 与本地直连抓取的
/// URL 常有跟踪参数/www./m./尾斜杠/AMP/https 等差异，精确匹配会漏判成新条目
/// 导致同文重复入库（文章数虚高 + 状态对不齐）。与去重键同源同口径。
pub fn article_id_by_url(conn: &Connection, url: &str) -> AppResult<Option<i64>> {
    let id = conn
        .query_row(
            "SELECT id FROM articles WHERE url_norm = ?1 ORDER BY id LIMIT 1",
            params![normalize_url(url)],
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

/// 记账服务端同文副本 entry（跨源同 URL 的另一条 entry）。
/// 幂等：已在列表中不重复；上限 16 个防脏数据撑爆字段。
pub fn add_article_dup_entry(conn: &Connection, id: i64, dup_entry_id: i64) -> AppResult<()> {
    if dup_entry_id <= 0 {
        return Ok(());
    }
    let cur: String = conn
        .query_row(
            "SELECT COALESCE(miniflux_dup_ids, '') FROM articles WHERE id = ?1",
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
        "UPDATE articles SET miniflux_dup_ids = ?1 WHERE id = ?2",
        params![joined, id],
    )?;
    Ok(())
}

/// 读某文章的副本 entry 列表（广播已读/收藏用）
pub fn article_dup_entries(conn: &Connection, id: i64) -> AppResult<Vec<i64>> {
    let cur: Option<String> = conn
        .query_row(
            "SELECT miniflux_dup_ids FROM articles WHERE id = ?1",
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

/// 是否存在「已入队未推送」的本地状态变更（读/收藏）。
/// 有 → 拉取状态时跳过该文章（本地变更优先推送，防止被服务端旧状态覆盖
/// 回来造成乒乓）。绑定回填后下一轮同步即恢复合并。
pub fn article_has_pending_sync(conn: &Connection, id: i64) -> AppResult<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sync_queue WHERE article_id = ?1 AND action IN ('read','unread','star','unstar')",
        params![id],
        |r| r.get(0),
    )?;
    Ok(n > 0)
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
    /// 搜索（LIKE 子串）：中文子串、多词 AND、通配符转义。
    /// 修复背景：unicode61 FTS 把整段中文当一个 token，搜「科技」匹配不到
    /// 「科技公司新闻」——改为子串匹配后语义对任意语言正确。
    #[test]
    fn search_finds_chinese_substring_and_multi_term_and() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let f = create_folder(&conn, "F", "article").unwrap();
        let feed = insert_feed(&conn, "https://x.example/f", None, "f", None, f, "inherit", true, false).unwrap();

        let art = |url: &str, guid: &str, title: &str, body: &str| {
            let mut a = new_article(url, guid);
            a.title = title.into();
            a.body_text = body.into();
            let _ = upsert_article_with_feed(&conn, feed, &a, false).unwrap();
        };
        art("https://n.example/1", "g1", "科技公司新闻", "今天发布了新产品");
        art("https://n.example/2", "g2", "无关标题", "正文提到了科技公司");
        art("https://n.example/3", "g3", "另一个", "完全没有相关内容");

        // 中文子串：标题或正文含「科技」都命中（FTS 时代这条是失败的）
        let r1 = search_articles(&conn, "科技", 50).unwrap();
        assert_eq!(r1.len(), 2, "chinese substring must match both: {:?}", r1.iter().map(|a| &a.title).collect::<Vec<_>>());

        // 多词 AND：两个词都命中才返回
        let r2 = search_articles(&conn, "科技 产品", 50).unwrap();
        assert_eq!(r2.len(), 1, "AND semantics: {:?}", r2.iter().map(|a| &a.title).collect::<Vec<_>>());
        assert_eq!(r2[0].title, "科技公司新闻");

        // 无命中
        let r3 = search_articles(&conn, "不存在的词", 50).unwrap();
        assert!(r3.is_empty());

        // LIKE 通配符按字面转义：存 % 和 _ 的标题不被 % 通配误命中
        art("https://n.example/4", "g4", "100%_安全", "特殊字符");
        let r4 = search_articles(&conn, "100%", 50).unwrap();
        assert_eq!(r4.len(), 1, "literal %% must match: {}", r4.len());
        assert_eq!(r4[0].title, "100%_安全");
        // 单下划线词不当作通配符命中任意单字符
        let r5 = search_articles(&conn, "100X_安全", 50).unwrap();
        assert!(r5.is_empty(), "underscore must be literal, not wildcard");

        // 含 FTS 特殊字符的词安全
        art("https://n.example/5", "g5", "node.js 指南", "C++ 与 Rust 对比");
        let r6 = search_articles(&conn, "node.js", 50).unwrap();
        assert_eq!(r6.len(), 1);
        let r7 = search_articles(&conn, "C++", 50).unwrap();
        assert_eq!(r7.len(), 1);
    }

    /// 搜索命中 AI 摘要/翻译（SRH-2）：正文/标题都不含关键词、仅 AI 产物含时也要命中。
    #[test]
    fn search_finds_ai_summary_and_translation() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let f = create_folder(&conn, "F", "article").unwrap();
        let feed = insert_feed(&conn, "https://x.example/f", None, "f", None, f, "inherit", true, false).unwrap();

        let mut a = new_article("https://n.example/1", "g1");
        a.title = "普通标题".into();
        a.body_text = "正文不含关键词".into();
        let (id, _) = upsert_article_with_feed(&conn, feed, &a, false).unwrap();

        // 仅摘要含关键词
        set_article_ai_fields(&conn, id, Some("这篇讲的是量子计算的突破"), None).unwrap();
        let r1 = search_articles(&conn, "量子计算", 50).unwrap();
        assert_eq!(r1.len(), 1, "ai_summary 命中");

        // 仅译文含关键词
        set_article_ai_fields(&conn, id, None, Some("译文里提到了深度学习模型")).unwrap();
        let r2 = search_articles(&conn, "深度学习", 50).unwrap();
        assert_eq!(r2.len(), 1, "translated_content 命中");

        // 无关词不命中
        let r3 = search_articles(&conn, "不存在的词", 50).unwrap();
        assert!(r3.is_empty());
    }
}
