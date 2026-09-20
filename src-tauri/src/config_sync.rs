//! 配置同步（GitHub Gist / WebDAV）：手动上传/下载客户端配置。
//! 同步范围（白名单原则）：分类（名称/布局/AI标志/位置）+ 订阅源（URL/标题/归属/
//! 布局/AI标志/site_url/favicon_url）+ app_settings（排除 autoStart/closePromptShown）
//! + 非敏感连接配置（sync_protocol/greader_endpoint/greader_username）。
//!
//! 凭据字段排除：greader_password, ai_config, config_sync_credentials, miniflux_token。
//! 不含正文/媒体/AI 缓存（按设计文档边界 DEC-008/009）。
//! 字段清单：.agents/notes/implemented/feature/2026-09-13-opt004-config-sync-field-inventory.md
//!
//! 不做冲突合并：下载应用仅更新白名单字段，本地凭据与本地特定设置保持不变。

use crate::db;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::State;
use tokio::sync::Mutex;

const SCHEMA_VERSION: u32 = 1;
const CONFIG_FILE_NAME: &str = "fluxreader-config.json";

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
    /// 非敏感连接配置（协议、服务器地址、用户名）
    pub connection_config: Option<ConnectionConfig>,
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct ConnectionConfig {
    pub sync_protocol: Option<String>,
    pub greader_endpoint: Option<String>,
    pub greader_username: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct FolderSpec {
    pub name: String,
    pub layout: String,
    pub auto_summary: bool,
    pub auto_translate: bool,
    pub position: i64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct FeedSpec {
    pub url: String,
    pub title: String,
    pub folder: String,
    pub layout: String,
    pub auto_summary: bool,
    pub auto_translate: bool,
    pub site_url: Option<String>,
    pub favicon_url: Option<String>,
}

/// 从本地库构建上传 payload（白名单原则：仅同步字段清单中 sync=允许 的字段）。
/// 排除全部凭据字段：greader_password, ai_config, config_sync_credentials, miniflux_token。
/// 参考：.agents/notes/implemented/feature/2026-09-13-opt004-config-sync-field-inventory.md
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
            position: f.position,
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
            site_url: f.site_url.clone(),
            favicon_url: f.favicon_url.clone(),
        })
        .collect();

    // 非敏感连接配置（白名单）
    let connection_config = ConnectionConfig {
        sync_protocol: db::get_setting(conn, "sync_protocol")?,
        greader_endpoint: db::get_setting(conn, "greader_endpoint")?,
        greader_username: db::get_setting(conn, "greader_username")?,
    };

    // app_settings 过滤本地特定字段（autoStart, closePromptShown）
    let app_settings = filter_app_settings(db::get_setting(conn, "app_settings")?)?;

    Ok(SyncPayload {
        schema: SCHEMA_VERSION,
        uploaded_at: chrono::Utc::now().to_rfc3339(),
        folders: folder_specs,
        feeds: feed_specs,
        app_settings,
        connection_config: Some(connection_config),
    })
}

/// 过滤 app_settings JSON，移除本地特定字段。
fn filter_app_settings(raw: Option<String>) -> AppResult<Option<String>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let mut settings: serde_json::Value = serde_json::from_str(&raw)?;
    if let Some(obj) = settings.as_object_mut() {
        // 排除本地特定字段（按字段清单）
        obj.remove("autoStart");
        obj.remove("closePromptShown");
    }
    Ok(Some(serde_json::to_string(&settings)?))
}

