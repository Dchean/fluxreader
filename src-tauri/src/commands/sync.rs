//! commands 的 sync 领域子模块（TASK-044 从 commands.rs 按既有章节拆分，纯搬运）。

use super::sync_configured;
use crate::db;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use tauri::State;

/* ============================================================
后端同步
============================================================ */

/// 测试连接（轻量）：按协议分派（Google Reader ClientLogin / Fever api_key），
/// 不落库、不做任何同步。用于填表时快速验证连通性。
#[tauri::command]
pub async fn sync_test(
    state: State<'_, AppState>,
    protocol: String,
    endpoint: String,
    username: String,
    password: String,
) -> AppResult<String> {
    let (msg, _, _) =
        crate::sync::test_connection(&protocol, &endpoint, &username, &password, &state.http)
            .await?;
    Ok(msg)
}

/// 保存凭据：先轻量测试（失败不保存），通过后立即落库返回。
/// 首连的重活（拉订阅、同步状态）由前端随后台阶段执行，不阻塞这里。
/// 密码留空且已连接 → 复用已存密码（仅改 Endpoint 的场景）。
/// 换账号检测：已连接其他账号（协议/endpoint/username 不同）时先清理旧账号
/// 数据（订阅/绑定/队列），避免两份订阅列表混杂。
#[tauri::command]
pub async fn sync_save(
    state: State<'_, AppState>,
    protocol: String,
    endpoint: String,
    username: String,
    password: String,
) -> AppResult<String> {
    // 协议归一：未知值回退 greader（前端下拉只有两个合法项）
    let protocol = if protocol == "fever" {
        "fever"
    } else {
        "greader"
    }
    .to_string();

    // 留空密码且已连接 → 复用旧密码（改地址不动密钥）
    //
    // P3[3]（REQ-104）：此前这一分支连**用户名**也一并复用（old_user.clone()），
    // 于是用户只改用户名、密码留空时，界面上的新用户名被静默丢弃——用户以为换了
    // 账号，实际仍连旧账号，且没有任何提示。现在只复用 password；
    // 用户名以本次输入为准（留空则同样回落到旧值，保持「只改地址」的既有便利）。
    let (endpoint, username, password) = {
        let conn = state.db.lock().await;
        let old = crate::sync::read_credentials(&conn);
        match (&old, password.trim().is_empty()) {
            (Some((_old_p, _old_ep, old_user, old_pw)), true) => {
                let typed_user = username.trim();
                (
                    endpoint.trim().to_string(),
                    if typed_user.is_empty() {
                        old_user.clone()
                    } else {
                        typed_user.to_string()
                    },
                    old_pw.clone(),
                )
            }
            (None, true) => {
                return Err(AppError::new("validate", "请填写密码"));
            }
            _ => (
                endpoint.trim().to_string(),
                username.trim().to_string(),
                password.trim().to_string(),
            ),
        }
    };
    // 换账号检测（锁内读旧凭据）；保存前凭据为空 = 首连
    let (account_changed, old_was_empty) = {
        let conn = state.db.lock().await;
        let old = crate::sync::read_credentials(&conn);
        match old {
            Some((old_p, old_ep, old_user, old_pw)) => {
                let changed = old_p != protocol
                    || old_ep.trim_end_matches('/') != endpoint.trim_end_matches('/')
                    || old_user != username
                    || old_pw != password;
                (changed, false)
            }
            None => (false, true),
        }
    };
    // 测试新凭据（失败不保存不动现状）；用户名随凭据落库（设置页动态显示）
    let (msg, _account, resolved_base) =
        crate::sync::test_connection(&protocol, &endpoint, &username, &password, &state.http)
            .await?;
    {
        let mut conn = state.db.lock().await;
        if account_changed {
            let (feeds, _) = db::purge_remote_data(&mut conn)?;
            log::info!("sync: 账号切换，清理旧账号数据：{feeds} 个订阅");
        }
        // 首连判定（保存前凭据为空 = 第一次连接）：供前端决定是否弹
        // 「同步本地订阅到后端」（本地有未绑源时）
        let unbound_local = db::count_unbound_local_feeds(&conn)?;
        let first_connect = old_was_empty && unbound_local > 0;
        db::set_setting(&conn, "sync_protocol", &protocol)?;
        db::set_setting(&conn, "greader_endpoint", &endpoint)?;
        db::set_setting(&conn, "greader_username", &username)?;
        db::set_setting(&conn, "greader_password", &password)?;
        // 端点解析结果落库（TASK-059）：设置页刚刚已验证过，同步侧直接复用，
        // 不必每轮再探测一遍。`greader_endpoint` 存的仍是用户原始输入。
        if let Err(e) = crate::endpoint_resolve::remember_base(
            &conn,
            &protocol,
            &endpoint,
            &resolved_base,
        ) {
            log::warn!("sync: 端点解析结果落库失败（不影响本次连接）: {e}");
        }
        // 新连接：清增量游标（GReader 时间戳 / Fever 条目 id），让首同步从全量开始
        db::set_setting(&conn, "sync_last_sync", "0")?;
        db::set_setting(&conn, "sync_last_entry_id", "0")?;
        if first_connect {
            return Ok(serde_json::json!({
                "message": msg,
                "firstConnect": true,
                "unboundLocalFeeds": unbound_local,
            })
            .to_string());
        }
    }
    Ok(
        serde_json::json!({ "message": msg, "firstConnect": false, "unboundLocalFeeds": 0 })
            .to_string(),
    )
}

