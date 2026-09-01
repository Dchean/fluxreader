//! Miniflux 同步引擎（实施方案 §4.3-4.5）：
//!
//! ① Push：sync_queue 里的本地变更推到 Miniflux
//! ② Pull：拉远端 feeds/categories/条目状态变化，URL 碰撞合并
//! ③ 兜底：直连失败的源从 Miniflux 拉条目（source='miniflux'）
//! 本地未连接期间添加的源，首次 Pull 时按 URL 碰撞检测：
//!   远端无 → 推送创建；远端有 → 合并（Miniflux id 绑定本地 feed）

use crate::db::{self, NewArticle};
use crate::error::{AppError, AppResult};
use crate::miniflux::{Entry, MinifluxClient};
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde::Serialize;

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

fn build_client(conn: &Connection, http: &reqwest::Client) -> Option<MinifluxClient> {
    let (endpoint, token) = read_credentials(conn)?;
    Some(MinifluxClient::new(&endpoint, &token, http.clone()))
}

/* ============================================================
   ① Push：本地变更 → Miniflux
   ============================================================ */

async fn push_queue(conn: &mut Connection, client: &MinifluxClient, report: &mut SyncReport) {
    let items = match db::take_sync_queue(conn) {
        Ok(v) => v,
        Err(e) => {
            report.errors.push(format!("读队列失败: {e}"));
            return;
        }
    };
    if items.is_empty() {
        return;
    }

    let mut done_ids: Vec<i64> = Vec::new();
    // 状态变更按 action 分组批量推（Miniflux PUT /v1/entries 支持批量）
    let mut read_ids: Vec<i64> = Vec::new();
    let mut unread_ids: Vec<i64> = Vec::new();

    for item in &items {
        let Some(article_id) = item.article_id else {
            // feed 级动作（add_feed/remove_feed）在 push_feeds 阶段处理
            continue;
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
            // 本地条目还没绑定 Miniflux id：保留在队列（Pull 的绑定回填会补上，
            // 下一轮同步重推），直接丢弃会让"已读"在服务端永久丢失
            continue;
        };
        match item.action.as_str() {
            "read" => read_ids.push(mf_id),
            "unread" => unread_ids.push(mf_id),
            "star" | "unstar" => {
                if client.toggle_bookmark(mf_id).await.is_ok() {
                    done_ids.push(item.id);
                    report.pushed_states += 1;
                } else {
                    report.errors.push(format!("收藏同步失败: entry {mf_id}"));
                }
            }
            _ => done_ids.push(item.id),
        }
    }

    for (ids, status) in [(read_ids, "read"), (unread_ids, "unread")] {
        if ids.is_empty() {
            continue;
        }
        match client.update_entries_status(&ids, status).await {
            Ok(()) => {
                report.pushed_states += ids.len();
                // 批量成功：把对应的队列条目标记完成
                for item in &items {
                    if let Some(aid) = item.article_id {
                        let mf: Option<i64> = conn
                            .query_row(
                                "SELECT miniflux_id FROM articles WHERE id = ?1",
                                [aid],
                                |r| r.get(0),
                            )
                            .ok()
                            .flatten();
                        if let Some(m) = mf {
                            if ids.contains(&m) {
                                done_ids.push(item.id);
                            }
                        }
                    }
                }
            }
            Err(e) => report.errors.push(format!("状态推送失败: {e}")),
        }
    }

    let _ = db::prune_sync(conn, &done_ids);
}