/// 应用下载 payload 到本地库（白名单原则：仅应用允许字段，保留本地凭据）。
/// 分类/源 upsert（按名称/URL 匹配，已存在跳过），设置字段级覆盖（不整体替换）。
/// 返回 (新增源数, 跳过数)。
/// 参考：.agents/notes/implemented/feature/2026-09-13-opt004-config-sync-field-inventory.md
pub fn apply_payload(conn: &rusqlite::Connection, p: &SyncPayload) -> AppResult<(usize, usize)> {
    if p.schema > SCHEMA_VERSION {
        return Err(AppError::internal(format!(
            "远端配置版本 v{} 高于本客户端支持的 v{}，请升级客户端",
            p.schema, SCHEMA_VERSION
        )));
    }

    // TASK-064 N6：全程单事务——中途失败（如某步 SQL 约束违约）全量回滚，
    // 不留「半套已应用配置」（此前已建的 folders / 已插的 feeds 会残留，
    // 用户重试得到叠加结果）。Err 路径 Transaction drop 自动回滚。
    let tx = conn.unchecked_transaction()?;

    // 分类按名称 upsert（已存在则更新布局/AI 标志/位置）
    let mut folder_ids: std::collections::HashMap<String, i64> = Default::default();
    for f in &p.folders {
        let existing = list_folder_id_by_name(&tx, &f.name)?;
        let id = match existing {
            Some(id) => {
                let _ = db::update_folder_layout(&tx, id, &f.layout);
                let _ = db::set_folder_ai_flags(&tx, id, f.auto_summary, f.auto_translate);
                // 更新位置
                tx.execute(
                    "UPDATE folders SET position = ?1 WHERE id = ?2",
                    rusqlite::params![f.position, id],
                )?;
                id
            }
            None => {
                let id = db::create_folder(&tx, &f.name, &f.layout)?;
                let _ = db::set_folder_ai_flags(&tx, id, f.auto_summary, f.auto_translate);
                tx.execute(
                    "UPDATE folders SET position = ?1 WHERE id = ?2",
                    rusqlite::params![f.position, id],
                )?;
                id
            }
        };
        folder_ids.insert(f.name.clone(), id);
    }

    // 源按 URL upsert；没有分类的落默认分类（建一个「导入」）
    let mut imported = 0usize;
    let mut skipped = 0usize;
    for f in &p.feeds {
        match db::find_feed_by_url(&tx, &f.url)? {
            Some(existing_id) => {
                // 已存在：更新白名单字段（title, folder, layout, AI flags, site_url, favicon_url）
                let folder_id = match folder_ids.get(&f.folder) {
                    Some(id) => *id,
                    None => {
                        // N6：兜底目录创建失败必须上抛（此前 unwrap_or(0) 产生
                        // folder_id=0 → insert_feed 外键违约且半套配置残留）
                        let id = match list_folder_id_by_name(&tx, "导入")? {
                            Some(id) => id,
                            None => db::create_folder(&tx, "导入", "article")?,
                        };
                        folder_ids.insert("导入".to_string(), id);
                        id
                    }
                };
                tx.execute(
                    "UPDATE feeds SET title = ?1, folder_id = ?2, layout = ?3, auto_summary = ?4, auto_translate = ?5, site_url = ?6, favicon_url = ?7 WHERE id = ?8",
                    rusqlite::params![
                        f.title,
                        folder_id,
                        f.layout,
                        f.auto_summary,
                        f.auto_translate,
                        f.site_url,
                        f.favicon_url,
                        existing_id
                    ],
                )?;
                skipped += 1;
            }
            None => {
                // 新源导入
                let folder_id = match folder_ids.get(&f.folder) {
                    Some(id) => *id,
                    None => {
                        // N6：同上，失败上抛进事务回滚
                        let id = match list_folder_id_by_name(&tx, "导入")? {
                            Some(id) => id,
                            None => db::create_folder(&tx, "导入", "article")?,
                        };
                        folder_ids.insert("导入".to_string(), id);
                        id
                    }
                };
                let title = if f.title.trim().is_empty() {
                    f.url.clone()
                } else {
                    f.title.clone()
                };
                db::insert_feed(
                    &tx,
                    &f.url,
                    f.site_url.as_deref(),
                    &title,
                    f.favicon_url.as_deref(),
                    folder_id,
                    &f.layout,
                    f.auto_summary,
                    f.auto_translate,
                )?;
                imported += 1;
            }
        }
    }

    // app_settings 字段级合并：只更新白名单字段，保留本地特定字段
    if let Some(remote_settings) = &p.app_settings {
        merge_app_settings(&tx, remote_settings)?;
    }

    // 非敏感连接配置应用（保留凭据字段不变）
    if let Some(cc) = &p.connection_config {
        if let Some(v) = &cc.sync_protocol {
            db::set_setting(&tx, "sync_protocol", v)?;
        }
        if let Some(v) = &cc.greader_endpoint {
            db::set_setting(&tx, "greader_endpoint", v)?;
        }
        if let Some(v) = &cc.greader_username {
            db::set_setting(&tx, "greader_username", v)?;
        }
    }

    tx.commit()?;
    Ok((imported, skipped))
}

