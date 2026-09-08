//! 同步引擎（Google Reader 兼容协议，后端 Miniflux）：
//!
//! ① Push：sync_queue 里的本地变更推到后端
//! ② Pull：拉远端订阅/分类/条目状态变化，URL 碰撞合并
//! ③ 兜底：直连失败的源从后端拉条目（source='miniflux'）
//! 本地未连接期间添加的源，首次 Pull 时按 URL 碰撞检测：
//!   远端无 → 推送创建；远端有 → 合并（remote id 绑定本地 feed）
//!
//! 锁纪律：与 refresh_feed_staged 相同的三段式——锁内读写 SQLite，
//! HTTP 全部在锁外执行，同步进行时其他 DB 命令不被冻结。
//!
//! 阶段划分（前端分步同步 + 后台自动同步复用）：
//!   feeds 阶段  = push_feeds + pull_feeds（订阅层，秒级）
//!   states 阶段 = push_queue + pull_entries（状态+条目层，慢）
//! sync_now = 两个阶段串联（全量路径，含绑定回填+全量状态对账）。

use crate::db::{self, NewArticle};
use crate::error::{AppError, AppResult};
use crate::fever;
use crate::greader::{self, GReaderClient, ItemContent};
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Default, Serialize)]
pub struct SyncReport {
    pub pushed_states: usize,
    pub pushed_feeds: usize,
    pub pulled_feeds: usize,
    pub pulled_entries: usize,
    pub merged_states: usize,
    pub fallback_entries: usize,
    pub errors: Vec<String>,
}

/* ============================================================
   凭据
   ============================================================ */

/// 后端凭据：协议 + endpoint + username + password。
/// username/password 是 Miniflux「集成」页单独配置的凭据（Google Reader 与
/// Fever 共用同一套集成凭据，非 Miniflux 账号密码）。
/// 协议从 settings 键 `sync_protocol` 读取（"greader" | "fever"，默认 "greader"）。
pub fn read_credentials(conn: &Connection) -> Option<(String, String, String, String)> {
    let protocol = db::get_setting(conn, "sync_protocol")
        .ok()
        .flatten()
        .filter(|p| p == "fever" || p == "greader")
        .unwrap_or_else(|| "greader".to_string());
    let endpoint = db::get_setting(conn, "greader_endpoint").ok().flatten()?;
    let username = db::get_setting(conn, "greader_username").ok().flatten()?;
    let password = db::get_setting(conn, "greader_password").ok().flatten()?;
    if endpoint.trim().is_empty() || username.trim().is_empty() || password.trim().is_empty() {
        return None;
    }
    Some((protocol, endpoint, username, password))
}

/// 协议无关后端客户端（Google Reader / Fever）。
/// sync 引擎只依赖这个枚举的统一方法，协议差异封装在内部。
pub enum Backend {
    GReader(GReaderClient),
    Fever(fever::FeverClient),
}

impl Backend {
    async fn subscriptions(&self) -> AppResult<Vec<greader::Subscription>> {
        match self {
            Backend::GReader(c) => c.subscriptions().await,
            Backend::Fever(c) => c.subscriptions().await,
        }
    }

    async fn tags(&self) -> AppResult<Vec<greader::TagRef>> {
        match self {
            Backend::GReader(c) => c.tags().await,
            Backend::Fever(c) => c.tags().await,
        }
    }

    async fn mark_read(&self, ids: &[i64]) -> AppResult<()> {
        match self {
            Backend::GReader(c) => c.mark_read(ids).await,
            Backend::Fever(c) => c.mark_read(ids).await,
        }
    }

    async fn mark_unread(&self, ids: &[i64]) -> AppResult<()> {
        match self {
            Backend::GReader(c) => c.mark_unread(ids).await,
            Backend::Fever(c) => c.mark_unread(ids).await,
        }
    }

    async fn mark_starred(&self, ids: &[i64]) -> AppResult<()> {
        match self {
            Backend::GReader(c) => c.mark_starred(ids).await,
            Backend::Fever(c) => c.mark_starred(ids).await,
        }
    }

    async fn mark_unstarred(&self, ids: &[i64]) -> AppResult<()> {
        match self {
            Backend::GReader(c) => c.mark_unstarred(ids).await,
            Backend::Fever(c) => c.mark_unstarred(ids).await,
        }
    }

    /// 订阅新源。Fever 协议无写订阅端点，降级为明确错误。
    async fn quick_add(&self, url: &str) -> AppResult<greader::QuickAddResponse> {
        match self {
            Backend::GReader(c) => c.quick_add(url).await,
            Backend::Fever(_) => Err(AppError::new(
                "unsupported",
                "Fever 协议不支持添加订阅，请在 Miniflux Web 端添加后重新同步",
            )),
        }
    }
}

/// 锁内读凭据 → 锁外按协议构建 client。
async fn build_client(db: &Arc<Mutex<Connection>>, http: &reqwest::Client) -> Option<Backend> {
    let (protocol, endpoint, username, password) = {
        let conn = db.lock().await;
        read_credentials(&conn)?
    };
    match protocol.as_str() {
        "fever" => {
            let client = fever::FeverClient::new(&endpoint, &username, &password, http.clone());
            match client.verify().await {
                Ok(()) => Some(Backend::Fever(client)),
                Err(e) => {
                    log::warn!("sync: Fever 认证失败: {e}");
                    None
                }
            }
        }
        _ => match GReaderClient::login(&endpoint, &username, &password, http.clone()).await {
            Ok(c) => Some(Backend::GReader(c)),
            Err(e) => {
                log::warn!("sync: ClientLogin 失败: {e}");
                None
            }
        },
    }
}

/* ============================================================
   ① Push：本地状态变更 → 后端（只推不拉）
   ============================================================ */

