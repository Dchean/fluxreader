//! sync 的 fever_pull 子模块（TASK-045 从 sync.rs 按既有章节拆分）。
// Note: 任一集合拉取失败即整轮跳过对账（失败 ≠ 空集合） — 见 .agents/notes/implemented/bug-fix/2026-09-18-状态写入事务化与对账守卫.md

use super::*;
use crate::db;
use crate::error::{AppError, AppResult};
use crate::fever;
use crate::greader::ItemContent;
use chrono::Utc;
use rusqlite::Connection;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Mutex;

/// 每个同步轮次的历史回溯页预算（50 条/页 → 单轮至多 1 万条）。
/// 到达预算**不是完成**：保存 checkpoint 并记「未完成」，下一次同步
/// （含自动 light）从 checkpoint 续取——不会永久只拉前一万条。
const HISTORY_MAX_PAGES_PER_SYNC: usize = 200;

/// 页内 id 范围（min, max）；非空页但无可解析 id → None（游标守卫据此报错）。
fn page_id_bounds(page: &[ItemContent]) -> Option<(i64, i64)> {
    let mut ids = page.iter().filter_map(item_numeric_id);
    let first = ids.next()?;
    let (mut min, mut max) = (first, first);
    for id in ids {
        min = min.min(id);
        max = max.max(id);
    }
    Some((min, max))
}

/// 页事务随页数据一起 commit 的游标写入（R2：页数据、checkpoint、since 同事务）。
enum PageCursorWrite {
    /// 增量页：连续 since 推进到该值。
    Since(i64),
    /// 历史续取页：checkpoint（Pending）推进到该值。
    Pending(i64),
    /// 顶部连续页：since 与 checkpoint 同一事务写入。
    Top { since: i64, pending: i64 },
}

/// 逐页落库（**页事务**）：页内所有 DB 写 + 该页的游标（since/checkpoint）在
/// 同一短事务里全有全无；离开函数后该页 `ItemContent`（含 HTML）即释放——
/// 不累计全历史（内存只留 `seen` 的 id 集合与当前页）。
///
/// 任何行/绑定/状态/回填/游标写失败 → 回滚事务，并回滚内存统计与 `seen`
/// （maps 由调用方按回滚后的 DB 重建——未提交绑定不得当事实）。
fn merge_page_tx(
    conn: &mut Connection,
    page: Vec<ItemContent>,
    seen: &mut HashSet<i64>,
    maps: &mut db::SyncMatchMaps,
    report: &mut SyncReport,
    cursor_write: Option<PageCursorWrite>,
) -> AppResult<()> {
    let stats = (report.pulled_entries, report.merged_states);
    let mut added: Vec<i64> = Vec::new();
    let tx = conn.transaction()?;
    let mut failure: Option<AppError> = None;
    for it in page {
        let eid = item_numeric_id(&it);
        if let Some(id) = eid {
            if seen.contains(&id) {
                continue; // 重复条目（页重叠）：本事务不重复处理
            }
        }
        match merge_pulled_entry(&tx, &it, maps, report) {
            Ok(()) => {
                if let Some(id) = eid {
                    seen.insert(id);
                    added.push(id);
                }
            }
            Err(e) => {
                failure = Some(e);
                break;
            }
        }
    }
    if failure.is_none() {
        if let Some(w) = cursor_write {
            let r = match w {
                PageCursorWrite::Since(v) => db::set_last_sync_entry_id(&tx, v),
                PageCursorWrite::Pending(v) => db::set_fever_history_pending(&tx, v),
                PageCursorWrite::Top { since, pending } => db::set_last_sync_entry_id(&tx, since)
                    .and_then(|_| db::set_fever_history_pending(&tx, pending)),
            };
            if let Err(e) = r {
                failure = Some(e);
            }
        }
    }
    if let Some(e) = failure {
        drop(tx); // 回滚：页数据与游标都不落地
        for id in added {
            seen.remove(&id);
        }
        report.pulled_entries = stats.0;
        report.merged_states = stats.1;
        return Err(e);
    }
    match tx.commit() {
        Ok(()) => Ok(()),
        Err(e) => {
            // COMMIT 失败同样按页失败处理：内存侧回滚（DB 侧由 rusqlite 回滚）。
            for id in added {
                seen.remove(&id);
            }
            report.pulled_entries = stats.0;
            report.merged_states = stats.1;
            Err(AppError::from(e))
        }
    }
}

