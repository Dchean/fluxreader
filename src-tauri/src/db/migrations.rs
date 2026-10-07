use super::*;
use rusqlite::params;

/// P3-7（自检 2026-09-29）v15 迁移语句：把 legacy SQLite 格式的 published_at
/// 归一为现行写入格式。抽成常量供幂等测试复用**同一生产字节**，防测试与迁移漂移。
/// 命中与改写细节见下方 v15 M::up 注释。
pub(crate) const V15_NORMALIZE_PUBLISHED_AT_SQL: &str = r#"
        -- legacy 'YYYY-MM-DD HH:MM:SS'（UTC，来自 v12/v14 的 fetched_at 回填）→
        -- 'YYYY-MM-DDTHH:MM:SS+00:00'（与 map_entry / item_published_at 的
        -- to_rfc3339() 逐字一致；秒级精度不带小数，因 to_rfc3339 的 AutoSi
        -- 在纳秒为 0 时省略小数部分）。
        UPDATE articles
           SET published_at = replace(published_at, ' ', 'T') || '+00:00'
         WHERE published_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9] [0-9][0-9]:[0-9][0-9]:[0-9][0-9]';
    "#;

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
        // REQ-108（M-5 / M-7 / M-9 前置）：列表排序走索引 + sync_queue 索引
        // + published_at 写入兜底。user_version=14。
        //
        // M-5：列表查询此前以 ORDER BY COALESCE(published_at, fetched_at) 排序，
        // 表达式排序让 idx_articles_published 对查询变体失效（SCAN +
        // USE TEMP B-TREE FOR ORDER BY，实测见 tmp/task-099/）。排序退化为纯
        // ORDER BY published_at 的前提是 published_at 非空：v12 已回填存量行，
        // 应用层写入经 map_entry / item_published_at 兜底，但 SQL 裸写路径
        // （测试夹具、历史回放）仍可能留下 NULL/''——故这里除幂等回填外，
        // 用触发器把「published_at 非空」固化为库级不变量，使新排序与旧
        // COALESCE 口径对任意写入路径逐行等价。
        //
        // 触发顺序实测（tmp/task-099/trigger_probe.log）：SQLite 同表同事件
        // 触发器按创建逆序执行，v14 后建的兜底触发器会先于 v3 的 articles_au 跑；
        // 旧 articles_au 对任意 UPDATE 都做 FTS delete+insert，会对「articles_ai
        // 尚未写入 FTS 的新行」执行 'delete' → database disk image is malformed。
        // FTS 行内容仅由 title/body_text/author/ai_summary/translated_content
        // 五列决定，故把 articles_au 收窄为 AFTER UPDATE OF 这五列：兜底 UPDATE
        // 不再触发 FTS 同步（旧行为是 delete+insert 相同内容，纯 churn），
        // 与触发器创建顺序解耦，语义零变化（FTS 自 REQ-104 起已非搜索入口）。
        //
        // M-7：sync_queue 建表以来零索引——每次 set_read/set_starred 的
        // enqueue_sync 都对 DELETE ... WHERE article_id = ? AND action IN (...)
        // 做全表 SCAN，pull 对账的 pending 集合与老化清理（created_at 过滤）同样
        // 全表扫。补覆盖索引 (article_id, action) 与 (created_at)：DEL 变
        // SEARCH、PEND 变 COVERING INDEX 扫描（免 TEMP B-TREE FOR DISTINCT）、
        // AGE 走 MULTI-INDEX OR；索引集取舍对比见 tmp/task-099/m7_*.log。
        //
        // 另补 (is_read, published_at)：未读视图（only_unread）此前走
        // idx_articles_unread 后仍需 TEMP B-TREE 排序，该索引使未读列表按序
        // SEARCH 直达（idx_articles_unread 保留给 mark_all_read 的 UPDATE 计划）。
        M::up(
            r#"
        UPDATE articles SET published_at = fetched_at
         WHERE published_at IS NULL OR published_at = '';

        DROP TRIGGER IF EXISTS articles_au;
        CREATE TRIGGER articles_au AFTER UPDATE OF title, body_text, author, ai_summary, translated_content ON articles BEGIN
            INSERT INTO articles_fts(articles_fts, rowid, title, body_text, author, ai_summary, translated_content)
            VALUES ('delete', old.id, old.title, old.body_text, COALESCE(old.author, ''),
                    COALESCE(old.ai_summary, ''), COALESCE(old.translated_content, ''));
            INSERT INTO articles_fts(rowid, title, body_text, author, ai_summary, translated_content)
            VALUES (new.id, new.title, new.body_text, COALESCE(new.author, ''),
                    COALESCE(new.ai_summary, ''), COALESCE(new.translated_content, ''));
        END;

        CREATE TRIGGER trg_articles_published_fallback_ins AFTER INSERT ON articles
        WHEN NEW.published_at IS NULL OR NEW.published_at = ''
        BEGIN
            UPDATE articles SET published_at = NEW.fetched_at WHERE id = NEW.id;
        END;
        CREATE TRIGGER trg_articles_published_fallback_upd AFTER UPDATE OF published_at ON articles
        WHEN NEW.published_at IS NULL OR NEW.published_at = ''
        BEGIN
            UPDATE articles SET published_at = NEW.fetched_at WHERE id = NEW.id;
        END;

        CREATE INDEX idx_articles_feed_published ON articles(feed_id, published_at);
        CREATE INDEX idx_articles_read_published ON articles(is_read, published_at);

        CREATE INDEX idx_sync_queue_article ON sync_queue(article_id, action);
        CREATE INDEX idx_sync_queue_created ON sync_queue(created_at);
    "#,
        ),
        // P3-7（自检 2026-09-29）：存量库 legacy 时间格式归一。v12/v14 回填把
        // NULL/'' 的 published_at 写成 fetched_at 的 SQLite 格式
        // 'YYYY-MM-DD HH:MM:SS'（UTC），而现行写入是 RFC3339 UTC
        // （ingestion/parse.rs map_entry :91-97 与 sync/entries.rs
        // item_published_at :38-46 的 to_rfc3339() → 'YYYY-MM-DDTHH:MM:SS[.fff]+00:00'）。
        // 混排两宗害：① ORDER BY published_at 是字符串比较，同日内 'T' > ' ' 使
        // RFC3339 行恒排在 legacy 行之后（无视真实时刻）；② 前端 Date.parse 空格
        // 格式按本地时区解析（+08:00 用户看到偏移）。归一语句见
        // V15_NORMALIZE_PUBLISHED_AT_SQL（同一常量，测试共用同一生产字节）：
        // GLOB 模式不含通配符 → 整串匹配，只命中 19 字符空格形态；RFC3339 行
        // 含 'T'/'+00:00'/小数均不命中 → 迁移天然幂等，重跑 no-op。生产写入
        // 路径（抓取 map_entry / 同步 item_published_at）恒写 RFC3339，归一后
        // 不会再产生该形态（v14 兜底触发器只对裸 SQL 写 NULL/'' 的路径生效，
        // 重引入场景由幂等测试覆盖）。前端 parseTs 对空格格式的解析兼容由
        // 前端轨道负责，本迁移只做后端归一。user_version=15。
        M::up(V15_NORMALIZE_PUBLISHED_AT_SQL),
        // TASK-116（同步四态展示）：sync_queue 增推送失败标记——attempts（失败
        // 次数，每次推送失败 +1）与 last_error（截断后的最近错误摘要）。此前失败
        // 只进聚合 SyncReport.errors 与日志，无 per-item 痕迹，UI 无法区分
        // 「等待同步」与「部分失败」。成功即 prune 出队（远端确认口径，不可累计
        // 溯源，stats 只报现存行）；状态变更重新入队会删旧行插新行，失败计数
        // 随之归零（新变更 = 新尝试）。旧库升级：既有行 attempts=0 /
        // last_error=NULL，即「等待同步」，与修前语义一致。user_version=16。
        M::up(
            r#"
        ALTER TABLE sync_queue ADD COLUMN attempts INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE sync_queue ADD COLUMN last_error TEXT;
    "#,
        ),
        // TASK-117（审计 P1-1：keyset 分页）：列表 ORDER 补 id 决胜后
        // （PUBLISHED_ORDER_DESC/ASC = published_at, id），无等值前缀的查询变体
        // （全部/按分类/布局）在既有 idx_articles_published（published_at DESC 声明，
        // 隐式 rowid ASC 尾巴）上给不出 (published_at DESC, id DESC)——反向扫描的
        // rowid 方向与声明方向相反，SQLite 需 TEMP B-TREE 补排最后一项（实测
        // EXPLAIN QUERY PLAN：USE TEMP B-TREE FOR LAST TERM OF ORDER BY）。
        // 本索引按 (published_at, id) 双 ASC 声明：正向扫描供 ASC + id ASC，反向
        // 扫描供 DESC + id DESC，两方向都免排序；带等值前缀的查询变体（feed/unread）
        // 仍由 v14 的 (feed_id, published_at) / (is_read, published_at) 反向扫描有序
        // 驱动（计划断言见 db/articles.rs 的 list_query_plan_*，keyset 谓词变体同测）。
        // 纯增索引：不改表数据、不动任何既有索引，旧版本语义零变化。user_version=17。
        M::up(
            r#"
        CREATE INDEX idx_articles_published_id ON articles(published_at, id);
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
    MIGRATIONS.to_latest(&mut conn)?;
    // v7 的 SQL 回填只是 lower(url) 占位；Rust 端 normalize_url 才是完整
    // 规范化（剥跟踪参数/www./AMP/锚点）。M-14：完成判定改用 settings 标记
    // 而非 user_version——旧实现以 prev_version < 7 为闸门，回填在迁移事务外
    // 逐行提交，半途中断后 user_version 已 ≥7，回填永不重试。现在标记与回填
    // 同事务落标，「已升级但标记缺失」的库（含已停在 v7+ 的存量库）启动即
    // 幂等补跑；已标记库直接跳过（零重复工作）。
    ensure_url_norm_backfill(&conn)?;
    // 启动迁移：历史明文敏感凭据升级为 DPAPI 密文（SEC-2）。幂等。
    let _ = crate::credentials::migrate_legacy_plaintext(&conn)?;
    Ok(conn)
}