/// 全局推送互斥：同一时刻只允许一个推送在飞（防抖即时推送 vs 后台自动
/// 同步 vs 手动同步并发）。exec_push 成功后按 queue_id prune——并发时 A
/// 可能 prune 掉 B 正在推的项；更糟的是收藏 toggle 非幂等，交错执行会把
/// 星标状态翻转两次。串行化后两场景只会先后重推同一状态（幂等），无害。
static PUSH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 待推送动作的锁内快照：HTTP 执行所需的全部信息。
struct PushPlan {
    /// (队列 id, action, entry ids)——read 广播副本展开后
    status: Vec<PushStatus>,
    /// (队列 id, entry id)——收藏切换（star/unstar 语义，Google Reader 无 toggle）
    stars: Vec<(i64, i64, bool)>,
}

struct PushStatus {
    queue_id: i64,
    action: String,
    entry_ids: Vec<i64>,
}

/// 锁内：解析 sync_queue → 推送计划。
/// 条目未绑定 entry 的跳过（保留在队列，Pull 的绑定回填会补上，直接丢弃
/// 会让"已读"在服务端永久丢失）。
fn plan_push(conn: &Connection) -> AppResult<PushPlan> {
    let items = db::take_sync_queue(conn)?;
    let mut plan = PushPlan { status: Vec::new(), stars: Vec::new() };
    for item in items {
        let Some(article_id) = item.article_id else {
            continue; // feed 级动作（add_feed）在 push_feeds 阶段处理
        };
        let remote_id: Option<i64> = conn
            .query_row(
                "SELECT remote_id FROM articles WHERE id = ?1",
                [article_id],
                |r| r.get(0),
            )
            .ok()
            .flatten();
        let Some(remote_id) = remote_id else {
            continue;
        };
        match item.action.as_str() {
            // 已读广播：绑定的 entry + 记账的全部同文副本 entry 一并标读
            // （双端场景：Read You 不去重，手机上另一源的副本也要已读，
            // 否则手机读完这篇、那个源里又冒出来一篇未读的"同一篇"）
            "read" => {
                let mut ids = vec![remote_id];
                for dup in db::article_dup_entries(conn, article_id).unwrap_or_default() {
                    if dup != remote_id {
                        ids.push(dup);
                    }
                }
                plan.status.push(PushStatus { queue_id: item.id, action: "read".into(), entry_ids: ids });
            }
            "unread" => plan.status.push(PushStatus { queue_id: item.id, action: "unread".into(), entry_ids: vec![remote_id] }),
            "star" => plan.stars.push((item.id, remote_id, true)),
            "unstar" => plan.stars.push((item.id, remote_id, false)),
            _ => {}
        }
    }
    Ok(plan)
}

/// 锁外：执行推送计划。返回成功清除的队列 id（失败项保留 → 天然重试）。
async fn exec_push(client: &Backend, plan: &PushPlan, report: &mut SyncReport) -> Vec<i64> {
    let mut done: Vec<i64> = Vec::new();
    // read/unread 聚合批量（Google Reader edit-tag 单请求可携带全部 id + tag）
    for action in ["read", "unread"] {
        let ids: Vec<i64> = plan
            .status
            .iter()
            .filter(|s| s.action == action)
            .flat_map(|s| s.entry_ids.iter().copied())
            .collect::<Vec<_>>();
        if ids.is_empty() {
            continue;
        }
        let result = if action == "read" {
            client.mark_read(&ids).await
        } else {
            client.mark_unread(&ids).await
        };
        match result {
            Ok(()) => {
                report.pushed_states += ids.len();
                done.extend(plan.status.iter().filter(|s| s.action == action).map(|s| s.queue_id));
            }
            Err(e) => report.errors.push(format!("状态推送失败: {e}")),
        }
    }
    // 收藏：star/unstar（Google Reader 有明确的 add/remove 语义，非 toggle）
    for (qid, remote_id, want_star) in &plan.stars {
        let result = if *want_star {
            client.mark_starred(&[*remote_id]).await
        } else {
            client.mark_unstarred(&[*remote_id]).await
        };
        match result {
            Ok(()) => {
                report.pushed_states += 1;
                done.push(*qid);
            }
            Err(e) => report.errors.push(format!("收藏同步失败: entry {remote_id}: {e}")),
        }
    }
    done
}

/// 即时状态推送：只推 sync_queue（read/unread/star/unstar + 副本广播），
/// 不做任何 pull。set_read/set_starred 变更后 ~1s 内到达服务端。
/// 失败静默（队列保留，下轮同步重推）——后台同步不打扰用户。
pub async fn push_states_now(db: &Arc<Mutex<Connection>>, http: &reqwest::Client) {
    let Some(client) = build_client(db, http).await else {
        return;
    };
    // 串行化：与 states_phase/feeds_phase 的推送段互斥（见 PUSH_LOCK 注释）
    let _guard = PUSH_LOCK.lock().await;
    let plan = {
        let conn = db.lock().await;
        match plan_push(&conn) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("sync: 读队列失败: {e}");
                return;
            }
        }
    };
    if plan.status.is_empty() && plan.stars.is_empty() {
        return;
    }
    let mut report = SyncReport::default();
    let done = exec_push(&client, &plan, &mut report).await;
    if !done.is_empty() {
        let conn = db.lock().await;
        if let Err(e) = db::prune_sync(&conn, &done) {
            log::warn!("sync: 清队列失败: {e}");
        }
    }
    if !report.errors.is_empty() {
        log::info!("sync: 即时推送失败（队列保留待重推）: {:?}", report.errors);
    } else {
        log::info!("sync: 即时推送 {} 项状态", report.pushed_states);
    }
}

/* ============================================================
   ② Pull：远端 → 本地（订阅关系 + 状态 + 条目）
   ============================================================ */

