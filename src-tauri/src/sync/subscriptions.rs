//! sync 的 subscriptions 子模块（TASK-045 从 sync.rs 按既有章节拆分）。

use super::*;
use crate::db;
use crate::error::AppResult;
use crate::greader::{self};
use rusqlite::{Connection, OptionalExtension};
use std::sync::Arc;
use tokio::sync::Mutex;

/* ============================================================
OPT-009 B3：subscription-outbox-changed 事件总线
（订阅写回状态变化 → 前端持久回执，不依赖用户手动刷新设置页）
============================================================ */

/// 事件目标注册点（与 push.rs 的 QUEUE_EVENT_TARGET 同模式）：lib.rs setup 时
/// 注册一次；集成测试不注册 → 所有发射点 no-op（既有测试零影响）。
/// 为什么独立于 sync-queue-changed：payload 语义与消费面不同——订阅写回是
/// (pending/failed/local_only) 的账号限定计数，侧栏 pill 的「等待同步 N 条」
/// 是 article 状态队列口径；混流会让两套统计互相污染。payload 单点见
/// `db::subscription_outbox_changed_payload`。
static SUBSCRIPTION_EVENT_TARGET: std::sync::OnceLock<tauri::AppHandle> =
    std::sync::OnceLock::new();

/// 进程内上一次实际发射的订阅写回 payload（跨发射点同值去重，W2 复审 P3-2）：
/// 离线重试期「record 落账 + try_push 兜底」每 tick 会产生 2–4 个同值事件——
/// payload 未变不重发。消费方语义是「状态变化通知」，同值不刷新正确；账号切换
/// 的可见性由挂载/保存/断开后的主动刷新承担（B3 既有口径），不依赖事件。
static LAST_OUTBOX_PAYLOAD: std::sync::OnceLock<std::sync::Mutex<Option<serde_json::Value>>> =
    std::sync::OnceLock::new();

/// setup 时注册事件目标（lib.rs；幂等：重复注册以首次为准）。
pub fn init_subscription_event_target(app: tauri::AppHandle) {
    let _ = SUBSCRIPTION_EVENT_TARGET.set(app);
}

/// 订阅写回状态变化通知：发轻量 Tauri 事件 `subscription-outbox-changed`
/// （payload = 当前账号 `{pending, failed, local_only, last_error}`）。
///
/// 锁纪律：调用方必须在 **DB 锁外** 调用——本函数内部自己短持锁。
/// 静默规则（与 notify_queue_changed 同源）：未配置同步（无凭据）→ 不发事件；
/// 事件目标未注册（集成测试）→ no-op；账号/统计读取失败 → 发可重试
/// `sync-error` 并跳过统计通知（不编造零值冒充「无待办」）；与上次实际
/// 发射的 payload 同值 → 跳过
/// （W2 复审 P3-2 的跨发射点去重，见 [`LAST_OUTBOX_PAYLOAD`]）。
pub async fn notify_subscription_outbox_changed(db: &Arc<Mutex<Connection>>) {
    let Some(app) = SUBSCRIPTION_EVENT_TARGET.get() else {
        return;
    };
    let payload = {
        let conn = db.lock().await;
        match super::current_account(&conn) {
            // 当前账号限定：异账号暂停行不进入展示口径
            Ok(Some(ctx)) => match db::subscription_outbox_stats(&conn, &ctx.key) {
                Ok(stats) => db::subscription_outbox_changed_payload(&stats, true),
                Err(error) => {
                    emit_sync_error(&format!("订阅写回状态读取失败，请重试：{error}"));
                    None
                }
            },
            // 未配置：静默（不误报、不刷状态）
            Ok(None) => None,
            Err(e) => {
                log::warn!("sync: 订阅写回状态通知读取账号失败，跳过本轮通知: {e}");
                emit_sync_error(&format!("订阅写回账号读取失败，请重试：{e}"));
                None
            }
        }
    };
    let Some(p) = payload else {
        // None（未配置/读取失败）：清空缓存——重新配置后即使统计与上次相同
        // 也必须再发（摘要卡需要重新出现），不得用旧缓存吞掉连续性断点。
        if let Some(last) = LAST_OUTBOX_PAYLOAD.get() {
            *last.lock().unwrap() = None;
        }
        return;
    };
    let last = LAST_OUTBOX_PAYLOAD.get_or_init(|| std::sync::Mutex::new(None));
    let mut guard = last.lock().unwrap();
    if guard.as_ref() == Some(&p) {
        return;
    }
    use tauri::Emitter;
    if app.emit("subscription-outbox-changed", &p).is_ok() {
        *guard = Some(p);
    } else {
        crate::diagnostics::record(0, "subscriptions", "event_failed", None);
    }
}

/// 推送订阅编辑（改名 / 移动目录）到远端（best-effort，A-2）。
/// 这是旧直接调用接口，失败返回 false；生产编辑经持久 outbox 执行并上报活动终态。
///
/// OPT-006：`expected_generation` 是命令在**同一持锁内**读远端 id 时捕获的代际
/// （record_feed_edit 返回）。构建会话期间账号被切换（代际不等）→ 不向新账号
/// 发送旧账号的 remote_id，直接返回 false。
///
/// OPT-009 阶段 B：本函数是**直接调用入口**（集成测试与旧调用方形态），不带
/// 旧 label 移除参数；生产命令的编辑推送已改经订阅 outbox 执行器
/// （[`try_push_subscription_outbox`]），移动的 `r=` 旧 label 由队列行携带。
pub async fn edit_remote_subscription(
    db: &Arc<Mutex<Connection>>,
    http: &reqwest::Client,
    remote_id: i64,
    title: Option<&str>,
    dest_label: Option<&str>,
    expected_generation: i64,
) -> bool {
    // TASK-124：build_client Result 化——两类失败（未配置/认证·网络失败）对
    // 订阅编辑推送的既有行为一致（best-effort，失败仅记 false），此处不记录不发事件。
    let lease = match acquire_session(db, http).await {
        Ok(l) => l,
        Err(_) => return false,
    };
    let session = lease.session;
    if session.generation != expected_generation {
        log::warn!("sync: 订阅编辑的账号代际已变更，取消向新账号发送旧 remote_id");
        return false;
    }
    match session
        .client
        .edit_subscription(remote_id, title, dest_label, None)
        .await
    {
        Ok(()) => true,
        Err(e) => {
            log::warn!("sync: 订阅编辑推送失败（本地已生效，待下轮收敛）: {e}");
            false
        }
    }
}

/// 退订远端订阅（best-effort；仅 GReader 协议有端点——Fever 无退订端点）。
///
/// 返回值仅表示**请求被后端接受**（2xx 且响应体 `OK`；`greader::post_form_text`
/// 严格校验正文，见 `post_form_text_inner`），**不等于「远端已删除该订阅」**：
/// GReader 的 `subscription/edit` 在 token 失效、权限不足等情况下仍可能返回
/// 2xx（配合错误体或静默无操作），服务端是否真正应用没有可核验的一致性界。
/// 另：目标 id 不存在时 FreshRSS 返回 400，客户端按「已不存在 = 已满足」容忍
/// （B2-R2 复审 P2-C，见 `GReaderClient::unsubscribe`）——那同样不是「远端列表
/// 已无该 URL」的证据。
///
/// **因此这里不清除删除墓碑**。墓碑只由 `pull_feeds` 在「远端订阅列表实际已不含该 URL」
/// 时清除（见本文件下方 tombstone 收敛段）——那是唯一有证据支撑的清除条件。
/// 曾经在此处按 `ok` 清墓碑，会导致 2xx 但远端未生效时清掉唯一防线，
/// 下次 pull 见远端仍列出该订阅便把它**复活**（用户现象：「删掉的订阅自己回来了」）。
///
/// OPT-006：同 `edit_remote_subscription`——`expected_generation` 不匹配时不向
/// 新账号发送旧账号的 remote_id。
///
/// OPT-009 阶段 B2：生产删除路径已改经类型化订阅 outbox——`record_feed_deletion`
/// 在同一事务里写墓碑 + 本地删除 + 退订意图，由执行器按账号 key/代际推送并重试
/// （见 [`push_subscription_outbox`]）；本函数不再被命令层调用，保留为直接调用
/// 入口与「2xx 不清墓碑」既有回归的锚点（成功仅为请求被接受，**不代表远端已
/// 删除**——墓碑只由 pull 的「远端列表确认已不含」收敛清除，TASK-055）。
pub async fn unsubscribe_remote(
    db: &Arc<Mutex<Connection>>,
    http: &reqwest::Client,
    remote_id: i64,
    feed_url: &str,
    expected_generation: i64,
) -> bool {
    // 保留签名语义：调用方语义不变；墓碑不再由此处清除
    let _ = feed_url;
    // TASK-124：build_client Result 化——两类失败（未配置/认证·网络失败）对
    // 退订推送的既有行为一致（best-effort，失败仅记 false），此处不记录不发事件。
    let lease = match acquire_session(db, http).await {
        Ok(l) => l,
        Err(_) => return false,
    };
    let session = lease.session;
    if session.generation != expected_generation {
        log::warn!("sync: 退订的账号代际已变更，取消向新账号发送旧 remote_id");
        return false;
    }
    match session.client {
        Backend::GReader(ref c) => c.unsubscribe(remote_id).await.is_ok(),
        Backend::Fever(_) => false,
    }
}

