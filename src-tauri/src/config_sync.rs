//! 配置同步（GitHub Gist / WebDAV）：手动上传/下载客户端配置。
//! 同步范围：分类（名称/布局/AI标志）+ 订阅源（URL/标题/归属/布局/AI标志）
//! + app_settings + ai_config。不含正文/媒体/AI 缓存（按设计文档边界）。
//!
//! 不做冲突合并：下载整体覆盖设置，源/分类按 URL/名称 upsert。

use crate::db;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::State;
use tokio::sync::Mutex;

const SCHEMA_VERSION: u32 = 1;
const FILE_NAME: &str = "fluxreader-config.json";

/* ============================================================
   同步 payload
   ============================================================ */

#[derive(Serialize, Deserialize, Debug)]
pub struct SyncPayload {
    pub schema: u32,
    pub uploaded_at: String,
    pub folders: Vec<FolderSpec>,
    pub feeds: Vec<FeedSpec>,
    pub app_settings: Option<String>,
    pub ai_config: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct FolderSpec {
    pub name: String,
    pub layout: String,
    pub auto_summary: bool,
    pub auto_translate: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct FeedSpec {
    pub url: String,
    pub title: String,
    pub folder: String,
    pub layout: String,
    pub auto_summary: bool,
    pub auto_translate: bool,
}

/// 从本地库构建上传 payload。
pub fn build_payload(conn: &rusqlite::Connection) -> AppResult<SyncPayload> {
    let folders = db::list_folders(conn)?;
    let feeds = db::list_feeds(conn)?;
    let folder_names: std::collections::HashMap<i64, String> =
        folders.iter().map(|f| (f.id, f.name.clone())).collect();

    let folder_specs = folders
        .iter()
        .map(|f| FolderSpec {
            name: f.name.clone(),
            layout: f.layout.clone(),
            auto_summary: f.auto_summary,
            auto_translate: f.auto_translate,
        })
        .collect();

    let feed_specs = feeds
        .iter()
        .map(|f| FeedSpec {
            url: f.feed_url.clone(),
            title: f.title.clone(),
            folder: folder_names.get(&f.folder_id).cloned().unwrap_or_default(),
            layout: f.layout.clone(),
            auto_summary: f.auto_summary,
            auto_translate: f.auto_translate,
        })
        .collect();

    Ok(SyncPayload {
        schema: SCHEMA_VERSION,
        uploaded_at: chrono::Utc::now().to_rfc3339(),
        folders: folder_specs,
        feeds: feed_specs,
        app_settings: db::get_setting(conn, "app_settings")?,
        ai_config: db::get_setting(conn, "ai_config")?,
    })
}

/// 应用下载 payload 到本地库：分类/源 upsert（按名称/URL 匹配，已存在跳过），
/// 设置整体覆盖。返回 (新增源数, 跳过数)。
pub fn apply_payload(conn: &rusqlite::Connection, p: &SyncPayload) -> AppResult<(usize, usize)> {
    if p.schema > SCHEMA_VERSION {
        return Err(AppError::internal(format!(
            "远端配置版本 v{} 高于本客户端支持的 v{}，请升级客户端",
            p.schema, SCHEMA_VERSION
        )));
    }

    // 分类按名称 upsert（已存在则更新布局/AI 标志）
    let mut folder_ids: std::collections::HashMap<String, i64> = Default::default();
    for f in &p.folders {
        let existing = list_folder_id_by_name(conn, &f.name)?;
        let id = match existing {
            Some(id) => {
                let _ = db::update_folder_layout(conn, id, &f.layout);
                let _ = db::set_folder_ai_flags(conn, id, f.auto_summary, f.auto_translate);
                id
            }
            None => db::create_folder(conn, &f.name, &f.layout)?,
        };
        folder_ids.insert(f.name.clone(), id);
    }

    // 源按 URL upsert；没有分类的落默认分类（建一个「导入」）
    let mut imported = 0usize;
    let mut skipped = 0usize;
    for f in &p.feeds {
        if db::find_feed_by_url(conn, &f.url)?.is_some() {
            skipped += 1;
            continue;
        }
        let folder_id = match folder_ids.get(&f.folder) {
            Some(id) => *id,
            None => {
                let id = list_folder_id_by_name(conn, "导入")?
                    .unwrap_or_else(|| db::create_folder(conn, "导入", "article").unwrap_or(0));
                folder_ids.insert("导入".to_string(), id);
                id
            }
        };
        let title = if f.title.trim().is_empty() { f.url.clone() } else { f.title.clone() };
        db::insert_feed(conn, &f.url, None, &title, None, folder_id, &f.layout, f.auto_summary, f.auto_translate)?;
        imported += 1;
    }

    // 设置整体覆盖（app_settings + ai_config）
    if let Some(s) = &p.app_settings {
        db::set_setting(conn, "app_settings", s)?;
    }
    if let Some(s) = &p.ai_config {
        db::set_setting(conn, "ai_config", s)?;
    }

    Ok((imported, skipped))
}

fn list_folder_id_by_name(conn: &rusqlite::Connection, name: &str) -> AppResult<Option<i64>> {
    let mut stmt = conn.prepare("SELECT id FROM folders WHERE name = ?1")?;
    let mut rows = stmt.query_map(rusqlite::params![name], |r| r.get(0))?;
    Ok(rows.next().transpose()?)
}

/* ============================================================
   远端后端：GitHub Gist / WebDAV
   ============================================================ */

/// 凭据（settings 键 `config_sync_credentials`）：
/// Gist 需 classic PAT（gist scope）；WebDAV 用服务器地址+账号密码。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SyncCredentials {
    pub backend: String, // "gist" | "webdav"
    pub token: String,   // gist: PAT；webdav: 密码
    pub server: String,  // webdav: 服务器根 URL
    pub username: String,
    pub gist_id: Option<String>,
}

async fn read_credentials(conn: &Arc<Mutex<rusqlite::Connection>>) -> AppResult<SyncCredentials> {
    let c = conn.lock().await;
    let raw = db::get_setting(&c, "config_sync_credentials")?
        .ok_or_else(|| AppError::not_found("未配置配置同步凭据"))?;
    Ok(serde_json::from_str(&raw)?)
}

/// Gist：创建（首次）或更新（已有 id）secret gist，内容为 payload JSON。
async fn gist_upsert(http: &reqwest::Client, cred: &SyncCredentials, json: &str) -> AppResult<String> {
    let auth = format!("Bearer {}", cred.token);
    match &cred.gist_id {
        Some(id) => {
            let url = format!("https://api.github.com/gists/{id}");
            let body = serde_json::json!({ "files": { FILE_NAME: { "content": json } } });
            let resp = http.patch(&url)
                .header("Authorization", &auth)
                .header("User-Agent", "FluxReader")
                .json(&body)
                .timeout(std::time::Duration::from_secs(30))
                .send().await?;
            if !resp.status().is_success() {
                return Err(AppError::network(format!("Gist 更新失败：HTTP {}", resp.status())));
            }
            Ok(id.clone())
        }
        None => {
            let body = serde_json::json!({
                "description": "FluxReader 配置同步（勿删）",
                "secret": true,
                "files": { FILE_NAME: { "content": json } }
            });
            let resp = http.post("https://api.github.com/gists")
                .header("Authorization", &auth)
                .header("User-Agent", "FluxReader")
                .json(&body)
                .timeout(std::time::Duration::from_secs(30))
                .send().await?;
            let status = resp.status();
            let text = resp.text().await?;
            if !status.is_success() {
                /* 字符级截断：字节位置 200 可能落在多字节字符中间（panic） */
                let head: String = text.chars().take(200).collect();
                return Err(AppError::network(format!("Gist 创建失败：HTTP {status}：{head}")));
            }
            let v: serde_json::Value = serde_json::from_str(&text)?;
            v.get("id")
                .and_then(|i| i.as_str())
                .map(String::from)
                .ok_or_else(|| AppError::network("Gist 响应缺少 id"))
        }
    }
}

/// Gist：读取 payload JSON。
async fn gist_read(http: &reqwest::Client, cred: &SyncCredentials) -> AppResult<String> {
    let id = cred.gist_id.as_ref()
        .ok_or_else(|| AppError::not_found("尚未上传过配置（无 Gist id）"))?;
    let url = format!("https://api.github.com/gists/{id}");
    let resp = http.get(&url)
        .header("Authorization", format!("Bearer {}", cred.token))
        .header("User-Agent", "FluxReader")
        .timeout(std::time::Duration::from_secs(30))
        .send().await?;
    let status = resp.status();
    let text = resp.text().await?;
    if !status.is_success() {
        return Err(AppError::network(format!("Gist 读取失败：HTTP {status}")));
    }
    let v: serde_json::Value = serde_json::from_str(&text)?;
    v.pointer(&format!("/files/{FILE_NAME}/content"))
        .and_then(|c| c.as_str())
        .map(String::from)
        .ok_or_else(|| AppError::not_found("Gist 中没有配置文件"))
}

/// WebDAV：PUT 写入。
pub async fn webdav_put(http: &reqwest::Client, cred: &SyncCredentials, json: &str) -> AppResult<()> {
    let url = format!("{}/{}", cred.server.trim_end_matches('/'), FILE_NAME);
    let resp = http.put(&url)
        .basic_auth(&cred.username, Some(&cred.token))
        .header("Content-Type", "application/json")
        .body(json.to_string())
        .timeout(std::time::Duration::from_secs(30))
        .send().await?;
    if !resp.status().is_success() {
        return Err(AppError::network(format!("WebDAV 上传失败：HTTP {}", resp.status())));
    }
    Ok(())
}

/// WebDAV：GET 读取。
pub async fn webdav_get(http: &reqwest::Client, cred: &SyncCredentials) -> AppResult<String> {
    let url = format!("{}/{}", cred.server.trim_end_matches('/'), FILE_NAME);
    let resp = http.get(&url)
        .basic_auth(&cred.username, Some(&cred.token))
        .timeout(std::time::Duration::from_secs(30))
        .send().await?;
    if !resp.status().is_success() {
        return Err(AppError::network(format!("WebDAV 读取失败：HTTP {}", resp.status())));
    }
    Ok(resp.text().await?)
}

/* ============================================================
   IPC 命令
   ============================================================ */

/// 保存凭据（前端设置页表单）。
#[tauri::command]
pub async fn config_sync_save_credentials(
    state: State<'_, AppState>,
    credentials: String,
) -> AppResult<()> {
    // 先校验 JSON 形状
    let cred: SyncCredentials = serde_json::from_str(&credentials)?;
    let conn = state.db.lock().await;
    db::set_setting(&conn, "config_sync_credentials", &serde_json::to_string(&cred)?)?;
    Ok(())
}

/// 上传：本地库 → 远端。
#[tauri::command]
pub async fn config_sync_upload(state: State<'_, AppState>) -> AppResult<String> {
    let (json, mut cred) = {
        let conn = state.db.lock().await;
        (serde_json::to_string(&build_payload(&conn)?)?, read_credentials(&state.db).await?)
    };
    let http = &state.http;
    match cred.backend.as_str() {
        "gist" => {
            let id = gist_upsert(http, &cred, &json).await?;
            if cred.gist_id.as_deref() != Some(&id) {
                cred.gist_id = Some(id);
                let conn = state.db.lock().await;
                db::set_setting(&conn, "config_sync_credentials", &serde_json::to_string(&cred)?)?;
            }
        }
        "webdav" => webdav_put(http, &cred, &json).await?,
        other => return Err(AppError::internal(format!("未知同步后端：{other}"))),
    }
    let now = chrono::Utc::now().to_rfc3339();
    {
        let conn = state.db.lock().await;
        db::set_setting(&conn, "config_sync_last_upload", &now)?;
    }
    Ok(now)
}

/// 下载：远端 → 本地库（不自动应用；返回 payload 供前端确认）。
#[tauri::command]
pub async fn config_sync_download(state: State<'_, AppState>) -> AppResult<String> {
    let cred = read_credentials(&state.db).await?;
    let http = &state.http;
    let json = match cred.backend.as_str() {
        "gist" => gist_read(http, &cred).await?,
        "webdav" => webdav_get(http, &cred).await?,
        other => return Err(AppError::internal(format!("未知同步后端：{other}"))),
    };
    // 校验是合法 payload（应用前先让前端确认）
    let _p: SyncPayload = serde_json::from_str(&json)?;
    Ok(json)
}

/// 应用下载的 payload（前端确认后调用）。
#[tauri::command]
pub async fn config_sync_apply(state: State<'_, AppState>, payload: String) -> AppResult<serde_json::Value> {
    let p: SyncPayload = serde_json::from_str(&payload)?;
    let conn = state.db.lock().await;
    let (imported, skipped) = apply_payload(&conn, &p)?;
    Ok(serde_json::json!({ "imported": imported, "skipped": skipped }))
}

/// 状态：远端配置时间戳 vs 本地上次同步时间。
#[tauri::command]
pub async fn config_sync_status(state: State<'_, AppState>) -> AppResult<serde_json::Value> {
    let (cred, last_local) = {
        let conn = state.db.lock().await;
        (
            db::get_setting(&conn, "config_sync_credentials")?,
            db::get_setting(&conn, "config_sync_last_upload")?,
        )
    };
    let cred: Option<SyncCredentials> = cred.and_then(|s| serde_json::from_str(&s).ok());
    Ok(serde_json::json!({
        "configured": cred.is_some(),
        "backend": cred.as_ref().map(|c| c.backend.clone()),
        "lastUpload": last_local,
    }))
}

/* ============================================================
   集成测试入口（tests/config_sync_e2e.rs）
   ============================================================ */

#[doc(hidden)]
pub async fn webdav_put_for_test(http: &reqwest::Client, cred: &SyncCredentials, json: &str) -> AppResult<()> {
    webdav_put(http, cred, json).await
}

#[doc(hidden)]
pub async fn webdav_get_for_test(http: &reqwest::Client, cred: &SyncCredentials) -> AppResult<String> {
    webdav_get(http, cred).await
}
