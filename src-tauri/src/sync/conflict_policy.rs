//! TASK-112 协议对账冲突政策单点（single source of policy）。
//!
//! 双协议（Google Reader / Fever）在「状态对账」上的方向差异此前散落在
//! `greader_pull::reconcile_reader_state` / `fever_pull::reconcile_fever_state`
//! 的实现分支里，是「代码读得出来的隐性事实」。本模块把它提升为显式政策：
//! 每个「操作 × 协议」一格政策常量 + 选择理由 + 历史依据，对账函数只消费
//! 政策、不再自持方向分支。用户可见的对照表在 `docs/sync-compat-matrix.md`，
//! 两处必须同步维护（改政策 = 改这里 + 改矩阵 + 改对应锁定测试）。
//!
//! 政策总览（行为快照，与显式化前逐列一致——本模块是收口不是改行为）：
//!
//! | 操作 × 协议      | 政策                       | 方向                     |
//! |------------------|----------------------------|--------------------------|
//! | GR × 读状态      | `GR_READ_DIRECTION`        | 单向 read-wins           |
//! | GR × 星标        | `GR_STAR_DIRECTION`        | 双向权威                 |
//! | Fever × 读状态   | `FEVER_READ_DIRECTION`     | 双向权威（unread 集合）  |
//! | Fever × 星标     | `FEVER_STAR_DIRECTION`     | 双向权威                 |
//!
//! 行级落地由 [`apply_read_by_policy`] / [`apply_star_by_policy`] 承担，
//! 两条 reconcile 循环（GR/Fever）共用同一行级语义，差别只在传入的政策常量。
//!
//! # 共享守卫（政策的一部分；消费点在各 reconcile 与其调用方）
//!
//! - **pending 保护**：`sync_queue` 里存在未推送 read/unread/star/unstar 的
//!   条目跳过本轮对账（共享 pending 查询在 `db::sync_match_maps` 的
//!   `pending_ids`）。防「刚标读/刚收藏」被远端陈旧快照瞬间回滚（防乒乓）。
//!   两个 reconcile 循环开头的 `pending_ids.contains → continue` 即本政策。
//! - **「失败 ≠ 空集合」守卫**：权威集合拉取失败（网络/服务端错误、分页截断）
//!   时整轮对账跳过，绝不把失败当成「远端什么都没有」做双向回写——否则一次
//!   请求失败就会静默清空本地收藏/已读态。消费点：GR 调用方
//!   `greader_pull.rs` 的 C-1 段（`fetch_stream_ids` 报 Err → 跳过对账）；
//!   Fever 的 `reconcile_ok` 守卫（`fever_pull.rs`）。分页截断按失败处理
//!   （`fetch_stream_ids` 的 P2-1 纪律，锁定于
//!   `tests/star_reconcile_truncation_e2e.rs`）。
//!
//! # 同文副本读状态传播政策（DEC-refactor-roadmap-20261005 第 6 条）
//!
//! **保持现状：读状态向同文副本跨源传播（read 广播），显式记录为政策。**
//! 原决策文（同文副本）：「owner 不在场，主控按保守默认处置——保持现状
//! （读状态向同文副本跨源传播，push.rs 既有设计，注释载明双端场景动机），
//! 显式记录为政策并同步进用户可见文档；是否改为『布局隔离优先』由 owner
//! 后续决定，第二阶段 TASK（同文建模分离）落地策略开关时不预设结论。」
//!
//! 现状语义（消费点 `push.rs` 的 `plan_push`，其注释引用本政策点）：
//! - `read` 动作广播到绑定 entry + 全部同文副本 entry（双端场景：Read You
//!   等客户端不去重，桌面读完一篇，手机上另一源的副本也要已读，否则同一篇
//!   在另一源里又冒出来一条未读）；
//! - `unread` / `star` / `unstar` 只推绑定 entry 本身，不广播（未读/收藏的
//!   跨源语义未获同等待信，且 unread 广播会与 GR 单向 read-wins 政策打架）；
//! - full 合并路径的跨源副本按 read-anywhere-wins 记账 + 标读
//!   （`entries.rs` 的 `merge_pulled_entry`），unread 只认同源绑定 entry。

use rusqlite::Connection;

use crate::db;
use crate::error::AppResult;

