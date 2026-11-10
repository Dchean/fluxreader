//! sync 的 greader_pull 子模块（TASK-045 从 sync.rs 按既有章节拆分）。

use super::*;
use crate::db;
use crate::error::{AppError, AppResult};
use crate::greader::{self, GReaderClient};
use chrono::Utc;
use rusqlite::Connection;
use std::collections::{BTreeSet, HashSet};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;

/// TASK-097 可测性小口子：greader pull 的游标墙钟统一走 `pull_now`，测试可经
/// `set_greader_pull_clock_override` 注入固定值，避免用例与真实时钟赛跑
/// （unix 秒在用例中途翻转会把「修前必红 / 修后必绿」退化成概率性）。
/// 仅 greader pull 消费；Fever / push / scheduler 等仍直接用 `Utc::now()`，语义不变。
static PULL_CLOCK_OVERRIDE: AtomicI64 = AtomicI64::new(0);
const GREADER_MAX_ID_PAGES: usize = 200;

/// B2 权威集合回补的单请求块大小（与主列举正文分块一致，100 条/请求）。
const GR_BACKFILL_CHUNK_SIZE: usize = 100;

/// B2 权威集合回补的每轮每集合预算（2000 条 = 至多 20 次请求/集合/轮）。
/// 预算到达**不是失败**：集合 id 列表本身完整（fetch_stream_ids 自带截断
/// 守卫），本轮已回补的子集照常参与对账，剩余未绑定 id 下一轮 light 同步
/// 续补；只有单请求失败才按「该侧跳过 + 游标保留」fail-closed 处理。
const GR_BACKFILL_MAX_IDS_PER_SYNC: usize = 2000;

/// W3-2：独立请求通道的有界并发度（单批在途请求上限）。
/// 依据：服务端友好（个人规模 Miniflux/FreshRSS 实例）+ 客户端内存有界
/// （在途响应 ≤ 并发度 × 单请求块大小）。3 = 任务卡给定的 2–4 区间中值，
/// 把串行往返的 RTT 排队压缩约 1/3。消费范围：GR 主列举正文分块、GR 权威
/// 集合回补、Fever `with_ids` 回补（fever_pull.rs 经 super::* 取用）；
/// **分页列举不在其列**——continuation / since_id 游标链必须串行，
/// PULL_LOCK 覆盖整个 pull 的编排约束不变。
pub(super) const PULL_FETCH_CONCURRENCY: usize = 3;

/// W3-2：单块正文并发拉取的结果。代际失配与网络失败分开——前者不发请求、
/// 不计失败，直接 staleSession 终止；后者按既有 chunk_failures 语义计数。
enum ContentFetch {
    Entries(Vec<greader::ItemContent>),
    Failed(AppError),
    Stale,
}

/// W3-2：单块锁内合并后的循环控制（计数与错误上报都在 [`merge_content_chunk`]
/// 内按串行版原样完成，这里只表达「继续 / 代际失配终止 / 重建失败终止」）。
enum ChunkMerge {
    Continue,
    Stale,
    Abort,
}

/// 单块正文拉取：请求前复核代际（与串行版的 `session_is_current` 同语义）。
async fn fetch_content_chunk(
    db: &Arc<Mutex<Connection>>,
    session: &SyncSession,
    client: &GReaderClient,
    ids: &[i64],
) -> ContentFetch {
    if !session_is_current(db, session).await {
        return ContentFetch::Stale;
    }
    match client.item_contents(ids).await {
        Ok(v) => ContentFetch::Entries(v),
        Err(e) => ContentFetch::Failed(e),
    }
}

/// 批内并发拉取（`tokio::join!` 定元数展开——仓库无 futures 依赖下的最简
/// 有界并发形态）；返回按**原块顺序**排列的结果，调用方逐块顺序合并。
/// 批间天然构成同步屏障：上一批全部合并完才发起下一批，在途响应内存有界。
async fn fetch_chunk_batch(
    db: &Arc<Mutex<Connection>>,
    session: &SyncSession,
    client: &GReaderClient,
    batch: &[&[i64]],
) -> Vec<ContentFetch> {
    match batch.len() {
        0 => Vec::new(),
        1 => vec![fetch_content_chunk(db, session, client, batch[0]).await],
        2 => {
            let (a, b) = tokio::join!(
                fetch_content_chunk(db, session, client, batch[0]),
                fetch_content_chunk(db, session, client, batch[1]),
            );
            vec![a, b]
        }
        // 并发度由 PULL_FETCH_CONCURRENCY 常量决定，本 match 的臂必须随常量
        // **同步扩展**。长度超界时显式 panic（release 构建同样生效）——绝不
        // 静默丢块：丢块既无请求也无失败计数，游标守卫不会触发，窗口会被
        // 假成功吞掉（W3 审计 P3-1）。
        3 => {
            let (a, b, c) = tokio::join!(
                fetch_content_chunk(db, session, client, batch[0]),
                fetch_content_chunk(db, session, client, batch[1]),
                fetch_content_chunk(db, session, client, batch[2]),
            );
            vec![a, b, c]
        }
        n => panic!(
            "fetch_chunk_batch 批次大小 {n} 超出已实现的并发臂（PULL_FETCH_CONCURRENCY={PULL_FETCH_CONCURRENCY}）；调大并发常量必须同步扩展本 match 的臂"
        ),
    }
}

