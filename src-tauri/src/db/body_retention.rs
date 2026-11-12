//! Device-local body cache maintenance. Never changes read/star, identity or wire state.
//! Note: .agents/notes/implemented/bug-fix/2026-11-05-缓存清理语义与未读保护.md
use super::*;
use crate::error::AppError;
use rusqlite::params;
use std::time::{Duration, Instant};

const CANDIDATES: usize = 64;
const RELEASES: usize = 8;
const SLICE: Duration = Duration::from_millis(15);

// Fetch bounded index candidates BEFORE testing L, holds or equivalence groups.
// A skipped candidate still advances its lane; protection cannot starve the tail.
pub(super) const EXPIRED_SQL: &str = "SELECT id,body_retention_since FROM articles INDEXED BY idx_articles_body_retention WHERE body_retention_since IS NOT NULL AND (content_id IS NOT NULL OR rss_content_id IS NOT NULL) AND (body_retention_since,id)>(?1,?2) AND body_retention_since<=?3 ORDER BY body_retention_since,id LIMIT ?4";
pub(super) const INITIALIZE_SQL: &str = "SELECT id FROM articles INDEXED BY idx_articles_body_retention_null WHERE body_retention_since IS NULL AND is_read=1 AND is_starred=0 AND (content_id IS NOT NULL OR rss_content_id IS NOT NULL) AND id>?1 ORDER BY id LIMIT ?2";

#[derive(Clone, Debug)]
pub(crate) struct RetentionCursor {
    expired: (i64, i64),
    initialize: i64,
    initialize_next: bool,
}
impl Default for RetentionCursor {
    fn default() -> Self {
        Self {
            expired: (-1, 0),
            initialize: 0,
            initialize_next: true,
        }
    }
}
#[derive(Default, Debug)]
pub(crate) struct RetentionReport {
    pub inspected: usize,
    pub initialized: usize,
    pub released: usize,
}

pub(crate) fn quiescent(conn: &Connection) -> AppResult<bool> {
    // Sealed publication retries do not delay local maintenance. Their real
    // protected bytes remain owned by holds; unread/starred L still wins.
    Ok(conn.query_row("SELECT applying=0 AND folding=0 AND (capture=0 OR initialized=1) AND accepted_job IS NULL AND NOT EXISTS(SELECT 1 FROM local_sync_work WHERE job='') AND NOT EXISTS(SELECT 1 FROM local_sync_intents) FROM local_sync_control WHERE id=1", [], |r| r.get(0))?)
}

fn retention_days(conn: &Connection) -> AppResult<i64> {
    let Some(raw) = super::get_setting(conn, "app_settings")? else {
        return Ok(0);
    };
    let settings: serde_json::Value = serde_json::from_str(&raw)?;
    let object = settings
        .as_object()
        .ok_or_else(|| AppError::new("invalidSettings", "正文保留设置不是对象，本轮未清理"))?;
    let Some(days) = object.get("readBodyRetentionDays") else {
        return Ok(0);
    };
    super::settings::validate_app_setting("readBodyRetentionDays", days)?;
    days.as_i64()
        .filter(|days| (0..=3650).contains(days))
        .ok_or_else(|| AppError::new("invalidSettings", "正文保留天数无效，本轮未清理"))
}

/// Used after an explicit read intent or a successful missing->nonempty restore.
/// L is checked even while physical capture is suppressed; this is not new sync input.
pub(crate) fn restart(conn: &Connection, id: i64, now: i64) -> AppResult<()> {
    let ordinary = crate::local_sync::logical::article_retention_state(conn, id)?
        .is_some_and(|(read, star, _active)| read && !star);
    conn.execute("UPDATE articles SET body_retention_since=?1 WHERE id=?2 AND body_retention_since IS NOT ?1",
        params![ordinary.then_some(now), id])?;
    Ok(())
}

pub(crate) fn complete_origin(conn: &Connection, id: i64, since: i64) -> AppResult<()> {
    let ordinary = crate::local_sync::logical::article_retention_state(conn, id)?
        .is_some_and(|(read, star, _active)| read && !star);
    if ordinary {
        conn.execute("UPDATE articles SET body_retention_since=?1 WHERE id=?2 AND (body_retention_since IS NULL OR body_retention_since<?1)",params![since,id])?;
    } else {
        conn.execute("UPDATE articles SET body_retention_since=NULL WHERE id=?1 AND body_retention_since IS NOT NULL",[id])?;
    }
    Ok(())
}

pub(crate) fn has_body(conn: &Connection, id: i64) -> AppResult<bool> {
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM articles a JOIN contents c ON c.id=a.content_id WHERE a.id=?1 AND (COALESCE(c.content_html,'')!='' OR c.body_text!=''))", [id], |r| r.get(0))?)
}

#[derive(Clone, Copy)]
pub(super) enum Qualification {
    Retention { now: i64, cutoff: i64 },
    Published { cutoff: i64 },
}

