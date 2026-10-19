//! 配置同步（GitHub Gist / WebDAV）：手动上传/下载客户端配置。
//! 同步范围（白名单原则）：分类（名称/布局/AI标志/位置 + schema2 起携带
//! 稳定 sync_id）+ 订阅源（URL/标题/归属/布局/AI标志/site_url/favicon_url）+
//! app_settings（排除 autoStart/closePromptShown）+ 非敏感连接配置
//! （sync_protocol/greader_endpoint/greader_username）。
//!
//! 凭据字段排除：greader_password, ai_config, config_sync_credentials, miniflux_token。
//! 不含正文/媒体/AI 缓存（按设计文档边界 DEC-008/009）。
//! 字段清单：白名单由本模块集中定义（见下方 SYNCED_SETTING_KEYS / 各域读取函数），
//! 新增可同步字段必须同时在此登记，避免「加了字段但没进同步」的静默缺口。
//!
//! OPT-010：应用改为两段式——`plan_config_apply` 预览（结构化 additions/updates/
//! removals + 快照 hash + 本地配置版本签名）→ 用户确认 → `config_sync_apply`
//! 在单事务内复核 hash/版本后执行；合并（保留本地独有）与镜像（以远端为准删除
//! 本地多出对象）两种模式，镜像删除不产生远端退订 outbox。实现与契约见
//! [`plan`] 子模块。
//!
//! 不做双向 CRDT 同步：下载应用仅按白名单更新，本地凭据与本地特定设置保持不变。

use crate::db;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::State;
use tokio::sync::Mutex;

mod plan;
pub use plan::{
    apply_payload_checked, plan_payload, snapshot_hash, ApplyMode, PlanItem, PlanPreview,
};

const SCHEMA_VERSION: u32 = 2;
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

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
pub struct ConnectionConfig {
    pub sync_protocol: Option<String>,
    pub greader_endpoint: Option<String>,
    pub greader_username: Option<String>,
}

impl ConnectionConfig {
    /// 是否含任一非空字段（空建议不落库）。
    pub fn has_any_value(&self) -> bool {
        [
            &self.sync_protocol,
            &self.greader_endpoint,
            &self.greader_username,
        ]
        .into_iter()
        .any(|v| v.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false))
    }
}

/// 配置导入的待确认连接建议（settings，非敏感：协议/地址/用户名，**无密码**）。
/// 导入只写这条建议，不改活动凭据/绑定；激活必须经 sync_save 专用保存流程
/// （重新输入凭据 + 携带本建议的 version 做 CAS），提交成功后同一事务消费本键。
/// 每次写入（即便同值重复导入）都递增 `version`——旧激活 token 不得误消费新建议。
// Note: 导入不直接应用连接配置、激活 CAS 与版本语义 — 见 .agents/notes/implemented/architecture/2026-10-08-账号会话与配置应用边界.md
pub(crate) const PENDING_CONNECTION_KEY: &str = "pending_connection_config";

/// 待确认建议的独立发行序号（settings，非敏感）。
/// R2 ③：token 由**持久单调 serial** 发行，清空建议（放弃/已消费）**只清内容、
/// 不重置发行器**——否则「清空后重导」会从 1 重新发行，与历史在途 token 撞车
/// （ABA：旧 token 命中新建议）。serial 只增不减（i64→u64 checked_add，溢出 Err）。
pub(crate) const PENDING_SERIAL_KEY: &str = "pending_connection_serial";

/// 待确认连接建议的落库形态：版本号 + 非敏感连接字段。
/// 旧版本（无 `version` 字段的平铺 JSON）反序列化为 version 0（兼容，不代表"从未导入"）。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PendingSuggestion {
    /// 每次导入写入递增；激活提交用它与当前值做 CAS。
    #[serde(default)]
    pub version: u64,
    #[serde(flatten)]
    pub connection: ConnectionConfig,
}

/// 读待确认连接建议（空串 = 无建议；损坏 JSON → 显式错误，不静默当无）。
pub fn read_pending_suggestion(
    conn: &rusqlite::Connection,
) -> AppResult<Option<PendingSuggestion>> {
    let Some(raw) = db::get_setting(conn, PENDING_CONNECTION_KEY)? else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    serde_json::from_str::<PendingSuggestion>(trimmed)
        .map(Some)
        .map_err(|e| {
            AppError::internal(format!(
                "待确认连接建议损坏（{PENDING_CONNECTION_KEY}）: {e}"
            ))
        })
}

