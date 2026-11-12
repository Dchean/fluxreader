//! contents：单一正文所有权（OPT-008B 第二阶段）。
//!
//! 设计（契约见 .agents/notes/implemented/architecture/2026-10-12-订阅条目与共享内容身份.md
//! 的「第二阶段契约」）：
//! - 正文（content_html / body_text / 提取标志）只存 `contents` 一行；多个条目
//!   （articles.content_id）可引用同一行——**内容真正等价**才共享。
//! - 身份 = `'h|' + md5(extraction_source + 0x1f + html + 0x1f + body)`；
//!   来源进身份：RSS 原文与 Readability 提取、不同 HTML/纯文各自成行（确定分叉
//!   边界）。哈希命中后**再做字节级等价复核**才复用（哈希碰撞退化为新行/私有行，
//!   不会误并）。空 URL 或空正文且 html 也为空 → `'e|<article_id>'` 条目私有行：
//!   不聚合、不误并无关条目。
//! - `revision` 单调递增；`(id, revision)` 是给 013 复用的稳定内容版本。
//! - id 稳定性（R3）：`contents.id` 为 AUTOINCREMENT——删除表内最大行后新行
//!   不复用该 id（普通 rowid 会复用）。前端以 `(content_id, revision)` 判定正文
//!   失效，id 复用会在「删除 + 重建恰好同快照窗口」时读到同值对。
//! - 回收：条目删除（含源/分类/账号级联）由迁移创建的
//!   `trg_articles_recycle_ad` 触发器在**最后引用**消失时删除 contents 行；
//!   条目重指新内容时调用方在同一语句末尾回收无引用的旧行（见
//!   articles/sync_map 的重指路径）。该触发器在 OPT-017B 之前与
//!   `trg_articles_fts_ad` 同体（顺带清 FTS 行）；v29 删除无消费者 FTS 后
//!   独立命名，只做回收。
//! - 原子性（R3）：`ensure_content` 的 INSERT 与随后的条目重指/回填是复合写，
//!   调用方经 [`with_write_savepoint`] 包住整组语句——任一步失败整体回滚，
//!   不会留下「已 INSERT 的 contents 行无任何引用」（孤儿）或「条目 content_id
//!   未回填」的半提交态。SAVEPOINT 可安全嵌套于调用方既有事务内。
//! - 搜索派生索引：v29 起为独立 trigram 候选表 `articles_trigram_idx`
//!   （维护触发器在 migrations.rs v29；候选只是收缩，等值验证仍由
//!   `search_articles` 的 LIKE 条件完成）。v22 时代的 `articles_fts` 已被
//!   v29 删除；本模块的 [`rebuild_fts`] 仅为历史 v22 迁移的实现细节保留。

use md5::{Digest, Md5};
use rusqlite::{params, Connection, OptionalExtension};

use crate::error::{AppError, AppResult};

/// 内容来源：直连/同步抓取到的 RSS/协议原文。
pub const SOURCE_RSS: &str = "rss";
/// 内容来源：Readability 全文提取结果。
pub const SOURCE_READABILITY: &str = "readability";

/// 待落库的一份内容（条目入库/刷新/提取的输入形状）。
#[derive(Debug, Clone, PartialEq)]
pub struct ContentInput {
    pub content_html: Option<String>,
    pub body_text: String,
    /// 'rss' | 'readability'（未来可扩展其它提取来源；进身份 → 不同来源分叉）
    pub extraction_source: String,
    pub fulltext_extracted: bool,
    pub url_norm: Option<String>,
}

/// 内容是否「非空」：html 或纯文本任一非空才可参与跨条目共享。
pub fn content_non_empty(html: Option<&str>, body: &str) -> bool {
    html.is_some_and(|h| !h.is_empty()) || !body.is_empty()
}

/// 等价内容的身份串（非空内容）。空内容由调用方走条目私有身份。
pub fn compute_identity(source: &str, html: Option<&str>, body: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(source.as_bytes());
    hasher.update([0x1f]);
    hasher.update(html.unwrap_or("").as_bytes());
    hasher.update([0x1f]);
    hasher.update(body.as_bytes());
    format!(
        "h|{}|{}|{}",
        hex_lower(&hasher.finalize()),
        html.map_or(0, str::len),
        body.len()
    )
}

