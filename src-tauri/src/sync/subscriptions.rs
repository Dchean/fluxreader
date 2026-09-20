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
///
/// 返回值仅表示**请求是否被后端接受**（HTTP 2xx），**不等于「远端已删除该订阅」**：
/// GReader 的 `subscription/edit` 在 token 失效、权限不足或 `s=feed/<id>` 目标不存在时
/// 可能返回 2xx + 错误体（`greader::post_form_text` 只见状态码，看不到错误体）。
///
/// **因此这里不清除删除墓碑**。墓碑只由 `pull_feeds` 在「远端订阅列表实际已不含该 URL」
/// 时清除（见本文件下方 tombstone 收敛段）——那是唯一有证据支撑的清除条件。
/// 曾经在此处按 `ok` 清墓碑，会导致 2xx 但远端未生效时清掉唯一防线，
/// 下次 pull 见远端仍列出该订阅便把它**复活**（用户现象：「删掉的订阅自己回来了」）。
pub async fn unsubscribe_remote(
    db: &Arc<Mutex<Connection>>,
    http: &reqwest::Client,
    remote_id: i64,
    feed_url: &str,
) -> bool {
    let _ = feed_url; // 保留签名：调用方语义不变；墓碑不再由此处清除
    let Some(client) = build_client(db, http).await else {
        return false;
    };
    match client {
        Backend::GReader(c) => c.unsubscribe(remote_id).await.is_ok(),
        Backend::Fever(_) => false,
    }
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
                // TASK-056 修复轮 1（审查 FINDING 3）：此前是 `let _ = create_folder(...)`。
                // 静默失败会让后续 find_folder_by_name 落空 → 该分类下的订阅被改挂「未分类」，
                // **用户的目录结构无声丢失**——与本任务修复的外键缺陷同族。
                if let Err(e) = db::create_folder(&conn, label, "article") {
                    report
                        .errors
                        .push(format!("建远端分类「{label}」失败: {e}"));
                }
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
                    //
                    // 目录兜底（TASK-056）：远端无分类（或 label 在本地找不到）时必须挂到一个
                    // **真实存在**的 folder。此前是 `get_first_folder_id().unwrap_or(1)`，
                    // 而新装应用 folders 表为空 → 硬编码的 1 指向不存在目录 →
                    // `feeds.folder_id REFERENCES folders(id)` 外键违约（连接开启了
                    // `PRAGMA foreign_keys=ON`）→ 插入失败。改用既有
                    // `ensure_uncategorized_folder`（不存在则创建「未分类」），保证外键成立。
                    let folder_id: i64 = match remote_folder_label
                        .as_deref()
                        .and_then(|label| db::find_folder_by_name(&conn, label).ok().flatten())
                    {
                        Some(fid) => fid,
                        None => match db::ensure_uncategorized_folder(&conn) {
                            Ok(fid) => fid,
                            Err(e) => {
                                report.errors.push(format!(
                                    "订阅 {} 建目录失败: {e}",
                                    rf.url
                                ));
                                continue;
                            }
                        },
                    };
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
                    // TASK-056：失败必须可见。此前是无 else 的 `if let Ok(fid)`，
                    // 错误被静默吞掉、report.errors 保持为空，前端仍提示「已拉取订阅源」
                    // 「后端同步完成」——用户看到成功、实际零订阅。
                    match inserted {
                        Ok(fid) => {
                            if let Some(nid) = remote_feed_id {
                                let _ = db::set_feed_remote_id(&conn, fid, nid);
                            }
                            report.pulled_feeds += 1;
                        }
                        Err(e) => {
                            report
                                .errors
                                .push(format!("拉取订阅 {} 建本地失败: {e}", rf.url));
                        }
                    }
                }
            }
        }
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
    // ② 仅当该源的规范化 URL 不在本轮的远端订阅列表里才删。
    // ③ **不写 feed_tombstone**：墓碑的语义是「用户本地删除、不许复活」。这里
    //    是跟随远端事实删除；若用户在服务端重新订阅，下一轮 pull 应当把源正常
    //    建回来。写墓碑反而会让它在服务端重新出现时被永久跳过。
    // ④ pending 保护：队列里还有未推送的该 URL 变更时不删——本地动作尚未回传，
    //    不能被远端旧快照抢先抹掉。
    // ============================================================
    {
        let conn = db.lock().await;
        // 本轮远端订阅的规范化 URL 集合（判据②）
        let remote_norm_set: std::collections::HashSet<String> = remote_subs
            .iter()
            .map(|rf| db::normalize_url(&rf.url))
            .collect();
        // 查询失败不阻塞本轮（订阅层是尽力而为）：记错误并跳过删除段
        let candidates: Vec<(i64, String)> = match conn
            .prepare("SELECT id, feed_url FROM feeds WHERE origin = 'remote' AND remote_id IS NOT NULL")
            .and_then(|mut stmt| {
                let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
                rows.collect::<Result<Vec<_>, _>>()
            }) {
            Ok(v) => v,
            Err(e) => {
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
        let mut pending_feed_ids: std::collections::HashSet<i64> = conn
            .prepare(
                "SELECT DISTINCT a.feed_id FROM sync_queue q
                 JOIN articles a ON a.id = q.article_id
                 WHERE q.article_id IS NOT NULL",
            )
            .and_then(|mut stmt| {
                let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
                rows.collect::<Result<Vec<_>, _>>()
            })
            .map(|ids| ids.into_iter().collect())
            .unwrap_or_default();
        let pending_urls: std::collections::HashSet<String> = conn
            .prepare("SELECT DISTINCT feed_url FROM sync_queue WHERE feed_url IS NOT NULL")
            .and_then(|mut stmt| {
                let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
                rows.collect::<Result<Vec<_>, _>>()
            })
            .map(|urls| {
                urls.into_iter()
                    .map(|u| db::normalize_url(&u))
                    .collect::<std::collections::HashSet<String>>()
            })
            .unwrap_or_default();
        // 地址型队项还要能匹配到具体的源，统一折算成 feed_id，供下面单条件判断
        if !pending_urls.is_empty() {
            if let Ok(mut stmt) = conn.prepare("SELECT id, feed_url FROM feeds") {
                if let Ok(rows) = stmt.query_map([], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
                }) {
                    for row in rows.flatten() {
                        if pending_urls.contains(&db::normalize_url(&row.1)) {
                            pending_feed_ids.insert(row.0);
                        }
                    }
                }
            }
        }
        for (feed_id, feed_url) in candidates {
            let norm = db::normalize_url(&feed_url);
            if remote_norm_set.contains(&norm) {
                continue; // 远端仍订阅：保留
            }
            if pending_feed_ids.contains(&feed_id) {
                continue; // ④ 本地变更未推送（含 article 级）：不删，等回传后再收敛
            }
            match db::delete_feed(&conn, feed_id) {
                Ok(()) => {
                    report.removed_feeds += 1;
                    log::info!("pull_feeds: removed locally-deleted remote subscription {feed_url}");
                }
                Err(e) => report
                    .errors
                    .push(format!("远端已退订、删除本地源 {feed_url} 失败: {e}")),
            }
        }
    }
}