/// 读待确认建议的发行序号（缺失 = 0 从未发行；现存但非法/负/溢出 = Err，
/// 不按 0 处理——那会让 serial 回退、重新发行历史 token）。
fn read_pending_serial(conn: &rusqlite::Connection) -> AppResult<u64> {
    let Some(raw) = db::get_setting(conn, PENDING_SERIAL_KEY)? else {
        return Ok(0);
    };
    raw.trim().parse::<u64>().map_err(|_| {
        AppError::internal(format!(
            "待确认建议发行序号损坏（{PENDING_SERIAL_KEY}={raw:?}）：拒绝继续，请排查/修复"
        ))
    })
}

/// 写待确认连接建议（导入事务内调用），返回本次 token（版本号）：
/// - **同值重复导入 = ignore**：不换 token（内容一致，旧 token 消费的是同一建议）；
/// - 换值 / 清空后重导 = 由独立单调 serial 发行**新 token**（绝不回退复用，
///   即使 `pending_connection_config` 被清空过——防 ABA）；
/// - serial 与内容写由调用方的导入事务捆在一起（`&tx`）：一起提交/一起回滚；
///   `checked_add` 溢出 Err（不饱和复用），serial 先写、内容后写——中途失败只留
///   「已用号段」的空洞，不会出现同号两建议。
pub fn store_pending_connection(
    conn: &rusqlite::Connection,
    cc: &ConnectionConfig,
) -> AppResult<u64> {
    if let Some(existing) = read_pending_suggestion(conn)? {
        if existing.connection == *cc {
            return Ok(existing.version); // 同值 ignore：不刷新 token
        }
    }
    let version = read_pending_serial(conn)?.checked_add(1).ok_or_else(|| {
        AppError::internal(format!(
            "待确认建议发行序号溢出（{PENDING_SERIAL_KEY} 已达 u64::MAX）：拒绝继续"
        ))
    })?;
    db::set_setting(conn, PENDING_SERIAL_KEY, &version.to_string())?;
    let suggestion = PendingSuggestion {
        version,
        connection: cc.clone(),
    };
    db::set_setting(
        conn,
        PENDING_CONNECTION_KEY,
        &serde_json::to_string(&suggestion)?,
    )?;
    Ok(version)
}

/// 清除待确认连接建议（仅专用激活提交 CAS 命中 / 用户放弃时调用）。
/// R2 ③：**只清内容，不触碰发行 serial**——历史 token 永不复用（防 ABA）。
pub fn clear_pending_connection(conn: &rusqlite::Connection) -> AppResult<()> {
    db::set_setting(conn, PENDING_CONNECTION_KEY, "")
}

#[derive(Serialize, Deserialize, Debug)]
pub struct FolderSpec {
    pub name: String,
    pub layout: String,
    pub auto_summary: bool,
    pub auto_translate: bool,
    pub position: i64,
    /// OPT-010（schema2）：分类的稳定身份（跨设备匹配键）。
    /// schema1 快照没有本字段（None）→ 导入端按名称保守匹配并采用本地身份。
    #[serde(default)]
    pub sync_id: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct FeedSpec {
    pub url: String,
    pub title: String,
    pub folder: String,
    pub layout: String,
    pub auto_summary: bool,
    pub auto_translate: bool,
    /// OPT-011：源级 AI 开关三态（'inherit' | 'on' | 'off'）。
    /// 旧 schema1 快照没有本字段（None）→ 导入时由旧 bool 映射显式 on/off
    /// （保守兼容：旧快照里关着的源绝不因导入突然开启）。非法值拒绝整包导入。
    #[serde(default)]
    pub summary_mode: Option<String>,
    #[serde(default)]
    pub translate_mode: Option<String>,
    pub site_url: Option<String>,
    pub favicon_url: Option<String>,
}

/// OPT-011：把快照的 AI 开关输入归一为三态真值——mode 优先（非法拒绝，
/// 不悄悄回默认），缺 mode 时由旧 bool 映射显式 on/off。
fn resolve_spec_modes(f: &FeedSpec) -> AppResult<(db::FeedAiMode, db::FeedAiMode)> {
    let summary = match f.summary_mode.as_deref() {
        Some(raw) => db::FeedAiMode::parse(raw).map_err(|e| {
            AppError::internal(format!("快照中的源 {} AI 模式非法：{}", f.url, e.message))
        })?,
        None => db::FeedAiMode::from_legacy_bool(f.auto_summary),
    };
    let translate = match f.translate_mode.as_deref() {
        Some(raw) => db::FeedAiMode::parse(raw).map_err(|e| {
            AppError::internal(format!("快照中的源 {} AI 模式非法：{}", f.url, e.message))
        })?,
        None => db::FeedAiMode::from_legacy_bool(f.auto_translate),
    };
    Ok((summary, translate))
}

/// 从本地库构建上传 payload（白名单原则：仅同步字段清单中 sync=允许 的字段）。
/// 排除全部凭据字段：greader_password, ai_config, config_sync_credentials, miniflux_token。
/// 排除全部凭据字段（见上方模块说明的白名单口径）。
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
            // OPT-010：schema2 携带稳定身份（v26 起每行必有；None 只在损坏库出现）。
            sync_id: f.sync_id.clone().filter(|s| !s.trim().is_empty()),
        })
        .collect();

    let feed_specs = feeds
        .iter()
        .map(|f| FeedSpec {
            url: f.feed_url.clone(),
            title: f.title.clone(),
            folder: folder_names.get(&f.folder_id).cloned().unwrap_or_default(),
            layout: f.layout.clone(),
            // 旧 bool 字段照旧导出（旧客户端读同一快照时的兼容输入），
            // 新真值 mode 同时携带（新客户端以 mode 为准）。
            auto_summary: f.auto_summary,
            auto_translate: f.auto_translate,
            summary_mode: Some(f.summary_mode.clone()),
            translate_mode: Some(f.translate_mode.clone()),
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
        // 排除本地专属字段（与下载侧共用 is_local_only_setting 单一判定）
        obj.retain(|k, _| !is_local_only_setting(k));
    }
    Ok(Some(serde_json::to_string(&settings)?))
}

