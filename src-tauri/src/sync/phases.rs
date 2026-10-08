//! sync 的 phases 子模块（TASK-045 从 sync.rs 按既有章节拆分）。

use super::*;
use crate::db;
use crate::error::{AppError, AppResult};
use crate::fever;
use crate::greader::GReaderClient;
use rusqlite::Connection;
use std::sync::Arc;
use tokio::sync::Mutex;

/// feeds 阶段（订阅层）：push_feeds + pull_feeds。秒级，首连先跑这段。
/// 锁纪律：HTTP 全在锁外；DB 读写在锁内短临界区完成。
/// TASK-124：feeds 阶段的构建失败不记录不发事件（记录/呈现职责在 states 段与
/// 即时推送，见 push.rs record_push_block）——「未配置」与「失败」对订阅层同步
/// 的既有行为一致（都是中止），两类失败映射回同一 notConnected 错误。
pub async fn feeds_phase(
    db: &Arc<Mutex<Connection>>,
    http: &reqwest::Client,
) -> AppResult<SyncReport> {
    let client = build_client(db, http).await.map_err(|_| {
        AppError::new(
            "notConnected",
            "未配置同步后端（Google Reader / Fever 凭据）",
        )
    })?;
    let mut report = SyncReport::default();
    push_feeds(db, &client, &mut report).await;
    pull_feeds(db, &client, &mut report).await;
    Ok(report)
}

/// states 阶段（状态+条目层）：push 队列 + pull entries。
/// `full=true` 含绑定回填+全量对账（手动同步/首连）；false 只做增量（后台自动同步）。
pub async fn states_phase(
    db: &Arc<Mutex<Connection>>,
    http: &reqwest::Client,
    full: bool,
) -> AppResult<SyncReport> {
    let client = match build_client(db, http).await {
        Ok(c) => c,
        Err(ClientBuildFailure::NotConfigured) => {
            return Err(AppError::new(
                "notConnected",
                "未配置同步后端（Google Reader / Fever 凭据）",
            ));
        }
        // TASK-124（审计 P2-6②）：有凭据但认证/端点/网络失败——记录阻塞标记
        // （attempts/last_error 此前对这类失败永不记录）+ 发 sync-queue-changed。
        // 返回错误保持既有 notConnected 形态：手动同步的「未连接静默跳过」前端
        // 语义（isNotConnectedError）不因本卡改变；可见性由 pill/摘要卡承担。
        Err(ClientBuildFailure::Failed(e)) => {
            record_push_block(db, &e).await;
            return Err(AppError::new(
                "notConnected",
                "未配置同步后端（Google Reader / Fever 凭据）",
            ));
        }
    };
    let mut report = SyncReport::default();
    // 推送段进 PUSH_LOCK（与 push_states_now/feeds_phase 的推送互斥，防 prune 竞态）
    {
        let _guard = PUSH_LOCK.lock().await;
        let (plan, stale) = {
            let conn = db.lock().await;
            let stale = age_stale_queue(&conn, &mut report);
            (plan_push(&conn)?, stale)
        };
        let (done, failed) = exec_push(&client, &plan, &mut report).await;
        {
            let conn = db.lock().await;
            // TASK-116：失败项落库标记（attempts+1 / last_error，四态展示口径）
            if !failed.is_empty() {
                if let Err(err) = db::mark_push_failed(&conn, &failed) {
                    log::warn!("sync: 推送失败标记落库失败（states 首次 push）: {err}");
                }
            }
            // P3[1]：剪除失败此前静默 → 队项残留会被重复推送。改为 warn。
            if !done.is_empty() {
                if let Err(err) = db::prune_sync(&conn, &done) {
                    log::warn!("sync: 剪除已推送队项失败（states 首次 push）: {err}");
                }
            }
        }
        // TASK-124：推送确认/失败后发事件（同 push_states_now 口径：成功出队
        // waiting 降、失败标记 failed/last_error 升；仅老化清理也发——waiting 已降）。
        // 统计读在锁外短临界区（notify_queue_changed 内部自持短锁）。
        if !done.is_empty() || !failed.is_empty() || stale > 0 {
            notify_queue_changed(db).await;
        }
    }
    pull_entries(db, &client, &mut report, full).await;
    // pull 后补推：pull 会为「本地已读但后端刚抓取成功的文章」绑定
    // remote_id（此前未绑定，push 段跳过）。绑定后再推一次，把它们的
    // pending read/star 推到后端——否则这些文章要等下一轮同步才同步状态，
    // 其他客户端会看到「本地已读、后端仍未读」。二次 push 幂等（队列已空则无操作）。
    {
        let _guard = PUSH_LOCK.lock().await;
        let (plan, stale) = {
            let conn = db.lock().await;
            let stale = age_stale_queue(&conn, &mut report);
            (plan_push(&conn)?, stale)
        };
        if !plan.status.is_empty() || !plan.stars.is_empty() {
            let (done, failed) = exec_push(&client, &plan, &mut report).await;
            {
                let conn = db.lock().await;
                // TASK-116：失败项落库标记（pull 后补推段，同首次 push 段口径）
                if !failed.is_empty() {
                    if let Err(err) = db::mark_push_failed(&conn, &failed) {
                        log::warn!("sync: 推送失败标记落库失败（states 补推）: {err}");
                    }
                }
                // P3[1]：同上（pull 后补推段）。
                if !done.is_empty() {
                    if let Err(err) = db::prune_sync(&conn, &done) {
                        log::warn!("sync: 剪除已推送队项失败（states 补推）: {err}");
                    }
                }
            }
            // TASK-124：补推段同口径发事件。
            if !done.is_empty() || !failed.is_empty() || stale > 0 {
                notify_queue_changed(db).await;
            }
        } else if stale > 0 {
            // TASK-124：补推段无可推项，但老化清理改变了统计 → 仍发事件。
            notify_queue_changed(db).await;
        }
    }
    Ok(report)
}