/// One member's current authority and pending facts, inside the writer transaction.
fn member_eligible(conn: &Connection, id: i64, qualification: Qualification) -> AppResult<bool> {
    let eligible: bool = match qualification {
        Qualification::Retention { now, cutoff } => conn.query_row("SELECT EXISTS(SELECT 1 FROM articles a WHERE id=?1 AND typeof(body_retention_since)='integer' AND body_retention_since>=0 AND body_retention_since<=?2 AND body_retention_since<=?3 AND NOT EXISTS(SELECT 1 FROM sync_queue WHERE article_id=a.id) AND NOT EXISTS(SELECT 1 FROM local_sync_body_hold WHERE article_id=a.id))", params![id, cutoff, now], |r| r.get(0))?,
        Qualification::Published { cutoff } => conn.query_row("SELECT EXISTS(SELECT 1 FROM articles a WHERE id=?1 AND unixepoch(published_at)<?2 AND NOT EXISTS(SELECT 1 FROM sync_queue WHERE article_id=a.id) AND NOT EXISTS(SELECT 1 FROM local_sync_body_hold WHERE article_id=a.id))", params![id, cutoff], |r| r.get(0))?,
    };
    if !eligible {
        return Ok(false);
    }
    Ok(crate::local_sync::logical::article_retention_state(conn, id)? == Some((true, false, true)))
}