/// 读状态对账方向（「协议 × 读状态」格的政策取值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReadDirection {
    /// 单向 read-wins：远端已读 → 本地已读；远端未读**不回写**（不复活未读）。
    ///
    /// 适用：Google Reader 轻量对账（`reconcile_reader_state`）。
    /// 选择理由：GR 协议下客户端能拿到的权威集合是「已读 id 集合」
    /// （`user/-/state/com.google/read` stream），不存在「明确保持未读」的
    /// 意图信号；本地未读是入库默认态，若把「不在已读集合」解释成
    /// 「远端刚取消已读」，历史遗留条目会被成批翻回未读（未读数抖动、重复
    /// 通知），而双向带来的唯一收益（远端显式标未读）本就由 push 段承担
    /// （本地标未读 → 入队 → 推 `unread`）。read-wins 单向已覆盖双端主诉求
    /// 「读到哪算读」（手机读 → 桌面跟随）。
    /// 历史依据：Miniflux→GR 协议切换（0ba940f）起轻量对账即此语义，
    /// TASK-054/068/069 系列守卫在其上加固；跨源副本的「未读不复活」在
    /// full 合并路径另有同源判定（`entries.rs` 的 `merge_remote_status`）。
    RemoteReadWins,
    /// 双向权威（unread 集合）：集合命中 → 本地未读（可复活）；未命中 → 本地已读。
    ///
    /// 适用：Fever 对账（`reconcile_fever_state`）。
    /// 选择理由：Fever 协议拿不到「已读 id 集合」，只有 `unread_item_ids`
    /// / `saved_item_ids` 两个权威集合——「未命中 = 已读」是唯一可用的读状态
    /// 信号，不双向就无法收敛任何一端的读状态变更。前提（原
    /// `fever_pull.rs:180-182` 注释）：Miniflux 按 URL 去重 entry，Fever 视角
    /// 无跨源副本，已绑定条目的集合状态可直接信任；FreshRSS 作为 Fever 实现
    /// 方同样只提供 unread 集合，语义同源。
    /// 误判代价由共享守卫兜底：pending 保护 + 「失败 ≠ 空集合」
    /// （集合没拿全时绝不双向回写）。
    UnreadSetBidirectional,
}

/// 星标对账方向（「协议 × 星标」格的政策取值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StarDirection {
    /// 双向权威（starred/saved 集合）：命中 → 本地收藏；未命中 → 取消收藏。
    ///
    /// 两协议一致（`GR_STAR_DIRECTION` = `FEVER_STAR_DIRECTION`）。
    /// 选择理由：收藏是低频强意图，「取消收藏」是显式动作——GR 的 starred
    /// stream 移除与 Fever 的 saved 集合缺失都是可信的取消信号；双向才能让
    /// 任意一端的取消收藏收敛到另一端。截断的权威集合（历史缺陷形态）由
    /// 「失败 ≠ 空集合」守卫拦截（`tests/star_reconcile_truncation_e2e.rs`）。
    AuthoritativeBidirectional,
}

/// 政策格：Google Reader × 读状态 = 单向 read-wins。
pub(super) const GR_READ_DIRECTION: ReadDirection = ReadDirection::RemoteReadWins;
/// 政策格：Google Reader × 星标 = 双向权威。
pub(super) const GR_STAR_DIRECTION: StarDirection = StarDirection::AuthoritativeBidirectional;
/// 政策格：Fever × 读状态 = 双向权威（unread 集合）。
pub(super) const FEVER_READ_DIRECTION: ReadDirection = ReadDirection::UnreadSetBidirectional;
/// 政策格：Fever × 星标 = 双向权威。
pub(super) const FEVER_STAR_DIRECTION: StarDirection = StarDirection::AuthoritativeBidirectional;

/// 按「协议 × 读状态」政策应用一行远端读状态。
///
/// 行级语义与显式化前的两条 reconcile 逐列一致：
/// - `RemoteReadWins`：`remote_read` 为真才 `sync_mark_read_if_unread`
///   （已读时写 0 行，天然幂等）；为假**不产生任何写**。
/// - `UnreadSetBidirectional`：`remote_read` 为真 `sync_mark_read_if_unread`，
///   为假 `sync_mark_unread_if_read`。
///
/// 返回 `AppResult<usize>`（实际写入行数）。审计 P2-8③：此处原为
/// `unwrap_or(0)`——DB 写失败被吞成「没有变化 0 行」，调用方与用户都无从
/// 区分「远端本来就是这个状态」与「本地写失败了」。现在失败经 `?` 向上传播，
/// 由 reconcile 循环的调用方记入 `SyncReport.errors` 并保留游标/重放语义
/// （失败现场仍可自愈：下一轮对账重放同一集合，但不再静默）。
pub(super) fn apply_read_by_policy(
    conn: &Connection,
    direction: ReadDirection,
    remote_read: bool,
    article_id: i64,
) -> AppResult<usize> {
    match direction {
        ReadDirection::RemoteReadWins => {
            if remote_read {
                db::sync_mark_read_if_unread(conn, article_id)
            } else {
                Ok(0)
            }
        }
        ReadDirection::UnreadSetBidirectional => {
            if remote_read {
                db::sync_mark_read_if_unread(conn, article_id)
            } else {
                db::sync_mark_unread_if_read(conn, article_id)
            }
        }
    }
}

