//! 数据库快照：迁移前、镜像删除前与用户手动触发的完整 SQLite 备份。
//!
//! 备份统一经 `VACUUM INTO` 生成自包含副本，不直接复制主库文件：SQLite 会在
//! 同一读快照内把 WAL 中尚未 checkpoint 的已提交页一并写入目标库，避免
//! `fluxreader.db` 与 `-wal` 分离复制造成的旧状态/半状态。目标文件先写为
//! `.partial`，经只读 `quick_check` 与 `user_version` 核验后再原子改名。
//!
//! 保留策略：自动快照按类型各保留最近 [`BACKUP_RETENTION`] 份；手动备份永不
//! 自动删除。文件名含类型、from/to schema 版本、UTC 时间与进程内序号。
//!
//! 安全边界：备份包含完整数据库（订阅、正文、读藏状态与 DPAPI 密文凭据）。
//! 凭据在快照前会先完成历史明文迁移；备份仍应视作用户私有数据。日志只记录
//! 脱敏后的说明，不输出含用户名的绝对路径。
//!
//! Note: 快照方案、保留策略与恢复边界 — 见
//! .agents/notes/implemented/architecture/2026-11-05-数据库快照与恢复边界.md

use crate::error::{AppError, AppResult};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// 自动快照保留份数（按 migration / mirror 两类分别计算）。
pub const BACKUP_RETENTION: usize = 10;

/// 迁移前备份失败时写入 settings 的一次性启动警告键。
pub(crate) const STARTUP_BACKUP_WARNING_KEY: &str = "backup_startup_warning";

static BACKUP_SEQ: AtomicU64 = AtomicU64::new(0);

/// 备份结果（IPC 与前端同形的 camelCase 契约）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupInfo {
    pub path: String,
    pub file_name: String,
    pub size_bytes: u64,
    pub schema_version: i64,
    pub from_version: i64,
    pub to_version: i64,
    pub created_at: String,
    pub kind: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackupKind {
    Migration,
    Mirror,
    Manual,
}

impl BackupKind {
    fn as_str(self) -> &'static str {
        match self {
            BackupKind::Migration => "migration",
            BackupKind::Mirror => "mirror",
            BackupKind::Manual => "manual",
        }
    }
}

/// 主库绝对/相对路径（来自 SQLite 自身，而不是另存一份可能漂移的配置）。
pub fn database_path(conn: &Connection) -> AppResult<PathBuf> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT file FROM pragma_database_list WHERE name = 'main'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let path = raw
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            AppError::new(
                "backupUnavailable",
                "当前数据库没有可备份的磁盘文件（内存库不支持备份）",
            )
        })?;
    Ok(PathBuf::from(path))
}

pub fn backup_dir_for_db(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("backups")
}

pub fn user_version(conn: &Connection) -> AppResult<i64> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}

/// 用户手动备份：from/to 都是当前 schema，不参与自动保留清理。
pub fn create_manual_backup(conn: &Connection) -> AppResult<BackupInfo> {
    let db_path = database_path(conn)?;
    let version = user_version(conn)?;
    create_backup(conn, &db_path, BackupKind::Manual, version, version, false)
}

/// 迁移前快照：由数据库当前版本升向目标版本时调用。
#[cfg(test)]
fn create_migration_backup(
    conn: &Connection,
    db_path: &Path,
    from_version: i64,
    to_version: i64,
) -> AppResult<BackupInfo> {
    create_backup(
        conn,
        db_path,
        BackupKind::Migration,
        from_version,
        to_version,
        true,
    )
}

/// 镜像删除前快照。调用方须在删除事务开始前调用。
pub(crate) fn create_mirror_backup(conn: &Connection) -> AppResult<BackupInfo> {
    let db_path = database_path(conn)?;
    let version = user_version(conn)?;
    create_backup(conn, &db_path, BackupKind::Mirror, version, version, true)
}

