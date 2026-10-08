// Note: 「远端已确认」不可累计溯源，不得虚构总数 — 见 .agents/notes/implemented/feature/2026-10-06-同步四态与失败可见性.md
use super::*;
use rusqlite::params;
use serde::Serialize;

/// 离线变更队列条目
#[derive(Debug, Serialize)]
pub struct SyncQueueItem {
    pub id: i64,
    pub article_id: Option<i64>,
    pub feed_url: Option<String>,
    pub action: String,
    pub payload: Option<String>,
    /// 入队时间（SQLite datetime('now')，UTC 'YYYY-MM-DD HH:MM:SS'）。
    /// A-8：无远端绑定项的老化清理依据。
    pub created_at: String,
}

/// 本地变更入队（已读/收藏等）。同一条目同向的旧记录先删，避免重复推送。
pub fn enqueue_sync(
    conn: &Connection,
    article_id: Option<i64>,
    feed_url: Option<&str>,
    action: &str,
    payload: Option<&str>,
) -> AppResult<()> {
    // 同 article+action 只保留最新一条（read/unread 视为同向互斥，直接覆盖）
    if let Some(aid) = article_id {
        conn.execute(
            "DELETE FROM sync_queue WHERE article_id = ?1 AND action IN (?2, ?3)",
            params![aid, opposite_action(action), action],
        )?;
    }
    // created_at 用 SQLite datetime('now')（UTC，'YYYY-MM-DD HH:MM:SS'）——与建表
    // 默认值及历史数据同格式，老化比较才能按字符串正确排序
    conn.execute(
        "INSERT INTO sync_queue (article_id, feed_url, action, payload, created_at)
         VALUES (?1, ?2, ?3, ?4, datetime('now'))",
        params![article_id, feed_url, action, payload],
    )?;
    Ok(())
}

/// read↔unread / star↔unstar 的反向动作（入队去重用）
fn opposite_action(action: &str) -> &str {
    match action {
        "read" => "unread",
        "unread" => "read",
        "star" => "unstar",
        "unstar" => "star",
        _ => "",
    }
}