/// 测试注入点（迁移中断复现）：≥0 时在更新第 N 行**之前**模拟回填失败，
/// 生产恒为 -1。见 [`ensure_url_norm_backfill`] 与对应单元测试。
#[cfg(test)]
pub(crate) static BACKFILL_FAIL_AFTER_ROWS: std::sync::atomic::AtomicIsize =
    std::sync::atomic::AtomicIsize::new(-1);

/// 中断注入判定（cfg 双版本保持调用点无条件编译一致）。
#[cfg(test)]
fn backfill_should_fail(rows_updated: usize) -> bool {
    use std::sync::atomic::Ordering;
    let fail_at = BACKFILL_FAIL_AFTER_ROWS.load(Ordering::SeqCst);
    fail_at >= 0 && rows_updated as isize == fail_at
}

#[cfg(not(test))]
fn backfill_should_fail(_rows_updated: usize) -> bool {
    false
}

/// url_norm 完整规范化回填（M-14，幂等可重入）：对 url 非空的行重算
/// normalize_url，**只 UPDATE 结果确有变化的行**（已规范化库零写放大），
/// 并在同一事务内落 settings 完成标记。返回是否实际执行了回填。
///
/// 与旧版 [`backfill_url_norm`] 的差别：① 回填 + 落标同事务——任何语句失败
/// 整体回滚，下次启动按「标记缺失」重试，不再出现「迁移已提交、回填半途
/// 而废、永不重试」的窗口；② 逐行错误不再被 `let _ =` 吞掉（吞错正是旧版
/// 「静默半完成」的来源）；③ 已标记库直接跳过，重启零重复工作。
fn ensure_url_norm_backfill(conn: &Connection) -> AppResult<bool> {
    const MARKER_KEY: &str = "url_norm_backfill_done";
    let done: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM settings WHERE key = ?1)",
            params![MARKER_KEY],
            |r| r.get::<_, i64>(0),
        )
        .map(|v| v != 0)?;
    if done {
        return Ok(false);
    }
    let tx = conn.unchecked_transaction()?;
    let rows: Vec<(i64, String, Option<String>)> = {
        let mut stmt = tx.prepare(
            "SELECT id, url, url_norm FROM articles WHERE url IS NOT NULL AND url != ''",
        )?;
        let it = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        it.collect::<Result<Vec<_>, _>>()?
    };
    let mut updated = 0usize;
    for (id, url, url_norm) in rows {
        if backfill_should_fail(updated) {
            return Err(crate::error::AppError::new(
                "migration",
                "simulated url_norm backfill interruption (test injection)",
            ));
        }
        let norm = normalize_url(&url);
        if url_norm.as_deref() != Some(norm.as_str()) {
            tx.execute(
                "UPDATE articles SET url_norm = ?1 WHERE id = ?2",
                params![norm, id],
            )?;
            updated += 1;
        }
    }
    tx.execute(
        "INSERT INTO settings (key, value) VALUES (?1, '1')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![MARKER_KEY],
    )?;
    tx.commit()?;
    Ok(true)
}

