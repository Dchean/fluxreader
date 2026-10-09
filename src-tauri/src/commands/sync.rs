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
/// 换账号检测（OPT-006）：身份 = 协议 + 规范化服务器根 + 用户名。身份变化时
/// 清理旧账号数据（订阅/绑定/队列/旧墓碑）并重置游标与 Fever 历史；同账号仅
/// 更新密码时保留全部本地数据与历史进度，但仍推进代际使旧请求失效。
///
/// OPT-014 R1 失败关闭：
/// - 旧凭据**读取/解密失败**（如密文损坏）→ 整体 Err，不写任何新账号数据
///   （修前 `read_credentials` 吞错成 None，被当「首次连接」跳过换号清理）；
/// - 清理与配置写入在同一事务提交（见 [`commit_account_settings`]），
///   任何一步失败（含密码 DPAPI 加密）整体回滚——旧配置/订阅/队列原样。
/// - 并发保护（OPT-006）：提交前复核「开始验证时捕获的代际」，两次保存并发时
///   迟到的验证不得覆盖较新的保存。
/// - R1：`activate_pending_version`（可选）是配置导入建议的**专用激活信号**；
///   旧调用方不传（None）= 普通保存，不消费任何建议。激活的版本/身份在 HTTP 前
///   与提交事务内两次校验（CAS），不匹配整体拒绝、活动配置保持原样。
#[tauri::command]
pub async fn sync_save(
    state: State<'_, AppState>,
    protocol: String,
    endpoint: String,
    username: String,
    password: String,
    activate_pending_version: Option<u64>,
) -> AppResult<String> {
    sync_save_impl(
        &state.db,
        &state.http,
        protocol,
        endpoint,
        username,
        password,
        activate_pending_version,
    )
    .await
}

