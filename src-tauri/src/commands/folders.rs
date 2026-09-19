//! commands 的 folders 领域子模块（TASK-044 从 commands.rs 按既有章节拆分，纯搬运）。

use super::{read_dedup_flag, sync_configured};
use crate::db;
use crate::error::{AppError, AppResult};
use crate::ingestion;
use crate::state::AppState;
use tauri::State;

/* ============================================================
Folders / Feeds
============================================================ */

#[tauri::command]
pub async fn list_folders(state: State<'_, AppState>) -> AppResult<Vec<db::FolderRow>> {
    let conn = state.db.lock().await;
    db::list_folders(&conn)
}

#[tauri::command]
pub async fn create_folder(
    state: State<'_, AppState>,
    name: String,
    layout: String,
) -> AppResult<i64> {
    let conn = state.db.lock().await;
    db::create_folder(&conn, &name, &layout)
}

/// 分类改名（命令与测试共用的真实逻辑）：本地改名 + 旧 label 墓碑（A-4）——
/// 否则下次 pull 按远端旧 label 重新 create_folder，留下重复空目录。
pub fn record_folder_rename(conn: &rusqlite::Connection, id: i64, new_name: &str) -> AppResult<()> {
    if let Some(old) = db::folder_name(conn, id)? {
        if old != new_name {
            db::add_folder_tombstone(conn, &old)?;
        }
    }
    db::rename_folder(conn, id, new_name)
}

/// 删除分类（命令与测试共用的真实逻辑）：为目录 label 及其内每个订阅写墓碑，
/// 再删目录（级联删订阅）。否则下次 pull 会把目录与订阅全部拉回（A-4）。
pub fn record_folder_delete(conn: &rusqlite::Connection, id: i64) -> AppResult<()> {
    if let Some(label) = db::folder_name(conn, id)? {
        db::add_folder_tombstone(conn, &label)?;
        for url in db::feed_urls_in_folder(conn, id)? {
            db::add_feed_tombstone(conn, &url)?;
        }
    }
    db::delete_folder(conn, id)
}

#[tauri::command]
pub async fn rename_folder(state: State<'_, AppState>, id: i64, name: String) -> AppResult<()> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::new("validate", "分类名称不能为空"));
    }
    // 本地改名 + 旧 label 墓碑（A-4）：远端分类是 label 名，不做远端改写，
    // 但必须阻挡 pull 按旧 label 复活空目录。
    let conn = state.db.lock().await;
    record_folder_rename(&conn, id, &name)
}

#[tauri::command]
pub async fn delete_folder(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    let conn = state.db.lock().await;
    record_folder_delete(&conn, id)
}

#[tauri::command]
pub async fn update_folder_layout(
    state: State<'_, AppState>,
    id: i64,
    layout: String,
) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::update_folder_layout(&conn, id, &layout)
}

#[tauri::command]
pub async fn set_folder_collapsed(
    state: State<'_, AppState>,
    id: i64,
    collapsed: bool,
) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::set_folder_collapsed(&conn, id, collapsed)
}

#[tauri::command]
pub async fn set_folder_ai_flags(
    state: State<'_, AppState>,
    id: i64,
    summary: bool,
    translate: bool,
) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::set_folder_ai_flags(&conn, id, summary, translate)
}

#[tauri::command]
pub async fn list_feeds(state: State<'_, AppState>) -> AppResult<Vec<db::FeedRow>> {
    let conn = state.db.lock().await;
    db::list_feeds(&conn)
}

/// 添加订阅源：先直连抓一次验证 URL 是有效 feed，成功才入库（不依赖 Miniflux）。
/// `folder_id = None`（UI 未选分类，如全新安装无任何分类时）→ 自动落到
/// 「未分类」文件夹（不存在则创建）——首次使用添加源不再报错。
/// 参数较多是 IPC 契约（前端 invoke 逐字段传），加 allow 避免 clippy 误报。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn add_feed(
    state: State<'_, AppState>,
    feed_url: String,
    title: Option<String>,
    folder_id: Option<i64>,
    layout: String,
    auto_summary: bool,
    auto_translate: bool,
    sync_to_backend: bool,
) -> AppResult<db::FeedRow> {
    // 1. 抓取验证（直连，第一优先级）
    let fetched = ingestion::conditional_get(&state.http, &feed_url, None, None).await?;
    let (bytes, etag, last_modified) = match fetched {
        ingestion::Fetched::NotModified => {
            return Err(AppError::new("parse", "unexpected 304 on first fetch"))
        }
        ingestion::Fetched::Body {
            bytes,
            etag,
            last_modified,
            ..
        } => (bytes, etag, last_modified),
    };
    let parsed = ingestion::parse_feed(&bytes, &feed_url)?;

    // 2. 入库（feed 元数据 + 全量条目，source='direct'）
    let conn = state.db.lock().await;
    persist_new_feed(
        &conn,
        &feed_url,
        &parsed,
        title.as_deref(),
        etag.as_deref(),
        last_modified.as_deref(),
        folder_id,
        &layout,
        auto_summary,
        auto_translate,
        sync_to_backend,
    )
}

