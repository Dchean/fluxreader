//! sync 的 fever_pull 子模块（TASK-045 从 sync.rs 按既有章节拆分）。

use super::*;
use crate::db;
use crate::fever;
use crate::greader::ItemContent;
use chrono::Utc;
use rusqlite::Connection;
use std::sync::Arc;
use tokio::sync::Mutex;

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
pub(super) async fn pull_entries_fever(
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

    // ① 权威状态集合（全量 id）：未读 + 收藏。
    // 拉取失败 ≠ 空集合（C-1）：失败即跳过本轮对账（下方 ⑤ 用 reconcile_ok 守卫），
    // 避免静默把本地全部标为已读 / 清空收藏——Fever 对账为远端权威双向语义，误判代价更高。
    // TASK-069 审查 F1：该失败同时意味着「权威集合没拿全」，与②的分块失败同源，
    // 故一并计入守卫——否则时间戳游标照常推进，切回 greader 时会跳过这个窗口。
    let mut collection_failures = 0usize;
    let (unread, starred) = tokio::join!(client.unread_item_ids(), client.saved_item_ids());
    let (unread, starred, reconcile_ok) = match (unread, starred) {
        (Ok(u), Ok(s)) => (u, s, true),
        (Err(e), _) | (_, Err(e)) => {
            collection_failures += 1;
            report.errors.push(format!(
                "状态对账跳过：Fever 状态集合拉取失败（{e}），本轮不合并远端状态"
            ));
            (Vec::new(), Vec::new(), false)
        }
    };

    // ② 拉条目正文：增量（since_id>0）或首次种子（since_id=0 → 最近 50 条）
    let mut all_items: Vec<ItemContent> = Vec::new();
    let mut seen: HashSet<i64> = HashSet::new();
    // TASK-068：抓取失败计数——时间戳游标仅在无失败时推进（对称 greader 守卫；
    // last_sync_entry_id 只计已合并条目，本就安全）。
    let mut fetch_failures = 0usize;

    if since_id > 0 {
        // 增量：items&since_id 升序分页，单页 50，不足 50 即拿完
        let mut cursor = since_id;
        loop {
            let batch = match client.items_since(cursor).await {
                Ok(b) => b,
                Err(e) => {
                    fetch_failures += 1;
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
            Err(e) => {
                fetch_failures += 1;
                report.errors.push(format!("拉取最近条目失败: {e}"));
            }
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
                fetch_failures += 1;
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
    // 集合拉取失败时整段跳过（C-1），绝不做"空集合 = 远端全变"的对账。
    if reconcile_ok {
        let conn = db.lock().await;
        reconcile_fever_state(&conn, &unread, &starred, &maps, report);
    }

    // ⑥ 更新游标（Fever 用条目 id；时间戳游标也记录，供切换回 greader 后的首拉）
    let conn = db.lock().await;
    let _ = db::set_last_sync_entry_id(&conn, last_id);
    // TASK-068/069：时间戳游标仅在「本轮窗口拿全」时推进——抓取失败与权威集合
    // 失败都算没拿全，否则切回 greader 时会跳过该窗口。
    let failures = fetch_failures + collection_failures;
    if failures == 0 {
        let _ = db::set_last_sync_ts(&conn, Utc::now().timestamp());
    } else {
        log::warn!(
            "fever pull: {fetch_failures} fetch(es) + {collection_failures} collection failure(s); keeping last_sync_ts"
        );
    }
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
            if let Ok(n) = db::sync_mark_read_if_unread(conn, aid) {
                report.merged_states += n;
            }
        } else if let Ok(n) = db::sync_mark_unread_if_read(conn, aid) {
            report.merged_states += n;
        }
        if starred_set.contains(remote_id) {
            if let Ok(n) = db::sync_mark_starred_if_unstarred(conn, aid) {
                report.merged_states += n;
            }
        } else if let Ok(n) = db::sync_mark_unstarred_if_starred(conn, aid) {
            report.merged_states += n;
        }
    }
}
