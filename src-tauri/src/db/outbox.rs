//! 类型化订阅动作 outbox（OPT-009 阶段 A）。
//!
//! 订阅写回（新增/改名/移动/退订）不再依赖「本地改完 best-effort 推一次」：
//! 本地修改与队列入队在同一事务落库，推送失败 = 队项保留重试；远端确认按
//! (id, account_generation) 精确出队。与 article 状态队列 `sync_queue` 分离——
//! feed 删除级联、`purge_remove_feed_zombies` 都不会清掉这里的行（退订目标
//! 必须在本地 feed 消亡后仍存活）。
//!
//! ## 身份与代际
//! - `account_key` = 协议 + 规范化服务器根 + 用户名（持久身份，跨代际不变）；
//! - `account_generation` = 入队时会话代际（同一账号改密码也会推进）。
//!
//! 推送门槛：key 匹配当前会话才可发；key 匹配但代际落后 → 重绑代际后发送
//! （同账号的意图仍然有效）；key 不匹配 → 暂停（换号不得把旧意图发给新账号，
//! 回原账号后恢复可发）。
//!
//! ## 合并与确认
//! 连续修改合并「最后 desired」= 删除同 URL 旧行 + 插入新行。id 为 AUTOINCREMENT
//! 不复用：旧 HTTP 计划按旧 id 确认只会命中「已不存在」的行，不可能误删中途
//! 改写的新目标（F01 的同一纪律，见 db/migrations.rs v18/v23 注释）。
// Note: 操作 id 不复用是旧推送计划按 id 精确确认的前提 — 见 .agents/notes/implemented/bug-fix/2026-09-18-状态写入事务化与对账守卫.md

use super::*;
use rusqlite::{params, OptionalExtension};
use serde::Serialize;

pub const ACTION_SUBSCRIBE: &str = "subscribe";
pub const ACTION_EDIT: &str = "edit";
pub const ACTION_UNSUBSCRIBE: &str = "unsubscribe";
/// subscribe 的阶段：远端尚未创建。
pub const STAGE_PENDING: &str = "pending";
/// subscribe 的阶段：quick_add 已成功（remote_id 已带回），待本地绑定/分类挂载确认。
pub const STAGE_REMOTE_CREATED: &str = "remote_created";
pub const STATUS_PENDING: &str = "pending";
/// 远端协议不支持该写回（Fever）：本地已完成、远端明确不宣称——终态，
/// 不再重试，也不能用 Ok 冒充远端完成。
pub const STATUS_LOCAL_ONLY: &str = "local_only";

/// 一条订阅 outbox 队项（类型化 payload；重启后仍完整可用）。
#[derive(Debug, Clone, Serialize)]
pub struct SubscriptionOutboxItem {
    pub id: i64,
    pub account_generation: i64,
    pub account_key: String,
    pub action: String,
    pub feed_url: String,
    pub remote_id: Option<i64>,
    pub desired_title: Option<String>,
    pub desired_folder: Option<String>,
    /// 编辑前的分类 label：移动时 wire 需要 r= 移除旧 label（仅追加 a= 不是移动）。
    pub prev_folder: Option<String>,
    pub stage: String,
    pub status: String,
    pub attempts: i64,
    pub last_error: Option<String>,
}

/// 源新增入队（本地入库同一事务内调用）。
/// 合并规则：同 URL 的既有队项一律替换为新意图——新意图即最新本地状态；
/// 同时清掉同 URL 的历史 `add_feed` 兼容队项（新 outbox 已接管订阅推送，
/// 留着会造成双份 quick_add）。
pub fn enqueue_subscribe(
    conn: &Connection,
    account_generation: i64,
    account_key: &str,
    feed_url: &str,
    desired_folder: Option<&str>,
) -> AppResult<()> {
    conn.execute(
        "DELETE FROM subscription_outbox WHERE feed_url = ?1",
        params![feed_url],
    )?;
    conn.execute(
        "DELETE FROM sync_queue WHERE action = 'add_feed' AND feed_url = ?1",
        params![feed_url],
    )?;
    conn.execute(
        "INSERT INTO subscription_outbox
            (account_generation, account_key, action, feed_url, desired_folder)
         VALUES (?1, ?2, 'subscribe', ?3, ?4)",
        params![account_generation, account_key, feed_url, desired_folder],
    )?;
    Ok(())
}

