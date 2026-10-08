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
///
/// OPT-014 R1 失败关闭：
/// - 旧凭据**读取/解密失败**（如密文损坏）→ 整体 Err，不写任何新账号数据
///   （修前 `read_credentials` 吞错成 None，被当「首次连接」跳过换号清理）；
/// - 清理与配置写入在同一事务提交（见 [`commit_account_settings`]），
///   任何一步失败（含密码 DPAPI 加密）整体回滚——旧配置/订阅/队列原样。
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

    // 旧凭据读取（仅一次，保留 Result）：Err 原样上抛——不得当未配置/首次连接（R1 P2）。
    let old = {
        let conn = state.db.lock().await;
        crate::sync::read_credentials(&conn)
    };
    // 密码复用 / 换号判定 / 首连判定（纯逻辑，模块单测覆盖）
    let plan = resolve_save_plan(old, &protocol, &endpoint, &username, &password)?;

    // 测试新凭据（失败不保存不动现状）；用户名随凭据落库（设置页动态显示）
    let (msg, _account, resolved_base) = crate::sync::test_connection(
        &plan.protocol,
        &plan.endpoint,
        &plan.username,
        &plan.password,
        &state.http,
    )
    .await?;

    let mut conn = state.db.lock().await;
    let unbound_local = commit_account_settings(
        &mut conn,
        &plan,
        &resolved_base,
        crate::credentials::encrypt_secret,
    )?;
    // 首连判定：保存前无凭据，且清理后仍有未绑定的本地直连源
    let first_connect = plan.old_was_empty && unbound_local > 0;
    Ok(serde_json::json!({
        "message": msg,
        "firstConnect": first_connect,
        "unboundLocalFeeds": if first_connect { unbound_local } else { 0 },
    })
    .to_string())
}

/// sync_save 的凭据规划结果（纯数据，无副作用；协议已归一）。
#[derive(Debug)]
struct SavePlan {
    protocol: String,
    endpoint: String,
    username: String,
    password: String,
    /// 协议/地址/用户名/密码任一变化 = 换账号（需要先清理旧账号数据）
    account_changed: bool,
    /// 保存前没有任何可用凭据 = 首次连接
    old_was_empty: bool,
}

/// 旧凭据 → 本次提交计划（OPT-014 R1 纯函数收口）。
///
/// `old` 是 [`crate::sync::read_credentials`] 的原样结果：
/// - `Err` 必须原样上抛——**绝不**当「首次连接」继续写新账号（修前后的
///   吞错路径正是这样跳过换号清理的）；
/// - `Ok(None)` + 空密码 → validate Err（无从复用密码）；
/// - `Ok(Some)` + 空密码 → 复用旧密码；用户名留空回落旧值，显式输入以本次
///   为准（P3[3] 语义保持）。
fn resolve_save_plan(
    old: AppResult<Option<(String, String, String, String)>>,
    protocol: &str,
    endpoint: &str,
    username: &str,
    password: &str,
) -> AppResult<SavePlan> {
    let old = old?;
    let typed_pw = password.trim();
    let (endpoint, username, password) = match (&old, typed_pw.is_empty()) {
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
            typed_pw.to_string(),
        ),
    };
    let (account_changed, old_was_empty) = match &old {
        Some((old_p, old_ep, old_user, old_pw)) => (
            old_p != protocol
                || old_ep.trim_end_matches('/') != endpoint.trim_end_matches('/')
                || old_user != &username
                || old_pw != &password,
            false,
        ),
        None => (false, true),
    };
    Ok(SavePlan {
        protocol: protocol.to_string(),
        endpoint,
        username,
        password,
        account_changed,
        old_was_empty,
    })
}