/// 应用下载 payload 到本地库（兼容入口 = 保守合并、无预览 CAS）。
///
/// OPT-010 起真正的应用内核在 [`plan`]：先结构化预览（plan_payload），用户
/// 确认后携带快照 hash 与配置版本 CAS 复核再执行（apply_payload_checked）。
/// 本函数保留给既有调用方/测试，等价于 merge 模式跳过 CAS 复核；镜像删除
/// 语义不经本入口（IPC 层强制要求两者非空）。
pub fn apply_payload(conn: &rusqlite::Connection, p: &SyncPayload) -> AppResult<ApplyOutcome> {
    apply_payload_checked(conn, p, ApplyMode::Merge, None, None)
}

/// 应用下载配置的结果计数（TASK-074）：把「已更新」与「已跳过」分开，
/// 供界面如实展示（此前 skipped 实为已更新数，文案却是「跳过」）。
/// OPT-006：`pending_connection` = 本次导入落下的待确认连接建议（含版本号，
/// 非敏感；需用户重新输入凭据后经专用激活提交才会生效；导入本身不改活动凭据）。
/// OPT-010：`removed_feeds` / `removed_folders` / `affected_article_count` =
/// 镜像模式实际删除的对象数与级联文章数（合并模式恒为 0）。
#[derive(Debug, Default, Clone, Serialize)]
pub struct ApplyOutcome {
    /// 新建的源数
    pub imported: usize,
    /// 已存在且白名单字段确有变化、已写库的源数
    pub updated: usize,
    /// 已存在但内容与远端一致、无需改动的源数
    pub skipped: usize,
    /// 本次导入的待确认连接建议（若有；带激活 CAS 用的版本号）
    pub pending_connection: Option<PendingSuggestion>,
    /// 镜像删除的源数（合并模式恒为 0）
    pub removed_feeds: usize,
    /// 镜像删除的分类数（合并模式恒为 0）
    pub removed_folders: usize,
    /// 上述被删源级联删除的文章数合计
    pub affected_article_count: i64,
}

/// OPT-011：远端 app_settings 的 refreshInterval 校验单点（先于 merge 的任何写入）。
///
/// 与本地写路径（settingsValidation：区间 + Number.isInteger）和 Rust scheduler
/// （read_refresh_interval）同口径：JSON 整数或整数值小数（30.0）接受并**归一为
/// 整数**；缺失 = None（走既有「远端缺键 → 本地删除」语义）；存在但非法
/// （30.5 / 4 / 721 / 字符串 / null）→ Err。
///
/// 为什么必须让整个 apply 失败：修前原样 merge 会把 30.5 写进本地 app_settings，
/// 而前端校验拒绝（UI 保留旧值 300）、Rust scheduler 读不到整数回落默认 30——
/// DB / UI / 运行值三方分叉。不静默写非法值，也不把正确性托付给 UI 忽略。
fn validated_remote_refresh_interval(remote: &serde_json::Value) -> AppResult<Option<i64>> {
    let Some(v) = remote.get("refreshInterval") else {
        return Ok(None);
    };
    let as_int = v
        .as_i64()
        .or_else(|| v.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64));
    match as_int {
        Some(n) if (5..=720).contains(&n) => Ok(Some(n)),
        _ => Err(AppError::new(
            "validate",
            format!("配置快照的 refreshInterval 非法（必须为 5–720 整数）：{v}"),
        )),
    }
}

