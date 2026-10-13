use super::*;
use rusqlite::params;

/// 按 Miniflux entry id 找本地条目（Pull 状态合并的匹配键之一）
pub fn article_by_remote_id(conn: &Connection, remote_id: i64) -> AppResult<Option<i64>> {
    let id = conn
        .query_row(
            "SELECT id FROM articles WHERE remote_id = ?1",
            params![remote_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(id)
}

/* ============================================================
Pull 合并的批量预取映射 —— 消除 N+1

同步对账（pull_entries）里，原先对每个远端 entry 逐条查询
（url_norm 匹配 / remote_id 匹配 / 同源判定 / pending 集合 / feed 绑定），
首次同步上千条 = 数千次 SQLite 查询。这里一次性把全部映射查进内存，
循环内改为 HashMap/HashSet 查找（O(1)），把「数千次查询」压成「5 次批量查询」。

TASK-070（REQ-104）：被本映射取代的那批逐条查询函数已删除
（article_id_by_url / article_matches_remote_feed / article_has_pending_sync /
feed_by_remote_id / set_folder_remote_id），生产零调用。
============================================================ */

/// Pull 合并所需的全部匹配映射（一次批量预取）。
pub struct SyncMatchMaps {
    /// 规范化 URL（url_norm）→ article id（全局最小 id）。
    /// OPT-008B 起仅服务「远端 feed 未绑定」的有限兼容路径（跨源副本记账 +
    /// 已读广播）；**同源匹配不再消费它**——条目的精确身份是 remote_id 与
    /// （同 feed, url_norm）对，见 [`Self::feed_url_to_id`]。
    pub url_to_id: HashMap<String, i64>,
    /// (feed id, 规范化 URL) → article id（同 feed 最小 id）。
    /// OPT-008B 第一阶段：URL 兜底只在**同一订阅源内**选行；跨源同 URL 各自
    /// 独立成条目，不得再被全局 URL 映射吞并（旧 `url_to_id` 先于 remote_id
    /// 匹配的根因）。
    pub feed_url_to_id: HashMap<(i64, String), i64>,
    /// 规范化 URL → 该 URL 的全部 article id（升序）。
    /// 已读广播的同文副本选择集：本地标读一条 → 推送广播到这些行的 remote_id；
    /// 远端任一副本已读 → 拉取合并广播到其余行。「同文」判定为 url_norm 相等
    /// （008A 保守规范化；内容等价的安全依据留待第二阶段 contents）。
    pub url_norm_to_ids: HashMap<String, Vec<i64>>,
    /// article id → remote_id（None 表示未绑定）
    pub id_to_mf_id: HashMap<i64, Option<i64>>,
    /// article id → 其所属 feed 的 remote_id（None 表示 feed 未绑定）。
    /// 精确 entry id 匹配后的**信任复核**：远端 entry 所属 feed 与本行 feed
    /// 均已知且不同时按「脏绑定」拒绝合并（跨源 entry 不得改写另一源的行）。
    pub id_to_feed_mf: HashMap<i64, Option<i64>>,
    /// 有「已入队未推送」读/收藏变更的 article id 集合（并集；字段粒度见下两列）
    pub pending_ids: HashSet<i64>,
    /// 有「已入队未推送」read/unread 变更的 article id 集合（字段粒度）
    pub pending_read_ids: HashSet<i64>,
    /// 有「已入队未推送」star/unstar 变更的 article id 集合（字段粒度）
    pub pending_star_ids: HashSet<i64>,
    /// 本轮拉取中**已落定「已读」**的 article id 集合（own 远端已读或兄弟已读
    /// 广播，任一先落定即入集合）。顺序确定性政策（OPT-008B R1，单点）：
    /// 「读到哪算读」——同轮内任何已读信号优先于 own 行的远端 unread；
    /// own unread 只在「本轮从未有过已读信号」时才翻转该行。
    /// 由此「A 已读 / B 未读」无论本页处理顺序如何都收敛到同一结果（B 读），
    /// 与「标读广播同内容副本」的被接受政策一致；unread 永不广播。
    /// 跨轮无影响：已读信号每轮随 own entry 重新产生，远端保持未读的行若同组
    /// 再无已读副本，own unread 照常生效（unread 只认绑定同源 entry 的边界仍立）。
    pub explicit_read_ids: HashSet<i64>,
    /// article id → (read_version, star_version)：**pull 起点**的字段版本快照。
    /// OPT-007 字段版本持久守卫：远端合并按「快照版本 == 当前 DB 版本 且 该字段
    /// 无 pending」接受该字段；不符则跳过（本地在拉取期间写过的字段不得被旧
    /// 网络快照回滚）。快照取整个 pull 共用的这一份；轮内才见到的 id（新入库，
    /// 或快照之后外部写入的行）由合并路径按 (0,0) 保守补记**且不覆盖已有值**
    /// （见 sync/entries.rs 的 upsert_remote_entry）。
    pub id_to_versions: HashMap<i64, (i64, i64)>,
    /// feed remote_id → feed id
    pub feed_mf_to_id: HashMap<i64, i64>,
    /// article remote_id → article id
    pub mf_id_to_article: HashMap<i64, i64>,
}

impl SyncMatchMaps {
    /// OPT-007：失败后按 DB 现状**重建映射**时，字段版本快照保持本轮原值
    /// （粘滞，不回刷现场版本）。
    ///
    /// 为什么不能直接用重建时的当前版本：快照的语义是「pull 起点（或轮内新入库
    /// 时刻）的版本」。合并中途失败后继续本轮的后续分块/对账时，若把快照刷新为
    /// 重建时点版本，则「pull 起点 ~ 重建点」之间发生的本地写入（即使随后即时
    /// 推送成功、队列已清）会被新快照当成「起点就是这样」，后续旧远端状态即可
    /// 把用户意图回滚——与「快照必须在任何 HTTP 之前抓」是同一不变量的两个面。
    ///
    /// 重建才见到的 id（本轮新入库或外部写入在快照之后）：按 (0,0) 保守处理——
    /// 与 upsert_remote_entry 的补记口径一致；该行若确有本地写入（版本 > 0）
    /// 会被守卫拒绝，宁可跳过远端合并也不冒回滚风险。
    ///
    /// R1 起同时**携带本轮已落定的已读集合**（explicit_read_ids）：重建发生在
    /// 同一轮中途（失败分块之后），已学到的「该行本轮已读」必须继续对后续分块
    /// 的 own unread 合并生效，否则顺序确定性会在重建点断裂。
    pub fn keep_field_versions_from(&mut self, previous: &SyncMatchMaps) {
        for (aid, version) in self.id_to_versions.iter_mut() {
            *version = previous.id_to_versions.get(aid).copied().unwrap_or((0, 0));
        }
        for aid in previous.explicit_read_ids.iter() {
            self.explicit_read_ids.insert(*aid);
        }
    }
}

/// 一次批量查询构建 Pull 合并所需的全部匹配映射。
pub fn sync_match_maps(conn: &Connection) -> AppResult<SyncMatchMaps> {
    // 1. URL 映射族（全局最小 id / 同 feed 最小 id / 全量行集）
    let mut url_to_id = HashMap::new();
    let mut feed_url_to_id = HashMap::new();
    let mut url_norm_to_ids: HashMap<String, Vec<i64>> = HashMap::new();
    {
        // ORDER BY id：同 URL 多篇时「最小 id」口径（or_insert 保留首见，
        // 首见即最小 id）；全量行集按 id 升序天然有序。
        let mut stmt = conn.prepare(
            "SELECT feed_id, url_norm, id FROM articles WHERE url_norm IS NOT NULL ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (feed_id, url_norm, id) = row?;
            url_to_id.entry(url_norm.clone()).or_insert(id);
            feed_url_to_id
                .entry((feed_id, url_norm.clone()))
                .or_insert(id);
            url_norm_to_ids.entry(url_norm).or_default().push(id);
        }
    }

    // 2. id → remote_id（含 None 的也要，用 Option 区分「未绑定」与「不存在」）
    let mut id_to_mf_id: HashMap<i64, Option<i64>> = HashMap::new();
    // 3. id → 所属 feed 的 remote_id（信任复核用，见字段注释）
    let mut id_to_feed_mf: HashMap<i64, Option<i64>> = HashMap::new();
    // 6. mf_id → article id（仅已绑定的）
    let mut mf_id_to_article: HashMap<i64, i64> = HashMap::new();
    // 7. id → (read_version, star_version)：pull 起点字段版本快照（OPT-007）
    let mut id_to_versions: HashMap<i64, (i64, i64)> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT a.id, a.remote_id, f.remote_id, a.read_version, a.star_version FROM articles a
             JOIN feeds f ON f.id = a.feed_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<i64>>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?;
        for row in rows {
            let (id, mf_id, feed_mf_id, read_version, star_version) = row?;
            id_to_mf_id.insert(id, mf_id);
            id_to_feed_mf.insert(id, feed_mf_id);
            id_to_versions.insert(id, (read_version, star_version));
            if let Some(mf) = mf_id {
                mf_id_to_article.insert(mf, id);
            }
        }
    }

    // 4. pending sync 的 article id 集合（并集 + 字段粒度）
    let mut pending_ids = HashSet::new();
    let mut pending_read_ids = HashSet::new();
    let mut pending_star_ids = HashSet::new();
    {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT article_id, action FROM sync_queue WHERE action IN ('read','unread','star','unstar')",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (aid, action) = row?;
            pending_ids.insert(aid);
            match action.as_str() {
                "read" | "unread" => {
                    pending_read_ids.insert(aid);
                }
                _ => {
                    pending_star_ids.insert(aid);
                }
            }
        }
    }

    // 5. feed mf_id → feed id
    let mut feed_mf_to_id = HashMap::new();
    {
        let mut stmt =
            conn.prepare("SELECT remote_id, id FROM feeds WHERE remote_id IS NOT NULL")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
        for row in rows {
            let (mf_id, id) = row?;
            feed_mf_to_id.insert(mf_id, id);
        }
    }

    Ok(SyncMatchMaps {
        url_to_id,
        feed_url_to_id,
        url_norm_to_ids,
        id_to_mf_id,
        id_to_feed_mf,
        pending_ids,
        pending_read_ids,
        pending_star_ids,
        explicit_read_ids: HashSet::new(),
        id_to_versions,
        feed_mf_to_id,
        mf_id_to_article,
    })
}

