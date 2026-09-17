//! sync 的 subscriptions 子模块（TASK-045 从 sync.rs 按既有章节拆分）。

use super::*;
use crate::db;
use crate::greader::{self};
use rusqlite::Connection;
use std::sync::Arc;
use tokio::sync::Mutex;

/// 推送订阅编辑（改名 / 移动目录）到远端（best-effort，A-2）。
/// 失败仅记日志：本地已生效，靠下次 pull 对账或用户重试收敛，不阻塞 UI。
pub async fn edit_remote_subscription(
    db: &Arc<Mutex<Connection>>,
    http: &reqwest::Client,
    remote_id: i64,
    title: Option<&str>,
    dest_label: Option<&str>,
) -> bool {
    let Some(client) = build_client(db, http).await else {
        return false;
    };
    match client.edit_subscription(remote_id, title, dest_label).await {
        Ok(()) => true,
        Err(e) => {
            log::warn!("sync: 订阅编辑推送失败（本地已生效，待下轮收敛）: {e}");
            false
        }
    }
}

/// 退订远端订阅（best-effort；仅 GReader 协议有端点——Fever 无退订端点）。
/// 成功后退订墓碑解除：远端已不再列出该订阅，pull 不会复活。返回远端是否确认。
pub async fn unsubscribe_remote(
    db: &Arc<Mutex<Connection>>,
    http: &reqwest::Client,
    remote_id: i64,
    feed_url: &str,
) -> bool {
    let Some(client) = build_client(db, http).await else {
        return false;
    };
    let ok = match client {
        Backend::GReader(c) => c.unsubscribe(remote_id).await.is_ok(),
        Backend::Fever(_) => false,
    };
    if ok {
        let conn = db.lock().await;
        let _ = db::remove_feed_tombstone(&conn, feed_url);
    }
    ok
}

/// 未连接期间本地新增的订阅推到远端（三段式：锁内读队列 → 锁外 HTTP → 锁内落库）
pub(super) async fn push_feeds(
    db: &Arc<Mutex<Connection>>,
    client: &Backend,
    report: &mut SyncReport,
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
            if item.action != "add_feed" {
                continue;
            }
            let Some(url) = item.feed_url else { continue };
            let folder_label = item
                .payload
                .as_deref()
                .and_then(|p| serde_json::from_str::<serde_json::Value>(p).ok())
                .and_then(|v| v.get("folder_id").and_then(|f| f.as_i64()))
                .and_then(|fid| db::folder_name(&conn, fid).ok().flatten());
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
    // 锁外：逐个订阅（quick_add 自动发现 feed，幂等——已存在返回既有 stream_id）
    let mut done: Vec<i64> = Vec::new();
    for it in &items {
        match client.quick_add(&it.url).await {
            Ok(r) => {
                report.pushed_feeds += 1;
                done.push(it.queue_id);
                // 锁内：绑定本地 feed（URL 匹配），若 quick_add 返回了数字 id
                let bound_remote_id = {
                    let conn = db.lock().await;
                    let mut nid = None;
                    if let Some(local_id) = db::feed_id_by_url(&conn, &it.url).ok().flatten() {
                        if let Some(stream_id) = r.stream_id.as_deref() {
                            if let Some(n) = greader::parse_feed_numeric_id(stream_id) {
                                let _ = db::set_feed_remote_id(&conn, local_id, n);
                                nid = Some(n);
                            }
                        }
                    }
                    nid
                };
                // A-3：锁外补挂目标分类（best-effort，失败仅记日志——订阅已推送）
                if let (Some(nid), Some(label)) = (bound_remote_id, it.folder_label.as_deref()) {
                    if let Err(e) = client.edit_subscription(nid, None, Some(label)).await {
                        report
                            .errors
                            .push(format!("订阅 {} 挂载分类「{label}」失败: {e}", it.url));
                    }
                }
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
pub(super) async fn pull_feeds(
    db: &Arc<Mutex<Connection>>,
    client: &Backend,
    report: &mut SyncReport,
) {
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
        let remote_labels: Vec<String> = remote_tags
            .iter()
            .filter(|t| t.r#type.as_deref() == Some("folder"))
            .filter_map(|t| t.label.clone())
            .filter(|l| !l.is_empty())
            .collect();
        // A-4：分类墓碑——改名/删除过的 label 不复活；远端已不再列出即清墓碑
        let tombstones = db::folder_tombstones(&conn).unwrap_or_default();
        let mut active: Vec<String> = Vec::new();
        for stale in tombstones {
            if remote_labels.iter().any(|l| l == &stale) {
                active.push(stale);
            } else {
                let _ = db::remove_folder_tombstone(&conn, &stale);
            }
        }
        for label in &remote_labels {
            if active.iter().any(|t| t == label) {
                continue; // 本地已改名/删除：不按远端旧 label 复活目录
            }
            let existing = db::find_folder_by_name(&conn, label).ok().flatten();
            if existing.is_none() {
                let _ = db::create_folder(&conn, label, "article");
            }
        }
    }

    // 锁内：订阅按 URL 碰撞合并
    {
        let conn = db.lock().await;
        // A-1：本地已删除（墓碑）的订阅不复活；远端列表已不含的墓碑可清除
        let tombstones = db::feed_tombstones(&conn).unwrap_or_default();
        let remote_norm: Vec<String> = remote_subs
            .iter()
            .map(|rf| db::normalize_url(&rf.url))
            .collect();
        for stale in tombstones.iter().filter(|t| !remote_norm.contains(t)) {
            let _ = db::remove_feed_tombstone(&conn, stale);
        }
        for rf in &remote_subs {
            if tombstones.contains(&db::normalize_url(&rf.url)) {
                continue; // 本地已删除且远端仍列出：保留墓碑，跳过复活
            }
            // 用规范化 URL 匹配本地 feed（后端返回的 URL 与本地直连添加时
            // 常有协议/www./尾斜杠/跟踪参数差异，精确匹配会漏判成新订阅 → 同一
            // 订阅出现两个本地 feed，文章翻倍、状态分裂、数量对不齐）。
            let local_feed = db::feed_id_by_url_normalized(&conn, &rf.url).ok().flatten();
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
                    let _ = db::update_feed_title_if_empty(
                        &conn,
                        lid,
                        &rf.title,
                        rf.html_url.as_deref(),
                    );
                    report.merged_states += 1;
                }
                None => {
                    // 本地没有 → 建本地 feed（挂到远端分类对应的本地 folder）
                    let folder_id: i64 = remote_folder_label
                        .as_deref()
                        .and_then(|label| db::find_folder_by_name(&conn, label).ok().flatten())
                        .or_else(|| db::get_first_folder_id(&conn).ok().flatten())
                        .unwrap_or(1);
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