/// 未连接期间本地新增/删除的订阅推到远端
async fn push_feeds(conn: &mut Connection, client: &MinifluxClient, report: &mut SyncReport) {
    // add_feed 队列动作
    let items = db::take_sync_queue(conn).unwrap_or_default();
    let mut done: Vec<i64> = Vec::new();
    for item in &items {
        if item.action != "add_feed" {
            continue;
        }
        let Some(url) = item.feed_url.as_deref() else {
            done.push(item.id);
            continue;
        };
        // 分类：payload 里带本地 folder_id → 查 miniflux_id；没有就推默认分类
        let folder_mf_id: Option<i64> = item
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
        let cat_id = match folder_mf_id {
            Some(c) => c,
            None => match client.categories().await {
                Ok(cats) => cats.first().map(|c| c.id).unwrap_or(1),
                Err(_) => 1,
            },
        };
        match client.create_feed(url, cat_id).await {
            Ok(mf_feed_id) => {
                if let Some(local_id) = db::feed_id_by_url(conn, url).ok().flatten() {
                    let _ = db::set_feed_miniflux_id(conn, local_id, mf_feed_id);
                }
                report.pushed_feeds += 1;
                done.push(item.id);
            }
            Err(e) => report.errors.push(format!("推送订阅 {url} 失败: {e}")),
        }
    }
    let _ = db::prune_sync(conn, &done);

    // remove_feed 队列动作：远端也有此源才删（本地删除 ≠ 强删远端，避免误删服务端数据；
    // 设计约定「同步服务端不受影响」——remove_feed 仅在用户显式操作时入队，暂不自动推删）
}

/* ============================================================
   ② Pull：远端 → 本地（订阅关系 + 状态 + 条目）
   ============================================================ */

async fn pull_feeds(conn: &mut Connection, client: &MinifluxClient, report: &mut SyncReport) {
    let (remote_cats, remote_feeds) = match tokio::join!(client.categories(), client.feeds()) {
        (Ok(c), Ok(f)) => (c, f),
        (Err(e), _) | (_, Err(e)) => {
            report.errors.push(format!("拉取订阅失败: {e}"));
            return;
        }
    };

    // 分类：按标题匹配本地 folder；不存在则创建本地 folder 并绑定
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
                let _ = db::set_folder_miniflux_id(conn, fid, rc.id);
            }
            None => {
                let fid = db::create_folder(conn, &rc.title, "article").unwrap_or(-1);
                if fid > 0 {
                    let _ = db::set_folder_miniflux_id(conn, fid, rc.id);
                }
            }
        }
    }

    // 订阅：URL 碰撞合并（§4.4）
    for rf in &remote_feeds {
        let local_feed = db::feed_id_by_url(conn, &rf.feed_url).ok().flatten();
        match local_feed {
            Some(lid) => {
                // 已存在（本地直连添加过）→ 绑定 miniflux_id，本地分类/布局保留
                let _ = db::set_feed_miniflux_id(conn, lid, rf.id);
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
                let inserted = db::insert_feed(
                    conn,
                    &rf.feed_url,
                    rf.site_url.as_deref(),
                    &rf.title,
                    rf.icon_url.as_deref(),
                    folder_id,
                    "inherit",
                    true,
                    false,
                );
                if let Ok(fid) = inserted {
                    let _ = db::set_feed_miniflux_id(conn, fid, rf.id);
                    report.pulled_feeds += 1;
                }
            }
        }
    }
}