/// 绑定 Miniflux entry id（Pull 时首次见到该条目）。
///
/// R3：返回**绑定值是否实际变化**（既有行由 NULL/旧 id 变为新 id → true；
/// 已等于目标值/行不存在 → false，且不再产生同值 UPDATE）。数据变化标记消费
/// 该返回值——绑定变化是可见数据变化的一部分（旧调用方丢弃 bool 即可）。
/// 写入本身即判定：WHERE 的 `remote_id IS NOT ?1` 只在值确实不同时命中一行
/// （同时覆盖 NULL 与旧值两种形态），无需额外读一次现值。
pub fn set_article_remote_id(conn: &Connection, id: i64, remote_id: i64) -> AppResult<bool> {
    let n = conn.execute(
        "UPDATE articles SET remote_id = ?1 WHERE id = ?2 AND (remote_id IS NOT ?1)",
        params![remote_id, id],
    )?;
    Ok(n > 0)
}

/// 记账服务端同文副本 entry（跨源同 URL 的另一条 entry）。
/// 幂等：已在列表中不重复；上限 16 个防脏数据撑爆字段。
pub fn add_article_dup_entry(conn: &Connection, id: i64, dup_entry_id: i64) -> AppResult<()> {
    if dup_entry_id <= 0 {
        return Ok(());
    }
    let cur: String = conn
        .query_row(
            "SELECT COALESCE(remote_dup_ids, '') FROM articles WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or_default();
    let ids: Vec<i64> = cur
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    if ids.contains(&dup_entry_id) || ids.len() >= 16 {
        return Ok(());
    }
    let mut next = ids;
    next.push(dup_entry_id);
    let joined = next
        .iter()
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(",");
    conn.execute(
        "UPDATE articles SET remote_dup_ids = ?1 WHERE id = ?2",
        params![joined, id],
    )?;
    Ok(())
}

/// 读某文章的副本 entry 列表（广播已读/收藏用）
pub fn article_dup_entries(conn: &Connection, id: i64) -> AppResult<Vec<i64>> {
    let cur: Option<String> = conn
        .query_row(
            "SELECT remote_dup_ids FROM articles WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(cur
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect())
}

/// 已读广播的推送目标集（OPT-008B 第一阶段消费点：push::plan_push 的 read 分支）。
///
/// 组成（选择规则显式化，不依赖“入库被吞”）：
/// - **同文兄弟条目**：`url_norm` 相等的其它文章行、已绑定 remote_id 的——
///   跨源同 URL 的副本现在是各自独立的持久条目（各源可达、收藏独立），标读的
///   跨源传播由本集合承载；不区分布局（布局是呈现维度，与本政策无关）。
/// - **旧 CSV 记账**（remote_dup_ids，有限兼容）：远端 feed 未绑定、无法成行的
///   历史副本 entry id 仍覆盖；full 重拉按真实 feed 补行后自然走兄弟集合。
///
/// 返回自身之外的 remote_id 去重集合（调用方另行放入自身的 remote_id）。
pub fn read_broadcast_targets(conn: &Connection, article_id: i64) -> AppResult<Vec<i64>> {
    let mut out: Vec<i64> = Vec::new();
    let url_norm: Option<String> = conn
        .query_row(
            "SELECT url_norm FROM articles WHERE id = ?1",
            params![article_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    if let Some(norm) = url_norm {
        let mut stmt = conn.prepare(
            "SELECT remote_id FROM articles
             WHERE url_norm = ?1 AND id != ?2 AND remote_id IS NOT NULL",
        )?;
        let rows = stmt.query_map(params![norm, article_id], |r| r.get::<_, i64>(0))?;
        out.extend(rows.collect::<Result<Vec<_>, _>>()?);
    }
    out.extend(article_dup_entries(conn, article_id)?);
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/* ============================================================
同步代际（OPT-006 / F05、F06；内部 settings，无 schema 迁移）

每个数据库独立的 `sync_generation`：账号配置提交（sync_save）与断开
（sync_disconnect）在同一事务里 +1；同步会话在同一个短 DB 临界区里
捕获「凭据 + 代际」，此后每个 HTTP→DB 回写边界在**同一次持锁内**复核
代际——不等 = 账号已变更，旧响应一律不落库（不复活旧绑定/游标/队列）。
代际缺失/非法按 0 处理（旧库首次升级即 0，第一次账号提交推进到 1）。
============================================================ */

const SYNC_GENERATION_KEY: &str = "sync_generation";

/// 读当前同步代际（每库独立）。
///
/// R2 三态收紧：**只有「键缺失」= 0**（旧库未初始化，第一次账号提交推进到 1）；
/// 键存在但为空串/空白/非法/负数/溢出一律 `Err`——「有键但不可解析」是状态损坏，
/// 按 0 会与旧代际 0 的会话假匹配，也绝不能在 `bump` 里被顺手覆写掩盖。
pub fn sync_generation(conn: &Connection) -> AppResult<i64> {
    let Some(raw) = get_setting(conn, SYNC_GENERATION_KEY)? else {
        return Ok(0);
    };
    match raw.trim().parse::<i64>() {
        Ok(v) if v >= 0 => Ok(v),
        _ => Err(crate::error::AppError::new(
            "protocol",
            format!("同步代际损坏（{SYNC_GENERATION_KEY}={raw:?}）：拒绝继续，请排查/修复后重试"),
        )),
    }
}

/// 推进代际并返回新值（调用方须已处于账号提交事务内）。
/// R2：`checked_add`——到 i64::MAX 时 Err（不饱和复用同一代际），且不改写现场。
pub fn bump_sync_generation(conn: &Connection) -> AppResult<i64> {
    let next = sync_generation(conn)?.checked_add(1).ok_or_else(|| {
        crate::error::AppError::new(
            "protocol",
            format!("同步代际溢出（{SYNC_GENERATION_KEY} 已达 i64::MAX）：拒绝继续"),
        )
    })?;
    set_setting(conn, SYNC_GENERATION_KEY, &next.to_string())?;
    Ok(next)
}

/// 记录上次同步时间戳（Pull 增量游标，unix 秒）
pub fn last_sync_ts(conn: &Connection) -> AppResult<i64> {
    let v: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'sync_last_sync'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    Ok(v.and_then(|s| s.parse().ok()).unwrap_or(0))
}

pub fn set_last_sync_ts(conn: &Connection, ts: i64) -> AppResult<()> {
    set_setting(conn, "sync_last_sync", &ts.to_string())
}

/// Fever 协议增量游标：上次同步拉到的最大条目 id（`since_id` 分页用）。
pub fn last_sync_entry_id(conn: &Connection) -> AppResult<i64> {
    let v: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'sync_last_entry_id'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    Ok(v.and_then(|s| s.parse().ok()).unwrap_or(0))
}

pub fn set_last_sync_entry_id(conn: &Connection, id: i64) -> AppResult<()> {
    set_setting(conn, "sync_last_entry_id", &id.to_string())
}

/* ============================================================
Fever 历史状态（OPT-005 R2；内部 settings，无 schema 迁移）

显式三态（`fever_history_state` 键，单键原子承载状态+游标）：
- 缺失/空串                       = Unknown：**绝不当完成**（旧库只有 since、
  或从未初始化）；同步据此启动回溯（从顶部）。
- `pending:<max_id>`（1..=i64::MAX）= Pending：历史未完成，下一次 `items&max_id`
  从该游标继续；页事务把页数据与它同 commit。
- `complete`                       = Complete：历史已取尽。
其余取值 = **损坏 → Err**（显式报错，不擅自当完成、不覆盖现场）。

与 `sync_last_entry_id`（只由连续成功范围推进）互不越权：续取旧历史不推动
增量游标；初始化 Pending 成功先于任何更大的 since 落库。
============================================================ */

const FEVER_HISTORY_STATE_KEY: &str = "fever_history_state";
const FEVER_HISTORY_STATE_COMPLETE: &str = "complete";

/// Fever 历史状态（见上方三态语义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeverHistoryState {
    /// 键缺失/空：状态未知，**绝不当完成**。
    Unknown,
    /// 未完成：下一次 max_id 从该游标继续。
    Pending(i64),
    /// 已取尽。
    Complete,
}

/// 读 Fever 历史状态；损坏值返回显式错误（调用方不得继续、不得覆盖）。
// Note: Unknown/Pending/Complete 与 checkpoint 同 commit 的语义 — 见 .agents/notes/implemented/architecture/2026-10-08-Fever身份与历史回溯.md
pub fn fever_history_state(conn: &Connection) -> AppResult<FeverHistoryState> {
    let raw = get_setting(conn, FEVER_HISTORY_STATE_KEY)?;
    let Some(raw) = raw else {
        return Ok(FeverHistoryState::Unknown);
    };
    let t = raw.trim();
    if t.is_empty() {
        return Ok(FeverHistoryState::Unknown);
    }
    if t == FEVER_HISTORY_STATE_COMPLETE {
        return Ok(FeverHistoryState::Complete);
    }
    if let Some(rest) = t.strip_prefix("pending:") {
        if let Ok(v) = rest.trim().parse::<i64>() {
            if v >= 1 {
                return Ok(FeverHistoryState::Pending(v));
            }
        }
    }
    Err(crate::error::AppError::new(
        "protocol",
        format!(
            "Fever 历史状态损坏（{FEVER_HISTORY_STATE_KEY}={raw:?}）：不擅自当完成，请排查/修复"
        ),
    ))
}

/// 写入 Pending（未完成）状态与续取游标。
pub fn set_fever_history_pending(conn: &Connection, max_id: i64) -> AppResult<()> {
    set_setting(conn, FEVER_HISTORY_STATE_KEY, &format!("pending:{max_id}"))
}

/// 历史已取尽：写 Complete（只有在真正走完空页后允许）。
pub fn set_fever_history_complete(conn: &Connection) -> AppResult<()> {
    set_setting(conn, FEVER_HISTORY_STATE_KEY, FEVER_HISTORY_STATE_COMPLETE)
}

/// 重置历史状态为 Unknown（OPT-006：断开/换号同一事务内调用）。
/// 写空串而非删除行：`fever_history_state` 读侧把空串当 Unknown（见三态语义），
/// 显式写空让「刚被账号事件重置」与「从未初始化」在同步行为上一致（都启动回溯），
/// 同时保留一行可审计记录。
pub fn reset_fever_history_state(conn: &Connection) -> AppResult<()> {
    set_setting(conn, FEVER_HISTORY_STATE_KEY, "")
}

/// feeds 的 remote_id 绑定
pub fn set_feed_remote_id(conn: &Connection, feed_id: i64, remote_id: i64) -> AppResult<()> {
    conn.execute(
        "UPDATE feeds SET remote_id = ?1 WHERE id = ?2",
        params![remote_id, feed_id],
    )?;
    Ok(())
}

/// 按 URL 找本地 feed（首次同步的碰撞检测键）
pub fn feed_id_by_url(conn: &Connection, feed_url: &str) -> AppResult<Option<i64>> {
    let id = conn
        .query_row(
            "SELECT id FROM feeds WHERE feed_url = ?1",
            params![feed_url],
            |r| r.get(0),
        )
        .optional()?;
    Ok(id)
}

/// 按 URL 检查 feed 是否存在（OPML 导入去重用）
pub fn feed_exists_by_url(conn: &Connection, feed_url: &str) -> AppResult<bool> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM feeds WHERE feed_url = ?1)",
        params![feed_url],
        |r| r.get(0),
    )?;
    Ok(exists)
}