/// 退订入队（本地删除 feed 的同一事务内调用）。
/// `remote_id`：删除时已知的远端绑定；None 时从**同 account_key** 的既有队项
/// 结转（quick_add 已建、本地绑定尚未完成的场景——远端其实存在，退订目标不能丢）。
/// 异账号一律不结转：A 的远端目标不得被 B 的退订借用（与代际屏障同源）。
/// 返回 true = 确实入了队（有远端目标）；false = 无从退订（从未绑定且无同账号队项），
/// 调用方只留本地墓碑即可。
pub fn enqueue_unsubscribe(
    conn: &Connection,
    account_generation: i64,
    account_key: &str,
    feed_url: &str,
    remote_id: Option<i64>,
) -> AppResult<bool> {
    let inherited: Option<i64> = conn
        .query_row(
            "SELECT remote_id FROM subscription_outbox
              WHERE feed_url = ?1 AND account_key = ?2 AND remote_id IS NOT NULL
              ORDER BY id DESC LIMIT 1",
            params![feed_url, account_key],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    conn.execute(
        "DELETE FROM subscription_outbox WHERE feed_url = ?1",
        params![feed_url],
    )?;
    conn.execute(
        "DELETE FROM sync_queue WHERE action = 'add_feed' AND feed_url = ?1",
        params![feed_url],
    )?;
    let Some(rid) = remote_id.or(inherited) else {
        return Ok(false);
    };
    conn.execute(
        "INSERT INTO subscription_outbox
            (account_generation, account_key, action, feed_url, remote_id)
         VALUES (?1, ?2, 'unsubscribe', ?3, ?4)",
        params![account_generation, account_key, feed_url, rid],
    )?;
    Ok(true)
}

/// 订阅编辑（改名/移动）入队。`desired_*` 是编辑后的完整本地目标（last-write
/// wins 的合并基础）；`prev_folder` 是编辑前的分类 label。同 URL 替换。
#[allow(clippy::too_many_arguments)] // 入队参数即 wire 意图字段，拆 struct 只是搬运
pub fn enqueue_edit(
    conn: &Connection,
    account_generation: i64,
    account_key: &str,
    feed_url: &str,
    remote_id: i64,
    desired_title: Option<&str>,
    desired_folder: Option<&str>,
    prev_folder: Option<&str>,
) -> AppResult<()> {
    conn.execute(
        "DELETE FROM subscription_outbox WHERE feed_url = ?1",
        params![feed_url],
    )?;
    conn.execute(
        "INSERT INTO subscription_outbox
            (account_generation, account_key, action, feed_url, remote_id,
             desired_title, desired_folder, prev_folder)
         VALUES (?1, ?2, 'edit', ?3, ?4, ?5, ?6, ?7)",
        params![
            account_generation,
            account_key,
            feed_url,
            remote_id,
            desired_title,
            desired_folder,
            prev_folder
        ],
    )?;
    Ok(())
}

fn row_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<SubscriptionOutboxItem> {
    Ok(SubscriptionOutboxItem {
        id: r.get(0)?,
        account_generation: r.get(1)?,
        account_key: r.get(2)?,
        action: r.get(3)?,
        feed_url: r.get(4)?,
        remote_id: r.get(5)?,
        desired_title: r.get(6)?,
        desired_folder: r.get(7)?,
        prev_folder: r.get(8)?,
        stage: r.get(9)?,
        status: r.get(10)?,
        attempts: r.get(11)?,
        last_error: r.get(12)?,
    })
}

const SELECT_COLS: &str = "id, account_generation, account_key, action, feed_url, remote_id,
     desired_title, desired_folder, prev_folder, stage, status, attempts, last_error";

/// 全部待推送队项（status='pending'，按入队顺序）。
pub fn pending_outbox(conn: &Connection) -> AppResult<Vec<SubscriptionOutboxItem>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {SELECT_COLS} FROM subscription_outbox WHERE status = 'pending' ORDER BY id"
    ))?;
    let rows = stmt.query_map([], row_from)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 同账号（key 相同）但代际落后的待推队项重绑到当前代际：
/// 同一账号改密码/重新保存会推进代际，旧意图仍属于该账号，重绑后继续可发。
/// key 不匹配的行一律不touch（换号后暂停，回原账号才恢复）。
pub fn rebind_outbox_generation(
    conn: &Connection,
    account_key: &str,
    account_generation: i64,
) -> AppResult<usize> {
    let n = conn.execute(
        "UPDATE subscription_outbox SET account_generation = ?1, updated_at = datetime('now')
          WHERE account_key = ?2 AND account_generation <> ?1 AND status = 'pending'",
        params![account_generation, account_key],
    )?;
    Ok(n)
}

