//! sync 的 fever_pull 子模块（TASK-045 从 sync.rs 按既有章节拆分）。
// Note: 任一集合拉取失败即整轮跳过对账（失败 ≠ 空集合） — 见 .agents/notes/implemented/architecture/2026-11-13-同步写回与状态一致性.md

use super::*;
use crate::db;
use crate::error::{AppError, AppResult};
use crate::fever;
use crate::greader::ItemContent;
use chrono::Utc;
use rusqlite::Connection;
use std::collections::{BTreeSet, HashSet};
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

fn unbound_page_feeds(page: &[ItemContent], maps: &db::SyncMatchMaps) -> BTreeSet<i64> {
    page.iter()
        .filter_map(|entry| entry_unbound_feed(entry, maps))
        .collect()
}

/// B2：with_ids 回补的 need 集合构造（预过滤的纯逻辑部分，返回**未排序**的
/// 候选，由调用方 sort+dedup 后作为请求集合）。两个排除维度彼此独立：
/// - `seen`：本轮页事务已合并过的条目（重复处理无意义）；
/// - `has_body`：DB 侧判定「本地已有正文」的 id——已有正文的条目每轮重拉
///   是纯固定成本（审计 F1 反例）；无绑定行的 id 不在 has_body 里，天然
///   留在 need（未拉过的远端条目不得被误跳过）。
fn with_ids_need(
    unread: &[i64],
    starred: &[i64],
    seen: &HashSet<i64>,
    has_body: &HashSet<i64>,
) -> Vec<i64> {
    unread
        .iter()
        .chain(starred.iter())
        .copied()
        .filter(|id| !seen.contains(id) && !has_body.contains(id))
        .collect()
}

/// W3-2：单页 with_ids 并发拉取的结果（代际失配与网络失败分开——前者不发
/// 请求、不计数，直接 staleSession 终止；后者按既有 fetch_failures 语义计数）。
enum FeverPageFetch {
    Page(Vec<ItemContent>),
    Failed(AppError),
    Stale,
}

/// 单页 with_ids 拉取：请求前复核代际（与串行版 `session_is_current` 同语义）。
async fn fetch_with_ids_page(
    db: &Arc<Mutex<Connection>>,
    session: &SyncSession,
    client: &fever::FeverClient,
    ids: &[i64],
) -> FeverPageFetch {
    if !session_is_current(db, session).await {
        return FeverPageFetch::Stale;
    }
    match client.items_with_ids(ids).await {
        Ok(page) => FeverPageFetch::Page(page),
        Err(e) => FeverPageFetch::Failed(e),
    }
}

/// 批内并发拉取（`tokio::join!` 定元数展开；并发度复用
/// `greader_pull::PULL_FETCH_CONCURRENCY`，与 GR 两个独立请求通道同口径）；
/// 返回按**原页顺序**排列的结果，调用方逐页顺序合并（seen/maps 就地突变
/// 依赖顺序）。批间天然构成同步屏障，在途响应内存有界。
async fn fetch_with_ids_batch(
    db: &Arc<Mutex<Connection>>,
    session: &SyncSession,
    client: &fever::FeverClient,
    batch: &[&[i64]],
) -> Vec<FeverPageFetch> {
    match batch.len() {
        0 => Vec::new(),
        1 => vec![fetch_with_ids_page(db, session, client, batch[0]).await],
        2 => {
            let (a, b) = tokio::join!(
                fetch_with_ids_page(db, session, client, batch[0]),
                fetch_with_ids_page(db, session, client, batch[1]),
            );
            vec![a, b]
        }
        // 与 greader_pull::fetch_chunk_batch 同契约：臂随 PULL_FETCH_CONCURRENCY
        // 同步扩展，超界显式 panic（release 生效）——绝不静默丢块（W3 审计 P3-1）。
        3 => {
            let (a, b, c) = tokio::join!(
                fetch_with_ids_page(db, session, client, batch[0]),
                fetch_with_ids_page(db, session, client, batch[1]),
                fetch_with_ids_page(db, session, client, batch[2]),
            );
            vec![a, b, c]
        }
        n => panic!(
            "fetch_with_ids_batch 批次大小 {n} 超出已实现的并发臂（PULL_FETCH_CONCURRENCY={PULL_FETCH_CONCURRENCY}）；调大并发常量必须同步扩展本 match 的臂"
        ),
    }
}

fn restore_fever_pull_state(
    conn: &mut Connection,
    previous_entry_id: i64,
    previous_history: db::FeverHistoryState,
) -> AppResult<()> {
    let tx = crate::db::write_transaction(conn)?;
    db::set_last_sync_entry_id(&tx, previous_entry_id)?;
    let restored = match previous_history {
        db::FeverHistoryState::Unknown => db::reset_fever_history_state(&tx),
        db::FeverHistoryState::Pending(cursor) => db::set_fever_history_pending(&tx, cursor),
        db::FeverHistoryState::Complete => db::set_fever_history_complete(&tx),
    };
    restored?;
    tx.commit()?;
    Ok(())
}

