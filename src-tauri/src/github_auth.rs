//! GitHub OAuth 设备流登录（Device Flow）——「跳转网页登录 GitHub，客户端回传获取登录信息」。
//!
//! 流程（RFC 8628 / GitHub 官方支持）：
//! 1. `POST https://github.com/login/device/code`（client_id + scope=gist）
//!    → `user_code`（用户在网页输入的码）+ `device_code`（轮询凭证）+ `verification_uri`
//! 2. 前端用 opener 插件打开 `verification_uri`，用户在浏览器完成 GitHub 授权
//! 3. 客户端按 interval 轮询 `POST https://github.com/login/oauth/access_token`
//!    （`authorization_pending` 时继续等，批准后返回 `access_token`）
//! 4. 拿 token 后 GET `/user` 验证 + 取登录名，凭据存 SQLite（settings 表
//!    `config_sync_credentials`，复用既有 Gist 后端——设备流 token 与 PAT 对
//!    Gist API 等效，gist_upsert/gist_read 无需改动）
//!
//! client_id：GitHub 官方 CLI（gh）的公开 Client ID——零配置，无需用户自建
//! OAuth App。授权页会显示「GitHub CLI」请求 gist 权限，属正常现象（社区
//! 通行做法；设备流只验证 client_id 本身，不校验调用方）。若未来策略变化，
//! settings 键 `github_oauth_client_id` 可覆盖为自建 App 的 Client ID。

use crate::config_sync::SyncCredentials;
use crate::db;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::State;
use tokio::sync::Mutex;

const GITHUB_BASE: &str = "https://github.com";
/// gh CLI 的公开 OAuth Client ID（https://github.com/cli/cli 源码常量）。
/// 设备流要求 client_id 属于真实存在的 OAuth App——官方 CLI 的 App 永远在，
/// 且其 scope 覆盖完整 REST API（含 gist）。零配置方案的基石。
const GH_CLI_CLIENT_ID: &str = "178c6fc778ccc68e1d6a";
const API_BASE: &str = "https://api.github.com";
/// 轮询间隔下限（GitHub 响应里的 interval 更小时用它防 429）
const POLL_FLOOR_SECS: u64 = 3;
/// 整个设备码有效期上限（GitHub 给 900s；提前 30s 放弃避免边界）
const DEVICE_TTL_SECS: u64 = 870;

/* ============================================================
   IPC 返回结构
   ============================================================ */

/// 第一步（发起登录）返回：前端打开 verification_uri 并展示 user_code。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DeviceLoginStart {
    /// 用户在浏览器输入的一次性码（如 ABCD-1234）
    pub user_code: String,
    /// 浏览器打开的授权页地址
    pub verification_uri: String,
    /// 轮询间隔（秒）：GitHub 建议值，前端按此节奏调用 poll
    #[allow(dead_code)] // 经 Serialize 输出到前端；rustc 侧无字段读
    pub interval: u64,
}

/// 登录完成后的账户摘要（前端展示「已登录：login」）。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GitHubAccount {
    pub login: String,
}

/* ============================================================
   内部逻辑（与命令分离，便于测试注入 base URL）
   ============================================================ */

/// 设备流第一步：向 GitHub 请求设备码。
/// 返回 (user_code, device_code, verification_uri, interval)。
async fn device_code_request(
    http: &reqwest::Client,
    base: &str,
    client_id: &str,
) -> AppResult<(String, String, String, u64)> {
    let resp = http
        .post(format!("{base}/login/device/code"))
        .header("User-Agent", "FluxReader")
        .header("Accept", "application/json")
        .form(&[("client_id", client_id), ("scope", "gist")])
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await?;
    let status = resp.status();
    let text = resp.text().await?;
    if !status.is_success() {
        /* 字符级截断：字节切片在多字节字符上会 panic（历史 bug 模式 A） */
        let head: String = text.chars().take(200).collect();
        return Err(AppError::network(format!("GitHub 设备码请求失败：HTTP {status}：{head}")));
    }
    let v: serde_json::Value = serde_json::from_str(&text)?;
    let user_code = v
        .get("user_code")
        .and_then(|c| c.as_str())
        .ok_or_else(|| AppError::network("设备码响应缺少 user_code"))?
        .to_string();
    let device_code = v
        .get("device_code")
        .and_then(|c| c.as_str())
        .ok_or_else(|| AppError::network("设备码响应缺少 device_code"))?
        .to_string();
    let verification_uri = v
        .get("verification_uri")
        .and_then(|c| c.as_str())
        .unwrap_or("https://github.com/login/device")
        .to_string();
    let interval = v.get("interval").and_then(|i| i.as_u64()).unwrap_or(5).max(POLL_FLOOR_SECS);
    Ok((user_code, device_code, verification_uri, interval))
}