fn create_backup(
    conn: &Connection,
    db_path: &Path,
    kind: BackupKind,
    from_version: i64,
    to_version: i64,
    prune: bool,
) -> AppResult<BackupInfo> {
    let backup_dir = backup_dir_for_db(db_path);
    fs::create_dir_all(&backup_dir).map_err(|e| backup_io_error(db_path, e))?;

    let created_at = chrono::Utc::now();
    let stamp = created_at.format("%Y%m%dT%H%M%S%.3fZ");
    let seq = BACKUP_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let file_name = format!(
        "{kind}-v{from_version}-to-v{to_version}-{stamp}-p{pid}-{seq:06}.db",
        kind = kind.as_str(),
        pid = std::process::id(),
    );
    let target = backup_dir.join(&file_name);
    let partial = backup_dir.join(format!(".{file_name}.partial"));
    if target.exists() || partial.exists() {
        return Err(AppError::new(
            "backup",
            "备份文件名冲突，未覆盖任何既有备份",
        ));
    }

    // VACUUM INTO 是单条 SQL 的一致性快照：会读取当前 WAL 可见页，不要求
    // checkpoint；参数绑定避免路径引号/反斜杠进入 SQL 文本。
    let partial_text = partial.to_string_lossy().to_string();
    if let Err(e) = conn.execute("VACUUM INTO ?1", params![partial_text]) {
        remove_if_exists(&partial);
        return Err(backup_error(db_path, "创建数据库快照失败", e.to_string()));
    }

    let verified = verify_backup(&partial, from_version).map_err(|e| {
        remove_if_exists(&partial);
        backup_error(db_path, "数据库快照完整性校验失败", e)
    })?;

    if let Err(e) = fs::rename(&partial, &target) {
        remove_if_exists(&partial);
        return Err(backup_io_error(db_path, e));
    }

    let size_bytes = verified.size_bytes;
    if prune && kind != BackupKind::Manual {
        if let Err(e) = prune_backups(&backup_dir, kind, BACKUP_RETENTION, &target) {
            log::warn!(
                "backup: 旧备份清理失败（本次快照已保留）: {}",
                sanitize_log_text(&backup_dir, &e.to_string())
            );
        }
    }

    Ok(BackupInfo {
        path: target.to_string_lossy().to_string(),
        file_name,
        size_bytes,
        schema_version: from_version,
        from_version,
        to_version,
        created_at: created_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        kind: kind.as_str().to_string(),
    })
}

struct VerifiedBackup {
    size_bytes: u64,
}

fn verify_backup(path: &Path, expected_version: i64) -> Result<VerifiedBackup, String> {
    let check = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("无法打开快照：{e}"))?;
    let quick: String = check
        .query_row("PRAGMA quick_check", [], |r| r.get(0))
        .map_err(|e| format!("quick_check 读取失败：{e}"))?;
    if quick != "ok" {
        return Err(format!("quick_check 未通过：{quick}"));
    }
    let actual: i64 = check
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| format!("user_version 读取失败：{e}"))?;
    if actual != expected_version {
        return Err(format!(
            "快照 schema 版本不匹配（期望 {expected_version}，实际 {actual}）"
        ));
    }
    let has_schema: i64 = check
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'",
            [],
            |r| r.get(0),
        )
        .map_err(|e| format!("schema 读取失败：{e}"))?;
    if has_schema == 0 && expected_version > 0 {
        return Err("快照缺少任何表，拒绝交付空备份".to_string());
    }
    drop(check);
    let size_bytes = fs::metadata(path)
        .map_err(|e| format!("读取快照大小失败：{e}"))?
        .len();
    if size_bytes == 0 {
        return Err("快照文件为空".to_string());
    }
    Ok(VerifiedBackup { size_bytes })
}

struct GeneratedBackupName {
    kind: BackupKind,
    created_at: chrono::NaiveDateTime,
    process_id: u32,
    sequence: u64,
}