/// 未连接期间本地新增的订阅推到远端（三段式：锁内读队列 → 锁外 HTTP → 锁内落库）
pub(super) async fn push_feeds(
    db: &Arc<Mutex<Connection>>,
    session: &SyncSession,
    report: &mut SyncReport,
    queue_scope: Option<&std::collections::HashSet<i64>>,
) {
    // add_feed 队列动作：锁内读出全部待处理项（feed_url + 目标分类）
    struct PendingFeed {
        queue_id: i64,
        url: String,
        /// A-3：队列 payload 携带的目标分类名（本地 folder id → name 解析）。
        /// quick_add 只传 URL，订阅会落到远端默认分类——推送后需补一次
        /// edit_subscription(a=label) 才能保住 OPML/add_feed 选择的目录结构。
        folder_label: Option<String>,
    }
    let items: Vec<PendingFeed> = {
        let conn = db.lock().await;
        // OPT-006：读队列入 HTTP 前先复核代际——账号已切换时旧队列已随 purge
        // 清空，本段不应再向新账号发起任何 add_feed。
        if let Err(e) = session.ensure_current(&conn) {
            log::warn!("sync: 订阅推送前账号代际已变更，跳过本轮推送: {e}");
            return;
        }
        let queued = match db::take_sync_queue(&conn) {
            Ok(v) => v,
            Err(e) => {
                // C-2：读队列失败不再静默当空队列
                report.errors.push(format!("读取同步队列失败: {e}"));
                Vec::new()
            }
        };
        let mut out = Vec::new();
        for item in queued {
            if queue_scope.is_some_and(|scope| !scope.contains(&item.id)) {
                continue;
            }
            if item.action != "add_feed" {
                continue;
            }
            let Some(url) = item.feed_url.filter(|u| !u.trim().is_empty()) else {
                continue;
            };
            // P3-2（自检 2026-09-29）：folder_name 的 Err 不再被 `.ok()` 压成 None。
            // 处置选择：warn + 记入 report.errors + 跳过该队项（不中止本轮）——
            // 依据：push_feeds 是 best-effort 阶段（A-2），单条目录名解析失败不应
            // 阻塞其余订阅的推送；而「静默当作 None」会把订阅挂到远端默认分类
            // （目录结构丢失）且无线索。跳过的队项不进 done、不被 prune_sync 剪除，
            // 下一轮自动重试（take_sync_queue 不删除队列行，可自愈）。
            let folder_id = item
                .payload
                .as_deref()
                .and_then(|p| serde_json::from_str::<serde_json::Value>(p).ok())
                .and_then(|v| v.get("folder_id").and_then(|f| f.as_i64()));
            let folder_label = match folder_id {
                Some(fid) => match db::folder_name(&conn, fid) {
                    Ok(label) => label,
                    Err(e) => {
                        log::warn!(
                            "sync: 解析队列项 {} 的目标分类名失败（folder={fid}，下轮重试）: {e}",
                            item.id
                        );
                        report.errors.push(format!(
                            "解析订阅 {} 的目标分类名失败（下轮重试）: {e}",
                            url
                        ));
                        continue;
                    }
                },
                None => None,
            };
            out.push(PendingFeed {
                queue_id: item.id,
                url,
                folder_label,
            });
        }
        out
    };
    if items.is_empty() {
        return;
    }
    // 锁外：逐个订阅。B1（F2）：先以列表证据解析「是否已订阅」（崩溃窗口/
    // 另一设备已订阅都会让重复 quick_add 被真服务端拒绝），确认不存在才
    // quick_add；quick_add 未返回 id（FreshRSS numResults=0）或失败时同样转
    // 列表证据解析——创建可能已在错误返回前落地。
    let mut done: Vec<i64> = Vec::new();
    // 挂载失败保留的 legacy 队项（W2 复审 P3-3）：保留 ≠ 无记录——落账
    // attempts/last_error（与 outbox 的 Retry 口径一致），否则 attempts 恒 0、
    // waiting 永不清零，失败不可见。
    let mut mount_failed: Vec<(i64, String)> = Vec::new();
    for it in &items {
        // OPT-006：每个订阅推送（新批次）前复核代际——账号已切换即停止后续请求。
        if !session_is_current(db, session).await {
            log::warn!("sync: 订阅推送期间账号代际已变更，停止后续推送");
            break;
        }
        let mut created = false;
        let rid = match resolve_existing_feed_id(session, &it.url).await {
            Ok(Some(v)) => v,
            Ok(None) => match session.client.quick_add(&it.url).await {
                Ok(r) => {
                    created = true;
                    match r
                        .stream_id
                        .as_deref()
                        .and_then(greader::parse_feed_numeric_id)
                    {
                        Some(v) => v,
                        None => match resolve_existing_feed_id(session, &it.url).await {
                            Ok(Some(v)) => v,
                            _ => {
                                report.errors.push(format!(
                                    "推送订阅 {} 失败: quick_add 未返回 stream id 且列表未见同 URL 订阅（保留队项下轮重试）",
                                    it.url
                                ));
                                continue;
                            }
                        },
                    }
                }
                Err(e) => match resolve_existing_feed_id(session, &it.url).await {
                    Ok(Some(v)) => v,
                    _ => {
                        report.push_client_error(&e, format!("推送订阅 {} 失败: {e}", it.url));
                        continue;
                    }
                },
            },
            Err(e) => {
                report.push_client_error(
                    &e,
                    format!(
                        "解析订阅 {} 是否已存在失败（保留队项下轮重试）: {e}",
                        it.url
                    ),
                );
                continue;
            }
        };
        if created {
            report.pushed_feeds += 1;
        }
        // 锁内：绑定本地 feed（URL 匹配）
        let bound_remote_id = {
            let conn = db.lock().await;
            // OPT-006：绑定是回写边界——响应在途期间换号时不得写绑定。
            if session.ensure_current(&conn).is_err() {
                log::warn!("sync: quick_add 响应返回时账号代际已变更，丢弃绑定");
                break;
            }
            let mut nid = None;
            // 绑定查找走规范化 URL（与 pull_feeds、OPML 去重同口径）：队列携带的
            // 字符串与本地入库的 URL 常有尾斜杠/www. 差异，精确匹配会漏判成
            // 「本地已删除」→ 永不绑定。DB 错误不再 `.ok()` 吞掉：沿用 folder_label
            // 的处置（warn + report.errors + continue，队项不进 done，下轮重试）。
            // Note: 绑定查找必须走规范化 URL 且 DB 错误不得吞 — 见 .agents/notes/implemented/bug-fix/2026-11-02-订阅绑定规范化与旁路16MiB界与SMTC浮点饱和.md
            match db::feed_id_by_url_normalized(&conn, &it.url) {
                Ok(Some(local_id)) => {
                    // P3[1]：绑定写失败此前静默——绑定缺失会让该源在下次 pull
                    // 被当作未绑定源重复处理。改为 warn（可自愈，不中断）。
                    if let Err(err) = db::set_feed_remote_id(&conn, local_id, rid) {
                        log::warn!(
                        "sync: quick_add 后绑定远端 id 失败（feed={local_id} remote={rid}）: {err}"
                    );
                    }
                    nid = Some(rid);
                }
                Ok(None) => {}
                Err(e) => {
                    log::warn!(
                        "sync: 解析队列项 {} 的本地 feed 绑定失败（下轮重试）: {e}",
                        it.queue_id
                    );
                    report.errors.push(format!(
                        "解析订阅 {} 的本地绑定失败（下轮重试）: {e}",
                        it.url
                    ));
                    continue;
                }
            }
            nid
        };
        // A-3：锁外补挂目标分类。挂载失败**保留队项**（不进 done → 不剪除）：
        // pull 对既有源不回灌远端分类（2026-11-01 审计 R2-P3），剪除后「远端订阅
        // 留在默认分类」的差距没有任何收敛通道。保留后下一轮先以列表证据解析
        // 既有 id（不重复 quick_add），只重试挂载——与订阅 outbox 的 Retry
        // 保留队项语义一致。
        // Note: 挂载失败保留队项的重试语义 — 见 .agents/notes/implemented/architecture/2026-10-14-订阅写回outbox执行器.md
        match (bound_remote_id, it.folder_label.as_deref()) {
            (Some(nid), Some(label)) => {
                match session
                    .client
                    .edit_subscription(nid, None, Some(label), None)
                    .await
                {
                    Ok(()) => done.push(it.queue_id),
                    Err(e) => {
                        let summary = format!(
                            "订阅 {} 挂载分类「{label}」失败（保留队项下轮重试）: {e}",
                            it.url
                        );
                        report.push_client_error(&e, summary.clone());
                        mount_failed.push((it.queue_id, summary));
                    }
                }
            }
            // 无本地绑定（本地已删除）或无目标分类：无挂载可做，照常出队
            _ => done.push(it.queue_id),
        }
    }
    let conn = db.lock().await;
    // OPT-006：剪除确认是回写边界——代际失配时 done 里的旧队项绝不能触碰
    //（账号已切换，旧队项已随 purge 清空；新账号队项与旧编号无对应关系）。
    if session.ensure_current(&conn).is_err() {
        log::warn!("sync: 订阅推送确认时账号代际已变更，跳过剪除与僵尸清理");
        return;
    }
    // P3-3：失败标记与剪除同一代际守卫边界（旧代际的失败不落到新队列）。
    if !mount_failed.is_empty() {
        if let Err(err) = db::mark_push_failed(&conn, &mount_failed) {
            log::warn!(
                "sync: 挂载失败保留队项的失败标记落库失败（{} 项）: {err}",
                mount_failed.len()
            );
        }
    }
    // P3[1]：剪除已推送队项失败此前静默——残留队项会被下一轮重复推送（重复订阅
    // 动作），且无日志可查。改为 warn（失败不中断：下轮会再尝试剪除）。
    if let Err(err) = db::prune_sync(&conn, &done) {
        log::warn!("sync: 剪除已推送队列项失败（{} 项）: {err}", done.len());
    }
    // 定向同步不得借道清理其它队列行；僵尸清理只属于全量 feeds 阶段。
    if queue_scope.is_none() {
        // remove_feed 语义（SUB-4/SYN-1）：本地删除不推远端。历史版本可能在
        // delete_feed 时建过 remove_feed 队项（从未被消费）——此处统一清僵尸项，
        // 与 delete_feed 命令「不再建 remove_feed」的新语义一致。
        if let Err(err) = db::purge_remove_feed_zombies(&conn) {
            log::warn!("sync: 清理 remove_feed 僵尸队项失败: {err}");
        }
    }
    drop(conn);
    // P3-3：失败标记改变了队列统计（waiting 中的 failed 计数升）——即时推送
    // pill（notify 内部自持短锁，此处已在 DB 锁外）。
    if !mount_failed.is_empty() {
        notify_queue_changed(db).await;
    }
}

/// 单条订阅 outbox 的执行结果。
enum OutboxOutcome {
    /// 完整确认（可精确出队）：edit/unsubscribe 按 id 出队。
    Confirmed,
    /// subscribe 的完整确认：携带**实际发送**的目标快照（标题/分类/旧 label）。
    /// 出队必须逐字段比对当前行（`confirm_subscribe_if_desired_unchanged`）——
    /// 挂载 HTTP 在途期间原地更新（同 id/代际）会让旧成功落空，队项保留待下轮
    /// 发送最新目标。
    ConfirmedSubscribe(Option<String>, Option<String>, Option<String>),
    /// 本轮失败：保留队项、累计 attempts/last_error，下轮重试。
    Retry(String),
    /// 客户端错误与显示消息一起传递，延迟汇聚不能丢认证分类。
    ClientRetry(String, bool),
    /// 结构性跳过（响应在途期间行被替换/删除/动作改变、本地订阅已删除并结转
    /// 退订）——不发请求、不确认、不记失败：旧响应不得触碰新意图。
    Skipped,
}

