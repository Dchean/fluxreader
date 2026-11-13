//! sync 的 entries 子模块（TASK-045 从 sync.rs 按既有章节拆分）。

use super::*;
use crate::db::{self, NewArticle};
use crate::error::{AppError, AppResult};
use crate::greader::{self, ItemContent};
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Shared admission for Content and State. A source import/tombstone failure is
/// not a trustworthy partial source map. Recheck this at state commit as well.
pub(super) async fn pull_admitted(
    database: &Arc<Mutex<Connection>>,
    session: &SyncSession,
    report: &mut SyncReport,
) -> AppResult<bool> {
    let generation = session.generation;
    let mut owned_report = std::mem::take(report);
    let (returned_report, result) = crate::state::database_work(database.clone(), move |conn| {
        let result = pull_admitted_generation(conn, generation, &mut owned_report);
        Ok((owned_report, result))
    })
    .await?;
    *report = returned_report;
    result
}

pub(super) fn pull_admitted_generation(
    conn: &Connection,
    generation: i64,
    report: &mut SyncReport,
) -> AppResult<bool> {
    SyncSession::ensure_generation(conn, generation)?;
    let Some(account) = super::current_account(conn)? else {
        return Err(crate::error::AppError::new(
            "notConnected",
            "缺少同步账号身份",
        ));
    };
    if db::binding_incomplete(conn, &account.key)? {
        report.binding_incomplete = true;
        report.pull_admission_blocked = true;
        report.errors.push(
            "持久绑定不完整标记存在：先完成 feeds 阶段绑定，本轮不请求条目或权威集合/不推进游标"
                .into(),
        );
        return Ok(false);
    }
    let unbound = db::unbound_remote_feeds(conn)?;
    if !unbound.is_empty() {
        report.binding_incomplete = true;
        report.pull_admission_blocked = true;
        report.errors.push(format!(
            "存在 {} 个 origin='remote' 源仍缺 remote_id，本轮不请求条目或权威集合（游标保留）",
            unbound.len(),
        ));
        return Ok(false);
    }
    db::has_remote_feed_binding(conn)
}

/// Move this round's unique merge state onto the DB worker without copying the
/// complete identity map for every page. Business failures restore partial
/// progress before the caller decides whether to rebuild or stop. Outer errors
/// (worker panic/cancellation or stale generation) MUST terminate the round:
/// the temporary Default map is never a valid replacement snapshot.
type PullWorkResult<T> = AppResult<AppResult<T>>;

/// Row-level pull failures remain soft unless the original session became
/// stale while waiting. A late stale must retain partial maps/report ownership
/// but still terminate the round; it cannot become a generic "replay later"
/// report error.
pub(super) fn record_pull_apply_failure(
    error: AppError,
    report: &mut SyncReport,
    message: impl FnOnce(&AppError) -> String,
) -> AppResult<()> {
    if error.code == "staleSession" {
        Err(error)
    } else {
        report.errors.push(message(&error));
        Ok(())
    }
}

pub(super) async fn pull_database_work<T: Send + 'static>(
    database: &std::sync::Arc<tokio::sync::Mutex<Connection>>,
    generation: i64,
    maps: &mut db::SyncMatchMaps,
    report: &mut SyncReport,
    work: impl FnOnce(&mut Connection, &mut db::SyncMatchMaps, &mut SyncReport) -> AppResult<T>
        + Send
        + 'static,
) -> PullWorkResult<T> {
    let mut owned_maps = std::mem::take(maps);
    let mut owned_report = std::mem::take(report);
    let (returned_maps, returned_report, result) =
        crate::state::database_work(database.clone(), move |conn| {
            let result = SyncSession::ensure_generation(conn, generation)
                .map(|()| work(conn, &mut owned_maps, &mut owned_report));
            Ok((owned_maps, owned_report, result))
        })
        .await?;
    *maps = returned_maps;
    *report = returned_report;
    // Admission can wait behind a session switch after the entry check. Return
    // the owned progress first, then promote that late stale to a round failure.
    match result? {
        Err(error) if error.code == "staleSession" => Err(error),
        business_result => Ok(business_result),
    }
}