/// 未连接期间本地新增的订阅推到远端（三段式：锁内读队列 → 锁外 HTTP → 锁内落库）
async fn push_feeds(db: &Arc<Mutex<Connection>>, client: &Backend, report: &mut SyncReport) {
    // add_feed 队列动作：锁内读出全部待处理项（feed_url + 目标分类）
    struct PendingFeed { queue_id: i64, url: String }
    let items: Vec<PendingFeed> = {
        let conn = db.lock().await;
        let mut out = Vec::new();
        for item in db::take_sync_queue(&conn).unwrap_or_default() {
            if item.action != "add_feed" {
                continue;
            }
            let Some(url) = item.feed_url else { continue };
            out.push(PendingFeed { queue_id: item.id, url });
        }
        out
    };
    if items.is_empty() {
        return;
    }
    // 锁外：逐个订阅（quick_add 自动发现 feed，幂等——已存在返回既有 stream_id）
    let mut done: Vec<i64> = Vec::new();
    for it in &items {
        match client.quick_add(&it.url).await {
            Ok(r) => {
                report.pushed_feeds += 1;
                done.push(it.queue_id);
                // 锁内：绑定本地 feed（URL 匹配），若 quick_add 返回了数字 id
                let conn = db.lock().await;
                if let Some(local_id) = db::feed_id_by_url(&conn, &it.url).ok().flatten() {
                    if let Some(stream_id) = r.stream_id.as_deref() {
                        if let Some(nid) = greader::parse_feed_numeric_id(stream_id) {
                            let _ = db::set_feed_remote_id(&conn, local_id, nid);
                        }
                    }
                }
                drop(conn);
            }
            Err(e) => report.errors.push(format!("推送订阅 {} 失败: {e}", it.url)),
        }
    }
    let conn = db.lock().await;
    let _ = db::prune_sync(&conn, &done);
    // remove_feed 语义（SUB-4/SYN-1）：本地删除不推远端。历史版本可能在
    // delete_feed 时建过 remove_feed 队项（从未被消费）——此处统一清僵尸项，
    // 与 delete_feed 命令「不再建 remove_feed」的新语义一致。
    let _ = db::purge_remove_feed_zombies(&conn);
    drop(conn);
}

/// 拉远端分类+订阅，URL 碰撞合并（三段式：锁外拉取 → 锁内合并）
async fn pull_feeds(db: &Arc<Mutex<Connection>>, client: &Backend, report: &mut SyncReport) {
    // Google Reader：subscription/list 同时含订阅 + 分类（categories 里的 label/folder）。
    // tag/list 提供独立分类（含空分类）。分类映射用 tag/list 的 label。
    let (remote_tags, remote_subs) = match tokio::join!(client.tags(), client.subscriptions()) {
        (Ok(t), Ok(s)) => (t, s),
        (Err(e), _) | (_, Err(e)) => {
            report.errors.push(format!("拉取订阅失败: {e}"));
            return;
        }
    };

    // 锁内：分类按 label 匹配本地 folder，不存在则创建。
    // Google Reader 分类没有数字 id，用 label 名做键（与 Miniflux 的 category id 不同）。
    // 本地 folder 的 remote_id 存「分类在远端无数字 id」，此处用 label 名作为稳定键——
    // 简单起见：按 label 名 upsert 本地 folder，不维护 remote_id（分类碰撞用名字）。
    {
        let conn = db.lock().await;
        for tag in &remote_tags {
            if tag.r#type.as_deref() != Some("folder") {
                continue; // 只处理 folder 类型（分类），跳过 starred/state
            }
            let label = tag.label.as_deref().unwrap_or_default();
            if label.is_empty() {
                continue;
            }
            // 按名称匹配本地 folder
            let existing: Option<i64> = conn
                .query_row(
                    "SELECT id FROM folders WHERE name = ?1",
                    rusqlite::params![label],
                    |r| r.get(0),
                )
                .ok()
                .flatten();
            if existing.is_none() {
                let _ = db::create_folder(&conn, label, "article");
            }
        }
    }

    // 锁内：订阅按 URL 碰撞合并
    {
        let conn = db.lock().await;
        for rf in &remote_subs {
            // 用规范化 URL 匹配本地 feed（后端返回的 URL 与本地直连添加时
            // 常有协议/www./尾斜杠/跟踪参数差异，精确匹配会漏判成新订阅 → 同一
            // 订阅出现两个本地 feed，文章翻倍、状态分裂、数量对不齐）。
            let local_feed = db::feed_id_by_url_normalized(&conn, &rf.url)
                .ok()
                .flatten();
            // 远端 feed 数字 id（读响应用 feed/数字）
            let remote_feed_id = greader::parse_feed_numeric_id(&rf.id);
            // 分类归属：subscription 的第一个 folder category
            let remote_folder_label = rf
                .categories
                .iter()
                .find(|c| c.r#type.as_deref() == Some("folder"))
                .and_then(|c| c.label.clone());
            match local_feed {
                Some(lid) => {
                    // 已存在（本地直连添加过）→ 绑定 remote_id，本地分类/布局保留。
                    if let Some(nid) = remote_feed_id {
                        let _ = db::set_feed_remote_id(&conn, lid, nid);
                    }
                    // 远端标题仅在本地标题等于 URL（从未抓取成功过）时回填
                    let _ = conn.execute(
                        "UPDATE feeds SET
                            title = CASE WHEN title = feed_url THEN ?1 ELSE title END,
                            site_url = COALESCE(site_url, ?2)
                         WHERE id = ?3",
                        rusqlite::params![rf.title, rf.html_url, lid],
                    );
                    report.merged_states += 1;
                }
                None => {
                    // 本地没有 → 建本地 feed（挂到远端分类对应的本地 folder）
                    let folder_id: i64 = remote_folder_label
                        .as_deref()
                        .and_then(|label| {
                            conn.query_row(
                                "SELECT id FROM folders WHERE name = ?1",
                                [label],
                                |r| r.get(0),
                            )
                            .ok()
                        })
                        .unwrap_or_else(|| {
                            conn.query_row("SELECT id FROM folders LIMIT 1", [], |r| r.get(0))
                                .unwrap_or(1)
                        });
                    let inserted = db::insert_feed_origin(
                        &conn,
                        &rf.url,
                        rf.html_url.as_deref(),
                        &rf.title,
                        None, // favicon 留空，交给本地直连抓取的 discover_favicon 发现
                        folder_id,
                        "inherit",
                        false,
                        false,
                        "remote",
                    );
                    if let Ok(fid) = inserted {
                        if let Some(nid) = remote_feed_id {
                            let _ = db::set_feed_remote_id(&conn, fid, nid);
                        }
                        report.pulled_feeds += 1;
                    }
                }
            }
        }
    }
}