/// sync_save 的真实实现（命令与集成测试共用同一路径；测试入口见
/// [`sync_save_for_test`]，不是 IPC、webview 不可达）。
async fn sync_save_impl(
    db: &std::sync::Arc<tokio::sync::Mutex<rusqlite::Connection>>,
    http: &reqwest::Client,
    protocol: String,
    endpoint: String,
    username: String,
    password: String,
    activate_pending_version: Option<u64>,
) -> AppResult<String> {
    // 协议归一：未知值回退 greader（前端下拉只有两个合法项）
    let protocol = if protocol == "fever" {
        "fever"
    } else {
        "greader"
    }
    .to_string();

    // 旧凭据 + 代际读取（同一次持锁；保留 Result）：Err 原样上抛——
    // 不得当未配置/首次连接（R1 P2）。
    let (old, expected_generation) = {
        let conn = db.lock().await;
        let old = crate::sync::read_credentials(&conn);
        let generation = db::sync_generation(&conn);
        (old, generation)
    };
    let expected_generation = expected_generation?;
    // 密码复用 / 身份变化判定 / 首连判定（纯逻辑，模块单测覆盖）
    let mut plan = resolve_save_plan(
        old,
        &protocol,
        &endpoint,
        &username,
        &password,
        expected_generation,
    )?;
    plan.activate_pending = activate_pending_version;

    // R2 ①：显式激活必须凭 **fresh credential**——即使身份与当前完全一致，
    // 也不得复用已保存密码（激活是用户对「导入建议」的明确确认动作）。
    // 放在 HTTP 之前：不满足即拒绝，不发任何请求。
    if plan.activate_pending.is_some() && !plan.password_supplied {
        return Err(AppError::new(
            "validate",
            "激活待确认连接建议必须重新输入密码（不能用已保存的旧密码）",
        ));
    }

    // R1 P2：激活的 HTTP 前校验——携带的版本必须仍指向**当前**建议，且建议内容
    // 与本次保存目标身份一致；不匹配在发任何请求之前拒绝（不信任 UI）。
    if let Some(expected_version) = activate_pending_version {
        let pending = {
            let conn = db.lock().await;
            crate::config_sync::read_pending_suggestion(&conn)?
        };
        match pending {
            Some(s)
                if s.version == expected_version
                    && suggestion_matches_plan(&s.connection, &plan) => {}
            Some(_) => {
                return Err(AppError::new(
                    "staleActivation",
                    "待确认连接建议已变更（版本或内容不匹配），本次激活已取消，当前连接未改变",
                ));
            }
            None => {
                return Err(AppError::new(
                    "staleActivation",
                    "没有可激活的待确认连接建议（可能已被处理），当前连接未改变",
                ));
            }
        }
    }

    // 测试新凭据（失败不保存不动现状）；用户名随凭据落库（设置页动态显示）
    let (msg, _account, resolved_base) = crate::sync::test_connection(
        &plan.protocol,
        &plan.endpoint,
        &plan.username,
        &plan.password,
        http,
    )
    .await?;

    let mut conn = db.lock().await;
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

/// doc(hidden) 集成测试入口：与 `sync_save` 完全同一实现（本地 mock 服务替代
/// 真实网关）。不是 IPC，webview 不可达。
#[doc(hidden)]
pub async fn sync_save_for_test(
    db: &std::sync::Arc<tokio::sync::Mutex<rusqlite::Connection>>,
    http: &reqwest::Client,
    protocol: &str,
    endpoint: &str,
    username: &str,
    password: &str,
    activate_pending_version: Option<u64>,
) -> AppResult<String> {
    sync_save_impl(
        db,
        http,
        protocol.to_string(),
        endpoint.to_string(),
        username.to_string(),
        password.to_string(),
        activate_pending_version,
    )
    .await
}

/// sync_save 的凭据规划结果（纯数据，无副作用；协议已归一）。
#[derive(Debug)]
struct SavePlan {
    protocol: String,
    endpoint: String,
    username: String,
    password: String,
    /// 身份变化（协议/规范化服务器根/用户名任一不同）= 换账号：清理旧账号数据
    /// 并重置游标/历史（旧 remote_id 命名空间不再适用）。
    identity_changed: bool,
    /// 同账号仅密码变化：保留本地数据与历史进度，但仍推进代际使旧请求失效。
    password_changed: bool,
    /// 保存前没有任何可用凭据 = 首次连接
    old_was_empty: bool,
    /// 开始验证（读旧凭据）时捕获的代际：提交前复核，防止迟到保存覆盖较新保存。
    expected_generation: i64,
    /// 配置导入建议的专用激活：携带激活时读到的建议版本；提交时与当前建议做
    /// CAS，命中才消费；None = 普通保存（绝不触碰建议）。由 `sync_save_impl`
    /// 在 HTTP 前填入并做预校验。
    activate_pending: Option<u64>,
    /// R2 ①：本次提交是否**显式提供了新密码**（空密码复用旧密码为 false）。
    /// 显式激活（activate_pending 有值）必须为 true——即使身份相同也不得复用
    /// 已保存密码，激活必须凭 fresh credential。
    password_supplied: bool,
}

/// 规范化「服务器根」身份：scheme + host + **有效端口** + basepath（host 小写、
/// 去尾斜杠）。只归并**等价形态**：省略端口与显式默认端口（http:80 / https:443）
/// 视为同一 server；同 host 不同端口、不同 basepath 都是不同 server（R2：端口
/// 丢失会让 `host:8080` 与 `host:8081` 被当同身份复用旧密码）。
/// 解析失败（如缺 scheme 的裸串）退回整体小写 + 去尾斜杠——保守等价。
fn normalized_server_root(endpoint: &str) -> String {
    let trimmed = endpoint.trim().trim_end_matches('/');
    match url::Url::parse(trimmed) {
        Ok(u) => {
            let host = u.host_str().unwrap_or("").to_ascii_lowercase();
            // port_or_known_default：未显式端口返回协议默认端口，显式默认端口
            // 与之相等 → 两种写法归并；非默认端口原样保留。
            let port = u
                .port_or_known_default()
                .map(|p| format!(":{p}"))
                .unwrap_or_default();
            let path = u.path().trim_end_matches('/');
            format!(
                "{}://{}{}{}",
                u.scheme().to_ascii_lowercase(),
                host,
                port,
                path
            )
        }
        Err(_) => trimmed.to_ascii_lowercase(),
    }
}

/// 待确认建议与本次保存目标是否指向同一身份（逐字段比对：建议只提供部分字段时，
/// 只校验已提供的字段）。激活提交与 HTTP 前预校验共用这一判定。
fn suggestion_matches_plan(
    suggestion: &crate::config_sync::ConnectionConfig,
    plan: &SavePlan,
) -> bool {
    let protocol_matches = match suggestion.sync_protocol.as_deref() {
        Some(p) if !p.trim().is_empty() => p.trim() == plan.protocol,
        _ => true,
    };
    let endpoint_matches = match suggestion.greader_endpoint.as_deref() {
        Some(e) if !e.trim().is_empty() => {
            normalized_server_root(e) == normalized_server_root(&plan.endpoint)
        }
        _ => true,
    };
    let username_matches = match suggestion.greader_username.as_deref() {
        Some(u) if !u.trim().is_empty() => u.trim() == plan.username,
        _ => true,
    };
    protocol_matches && endpoint_matches && username_matches
}

/// 旧凭据 → 本次提交计划（OPT-014 R1 纯函数收口；OPT-006 增身份/密码分离）。
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
    expected_generation: i64,
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
    let (identity_changed, password_changed, old_was_empty) = match &old {
        Some((old_p, old_ep, old_user, old_pw)) => {
            // 身份定义（OPT-006）：协议 + 规范化服务器根 + 用户名。
            // 密码**不属于身份**——同账号换密码保留数据与历史。
            let identity = old_p != protocol
                || normalized_server_root(old_ep) != normalized_server_root(&endpoint)
                || old_user != &username;
            // 二者互斥：身份已变时 "同账号改密码" 不存在（整包按换号处理）。
            (identity, !identity && old_pw != &password, false)
        }
        None => (false, false, true),
    };
    // R1 P1（后端强制，非 UI 文案）：空密码只允许**同一身份**复用旧密码。
    // 新身份（地址/用户名/协议变化）时旧密码绝不随请求发给新服务——在 HTTP
    // 之前就拒绝，从根上封死「新地址 + 旧密码」的 F05 拼接路径。
    if typed_pw.is_empty() && identity_changed {
        return Err(AppError::new(
            "validate",
            "更换后端地址/用户名/协议时必须重新输入密码（不能复用旧账号密码）",
        ));
    }
    Ok(SavePlan {
        protocol: protocol.to_string(),
        endpoint,
        username,
        password,
        identity_changed,
        password_changed,
        old_was_empty,
        expected_generation,
        activate_pending: None,
        password_supplied: !typed_pw.is_empty(),
    })
}

