//! sync 的 entries 子模块（TASK-045 从 sync.rs 按既有章节拆分）。

use super::*;
use crate::db::{self, NewArticle};
use crate::error::AppResult;
use crate::greader::{self, ItemContent};
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use std::sync::Arc;
use tokio::sync::Mutex;

/// 从 ItemContent 提取远端 feed 数字 id（origin.stream_id = feed/42）。
fn item_feed_id(e: &ItemContent) -> Option<i64> {
    e.origin
        .as_ref()
        .and_then(|o| greader::parse_feed_numeric_id(&o.stream_id))
}

/// 从 ItemContent 提取条目十进制 id。解析规则在 `greader::parse_item_id`
/// 单点（协议形状：长格式 tag 尾部十六进制；无前缀纯数字按十进制——
/// Fever 长 id 与 greader 十进制 id 都不得按十六进制猜，OPT-005/F03）。
pub(super) fn item_numeric_id(e: &ItemContent) -> Option<i64> {
    greader::parse_item_id(&e.id)
}

/// 从 ItemContent 提取原文 URL（alternate[0].href）。
fn item_url(e: &ItemContent) -> Option<String> {
    e.alternate.first().map(|a| a.href.clone())
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
fn item_published_at(e: &ItemContent) -> String {
    if e.published > 0 {
        DateTime::from_timestamp(e.published, 0)
            .map(|d| d.with_timezone(&Utc).to_rfc3339())
            .unwrap_or_else(|| Utc::now().to_rfc3339())
    } else {
        Utc::now().to_rfc3339()
    }
}

/// 状态合并的守卫语义（pull_entries 与对账共用）：
/// - 待推保护：本地有未推送变更 → 跳过该**字段**（本地优先，防乒乓）；
/// - OPT-007 字段版本守卫：pull 起点版本快照 != 当前 DB 版本 → 跳过该字段
///   （拉取期间本地写过；即使推送已确认、队列已清，版本仍保留）；
/// - read-anywhere-wins：「读」是强意图，任何副本的已读都接受
/// - unread 只认绑定同源 entry：跨源副本的未读不能复活桌面已读
///
/// OPT-008B 第一阶段：本函数只由**结构性命中自己的行**的调用点进入——
/// 精确 entry ID 直配（同一 eid 的行）或同 feed URL 兜底（未绑定行首绑）。
/// 旧 `same_feed_trusted` 启发式（article remote_id + feed remote_id 推导）
/// 被结构匹配取代：跨源 entry 在选行阶段就不会落到本行，「远端 unread 不得
/// 复活另一源条目」由身份层而非合并守卫承担。`accept_unread` 保留为显式
/// 参数，便于未来策略单点扩展。
///
/// 字段彼此独立：读的守卫不压收藏的合并，反之亦然（任务卡负例②）。
///
/// R2：所有 DB 写失败**透传 Err**（不再 warn 后继续）——Fever 侧据此整页回滚
/// 并保留输入页游标；GR 侧由调用方记错误且不推进游标（不吞失败）。
///
/// R3：状态写改用**值条件**原语（`*_if_unread/_if_read/_if_unstarred/_if_starred`
/// 的 at_version 形态）——命中行数 = 值实际改变的证据，据此置
/// `report.entries_data_changed`；`pulled_entries` 仍按旧口径（守卫通过即计数，
/// 含同值）累计，保持对外统计兼容，但不再充当事件判定依据。
///
/// R4：标记在**每个成功实际变更点立即合并**（不等函数尾）——GR 是逐语句提交
/// （无页事务），绑定/read 写成功后同函数内的 star 写可能 Err，若攒到尾部再
/// 置位，已提交的变化就失去事件信号。失败本身不算变化（Err 路径不置位），
/// 只有 rows>0 / binding_changed 才置。
fn merge_remote_status(
    conn: &Connection,
    aid: i64,
    e: &ItemContent,
    maps: &mut db::SyncMatchMaps,
    report: &mut SyncReport,
    accept_unread: bool,
) -> AppResult<()> {
    let Some(eid) = item_numeric_id(e) else {
        return Ok(());
    };
    let binding_changed = db::set_article_remote_id(conn, aid, eid)?;
    // R3/R4/R5：绑定本身是可见数据变化（NULL/旧 id → 该 entry 的 id）——在
    // set_article_remote_id **成功之后立即**并入标记：其后任何可失败读取
    // （article_by_remote_id、字段版本守卫等）或写入失败都不得让已提交的绑定
    // 变化失去信号（GR 逐语句提交，无页事务）。纯失败不置位（binding_changed
    // 仅在值实际变化时为 true）。
    if binding_changed {
        report.entries_data_changed = true;
    }
    // 同步 maps 的绑定状态：同一轮后续条目若命中原行，能读到「已绑定 eid」，
    // 避免时序偏差（重复页/同页回响）。
    maps.id_to_mf_id.insert(aid, Some(eid));
    maps.mf_id_to_article.insert(eid, aid);
    let remote_read = greader::has_tag(&e.categories, "/com.google/read");
    let remote_starred = greader::has_tag(&e.categories, "/com.google/starred");
    let local_bound = db::article_by_remote_id(conn, eid)?.is_some();
    if remote_read || (accept_unread && local_bound) {
        // OPT-007：逐字段过版本守卫（pull 起点快照 + 当前 DB 版本 + 该字段
        // pending）。任一方被本地新写入/未推送意图压住时只跳过该字段，另一
        // 字段不受牵连；两侧都过才把整条计入 pulled_entries（与旧口径「状态
        // 已接受」对齐）。merge_remote_status 只在 merge_pulled_entry 的 own
        // 分支调用（同库测试锁定时序），入参 maps 与写入同处一个 DB 临界区。
        let (snap_read, snap_star) = maps.id_to_versions.get(&aid).copied().unwrap_or((0, 0));
        let read_ok = conflict_policy::field_accepts_remote(
            conn,
            aid,
            conflict_policy::SyncField::Read,
            snap_read,
            maps.pending_read_ids.contains(&aid),
        )?;
        let star_ok = conflict_policy::field_accepts_remote(
            conn,
            aid,
            conflict_policy::SyncField::Star,
            snap_star,
            maps.pending_star_ids.contains(&aid),
        )?;
        if read_ok {
            if remote_read {
                let rows = db::sync_mark_read_if_unread_at_version(conn, aid, snap_read)?;
                if rows > 0 {
                    report.entries_data_changed = true;
                }
                // R1 顺序确定性：本轮已落定「已读」——同轮 own unread 不得再翻转。
                maps.explicit_read_ids.insert(aid);
            } else if !maps.explicit_read_ids.contains(&aid) {
                let rows = db::sync_mark_unread_if_read_at_version(conn, aid, snap_read)?;
                if rows > 0 {
                    report.entries_data_changed = true;
                }
            }
        }
        if star_ok {
            let rows = if remote_starred {
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

/// 已读广播（行级）：把一条本地行按字段版本/pending 守卫标读（远端副本已读的
/// read-anywhere-wins 扩展；unread 不广播）。返回是否有真实变更。
/// R1：无论写值是否变化，守卫通过即记录「本轮已读」——同轮后续 own unread
/// 不得再翻转该行（顺序确定性，见 [`db::SyncMatchMaps::explicit_read_ids`]）。
fn mark_read_guarded(
    conn: &Connection,
    aid: i64,
    maps: &mut db::SyncMatchMaps,
    report: &mut SyncReport,
) -> AppResult<()> {
    let snap_read = maps.id_to_versions.get(&aid).copied().unwrap_or((0, 0)).0;
    let pending_read = maps.pending_read_ids.contains(&aid);
    if conflict_policy::field_accepts_remote(
        conn,
        aid,
        conflict_policy::SyncField::Read,
        snap_read,
        pending_read,
    )? {
        let rows = db::sync_mark_read_if_unread_at_version(conn, aid, snap_read)?;
        if rows > 0 {
            report.entries_data_changed = true;
        }
        maps.explicit_read_ids.insert(aid);
    }
    Ok(())
}

/// 已读广播（同文兄弟集合）：远端任一副本已读时，把 `url_norm` 相同的其它
/// 条目一并标读（读到哪算读；布局/源不参与选择，见 db::read_broadcast_targets
/// 的同款选择规则）。unread 不广播（只作用当前条目）。
/// 逐行过字段版本/pending 守卫：拉取期间的本地写入不被旧快照回滚。
fn broadcast_read_to_siblings(
    conn: &Connection,
    aid: i64,
    url_norm: &str,
    maps: &mut db::SyncMatchMaps,
    report: &mut SyncReport,
) -> AppResult<()> {
    // 先克隆兄弟列表：广播落定会写 explicit_read_ids（&mut maps），不与迭代借用共存。
    let Some(siblings) = maps.url_norm_to_ids.get(url_norm).cloned() else {
        return Ok(());
    };
    for sid in siblings {
        if sid == aid {
            continue;
        }
        mark_read_guarded(conn, sid, maps, report)?;
    }
    Ok(())
}

/// 共享：合并一条已拉取的远端条目。
///
/// OPT-008B 第一阶段（条目身份）：匹配顺序与语义显式化——
///   ① 精确协议 entry ID 直配（`mf_id_to_article`）：命中即自己的行（跨源
///      同 URL 不再借位；feed 元数据不一致的脏绑定除外）；
///   ② URL 兜底**只在同一订阅源内选行**（`feed_url_to_id`）；同 feed 同 URL
///      的未绑定行首绑（guid 变化但 URL 一致保持稳定 ID）；已绑另一 entry 的
///      行只做副本记账 + 已读广播（不换绑）；
///   ③ 该源尚无此 URL 行 → 独立 upsert（跨源同 URL 各自成条目）；
///   ④ 远端 feed 未绑定（无本地源可挂）→ 有限兼容：全局 URL 行记账副本 +
///      已读广播，不猜 feed、不搬迁行（full 重拉按真实 feed 补行）。
/// 不复制旧行为里的「全局 URL 先于 remote_id」：那会让第二条同 URL entry 在
/// 快照已含行时被吞成 CSV 副本（状态不再合并、收藏/未读丢失）。
///
/// R2：返回 `AppResult<()>`——任何 DB 写失败透传（Fever 页事务据此整页回滚；
/// GR 调用方记错误并保留游标）。**禁止**在此吞成 warn/`let _`。
pub(super) fn merge_pulled_entry(
    conn: &Connection,
    e: &ItemContent,
    maps: &mut db::SyncMatchMaps,
    report: &mut SyncReport,
) -> AppResult<()> {
    let eid = item_numeric_id(e);
    let feed_remote = item_feed_id(e);
    let remote_read = greader::has_tag(&e.categories, "/com.google/read");
    let norm = item_url(e).as_deref().map(db::normalize_url);

    // ① 精确协议 entry ID 优先。
    if let Some(eid) = eid {
        if let Some(&aid) = maps.mf_id_to_article.get(&eid) {
            let row_feed_mf = maps.id_to_feed_mf.get(&aid).copied().flatten();
            let feed_compatible = match (row_feed_mf, feed_remote) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            };
            if feed_compatible {
                merge_own_entry(conn, aid, e, maps, report, remote_read, norm.as_deref())?;
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

    // ② 同 feed URL 兜底（含 ③ 该源尚无行 → 独立入库）。
    if let (Some(feed_remote), Some(norm)) = (feed_remote, norm.as_deref()) {
        if let Some(&local_feed) = maps.feed_mf_to_id.get(&feed_remote) {
            if let Some(&aid) = maps.feed_url_to_id.get(&(local_feed, norm.to_string())) {
                let bound = maps.id_to_mf_id.get(&aid).copied().flatten();
                if bound.is_none() {
                    // 未绑定行首绑（同 feed、URL 稳定的既有匹配；直连 guid 变化
                    // 但 URL 一致 → 复用同一行）。
                    merge_own_entry(conn, aid, e, maps, report, remote_read, Some(norm))?;
                    return Ok(());
                }
                if bound == eid {
                    // 已绑定同一 eid 由 ① 覆盖；此处是 feed 元数据冲突复核后的
                    // 回退路径（身份仍是同一精确 entry）。
                    merge_own_entry(conn, aid, e, maps, report, remote_read, Some(norm))?;
                    return Ok(());
                }
                // 同源同 URL 的另一条**已绑定精确 entry**：精确协议 ID 必须保有
                // 自己身份——该 entry 独立入库（各自绑定/读藏；不再以 CSV 记账
                // 充当条目实体）。remote_id 唯一约束由 eid 未绑定保证（命中的
                // 行绑定的是别的 eid）。
                upsert_remote_entry(conn, local_feed, e, maps, report)?;
                return Ok(());
            }
            upsert_remote_entry(conn, local_feed, e, maps, report)?;
            return Ok(());
        }
    }

    // ④ 远端 feed 未绑定：有限兼容（副本记账 + 已读广播）；不猜 feed。
    if let Some(norm) = norm.as_deref() {
        if let Some(&aid) = maps.url_to_id.get(norm) {
            if let Some(eid) = eid {
                db::add_article_dup_entry(conn, aid, eid)?;
            }
            if remote_read {
                mark_read_guarded(conn, aid, maps, report)?;
            }
        }
    }
    Ok(())
}

/// 自己的行：状态合并 → （远端已读时的）同文兄弟已读广播 → 正文兜底回填。
fn merge_own_entry(
    conn: &Connection,
    aid: i64,
    e: &ItemContent,
    maps: &mut db::SyncMatchMaps,
    report: &mut SyncReport,
    remote_read: bool,
    norm: Option<&str>,
) -> AppResult<()> {
    merge_remote_status(conn, aid, e, maps, report, true)?;
    if remote_read {
        if let Some(norm) = norm {
            broadcast_read_to_siblings(conn, aid, norm, maps, report)?;
        }
    }
    // 正文/封面/enclosure 兜底回填：本地为空才补，已有内容不覆盖。
    // R3：只有真实补进了内容才置数据变化标记（同值重复/无可回填 = false）。
    if backfill_entry_content(conn, aid, e)? {
        report.entries_data_changed = true;
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
) -> AppResult<()> {
    match &session.client {
        Backend::GReader(g) => pull_entries_greader(db, session, g, report, full).await,
        Backend::Fever(f) => pull_entries_fever(db, session, f, report, full).await,
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
    let published = item_published_at(e);
    let content_html = item_content_html(e);

    // enclosure：Google Reader 的 enclosure 无 duration，只有 url + type
    let enclosure = e.enclosure.first();
    let (enc_url, enc_mime) = match enclosure {
        Some(enc) => (Some(enc.url.clone()), enc.r#type.clone()),
        None => (None, None),
    };

    let remote_read = greader::has_tag(&e.categories, "/com.google/read");
    let remote_starred = greader::has_tag(&e.categories, "/com.google/starred");

    let a = NewArticle {
        guid: item_numeric_id(e)
            .map(|eid| format!("remote-{eid}"))
            .unwrap_or_else(|| format!("remote-{}", e.id)),
        url: item_url(e),
        title: e.title.clone(),
        author: e.author.clone(),
        summary: None,
        content_html: Some(crate::sanitize::sanitize(
            &content_html,
            item_url(e).as_deref(),
        )),
        body_text: strip_html_text(&content_html),
        image_url: crate::sanitize::first_image(&content_html),
        enclosure_url: enc_url,
        enclosure_mime: enc_mime,
        duration_sec: None,
        published_at: Some(published),
        source: "miniflux".into(),
    };
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
    // OPT-008B：把轮内新增行补进 URL 映射族——同一轮后续条目/广播按最新
    // 行集选行（否则本轮新行要到下一轮快照才可见，时序偏差会漏匹配）。
    let norm = a.url.as_deref().map(db::normalize_url);
    if let Some(norm) = norm.as_deref() {
        maps.url_to_id.entry(norm.to_string()).or_insert(aid);
        maps.feed_url_to_id
            .entry((feed_id, norm.to_string()))
            .or_insert(aid);
        let ids = maps.url_norm_to_ids.entry(norm.to_string()).or_default();
        if !ids.contains(&aid) {
            ids.push(aid);
        }
    }
    // R3/R4：新插入 / 既有行可见字段真实变化（tracked 结果）——**立即**并入
    // 标记：这些变化已随本语句提交，其后绑定/状态任一步失败都不得让它们失去
    // 事件信号（GR 逐语句提交，无页事务可依赖）。
    if outcome.inserted || outcome.content_changed {
        report.entries_data_changed = true;
    }
    if let Some(eid) = item_numeric_id(e) {
        // R4：绑定成功即置位（后续步骤失败不回退已提交的绑定变化）。
        if db::set_article_remote_id(conn, aid, eid)? {
            report.entries_data_changed = true;
        }
        maps.id_to_mf_id.insert(aid, Some(eid));
        maps.mf_id_to_article.insert(eid, aid);
    }
    if outcome.inserted {
        // 真正的新行：插入发生在同一 DB 临界区内，其间不可能有本地写入，快照
        // (0,0) 与行初值恒等，整值写状态不会覆盖任何用户意图。远端确认不产生
        // 本地版本（保持 0）。新行本身的标记已在上面置位。
        //
        // R1 读优先 + 顺序确定性：新行远端未读但同组已有已读副本（同轮先处理的
        // 兄弟广播 / 既有本地已读）→ 新副本按读落定——否则「先建读行再建未读行」
        // 与「先建未读行再广播」两种同轮顺序会给出不同结果（本页顺序不得决定
        // 状态）。对照：同组无任何已读信号时，own unread 照常生效。
        let effective_read =
            remote_read || (!remote_read && group_has_read_this_round(maps, aid, norm.as_deref()));
        db::sync_set_article_status(conn, aid, effective_read, remote_starred)?;
        report.pulled_entries += 1;
        if effective_read {
            maps.explicit_read_ids.insert(aid);
        }
    } else {
        // 既有行（典型：pull 起点快照之后直连入库、快照映射尚未包含的同 feed 同
        // URL 行）：状态逐字段过版本/pending 守卫，与 merge_remote_status 同一套
        // 语义（read/star 彼此独立，无冲突字段照常收敛）——整值 sync_set_article_status
        // 在这里会回滚快照之后用户的标读/收藏（R2 修复的 P2 反例）。
        // R3/R4：状态写用值条件原语，命中行数当场进标记（守卫通过但值未变仍按
        // 旧口径计入 pulled_entries，保持统计兼容；写 Err 不置位——失败不是变化）。
        let (snap_read, snap_star) = maps.id_to_versions.get(&aid).copied().unwrap_or((0, 0));
        let read_ok = conflict_policy::field_accepts_remote(
            conn,
            aid,
            conflict_policy::SyncField::Read,
            snap_read,
            maps.pending_read_ids.contains(&aid),
        )?;
        let star_ok = conflict_policy::field_accepts_remote(
            conn,
            aid,
            conflict_policy::SyncField::Star,
            snap_star,
            maps.pending_star_ids.contains(&aid),
        )?;
        if read_ok {
            if remote_read {
                let rows = db::sync_mark_read_if_unread_at_version(conn, aid, snap_read)?;
                if rows > 0 {
                    report.entries_data_changed = true;
                }
                // R1 顺序确定性：本轮已落定「已读」——同轮 own unread 不得再翻转。
                maps.explicit_read_ids.insert(aid);
            } else if !maps.explicit_read_ids.contains(&aid) {
                let rows = db::sync_mark_unread_if_read_at_version(conn, aid, snap_read)?;
                if rows > 0 {
                    report.entries_data_changed = true;
                }
            }
        }
        if star_ok {
            let rows = if remote_starred {
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
    // 已读广播：本条远端已读 → 同文兄弟条目一并标读（读到哪算读）。放在新行/
    // 既有行两分支之后统一执行——新行入库同样要广播（本轮先处理的行）。
    if remote_read {
        if let Some(norm) = norm.as_deref() {
            broadcast_read_to_siblings(conn, aid, norm, maps, report)?;
        }
    }
    Ok(())
}

fn strip_html_text(html: &str) -> String {
    crate::sanitize::html_to_text(html)
}

/// 同组（同 url_norm）**本轮**是否已有已读信号（排除自身）。用于新行落定前的
/// 读优先判定（R1 引入，R2 校准）：只看本轮 `explicit_read_ids`（own 已读或
/// 广播落定），不看历史本地读态——历史已读不得永久把之后远端显式 unread 的
/// 新副本翻读（N4 裁定：无本轮读信号时 unread 权威照旧）。
fn group_has_read_this_round(maps: &db::SyncMatchMaps, aid: i64, url: Option<&str>) -> bool {
    let Some(url) = url.filter(|u| !u.is_empty()) else {
        return false;
    };
    let Some(ids) = maps.url_norm_to_ids.get(&db::normalize_url(url)) else {
        return false;
    };
    ids.iter()
        .any(|sid| *sid != aid && maps.explicit_read_ids.contains(sid))
}

/// 已有条目正文/封面/enclosure 兜底回填：本地为空才补（COALESCE），已有内容绝不覆盖。
/// （封面 / enclosure 单列 COALESCE：既不抢本地封面，也能补上 Miniflux 后来抓到的图。）
/// OPT-002（F07）语义保留（失败必须可见）；R2 升级形态：回填失败透传 Err——
/// 由调用方决定粒度（Fever 整页回滚 + 保留游标；GR 记错误且不推游标）。
/// R3：返回是否**实际补进了内容**（同值重复/无可回填 = false），供数据变化标记。
// Note: 回填写入的净化在 db::backfill_article_content 内完成（同 sanitize 口径） — 见 .agents/notes/implemented/architecture/2026-10-08-可渲染正文安全边界.md
fn backfill_entry_content(conn: &Connection, aid: i64, e: &ItemContent) -> AppResult<bool> {
    let content_html = item_content_html(e);
    let enclosure = e.enclosure.first();
    let (enc_url, enc_mime) = match enclosure {
        Some(enc) => (Some(enc.url.clone()), enc.r#type.clone()),
        None => (None, None),
    };
    let content_image = crate::sanitize::first_image(&content_html);
    db::backfill_article_content(
        conn,
        aid,
        &content_html,
        &strip_html_text(&content_html),
        content_image.as_deref(),
        enc_url.as_deref(),
        enc_mime.as_deref(),
    )
}