/// 从 ItemContent 提取远端 feed 数字 id（origin.stream_id = feed/42）。
fn item_feed_id(e: &ItemContent) -> Option<i64> {
    e.origin
        .as_ref()
        .and_then(|o| greader::parse_feed_numeric_id(&o.stream_id))
}

/// 从 ItemContent 提取条目十进制 id（长格式 id 尾部十六进制）。
fn item_numeric_id(e: &ItemContent) -> Option<i64> {
    greader::parse_item_id(&e.id)
}

/// 从 ItemContent 提取原文 URL（alternate[0].href）。
fn item_url(e: &ItemContent) -> Option<String> {
    e.alternate.first().map(|a| a.href.clone())
}

/// 从 ItemContent 提取正文 HTML（content 优先，summary 兜底）。
fn item_content_html(e: &ItemContent) -> String {
    e.content
        .as_ref()
        .map(|c| c.content.clone())
        .or_else(|| e.summary.as_ref().map(|c| c.content.clone()))
        .unwrap_or_default()
}

/// 从 ItemContent 提取发布时间（unix 秒 → RFC3339）。
fn item_published_at(e: &ItemContent) -> String {
    if e.published > 0 {
        DateTime::from_timestamp(e.published, 0)
            .map(|d| d.with_timezone(&Utc).to_rfc3339())
            .unwrap_or_else(|| Utc::now().to_rfc3339())
    } else {
        Utc::now().to_rfc3339()
    }
}

/// 状态合并的守卫语义（pull_entries 与对账共用）：
/// - 待推保护：本地有未推送变更 → 跳过（本地优先，防乒乓）
/// - read-anywhere-wins：「读」是强意图，任何副本的已读都接受
/// - unread 只认绑定同源 entry：跨源副本的未读不能复活桌面已读
fn merge_remote_status(
    conn: &Connection,
    aid: i64,
    e: &ItemContent,
    maps: &mut db::SyncMatchMaps,
    report: &mut SyncReport,
) {
    let Some(eid) = item_numeric_id(e) else { return };
    let feed_id = item_feed_id(e);
    let _ = db::set_article_remote_id(conn, aid, eid);
    // 同步 maps 的绑定状态：后续 entry 若 URL 兜底匹配到同一 aid，能读到
    // 「已绑定 eid」而非批量快照里的「未绑定」，避免跨源同 URL 副本被误判
    // 为自己的条目（时序偏差）。
    maps.id_to_mf_id.insert(aid, Some(eid));
    maps.id_to_mf_pair.insert(aid, (Some(eid), maps.id_to_mf_pair.get(&aid).map(|p| p.1).unwrap_or(None)));
    maps.mf_id_to_article.insert(eid, aid);
    if maps.pending_ids.contains(&aid) {
        return;
    }
    let remote_read = greader::has_tag(&e.categories, "/com.google/read");
    let remote_starred = greader::has_tag(&e.categories, "/com.google/starred");
    let local_bound = db::article_by_remote_id(conn, eid).ok().flatten().is_some();
    let same_feed_trusted = maps
        .id_to_mf_pair
        .get(&aid)
        .map(|(entry_mf, feed_mf)| match entry_mf {
            Some(_) => *feed_mf == feed_id,
            None => true,
        })
        .unwrap_or(false);
    let accept_unread = local_bound && same_feed_trusted;
    if remote_read || accept_unread {
        let _ = conn.execute(
            "UPDATE articles SET is_read = ?1, is_starred = ?2 WHERE id = ?3",
            rusqlite::params![remote_read as i64, remote_starred as i64, aid],
        );
        report.pulled_entries += 1;
    }
}