/// Bound even a pathological equivalence prefix: >64 physical members is retained.
/// No LIMIT after enclosure or logical predicates (which could hide an unbounded scan).
fn eligible(conn: &Connection, id: i64, qualification: Qualification) -> AppResult<bool> {
    if !member_eligible(conn, id, qualification)? {
        return Ok(false);
    }
    type Group = (i64, Option<i64>, String, Option<String>, Option<String>);
    let Some((feed, rss, title, enclosure, mime)): Option<Group> = conn.query_row(
        "SELECT feed_id,rss_content_id,title,enclosure_url,enclosure_mime FROM articles WHERE id=?1", [id],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()? else { return Ok(false) };
    if let Some(rss) = rss {
        let mut stmt = conn.prepare("SELECT id,enclosure_url,enclosure_mime FROM articles INDEXED BY idx_articles_rss_group WHERE feed_id=?1 AND rss_content_id=?2 AND title=?3 ORDER BY id LIMIT 65")?;
        let rows = stmt
            .query_map(params![feed, rss, title], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if rows.len() > CANDIDATES {
            return Ok(false);
        }
        for (other, other_enclosure, other_mime) in rows {
            if other != id
                && other_enclosure.as_deref().unwrap_or("") == enclosure.as_deref().unwrap_or("")
                && other_mime.as_deref().unwrap_or("") == mime.as_deref().unwrap_or("")
                && !member_eligible(conn, other, qualification)?
            {
                return Ok(false);
            }
        }
    }
    // Holds without an article locator still own real content bytes. Use the
    // expression indexes shared with the contents recycling triggers.
    let held: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM articles a JOIN contents c ON c.id IN(a.content_id,a.rss_content_id) WHERE a.id=?1 AND (EXISTS(SELECT 1 FROM local_sync_body_hold WHERE json_extract(body_ref,'$.primary')=c.identity) OR EXISTS(SELECT 1 FROM local_sync_body_hold WHERE json_extract(body_ref,'$.rss')=c.identity)))", [id], |r| r.get(0))?;
    Ok(!held)
}

fn external_url(value: &str) -> bool {
    url::Url::parse(value)
        .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
}

/// Shared payload release for automatic and manual cleanup. The caller owns the
/// writer and qualifies immediately before calling. Restoring saved lightweight
/// metadata after the existing media trigger preserves gallery-first != cover.
fn release(conn: &Connection, id: i64) -> AppResult<bool> {
    let media: Option<(String, Option<String>, Option<String>)> = conn.query_row("SELECT images_json,image_url,enclosure_url FROM articles WHERE id=?1 AND (content_id IS NOT NULL OR rss_content_id IS NOT NULL)", [id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    let Some((images, cover, enclosure)) = media else {
        return Ok(false);
    };
    let images: Vec<String> = serde_json::from_str::<Vec<String>>(&images)?
        .into_iter()
        .filter(|url| external_url(url))
        .collect();
    let cover = cover.filter(|url| external_url(url));
    let enclosure = enclosure.filter(|url| external_url(url));
    let applying: bool = conn.query_row(
        "SELECT applying FROM local_sync_control WHERE id=1",
        [],
        |r| r.get(0),
    )?;
    conn.execute("UPDATE local_sync_control SET applying=1 WHERE id=1", [])?;
    let changed = conn.execute("UPDATE articles SET content_id=NULL,rss_content_id=NULL,summary=NULL,ai_summary=NULL,translated_content=NULL,ai_summary_sig=NULL,translated_sig=NULL,ai_epoch=ai_epoch+1,image_url=?2,enclosure_url=?3,enclosure_mime=CASE WHEN ?3 IS NOT NULL THEN enclosure_mime END,duration_sec=CASE WHEN ?3 IS NOT NULL THEN duration_sec END WHERE id=?1 AND (content_id IS NOT NULL OR rss_content_id IS NOT NULL)", params![id,cover,enclosure])?;
    // The body detach trigger may temporarily derive images from the cover. No
    // observer can see that intermediate value; capture remains suppressed.
    conn.execute("UPDATE articles SET images_json=?2,image_count=?3,media_revision=media_revision+1 WHERE id=?1", params![id,serde_json::to_string(&images)?,images.len() as i64])?;
    conn.execute("DELETE FROM ai_segments WHERE article_id=?1", [id])?;
    conn.execute(
        "UPDATE local_sync_control SET applying=?1 WHERE id=1",
        [applying],
    )?;
    Ok(changed > 0)
}

pub(super) fn release_qualified(
    conn: &Connection,
    id: i64,
    qualification: Qualification,
) -> AppResult<bool> {
    if !eligible(conn, id, qualification)? {
        return Ok(false);
    }
    release(conn, id)
}

/// Caller has the foreground mutex. A competing SQLite writer gets an immediate
/// skip instead of the connection's normal 5s wait; the original timeout is restored.
pub(crate) fn retire_read_bodies_slice(
    conn: &Connection,
    now: i64,
    cursor: &mut RetentionCursor,
) -> AppResult<RetentionReport> {
    let timeout: u64 = conn.pragma_query_value(None, "busy_timeout", |r| r.get(0))?;
    conn.busy_timeout(Duration::ZERO)?;
    let result = retire_inner(conn, now, cursor);
    let restore = conn.busy_timeout(Duration::from_millis(timeout));
    match (result, restore) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error.into()),
        (Ok(report), Ok(())) => Ok(report),
    }
}

fn retire_inner(
    conn: &Connection,
    now: i64,
    cursor: &mut RetentionCursor,
) -> AppResult<RetentionReport> {
    let mut report = RetentionReport::default();
    let days = retention_days(conn)?;
    if days == 0 || now < 0 || !quiescent(conn)? {
        return Ok(report);
    }
    let Some(cutoff) = now.checked_sub(days * 86400).filter(|v| *v >= 0) else {
        return Ok(report);
    };
    let started = Instant::now();
    let tx = super::try_write_transaction(conn)?;
    // Settings and logical work can change between the initial cheap read and
    // writer reservation. The actual eligibility uses this transaction only.
    if retention_days(&tx)? != days || !quiescent(&tx)? {
        return Ok(report);
    }
    let mut next = cursor.clone();
    let init_rows = {
        let mut stmt = tx.prepare(INITIALIZE_SQL)?;
        let rows = stmt
            .query_map(params![next.initialize, (CANDIDATES / 2) as i64], |r| {
                r.get::<_, i64>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let expired_rows = {
        let mut stmt = tx.prepare(EXPIRED_SQL)?;
        let rows = stmt
            .query_map(
                params![
                    next.expired.0,
                    next.expired.1,
                    cutoff,
                    (CANDIDATES / 2) as i64
                ],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let (mut init_index, mut expired_index) = (0, 0);
    while report.inspected < CANDIDATES
        && report.released < RELEASES
        && (report.inspected == 0 || started.elapsed() < SLICE)
    {
        let initialize = if init_index == init_rows.len() {
            false
        } else if expired_index == expired_rows.len() {
            true
        } else {
            next.initialize_next
        };
        next.initialize_next = !initialize;
        if initialize {
            let Some(&id) = init_rows.get(init_index) else {
                break;
            };
            init_index += 1;
            next.initialize = id;
            report.inspected += 1;
            if crate::local_sync::logical::article_retention_state(&tx, id)?
                == Some((true, false, true))
            {
                report.initialized+=tx.execute("UPDATE articles SET body_retention_since=?1 WHERE id=?2 AND body_retention_since IS NULL", params![now,id])?;
            }
        } else {
            let Some(&(id, since)) = expired_rows.get(expired_index) else {
                break;
            };
            expired_index += 1;
            next.expired = (since, id);
            report.inspected += 1;
            report.released += usize::from(release_qualified(
                &tx,
                id,
                Qualification::Retention { now, cutoff },
            )?);
        }
    }
    if init_index == init_rows.len() && init_rows.len() < CANDIDATES / 2 {
        next.initialize = 0;
    }
    if expired_index == expired_rows.len() && expired_rows.len() < CANDIDATES / 2 {
        next.expired = (-1, 0);
    }
    tx.commit()?;
    *cursor = next;
    Ok(report)
}

#[cfg(test)]
#[path = "body_retention_tests.rs"]
mod tests;