/// 完整同步（全量路径）：feeds 阶段 + states 阶段（full 对账）串联。
pub async fn sync_now(
    db: &Arc<Mutex<Connection>>,
    http: &reqwest::Client,
) -> AppResult<SyncReport> {
    let mut report = feeds_phase(db, http).await?;
    let states = states_phase(db, http, true).await?;
    report.pushed_states = states.pushed_states;
    report.pulled_entries = states.pulled_entries;
    report.errors.extend(states.errors);
    Ok(report)
}

/// 轻量同步（后台自动调度）：push 队列 + 增量 pull。
pub async fn sync_light(
    db: &Arc<Mutex<Connection>>,
    http: &reqwest::Client,
) -> AppResult<SyncReport> {
    states_phase(db, http, false).await
}

/// 测试连接（设置页「测试连接」按钮）。
/// 按协议分派：Google Reader 走 ClientLogin，Fever 走 `api_key` 认证。
/// 返回 (展示消息, 用户名, 解析出的 API 根)——用户名供 sync_save 落库做账号显示，
/// API 根供其写入解析缓存（TASK-059：后续同步不必重复探测）。
pub async fn test_connection(
    protocol: &str,
    endpoint: &str,
    username: &str,
    password: &str,
    http: &reqwest::Client,
) -> AppResult<(String, String, String)> {
    if endpoint.trim().is_empty() || username.trim().is_empty() || password.trim().is_empty() {
        return Err(AppError::new(
            "notConnected",
            "请先填写 Endpoint、用户名和密码",
        ));
    }
    // 两个协议都要先**解析端点**（用户只填域名时自动适配），再用解析出的地址拉订阅。
    let (subs, base) = match protocol {
        "fever" => {
            let client = fever::FeverClient::new(endpoint, username, password, http.clone())
                .resolve()
                .await?;
            (
                client.subscriptions().await?.len(),
                client.resolved_base().to_string(),
            )
        }
        _ => {
            let client = GReaderClient::login(endpoint, username, password, http.clone()).await?;
            (
                client.subscriptions().await?.len(),
                client.resolved_base().to_string(),
            )
        }
    };
    Ok((
        format!("已连接：{username}（{subs} 个订阅）"),
        username.to_string(),
        base,
    ))
}