/// 拉远端条目（新条目 + 状态变化），按 remote_id/URL 匹配合并。
/// 分页三段式：每页「锁外拉取 → 锁内合并」，锁从不跨分页 HTTP await。
/// `full=true`（手动同步/首连）：先做绑定回填 + 全量状态对账。
/// `full=false`（后台自动同步）：只拉增量（ot 游标），便宜。
async fn pull_entries_greader(
    db: &Arc<Mutex<Connection>>,
    client: &GReaderClient,
    report: &mut SyncReport,
    full: bool,
) {
    let since_s = {
        let conn = db.lock().await;
        db::last_sync_ts(&conn).unwrap_or(0)
    };

    // 拉取目标：reading-list 全部条目 id（分页），full 时 ot=0（全量），增量时 ot=since_s
    let ot = if full { Some(0i64) } else { Some(since_s) };
    let mut all_item_ids: Vec<i64> = Vec::new();
    let mut continuation: Option<u64> = None;
    loop {
        let r = match client
            .item_ids(
                "user/-/state/com.google/reading-list",
                ot,
                None,
                Some(1000),
                continuation,
            )
            .await
        {
            Ok(r) => r,
            Err(e) => {
                report.errors.push(format!("拉取条目 id 失败: {e}"));
                break;
            }
        };
        let mut got = 0;
        for it in &r.item_refs {
            if let Ok(id) = it.id.parse::<i64>() {
                all_item_ids.push(id);
                got += 1;
            }
        }
        match r.continuation.and_then(|c| c.parse::<u64>().ok()) {
            Some(c) if got > 0 => continuation = Some(c),
            _ => break,
        }
    }

    // 分批拉正文（每次 100 条，避免单请求过大），锁内合并
    let mut maps = {
        let conn = db.lock().await;
        db::sync_match_maps(&conn).unwrap_or_else(|e| {
            report.errors.push(format!("同步匹配映射构建失败: {e}"));
            db::SyncMatchMaps {
                url_to_id: Default::default(),
                id_to_mf_id: Default::default(),
                id_to_mf_pair: Default::default(),
                pending_ids: Default::default(),
                feed_mf_to_id: Default::default(),
                mf_id_to_article: Default::default(),
            }
        })
    };

    for chunk in all_item_ids.chunks(100) {
        let entries = match client.item_contents(chunk).await {
            Ok(v) => v,
            Err(e) => {
                report.errors.push(format!("拉取条目正文失败: {e}"));
                continue;
            }
        };
        let conn = db.lock().await;
        for e in &entries {
            merge_pulled_entry(&conn, e, &mut maps, report);
        }
        drop(conn);
    }

    // 轻量同步（full=false）状态对账：增量 item_contents 只覆盖「变更过的」条目，
    // 漏掉「手机很早前标读 / 收藏、changed_at 早于游标」的旧变更。这里用 read /
    // starred 权威 id 集合补齐（与 Fever 的 unread/saved 对账对称）。
    if !full {
        let (read_ids, starred_ids) = tokio::join!(
            fetch_stream_ids(client, greader::tags::READ),
            fetch_stream_ids(client, greader::tags::STARRED),
        );
        let read_ids = read_ids.unwrap_or_default();
        let starred_ids = starred_ids.unwrap_or_default();
        let conn = db.lock().await;
        reconcile_reader_state(&conn, &read_ids, &starred_ids, &maps, report);
        drop(conn);
    }

    // 更新游标（unix 秒）
    let now = Utc::now().timestamp();
    let conn = db.lock().await;
    let _ = db::set_last_sync_ts(&conn, now);
    drop(conn);

}

/// 分页拉取某 Google Reader stream 的全部条目 id（read / starred 权威集合）。
async fn fetch_stream_ids(client: &GReaderClient, stream: &str) -> AppResult<Vec<i64>> {
    let mut ids: Vec<i64> = Vec::new();
    let mut continuation: Option<u64> = None;
    loop {
        let r = client
            .item_ids(stream, Some(0), None, Some(1000), continuation)
            .await?;
        let mut got = 0;
        for it in &r.item_refs {
            if let Ok(id) = it.id.parse::<i64>() {
                ids.push(id);
                got += 1;
            }
        }
        match r.continuation.and_then(|c| c.parse::<u64>().ok()) {
            Some(c) if got > 0 => continuation = Some(c),
            _ => break,
        }
    }
    Ok(ids)
}

/// Google Reader 权威状态对账（轻量同步用）：
/// - read 集合含 remote_id → 远端已读 → 本地已读（read-wins，不反向复活未读）
/// - starred 集合含 remote_id → 本地收藏；不含 → 取消收藏
/// - pending 保护：本地有未推送变更的条目跳过，防「刚标读/刚收藏」被远端快照回滚
fn reconcile_reader_state(
    conn: &Connection,
    read_ids: &[i64],
    starred_ids: &[i64],
    maps: &db::SyncMatchMaps,
    report: &mut SyncReport,
) {
    use std::collections::HashSet;
    let read_set: HashSet<i64> = read_ids.iter().copied().collect();
    let starred_set: HashSet<i64> = starred_ids.iter().copied().collect();

    for (remote_id, aid) in &maps.mf_id_to_article {
        let aid = *aid;
        if maps.pending_ids.contains(&aid) {
            continue; // 交给 push 段队列，不被远端快照回滚
        }
        if read_set.contains(remote_id) {
            if let Ok(n) = conn.execute(
                "UPDATE articles SET is_read = 1 WHERE id = ?1 AND is_read = 0",
                rusqlite::params![aid],
            ) {
                report.merged_states += n;
            }
        }
        if starred_set.contains(remote_id) {
            if let Ok(n) = conn.execute(
                "UPDATE articles SET is_starred = 1 WHERE id = ?1 AND is_starred = 0",
                rusqlite::params![aid],
            ) {
                report.merged_states += n;
            }
        } else if let Ok(n) = conn.execute(
            "UPDATE articles SET is_starred = 0 WHERE id = ?1 AND is_starred = 1",
            rusqlite::params![aid],
        ) {
            report.merged_states += n;
        }
    }
}

/// 共享：合并一条已拉取的远端条目（URL 兜底匹配 / remote_id 直配 / 同源判定 /
/// 跨源副本记账 / 新条目 upsert）。Google Reader 与 Fever 两条 pull 路径共用。
fn merge_pulled_entry(
    conn: &Connection,
    e: &ItemContent,
    maps: &mut db::SyncMatchMaps,
    report: &mut SyncReport,
) {
    // URL 兜底匹配（规范化）
    let aid = item_url(e)
        .as_deref()
        .and_then(|u| maps.url_to_id.get(&db::normalize_url(u)).copied())
        .or_else(|| {
            // 已绑定 remote_id 直配
            item_numeric_id(e).and_then(|eid| maps.mf_id_to_article.get(&eid).copied())
        });
    match aid {
        Some(aid) => {
            // 已存在：判断是否自己的条目（同源），合并状态；跨源副本记账
            let eid = item_numeric_id(e);
            let feed_id = item_feed_id(e);
            let bound_entry = maps.id_to_mf_id.get(&aid).copied().flatten();
            let is_own = bound_entry == eid
                || (bound_entry.is_none()
                    && maps
                        .id_to_mf_pair
                        .get(&aid)
                        .map(|(entry_mf, feed_mf)| match entry_mf {
                            Some(_) => *feed_mf == feed_id,
                            None => true,
                        })
                        .unwrap_or(false));
            if !is_own {
                // 跨源副本：记账（已读广播对象）。read-anywhere-wins
                if let Some(eid) = eid {
                    let _ = db::add_article_dup_entry(conn, aid, eid);
                }
                if greader::has_tag(&e.categories, "/com.google/read")
                    && !maps.pending_ids.contains(&aid)
                {
                    let _ = conn.execute(
                        "UPDATE articles SET is_read = 1 WHERE id = ?1",
                        rusqlite::params![aid],
                    );
                }
                return;
            }
            merge_remote_status(conn, aid, e, maps, report);
            // 正文/封面/enclosure 兜底回填：本地为空才补，已有内容不覆盖。
            backfill_entry_content(conn, aid, e);
        }
        None => {
            // 本地没有 → 若其远端 feed 已绑定，则 upsert 补齐
            let feed_id = item_feed_id(e);
            let local_feed = feed_id.and_then(|fid| maps.feed_mf_to_id.get(&fid).copied());
            if let Some(local_feed) = local_feed {
                upsert_remote_entry(conn, local_feed, e, maps, report);
            }
        }
    }
}

