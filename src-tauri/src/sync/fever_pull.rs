//! sync 的 fever_pull 子模块（TASK-045 从 sync.rs 按既有章节拆分）。

use super::*;
use crate::db;
use crate::error::AppResult;
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

/// Fever 拉取：items 端点单页仅给最近 50 条，拆两段：
/// ① `since_id` 分页增量拉新条目（含已读+未读 → 同源判定 + upsert）
/// ② `unread_item_ids`/`saved_item_ids` 权威集合全量对账（已读/收藏反推）。
///
/// TASK-125（审计 P2-8）：协议/当前 Miniflux 服务端支持 `items&max_id` 向
/// **更旧**条目翻页（历史回溯），但本客户端**未实现**该方向（见 `fever.rs`
/// 模块头「历史回溯能力」段）；此处两段是能力现状，不是协议上限。
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
    // TASK-125：since_id=0 走首种子（最近 50 条）是因本客户端未实现 max_id
    // 历史回溯，不是协议没有历史端点（见 `fever.rs` 模块头）。
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
        // 首次：本客户端未实现 max_id 历史回溯（协议/当前 Miniflux 支持，见
        // `fever.rs` 模块头）；最近 50 条作已读种子，未读/收藏由下方 with_ids 补齐
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
        // 审计 P2-8③：对账内的 DB 写失败必须可见——记入 report.errors
        // （不再被 unwrap_or(0) 伪装成 0 行变化）。已写入行不回滚，
        // 剩余行下一轮同集合对账幂等重放。
        if let Err(e) = reconcile_fever_state(&conn, &unread, &starred, &maps, report) {
            report.errors.push(format!(
                "状态对账中断：本地状态写入失败（{e}），本轮剩余条目未对账"
            ));
        }
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
    // 失败守卫（reconcile_ok：集合拉取失败跳过对账）的端到端锁定依赖 HTTP 层，
    // 由 live 测试（tests/fever_sync_live_e2e.rs，#[ignore]）与 CI 承担——
    // mock_greader 的 Fever 路由未实现 unread/saved/items 端点，无法在
    // src/ 范围内注入（tests/ 不在本卡允许修改范围）。

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