impl OutboxOutcome {
    fn client_retry(err: &crate::error::AppError, message: String) -> Self {
        Self::ClientRetry(message, err.is_auth_failure())
    }
}

/// subscribe 的已发送目标快照：`(id, 标题, 分类, 旧 label)`——确认逐字段比对用。
type SentSubscribeTarget = (i64, Option<String>, Option<String>, Option<String>);

/// 按规范化 URL 在远端订阅列表中解析既有 feed 的数字 id（B1 F2 收敛）。
///
/// 真实服务端对重复 URL 的 quick_add 一律拒绝（FreshRSS 200+numResults=0、
/// Miniflux 500 feed_already_exists），「已订阅与否」的唯一可靠证据是订阅列表。
/// 列表读取失败向上传播——调用方在「无法确认缺席」时**不得**盲发 quick_add。
async fn resolve_existing_feed_id(session: &SyncSession, feed_url: &str) -> AppResult<Option<i64>> {
    let norm = db::normalize_url(feed_url);
    let subs = session.client.subscriptions().await?;
    Ok(subs
        .iter()
        .filter(|s| db::normalize_url(&s.url) == norm)
        .find_map(|s| greader::parse_feed_numeric_id(&s.id)))
}

/// 订阅 outbox 生产执行器（OPT-009 阶段 B）。
///
/// ## 账号屏障与代际
/// - 计划只取 **account_key 等于当前会话账号** 的行；异 key 在本层跳过。
///   身份切换由账号提交事务清理旧队项，旧操作不会在切回原账号时恢复；
/// - 同 key、旧代际的行在计划前**明确重绑**到当前代际（同账号改密码后旧意图
///   仍有效，继续可发）；异 key 一律不 touch。
///
/// ## 执行状态机（精确确认，不立即 done/prune）
/// - subscribe(pending)：quick_add → 成功先 `advance remote_created` 记下 remote_id
///   （**重启后不重复 quick_add**）→ 绑定本地 feed（写失败保留队项重试）→
///   **同一临界区重读当前行**，按重读后的最新标题/分类补发一次 edit_subscription
///   → 全部成功才按 (id, generation) + 「目标未变」精确出队；
/// - subscribe(remote_created)：只做绑定与目标挂载，绝不重发 quick_add；
/// - 计划快照不是发送依据（B1-R2）：quick_add 网络窗口内 record_feed_edit 会经
///   `update_pending_subscribe_desired` 原地更新同一行（同 id）；行被替换/删除/
///   动作改变 → 安全跳过；发送后目标再变 → 确认落空、保留队项发送最新目标；
/// - edit：`t`=新标题、`a`=目标 label、`r`=**旧 label**（真实移动语义，只发 a
///   是追加分类）；
/// - unsubscribe：无端点协议（Fever）由能力声明拦下；GReader 走 ac=unsubscribe。
///
/// ## 孤儿防护
/// - 响应在途期间行被合并替换 → 旧响应不确认新行；quick_add 已建的 remote_id
///   补给替代 pending 行（退订/重新添加），不留远端孤儿；
/// - 本地订阅已删除而队项仍在 → 远端 id 结转同账号退订意图；
/// - Fever 等不支持写订阅的协议：全部计划项置 `local_only` 终态（保留展示、
///   不重试、不冒充远端完成）。
pub(super) async fn push_subscription_outbox(
    db: &Arc<Mutex<Connection>>,
    session: &SyncSession,
    report: &mut SyncReport,
    outbox_scope: Option<&std::collections::HashSet<i64>>,
) {
    // 与 state 推送/命令层一次性推送串行化（同一时刻一个 outbox 推送在飞）。
    let _guard = PUSH_LOCK.lock().await;

    // ① 锁内：代际复核 → 取当前账号 key → 同账号旧代际重绑 → 计划
    let (key, plan): (String, Vec<db::SubscriptionOutboxItem>) = {
        let conn = db.lock().await;
        if let Err(e) = session.ensure_current(&conn) {
            log::warn!("sync: 订阅 outbox 推送前账号代际已变更，跳过本轮: {e}");
            return;
        }
        let key = match super::current_account(&conn) {
            Ok(Some(ctx)) => ctx.key,
            Ok(None) => return, // 会话存在即凭据完整；防御：无凭据不推
            Err(e) => {
                report
                    .errors
                    .push(format!("读取同步账号身份失败，跳过订阅写回: {e}"));
                return;
            }
        };
        let rebind_result = match outbox_scope {
            Some(scope) => {
                let ids: Vec<i64> = scope.iter().copied().collect();
                db::rebind_outbox_generation_for_ids(&conn, &key, session.generation, &ids)
            }
            None => db::rebind_outbox_generation(&conn, &key, session.generation),
        };
        if let Err(e) = rebind_result {
            // 重绑失败不阻断：本轮仍只发代际相等的行（下轮再重绑）
            report.errors.push(format!("订阅队项代际重绑失败: {e}"));
        }
        match db::pending_outbox(&conn) {
            Ok(items) => {
                let total = items.len();
                let mine: Vec<db::SubscriptionOutboxItem> = items
                    .into_iter()
                    .filter(|i| i.account_key == key && i.account_generation == session.generation)
                    .filter(|i| {
                        outbox_scope.map_or(true, |scope| {
                            scope.contains(&i.id) && i.action == db::ACTION_SUBSCRIBE
                        })
                    })
                    .collect();
                if outbox_scope.is_none() && mine.len() < total {
                    log::info!(
                        "sync: 订阅 outbox 跳过 {} 项账号或代际不匹配队项（身份切换会清理旧操作，切回原账号不恢复）",
                        total - mine.len()
                    );
                }
                (key, mine)
            }
            Err(e) => {
                report.errors.push(format!("读取订阅写回队列失败: {e}"));
                (key, Vec::new())
            }
        }
    };
    if plan.is_empty() {
        return;
    }

    // ② Fever 能力门：无写订阅端点 → local_only 终态（保留展示、不重试、
    //    不返回 Ok 冒充远端完成）。B3 起能力矩阵（subscription_capabilities）
    //    与 UI 呈现（设置页能力卡/摘要行、FeedsTab 确认弹窗）已按此边界接线。
    // Note: 能力矩阵与持久回执的 IPC/呈现落点 — 见 .agents/notes/implemented/architecture/2026-10-15-订阅写回能力矩阵与持久回执.md
    if !session.client.supports_subscription_write() {
        let pairs: Vec<(i64, i64)> = plan.iter().map(|i| (i.id, session.generation)).collect();
        let marked = {
            let conn = db.lock().await;
            if session.ensure_current(&conn).is_err() {
                log::warn!("sync: local-only 标记前账号代际已变更，跳过");
                return;
            }
            db::mark_outbox_local_only(&conn, &pairs)
        };
        match marked {
            Ok(n) => {
                log::info!("sync: Fever 协议不支持订阅写回，{n} 项标记 local_only");
                report.errors.push(format!(
                    "Fever 协议不支持订阅写回：{n} 项仅本地生效（local_only，不再重试，也不冒充远端完成）"
                ));
                // B3：local_only 标记也是状态变化——推送前端回执（锁已在标记临界区外）
                if n > 0 {
                    notify_subscription_outbox_changed(db).await;
                }
            }
            Err(e) => report
                .errors
                .push(format!("Fever local-only 标记落库失败: {e}")),
        }
        return;
    }

    // ③ 锁外逐项执行（每项发送前复核代际；失败保留队项）
    let mut done_plain: Vec<i64> = Vec::new();
    let mut done_subscribe: Vec<SentSubscribeTarget> = Vec::new();
    // 本轮**发送成功**的移动 (id, feed_url, 已接受目标 label)：确认落空时按它
    // 重定同账号后继意图的移除基准（见 §④ 与 db::rebase_successor_edit_prev）。
    let mut sent_moves: Vec<(i64, String, String)> = Vec::new();
    let mut failed: Vec<(i64, i64, String)> = Vec::new();
    for item in &plan {
        if !session_is_current(db, session).await {
            log::warn!("sync: 订阅写回期间账号代际已变更，停止后续推送");
            break;
        }
        let outcome = match item.action.as_str() {
            db::ACTION_SUBSCRIBE => exec_subscribe_item(db, session, &key, item, report).await,
            db::ACTION_EDIT => exec_edit_item(session, item).await,
            db::ACTION_UNSUBSCRIBE => exec_unsubscribe_item(db, session, item).await,
            other => OutboxOutcome::Retry(format!(
                "订阅写回包含未知动作「{other}」，保留队项（不猜测语义）"
            )),
        };
        match outcome {
            OutboxOutcome::Confirmed => {
                if item.action == db::ACTION_EDIT {
                    if let Some(target) = item.desired_folder.clone() {
                        sent_moves.push((item.id, item.feed_url.clone(), target));
                    }
                }
                done_plain.push(item.id)
            }
            OutboxOutcome::ConfirmedSubscribe(title, folder, prev) => {
                // A-P2-3②：挂载成功的 `a=目标` 与 edit 移动同属「远端已接受该
                // label」的账号作用域发送事实——确认落空（行被替换）时同样由
                // ④b 把替代行的移除基准重定为已接受目标，否则替代行按链起点
                // 发 r=，远端残留已接受的中间 label。
                if let Some(target) = folder.clone() {
                    sent_moves.push((item.id, item.feed_url.clone(), target));
                }
                done_subscribe.push((item.id, title, folder, prev))
            }
            OutboxOutcome::Retry(summary) => {
                report.errors.push(summary.clone());
                failed.push((item.id, session.generation, summary));
            }
            OutboxOutcome::ClientRetry(summary, is_auth) => {
                report.push_classified_error(summary.clone(), is_auth);
                failed.push((item.id, session.generation, summary));
            }
            OutboxOutcome::Skipped => {}
        }
    }

    // ④ 锁内：失败标记 + 精确确认（代际复核后落库）+ 后继意图移除基准重定
    if done_plain.is_empty()
        && done_subscribe.is_empty()
        && failed.is_empty()
        && sent_moves.is_empty()
    {
        return;
    }
    let conn = db.lock().await;
    if session.ensure_current(&conn).is_err() {
        // OPT-006 纪律：代际已变的**确认/失败标记**一律不落库（队项保留，下轮按
        // 新会话重发）。但「在途移动已成功、同账号后继意图已排队」这一事实属于
        // **账号作用域**：换代（同账号改密码/重连）后若不重定，下一轮会按过期
        // 链起点发 r=，远端长期残留已发送的中间 label（B2-1）。故下面仍按
        // 发送时捕获的 account key 重定——跨账号由 key 守卫隔离。
        log::warn!(
            "sync: 订阅 outbox 确认时账号代际已变更，丢弃确认与失败标记（队项保留待下轮）；\
             同账号后继意图的移除基准仍按已接受目标重定"
        );
    } else {
        if !failed.is_empty() {
            if let Err(e) = db::mark_outbox_failed(&conn, &failed) {
                log::warn!("sync: 订阅失败标记落库失败: {e}");
            }
        }
        // subscribe：确认必须与**实际发送**的目标逐字段相等（B1-R2 / B2-3）——
        // 挂载 HTTP 在途期间原地更新（同 id/代际）时确认落空，最新目标留待下一轮。
        for (id, title, folder, prev) in &done_subscribe {
            match db::confirm_subscribe_if_desired_unchanged(
                &conn,
                *id,
                session.generation,
                title.as_deref(),
                folder.as_deref(),
                prev.as_deref(),
            ) {
                Ok(true) => {}
                Ok(false) => log::warn!(
                    "sync: 订阅队项 {id} 发送后目标已被更新，保留队项待下轮发送最新目标（旧成功不得确认新意图）"
                ),
                Err(e) => report.errors.push(format!("订阅队项 {id} 确认出队失败（远端已接受，队项保留待重试）: {e}")),
            }
        }
        if !done_plain.is_empty() {
            // edit/unsubscribe 按 id 精确确认（换新 id 的替换路径天然落空）。
            for id in &done_plain {
                match db::prune_outbox(&conn, &[*id], session.generation) {
                    Ok(1) => {}
                    Ok(_) => log::warn!(
                        "sync: 订阅队项 {id} 精确确认落空（已被合并替换或删除），旧成功不确认新意图"
                    ),
                    Err(e) => report.errors.push(format!(
                        "订阅队项 {id} 确认出队失败（远端已接受，队项保留待重试）: {e}"
                    )),
                }
            }
        }
    }
    // ④b 在途移动成功后的后继重定（**不受代际守卫阻断**，见上）：
    // 只有执行器在确认时刻知道「远端已接受该目标」；被处理的 id 已不在队
    // （确认成功删除，或被新意图合并替换）→ 后者按已接受目标重定替代行。
    for (id, feed_url, target) in &sent_moves {
        match db::outbox_pending_by_id(&conn, *id) {
            Ok(true) => {} // 行仍在（本轮未确认，例如代际已变）：它自己就是待发意图
            Ok(false) => match db::rebase_successor_edit_prev(&conn, &key, feed_url, target) {
                Ok(0) => {} // 无替代行（或被重写为别的账号/动作）：无需处理
                Ok(n) => log::warn!(
                    "sync: 订阅队项 {id} 发送成功但已被在途意图替换：{n} 个替代行移除基准重定为「{target}」（旧成功不确认新目标）"
                ),
                Err(e) => log::warn!("sync: 重定替代行移除基准失败: {e}"),
            },
            Err(e) => log::warn!("sync: 查询订阅队项 {id} 是否仍待推失败，跳过重定: {e}"),
        }
    }
    // B3：确认出队/失败累计也是状态变化——推送前端回执。事件在 DB 锁外发
    // （notify 内部自持短锁）；仅本轮确实触达了确认/失败落库时才发（不变不发）。
    drop(conn);
    if !done_plain.is_empty() || !done_subscribe.is_empty() || !failed.is_empty() {
        notify_subscription_outbox_changed(db).await;
    }
}