/// 协议分派：Google Reader 用 `ot` 时间游标，Fever 用 `since_id` 条目游标。
async fn pull_entries(
    db: &Arc<Mutex<Connection>>,
    client: &Backend,
    report: &mut SyncReport,
    full: bool,
) {
    match client {
        Backend::GReader(g) => pull_entries_greader(db, g, report, full).await,
        Backend::Fever(f) => pull_entries_fever(db, f, report, full).await,
    }
}

/// 收集一批 Fever 条目：记录已见 id（供 with_ids 补齐去重）+ 追加到总列表。
fn collect_fever_items(
    all_items: &mut Vec<ItemContent>,
    seen: &mut std::collections::HashSet<i64>,
    incoming: Vec<ItemContent>,
) {
    for it in incoming {
        if let Some(eid) = item_numeric_id(&it) {
            seen.insert(eid);
        }
        all_items.push(it);
    }
}

/// Fever 拉取：Fever 无「全部条目 id」端点（items 仅给最近 50 条），拆两段：
/// ① `since_id` 分页增量拉新条目（含已读+未读 → 同源判定 + upsert）
/// ② `unread_item_ids`/`saved_item_ids` 权威集合全量对账（已读/收藏反推）。
async fn pull_entries_fever(
    db: &Arc<Mutex<Connection>>,
    client: &fever::FeverClient,
    report: &mut SyncReport,
    full: bool,
) {
    use std::collections::HashSet;

    let since_id = {
        let conn = db.lock().await;
        if full {
            0
        } else {
            db::last_sync_entry_id(&conn).unwrap_or(0)
        }
    };

    // ① 权威状态集合（全量 id）：未读 + 收藏
    let (unread, starred) = tokio::join!(client.unread_item_ids(), client.saved_item_ids());
    let unread = unread.unwrap_or_default();
    let starred = starred.unwrap_or_default();

    // ② 拉条目正文：增量（since_id>0）或首次种子（since_id=0 → 最近 50 条）
    let mut all_items: Vec<ItemContent> = Vec::new();
    let mut seen: HashSet<i64> = HashSet::new();

    if since_id > 0 {
        // 增量：items&since_id 升序分页，单页 50，不足 50 即拿完
        let mut cursor = since_id;
        loop {
            let batch = match client.items_since(cursor).await {
                Ok(b) => b,
                Err(e) => {
                    report.errors.push(format!("拉取增量条目失败: {e}"));
                    break;
                }
            };
            let n = batch.len();
            if n == 0 {
                break;
            }
            cursor = batch
                .iter()
                .filter_map(item_numeric_id)
                .max()
                .unwrap_or(cursor);
            let got_all = n < 50;
            collect_fever_items(&mut all_items, &mut seen, batch);
            if got_all {
                break;
            }
        }
    } else {
        // 首次：Fever 无全量历史端点；最近 50 条作已读种子，未读/收藏由下方补齐
        match client.items_recent().await {
            Ok(seed) => collect_fever_items(&mut all_items, &mut seen, seed),
            Err(e) => report.errors.push(format!("拉取最近条目失败: {e}")),
        }
    }

    // ③ 权威集合中本地还没有正文的条目（未读/收藏），用 with_ids 分块补齐
    let mut need: Vec<i64> = unread
        .iter()
        .chain(starred.iter())
        .copied()
        .filter(|id| !seen.contains(id))
        .collect();
    need.sort_unstable();
    need.dedup();
    for chunk in need.chunks(50) {
        match client.items_with_ids(chunk).await {
            Ok(batch) => collect_fever_items(&mut all_items, &mut seen, batch),
            Err(e) => {
                report.errors.push(format!("拉取未读/收藏条目失败: {e}"));
                break;
            }
        }
    }

    // ④ 构建匹配映射 + 锁内合并（复用同一条目合并逻辑）
    let mut maps = {
        let conn = db.lock().await;
        db::sync_match_maps(&conn).unwrap_or_else(|e| {
            report.errors.push(format!("同步匹配映射构建失败: {e}"));
            db::SyncMatchMaps {
                url_to_id: Default::default(),
                id_to_mf_id: Default::default(),
                id_to_mf_pair: Default::default(),
                pending_ids: Default::default(),
                feed_mf_to_id: Default::default(),
                mf_id_to_article: Default::default(),
            }
        })
    };

    let mut last_id = since_id;
    for chunk in all_items.chunks(100) {
        let conn = db.lock().await;
        for e in chunk {
            if let Some(eid) = item_numeric_id(e) {
                last_id = last_id.max(eid);
            }
            merge_pulled_entry(&conn, e, &mut maps, report);
        }
        drop(conn);
    }

    // ⑤ 权威状态对账：Fever 无法直接拉已读条目，靠「未读/收藏集合」反推。
    {
        let conn = db.lock().await;
        reconcile_fever_state(&conn, &unread, &starred, &maps, report);
    }

    // ⑥ 更新游标（Fever 用条目 id；时间戳游标也记录，供切换回 greader 后的首拉）
    let conn = db.lock().await;
    let _ = db::set_last_sync_entry_id(&conn, last_id);
    let _ = db::set_last_sync_ts(&conn, Utc::now().timestamp());
    drop(conn);
}

