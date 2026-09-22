//! sync 的 greader_pull 子模块（TASK-045 从 sync.rs 按既有章节拆分）。

use super::*;
use crate::db;
use crate::error::AppResult;
use crate::greader::{self, GReaderClient};
use chrono::Utc;
use rusqlite::Connection;
use std::sync::Arc;
use tokio::sync::Mutex;

/// 拉远端条目（新条目 + 状态变化），按 remote_id/URL 匹配合并。
/// 分页三段式：每页「锁外拉取 → 锁内合并」，锁从不跨分页 HTTP await。
/// `full=true`（手动同步/首连）：先做绑定回填 + 全量状态对账。
/// `full=false`（后台自动同步）：只拉增量（ot 游标），便宜。
pub(super) async fn pull_entries_greader(
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
    // TASK-069 审查 F1：id 列举失败同样是「本轮没拿到该窗口」——不计入守卫的话，
    // all_item_ids 为空会让下方 chunks(100) 一次都不执行，chunk_failures 保持 0、
    // 游标照常推进，失败窗口被跳过（正是本守卫要关闭的漏文章形态）。故与分块失败同源计数。
    let mut id_failures = 0usize;
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
                id_failures += 1;
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
        // TASK-069 审查 F1：分页未走完就中断同样是「窗口没拿全」——静默 break 会被
        // 下游误当成「本轮无新条目」，故显式计数 + 记录错误（下一轮重拉同一窗口）。
        match r.continuation.as_deref() {
            None | Some("") => break,
            Some(c) => match c.parse::<u64>() {
                Ok(next) if got > 0 => continuation = Some(next),
                Ok(_) => {
                    id_failures += 1;
                    report.errors.push(format!(
                        "拉取条目 id 分页中断：continuation={c} 但本页无可用条目 id"
                    ));
                    break;
                }
                Err(_) => {
                    id_failures += 1;
                    report
                        .errors
                        .push(format!("拉取条目 id 分页中断：无法解析 continuation={c}"));
                    break;
                }
            },
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

    // TASK-068：分块失败计数——失败块的条目本轮丢失，若游标照常推进，下一轮
    // 增量从新游标起步，这些条目就只能等全量同步补回（「偶发漏文章」的温床）。
    let mut chunk_failures = 0usize;
    for chunk in all_item_ids.chunks(100) {
        let entries = match client.item_contents(chunk).await {
            Ok(v) => v,
            Err(e) => {
                chunk_failures += 1;
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
        // 拉取失败 ≠ 空集合（C-1）：任一权威集合拉取失败即跳过本轮对账，
        // 避免把网络/服务端错误当成"远端什么都没有"，静默清空本地收藏
        match tokio::join!(
            fetch_stream_ids(client, greader::tags::READ),
            fetch_stream_ids(client, greader::tags::STARRED),
        ) {
            (Ok(read_ids), Ok(starred_ids)) => {
                let conn = db.lock().await;
                reconcile_reader_state(&conn, &read_ids, &starred_ids, &maps, report);
                drop(conn);
            }
            (Err(e), _) | (_, Err(e)) => {
                report.errors.push(format!(
                    "状态对账跳过：远端状态集合拉取失败（{e}），本轮不合并远端状态"
                ));
            }
        }
    }

    // 更新游标（unix 秒）。TASK-068/069：仅在本轮「窗口确实拿全」时推进——
    // id 列举失败/分页中断（id_failures）与分块失败（chunk_failures）都算没拿全，
    // 下一轮重拉同一窗口补回（合并幂等，不会产生重复条目）。
    let failures = id_failures + chunk_failures;
    if failures == 0 {
        let now = Utc::now().timestamp();
        let conn = db.lock().await;
        let _ = db::set_last_sync_ts(&conn, now);
        drop(conn);
    } else {
        log::warn!(
            "greader pull: {id_failures} id-listing failure(s) + {chunk_failures} chunk(s) failed; keeping last_sync_ts（下一轮重拉同一窗口）"
        );
    }
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
            if let Ok(n) = db::sync_mark_read_if_unread(conn, aid) {
                report.merged_states += n;
            }
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
