//! sync 的 entries 子模块（TASK-045 从 sync.rs 按既有章节拆分）。

use super::*;
use crate::db::{self, NewArticle};
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

/// 从 ItemContent 提取条目十进制 id（长格式 id 尾部十六进制）。
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
/// - 待推保护：本地有未推送变更 → 跳过（本地优先，防乒乓）
/// - read-anywhere-wins：「读」是强意图，任何副本的已读都接受
/// - unread 只认绑定同源 entry：跨源副本的未读不能复活桌面已读
fn merge_remote_status(
    conn: &Connection,
    aid: i64,
    e: &ItemContent,
    maps: &mut db::SyncMatchMaps,
    report: &mut SyncReport,
) {
    let Some(eid) = item_numeric_id(e) else {
        return;
    };
    let feed_id = item_feed_id(e);
    let _ = db::set_article_remote_id(conn, aid, eid);
    // 同步 maps 的绑定状态：后续 entry 若 URL 兜底匹配到同一 aid，能读到
    // 「已绑定 eid」而非批量快照里的「未绑定」，避免跨源同 URL 副本被误判
    // 为自己的条目（时序偏差）。
    maps.id_to_mf_id.insert(aid, Some(eid));
    maps.id_to_mf_pair.insert(
        aid,
        (
            Some(eid),
            maps.id_to_mf_pair.get(&aid).map(|p| p.1).unwrap_or(None),
        ),
    );
    maps.mf_id_to_article.insert(eid, aid);
    if maps.pending_ids.contains(&aid) {
        return;
    }
    let remote_read = greader::has_tag(&e.categories, "/com.google/read");
    let remote_starred = greader::has_tag(&e.categories, "/com.google/starred");
    let local_bound = db::article_by_remote_id(conn, eid).ok().flatten().is_some();
    let same_feed_trusted = maps
        .id_to_mf_pair
        .get(&aid)
        .map(|(entry_mf, feed_mf)| match entry_mf {
            Some(_) => *feed_mf == feed_id,
            None => true,
        })
        .unwrap_or(false);
    let accept_unread = local_bound && same_feed_trusted;
    if remote_read || accept_unread {
        let _ = db::sync_set_article_status(conn, aid, remote_read, remote_starred);
        report.pulled_entries += 1;
    }
}

/// 共享：合并一条已拉取的远端条目（URL 兜底匹配 / remote_id 直配 / 同源判定 /
/// 跨源副本记账 / 新条目 upsert）。Google Reader 与 Fever 两条 pull 路径共用。
pub(super) fn merge_pulled_entry(
    conn: &Connection,
    e: &ItemContent,
    maps: &mut db::SyncMatchMaps,
    report: &mut SyncReport,
) {
    // URL 兜底匹配（规范化）
    let aid = item_url(e)
        .as_deref()
        .and_then(|u| maps.url_to_id.get(&db::normalize_url(u)).copied())
        .or_else(|| {
            // 已绑定 remote_id 直配
            item_numeric_id(e).and_then(|eid| maps.mf_id_to_article.get(&eid).copied())
        });
    match aid {
        Some(aid) => {
            // 已存在：判断是否自己的条目（同源），合并状态；跨源副本记账
            let eid = item_numeric_id(e);
            let feed_id = item_feed_id(e);
            let bound_entry = maps.id_to_mf_id.get(&aid).copied().flatten();
            let is_own = bound_entry == eid
                || (bound_entry.is_none()
                    && maps
                        .id_to_mf_pair
                        .get(&aid)
                        .map(|(entry_mf, feed_mf)| match entry_mf {
                            Some(_) => *feed_mf == feed_id,
                            None => true,
                        })
                        .unwrap_or(false));
            if !is_own {
                // 跨源副本：记账（已读广播对象）。read-anywhere-wins
                if let Some(eid) = eid {
                    let _ = db::add_article_dup_entry(conn, aid, eid);
                }
                if greader::has_tag(&e.categories, "/com.google/read")
                    && !maps.pending_ids.contains(&aid)
                {
                    let _ = db::sync_mark_read_if_unread(conn, aid);
                }
                return;
            }
            merge_remote_status(conn, aid, e, maps, report);
            // 正文/封面/enclosure 兜底回填：本地为空才补，已有内容不覆盖。
            backfill_entry_content(conn, aid, e);
        }
        None => {
            // 本地没有 → 若其远端 feed 已绑定，则 upsert 补齐
            let feed_id = item_feed_id(e);
            let local_feed = feed_id.and_then(|fid| maps.feed_mf_to_id.get(&fid).copied());
            if let Some(local_feed) = local_feed {
                upsert_remote_entry(conn, local_feed, e, maps, report);
            }
        }
    }
}