/// 取出全部待推送条目（不删除；成功后由 prune_sync 清除）
pub fn take_sync_queue(conn: &Connection) -> AppResult<Vec<SyncQueueItem>> {
    let mut stmt = conn.prepare(
        "SELECT id, article_id, feed_url, action, payload, created_at FROM sync_queue ORDER BY id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(SyncQueueItem {
            id: r.get(0)?,
            article_id: r.get(1)?,
            feed_url: r.get(2)?,
            action: r.get(3)?,
            payload: r.get(4)?,
            created_at: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 清除已成功推送的队列条目（远端确认 = 按 id 精确出队）。
/// 精确确认的前提是**操作 id 在删除后不复用**：推送计划取快照后即释放 DB
/// 锁，网络往返只由 PUSH_LOCK 串行保护，而本地入队不经过 PUSH_LOCK——窗口内
/// 复用 id 会让旧计划返回后按 id prune 误删新的用户意图（审计 F01；v18 起
/// sync_queue 为 AUTOINCREMENT，见 db/migrations.rs 的 v18 迁移）。
// Note: 操作 id 不复用是旧推送计划按 id 精确确认的前提 — 见 .agents/notes/implemented/bug-fix/2026-09-18-状态写入事务化与对账守卫.md
pub fn prune_sync(conn: &Connection, ids: &[i64]) -> AppResult<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    conn.execute(
        &format!("DELETE FROM sync_queue WHERE id IN ({placeholders})"),
        rusqlite::params_from_iter(ids.iter()),
    )?;
    Ok(())
}

/// 清掉历史版本残留的 remove_feed 僵尸队项（该动作从未被 sync 消费；现行
/// 删除语义为「本地删除不推远端」，见 delete_feed 命令）。返回清除条数。
pub fn purge_remove_feed_zombies(conn: &Connection) -> AppResult<usize> {
    let n = conn.execute("DELETE FROM sync_queue WHERE action = 'remove_feed'", [])?;
    Ok(n)
}

/// 老化清理（A-8）：删除超过保留期、且仍无远端绑定的状态队列项。
/// 只清理状态类动作（read/unread/star/unstar）——add_feed 由 feeds 阶段处理。
/// `cutoff` 与 created_at 同为 'YYYY-MM-DD HH:MM:SS'（UTC）。返回清理条数。
pub fn prune_stale_unbound(conn: &Connection, cutoff: &str) -> AppResult<usize> {
    let n = conn.execute(
        "DELETE FROM sync_queue
          WHERE created_at < ?1
            AND action IN ('read', 'unread', 'star', 'unstar')
            AND (article_id IS NULL
                 OR article_id IN (SELECT id FROM articles WHERE remote_id IS NULL))",
        params![cutoff],
    )?;
    Ok(n)
}

/* ============================================================
TASK-116（同步四态展示）：失败标记 + 队列统计
============================================================ */

/// last_error 摘要的最大长度（字符数）。错误摘要进 UI「部分失败（最新错误）」
/// 单行展示，超长只留开头——错误尾巴通常是冗余的堆栈/URL 细节。
const LAST_ERROR_MAX_CHARS: usize = 200;

/// 错误摘要截断：按字符（码点）截断保证多字节安全，截断后以「…」示意。
fn truncate_last_error(err: &str) -> String {
    if err.chars().count() <= LAST_ERROR_MAX_CHARS {
        return err.to_string();
    }
    let head: String = err.chars().take(LAST_ERROR_MAX_CHARS).collect();
    format!("{head}…")
}

/// 推送失败项落库标记（TASK-116）：attempts +1 并记录错误摘要。
/// 队列行保留（失败 = 天然重试语义，见 sync/push.rs）；成功路径不走这里——
/// 成功项由 prune_sync 整行删除，attempts/last_error 随之消失（stats 归零）。
/// 状态变更重新入队（enqueue_sync 删旧插新）也会重置为未失败。
pub fn mark_push_failed(conn: &Connection, failed: &[(i64, String)]) -> AppResult<()> {
    if failed.is_empty() {
        return Ok(());
    }
    let mut stmt = conn
        .prepare("UPDATE sync_queue SET attempts = attempts + 1, last_error = ?1 WHERE id = ?2")?;
    for (id, summary) in failed {
        stmt.execute(params![truncate_last_error(summary), id])?;
    }
    Ok(())
}

/// 推送阻塞标记（TASK-124，审计 P2-6②）：后端客户端构建失败（认证/端点解析/
/// 网络不可达）时，本轮推送**未进 exec_push**——此前 attempts/last_error 对这类
/// 失败永不记录，用户在 UI 上永远看不到「为什么一直不同步」。现在把**全部现存
/// 队项**计一次失败尝试（attempts+1）并记录摘要——与 mark_push_failed 同口径
/// （attempts 累加、摘要按码点截断 200 字符）；摘要带「认证失败」/实际错误文案，
/// 由 sync::push_block_summary 分类。空队列为 no-op，返回标记行数（调用方据
/// n>0 决定是否发 sync-queue-changed——统计真变了才发，X3 不制造噪音）。
pub fn mark_push_blocked(conn: &Connection, summary: &str) -> AppResult<usize> {
    let summary = truncate_last_error(summary);
    let n = conn.execute(
        "UPDATE sync_queue SET attempts = attempts + 1, last_error = ?1",
        params![summary],
    )?;
    Ok(n)
}

/// 队列统计（四态展示口径，TASK-116）：
/// - waiting = 队列现存行数（含 add_feed；「等待同步 N 条」的 N）；
/// - failed  = attempts > 0 的行数（「部分失败」的 N；是 waiting 的子集）；
/// - last_error = 最近一次失败摘要（按队列 id 最大者——入队互斥合并会删旧插
///   新，id 单调递增；失败标记不删除行，故 id 最大 ≡ 最晚入队，无独立失败
///   时间戳列也够用）。
#[derive(Debug, Serialize)]
pub struct SyncQueueStats {
    pub waiting: usize,
    pub failed: usize,
    pub last_error: Option<String>,
}

pub fn sync_queue_stats(conn: &Connection) -> AppResult<SyncQueueStats> {
    let (waiting, failed, last_error): (i64, i64, Option<String>) = conn.query_row(
        "SELECT (SELECT COUNT(*) FROM sync_queue),
                (SELECT COUNT(*) FROM sync_queue WHERE attempts > 0),
                (SELECT last_error FROM sync_queue
                  WHERE attempts > 0 AND last_error IS NOT NULL
                  ORDER BY id DESC LIMIT 1)",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    Ok(SyncQueueStats {
        waiting: waiting as usize,
        failed: failed as usize,
        last_error,
    })
}