/// 设备流轮询一步：返回 Ok(Some(token))（批准）/ Ok(None)（继续等）。
async fn token_poll_once(
    http: &reqwest::Client,
    base: &str,
    client_id: &str,
    device_code: &str,
) -> AppResult<Option<String>> {
    let resp = http
        .post(format!("{base}/login/oauth/access_token"))
        .header("User-Agent", "FluxReader")
        .header("Accept", "application/json")
        .form(&[
            ("client_id", client_id),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await?;
    let status = resp.status();
    let text = resp.text().await?;
    if !status.is_success() {
        let head: String = text.chars().take(200).collect();
        return Err(AppError::network(format!("GitHub 轮询失败：HTTP {status}：{head}")));
    }
    let v: serde_json::Value = serde_json::from_str(&text)?;
    if let Some(tok) = v.get("access_token").and_then(|t| t.as_str()) {
        return Ok(Some(tok.to_string()));
    }
    match v.get("error").and_then(|e| e.as_str()) {
        Some("authorization_pending") => Ok(None),
        Some("slow_down") => Ok(None), /* 客户端节流：FE 固定 5s 轮询 + GitHub interval 下限 3s，命中概率极低；命中时本次按 pending 处理即可 */
        Some("expired_token") => Err(AppError::network("设备码已过期，请重新发起登录")),
        Some("access_denied") => Err(AppError::network("用户在网页上拒绝了授权")),
        Some(other) => Err(AppError::network(format!("GitHub 授权失败：{other}"))),
        None => Err(AppError::network("GitHub 响应缺少 access_token 与 error")),
    }
}

/// 用 token 拉 /user 验证并取登录名（顺带确认 token 可用）。
async fn fetch_account(http: &reqwest::Client, api_base: &str, token: &str) -> AppResult<GitHubAccount> {
    let resp = http
        .get(format!("{api_base}/user"))
        .header("User-Agent", "FluxReader")
        .header("Authorization", format!("Bearer {token}"))
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(AppError::network(format!("GitHub 账户验证失败：HTTP {status}（token 无效或无 gist 权限）")));
    }
    let v: serde_json::Value = resp.json().await?;
    let login = v
        .get("login")
        .and_then(|l| l.as_str())
        .ok_or_else(|| AppError::network("GitHub /user 响应缺少 login"))?
        .to_string();
    Ok(GitHubAccount { login })
}

/* ============================================================
   IPC 命令
   ============================================================ */

/// 发起 GitHub 登录：返回 user_code + verification_uri。
/// 前端负责用 opener 打开网页。client_id 参数为空时用 gh CLI 公开 ID
/// （零配置）；非空视为高级覆盖（自建 OAuth App）。
/// force=false 时若现有凭据是 WebDAV 会拒绝（防止设备流静默覆盖）——前端
/// confirm 后带 force=true 重发。
#[tauri::command]
pub async fn github_login_start(
    state: State<'_, AppState>,
    client_id: Option<String>,
    force: Option<bool>,
) -> AppResult<DeviceLoginStart> {
    if !force.unwrap_or(false) {
        let existing = {
            let conn = state.db.lock().await;
            db::get_setting(&conn, "config_sync_credentials")?
        };
        if let Some(raw) = existing {
            if let Ok(c) = serde_json::from_str::<SyncCredentials>(&raw) {
                if c.backend == "webdav" {
                    return Err(AppError::new(
                        "webdavConflict",
                        "当前配置同步使用 WebDAV。登录 GitHub 会把同步后端切换为 Gist 并替换 WebDAV 凭据",
                    ));
                }
            }
        }
    }
    let override_cid = client_id.map(|c| c.trim().to_string()).filter(|c| !c.is_empty());
    let cid = override_cid.unwrap_or_else(|| GH_CLI_CLIENT_ID.to_string());
    let (user_code, device_code, verification_uri, interval) =
        device_code_request(&state.http, GITHUB_BASE, &cid).await?;
    /* device_code 与 interval 存内存态：轮询命令从 AppState 取（单窗口单流程足够；
       不落库——设备码是一次性短命凭证） */
    let device_flow = DeviceFlowState {
        client_id: cid,
        device_code,
        started_at: std::time::Instant::now(),
    };
    *state.github_flow.lock().await = Some(device_flow);
    Ok(DeviceLoginStart { user_code, verification_uri, interval })
}

/// 轮询授权状态：批准则取 token → 验证 /user → 存凭据（Gist 后端）→ 返回账户。
/// 未批准时返回 Ok(None)（前端继续按 interval 轮询）。
#[tauri::command]
pub async fn github_login_poll(state: State<'_, AppState>) -> AppResult<Option<GitHubAccount>> {
    let flow = {
        let f = state.github_flow.lock().await;
        f.clone()
    };
    let Some(flow) = flow else {
        return Err(AppError::new("badRequest", "尚未发起登录（设备码缺失）"));
    };
    if flow.started_at.elapsed().as_secs() > DEVICE_TTL_SECS {
        *state.github_flow.lock().await = None;
        return Err(AppError::network("登录等待超时（设备码过期），请重新发起"));
    }
    let token = match token_poll_once(&state.http, GITHUB_BASE, &flow.client_id, &flow.device_code).await? {
        Some(t) => t,
        None => return Ok(None), // 仍在等待用户在浏览器授权
    };
    /* 拿到 token：流程结束，清设备码态；失败也清（token 已消耗） */
    *state.github_flow.lock().await = None;
    let account = fetch_account(&state.http, API_BASE, &token).await?;
    let cred = SyncCredentials {
        backend: "gist".to_string(),
        token,
        server: String::new(),
        username: account.login.clone(),
        gist_id: None,
    };
    {
        let conn = state.db.lock().await;
        db::set_setting(&conn, "config_sync_credentials", &serde_json::to_string(&cred)?)?;
        /* 记录实际使用的 client_id：默认 gh CLI ID 或用户覆盖值（排障用） */
        db::set_setting(&conn, "github_oauth_client_id", &flow.client_id)?;
        /* OAuth 换来的凭据标注来源：断开后 UI 能提示「已断开 GitHub 登录」 */
        db::set_setting(&conn, "github_login_name", &account.login)?;
    }
    Ok(Some(account))
}

/// 已登录账户名（设置页展示）。未登录返回 None。
#[tauri::command]
pub async fn github_login_status(state: State<'_, AppState>) -> AppResult<Option<GitHubAccount>> {
    let conn = state.db.lock().await;
    let name = db::get_setting(&conn, "github_login_name")?;
    let token = db::get_setting(&conn, "config_sync_credentials")?
        .and_then(|s| serde_json::from_str::<SyncCredentials>(&s).ok())
        .map(|c| c.backend == "gist" && !c.token.is_empty());
    /* 两个条件同时满足才算「GitHub 登录态」：账户名存在 + Gist 凭据在 */
    match (name, token) {
        (Some(n), Some(true)) => Ok(Some(GitHubAccount { login: n })),
        _ => Ok(None),
    }
}

/// 断开 GitHub 登录：清凭据 + 账户名（不动 WebDAV 用户的凭据）。
#[tauri::command]
pub async fn github_login_disconnect(state: State<'_, AppState>) -> AppResult<()> {
    let conn = state.db.lock().await;
    let raw = db::get_setting(&conn, "config_sync_credentials")?;
    if let Some(s) = raw {
        if let Ok(c) = serde_json::from_str::<SyncCredentials>(&s) {
            if c.backend != "gist" {
                /* WebDAV 凭据不是 GitHub 登录——不误删 */
                return Ok(());
            }
        }
    }
    db::set_setting(&conn, "config_sync_credentials", "")?;
    db::set_setting(&conn, "github_login_name", "")?;
    db::set_setting(&conn, "config_sync_last_upload", "")?;
    Ok(())
}

/* ============================================================
   内存态：进行中的设备流（发起 → 轮询完成期间持有）
   ============================================================ */

#[derive(Debug, Clone)]
pub struct DeviceFlowState {
    client_id: String,
    device_code: String,
    started_at: std::time::Instant,
}

pub type SharedDeviceFlow = Arc<Mutex<Option<DeviceFlowState>>>;

/* ============================================================
   测试后门（tests/github_auth_e2e.rs）：注入 mock base URL
   ============================================================ */

#[doc(hidden)]
pub async fn device_code_request_for_test(
    http: &reqwest::Client,
    base: &str,
    client_id: &str,
) -> AppResult<(String, String, String, u64)> {
    device_code_request(http, base, client_id).await
}

#[doc(hidden)]
pub async fn token_poll_once_for_test(
    http: &reqwest::Client,
    base: &str,
    client_id: &str,
    device_code: &str,
) -> AppResult<Option<String>> {
    token_poll_once(http, base, client_id, device_code).await
}

#[doc(hidden)]
pub async fn fetch_account_for_test(http: &reqwest::Client, api_base: &str, token: &str) -> AppResult<GitHubAccount> {
    fetch_account(http, api_base, token).await
}