/// 页事务失败后按 DB 重建匹配映射（未提交绑定不得当事实）。
/// **重建失败必须错误传播**：调用方立即终止本轮（不执行后续 history/with_ids/
/// 完成时间），Pending 与已确认 since 保留——绝不把 DB 错误退化成空映射继续
/// （空 feed 映射会让 merge 全部「合法跳过」，却照常确认 since/checkpoint）。
fn rebuild_maps_after_failure(
    conn: &Connection,
    maps: &mut db::SyncMatchMaps,
    report: &mut SyncReport,
) -> AppResult<()> {
    match db::sync_match_maps(conn) {
        Ok(m) => {
            *maps = m;
            Ok(())
        }
        Err(e) => {
            report.errors.push(format!(
                "页回滚后重建匹配映射失败（本轮终止，保留 Pending 与已确认游标）: {e}"
            ));
            Err(e)
        }
    }
}

/// Fever 拉取，三段式（R2：页事务 + 显式状态）：
/// ① 增量：`items&since_id` 向新分页。**非空页一律继续直到空页**——服务端
///    （FreshRSS `fever.php`）先 `LIMIT 50` 再由扩展 hook 过滤（hook 含
///    with_ids 全分支），49 条非空短页不代表没有后页。
/// ② 历史回溯：状态显式三态（Unknown/Pending/Complete）。缺失/损坏/读取失败
///    **绝不当完成**：Unknown（含旧库只有 since）自动从顶部补旧历史；损坏显式
///    报错且不动任何游标。每页「页数据 + checkpoint」同事务；预算/失败保留
///    Pending 续取点；只有真正走到空页才写 Complete。
/// ③ `unread_item_ids`/`saved_item_ids` 权威集合对账（失败 ≠ 空集合）。
///
/// 游标纪律：`last_sync_entry_id` 只由**连续成功**的增量页（或从顶部连续走下的
/// 历史页）在同页事务里推进；with_ids 补齐、Pending 续取页、失败页都不推动。
///
/// OPT-006：`session` 的代际在每个 HTTP→DB 边界复核（游标/历史状态读、Pending
/// 初始化、每页请求与页事务、Complete、对账、完成时间）。代际失配立即以
/// `staleSession` 终止本轮——Pending 页游标只对原会话有效，旧响应不落库。
/// Note: 页事务/显式状态/失败回滚语义 — 见 .agents/notes/implemented/architecture/2026-10-08-Fever身份与历史回溯.md
pub(super) async fn pull_entries_fever(
    db: &Arc<Mutex<Connection>>,
    session: &SyncSession,
    client: &fever::FeverClient,
    report: &mut SyncReport,
    full: bool,
) -> AppResult<()> {
    // 读取 since 与历史三态。状态损坏/读失败：显式报错并整轮不拉取、不动游标
    // （绝不能把损坏/未知当完成继续推进）。
    let (since_id, history_state) = {
        let conn = db.lock().await;
        // OPT-006：读游标/历史状态即边界——旧会话不得基于已切换账号的进度拉取。
        session.ensure_current(&conn)?;
        let since = if full {
            0
        } else {
            db::last_sync_entry_id(&conn).unwrap_or(0)
        };
        match db::fever_history_state(&conn) {
            Ok(s) => (since, s),
            Err(e) => {
                report.errors.push(format!(
                    "Fever 历史状态不可读，本轮不拉取条目（游标保留）: {e}"
                ));
                return Ok(());
            }
        }
    };

    // 历史计划：
    // - full / 首连（since==0）：从顶部全量重放（幂等；覆盖旧 Pending/Complete）。
    // - Unknown（旧库只有 since、无状态键）：必须启动回溯（从顶部补旧历史）。
    // - Pending(c)：从 c 续取（含 c==i64::MAX 的顶走重试）。
    // - Complete：无需回溯。
    let history_plan: Option<(i64, bool)> = if full || since_id == 0 {
        Some((i64::MAX, true))
    } else {
        match history_state {
            db::FeverHistoryState::Unknown => Some((i64::MAX, true)),
            db::FeverHistoryState::Pending(c) => Some((c, c == i64::MAX)),
            db::FeverHistoryState::Complete => None,
        }
    };

    // R2-P1：先初始化 Pending 成功，之后才允许任何更大的 since 落库。
    if let Some((start, _)) = history_plan {
        let conn = db.lock().await;
        // OPT-006：Pending 初始化是游标写边界——代际失配不得写。
        session.ensure_current(&conn)?;
        if let Err(e) = db::set_fever_history_pending(&conn, start) {
            report.errors.push(format!(
                "初始化 Fever 历史状态失败（未开始拉取，旧游标保留）: {e}"
            ));
            return Ok(());
        }
    }

    // OPT-006：权威集合请求前复核代际。
    if !session_is_current(db, session).await {
        return Err(SyncSession::stale_error());
    }

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

    // 匹配映射一次构建；逐页合并时增量更新（新绑定/新条目立即可被后续页匹配）。
    // R3-P1：读取失败**立即终止本轮**——绝不退化成空映射继续（空 feed 映射会让
    // merge「合法跳过」却照常确认 since/checkpoint）。Pending 与旧游标保留，
    // 不执行后续 history/with_ids/完成时间，等待下一次同步重试。
    let mut maps = {
        let conn = db.lock().await;
        session.ensure_current(&conn)?;
        match db::sync_match_maps(&conn) {
            Ok(m) => m,
            Err(e) => {
                report.errors.push(format!(
                    "同步匹配映射构建失败，本轮不拉取/不合并（Pending 与游标保留）: {e}"
                ));
                return Ok(());
            }
        }
    };

    let mut seen: HashSet<i64> = HashSet::new();
    // TASK-068：抓取失败计数——时间戳游标仅在无失败时推进（对称 greader 守卫）。
    let mut fetch_failures = 0usize;
    // R2：页事务（DB 级）失败单独计数——同样不推进完成时间、保留重拉游标。
    let mut merge_failures = 0usize;
    // 只由成功提交的页事务推进（见函数头游标纪律）；with_ids/Pending 页不推动。
    let mut since_cursor = since_id;

    // ② 增量（since_id>0）：非空页一律继续直到空页（短页 ≠ 结束）。
    // 游标必须严格前进：服务端若忽略 since_id / 返回重复页，页内 max 不会
    // 大于旧游标——显式报错并保留已拉进度，绝不无限循环。
    // 页事务：页数据 + since 同 commit；失败整页回滚并保留 since=cursor 重拉。
    if since_id > 0 {
        let mut cursor = since_id;
        loop {
            // OPT-006：每页请求前复核代际——账号已切换即停止后续请求。
            if !session_is_current(db, session).await {
                return Err(SyncSession::stale_error());
            }
            let page = match client.items_since(cursor).await {
                Ok(p) => p,
                Err(e) => {
                    fetch_failures += 1;
                    report.errors.push(format!("拉取增量条目失败: {e}"));
                    break;
                }
            };
            if page.is_empty() {
                break; // 空页才是结束
            }
            let next = match page_id_bounds(&page) {
                Some((_, max)) if max > cursor => max,
                _ => {
                    fetch_failures += 1;
                    report.errors.push(format!(
                        "Fever 增量分页游标未前进（since_id={cursor}）：已中止本轮，下轮重试"
                    ));
                    break;
                }
            };
            let mut guard = db.lock().await;
            // OPT-006：页事务（页数据 + since 同 commit）是游标写边界——代际
            // 失配整页丢弃：不合并、不推进 since，Pending 由账号提交事务重置。
            session.ensure_current(&guard)?;
            match merge_page_tx(
                &mut guard,
                page,
                &mut seen,
                &mut maps,
                report,
                Some(PageCursorWrite::Since(next)),
            ) {
                Ok(()) => {
                    cursor = next;
                    since_cursor = since_cursor.max(next);
                }
                Err(e) => {
                    merge_failures += 1;
                    report.errors.push(format!(
                        "增量页合并失败（已回滚，保留 since={cursor} 下轮重拉）: {e}"
                    ));
                    if rebuild_maps_after_failure(&guard, &mut maps, report).is_err() {
                        return Ok(()); // R3-P1：重建失败 = 不再有可信映射，立即终止本轮
                    }
                    break;
                }
            }
        }
    }

    // ③ 历史回溯：顶部重放（full/首连/Unknown）或 Pending 续取。
    // 页事务：页数据 + checkpoint（顶部页连带 since）同 commit；
    // 预算到达/分页失败/游标不前进都保留 Pending=当前页输入游标（可重拉），
    // 只有真正走到空页才写 Complete。
    // 游标取页内**最小** id（固定实现页内顺序均为 DESC；游标与顺序无关）；
    // 游标必须严格减小，否则（服务端不支持 max_id 等）显式失败并保留续取点。
    let mut history_completed = false;
    if let Some((start, top_walk)) = history_plan {
        let mut boundary_ok = true;
        if top_walk {
            // `max_id` 严格 `<` 不会返回恰好 i64::MAX 的条目；已读非收藏的它
            // 不在权威集合里、with_ids 补齐也摸不到。显式 with_ids 顶覆盖一次
            // （hook 对 with_ids 同样生效——被过滤时视为服务端没有该条目）。
            // OPT-006：请求前复核代际。
            if !session_is_current(db, session).await {
                return Err(SyncSession::stale_error());
            }
            match client.items_with_ids(&[i64::MAX]).await {
                Ok(page) => {
                    let bounds = page_id_bounds(&page);
                    let write = bounds.map(|(_, max)| PageCursorWrite::Top {
                        since: since_cursor.max(max),
                        pending: start,
                    });
                    let mut guard = db.lock().await;
                    // OPT-006：页事务是游标写边界——代际失配整页丢弃。
                    session.ensure_current(&guard)?;
                    match merge_page_tx(&mut guard, page, &mut seen, &mut maps, report, write) {
                        Ok(()) => {
                            if let Some((_, max)) = bounds {
                                since_cursor = since_cursor.max(max);
                            }
                        }
                        Err(e) => {
                            // 顶覆盖页失败：Pending=start（初始化已落库）保留，
                            // 下次同步（含 light）重新顶走——不提前完成。
                            merge_failures += 1;
                            report.errors.push(format!(
                                "历史顶部覆盖页失败（已回滚，Pending=MAX 保留）: {e}"
                            ));
                            if rebuild_maps_after_failure(&guard, &mut maps, report).is_err() {
                                return Ok(()); // R3-P1：重建失败立即终止本轮
                            }
                            boundary_ok = false;
                        }
                    }
                }
                Err(e) => {
                    // 顶覆盖网络失败 = 历史阶段没有开始：Pending=MAX 保留。
                    fetch_failures += 1;
                    report.errors.push(format!("拉取历史条目前失败: {e}"));
                    boundary_ok = false;
                }
            }
        }
        if boundary_ok {
            let mut cursor = start;
            let mut pages = 0usize;
            loop {
                if pages >= HISTORY_MAX_PAGES_PER_SYNC {
                    // 预算到达不是完成：Pending=cursor 已在库（初始化或上一页
                    // 事务），报告/完成时间同样如实反映未完成。
                    fetch_failures += 1;
                    report.errors.push(format!(
                        "Fever 历史回溯未完成：本轮已达 {HISTORY_MAX_PAGES_PER_SYNC} 页预算（Pending={cursor} 已保留，下次同步自动继续）"
                    ));
                    break;
                }
                // OPT-006：每页请求前复核代际。
                if !session_is_current(db, session).await {
                    return Err(SyncSession::stale_error());
                }
                let page = match client.items_before(cursor).await {
                    Ok(p) => p,
                    Err(e) => {
                        fetch_failures += 1;
                        report
                            .errors
                            .push(format!("拉取历史条目失败（Pending={cursor} 保留）: {e}"));
                        break;
                    }
                };
                if page.is_empty() {
                    history_completed = true; // 空数组 = 服务端保留的历史已取尽
                    break;
                }
                let Some((min, max)) = page_id_bounds(&page) else {
                    fetch_failures += 1;
                    report.errors.push(format!(
                        "Fever 历史页无可解析 id（max_id={cursor}）：已中止，下轮重试"
                    ));
                    break;
                };
                if min >= cursor {
                    fetch_failures += 1;
                    report.errors.push(format!(
                        "Fever 历史分页游标未前进（max_id={cursor}）：已中止，下轮重试"
                    ));
                    break;
                }
                let write = if top_walk {
                    PageCursorWrite::Top {
                        since: since_cursor.max(max),
                        pending: min,
                    }
                } else {
                    PageCursorWrite::Pending(min)
                };
                let mut guard = db.lock().await;
                // OPT-006：页事务是游标写边界——代际失配整页丢弃。
                session.ensure_current(&guard)?;
                match merge_page_tx(&mut guard, page, &mut seen, &mut maps, report, Some(write)) {
                    Ok(()) => {
                        if top_walk {
                            since_cursor = since_cursor.max(max);
                        }
                        cursor = min;
                        pages += 1;
                    }
                    Err(e) => {
                        merge_failures += 1;
                        report.errors.push(format!(
                            "历史页合并失败（已回滚，Pending={cursor} 保留）: {e}"
                        ));
                        if rebuild_maps_after_failure(&guard, &mut maps, report).is_err() {
                            return Ok(()); // R3-P1：重建失败立即终止本轮
                        }
                        break;
                    }
                }
            }
        }
        if history_completed {
            // 只有真正取尽才允许 Complete；写失败保持 Pending（下轮会再走到这里）。
            let conn = db.lock().await;
            // OPT-006：Complete 是历史状态写边界——代际失配不得落库。
            session.ensure_current(&conn)?;
            if let Err(e) = db::set_fever_history_complete(&conn) {
                merge_failures += 1;
                report.errors.push(format!(
                    "写 Complete 状态失败（保持 Pending，下轮重试）: {e}"
                ));
            }
        }
    }

    // ④ 权威集合中本地还没有正文的条目（未读/收藏），用 with_ids 分块补齐。
    // 这是 id 精确通道，结果**不**推动 `last_sync_entry_id`（游标纪律）。
    let mut need: Vec<i64> = unread
        .iter()
        .chain(starred.iter())
        .copied()
        .filter(|id| !seen.contains(id))
        .collect();
    need.sort_unstable();
    need.dedup();
    for chunk in need.chunks(50) {
        // OPT-006：补齐请求前复核代际。
        if !session_is_current(db, session).await {
            return Err(SyncSession::stale_error());
        }
        match client.items_with_ids(chunk).await {
            Ok(page) => {
                // 页事务（无游标写入）：失败整页回滚，**不**推动任何游标。
                let mut guard = db.lock().await;
                // OPT-006：合并是回写边界——代际失配整页丢弃。
                session.ensure_current(&guard)?;
                if let Err(e) = merge_page_tx(&mut guard, page, &mut seen, &mut maps, report, None)
                {
                    merge_failures += 1;
                    report
                        .errors
                        .push(format!("with_ids 补齐页合并失败（已回滚，游标不动）: {e}"));
                    if rebuild_maps_after_failure(&guard, &mut maps, report).is_err() {
                        return Ok(()); // R3-P1：重建失败立即终止本轮
                    }
                    break;
                }
            }
            Err(e) => {
                fetch_failures += 1;
                report.errors.push(format!("拉取未读/收藏条目失败: {e}"));
                break;
            }
        }
    }

    // ⑤ 权威状态对账：Fever 无法直接拉已读条目，靠「未读/收藏集合」反推。
    // 集合拉取失败时整段跳过（C-1），绝不做"空集合 = 远端全变"的对账。
    if reconcile_ok {
        let conn = db.lock().await;
        // OPT-006：对账是回写边界——代际失配丢弃整份权威集合。
        session.ensure_current(&conn)?;
        // 审计 P2-8③：对账内的 DB 写失败必须可见——记入 report.errors
        // （不再被 unwrap_or(0) 伪装成 0 行变化）。已写入行不回滚，
        // 剩余行下一轮同集合对账幂等重放。
        if let Err(e) = reconcile_fever_state(&conn, &unread, &starred, &maps, report) {
            report.errors.push(format!(
                "状态对账中断：本地状态写入失败（{e}），本轮剩余条目未对账"
            ));
        }
    }

    // ⑥ 完成时间。`last_sync_entry_id` 与历史 checkpoint 已在各页事务内提交
    // （页数据与游标同 commit），这里不再有「事务外的游标补写」。
    // 时间戳游标仅在「本轮窗口拿全」时推进——抓取失败、页事务失败、预算未完成
    // 与权威集合失败都算没拿全，否则切回 greader 时会跳过该窗口。
    let conn = db.lock().await;
    // OPT-006：完成时间推进前复核代际——旧会话不得写新账号的时间戳游标。
    session.ensure_current(&conn)?;
    let failures = fetch_failures + collection_failures + merge_failures;
    if failures == 0 {
        let _ = db::set_last_sync_ts(&conn, Utc::now().timestamp());
    } else {
        log::warn!(
            "fever pull: {fetch_failures} fetch(es) + {collection_failures} collection + {merge_failures} merge failure(s); keeping last_sync_ts"
        );
    }
    drop(conn);
    Ok(())
}