/// subscribe 项：quick_add（仅 pending 阶段）→ 推进 remote_created → 绑定本地
/// feed → 补挂目标标题/分类 → 完整确认。
async fn exec_subscribe_item(
    db: &Arc<Mutex<Connection>>,
    session: &SyncSession,
    key: &str,
    item: &db::SubscriptionOutboxItem,
    report: &mut SyncReport,
) -> OutboxOutcome {
    // 阶段一：远端 id（pending 阶段先按列表证据解析「是否已订阅」，确认不存在
    // 才 quick_add；remote_created 直接用已记录的 id）
    let rid = match item.remote_id {
        Some(v) => v,
        None => {
            // B1（F2 跨重启收敛）：pending 行的重发不再盲目——崩溃窗口内的
            // quick_add 可能已被服务端执行，而真实服务端对重复 URL 一律拒绝
            // （FreshRSS 200+numResults=0、Miniflux 500 feed_already_exists）。
            // 先以订阅列表按 URL 解析既有 feed；列表读失败 = 无法确认缺席，
            // 同样不发 quick_add（fail-closed），保留队项下轮重试。
            match resolve_existing_feed_id(session, &item.feed_url).await {
                Ok(Some(v)) => v,
                Ok(None) => match session.client.quick_add(&item.feed_url).await {
                    Ok(r) => {
                        report.pushed_feeds += 1;
                        match r
                            .stream_id
                            .as_deref()
                            .and_then(greader::parse_feed_numeric_id)
                        {
                            Some(v) => v,
                            // 真实服务端对已存在 URL 不返回 stream id（FreshRSS
                            // numResults=0）；Miniflux 已存在走 Err。二者都转列表
                            // 证据解析——创建可能已在错误返回前落地。
                            None => match resolve_existing_feed_id(session, &item.feed_url).await {
                                Ok(Some(v)) => v,
                                _ => {
                                    return OutboxOutcome::Retry(format!(
                                        "推送订阅 {} 失败: quick_add 未返回 stream id 且列表未见同 URL 订阅（下轮重试）",
                                        item.feed_url
                                    ))
                                }
                            },
                        }
                    }
                    Err(e) => match resolve_existing_feed_id(session, &item.feed_url).await {
                        Ok(Some(v)) => v,
                        _ => {
                            return OutboxOutcome::client_retry(
                                &e,
                                format!("推送订阅 {} 失败: {e}", item.feed_url),
                            );
                        }
                    },
                },
                Err(e) => {
                    return OutboxOutcome::client_retry(
                        &e,
                        format!(
                            "解析订阅 {} 是否已存在失败（保留队项，下轮重试）: {e}",
                            item.feed_url
                        ),
                    );
                }
            }
        }
    };

    // 阶段二：锁内推进阶段 + 绑定本地 feed + 重读最新目标（回写边界，代际复核）
    //
    // B1-R2（复审 P2）：计划快照里的 desired_* 可能在 quick_add 网络窗口内过时
    // ——record_feed_edit 对 remote_id=None 的 subscribe 行经
    // `update_pending_subscribe_desired` **原地**更新同一行（同 id）。因此绑定的
    // 同一 DB 临界区内重读当前行，挂载/确认以重读结果为准；行被替换/删除/动作
    // 改变则安全跳过（不得继续旧目标，也不得确认）。
    let (fresh_title, fresh_folder, fresh_prev) = {
        let conn = db.lock().await;
        if session.ensure_current(&conn).is_err() {
            log::warn!("sync: quick_add 响应返回时账号代际已变更，丢弃订阅推进");
            return OutboxOutcome::Skipped;
        }
        if item.stage == db::STAGE_PENDING {
            match db::advance_subscribe_stage(&conn, item.id, session.generation, rid) {
                Ok(true) => {}
                Ok(false) => {
                    // 行已被合并/退订/删除替换：把远端已建 id 补给替代行，避免孤儿；
                    // 旧 HTTP 成功不得确认（或绑定）替代行之外的新意图。
                    match db::adopt_remote_id_for_pending(
                        &conn,
                        key,
                        session.generation,
                        &item.feed_url,
                        rid,
                    ) {
                        Ok(true) => log::info!(
                            "sync: 订阅 {} 行已被替换，远端已建 id {rid} 已结转替代行",
                            item.feed_url
                        ),
                        Ok(false) => log::warn!(
                            "sync: 订阅 {} 行已被替换且无替代行可结转远端 id {rid}（无本地残留）",
                            item.feed_url
                        ),
                        Err(e) => log::warn!("sync: 接管远端已建订阅 id 失败: {e}"),
                    }
                    return OutboxOutcome::Skipped;
                }
                Err(e) => {
                    return OutboxOutcome::Retry(format!(
                        "订阅 {} 阶段推进失败: {e}",
                        item.feed_url
                    ))
                }
            }
        }
        // 绑定义务：本地订阅仍存在则写入 remote_id；已删除则把远端 id 结转退订。
        match db::feed_id_by_url_normalized(&conn, &item.feed_url) {
            Ok(Some(local_id)) => {
                if let Err(e) = db::set_feed_remote_id(&conn, local_id, rid) {
                    return OutboxOutcome::Retry(format!(
                        "绑定本地订阅 {} 失败（队项保留，重启重试不重复 quick_add）: {e}",
                        item.feed_url
                    ));
                }
            }
            Ok(None) => {
                // 结转退订 = enqueue_unsubscribe 内部的 DELETE pending + DELETE
                // add_feed 队项 + INSERT 退订三写。崩溃窗口会留下「旧创建意图
                // 已删、退订意图未落」的半程状态（远端订阅永久无人退订）。全库
                // 其余调用方均在事务内执行（commands/folders.rs 退订命令等），
                // 此处对齐同一口径；开事务/入队/提交任一步失败按既有软失败处理
                // （warn + Skipped，事务对象 drop 自动回滚）。
                let carried = (|| -> AppResult<bool> {
                    let tx = conn.unchecked_transaction()?;
                    let queued = db::enqueue_unsubscribe(
                        &tx,
                        session.generation,
                        key,
                        &item.feed_url,
                        Some(rid),
                        None,
                    )?;
                    tx.commit()?;
                    Ok(queued)
                })();
                match carried {
                    Ok(queued) => log::warn!(
                        "sync: 订阅 {} 本地已删除，远端已建 id {rid} 结转退订（queued={queued}）",
                        item.feed_url
                    ),
                    Err(e) => log::warn!("sync: 结转退订意图失败（远端 id {rid}）: {e}"),
                }
                return OutboxOutcome::Skipped;
            }
            Err(e) => {
                return OutboxOutcome::Retry(format!("查询本地订阅 {} 失败: {e}", item.feed_url))
            }
        }
        // 与绑定同一临界区重读：此后发送的目标即「发送时最新」（标题/分类/旧
        // label 三字段都参与确认——B2-3 起挂载也带 r= 移除旧 label）。
        match conn
            .query_row(
                "SELECT desired_title, desired_folder, prev_folder FROM subscription_outbox
                  WHERE id = ?1 AND account_generation = ?2 AND action = 'subscribe'
                    AND status = 'pending'",
                rusqlite::params![item.id, session.generation],
                |r| {
                    Ok((
                        r.get::<_, Option<String>>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()
        {
            Ok(Some(fresh)) => fresh,
            Ok(None) => {
                log::warn!(
                    "sync: 订阅 {} 行已被替换/删除/动作改变，本轮不发送、不确认（由新意图接管）",
                    item.feed_url
                );
                return OutboxOutcome::Skipped;
            }
            Err(e) => {
                return OutboxOutcome::Retry(format!("重读订阅行 {} 失败: {e}", item.feed_url))
            }
        }
    };

    // 阶段三：目标标题/分类补挂（绑定完成后的最后一步；失败则保留队项，
    // 下轮只重试挂载——阶段已是 remote_created，不会重复 quick_add）。
    // `prev`（旧 label）只在有目标分类时发送：分类改名/移动在 remote_created
    // 窗口留下的旧 label 由它移除（greader 客户端对 a==r 自行抑制）。
    if fresh_title.is_some() || fresh_folder.is_some() {
        match session
            .client
            .edit_subscription(
                rid,
                fresh_title.as_deref(),
                fresh_folder.as_deref(),
                fresh_prev.as_deref(),
            )
            .await
        {
            Ok(()) => OutboxOutcome::ConfirmedSubscribe(fresh_title, fresh_folder, fresh_prev),
            Err(e) => OutboxOutcome::client_retry(
                &e,
                format!("订阅 {} 挂载目标（标题/分类）失败: {e}", item.feed_url),
            ),
        }
    } else {
        OutboxOutcome::ConfirmedSubscribe(fresh_title, fresh_folder, fresh_prev)
    }
}

/// edit 项：改名 `t` + 移动 `a`/`r`（旧 label 移除）。
async fn exec_edit_item(session: &SyncSession, item: &db::SubscriptionOutboxItem) -> OutboxOutcome {
    let Some(rid) = item.remote_id else {
        return OutboxOutcome::Retry(format!(
            "编辑订阅 {} 缺远端绑定，保留队项待绑定回填",
            item.feed_url
        ));
    };
    match session
        .client
        .edit_subscription(
            rid,
            item.desired_title.as_deref(),
            item.desired_folder.as_deref(),
            item.prev_folder.as_deref(),
        )
        .await
    {
        Ok(()) => OutboxOutcome::Confirmed,
        Err(e) => {
            OutboxOutcome::client_retry(&e, format!("订阅编辑 {} 推送失败: {e}", item.feed_url))
        }
    }
}

/// unsubscribe 项：远端退订（2xx-strict）。
///
/// ## 入队时已知远端目标（`remote_id` 非空）
/// 直接 `ac=unsubscribe s=feed/<id>`（含响应在途 adopt 结转的 id）。2xx 才确认，
/// 失败保留队项重试。
///
/// ## 远端目标未知（`remote_id IS NULL`）的恢复规则（B2-R1 P2 / B2-R2 复审 P2）
/// 这类行只在删除时同账号同 URL 仍有 pending 队项（可能已发出的 quick_add）时
/// 写入——远端可能已经存在这次创建，但 id 在响应丢失/换代/重启后拿不到
/// （`adopt_remote_id_for_pending` 只覆盖「响应在本地、同 key 同代际」的窗口）。
/// 真实服务端对重复 URL 一律拒绝（FreshRSS 200+numResults=0、Miniflux 500），
/// 「幂等 quick_add-by-URL 解析」在恢复路径上不成立——B1 起解析与发现共用
/// **一次订阅列表读取**：
/// 1. **解析**：列表里同 URL 的第一个 id 即已验证目标；成功后**立即持久化**
///    （`advance_unsubscribe_resolution` → `resolved_remote_id`，按
///    account_key+generation 严格限定作用域），重启/换代/换号后的重试直接采用。
/// 2. **发现**：同一次列表读取收集同 URL 的其它 id。列表读失败 = 重复目标发现
///    未完成 → `Retry` 保留队项，**本轮零发送**；列表读成功但无同 URL id =
///    远端没有可退订目标 → 意图已满足，直接 `Confirmed`。
///
/// 发送对象 = 持久化解析目标 ∪ 列表同 URL id，全部真实 2xx（目标已不存在按
/// 400/404 = 已满足，见 `GReaderClient::unsubscribe`）才 `Confirmed`。
async fn exec_unsubscribe_item(
    db: &Arc<Mutex<Connection>>,
    session: &SyncSession,
    item: &db::SubscriptionOutboxItem,
) -> OutboxOutcome {
    let Some(known) = item.remote_id else {
        return exec_unsubscribe_recovery(db, session, item).await;
    };
    match session.client.unsubscribe(known).await {
        Ok(()) => OutboxOutcome::Confirmed,
        Err(e) => OutboxOutcome::client_retry(
            &e,
            format!("退订 {}（远端 id {known}）推送失败: {e}", item.feed_url),
        ),
    }
}

/// 未知远端目标的退订恢复（见 [`exec_unsubscribe_item`] 的恢复规则）。
///
/// B1（F2）：解析与发现共用**一次**订阅列表读取——真实服务端对重复 URL 一律
/// 拒绝，「幂等 quick_add-by-URL 解析」不再成立，列表是唯一可靠证据源：
/// - 列表读失败 = 重复目标发现未完成 → 保留队项重试，本轮零发送；
/// - 列表读成功且无同 URL id → 远端没有可退订目标 = 意图已满足，直接确认；
/// - 已持久化的解析目标仍参与退订（它在远端消失 = 400/404 = 已满足，由客户端
///   2xx-strict 容错吸收），不得因它缺席而永久 Retry。
async fn exec_unsubscribe_recovery(
    db: &Arc<Mutex<Connection>>,
    session: &SyncSession,
    item: &db::SubscriptionOutboxItem,
) -> OutboxOutcome {
    // ①②解析+发现段（B1）：共用一次列表读取。
    let subs = match session.client.subscriptions().await {
        Ok(v) => v,
        Err(e) => {
            return OutboxOutcome::client_retry(
                &e,
                format!(
                    "退订 {} 列远端订阅失败: 重复目标发现未完成，保留队项（下轮重试）: {e}",
                    item.feed_url
                ),
            );
        }
    };
    let norm = db::normalize_url(&item.feed_url);
    let mut targets: Vec<i64> = Vec::new();
    for s in subs.iter().filter(|s| db::normalize_url(&s.url) == norm) {
        if let Some(id) = greader::parse_feed_numeric_id(&s.id) {
            if !targets.contains(&id) {
                targets.push(id);
            }
        }
    }
    // 首次解析到的已验证 id 先落库（重启/换代/换号后的重试直接采用；写入按
    // account_key+generation 严格限定作用域）。列表成功但无同 URL id 时不持久化。
    if item.resolved_remote_id.is_none() {
        if let Some(&first) = targets.first() {
            let stored = {
                let conn = db.lock().await;
                if session.ensure_current(&conn).is_err() {
                    log::warn!(
                        "sync: 退订 {} 解析响应返回时账号代际已变更，丢弃本轮（不写库不发送）",
                        item.feed_url
                    );
                    return OutboxOutcome::Skipped;
                }
                match db::advance_unsubscribe_resolution(&conn, item.id, session.generation, first)
                {
                    Ok(true) => true,
                    Ok(false) => {
                        // 行已被替换/删除（新意图接管）：把已验证 id 结转替代行，
                        // 避免远端孤儿；旧解析结果绝不推进不属于它的行。
                        match db::adopt_remote_id_for_pending(
                            &conn,
                            &item.account_key,
                            session.generation,
                            &item.feed_url,
                            first,
                        ) {
                            Ok(true) => log::info!(
                                "sync: 退订 {} 行已被替换，远端已解析 id {first} 已结转替代行",
                                item.feed_url
                            ),
                            Ok(false) => log::warn!(
                                "sync: 退订 {} 行已被替换且无替代行可结转远端 id {first}",
                                item.feed_url
                            ),
                            Err(e) => log::warn!("sync: 退订解析目标结转替代行失败: {e}"),
                        }
                        false
                    }
                    Err(e) => {
                        return OutboxOutcome::Retry(format!(
                            "退订 {} 解析目标落库失败（保留队项，不发送半套退订）: {e}",
                            item.feed_url
                        ))
                    }
                }
            };
            if !stored {
                return OutboxOutcome::Skipped;
            }
        }
    }
    // 已持久化的解析目标也参与退订：它在远端消失 = 400/404 = 意图已满足
    //（由 `GReaderClient::unsubscribe` 的 2xx-strict 容错吸收），不得永久 Retry。
    if let Some(v) = item.resolved_remote_id {
        if !targets.contains(&v) {
            targets.push(v);
        }
    }
    if targets.is_empty() {
        // 成功读取的列表不含同 URL 订阅：远端没有可退订目标 = 意图已满足。
        return OutboxOutcome::Confirmed;
    }

    for rid in targets {
        match session.client.unsubscribe(rid).await {
            Ok(()) => {}
            Err(e) => {
                return OutboxOutcome::client_retry(
                    &e,
                    format!("退订 {}（远端 id {rid}）推送失败: {e}", item.feed_url),
                );
            }
        }
    }
    OutboxOutcome::Confirmed
}

/// 命令层一次性推送订阅 outbox（本地已入队；执行失败发布活动终态，
/// 队列保留，下一轮同步自动重试；会话构建失败由 [`record_outbox_session_block`]
/// 落账，A-P2-6）。命令返回不阻塞在网络上太久：与旧 `edit_remote_subscription`
/// 的调用成本同量级（都需构建一次会话/登录）。
///
/// B3：无论本轮是否发出请求，结束都广播一次订阅写回状态（`subscription-
/// outbox-changed`）——「入队刚发生而会话构建失败（离线/认证失败）」时，事件
/// 是 UI 看到「等待 N 项」的即时通道；执行器内的确认/失败路径已各自通知，
/// 重复载荷由 notify 单点同值去重（W2 复审 P3-2）：入队可见性由第一次发射
/// 保障，同值不重发。未配置同步时 notify 内部静默。
pub async fn try_push_subscription_outbox(db: &Arc<Mutex<Connection>>, http: &reqwest::Client) {
    let _ = push_subscription_changes(db, http).await;
}

pub async fn push_subscription_changes(
    db: &Arc<Mutex<Connection>>,
    http: &reqwest::Client,
) -> AppResult<SyncReport> {
    let runtime = runtime::for_db(db);
    let db = db.clone();
    let http = http.clone();
    runtime
        .run("subscriptions", async move {
            let result = push_subscription_changes_inner(&db, &http).await;
            notify_subscription_outbox_changed(&db).await;
            result
        })
        .await
}

async fn push_subscription_changes_inner(
    db: &Arc<Mutex<Connection>>,
    http: &reqwest::Client,
) -> AppResult<SyncReport> {
    runtime::check_generation(db).await?;
    let lease = match acquire_session(db, http).await {
        Ok(lease) => lease,
        // A-P2-6：构建失败不再静默吞掉——对齐推送侧 TASK-124 的 record_push_block：
        // 认证/端点/网络失败为当前账号 pending 行落账（attempts/last_error + 事件），
        // UI 摘要卡才能解释「为什么一直等待」。未配置（无归属身份）与代际失配
        // （旧会话作废，OPT-006）不落账。
        Err(ClientBuildFailure::Failed {
            err,
            endpoint,
            username,
        }) => {
            record_outbox_session_block(db, &err, &endpoint, &username).await;
            return Err(err);
        }
        Err(ClientBuildFailure::NotConfigured) => {
            return Err(crate::error::AppError::new(
                "notConnected",
                "未配置同步服务",
            ))
        }
        Err(ClientBuildFailure::Stale) => return Err(SyncSession::stale_error()),
        Err(ClientBuildFailure::CorruptGeneration(e)) => {
            log::warn!("sync: 订阅写回即时推送因同步代际损坏中止: {e}");
            return Err(e);
        }
    };
    runtime::accept_generation(lease.session.generation)?;
    let mut report = SyncReport::default();
    push_subscription_outbox(db, &lease.session, &mut report, None).await;
    // W3-1：401/认证类失败回落（命中缓存会话才触发；只重试一次）。
    if lease.from_cache && report_has_auth_failure(&report) {
        if let Some(fresh) = reauth_after_auth_failure(db, http, &lease).await {
            runtime::accept_generation(fresh.generation)?;
            let mut retry_report = SyncReport::default();
            push_subscription_outbox(db, &fresh, &mut retry_report, None).await;
            retry_report.preserve_changes_from(&report);
            report = retry_report;
        }
    }
    Ok(report)
}

/// 订阅 outbox 的会话构建失败落账（A-P2-6；对齐 push.rs record_push_block 的
/// TASK-124 模式）：有凭据但认证/端点/网络失败时，当前账号的全部 pending 队项
/// 累计一次失败尝试并记录摘要，确有标记行时广播 `subscription-outbox-changed`
/// ——「等待 N 项」必须能解释原因，而不是永远停在等待。
/// 摘要携带失败会话的端点/用户名（W2 复审 P3-4）：换号竞态下落账按当前账号
/// key，标注失败会话身份便于人工辨认归属偏差；快照身份未知（解密失败）不伪造。
/// 未配置（无账号身份可归属）与读取失败跳过；落账只做 DB 标记，事件在锁外发。
pub(super) async fn record_outbox_session_block(
    db: &Arc<Mutex<Connection>>,
    err: &crate::error::AppError,
    session_endpoint: &str,
    session_username: &str,
) {
    let summary = outbox_block_summary(err, session_endpoint, session_username);
    log::warn!("sync: 订阅写回被阻塞（会话构建失败）: {summary}");
    let marked = {
        let conn = db.lock().await;
        match super::current_account(&conn) {
            Ok(Some(ctx)) => db::mark_outbox_session_blocked(&conn, &ctx.key, &summary),
            // 未配置：无账号身份可归属（B3 口径：未连接统计为空），不标记
            Ok(None) => Ok(0),
            Err(e) => {
                log::warn!("sync: 订阅 outbox 落账读取账号身份失败: {e}");
                Err(e)
            }
        }
    };
    match marked {
        Ok(n) if n > 0 => notify_subscription_outbox_changed(db).await,
        Ok(_) => {}
        Err(e) => {
            log::warn!("sync: 订阅 outbox 阻塞标记落库失败: {e}");
            emit_sync_error(&format!("订阅写回阻塞标记落库失败：{e}"));
        }
    }
}

/// 会话构建失败的落账摘要单点（W2 复审 P3-1/P3-4）。
/// 判定与 push.rs `push_block_summary` 同判据——**code=auth 之外还必须认
/// ClientLogin 文案**：GReader 凭据错误经 ClientLogin 报出，错误 code 实为
/// network（push.rs `push_block_summary_distinguishes_auth_from_network`
/// 锁定的协议事实），只判 code 会把 GReader 认证失败标成「网络/端点失败」。
/// `session_*` 是失败会话的凭据快照（见 `ClientBuildFailure::Failed`）；
/// 两者皆空（解密失败、身份未知）时不伪造标注。
fn outbox_block_summary(
    err: &crate::error::AppError,
    session_endpoint: &str,
    session_username: &str,
) -> String {
    let msg = err.message.trim();
    let core = if err.code == "auth" || msg.contains("ClientLogin") {
        format!("订阅写回认证失败：{msg}")
    } else {
        format!("订阅写回被阻塞（网络/端点失败）：{msg}")
    };
    if session_endpoint.is_empty() && session_username.is_empty() {
        core
    } else {
        format!("{core}（失败会话端点 {session_endpoint}，用户 {session_username}）")
    }
}

/// 拉远端分类+订阅，URL 碰撞合并（三段式：锁外拉取 → 锁内合并）
async fn mark_subscription_import_incomplete(
    db: &Arc<Mutex<Connection>>,
    session: &SyncSession,
    account_key: &str,
    report: &mut SyncReport,
) {
    let conn = db.lock().await;
    if session.ensure_current(&conn).is_err() {
        return;
    }
    if let Err(e) = db::mark_binding_incomplete(&conn, account_key) {
        report
            .errors
            .push(format!("写入订阅绑定不完整 marker 失败: {e}"));
    }
}

#[allow(clippy::too_many_arguments)]
fn insert_remote_feed_bound(
    conn: &mut Connection,
    url: &str,
    site_url: Option<&str>,
    title: &str,
    folder_id: i64,
    remote_id: i64,
) -> AppResult<i64> {
    let tx = conn.transaction()?;
    let feed_id = db::insert_feed_origin(
        &tx, url, site_url, title, None, folder_id, "inherit", false, false, "remote",
    )?;
    db::set_feed_remote_id(&tx, feed_id, remote_id)?;
    tx.commit()?;
    Ok(feed_id)
}

fn prune_historical_unbound_remote_orphans(
    conn: &Connection,
    remote_urls: &[String],
    report: &mut SyncReport,
) -> bool {
    let remote: std::collections::HashSet<String> = remote_urls
        .iter()
        .map(|url| db::normalize_url(url))
        .collect();
    let candidates = match db::unbound_remote_feeds(conn) {
        Ok(v) => v,
        Err(e) => {
            report.errors.push(format!(
                "历史未绑定远端源清理：读取候选失败（fail-closed）: {e}"
            ));
            return false;
        }
    };
    for (feed_id, feed_url) in candidates {
        if remote.contains(&db::normalize_url(&feed_url)) {
            continue;
        }
        if let Err(e) = db::delete_feed(conn, feed_id) {
            report
                .errors
                .push(format!("历史未绑定远端源 {} 清理失败: {e}", feed_url));
            return false;
        }
    }
    true
}

pub(super) async fn pull_feeds(
    db: &Arc<Mutex<Connection>>,
    session: &SyncSession,
    report: &mut SyncReport,
) {
    // OPT-006：发起拉取前先复核代际——账号已切换时不向新服务端发旧请求。
    if !session_is_current(db, session).await {
        report
            .errors
            .push("同步会话已失效（账号配置已变更），跳过订阅拉取".into());
        return;
    }
    let account_key = {
        let conn = db.lock().await;
        match super::current_account(&conn) {
            Ok(Some(ctx)) => ctx.key,
            Ok(None) => {
                report
                    .errors
                    .push("订阅拉取：当前无同步账号身份，本轮跳过（fail-closed）".into());
                return;
            }
            Err(e) => {
                report.errors.push(format!(
                    "订阅拉取：读取同步账号身份失败，本轮跳过（fail-closed）: {e}"
                ));
                return;
            }
        }
    };
    // Google Reader：subscription/list 同时含订阅 + 分类（categories 里的 label/folder）。
    // tag/list 提供独立分类（含空分类）。分类映射用 tag/list 的 label。
    let (remote_tags, remote_subs) =
        match tokio::join!(session.client.tags(), session.client.subscriptions()) {
            (Ok(t), Ok(s)) => (t, s),
            (Err(e), _) | (_, Err(e)) => {
                report.push_client_error(&e, format!("拉取订阅失败: {e}"));
                mark_subscription_import_incomplete(db, session, &account_key, report).await;
                return;
            }
        };

    let mut import_incomplete = false;
    let mut subscriptions_exhausted = true;

    // 锁内：分类按 label 匹配本地 folder，不存在则创建。
    // Google Reader 分类没有数字 id，用 label 名做键（与 Miniflux 的 category id 不同）。
    // 本地 folder 的 remote_id 存「分类在远端无数字 id」，此处用 label 名作为稳定键——
    // 简单起见：按 label 名 upsert 本地 folder，不维护 remote_id（分类碰撞用名字）。
    {
        let conn = db.lock().await;
        // OPT-006：建分类是回写边界——响应在途期间账号被切换则整段丢弃。
        if let Err(e) = session.ensure_current(&conn) {
            log::warn!("sync: 分类导入前账号代际已变更，丢弃本轮响应: {e}");
            report
                .errors
                .push("同步会话已失效（账号配置已在其他窗口变更），本轮订阅响应已丢弃".into());
            return;
        }
        // 分类规范化收口在 greader::category_name（OPT-004）：
        // Miniflux folder tag 带 label；FreshRSS folder tag 只有 {id,type:folder}
        // （从 user/.../label/ 后缀取名）；state tag（含 org.freshrss/main）一律不成目录。
        // Note: 分类识别规则必须与固定上游源码一致 — 见 .agents/notes/implemented/architecture/2026-10-08-Reader适配的鉴权与分类契约.md
        let remote_labels: Vec<String> = remote_tags
            .iter()
            .filter_map(|t| greader::category_name(&t.id, t.label.as_deref(), t.r#type.as_deref()))
            .collect();
        // A-4 / B2-4：分类墓碑——改名/删除过的 label 不复活；远端已不再列出即清
        // 墓碑（只清**当前账号**的，见 db::folders 墓碑段）。
        // 失败关闭：墓碑读取/回收或账号身份读取**任一失败 = 状态未知**——不得按
        // 空集合继续（空集合会把已改名/删除的分类按远端旧 label 复活，并静默丢弃
        // 当前账号的墓碑），记同步错误并跳过本轮分类与订阅导入（订阅导入依赖
        // label → folder 映射，映射不可信时不写任何依赖它的本地状态）。
        let active = match active_folder_tombstones(&conn, &account_key, &remote_labels) {
            Ok(v) => v,
            Err(e) => {
                report.errors.push(format!(
                    "分类墓碑读取/回收失败，本轮跳过分类与订阅导入（fail-closed，保留原值待重试）: {e}"
                ));
                if let Err(mark_err) = db::mark_binding_incomplete(&conn, &account_key) {
                    report.errors.push(format!(
                        "分类墓碑失败后写入绑定不完整 marker 失败: {mark_err}"
                    ));
                }
                return;
            }
        };
        for label in &remote_labels {
            if active.iter().any(|t| t == label) {
                continue; // 本地已改名/删除：不按远端旧 label 复活目录
            }
            let existing = match db::find_folder_by_name(&conn, label) {
                Ok(v) => v,
                Err(e) => {
                    import_incomplete = true;
                    report
                        .errors
                        .push(format!("读取远端分类「{label}」对应本地目录失败: {e}"));
                    continue;
                }
            };
            if existing.is_none() {
                // TASK-056 修复轮 1（审查 FINDING 3）：此前是 `let _ = create_folder(...)`。
                // 静默失败会让后续 find_folder_by_name 落空 → 该分类下的订阅被改挂「未分类」，
                // **用户的目录结构无声丢失**——与本任务修复的外键缺陷同族。
                if let Err(e) = db::create_folder(&conn, label, "article") {
                    import_incomplete = true;
                    report
                        .errors
                        .push(format!("建远端分类「{label}」失败: {e}"));
                }
            }
        }
    }

    // 锁内：订阅按 URL 碰撞合并
    {
        let mut conn = db.lock().await;
        // OPT-006：绑定/建源也是回写边界——代际失配整段丢弃（不建源、不绑定）。
        if let Err(e) = session.ensure_current(&conn) {
            log::warn!("sync: 订阅导入前账号代际已变更，丢弃本轮响应: {e}");
            report
                .errors
                .push("同步会话已失效（账号配置已在其他窗口变更），本轮订阅响应已丢弃".into());
            return;
        }
        // A-1 + OPT-008A R1/R2：本地已删除（墓碑）的订阅不复活；远端列表已不含的
        // 墓碑可清除。匹配/收敛必须跨算法版本兼容（旧算法键在 legacy 命名空间，
        // 见 db::feeds 墓碑段），不能在 subscriptions 里重算或混用算法。
        // R2：墓碑读取/清理任何错误 = 状态未知——本轮停止订阅导入并记入 report，
        // setting 原值保留（修复后下一轮自然重试）；绝不能把「未知」当「空表」
        // 放行导入（会复活用户已删除的订阅）。
        let remote_urls: Vec<String> = remote_subs.iter().map(|rf| rf.url.clone()).collect();
        let tombstones_known = match db::prune_feed_tombstones(&conn, &remote_urls) {
            Ok(()) => true,
            Err(e) => {
                report.errors.push(format!(
                    "订阅墓碑读取/清理失败，本轮停止订阅导入（保留原值待重试）: {e}"
                ));
                false
            }
        };
        if !tombstones_known {
            import_incomplete = true;
            subscriptions_exhausted = false;
        }
        for rf in &remote_subs {
            if import_incomplete || !tombstones_known {
                subscriptions_exhausted = false;
                break; // 状态未知/已有失败：停止后续订阅写入
            }
            match db::feed_url_tombstoned(&conn, &rf.url) {
                // known tombstone 是用户本机主动删除的预期 continue，不计 incomplete。
                Ok(true) => continue,
                Ok(false) => {}
                Err(e) => {
                    import_incomplete = true;
                    subscriptions_exhausted = false;
                    report.errors.push(format!(
                        "订阅墓碑读取失败，本轮停止订阅导入（保留原值待重试）: {e}"
                    ));
                    break;
                }
            }
            let Some(remote_feed_id) = greader::parse_feed_numeric_id(&rf.id) else {
                import_incomplete = true;
                report.errors.push(format!(
                    "远端订阅 {} 缺少可绑定的数字 feed id（保留原值待重试）",
                    rf.url
                ));
                continue;
            };
            // 稳定远端 ID 优先；弱 URL 仅兜底未绑定源。元数据与绑定同事务，
            // 唯一 URL 冲突不能改绑或删旧建新。
            let local_feed = match db::match_remote_feed(
                &conn,
                remote_feed_id,
                &rf.url,
                &rf.title,
                rf.html_url.as_deref(),
            ) {
                Ok(v) => v,
                Err(e) => {
                    import_incomplete = true;
                    report.errors.push(format!(
                        "订阅 {} 身份匹配/绑定更新失败（保留原值待重试）: {e}",
                        rf.url
                    ));
                    continue;
                }
            };
            // 分类归属：subscription 的第一个分类 category。
            // FreshRSS 的 category 形如 {id:"user/-/label/名", label:"名"}——**没有 type**，
            // 靠 id 的 label 前缀识别；Miniflux 仍是 type:folder。规则同 tag/list 收口在
            // greader::category_name。
            let remote_folder_label = rf.categories.iter().find_map(|c| {
                greader::category_name(&c.id, c.label.as_deref(), c.r#type.as_deref())
            });
            match local_feed {
                Some((_lid, changed)) => {
                    report.feeds_data_changed |= changed;
                }
                None => {
                    // 本地没有 → 建本地 feed（挂到远端分类对应的本地 folder）
                    //
                    // 目录兜底（TASK-056）：远端无分类（或 label 在本地找不到）时必须挂到一个
                    // **真实存在**的 folder。此前是 `get_first_folder_id().unwrap_or(1)`，
                    // 而新装应用 folders 表为空 → 硬编码的 1 指向不存在目录 →
                    // `feeds.folder_id REFERENCES folders(id)` 外键违约（连接开启了
                    // `PRAGMA foreign_keys=ON`）→ 插入失败。改用既有
                    // `ensure_uncategorized_folder`（不存在则创建「未分类」），保证外键成立。
                    let folder_id: i64 = match remote_folder_label.as_deref() {
                        Some(label) => match db::find_folder_by_name(&conn, label) {
                            Ok(Some(fid)) => fid,
                            Ok(None) => match db::ensure_uncategorized_folder(&conn) {
                                Ok(fid) => fid,
                                Err(e) => {
                                    import_incomplete = true;
                                    report
                                        .errors
                                        .push(format!("订阅 {} 建目录失败: {e}", rf.url));
                                    continue;
                                }
                            },
                            Err(e) => {
                                import_incomplete = true;
                                report
                                    .errors
                                    .push(format!("订阅 {} 本地目录读取失败: {e}", rf.url));
                                continue;
                            }
                        },
                        None => match db::ensure_uncategorized_folder(&conn) {
                            Ok(fid) => fid,
                            Err(e) => {
                                import_incomplete = true;
                                report
                                    .errors
                                    .push(format!("订阅 {} 建目录失败: {e}", rf.url));
                                continue;
                            }
                        },
                    };
                    let inserted = insert_remote_feed_bound(
                        &mut conn,
                        &rf.url,
                        rf.html_url.as_deref(),
                        &rf.title,
                        folder_id,
                        remote_feed_id,
                    );
                    // TASK-056：失败必须可见。此前是无 else 的 `if let Ok(fid)`，
                    // 错误被静默吞掉、report.errors 保持为空，前端仍提示「已拉取订阅源」
                    // 「后端同步完成」——用户看到成功、实际零订阅。
                    match inserted {
                        Ok(_) => {
                            report.pulled_feeds += 1;
                        }
                        Err(e) => {
                            import_incomplete = true;
                            report.errors.push(format!(
                                "拉取订阅 {} 建本地并绑定失败（事务已回滚）: {e}",
                                rf.url
                            ));
                        }
                    }
                }
            }
        }
        if !prune_historical_unbound_remote_orphans(&conn, &remote_urls, report) {
            import_incomplete = true;
        }
    }

    if import_incomplete || !subscriptions_exhausted {
        mark_subscription_import_incomplete(db, session, &account_key, report).await;
        return;
    }

    // ============================================================
    // P3-11（TASK-074，DEC-req104-p3-11-remote-unsub-20260920）：
    // 远端退订 → 本地同步删除。此前 pull_feeds 只做 upsert、没有删除分支，
    // 远端退掉的订阅在本地永久残留（文章与未读计数与远端长期不一致）。
    //
    // 边界（owner 裁决的逐条落地）：
    // ① 只删「服务端来源」的源：origin='remote' 且 remote_id IS NOT NULL。
    //    本地直连添加（origin='local'）与未绑定源一律保留——即使用户在服务端
    //    退掉了曾经 URL 碰撞绑定的本地源，也不动本地数据（这是最容易丢数据的
    //    方向，故从保守侧处理）。
    // ② 已绑定源只按明确的远端 ID 判断退订，URL 改变不代表订阅消失。
    // ③ **不写 feed_tombstone**：墓碑的语义是「用户本地删除、不许复活」。这里
    //    是跟随远端事实删除；若用户在服务端重新订阅，下一轮 pull 应当把源正常
    //    建回来。写墓碑反而会让它在服务端重新出现时被永久跳过。
    // ④ pending 保护：队列里还有未推送的该 URL 变更时不删——本地动作尚未回传，
    //    不能被远端旧快照抢先抹掉。保护集合按**当前账号**读取（异账号暂停意图
    //    不参与本账号快照），读取失败 fail-closed 跳过本轮删除
    //    （见 [`pending_protected_feed_ids`]，B1-R3 复审 P2-3 / P3-4）。
    // ============================================================
    {
        let conn = db.lock().await;
        // OPT-006：远端退订对账的本地删除是回写边界——代际失配时整段跳过
        //（换号后新账号的订阅列表与本轮旧响应无对应关系，绝不能删）。
        if let Err(e) = session.ensure_current(&conn) {
            log::warn!("sync: 退订对账前账号代际已变更，跳过删除段: {e}");
            return;
        }
        let remote_id_set: std::collections::HashSet<i64> = remote_subs
            .iter()
            .filter_map(|rf| greader::parse_feed_numeric_id(&rf.id))
            .collect();
        // 查询失败不阻塞本轮（订阅层是尽力而为）：记错误并跳过删除段
        let candidates: Vec<(i64, String, i64)> = match conn
            .prepare(
                "SELECT id, feed_url, remote_id FROM feeds WHERE origin = 'remote' AND remote_id IS NOT NULL",
            )
            .and_then(|mut stmt| {
                let rows =
                    stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))?;
                rows.collect::<Result<Vec<_>, _>>()
            }) {
            Ok(v) => v,
            Err(e) => {
                import_incomplete = true;
                report
                    .errors
                    .push(format!("远端退订对账：读取候选源失败，本轮跳过删除: {e}"));
                Vec::new()
            }
        };
        // 队列里仍有未推送变更的源——这些源本轮不删。
        //
        // 两条来源都要覆盖（TASK-075 独立审查 FINDING：只查 feed_url 会漏判）：
        //  ① feed_url 非空的队项（本地新增/订阅类变更，直接按 URL 记账）；
        //  ② **article 级队项**（read/unread/star/unstar 由 commands/articles.rs 的
        //     record_read_state / record_star_state / mark_all_read 入队，这些行的
        //     feed_url 恒为 NULL，只能经 articles.feed_id 反查所属源）。
        // 只查 ① 时，离线期间「标星/已读但未推送」的源会被远端退订连源带文章一起删掉，
        // 未推送状态也被静默丢弃——正是本保护要防的静默数据丢失。
        //
        // B1-R3 复审（P3-4 fail-closed / P2-3 账号限定）：
        // - 保护集合读取**任一** prepare/query/row 错误 = 状态未知 → 记同步错误并
        //   跳过本轮删除对账；绝不能把「未知」当「空集合」继续删除（会误删本地订阅）；
        // - 订阅写回 outbox 只计**当前同步账号**的 pending 意图：异账号的暂停行
        //   不得保护本账号快照（否则换号后本账号的远端删除对账永久失效）。
        // Note: 保护集合的账号限定与 fail-closed — 见 .agents/notes/implemented/architecture/2026-10-14-订阅写回outbox执行器.md
        let account_key = match super::current_account(&conn) {
            Ok(Some(ctx)) => ctx.key,
            Ok(None) => {
                report
                    .errors
                    .push("远端退订对账：当前无同步账号身份，本轮跳过删除（fail-closed）".into());
                if let Err(mark_err) = db::mark_binding_incomplete(&conn, &account_key) {
                    report
                        .errors
                        .push(format!("写入绑定不完整 marker 失败: {mark_err}"));
                }
                return;
            }
            Err(e) => {
                report.errors.push(format!(
                    "远端退订对账：读取同步账号身份失败，本轮跳过删除（fail-closed）: {e}"
                ));
                if let Err(mark_err) = db::mark_binding_incomplete(&conn, &account_key) {
                    report
                        .errors
                        .push(format!("写入绑定不完整 marker 失败: {mark_err}"));
                }
                return;
            }
        };
        let pending_feed_ids = match pending_protected_feed_ids(&conn, &account_key) {
            Ok(v) => v,
            Err(e) => {
                report.errors.push(format!(
                    "远端退订对账：读取本地未推送保护集合失败，本轮跳过删除（fail-closed）: {e}"
                ));
                let marker = db::mark_binding_incomplete(&conn, &account_key);
                if let Err(mark_err) = marker {
                    report
                        .errors
                        .push(format!("写入绑定不完整 marker 失败: {mark_err}"));
                }
                return;
            }
        };
        for (feed_id, feed_url, remote_id) in candidates {
            if remote_id_set.contains(&remote_id) {
                continue; // 远端仍订阅：保留
            }
            if pending_feed_ids.contains(&feed_id) {
                continue; // ④ 本地变更未推送（含 article 级）：不删，等回传后再收敛
            }
            match db::delete_feed(&conn, feed_id) {
                Ok(()) => {
                    report.removed_feeds += 1;
                    log::info!(
                        "pull_feeds: removed locally-deleted remote subscription {feed_url}"
                    );
                }
                Err(e) => {
                    import_incomplete = true;
                    report
                        .errors
                        .push(format!("远端已退订、删除本地源 {feed_url} 失败: {e}"));
                }
            }
        }
    }

    let conn = db.lock().await;
    if let Err(e) = session.ensure_current(&conn) {
        log::warn!("sync: 写入绑定完整性标记前账号已变更，丢弃本轮结果: {e}");
        return;
    }
    let should_mark = import_incomplete || !subscriptions_exhausted;
    let marker = if should_mark {
        db::mark_binding_incomplete(&conn, &account_key)
    } else {
        db::clear_binding_incomplete(&conn, &account_key)
    };
    if let Err(e) = marker {
        report.errors.push(format!(
            "写入账号绑定完整性标记失败（{}）: {e}",
            if should_mark {
                "incomplete"
            } else {
                "complete"
            }
        ));
    }
}

/// 分类墓碑的「回收 + 读取」单点（fail-closed，B2-4）：先按当前账号回收远端
/// 已不再列出的墓碑（写），再返回当前账号生效的 label 集合（本账号 + 无归属的
/// 空 key 墓碑）。任一读写失败原样上抛——调用方必须记同步错误并跳过本轮分类/
/// 订阅导入，不得把「状态未知」当「无墓碑」继续（会复活已改名/删除的分类）。
///
/// 异账号墓碑既不生效也不回收：换号后 B 的同名分类可以正常导入，回 A 时旧墓碑
/// 仍防复活（跨账号隔离，见 db::folders 墓碑段）。
// Note: 墓碑的账号隔离与读取失败关闭 — 见 .agents/notes/implemented/architecture/2026-10-14-分类墓碑账号隔离.md
fn active_folder_tombstones(
    conn: &Connection,
    account_key: &str,
    remote_labels: &[String],
) -> AppResult<Vec<String>> {
    db::prune_folder_tombstones(conn, account_key, remote_labels)?;
    db::folder_tombstones_for(conn, account_key)
}

/// 删除对账的保护集合（fail-closed）：本地尚未回传变更的源 id 集合。
///
/// 覆盖三类尚未回传的变更：
/// - `sync_queue` 的 article 级队项（经 articles.feed_id 反查所属源）；
/// - `sync_queue` 的地址型队项；
/// - **当前账号**（`account_key`）的订阅 outbox pending 意图——异账号暂停的行
///   不属于本账号快照（B1-R3 复审 P2-3）。
///
/// 任一 prepare/query/row 错误**原样上抛**：调用方必须记同步错误并跳过本轮删除，
/// 不得把「状态未知」当「无保护」继续删（B1-R3 复审 P3-4）。
fn pending_protected_feed_ids(
    conn: &Connection,
    account_key: &str,
) -> AppResult<std::collections::HashSet<i64>> {
    let mut pending_feed_ids: std::collections::HashSet<i64> = conn
        .prepare(
            "SELECT DISTINCT a.feed_id FROM sync_queue q
             JOIN articles a ON a.id = q.article_id
             WHERE q.article_id IS NOT NULL",
        )?
        .query_map([], |r| r.get::<_, i64>(0))?
        .collect::<Result<std::collections::HashSet<_>, _>>()?;
    let mut pending_urls: std::collections::HashSet<String> = conn
        .prepare("SELECT DISTINCT feed_url FROM sync_queue WHERE feed_url IS NOT NULL")?
        .query_map([], |r| r.get::<_, String>(0))?
        .map(|row| row.map(|u| db::normalize_url(&u)))
        .collect::<Result<std::collections::HashSet<_>, _>>()?;
    for row in conn
        .prepare(
            "SELECT DISTINCT feed_url FROM subscription_outbox
              WHERE status = 'pending' AND account_key = ?1",
        )?
        .query_map(rusqlite::params![account_key], |r| r.get::<_, String>(0))?
    {
        pending_urls.insert(db::normalize_url(&row?));
    }
    // 地址型队项还要能匹配到具体的源，统一折算成 feed_id，供下面单条件判断
    if !pending_urls.is_empty() {
        for row in conn
            .prepare("SELECT id, feed_url FROM feeds")?
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
        {
            let (id, url) = row?;
            if pending_urls.contains(&db::normalize_url(&url)) {
                pending_feed_ids.insert(id);
            }
        }
    }
    Ok(pending_feed_ids)
}

/* ============================================================
W2 复审 P3-1：会话构建失败摘要的判定单测
（行为验证由 CI 执行；本机不可链接）
============================================================ */
#[cfg(test)]
mod outbox_block_summary_tests {
    use super::outbox_block_summary;
    use crate::error::AppError;

    /// 与 push.rs `push_block_summary_distinguishes_auth_from_network` 同形：
    /// GReader 凭据错误经 ClientLogin 报出、错误 code 实为 network——只判
    /// code 会把它标成「网络/端点失败」（判别力：删掉 ClientLogin 判据必红）。
    #[test]
    fn outbox_block_summary_distinguishes_auth_from_network() {
        // Fever：code=auth
        let fever_auth = AppError::new("auth", "Fever 认证失败（api_key 不正确）");
        assert!(outbox_block_summary(&fever_auth, "", "").starts_with("订阅写回认证失败："));

        // GReader：路径存在但凭据被拒（实际文案，code 为 network）
        let greader_401 = AppError::network(
            "ClientLogin → 401 Unauthorized（已定位 API：https://example.com/api/greader.php）",
        );
        assert!(
            outbox_block_summary(&greader_401, "", "").starts_with("订阅写回认证失败："),
            "GReader 凭据错误经 ClientLogin 报出（code=network），必须归入认证失败"
        );

        // 网络/端点失败：不冒充认证失败
        let dns = AppError::network("error sending request for url (https://example.com/)");
        assert!(outbox_block_summary(&dns, "", "").starts_with("订阅写回被阻塞（网络/端点失败）："));

        // 快照会话标注（P3-4）：非空时携带、便于辨认换号竞态下的归属；
        // 空（解密失败、身份未知）不伪造
        let with_session = outbox_block_summary(&dns, "https://a.example/api/greader.php", "alice");
        assert!(with_session.contains("失败会话端点 https://a.example/api/greader.php"));
        assert!(with_session.contains("用户 alice"));
        assert!(!outbox_block_summary(&dns, "", "").contains("失败会话端点"));
    }
}
