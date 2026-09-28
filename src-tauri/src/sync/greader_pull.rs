//! sync 的 greader_pull 子模块（TASK-045 从 sync.rs 按既有章节拆分）。

use super::*;
use crate::db;
use crate::error::{AppError, AppResult};
use crate::greader::{self, GReaderClient};
use chrono::Utc;
use rusqlite::Connection;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;

/// TASK-097 可测性小口子：greader pull 的游标墙钟统一走 `pull_now`，测试可经
/// `set_greader_pull_clock_override` 注入固定值，避免用例与真实时钟赛跑
/// （unix 秒在用例中途翻转会把「修前必红 / 修后必绿」退化成概率性）。
/// 仅 greader pull 消费；Fever / push / scheduler 等仍直接用 `Utc::now()`，语义不变。
static PULL_CLOCK_OVERRIDE: AtomicI64 = AtomicI64::new(0);

/// greader pull 的「现在」（unix 秒）：未注入时即系统墙钟。
fn pull_now() -> i64 {
    let injected = PULL_CLOCK_OVERRIDE.load(Ordering::Relaxed);
    if injected != 0 {
        injected
    } else {
        Utc::now().timestamp()
    }
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

    // TASK-097 游标语义修正：本轮游标候选在 id 列举**开始前**取（拉取起点墙钟），
    // 拉取成功时写候选，而非修前的「拉取结束墙钟」。论证（起点游标 + 合并幂等 ⇒ 无漏无重）：
    // - 无漏：设起点候选为 C、id 列举时刻为 L（C ≤ L）。凡列举时刻已存在且
    //   changed_at ≥ 旧游标的条目已被本轮列举；凡**未被本轮列举**的服务端变更
    //   （列举之后才入库/变更）必有 changed_at ≥ L ≥ C，必然落进下一轮增量窗口
    //   （ot = C）。修前写结束墙钟 E 时，changed_at ∈ [L, E) 的变更被排除在下一轮
    //   之外——只能等全量对账补回（JOURNAL 09-24 登记的漏拉窗口，拉取越慢漏得越多）。
    // - 无重：本轮已合并的条目凡 changed_at ≥ C，下一轮会重复列举；合并幂等
    //   （upsert + 状态写同值 + 正文/封面只回填空位），不产生重复数据。
    // - 比较方向：服务端过滤为 changed_at >= ot（mock_greader.rs 的 ids 路由实现；
    //   真实 Miniflux main 分支把 ot 映射为 published_at > ot 严格大于，过滤列与
    //   方向随服务端版本/实现而异）。边界包含（>=）使「changed_at 恰等于起点游标」
    //   的条目会被重复拉——幂等合并下可接受；若是严格 >，「同一秒内、id 列举先于
    //   变更」的条目（changed_at == 起点）会被漏掉、重新打开漏拉窗口，故边界必须含入。
    // - 首次同步：last_sync_ts 为空取 0，ot=0 即全窗口列举，成功后写起点候选，
    //   与「游标 0 ⇒ 视为未同步」的既有语义兼容；全量（full=true，ot=0）同理写起点。
    let cursor_candidate = pull_now();

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
    // TASK-097：推进值写「id 列举开始前取的起点候选」（论证见函数头部），
    // 不再取结束墙钟；failures > 0 时候选被丢弃、游标保持旧值，语义不变。
    let failures = id_failures + chunk_failures;
    if failures == 0 {
        let conn = db.lock().await;
        let _ = db::set_last_sync_ts(&conn, cursor_candidate);
        drop(conn);
    } else {
        log::warn!(
            "greader pull: {id_failures} id-listing failure(s) + {chunk_failures} chunk(s) failed; keeping last_sync_ts（下一轮重拉同一窗口）"
        );
    }
}

/// 分页拉取某 Google Reader stream 的全部条目 id（read / starred 权威集合）。
///
/// P2-1（自检 2026-09-29）：分页未走完（continuation 非数字 / 有 continuation
/// 但本页无可用 id）必须报错中止——与主列举循环的 TASK-069-F1 守卫同口径。
/// 此前这两种形态被静默 break 当「拿全了」，**截断**的 starred 权威集合进
/// `reconcile_reader_state` 后，排在截断点之后的已收藏条目被误判为「远端已
/// 取消收藏」（本地星标静默丢失，无队列记录、无报错）。报 Err 后由调用方的
/// 既有 C-1 守卫（「失败 ≠ 空集合」）跳过本轮对账，下一轮重拉完整集合。
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
        match r.continuation.as_deref() {
            None | Some("") => break,
            Some(c) => match c.parse::<u64>() {
                Ok(next) if got > 0 => continuation = Some(next),
                Ok(_) => {
                    return Err(AppError::network(format!(
                        "拉取 {stream} 分页中断：continuation={c} 但本页无可用条目 id"
                    )));
                }
                Err(_) => {
                    return Err(AppError::network(format!(
                        "拉取 {stream} 分页中断：无法解析 continuation={c}"
                    )));
                }
            },
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