/// 账号提交事务内核（OPT-014 R1；OPT-006 代际与身份边界）：换号清理 +
/// 凭据/游标落库 + 代际推进在**同一短事务**内全有全无——任一步失败
/// （含密码 DPAPI 加密）整体回滚，旧账号配置/订阅/队列原样。
/// 返回清理后仍未绑定的本地直连源数（首连弹窗判定输入）。
///
/// OPT-006 语义：
/// - 提交前复核 `plan.expected_generation`（保存开始验证时捕获）——两个保存
///   并发时迟到者整体失败，不覆盖较新保存；
/// - `identity_changed`（协议/规范化根/用户名变化）→ 清远端数据、清旧账号
///   退订墓碑（不污染新账号）、重置两端游标 + Fever 历史回 Unknown、清端点缓存；
/// - 仅密码变化（同账号）→ 保留全部数据与历史进度，只推进代际使旧请求失效；
/// - 待确认建议只在**专用激活提交**（`plan.activate_pending`）里消费：与当前
///   建议做版本/身份 CAS，命中才清除；普通保存（None）绝不触碰建议；CAS 不匹配
///   整体拒绝（旧 active 配置保持原样）。
///
/// `encrypt` 注入仅本模块单测使用（生产恒为 `credentials::encrypt_secret`）；
/// 不是 IPC 命令参数，webview 无法触达。
// Note: 提交前复核代际/身份-密码分离/激活建议 CAS — 见 .agents/notes/implemented/architecture/2026-10-08-账号会话与配置应用边界.md
fn commit_account_settings(
    conn: &mut rusqlite::Connection,
    plan: &SavePlan,
    resolved_base: &str,
    encrypt: fn(&str) -> AppResult<String>,
) -> AppResult<i64> {
    let tx = conn.transaction()?;
    // 并发保存防覆盖：提交前复核开始验证时捕获的代际。
    let current_generation = db::sync_generation(&tx)?;
    if current_generation != plan.expected_generation {
        return Err(AppError::new(
            "staleAccountSave",
            "账号配置已在其他窗口变更，本次保存未写入（请刷新后重试）",
        ));
    }
    if plan.identity_changed {
        let (feeds, _) = db::purge_remote_data_in(&tx)?;
        log::info!("sync: 账号切换，清理旧账号数据：{feeds} 个订阅");
        // 旧账号的删除墓碑不压制新账号订阅导入（F05；断开不调用，保防复活）。
        db::clear_feed_tombstones(&tx)?;
        crate::endpoint_resolve::clear_cached_base(&tx)?;
        // 新账号的远端 id/游标命名空间不同：两端游标与 Fever 历史全部归零/Unknown。
        db::set_setting_with(&tx, "sync_last_sync", "0", encrypt)?;
        db::set_setting_with(&tx, "sync_last_entry_id", "0", encrypt)?;
        db::reset_fever_history_state(&tx)?;
    } else if plan.old_was_empty {
        // 首连（此前无凭据）：游标/历史未知状态归零，从全量开始。
        db::set_setting_with(&tx, "sync_last_sync", "0", encrypt)?;
        db::set_setting_with(&tx, "sync_last_entry_id", "0", encrypt)?;
        db::reset_fever_history_state(&tx)?;
    } else if plan.password_changed {
        // 同账号仅密码变化：数据/游标/Fever 历史进度**保留**（不清内容、不重置
        // 历史）；代际推进在下方统一做——旧请求仍失效（OPT-006 明确边界）。
        log::info!("sync: 同账号凭据更新，保留本地数据与历史进度（代际推进使旧请求失效）");
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
    // OPT-006 R1：待确认建议的消费只发生在专用激活提交里（携带激活时的版本号）：
    // 与当前建议做版本 + 身份的 CAS，命中才清除；不匹配**整体拒绝**（活动配置
    // 保持原样、新建议保持）——迟到激活不得覆盖/消费在 HTTP 期间新导入的建议。
    if let Some(expected_version) = plan.activate_pending {
        let current = crate::config_sync::read_pending_suggestion(&tx)?;
        let matched = matches!(
            &current,
            Some(s) if s.version == expected_version
                && suggestion_matches_plan(&s.connection, plan)
        );
        if !matched {
            return Err(AppError::new(
                "staleActivation",
                "待确认连接建议已在保存过程中变更（版本或内容不匹配），本次激活未写入，当前连接未改变",
            ));
        }
        crate::config_sync::clear_pending_connection(&tx)?;
    }
    // 普通保存（activate_pending=None）绝不触碰建议——无关保存不得清理它。
    // 推进代际：所有既有会话（含仅密码更新场景的在途请求）自此失效。
    db::bump_sync_generation(&tx)?;
    tx.commit()?;
    Ok(unbound_local)
}

/// 断开提交事务内核（OPT-014 R1；OPT-006 代际）：服务端数据清理 + 凭据/游标
/// 清空 + Fever 历史回 Unknown + 端点缓存清除 + 代际推进在同一短事务内全有全无。
/// `encrypt` 注入仅本模块单测使用。
fn commit_disconnect(
    conn: &mut rusqlite::Connection,
    expected_generation: i64,
    encrypt: fn(&str) -> AppResult<String>,
) -> AppResult<(usize, usize)> {
    let tx = conn.transaction()?;
    // 与保存同规则：提交前复核代际，避免与并发保存交错出半套状态。
    let current_generation = db::sync_generation(&tx)?;
    if current_generation != expected_generation {
        return Err(AppError::new(
            "staleAccountSave",
            "账号配置已在其他窗口变更，断开未执行（请刷新后重试）",
        ));
    }
    let r = db::purge_remote_data_in(&tx)?;
    db::set_setting_with(&tx, "greader_endpoint", "", encrypt)?;
    db::set_setting_with(&tx, "greader_password", "", encrypt)?;
    db::set_setting_with(&tx, "greader_username", "", encrypt)?;
    db::set_setting_with(&tx, "sync_last_sync", "0", encrypt)?;
    // OPT-006：Fever 历史进度同步回 Unknown（不能只重置 GR 时间戳而让下一个
    // 账号继承旧 Complete/页游标）；端点缓存一并清除。墓碑保留：重连同一
    // 账号时仍要防「已删订阅复活」（TASK-055）。
    db::set_setting_with(&tx, "sync_last_entry_id", "0", encrypt)?;
    db::reset_fever_history_state(&tx)?;
    crate::endpoint_resolve::clear_cached_base(&tx)?;
    db::bump_sync_generation(&tx)?;
    tx.commit()?;
    Ok(r)
}

/* ============================================================
OPT-006 集成测试入口（非 IPC；屏障测试组合真实的账号切换事务）
============================================================ */

/// doc(hidden) 集成测试入口：走与 `sync_save` 完全相同的规划 + 提交内核
/// （不含 HTTP 验证——测试用本地 mock 服务替代）。返回新代际。
#[doc(hidden)]
pub fn save_account_for_test(
    conn: &mut rusqlite::Connection,
    protocol: &str,
    endpoint: &str,
    username: &str,
    password: &str,
    resolved_base: &str,
) -> AppResult<i64> {
    let protocol = if protocol == "fever" {
        "fever"
    } else {
        "greader"
    }
    .to_string();
    let old = crate::sync::read_credentials(conn)?;
    let generation = db::sync_generation(conn)?;
    let plan = resolve_save_plan(Ok(old), &protocol, endpoint, username, password, generation)?;
    commit_account_settings(
        conn,
        &plan,
        resolved_base,
        crate::credentials::encrypt_secret,
    )?;
    db::sync_generation(conn)
}

/// doc(hidden) 集成测试入口：断开走生产同一事务内核。返回 (清理订阅数, 清文章数)。
#[doc(hidden)]
pub fn disconnect_for_test(conn: &mut rusqlite::Connection) -> AppResult<(usize, usize)> {
    let generation = db::sync_generation(conn)?;
    commit_disconnect(conn, generation, crate::credentials::encrypt_secret)
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
    // OPT-006：入队时同一次持锁捕获代际——随后推送（feeds_phase_at）要求代际
    // 未变，防止「入队后账号被切换」把本地订阅自动推给新账号（首连推送必须
    // 经用户确认，不能绕过 SyncTab 的 firstConnect 流程）。
    let (queued, generation) = {
        let conn = state.db.lock().await;
        // OPT-014 R1：凭据读取失败 → Err（不得当未连接静默返回）
        if !sync_configured(&conn)? {
            return Err(AppError::new("notConnected", "未连接后端"));
        }
        let generation = db::sync_generation(&conn)?;
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
        (n, generation)
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
    // OPT-006：要求代际仍是入队时捕获的值——入队后账号被切换则整体作废，
    // 不把本地订阅推给新账号（见上方捕获点注释）。
    let report = crate::sync::feeds_phase_at(&state.db, &state.http, generation).await?;
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
/// OPT-006：同事务重置 Fever 历史为 Unknown、清端点缓存并推进代际——断开即刻
/// 使所有在途同步会话失效（旧响应不得复活旧内容，也不得写入后续游标）。
#[tauri::command]
pub async fn sync_disconnect(state: State<'_, AppState>) -> AppResult<String> {
    let (feeds, articles) = {
        let mut conn = state.db.lock().await;
        let generation = db::sync_generation(&conn)?;
        commit_disconnect(&mut conn, generation, crate::credentials::encrypt_secret)?
    };
    Ok(format!(
        "已断开并清理：移除 {feeds} 个服务端订阅（{articles} 处绑定），本地直连订阅保留"
    ))
}

/* ============================================================
配置导入的待确认连接建议（OPT-006 / F05）
============================================================ */

/// 读取配置导入留下的待确认连接建议（非敏感：版本号 + 协议/地址/用户名，无密码）。
/// 前端据此展示「需要重新输入凭据后保存才会激活」的入口；激活时必须把这里的
/// `version` 原样回传给 `sync_save`（后端独立做 CAS，不信任 UI）。
#[tauri::command]
pub async fn sync_pending_connection(
    state: State<'_, AppState>,
) -> AppResult<Option<crate::config_sync::PendingSuggestion>> {
    let conn = state.db.lock().await;
    crate::config_sync::read_pending_suggestion(&conn)
}

/// 放弃待确认连接建议（不触碰活动凭据/绑定）。激活入口仍走 sync_save。
#[tauri::command]
pub async fn sync_dismiss_pending_connection(state: State<'_, AppState>) -> AppResult<()> {
    let conn = state.db.lock().await;
    crate::config_sync::clear_pending_connection(&conn)
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
    /// `identity_changed` 与 `password_changed` 互斥；expected_generation 默认按
    /// 当前库代际捕获（测试内可显式构造过期计划）。
    fn plan(
        identity_changed: bool,
        protocol: &str,
        endpoint: &str,
        user: &str,
        pw: &str,
    ) -> SavePlan {
        SavePlan {
            protocol: protocol.to_string(),
            endpoint: endpoint.to_string(),
            username: user.to_string(),
            password: pw.to_string(),
            identity_changed,
            password_changed: !identity_changed,
            old_was_empty: false,
            expected_generation: 0,
            activate_pending: None,
            password_supplied: true,
        }
    }

    /// 按当前库代际构造计划（正常提交路径）。
    fn plan_now(
        conn: &rusqlite::Connection,
        identity_changed: bool,
        protocol: &str,
        endpoint: &str,
        user: &str,
        pw: &str,
    ) -> SavePlan {
        SavePlan {
            expected_generation: db::sync_generation(conn).unwrap(),
            ..plan(identity_changed, protocol, endpoint, user, pw)
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

        let commit_plan = plan_now(
            &conn,
            true,
            "fever",
            "http://new.example",
            "new-user",
            "new-pw",
        );
        let err =
            commit_account_settings(&mut conn, &commit_plan, "http://new.example/api", |_| {
                Err(AppError::new("credentialEncrypt", "注入的 DPAPI 失败"))
            })
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

        let commit_plan = plan_now(
            &conn,
            true,
            "fever",
            "http://new.example",
            "new-user",
            "new-pw",
        );
        let unbound = commit_account_settings(
            &mut conn,
            &commit_plan,
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
        assert_eq!(
            db::sync_generation(&conn).unwrap(),
            1,
            "每次成功提交必须推进代际"
        );
    }

    /// OPT-006：同账号仅改密码——数据保留、Fever 历史进度保留、代际推进。
    #[test]
    fn password_only_update_keeps_data_and_history_and_bumps_generation() {
        let mut conn = conn();
        seed_old_account(&conn);
        seed_remote(&conn);
        db::set_fever_history_pending(&conn, 76).unwrap();
        db::set_last_sync_ts(&conn, 1234).unwrap();
        db::set_setting(
            &conn,
            "feed_tombstones",
            r#"["http://old.example/gone.xml"]"#,
        )
        .unwrap();

        let commit_plan = plan_now(
            &conn,
            false,
            "greader",
            "http://old.example",
            "old-user",
            "new-pw",
        );
        commit_account_settings(
            &mut conn,
            &commit_plan,
            "http://old.example",
            crate::credentials::encrypt_secret,
        )
        .unwrap();

        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM feeds WHERE origin = 'remote'"),
            1,
            "同账号仅改密码不得清远端订阅"
        );
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM sync_queue"),
            1,
            "队列保留"
        );
        assert_eq!(
            db::last_sync_ts(&conn).unwrap(),
            1234,
            "同账号历史进度（时间戳游标）保留"
        );
        assert_eq!(
            db::fever_history_state(&conn).unwrap(),
            db::FeverHistoryState::Pending(76),
            "同账号 Fever 历史进度保留"
        );
        assert_eq!(
            db::get_setting(&conn, "greader_password").unwrap().unwrap(),
            "new-pw"
        );
        assert_eq!(
            db::sync_generation(&conn).unwrap(),
            1,
            "密码更新仍推进代际使旧请求失效"
        );
        assert!(
            !db::feed_tombstones(&conn).unwrap().is_empty(),
            "同账号换密码不清删除墓碑"
        );
    }

    /// OPT-006：身份变化（协议切换）——清远端数据/绑定/队列、重置两端游标与
    /// Fever 历史、清旧账号删除墓碑、清端点缓存、推进代际。
    #[test]
    fn identity_change_resets_progress_and_clears_old_account_tombstones() {
        let mut conn = conn();
        seed_old_account(&conn);
        seed_remote(&conn);
        db::set_fever_history_complete(&conn).unwrap();
        db::set_last_sync_ts(&conn, 4321).unwrap();
        db::set_last_sync_entry_id(&conn, 987).unwrap();
        db::set_setting(
            &conn,
            "feed_tombstones",
            r#"["http://old.example/gone.xml"]"#,
        )
        .unwrap();
        crate::endpoint_resolve::remember_base(
            &conn,
            "greader",
            "http://old.example",
            "http://old.example",
        )
        .unwrap();
        assert!(
            crate::endpoint_resolve::cached_base(&conn, "greader", "http://old.example").is_some()
        );

        let commit_plan = plan_now(
            &conn,
            true,
            "fever",
            "http://new.example",
            "new-user",
            "new-pw",
        );
        commit_account_settings(
            &mut conn,
            &commit_plan,
            "http://new.example",
            crate::credentials::encrypt_secret,
        )
        .unwrap();

        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM feeds WHERE origin = 'remote'"),
            0,
            "换号清远端订阅"
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM articles WHERE remote_id IS NOT NULL"
            ),
            0,
            "换号清绑定"
        );
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM sync_queue"),
            0,
            "换号清队列"
        );
        assert_eq!(db::last_sync_ts(&conn).unwrap(), 0, "时间戳游标重置");
        assert_eq!(
            db::last_sync_entry_id(&conn).unwrap(),
            0,
            "Fever 条目游标重置"
        );
        assert_eq!(
            db::fever_history_state(&conn).unwrap(),
            db::FeverHistoryState::Unknown,
            "Fever 历史状态回 Unknown（不继承旧 Complete/页游标）"
        );
        assert!(
            db::feed_tombstones(&conn).unwrap().is_empty(),
            "旧账号删除墓碑不得压制新账号订阅导入"
        );
        assert!(
            crate::endpoint_resolve::cached_base(&conn, "greader", "http://old.example").is_none(),
            "旧端点缓存清除"
        );
        assert_eq!(db::sync_generation(&conn).unwrap(), 1);
    }

    /// OPT-006：两个保存并发——迟到的验证（旧代际计划）不得覆盖较新保存。
    #[test]
    fn concurrent_save_with_stale_generation_is_rejected_and_newer_state_kept() {
        let mut conn = conn();
        seed_old_account(&conn);
        seed_remote(&conn);

        // 保存 A 在验证前捕获代际 0（迟到者）
        let stale_plan = plan_now(
            &conn,
            true,
            "fever",
            "http://late.example",
            "late-user",
            "late-pw",
        );
        // 保存 B 先完成（代际 0 → 1）
        let winner_plan = plan_now(
            &conn,
            true,
            "greader",
            "http://winner.example",
            "win-user",
            "win-pw",
        );
        commit_account_settings(
            &mut conn,
            &winner_plan,
            "http://winner.example",
            crate::credentials::encrypt_secret,
        )
        .unwrap();
        // 保存 A 迟到提交：必须整体拒绝，不得覆盖 B
        let err = commit_account_settings(
            &mut conn,
            &stale_plan,
            "http://late.example",
            crate::credentials::encrypt_secret,
        )
        .expect_err("迟到保存必须被代际复核拒绝");
        assert_eq!(err.code, "staleAccountSave");

        assert_eq!(
            db::get_setting(&conn, "greader_endpoint").unwrap().unwrap(),
            "http://winner.example",
            "较新保存的地址必须保持"
        );
        assert_eq!(
            db::get_setting(&conn, "greader_username").unwrap().unwrap(),
            "win-user"
        );
        assert_eq!(
            db::get_setting(&conn, "greader_password").unwrap().unwrap(),
            "win-pw"
        );
        assert_eq!(
            db::sync_generation(&conn).unwrap(),
            1,
            "代际只被较新保存推进一次"
        );
    }

    /// R1：断开的清理与凭据清空在**同一短事务**——任一步失败整体回滚（远端数据/
    /// 旧配置不被半清）。
    #[test]
    fn disconnect_failure_rolls_back_cleanup_and_settings() {
        let mut conn = conn();
        seed_old_account(&conn);
        seed_remote(&conn);

        let failure: fn(&str) -> AppResult<String> =
            |_| Err(AppError::new("credentialEncrypt", "注入的 DPAPI 失败"));
        let err =
            commit_disconnect(&mut conn, 0, failure).expect_err("清空密码加密失败必须整体失败");
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
        assert_eq!(db::sync_generation(&conn).unwrap(), 0, "失败不得推进代际");
    }

    /// 断开成功：远端数据清理 + 凭据清空提交（本地直连保留由既有 e2e 锁）。
    /// OPT-006：同一事务重置 Fever 历史、清端点缓存并推进代际。
    #[test]
    fn disconnect_success_clears_and_purges() {
        let mut conn = conn();
        seed_old_account(&conn);
        seed_remote(&conn);
        db::set_fever_history_pending(&conn, 55).unwrap();
        db::set_setting(
            &conn,
            "feed_tombstones",
            r#"["http://old.example/gone.xml"]"#,
        )
        .unwrap();
        crate::endpoint_resolve::remember_base(
            &conn,
            "greader",
            "http://old.example",
            "http://old.example",
        )
        .unwrap();

        let (feeds, _) =
            commit_disconnect(&mut conn, 0, crate::credentials::encrypt_secret).unwrap();
        assert_eq!(feeds, 1);
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM feeds WHERE origin = 'remote'"),
            0
        );
        assert!(matches!(crate::sync::read_credentials(&conn), Ok(None)));
        assert_eq!(
            db::fever_history_state(&conn).unwrap(),
            db::FeverHistoryState::Unknown,
            "断开必须把 Fever 历史状态回 Unknown"
        );
        assert!(
            crate::endpoint_resolve::cached_base(&conn, "greader", "http://old.example").is_none(),
            "断开清端点缓存"
        );
        assert_eq!(db::sync_generation(&conn).unwrap(), 1, "断开推进代际");
        assert!(
            !db::feed_tombstones(&conn).unwrap().is_empty(),
            "断开不清删除墓碑（重连同一账号仍须防复活）"
        );
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

        commit_disconnect(&mut conn, 0, crate::credentials::encrypt_secret).unwrap();
        assert!(matches!(crate::sync::read_credentials(&conn), Ok(None)));

        let commit_plan = plan_now(
            &conn,
            false,
            "greader",
            "http://new.example",
            "new-user",
            "new-pw",
        );
        commit_account_settings(
            &mut conn,
            &commit_plan,
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
            0,
        )
        .unwrap_err();
        assert_eq!(err.code, "credentialDecrypt");
    }

    #[test]
    fn save_plan_blank_password_needs_existing_credentials() {
        let err = resolve_save_plan(Ok(None), "greader", "http://x", "u", "", 0).unwrap_err();
        assert_eq!(err.code, "validate");
        assert_eq!(err.message, "请填写密码");
    }

    #[test]
    fn save_plan_first_connect_flags_and_trim() {
        let p = resolve_save_plan(Ok(None), "greader", " http://x ", " u ", " pw ", 7).unwrap();
        assert_eq!(p.endpoint, "http://x");
        assert_eq!(p.username, "u");
        assert_eq!(p.password, "pw");
        assert!(p.old_was_empty && !p.identity_changed && !p.password_changed);
        assert_eq!(p.expected_generation, 7, "代际随计划捕获");
    }

    /// R2 ①：规范化服务器根必须保留 **scheme/host/有效端口/basepath**——只消除
    /// 等价默认端口（http:80 / https:443）与尾斜杠；同 host 不同 port 是不同身份，
    /// 空密码必须拒绝（防旧密码发给另一台服务）。
    #[test]
    fn save_plan_identity_keeps_port_and_basepath() {
        let old = Some((
            "greader".to_string(),
            "http://h.example:8080".to_string(),
            "user-a".to_string(),
            "pw-a".to_string(),
        ));
        // 仅端口不同 + 空密码 → 拒绝（RED 判别：修前被当同身份复用旧密码）
        assert!(
            resolve_save_plan(
                Ok(old.clone()),
                "greader",
                "http://h.example:8081",
                "user-a",
                "",
                0
            )
            .is_err(),
            "同 host 不同 port 必须按换号处理"
        );
        // basepath 不同 → 拒绝
        assert!(
            resolve_save_plan(
                Ok(old.clone()),
                "greader",
                "http://h.example:8080/other",
                "user-a",
                "",
                0
            )
            .is_err(),
            "不同 basepath 必须按换号处理"
        );
        // 显式新密码时允许（正常换号），且 password_supplied 标记为真
        let p = resolve_save_plan(
            Ok(old),
            "greader",
            "http://h.example:8081",
            "user-a",
            "pw-new",
            0,
        )
        .unwrap();
        assert!(p.identity_changed && p.password_supplied);

        // 等价默认端口：显式 :80 与省略等价（http）；https:443 同理
        let old80 = Some((
            "greader".to_string(),
            "http://h.example:80".to_string(),
            "user-a".to_string(),
            "pw-a".to_string(),
        ));
        let ok =
            resolve_save_plan(Ok(old80), "greader", "http://H.example", "user-a", "", 0).unwrap();
        assert!(!ok.identity_changed, "显式默认端口与省略必须等价");
        assert!(!ok.password_supplied, "空密码标记为未提供");
        let old443 = Some((
            "greader".to_string(),
            "https://h.example:443/a".to_string(),
            "user-a".to_string(),
            "pw-a".to_string(),
        ));
        let ok2 = resolve_save_plan(
            Ok(old443),
            "greader",
            "https://h.example/a/",
            "user-a",
            "",
            0,
        )
        .unwrap();
        assert!(!ok2.identity_changed, "https 默认端口 + 尾斜杠必须等价");
    }

    /// R2 ①：显式激活即使身份相同也必须重新输入密码（plan 侧标记）。
    #[test]
    fn save_plan_marks_password_supplied_for_activation_gate() {
        let old = Some((
            "greader".to_string(),
            "http://a.example".to_string(),
            "user-a".to_string(),
            "pw-a".to_string(),
        ));
        let blank = resolve_save_plan(
            Ok(old.clone()),
            "greader",
            "http://a.example",
            "user-a",
            "",
            0,
        )
        .unwrap();
        assert!(!blank.password_supplied);
        let fresh =
            resolve_save_plan(Ok(old), "greader", "http://a.example", "user-a", "pw-x", 0).unwrap();
        assert!(fresh.password_supplied);
    }

    /// OPT-006 R1 P1：空密码只允许**同一身份**复用旧密码；新身份（协议/规范化
    /// 服务器根/用户名任一变化）必须先重新输入密码——后端在 HTTP 前拒绝，
    /// 绝不把旧账号密码发给新地址（防「新地址 + 旧密码」拼接）。
    #[test]
    fn save_plan_blank_password_rejects_new_identity() {
        let old = Some((
            "greader".to_string(),
            "http://a.example".to_string(),
            "user-a".to_string(),
            "pw-a".to_string(),
        ));
        // 地址变化
        let err = resolve_save_plan(
            Ok(old.clone()),
            "greader",
            "http://b.example",
            "user-a",
            "",
            0,
        )
        .unwrap_err();
        assert_eq!(err.code, "validate");
        assert!(
            err.message.contains("密码"),
            "错误应指明需重新输入密码：{err}"
        );
        // 用户名变化
        assert!(resolve_save_plan(
            Ok(old.clone()),
            "greader",
            "http://a.example",
            "user-b",
            "",
            0
        )
        .is_err());
        // 协议变化
        assert!(resolve_save_plan(
            Ok(old.clone()),
            "fever",
            "http://a.example",
            "user-a",
            "",
            0
        )
        .is_err());
        // 尾斜杠/主机大小写差异仍算同一身份（规范化服务器根）→ 允许复用
        let ok =
            resolve_save_plan(Ok(old), "greader", "http://A.example/", "user-a", "", 0).unwrap();
        assert_eq!(ok.password, "pw-a");
        assert!(!ok.identity_changed);
    }

    /// OPT-006 R1 P2：普通保存（同账号只改密码）不得清理待确认连接建议——
    /// 建议的消费只发生在携带 activation 版本的专用激活提交里。
    #[test]
    fn normal_save_preserves_pending_suggestion() {
        let mut conn = conn();
        seed_old_account(&conn);
        let suggestion = crate::config_sync::ConnectionConfig {
            sync_protocol: Some("greader".into()),
            greader_endpoint: Some("http://b.example".into()),
            greader_username: Some("user-b".into()),
        };
        let _ = crate::config_sync::store_pending_connection(&conn, &suggestion).unwrap();

        save_account_for_test(
            &mut conn,
            "greader",
            "http://old.example",
            "old-user",
            "new-pw",
            "http://old.example",
        )
        .unwrap();

        let raw = db::get_setting(&conn, "pending_connection_config")
            .unwrap()
            .unwrap_or_default();
        assert!(
            raw.contains("user-b"),
            "无关保存（同账号改密码）不得清掉待确认建议：{raw}"
        );
    }

    fn suggestion(
        protocol: &str,
        endpoint: &str,
        user: &str,
    ) -> crate::config_sync::ConnectionConfig {
        crate::config_sync::ConnectionConfig {
            sync_protocol: Some(protocol.into()),
            greader_endpoint: Some(endpoint.into()),
            greader_username: Some(user.into()),
        }
    }

    /// R1 P2：激活提交的 CAS——携带版本与当前建议不符（期间被新导入替换）/
    /// 身份不一致 / 无建议，一律**整体拒绝**：活动配置保持原样、当前建议保持
    /// （新 C 绝不被旧 B 的迟到激活消费）、代际不推进。
    #[test]
    fn activation_commit_rejects_stale_version_and_keeps_newer_suggestion() {
        let mut conn = conn();
        seed_old_account(&conn);
        let v_b = crate::config_sync::store_pending_connection(
            &conn,
            &suggestion("greader", "http://b.example", "user-b"),
        )
        .unwrap();
        // 期间新导入 C 覆盖建议（版本 +1）
        let v_c = crate::config_sync::store_pending_connection(
            &conn,
            &suggestion("greader", "http://c.example", "user-c"),
        )
        .unwrap();
        assert!(v_c > v_b, "每次导入必须换新版本");

        // ① 迟到 B 激活（旧版本）：整体拒绝，不得消费 C
        let mut late = plan_now(&conn, true, "greader", "http://b.example", "user-b", "pw-b");
        late.activate_pending = Some(v_b);
        let err = commit_account_settings(
            &mut conn,
            &late,
            "http://b.example",
            crate::credentials::encrypt_secret,
        )
        .expect_err("迟到激活必须整体拒绝");
        assert_eq!(err.code, "staleActivation");
        assert_eq!(
            db::get_setting(&conn, "greader_endpoint").unwrap().unwrap(),
            "http://old.example",
            "拒绝时活动配置必须保持原样"
        );
        let raw = db::get_setting(&conn, "pending_connection_config")
            .unwrap()
            .unwrap_or_default();
        assert!(raw.contains("c.example"), "新建议 C 不得被消费：{raw}");
        assert_eq!(db::sync_generation(&conn).unwrap(), 0, "拒绝不得推进代际");

        // ② 身份不一致（保存目标 D，token 指向 C 的建议）同样拒绝
        let mut mismatch = plan_now(&conn, true, "greader", "http://d.example", "user-d", "pw-d");
        mismatch.activate_pending = Some(v_c);
        assert_eq!(
            commit_account_settings(
                &mut conn,
                &mismatch,
                "http://d.example",
                crate::credentials::encrypt_secret
            )
            .unwrap_err()
            .code,
            "staleActivation"
        );
        assert!(db::get_setting(&conn, "pending_connection_config")
            .unwrap()
            .unwrap_or_default()
            .contains("c.example"));

        // ③ 无建议时携带 token：拒绝
        crate::config_sync::clear_pending_connection(&conn).unwrap();
        let mut none_token = plan_now(&conn, true, "greader", "http://c.example", "user-c", "pw-c");
        none_token.activate_pending = Some(v_c);
        assert_eq!(
            commit_account_settings(
                &mut conn,
                &none_token,
                "http://c.example",
                crate::credentials::encrypt_secret
            )
            .unwrap_err()
            .code,
            "staleActivation"
        );
    }

    /// R1 P2 正例：版本与身份都匹配时才消费，激活提交正常落地。
    #[test]
    fn activation_commit_consumes_when_version_and_identity_match() {
        let mut conn = conn();
        seed_old_account(&conn);
        let v = crate::config_sync::store_pending_connection(
            &conn,
            &suggestion("greader", "http://b.example", "user-b"),
        )
        .unwrap();
        let mut plan = plan_now(&conn, true, "greader", "http://b.example", "user-b", "pw-b");
        plan.activate_pending = Some(v);
        commit_account_settings(
            &mut conn,
            &plan,
            "http://b.example",
            crate::credentials::encrypt_secret,
        )
        .unwrap();
        assert_eq!(
            db::get_setting(&conn, "greader_endpoint").unwrap().unwrap(),
            "http://b.example"
        );
        assert!(
            crate::config_sync::read_pending_suggestion(&conn)
                .unwrap()
                .is_none(),
            "CAS 命中才消费建议"
        );
        assert_eq!(db::sync_generation(&conn).unwrap(), 1);
    }

    /// 密码留空复用旧密码（同身份）；用户名留空回落旧值、显式输入以本次为准（P3[3] 语义保持）。
    /// OPT-006：仅尾斜杠/大小写差异不算换号；密码变化 ≠ 身份变化。
    /// R1：用户名变化属于换号——空密码路径已被拒绝（见
    /// `save_plan_blank_password_rejects_new_identity`），本用例以显式新密码验证优先级。
    #[test]
    fn save_plan_reuses_password_and_username_when_blank() {
        let old = Some((
            "greader".to_string(),
            "http://Old.example/".to_string(),
            "old-user".to_string(),
            "old-pw".to_string(),
        ));
        let p =
            resolve_save_plan(Ok(old.clone()), "greader", "http://old.example", "", "", 0).unwrap();
        assert_eq!(p.endpoint, "http://old.example");
        assert_eq!(p.username, "old-user");
        assert_eq!(p.password, "old-pw");
        assert!(
            !p.identity_changed,
            "仅尾斜杠/主机大小写差异不算换号（规范化服务器根）"
        );
        assert!(!p.password_changed);

        let p2 = resolve_save_plan(
            Ok(old),
            "greader",
            "http://old.example",
            "new-user",
            "new-pw",
            0,
        )
        .unwrap();
        assert_eq!(p2.username, "new-user", "显式用户名不被旧值覆盖");
        assert!(p2.identity_changed, "用户名变化 = 换号");
    }

    /// OPT-006：同账号改密码 → password_changed=true 且 identity_changed=false。
    #[test]
    fn save_plan_password_change_is_not_identity_change() {
        let old = Some((
            "greader".to_string(),
            "http://old.example".to_string(),
            "old-user".to_string(),
            "old-pw".to_string(),
        ));
        let p = resolve_save_plan(
            Ok(old),
            "greader",
            "http://old.example",
            "old-user",
            "new-pw",
            3,
        )
        .unwrap();
        assert!(p.password_changed, "密码变化必须标记（推进代际）");
        assert!(!p.identity_changed, "密码不属于身份——不得清数据");
        assert_eq!(p.expected_generation, 3);
    }

    #[test]
    fn save_plan_full_new_account_marks_changed() {
        let old = Some((
            "greader".to_string(),
            "http://old.example".to_string(),
            "old-user".to_string(),
            "old-pw".to_string(),
        ));
        let p = resolve_save_plan(
            Ok(old),
            "fever",
            "http://new.example",
            "new-user",
            "new-pw",
            0,
        )
        .unwrap();
        assert!(p.identity_changed && !p.old_was_empty && !p.password_changed);
    }
}