/// 拉远端条目（新条目 + 状态变化），按 miniflux_id/URL 匹配合并
async fn pull_entries(conn: &mut Connection, client: &MinifluxClient, report: &mut SyncReport) {
    // ②' 绑定回填：直连抓取的文章（首同步后入库）尚未绑定 miniflux_id，
    // 推送会被跳过。按 URL 全量对齐远程条目 id（增量窗口会漏掉
    // 早于上次同步入库的文章；个人订阅几百条，全量成本可接受）。
    let all_entries = match client.entries(None, 0, false).await {
        Ok(v) => v,
        Err(e) => {
            report.errors.push(format!("绑定回填拉取失败: {e}"));
            Vec::new()
        }
    };
    for e in &all_entries {
        if let Some(u) = e.url.as_deref() {
            if let Some(aid) = db::article_id_by_url(conn, u).ok().flatten() {
                // 同源校验：跨源同 URL entry 不抢绑定（绑错会让已读/收藏
                // 推到服务端另一条的 entry 上，状态从此两边发散）
                if db::article_matches_remote_feed(conn, aid, e.feed_id).unwrap_or(false) {
                    let _ = db::set_article_miniflux_id(conn, aid, e.id);
                }
            }
        }
    }

    // ① 新条目（只补直连失败的源 + 远端新订阅的源）
    let since_ms = db::last_sync_ts(conn).unwrap_or(0) * 1000;
    let failed_feeds = db::feeds_fetch_failed(conn).unwrap_or_default();
    for feed in &failed_feeds {
        let Some(mf_id) = feed_miniflux_id(conn, feed.id) else {
            continue;
        };
        match client.entries(Some(mf_id), since_ms, false).await {
            Ok(entries) => {
                let before = report.pulled_entries;
                for e in &entries {
                    upsert_miniflux_entry(conn, feed.id, e, report);
                }
                // 只统计真实入库/合并的条目（upsert_miniflux_entry 内累计 pulled_entries）
                report.fallback_entries = report.pulled_entries - before;
            }
            Err(err) => report.errors.push(format!("兜底拉取 {} 失败: {}", feed.title, err)),
        }
    }

    // ② 状态变化（全部已绑定的源，changed_after 增量）
    let entries = match client.entries(None, since_ms, true).await {
        Ok(v) => v,
        Err(e) => {
            report.errors.push(format!("拉取状态变化失败: {e}"));
            return;
        }
    };
    for e in &entries {
        // 匹配：miniflux_id 直配 → URL 兜底
        // URL 兜底需同源校验：跨源的同 URL entry 无权写本地状态
        // （防止已读文章被服务端另一条同 URL entry 的未读状态复活）
        let local = db::article_by_miniflux_id(conn, e.id)
            .ok()
            .flatten()
            .or_else(|| {
                e.url.as_deref().and_then(|u| db::article_id_by_url(conn, u).ok().flatten())
                    .filter(|aid| db::article_matches_remote_feed(conn, *aid, e.feed_id).unwrap_or(false))
            });
        let Some(aid) = local else {
            continue;
        };
        let _ = db::set_article_miniflux_id(conn, aid, e.id);
        // Miniflux 是状态权威（starred 需读列表接口的 starred 标志位）
        let _ = conn.execute(
            "UPDATE articles SET is_read = ?1, is_starred = ?2 WHERE id = ?3",
            rusqlite::params![(e.status == "read") as i64, e.starred as i64, aid],
        );
        report.pulled_entries += 1;
    }

    let now = Utc::now().timestamp();
    let _ = db::set_last_sync_ts(conn, now);
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
fn upsert_miniflux_entry(conn: &mut Connection, feed_id: i64, e: &Entry, report: &mut SyncReport) {
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

    if let Some(aid) = existing {
        // 状态以 Miniflux 为准；正文仅在本地为空时补
        let _ = db::set_article_miniflux_id(conn, aid, e.id);
        let _ = conn.execute(
            "UPDATE articles SET
                is_read = ?1, is_starred = ?2,
                content_html = CASE WHEN COALESCE(content_html, '') = '' THEN ?3 ELSE content_html END,
                body_text = CASE WHEN body_text = '' THEN ?4 ELSE body_text END
             WHERE id = ?5",
            rusqlite::params![(e.status == "read") as i64, e.starred as i64, e.content, strip_html_text(&e.content), aid],
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
            image_url: None,
            enclosure_url: None,
            enclosure_mime: None,
            duration_sec: None,
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

/// 完整同步：push → pull feeds → pull entries。返回报告（前端 Toast/同步中心展示）。
pub async fn sync_now(conn: &mut Connection, http: &reqwest::Client) -> AppResult<SyncReport> {
    let Some(client) = build_client(conn, http) else {
        return Err(AppError::new("notConnected", "未配置 Miniflux Endpoint/Token"));
    };

    // 连接验证（凭据错误早失败）
    client.me().await?;

    let mut report = SyncReport::default();
    push_feeds(conn, &client, &mut report).await;
    push_queue(conn, &client, &mut report).await;
    pull_feeds(conn, &client, &mut report).await;
    pull_entries(conn, &client, &mut report).await;
    Ok(report)
}

/// 测试连接（设置页「测试连接」按钮）。
/// 直接收凭据：rusqlite Connection 非 Sync，不能把 &Connection 跨 await 传进来。
pub async fn test_connection(endpoint: &str, token: &str, http: &reqwest::Client) -> AppResult<String> {
    if endpoint.trim().is_empty() || token.trim().is_empty() {
        return Err(AppError::new("notConnected", "请先填写 Endpoint 和 Token"));
    }
    let client = MinifluxClient::new(endpoint, token, http.clone());
    let me = client.me().await?;
    Ok(format!("已连接：{} (id {})", me.username, me.id))
}