/* ============================================================
TASK-124（同步状态变化事件）：sync-queue-changed 事件 payload 单点
============================================================ */

/// `sync-queue-changed` 事件 payload（纯函数，wire 形态单点 + 可测）：
/// `{waiting, failed, last_error}` 与 [`SyncQueueStats`] / 前端 `SyncQueueStats`
/// （src/lib/api.ts）三方同形。configured=false → None：未配置同步静默不发事件
/// （审计「区分未配置/认证失败/网络失败」的「未配置」半边——前端 pill 靠既有
/// 「本地模式 · 直连抓取」分支，不误报不刷状态）。
pub fn queue_changed_payload(
    stats: &SyncQueueStats,
    configured: bool,
) -> Option<serde_json::Value> {
    if !configured {
        return None;
    }
    Some(serde_json::json!({
        "waiting": stats.waiting,
        "failed": stats.failed,
        "last_error": stats.last_error,
    }))
}

/* ============================================================
文件级测试助手（#[cfg(test)]）：t116/t124 两个测试模块共用。
TASK-124 修复轮 R1：`attempts_of` 原私有定义在 `mod t116_tests` 内，
兄弟模块 `mod t124_tests` 引用它报 E0425（Rust 模块私有性：兄弟模块的
私有项互不可见）。上移到文件级后两模块经 `use super::*` 解析同一份。
============================================================ */
#[cfg(test)]
fn attempts_of(c: &Connection, id: i64) -> i64 {
    c.query_row(
        "SELECT attempts FROM sync_queue WHERE id = ?1",
        params![id],
        |r| r.get(0),
    )
    .unwrap()
}

/* ============================================================
TASK-116 单元测试：失败标记落库 / 成功 prune 归零 / stats 口径
（cargo test 由 CI 执行，DEC-local-cargo-gate-20261005）
============================================================ */
#[cfg(test)]
mod t116_tests {
    use super::*;