/// 条目私有身份（空内容：不聚合；同一条目重放幂等）。
pub fn entry_private_identity(article_id: i64) -> String {
    format!("e|{article_id}")
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

fn input_html(input: &ContentInput) -> Option<&str> {
    input.content_html.as_deref().filter(|s| !s.is_empty())
}

struct SavepointGuard<'a> {
    conn: &'a Connection,
    name: &'a str,
    recovery: super::writer::RecoveryToken,
    active: bool,
}

impl<'a> SavepointGuard<'a> {
    fn begin(
        conn: &'a Connection,
        name: &'a str,
        recovery: super::writer::RecoveryToken,
    ) -> rusqlite::Result<Self> {
        conn.execute_batch(&format!("SAVEPOINT {name}"))?;
        Ok(Self {
            conn,
            name,
            recovery,
            active: true,
        })
    }

    fn release(mut self) -> AppResult<()> {
        self.active = false;
        match self.conn.execute_batch(&format!("RELEASE {}", self.name)) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.recovery.fail_closed();
                Err(AppError::new(
                    "dbWriterRecovery",
                    format!(
                        "savepoint {} RELEASE 失败，当前数据库连接已故障关闭，请重启应用后重试: {error}",
                        self.name
                    ),
                ))
            }
        }
    }

    fn rollback(mut self) -> AppResult<()> {
        self.active = false;
        match self.conn.execute_batch(&format!(
            "ROLLBACK TO {name}; RELEASE {name}",
            name = self.name
        )) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.recovery.fail_closed();
                Err(AppError::new(
                    "dbWriterRecovery",
                    format!(
                        "savepoint {} ROLLBACK TO/RELEASE 失败，当前数据库连接已故障关闭，请重启应用后重试: {error}",
                        self.name
                    ),
                ))
            }
        }
    }
}

impl Drop for SavepointGuard<'_> {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        if let Err(error) = self.conn.execute_batch(&format!(
            "ROLLBACK TO {name}; RELEASE {name}",
            name = self.name
        )) {
            self.recovery.fail_closed();
            log::error!(
                "savepoint {} RAII rollback failed during unwinding: {error}",
                self.name
            );
        }
    }
}

fn preserve_business_error(mut original: AppError, recovery: AppError) -> AppError {
    let original_message = std::mem::take(&mut original.message);
    original.message = format!(
        "{}；内层 savepoint 回滚失败，数据库连接已故障关闭：{recovery}",
        original_message
    );
    original
}

/// 复合写的原子性守卫（R3 内容重指/条目创建的故障原子性）：把「新 contents 行
/// 写入 + 条目重指/回填 + 旧行回收」包成可嵌套的 SAVEPOINT——`f` 内任一语句
/// 失败即整体回滚，「新行已建但条目 UPDATE 失败」的孤儿窗口与「条目已插但
/// content_id 未回填」的半提交态在事务层不存在。
///
/// 为什么用 SAVEPOINT 而不是 `Connection::transaction`：本函数被 sync 入库
/// （staged/文件夹路径在调用方已有事务内以 `&Transaction` 调用）与命令层
/// （autocommit）共用；SAVEPOINT 在两种上下文中都可安全嵌套。autocommit 下
/// RELEASE 即提交，进程崩溃由 WAL 恢复丢弃未提交写。名称由调用方按站点给固定
/// 字面量（无外部输入进 SQL）。
pub(crate) fn with_write_savepoint<T>(
    conn: &Connection,
    name: &str,
    f: impl FnOnce() -> AppResult<T>,
) -> AppResult<T> {
    let writer = super::writer_guard(conn)?;
    let recovery = writer.recovery_token();
    // Standalone callers need a write reservation before the savepoint reads.
    // Nested callers already own their outer write transaction; preserve the
    // savepoint rollback boundary rather than opening a nested BEGIN.
    let outer = if conn.is_autocommit() {
        Some(super::write_transaction(conn)?)
    } else {
        None
    };
    let savepoint = SavepointGuard::begin(conn, name, recovery)?;
    match f() {
        Ok(value) => {
            savepoint.release()?;
            if let Some(outer) = outer {
                outer.commit()?;
            }
            Ok(value)
        }
        Err(err) => {
            if let Err(rb) = savepoint.rollback() {
                return Err(preserve_business_error(err, rb));
            }
            Err(err)
        }
    }
}