fn parse_generated_backup_name(file_name: &str) -> Option<GeneratedBackupName> {
    let (kind, rest) = file_name.strip_suffix(".db")?.split_once("-v")?;
    let kind = match kind {
        "migration" => BackupKind::Migration,
        "mirror" => BackupKind::Mirror,
        _ => return None,
    };
    let (from, rest) = rest.split_once("-to-v")?;
    let (to, rest) = rest.split_once('-')?;
    let from_version = from.parse::<u32>().ok()?;
    let to_version = to.parse::<u32>().ok()?;
    if from_version.to_string() != from
        || to_version.to_string() != to
        || from_version > i32::MAX as u32
        || to_version > i32::MAX as u32
        || (kind == BackupKind::Migration && from_version >= to_version)
        || (kind == BackupKind::Mirror && from_version != to_version)
    {
        return None;
    }

    let (stamp, rest) = rest.split_once("-p")?;
    let created_at = chrono::NaiveDateTime::parse_from_str(stamp, "%Y%m%dT%H%M%S%.3fZ").ok()?;
    if stamp.len() != 20 || created_at.format("%Y%m%dT%H%M%S%.3fZ").to_string() != stamp {
        return None;
    }
    let (pid, seq) = rest.split_once('-')?;
    let process_id = pid.parse::<u32>().ok()?;
    let sequence = seq.parse::<u64>().ok()?;
    if process_id == 0
        || process_id.to_string() != pid
        || sequence == 0
        || format!("{sequence:06}") != seq
    {
        return None;
    }
    Some(GeneratedBackupName {
        kind,
        created_at,
        process_id,
        sequence,
    })
}

/// 只清理完整合法生成名的普通自动快照，按 UTC、数值序号、PID、文件名排序。
/// 本次目标无条件保留；手动、未知命名、目录与符号链接不进入候选集。
fn prune_backups(
    dir: &Path,
    kind: BackupKind,
    keep: usize,
    protected: &Path,
) -> std::io::Result<usize> {
    if kind == BackupKind::Manual {
        return Ok(0);
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let file_name = entry.file_name();
        let Some(parsed) = file_name.to_str().and_then(parse_generated_backup_name) else {
            continue;
        };
        if parsed.kind == kind {
            files.push((parsed, entry.path()));
        }
    }
    // Note: schema 版本不是时间顺序，本次目标也不得被时钟回拨挤掉 — 见
    // .agents/notes/implemented/architecture/2026-11-05-数据库快照与恢复边界.md
    files.sort_by(|(left, left_path), (right, right_path)| {
        (left.created_at, left.sequence, left.process_id)
            .cmp(&(right.created_at, right.sequence, right.process_id))
            .then_with(|| left_path.file_name().cmp(&right_path.file_name()))
    });
    let remove_count = files.len().saturating_sub(keep);
    let mut removed = 0usize;
    for (_, path) in files
        .iter()
        .filter(|(_, path)| path != protected)
        .take(remove_count)
    {
        if !fs::symlink_metadata(path)?.file_type().is_file() {
            continue;
        }
        match fs::remove_file(path) {
            Ok(()) => removed += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(removed)
}

/// 读取并消费一次性启动备份警告（成功读取后删除，避免每次启动重复弹）。
pub(crate) fn take_startup_backup_warning(conn: &Connection) -> AppResult<Option<String>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![STARTUP_BACKUP_WARNING_KEY],
            |r| r.get(0),
        )
        .optional()?;
    let Some(raw) = raw.filter(|s| !s.trim().is_empty()) else {
        return Ok(None);
    };
    conn.execute(
        "DELETE FROM settings WHERE key = ?1",
        params![STARTUP_BACKUP_WARNING_KEY],
    )?;
    Ok(Some(raw))
}

/// 错误文案落库/日志前去掉应用数据目录（通常含用户名）。
pub(crate) fn sanitize_log_text(app_dir: &Path, text: &str) -> String {
    let raw = app_dir.to_string_lossy();
    if raw.is_empty() {
        return text.to_string();
    }
    text.replace(raw.as_ref(), "<app-data>")
}

fn backup_error(db_path: &Path, context: &str, detail: String) -> AppError {
    // 公开错误与日志都经同一脱敏：保留底层原因，但不回显含用户名的目录。
    let app_dir = db_path.parent().unwrap_or_else(|| Path::new("."));
    AppError::new(
        "backup",
        format!("{context}：{}", sanitize_log_text(app_dir, &detail)),
    )
}