/// 账号提交事务内核（OPT-014 R1）：换号清理 + 凭据/游标落库在**同一短事务**
/// 内全有全无——任一步失败（含密码 DPAPI 加密）整体回滚，旧账号配置/订阅/
/// 队列原样。返回清理后仍未绑定的本地直连源数（首连弹窗判定输入）。
///
/// `encrypt` 注入仅本模块单测使用（生产恒为 `credentials::encrypt_secret`）；
/// 不是 IPC 命令参数，webview 无法触达。
fn commit_account_settings(
    conn: &mut rusqlite::Connection,
    plan: &SavePlan,
    resolved_base: &str,
    encrypt: fn(&str) -> AppResult<String>,
) -> AppResult<i64> {
    let tx = conn.transaction()?;
    if plan.account_changed {
        let (feeds, _) = db::purge_remote_data_in(&tx)?;
        log::info!("sync: 账号切换，清理旧账号数据：{feeds} 个订阅");
    }
    let unbound_local = db::count_unbound_local_feeds(&tx)?;
    db::set_setting_with(&tx, "sync_protocol", &plan.protocol, encrypt)?;
    db::set_setting_with(&tx, "greader_endpoint", &plan.endpoint, encrypt)?;
    db::set_setting_with(&tx, "greader_username", &plan.username, encrypt)?;
    db::set_setting_with(&tx, "greader_password", &plan.password, encrypt)?;
    // 端点解析结果落库（TASK-059）：设置页刚刚已验证过，同步侧直接复用，
    // 不必每轮再探测一遍。`greader_endpoint` 存的仍是用户原始输入。
    if let Err(e) =
        crate::endpoint_resolve::remember_base(&tx, &plan.protocol, &plan.endpoint, resolved_base)
    {
        log::warn!("sync: 端点解析结果落库失败（不影响本次连接）: {e}");
    }
    // 新连接：清增量游标（GReader 时间戳 / Fever 条目 id），让首同步从全量开始
    db::set_setting_with(&tx, "sync_last_sync", "0", encrypt)?;
    db::set_setting_with(&tx, "sync_last_entry_id", "0", encrypt)?;
    tx.commit()?;
    Ok(unbound_local)
}