/// 合并 app_settings：远端白名单字段覆盖本地，本地特定字段保留。
///
/// TASK-074（P2-12，DEC-req104-p2-12-config-delete-20260920）：此前只做 upsert——
/// 远端删掉的配置键在本地永远残留（用户在服务端清掉某设置，本地却仍按旧值运行，
/// 且下一次上传会把它重新带回远端，形成「删不掉」）。现在的语义：
/// ① 远端存在且属白名单 → 覆盖本地；
/// ② 远端**不存在**且属白名单、但本地存在 → 本地同步删除（跟随远端事实）；
/// ③ 本地专属字段（autoStart / closePromptShown）永不接受远端删除或覆盖。
///
/// OPT-011：refreshInterval 是数值型配置——远端存在时先严格校验（见上），非法
/// 整体 Err 由调用方事务回滚（本地 settings/feeds 原样）；合法值归一为整数写回。
///
/// OPT-010：合并与增量（预览用 changed/removed 键）同源——[`plan`] 的
/// `merged_app_settings_json` 是唯一计算口径，本函数只负责写库。
fn merge_app_settings(conn: &rusqlite::Connection, remote_raw: &str) -> AppResult<()> {
    let (merged, _, _) = plan::merged_app_settings_json(conn, remote_raw)?;
    db::set_setting(conn, "app_settings", &serde_json::to_string(&merged)?)?;
    Ok(())
}

/// 本地专属配置键：只存本机、不参与远端同步（上传时被 filter_app_settings 剔除，
/// 应用时既不接受远端覆盖，也不因远端缺失而删除）。TASK-074：此前这两个键名在
/// 上传过滤与下载合并两处各写一遍，收敛为单一判定，避免两侧漂移。
fn is_local_only_setting(key: &str) -> bool {
    matches!(key, "autoStart" | "closePromptShown")
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

/// 预览下载的 payload（OPT-010 两段式第一步，只读）：
/// 返回结构化 additions/updates/removals（含受影响文章数）、快照 hash 与本地
/// 配置版本签名——前端展示给用户确认；取消 = 不调用 apply，零写入。
#[tauri::command]
pub async fn config_sync_plan(
    state: State<'_, AppState>,
    payload: String,
    mode: Option<String>,
) -> AppResult<serde_json::Value> {
    let p: SyncPayload = serde_json::from_str(&payload)?;
    let mode = match mode.as_deref() {
        None => ApplyMode::Merge,
        Some(raw) => ApplyMode::parse(raw)?,
    };
    let conn = state.db.lock().await;
    let preview = plan_payload(&conn, &p, mode)?;
    Ok(serde_json::to_value(preview)?)
}

/// 应用下载的 payload（OPT-010 两段式第二步：前端确认后调用）。
///
/// `expected_snapshot_hash` / `expected_version` 必须携带预览返回的两个签名；
/// 单事务内复核「快照内容未变 + 本地配置未变」后执行，任一不符返回
/// `stalePreview` 且零写入（旧预览必须重新确认，绝不重新计算后直接删）。
/// 镜像模式的删除不产生远端退订 outbox，也不改活动连接凭据（连接走
/// OPT-006 待确认建议 + 专用激活流程）。
#[tauri::command]
pub async fn config_sync_apply(
    state: State<'_, AppState>,
    payload: String,
    mode: String,
    expected_snapshot_hash: String,
    expected_version: String,
) -> AppResult<serde_json::Value> {
    let p: SyncPayload = serde_json::from_str(&payload)?;
    let mode = ApplyMode::parse(&mode)?;
    let conn = state.db.lock().await;
    let outcome = apply_payload_checked(
        &conn,
        &p,
        mode,
        Some(&expected_snapshot_hash),
        Some(&expected_version),
    )?;
    // TASK-074：如实分开 imported / updated / skipped（此前 skipped 是已更新数）
    // OPT-006：pendingConnection = 待确认连接建议——前端必须展示「需重新输入凭据
    // 后保存才会激活」，激活走 sync_save 专用流程（导入不改活动凭据）。
    // OPT-010：removedFeeds/removedFolders/affectedArticleCount = 镜像实际删除数。
    Ok(serde_json::json!({
        "mode": mode.as_str(),
        "imported": outcome.imported,
        "updated": outcome.updated,
        "skipped": outcome.skipped,
        "removedFeeds": outcome.removed_feeds,
        "removedFolders": outcome.removed_folders,
        "affectedArticleCount": outcome.affected_article_count,
        "pendingConnection": outcome.pending_connection,
    }))
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