/// Fever 全量状态对账。Miniflux 按 URL 去重 entry，故 Fever 视角无跨源副本，
/// 已绑定条目（mf_id_to_article）的远端状态可直接信任：
/// - `unread_item_ids` 含 remote_id → 远端未读 → 本地未读；不含 → 已读（read-wins）
/// - `saved_item_ids` 含 remote_id → 本地收藏；不含 → 取消收藏
///
/// pending 保护：本地有未推送变更的条目跳过，防把「刚标读/刚收藏」瞬间回滚。
fn reconcile_fever_state(
    conn: &Connection,
    unread: &[i64],
    starred: &[i64],
    maps: &db::SyncMatchMaps,
    report: &mut SyncReport,
) {
    use std::collections::HashSet;
    let unread_set: HashSet<i64> = unread.iter().copied().collect();
    let starred_set: HashSet<i64> = starred.iter().copied().collect();

    for (remote_id, aid) in &maps.mf_id_to_article {
        let aid = *aid;
        if maps.pending_ids.contains(&aid) {
            continue; // 交给 push 段队列，不被远端快照回滚
        }
        let want_read = !unread_set.contains(remote_id);
        if want_read {
            if let Ok(n) = conn.execute(
                "UPDATE articles SET is_read = 1 WHERE id = ?1 AND is_read = 0",
                rusqlite::params![aid],
            ) {
                report.merged_states += n;
            }
        } else if let Ok(n) = conn.execute(
            "UPDATE articles SET is_read = 0 WHERE id = ?1 AND is_read = 1",
            rusqlite::params![aid],
        ) {
            report.merged_states += n;
        }
        if starred_set.contains(remote_id) {
            if let Ok(n) = conn.execute(
                "UPDATE articles SET is_starred = 1 WHERE id = ?1 AND is_starred = 0",
                rusqlite::params![aid],
            ) {
                report.merged_states += n;
            }
        } else if let Ok(n) = conn.execute(
            "UPDATE articles SET is_starred = 0 WHERE id = ?1 AND is_starred = 1",
            rusqlite::params![aid],
        ) {
            report.merged_states += n;
        }
    }
}