/// subscribe 已越过 quick_add：记录 remote_id 并进入 remote_created 阶段
/// （后续只做本地绑定与分类挂载，重启后不会重新 quick_add）。
/// 守卫 (id, generation, action='subscribe', status)：过期响应不得推进新行；
/// 阶段只为 subscribe 定义，edit/unsubscribe 行不得被写进 remote_created
/// 或改写 remote_id（R2/P3-3）。
pub fn advance_subscribe_stage(
    conn: &Connection,
    id: i64,
    account_generation: i64,
    remote_id: i64,
) -> AppResult<bool> {
    let n = conn.execute(
        "UPDATE subscription_outbox
            SET stage = 'remote_created', remote_id = ?1, updated_at = datetime('now')
          WHERE id = ?2 AND account_generation = ?3 AND action = 'subscribe' AND status = 'pending'",
        params![remote_id, id, account_generation],
    )?;
    Ok(n > 0)
}

/// 精确确认出队：只删「本轮计划的那一行、代际未变、仍是 pending」。
/// 中途被合并改写的队项是新 id（AUTOINCREMENT 不复用），旧 id 的确认天然落空。
pub fn prune_outbox(conn: &Connection, ids: &[i64], account_generation: i64) -> AppResult<usize> {
    let mut n = 0usize;
    for id in ids {
        n += conn.execute(
            "DELETE FROM subscription_outbox
              WHERE id = ?1 AND account_generation = ?2 AND status = 'pending'",
            params![id, account_generation],
        )?;
    }
    Ok(n)
}

/// 远端协议不支持：置 local_only 终态（保留展示，不重试、不冒充完成）。
pub fn mark_outbox_local_only(conn: &Connection, ids: &[i64]) -> AppResult<usize> {
    let mut n = 0usize;
    for id in ids {
        n += conn.execute(
            "UPDATE subscription_outbox
                SET status = 'local_only', updated_at = datetime('now')
              WHERE id = ?1 AND status = 'pending'",
            params![id],
        )?;
    }
    Ok(n)
}

/// 失败标记：attempts +1、last_error 摘要（截断口径与 sync_queue 一致）。
/// 入参 (id, account_generation, 摘要)：代际守卫防「过期计划的失败」记到同 id
/// 但代际已推进的行上（id 不复用盖不住重绑这一维，R2/P3-2）。
pub fn mark_outbox_failed(conn: &Connection, failed: &[(i64, i64, String)]) -> AppResult<()> {
    if failed.is_empty() {
        return Ok(());
    }
    let mut stmt = conn.prepare(
        "UPDATE subscription_outbox
            SET attempts = attempts + 1, last_error = ?1, updated_at = datetime('now')
          WHERE id = ?2 AND account_generation = ?3 AND status = 'pending'",
    )?;
    for (id, generation, summary) in failed {
        stmt.execute(params![
            super::sync_queue::truncate_last_error(summary),
            id,
            generation
        ])?;
    }
    Ok(())
}

/// 订阅写回统计（UI 持久展示）：pending = 待推/待重试；failed = pending 中
/// attempts>0；local_only = 远端不支持、仅本地生效的终态项。
#[derive(Debug, Serialize)]
pub struct SubscriptionOutboxStats {
    pub pending: usize,
    pub failed: usize,
    pub local_only: usize,
    pub last_error: Option<String>,
}

pub fn subscription_outbox_stats(conn: &Connection) -> AppResult<SubscriptionOutboxStats> {
    let (pending, failed, local_only, last_error): (i64, i64, i64, Option<String>) = conn.query_row(
        "SELECT (SELECT COUNT(*) FROM subscription_outbox WHERE status = 'pending'),
                (SELECT COUNT(*) FROM subscription_outbox WHERE status = 'pending' AND attempts > 0),
                (SELECT COUNT(*) FROM subscription_outbox WHERE status = 'local_only'),
                (SELECT last_error FROM subscription_outbox
                  WHERE status = 'pending' AND attempts > 0 AND last_error IS NOT NULL
                  ORDER BY id DESC LIMIT 1)",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    Ok(SubscriptionOutboxStats {
        pending: pending as usize,
        failed: failed as usize,
        local_only: local_only as usize,
        last_error,
    })
}

/// 某 URL 是否仍有 pending 队项（删除对账保护用：本地动作尚未回传时不删）。
pub fn outbox_pending_for_url(conn: &Connection, feed_url: &str) -> AppResult<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM subscription_outbox
          WHERE feed_url = ?1 AND status = 'pending'",
        params![feed_url],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/* ============================================================
单元测试：合并/精确确认/代际重绑/local-only/stats
（cargo test 由 CI 执行，DEC-local-cargo-gate-20261005）
============================================================ */
#[cfg(test)]
mod t_opt009_tests {
    use super::*;

