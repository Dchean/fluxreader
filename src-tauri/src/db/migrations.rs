use super::*;
use rusqlite::params;

pub(crate) static MIGRATIONS: LazyLock<Migrations> = LazyLock::new(|| {
    Migrations::new(vec![
        M::up(
            r#"
        CREATE TABLE folders (
            id            INTEGER PRIMARY KEY,
            name          TEXT NOT NULL,
            position      INTEGER NOT NULL DEFAULT 0,
            layout        TEXT NOT NULL DEFAULT 'article',
            auto_summary  INTEGER NOT NULL DEFAULT 0,
            auto_translate INTEGER NOT NULL DEFAULT 0,
            collapsed     INTEGER NOT NULL DEFAULT 1,
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
    "#,
        ),
        // Miniflux 同步支持 —— 条目/源/分类的 Miniflux id 映射 + 离线变更队列
        M::up(
            r#"
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
    "#,
        ),
        // FTS5 全文索引：标题/正文纯文本/作者/AI 摘要/翻译。触发器保持与 articles 同步，
        // user_version=3。unicode61 分词器：中文按字、英文按词，个人规模足够（无需 ICU）。
        M::up(
            r#"
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
    "#,
        ),
        // 后台刷新调度：失败计数 + 下次重试时间（指数退避 5min→30min→2h）。
        // user_version=4。
        M::up(
            r#"
        ALTER TABLE feeds ADD COLUMN fail_count INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE feeds ADD COLUMN next_retry_at TEXT;
    "#,
        ),
        // 全文提取标志：1 = 正文已被 Readability 全文覆盖（工具栏按钮状态与
        // 设置「自动全文」共用此标志，重启不丢）。user_version=5。
        M::up("ALTER TABLE articles ADD COLUMN fulltext_extracted INTEGER NOT NULL DEFAULT 0;"),
        // 智能去重墓碑：被丢弃的同 URL 文章记下「保留了哪篇」，关闭去重时
        // 清空墓碑（尊重用户想让重复文章回来的意图）。墓碑存在期间，任何抓取
        // 轮次重放同 URL 都直接跳过——否则 feed B 的 guid 稳定，每轮刷新都会
        // 把被去重的那篇重新插进来（关开关→重影的真正来源）。
        // url 列存规范化匹配键（v7 起）。user_version=6。
        M::up(
            r#"
        CREATE TABLE deduped_urls (
            url     TEXT PRIMARY KEY,
            kept_aid INTEGER NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
            kept_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
    "#,
        ),
        // 去重精确化：url_norm = URL 规范化匹配键（剥跟踪参数/www./m./尾斜杠/
        // AMP/锚点，https→http 统一），原始 url 保留用于「打开源网页」。
        // 同一篇被多个源用不同饰词引用时也能正确去重。
        // miniflux_dup_ids：服务端同文副本 entry 记账（逗号分隔）——双端场景
        // （Read You + FluxReader 共用 Miniflux）下，桌面端的已读/收藏变更
        // 广播到全部副本，手机上任意副本的已读也能被桌面正确跟随。
        // user_version=7。
        M::up(
            r#"
        ALTER TABLE articles ADD COLUMN url_norm TEXT;
        ALTER TABLE articles ADD COLUMN miniflux_dup_ids TEXT NOT NULL DEFAULT '';
        CREATE INDEX idx_articles_url_norm ON articles(url_norm);
        UPDATE articles SET url_norm = lower(url) WHERE url IS NOT NULL AND url != '';
    "#,
        ),
        // 后续迁移在此追加（M::up），已发布的不可改
        // 账号数据边界：feeds.origin 标记订阅来源（'local' 用户直连添加 |
        // 'miniflux' 从服务端拉取）。断开连接时删 miniflux 来源的订阅（级联
        // 清掉其文章/绑定/队列），本地直连订阅保留——换账号登录不会混杂两份
        // 订阅列表。user_version=8。
        M::up(
            r#"
        ALTER TABLE feeds ADD COLUMN origin TEXT NOT NULL DEFAULT 'local';
    "#,
        ),
        // 「跟随服务端」（hybrid）模式下，本地直连添加的源（origin='local'）绑定
        // Miniflux 后转为服务端来源（origin='miniflux'，内容由 Miniflux 提供）。
        // 但断开连接时需把这类源恢复为 'local'（保留本地直连订阅），而非删除——
        // origin_was_local 标记「原本是本地直连添加」。user_version=9。
        M::up(
            r#"
        ALTER TABLE feeds ADD COLUMN origin_was_local INTEGER NOT NULL DEFAULT 0;
    "#,
        ),
        // 订阅源分组默认折叠：新库建表 DEFAULT 已改为 1，这里把已有库的分类
        // 统一折叠（用户诉求：分组默认收起，腾出滚动区给订阅源列表）。user_version=10。
        M::up(
            r#"
        UPDATE folders SET collapsed = 1;
    "#,
        ),
        // 清理历史重复：旧版在「本地抓取」模式下直连抓取 origin='miniflux' 源，
        // 产生 source='direct' 文章与已有的 source='miniflux' 文章 URL 重复
        // （guid 不同 + 智能去重默认关），导致文章翻倍、状态错乱。删除这些重复的
        // direct 文章（内容/状态已由同 URL 的 miniflux 文章承载）。user_version=11。
        M::up(
            r#"
        DELETE FROM articles
         WHERE source = 'direct'
           AND url_norm IN (SELECT url_norm FROM articles WHERE source = 'miniflux');
    "#,
        ),
        // 回填缺失发布时间：某些 RSS 源不提供 pubDate/updated（如 kirikira.moe），
        // 历史入库的 direct 文章 published_at 为 NULL，前端 publishedAt=0 显示成
        // 1970-01-01、「今天」过滤与排序失准。用 fetched_at（抓取时间）兜底回填，
        // 与 map_entry 的新抓取兜底逻辑（Utc::now）口径一致。user_version=12。
        M::up(
            r#"
        UPDATE articles
           SET published_at = fetched_at
         WHERE published_at IS NULL OR published_at = '';
    "#,
        ),
        // 协议中立化：miniflux_id → remote_id、miniflux_dup_ids → remote_dup_ids、
        // origin='miniflux' → origin='remote'。同步层从 Miniflux 专用协议迁移到
        // 标准协议（Google Reader / Fever），后端可替换（Miniflux/FreshRSS/自建）。
        // 物理列用 RENAME COLUMN（SQLite 3.25+，bundled 3.46 支持），数据无损。
        // user_version=13。
        M::up(
            r#"
        ALTER TABLE articles RENAME COLUMN miniflux_id TO remote_id;
        ALTER TABLE articles RENAME COLUMN miniflux_dup_ids TO remote_dup_ids;
        ALTER TABLE feeds RENAME COLUMN miniflux_id TO remote_id;
        ALTER TABLE folders RENAME COLUMN miniflux_id TO remote_id;
        UPDATE feeds SET origin = 'remote' WHERE origin = 'miniflux';
    "#,
        ),
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