/// 断开提交事务内核（OPT-014 R1）：服务端数据清理 + 凭据/游标清空在同一短
/// 事务内全有全无。`encrypt` 注入仅本模块单测使用。
fn commit_disconnect(
    conn: &mut rusqlite::Connection,
    encrypt: fn(&str) -> AppResult<String>,
) -> AppResult<(usize, usize)> {
    let tx = conn.transaction()?;
    let r = db::purge_remote_data_in(&tx)?;
    db::set_setting_with(&tx, "greader_endpoint", "", encrypt)?;
    db::set_setting_with(&tx, "greader_password", "", encrypt)?;
    db::set_setting_with(&tx, "greader_username", "", encrypt)?;
    db::set_setting_with(&tx, "sync_last_sync", "0", encrypt)?;
    tx.commit()?;
    Ok(r)
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
        // OPT-014 R1：凭据读取失败 → Err（不得当未连接静默返回）
        if !sync_configured(&conn)? {
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
/// OPT-014 R1：清理与清空在同一事务（见 [`commit_disconnect`]），失败整体回滚；
/// 本命令不读取旧密码，故损坏密文导致的 sync_save 失败可用「断开 → 重连」恢复。
#[tauri::command]
pub async fn sync_disconnect(state: State<'_, AppState>) -> AppResult<String> {
    let (feeds, articles) = {
        let mut conn = state.db.lock().await;
        commit_disconnect(&mut conn, crate::credentials::encrypt_secret)?
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

/// 同步队列统计（TASK-116 四态展示）：侧栏 pill 与设置页「同步状态」摘要卡的
/// 等待/部分失败口径。waiting = 队列现存行数；failed = attempts>0 行数；
/// last_error = 最近一次推送失败的错误摘要。纯读命令，未连接也可调（队列是
/// 本地事实——「本地已保存，连接后自动补推」的展示依据）。
#[tauri::command]
pub async fn sync_queue_stats(state: State<'_, AppState>) -> AppResult<db::SyncQueueStats> {
    let conn = state.db.lock().await;
    db::sync_queue_stats(&conn)
}

/* ============================================================
R1 返工：账号提交事务与读取错误区分（OPT-014 F22 收口）
============================================================ */

#[cfg(test)]
mod account_commit_tests {
    use super::*;
    use crate::db;

    fn conn() -> rusqlite::Connection {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        db::MIGRATIONS.to_latest(&mut conn).unwrap();
        conn
    }

    fn count(conn: &rusqlite::Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    /// 测试用提交计划（字段与纯函数产物同构）。
    fn plan(purge: bool, protocol: &str, endpoint: &str, user: &str, pw: &str) -> SavePlan {
        SavePlan {
            protocol: protocol.to_string(),
            endpoint: endpoint.to_string(),
            username: user.to_string(),
            password: pw.to_string(),
            account_changed: purge,
            old_was_empty: false,
        }
    }

    /// 旧账号配置（完整四键）
    fn seed_old_account(conn: &rusqlite::Connection) {
        db::set_setting(conn, "sync_protocol", "greader").unwrap();
        db::set_setting(conn, "greader_endpoint", "http://old.example").unwrap();
        db::set_setting(conn, "greader_username", "old-user").unwrap();
        db::set_setting(conn, "greader_password", "old-pw").unwrap();
    }

    /// 旧账号的远端数据：订阅 + 文章 + 绑定 + 队列 + 墓碑
    fn seed_remote(conn: &rusqlite::Connection) {
        let folder = db::create_folder(conn, "远端分类", "article").unwrap();
        let feed = db::insert_feed_origin(
            conn,
            "http://old.example/feed.xml",
            None,
            "Remote",
            None,
            folder,
            "inherit",
            true,
            false,
            "remote",
        )
        .unwrap();
        let a = db::NewArticle {
            guid: "r1".into(),
            url: Some("http://old.example/a1".into()),
            title: "A".into(),
            author: None,
            summary: None,
            content_html: None,
            body_text: "a".into(),
            image_url: None,
            enclosure_url: None,
            enclosure_mime: None,
            duration_sec: None,
            published_at: Some(chrono::Utc::now().to_rfc3339()),
            source: "miniflux".into(),
        };
        let (aid, _) = db::upsert_article_with_feed(conn, feed, &a, false).unwrap();
        db::set_article_remote_id(conn, aid, 100).unwrap();
        db::set_feed_remote_id(conn, feed, 10).unwrap();
        db::enqueue_sync(conn, Some(aid), None, "read", None).unwrap();
        conn.execute(
            "INSERT INTO deduped_urls (url, kept_aid) VALUES ('http://old.example/dup', ?1)",
            [aid],
        )
        .unwrap();
    }

    /// R1 P1 反例：换号提交中密码加密失败 → 整个账号提交回滚。
    /// 旧配置/远端订阅/绑定/队列/墓碑必须全部原样（修前：purge 与新 protocol/
    /// endpoint/username 已落库，遇 encrypt Err 留下「新地址 + 旧密码 + 空数据」）。
    #[test]
    fn commit_failure_rolls_back_purge_and_all_settings() {
        let mut conn = conn();
        seed_old_account(&conn);
        seed_remote(&conn);

        let err = commit_account_settings(
            &mut conn,
            &plan(true, "fever", "http://new.example", "new-user", "new-pw"),
            "http://new.example/api",
            |_| Err(AppError::new("credentialEncrypt", "注入的 DPAPI 失败")),
        )
        .expect_err("加密失败必须让整个账号提交失败");
        assert_eq!(err.code, "credentialEncrypt");

        // 旧配置四键原样（密码仍可正常读回旧值）
        assert_eq!(
            db::get_setting(&conn, "sync_protocol").unwrap().unwrap(),
            "greader"
        );
        assert_eq!(
            db::get_setting(&conn, "greader_endpoint").unwrap().unwrap(),
            "http://old.example"
        );
        assert_eq!(
            db::get_setting(&conn, "greader_username").unwrap().unwrap(),
            "old-user"
        );
        assert_eq!(
            db::get_setting(&conn, "greader_password").unwrap().unwrap(),
            "old-pw"
        );
        // 远端订阅/绑定/队列/墓碑原样
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM feeds WHERE origin = 'remote'"),
            1,
            "换号清理必须整体回滚"
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM articles WHERE remote_id IS NOT NULL"
            ),
            1,
            "绑定必须原样"
        );
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM sync_queue"),
            1,
            "待推队列必须原样"
        );
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM deduped_urls"),
            1,
            "墓碑必须原样"
        );
    }

    /// 对照：同一路径成功加密器 → 事务提交，换号清理与新配置同时生效。
    #[test]
    fn commit_success_purges_old_account_and_writes_new() {
        let mut conn = conn();
        seed_old_account(&conn);
        seed_remote(&conn);

        let unbound = commit_account_settings(
            &mut conn,
            &plan(true, "fever", "http://new.example", "new-user", "new-pw"),
            "http://new.example",
            crate::credentials::encrypt_secret,
        )
        .unwrap();
        assert_eq!(unbound, 0, "无本地未绑定源");
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM feeds WHERE origin = 'remote'"),
            0
        );
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM sync_queue"), 0);
        assert_eq!(
            db::get_setting(&conn, "greader_endpoint").unwrap().unwrap(),
            "http://new.example"
        );
        assert_eq!(
            db::get_setting(&conn, "greader_password").unwrap().unwrap(),
            "new-pw"
        );
    }

    /// R1：断开的清理与凭据清空在**同一短事务**——任一步失败整体回滚（远端数据/
    /// 旧配置不被半清）。
    #[test]
    fn disconnect_failure_rolls_back_cleanup_and_settings() {
        let mut conn = conn();
        seed_old_account(&conn);
        seed_remote(&conn);

        let err = commit_disconnect(&mut conn, |_| {
            Err(AppError::new("credentialEncrypt", "注入的 DPAPI 失败"))
        })
        .expect_err("清空密码加密失败必须整体失败");
        assert_eq!(err.code, "credentialEncrypt");

        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM feeds WHERE origin = 'remote'"),
            1,
            "清理必须随事务回滚"
        );
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM sync_queue"), 1);
        assert_eq!(
            db::get_setting(&conn, "greader_endpoint").unwrap().unwrap(),
            "http://old.example"
        );
        assert_eq!(
            db::get_setting(&conn, "greader_password").unwrap().unwrap(),
            "old-pw"
        );
    }

    /// 断开成功：远端数据清理 + 凭据清空提交（本地直连保留由既有 e2e 锁）。
    #[test]
    fn disconnect_success_clears_and_purges() {
        let mut conn = conn();
        seed_old_account(&conn);
        seed_remote(&conn);

        let (feeds, _) = commit_disconnect(&mut conn, crate::credentials::encrypt_secret).unwrap();
        assert_eq!(feeds, 1);
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM feeds WHERE origin = 'remote'"),
            0
        );
        assert!(matches!(crate::sync::read_credentials(&conn), Ok(None)));
    }

    /// R1 P2 恢复负例：损坏密文 → 读取 Err（不得当未配置）；
    /// 断开（不读旧密码）→ 清空 → 完整凭据重新提交 → 恢复可读。
    #[test]
    fn corrupt_cipher_recovers_via_disconnect_then_reconnect() {
        let mut conn = conn();
        seed_old_account(&conn);
        conn.execute(
            "UPDATE settings SET value = 'dpapi:@@corrupt@@' WHERE key = 'greader_password'",
            [],
        )
        .unwrap();
        assert!(
            crate::sync::read_credentials(&conn).is_err(),
            "损坏密文必须 Err，不得吞成未配置"
        );

        commit_disconnect(&mut conn, crate::credentials::encrypt_secret).unwrap();
        assert!(matches!(crate::sync::read_credentials(&conn), Ok(None)));

        commit_account_settings(
            &mut conn,
            &plan(false, "greader", "http://new.example", "new-user", "new-pw"),
            "http://new.example",
            crate::credentials::encrypt_secret,
        )
        .unwrap();
        let c = crate::sync::read_credentials(&conn).unwrap().unwrap();
        assert_eq!(c.3, "new-pw", "重配后凭据恢复可读");
    }

    /// R1 P2：保存计划的读取错误必须原样上抛——不得当「首次连接」继续写新账号。
    #[test]
    fn save_plan_rejects_read_error_instead_of_first_connect() {
        let err = resolve_save_plan(
            Err(AppError::new("credentialDecrypt", "凭据损坏")),
            "greader",
            "http://new.example",
            "new-user",
            "new-pw",
        )
        .unwrap_err();
        assert_eq!(err.code, "credentialDecrypt");
    }

    #[test]
    fn save_plan_blank_password_needs_existing_credentials() {
        let err = resolve_save_plan(Ok(None), "greader", "http://x", "u", "").unwrap_err();
        assert_eq!(err.code, "validate");
        assert_eq!(err.message, "请填写密码");
    }

    #[test]
    fn save_plan_first_connect_flags_and_trim() {
        let p = resolve_save_plan(Ok(None), "greader", " http://x ", " u ", " pw ").unwrap();
        assert_eq!(p.endpoint, "http://x");
        assert_eq!(p.username, "u");
        assert_eq!(p.password, "pw");
        assert!(p.old_was_empty && !p.account_changed);
    }

    /// 密码留空复用旧密码；用户名留空回落旧值、显式输入以本次为准（P3[3] 语义保持）。
    #[test]
    fn save_plan_reuses_password_and_username_when_blank() {
        let old = Some((
            "greader".to_string(),
            "http://old/".to_string(),
            "old-user".to_string(),
            "old-pw".to_string(),
        ));
        let p = resolve_save_plan(Ok(old.clone()), "greader", "http://old", "", "").unwrap();
        assert_eq!(p.endpoint, "http://old");
        assert_eq!(p.username, "old-user");
        assert_eq!(p.password, "old-pw");
        assert!(!p.account_changed, "仅尾斜杠差异不算换号");

        let p2 = resolve_save_plan(Ok(old), "greader", "http://old", "new-user", "").unwrap();
        assert_eq!(p2.username, "new-user", "显式用户名不被旧值覆盖");
        assert!(p2.account_changed, "用户名变化 = 换号");
    }

    #[test]
    fn save_plan_full_new_account_marks_changed() {
        let old = Some((
            "greader".to_string(),
            "http://old".to_string(),
            "old-user".to_string(),
            "old-pw".to_string(),
        ));
        let p = resolve_save_plan(Ok(old), "fever", "http://new", "new-user", "new-pw").unwrap();
        assert!(p.account_changed && !p.old_was_empty);
    }
}