async fn restore_after_binding_failure(
    db: &Arc<Mutex<Connection>>,
    session: &SyncSession,
    previous_entry_id: i64,
    previous_history: db::FeverHistoryState,
    report: &mut SyncReport,
) -> AppResult<()> {
    let generation = session.generation;
    let restored = crate::state::database_work(db.clone(), move |conn| {
        let _writer = db::writer_guard(conn)?;
        SyncSession::ensure_generation(conn, generation)?;
        Ok(restore_fever_pull_state(
            conn,
            previous_entry_id,
            previous_history,
        ))
    })
    .await?;
    if let Err(e) = restored {
        record_pull_apply_failure(e, report, |e| {
            format!("条目绑定失败后原子恢复游标/历史失败: {e}")
        })?;
    }
    Ok(())
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

/// Both page identities and their merge map belong exclusively to this pull.
/// Only a completed worker may return them; a worker failure terminates the pull.
#[allow(clippy::too_many_arguments)]
async fn apply_fever_page(
    db: &Arc<Mutex<Connection>>,
    generation: i64,
    page: Vec<ItemContent>,
    seen: &mut HashSet<i64>,
    maps: &mut db::SyncMatchMaps,
    report: &mut SyncReport,
    cursor_write: Option<PageCursorWrite>,
) -> AppResult<AppResult<()>> {
    let mut owned_seen = std::mem::take(seen);
    let (returned_seen, result) =
        pull_database_work(db, generation, maps, report, move |conn, maps, report| {
            let _writer = db::writer_guard(conn)?;
            SyncSession::ensure_generation(conn, generation)?;
            let result = merge_page_tx(conn, page, &mut owned_seen, maps, report, cursor_write);
            Ok((owned_seen, result))
        })
        .await??;
    *seen = returned_seen;
    Ok(result)
}

/// 逐页落库（**页事务**）：页内所有 DB 写 + 该页的游标（since/checkpoint）在
/// 同一短事务里全有全无；离开函数后该页 `ItemContent`（含 HTML）即释放——
/// 不累计全历史（内存只留 `seen` 的 id 集合与当前页）。
///
/// 任何行/绑定/状态/回填/游标写失败 → 回滚事务，并回滚内存统计（含 R3 数据
/// 变化标记）与 `seen`（maps 由调用方按回滚后的 DB 重建——未提交绑定不得
/// 当事实）。
fn merge_page_tx(
    conn: &mut Connection,
    page: Vec<ItemContent>,
    seen: &mut HashSet<i64>,
    maps: &mut db::SyncMatchMaps,
    report: &mut SyncReport,
    cursor_write: Option<PageCursorWrite>,
) -> AppResult<()> {
    let stats = (
        report.pulled_entries,
        report.merged_states,
        report.entries_data_changed,
    );
    let mut added: Vec<i64> = Vec::new();
    let tx = crate::db::write_transaction(conn)?;
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
                if report.binding_incomplete {
                    failure = Some(AppError::new(
                        "bindingIncomplete",
                        "条目来源基线失配，本页与游标保留供下一轮重试",
                    ));
                    break;
                }
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
        report.entries_data_changed = stats.2;
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
            report.entries_data_changed = stats.2;
            Err(e)
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
        Ok(mut m) => {
            // OPT-007：重建按 DB 现状，但字段版本快照保持轮内原值——与 GR 失败
            // 重建同口径（见 db/sync_map.rs 的 keep_field_versions_from）。
            m.keep_field_versions_from(maps);
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
    versions: Option<&db::SyncMatchMaps>,
) -> AppResult<()> {
    let generation = session.generation;
    if !pull_admitted(db, session, report).await? {
        return Ok(());
    }

    // 匹配映射一次构建；逐页合并时增量更新（新绑定/新条目立即可被后续页匹配）。
    // R3-P1：读取失败**立即终止本轮**——绝不退化成空映射继续（空 feed 映射会让
    // merge「合法跳过」却照常确认 since/checkpoint）。
    //
    // OPT-007 字段版本快照也必须在任何 HTTP 请求之前抓取。这里把它提前到
    // history Pending 初始化之前：零远端 feed 绑定时既不写历史状态，也不请求
    // 条目/权威集合。
    let snapshot = crate::state::database_work(db.clone(), move |conn| {
        SyncSession::ensure_generation(conn, generation)?;
        Ok(db::sync_match_maps(conn))
    })
    .await?;
    let mut maps = {
        match snapshot {
            Ok(m) => m,
            Err(e) => {
                record_pull_apply_failure(e, report, |e| {
                    format!("同步匹配映射构建失败，本轮不拉取/不合并（Pending 与游标保留）: {e}")
                })?;
                return Ok(());
            }
        }
    };

    if let Some(versions) = versions {
        maps.keep_field_versions_from(versions);
        maps.pending_read_ids.extend(&versions.pending_read_ids);
        maps.pending_star_ids.extend(&versions.pending_star_ids);
    }

    // Note: 零远端 feed 绑定不得初始化历史、请求集合或推进游标 — 见
    // .agents/notes/implemented/architecture/2026-11-13-同步写回与状态一致性.md
    if maps.feed_mf_to_id.is_empty() {
        log::debug!("fever pull: 尚无远端 feed 绑定，本轮跳过条目拉取");
        return Ok(());
    }

    // 读取 since 与历史三态。状态损坏/读失败：显式报错并整轮不拉取、不动游标
    // （绝不能把损坏/未知当完成继续推进）。
    let history = crate::state::database_work(db.clone(), move |conn| {
        // OPT-006：读游标/历史状态即边界——旧会话不得基于已切换账号的进度拉取。
        SyncSession::ensure_generation(conn, generation)?;
        let previous = db::last_sync_entry_id(conn)?;
        let since = if full { 0 } else { previous };
        Ok(db::fever_history_state(conn).map(|state| (previous, since, state)))
    })
    .await?;
    let (previous_entry_id, since_id, history_state) = {
        match history {
            Ok(snapshot) => snapshot,
            Err(e) => {
                record_pull_apply_failure(e, report, |e| {
                    format!("Fever 历史状态不可读，本轮不拉取条目（游标保留）: {e}")
                })?;
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
        let generation = session.generation;
        // OPT-006：Pending 初始化是游标写边界——代际失配不得写。
        let pending = crate::state::database_work(db.clone(), move |conn| {
            let _writer = db::writer_guard(conn)?;
            SyncSession::ensure_generation(conn, generation)?;
            Ok(db::set_fever_history_pending(conn, start))
        })
        .await?;
        if let Err(e) = pending {
            record_pull_apply_failure(e, report, |e| {
                format!("初始化 Fever 历史状态失败（未开始拉取，旧游标保留）: {e}")
            })?;
            return Ok(());
        }
    }

    // OPT-006：权威集合请求前复核代际。
    if !session_is_current(db, session).await {
        return Err(SyncSession::stale_error());
    }

    // maps 已在任何游标初始化/HTTP 请求前构建，见函数入口。

    // ① 权威状态集合（全量 id）：未读 + 收藏。
    // 拉取失败 ≠ 空集合（C-1）：失败即跳过本轮集合正文回补，
    // 避免静默把本地全部标为已读 / 清空收藏——Fever 对账为远端权威双向语义，误判代价更高。
    // TASK-069 审查 F1：该失败同时意味着「权威集合没拿全」，与②的分块失败同源，
    // 故一并计入守卫——否则时间戳游标照常推进，切回 greader 时会跳过这个窗口。
    let mut collection_failures = 0usize;
    let authority = match report.authority_snapshot.clone() {
        Some(authority) => authority,
        None => load_fever_authority(client, None, report).await,
    };
    let (unread, starred) = match (&authority.unread, &authority.starred) {
        (Some(unread), Some(starred)) => (unread.clone(), starred.clone()),
        _ => {
            collection_failures += 1;
            (Vec::new(), Vec::new())
        }
    };
    // 匹配映射已在权威集合请求前构建（字段版本快照必须早于任何 HTTP，见上方）。

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
                    report.push_client_error(&e, format!("拉取增量条目失败: {e}"));
                    break;
                }
            };
            if page.is_empty() {
                break; // 空页才是结束
            }
            let unbound_feeds = unbound_page_feeds(&page, &maps);
            if !unbound_feeds.is_empty() {
                report.binding_incomplete = true;
                report.errors.push(format!(
                    "Fever 增量页含尚无本地绑定的远端 feed，未提交任何条目/游标: {:?}",
                    unbound_feeds
                ));
                return restore_after_binding_failure(
                    db,
                    session,
                    previous_entry_id,
                    history_state,
                    report,
                )
                .await;
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
            // OPT-006：页事务（页数据 + since 同 commit）是游标写边界——代际
            // 失配整页丢弃：不合并、不推进 since，Pending 由账号提交事务重置。
            match apply_fever_page(
                db,
                session.generation,
                page,
                &mut seen,
                &mut maps,
                report,
                Some(PageCursorWrite::Since(next)),
            )
            .await?
            {
                Ok(()) => {
                    cursor = next;
                    since_cursor = since_cursor.max(next);
                }
                Err(e) => {
                    merge_failures += 1;
                    record_pull_apply_failure(e, report, |e| {
                        format!("增量页合并失败（已回滚，保留 since={cursor} 下轮重拉）: {e}")
                    })?;
                    if pull_database_work(
                        db,
                        generation,
                        &mut maps,
                        report,
                        |conn, maps, report| rebuild_maps_after_failure(conn, maps, report),
                    )
                    .await?
                    .is_err()
                    {
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
                    let unbound_feeds = unbound_page_feeds(&page, &maps);
                    if !unbound_feeds.is_empty() {
                        report.binding_incomplete = true;
                        report.errors.push(format!(
                            "Fever 历史顶部页含尚无本地绑定的远端 feed，未提交条目/游标: {:?}",
                            unbound_feeds
                        ));
                        return restore_after_binding_failure(
                            db,
                            session,
                            previous_entry_id,
                            history_state,
                            report,
                        )
                        .await;
                    } else {
                        let bounds = page_id_bounds(&page);
                        let write = bounds.map(|(_, max)| PageCursorWrite::Top {
                            since: since_cursor.max(max),
                            pending: start,
                        });
                        // OPT-006：页事务是游标写边界——代际失配整页丢弃。
                        match apply_fever_page(
                            db,
                            session.generation,
                            page,
                            &mut seen,
                            &mut maps,
                            report,
                            write,
                        )
                        .await?
                        {
                            Ok(()) => {
                                if let Some((_, max)) = bounds {
                                    since_cursor = since_cursor.max(max);
                                }
                            }
                            Err(e) => {
                                // 顶覆盖页失败：Pending=start（初始化已落库）保留，
                                // 下次同步（含 light）重新顶走——不提前完成。
                                merge_failures += 1;
                                record_pull_apply_failure(e, report, |e| {
                                    format!("历史顶部覆盖页失败（已回滚，Pending=MAX 保留）: {e}")
                                })?;
                                if pull_database_work(
                                    db,
                                    generation,
                                    &mut maps,
                                    report,
                                    |conn, maps, report| {
                                        rebuild_maps_after_failure(conn, maps, report)
                                    },
                                )
                                .await?
                                .is_err()
                                {
                                    return Ok(()); // R3-P1：重建失败立即终止本轮
                                }
                                boundary_ok = false;
                            }
                        }
                    }
                }
                Err(e) => {
                    // 顶覆盖网络失败 = 历史阶段没有开始：Pending=MAX 保留。
                    fetch_failures += 1;
                    report.push_client_error(&e, format!("拉取历史条目前失败: {e}"));
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
                        report.push_client_error(
                            &e,
                            format!("拉取历史条目失败（Pending={cursor} 保留）: {e}"),
                        );
                        break;
                    }
                };
                if page.is_empty() {
                    history_completed = true; // 空数组 = 服务端保留的历史已取尽
                    break;
                }
                let unbound_feeds = unbound_page_feeds(&page, &maps);
                if !unbound_feeds.is_empty() {
                    report.binding_incomplete = true;
                    report.errors.push(format!(
                        "Fever 历史页含尚无本地绑定的远端 feed，未提交条目/checkpoint: {:?}",
                        unbound_feeds
                    ));
                    return restore_after_binding_failure(
                        db,
                        session,
                        previous_entry_id,
                        history_state,
                        report,
                    )
                    .await;
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
                // OPT-006：页事务是游标写边界——代际失配整页丢弃。
                match apply_fever_page(
                    db,
                    session.generation,
                    page,
                    &mut seen,
                    &mut maps,
                    report,
                    Some(write),
                )
                .await?
                {
                    Ok(()) => {
                        if top_walk {
                            since_cursor = since_cursor.max(max);
                        }
                        cursor = min;
                        pages += 1;
                    }
                    Err(e) => {
                        merge_failures += 1;
                        record_pull_apply_failure(e, report, |e| {
                            format!("历史页合并失败（已回滚，Pending={cursor} 保留）: {e}")
                        })?;
                        if pull_database_work(
                            db,
                            generation,
                            &mut maps,
                            report,
                            |conn, maps, report| rebuild_maps_after_failure(conn, maps, report),
                        )
                        .await?
                        .is_err()
                        {
                            return Ok(()); // R3-P1：重建失败立即终止本轮
                        }
                        break;
                    }
                }
            }
        }
        if history_completed {
            // 只有真正取尽才允许 Complete；写失败保持 Pending（下轮会再走到这里）。
            let generation = session.generation;
            // OPT-006：Complete 是历史状态写边界——代际失配不得落库。
            let complete = crate::state::database_work(db.clone(), move |conn| {
                let _writer = db::writer_guard(conn)?;
                SyncSession::ensure_generation(conn, generation)?;
                Ok(db::set_fever_history_complete(conn))
            })
            .await?;
            if let Err(e) = complete {
                merge_failures += 1;
                record_pull_apply_failure(e, report, |e| {
                    format!("写 Complete 状态失败（保持 Pending，下轮重试）: {e}")
                })?;
            }
        }
    }

    // ④ 权威集合中本地还没有正文的条目（未读/收藏），用 with_ids 分块补齐。
    // 这是 id 精确通道，结果**不**推动 `last_sync_entry_id`（游标纪律）。
    let candidates: Vec<i64> = unread
        .iter()
        .chain(starred.iter())
        .copied()
        .filter(|id| !seen.contains(id))
        .collect();
    // B2 预过滤：构造 need 前先按 DB 排除「本地已有正文」的 id——注释声称的
    // 语义此前只排除了本轮 seen，权威集合里已有正文的条目每轮被全量重拉
    // （审计实测：2 万条库最坏 403 次串行往返/轮的固定成本）。has_body 取
    // 补集：无绑定行的 id 天然留在 need（未拉过的远端条目不得被误跳过）。
    // 状态对账由 ⑤ 的权威集合独立承担，不依赖本次正文往返。DB 读失败 =
    // 无法判定哪些已有正文 = has_body 视为空集（fail-open 退回全量重拉：
    // 宁多打请求，不漏补正文）。
    // Note: 预过滤口径与 fail-open 方向的取舍 — 见 .agents/notes/implemented/architecture/2026-11-13-同步写回与状态一致性.md
    let has_body: HashSet<i64> = if candidates.is_empty() {
        HashSet::new()
    } else {
        let covered = crate::state::database_work(db.clone(), move |conn| {
            SyncSession::ensure_generation(conn, generation)?;
            Ok(db::remote_ids_with_body(conn, &candidates))
        })
        .await?;
        {
            match covered {
                Ok(with_body) => with_body.into_iter().collect(),
                Err(e) => {
                    record_pull_apply_failure(e, report, |e| {
                        format!("读取本地正文覆盖失败，with_ids 退回全量补齐: {e}")
                    })?;
                    HashSet::new()
                }
            }
        }
    };
    let mut need = with_ids_need(&unread, &starred, &seen, &has_body);
    // Cold proof is separate from body presence. Reuse this Content lane's
    // with_ids transport, at most 2000 cold identities per round, with rotation.
    let mut cold: Vec<i64> = report
        .unverified_state_ids
        .iter()
        .copied()
        .filter(|id| {
            !maps.verified_remote_ids.contains(id)
                && !seen.contains(id)
                && !report.content_requested_ids.contains(id)
        })
        .collect();
    let previous = crate::state::database_work(db.clone(), move |conn| {
        SyncSession::ensure_generation(conn, generation)?;
        Ok(db::get_setting(conn, "fever_origin_proof_cursor")?
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .filter(|value| value["generation"].as_i64() == Some(generation))
            .and_then(|value| value["last_id"].as_i64()))
    })
    .await?;
    {
        if let Some(previous) = previous {
            let start = cold.partition_point(|id| *id <= previous);
            cold.rotate_left(start);
        }
    }
    cold.truncate(2000usize.saturating_sub(report.origin_proof_requested_ids.len()));
    need.extend(cold.iter().copied());
    need.sort_unstable();
    need.dedup();
    let failures_before_proof = (fetch_failures, merge_failures);
    // W3-2：with_ids 回补**有界并发**（批内 ≤ PULL_FETCH_CONCURRENCY 个在途，
    // 按原页顺序逐页短事务合并）；首块失败即停（fetch_failures 计数、游标不动
    // 语义逐字不变），同批在途请求不可撤回属既有「已发出的请求不能撤回」边界，
    // 后续批次不再发起。分页列举（since/max_id 链）保持串行不变。
    let page_slices: Vec<&[i64]> = need.chunks(50).collect();
    'with_ids: for start in (0..page_slices.len()).step_by(PULL_FETCH_CONCURRENCY) {
        let end = (start + PULL_FETCH_CONCURRENCY).min(page_slices.len());
        for chunk in &page_slices[start..end] {
            report.content_requested_ids.extend(chunk.iter().copied());
            report
                .origin_proof_requested_ids
                .extend(chunk.iter().copied().filter(|id| cold.contains(id)));
        }
        let results = fetch_with_ids_batch(db, session, client, &page_slices[start..end]).await;
        for result in results {
            match result {
                // OPT-006：补齐请求前复核代际。
                FeverPageFetch::Stale => return Err(SyncSession::stale_error()),
                FeverPageFetch::Failed(e) => {
                    fetch_failures += 1;
                    report.push_client_error(&e, format!("拉取未读/收藏条目失败: {e}"));
                    break 'with_ids;
                }
                FeverPageFetch::Page(page) => {
                    let unbound_feeds = unbound_page_feeds(&page, &maps);
                    if !unbound_feeds.is_empty() {
                        report.binding_incomplete = true;
                        report.errors.push(format!(
                            "Fever with_ids 补齐页含尚无本地绑定的远端 feed，未合并且游标不动: {:?}",
                            unbound_feeds
                        ));
                        return restore_after_binding_failure(
                            db,
                            session,
                            previous_entry_id,
                            history_state,
                            report,
                        )
                        .await;
                    }
                    // 页事务（无游标写入）：失败整页回滚，**不**推动任何游标。
                    // OPT-006：合并是回写边界——代际失配整页丢弃。
                    if let Err(e) = apply_fever_page(
                        db,
                        session.generation,
                        page,
                        &mut seen,
                        &mut maps,
                        report,
                        None,
                    )
                    .await?
                    {
                        merge_failures += 1;
                        record_pull_apply_failure(e, report, |e| {
                            format!("with_ids 补齐页合并失败（已回滚，游标不动）: {e}")
                        })?;
                        if pull_database_work(
                            db,
                            generation,
                            &mut maps,
                            report,
                            |conn, maps, report| rebuild_maps_after_failure(conn, maps, report),
                        )
                        .await?
                        .is_err()
                        {
                            return Ok(()); // R3-P1：重建失败立即终止本轮
                        }
                        break 'with_ids;
                    }
                }
            }
        }
    }

    if (fetch_failures, merge_failures) == failures_before_proof {
        if let Some(last_id) = cold.last() {
            let last_id = *last_id;
            let generation = session.generation;
            crate::state::database_work(db.clone(), move |conn| {
                let _writer = db::writer_guard(conn)?;
                SyncSession::ensure_generation(conn, generation)?;
                db::set_setting(
                    conn,
                    "fever_origin_proof_cursor",
                    &serde_json::json!({
                        "generation": generation, "last_id": last_id,
                    })
                    .to_string(),
                )
            })
            .await?;
        }
    }

    // Content collection snapshots above are only for missing-body discovery.
    // Existing bound flags belong exclusively to pull_states_fever.

    // ⑥ 完成时间。`last_sync_entry_id` 与历史 checkpoint 已在各页事务内提交
    // （页数据与游标同 commit），这里不再有「事务外的游标补写」。
    // 时间戳游标仅在「本轮窗口拿全」时推进——抓取失败、页事务失败、预算未完成
    // 与权威集合失败都算没拿全，否则切回 greader 时会跳过该窗口。
    // OPT-006：完成时间推进前复核代际——旧会话不得写新账号的时间戳游标。
    crate::state::database_work(db.clone(), move |conn| {
        SyncSession::ensure_generation(conn, generation)
    })
    .await?;
    let failures = fetch_failures + collection_failures + merge_failures;
    if failures == 0 && !report.states_incomplete && !report.binding_incomplete {
        report.content_cursor_candidate = Some(Utc::now().timestamp());
    } else {
        log::warn!(
            "fever pull: {fetch_failures} fetch(es) + {collection_failures} collection + {merge_failures} merge failure(s); keeping last_sync_ts"
        );
    }
    Ok(())
}

async fn load_fever_authority(
    client: &fever::FeverClient,
    previous: Option<&AuthoritySnapshot>,
    report: &mut SyncReport,
) -> Arc<AuthoritySnapshot> {
    if let Some(previous) =
        previous.filter(|snapshot| snapshot.unread.is_none() || snapshot.starred.is_none())
    {
        return Arc::new(previous.clone());
    }
    let (unread, starred) = tokio::join!(client.unread_item_ids(), client.saved_item_ids());
    let snapshot = match (unread, starred) {
        (Ok(unread), Ok(starred)) => AuthoritySnapshot {
            unread: Some(unread),
            starred: Some(starred),
        },
        (Err(error), _) | (_, Err(error)) => {
            report.push_client_error(
                &error,
                format!("状态对账跳过：Fever 状态集合拉取失败，本轮不合并远端状态: {error}"),
            );
            AuthoritySnapshot {
                unread: None,
                starred: None,
            }
        }
    };
    let snapshot = Arc::new(snapshot);
    report.authority_snapshot = Some(snapshot.clone());
    snapshot
}

/// A complete unread/saved pair is required. Failure is never an empty set.
/// This method performs no since/history/with_ids content pagination.
pub(super) async fn pull_states_fever(
    db: &Arc<Mutex<Connection>>,
    session: &SyncSession,
    client: &fever::FeverClient,
    report: &mut SyncReport,
    versions: Option<&db::SyncMatchMaps>,
) -> AppResult<()> {
    let generation = session.generation;
    if !pull_admitted(db, session, report).await? {
        return Ok(());
    }
    let mut maps = crate::state::database_work(db.clone(), move |conn| {
        SyncSession::ensure_generation(conn, generation)?;
        db::sync_match_maps(conn)
    })
    .await?;
    if let Some(versions) = versions {
        maps.keep_field_versions_from(versions);
        maps.pending_read_ids.extend(&versions.pending_read_ids);
        maps.pending_star_ids.extend(&versions.pending_star_ids);
    }
    if maps.mf_id_to_article.is_empty() {
        return Ok(());
    }
    let previous = report.authority_snapshot.clone();
    let authority = load_fever_authority(client, previous.as_deref(), report).await;
    if authority.unread.is_none() || authority.starred.is_none() {
        return Ok(());
    }
    let generation = session.generation;
    let authority = authority.clone();
    pull_database_work(
        db,
        generation,
        &mut maps,
        report,
        move |conn, maps, report| {
            if !pull_admitted_generation(conn, generation, report)? {
                return Ok(());
            }
            collect_unverified_state_ids(conn, maps, &authority, report)?;
            let (Some(unread), Some(starred)) = (&authority.unread, &authority.starred) else {
                return Ok(());
            };
            if let Err(error) =
                reconcile_fever_state_at(conn, generation, unread, starred, maps, report)
            {
                record_pull_apply_failure(error, report, |error| {
                    format!("Fever 状态对账写入失败，本轮剩余条目未对账: {error}")
                })?;
            }
            Ok(())
        },
    )
    .await?
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
///
/// OPT-007 字段版本守卫：与 GR 对账同构——pull 起点版本快照 + 当前 DB 版本 +
/// 该字段 pending，**按字段**独立判定（本地只改读时收藏照常收敛；反之亦然）。
#[cfg(test)]
fn reconcile_fever_state(
    conn: &Connection,
    unread: &[i64],
    starred: &[i64],
    maps: &db::SyncMatchMaps,
    report: &mut SyncReport,
) -> AppResult<()> {
    reconcile_fever_state_at(
        conn,
        db::sync_generation(conn)?,
        unread,
        starred,
        maps,
        report,
    )
}

fn reconcile_fever_state_at(
    conn: &Connection,
    generation: i64,
    unread: &[i64],
    starred: &[i64],
    maps: &db::SyncMatchMaps,
    report: &mut SyncReport,
) -> AppResult<()> {
    use std::collections::HashSet;
    let unread_set: HashSet<i64> = unread.iter().copied().collect();
    let starred_set: HashSet<i64> = starred.iter().copied().collect();

    for (remote_id, aid) in &maps.mf_id_to_article {
        let _writer = db::writer_guard(conn)?;
        SyncSession::ensure_generation(conn, generation)?;
        let aid = *aid;
        if !maps.state_binding_is_current(conn, aid, *remote_id)? {
            continue;
        }
        // OPT-007：字段版本快照（pull 起点；缺失按 (0,0) 保守处理）——
        // 任务卡负例①② 的 Fever 侧对称锁定。
        let (snap_read, snap_star) = maps.id_to_versions.get(&aid).copied().unwrap_or((0, 0));
        // TASK-112 政策（FEVER_READ_DIRECTION = UnreadSetBidirectional 双向权威）：
        // unread 集合权威——命中 → 本地未读（可复活未读）；未命中 → 本地已读。
        let remote_read = !unread_set.contains(remote_id);
        let read_rows = conflict_policy::apply_read_by_policy(
            conn,
            conflict_policy::FEVER_READ_DIRECTION,
            remote_read,
            aid,
            snap_read,
            maps.pending_read_ids.contains(&aid),
        )?;
        // Each successful field may already be committed. A later field failure
        // must not erase its refresh signal or actual changed-row count (A1).
        report.entries_data_changed |= read_rows > 0;
        report.merged_states += read_rows;
        // TASK-112 政策（FEVER_STAR_DIRECTION = AuthoritativeBidirectional 双向权威）：
        // 命中 → 收藏；未命中 → 取消收藏。
        let star_rows = conflict_policy::apply_star_by_policy(
            conn,
            conflict_policy::FEVER_STAR_DIRECTION,
            starred_set.contains(remote_id),
            aid,
            snap_star,
            maps.pending_star_ids.contains(&aid),
        )?;
        report.entries_data_changed |= star_rows > 0;
        report.merged_states += star_rows;
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
        // Policy fixture: a prior content response explicitly supplied feed/10.
        assert!(db::confirm_article_remote_origin(conn, aid, remote_id, 10).unwrap());
        aid
    }

    /// 单条映射的 SyncMatchMaps（reconcile 消费 mf_id_to_article / 字段 pending /
    /// 字段版本快照）。`pending=true` 表示**读/藏两字段都有**未推送意图（逐字段
    /// 判定的旧口径对照）；版本快照取 (0,0) = 与夹具行（raw SQL 写入、版本 0）
    /// 相等，即「拉取期间无本地写入」的默认起点。
    fn maps_for(conn: &Connection, remote_id: i64, aid: i64, pending: bool) -> db::SyncMatchMaps {
        db::SyncMatchMaps {
            source_versions: db::sync_match_maps(conn).unwrap().source_versions,
            id_to_mf_id: Default::default(),
            id_to_feed_mf: Default::default(),
            pending_ids: if pending {
                std::iter::once(aid).collect()
            } else {
                Default::default()
            },
            pending_read_ids: if pending {
                std::iter::once(aid).collect()
            } else {
                Default::default()
            },
            pending_star_ids: if pending {
                std::iter::once(aid).collect()
            } else {
                Default::default()
            },
            id_to_versions: std::iter::once((aid, (0, 0))).collect(),
            feed_mf_to_id: Default::default(),
            excluded_feed_ids: Default::default(),
            mf_id_to_article: std::iter::once((remote_id, aid)).collect(),
            verified_remote_ids: db::sync_match_maps(conn).unwrap().verified_remote_ids,
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
        reconcile_fever_state(
            &conn,
            &[201],
            &[],
            &maps_for(&conn, 201, aid, false),
            &mut report,
        )
        .unwrap();

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
        reconcile_fever_state(
            &conn,
            &[],
            &[],
            &maps_for(&conn, 202, aid, false),
            &mut report,
        )
        .unwrap();

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
            ..maps_for(&conn, 203, starred, false)
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
        reconcile_fever_state(
            &conn,
            &[205],
            &[],
            &maps_for(&conn, 205, aid, true),
            &mut report,
        )
        .unwrap();

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

    /// OPT-007 字段版本守卫（Fever 对账侧，任务卡负例①②的对称锁定）：
    /// 拉取期间本地标读（read_version 0→1、队列已清无 pending）→ 远端 unread
    /// 集合不得把已读翻回；同文章收藏字段版本未变 → saved 命中照常收藏
    /// （字段独立，不被读字段的守卫牵连）。判别力：aid 级 pending 集合的旧形态
    /// 在版本/队列双空时无法保护（已读被翻回）；字段版本守卫被移除时同样红。
    #[test]
    fn fever_reconcile_field_version_guard_protects_local_write_and_keeps_other_field() {
        let conn = conn();
        let aid = seed_bound(&conn, 206);
        db::set_read(&conn, aid, true).unwrap(); // 本地标读：read_version 0→1
        let mut maps = maps_for(&conn, 206, aid, false);
        maps.id_to_versions.insert(aid, (0, 0)); // pull 起点快照早于该本地写入

        let mut report = SyncReport::default();
        // 远端快照：unread 命中（会翻回未读）+ saved 命中（应收藏）。
        reconcile_fever_state(&conn, &[206], &[206], &maps, &mut report).unwrap();

        assert!(
            is_read(&conn, aid),
            "拉取期间本地标读（版本已变）：远端 unread 集合不得翻回未读"
        );
        assert!(
            is_starred(&conn, aid),
            "收藏字段无本地写入：saved 命中照常收藏（字段独立）"
        );
        assert_eq!(
            report.merged_states, 1,
            "仅收藏一次写入——读被字段版本守卫跳过，不牵连收藏"
        );
    }

    /// P2 返工：cursor + history 恢复必须是单事务；history 第二次写失败时，
    /// last_sync_entry_id 的第一笔恢复也必须回滚，不能留下半恢复。
    #[test]
    fn binding_failure_restore_is_atomic_when_second_write_fails() {
        let mut conn = conn();
        db::set_last_sync_entry_id(&conn, 200).unwrap();
        db::set_fever_history_pending(&conn, 55).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER opt019_fail_fever_history_insert
             BEFORE INSERT ON settings
             WHEN NEW.key = 'fever_history_state'
             BEGIN SELECT RAISE(ABORT, 'injected history restore failure'); END;
             CREATE TRIGGER opt019_fail_fever_history_update
             BEFORE UPDATE ON settings
             WHEN NEW.key = 'fever_history_state'
             BEGIN SELECT RAISE(ABORT, 'injected history restore failure'); END;",
        )
        .unwrap();

        let err = restore_fever_pull_state(&mut conn, 100, db::FeverHistoryState::Pending(55))
            .expect_err("第二笔写入必须失败");
        assert!(err.to_string().contains("history restore failure"), "{err}");
        assert_eq!(
            db::last_sync_entry_id(&conn).unwrap(),
            200,
            "history 写失败时 cursor 恢复必须随事务回滚"
        );
        assert_eq!(
            db::fever_history_state(&conn).unwrap(),
            db::FeverHistoryState::Pending(55)
        );
    }

    /// B2 预过滤纯逻辑锁定：need = 权威集合 − seen − 已有正文。判别力：
    /// 去掉 has_body 补集过滤（回退旧行为）时「已有正文」断言红；去掉 seen
    /// 过滤时 seen 条目断言红。
    #[test]
    fn with_ids_need_excludes_seen_and_entries_with_local_body() {
        let seen: HashSet<i64> = [3].into_iter().collect();
        let has_body: HashSet<i64> = [1].into_iter().collect();
        let need = with_ids_need(&[1, 2], &[3, 4], &seen, &has_body);
        let mut sorted = need.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted,
            vec![2, 4],
            "已有正文的 1 不得出现在请求集合；seen 的 3 不得出现；无正文的 2/4 仍回补"
        );
        // 未读与收藏集合的并集去重由调用方 sort+dedup 承担。
        assert_eq!(need.len(), sorted.len(), "夹具无重复输入");
    }

    /// B2 预过滤 DB 判定锁定：`remote_ids_with_body` 只返回「绑定行正文
    /// （html 或纯文本）非空」的 id；无绑定行的 id 天然不在集合里（取补集
    /// 后留在 need，未拉过的远端条目不得被误跳过——修前 missing 查询只扫
    /// 已有行会把它漏掉）。
    #[test]
    fn remote_ids_with_body_and_need_composition_keeps_unbound_ids() {
        let conn = conn();
        let folder = db::create_folder(&conn, "分类", "article").unwrap();
        let feed = db::insert_feed(
            &conn,
            "https://f.example/body-prefilter",
            None,
            "F",
            None,
            folder,
            "inherit",
            false,
            false,
        )
        .unwrap();
        let with_body = NewArticle {
            guid: "g-body".into(),
            url: Some("https://e.example/with-body".into()),
            title: "t".into(),
            author: None,
            summary: None,
            content_html: Some("<p>正文</p>".into()),
            body_text: "正文".into(),
            image_url: None,
            enclosure_url: None,
            enclosure_mime: None,
            duration_sec: None,
            published_at: Some("2026-01-01T00:00:00+00:00".into()),
            source: "direct".into(),
        };
        let aid_body = db::upsert_article_with_feed(&conn, feed, &with_body, false)
            .unwrap()
            .0;
        db::set_article_remote_id(&conn, aid_body, 101).unwrap();
        let empty_body = NewArticle {
            guid: "g-empty".into(),
            url: Some("https://e.example/empty-body".into()),
            title: "t".into(),
            author: None,
            summary: None,
            content_html: None,
            body_text: String::new(),
            image_url: None,
            enclosure_url: None,
            enclosure_mime: None,
            duration_sec: None,
            published_at: Some("2026-01-01T00:00:00+00:00".into()),
            source: "direct".into(),
        };
        let aid_empty = db::upsert_article_with_feed(&conn, feed, &empty_body, false)
            .unwrap()
            .0;
        db::set_article_remote_id(&conn, aid_empty, 102).unwrap();

        // 101 已有正文；102 绑定行无正文；103 无任何绑定行（远端未拉过）。
        let mut with_body = db::remote_ids_with_body(&conn, &[101, 102, 103]).unwrap();
        with_body.sort_unstable();
        assert_eq!(
            with_body,
            vec![101],
            "只有绑定行正文非空的 101 进入 has_body 集合"
        );
        let has_body: HashSet<i64> = with_body.into_iter().collect();
        let mut need = with_ids_need(&[101, 102, 103], &[], &HashSet::new(), &has_body);
        need.sort_unstable();
        assert_eq!(
            need,
            vec![102, 103],
            "取补集后：无正文 102 与未绑定 103 仍回补，已有正文 101 被排除"
        );
    }
    #[test]
    fn authority_rejects_retired_and_reactivated_source_snapshots() {
        for reactivate in [false, true] {
            let conn = conn();
            let aid = seed_bound(&conn, 907);
            db::set_starred(&conn, aid, true).unwrap();
            let maps = db::sync_match_maps(&conn).unwrap();
            let (feed, folder): (i64, i64) = conn.query_row(
                "SELECT f.id,f.folder_id FROM feeds f JOIN articles a ON a.feed_id=f.id WHERE a.id=?1",
                [aid], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
            db::delete_feed(&conn, feed).unwrap();
            if reactivate {
                assert_eq!(
                    db::insert_feed(
                        &conn,
                        "https://f.example/907.rss",
                        None,
                        "F",
                        None,
                        folder,
                        "inherit",
                        false,
                        false
                    )
                    .unwrap(),
                    feed
                );
            }
            let mut rebuilt = db::sync_match_maps(&conn).unwrap();
            // Preserve the old entry map too, exercising both commit rejection and snapshot rebuild.
            rebuilt.mf_id_to_article = maps.mf_id_to_article.clone();
            rebuilt.keep_field_versions_from(&maps);
            let mut report = SyncReport::default();
            reconcile_fever_state(&conn, &[], &[], &rebuilt, &mut report).unwrap();
            assert!(is_starred(&conn, aid));
            assert!(!is_read(&conn, aid));
            assert_eq!(report.merged_states, 0);
        }
    }
}