/// Fever 全量状态对账。Miniflux 按 URL 去重 entry，故 Fever 视角无跨源副本，
/// 已绑定条目（mf_id_to_article）的远端状态可直接信任：
/// - `unread_item_ids` 含 remote_id → 远端未读 → 本地未读；不含 → 已读（read-wins）
/// - `saved_item_ids` 含 remote_id → 本地收藏；不含 → 取消收藏
///
/// pending 保护：本地有未推送变更的条目跳过，防把「刚标读/刚收藏」瞬间回滚。
///
/// TASK-112：方向语义收口到 `conflict_policy.rs` 政策单点——本函数按
/// `FEVER_READ_DIRECTION`（UnreadSetBidirectional 双向权威）/
/// `FEVER_STAR_DIRECTION` 消费行级落地函数，不再自持方向分支；行为与
/// 显式化前逐列一致（收口不是改行为），双向选择的前提（Fever 只有
/// unread/saved 集合、Miniflux 按 URL 去重）与守卫说明见政策点及
/// `docs/sync-compat-matrix.md`。
///
/// 审计 P2-8③：返回 `AppResult<()>`——行级写失败经 `?` 中断本轮对账并向上
/// 传播（调用方记入 `report.errors`），不再像旧的 `unwrap_or(0)` 那样把 DB
/// 写失败伪装成「0 行变化」。已写入行不回滚；剩余行由下一轮同集合对账重放。
fn reconcile_fever_state(
    conn: &Connection,
    unread: &[i64],
    starred: &[i64],
    maps: &db::SyncMatchMaps,
    report: &mut SyncReport,
) -> AppResult<()> {
    use std::collections::HashSet;
    let unread_set: HashSet<i64> = unread.iter().copied().collect();
    let starred_set: HashSet<i64> = starred.iter().copied().collect();

    for (remote_id, aid) in &maps.mf_id_to_article {
        let aid = *aid;
        // TASK-112 政策（conflict_policy 头注「共享守卫」）：pending 保护——
        // 本地有未推送变更的条目跳过远端快照，交给 push 段队列，不被回滚。
        if maps.pending_ids.contains(&aid) {
            continue;
        }
        // TASK-112 政策（FEVER_READ_DIRECTION = UnreadSetBidirectional 双向权威）：
        // unread 集合权威——命中 → 本地未读（可复活未读）；未命中 → 本地已读。
        report.merged_states += conflict_policy::apply_read_by_policy(
            conn,
            conflict_policy::FEVER_READ_DIRECTION,
            !unread_set.contains(remote_id),
            aid,
        )?;
        // TASK-112 政策（FEVER_STAR_DIRECTION = AuthoritativeBidirectional 双向权威）：
        // 命中 → 收藏；未命中 → 取消收藏。
        report.merged_states += conflict_policy::apply_star_by_policy(
            conn,
            conflict_policy::FEVER_STAR_DIRECTION,
            starred_set.contains(remote_id),
            aid,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{NewArticle, MIGRATIONS};

    // TASK-113：本段是测试模块级说明而非某条目的文档——原 `///` 块后接空行
    // 触发 clippy empty_line_after_doc_comments，且列表后的未缩进续行触发
    // doc_lazy_continuation（-D warnings 下致命），故改普通注释（文字不变）。
    // TASK-112 政策锁定测试：Fever 对账的「操作 × 协议」格逐格锁死。
    // 判别力声明（对应政策被翻转/守卫被移除时必红）——
    // - 双向格：FEVER_READ_DIRECTION 被改成单向（RemoteReadWins）→
    //   remote_unread 复活用例红（本地已读不再被翻回未读）；
    // - 单向命中侧被移除 → read 落地用例红；
    // - pending 保护：守卫被移除 → pending 用例红。
    // 本卡行为零变化，故全部用例在显式化前后都绿；CI（cargo test）承担执行。
    //
    // 失败守卫（reconcile_ok：集合拉取失败跳过对账）与 max_id 历史回溯的
    // 端到端锁定在 tests/fever_compat_e2e.rs（OPT-005 严格 HTTP 夹具：缺字段/
    // 分页失败/游标不前进/长 id 字符串都真实过 HTTP 层），单元测试只锁纯逻辑。

    fn conn() -> rusqlite::Connection {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        conn
    }

    /// 造一篇已绑定远端 Fever item 的本地文章（feeds.feed_url UNIQUE →
    /// feed/folder/guid 按 remote_id 派生，同一连接可造多篇）。
    fn seed_bound(conn: &rusqlite::Connection, remote_id: i64) -> i64 {
        let folder = db::create_folder(conn, &format!("测试分类{remote_id}"), "article").unwrap();
        let feed = db::insert_feed(
            conn,
            &format!("https://f.example/{remote_id}.rss"),
            None,
            "F",
            None,
            folder,
            "inherit",
            false,
            false,
        )
        .unwrap();
        let a = NewArticle {
            guid: format!("g{remote_id}"),
            url: Some(format!("https://e.example/p/{remote_id}")),
            title: "t".into(),
            author: None,
            summary: None,
            content_html: None,
            body_text: "b".into(),
            image_url: None,
            enclosure_url: None,
            enclosure_mime: None,
            duration_sec: None,
            published_at: Some("2026-01-01T00:00:00+00:00".into()),
            source: "direct".into(),
        };
        let aid = db::upsert_article_with_feed(conn, feed, &a, false)
            .unwrap()
            .0;
        db::set_article_remote_id(conn, aid, remote_id).unwrap();
        aid
    }

    /// 单条映射的 SyncMatchMaps（reconcile 只消费 mf_id_to_article 与 pending_ids）。
    fn maps_for(remote_id: i64, aid: i64, pending: bool) -> db::SyncMatchMaps {
        db::SyncMatchMaps {
            url_to_id: Default::default(),
            id_to_mf_id: Default::default(),
            id_to_mf_pair: Default::default(),
            pending_ids: if pending {
                std::iter::once(aid).collect()
            } else {
                Default::default()
            },
            feed_mf_to_id: Default::default(),
            mf_id_to_article: std::iter::once((remote_id, aid)).collect(),
        }
    }

    fn is_read(conn: &rusqlite::Connection, aid: i64) -> bool {
        conn.query_row("SELECT is_read FROM articles WHERE id = ?1", [aid], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap()
            != 0
    }

    fn is_starred(conn: &rusqlite::Connection, aid: i64) -> bool {
        conn.query_row(
            "SELECT is_starred FROM articles WHERE id = ?1",
            [aid],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
            != 0
    }

    /// Fever 双向格核心锁定：远端 unread 集合命中 → 本地已读被翻回未读。
    /// 政策被改成单向（RemoteReadWins）时本用例红——这正是 Fever 与 GR 的
    /// 分叉点，必须显式锁死。
    #[test]
    fn fever_reconcile_remote_unread_revives_local_unread() {
        let conn = conn();
        let aid = seed_bound(&conn, 201);
        conn.execute("UPDATE articles SET is_read = 1 WHERE id = ?1", [aid])
            .unwrap();

        let mut report = SyncReport::default();
        reconcile_fever_state(&conn, &[201], &[], &maps_for(201, aid, false), &mut report).unwrap();

        assert!(
            !is_read(&conn, aid),
            "Fever 政策（unread 集合双向权威）：远端未读必须复活本地未读"
        );
        assert_eq!(report.merged_states, 1, "命中侧恰好一次状态写入");
    }

    /// Fever 双向格未命中侧：unread 集合不含 → 本地未读被翻成已读
    /// （Fever 无法列举已读条目，「未命中 = 已读」是唯一可用信号）。
    #[test]
    fn fever_reconcile_unread_miss_marks_local_read() {
        let conn = conn();
        let aid = seed_bound(&conn, 202);

        let mut report = SyncReport::default();
        reconcile_fever_state(&conn, &[], &[], &maps_for(202, aid, false), &mut report).unwrap();

        assert!(is_read(&conn, aid), "unread 未命中必须落地本地已读");
        assert_eq!(report.merged_states, 1, "未命中侧恰好一次状态写入");
    }

    /// Fever 星标格双向锁定：未命中 → 取消收藏；命中 → 收藏。
    #[test]
    fn fever_reconcile_starred_is_bidirectional() {
        let conn = conn();
        let starred = seed_bound(&conn, 203);
        let unstarred = seed_bound(&conn, 204);
        conn.execute(
            "UPDATE articles SET is_starred = 1 WHERE id = ?1",
            [starred],
        )
        .unwrap();

        let mut report = SyncReport::default();
        // saved 集合只含 204：203 未命中（远端已取消收藏），204 命中。
        let maps = db::SyncMatchMaps {
            mf_id_to_article: [(203, starred), (204, unstarred)].into_iter().collect(),
            ..maps_for(203, starred, false)
        };
        reconcile_fever_state(&conn, &[203, 204], &[204], &maps, &mut report).unwrap();

        assert!(
            !is_starred(&conn, starred),
            "Fever 星标双向：saved 未命中必须取消本地收藏"
        );
        assert!(
            is_starred(&conn, unstarred),
            "Fever 星标双向：saved 命中必须收藏本地"
        );
        // 读状态侧：两者都在 unread 集合 → 双向落点为「未读」，但两篇本就是
        // 未读（条件写不命中 0 行），不计入 merged_states。
        assert!(
            !is_read(&conn, starred) && !is_read(&conn, unstarred),
            "unread 命中的条目应保持未读"
        );
        assert_eq!(report.merged_states, 2, "星标双向各一次写入");
    }

    /// 共享守卫锁定：pending（已入队未推送）条目整行跳过——unread 命中
    /// 不能翻回未读、saved 未命中不能清收藏。守卫被移除时本用例红。
    #[test]
    fn fever_reconcile_pending_guard_blocks_snapshot_rollback() {
        let conn = conn();
        let aid = seed_bound(&conn, 205);
        conn.execute("UPDATE articles SET is_read = 1 WHERE id = ?1", [aid])
            .unwrap();
        conn.execute("UPDATE articles SET is_starred = 1 WHERE id = ?1", [aid])
            .unwrap();

        let mut report = SyncReport::default();
        // 远端快照：unread 命中 + saved 未命中——若无 pending 保护会同时翻转两列。
        reconcile_fever_state(&conn, &[205], &[], &maps_for(205, aid, true), &mut report).unwrap();

        assert!(
            is_read(&conn, aid),
            "pending 保护：本地已读不得被远端未读快照覆盖"
        );
        assert!(
            is_starred(&conn, aid),
            "pending 保护：本地收藏不得被远端未收藏快照清除"
        );
        assert_eq!(report.merged_states, 0, "pending 命中行不产生任何写入");
    }
}