/// 按「协议 × 星标」政策应用一行远端星标状态。
///
/// 行级语义与显式化前一致：命中 `sync_mark_starred_if_unstarred`，未命中
/// `sync_mark_unstarred_if_starred`（两协议同构，方向由参数声明）。
/// 返回 `AppResult<usize>`（实际写入行数）——失败不再按 0 计，向上传播
/// （审计 P2-8③，同 [`apply_read_by_policy`]）。
pub(super) fn apply_star_by_policy(
    conn: &Connection,
    direction: StarDirection,
    remote_starred: bool,
    article_id: i64,
) -> AppResult<usize> {
    match direction {
        StarDirection::AuthoritativeBidirectional => {
            if remote_starred {
                db::sync_mark_starred_if_unstarred(conn, article_id)
            } else {
                db::sync_mark_unstarred_if_starred(conn, article_id)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 政策格取值锁定：方向常量若被翻转（例如把 GR 改双向、Fever 改单向），
    /// 本测试先行红——它是政策点的直接锚，比行级测试更早暴露「政策被改」。
    #[test]
    fn policy_cells_have_locked_directions() {
        assert_eq!(GR_READ_DIRECTION, ReadDirection::RemoteReadWins);
        assert_eq!(FEVER_READ_DIRECTION, ReadDirection::UnreadSetBidirectional);
        assert_eq!(GR_STAR_DIRECTION, StarDirection::AuthoritativeBidirectional);
        assert_eq!(
            FEVER_STAR_DIRECTION,
            StarDirection::AuthoritativeBidirectional
        );
    }

    /// 建一个带最新 schema 的内存库并造一篇已绑定远端 id 的文章。
    fn conn_with_article() -> (Connection, i64) {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::MIGRATIONS.to_latest(&mut conn).unwrap();
        let folder = db::create_folder(&conn, "测试分类", "article").unwrap();
        let feed = db::insert_feed(
            &conn,
            "https://f.example/cp.rss",
            None,
            "F",
            None,
            folder,
            "inherit",
            false,
            false,
        )
        .unwrap();
        let a = crate::db::NewArticle {
            guid: "g-cp".into(),
            url: Some("https://e.example/p/cp".into()),
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
        let aid = db::upsert_article_with_feed(&conn, feed, &a, false)
            .unwrap()
            .0;
        (conn, aid)
    }

    /// 审计 P2-8③ 正向：写成功返回实际写入行数（此处条件写命中 1 行）。
    /// 同时锁定签名已变为 `AppResult<usize>`（`?` / `unwrap` 才能取出计数）。
    #[test]
    fn apply_read_by_policy_returns_written_rows_on_success() {
        let (conn, aid) = conn_with_article();
        let n = apply_read_by_policy(&conn, GR_READ_DIRECTION, true, aid).unwrap();
        assert_eq!(n, 1, "未读 → 已读 命中一行");
        // 幂等：再次应用同一远端状态写 0 行（不是失败）。
        let n = apply_read_by_policy(&conn, GR_READ_DIRECTION, true, aid).unwrap();
        assert_eq!(n, 0, "条件写未命中即 0 行变化，而非错误");
        // 单向格未命中侧不产生写。
        let n = apply_read_by_policy(&conn, GR_READ_DIRECTION, false, aid).unwrap();
        assert_eq!(n, 0, "GR 单向：远端未读不产生写");
    }

    /// 审计 P2-8③ 反向：DB 写失败必须返回 Err，不再 `unwrap_or(0)` 吞成
    /// 「0 行变化」。注入方式：用一个**未建 schema** 的空内存库（无 articles
    /// 表），UPDATE 必然以 `no such table: articles` 失败——确定性、无 mock、
    /// 不依赖触发器/FK 行为。
    /// 判别力声明：若恢复 `unwrap_or(0)` 语义，本用例两处断言都红。
    #[test]
    fn apply_read_by_policy_propagates_db_write_failure() {
        let conn = Connection::open_in_memory().unwrap();

        // 读格：双向权威的「命中 → 标读」分支（DB 写失败）。
        let read_err = apply_read_by_policy(&conn, FEVER_READ_DIRECTION, true, 1);
        assert!(
            read_err.is_err(),
            "DB 写失败必须向上传播为 Err，不得被吞成 Ok(0)：{read_err:?}"
        );

        // 星标格：命中 → 收藏分支（DB 写失败）。
        let star_err = apply_star_by_policy(&conn, FEVER_STAR_DIRECTION, true, 1);
        assert!(
            star_err.is_err(),
            "星标格写失败同样必须传播为 Err：{star_err:?}"
        );
    }
}