/// add_feed 抓取验证后的入库段（TASK-064 抽出以便脱离 Tauri State 测试）。
/// 查重 → 标题兜底 → 「未分类」兜底 → 插入 → 清同 URL 删除墓碑（N4）→
/// 写抓取状态 → 建文章 → 按需入队推送。
#[allow(clippy::too_many_arguments)]
fn persist_new_feed(
    conn: &rusqlite::Connection,
    feed_url: &str,
    parsed: &ingestion::ParsedFeed,
    title: Option<&str>,
    etag: Option<&str>,
    last_modified: Option<&str>,
    folder_id: Option<i64>,
    layout: &str,
    auto_summary: bool,
    auto_translate: bool,
    sync_to_backend: bool,
) -> AppResult<db::FeedRow> {
    if db::find_feed_by_url(conn, feed_url)?.is_some() {
        return Err(AppError::new("duplicate", "该订阅地址已存在"));
    }
    let final_title = title
        .filter(|t| !t.trim().is_empty())
        .map(String::from)
        .or_else(|| parsed.title.clone())
        .unwrap_or_else(|| feed_url.to_string());
    // 未选分类 → 「未分类」文件夹（无则建）。创建失败必须上抛——
    // 兜底到 id=1 会在 folder 1 不存在时触发外键违约，文章静默丢失。
    let folder_id = match folder_id {
        Some(fid) => fid,
        None => db::ensure_uncategorized_folder(conn)?,
    };
    let feed_id = db::insert_feed(
        conn,
        feed_url,
        parsed.site_url.as_deref(),
        &final_title,
        parsed.icon.as_deref(),
        folder_id,
        layout,
        auto_summary,
        auto_translate,
    )?;
    // TASK-064 N4：重新添加 = 用户改变主意的最强证据——清掉同 URL 的删除
    // 墓碑。此前墓碑只在 pull 的「远端不再列出」分支清除，重新添加的源被永久
    // 压制：pull 跳过绑定、其未推送状态 30 天后被 prune_stale_unbound 物理删除。
    db::remove_feed_tombstone(conn, feed_url)?;
    db::set_feed_fetch_state(
        conn,
        feed_id,
        false,
        None,
        etag,
        last_modified,
    )?;
    let dedup = read_dedup_flag(conn);
    for a in &parsed.articles {
        db::upsert_article_with_feed(conn, feed_id, a, dedup)?;
    }
    // 勾选「同步到后端」且已连接 → 入队推送新订阅（feeds 阶段推远端）
    if sync_to_backend && sync_configured(conn) {
        let payload = serde_json::json!({ "folder_id": folder_id }).to_string();
        db::enqueue_sync(conn, None, Some(feed_url), "add_feed", Some(&payload))?;
    }
    let row = db::list_feeds(conn)?
        .into_iter()
        .find(|f| f.id == feed_id)
        .ok_or_else(|| AppError::internal("feed row vanished after insert"))?;
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn test_conn() -> rusqlite::Connection {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        db::MIGRATIONS.to_latest(&mut conn).unwrap();
        conn
    }

    fn minimal_parsed() -> ingestion::ParsedFeed {
        ingestion::parse_feed(
            br#"<?xml version="1.0"?><rss version="2.0"><channel><title>T</title>
<item><title>a</title><link>https://e.example/1</link><guid>g1</guid></item>
</channel></rss>"#,
            "https://f.example/rss",
        )
        .unwrap()
    }

    /// N4：重新添加清墓碑——先删源留墓碑，再 persist 同 URL，墓碑必须消失
    /// （否则 pull 永久跳过该源，其未推送状态 30 天后被老化物理删除）。
    #[test]
    fn republishing_a_url_clears_its_tombstone() {
        let conn = test_conn();
        db::add_feed_tombstone(&conn, "https://f.example/rss").unwrap();
        assert!(db::feed_tombstones(&conn).unwrap().contains(&"http://f.example/rss".to_string()));

        let parsed = minimal_parsed();
        let row = persist_new_feed(
            &conn,
            "https://f.example/rss",
            &parsed,
            None,
            None,
            None,
            None,
            "inherit",
            true,
            false,
            false,
        )
        .unwrap();
        assert_eq!(row.feed_url, "https://f.example/rss");
        assert!(
            !db::feed_tombstones(&conn).unwrap().contains(&"http://f.example/rss".to_string()),
            "重新添加后墓碑必须清除（N4：否则 pull 永久跳过该源）"
        );
    }

    /// 重复 URL 仍被拒绝（既有行为锚定，抽取不改变查重）
    #[test]
    fn duplicate_url_is_rejected() {
        let conn = test_conn();
        let parsed = minimal_parsed();
        persist_new_feed(
            &conn, "https://f.example/rss", &parsed, None, None, None, None, "inherit", true, false, false,
        )
        .unwrap();
        let err = persist_new_feed(
            &conn, "https://f.example/rss", &parsed, None, None, None, None, "inherit", true, false, false,
        )
        .unwrap_err();
        assert_eq!(err.code, "duplicate");
    }
}