/// 找到或创建与输入等价的内容行（幂等）。
///
/// 查找顺序：身份命中 → 字节级复核通过则复用；复核失败（哈希碰撞）或未命中
/// → 以带盐身份重试/新建。空内容用条目私有身份（`private_id` 提供条目 id）。
/// 返回 `(content_id, revision, created)`。
pub fn ensure_content(
    conn: &Connection,
    input: &ContentInput,
    private_id: Option<i64>,
) -> AppResult<(i64, i64, bool)> {
    let _writer = super::writer_guard(conn)?;
    let base = if content_non_empty(input_html(input), &input.body_text) {
        compute_identity(
            &input.extraction_source,
            input_html(input),
            &input.body_text,
        )
    } else {
        match private_id {
            Some(id) => entry_private_identity(id),
            None => "e|orphan".to_string(),
        }
    };
    let mut candidate = base.clone();
    for _ in 0..16 {
        let existing: Option<(i64, i64, Option<String>, String, String)> = conn
            .query_row(
                "SELECT id, revision, content_html, body_text, extraction_source FROM contents WHERE identity = ?1",
                params![candidate],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                    ))
                },
            )
            .optional()?;
        match existing {
            Some((id, revision, html, body, source)) => {
                let same = html.as_deref().unwrap_or("") == input_html(input).unwrap_or("")
                    && body == input.body_text
                    && source == input.extraction_source;
                if same {
                    return Ok((id, revision, false));
                }
                // 哈希碰撞/同身份不同字节：加盐新身份，绝不误并。
                candidate = format!("{base}|c{}", candidate.len());
            }
            None => {
                let images = crate::sanitize::ordered_images(input_html(input).unwrap_or(""));
                let images_json = serde_json::to_string(&images)
                    .map_err(|error| crate::error::AppError::internal(error.to_string()))?;
                conn.execute(
                    "INSERT INTO contents(identity, url_norm, content_html, body_text, extraction_source, fulltext_extracted, images_json, image_count)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        candidate,
                        input.url_norm,
                        input.content_html,
                        input.body_text,
                        input.extraction_source,
                        input.fulltext_extracted as i64,
                        images_json,
                        images.len() as i64
                    ],
                )?;
                return Ok((conn.last_insert_rowid(), 1, true));
            }
        }
    }
    Err(crate::error::AppError::internal(format!(
        "内容身份冲突无法收敛（{base}）：拒绝写入以避免误并"
    )))
}

