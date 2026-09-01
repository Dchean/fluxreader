//! 迁移升级测试：v1 库（P1 schema）→ v2（P2 sync 列 + sync_queue 表）。
//! 模拟真实用户旧库升级：先手工建 v1 形状的数据，再 open() 触发迁移。

use app_lib::db;
use rusqlite::Connection;

#[test]
fn migration_v1_to_v2_preserves_data() {
    let tmp = std::env::temp_dir().join("fluxreader_migration_test.db");
    let _ = std::fs::remove_file(&tmp);

    // ---- 手工建 v1 库（P1 发布时的 schema）----
    {
        let conn = Connection::open(&tmp).unwrap();
        conn.execute_batch(r#"
            CREATE TABLE folders (id INTEGER PRIMARY KEY, name TEXT NOT NULL, position INTEGER NOT NULL DEFAULT 0,
                layout TEXT NOT NULL DEFAULT 'article', auto_summary INTEGER NOT NULL DEFAULT 1,
                auto_translate INTEGER NOT NULL DEFAULT 0, collapsed INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now')));
            CREATE TABLE feeds (id INTEGER PRIMARY KEY, feed_url TEXT NOT NULL UNIQUE, site_url TEXT,
                title TEXT NOT NULL, favicon_url TEXT, folder_id INTEGER REFERENCES folders(id) ON DELETE CASCADE,
                layout TEXT NOT NULL DEFAULT 'inherit', auto_summary INTEGER NOT NULL DEFAULT 1,
                auto_translate INTEGER NOT NULL DEFAULT 0, etag TEXT, last_modified TEXT, last_fetched_at TEXT,
                fetch_error TEXT, fetch_failed INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL DEFAULT (datetime('now')));
            CREATE TABLE articles (id INTEGER PRIMARY KEY, feed_id INTEGER NOT NULL REFERENCES feeds(id) ON DELETE CASCADE,
                guid TEXT NOT NULL, url TEXT, title TEXT NOT NULL, author TEXT, summary TEXT, content_html TEXT,
                body_text TEXT NOT NULL DEFAULT '', image_url TEXT, enclosure_url TEXT, enclosure_mime TEXT,
                duration_sec INTEGER, ai_summary TEXT, translated_content TEXT, source TEXT NOT NULL DEFAULT 'direct',
                published_at TEXT, fetched_at TEXT NOT NULL DEFAULT (datetime('now')),
                is_read INTEGER NOT NULL DEFAULT 0, is_starred INTEGER NOT NULL DEFAULT 0, UNIQUE(feed_id, guid));
            CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        "#).unwrap();
        // 用户数据
        conn.execute("INSERT INTO folders (name, layout) VALUES ('旧分类', 'article')", []).unwrap();
        conn.execute("INSERT INTO feeds (feed_url, title, folder_id) VALUES ('https://old.example.com/rss', 'Old Feed', 1)", []).unwrap();
        conn.execute(
            "INSERT INTO articles (feed_id, guid, title, is_read, is_starred) VALUES (1, 'g1', 'Old Article', 1, 1)",
            [],
        ).unwrap();
        conn.execute("INSERT INTO settings VALUES ('miniflux_endpoint', 'https://keep.example.com')", []).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
    }

    // ---- open() 触发迁移到 v2 ----
    let conn = db::open(&tmp).expect("migration must succeed");

    // 数据完好
    let (fname, fcount): (String, i64) = conn
        .query_row("SELECT name, COUNT(*) FROM folders", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!(fname, "旧分类");
    assert_eq!(fcount, 1);

    let (ftitle, feedcount): (String, i64) = conn
        .query_row("SELECT title, COUNT(*) FROM feeds", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!(ftitle, "Old Feed");
    assert_eq!(feedcount, 1);

    let (atitle, read, starred): (String, bool, bool) = conn
        .query_row("SELECT title, is_read, is_starred FROM articles", [], |r| {
            Ok((r.get(0)?, r.get::<_, i64>(1)? != 0, r.get::<_, i64>(2)? != 0))
        })
        .unwrap();
    assert_eq!(atitle, "Old Article");
    assert!(read && starred, "read/starred state preserved");

    // 设置保留
    let ep: String = conn
        .query_row("SELECT value FROM settings WHERE key = 'miniflux_endpoint'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(ep, "https://keep.example.com");

    // v2 新结构可用
    conn.execute("INSERT INTO sync_queue (article_id, action) VALUES (1, 'read')", []).unwrap();
    let queue = db::take_sync_queue(&conn).unwrap();
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0].action, "read");

    // 新列可写
    db::set_article_miniflux_id(&conn, 1, 42).unwrap();
    let bound: i64 = conn
        .query_row("SELECT miniflux_id FROM articles WHERE id = 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(bound, 42);

    // 版本号
    let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert!(v >= 2, "user_version must be >= 2, got {v}");

    let _ = std::fs::remove_file(&tmp);
    println!("=== MIGRATION v1→v2 PASS (version {v}) ===");
}

#[test]
fn migration_v6_to_v7_backfills_precise_url_norm() {
    let tmp = std::env::temp_dir().join("fluxreader_migration_v7_test.db");
    let _ = std::fs::remove_file(&tmp);

    // 手工建 v6 形状库：v5 + deduped_urls 表
    {
        let conn = Connection::open(&tmp).unwrap();
        conn.execute_batch(r#"
            CREATE TABLE folders (id INTEGER PRIMARY KEY, name TEXT NOT NULL, position INTEGER NOT NULL DEFAULT 0,
                layout TEXT NOT NULL DEFAULT 'article', auto_summary INTEGER NOT NULL DEFAULT 1,
                auto_translate INTEGER NOT NULL DEFAULT 0, collapsed INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now')), miniflux_id INTEGER);
            CREATE TABLE feeds (id INTEGER PRIMARY KEY, feed_url TEXT NOT NULL UNIQUE, site_url TEXT,
                title TEXT NOT NULL, favicon_url TEXT, folder_id INTEGER REFERENCES folders(id) ON DELETE CASCADE,
                layout TEXT NOT NULL DEFAULT 'inherit', auto_summary INTEGER NOT NULL DEFAULT 1,
                auto_translate INTEGER NOT NULL DEFAULT 0, etag TEXT, last_modified TEXT, last_fetched_at TEXT,
                fetch_error TEXT, fetch_failed INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL DEFAULT (datetime('now')),
                fail_count INTEGER NOT NULL DEFAULT 0, next_retry_at TEXT, miniflux_id INTEGER);
            CREATE TABLE articles (id INTEGER PRIMARY KEY, feed_id INTEGER NOT NULL REFERENCES feeds(id) ON DELETE CASCADE,
                guid TEXT NOT NULL, url TEXT, title TEXT NOT NULL, author TEXT, summary TEXT, content_html TEXT,
                body_text TEXT NOT NULL DEFAULT '', image_url TEXT, enclosure_url TEXT, enclosure_mime TEXT,
                duration_sec INTEGER, ai_summary TEXT, translated_content TEXT, source TEXT NOT NULL DEFAULT 'direct',
                published_at TEXT, fetched_at TEXT NOT NULL DEFAULT (datetime('now')),
                is_read INTEGER NOT NULL DEFAULT 0, is_starred INTEGER NOT NULL DEFAULT 0,
                miniflux_id INTEGER, fulltext_extracted INTEGER NOT NULL DEFAULT 0, UNIQUE(feed_id, guid));
            CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE deduped_urls (url TEXT PRIMARY KEY, kept_aid INTEGER NOT NULL, kept_at TEXT NOT NULL DEFAULT (datetime('now')));
        "#).unwrap();
        conn.execute("INSERT INTO folders (name) VALUES ('Cat')", []).unwrap();
        conn.execute(
            "INSERT INTO feeds (feed_url, title, folder_id) VALUES ('https://example.com/rss', 'F', 1)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO articles (feed_id, guid, url, title) VALUES (1, 'g1', 'https://www.example.com/story/?utm_source=rss&id=9', 'Dressed URL')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO articles (feed_id, guid, url, title) VALUES (1, 'g2', 'http://example.com/story?id=9', 'Clean URL')",
            [],
        ).unwrap();
        conn.pragma_update(None, "user_version", 6).unwrap();
    }

    let conn = db::open(&tmp).expect("migrate v6→v7");

    // url_norm 由 Rust normalize_url 精确回填（非 lower(url) 占位）
    let (n1, n2): (String, String) = conn
        .query_row(
            "SELECT (SELECT url_norm FROM articles WHERE guid='g1'), (SELECT url_norm FROM articles WHERE guid='g2')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(n1, n2, "dressed URL and clean URL must normalize to the same key");
    assert!(n1 == "http://example.com/story?id=9", "normalized form: got {n1}");

    // miniflux_dup_ids 列存在且默认空
    let dups: String = conn
        .query_row("SELECT miniflux_dup_ids FROM articles WHERE guid='g1'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(dups, "");

    let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert!(v >= 7, "v6 schema must migrate through v7 (url_norm), got {v}");

    let _ = std::fs::remove_file(&tmp);
    println!("=== MIGRATION v6→v7 PASS ===");
}
