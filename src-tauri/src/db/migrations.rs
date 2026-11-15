//! Current format initialization. Old files are refused before WAL or credential writes.
// Note: 新格式不自动升级/删除旧库 — 见 docs/architecture.md
use super::*;
use crate::error::AppError;

pub(crate) const LATEST_SCHEMA_VERSION: i64 = 1;
pub const APPLICATION_ID: i64 = 1_179_796_018;
pub(crate) static MIGRATIONS: LazyLock<Migrations> =
    LazyLock::new(|| Migrations::new(vec![M::up(include_str!("schema.sql"))]));

/// 形状探针要求的索引清单（与 schema.sql 同步维护；不含已删除的 url_norm 部分索引
/// `idx_articles_url_norm_unread`——存量库允许保留它，探针只查名单内对象在场）。
const SHAPE_INDEXES: &[&str] = &[
    "idx_articles_feed",
    "idx_articles_sync_source_guid",
    "idx_articles_published",
    "idx_articles_unread",
    "idx_articles_miniflux_id",
    "idx_articles_url_norm",
    "idx_articles_feed_published",
    "idx_articles_read_published",
    "idx_articles_published_id",
    "idx_sync_queue_article",
    "idx_sync_queue_created",
    "idx_contents_url_norm",
    "idx_articles_content",
    "idx_articles_rss_content",
    "idx_articles_body_retention",
    "idx_articles_body_retention_null",
    "idx_subscription_outbox_url",
    "idx_subscription_outbox_pending",
    "idx_sync_queue_status",
    "idx_articles_rss_group",
    "idx_feeds_sync_id",
    "local_sync_article_guid",
    "local_sync_object_child",
    "local_sync_hold_article",
    "local_sync_body_ref",
    "local_sync_hold_primary",
    "local_sync_hold_rss",
    "local_sync_receipt_hold_id",
    "local_sync_fields_source",
    "local_sync_fields_url",
    "local_sync_fields_body_rss",
    "local_sync_candidates_source",
    "local_sync_candidates_guid",
    "local_sync_candidates_url",
    "local_sync_candidates_body_rss",
    "local_sync_baseline_key",
];

/// 形状探针要求的触发器清单（含 retention/media 全部触发器；与 schema.sql 同步维护）。
const SHAPE_TRIGGERS: &[&str] = &[
    "articles_remote_origin_invalidate",
    "feeds_remote_origin_invalidate",
    "feeds_remote_identity_advance",
    "trg_articles_retention_ai",
    "trg_articles_retention_state",
    "trg_articles_retention_restore",
    "trg_articles_published_fallback_ins",
    "trg_articles_published_fallback_upd",
    "trg_articles_recycle_ad",
    "trg_articles_recycle_au",
    "trg_articles_trigram_ai",
    "trg_articles_trigram_ad",
    "trg_articles_trigram_au",
    "trg_contents_trigram_au",
    "trg_sync_queue_article_bound",
    "trg_sync_queue_origin_waiting",
    "trg_articles_first_seen_fallback_ins",
    "trg_feeds_refresh_title_defaults_ins",
    "trg_local_feed_folder_insert",
    "trg_local_feed_folder_update",
    "trg_source_local_history",
    "trg_articles_media_au",
];

