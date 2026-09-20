use super::*;
use rusqlite::params;
use serde::Serialize;

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
        .query_row(
            "SELECT id FROM feeds WHERE feed_url = ?1",
            params![url],
            |r| r.get(0),
        )
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
    insert_feed_origin(
        conn,
        feed_url,
        site_url,
        title,
        favicon_url,
        folder_id,
        layout,
        auto_summary,
        auto_translate,
        "local",
    )
}

/// 同 insert_feed，带来源标记（'local' 用户直连添加 | 'remote' 服务端拉取）。
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
/// 到期源 id。`include_remote = false`（跟随服务端同步模式）时跳过
/// origin='remote' 的源——服务端源的内容由 Miniflux 同步提供，
/// 直连抓取会与服务端状态产生两份不一致的真相。
pub fn feeds_due_for_refresh(
    conn: &Connection,
    interval_min: i64,
    include_remote: bool,
) -> AppResult<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM feeds
         WHERE (last_fetched_at IS NULL
            OR ( (julianday('now') - julianday(last_fetched_at)) * 1440.0 >= ?1
                 AND (next_retry_at IS NULL OR julianday('now') >= julianday(next_retry_at)) ))
           AND (?2 OR origin != 'remote')",
    )?;
    let rows = stmt.query_map(params![interval_min, include_remote], |r| r.get(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 全部源 id（托盘「刷新全部订阅」与手动全刷入口，忽略到期与退避）。
/// 手动入口始终包含 Miniflux 源（用户显式动作 = 要全部内容）。
pub fn feeds_all_ids(conn: &Connection, include_remote: bool) -> AppResult<Vec<i64>> {
    let mut stmt = conn.prepare("SELECT id FROM feeds WHERE (?1 OR origin != 'remote')")?;
    let rows = stmt.query_map(params![include_remote], |r| r.get(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn set_feed_title_and_icon(
    conn: &Connection,
    id: i64,
    title: Option<&str>,
    favicon: Option<&str>,
    site_url: Option<&str>,
) -> AppResult<()> {
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
    conn.execute(
        "UPDATE feeds SET layout = ?1 WHERE id = ?2",
        params![layout, id],
    )?;
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

pub fn set_feed_ai_flags(
    conn: &Connection,
    id: i64,
    summary: bool,
    translate: bool,
) -> AppResult<()> {
    conn.execute(
        "UPDATE feeds SET auto_summary = ?1, auto_translate = ?2 WHERE id = ?3",
        params![summary as i64, translate as i64, id],
    )?;
    Ok(())
}

/* ============================================================
Articles
============================================================ */

/* ============================================================
订阅删除墓碑（A-1）：本地删除后 pull 不得按远端列表复活。
存 app_settings 的 JSON 数组（规范化 URL），避免为一次删除语义新增迁移；
退订成功（远端确认）后清除。
============================================================ */
const FEED_TOMBSTONE_KEY: &str = "feed_tombstones";

/// 删除订阅的墓碑 URL 列表（规范化）。
pub fn feed_tombstones(conn: &Connection) -> AppResult<Vec<String>> {
    let raw = super::get_setting(conn, FEED_TOMBSTONE_KEY)?;
    Ok(raw
        .as_deref()
        .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
        .unwrap_or_default())
}

fn save_feed_tombstones(conn: &Connection, list: &[String]) -> AppResult<()> {
    let raw = serde_json::to_string(list).unwrap_or_else(|_| "[]".into());
    super::set_setting(conn, FEED_TOMBSTONE_KEY, &raw)
}

/// 写入删除墓碑（幂等，按规范化 URL 去重）。
pub fn add_feed_tombstone(conn: &Connection, feed_url: &str) -> AppResult<()> {
    let norm = super::normalize_url(feed_url);
    let mut list = feed_tombstones(conn)?;
    if !list.iter().any(|u| u == &norm) {
        list.push(norm);
        save_feed_tombstones(conn, &list)?;
    }
    Ok(())
}

/// 清除删除墓碑（远端已确认不再订阅后调用）。
pub fn remove_feed_tombstone(conn: &Connection, feed_url: &str) -> AppResult<()> {
    let norm = super::normalize_url(feed_url);
    let mut list = feed_tombstones(conn)?;
    let before = list.len();
    list.retain(|u| u != &norm);
    if list.len() != before {
        save_feed_tombstones(conn, &list)?;
    }
    Ok(())
}

/// 订阅的同步信息：URL 与远端绑定 id（删除订阅时用于退订与墓碑）。
pub fn feed_remote_info(conn: &Connection, id: i64) -> AppResult<(String, Option<i64>)> {
    conn.query_row(
        "SELECT feed_url, remote_id FROM feeds WHERE id = ?1",
        [id],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?)),
    )
    .map_err(Into::into)
}