    fn conn() -> Connection {
        let mut c = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut c).unwrap();
        c
    }

    fn last_error_of(c: &Connection, id: i64) -> Option<String> {
        c.query_row(
            "SELECT last_error FROM sync_queue WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .unwrap()
    }

    /// (t116-r1) 失败标记：attempts +1 / last_error 摘要落库，只标记失败 id；
    /// 重复失败累加不覆盖计数；超长摘要按码点截断。判别力：attempts 恒等写死
    /// （不 +1）、标记全队列、或 last_error 不落库的实现必红。
    #[test]
    fn mark_push_failed_records_attempts_and_error() {
        let c = conn();
        enqueue_sync(&c, None, None, "read", None).unwrap();
        enqueue_sync(&c, None, None, "star", None).unwrap();
        let items = take_sync_queue(&c).unwrap();
        assert_eq!(items.len(), 2, "前置：两条队项就位");
        let (id_a, id_b) = (items[0].id, items[1].id);

        // 首次失败：只标记 id_a
        mark_push_failed(&c, &[(id_a, "状态推送失败: HTTP 500".into())]).unwrap();
        assert_eq!(attempts_of(&c, id_a), 1, "失败一次 attempts=1");
        assert_eq!(
            last_error_of(&c, id_a).as_deref(),
            Some("状态推送失败: HTTP 500"),
            "错误摘要落库"
        );
        assert_eq!(attempts_of(&c, id_b), 0, "未失败的队项不得被标记");
        assert_eq!(last_error_of(&c, id_b), None);

        // 再次失败：累加（不重置），摘要更新为最新一次
        mark_push_failed(&c, &[(id_a, "状态推送失败: timeout".into())]).unwrap();
        assert_eq!(attempts_of(&c, id_a), 2, "重复失败 attempts 累加");
        assert_eq!(
            last_error_of(&c, id_a).as_deref(),
            Some("状态推送失败: timeout"),
            "摘要更新为最近一次"
        );

        // 超长摘要截断：200 字符 + 「…」，按码点截（多字节安全）
        let long = "错".repeat(300);
        mark_push_failed(&c, &[(id_b, long)]).unwrap();
        let stored = last_error_of(&c, id_b).unwrap();
        assert_eq!(
            stored.chars().count(),
            LAST_ERROR_MAX_CHARS + 1,
            "截断到 200 字符 + 「…」"
        );
        assert!(stored.ends_with('…'));
        assert!(stored.starts_with('错'));
        // 空列表 no-op（生产调用方以 is_empty 守卫，这里锁死该语义）
        mark_push_failed(&c, &[]).unwrap();
        assert_eq!(attempts_of(&c, id_a), 2);
    }

    /// (t116-r2) 成功路径：成功项 prune 出队后 stats 归零；同轮失败项不受影响。
    /// 判别力：prune 语义被改成「成功也留队」或「清空全表」时必红（远端确认 =
    /// 出队、失败保留 = 重试，两条口径同时锁死）。
    #[test]
    fn successful_prune_zeros_stats_failed_rows_survive() {
        let c = conn();
        enqueue_sync(&c, None, None, "read", None).unwrap();
        enqueue_sync(&c, None, None, "unread", None).unwrap();
        let items = take_sync_queue(&c).unwrap();
        let (id_a, id_b) = (items[0].id, items[1].id);

        // 混合轮：id_a 成功、id_b 失败
        mark_push_failed(&c, &[(id_b, "状态推送失败: HTTP 503".into())]).unwrap();
        prune_sync(&c, &[id_a]).unwrap();
        let s = sync_queue_stats(&c).unwrap();
        assert_eq!(s.waiting, 1, "成功项已出队，失败项保留");
        assert_eq!(s.failed, 1, "失败标记随保留行留存");
        assert_eq!(s.last_error.as_deref(), Some("状态推送失败: HTTP 503"));

        // 重试成功：失败项也出队 → stats 全归零（X3：无队列无失败不制造噪音）
        prune_sync(&c, &[id_b]).unwrap();
        let s = sync_queue_stats(&c).unwrap();
        assert_eq!((s.waiting, s.failed, s.last_error.is_none()), (0, 0, true));
    }

    /// (t116-r3) stats 口径：waiting=全部行、failed=attempts>0 子集、
    /// last_error 取 id 最大（最晚入队）者；空队列为零值。
    #[test]
    fn sync_queue_stats_counts_waiting_failed_and_latest_error() {
        let c = conn();
        let empty = sync_queue_stats(&c).unwrap();
        assert_eq!(
            (empty.waiting, empty.failed, empty.last_error.is_none()),
            (0, 0, true),
            "空队列 = 零值（新装用户无噪音）"
        );

        enqueue_sync(&c, None, None, "read", None).unwrap();
        enqueue_sync(&c, None, None, "star", None).unwrap();
        enqueue_sync(&c, None, None, "unstar", None).unwrap();
        let items = take_sync_queue(&c).unwrap();
        let (id_a, id_b, id_c) = (items[0].id, items[1].id, items[2].id);

        // 只标记最早一条：last_error = 它自己的摘要
        mark_push_failed(&c, &[(id_a, "第一条失败".into())]).unwrap();
        let s = sync_queue_stats(&c).unwrap();
        assert_eq!(s.waiting, 3);
        assert_eq!(s.failed, 1);
        assert_eq!(s.last_error.as_deref(), Some("第一条失败"));

        // 再标记最晚一条：last_error 切到 id 最大者（最近口径）
        mark_push_failed(&c, &[(id_c, "第三条失败".into())]).unwrap();
        let s = sync_queue_stats(&c).unwrap();
        assert_eq!(s.waiting, 3, "waiting=队列全部行（含未失败）");
        assert_eq!(s.failed, 2, "failed=attempts>0 的子集");
        assert_eq!(
            s.last_error.as_deref(),
            Some("第三条失败"),
            "last_error 取 id 最大（最晚入队）的失败行"
        );
        assert_eq!(attempts_of(&c, id_b), 0, "中间行未受影响");
    }
}