/// 按规范化 URL 找本地 feed（与条目侧的规范化匹配同口径）。
/// 用于 pull_feeds 与 Miniflux 的 feed_url 碰撞匹配：Miniflux 返回的 URL 与
/// 本地直连添加时的 URL 常有协议/www./尾斜杠/跟踪参数差异，精确匹配会漏判成
/// 新订阅 → 同一订阅出现两个本地 feed（文章翻倍、状态分裂、数量与未读数
/// 对不齐的根因）。feeds 数量少（个人订阅几十个），内存规范化匹配即可。
pub fn feed_id_by_url_normalized(conn: &Connection, feed_url: &str) -> AppResult<Option<i64>> {
    let norm = normalize_url(feed_url);
    let feeds = list_feeds(conn)?;
    Ok(feeds
        .iter()
        .find(|f| normalize_url(&f.feed_url) == norm)
        .map(|f| f.id))
}

/* ============================================================
commands.rs SQL 抽取层（TASK-017）
============================================================ */

/// 确保「未分类」folder 存在：已有则返回其 id，无则创建后返回。
/// 用于 add_feed 未指定分类时的兜底逻辑。
pub fn ensure_uncategorized_folder(conn: &Connection) -> AppResult<i64> {
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM folders WHERE name = '未分类' ORDER BY id LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    match existing {
        Some(fid) => Ok(fid),
        None => create_folder(conn, "未分类", "article"),
    }
}

