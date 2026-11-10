//! Current format initialization, refusal and recoverability replace historical upgrade fixtures.
mod common;
use app_lib::db;
use rusqlite::Connection;

#[test]
fn old_database_is_rejected_without_changing_its_bytes_or_data() {
    let path = common::unique_db_path("old_format_refusal");
    let _guard = common::TempDbGuard::new(path.clone());
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE legacy(value TEXT); INSERT INTO legacy VALUES('keep me'); PRAGMA user_version=34;").unwrap();
    }
    let before = std::fs::read(&path).unwrap();
    let err = db::open(&path).unwrap_err();
    assert_eq!(err.code, "unsupported_database");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let conn =
        Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert_eq!(
        conn.query_row("SELECT value FROM legacy", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "keep me"
    );
}

#[test]
fn new_format_reopen_preserves_current_entities_and_pending_intent() {
    let path = common::unique_db_path("current_format_reopen");
    let _guard = common::TempDbGuard::new(path.clone());
    let conn = db::open(&path).unwrap();
    conn.execute_batch("INSERT INTO feeds(id,feed_url,title) VALUES(1,'https://example.invalid/rss','source'); INSERT INTO articles(id,feed_id,guid,title) VALUES(1,1,'guid','article');").unwrap();
    db::set_read_with_enqueue(&conn, 1, true).unwrap();
    let before = db::take_sync_queue(&conn).unwrap().remove(0).id;
    drop(conn);
    let conn = db::open(&path).unwrap();
    assert!(db::get_article(&conn, 1).unwrap().unwrap().is_read);
    assert_eq!(db::take_sync_queue(&conn).unwrap()[0].id, before);
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert!(
        conn.pragma_query_value(None, "application_id", |r| r.get::<_, i64>(0))
            .unwrap()
            > 0
    );
}

#[test]
fn manual_current_backup_can_be_opened_with_saved_body_and_archived_source() {
    let path = common::unique_db_path("current_backup_restore");
    let _guard = common::TempDbGuard::new(path.clone());
    let conn = db::open(&path).unwrap();
    conn.execute_batch("INSERT INTO feeds(id,feed_url,title) VALUES(1,'https://example.invalid/rss','source'); INSERT INTO articles(id,feed_id,guid,title,is_starred) VALUES(1,1,'guid','saved',1);").unwrap();
    db::backfill_article_content(
        &conn,
        1,
        "<p>saved body</p>",
        "saved body",
        None,
        None,
        None,
    )
    .unwrap();
    app_lib::commands::record_feed_deletion(&conn, 1).unwrap();
    let info = db::create_manual_backup(&conn).unwrap();
    let snapshot = std::path::PathBuf::from(info.path);
    let _backup_guard = common::TempDbGuard::new(snapshot.clone());
    let restored = db::open(&snapshot).unwrap();
    let saved = db::get_article(&restored, 1).unwrap().unwrap();
    assert!(saved.is_starred && saved.retained);
    assert_eq!(saved.content_html.as_deref(), Some("<p>saved body</p>"));
    assert!(db::list_feeds(&restored).unwrap().is_empty());
    assert_eq!(db::list_article_sources(&restored).unwrap().len(), 1);
}

#[test]
fn current_schema_constraints_and_foreign_keys_hold_after_reopen() {
    let path = common::unique_db_path("current_schema_constraints");
    let _guard = common::TempDbGuard::new(path.clone());
    let conn = db::open(&path).unwrap();
    assert_eq!(
        conn.pragma_query_value(None, "foreign_keys", |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    let folder = db::create_folder(&conn, "Current", "article").unwrap();
    let feed = db::insert_feed(
        &conn,
        "https://current.invalid/rss",
        None,
        "Current",
        None,
        folder,
        "inherit",
        false,
        false,
    )
    .unwrap();
    let defaults: (String, String) = conn
        .query_row(
            "SELECT summary_mode,translate_mode FROM folders WHERE id=?1",
            [folder],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(defaults, ("inherit".into(), "inherit".into()));
    for table in ["feeds", "folders"] {
        assert!(conn
            .execute(&format!("UPDATE {table} SET summary_mode='invalid'"), [])
            .is_err());
        assert!(conn
            .execute(&format!("UPDATE {table} SET translate_mode='invalid'"), [])
            .is_err());
    }
    assert!(conn.execute("INSERT INTO articles(feed_id,guid,title,image_source) VALUES(?1,'bad-cover','bad','unknown')", [feed]).is_err());
    assert!(conn
        .execute(
            "INSERT INTO articles(feed_id,guid,title) VALUES(99999,'bad','bad')",
            []
        )
        .is_err());
    conn.execute(
        "INSERT INTO articles(feed_id,guid,title,published_at) VALUES(?1,'date','date','')",
        [feed],
    )
    .unwrap();
    let inferred: (String, String) = conn
        .query_row(
            "SELECT published_at,first_seen_at FROM articles WHERE guid='date'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(!inferred.0.is_empty() && !inferred.1.is_empty());
    assert!(conn.execute("INSERT INTO subscription_outbox(account_generation,account_key,action,feed_url) VALUES(0,'x','invalid','x')",[]).is_err());
    assert!(conn
        .execute(
            "INSERT INTO sync_queue(action,status) VALUES('read','invalid')",
            []
        )
        .is_err());
    assert!(conn
        .prepare("SELECT rowid FROM articles_trigram_idx WHERE articles_trigram_idx MATCH 'dat'")
        .is_ok());
    drop(conn);
    let conn = db::open(&path).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        0
    );
}

#[test]
fn foreign_application_id_and_future_schema_are_read_only_rejected() {
    for (tag, version, app_id) in [("foreign", 1, 17), ("future", 2, 1_179_796_018)] {
        let path = common::unique_db_path(tag);
        let _guard = common::TempDbGuard::new(path.clone());
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE retained(value TEXT);INSERT INTO retained VALUES('keep');",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", version).unwrap();
        conn.pragma_update(None, "application_id", app_id).unwrap();
        drop(conn);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(db::open(&path).unwrap_err().code, "unsupported_database");
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn pre_provenance_current_format_is_read_only_rejected() {
    let path = common::unique_db_path("pre_provenance_format");
    let _guard = common::TempDbGuard::new(path.clone());
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE articles(id INTEGER PRIMARY KEY,image_url TEXT);INSERT INTO articles VALUES(1,'keep.png');PRAGMA application_id=1179796018;PRAGMA user_version=1;").unwrap();
    drop(conn);
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(db::open(&path).unwrap_err().code, "unsupported_database");
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}