/* ============================================================
TASK-124 单元测试：事件 payload 正确性 / 推送阻塞标记
（cargo test 由 CI 执行，DEC-local-cargo-gate-20261005）
============================================================ */
#[cfg(test)]
mod t124_tests {
    use super::*;

    fn conn() -> Connection {
        let mut c = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut c).unwrap();
        c
    }

    /// (t124-r1) 事件 payload 正确性（探针 P8 链路的 Rust 半边）：队列变更后
    /// stats 查询即得最新三元组，payload 与之一一对应——入队 → waiting=1 且
    /// failed=0（无 last_error）；失败标记 → failed=1 + 摘要；成功出队 → 全归零。
    /// 未配置（configured=false）→ None（静默不发事件）。
    /// 判别力：payload 字段名/口径漂移（如 failed 误用全队行数、last_error 丢失）
    /// 或未配置误发事件的实现必红。
    #[test]
    fn queue_changed_payload_reflects_stats_after_mutation() {
        let c = conn();

        // 未配置静默：无论队列什么状态，configured=false 一律 None
        let s = sync_queue_stats(&c).unwrap();
        assert!(queue_changed_payload(&s, false).is_none(), "未配置不发事件");

        // 空队列（已配置）：零值 payload（新装用户无噪音）
        let p = queue_changed_payload(&sync_queue_stats(&c).unwrap(), true).unwrap();
        assert_eq!(p["waiting"].as_u64(), Some(0));
        assert_eq!(p["failed"].as_u64(), Some(0));
        assert!(p["last_error"].is_null());

        // 探针 P8 本体（Rust 半边）：本地标读入队 → waiting=1（「等待同步 1 条」的 N）
        enqueue_sync(&c, None, None, "read", None).unwrap();
        let p = queue_changed_payload(&sync_queue_stats(&c).unwrap(), true).unwrap();
        assert_eq!(p["waiting"].as_u64(), Some(1));
        assert_eq!(p["failed"].as_u64(), Some(0));
        assert!(p["last_error"].is_null());

        // 推送失败标记 → failed=1 + last_error 摘要（「· 部分失败」的依据）
        let id = take_sync_queue(&c).unwrap().remove(0).id;
        mark_push_failed(&c, &[(id, "状态推送失败: HTTP 503".into())]).unwrap();
        let p = queue_changed_payload(&sync_queue_stats(&c).unwrap(), true).unwrap();
        assert_eq!(p["waiting"].as_u64(), Some(1));
        assert_eq!(p["failed"].as_u64(), Some(1));
        assert_eq!(p["last_error"], "状态推送失败: HTTP 503");

        // 推送成功出队 → 恢复「后端已同步」（全归零、last_error 清失）
        prune_sync(&c, &[id]).unwrap();
        let p = queue_changed_payload(&sync_queue_stats(&c).unwrap(), true).unwrap();
        assert_eq!(p["waiting"].as_u64(), Some(0));
        assert_eq!(p["failed"].as_u64(), Some(0));
        assert!(p["last_error"].is_null());
    }

    /// (t124-r3) 推送阻塞标记（审计 P2-6②）：客户端构建失败时全部现存队项
    /// attempts+1 + last_error 落库（failed 计数口径不变 = attempts>0 行数，
    /// 认证失败后 failed == waiting）；空队列为 no-op；重复阻塞累加（与
    /// exec_push 失败的 mark_push_failed 同口径——每次被阻塞的推送都是一次失败尝试）。
    #[test]
    fn mark_push_blocked_records_all_rows_and_accumulates() {
        let c = conn();

        // 空队列 no-op：返回 0（调用方据此不发事件）
        assert_eq!(mark_push_blocked(&c, "认证失败：x").unwrap(), 0);
        assert_eq!(sync_queue_stats(&c).unwrap().waiting, 0);

        enqueue_sync(&c, None, None, "read", None).unwrap();
        enqueue_sync(&c, None, None, "star", None).unwrap();
        let items = take_sync_queue(&c).unwrap();
        let (id_a, id_b) = (items[0].id, items[1].id);

        // 首次阻塞：全部现存队项标记（含 add_feed 类队列行——客户端构建失败时
        // 它们同样推不出去）
        assert_eq!(
            mark_push_blocked(&c, "认证失败：ClientLogin → 401").unwrap(),
            2
        );
        let s = sync_queue_stats(&c).unwrap();
        assert_eq!(
            (s.waiting, s.failed),
            (2, 2),
            "认证失败后 failed == waiting"
        );
        assert_eq!(
            s.last_error.as_deref(),
            Some("认证失败：ClientLogin → 401"),
            "阻塞摘要落库（认证失败文案）"
        );
        assert_eq!(attempts_of(&c, id_a), 1);
        assert_eq!(attempts_of(&c, id_b), 1);

        // 重复阻塞：attempts 累加（不重置），摘要更新为最新一次
        mark_push_blocked(&c, "认证失败：密码已变更").unwrap();
        assert_eq!(attempts_of(&c, id_a), 2, "重复阻塞 attempts 累加");
        assert_eq!(
            sync_queue_stats(&c).unwrap().last_error.as_deref(),
            Some("认证失败：密码已变更")
        );

        // 新入队行是未失败的新行（article_id=None 无同向合并可触发，直接追加）：
        // failed 是 waiting 的真子集——新行不继承旧行的失败标记
        enqueue_sync(&c, None, None, "read", None).unwrap();
        let s = sync_queue_stats(&c).unwrap();
        assert_eq!(s.waiting, 3, "None 入队无合并：追加一行");
        assert_eq!(s.failed, 2, "新行未失败，failed 保持原两行");
    }
}