    fn conn() -> Connection {
        let mut c = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut c).unwrap();
        c
    }

    fn ids(c: &Connection) -> Vec<i64> {
        pending_outbox(c)
            .unwrap()
            .into_iter()
            .map(|i| i.id)
            .collect()
    }

    /// 合并纪律：同 URL 的新意图替换旧意图且**换新 id**；旧 id 的确认落空，
    /// 不会误删合并后的新目标（旧 HTTP 成功不得确认新意图）。
    /// 判别力：若实现改为「UPDATE 复用同一行」，旧 id 确认会删掉新 desired 目标，
    /// 下面 `prune_outbox(old_id)` 后队列清空的断言即红。
    #[test]
    fn merge_replaces_intent_without_id_reuse_old_confirm_misses_new() {
        let c = conn();
        enqueue_edit(
            &c,
            0,
            "k",
            "https://f.example/rss",
            10,
            Some("旧标题"),
            Some("A"),
            None,
        )
        .unwrap();
        let old = ids(&c)[0];
        enqueue_edit(
            &c,
            0,
            "k",
            "https://f.example/rss",
            10,
            Some("新标题"),
            Some("B"),
            Some("A"),
        )
        .unwrap();
        let new = ids(&c)[0];
        assert_ne!(new, old, "合并必须换新 id（不复用）");
        prune_outbox(&c, &[old], 0).unwrap();
        let left = pending_outbox(&c).unwrap();
        assert_eq!(left.len(), 1, "旧 id 的确认不得删掉新意图");
        assert_eq!(left[0].desired_title.as_deref(), Some("新标题"));
        assert_eq!(left[0].desired_folder.as_deref(), Some("B"));
        assert_eq!(
            left[0].prev_folder.as_deref(),
            Some("A"),
            "移动需带旧 label"
        );
        // 新 id 确认才清队
        prune_outbox(&c, &[new], 0).unwrap();
        assert!(pending_outbox(&c).unwrap().is_empty());
    }

    /// 代际屏障：key 不匹配的行不重绑（换号暂停）；key 相同但代际落后 → 重绑
    /// 到当前代际继续可发；已 local_only 的终态不参与重绑。
    #[test]
    fn generation_rebind_only_for_same_account_key() {
        let c = conn();
        enqueue_subscribe(&c, 1, "acct-A", "https://a.example/rss", None).unwrap();
        enqueue_subscribe(&c, 1, "acct-B", "https://b.example/rss", None).unwrap();
        rebind_outbox_generation(&c, "acct-A", 5).unwrap();
        let items = pending_outbox(&c).unwrap();
        let a = items
            .iter()
            .find(|i| i.feed_url.contains("a.example"))
            .unwrap();
        let b = items
            .iter()
            .find(|i| i.feed_url.contains("b.example"))
            .unwrap();
        assert_eq!(a.account_generation, 5, "同账号（key 相同）重绑到当前代际");
        assert_eq!(b.account_generation, 1, "异账号队项暂停，代际不动");
    }

    /// 退订结转：quick_add 已建但本地未绑定的目标 id，删源入队退订时必须结转，
    /// 且 feed 级联不会清 outbox（本表无外键）。
    /// 判别力：不结转 remote_id 时入队返回 false、退订请求发不出去。
    #[test]
    fn unsubscribe_inherits_remote_id_from_pending_subscribe() {
        let c = conn();
        enqueue_subscribe(&c, 0, "k", "https://f.example/rss", Some("分类")).unwrap();
        advance_subscribe_stage(&c, ids(&c)[0], 0, 42).unwrap();
        let queued = enqueue_unsubscribe(&c, 0, "k", "https://f.example/rss", None).unwrap();
        assert!(queued, "有结转目标必须入队");
        let items = pending_outbox(&c).unwrap();
        assert_eq!(items.len(), 1, "同 URL 只留最新意图");
        assert_eq!(items[0].action, "unsubscribe");
        assert_eq!(items[0].remote_id, Some(42), "remote_id 必须结转");
    }

    /// R2(P2-1) 跨账号结转屏障：A 的 pending subscribe（remote_id=42）不得被
    /// B 的退订借用——结转只认同 account_key（key 不同即换号，换号不得把旧
    /// 账号的远端目标发给新账号）。
    /// 判别力：修前查询只按 feed_url，B 的调用会返回 true 并插入
    /// account_key='acct-B', remote_id=42 的 unsubscribe；修后返回 false 且
    /// B 无队项。对照面钉住「同账号仍结转」，防修成「一律不结转」。
    #[test]
    fn unsubscribe_inherit_is_account_key_scoped() {
        let c = conn();
        enqueue_subscribe(&c, 1, "acct-A", "https://f.example/rss", None).unwrap();
        advance_subscribe_stage(&c, ids(&c)[0], 1, 42).unwrap();

        let queued = enqueue_unsubscribe(&c, 1, "acct-B", "https://f.example/rss", None).unwrap();
        assert!(!queued, "异账号不得结转 A 的 remote_id");
        assert!(
            pending_outbox(&c).unwrap().is_empty(),
            "不得形成可向 B 发送的 unsubscribe 队项"
        );

        // 对照面：同账号结转仍生效
        enqueue_subscribe(&c, 1, "acct-A", "https://f.example/rss", None).unwrap();
        advance_subscribe_stage(&c, ids(&c)[0], 1, 42).unwrap();
        let queued = enqueue_unsubscribe(&c, 1, "acct-A", "https://f.example/rss", None).unwrap();
        assert!(queued, "同账号必须继续结转");
        let items = pending_outbox(&c).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].account_key, "acct-A");
        assert_eq!(items[0].remote_id, Some(42));
    }

    /// R2(P3-3) 阶段推进只对 subscribe 行生效：edit 行即使 (id, generation,
    /// status) 全配也不得被旧 subscribe 响应写成 remote_created / 改写 remote_id。
    /// 判别力：无 action 守卫时本调用返回 true 且 remote_id 被改写为 99。
    #[test]
    fn advance_stage_only_touches_subscribe_action() {
        let c = conn();
        enqueue_edit(
            &c,
            0,
            "k",
            "https://f.example/rss",
            10,
            Some("t"),
            None,
            None,
        )
        .unwrap();
        let id = ids(&c)[0];
        assert!(
            !advance_subscribe_stage(&c, id, 0, 99).unwrap(),
            "edit 行不得被 subscribe 阶段推进"
        );
        let item = &pending_outbox(&c).unwrap()[0];
        assert_eq!(
            item.remote_id,
            Some(10),
            "remote_id 不得被 subscribe 响应改写"
        );
        assert_eq!(item.stage, "pending");
    }

    /// local-only 终态：标记后不再 pending、不再重试；stats 分开计数；
    /// prune 只清 pending（local_only 行保留展示）。
    #[test]
    fn local_only_is_terminal_and_counted_separately() {
        let c = conn();
        enqueue_subscribe(&c, 0, "fever", "https://f.example/rss", None).unwrap();
        let id = ids(&c)[0];
        mark_outbox_local_only(&c, &[id]).unwrap();
        assert!(pending_outbox(&c).unwrap().is_empty(), "终态退出待推");
        assert_eq!(prune_outbox(&c, &[id], 0).unwrap(), 0, "prune 不得清终态行");
        let s = subscription_outbox_stats(&c).unwrap();
        assert_eq!((s.pending, s.failed, s.local_only), (0, 0, 1));
    }

    /// stats 口径 + 失败标记：attempts 累加、last_error 截断口径与 sync_queue 一致。
    /// R2(P3-2)：入参带代际，过期代际的失败标记不得落到当前行（判别力见下）。
    #[test]
    fn stats_and_failed_marker() {
        let c = conn();
        enqueue_subscribe(&c, 3, "k", "https://f.example/rss", None).unwrap();
        let id = ids(&c)[0];
        mark_outbox_failed(&c, &[(id, 3, "订阅推送失败: timeout".into())]).unwrap();
        mark_outbox_failed(&c, &[(id, 3, "订阅推送失败: HTTP 500".into())]).unwrap();
        // 代际守卫：过期计划的失败不得记到当前代际行（误记会让 attempts/last_error 撒谎）
        mark_outbox_failed(&c, &[(id, 99, "过期计划失败".into())]).unwrap();
        let s = subscription_outbox_stats(&c).unwrap();
        assert_eq!((s.pending, s.failed, s.local_only), (1, 1, 0));
        assert_eq!(s.last_error.as_deref(), Some("订阅推送失败: HTTP 500"));
        let item = &pending_outbox(&c).unwrap()[0];
        assert_eq!(item.attempts, 2);
        // 精确确认带代际：代际不符不得删行
        assert_eq!(prune_outbox(&c, &[id], 99).unwrap(), 0);
        assert_eq!(prune_outbox(&c, &[id], 3).unwrap(), 1);
    }
}
