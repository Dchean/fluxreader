//! commands 的 opml 领域子模块（TASK-044 从 commands.rs 按既有章节拆分，纯搬运）。

use crate::db;
use crate::error::AppResult;
use crate::state::AppState;
use tauri::State;

/* ============================================================
OPML 导入导出
============================================================ */

/// 导入 OPML：按目录建 folder → 插入 feed（已存在的 URL 跳过）→ 入同步队列。
/// 返回 (新增源数, 跳过数)。
#[derive(serde::Serialize)]
pub struct OpmlImportReport {
    pub imported: usize,
    pub skipped: usize,
}

#[tauri::command]
pub async fn opml_import(
    state: State<'_, AppState>,
    content: String,
) -> AppResult<OpmlImportReport> {
    let feeds = crate::opml::parse(&content)?;
    let mut report = OpmlImportReport {
        imported: 0,
        skipped: 0,
    };
    let conn = state.db.lock().await;

    // 目录名 → folder_id 缓存（一次导入内同名目录只建一次）
    let mut folder_ids: std::collections::HashMap<String, i64> = std::collections::HashMap::new();

    for f in &feeds {
        // 已存在（URL 碰撞）→ 跳过
        if db::feed_exists_by_url(&conn, &f.feed_url)? {
            report.skipped += 1;
            continue;
        }
        let folder_id = match f.folder.as_deref() {
            Some(name) => match folder_ids.get(name) {
                Some(id) => *id,
                None => {
                    let id = db::create_folder(&conn, name, "article")?;
                    folder_ids.insert(name.to_string(), id);
                    id
                }
            },
            None => db::create_folder(&conn, "导入", "article")?,
        };
        db::insert_feed(
            &conn,
            &f.feed_url,
            None,
            &f.title,
            None,
            folder_id,
            "inherit",
            true,
            false,
        )?;
        // 新增订阅入同步队列（连接 Miniflux 后补推）。payload 必须是含 folder_id
        // 的 JSON——push_feeds 据此把订阅挂到远端对应分类；此前误传标题字符串，
        // serde_json 解析失败导致 payload 丢弃、源被推到远端默认分类（目录丢失）。
        let payload = serde_json::json!({ "folder_id": folder_id }).to_string();
        db::enqueue_sync(&conn, None, Some(&f.feed_url), "add_feed", Some(&payload))?;
        report.imported += 1;
    }
    Ok(report)
}

/// 导出 OPML：全部源 + 目录名 → OPML 文档字符串。
#[tauri::command]
pub async fn opml_export(state: State<'_, AppState>) -> AppResult<String> {
    let rows = {
        let conn = state.db.lock().await;
        db::export_feeds_with_folders(&conn)?
    };
    crate::opml::build(&rows)
}