/* ============================================================
行类型（前端 IPC 契约）—— 与 src/types.ts 保持同构
============================================================ */

/* ============================================================
REQ-108 单元测试：M-14 迁移回填事务性 + v14 索引/兜底 up 测试
============================================================ */
#[cfg(test)]
mod req108_migration_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// 进程内唯一临时库路径（std 实现，不新增依赖；同 tests/common 惯例）。
    fn unique_test_db(base: &str) -> std::path::PathBuf {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        std::env::temp_dir().join(format!(
            "fluxreader_migr_{base}_{pid}_{nanos}_{seq}.db",
            pid = std::process::id()
        ))
    }

    const MARKER_SQL: &str =
        "SELECT EXISTS(SELECT 1 FROM settings WHERE key = 'url_norm_backfill_done')";

    fn marker_value(conn: &Connection) -> i64 {
        conn.query_row(MARKER_SQL, [], |r| r.get(0)).unwrap()
    }

    /// ① M-14 中断复现：回填中途模拟失败 → 迁移已提交、回滚零残留 →
    /// 重启按缺失标记补完 → 已标记库零重复工作（幂等）。
    #[test]
    fn url_norm_backfill_interrupted_is_repaired_on_restart() {
        let path = unique_test_db("interrupt");
        let _ = std::fs::remove_file(&path);

        // ---- 造 v6 存量库：4 篇带 URL 的文章（3 篇需规范化，1 篇已规范）----
        {
            let mut conn = Connection::open(&path).unwrap();
            MIGRATIONS.to_version(&mut conn, 6).unwrap();
            conn.execute_batch(
                "INSERT INTO feeds (feed_url, title) VALUES ('https://f.example/rss', 'F');
                 INSERT INTO articles (feed_id, guid, title, url) VALUES
                   (1, 'g1', 't1', 'https://WWW.Example.com/a?utm_source=x#frag'),
                   (1, 'g2', 't2', 'https://m.example.com/b/'),
                   (1, 'g3', 't3', 'http://example.com/AMP/c.amp.html'),
                   (1, 'g4', 't4', 'http://example.com/plain');",
            )
            .unwrap();
        }

        // ---- 模拟「迁移已提交、回填半途中断」：to_latest 成功后回填在第 3 行前失败 ----
        BACKFILL_FAIL_AFTER_ROWS.store(2, Ordering::SeqCst);
        {
            let mut conn = Connection::open(&path).unwrap();
            MIGRATIONS.to_latest(&mut conn).unwrap();
            let err = ensure_url_norm_backfill(&conn);
            assert!(err.is_err(), "注入的回填中断必须以错误返回");
        }
        BACKFILL_FAIL_AFTER_ROWS.store(-1, Ordering::SeqCst);

        // ---- 中断现场：user_version 已推进；标记缺失；整体回滚零半更新 ----
        {
            let conn = Connection::open(&path).unwrap();
            let v: i64 = conn
                .query_row("PRAGMA user_version", [], |r| r.get(0))
                .unwrap();
            // P3-7 后最新版本为 v15：以「全新库 to_latest 落到的版本」为基准，
            // 测试随追加式迁移自动跟进，不再硬编码版本号。
            let latest: i64 = {
                let mut fresh = Connection::open_in_memory().unwrap();
                MIGRATIONS.to_latest(&mut fresh).unwrap();
                fresh
                    .query_row("PRAGMA user_version", [], |r| r.get(0))
                    .unwrap()
            };
            assert_eq!(
                v, latest,
                "迁移事务独立提交：user_version 已到最新（旧实现据此永不重试）"
            );
            assert_eq!(
                marker_value(&conn),
                0,
                "回填回滚：完成标记必须缺失（下次启动据此重试）"
            );
            let placeholders: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM articles
                      WHERE url IS NOT NULL AND url != '' AND url_norm = lower(url)",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(
                placeholders, 4,
                "整体回滚：全部行仍是 v7 的 lower(url) 占位（事务原子性，零半更新）"
            );
        }

        // ---- 重启：open() 按缺失标记幂等补跑 ----
        {
            let conn = open(&path).expect("重启（带补跑）必须成功");
            let mut stmt = conn
                .prepare("SELECT url, url_norm FROM articles WHERE url IS NOT NULL AND url != ''")
                .unwrap();
            let rows: Vec<(String, String)> = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    ))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(rows.len(), 4);
            for (url, norm) in &rows {
                assert_eq!(
                    norm,
                    &normalize_url(url),
                    "重启补跑必须把 url_norm 规范化到完整口径（中断前已更新的行也要重算）"
                );
            }
            assert_eq!(marker_value(&conn), 1, "补跑成功后同事务落标");
            // 幂等：已标记库再调一次 = 零工作（零重复回填）
            assert!(
                !ensure_url_norm_backfill(&conn).unwrap(),
                "已标记库不得重复回填"
            );
        }
        let _ = std::fs::remove_file(&path);
    }

    /// ② M-14 存量库补跑：已停在 v7+（标记缺失、url_norm 为 lower(url) 占位）
    /// 的库，首次 open() 即补规范化并落标。
    #[test]
    fn legacy_upgraded_db_without_marker_gets_backfill_on_open() {
        let path = unique_test_db("legacy");
        let _ = std::fs::remove_file(&path);
        {
            // 直接用迁移框架升到最新（模拟旧版本完成升级、未经本修复的存量库），
            // 再把 url_norm 置回 v7 的 SQL 占位值，且不写完成标记
            let mut conn = Connection::open(&path).unwrap();
            MIGRATIONS.to_latest(&mut conn).unwrap();
            conn.execute_batch(
                "INSERT INTO feeds (feed_url, title) VALUES ('https://f.example/rss', 'F');
                 INSERT INTO articles (feed_id, guid, title, url) VALUES
                   (1, 'g1', 't1', 'https://www.example.com/post?utm_medium=rss');",
            )
            .unwrap();
            conn.execute(
                "UPDATE articles SET url_norm = lower(url) WHERE url IS NOT NULL",
                [],
            )
            .unwrap();
        }
        let conn = open(&path).unwrap();
        let norm: String = conn
            .query_row("SELECT url_norm FROM articles WHERE id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            norm,
            normalize_url("https://www.example.com/post?utm_medium=rss"),
            "存量库（已停在 v7+）首启必须幂等补跑规范化"
        );
        assert_eq!(marker_value(&conn), 1);
        assert!(
            !ensure_url_norm_backfill(&conn).unwrap(),
            "补跑落标后重复启动零重复工作"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// ③ M-14 零重复工作：已标记库的 ensure 不产生任何对 articles 的 UPDATE。
    #[test]
    fn backfill_marker_makes_repeat_run_zero_work() {
        let conn = {
            let path = unique_test_db("zero");
            let _ = std::fs::remove_file(&path);
            let c = open(&path).unwrap();
            // open 已落标；本测试用内存断言即可，文件随后清理
            c
        };
        // （内存库方便断言计划/计数：直接在内存连接上再验证一次）
        let mut mem = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut mem).unwrap();
        assert!(
            ensure_url_norm_backfill(&mem).unwrap(),
            "新库首跑：零行回填但落标（幂等标记）"
        );

        let updates = super::with_count(
            &conn,
            Some((rusqlite::ffi::SQLITE_UPDATE, "articles")),
            || ensure_url_norm_backfill(&conn).unwrap(),
        );
        assert!(!updates.0, "已标记库直接跳过");
        assert_eq!(
            updates.1, 0,
            "已标记库的重复回填不得触碰任何 articles 行（零重复工作）"
        );
    }

    /// ④ v14 up 测试：索引创建 + published_at 存量兜底回填（v12 语义重申）
    /// + FTS 更新触发器收窄 + 兜底触发器存在性。
    #[test]
    fn v14_adds_indexes_and_backfills_missing_published_at() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_version(&mut conn, 13).unwrap();
        conn.execute_batch(
            "INSERT INTO feeds (feed_url, title) VALUES ('https://f.example/rss', 'F');
             INSERT INTO articles (feed_id, guid, title, fetched_at, published_at) VALUES
               (1, 'g-null', 't', '2025-01-01 00:00:01', NULL),
               (1, 'g-empty', 't', '2025-01-01 00:00:02', ''),
               (1, 'g-real', 't', '2025-01-01 00:00:03', '2024-12-31T10:00:00Z');",
        )
        .unwrap();
        MIGRATIONS.to_version(&mut conn, 14).unwrap();

        // 回填：NULL/'' 补为 fetched_at；真值不动（断言停在 v14 的**当时现场**，
        // P3-7 的 v15 归一不改变 v12/v14 回填语义，见下方 to_latest 后的接缝断言）
        let rows: Vec<(String, String)> = {
            let mut stmt = conn
                .prepare("SELECT guid, published_at FROM articles ORDER BY id")
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(
            rows,
            vec![
                ("g-null".to_string(), "2025-01-01 00:00:01".to_string()),
                ("g-empty".to_string(), "2025-01-01 00:00:02".to_string()),
                ("g-real".to_string(), "2024-12-31T10:00:00Z".to_string()),
            ],
            "NULL/'' 存量行回填为 fetched_at，真值不动"
        );

        // 推进到最新（v15 起含 P3-7 归一）：legacy 空格形态（回填产物）归一为
        // 现行 RFC3339 UTC 写入格式，真值（RFC3339 'Z' 形态）仍不动
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let rows_after_latest: Vec<(String, String)> = {
            let mut stmt = conn
                .prepare("SELECT guid, published_at FROM articles ORDER BY id")
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(
            rows_after_latest,
            vec![
                (
                    "g-null".to_string(),
                    "2025-01-01T00:00:01+00:00".to_string()
                ),
                (
                    "g-empty".to_string(),
                    "2025-01-01T00:00:02+00:00".to_string()
                ),
                ("g-real".to_string(), "2024-12-31T10:00:00Z".to_string()),
            ],
            "v15 只归一 legacy 空格形态，RFC3339 真值不动"
        );

        // M-5/M-7 索引存在性（EXPLAIN 走索引的前提）
        for idx in [
            "idx_articles_feed_published",
            "idx_articles_read_published",
            "idx_sync_queue_article",
            "idx_sync_queue_created",
        ] {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
                    params![idx],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "v14 必须创建索引 {idx}");
        }

        // FTS 更新触发器收窄到内容列（兜底触发器合用同一表时的前置，见迁移注释）
        let au_sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = 'articles_au'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            au_sql.contains("AFTER UPDATE OF title, body_text, author, ai_summary, translated_content"),
            "articles_au 必须收窄到内容列（否则 published_at 兜底 UPDATE 触发 FTS 的 delete 对未入索引行执行 → malformed，实测）：{au_sql}"
        );

        // 兜底触发器存在性
        for trg in [
            "trg_articles_published_fallback_ins",
            "trg_articles_published_fallback_upd",
        ] {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND name = ?1",
                    params![trg],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "v14 必须创建触发器 {trg}");
        }
    }

    /// ⑤ M-5 写入兜底：任何裸写入路径的 NULL/'' published_at 都被触发器补齐为
    /// fetched_at，真值永不被覆盖——「published_at 非空」是 ORDER BY published_at
    /// 与旧 COALESCE 口径等价的前提。
    #[test]
    fn published_at_write_guard_fills_null_and_empty() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        conn.execute_batch(
            "INSERT INTO feeds (feed_url, title) VALUES ('https://f.example/rss', 'F');
             INSERT INTO articles (feed_id, guid, title, fetched_at, published_at) VALUES
               (1, 'g1', 't', '2025-02-01 00:00:01', NULL),
               (1, 'g2', 't', '2025-02-01 00:00:02', '');",
        )
        .unwrap();
        let read = |guid: &str| -> String {
            conn.query_row(
                "SELECT published_at FROM articles WHERE guid = ?1",
                params![guid],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(
            read("g1"),
            "2025-02-01 00:00:01",
            "INSERT 的 NULL 必须补为 fetched_at"
        );
        assert_eq!(
            read("g2"),
            "2025-02-01 00:00:02",
            "INSERT 的空串必须补为 fetched_at"
        );

        // UPDATE 置 NULL/'' → 同样补回 fetched_at（本轮 fetched_at 未变）
        conn.execute(
            "UPDATE articles SET published_at = NULL WHERE guid = 'g1'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE articles SET published_at = '' WHERE guid = 'g2'",
            [],
        )
        .unwrap();
        assert_eq!(read("g1"), "2025-02-01 00:00:01", "UPDATE 置 NULL 必须补回");
        assert_eq!(read("g2"), "2025-02-01 00:00:02", "UPDATE 置空串必须补回");

        // 真值写入不受兜底触发器影响（不覆盖、不重写）
        conn.execute(
            "UPDATE articles SET published_at = '2025-03-01T00:00:00Z' WHERE guid = 'g1'",
            [],
        )
        .unwrap();
        assert_eq!(read("g1"), "2025-03-01T00:00:00Z", "真值不得被覆盖");
    }

    /// ⑥ M-5 排序等价性：兜底不变量生效后，`ORDER BY published_at` 与旧
    /// `ORDER BY COALESCE(published_at, fetched_at)` 对同一批数据（含裸写入的
    /// NULL/'' 行与并列值）产出完全相同的行序——列表查询换排序式的正确性依据。
    #[test]
    fn published_ordering_matches_legacy_coalesce() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        conn.execute_batch(
            "INSERT INTO feeds (feed_url, title) VALUES ('https://f.example/rss', 'F');
             INSERT INTO articles (feed_id, guid, title, fetched_at, published_at) VALUES
               (1, 'a', 't', '2025-01-01 00:00:03', NULL),
               (1, 'b', 't', '2025-01-01 00:00:01', ''),
               (1, 'c', 't', '2025-01-01 00:00:02', '2024-12-01T00:00:00Z'),
               (1, 'd', 't', '2025-01-01 00:00:04', '2025-06-01T00:00:00Z'),
               (1, 'e', 't', '2025-01-01 00:00:05', NULL),
               (1, 'f', 't', '2025-01-01 00:00:06', '2025-06-01T00:00:00Z');",
        )
        .unwrap();
        let ordered = |order_expr: &str, dir: &str| -> Vec<String> {
            let sql = format!("SELECT guid FROM articles ORDER BY {order_expr} {dir}, guid");
            // 并列值用 guid 显式定序，避免依赖扫描序（本测试只证明「排序键」等价）
            let mut stmt = conn.prepare(&sql).unwrap();
            stmt.query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        for dir in ["DESC", "ASC"] {
            assert_eq!(
                ordered("published_at", dir),
                ordered("COALESCE(published_at, fetched_at)", dir),
                "ORDER BY published_at 必须与旧 COALESCE 口径逐行等价（{dir}）"
            );
        }
        // 防退化：夹具里确实有「靠兜底补齐才可排」的行（否则本测试没有判别力）
        let null_like: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM articles WHERE published_at IS NULL OR published_at = ''",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            null_like, 0,
            "测试夹具必须覆盖 NULL/'' 写入路径（触发器已补齐）"
        );
    }
}