/// 后端条目入库（source='miniflux'，不覆盖直连正文）。
/// URL 兜底合并需同源校验：跨源同 URL entry 不写状态、不抢绑定。
/// enclosure（播客音频/视频）与图片一并落库——播放器与卡片封面依赖。
fn upsert_remote_entry(
    conn: &Connection,
    feed_id: i64,
    e: &ItemContent,
    maps: &mut db::SyncMatchMaps,
    report: &mut SyncReport,
) {
    let existing = item_numeric_id(e)
        .and_then(|eid| maps.mf_id_to_article.get(&eid).copied())
        .or_else(|| {
            item_url(e)
                .as_deref()
                .and_then(|u| maps.url_to_id.get(&db::normalize_url(u)).copied())
                .filter(|aid| {
                    maps.id_to_mf_pair
                        .get(aid)
                        .map(|(entry_mf, feed_mf)| match entry_mf {
                            Some(_) => *feed_mf == item_feed_id(e),
                            None => true,
                        })
                        .unwrap_or(false)
                })
        });

    let published = item_published_at(e);
    let content_html = item_content_html(e);

    // enclosure：Google Reader 的 enclosure 无 duration，只有 url + type
    let enclosure = e.enclosure.first();
    let (enc_url, enc_mime) = match enclosure {
        Some(enc) => (Some(enc.url.clone()), enc.r#type.clone()),
        None => (None, None),
    };

    let remote_read = greader::has_tag(&e.categories, "/com.google/read");
    let remote_starred = greader::has_tag(&e.categories, "/com.google/starred");

    if let Some(aid) = existing {
        // 状态以后端为准；正文仅在本地为空时补。
        // 待推保护：本地有未推送的读/收藏变更时，跳过状态覆盖（本地优先，防乒乓）。
        if let Some(eid) = item_numeric_id(e) {
            let _ = db::set_article_remote_id(conn, aid, eid);
            maps.id_to_mf_id.insert(aid, Some(eid));
            maps.id_to_mf_pair.insert(aid, (Some(eid), maps.id_to_mf_pair.get(&aid).map(|p| p.1).unwrap_or(None)));
            maps.mf_id_to_article.insert(eid, aid);
        }
        if !maps.pending_ids.contains(&aid) {
            let _ = conn.execute(
                "UPDATE articles SET is_read = ?1, is_starred = ?2 WHERE id = ?3",
                rusqlite::params![remote_read as i64, remote_starred as i64, aid],
            );
        }
        // 封面回填：正文第一图，本地已有封面不覆盖（COALESCE）
        backfill_entry_content(conn, aid, e);
    } else {
        let a = NewArticle {
            guid: item_numeric_id(e)
                .map(|eid| format!("remote-{eid}"))
                .unwrap_or_else(|| format!("remote-{}", e.id)),
            url: item_url(e),
            title: e.title.clone(),
            author: e.author.clone(),
            summary: None,
            content_html: Some(crate::sanitize::sanitize(&content_html, item_url(e).as_deref())),
            body_text: strip_html_text(&content_html),
            image_url: crate::sanitize::first_image(&content_html),
            enclosure_url: enc_url,
            enclosure_mime: enc_mime,
            duration_sec: None,
            published_at: Some(published),
            source: "miniflux".into(),
        };
        if let Ok((aid, _)) = db::upsert_article_with_feed(conn, feed_id, &a, false) {
            if let Some(eid) = item_numeric_id(e) {
                let _ = db::set_article_remote_id(conn, aid, eid);
                maps.id_to_mf_id.insert(aid, Some(eid));
                maps.mf_id_to_article.insert(eid, aid);
            }
            let _ = conn.execute(
                "UPDATE articles SET is_read = ?1, is_starred = ?2 WHERE id = ?3",
                rusqlite::params![remote_read as i64, remote_starred as i64, aid],
            );
            report.pulled_entries += 1;
        }
    }
}

fn strip_html_text(html: &str) -> String {
    crate::sanitize::html_to_text(html)
}

/// 已有条目正文/封面/enclosure 兜底回填：本地为空才补（COALESCE），已有内容绝不覆盖。
/// （封面 / enclosure 单列 COALESCE：既不抢本地封面，也能补上 Miniflux 后来抓到的图。）
fn backfill_entry_content(conn: &Connection, aid: i64, e: &ItemContent) {
    let content_html = item_content_html(e);
    let enclosure = e.enclosure.first();
    let (enc_url, enc_mime) = match enclosure {
        Some(enc) => (Some(enc.url.clone()), enc.r#type.clone()),
        None => (None, None),
    };
    let content_image = crate::sanitize::first_image(&content_html);
    let _ = conn.execute(
        "UPDATE articles SET
            content_html = CASE WHEN COALESCE(content_html, '') = '' THEN ?1 ELSE content_html END,
            body_text = CASE WHEN body_text = '' THEN ?2 ELSE body_text END,
            image_url = COALESCE(image_url, ?3),
            enclosure_url = COALESCE(enclosure_url, ?4),
            enclosure_mime = COALESCE(enclosure_mime, ?5)
         WHERE id = ?6",
        rusqlite::params![
            content_html,
            strip_html_text(&content_html),
            content_image,
            enc_url,
            enc_mime,
            aid
        ],
    );
}

/* ============================================================
   总入口
   ============================================================ */

/// feeds 阶段（订阅层）：push_feeds + pull_feeds。秒级，首连先跑这段。
/// 锁纪律：HTTP 全在锁外；DB 读写在锁内短临界区完成。
pub async fn feeds_phase(db: &Arc<Mutex<Connection>>, http: &reqwest::Client) -> AppResult<SyncReport> {
    let Some(client) = build_client(db, http).await else {
        return Err(AppError::new("notConnected", "未配置同步后端（Google Reader / Fever 凭据）"));
    };
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
    let Some(client) = build_client(db, http).await else {
        return Err(AppError::new("notConnected", "未配置同步后端（Google Reader / Fever 凭据）"));
    };
    let mut report = SyncReport::default();
    // 推送段进 PUSH_LOCK（与 push_states_now/feeds_phase 的推送互斥，防 prune 竞态）
    {
        let _guard = PUSH_LOCK.lock().await;
        let plan = {
            let conn = db.lock().await;
            plan_push(&conn)?
        };
        let done = exec_push(&client, &plan, &mut report).await;
        if !done.is_empty() {
            let conn = db.lock().await;
            let _ = db::prune_sync(&conn, &done);
        }
    }
    pull_entries(db, &client, &mut report, full).await;
    // pull 后补推：pull 会为「本地已读但后端刚抓取成功的文章」绑定
    // remote_id（此前未绑定，push 段跳过）。绑定后再推一次，把它们的
    // pending read/star 推到后端——否则这些文章要等下一轮同步才同步状态，
    // 其他客户端会看到「本地已读、后端仍未读」。二次 push 幂等（队列已空则无操作）。
    {
        let _guard = PUSH_LOCK.lock().await;
        let plan = {
            let conn = db.lock().await;
            plan_push(&conn)?
        };
        if !plan.status.is_empty() || !plan.stars.is_empty() {
            let done = exec_push(&client, &plan, &mut report).await;
            if !done.is_empty() {
                let conn = db.lock().await;
                let _ = db::prune_sync(&conn, &done);
            }
        }
    }
    Ok(report)
}

/// 完整同步（全量路径）：feeds 阶段 + states 阶段（full 对账）串联。
pub async fn sync_now(db: &Arc<Mutex<Connection>>, http: &reqwest::Client) -> AppResult<SyncReport> {
    let mut report = feeds_phase(db, http).await?;
    let states = states_phase(db, http, true).await?;
    report.pushed_states = states.pushed_states;
    report.pulled_entries = states.pulled_entries;
    report.fallback_entries = states.fallback_entries;
    report.errors.extend(states.errors);
    Ok(report)
}

/// 轻量同步（后台自动调度）：push 队列 + 增量 pull。
pub async fn sync_light(db: &Arc<Mutex<Connection>>, http: &reqwest::Client) -> AppResult<SyncReport> {
    states_phase(db, http, false).await
}

/// 测试连接（设置页「测试连接」按钮）。
/// 按协议分派：Google Reader 走 ClientLogin，Fever 走 `api_key` 认证。
/// 返回 (展示消息, 用户名)——用户名供 sync_save 落库做账号显示。
pub async fn test_connection(
    protocol: &str,
    endpoint: &str,
    username: &str,
    password: &str,
    http: &reqwest::Client,
) -> AppResult<(String, String)> {
    if endpoint.trim().is_empty() || username.trim().is_empty() || password.trim().is_empty() {
        return Err(AppError::new("notConnected", "请先填写 Endpoint、用户名和密码"));
    }
    let subs = match protocol {
        "fever" => {
            let client = fever::FeverClient::new(endpoint, username, password, http.clone());
            client.subscriptions().await?.len()
        }
        _ => {
            let client = GReaderClient::login(endpoint, username, password, http.clone()).await?;
            client.subscriptions().await?.len()
        }
    };
    Ok((
        format!("已连接：{username}（{subs} 个订阅）"),
        username.to_string(),
    ))
}
