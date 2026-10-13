//! contents：单一正文所有权（OPT-008B 第二阶段）。
//!
//! 设计（契约见 .agents/notes/proposed/architecture/2026-10-12-订阅条目与共享内容身份.md
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
//! - 回收：条目删除（含源/分类/账号级联）由迁移创建的 `trg_articles_fts_ad`
//!   触发器在**最后引用**消失时删除 contents 行；条目重指新内容时调用方在同一
//!   语句末尾回收无引用的旧行（见 articles/sync_map 的重指路径）。
//! - 原子性（R3）：`ensure_content` 的 INSERT 与随后的条目重指/回填是复合写，
//!   调用方经 [`with_write_savepoint`] 包住整组语句——任一步失败整体回滚，
//!   不会留下「已 INSERT 的 contents 行无任何引用」（孤儿）或「条目 content_id
//!   未回填」的半提交态。SAVEPOINT 可安全嵌套于调用方既有事务内。
//! - FTS 是**可重建派生索引**（独立 fts5 表，由迁移创建的触发器随条目/内容变化
//!   维护），本模块提供全量重建入口 `rebuild_fts`（迁移与维护路径消费）。

use md5::{Digest, Md5};
use rusqlite::{params, Connection, OptionalExtension};

use crate::error::AppResult;

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
    conn.execute_batch(&format!("SAVEPOINT {name}"))?;
    match f() {
        Ok(value) => {
            conn.execute_batch(&format!("RELEASE {name}"))?;
            Ok(value)
        }
        Err(err) => {
            if let Err(rb) = conn.execute_batch(&format!("ROLLBACK TO {name}; RELEASE {name}")) {
                return Err(crate::error::AppError::internal(format!(
                    "savepoint {name} 回滚失败（原始错误：{err}；回滚错误：{rb}）"
                )));
            }
            Err(err)
        }
    }
}

/// 受保护提取行（`readability` + `fulltext_extracted`）的正文错配修复：
/// body_text 与保留 HTML 不同源（历史只写 HTML+flag 的旧形态，或 body 缺失）
/// 时按 HTML 重算修复并 `revision+1`（前端修订失效可感知）。
/// **不重指、不改来源/提取标志**——RSS 刷新与远端回填不得覆盖既定提取
/// （OPT-003 R1 语义在 contents 上的延续）。返回是否发生修复。
pub(crate) fn repair_extracted_body_if_mismatch(
    conn: &Connection,
    content_id: i64,
) -> AppResult<bool> {
    let row: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT body_text, content_html FROM contents WHERE id = ?1",
            params![content_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((body, html)) = row else {
        return Ok(false);
    };
    let Some(html) = html.filter(|h| !h.trim().is_empty()) else {
        return Ok(false);
    };
    let same_source = crate::sanitize::html_to_text(&html);
    if body == same_source {
        return Ok(false);
    }
    conn.execute(
        "UPDATE contents SET body_text = ?1, revision = revision + 1, updated_at = datetime('now') WHERE id = ?2",
        params![same_source, content_id],
    )?;
    Ok(true)
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
                conn.execute(
                    "INSERT INTO contents(identity, url_norm, content_html, body_text, extraction_source, fulltext_extracted)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        candidate,
                        input.url_norm,
                        input.content_html,
                        input.body_text,
                        input.extraction_source,
                        input.fulltext_extracted as i64
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

/// FTS 全量重建（派生索引，可从 articles×contents 无损重算）。
pub fn rebuild_fts(conn: &Connection) -> AppResult<usize> {
    conn.execute("DELETE FROM articles_fts", [])?;
    let n = conn.execute(
        "INSERT INTO articles_fts(rowid, title, body_text, author, ai_summary, translated_content)
         SELECT a.id, a.title, COALESCE(c.body_text, ''), COALESCE(a.author, ''),
                COALESCE(a.ai_summary, ''), COALESCE(a.translated_content, '')
         FROM articles a LEFT JOIN contents c ON c.id = a.content_id",
        [],
    )?;
    Ok(n)
}