/// 分步同步：which="feeds"（订阅层，秒级）| "states"（状态+条目层，慢）。
/// states 全量对账只在 full=true（手动/首连）时做。
#[tauri::command]
pub async fn sync_phase(
    state: State<'_, AppState>,
    which: String,
    full: Option<bool>,
) -> AppResult<crate::sync::SyncReport> {
    match which.as_str() {
        "feeds" => crate::sync::feeds_phase(&state.db, &state.http).await,
        "states" => crate::sync::states_phase(&state.db, &state.http, full.unwrap_or(false)).await,
        _ => Err(AppError::new("validate", "which 必须是 feeds 或 states")),
    }
}

/// 把本地直连订阅（origin='local' 且未绑定 remote_id）推送到服务端：
/// 入队 add_feed（带分类映射 payload）→ 立即跑 feeds 阶段（推送+碰撞绑定）。
/// 幂等：已绑定的源不入队；服务端已存在同 URL（409）回查绑定，不构成错误。
/// 返回 (待推数, 推送摘要)——首连弹窗与手动按钮共用此入口。
#[tauri::command]
pub async fn sync_local_feeds(state: State<'_, AppState>) -> AppResult<String> {
    // 锁内：找未绑定的本地源并入队（分类 id 随 payload，push 时映射远端分类）；
    // 查重：队列里已有同 URL 的 add_feed 项则跳过（弹窗确认 + 手动按钮连点
    // 不会堆积重复队列——失败项保留是重试语义，重复入队才是堆积）
    let queued = {
        let conn = state.db.lock().await;
        if !sync_configured(&conn) {
            return Err(AppError::new("notConnected", "未连接后端"));
        }
        let rows = db::list_unbound_local_feeds(&conn)?;
        // P3[7]（REQ-104）：读队列失败**不能降级成空队列**再继续入队——那样
        // pending_urls 变空，下面会把每个未绑定源都重新 enqueue，产生重复 add_feed
        // 队项（重复推送订阅）。此前是 warn + Vec::new() 后照常入队。
        // 读失败说明无法判断「谁已在队列」，此时正确做法是中止本次操作：宁可让用户
        // 重试，也不要写入重复队项（重复是**不可逆**的，重试是幂等的）。
        let queued_items = db::take_sync_queue(&conn).map_err(|e| {
            log::warn!("sync: 读队列失败，中止本次推送以免重复入队: {e}");
            e
        })?;
        let pending_urls: std::collections::HashSet<String> = queued_items
            .into_iter()
            .filter(|i| i.action == "add_feed")
            .filter_map(|i| i.feed_url)
            .collect();
        let mut n = 0usize;
        for (_id, url, folder) in rows {
            if pending_urls.contains(&url) {
                continue; // 已在队列（上次失败待重试）
            }
            let payload = serde_json::json!({ "folder_id": folder }).to_string();
            db::enqueue_sync(&conn, None, Some(&url), "add_feed", Some(&payload))?;
            n += 1;
        }
        n
    };
    if queued == 0 {
        // 没有新入队，但可能仍有待推队列项（上次失败的）——检查后再决定。
        // P3[7]：读队列失败时**不能**当作「没有待推项」直接返回「无需同步」——
        // 那会在队列非空时误导用户以为已同步完。改为按「可能有待推」处理
        // （继续走 feeds_phase；它是幂等的，多跑一次无害，漏跑才有害）。
        let has_pending = {
            let conn = state.db.lock().await;
            match db::take_sync_queue(&conn) {
                Ok(q) => q.iter().any(|i| i.action == "add_feed"),
                Err(e) => {
                    log::warn!("sync: 读队列失败，按「可能有待推项」继续（避免误报无需同步）: {e}");
                    true
                }
            }
        };
        if !has_pending {
            return Ok("没有需要同步的本地订阅（全部已绑定或已推送）".into());
        }
    }
    // feeds 阶段：push（新入队的 + 队列残留的）+ pull（碰撞绑定 + 远端新订阅）
    let report = crate::sync::feeds_phase(&state.db, &state.http).await?;
    if report.errors.is_empty() {
        Ok(format!(
            "已同步 {queued} 个本地订阅到后端（推送 {}）",
            report.pushed_feeds
        ))
    } else {
        Ok(format!(
            "已同步 {queued} 个本地订阅，其中 {} 个失败（下次同步自动重试）：{}",
            report.errors.len(),
            report.errors.join("；")
        ))
    }
}

