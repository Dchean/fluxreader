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

/// 状态合并的守卫语义（pull_entries 与对账共用）：
/// - 待推保护：本地有未推送变更 → 跳过该**字段**（本地优先，防乒乓）；
/// - OPT-007 字段版本守卫：pull 起点版本快照 != 当前 DB 版本 → 跳过该字段
///   （拉取期间本地写过；即使推送已确认、队列已清，版本仍保留）；
/// - 双向接受精确条目的远端读/藏状态，其他源没有状态传播路径。
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
    let remote_read = e.read_state();
    let remote_starred = e.starred_state();
    let local_bound = db::article_by_remote_id(conn, eid)?.is_some();
    let apply_read = remote_read == greader::RemoteState::True
        || (remote_read == greader::RemoteState::False && accept_unread && local_bound);
    let apply_star = remote_starred != greader::RemoteState::Missing;
    if apply_read || apply_star {
        // OPT-007：逐字段过版本守卫（pull 起点快照 + 当前 DB 版本 + 该字段
        // pending）。任一方被本地新写入/未推送意图压住时只跳过该字段，另一
        // 字段不受牵连；两侧都过才把整条计入 pulled_entries（与旧口径「状态
        // 已接受」对齐）。merge_remote_status 只在 merge_pulled_entry 的 own
        // 分支调用（同库测试锁定时序），入参 maps 与写入同处一个 DB 临界区。
        let (snap_read, snap_star) = maps.id_to_versions.get(&aid).copied().unwrap_or((0, 0));
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
    let remote_read = e.read_state();
    if feed_remote.is_some_and(|id| maps.excluded_feed_ids.contains(&id)) {
        return Ok(());
    }
    // B2（空 link 身份）：normalize_url 把空串归一为 ''，而 '' 是合法匹配键——
    // 同轮多条空 link 条目会经 feed_url_to_id[""] 互相借位绑定。与入库路径
    // （normalized_url_key）同口径：空/纯空白 link 无身份语义，匹配键为 None。
    // Note: 空 link 身份契约与迁移边界 — 见 .agents/notes/implemented/bug-fix/2026-10-31-GR回补与拉取侧身份与性能修复.md
    let norm = item_url(e)
        .as_deref()
        .map(db::normalize_url)
        .filter(|n| !n.is_empty());

    // ① 精确协议 entry ID 优先。
    if let Some(eid) = eid {
        if let Some(&aid) = maps.mf_id_to_article.get(&eid) {
            if !maps.article_source_is_current(conn, aid)? {
                return Ok(());
            }
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

    // 已知 entry 的稀疏回包可走①；新身份没有归属则不能消费列举窗口。
    // Note: 精确身份与 URL 兜底边界 — 见 .agents/notes/implemented/bug-fix/2026-10-08-保守URL匹配与重建标记.md
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
        }
        return Ok(());
    }
    // GR continues processing known source entries after an unknown source, while
    // its caller records the incomplete binding and holds the cursor. Fever rejects
    // such a page before its transaction, so neither protocol invents a source.
    report.binding_incomplete = true;
    Ok(())
}

/// 精确身份：逐字段合并状态，并接收同一远端条目的正文修订。
fn merge_own_entry(
    conn: &Connection,
    aid: i64,
    e: &ItemContent,
    maps: &mut db::SyncMatchMaps,
    report: &mut SyncReport,
    _remote_read: greader::RemoteState,
    _norm: Option<&str>,
) -> AppResult<()> {
    merge_remote_status(conn, aid, e, maps, report, true)?;
    if db::refresh_bound_remote_article(conn, aid, &remote_article_input(e))? {
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
    report.binding_incomplete = false;
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
        // 新身份接收其自己远端状态，不从其他源或组成员借用状态。
        let effective_read = remote_read == greader::RemoteState::True;
        if effective_read {
            db::sync_set_read_at_version(conn, aid, true, 0)?;
        }
        if remote_starred == greader::RemoteState::True {
            db::sync_set_star_at_version(conn, aid, true, 0)?;
        }
        report.pulled_entries += 1;
    } else {
        // 既有行（典型：pull 起点快照之后直连入库、快照映射尚未包含的同 feed 同
        // URL 行）：状态逐字段过版本/pending 守卫，与 merge_remote_status 同一套
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

    /// 三态的另一半：显式空 categories 是明确 false，不是未知。
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
            (0, 0),
            "显式 []=false 必须与缺字段未知区分"
        );
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