/// 删除订阅的本地记录 + 删除墓碑（命令与测试共用的真实逻辑）。
/// 返回 Some((remote_id, feed_url))：该订阅已绑定远端且同步已配置，
/// 调用方应 best-effort 退订远端（GReader）；Fever 或未连接时仅靠墓碑防复活。
pub fn record_feed_deletion(
    conn: &rusqlite::Connection,
    id: i64,
) -> AppResult<Option<(i64, String)>> {
    let (feed_url, remote_id) = db::feed_remote_info(conn, id)?;
    // A-1：墓碑先落，退订失败也不得让 pull 把已删订阅拉回来
    db::add_feed_tombstone(conn, &feed_url)?;
    db::delete_feed(conn, id)?;
    Ok(if sync_configured(conn) {
        remote_id.map(|rid| (rid, feed_url))
    } else {
        None
    })
}

#[tauri::command]
pub async fn delete_feed(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    // 删除语义（SUB-4/SYN-1）：本地删除 ≠ 强删远端。
    // 不建 remove_feed 队项（该动作从未被 sync.rs 消费，只会累积僵尸队列）。
    let unsubscribe = {
        let conn = state.db.lock().await;
        record_feed_deletion(&conn, id)?
    };
    // 锁外 best-effort 退订远端。注意：**成功仅意味着请求被后端接受（2xx），不代表远端已删除**，
    // 故此处不清墓碑——墓碑由后续 pull 在「远端列表确认已不含该 URL」时收敛清除。
    // 若按请求成功就清墓碑，2xx 但未生效时会让已删订阅在下次 pull 复活（TASK-055 修复）。
    if let Some((remote_id, feed_url)) = unsubscribe {
        let _ = crate::sync::unsubscribe_remote(&state.db, &state.http, remote_id, &feed_url).await;
    }
    Ok(())
}

#[tauri::command]
pub async fn update_feed_layout(
    state: State<'_, AppState>,
    id: i64,
    layout: String,
) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::update_feed_layout(&conn, id, &layout)
}

/// 编辑源：标题/所属分类/布局/AI 开关一次性更新。
/// 连接 Miniflux 时改名与移动分类尽量同步到远端（best-effort，失败不阻塞本地落库结果）。
#[tauri::command]
pub async fn update_feed(
    state: State<'_, AppState>,
    id: i64,
    title: Option<String>,
    folder_id: Option<i64>,
    layout: Option<String>,
    auto_summary: Option<bool>,
    auto_translate: Option<bool>,
) -> AppResult<()> {
    let title = title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());

    let push = {
        let conn = state.db.lock().await;
        record_feed_edit(
            &conn,
            id,
            title.as_deref(),
            folder_id,
            layout.as_deref(),
            auto_summary,
            auto_translate,
        )?
    };
    // 锁外 best-effort 推送远端（GReader ac=edit；Fever no-op）：
    // 失败仅记日志，本地更新已生效，靠下次 pull 对账/用户重试收敛（A-2）
    if let Some((remote_id, new_title, dest_label)) = push {
        let _ = crate::sync::edit_remote_subscription(
            &state.db,
            &state.http,
            remote_id,
            new_title.as_deref(),
            dest_label.as_deref(),
        )
        .await;
    }
    Ok(())
}

/// 远端订阅编辑的推送目标：远端 id、新标题（None 表示不改标题）、目标分类名（None 表示未移动分类）。
pub type FeedEditPush = (i64, Option<String>, Option<String>);

/// 更新订阅并返回需推送远端的编辑目标（命令与测试共用的真实逻辑，A-2）。
/// 返回 Some((remote_id, 新标题, 目标分类名))：该订阅已绑定远端且同步已配置。
pub fn record_feed_edit(
    conn: &rusqlite::Connection,
    id: i64,
    title: Option<&str>,
    folder_id: Option<i64>,
    layout: Option<&str>,
    auto_summary: Option<bool>,
    auto_translate: Option<bool>,
) -> AppResult<Option<FeedEditPush>> {
    // 目标分类必须存在（防 UI 传错 id 把源挂飞）
    if let Some(fid) = folder_id {
        if !db::folder_exists(conn, fid)? {
            return Err(AppError::new("validate", "目标分类不存在"));
        }
    }
    db::update_feed(
        conn,
        id,
        title,
        folder_id,
        layout,
        auto_summary,
        auto_translate,
    )?;
    if !sync_configured(conn) {
        return Ok(None);
    }
    let (_feed_url, remote_id) = db::feed_remote_info(conn, id)?;
    let Some(rid) = remote_id else {
        return Ok(None);
    };
    let dest_label = match folder_id {
        Some(fid) => db::folder_name(conn, fid)?,
        None => None,
    };
    Ok(Some((rid, title.map(|t| t.to_string()), dest_label)))
}

#[tauri::command]
pub async fn set_feed_ai_flags(
    state: State<'_, AppState>,
    id: i64,
    summary: bool,
    translate: bool,
) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::set_feed_ai_flags(&conn, id, summary, translate)
}