/// 断开连接：清凭据 + 清理服务端来源数据（订阅/条目/绑定/队列）。
/// 用户直连订阅（origin='local'）保留——断开只清服务端数据的产品语义。
#[tauri::command]
pub async fn sync_disconnect(state: State<'_, AppState>) -> AppResult<String> {
    let (feeds, articles) = {
        let mut conn = state.db.lock().await;
        let r = db::purge_remote_data(&mut conn)?;
        db::set_setting(&conn, "greader_endpoint", "")?;
        db::set_setting(&conn, "greader_password", "")?;
        db::set_setting(&conn, "greader_username", "")?;
        db::set_setting(&conn, "sync_last_sync", "0")?;
        r
    };
    Ok(format!(
        "已断开并清理：移除 {feeds} 个服务端订阅（{articles} 处绑定），本地直连订阅保留"
    ))
}

/// 缓存清理：删除指定天数前的文章（收藏/待同步项保留）或仅清 AI 缓存。
/// scope='articles' | 'ai'。返回 (删文章数, 清 AI 字段数)。
#[tauri::command]
pub async fn cache_cleanup(
    state: State<'_, AppState>,
    days: i64,
    scope: String,
) -> AppResult<String> {
    if !(1..=3650).contains(&days) {
        return Err(AppError::new("validate", "天数需在 1–3650 之间"));
    }
    if scope != "articles" && scope != "ai" {
        return Err(AppError::new("validate", "scope 必须是 articles 或 ai"));
    }
    let (deleted, ai_cleared) = {
        let mut conn = state.db.lock().await;
        db::cleanup_cache(&mut conn, days, &scope)?
    };
    Ok(if scope == "articles" {
        format!("已清理 {days} 天前的文章 {deleted} 篇（收藏文章已保留）")
    } else {
        format!("已清理 {days} 天前文章的 AI 摘要与翻译缓存 {ai_cleared} 篇")
    })
}

/// 同步配置状态（设置页显示用）。account = 连接时记录的服务端用户名。
#[derive(serde::Serialize)]
pub struct SyncStatusInfo {
    pub connected: bool,
    pub endpoint: Option<String>,
    pub account: Option<String>,
    pub last_sync: i64,
    /// 同步协议："greader" | "fever"
    pub protocol: Option<String>,
}

#[tauri::command]
pub async fn sync_status(state: State<'_, AppState>) -> AppResult<SyncStatusInfo> {
    let conn = state.db.lock().await;
    let endpoint = db::get_setting(&conn, "greader_endpoint")
        .ok()
        .flatten()
        .filter(|e| !e.trim().is_empty());
    let account = db::get_setting(&conn, "greader_username")
        .ok()
        .flatten()
        .filter(|a| !a.trim().is_empty());
    Ok(SyncStatusInfo {
        connected: endpoint.is_some(),
        endpoint,
        account,
        last_sync: db::last_sync_ts(&conn).unwrap_or(0),
        protocol: db::get_setting(&conn, "sync_protocol")
            .ok()
            .flatten()
            .filter(|p| p == "fever" || p == "greader"),
    })
}