/* ============================================================
OPT-001（F01）单元测试：操作身份不复用（v17 复现锚 + v18 AUTOINCREMENT）
（cargo test 由 CI 执行，DEC-local-cargo-gate-20261005）
============================================================ */
#[cfg(test)]
mod opt001_tests {
    use super::*;

    /// (opt001-u1) 替换入队不复用操作 id（验收伪码的单元版）：
    /// ① v17 修前锚——替换入队复用被删行的最大 ROWID，旧计划按 id 确认会
    ///    误删新的用户意图；② v18（AUTOINCREMENT）同一序列——新 id 必不同、
    ///    旧 id 的确认不伤新项，清空后序列继续走高不回落。断言只验证
    ///    「不复用」的实际行为，不绑定 sqlite_sequence 表的实现细节。
    /// 判别力：v18 退回普通 INTEGER PRIMARY KEY 时，star/unstar 段必红
    /// （复用使 assert_ne 失败、prune 旧 id 直接删掉新行）。
    #[test]
    fn queue_identity_not_reused_across_replacement() {
        let mut c = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_version(&mut c, 17).unwrap();
        c.execute_batch(
            "INSERT INTO feeds (feed_url, title) VALUES ('https://f.example/rss', 'F');
             INSERT INTO articles (feed_id, guid, title) VALUES (1, 'g1', 't');",
        )
        .unwrap();

        // ---- ① 修前锚（v17 普通 rowid）：id 复用 → 旧确认误删新意图 ----
        enqueue_sync(&c, Some(1), None, "read", None).unwrap();
        let v17_old = take_sync_queue(&c).unwrap().remove(0).id;
        enqueue_sync(&c, Some(1), None, "unread", None).unwrap();
        let v17_new = take_sync_queue(&c).unwrap().remove(0).id;
        assert_eq!(
            v17_new, v17_old,
            "修前锚：v17 替换入队复用被删行的最大 ROWID（F01 复现的前提）"
        );
        prune_sync(&c, &[v17_old]).unwrap();
        assert!(
            take_sync_queue(&c).unwrap().is_empty(),
            "修前锚：旧计划按 id 确认会误删新的 unread 意图（F01 现场）"
        );

        // 升级前留下一条现存行（重申 read 再复用 id=1），升级后先确认清空它
        enqueue_sync(&c, Some(1), None, "read", None).unwrap();
        MIGRATIONS.to_latest(&mut c).unwrap();
        let basis = take_sync_queue(&c).unwrap().remove(0).id;
        assert_eq!(basis, 1, "前置：升级时现存最大 id = 1（v17 复用结果）");
        prune_sync(&c, &[basis]).unwrap();

        // ---- ② v18：同一序列不得复用；空队列后序列继续走高 ----
        // （sequence 由 SQLite 随拷贝/RENAME 自动维护，不手工重置）
        enqueue_sync(&c, Some(1), None, "star", None).unwrap();
        let star = take_sync_queue(&c).unwrap().remove(0).id;
        assert!(
            star > basis,
            "升级后新项 id 必须高于已提交最大 id（不复用被删行）"
        );

        enqueue_sync(&c, Some(1), None, "unstar", None).unwrap();
        let unstar = take_sync_queue(&c).unwrap().remove(0).id;
        assert_ne!(unstar, star, "v18：替换入队必须换新 id（不复用被删行）");
        prune_sync(&c, &[star]).unwrap();
        let q = take_sync_queue(&c).unwrap();
        assert_eq!(q.len(), 1, "旧 id 的确认不得删掉新用户意图");
        assert_eq!(q[0].id, unstar);
        assert_eq!(q[0].action, "unstar");

        // unstar 残留先清场：read/unread 入队不会清除 unstar，保留会让下一段
        // remove(0) 拿到旧行（清场后重新断言队列空，再进入 read 场景）
        prune_sync(&c, &[unstar]).unwrap();
        assert!(take_sync_queue(&c).unwrap().is_empty(), "unstar 场景清场");

        // read→unread 同断言（队列已空也不回落复用）
        enqueue_sync(&c, Some(1), None, "read", None).unwrap();
        let read = take_sync_queue(&c).unwrap().remove(0).id;
        assert!(read > unstar, "清空后新项 id 继续走高，不回落复用");
        enqueue_sync(&c, Some(1), None, "unread", None).unwrap();
        let unread = take_sync_queue(&c).unwrap().remove(0).id;
        assert!(unread > read, "替换入队 id 严格递增");
        prune_sync(&c, &[read]).unwrap();
        let q = take_sync_queue(&c).unwrap();
        assert_eq!(
            (q.len(), q[0].id, q[0].action.as_str()),
            (1, unread, "unread"),
            "旧 read 计划的确认不得伤及新的 unread 意图"
        );
    }
}