/// 缺任一探针对象即报错（open_with_report 据此拒绝旧库）：防止纯索引/触发器层面
/// 的形状漂移被静默接受。逐项查在场（COUNT=1），多余对象不拒。
fn shape_probe(conn: &Connection) -> rusqlite::Result<()> {
    for (kind, names) in [("index", SHAPE_INDEXES), ("trigger", SHAPE_TRIGGERS)] {
        for name in names {
            let n: i64 = conn.query_row(
                &format!("SELECT COUNT(*) FROM sqlite_master WHERE type='{kind}' AND name=?1"),
                rusqlite::params![name],
                |r| r.get(0),
            )?;
            if n != 1 {
                return Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
                    Some(format!("shape probe: missing {kind} {name}")),
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn current_shape(conn: &Connection) -> rusqlite::Result<()> {
    conn.prepare("SELECT image_source,remote_origin_feed_id,local_sync_source,body_retention_since,(SELECT wire_key FROM local_sync_source_values LIMIT 1),(SELECT id FROM local_sync_body_hold LIMIT 1),(SELECT entry FROM local_sync_body_receipts LIMIT 1),(SELECT hold_id FROM local_sync_body_receipt_holds LIMIT 1),(SELECT rejected_code FROM local_sync_receive LIMIT 1),(SELECT validated_clock FROM local_sync_receive LIMIT 1), (SELECT actor FROM local_sync_control WHERE id=1), (SELECT origin FROM folders LIMIT 1), (SELECT child FROM local_sync_object_links LIMIT 1), (SELECT digest FROM local_sync_object_pins LIMIT 1) FROM articles LIMIT 0")?;
    shape_probe(conn)
}

pub fn open(path: &Path) -> AppResult<Connection> {
    open_with_report(path)
}

pub(crate) fn open_with_report(path: &Path) -> AppResult<Connection> {
    // Inspect existing files read-only first: even switching journal_mode writes a database.
    if path.exists() {
        let check = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let version: i64 = check.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let app_id: i64 = check.pragma_query_value(None, "application_id", |r| r.get(0))?;
        let has_tables: bool = check.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%')", [], |r| r.get(0))?;
        if (has_tables || version != 0 || app_id != 0)
            && (version != LATEST_SCHEMA_VERSION
                || app_id != APPLICATION_ID
                || current_shape(&check).is_err())
        {
            return Err(AppError::new("unsupported_database", format!("此数据库为不兼容的旧格式（版本 {version}）。原文件已保留：{}。请先关闭应用，将该文件及同名 -wal/-shm 文件移到备份目录，再启动以创建新库；不要覆盖原文件。", path.display())));
        }
    }
    let mut conn = Connection::open(path)?;
    crate::local_sync::database::register_functions(&conn)?;
    let writer = super::writer_guard(&conn)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    MIGRATIONS.to_latest(&mut conn)?;
    drop(writer);
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_schema_has_format_id_foreign_keys_and_no_legacy_upgrade_steps() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        assert_eq!(
            super::super::backup::user_version(&conn).unwrap(),
            LATEST_SCHEMA_VERSION
        );
        assert_eq!(
            conn.pragma_query_value(None, "application_id", |r| r.get::<_, i64>(0))
                .unwrap(),
            APPLICATION_ID
        );
        assert!(conn
            .prepare("SELECT rss_content_id, media_revision FROM articles")
            .is_ok());
        assert!(conn.prepare("SELECT active, sync_id FROM feeds").is_ok());
    }

    /// N-20 验收：建全形库 → 手动 DROP 一个索引/触发器 → open_with_report 必须
    /// 拒绝（unsupported_database）而非静默接受；另设对照臂证明完整形状可打开。
    #[test]
    fn open_with_report_rejects_dropped_index_or_trigger() {
        // 临时根与 db/writer_tests.rs 同约定：FLUXREADER_TEST_ROOT 优先，缺省回退
        // std::env::temp_dir()（隔离环境把两者都指向可写目录）。
        let root: std::path::PathBuf = std::env::var_os("FLUXREADER_TEST_ROOT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let cases: [(&str, &str); 3] = [
            ("INDEX", "idx_articles_feed_published"),
            ("TRIGGER", "trg_articles_retention_state"),
            ("TRIGGER", "trg_articles_media_au"),
        ];
        let pid = std::process::id();
        for (i, (kind, obj)) in cases.iter().enumerate() {
            let path = root.join(format!("fluxreader_n20_{pid}_{i}.db"));
            let _ = std::fs::remove_file(&path);
            // 全形库
            {
                let mut conn = Connection::open(&path).unwrap();
                MIGRATIONS.to_latest(&mut conn).unwrap();
            }
            // 故意删对象（user_version/application_id 仍正确 → 只能靠形状探针拦截）
            {
                let conn = Connection::open(&path).unwrap();
                conn.execute(&format!("DROP {kind} {obj}"), []).unwrap();
            }
            let err = open_with_report(&path).unwrap_err();
            assert_eq!(
                err.code, "unsupported_database",
                "缺 {kind} {obj} 的库必须被拒绝"
            );
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_file(path.with_extension("db-wal"));
            let _ = std::fs::remove_file(path.with_extension("db-shm"));
        }
        // 对照臂：完整形状（含全部探针对象）允许打开。
        let path = root.join(format!("fluxreader_n20_{pid}_ok.db"));
        let _ = std::fs::remove_file(&path);
        {
            let mut conn = Connection::open(&path).unwrap();
            MIGRATIONS.to_latest(&mut conn).unwrap();
        }
        let conn = open_with_report(&path).unwrap();
        drop(conn);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }
}