#[cfg(test)]
mod savepoint_recovery_tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    struct TestDb(PathBuf);

    impl TestDb {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "fluxreader-savepoint-recovery-{}-{}",
                std::process::id(),
                NEXT_DIR.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn open(&self) -> Connection {
            let conn = Connection::open(self.0.join("main.db")).unwrap();
            conn.execute_batch("CREATE TABLE writes(seq INTEGER PRIMARY KEY,actor TEXT NOT NULL);")
                .unwrap();
            conn
        }
    }

    impl Drop for TestDb {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).expect("all test connections must be closed");
        }
    }

    fn actors(conn: &Connection) -> Vec<String> {
        conn.prepare("SELECT actor FROM writes ORDER BY seq")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    fn assert_recovery_poisoned(conn: &Connection) {
        assert_eq!(
            super::super::writer::writer_guard(conn).err().unwrap().code,
            "dbWriterRecovery"
        );
    }

    #[test]
    fn rollback_to_failure_poisons_connection_and_outer_commit_rolls_back() {
        let dir = TestDb::new();
        let conn = dir.open();
        let tx = super::super::write_transaction(&conn).unwrap();
        tx.execute("INSERT INTO writes(actor) VALUES('outer')", [])
            .unwrap();
        let error = with_write_savepoint(&tx, "missing_savepoint", || -> AppResult<()> {
            tx.execute_batch(
                "RELEASE missing_savepoint; INSERT INTO writes(actor) VALUES('inner')",
            )?;
            Err(AppError::new("businessReplay", "rollback trigger"))
        })
        .unwrap_err();
        assert_eq!(error.code, "businessReplay");
        assert!(error.message.contains("rollback trigger"));
        assert!(error.message.contains("ROLLBACK TO/RELEASE"));
        assert_eq!(
            actors(&tx),
            ["outer", "inner"],
            "failed rollback is detected before the outer transaction can commit"
        );

        let commit_error = tx.commit().unwrap_err();
        assert_eq!(commit_error.code, "dbWriterRecovery");
        assert!(conn.is_autocommit());
        assert!(
            actors(&conn).is_empty(),
            "the poisoned outer transaction can never become durable"
        );
        assert_recovery_poisoned(&conn);
        super::super::writer::clear_recovery_for_test(&conn);
    }

    #[test]
    fn release_failure_poisons_connection_and_outer_commit_rolls_back() {
        let dir = TestDb::new();
        let conn = dir.open();
        let tx = super::super::write_transaction(&conn).unwrap();
        tx.execute("INSERT INTO writes(actor) VALUES('outer')", [])
            .unwrap();
        let error = with_write_savepoint(&tx, "consumed_elsewhere", || {
            tx.execute_batch(
                "INSERT INTO writes(actor) VALUES('inner');
                 RELEASE consumed_elsewhere",
            )?;
            Ok(())
        })
        .unwrap_err();
        assert_eq!(error.code, "dbWriterRecovery");
        assert!(error.message.contains("RELEASE"));
        assert_eq!(
            actors(&tx),
            ["outer", "inner"],
            "failed release is detected before the outer transaction can commit"
        );

        assert_eq!(tx.commit().unwrap_err().code, "dbWriterRecovery");
        assert!(conn.is_autocommit());
        assert!(actors(&conn).is_empty());
        assert_recovery_poisoned(&conn);
        super::super::writer::clear_recovery_for_test(&conn);
    }

    #[test]
    fn explicit_business_error_survives_rollback_failure_without_second_panic() {
        let dir = TestDb::new();
        let conn = dir.open();
        let tx = super::super::write_transaction(&conn).unwrap();
        let error = with_write_savepoint(&tx, "business_release", || -> AppResult<()> {
            tx.execute_batch("RELEASE business_release")?;
            tx.execute("INSERT INTO writes(actor) VALUES('inner')", [])?;
            Err(AppError::new("businessReplay", "original business failure"))
        })
        .unwrap_err();
        assert_eq!(error.code, "businessReplay");
        assert!(error.message.contains("original business failure"));
        assert!(error.message.contains("故障关闭"));

        assert_eq!(tx.commit().unwrap_err().code, "dbWriterRecovery");
        assert!(conn.is_autocommit());
        assert!(actors(&conn).is_empty());
        super::super::writer::clear_recovery_for_test(&conn);
    }

    #[test]
    fn panic_payload_survives_rollback_failure_and_connection_stays_closed() {
        let dir = TestDb::new();
        let conn = dir.open();
        let tx = super::super::write_transaction(&conn).unwrap();
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_write_savepoint(&tx, "panic_release", || -> AppResult<()> {
                tx.execute_batch("RELEASE panic_release")?;
                tx.execute("INSERT INTO writes(actor) VALUES('inner-panic')", [])?;
                panic!("original business panic");
            })
        }));
        let payload = panicked.unwrap_err();
        let original = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str));
        assert_eq!(original, Some("original business panic"));

        assert_eq!(tx.commit().unwrap_err().code, "dbWriterRecovery");
        assert!(conn.is_autocommit());
        assert!(actors(&conn).is_empty());
        assert_recovery_poisoned(&conn);
        super::super::writer::clear_recovery_for_test(&conn);
    }
}
