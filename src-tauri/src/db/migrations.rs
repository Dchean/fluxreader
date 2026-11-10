//! Current format initialization. Old files are refused before WAL or credential writes.
// Note: 新格式不自动升级/删除旧库 — 见 .agents/notes/proposed/architecture/2026-11-09-分区阅读与源内身份及持续同步契约.md
use super::*;
use crate::error::AppError;

pub(crate) const LATEST_SCHEMA_VERSION: i64 = 1;
pub const APPLICATION_ID: i64 = 1_179_796_018;
pub(crate) static MIGRATIONS: LazyLock<Migrations> =
    LazyLock::new(|| Migrations::new(vec![M::up(include_str!("schema.sql"))]));

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
                || check
                    .prepare("SELECT image_source, remote_origin_feed_id FROM articles LIMIT 0")
                    .is_err())
        {
            return Err(AppError::new("unsupported_database", format!("此数据库为不兼容的旧格式（版本 {version}）。原文件已保留：{}。请先关闭应用，将该文件及同名 -wal/-shm 文件移到备份目录，再启动以创建新库；不要覆盖原文件。", path.display())));
        }
    }
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    MIGRATIONS.to_latest(&mut conn)?;
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
}
