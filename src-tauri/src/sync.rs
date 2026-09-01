//! Miniflux 同步引擎（实施方案 §4.3-4.5）：
//!
//! ① Push：sync_queue 里的本地变更推到 Miniflux
//! ② Pull：拉远端 feeds/categories/条目状态变化，URL 碰撞合并
//! ③ 兜底：直连失败的源从 Miniflux 拉条目（source='miniflux'）
//! 本地未连接期间添加的源，首次 Pull 时按 URL 碰撞检测：
//!   远端无 → 推送创建；远端有 → 合并（Miniflux id 绑定本地 feed）
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
use crate::miniflux::{Entry, MinifluxClient};
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

pub fn read_credentials(conn: &Connection) -> Option<(String, String)> {
    let endpoint = db::get_setting(conn, "miniflux_endpoint").ok().flatten()?;
    let token = db::get_setting(conn, "miniflux_token").ok().flatten()?;
    if endpoint.trim().is_empty() || token.trim().is_empty() {
        return None;
    }
    Some((endpoint, token))
}

fn client_from_creds(endpoint: &str, token: &str, http: &reqwest::Client) -> MinifluxClient {
    MinifluxClient::new(endpoint, token, http.clone())
}

/// 锁内读凭据 → 构建 client（锁外使用）
async fn build_client(db: &Arc<Mutex<Connection>>, http: &reqwest::Client) -> Option<MinifluxClient> {
    let (endpoint, token) = {
        let conn = db.lock().await;
        read_credentials(&conn)?
    };
    Some(client_from_creds(&endpoint, &token, http))
}

/* ============================================================
   ① Push：本地状态变更 → Miniflux（只推不拉）
   ============================================================ */

/// 全局推送互斥：同一时刻只允许一个推送在飞（防抖即时推送 vs 后台自动
/// 同步 vs 手动同步并发）。exec_push 成功后按 queue_id prune——并发时 A
/// 可能 prune 掉 B 正在推的项；更糟的是收藏 toggle 非幂等，交错执行会把
/// 星标状态翻转两次。串行化后两场景只会先后重推同一状态（幂等），无害。
static PUSH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 待推送动作的锁内快照：HTTP 执行所需的全部信息。
struct PushPlan {
    /// (队列 id, article_id, entry ids)——read 广播副本展开后
    status: Vec<PushStatus>,
    /// (队列 id, entry id)——收藏切换（Miniflux 只有 toggle 语义）
    stars: Vec<(i64, i64)>,
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
        let mf_id: Option<i64> = conn
            .query_row(
                "SELECT miniflux_id FROM articles WHERE id = ?1",
                [article_id],
                |r| r.get(0),
            )
            .ok()
            .flatten();
        let Some(mf_id) = mf_id else {
            continue;
        };
        match item.action.as_str() {
            // 已读广播：绑定的 entry + 记账的全部同文副本 entry 一并标读
            // （双端场景：Read You 不去重，手机上另一源的副本也要已读，
            // 否则手机读完这篇、那个源里又冒出来一篇未读的"同一篇"）
            "read" => {
                let mut ids = vec![mf_id];
                for dup in db::article_dup_entries(conn, article_id).unwrap_or_default() {
                    if dup != mf_id {
                        ids.push(dup);
                    }
                }
                plan.status.push(PushStatus { queue_id: item.id, action: "read".into(), entry_ids: ids });
            }
            "unread" => plan.status.push(PushStatus { queue_id: item.id, action: "unread".into(), entry_ids: vec![mf_id] }),
            "star" | "unstar" => plan.stars.push((item.id, mf_id)),
            _ => {}
        }
    }
    Ok(plan)
}