/* ============================================================
TASK-116 v16 单元测试：sync_queue 增 attempts/last_error + 旧库升级
（cargo test 由 CI 执行，DEC-local-cargo-gate-20261005）
============================================================ */
#[cfg(test)]
mod t116_migration_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// 进程内唯一临时库路径（同 req108 模块的 unique_test_db 惯例，std 实现）。
    fn unique_test_db(base: &str) -> std::path::PathBuf {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        std::env::temp_dir().join(format!(
            "fluxreader_migr_{base}_{pid}_{nanos}_{seq}.db",
            pid = std::process::id()
        ))
    }

    fn queue_column_names(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT name FROM pragma_table_info('sync_queue') ORDER BY name")
            .unwrap();
        stmt.query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    /// (t116-r0) v16 迁移：① 全新库 to_latest 后新列存在；② 停在 v15 的旧库
    /// （旧形态 sync_queue + 既有队列行）经生产 open() 打开即迁移——既有行拿到
    /// 缺省值（attempts=0 / last_error=NULL =「等待同步」，与修前语义一致），
    /// 数据零丢失，且新列立即可写。判别力：迁移漏建列时 SELECT attempts 直接
    /// 报错红；把缺省建错（如 attempts NULL 或非 0）时断言红。
    #[test]
    fn v16_sync_queue_columns_and_legacy_upgrade() {
        // ① 全新库：最新 schema 含两列
        {
            let mut fresh = Connection::open_in_memory().unwrap();
            MIGRATIONS.to_latest(&mut fresh).unwrap();
            let cols = queue_column_names(&fresh);
            assert!(cols.contains(&"attempts".to_string()), "新库含 attempts 列");
            assert!(
                cols.contains(&"last_error".to_string()),
                "新库含 last_error 列"
            );
        }

        // ② 旧库升级：停在 v15（迁移追加前最后一版），已有队列行
        let path = unique_test_db("t116_v15");
        let _ = std::fs::remove_file(&path);
        {
            let mut conn = Connection::open(&path).unwrap();
            MIGRATIONS.to_version(&mut conn, 15).unwrap();
            conn.execute_batch(
                "INSERT INTO feeds (feed_url, title) VALUES ('https://f.example/rss', 'F');
                 INSERT INTO articles (feed_id, guid, title) VALUES (1, 'g1', 't');",
            )
            .unwrap();
            conn.execute_batch(
                "INSERT INTO sync_queue (article_id, action) VALUES (1, 'read');
                 INSERT INTO sync_queue (article_id, action, feed_url) VALUES (1, 'add_feed', 'https://x.example/rss');",
            )
            .unwrap();
        }
        // 生产打开路径：迁移 + 回填 + 凭据迁移一路跑完（既有库打开即迁移）
        let conn = open(&path).expect("v15 旧库打开必须成功完成 v16 迁移");
        let cols = queue_column_names(&conn);
        assert!(cols.contains(&"attempts".to_string()));
        assert!(cols.contains(&"last_error".to_string()));

        // 既有行零丢失 + 缺省值 =「等待同步」（修前语义）
        let (n, attempts_sum): (i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(attempts), -1) FROM sync_queue",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(n, 2, "既有队列行零丢失");
        assert_eq!(attempts_sum, 0, "升级后既有行 attempts 全为缺省 0");
        let null_errors: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_queue WHERE last_error IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(null_errors, 0, "升级后既有行 last_error 全为 NULL");

        // 新列立即可写（失败标记路径在升级库上可用）
        conn.execute(
            "UPDATE sync_queue SET attempts = attempts + 1, last_error = '升级后可写' WHERE action = 'read'",
            [],
        )
        .unwrap();
        let (a, e): (i64, String) = conn
            .query_row(
                "SELECT attempts, last_error FROM sync_queue WHERE action = 'read'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((a, e.as_str()), (1, "升级后可写"));

        // user_version 与全新库一致（追加式迁移推进到最新，不硬编码版本号）
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        let latest: i64 = {
            let mut fresh = Connection::open_in_memory().unwrap();
            MIGRATIONS.to_latest(&mut fresh).unwrap();
            fresh
                .query_row("PRAGMA user_version", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(v, latest, "旧库必须升到与全新库相同的最新版本");
        let _ = std::fs::remove_file(&path);
    }
}

/* ============================================================
P3-7（自检 2026-09-29）v15 单元测试：legacy published_at 归一
============================================================ */
#[cfg(test)]
mod v15_migration_tests {
    use super::*;

    /// v14 夹具：仅建一个 feed，供逐条插入文章（published_at 由用例自定）。
    fn seeded_v14_conn() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_version(&mut conn, 14).unwrap();
        conn.execute_batch(
            "INSERT INTO feeds (feed_url, title) VALUES ('https://f.example/rss', 'F');",
        )
        .unwrap();
        conn
    }

    fn published_at(conn: &Connection, guid: &str) -> String {
        conn.query_row(
            "SELECT published_at FROM articles WHERE guid = ?1",
            params![guid],
            |r| r.get(0),
        )
        .unwrap()
    }

    const LEGACY_GLOB: &str =
        "[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9] [0-9][0-9]:[0-9][0-9]:[0-9][0-9]";

    /// ① up 测试：legacy 空格格式（v12/v14 回填形态）被归一为与现行写入逐字
    /// 一致的 RFC3339 UTC（to_rfc3339() 的 UTC 形态：'+00:00' 后缀、秒级精度
    /// 不带小数）；已是 RFC3339 的真值（含带小数）原样不动。
    #[test]
    fn v15_normalizes_legacy_published_at_to_current_rfc3339() {
        let mut conn = seeded_v14_conn();
        conn.execute_batch(
            "INSERT INTO articles (feed_id, guid, title, fetched_at, published_at) VALUES
               (1, 'legacy', 't', '2025-01-01 00:00:01', '2025-06-01 08:00:00'),
               (1, 'rfc3339', 't', '2025-01-01 00:00:02', '2025-06-01T02:00:00+00:00'),
               (1, 'rfc3339-frac', 't', '2025-01-01 00:00:03', '2025-06-01T09:00:00.123456+00:00');",
        )
        .unwrap();

        MIGRATIONS.to_latest(&mut conn).unwrap();

        assert_eq!(
            published_at(&conn, "legacy"),
            "2025-06-01T08:00:00+00:00",
            "legacy 空格格式必须归一为现行写入格式（ingestion/parse.rs 与 sync/entries.rs 的 to_rfc3339() 形态）"
        );
        assert_eq!(
            published_at(&conn, "rfc3339"),
            "2025-06-01T02:00:00+00:00",
            "已是 RFC3339 的真值不得被改写"
        );
        assert_eq!(
            published_at(&conn, "rfc3339-frac"),
            "2025-06-01T09:00:00.123456+00:00",
            "带小数的 RFC3339 真值不得被改写"
        );
        let legacy_left: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM articles WHERE published_at GLOB '{LEGACY_GLOB}'"),
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(legacy_left, 0, "归一后库内不得再有任何 legacy 形态");
    }

    /// ② 幂等测试：迁移语句连跑两遍第二遍零改动（GLOB 只命中 legacy 形态，
    /// 归一后的 RFC3339 不再命中）；并覆盖「迁移后经裸 SQL 重新引入 legacy 行」
    /// 的重跑场景（v14 兜底触发器写 fetched_at 的形态）。
    #[test]
    fn v15_is_idempotent_rerun_changes_zero_rows() {
        let mut conn = seeded_v14_conn();
        conn.execute_batch(
            "INSERT INTO articles (feed_id, guid, title, fetched_at, published_at) VALUES
               (1, 'legacy', 't', '2025-01-01 00:00:01', '2025-06-01 08:00:00');",
        )
        .unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        assert_eq!(
            published_at(&conn, "legacy"),
            "2025-06-01T08:00:00+00:00",
            "up 阶段先归一（前置确认）"
        );

        // 对已归一的库重跑同一生产语句：零改动
        let rerun = conn.execute(V15_NORMALIZE_PUBLISHED_AT_SQL, []).unwrap();
        assert_eq!(rerun, 0, "已归一的库重跑必须 no-op");

        // 模拟迁移后重新引入的 legacy 行：首跑归一、再跑零改动
        conn.execute_batch(
            "INSERT INTO articles (feed_id, guid, title, published_at) VALUES
               (1, 'late-legacy', 't', '2025-07-01 10:00:00');",
        )
        .unwrap();
        let first = conn.execute(V15_NORMALIZE_PUBLISHED_AT_SQL, []).unwrap();
        assert_eq!(first, 1, "重新引入的 legacy 行必须被归一");
        assert_eq!(
            published_at(&conn, "late-legacy"),
            "2025-07-01T10:00:00+00:00"
        );
        let second = conn.execute(V15_NORMALIZE_PUBLISHED_AT_SQL, []).unwrap();
        assert_eq!(second, 0, "连跑两遍第二遍必须零改动");
    }

    /// ③ 排序等价测试：同一日的 legacy 行与 RFC3339 行，修前 ORDER BY
    /// published_at 按字符串比较（'T' > ' ' → RFC3339 行恒排 legacy 行之后，
    /// 无视真实时刻），修后按真实时刻排序（与 datetime(published_at) 口径逐行
    /// 等价）。
    #[test]
    fn v15_same_day_ordering_follows_true_instant() {
        let mut conn = seeded_v14_conn();
        conn.execute_batch(
            "INSERT INTO articles (feed_id, guid, title, fetched_at, published_at) VALUES
               (1, 'legacy-08h', 't', '2025-01-01 00:00:01', '2025-06-01 08:00:00'),
               (1, 'rfc3339-02h', 't', '2025-01-01 00:00:02', '2025-06-01T02:00:00+00:00');",
        )
        .unwrap();

        let order = |conn: &Connection| -> Vec<String> {
            let mut stmt = conn
                .prepare("SELECT guid FROM articles ORDER BY published_at ASC")
                .unwrap();
            stmt.query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };

        // 修前锚（v14）：字符串比较使 02:00 的 RFC3339 行排到 08:00 的 legacy 行之后
        assert_eq!(
            order(&conn),
            vec!["legacy-08h".to_string(), "rfc3339-02h".to_string()],
            "修前锚：同日内 'T' > ' ' 使 RFC3339 行恒排 legacy 行之后（乱序，否则无判别力）"
        );

        MIGRATIONS.to_latest(&mut conn).unwrap();

        // 修后：按真实时刻排序（02:00 早于 08:00）
        assert_eq!(
            order(&conn),
            vec!["rfc3339-02h".to_string(), "legacy-08h".to_string()],
            "修后必须按真实时刻排序"
        );
        let mut stmt = conn
            .prepare("SELECT guid FROM articles ORDER BY datetime(published_at) ASC")
            .unwrap();
        let by_true: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            order(&conn),
            by_true,
            "ORDER BY published_at 必须与真实时刻口径（datetime 归一）逐行等价"
        );
    }
}