/// 协议分派：Google Reader 用 `ot` 时间游标，Fever 用 `since_id` 条目游标。
pub(super) async fn pull_entries(
    db: &Arc<Mutex<Connection>>,
    client: &Backend,
    report: &mut SyncReport,
    full: bool,
) {
    match client {
        Backend::GReader(g) => pull_entries_greader(db, g, report, full).await,
        Backend::Fever(f) => pull_entries_fever(db, f, report, full).await,
    }
}

/// 后端条目入库（source='miniflux'，不覆盖直连正文）。
/// URL 兜底合并需同源校验：跨源同 URL entry 不写状态、不抢绑定。
/// enclosure（播客音频/视频）与图片一并落库——播放器与卡片封面依赖。
fn upsert_remote_entry(
    conn: &Connection,
    feed_id: i64,
    e: &ItemContent,
    maps: &mut db::SyncMatchMaps,
    report: &mut SyncReport,
) {
    let existing = item_numeric_id(e)
        .and_then(|eid| maps.mf_id_to_article.get(&eid).copied())
        .or_else(|| {
            item_url(e)
                .as_deref()
                .and_then(|u| maps.url_to_id.get(&db::normalize_url(u)).copied())
                .filter(|aid| {
                    maps.id_to_mf_pair
                        .get(aid)
                        .map(|(entry_mf, feed_mf)| match entry_mf {
                            Some(_) => *feed_mf == item_feed_id(e),
                            None => true,
                        })
                        .unwrap_or(false)
                })
        });

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

    if let Some(aid) = existing {
        // 状态以后端为准；正文仅在本地为空时补。
        // 待推保护：本地有未推送的读/收藏变更时，跳过状态覆盖（本地优先，防乒乓）。
        if let Some(eid) = item_numeric_id(e) {
            let _ = db::set_article_remote_id(conn, aid, eid);
            maps.id_to_mf_id.insert(aid, Some(eid));
            maps.id_to_mf_pair.insert(
                aid,
                (
                    Some(eid),
                    maps.id_to_mf_pair.get(&aid).map(|p| p.1).unwrap_or(None),
                ),
            );
            maps.mf_id_to_article.insert(eid, aid);
        }
        if !maps.pending_ids.contains(&aid) {
            let _ = db::sync_set_article_status(conn, aid, remote_read, remote_starred);
        }
        // 封面回填：正文第一图，本地已有封面不覆盖（COALESCE）
        backfill_entry_content(conn, aid, e);
    } else {
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
        if let Ok((aid, _)) = db::upsert_article_with_feed(conn, feed_id, &a, false) {
            if let Some(eid) = item_numeric_id(e) {
                let _ = db::set_article_remote_id(conn, aid, eid);
                maps.id_to_mf_id.insert(aid, Some(eid));
                maps.mf_id_to_article.insert(eid, aid);
            }
            let _ = db::sync_set_article_status(conn, aid, remote_read, remote_starred);
            report.pulled_entries += 1;
        }
    }
}

fn strip_html_text(html: &str) -> String {
    crate::sanitize::html_to_text(html)
}

/// 已有条目正文/封面/enclosure 兜底回填：本地为空才补（COALESCE），已有内容绝不覆盖。
/// （封面 / enclosure 单列 COALESCE：既不抢本地封面，也能补上 Miniflux 后来抓到的图。）
fn backfill_entry_content(conn: &Connection, aid: i64, e: &ItemContent) {
    let content_html = item_content_html(e);
    let enclosure = e.enclosure.first();
    let (enc_url, enc_mime) = match enclosure {
        Some(enc) => (Some(enc.url.clone()), enc.r#type.clone()),
        None => (None, None),
    };
    let content_image = crate::sanitize::first_image(&content_html);
    let _ = db::backfill_article_content(
        conn,
        aid,
        &content_html,
        &strip_html_text(&content_html),
        content_image.as_deref(),
        enc_url.as_deref(),
        enc_mime.as_deref(),
    );
}