#[cfg(test)]
mod blocking_pull_tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    struct FileDatabase(PathBuf);

    impl FileDatabase {
        fn new() -> Self {
            let root = std::env::var_os("FLUXREADER_TEST_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            let dir = root.join(format!(
                "fluxreader-pull-worker-{}-{}",
                std::process::id(),
                NEXT_DIR.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&dir).unwrap();
            Self(dir)
        }

        fn open(&self) -> Connection {
            let conn = Connection::open(self.0.join("main.db")).unwrap();
            conn.busy_timeout(Duration::ZERO).unwrap();
            conn.pragma_update(None, "journal_mode", "WAL").unwrap();
            conn.pragma_update(None, "foreign_keys", true).unwrap();
            conn
        }
    }

    impl Drop for FileDatabase {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).expect("all test connections must be closed");
        }
    }

    fn database() -> Arc<Mutex<Connection>> {
        let mut conn = Connection::open_in_memory().unwrap();
        db::MIGRATIONS.to_latest(&mut conn).unwrap();
        Arc::new(Mutex::new(conn))
    }

    #[tokio::test]
    async fn business_failure_returns_original_map_and_partial_report_before_handling_error() {
        let db = database();
        let mut maps = db::SyncMatchMaps::default();
        maps.id_to_mf_id.insert(17, Some(71));
        let mut report = SyncReport {
            pulled_entries: 4,
            ..Default::default()
        };
        let result = pull_database_work(&db, 0, &mut maps, &mut report, |conn, maps, report| {
            maps.id_to_mf_id.insert(18, Some(81));
            report.pulled_entries += 1;
            conn.execute("INSERT INTO unavailable_table VALUES(1)", [])?;
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(result.unwrap_err().code, "db");
        assert_eq!(maps.id_to_mf_id.get(&17), Some(&Some(71)));
        assert_eq!(maps.id_to_mf_id.get(&18), Some(&Some(81)));
        assert_eq!(report.pulled_entries, 5);
    }

    #[tokio::test]
    async fn stale_generation_does_not_run_work_and_keeps_captured_map() {
        let db = database();
        let mut maps = db::SyncMatchMaps::default();
        maps.source_versions
            .insert(7, ("original-source".into(), 3));
        let mut report = SyncReport::default();
        let result =
            pull_database_work(&db, 1, &mut maps, &mut report, |_, _, _| -> AppResult<()> {
                panic!("stale callback must not run");
            })
            .await;
        assert_eq!(result.unwrap_err().code, "staleSession");
        assert_eq!(
            maps.source_versions.get(&7),
            Some(&("original-source".into(), 3))
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stale_after_writer_wait_returns_partial_map_and_terminates_round() {
        let files = FileDatabase::new();
        let mut main = files.open();
        db::MIGRATIONS.to_latest(&mut main).unwrap();
        let concurrent = files.open();
        let generation = db::sync_generation(&main).unwrap();
        let database = Arc::new(Mutex::new(main));

        let (held_tx, held_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let writer = std::thread::spawn(move || {
            let tx = db::write_transaction(&concurrent).unwrap();
            assert_eq!(db::bump_sync_generation(&tx).unwrap(), generation + 1);
            held_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            tx.commit().unwrap();
        });
        held_rx.recv_timeout(Duration::from_secs(5)).unwrap();

        let (entered_tx, entered_rx) = mpsc::channel();
        let waiting = tokio::spawn({
            let database = database.clone();
            async move {
                let mut maps = db::SyncMatchMaps::default();
                let mut report = SyncReport::default();
                let result = pull_database_work(
                    &database,
                    generation,
                    &mut maps,
                    &mut report,
                    move |conn, maps, report| -> AppResult<()> {
                        maps.source_versions
                            .insert(7, ("partial-before-wait".into(), 1));
                        report.pulled_entries = 3;
                        entered_tx.send(()).unwrap();
                        let _writer = db::writer_guard(conn)?;
                        SyncSession::ensure_generation(conn, generation)?;
                        panic!("stale callback must not reach business work");
                    },
                )
                .await;
                (result, maps, report)
            }
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            tokio::task::spawn_blocking(move || entered_rx.recv_timeout(Duration::from_secs(2)))
                .await
                .unwrap()
                .unwrap()
        })
        .await
        .unwrap();
        release_tx.send(()).unwrap();
        writer.join().unwrap();

        let (result, maps, report) = tokio::time::timeout(Duration::from_secs(2), waiting)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.unwrap_err().code, "staleSession");
        assert_eq!(
            maps.source_versions.get(&7),
            Some(&("partial-before-wait".into(), 1))
        );
        assert_eq!(report.pulled_entries, 3);
    }

    #[test]
    fn record_pull_apply_failure_keeps_non_stale_row_errors_soft() {
        let mut report = SyncReport::default();
        record_pull_apply_failure(
            AppError::new("db", "injected row failure"),
            &mut report,
            |error| format!("row: {error}"),
        )
        .unwrap();
        assert_eq!(report.errors, vec!["row: [db] injected row failure"]);
    }

    #[tokio::test]
    async fn worker_panic_is_outer_error_and_cannot_be_a_soft_page_failure() {
        let db = database();
        let mut maps = db::SyncMatchMaps::default();
        let mut report = SyncReport::default();
        let result = pull_database_work::<()>(&db, 0, &mut maps, &mut report, |conn, _, _| {
            let tx = db::write_transaction(conn)?;
            tx.execute(
                "INSERT INTO settings(key,value) VALUES('panic_page','must rollback')",
                [],
            )?;
            panic!("injected blocking worker panic");
        })
        .await;
        // Call sites use .await? before their soft-page-error match. A panic
        // therefore leaves the pull immediately, never consuming Default maps.
        assert_eq!(result.unwrap_err().code, "internal");
        assert_eq!(
            db::get_setting(&*db.lock().await, "panic_page").unwrap(),
            None
        );
    }
}

/// Only cold bindings that a state read or pending local action actually needs
/// are sent to Content. The State lane never waits for their body/origin proof.
pub(super) fn collect_unverified_state_ids(
    conn: &Connection,
    maps: &db::SyncMatchMaps,
    authority: &AuthoritySnapshot,
    report: &mut SyncReport,
) -> AppResult<()> {
    let unread: Option<std::collections::HashSet<_>> = authority
        .unread
        .as_ref()
        .map(|ids| ids.iter().copied().collect());
    let starred: Option<std::collections::HashSet<_>> = authority
        .starred
        .as_ref()
        .map(|ids| ids.iter().copied().collect());
    for (remote_id, aid) in &maps.mf_id_to_article {
        if maps.verified_remote_ids.contains(remote_id)
            || !maps.article_source_is_current(conn, *aid)?
        {
            continue;
        }
        let (read, star): (bool, bool) = conn.query_row(
            "SELECT is_read,is_starred FROM articles WHERE id=?1",
            [aid],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if maps.pending_ids.contains(aid)
            || unread
                .as_ref()
                .is_some_and(|ids| read == ids.contains(remote_id))
            || starred
                .as_ref()
                .is_some_and(|ids| star != ids.contains(remote_id))
        {
            report.unverified_state_ids.insert(*remote_id);
        }
    }
    Ok(())
}

/// 从 ItemContent 提取远端 feed 数字 id（origin.stream_id = feed/42）。
fn item_feed_id(e: &ItemContent) -> Option<i64> {
    e.origin
        .as_ref()
        .and_then(|o| greader::parse_feed_numeric_id(&o.stream_id))
}

fn feed_ids_compatible(local: Option<i64>, observed: Option<i64>) -> bool {
    match (local, observed) {
        (Some(local), Some(observed)) => local == observed,
        _ => true,
    }
}

/// A known exact entry with compatible/unknown local feed already has a merge
/// destination. Do not classify it as a new source merely because feed_mf_to_id
/// lacks its origin: that redundant discovery can invalidate its fresh proof.
/// This only decides discovery; the commit path still rechecks the live source
/// token and current feed compatibility before accepting content or provenance.
pub(super) fn entry_unbound_feed(e: &ItemContent, maps: &db::SyncMatchMaps) -> Option<i64> {
    let remote_feed = item_feed_id(e)?;
    if maps.feed_mf_to_id.contains_key(&remote_feed)
        || maps.excluded_feed_ids.contains(&remote_feed)
    {
        return None;
    }
    let exact_local_feed = item_numeric_id(e)
        .and_then(|id| maps.mf_id_to_article.get(&id))
        .and_then(|aid| maps.id_to_feed_mf.get(aid));
    if exact_local_feed.is_some_and(|local| feed_ids_compatible(*local, Some(remote_feed))) {
        return None;
    }
    Some(remote_feed)
}

/// 从 ItemContent 提取条目十进制 id。解析规则在 `greader::parse_item_id`
/// 单点（协议形状：长格式 tag 尾部十六进制；无前缀纯数字按十进制——
/// Fever 长 id 与 greader 十进制 id 都不得按十六进制猜，OPT-005/F03）。
pub(super) fn item_numeric_id(e: &ItemContent) -> Option<i64> {
    greader::parse_item_id(&e.id)
}

/// 从 ItemContent 提取原文 URL（alternate[0].href）。
fn item_url(e: &ItemContent) -> Option<String> {
    e.alternate
        .first()
        .map(|a| a.href.clone())
        .filter(|url| !url.trim().is_empty())
}

/// 从 ItemContent 提取正文 HTML（content 优先，summary 兜底）。
fn item_content_html(e: &ItemContent) -> String {
    e.content
        .as_ref()
        .map(|c| c.content.clone())
        .or_else(|| e.summary.as_ref().map(|c| c.content.clone()))
        .unwrap_or_default()
}

/// 从 ItemContent 提取发布时间（unix 秒 → RFC3339）。
/// `None` 表示协议没有提供可用日期；由 DB 写 first_seen_at 兜底并标
/// `inferred_fetch`，不得在这里用当前时间冒充源发布时间。
fn item_published_at(e: &ItemContent) -> Option<String> {
    if e.published > 0 {
        DateTime::from_timestamp(e.published, 0).map(|d| d.with_timezone(&Utc).to_rfc3339())
    } else {
        None
    }
}

/// 精确远端 ID 优先；新身份只在已绑定且有效的源内入库。
/// URL 仅作为未绑定直连行的候选，正文、标题和附件相等才可首绑。
/// 未知源不猜归属；调用方记录绑定不完整并保留游标。DB 失败透传。
pub(super) fn merge_pulled_entry(
    conn: &Connection,
    e: &ItemContent,
    maps: &mut db::SyncMatchMaps,
    report: &mut SyncReport,
) -> AppResult<()> {
    let eid = item_numeric_id(e);
    let feed_remote = item_feed_id(e);
    if feed_remote.is_some_and(|id| maps.excluded_feed_ids.contains(&id)) {
        return Ok(());
    }
    // ① 精确协议 entry ID 优先。
    if let Some(eid) = eid {
        if let Some(&aid) = maps.mf_id_to_article.get(&eid) {
            if !maps.article_source_is_current(conn, aid)? {
                report.binding_incomplete = true;
                return Ok(());
            }
            let row_feed_mf: Option<i64> = conn.query_row(
                "SELECT f.remote_id FROM articles a JOIN feeds f ON f.id=a.feed_id WHERE a.id=?1",
                [aid],
                |row| row.get(0),
            )?;
            if feed_ids_compatible(row_feed_mf, feed_remote) {
                merge_own_entry(conn, aid, e, maps, report)?;
                return Ok(());
            }
            // 精确绑定与 feed 元数据冲突（历史脏数据）：**干净拒绝**——既不改写
            // 该行（不得跨源写状态），也不落 upsert（remote_id 有 UNIQUE 约束，
            // 重复绑定会撞约束把整页拖垮）。留待远端重拉/人工修复；不猜归属。
            log::warn!(
                "sync: entry {eid} 的绑定行 feed 元数据冲突（行 feed={row_feed_mf:?} entry feed={feed_remote:?}），跳过本轮合并"
            );
            return Ok(());
        }
    }

    // 已知 entry 的稀疏回包可走①；新身份没有归属则不能消费列举窗口。
    // Note: 精确身份与 URL 兜底边界 — 见 .agents/notes/implemented/architecture/2026-11-13-正文封面与媒体预算.md
    let Some(feed_remote) = feed_remote else {
        return Err(crate::error::AppError::new(
            "protocol",
            format!(
                "新条目 {} 缺少有效 origin feed，无法确认归属（保留游标待重试）",
                e.id
            ),
        ));
    };

    if maps.excluded_feed_ids.contains(&feed_remote) {
        return Ok(());
    }
    if let Some(&local_feed) = maps.feed_mf_to_id.get(&feed_remote) {
        // A local unsubscribe can occur while HTTP is in flight. Recheck at commit time.
        if maps.feed_source_is_current(conn, local_feed)? {
            upsert_remote_entry(conn, local_feed, e, maps, report)?;
        } else {
            // Missing/stale source evidence is a skipped content window, not an
            // empty success. Retry may admit a committed new-source receipt;
            // an old retired/reactivated source must wait for the next round.
            report.binding_incomplete = true;
        }
        return Ok(());
    }
    // GR continues processing known source entries after an unknown source, while
    // its caller records the incomplete binding and holds the cursor. Fever rejects
    // such a page before its transaction, so neither protocol invents a source.
    report.binding_incomplete = true;
    Ok(())
}

/// 已绑定身份只接收正文修订；flags由独立状态通道唯一写入。
fn merge_own_entry(
    conn: &Connection,
    aid: i64,
    e: &ItemContent,
    maps: &mut db::SyncMatchMaps,
    report: &mut SyncReport,
) -> AppResult<()> {
    // Note: 不允许慢正文回写已绑定flags — 见
    // .agents/notes/implemented/architecture/2026-11-05-同步运行生命周期与诊断.md
    report.pulled_entries += 1;
    if db::refresh_bound_remote_article(conn, aid, &remote_article_input(e))? {
        report.entries_data_changed = true;
    }
    confirm_content_origin(conn, aid, e, maps)?;
    Ok(())
}

fn confirm_content_origin(
    conn: &Connection,
    aid: i64,
    entry: &ItemContent,
    maps: &mut db::SyncMatchMaps,
) -> AppResult<()> {
    if let (Some(remote_id), Some(remote_feed_id)) = (item_numeric_id(entry), item_feed_id(entry)) {
        db::confirm_article_remote_origin(conn, aid, remote_id, remote_feed_id)?;
        if maps.state_binding_is_current(conn, aid, remote_id)? {
            maps.verified_remote_ids.insert(remote_id);
        }
    }
    Ok(())
}

/// 协议分派：Google Reader 用 `ot` 时间游标，Fever 用 `since_id` 条目游标。
/// OPT-006：返回值 `AppResult<()>`——会话代际失配（账号在拉取期间变更）以
/// `staleSession` 终止本轮，调用方据此返回可见错误而不是假成功。
pub(super) async fn pull_entries(
    db: &Arc<Mutex<Connection>>,
    session: &SyncSession,
    report: &mut SyncReport,
    full: bool,
    versions: Option<&db::SyncMatchMaps>,
) -> AppResult<()> {
    report.binding_incomplete = false;
    report.pull_admission_blocked = false;
    report.content_cursor_candidate = None;
    match &session.client {
        Backend::GReader(g) => pull_entries_greader(db, session, g, report, full, versions).await,
        Backend::Fever(f) => pull_entries_fever(db, session, f, report, full, versions).await,
    }
}

/// 后端条目入库（source='miniflux'，不覆盖直连正文）。
/// 调用约定（R2 修正，原「本地必然没有」论证只对**快照时刻**成立；
/// OPT-008B 起调用条件收紧为「精确 entry ID 未命中 且 同 feed 快照 URL 映射
/// 未命中」）：快照在任何 HTTP 之前抓取，因此「快照未命中」不等于「此刻 DB 里
/// 没有」——HTTP 在途期间直连抓取插入的同 feed 同 URL 行不在快照里，
/// upsert_article_with_feed 会按 (feed, url_norm) 或 guid 命中该既有行并返回
/// inserted=false。调用方必须区分新行/既有行：既有行的状态逐字段过版本/pending
/// 守卫，不得整值覆写（否则回滚快照之后用户的标读/收藏——主控 2026-10-10 反例）。
/// enclosure（播客音频/视频）与图片一并落库——播放器与卡片封面依赖。
fn upsert_remote_entry(
    conn: &Connection,
    feed_id: i64,
    e: &ItemContent,
    maps: &mut db::SyncMatchMaps,
    report: &mut SyncReport,
) -> AppResult<()> {
    let remote_read = e.read_state();
    let remote_starred = e.starred_state();
    let a = remote_article_input(e);
    // TASK-056 语义保留（失败必须可见），R2 升级形态：整条链路 Result 化——
    // 插入/绑定/状态任何一步失败都透传 Err（不再「记 report 后继续」，那样调用方
    // 无法整页回滚、也无法保住重拉游标）。R3 换 tracked 形态：既有行的
    // content_changed 由 UPDATE 前后值比对得出（同值 replay/无内容差异 = false），
    // 是数据变化标记的一部分，不能拿 changes() 冒充。
    let outcome = db::upsert_remote_article(conn, feed_id, &a, item_numeric_id(e))?;
    let aid = outcome.id;
    // OPT-007：版本基线只在**缺失时**补记 (0,0)——已有快照不得被覆盖为 0，
    // 也绝不能用 HTTP 之后现读的 DB 版本重新定义快照（那会把拉取期间的本地
    // 写入洗成「起点无变化」，旧远端状态即可回滚用户意图）。轮内才见到的 id
    // （新入库或快照后外部写入）按 (0,0) 保守处理：该行若已有本地写入
    // （版本 > 0）会被逐字段守卫拒绝，宁可跳过远端合并不冒回滚风险——
    // 与 db/sync_map.rs 的 keep_field_versions_from 同口径。
    maps.id_to_versions.entry(aid).or_insert((0, 0));
    // R3/R4：新插入 / 既有行可见字段真实变化（tracked 结果）——**立即**并入
    // 标记：这些变化已随本语句提交，其后绑定/状态任一步失败都不得让它们失去
    // 事件信号（GR 逐语句提交，无页事务可依赖）。
    if outcome.inserted || outcome.content_changed {
        report.entries_data_changed = true;
    }
    // Read the actual binding at commit time, not only the pre-HTTP maps. A
    // concurrent first bind may already have handed flags to the state lane.
    let was_bound = db::get_article_remote_id(conn, aid)?.is_some();
    if let Some(eid) = item_numeric_id(e) {
        // R4：绑定成功即置位（后续步骤失败不回退已提交的绑定变化）。
        if db::set_article_remote_id(conn, aid, eid)? {
            report.entries_data_changed = true;
        }
        maps.id_to_mf_id.insert(aid, Some(eid));
        maps.mf_id_to_article.insert(eid, aid);
    }
    confirm_content_origin(conn, aid, e, maps)?;
    if outcome.inserted {
        // 真正的新行：插入发生在同一 DB 临界区内，其间不可能有本地写入，快照
        // (0,0) 与行初值恒等，整值写状态不会覆盖任何用户意图。远端确认不产生
        // 本地版本（保持 0）。新行本身的标记已在上面置位。
        //
        // 新身份接收其自己远端状态，不从其他源或组成员借用状态。
        let effective_read = remote_read == greader::RemoteState::True;
        if effective_read {
            db::sync_set_read_at_version(conn, aid, true, 0)?;
        }
        if remote_starred == greader::RemoteState::True {
            db::sync_set_star_at_version(conn, aid, true, 0)?;
        }
        report.pulled_entries += 1;
    } else if !was_bound {
        // 首次绑定的既有行（典型：pull 起点快照之后直连入库、快照映射尚未包含的同 feed 同
        // URL 行）：状态逐字段过版本/pending 守卫，与权威状态读取同一套
        // 语义（read/star 彼此独立，无冲突字段照常收敛）——整值 sync_set_article_status
        // 在这里会回滚快照之后用户的标读/收藏（R2 修复的 P2 反例）。
        // R3/R4：状态写用值条件原语，命中行数当场进标记（守卫通过但值未变仍按
        // 旧口径计入 pulled_entries，保持统计兼容；写 Err 不置位——失败不是变化）。
        let (snap_read, snap_star) = maps.id_to_versions.get(&aid).copied().unwrap_or((0, 0));
        let apply_read = remote_read != greader::RemoteState::Missing;
        let apply_star = remote_starred != greader::RemoteState::Missing;
        let read_ok = apply_read
            && conflict_policy::field_accepts_remote(
                conn,
                aid,
                conflict_policy::SyncField::Read,
                snap_read,
                maps.pending_read_ids.contains(&aid),
            )?;
        let star_ok = apply_star
            && conflict_policy::field_accepts_remote(
                conn,
                aid,
                conflict_policy::SyncField::Star,
                snap_star,
                maps.pending_star_ids.contains(&aid),
            )?;
        if read_ok {
            if remote_read == greader::RemoteState::True {
                let rows = db::sync_mark_read_if_unread_at_version(conn, aid, snap_read)?;
                if rows > 0 {
                    report.entries_data_changed = true;
                }
            } else {
                let rows = db::sync_mark_unread_if_read_at_version(conn, aid, snap_read)?;
                if rows > 0 {
                    report.entries_data_changed = true;
                }
            }
        }
        if star_ok {
            let rows = if remote_starred == greader::RemoteState::True {
                db::sync_mark_starred_if_unstarred_at_version(conn, aid, snap_star)?
            } else {
                db::sync_mark_unstarred_if_starred_at_version(conn, aid, snap_star)?
            };
            if rows > 0 {
                report.entries_data_changed = true;
            }
        }
        if read_ok || star_ok {
            report.pulled_entries += 1;
        }
    }
    Ok(())
}

fn strip_html_text(html: &str) -> String {
    crate::sanitize::html_to_text(html)
}

/// One sanitized representation for initial import and stable remote revisions.
fn remote_article_input(e: &ItemContent) -> NewArticle {
    let published = item_published_at(e);
    let content_html = crate::sanitize::sanitize(&item_content_html(e), item_url(e).as_deref());

    // enclosure：Google Reader 的 enclosure 无 duration，只有 url + type
    let enclosure = e.enclosure.first();
    let (enc_url, enc_mime) = match enclosure {
        Some(enc) => (Some(enc.url.clone()), enc.r#type.clone()),
        None => (None, None),
    };

    NewArticle {
        guid: item_numeric_id(e)
            .map(|eid| db::remote_identity_guid(&eid.to_string()))
            .unwrap_or_else(|| db::remote_identity_guid(&e.id)),
        url: item_url(e),
        title: e.title.clone(),
        author: e.author.clone(),
        summary: None,
        content_html: Some(content_html.clone()),
        body_text: strip_html_text(&content_html),
        image_url: crate::sanitize::first_image(&content_html),
        enclosure_url: enc_url,
        enclosure_mime: enc_mime,
        duration_sec: None,
        published_at: published,
        source: "miniflux".into(),
    }
}

#[cfg(test)]
mod tri_state_tests {
    use super::*;

    fn conn_with_bound_article() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        db::MIGRATIONS.to_latest(&mut conn).unwrap();
        conn.execute_batch(
            "INSERT INTO feeds (feed_url, title, remote_id)
             VALUES ('https://tri.example/rss', 'tri', 10);
             INSERT INTO articles
                 (feed_id, guid, title, url, is_read, is_starred, remote_id)
             VALUES (1, 'tri-1', 'Tri', 'https://tri.example/1', 1, 1, 9001);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn discovery_uses_the_exact_identity_trust_matrix_without_reviving_retired_sources() {
        for (label, local_feed, entry_id, active, discovered, proven) in [
            ("exact unknown", None, 9001, true, None, true),
            ("exact compatible", Some(10), 9001, true, None, true),
            ("exact conflicting", Some(20), 9001, true, Some(10), false),
            ("new known source", Some(10), 9002, true, None, true),
            ("new unknown source", None, 9002, true, Some(10), false),
            ("retired exact", Some(10), 9001, false, None, false),
        ] {
            let conn = conn_with_bound_article();
            conn.execute(
                "UPDATE feeds SET remote_id=?1,active=?2 WHERE id=1",
                rusqlite::params![local_feed, active],
            )
            .unwrap();
            let mut maps = db::sync_match_maps(&conn).unwrap();
            let entry: ItemContent = serde_json::from_value(serde_json::json!({
                "id": entry_id.to_string(), "title": "observed content",
                "origin": {"streamId":"feed/10"},
                "summary": {"content":"<p>observed content</p>"},
            }))
            .unwrap();
            assert_eq!(entry_unbound_feed(&entry, &maps), discovered, "{label}");
            let mut report = SyncReport::default();
            merge_pulled_entry(&conn, &entry, &mut maps, &mut report).unwrap();
            let proof = db::article_by_remote_id(&conn, entry_id)
                .unwrap()
                .and_then(|aid| db::verified_remote_binding(&conn, aid).unwrap());
            assert_eq!(proof.is_some(), proven, "{label}");
            if label == "exact conflicting" || !active {
                assert_eq!(
                    conn.query_row("SELECT title FROM articles WHERE id=1", [], |r| r
                        .get::<_, String>(0))
                        .unwrap(),
                    "Tri",
                    "{label}"
                );
            }
        }
    }

    /// 判别力：删除 categories 后 `has_tag` 仍会返回 false；合并若继续使用它，
    /// 本用例的本地 read/star 会被回滚。三态路径必须把缺失当未知并跳过状态写。
    #[test]
    fn missing_categories_only_updates_content() {
        let conn = conn_with_bound_article();
        let remote: ItemContent = serde_json::from_str(
            r#"{
                "id":"tag:google.com,2005:reader/item/2329",
                "title":"remote",
                "summary":{"content":"<p>filled from remote</p>"}
            }"#,
        )
        .unwrap();
        assert_eq!(remote.read_state(), greader::RemoteState::Missing);
        assert_eq!(remote.starred_state(), greader::RemoteState::Missing);

        let mut maps = db::sync_match_maps(&conn).unwrap();
        let mut report = SyncReport::default();
        merge_pulled_entry(&conn, &remote, &mut maps, &mut report).unwrap();

        let (read, starred, body): (i64, i64, String) = conn
            .query_row(
                "SELECT a.is_read, a.is_starred, COALESCE(c.body_text, '')
                   FROM articles a
                   LEFT JOIN contents c ON c.id = a.content_id
                  WHERE a.id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((read, starred), (1, 1), "缺 categories 不得改写本地读/藏");
        assert!(
            body.contains("filled from remote"),
            "缺状态字段时内容仍应允许回填"
        );
    }

    /// Parsing still distinguishes false from Missing; neither value lets a
    /// slow content response take ownership of an already-bound entry's flags.
    #[test]
    fn explicit_empty_categories_are_false() {
        let conn = conn_with_bound_article();
        let remote: ItemContent = serde_json::from_str(
            r#"{
                "id":"tag:google.com,2005:reader/item/2329",
                "title":"remote",
                "categories":[]
            }"#,
        )
        .unwrap();
        assert_eq!(remote.read_state(), greader::RemoteState::False);
        assert_eq!(remote.starred_state(), greader::RemoteState::False);

        let mut maps = db::sync_match_maps(&conn).unwrap();
        let mut report = SyncReport::default();
        merge_pulled_entry(&conn, &remote, &mut maps, &mut report).unwrap();

        let (read, starred): (i64, i64) = conn
            .query_row(
                "SELECT is_read, is_starred FROM articles WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (read, starred),
            (1, 1),
            "显式false仍不得让正文通道覆盖已绑定状态"
        );
    }

    #[test]
    fn first_bind_initializes_remote_flags_but_keeps_field_pending() {
        for pending in [false, true] {
            let conn = conn_with_bound_article();
            conn.execute(
                "UPDATE articles SET guid=?1,remote_id=NULL WHERE id=1",
                [db::remote_identity_guid("9001")],
            )
            .unwrap();
            if pending {
                db::set_read_with_enqueue(&conn, 1, true).unwrap();
            }
            let remote: ItemContent = serde_json::from_str(
                r#"{
                "id":"9001","title":"remote","categories":[],
                "origin":{"streamId":"feed/10"}
            }"#,
            )
            .unwrap();
            let mut maps = db::sync_match_maps(&conn).unwrap();
            let mut report = SyncReport::default();
            merge_pulled_entry(&conn, &remote, &mut maps, &mut report).unwrap();
            let flags: (bool, bool) = conn
                .query_row(
                    "SELECT is_read,is_starred FROM articles WHERE id=1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(
                flags,
                (pending, false),
                "first binding preserves only the pending field"
            );
            assert_eq!(db::get_article_remote_id(&conn, 1).unwrap(), Some(9001));
        }
    }

    /// Fever 侧的同构保护：categories 存在（含 reading-list）但 is_read/is_saved
    /// 缺失时，解析器把两个 override 置为 Missing；合并同样不得改写本地状态。
    #[test]
    fn missing_fever_state_overrides_preserve_local_state() {
        let conn = conn_with_bound_article();
        let mut remote: ItemContent = serde_json::from_str(
            r#"{
                "id":"tag:google.com,2005:reader/item/2329",
                "title":"remote",
                "categories":["user/-/state/com.google/reading-list"]
            }"#,
        )
        .unwrap();
        remote.read_state_override = Some(greader::RemoteState::Missing);
        remote.starred_state_override = Some(greader::RemoteState::Missing);
        assert_eq!(remote.read_state(), greader::RemoteState::Missing);
        assert_eq!(remote.starred_state(), greader::RemoteState::Missing);

        let mut maps = db::sync_match_maps(&conn).unwrap();
        let mut report = SyncReport::default();
        merge_pulled_entry(&conn, &remote, &mut maps, &mut report).unwrap();

        let (read, starred): (i64, i64) = conn
            .query_row(
                "SELECT is_read, is_starred FROM articles WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (read, starred),
            (1, 1),
            "Fever 缺 is_read/is_saved 不得当 false 回写"
        );
    }
}