fn backup_io_error(db_path: &Path, error: std::io::Error) -> AppError {
    let app_dir = db_path.parent().unwrap_or_else(|| Path::new("."));
    AppError::new(
        "backup",
        format!(
            "备份目录不可用：{}",
            sanitize_log_text(app_dir, &error.to_string())
        ),
    )
}

fn remove_if_exists(path: &Path) {
    let _ = fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_path(base: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "fluxreader_backup_{base}_{}_{nanos}_{seq}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir.join("fluxreader.db")
    }

    fn wal_conn(path: &Path) -> Connection {
        let conn = Connection::open(path).unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        conn.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
        conn.execute_batch(
            "CREATE TABLE probe (id INTEGER PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO probe (value) VALUES ('before-wal-checkpoint');",
        )
        .unwrap();
        conn
    }

    #[test]
    fn vacuum_snapshot_includes_uncheckpointed_wal_data() {
        let path = temp_path("wal");
        let _ = fs::remove_file(&path);
        let conn = wal_conn(&path);
        let wal = PathBuf::from(format!("{}-wal", path.display()));
        let wal_len = fs::metadata(&wal).unwrap().len();
        assert!(wal_len > 0, "前置：已提交数据仍在 WAL，尚未 checkpoint");

        let info = create_manual_backup(&conn).expect("WAL 快照必须成功");
        assert!(info.size_bytes > 0);
        assert_eq!(info.schema_version, 0);
        let backed = Connection::open(&info.path).unwrap();
        let value: String = backed
            .query_row("SELECT value FROM probe WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(value, "before-wal-checkpoint");
        let quick: String = backed
            .query_row("PRAGMA quick_check", [], |r| r.get(0))
            .unwrap();
        assert_eq!(quick, "ok");
        drop(backed);
        drop(conn);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn automatic_snapshots_keep_latest_n_and_manual_is_never_pruned() {
        let path = temp_path("retention");
        let _ = fs::remove_file(&path);
        let conn = wal_conn(&path);
        let manual = create_manual_backup(&conn).unwrap();
        for _ in 0..(BACKUP_RETENTION + 2) {
            create_migration_backup(&conn, &path, 0, 1).unwrap();
            create_mirror_backup(&conn).unwrap();
        }
        let dir = backup_dir_for_db(&path);
        for kind in [BackupKind::Migration, BackupKind::Mirror] {
            assert_eq!(automatic_snapshot_paths(&dir, kind).len(), BACKUP_RETENTION);
        }
        assert!(
            Path::new(&manual.path).exists(),
            "手动备份不得被自动保留策略删除"
        );
        drop(conn);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    fn automatic_snapshot_paths(dir: &Path, kind: BackupKind) -> Vec<PathBuf> {
        fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap())
            .filter(|entry| {
                entry.file_type().unwrap().is_file()
                    && entry
                        .file_name()
                        .to_str()
                        .and_then(parse_generated_backup_name)
                        .is_some_and(|parsed| parsed.kind == kind)
            })
            .map(|entry| entry.path())
            .collect()
    }

    #[test]
    fn automatic_backup_retention_orders_mixed_schema_versions_chronologically() {
        let path = temp_path("mixed_schema_retention");
        let conn = wal_conn(&path);
        conn.pragma_update(None, "user_version", 9).unwrap();
        let manual = create_manual_backup(&conn).unwrap();
        let mut previous = Vec::new();
        for _ in 0..BACKUP_RETENTION {
            previous.push(create_migration_backup(&conn, &path, 9, 10).unwrap());
        }
        conn.pragma_update(None, "user_version", 10).unwrap();
        let current = create_migration_backup(&conn, &path, 10, 11).unwrap();

        assert!(
            Path::new(&current.path).is_file(),
            "返回成功的本次快照必须存在"
        );
        assert!(!Path::new(&previous[0].path).exists(), "删除时间最早的快照");
        assert!(previous[1..]
            .iter()
            .all(|info| Path::new(&info.path).is_file()));
        assert!(Path::new(&manual.path).is_file(), "手动备份不参与清理");
        assert_eq!(
            automatic_snapshot_paths(&backup_dir_for_db(&path), BackupKind::Migration).len(),
            BACKUP_RETENTION
        );
        drop(conn);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn automatic_backup_retention_uses_numeric_nonce_with_equal_timestamps() {
        let path = temp_path("nonce_retention");
        let dir = backup_dir_for_db(&path);
        fs::create_dir_all(&dir).unwrap();
        let mut files = Vec::new();
        for offset in 0..(BACKUP_RETENTION + 2) {
            let sequence = 999_999 + offset;
            let (from, to) = if offset % 2 == 0 { (9, 10) } else { (10, 11) };
            let file = dir.join(format!(
                "migration-v{from}-to-v{to}-20261105T100000.000Z-p7-{sequence:06}.db"
            ));
            fs::write(&file, b"synthetic-retention-fixture").unwrap();
            files.push(file);
        }
        let current = files.last().unwrap();
        assert_eq!(
            prune_backups(&dir, BackupKind::Migration, BACKUP_RETENTION, current).unwrap(),
            2
        );
        assert!(files[..2].iter().all(|file| !file.exists()));
        assert!(files[2..].iter().all(|file| file.is_file()));
        assert_eq!(
            automatic_snapshot_paths(&dir, BackupKind::Migration).len(),
            BACKUP_RETENTION
        );
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn automatic_backup_retention_protects_current_target_after_clock_rollback() {
        let path = temp_path("clock_rollback_retention");
        let dir = backup_dir_for_db(&path);
        fs::create_dir_all(&dir).unwrap();
        let current = dir.join("mirror-v34-to-v34-20261104T100000.000Z-p7-000099.db");
        fs::write(&current, b"synthetic-current-fixture").unwrap();
        let mut previous = Vec::new();
        for sequence in 1..=BACKUP_RETENTION {
            let file = dir.join(format!(
                "mirror-v34-to-v34-20261105T100000.000Z-p7-{sequence:06}.db"
            ));
            fs::write(&file, b"synthetic-retention-fixture").unwrap();
            previous.push(file);
        }

        assert_eq!(
            prune_backups(&dir, BackupKind::Mirror, BACKUP_RETENTION, &current).unwrap(),
            1
        );
        assert!(current.is_file(), "本次目标即使时间回拨也必须保留");
        assert!(!previous[0].exists());
        assert!(previous[1..].iter().all(|file| file.is_file()));
        assert_eq!(
            automatic_snapshot_paths(&dir, BackupKind::Mirror).len(),
            BACKUP_RETENTION
        );
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn automatic_backup_retention_preserves_non_generated_files_and_directories() {
        let path = temp_path("foreign_files_retention");
        let conn = wal_conn(&path);
        let manual = create_manual_backup(&conn).unwrap();
        let dir = backup_dir_for_db(&path);
        let names = [
            "migration-v0-user-archive.db",
            "migration-v0-to-v1-20260230T100000.000Z-p7-000001.db",
            "migration-v0-to-v1-20261105T250000.000Z-p7-000001.db",
            "migration-v0-to-v1-20261105T100000.000-p7-000001.db",
            "migration-v0-to-v1-20261105T100000.000Z-p0-000001.db",
            "migration-v0-to-v1-20261105T100000.000Z-p7-000000.db",
            "migration-v00-to-v1-20261105T100000.000Z-p7-000001.db",
            "migration-v0-to-v1-20261105T100000.000Z-p07-000001.db",
            "migration-v0-to-v1-20261105T100000.000Z-p7-1.db",
            "migration-v2-to-v1-20261105T100000.000Z-p7-000001.db",
            "migration-v0-to-v2147483648-20261105T100000.000Z-p7-000001.db",
            ".migration-v0-to-v1-20261105T100000.000Z-p7-000001.db.partial",
            "migration-v0-to-v1-20261105T100000.000Z-p7-000001.db.partial",
            "migration-v0-to-v1-20261105T100000.000Z-p7-000001.db.other",
        ];
        for name in names {
            fs::write(dir.join(name), b"synthetic-user-file").unwrap();
        }
        let directory = dir.join("migration-v0-to-v1-20000101T000000.000Z-p7-000001.db");
        fs::create_dir(&directory).unwrap();
        let marker = directory.join("keep.txt");
        fs::write(&marker, b"synthetic-user-directory").unwrap();

        for _ in 0..(BACKUP_RETENTION + 2) {
            create_migration_backup(&conn, &path, 0, 1).unwrap();
        }
        for name in names {
            assert_eq!(
                fs::read(dir.join(name)).unwrap(),
                b"synthetic-user-file",
                "{name}"
            );
        }
        assert_eq!(fs::read(&marker).unwrap(), b"synthetic-user-directory");
        assert!(Path::new(&manual.path).is_file());
        assert_eq!(
            automatic_snapshot_paths(&dir, BackupKind::Migration).len(),
            BACKUP_RETENTION
        );
        drop(conn);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[cfg(any(windows, unix))]
    #[test]
    fn automatic_backup_retention_preserves_symlinks() {
        let path = temp_path("symlink_retention");
        let conn = wal_conn(&path);
        let dir = backup_dir_for_db(&path);
        fs::create_dir_all(&dir).unwrap();
        let user_file = path.parent().unwrap().join("user-archive.db");
        fs::write(&user_file, b"synthetic-user-archive").unwrap();
        let symlink = dir.join("migration-v0-to-v1-20000101T000000.000Z-p7-000001.db");
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&user_file, &symlink).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&user_file, &symlink).unwrap();

        for _ in 0..(BACKUP_RETENTION + 2) {
            create_migration_backup(&conn, &path, 0, 1).unwrap();
        }
        assert!(fs::symlink_metadata(&symlink)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read(&user_file).unwrap(), b"synthetic-user-archive");
        assert_eq!(
            automatic_snapshot_paths(&dir, BackupKind::Migration).len(),
            BACKUP_RETENTION
        );
        drop(conn);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn manual_backup_stays_consistent_while_another_connection_writes() {
        let path = temp_path("concurrent");
        let _ = fs::remove_file(&path);
        let backup_conn = wal_conn(&path);
        backup_conn
            .execute(
                "WITH RECURSIVE seq(n) AS (
                     SELECT 1 UNION ALL SELECT n + 1 FROM seq WHERE n < 2000
                 )
                 INSERT INTO probe (value) SELECT printf('seed-%04d-', n) || zeroblob(1024) FROM seq",
                [],
            )
            .unwrap();
        let writer = Connection::open(&path).unwrap();
        writer.pragma_update(None, "busy_timeout", 5000).unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let writer_thread = std::thread::spawn(move || {
            for i in 0..600 {
                writer
                    .execute(
                        "INSERT INTO probe (value) VALUES (printf('write-%04d-', ?1) || zeroblob(1024))",
                        params![i],
                    )
                    .unwrap();
                if i == 50 {
                    started_tx.send(()).unwrap();
                }
            }
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("并发写必须已经开始");

        let info = create_manual_backup(&backup_conn).expect("并发写期间备份必须成功");
        writer_thread.join().unwrap();
        let final_count: i64 = backup_conn
            .query_row("SELECT COUNT(*) FROM probe", [], |r| r.get(0))
            .unwrap();
        assert!(final_count > 2000, "并发写确实推进了源库");

        let snap = Connection::open(&info.path).unwrap();
        let quick: String = snap
            .query_row("PRAGMA quick_check", [], |r| r.get(0))
            .unwrap();
        assert_eq!(quick, "ok", "并发写期间生成的快照必须完整可读");
        let snapshot_count: i64 = snap
            .query_row("SELECT COUNT(*) FROM probe", [], |r| r.get(0))
            .unwrap();
        assert!(
            (2001..=final_count).contains(&snapshot_count),
            "快照必须是某一一致时点（seed 全在、写入数不超过源库最终值）：{snapshot_count}/{final_count}"
        );
        drop(snap);
        drop(backup_conn);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn sanitize_log_text_removes_user_profile_prefix() {
        let app_dir = Path::new(r"C:\Users\alice\AppData\Roaming\com.fluxreader.app");
        let text = r"failed: C:\Users\alice\AppData\Roaming\com.fluxreader.app\backups\x.db";
        let safe = sanitize_log_text(app_dir, text);
        assert!(!safe.contains("alice"));
        assert!(safe.contains("<app-data>"));
    }
}
