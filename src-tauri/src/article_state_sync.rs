//! 文章状态同步（GitHub Gist / WebDAV）：跨设备同步已读/收藏状态。

use crate::db;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::State;
use tokio::sync::Mutex;

const SCHEMA_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Debug)]
pub struct StatePayload {
    pub schema: u32,
    pub uploaded_at: String,
    pub states: Vec<StateEntry>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct StateEntry {
    pub url_norm: String,
    pub is_read: bool,
    pub is_starred: bool,
    pub updated_at: String,
}

fn build_state_payload(conn: &rusqlite::Connection) -> AppResult<StatePayload> {
    let mut stmt = conn.prepare(
        "SELECT url_norm, is_read, is_starred, COALESCE(updated_at, fetched_at) AS ts
           FROM articles
          WHERE url_norm IS NOT NULL AND url_norm != ''
          ORDER BY id"
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)? != 0,
            r.get::<_, i64>(2)? != 0,
            r.get::<_, String>(3)?,
        ))
    })?;
    let states: Vec<StateEntry> = rows
        .map(|r| {
            let (url_norm, is_read, is_starred, updated_at) = r?;
            Ok(StateEntry { url_norm, is_read, is_starred, updated_at })
        })
        .collect::<Result<Vec<_>, rusqlite::Error>>()?;
    Ok(StatePayload {
        schema: SCHEMA_VERSION,
        uploaded_at: chrono::Utc::now().to_rfc3339(),
        states,
    })
}

#[derive(Serialize, Debug, Default)]
pub struct ApplyReport {
    pub matched: usize,
    pub set_read: usize,
    pub set_starred: usize,
}

fn apply_state_payload(conn: &mut rusqlite::Connection, p: &StatePayload) -> AppResult<ApplyReport> {
    if p.schema > SCHEMA_VERSION {
        return Err(AppError::internal(format!("remote version v{} > client v{}", p.schema, SCHEMA_VERSION)));
    }
    let tx = conn.transaction()?;
    let mut report = ApplyReport::default();
    let mut local: std::collections::HashMap<String, (i64, i64)> = std::collections::HashMap::new();
    {
        let mut stmt = tx.prepare("SELECT url_norm, is_read, is_starred FROM articles WHERE url_norm IS NOT NULL AND url_norm != ''")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))?;
        for r in rows {
            let (u, r_, s) = r?;
            local.insert(u, (r_, s));
        }
    }
    for e in &p.states {
        if let Some((lr, ls)) = local.get(&e.url_norm).copied() {
            report.matched += 1;
            let want_read = lr != 0 || e.is_read;
            let want_starred = ls != 0 || e.is_starred;
            if want_read && lr == 0 {
                tx.execute("UPDATE articles SET is_read = 1 WHERE url_norm = ?1", rusqlite::params![e.url_norm])?;
                report.set_read += 1;
            }
            if want_starred && ls == 0 {
                tx.execute("UPDATE articles SET is_starred = 1 WHERE url_norm = ?1", rusqlite::params![e.url_norm])?;
                report.set_starred += 1;
            }
        }
    }
    tx.commit()?;
    Ok(report)
}

#[tauri::command]
pub async fn article_state_upload(state: State<'_, AppState>) -> AppResult<String> {
    let (json, mut cred) = {
        let conn = state.db.lock().await;
        let raw = db::get_setting(&conn, "config_sync_credentials")?
            .ok_or_else(|| AppError::not_found("creds not configured"))?;
        let p = build_state_payload(&conn)?;
        (serde_json::to_string(&p)?, serde_json::from_str::<crate::config_sync::SyncCredentials>(&raw)?)
    };
    let http = &state.http;
    let file_name = crate::config_sync::STATE_FILE_NAME;
    match cred.backend.as_str() {
        "gist" => {
            let id = crate::config_sync::gist_upsert(http, &cred, &json, file_name).await?;
            if cred.gist_id.as_deref() != Some(&id) {
                cred.gist_id = Some(id);
                let conn = state.db.lock().await;
                db::set_setting(&conn, "config_sync_credentials", &serde_json::to_string(&cred)?)?;
            }
        }
        "webdav" => crate::config_sync::webdav_put(http, &cred, &json, file_name).await?,
        _ => return Err(AppError::internal(format!("unknown backend: {}", cred.backend))),
    }
    let now = chrono::Utc::now().to_rfc3339();
    {
        let conn = state.db.lock().await;
        db::set_setting(&conn, "article_state_last_upload", &now)?;
    }
    Ok(now)
}

#[tauri::command]
pub async fn article_state_download(state: State<'_, AppState>) -> AppResult<String> {
    let cred = crate::config_sync::read_credentials(&state.db).await?;
    let http = &state.http;
    let file_name = crate::config_sync::STATE_FILE_NAME;
    let json = match cred.backend.as_str() {
        "gist" => crate::config_sync::gist_read(http, &cred, file_name).await?,
        "webdav" => crate::config_sync::webdav_get(http, &cred, file_name).await?,
        _ => return Err(AppError::internal(format!("unknown backend: {}", cred.backend))),
    };
    let _p: StatePayload = serde_json::from_str(&json)?;
    Ok(json)
}

#[tauri::command]
pub async fn article_state_apply(state: State<'_, AppState>, payload: String) -> AppResult<ApplyReport> {
    let p: StatePayload = serde_json::from_str(&payload)?;
    let mut conn = state.db.lock().await;
    apply_state_payload(&mut conn, &p)
}

#[tauri::command]
pub async fn article_state_status(state: State<'_, AppState>) -> AppResult<serde_json::Value> {
    let (cred, last_upload, local_count) = {
        let conn = state.db.lock().await;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM articles WHERE url_norm IS NOT NULL AND url_norm != ''", [], |r| r.get(0)).unwrap_or(0);
        (db::get_setting(&conn, "config_sync_credentials")?, db::get_setting(&conn, "article_state_last_upload")?, count)
    };
    let cred: Option<crate::config_sync::SyncCredentials> = cred.and_then(|s| serde_json::from_str(&s).ok());
    Ok(serde_json::json!({
        "configured": cred.is_some(),
        "backend": cred.as_ref().map(|c| c.backend.clone()),
        "lastUpload": last_upload,
        "localCount": local_count,
    }))
}

pub async fn article_state_sync_tick(db: &Arc<Mutex<rusqlite::Connection>>, http: &reqwest::Client) -> AppResult<bool> {
    let cred = {
        let conn = db.lock().await;
        match db::get_setting(&conn, "config_sync_credentials")? {
            Some(raw) => serde_json::from_str::<crate::config_sync::SyncCredentials>(&raw)?,
            None => return Ok(false),
        }
    };
    let file_name = crate::config_sync::STATE_FILE_NAME;
    let json = match cred.backend.as_str() {
        "gist" => crate::config_sync::gist_read(http, &cred, file_name).await?,
        "webdav" => crate::config_sync::webdav_get(http, &cred, file_name).await?,
        _ => return Ok(false),
    };
    let p: StatePayload = serde_json::from_str(&json)?;
    let mut conn = db.lock().await;
    let _report = apply_state_payload(&mut conn, &p)?;
    Ok(true)
}