/// 检查 folder 是否存在（update_feed 参数校验用）
pub fn folder_exists(conn: &Connection, folder_id: i64) -> AppResult<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM folders WHERE id = ?1",
        params![folder_id],
        |r| r.get(0),
    )?;
    Ok(count > 0)
}

/// 列出指定范围内的未读文章 id（范围筛选一致性测试仍消费；「全部已读」的
/// 入队已集合化，不再经本函数逐 id 往返）。
/// feed_id/folder_id 为 None 时查全部未读。范围子句与 db::mark_all_read 同源
/// （scope_clause）。
///
/// OPT-008B R1 边界：本函数返回**范围未读行**（不含同组闭包）——「全部已读」
/// 的读目标集合 = 范围未读 ∪ 同组闭包，见
/// [`super::articles::list_read_target_ids_scoped`]；显示去重不参与操作集合
/// （操作不得只改代表）。
pub fn list_unread_ids_scoped(
    conn: &Connection,
    feed_id: Option<i64>,
    folder_id: Option<i64>,
    starred_only: bool,
    since_ms: Option<i64>,
    layout: Option<&str>,
) -> AppResult<Vec<i64>> {
    let (clause, binds) =
        super::articles::scope_clause(feed_id, folder_id, starred_only, since_ms, layout);
    let sql = format!("SELECT a.id FROM articles a WHERE a.is_read = 0{clause}");
    let refs: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(refs.as_slice(), |r| r.get(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 获取文章 URL（extract_fulltext 取原文网页地址用）
pub fn get_article_url(conn: &Connection, article_id: i64) -> AppResult<Option<String>> {
    let url = conn
        .query_row(
            "SELECT url FROM articles WHERE id = ?1",
            params![article_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    Ok(url)
}

/// 获取文章正文 HTML（extract_fulltext 退化判定用）。
/// OPT-008B 第二阶段：正文真值源 = contents（单一正文所有权）。
pub fn get_article_content_html(conn: &Connection, article_id: i64) -> AppResult<String> {
    let html: String = conn
        .query_row(
            "SELECT COALESCE(c.content_html, '')
             FROM articles a LEFT JOIN contents c ON c.id = a.content_id
             WHERE a.id = ?1",
            params![article_id],
            |r| r.get(0),
        )
        .unwrap_or_default();
    Ok(html)
}

/// 更新文章全文提取结果（**兼容包装**）：OPT-008B 第二阶段起正文由 contents
/// 承载，提取落库走带前提校验的 [`super::articles::replace_article_content_checked`]
/// （迟到提交不得写已删除/已分叉/已替换内容）。本函数保留为「无前提的显式覆盖」
/// 供非网络路径（夹具/测试）使用，语义 = 立即读取当前前提后提交一次。
pub fn update_article_fulltext(
    conn: &Connection,
    article_id: i64,
    content_html: &str,
    _extracted: bool,
) -> AppResult<()> {
    let Some((token, _html)) = super::articles::article_content_commit_token(conn, article_id)?
    else {
        return Ok(());
    };
    // R3：提交返回 Option<(content_id, revision)>；本兼容包装不消费身份（夹具/
    // 非网络路径），投递失败（前提已变）时静默返回与旧 `let _` 语义一致。
    let _committed =
        super::articles::replace_article_content_checked(conn, article_id, &token, content_html)?;
    Ok(())
}

/// 更新文章封面（仅在现有封面为空时）：extract_fulltext 头图兜底用
pub fn update_article_image_if_empty(
    conn: &Connection,
    article_id: i64,
    image_url: &str,
) -> AppResult<()> {
    conn.execute(
        "UPDATE articles SET image_url = COALESCE(image_url, ?1) WHERE id = ?2",
        params![image_url, article_id],
    )?;
    Ok(())
}

/// 导出全部 feeds 附带 folder 名（OPML 导出用）
pub fn export_feeds_with_folders(
    conn: &Connection,
) -> AppResult<Vec<(String, String, Option<String>)>> {
    let mut stmt = conn.prepare(
        "SELECT f.title, f.feed_url, fo.name
         FROM feeds f LEFT JOIN folders fo ON f.folder_id = fo.id
         ORDER BY fo.name, f.title",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
        ))
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 统计未绑定的本地源数量（sync_save 首连判定用）
pub fn count_unbound_local_feeds(conn: &Connection) -> AppResult<i64> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM feeds WHERE origin = 'local' AND remote_id IS NULL",
        [],
        |r| r.get(0),
    )?;
    Ok(count)
}

/// 列出未绑定的本地源（sync_local_feeds 推送用）
pub fn list_unbound_local_feeds(conn: &Connection) -> AppResult<Vec<(i64, String, Option<i64>)>> {
    let mut stmt = conn.prepare(
        "SELECT f.id, f.feed_url, f.folder_id FROM feeds f
         WHERE f.origin = 'local' AND f.remote_id IS NULL",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 获取文章摘要用数据（ai_summarize 用）：(title, body_text, ai_summary)。
/// OPT-008B 第二阶段：正文纯文本来自 contents；输入键 = (title, body_text)
/// ——标题不同的条目即便共享正文也不共用产物（产物仍存条目行）。
pub fn get_article_for_summary(
    conn: &Connection,
    article_id: i64,
) -> AppResult<Option<(String, String, Option<String>)>> {
    let row = conn
        .query_row(
            "SELECT a.title, COALESCE(c.body_text, ''), a.ai_summary
             FROM articles a LEFT JOIN contents c ON c.id = a.content_id
             WHERE a.id = ?1",
            params![article_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()?;
    Ok(row)
}

/// 获取文章翻译用数据（ai_translate 用）：(title, content_html, translated_content)。
/// OPT-008B 第二阶段：正文 HTML 来自 contents；输入键 = (title, content_html, lang)。
pub fn get_article_for_translation(
    conn: &Connection,
    article_id: i64,
) -> AppResult<Option<(String, String, Option<String>)>> {
    let row = conn
        .query_row(
            "SELECT a.title, COALESCE(c.content_html, ''), a.translated_content
             FROM articles a LEFT JOIN contents c ON c.id = a.content_id
             WHERE a.id = ?1",
            params![article_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()?;
    Ok(row)
}

/* ============================================================
sync.rs SQL 抽取层（TASK-022）
============================================================ */

/// 获取文章的 remote_id（Push 段检查绑定用）
pub fn get_article_remote_id(conn: &Connection, article_id: i64) -> AppResult<Option<i64>> {
    let remote_id = conn
        .query_row(
            "SELECT remote_id FROM articles WHERE id = ?1",
            params![article_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(remote_id.flatten())
}

/// 按名称查询 folder id（Pull 段分类匹配用）
pub fn find_folder_by_name(conn: &Connection, name: &str) -> AppResult<Option<i64>> {
    let id = conn
        .query_row(
            "SELECT id FROM folders WHERE name = ?1",
            params![name],
            |r| r.get(0),
        )
        .optional()?;
    Ok(id)
}

/// 获取第一个 folder id（Pull 段兜底分类用）
///
/// TASK-070：本函数零生产调用（生产已改用 ensure_uncategorized_folder），
/// 但其唯一引用是 crate 内测试 db/sync_extraction_tests.rs（不在本任务
/// allowed_paths 内），故按范围约束保留，待后续任务连同该测试一并处置。
pub fn get_first_folder_id(conn: &Connection) -> AppResult<Option<i64>> {
    let id = conn
        .query_row("SELECT id FROM folders LIMIT 1", [], |r| r.get(0))
        .optional()?;
    Ok(id)
}

/// 更新 feed 标题和站点 URL（仅当本地标题等于 feed_url 时，Pull 段远端标题回填用）
pub fn update_feed_title_if_empty(
    conn: &Connection,
    feed_id: i64,
    title: &str,
    site_url: Option<&str>,
) -> AppResult<()> {
    conn.execute(
        "UPDATE feeds SET
            title = CASE WHEN title = feed_url THEN ?1 ELSE title END,
            site_url = COALESCE(site_url, ?2)
         WHERE id = ?3",
        params![title, site_url, feed_id],
    )?;
    Ok(())
}

/// 同步专用：无条件更新文章已读和收藏状态。
///
/// R2 起**只允许用于同一 DB 临界区内刚插入的新行**（`upsert_remote_entry` 的
/// inserted=true 分支：行不存在→插入→写状态之间不可能有本地写入，快照 (0,0)
/// 与行初值恒等，整值写不覆盖任何用户意图）。既有行的 pull 状态合并必须逐字段
/// 过版本/pending 守卫，走下方带版本条件的原语——直接对既有行调用本函数会
/// 回滚拉取期间用户的标读/收藏。
pub fn sync_set_article_status(
    conn: &Connection,
    article_id: i64,
    is_read: bool,
    is_starred: bool,
) -> AppResult<()> {
    conn.execute(
        "UPDATE articles SET is_read = ?1, is_starred = ?2 WHERE id = ?3",
        params![is_read as i64, is_starred as i64, article_id],
    )?;
    Ok(())
}

/// 同步专用：标记文章为已读（仅当未读时），返回影响行数（Pull 段对账计数用）
pub fn sync_mark_read_if_unread(conn: &Connection, article_id: i64) -> AppResult<usize> {
    let n = conn.execute(
        "UPDATE articles SET is_read = 1 WHERE id = ?1 AND is_read = 0",
        params![article_id],
    )?;
    Ok(n)
}

/// 同步专用：标记文章为未读（仅当已读时），返回影响行数
pub fn sync_mark_unread_if_read(conn: &Connection, article_id: i64) -> AppResult<usize> {
    let n = conn.execute(
        "UPDATE articles SET is_read = 0 WHERE id = ?1 AND is_read = 1",
        params![article_id],
    )?;
    Ok(n)
}

/// 同步专用：标记文章为收藏（仅当未收藏时），返回影响行数
pub fn sync_mark_starred_if_unstarred(conn: &Connection, article_id: i64) -> AppResult<usize> {
    let n = conn.execute(
        "UPDATE articles SET is_starred = 1 WHERE id = ?1 AND is_starred = 0",
        params![article_id],
    )?;
    Ok(n)
}

/// 同步专用：取消文章收藏（仅当已收藏时），返回影响行数
pub fn sync_mark_unstarred_if_starred(conn: &Connection, article_id: i64) -> AppResult<usize> {
    let n = conn.execute(
        "UPDATE articles SET is_starred = 0 WHERE id = ?1 AND is_starred = 1",
        params![article_id],
    )?;
    Ok(n)
}

/* ============================================================
OPT-007（F09/F10）：远端合并的字段版本守卫原语

与上方无版本条件版本的分工：上方是「不带版本」的通用条件写——其中
`sync_set_article_status` 仍有一处生产消费（R2 起仅限同一 DB 临界区内刚插入
的新行：快照 (0,0) 与行初值恒等，见 sync/entries.rs 的 upsert_remote_entry），
其余四个为测试与历史消费点。**既有行**的 pull 状态合并（轻量对账 +
merge_remote_status + upsert 命中既有行）唯一生产写路径是下方带版本条件的
原语——每条 WHERE 都额外要求该字段版本仍等于 pull 起点快照。版本不符 =
本地在拉取期间写过该字段（本地写入在同一事务里递增版本），本次远端快照对
该字段作废（0 行）；**远端合并不递增版本**（版本只记录本地意图，远端确认
不产生新意图）。
============================================================ */

/// 读文章的 (read_version, star_version)；行不存在返回 None。
pub fn article_field_versions(conn: &Connection, article_id: i64) -> AppResult<Option<(i64, i64)>> {
    let v = conn
        .query_row(
            "SELECT read_version, star_version FROM articles WHERE id = ?1",
            params![article_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(v)
}

/// 远端合并（带版本条件）：未读 → 已读；本地版本已变 → 0 行。
pub fn sync_mark_read_if_unread_at_version(
    conn: &Connection,
    article_id: i64,
    read_version: i64,
) -> AppResult<usize> {
    let n = conn.execute(
        "UPDATE articles SET is_read = 1 WHERE id = ?1 AND is_read = 0 AND read_version = ?2",
        params![article_id, read_version],
    )?;
    Ok(n)
}

/// 远端合并（带版本条件）：已读 → 未读；本地版本已变 → 0 行。
pub fn sync_mark_unread_if_read_at_version(
    conn: &Connection,
    article_id: i64,
    read_version: i64,
) -> AppResult<usize> {
    let n = conn.execute(
        "UPDATE articles SET is_read = 0 WHERE id = ?1 AND is_read = 1 AND read_version = ?2",
        params![article_id, read_version],
    )?;
    Ok(n)
}

/// 远端合并（带版本条件）：未收藏 → 收藏；本地版本已变 → 0 行。
pub fn sync_mark_starred_if_unstarred_at_version(
    conn: &Connection,
    article_id: i64,
    star_version: i64,
) -> AppResult<usize> {
    let n = conn.execute(
        "UPDATE articles SET is_starred = 1 WHERE id = ?1 AND is_starred = 0 AND star_version = ?2",
        params![article_id, star_version],
    )?;
    Ok(n)
}

/// 远端合并（带版本条件）：已收藏 → 取消收藏；本地版本已变 → 0 行。
pub fn sync_mark_unstarred_if_starred_at_version(
    conn: &Connection,
    article_id: i64,
    star_version: i64,
) -> AppResult<usize> {
    let n = conn.execute(
        "UPDATE articles SET is_starred = 0 WHERE id = ?1 AND is_starred = 1 AND star_version = ?2",
        params![article_id, star_version],
    )?;
    Ok(n)
}

/// 远端合并（带版本条件）：整值写读状态（GR 条目的 read=false 侧与
/// accept-unread 路径）；本地版本已变 → 0 行。
pub fn sync_set_read_at_version(
    conn: &Connection,
    article_id: i64,
    read: bool,
    read_version: i64,
) -> AppResult<usize> {
    let n = conn.execute(
        "UPDATE articles SET is_read = ?1 WHERE id = ?2 AND read_version = ?3",
        params![read as i64, article_id, read_version],
    )?;
    Ok(n)
}

/// 远端合并（带版本条件）：整值写收藏状态；本地版本已变 → 0 行。
pub fn sync_set_star_at_version(
    conn: &Connection,
    article_id: i64,
    starred: bool,
    star_version: i64,
) -> AppResult<usize> {
    let n = conn.execute(
        "UPDATE articles SET is_starred = ?1 WHERE id = ?2 AND star_version = ?3",
        params![starred as i64, article_id, star_version],
    )?;
    Ok(n)
}

/// 回填涉及的可见字段快照（R3 前后比对；文本经 `null_if_empty` 归一）。
/// OPT-008B 第二阶段：正文两列从 contents JOIN 读；content_id 一并参与比对
/// （内容等价则身份不变——重指也计变化，驱动数据变化标记）。
/// R3：来源/提取标志只读不参与比对——回填的提取保护判定（readability 行不得
/// 被合成 RSS 覆盖）需要它们。
#[derive(Debug, PartialEq, Eq)]
struct FillVisible {
    content_html: Option<String>,
    body_text: Option<String>,
    image_url: Option<String>,
    enclosure_url: Option<String>,
    enclosure_mime: Option<String>,
    content_id: Option<i64>,
    /// 当前指向内容的来源（'rss' | 'readability' | ''=无内容行）
    source: String,
    /// 当前指向内容是否已全文提取（受保护提取行判定）
    fulltext_extracted: i64,
}

/// 读一行的 (url, 回填可见字段)；行不存在返回 None。读取失败透传 Err。
fn fill_visible(
    conn: &Connection,
    article_id: i64,
) -> AppResult<Option<(Option<String>, FillVisible)>> {
    let row = conn
        .query_row(
            "SELECT a.url, c.content_html, c.body_text, a.image_url, a.enclosure_url, a.enclosure_mime, a.content_id,
                    COALESCE(c.extraction_source, ''), COALESCE(c.fulltext_extracted, 0)
             FROM articles a LEFT JOIN contents c ON c.id = a.content_id
             WHERE a.id = ?1",
            params![article_id],
            |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    FillVisible {
                        content_html: null_if_empty(r.get(1)?),
                        body_text: null_if_empty(r.get(2)?),
                        image_url: null_if_empty(r.get(3)?),
                        enclosure_url: null_if_empty(r.get(4)?),
                        enclosure_mime: null_if_empty(r.get(5)?),
                        content_id: r.get(6)?,
                        source: r.get(7)?,
                        fulltext_extracted: r.get(8)?,
                    },
                ))
            },
        )
        .optional()?;
    Ok(row)
}

/// 同步专用：回填文章内容字段（本地为空才补，Pull 段正文兜底用）。
///
/// F07：补全的 HTML 必须与首次入库走同一净化边界——此前原始 HTML 直接补入空
/// content_html，绕过了 sanitizer 并一路抵达 dangerouslySetInnerHTML。
/// 基址取该文章的 url（与抓取入库的相对链接重写口径一致）；DB 错误正常上抛。
///
/// R3：返回**本次调用是否实际改变了数据**（UPDATE 前后可见字段比对，空串≡NULL；
/// 同值重复回填/本地已有内容不覆盖 → false）。消费点以此驱动数据变化标记，
/// 不允许拿 UPDATE 的 changes()（同值也计命中行）冒充变化；读取失败 `?` 上抛，
/// 不得吞成「无变化」。行在调用时已被删除（拉取期间的删除竞态）→ Ok(false)：
/// 没有可回填的对象，不是读取失败。
///
/// OPT-008B 第二阶段：正文回填按「逐字段空才补」合成目标内容，经 contents
/// 身份归并（等价即复用——跨源同文共享；不把内容泄写进共享行的整值覆盖）。
/// OPT-008B R3：当前指向**受保护提取行**（readability + 已提取）时不做 RSS
/// 合成——就地修复异常 body（来源/标志保持 readability），防止历史「html 非空/
/// body 缺失」行被降级成 rss 行而丢 fulltext_extracted（隔离回归见
/// sync_extraction_tests::backfill_keeps_protected_readability_source）。
// Note: 可渲染 HTML 的净化口径只有 sanitize()（写入口与读边界共用） — 见 .agents/notes/implemented/architecture/2026-10-08-可渲染正文安全边界.md
pub fn backfill_article_content(
    conn: &Connection,
    article_id: i64,
    content_html: &str,
    body_text: &str,
    image_url: Option<&str>,
    enclosure_url: Option<&str>,
    enclosure_mime: Option<&str>,
) -> AppResult<bool> {
    let Some((base, before)) = fill_visible(conn, article_id)? else {
        return Ok(false);
    };
    let cur_html = before.content_html.clone().unwrap_or_default();
    let cur_body = before.body_text.clone().unwrap_or_default();
    /* R3（历史异常 Readability 保护）：当前指向的是受保护提取行（readability +
    fulltext_extracted）时，回填不得合成 `(提取 HTML, RSS 纯文本)` 的 **rss 源**
    新行——那会把提取标志丢掉、把受保护行降级成 RSS 摘要（常规提取会被防退化
    拦截，仅「html 非空但 body 缺失/错配」的历史异常行可达）。语义与 RSS 刷新
    保护一致：就地修复 body（html_to_text 同源重算，revision+1），来源与标志
    保持 readability。 */
    let protected =
        before.source == super::contents::SOURCE_READABILITY && before.fulltext_extracted != 0;
    if protected {
        if let Some(cid) = before.content_id {
            super::contents::repair_extracted_body_if_mismatch(conn, cid)?;
        }
    } else {
        // 逐字段空才补（旧语义），再按合成内容归并到 contents。
        let fill_html = cur_html.is_empty() && !content_html.trim().is_empty();
        let fill_body = cur_body.is_empty() && !body_text.is_empty();
        if (fill_html || fill_body)
            && super::contents::content_non_empty(
                Some(if fill_html { content_html } else { &cur_html }),
                if fill_body { body_text } else { &cur_body },
            )
        {
            let merged_html = if fill_html {
                crate::sanitize::sanitize(content_html, base.as_deref())
            } else {
                cur_html.clone()
            };
            let input = super::contents::ContentInput {
                content_html: Some(merged_html).filter(|h| !h.is_empty()),
                body_text: if fill_body {
                    body_text.to_string()
                } else {
                    cur_body.clone()
                },
                extraction_source: super::contents::SOURCE_RSS.to_string(),
                fulltext_extracted: false,
                url_norm: None,
            };
            /* R3（故障原子性）：ensure_content + 重指 + 旧行回收同 SAVEPOINT——
            失败整体回滚，不留无引用新行。 */
            super::contents::with_write_savepoint(conn, "contents_backfill_repoint", || {
                let (content_id, _rev, _created) =
                    super::contents::ensure_content(conn, &input, Some(article_id))?;
                if Some(content_id) != before.content_id {
                    conn.execute(
                        "UPDATE articles SET content_id = ?1 WHERE id = ?2",
                        params![content_id, article_id],
                    )?;
                    // 重指后旧行若无引用：回收（最后引用才回收；触发器只在条目删除时兜底）
                    if let Some(old_id) = before.content_id {
                        conn.execute(
                            "DELETE FROM contents WHERE id = ?1
                             AND NOT EXISTS (SELECT 1 FROM articles a WHERE a.content_id = ?1)",
                            params![old_id],
                        )?;
                    }
                }
                Ok(())
            })?;
        }
    }
    conn.execute(
        "UPDATE articles SET
            image_url = COALESCE(image_url, ?1),
            enclosure_url = COALESCE(enclosure_url, ?2),
            enclosure_mime = COALESCE(enclosure_mime, ?3)
         WHERE id = ?4",
        params![image_url, enclosure_url, enclosure_mime, article_id],
    )?;
    // 复读比对（同一连接内行必然还在；读取失败按 R3 纪律上抛 Err）。
    let after = fill_visible(conn, article_id)?
        .map(|(_, f)| f)
        .ok_or_else(|| {
            crate::error::AppError::new(
                "db",
                format!("回填后复读失败：文章 {article_id} 不存在（并发删除？）"),
            )
        })?;
    Ok(before != after)
}

/* ============================================================
测试模块（TASK-017 追加）
============================================================ */

#[cfg(test)]
mod fever_checkpoint_tests {
    use super::*;
    use crate::db::MIGRATIONS;

    fn conn() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        conn
    }

    /// 三态往返与边界（i64::MAX 合法的 Pending）。
    #[test]
    fn fever_history_state_roundtrip() {
        let conn = conn();
        assert_eq!(
            fever_history_state(&conn).unwrap(),
            FeverHistoryState::Unknown,
            "缺省 = Unknown（绝不当完成）"
        );
        set_fever_history_pending(&conn, 76).unwrap();
        assert_eq!(
            fever_history_state(&conn).unwrap(),
            FeverHistoryState::Pending(76)
        );
        set_fever_history_pending(&conn, i64::MAX).unwrap();
        assert_eq!(
            fever_history_state(&conn).unwrap(),
            FeverHistoryState::Pending(i64::MAX)
        );
        set_fever_history_complete(&conn).unwrap();
        assert_eq!(
            fever_history_state(&conn).unwrap(),
            FeverHistoryState::Complete
        );
        set_setting(&conn, "fever_history_state", "   ").unwrap();
        assert_eq!(
            fever_history_state(&conn).unwrap(),
            FeverHistoryState::Unknown,
            "空白视为缺失 = Unknown"
        );
    }

    /// 损坏值必须显式报错（不得当完成、不得静默 reset）。
    #[test]
    fn fever_history_state_corrupt_is_error() {
        let conn = conn();
        for bad in [
            "not-a-state",
            "pending:",
            "pending:0",
            "pending:-9",
            "pending:abc",
            "completed",
            "PENDING:5",
        ] {
            set_setting(&conn, "fever_history_state", bad).unwrap();
            let err = fever_history_state(&conn).expect_err("损坏状态必须报错");
            assert!(
                err.to_string().contains("fever_history_state"),
                "{bad:?} 的错误应带键名：{err}"
            );
        }
    }
}

#[cfg(test)]
mod r3_change_detection_tests {
    use super::*;
    use crate::db::{create_folder, insert_feed, upsert_article_with_feed, NewArticle, MIGRATIONS};

    fn seed(conn: &Connection, guid: &str) -> i64 {
        let folder = create_folder(conn, &format!("F{guid}"), "article").unwrap();
        let feed = insert_feed(
            conn,
            &format!("https://f.example/{guid}.rss"),
            None,
            "f",
            None,
            folder,
            "inherit",
            false,
            false,
        )
        .unwrap();
        let a = NewArticle {
            guid: guid.into(),
            url: Some(format!("https://e.example/{guid}")),
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
        upsert_article_with_feed(conn, feed, &a, false).unwrap().0
    }

    fn conn() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        conn
    }

    /// R3：绑定返回**实际变化**——NULL→新值 true；同值 false；换绑 true；
    /// 行不存在 false（不误报）。DB 值同步核对。
    #[test]
    fn remote_id_binding_reports_actual_change() {
        let conn = conn();
        let aid = seed(&conn, "bind");
        assert!(
            set_article_remote_id(&conn, aid, 77).unwrap(),
            "NULL → 77 是真实变化"
        );
        assert!(
            !set_article_remote_id(&conn, aid, 77).unwrap(),
            "同值重复绑定不得报变化"
        );
        assert!(
            set_article_remote_id(&conn, aid, 88).unwrap(),
            "换绑是真实变化"
        );
        let got: Option<i64> = conn
            .query_row("SELECT remote_id FROM articles WHERE id = ?1", [aid], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(got, Some(88), "换绑后的现场值");
        assert!(
            !set_article_remote_id(&conn, 999_999, 1).unwrap(),
            "行不存在：无可绑定，false"
        );
    }

    /// R3：backfill 只在**真正补进内容**时返回 true——首次补空行 true；
    /// 同值重复 false；本地已有内容不覆盖 false；无可回填内容 false。
    /// 每次断言都同时核对 DB 前后值（不只查 flag）。
    #[test]
    fn backfill_reports_change_only_when_it_fills() {
        let conn = conn();
        let aid = seed(&conn, "fill");
        let before: (Option<String>, String) = conn
            .query_row(
                "SELECT c.content_html, COALESCE(c.body_text, '') FROM articles a LEFT JOIN contents c ON c.id = a.content_id WHERE a.id = ?1",
                [aid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(before, (None, String::new()), "前置：空正文行");

        let first = backfill_article_content(
            &conn,
            aid,
            "<p>远端正文</p>",
            "远端正文",
            Some("https://img.example/i.jpg"),
            Some("https://audio.example/a.mp3"),
            Some("audio/mpeg"),
        )
        .unwrap();
        assert!(first, "空行首次回填必须报变化");
        let after: (String, String, Option<String>) = conn
            .query_row(
                "SELECT COALESCE(c.content_html, ''), COALESCE(c.body_text, ''), a.image_url FROM articles a LEFT JOIN contents c ON c.id = a.content_id WHERE a.id = ?1",
                [aid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            after,
            (
                "<p>远端正文</p>".into(),
                "远端正文".into(),
                Some("https://img.example/i.jpg".into())
            ),
            "首次回填的现场值"
        );

        let again = backfill_article_content(
            &conn,
            aid,
            "<p>远端正文</p>",
            "远端正文",
            Some("https://img.example/i.jpg"),
            Some("https://audio.example/a.mp3"),
            Some("audio/mpeg"),
        )
        .unwrap();
        assert!(!again, "同值重复回填不得再报变化");

        let overwrite_try =
            backfill_article_content(&conn, aid, "<p>换一篇</p>", "换一篇", None, None, None)
                .unwrap();
        assert!(!overwrite_try, "本地已有内容不被覆盖：不是变化");
        let kept: String = conn
            .query_row(
                "SELECT COALESCE(c.content_html, '') FROM articles a LEFT JOIN contents c ON c.id = a.content_id WHERE a.id = ?1",
                [aid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kept, "<p>远端正文</p>", "不被覆盖的现场值");

        // 远端无可回填内容（空串）→ 无变化（空串 ≡ NULL 归一）
        let empty = seed(&conn, "fill-empty");
        let none_filled = backfill_article_content(&conn, empty, "", "", None, None, None).unwrap();
        assert!(!none_filled, "无可回填内容不得报变化");
    }
}