/// 锁外：执行推送计划。返回成功清除的队列 id（失败项保留 → 天然重试）。
async fn exec_push(client: &MinifluxClient, plan: &PushPlan, report: &mut SyncReport) -> Vec<i64> {
    let mut done: Vec<i64> = Vec::new();
    // read/unread 聚合批量 PUT（Miniflux 单请求可携带全部 id）
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
        match client.update_entries_status(&ids, action).await {
            Ok(()) => {
                report.pushed_states += ids.len();
                done.extend(plan.status.iter().filter(|s| s.action == action).map(|s| s.queue_id));
            }
            Err(e) => report.errors.push(format!("状态推送失败: {e}")),
        }
    }
    // 收藏逐条 toggle
    for (qid, mf_id) in &plan.stars {
        match client.toggle_bookmark(*mf_id).await {
            Ok(()) => {
                report.pushed_states += 1;
                done.push(*qid);
            }
            Err(e) => report.errors.push(format!("收藏同步失败: entry {mf_id}: {e}")),
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

/// 未连接期间本地新增的订阅推到远端
/// 未连接期间本地新增的订阅推到远端（三段式：锁内读队列 → 锁外 HTTP → 锁内落库）
async fn push_feeds(db: &Arc<Mutex<Connection>>, client: &MinifluxClient, report: &mut SyncReport) {
    // add_feed 队列动作：锁内读出全部待处理项（feed_url + 目标分类）
    struct PendingFeed { queue_id: i64, url: String, folder_mf: Option<i64> }
    let items: Vec<PendingFeed> = {
        let conn = db.lock().await;
        let mut out = Vec::new();
        for item in db::take_sync_queue(&conn).unwrap_or_default() {
            if item.action != "add_feed" {
                continue;
            }
            let Some(url) = item.feed_url else { continue };
            let folder_mf = item
                .payload
                .as_deref()
                .and_then(|p| serde_json::from_str::<serde_json::Value>(p).ok())
                .and_then(|v| v.get("folder_id").and_then(|f| f.as_i64()))
                .and_then(|fid| {
                    conn.query_row(
                        "SELECT miniflux_id FROM folders WHERE id = ?1",
                        [fid],
                        |r| r.get(0),
                    )
                    .ok()
                });
            out.push(PendingFeed { queue_id: item.id, url, folder_mf });
        }
        out
    };
    if items.is_empty() {
        return;
    }
    // 锁外：逐个创建远端 feed（分类缺失时补拉一次 categories）
    let mut done: Vec<i64> = Vec::new();
    let mut default_cat: Option<i64> = None;
    for it in &items {
        let cat_id = match it.folder_mf {
            Some(c) => c,
            None => {
                if default_cat.is_none() {
                    default_cat = client.categories().await.ok().and_then(|c| c.first().map(|c| c.id));
                }
                default_cat.unwrap_or(1)
            }
        };
        match client.create_feed(&it.url, cat_id).await {
            Ok(mf_feed_id) => {
                report.pushed_feeds += 1;
                done.push(it.queue_id);
                // 锁内：绑定本地 feed（URL 匹配）
                let conn = db.lock().await;
                if let Some(local_id) = db::feed_id_by_url(&conn, &it.url).ok().flatten() {
                    let _ = db::set_feed_miniflux_id(&conn, local_id, mf_feed_id);
                }
                drop(conn);
            }
            Err(e) => report.errors.push(format!("推送订阅 {} 失败: {e}", it.url)),
        }
    }
    let conn = db.lock().await;
    let _ = db::prune_sync(&conn, &done);
    drop(conn);

    // remove_feed 队列动作：远端也有此源才删（本地删除 ≠ 强删远端，避免误删服务端数据；
    // 设计约定「同步服务端不受影响」——remove_feed 仅在用户显式操作时入队，暂不自动推删）
}

/// 拉远端分类+订阅，URL 碰撞合并（三段式：锁外拉取 → 锁内合并）
async fn pull_feeds(db: &Arc<Mutex<Connection>>, client: &MinifluxClient, report: &mut SyncReport) {
    let (remote_cats, remote_feeds) = match tokio::join!(client.categories(), client.feeds()) {
        (Ok(c), Ok(f)) => (c, f),
        (Err(e), _) | (_, Err(e)) => {
            report.errors.push(format!("拉取订阅失败: {e}"));
            return;
        }
    };

    // 锁内：分类按标题/miniflux_id 匹配本地 folder，不存在则创建并绑定
    {
        let conn = db.lock().await;
        for rc in &remote_cats {
            let existing: Option<i64> = conn
                .query_row(
                    "SELECT id FROM folders WHERE name = ?1 OR miniflux_id = ?2",
                    rusqlite::params![rc.title, rc.id],
                    |r| r.get(0),
                )
                .ok()
                .flatten();
            match existing {
                Some(fid) => {
                    let _ = db::set_folder_miniflux_id(&conn, fid, rc.id);
                }
                None => {
                    let fid = db::create_folder(&conn, &rc.title, "article").unwrap_or(-1);
                    if fid > 0 {
                        let _ = db::set_folder_miniflux_id(&conn, fid, rc.id);
                    }
                }
            }
        }
    }

    // 锁内：订阅按 URL 碰撞合并
    {
        let conn = db.lock().await;
        for rf in &remote_feeds {
            let local_feed = db::feed_id_by_url(&conn, &rf.feed_url).ok().flatten();
            match local_feed {
                Some(lid) => {
                    // 已存在（本地直连添加过）→ 绑定 miniflux_id，本地分类/布局保留
                    let _ = db::set_feed_miniflux_id(&conn, lid, rf.id);
                    // 远端标题仅在本地标题等于 URL（从未抓取成功过）时回填
                    let _ = conn.execute(
                        "UPDATE feeds SET
                            title = CASE WHEN title = feed_url THEN ?1 ELSE title END,
                            favicon_url = COALESCE(favicon_url, ?2),
                            site_url = COALESCE(site_url, ?3)
                         WHERE id = ?4",
                        rusqlite::params![rf.title, rf.icon_url, rf.site_url, lid],
                    );
                    report.merged_states += 1;
                }
                None => {
                    // 本地没有 → 建本地 feed（挂到远端分类对应的本地 folder）
                    let folder_id: i64 = rf
                        .category
                        .as_ref()
                        .and_then(|c| {
                            conn.query_row(
                                "SELECT id FROM folders WHERE miniflux_id = ?1",
                                [c.id],
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
                        &rf.feed_url,
                        rf.site_url.as_deref(),
                        &rf.title,
                        rf.icon_url.as_deref(),
                        folder_id,
                        "inherit",
                        true,
                        false,
                        "miniflux",
                    );
                    if let Ok(fid) = inserted {
                        let _ = db::set_feed_miniflux_id(&conn, fid, rf.id);
                        report.pulled_feeds += 1;
                    }
                }
            }
        }
    }
}

/// 状态合并的守卫语义（pull_entries 与对账共用）：
/// - 待推保护：本地有未推送变更 → 跳过（本地优先，防乒乓）
/// - read-anywhere-wins：「读」是强意图，任何副本的已读都接受
/// - unread 只认绑定同源 entry：跨源副本的未读不能复活桌面已读
fn merge_remote_status(conn: &Connection, aid: i64, e: &Entry, report: &mut SyncReport) {
    let _ = db::set_article_miniflux_id(conn, aid, e.id);
    if db::article_has_pending_sync(conn, aid).unwrap_or(false) {
        return;
    }
    let remote_read = e.status == "read";
    let local_bound = db::article_by_miniflux_id(conn, e.id).ok().flatten().is_some();
    let same_feed_trusted = db::article_matches_remote_feed(conn, aid, e.feed_id).unwrap_or(false);
    let accept_unread = local_bound && same_feed_trusted;
    if remote_read || accept_unread {
        let _ = conn.execute(
            "UPDATE articles SET is_read = ?1, is_starred = ?2 WHERE id = ?3",
            rusqlite::params![remote_read as i64, e.starred as i64, aid],
        );
        report.pulled_entries += 1;
    }
}

/// 拉远端条目（新条目 + 状态变化），按 miniflux_id/URL 匹配合并。
/// `full=true`（手动同步/首连）：先做绑定回填 + 全量状态对账——
/// 全量条目已经在手上（绑定回填本来就要拉），对已匹配条目直接应用远端
/// 状态，changed_at 早于游标的旧变更从此收敛（未读数漂移根因）。
/// `full=false`（后台自动同步）：只拉 changed_after 增量，便宜。
/// 拉远端条目（新条目 + 状态变化），按 miniflux_id/URL 匹配合并。
/// 分页三段式：每页「锁外拉取 → 锁内合并」，锁从不跨分页 HTTP await。
/// `full=true`（手动同步/首连）：先做绑定回填 + 全量状态对账——
/// 全量条目已经在手上（绑定回填本来就要拉），对已匹配条目直接应用远端
/// 状态，changed_at 早于游标的旧变更从此收敛（未读数漂移根因）。
/// `full=false`（后台自动同步）：只拉 changed_after 增量，便宜。
async fn pull_entries(db: &Arc<Mutex<Connection>>, client: &MinifluxClient, report: &mut SyncReport, full: bool) {
    // ②' 绑定回填 + 状态对账（full 路径）：锁外拉全量，锁内逐条合并
    if full {
        let all_entries = match client.entries(None, 0, false).await {
            Ok(v) => v,
            Err(e) => {
                report.errors.push(format!("绑定回填拉取失败: {e}"));
                Vec::new()
            }
        };
        {
            let conn = db.lock().await;
            for e in &all_entries {
                let Some(u) = e.url.as_deref() else { continue };
                let Some(aid) = db::article_id_by_url(&conn, u).ok().flatten() else {
                    continue;
                };
                // 已绑定的 entry id 直配 = 自己的条目（feed 可能尚未绑定——
                // states 阶段先于 feeds 阶段的窗口），无需再查 feed 归属
                let bound_entry: Option<i64> = conn
                    .query_row("SELECT miniflux_id FROM articles WHERE id = ?1", [aid], |r| r.get(0))
                    .ok()
                    .flatten();
                let is_own = bound_entry == Some(e.id)
                    || (bound_entry.is_none() && db::article_matches_remote_feed(&conn, aid, e.feed_id).unwrap_or(false));
                if !is_own {
                    // 跨源副本：记账（已读广播对象）。read-anywhere-wins：远端副本的
                    // 已读也是真读意图 → 接受已读，但不抢绑定、不接受未读
                    let _ = db::add_article_dup_entry(&conn, aid, e.id);
                    if e.status == "read" && !db::article_has_pending_sync(&conn, aid).unwrap_or(false) {
                        let _ = conn.execute(
                            "UPDATE articles SET is_read = 1 WHERE id = ?1",
                            rusqlite::params![aid],
                        );
                    }
                    continue;
                }
                merge_remote_status(&conn, aid, e, report);
            }
        }
    }

    // ① 新条目（只补直连失败的源 + 远端新订阅的源）
    let (since_s, failed_feeds) = {
        let conn = db.lock().await;
        (
            db::last_sync_ts(&conn).unwrap_or(0),
            db::feeds_fetch_failed(&conn).unwrap_or_default(),
        )
    };
    for feed in &failed_feeds {
        let mf_id: Option<i64> = {
            let conn = db.lock().await;
            feed_miniflux_id(&conn, feed.id)
        };
        let Some(mf_id) = mf_id else { continue };
        match client.entries(Some(mf_id), since_s, false).await {
            Ok(entries) => {
                let before = report.pulled_entries;
                let conn = db.lock().await;
                for e in &entries {
                    upsert_miniflux_entry(&conn, feed.id, e, report);
                }
                // 只统计真实入库/合并的条目（upsert_miniflux_entry 内累计 pulled_entries）
                report.fallback_entries = report.pulled_entries - before;
            }
            Err(err) => report.errors.push(format!("兜底拉取 {} 失败: {}", feed.title, err)),
        }
    }

    // ② 状态变化（全部已绑定的源，changed_after 增量，unix 秒）
    let entries = match client.entries(None, since_s, true).await {
        Ok(v) => v,
        Err(e) => {
            report.errors.push(format!("拉取状态变化失败: {e}"));
            return;
        }
    };
    {
        let conn = db.lock().await;
        for e in &entries {
            // 匹配：miniflux_id 直配 → URL 兜底（同源校验）
            let local = db::article_by_miniflux_id(&conn, e.id)
                .ok()
                .flatten()
                .or_else(|| {
                    e.url.as_deref().and_then(|u| db::article_id_by_url(&conn, u).ok().flatten())
                        .filter(|aid| db::article_matches_remote_feed(&conn, *aid, e.feed_id).unwrap_or(false))
                });
            let Some(aid) = local else {
                // 跨源同 URL entry（手机端另一源的副本）：不写状态，但记账
                // 副本 entry——桌面端的已读变更要广播到它
                if let Some(u) = e.url.as_deref() {
                    if let Ok(Some(aid)) = db::article_id_by_url(&conn, u) {
                        let _ = db::add_article_dup_entry(&conn, aid, e.id);
                    }
                }
                continue;
            };
            merge_remote_status(&conn, aid, e, report);
        }
        let now = Utc::now().timestamp();
        let _ = db::set_last_sync_ts(&conn, now);
    }
}

fn feed_miniflux_id(conn: &Connection, feed_id: i64) -> Option<i64> {
    conn.query_row(
        "SELECT miniflux_id FROM feeds WHERE id = ?1",
        [feed_id],
        |r| r.get(0),
    )
    .ok()
    .flatten()
}

/// Miniflux 兜底条目入库（source='miniflux'，不覆盖直连正文）。
/// URL 兜底合并需同源校验：跨源同 URL entry 不写状态、不抢绑定。
/// enclosure（播客音频/视频）与图片一并落库——播放器与卡片封面依赖。
fn upsert_miniflux_entry(conn: &Connection, feed_id: i64, e: &Entry, report: &mut SyncReport) {
    let existing = db::article_by_miniflux_id(conn, e.id)
        .ok()
        .flatten()
        .or_else(|| {
            e.url.as_deref().and_then(|u| db::article_id_by_url(conn, u).ok().flatten())
                .filter(|aid| db::article_matches_remote_feed(conn, *aid, e.feed_id).unwrap_or(false))
        });

    let published = DateTime::parse_from_rfc3339(&e.published_at)
        .map(|d| d.with_timezone(&Utc).to_rfc3339())
        .unwrap_or_else(|_| Utc::now().to_rfc3339());

    // enclosure：Miniflux entry 的 enclosures 数组，取第一个音/视频
    let enclosure = e.enclosures.first();
    let (enc_url, enc_mime, duration) = match enclosure {
        Some(enc) => (Some(enc.url.clone()), Some(enc.mime_type.clone()), enc.duration),
        None => (None, None, None),
    };

    if let Some(aid) = existing {
        // 状态以 Miniflux 为准；正文仅在本地为空时补
        let _ = db::set_article_miniflux_id(conn, aid, e.id);
        let _ = conn.execute(
            "UPDATE articles SET
                is_read = ?1, is_starred = ?2,
                content_html = CASE WHEN COALESCE(content_html, '') = '' THEN ?3 ELSE content_html END,
                body_text = CASE WHEN body_text = '' THEN ?4 ELSE body_text END,
                enclosure_url = COALESCE(enclosure_url, ?5),
                enclosure_mime = COALESCE(enclosure_mime, ?6),
                duration_sec = COALESCE(duration_sec, ?7)
             WHERE id = ?8",
            rusqlite::params![
                (e.status == "read") as i64,
                e.starred as i64,
                e.content,
                strip_html_text(&e.content),
                enc_url,
                enc_mime,
                duration,
                aid
            ],
        );
    } else {
        let a = NewArticle {
            guid: format!("miniflux-{}", e.id),
            url: e.url.clone(),
            title: e.title.clone(),
            author: e.author.clone(),
            summary: None,
            content_html: Some(crate::sanitize::sanitize(&e.content, e.url.as_deref())),
            body_text: strip_html_text(&e.content),
            image_url: enclosure
                .map(|enc| enc.url.clone())
                .filter(|u| u.starts_with("http"))
                .or_else(|| crate::sanitize::first_image(&e.content)),
            enclosure_url: enc_url,
            enclosure_mime: enc_mime,
            duration_sec: duration,
            published_at: Some(published),
            source: "miniflux".into(),
        };
        if let Ok((aid, _)) = db::upsert_article_with_feed(conn, feed_id, &a, false) {
            let _ = db::set_article_miniflux_id(conn, aid, e.id);
            let _ = conn.execute(
                "UPDATE articles SET is_read = ?1, is_starred = ?2 WHERE id = ?3",
                rusqlite::params![(e.status == "read") as i64, e.starred as i64, aid],
            );
            report.pulled_entries += 1;
        }
    }
}

fn strip_html_text(html: &str) -> String {
    crate::sanitize::html_to_text(html)
}

/* ============================================================
   总入口
   ============================================================ */

/// feeds 阶段（订阅层）：push_feeds + pull_feeds。秒级，首连先跑这段。
/// 锁纪律：HTTP 全在锁外；DB 读写在锁内短临界区完成。
pub async fn feeds_phase(db: &Arc<Mutex<Connection>>, http: &reqwest::Client) -> AppResult<SyncReport> {
    let Some(client) = build_client(db, http).await else {
        return Err(AppError::new("notConnected", "未配置 Miniflux Endpoint/Token"));
    };
    client.me().await?;
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
        return Err(AppError::new("notConnected", "未配置 Miniflux Endpoint/Token"));
    };
    client.me().await?;
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
    Ok(report)
}

/// 完整同步（全量路径）：feeds 阶段 + states 阶段（full 对账）串联。
/// 测试与既有调用方使用；生产前端走 sync_phase 分步 API。
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
/// 直接收凭据：rusqlite Connection 非 Sync，不能把 &Connection 跨 await 传进来。
/// 返回 (展示消息, 用户名)——用户名供 sync_save 落库做账号显示。
pub async fn test_connection(
    endpoint: &str,
    token: &str,
    http: &reqwest::Client,
) -> AppResult<(String, String)> {
    if endpoint.trim().is_empty() || token.trim().is_empty() {
        return Err(AppError::new("notConnected", "请先填写 Endpoint 和 Token"));
    }
    let client = client_from_creds(endpoint, token, http);
    let me = client.me().await?;
    Ok((format!("已连接：{} (id {})", me.username, me.id), me.username))
}