/// 单块正文的锁内合并（W3-2 抽取：串行版循环体逐字搬移——unbound 计数、
/// 代际复核、逐条合并、失败重建映射的语义与错误文案全部不变）。
async fn merge_content_chunk(
    db: &Arc<Mutex<Connection>>,
    session: &SyncSession,
    entries: &[greader::ItemContent],
    maps: &mut db::SyncMatchMaps,
    report: &mut SyncReport,
    chunk_failures: &mut usize,
    binding_failures: &mut usize,
) -> ChunkMerge {
    let mut unbound_feeds = BTreeSet::new();
    for e in entries {
        if let Some(feed_id) = item_feed_remote_id(e) {
            if !maps.feed_mf_to_id.contains_key(&feed_id)
                && !maps.excluded_feed_ids.contains(&feed_id)
            {
                unbound_feeds.insert(feed_id);
            }
        }
    }
    if !unbound_feeds.is_empty() {
        *binding_failures += 1;
        report.binding_incomplete = true;
        report.errors.push(format!(
            "条目所属远端 feed 尚无本地绑定，本轮保留游标并跳过这些条目: {:?}",
            unbound_feeds
        ));
    }
    // R2：entries 的合并链路 Result 化——DB 写失败不得吞。本块中途失败即计数
    // （时间戳游标不推进，下一轮重拉同一窗口），已处理条目留库（GR 无页事务，
    // 细粒度恢复仍归 OPT-007）。错误信息带条目 URL（TASK-056 可定位性契约）。
    let mut merge_error: Option<(String, AppError)> = None;
    {
        let conn = db.lock().await;
        // OPT-006：合并是回写边界——响应在途期间换号则整块丢弃。
        if session.ensure_current(&conn).is_err() {
            return ChunkMerge::Stale;
        }
        for e in entries {
            if let Err(err) = merge_pulled_entry(&conn, e, maps, report) {
                let url = e
                    .alternate
                    .first()
                    .map(|a| a.href.clone())
                    .unwrap_or_default();
                merge_error = Some((url, err));
                break;
            }
        }
    }
    if let Some((url, err)) = merge_error {
        *chunk_failures += 1;
        report
            .errors
            .push(format!("合并条目 {url} 失败（本块中止，游标保留）: {err}"));
        // OPT-007：失败块按 DB 现状**重建匹配映射**——合并是逐语句回写
        // 并就地改内存 maps，中途失败后不得让后续块继续用可能带半程突变
        // 的映射（否则失败窗口重拉时按被污染绑定「合法跳过」）。重建
        // 继续不了就终止本轮（游标已因 chunk_failures 保留，不假成功）。
        // 字段版本快照经 keep_field_versions_from **保持轮内原值**：重建
        // 不回刷版本，否则「pull 起点 ~ 重建点」的本地写入会被洗成「起点
        // 无变化」，后续块的旧远端状态即可回滚用户意图。
        let conn = db.lock().await;
        if session.ensure_current(&conn).is_err() {
            return ChunkMerge::Stale;
        }
        match db::sync_match_maps(&conn) {
            Ok(mut m) => {
                m.keep_field_versions_from(maps);
                *maps = m;
            }
            Err(e) => {
                report.errors.push(format!(
                    "合并失败后重建匹配映射失败（本轮终止，游标保留）: {e}"
                ));
                return ChunkMerge::Abort;
            }
        }
    }
    ChunkMerge::Continue
}

/// greader pull 的「现在」（unix 秒）：未注入时即系统墙钟。
fn pull_now() -> i64 {
    let injected = PULL_CLOCK_OVERRIDE.load(Ordering::Relaxed);
    if injected != 0 {
        injected
    } else {
        Utc::now().timestamp()
    }
}

fn item_feed_remote_id(e: &greader::ItemContent) -> Option<i64> {
    e.origin
        .as_ref()
        .and_then(|o| greader::parse_feed_numeric_id(&o.stream_id))
}

/// 注入 greader pull 的游标墙钟（`None` 恢复真实时钟）。
/// `#[doc(hidden)]`：仅供集成测试构造确定性时间线（TASK-097 成对测试），非公开 API。
#[doc(hidden)]
pub fn set_greader_pull_clock_override(ts: Option<i64>) {
    PULL_CLOCK_OVERRIDE.store(ts.unwrap_or(0), Ordering::Relaxed);
}