/// 合并 app_settings：远端白名单字段覆盖本地，本地特定字段保留。
fn merge_app_settings(conn: &rusqlite::Connection, remote_raw: &str) -> AppResult<()> {
    let remote: serde_json::Value = serde_json::from_str(remote_raw)?;
    let local_raw = db::get_setting(conn, "app_settings")?;

    let mut merged = if let Some(local_raw) = local_raw {
        serde_json::from_str::<serde_json::Value>(&local_raw)?
    } else {
        serde_json::json!({})
    };

    // 远端白名单字段覆盖（排除 autoStart, closePromptShown）
    if let (Some(remote_obj), Some(merged_obj)) = (remote.as_object(), merged.as_object_mut()) {
        for (key, value) in remote_obj {
            if key != "autoStart" && key != "closePromptShown" {
                merged_obj.insert(key.clone(), value.clone());
            }
        }
    }

    db::set_setting(conn, "app_settings", &serde_json::to_string(&merged)?)?;
    Ok(())
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

pub async fn read_credentials(
    conn: &Arc<Mutex<rusqlite::Connection>>,
) -> AppResult<SyncCredentials> {
    let c = conn.lock().await;
    let raw = db::get_setting(&c, "config_sync_credentials")?
        .ok_or_else(|| AppError::not_found("未配置配置同步凭据"))?;
    Ok(serde_json::from_str(&raw)?)
}

/// Gist：创建（首次）或更新（已有 id）secret gist，内容为 payload JSON。
/// `file_name`：Gist 文件名（当前仅用于配置同步 = fluxreader-config.json）
pub async fn gist_upsert(
    http: &reqwest::Client,
    cred: &SyncCredentials,
    json: &str,
    file_name: &str,
) -> AppResult<String> {
    let auth = format!("Bearer {}", cred.token);
    match &cred.gist_id {
        Some(id) => {
            let url = format!("https://api.github.com/gists/{id}");
            let body = serde_json::json!({ "files": { file_name: { "content": json } } });
            let resp = http
                .patch(&url)
                .header("Authorization", &auth)
                .header("User-Agent", "FluxReader")
                .json(&body)
                .timeout(std::time::Duration::from_secs(30))
                .send()
                .await?;
            if !resp.status().is_success() {
                return Err(AppError::network(format!(
                    "Gist 更新失败：HTTP {}",
                    resp.status()
                )));
            }
            Ok(id.clone())
        }
        None => {
            let body = serde_json::json!({
                "description": "FluxReader 同步（勿删）",
                "public": false,
                "files": { file_name: { "content": json } }
            });
            let resp = http
                .post("https://api.github.com/gists")
                .header("Authorization", &auth)
                .header("User-Agent", "FluxReader")
                .json(&body)
                .timeout(std::time::Duration::from_secs(30))
                .send()
                .await?;
            let status = resp.status();
            let text = resp.text().await?;
            if !status.is_success() {
                /* 字符级截断：字节位置 200 可能落在多字节字符中间（panic） */
                let head: String = text.chars().take(200).collect();
                return Err(AppError::network(format!(
                    "Gist 创建失败：HTTP {status}：{head}"
                )));
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
pub async fn gist_read(
    http: &reqwest::Client,
    cred: &SyncCredentials,
    file_name: &str,
) -> AppResult<String> {
    let id = cred
        .gist_id
        .as_ref()
        .ok_or_else(|| AppError::not_found("尚未上传过同步（无 Gist id）"))?;
    let url = format!("https://api.github.com/gists/{id}");
    let resp = http
        .get(&url)
        .header("Authorization", format!("Bearer {}", cred.token))
        .header("User-Agent", "FluxReader")
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await?;
    let status = resp.status();
    let text = resp.text().await?;
    if !status.is_success() {
        return Err(AppError::network(format!("Gist 读取失败：HTTP {status}")));
    }
    let v: serde_json::Value = serde_json::from_str(&text)?;
    v.pointer(&format!("/files/{file_name}/content"))
        .and_then(|c| c.as_str())
        .map(String::from)
        .ok_or_else(|| AppError::not_found(format!("Gist 中没有文件 {file_name}")))
}

/// WebDAV：PUT 写入。
pub async fn webdav_put(
    http: &reqwest::Client,
    cred: &SyncCredentials,
    json: &str,
    file_name: &str,
) -> AppResult<()> {
    let url = format!("{}/{}", cred.server.trim_end_matches('/'), file_name);
    let resp = http
        .put(&url)
        .basic_auth(&cred.username, Some(&cred.token))
        .header("Content-Type", "application/json")
        .body(json.to_string())
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(AppError::network(format!(
            "WebDAV 上传失败：HTTP {}",
            resp.status()
        )));
    }
    Ok(())
}

/// WebDAV：GET 读取。
pub async fn webdav_get(
    http: &reqwest::Client,
    cred: &SyncCredentials,
    file_name: &str,
) -> AppResult<String> {
    let url = format!("{}/{}", cred.server.trim_end_matches('/'), file_name);
    let resp = http
        .get(&url)
        .basic_auth(&cred.username, Some(&cred.token))
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(AppError::network(format!(
            "WebDAV 读取失败：HTTP {}",
            resp.status()
        )));
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
    db::set_setting(
        &conn,
        "config_sync_credentials",
        &serde_json::to_string(&cred)?,
    )?;
    Ok(())
}

/// 上传：本地库 → 远端。
#[tauri::command]
pub async fn config_sync_upload(state: State<'_, AppState>) -> AppResult<String> {
    /* 锁内直接读凭据并构建 payload——此前在这里再调 read_credentials(内部二次 lock)
    会对同一 tokio Mutex 重入挂死（配好 Gist 后一上传即无响应） */
    let (json, mut cred) = {
        let conn = state.db.lock().await;
        let raw = db::get_setting(&conn, "config_sync_credentials")?
            .ok_or_else(|| AppError::not_found("未配置配置同步凭据"))?;
        (
            serde_json::to_string(&build_payload(&conn)?)?,
            serde_json::from_str::<SyncCredentials>(&raw)?,
        )
    };
    let http = &state.http;
    match cred.backend.as_str() {
        "gist" => {
            let id = gist_upsert(http, &cred, &json, CONFIG_FILE_NAME).await?;
            if cred.gist_id.as_deref() != Some(&id) {
                cred.gist_id = Some(id);
                let conn = state.db.lock().await;
                db::set_setting(
                    &conn,
                    "config_sync_credentials",
                    &serde_json::to_string(&cred)?,
                )?;
            }
        }
        "webdav" => webdav_put(http, &cred, &json, CONFIG_FILE_NAME).await?,
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
        "gist" => gist_read(http, &cred, CONFIG_FILE_NAME).await?,
        "webdav" => webdav_get(http, &cred, CONFIG_FILE_NAME).await?,
        other => return Err(AppError::internal(format!("未知同步后端：{other}"))),
    };
    // 校验是合法 payload（应用前先让前端确认）
    let _p: SyncPayload = serde_json::from_str(&json)?;
    Ok(json)
}

/// 应用下载的 payload（前端确认后调用）。
#[tauri::command]
pub async fn config_sync_apply(
    state: State<'_, AppState>,
    payload: String,
) -> AppResult<serde_json::Value> {
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
pub async fn webdav_put_for_test(
    http: &reqwest::Client,
    cred: &SyncCredentials,
    json: &str,
) -> AppResult<()> {
    webdav_put(http, cred, json, CONFIG_FILE_NAME).await
}

#[doc(hidden)]
pub async fn webdav_get_for_test(
    http: &reqwest::Client,
    cred: &SyncCredentials,
) -> AppResult<String> {
    webdav_get(http, cred, CONFIG_FILE_NAME).await
}