/// 拉远端条目（新条目 + 状态变化），按 remote_id/URL 匹配合并。
/// 分页三段式：每页「锁外拉取 → 锁内合并」，锁从不跨分页 HTTP await。
/// `full=true`（手动同步/首连）：先做绑定回填 + 全量状态对账。
/// `full=false`（后台自动同步）：普通正文按 ot 窗口增量拉取，另取完整
/// unread/starred 集合做状态对账及有界的缺身份/正文回补。
///
/// OPT-006：`session` 的代际在每个 HTTP→DB 边界复核（游标读、每页 id 请求、
/// 每块正文合并、状态对账、游标推进）；代际失配立即以 `staleSession` 终止本轮，
/// 旧响应不落库、后续请求不再发出。
pub(super) async fn pull_entries_greader(
    db: &Arc<Mutex<Connection>>,
    session: &SyncSession,
    client: &GReaderClient,
    report: &mut SyncReport,
    full: bool,
    versions: Option<&db::SyncMatchMaps>,
) -> AppResult<()> {
    let binding_incomplete = {
        let conn = db.lock().await;
        session.ensure_current(&conn)?;
        let account = super::current_account(&conn)?.ok_or_else(|| {
            AppError::new(
                "notConnected",
                "GReader pull 缺少当前同步账号身份，本轮不请求条目",
            )
        })?;
        db::binding_incomplete(&conn, &account.key)?
    };
    if binding_incomplete {
        report.errors.push(
            "持久绑定不完整标记存在：先完成 feeds 阶段绑定，本轮不请求条目/不推进游标".into(),
        );
        return Ok(());
    }

    let since_s = {
        let conn = db.lock().await;
        session.ensure_current(&conn)?;
        db::last_sync_ts(&conn)?
    };

    // OPT-007 字段版本快照：必须在**任何 HTTP 请求之前**抓取（任务卡「版号快照在
    // HTTP 前抓」）。快照是「pull 起点该字段无本地写入」的判据；若晚抓（如放在
    // id 列举之后），用户在 id 列举在途期间标读/收藏并即时推送成功（防抖 ~1s，
    // 队列已清）后，晚抓的快照会把**新版本**当成起点版本，后续正文/对账携带的
    // 旧远端状态即可回滚用户意图——只看当前 pending 的实现同样失守（队列已清）。
    //
    // 构建失败**不得退化成空映射继续**——空 feed 映射会让 merge 全部「合法跳过」，
    // 而 chunk_failures 保持 0、游标照常推进，整个窗口被静默跳过（与 Fever R3-P1
    // 同口径；错误可见 + 游标保留，下一轮重拉同一窗口）。
    let mut maps = {
        let conn = db.lock().await;
        session.ensure_current(&conn)?;
        match db::sync_match_maps(&conn) {
            Ok(m) => m,
            Err(e) => {
                report.errors.push(format!(
                    "同步匹配映射构建失败，本轮不合并条目（游标保留）: {e}"
                ));
                return Ok(());
            }
        }
    };

    if let Some(versions) = versions {
        maps.keep_field_versions_from(versions);
        maps.pending_read_ids.extend(&versions.pending_read_ids);
        maps.pending_star_ids.extend(&versions.pending_star_ids);
    }

    let unbound_remote = {
        let conn = db.lock().await;
        session.ensure_current(&conn)?;
        match db::unbound_remote_feeds(&conn) {
            Ok(v) => v,
            Err(e) => {
                report.errors.push(format!(
                    "读取未绑定远端源失败，本轮不拉取条目（游标保留）: {e}"
                ));
                return Ok(());
            }
        }
    };
    if !unbound_remote.is_empty() {
        report.binding_incomplete = true;
        report.errors.push(format!(
            "存在 {} 个 origin='remote' 源仍缺 remote_id，本轮不拉取条目（游标保留）: {}",
            unbound_remote.len(),
            unbound_remote
                .iter()
                .map(|(_, url)| url.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
        return Ok(());
    }

    // 没有任何远端 feed 绑定时，订阅层尚未提供条目可归属的本地源。此时拉取
    // 条目只会全部“合法跳过”，却仍可能推进时间游标；历史/权威集合请求也
    // 没有可对账对象。直接跳过本轮条目 pull，等 feeds 阶段建立绑定。
    // Note: 零远端 feed 绑定不得消费 pull 窗口 — 见 .agents/notes/implemented/bug-fix/2026-10-19-同步集合失败关闭.md
    if maps.feed_mf_to_id.is_empty() {
        log::debug!("greader pull: 尚无远端 feed 绑定，本轮跳过条目拉取");
        return Ok(());
    }

    // Note: published 是源发布时间，不能当服务端入库墙钟；晚到旧文由完整 unread/starred 集合回补 — 见 .agents/notes/implemented/bug-fix/2026-11-05-GR权威集合回补与过期意图恢复.md
    // ot 在固定 Miniflux 实现上是 published > ot，起点墙钟只缩小普通增量窗口。
    // 它不能保证抓到晚收录旧文或旧文章状态变化；完整权威集合负责状态与缺文发现。
    // 已读且未收藏的旧文章仍由 full 的 ot=0 列举覆盖，不把发布时间用作冲突时钟。
    let cursor_candidate = pull_now();

    // 拉取目标：reading-list 全部条目 id（分页），full 时 ot=0（全量），增量时 ot=since_s
    let ot = if full { Some(0i64) } else { Some(since_s) };
    let mut all_item_ids: Vec<i64> = Vec::new();
    let mut continuation: Option<u64> = None;
    let mut seen_continuations = HashSet::new();
    let mut id_pages = 0usize;
    // TASK-069 审查 F1：id 列举失败同样是「本轮没拿到该窗口」——不计入守卫的话，
    // all_item_ids 为空会让下方 chunks(100) 一次都不执行，chunk_failures 保持 0、
    // 游标照常推进，失败窗口被跳过（正是本守卫要关闭的漏文章形态）。故与分块失败同源计数。
    let mut id_failures = 0usize;
    let mut stale = false;
    loop {
        if id_pages >= GREADER_MAX_ID_PAGES {
            id_failures += 1;
            let err = AppError::new(
                "protocol",
                format!("GReader itemRefs 分页超过 {GREADER_MAX_ID_PAGES} 页上限"),
            );
            report.errors.push(format!("拉取条目 id 分页中止: {err}"));
            break;
        }
        // OPT-006：每页 id 请求前复核代际——账号已切换即停止后续请求。
        if !session_is_current(db, session).await {
            stale = true;
            break;
        }
        let r = match client
            .item_ids(
                "user/-/state/com.google/reading-list",
                ot,
                None,
                Some(10000),
                continuation,
            )
            .await
        {
            Ok(r) => r,
            Err(e) => {
                id_failures += 1;
                report.push_client_error(&e, format!("拉取条目 id 失败: {e}"));
                break;
            }
        };
        let mut got = 0;
        id_pages += 1;
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
                Ok(next) if got > 0 => {
                    let non_advancing = continuation.is_some_and(|previous| next <= previous);
                    let repeated = !seen_continuations.insert(next);
                    if non_advancing || repeated {
                        id_failures += 1;
                        let err = AppError::new(
                            "protocol",
                            format!(
                                "GReader continuation 重复或未前进：previous={continuation:?}, next={next}"
                            ),
                        );
                        report.errors.push(format!("拉取条目 id 分页中止: {err}"));
                        break;
                    }
                    continuation = Some(next);
                }
                Ok(_) => {
                    id_failures += 1;
                    let err = AppError::new(
                        "protocol",
                        format!("GReader continuation={c} 但本页无有效条目 id"),
                    );
                    report.errors.push(format!("拉取条目 id 分页中止: {err}"));
                    break;
                }
                Err(_) => {
                    id_failures += 1;
                    let err = AppError::new(
                        "protocol",
                        format!("GReader continuation 不是合法非负整数：{c:?}"),
                    );
                    report.errors.push(format!("拉取条目 id 分页中止: {err}"));
                    break;
                }
            },
        }
    }

    // 分批拉正文（每次 100 条，避免单请求过大），锁内合并。
    // TASK-068：分块失败计数——失败块的条目本轮丢失，若游标照常推进，下一轮
    // 增量从新游标起步，这些条目就只能等全量同步补回（「偶发漏文章」的温床）。
    // W3-2：正文请求**有界并发**（批内 ≤ PULL_FETCH_CONCURRENCY 个在途，批间
    // 按原块顺序逐块短事务合并）——失败计数/绑定计数/映射重建/游标守卫语义
    // 与串行版逐字一致，仅 HTTP 段并发化；合并仍串行（maps 就地突变依赖顺序）。
    let chunk_slices: Vec<&[i64]> = all_item_ids.chunks(100).collect();
    let mut chunk_failures = 0usize;
    let mut binding_failures = 0usize;
    for start in (0..chunk_slices.len()).step_by(PULL_FETCH_CONCURRENCY) {
        let end = (start + PULL_FETCH_CONCURRENCY).min(chunk_slices.len());
        let results = fetch_chunk_batch(db, session, client, &chunk_slices[start..end]).await;
        let mut batch_abort = false;
        for result in results {
            match result {
                // OPT-006：每块正文请求前复核代际（并发形态在拉取 future 内完成）。
                ContentFetch::Stale => {
                    stale = true;
                    batch_abort = true;
                }
                ContentFetch::Failed(e) => {
                    chunk_failures += 1;
                    report.push_client_error(&e, format!("拉取条目正文失败: {e}"));
                }
                ContentFetch::Entries(entries) => {
                    match merge_content_chunk(
                        db,
                        session,
                        &entries,
                        &mut maps,
                        report,
                        &mut chunk_failures,
                        &mut binding_failures,
                    )
                    .await
                    {
                        ChunkMerge::Continue => {}
                        ChunkMerge::Stale => {
                            stale = true;
                            batch_abort = true;
                        }
                        // 重建映射失败：本轮终止（游标已因 chunk_failures 保留）。
                        ChunkMerge::Abort => batch_abort = true,
                    }
                }
            }
            if batch_abort {
                break;
            }
        }
        if batch_abort {
            break;
        }
    }
    if stale {
        return Err(SyncSession::stale_error());
    }

    // 轻量同步（full=false）状态对账：ot 过滤发布时间，不能覆盖旧文章的状态
    // 变更或晚收录旧文；完整状态集合提供独立的对账/回补通道。read 侧不再全量拉
    // read 集合（ot×状态流增量已被服务端实现证伪，见 docs/sync-compat-matrix.md
    // §1），改拉 reading-list + xt=read 的当前未读全集，与本地未读绑定行差分出
    // 「远端新变已读」（xt= 三家服务端官方支持的等价替代）；starred 侧保持
    // 全量双向权威对账不变（取消收藏信号必须可信，集合缺失才可解释为「远端已
    // 取消收藏」——未读差分形态对 starred 不可行，详见未读差分 Note）。
    let mut authority_cursor_failures = 0usize;
    if !full {
        // OPT-006：对账集合请求前复核代际。
        if !session_is_current(db, session).await {
            return Err(SyncSession::stale_error());
        }
        // 拉取失败 ≠ 空集合（C-1）：任一权威集合拉取失败即跳过本轮对账，
        // 避免把网络/服务端错误当成"远端什么都没有"，静默清空本地收藏
        let (unread_result, starred_result) = tokio::join!(
            fetch_stream_ids(
                client,
                greader::tags::READING_LIST,
                Some(greader::tags::READ),
            ),
            fetch_stream_ids(client, greader::tags::STARRED, None),
        );
        // 完整性守卫（结构性）：fetch_stream_ids 的 P2-1 纪律保证 Ok(ids) ⟺
        // continuation 耗尽——页上限命中 / 请求失败 / 损坏 continuation 一律
        // Err，本轮跳过 read 差分（fail-closed），不落地任何差分结果；游标
        // 语义不变（是否推进仍由下方 failures 汇总决定）。
        let curr_unread: Option<Vec<i64>> = match unread_result {
            Ok(ids) => Some(ids),
            Err(e) => {
                authority_cursor_failures += 1;
                report.push_client_error(
                    &e,
                    format!("unread 权威集合拉取失败，本轮跳过 read 差分: {e}"),
                );
                None
            }
        };
        let starred_ids = match starred_result {
            Ok(ids) => Some(ids),
            Err(e) => {
                authority_cursor_failures += 1;
                report.push_client_error(
                    &e,
                    format!("starred 权威集合拉取失败，本字段跳过对账: {e}"),
                );
                None
            }
        };
        // Note: 两个完整集合都补缺身份/正文，保持 pending/版本守卫 — 见 .agents/notes/implemented/bug-fix/2026-11-05-GR权威集合回补与过期意图恢复.md
        if let Some(ids) = curr_unread.as_ref() {
            if !backfill_authority_collection(db, session, client, "unread", ids, &mut maps, report)
                .await?
            {
                authority_cursor_failures += 1;
            }
        }
        let starred_for_reconcile = starred_ids;
        if let Some(ids) = starred_for_reconcile.as_ref() {
            match backfill_authority_collection(
                db, session, client, "starred", ids, &mut maps, report,
            )
            .await
            {
                Ok(true) => {}
                Ok(false) => {
                    authority_cursor_failures += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }

    // 更新游标（unix 秒）。TASK-068/069：仅在本轮「窗口确实拿全」时推进——
    // id 列举失败/分页中断（id_failures）与分块失败（chunk_failures）都算没拿全，
    // 下一轮重拉同一窗口补回（合并幂等，不会产生重复条目）。
    // TASK-097：推进值写「id 列举开始前取的起点候选」（论证见函数头部），
    // 不再取结束墙钟；failures > 0 时候选被丢弃、游标保持旧值，语义不变。
    // OPT-006：推进前在同一次持锁内复核代际——旧会话不得把游标推过账号边界。
    let failures = id_failures
        + chunk_failures
        + binding_failures
        + authority_cursor_failures
        + usize::from(report.binding_incomplete);
    if failures == 0 && !report.states_incomplete {
        let conn = db.lock().await;
        session.ensure_current(&conn)?;
        // 审计（2026-11-01）：写失败此前静默吞掉——窗口已拿全却没留下游标痕迹，
        // 用户与日志都无从区分「推进成功」与「写入失败」。改为 warn + report
        // 可见；游标不前进的语义不变（本轮没写上，下一轮窗口起点不变）。
        report.content_cursor_candidate = Some(cursor_candidate);
        drop(conn);
    } else {
        log::warn!(
            "greader pull: {id_failures} id-listing + {chunk_failures} chunk + {binding_failures} binding failure(s); keeping last_sync_ts（下一轮重拉同一窗口）"
        );
    }
    Ok(())
}

/// Authority-only state read. The state lane serializes HTTP snapshot -> apply;
/// content pages never reapply flags for existing bound identities.
pub(super) async fn pull_states_greader(
    db: &Arc<Mutex<Connection>>,
    session: &SyncSession,
    client: &GReaderClient,
    report: &mut SyncReport,
    versions: Option<&db::SyncMatchMaps>,
) -> AppResult<()> {
    let mut maps = {
        let conn = db.lock().await;
        session.ensure_current(&conn)?;
        db::sync_match_maps(&conn)?
    };
    if let Some(versions) = versions {
        maps.keep_field_versions_from(versions);
        maps.pending_read_ids.extend(&versions.pending_read_ids);
        maps.pending_star_ids.extend(&versions.pending_star_ids);
    }
    if maps.mf_id_to_article.is_empty() {
        return Ok(());
    }
    let (unread_result, starred_result) = tokio::join!(
        fetch_stream_ids(
            client,
            greader::tags::READING_LIST,
            Some(greader::tags::READ)
        ),
        fetch_stream_ids(client, greader::tags::STARRED, None),
    );
    let curr_unread = match unread_result {
        Ok(ids) => Some(ids),
        Err(error) => {
            report.push_client_error(
                &error,
                format!("unread 权威集合拉取失败，本字段跳过对账: {error}"),
            );
            None
        }
    };
    let starred_for_reconcile = match starred_result {
        Ok(ids) => Some(ids),
        Err(error) => {
            report.push_client_error(
                &error,
                format!("starred 权威集合拉取失败，本字段跳过对账: {error}"),
            );
            None
        }
    };
    // Complete collection only; missing identities are left to the content lane.
    if let Some(curr_unread_ids) = curr_unread.as_ref() {
        let unread_apply_result = {
            let conn = db.lock().await;
            session.ensure_current(&conn)?;
            (|| -> AppResult<()> {
                for remote_id in curr_unread_ids {
                    let Some(&aid) = maps.mf_id_to_article.get(remote_id) else {
                        continue;
                    };
                    if !maps.article_source_is_current(&conn, aid)? {
                        continue;
                    }
                    let version = maps.id_to_versions.get(&aid).copied().unwrap_or((0, 0)).0;
                    let rows = conflict_policy::apply_read_by_policy(
                        &conn,
                        conflict_policy::GR_READ_DIRECTION,
                        false,
                        aid,
                        version,
                        maps.pending_read_ids.contains(&aid),
                    )?;
                    report.entries_data_changed |= rows > 0;
                    report.merged_states += rows;
                }
                Ok(())
            })()
        };
        if let Err(e) = unread_apply_result {
            report.errors.push(format!(
                "unread 状态对账中断：本地写入失败（{e}），下一轮重放"
            ));
        }
        let candidates =
            collect_unread_diff_candidates(db, session, curr_unread_ids, &maps).await?;
        let confirmed = probe_unread_diff_candidates(client, &candidates, report).await;
        if !confirmed.is_empty() {
            let conn = db.lock().await;
            // OPT-006：差分落地是回写边界——代际失配丢弃整份确认集。
            session.ensure_current(&conn)?;
            if let Err(e) = apply_unread_diff_marks(&conn, &confirmed, &candidates, &maps, report) {
                report.errors.push(format!(
                    "read 差分落地中断：本地写入失败（{e}），下一轮重放"
                ));
            }
            drop(conn);
        }
    }
    if starred_for_reconcile.is_some() {
        let conn = db.lock().await;
        // OPT-006：对账是回写边界——代际失配丢弃整份成功集合。
        session.ensure_current(&conn)?;
        if let Some(ids) = starred_for_reconcile.as_deref() {
            if let Err(e) = reconcile_reader_state(&conn, None, Some(ids), &maps, report) {
                report.errors.push(format!(
                    "starred 状态对账中断：本地写入失败（{e}），下一轮重放"
                ));
            }
        }
        drop(conn);
    }
    Ok(())
}

/// 把权威集合中缺本地身份或正文的 id 逐块拉 contents 并合并。
/// 返回 `Ok(true)` = 该集合全部处理完成（含预算内子集）；`Ok(false)` =
/// 单请求/合并失败（错误已记入 report），调用方跳过该侧对账并保留游标；
/// `Err` 包括会话代际失配及正文预过滤/进度读写的数据库失败，整轮终止。
///
/// 只补集合内缺身份或正文的 id，
/// 绝不额外列举；合并走 [`merge_pulled_entry`] 既有四分支匹配链（远端 feed
/// 未绑定源的条目跳过并保留游标；其他已知源仍继续处理）。
/// 成功后保存本代际遍历位置；失败保留遍历位置及主列举游标。
async fn backfill_authority_collection(
    db: &Arc<Mutex<Connection>>,
    session: &SyncSession,
    client: &GReaderClient,
    collection: &'static str,
    ids: &[i64],
    maps: &mut db::SyncMatchMaps,
    report: &mut SyncReport,
) -> AppResult<bool> {
    let progress_key = format!("greader_backfill_{collection}");
    let (mut needed_ids, previous_id) = {
        let conn = db.lock().await;
        session.ensure_current(&conn)?;
        let with_body: HashSet<i64> = db::remote_ids_with_body(&conn, ids)?.into_iter().collect();
        let needed: BTreeSet<i64> = ids
            .iter()
            .copied()
            .filter(|id| !with_body.contains(id))
            .collect();
        let previous = db::get_setting(&conn, &progress_key)?
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .filter(|value| value["generation"].as_i64() == Some(session.generation))
            .and_then(|value| value["last_id"].as_i64());
        (needed.into_iter().collect::<Vec<_>>(), previous)
    };
    if needed_ids.is_empty() {
        return Ok(true);
    }
    if let Some(previous_id) = previous_id {
        let start = needed_ids.partition_point(|id| *id <= previous_id);
        needed_ids.rotate_left(start);
    }
    let budget = needed_ids.len().min(GR_BACKFILL_MAX_IDS_PER_SYNC);
    // W3-2：回补请求同样有界并发（批内 ≤ PULL_FETCH_CONCURRENCY、按原块顺序
    // 逐块合并）；首块失败即该侧软失败（fail-closed），同批在途请求无法撤回
    // 属既有「已发出的请求不能撤回」边界，后续批次不再发起。
    let chunk_slices: Vec<&[i64]> = needed_ids[..budget]
        .chunks(GR_BACKFILL_CHUNK_SIZE)
        .collect();
    for start in (0..chunk_slices.len()).step_by(PULL_FETCH_CONCURRENCY) {
        let end = (start + PULL_FETCH_CONCURRENCY).min(chunk_slices.len());
        let results = fetch_chunk_batch(db, session, client, &chunk_slices[start..end]).await;
        for result in results {
            let entries = match result {
                // OPT-006：每个回补请求前复核代际。
                ContentFetch::Stale => return Err(SyncSession::stale_error()),
                ContentFetch::Failed(e) => {
                    report.push_client_error(
                        &e,
                        format!(
                            "GR {collection} 集合回补请求失败，该侧本轮跳过对账（游标保留）: {e}"
                        ),
                    );
                    return Ok(false);
                }
                ContentFetch::Entries(mut entries) => {
                    for entry in &mut entries {
                        if entry.read_state() == greader::RemoteState::False {
                            entry.read_state_override = Some(greader::RemoteState::Missing);
                        }
                    }
                    entries
                }
            };
            // 未绑定的源只跳过其条目，继续处理同块中已绑定的尾部；保留游标。
            let mut unbound_feeds = BTreeSet::new();
            for e in &entries {
                if let Some(feed_id) = item_feed_remote_id(e) {
                    if !maps.feed_mf_to_id.contains_key(&feed_id)
                        && !maps.excluded_feed_ids.contains(&feed_id)
                    {
                        unbound_feeds.insert(feed_id);
                    }
                }
            }
            if !unbound_feeds.is_empty() {
                report.binding_incomplete = true;
                report.errors.push(format!(
                    "GR {collection} 回补条目所属远端 feed 尚无本地绑定，跳过这些条目并保留游标: {:?}",
                    unbound_feeds
                ));
            }
            let mut merge_error: Option<AppError> = None;
            {
                let conn = db.lock().await;
                // OPT-006：合并是回写边界——响应在途期间换号则本块丢弃。
                if session.ensure_current(&conn).is_err() {
                    return Err(SyncSession::stale_error());
                }
                for e in &entries {
                    if let Err(err) = merge_pulled_entry(&conn, e, maps, report) {
                        merge_error = Some(err);
                        break;
                    }
                }
            }
            if let Some(err) = merge_error {
                // GR 逐语句提交：maps 的每次突变都对应已提交的 DB 写；失败条目
                // 之后的条目未处理——本侧立即停止（不继续消耗预算），按软失败
                // 跳过该侧对账，游标保留，下一轮重拉同一集合续补。
                report.errors.push(format!(
                    "GR {collection} 集合回复合并失败，该侧本轮跳过对账（游标保留）: {err}"
                ));
                return Ok(false);
            }
        }
    }
    let conn = db.lock().await;
    session.ensure_current(&conn)?;
    db::set_setting(
        &conn,
        &progress_key,
        &serde_json::json!({
            "generation": session.generation,
            "last_id": needed_ids[budget - 1],
        })
        .to_string(),
    )?;
    Ok(true)
}

/// 分页拉取某 Google Reader stream 的全部条目 id（unread / starred 权威集合）。
/// `xt` 为 GR 排除 tag 参数：unread 集合 = reading-list + xt=read（当前未读全集）。
///
/// P2-1（自检 2026-09-29）：分页未走完（continuation 非数字 / 有 continuation
/// 但本页无可用 id）必须报错中止——与主列举循环的 TASK-069-F1 守卫同口径。
/// 此前这两种形态被静默 break 当「拿全了」，**截断**的 starred 权威集合进
/// `reconcile_reader_state` 后，排在截断点之后的已收藏条目被误判为「远端已
/// 取消收藏」（本地星标静默丢失，无队列记录、无报错）。报 Err 后由调用方的
/// 既有 C-1 守卫（「失败 ≠ 空集合」）跳过本轮对账，下一轮重拉完整集合。
async fn fetch_stream_ids(
    client: &GReaderClient,
    stream: &str,
    xt: Option<&str>,
) -> AppResult<Vec<i64>> {
    let mut ids: Vec<i64> = Vec::new();
    let mut continuation: Option<u64> = None;
    let mut seen_continuations = HashSet::new();
    let mut pages = 0usize;
    loop {
        if pages >= GREADER_MAX_ID_PAGES {
            return Err(AppError::new(
                "protocol",
                format!("拉取 {stream} 超过 {GREADER_MAX_ID_PAGES} 页上限（continuation 异常）"),
            ));
        }
        let r = client
            .item_ids_excluding(stream, Some(0), None, Some(10000), continuation, xt)
            .await?;
        pages += 1;
        let mut got = 0;
        for it in &r.item_refs {
            if let Ok(id) = it.id.parse::<i64>() {
                ids.push(id);
                got += 1;
            }
        }
        match r.continuation.as_deref() {
            None | Some("") => break,
            Some(c) => match c.parse::<u64>() {
                Ok(next) if got > 0 => {
                    let non_advancing = continuation.is_some_and(|previous| next <= previous);
                    let repeated = !seen_continuations.insert(next);
                    if non_advancing || repeated {
                        return Err(AppError::new(
                            "protocol",
                            format!(
                                "拉取 {stream} continuation 重复或未前进：previous={continuation:?}, next={next}"
                            ),
                        ));
                    }
                    continuation = Some(next);
                }
                Ok(_) => {
                    return Err(AppError::new(
                        "protocol",
                        format!("拉取 {stream} continuation={c} 但本页无有效条目 id"),
                    ));
                }
                Err(_) => {
                    return Err(AppError::new(
                        "protocol",
                        format!("拉取 {stream} continuation 不是合法非负整数：{c:?}"),
                    ));
                }
            },
        }
    }
    Ok(ids)
}

/// 未读差分候选收集：`prev_unread − curr_unread` 的 maps 域截面。差分域与旧
/// read 全量对账（[`reconcile_reader_state`]）一致取 `maps.mf_id_to_article`
/// （远端 feed 绑定行），叠加「本地未读」过滤——本地已读行不进域（旧路径对
/// 它们也写 0 行，幂等等价）；remote_id ∉ curr_unread 即「远端不再报未读」。
/// 返回 (remote_id, aid) 列表；会话代际失配或数据库读取失败返回 `Err`。
async fn collect_unread_diff_candidates(
    db: &Arc<Mutex<Connection>>,
    session: &SyncSession,
    curr_unread: &[i64],
    maps: &db::SyncMatchMaps,
) -> AppResult<Vec<(i64, i64)>> {
    let curr_unread_set: HashSet<i64> = curr_unread.iter().copied().collect();
    let conn = db.lock().await;
    session.ensure_current(&conn)?;
    let mut stmt =
        conn.prepare("SELECT remote_id FROM articles WHERE is_read = 0 AND remote_id IS NOT NULL")?;
    let unread_remote: HashSet<i64> = stmt
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<Result<HashSet<_>, _>>()?;
    drop(stmt);
    Ok(maps
        .mf_id_to_article
        .iter()
        .filter(|(remote_id, _)| {
            unread_remote.contains(remote_id) && !curr_unread_set.contains(remote_id)
        })
        .map(|(remote_id, aid)| (*remote_id, *aid))
        .collect())
}

/// 差分候选的 contents 存在性校验（批块优先，失败逐 id），返回确认仍被远端
/// 跟踪的 remote id（即 curr_unread 请求覆盖范围内的成员）。
///
/// 「远端已读」与「远端已删除」在未读集合中同样缺席，凭未读集合本身不可分；
/// 差分不能把缺席一律解释成已读（远端删除的未读条目会被误标）。以
/// stream/items/contents 必须覆盖请求 ID（重复/无关条目不能凑数）；块 Err 再
/// 逐 id 探测——`protocol`（缺条/删除形态）静默跳过该候选，`network`/HTTP 型
/// 失败计入聚合诊断并推进连续失败计数，连续 2 次即 fail-fast 本 chunk 剩余
/// 兜底（防最坏 100 次/块串行重试），剩余候选按「网络未确认」计入同一条
/// 聚合诊断（fail-closed：宁漏标一轮、下一轮重差分，绝不把删除误标成已读）。
/// 候选稳态为 0（两端一致时 id ∈ curr_unread），批块探测把代价摊到 ≤1 请求/
/// 百候选；确认集整体经调用方的回写边界代际复核后才落地。
async fn probe_unread_diff_candidates(
    client: &GReaderClient,
    candidates: &[(i64, i64)],
    report: &mut SyncReport,
) -> Vec<i64> {
    let mut confirmed: Vec<i64> = Vec::new();
    let mut network_unconfirmed: usize = 0;
    let mut all_unconfirmed_auth = true;
    for chunk in candidates.chunks(GR_BACKFILL_CHUNK_SIZE) {
        let ids: Vec<i64> = chunk.iter().map(|(remote_id, _)| *remote_id).collect();
        match client.item_contents(&ids).await {
            Ok(_) => confirmed.extend(ids),
            Err(e) => {
                report.note_client_error(&e);
                let mut network_streak = 0usize;
                for (idx, id) in ids.iter().copied().enumerate() {
                    if network_streak >= 2 {
                        // fail-fast：本 chunk 剩余候选不再逐个探测，按网络未确认计。
                        network_unconfirmed += ids.len() - idx;
                        break;
                    }
                    match client.item_contents(&[id]).await {
                        Ok(_) => confirmed.push(id),
                        Err(e) if e.code == "protocol" => {
                            log::debug!(
                                "greader pull: read 差分候选 {id} 存在性校验未确认（缺条/删除形态），本轮跳过: {e}"
                            );
                        }
                        Err(e) => {
                            report.note_client_error(&e);
                            all_unconfirmed_auth &= e.is_auth_failure();
                            network_unconfirmed += 1;
                            network_streak += 1;
                            log::debug!("greader pull: read 差分候选 {id} 存在性校验网络失败: {e}");
                        }
                    }
                }
            }
        }
    }
    if network_unconfirmed > 0 {
        report.push_classified_error(
            format!(
            "read 差分存在性校验 {network_unconfirmed} 条未确认（网络型，本轮不落地、下一轮重差分）"
        ),
            all_unconfirmed_auth,
        );
    }
    confirmed
}

/// 把「确认远端已读」的差分候选按政策单向落地。复用 conflict_policy 单点
/// （字段版本守卫 + pending 保护 + 值条件写幂等），与旧 read 全量对账的命中侧
/// 逐列一致；远端未读不回写（不复活未读）的政策语义由候选域本身保证
/// （候选必然 ∉ curr_unread）。
fn apply_unread_diff_marks(
    conn: &Connection,
    confirmed: &[i64],
    candidates: &[(i64, i64)],
    maps: &db::SyncMatchMaps,
    report: &mut SyncReport,
) -> AppResult<()> {
    let confirmed_set: HashSet<i64> = confirmed.iter().copied().collect();
    for (remote_id, aid) in candidates {
        if !maps.article_source_is_current(conn, *aid)? {
            continue;
        }
        if !confirmed_set.contains(remote_id) {
            continue;
        }
        // OPT-007：字段版本快照（pull 起点）+ 字段 pending 交给政策守卫；
        // 快照缺失（理论不可达）按 (0,0) 保守处理——与 reconcile 一致。
        let (snap_read, _) = maps.id_to_versions.get(aid).copied().unwrap_or((0, 0));
        let rows = conflict_policy::apply_read_by_policy(
            conn,
            conflict_policy::GR_READ_DIRECTION,
            true,
            *aid,
            snap_read,
            maps.pending_read_ids.contains(aid),
        )?;
        if rows > 0 {
            report.entries_data_changed = true;
        }
        report.merged_states += rows;
    }
    Ok(())
}

/// Google Reader 权威状态对账：
/// - read 集合含 remote_id → 远端已读 → 本地已读（read-wins，不反向复活未读；
///   生产 light 路径已改未读差分，read 侧保留为全量语义契约与政策锁定测试锚点）
/// - starred 集合含 remote_id → 本地收藏；不含 → 取消收藏（light 轮唯一生产消费面）
/// - 字段守卫（OPT-007 / 任务卡负例②）：pull 起点版本快照 + 当前 DB 版本 +
///   该字段 pending——按 **字段** 独立判定（本地只改过读时，收藏照常收敛；
///   aid 级 pending 并集会错误地把无冲突字段一起跳过）
///
/// TASK-112：方向语义收口到 `conflict_policy.rs` 政策单点——本函数按
/// `GR_READ_DIRECTION` / `GR_STAR_DIRECTION` 消费行级落地函数，不再自持
/// 方向分支；行为与显式化前逐列一致（收口不是改行为），政策理由与历史
/// 依据见政策点及 `docs/sync-compat-matrix.md`。
///
/// 审计 P2-8③：返回 `AppResult<()>`——行级写失败经 `?` 中断本轮对账并向上
/// 传播（调用方记入 `report.errors`），不再像旧的 `unwrap_or(0)` 那样把 DB
/// 写失败伪装成「0 行变化」。已写入的行不会回滚；剩余未对账的行由下一轮
/// 同集合对账重放（幂等自愈），但失败本身对用户可见。
fn reconcile_reader_state(
    conn: &Connection,
    read_ids: Option<&[i64]>,
    starred_ids: Option<&[i64]>,
    maps: &db::SyncMatchMaps,
    report: &mut SyncReport,
) -> AppResult<()> {
    use std::collections::HashSet;
    let read_set: Option<HashSet<i64>> = read_ids.map(|ids| ids.iter().copied().collect());
    let starred_set: Option<HashSet<i64>> = starred_ids.map(|ids| ids.iter().copied().collect());

    for (remote_id, aid) in &maps.mf_id_to_article {
        let aid = *aid;
        if !maps.article_source_is_current(conn, aid)? {
            continue;
        }
        // OPT-007：字段版本快照（pull 起点）+ 字段 pending 交给政策守卫；
        // 快照缺失（理论不可达）按 (0,0) 保守处理——与旧行版本一致。
        let (snap_read, snap_star) = maps.id_to_versions.get(&aid).copied().unwrap_or((0, 0));
        // TASK-112 政策（GR_READ_DIRECTION = RemoteReadWins 单向 read-wins）：
        // 远端已读 → 本地已读；远端未读不回写（不复活未读）。
        // R3：apply_* 返回的是**值条件写命中行数**（值实际改变）——据此置数据
        // 变化标记；merged_states 仍按旧口径累计（对外统计兼容）。
        let read_rows = match read_set.as_ref() {
            Some(read_set) => conflict_policy::apply_read_by_policy(
                conn,
                conflict_policy::GR_READ_DIRECTION,
                read_set.contains(remote_id),
                aid,
                snap_read,
                maps.pending_read_ids.contains(&aid),
            )?,
            None => 0,
        };
        // TASK-112 政策（GR_STAR_DIRECTION = AuthoritativeBidirectional 双向权威）：
        // 命中 → 收藏；未命中 → 取消收藏。
        let star_rows = match starred_set.as_ref() {
            Some(starred_set) => conflict_policy::apply_star_by_policy(
                conn,
                conflict_policy::GR_STAR_DIRECTION,
                starred_set.contains(remote_id),
                aid,
                snap_star,
                maps.pending_star_ids.contains(&aid),
            )?,
            None => 0,
        };
        if read_rows > 0 || star_rows > 0 {
            report.entries_data_changed = true;
        }
        report.merged_states += read_rows + star_rows;
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
    // TASK-112 政策锁定测试：GR 轻量对账的「操作 × 协议」格逐格锁死。
    // 判别力声明（「修前红」语义：对应政策被翻转/守卫被移除时必红）——
    // - 单向格：GR_READ_DIRECTION 被改成双向 → remote_unread 用例红；
    // - 双向格：GR_STAR_DIRECTION 被改成单向 → 两个 star 用例红；
    // - pending 保护：守卫被移除 → pending 用例红。
    // 本卡行为零变化，故全部用例在显式化前后都绿；CI（cargo test）承担执行。

    fn conn() -> rusqlite::Connection {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        conn
    }

    /// 造一篇已绑定远端 entry 的本地文章，返回新建文章的 id
    /// （远端 entry id 即调用方传入的 remote_id，不重复返回）。
    /// feeds.feed_url 有 UNIQUE 约束，故 feed/folder/guid 均按 remote_id 派生，
    /// 同一连接内可造多篇。
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

    /// 单条映射的 SyncMatchMaps（reconcile 消费 mf_id_to_article / 字段 pending /
    /// 字段版本快照）。`pending=true` 表示**读/藏两字段都有**未推送意图（逐字段
    /// 判定的旧口径对照）；版本快照取 (0,0) = 与夹具行（raw SQL 写入、版本 0）
    /// 相等，即「拉取期间无本地写入」的默认起点。
    fn maps_for(conn: &Connection, remote_id: i64, aid: i64, pending: bool) -> db::SyncMatchMaps {
        db::SyncMatchMaps {
            source_versions: db::sync_match_maps(conn).unwrap().source_versions,
            excluded_feed_ids: Default::default(),
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

    /// GR 单向格锁定：远端「未读」（read 集合未命中）不得复活本地已读。
    /// 政策被翻成双向时本用例红（sync_mark_unread_if_read 会把 is_read 打回 0）。
    #[test]
    fn gr_reconcile_remote_unread_restores_local_unread() {
        let conn = conn();
        let aid = seed_bound(&conn, 101);
        conn.execute("UPDATE articles SET is_read = 1 WHERE id = ?1", [aid])
            .unwrap();

        let mut report = SyncReport::default();
        // 审计 P2-8③：reconcile 返回 AppResult<()>——本用例锁政策方向，
        // 内存库上写必然成功，故 unwrap 即断言「无 DB 写失败」。
        reconcile_reader_state(
            &conn,
            Some(&[]),
            Some(&[]),
            &maps_for(&conn, 101, aid, false),
            &mut report,
        )
        .unwrap();

        assert!(!is_read(&conn, aid), "GR 双向：远端未读必须返回本地");
        assert_eq!(report.merged_states, 1, "双向：远端未读产生一次状态写入");
    }

    /// GR 单向格命中侧：远端已读 → 本地已读（read-wins 收敛路径）。
    #[test]
    fn gr_reconcile_remote_read_marks_local_read() {
        let conn = conn();
        let aid = seed_bound(&conn, 102);

        let mut report = SyncReport::default();
        reconcile_reader_state(
            &conn,
            Some(&[102]),
            Some(&[]),
            &maps_for(&conn, 102, aid, false),
            &mut report,
        )
        .unwrap();

        assert!(is_read(&conn, aid), "远端已读必须落地本地已读");
        assert_eq!(report.merged_states, 1, "命中侧恰好一次状态写入");
    }

    /// GR 星标格双向锁定：同一轮里两个条目分别验证「未命中 → 取消收藏」
    /// 与「命中 → 收藏」。政策被改成单向时两个断言其一必红。
    #[test]
    fn gr_reconcile_starred_is_bidirectional() {
        let conn = conn();
        let starred = seed_bound(&conn, 103);
        let unstarred = seed_bound(&conn, 104);
        conn.execute(
            "UPDATE articles SET is_starred = 1 WHERE id = ?1",
            [starred],
        )
        .unwrap();

        let mut report = SyncReport::default();
        // starred_ids 只含 104：103 未命中（远端已取消收藏），104 命中。
        let maps = db::SyncMatchMaps {
            mf_id_to_article: [(103, starred), (104, unstarred)].into_iter().collect(),
            ..maps_for(&conn, 103, starred, false)
        };
        reconcile_reader_state(&conn, Some(&[]), Some(&[104]), &maps, &mut report).unwrap();

        assert!(
            !is_starred(&conn, starred),
            "GR 星标双向：远端未命中必须取消本地收藏"
        );
        assert!(
            is_starred(&conn, unstarred),
            "GR 星标双向：远端命中必须收藏本地"
        );
        assert_eq!(report.merged_states, 2, "双向各一次写入");
    }

    /// 共享守卫锁定：pending（已入队未推送）条目整行跳过——远端快照既不能
    /// 标读它、也不能取消它的收藏。守卫被移除时本用例红（命中侧会写 is_read、
    /// 未命中侧会清 is_starred）。
    #[test]
    fn gr_reconcile_pending_guard_blocks_snapshot_rollback() {
        let conn = conn();
        let aid = seed_bound(&conn, 105);
        conn.execute("UPDATE articles SET is_starred = 1 WHERE id = ?1", [aid])
            .unwrap();

        let mut report = SyncReport::default();
        // 远端快照：已读命中 + 收藏未命中——若无 pending 保护会同时翻转两列。
        reconcile_reader_state(
            &conn,
            Some(&[105]),
            Some(&[]),
            &maps_for(&conn, 105, aid, true),
            &mut report,
        )
        .unwrap();

        assert!(
            !is_read(&conn, aid),
            "pending 保护：本地未读不得被远端已读快照覆盖"
        );
        assert!(
            is_starred(&conn, aid),
            "pending 保护：本地收藏不得被远端未收藏快照清除"
        );
        assert_eq!(report.merged_states, 0, "pending 命中行不产生任何写入");
    }

    /// OPT-007 字段版本守卫（对账入口，任务卡负例①②）：
    /// 拉取期间本地收藏（star_version 0→1，且队列已清/无 pending——只有版本
    /// 能保护）→ 远端「未命中」集合不得取消收藏；同一文章的读字段无本地写入
    /// （快照 == 当前版本）→ 远端已读照常收敛——字段彼此独立，不因收藏被保护
    /// 而放弃读的合并。判别力：只用 aid 级 pending 集合的实现（old 形态）在
    /// 队列已清时会把收藏取消（第三断言红）；字段版本守卫被移除时同样红。
    #[test]
    fn gr_reconcile_field_version_guard_protects_local_write_and_keeps_other_field() {
        let conn = conn();
        let aid = seed_bound(&conn, 106);
        db::set_starred(&conn, aid, true).unwrap(); // 本地收藏：star_version 0→1
        let mut maps = maps_for(&conn, 106, aid, false);
        maps.id_to_versions.insert(aid, (0, 0)); // pull 起点快照早于该本地写入

        let mut report = SyncReport::default();
        // 远端快照：starred 未命中（会取消收藏）+ read 命中（应照常标读）。
        reconcile_reader_state(&conn, Some(&[106]), Some(&[]), &maps, &mut report).unwrap();

        assert!(
            is_starred(&conn, aid),
            "拉取期间本地收藏（版本已变、队列已清）：远端未命中集合不得取消"
        );
        assert!(
            is_read(&conn, aid),
            "读字段无本地写入、版本快照相等：远端已读照常收敛（字段独立）"
        );
        assert_eq!(
            report.merged_states, 1,
            "仅读字段一次写入——收藏被字段版本守卫跳过，不牵连读"
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
            reconcile_reader_state(&conn, Some(&[907]), Some(&[]), &rebuilt, &mut report).unwrap();
            assert!(is_starred(&conn, aid));
            assert!(!is_read(&conn, aid));
            assert_eq!(report.merged_states, 0);
        }
    }
}
