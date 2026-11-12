//! The single SQL register join used by capture, transport and materialization.
//! Note: partial projection cannot delay or erase explicit local intentions —
//! see .agents/notes/implemented/architecture/2026-11-09-纯本地资料库同步与便携恢复.md.
use super::{database, model::*, storage::Manifest, transport_error};
use crate::{
    db,
    error::{AppError, AppResult},
    webdav_transport::{
        Batch, Checkpoint, FieldPatch, FieldValue, Frontier, Intent, Register, Stamp,
    },
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

pub const WINNERS: &str = "local_sync_winners";
const PROPOSED: &str = "temp.local_sync_proposed";
const ROWS: usize = 64;
/// Test-only accumulator for the retention drain, so the capacity test can
/// report that segment separately from article materialization.
#[cfg(test)]
static RETENTION_DRAIN_MICROS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
#[cfg(test)]
pub(super) fn retention_drain_take_ms() -> u128 {
    std::sync::atomic::AtomicU64::swap(
        &RETENTION_DRAIN_MICROS,
        0,
        std::sync::atomic::Ordering::Relaxed,
    ) as u128
        / 1000
}

/// The one fan-out table shared by capture's SQL triggers and [`touch_field`].
/// Change both sides together; `flx_work_fanout_matches_between_capture_paths`
/// fails when they drift.
///
/// - A ordinary article field (meta/read/star/body value changes, same source
///   and GUID): only the old/new `a/<entry>` keys, plus the display-alias
///   entries that share the same GUID *and* the same source URL — the alias set
///   is bounded by how many sources publish that URL, normally one. A metadata
///   edit is not a source-wide event: expanding every member of the source costs
///   O(articles) and cannot keep up with foreground writes. No `url/` key
///   either: a `url/` key is derived from `s/*/feed_url` registers alone, and
///   expanding one fans out to `source/`, which would re-expand the source.
/// - B article identity change (meta.source or meta.guid): falls out of A, the
///   old and the new entry are different keys and both are materialized.
/// - C source identity/lifecycle (`s/*/feed_url|created|deleted|reactivate`):
///   `source/<sid>` plus `url/<old|new url>` for feed_url.
/// - D folder membership (`s/*/folder`): `source/<sid>` plus `group/<folder>`.
/// - E group lifecycle (`g/*/deleted`): `group/<gid>`.
/// - F settings (`p/*`): `p`; retention keeps its own branch.
///
/// Capture's SQL path and Rust's explicit path maintain the same work table.
/// These triggers do not allocate operation stamps and are disabled only for
/// logic-preserving candidate folds (not ordinary remote/local register writes).
pub(super) fn capture_sql() -> String {
    let mut sql = String::new();
    for (event, suffix, old) in [
        ("INSERT", "insert", false),
        ("UPDATE OF register", "update", true),
    ] {
        let mut values=vec!["CASE WHEN NEW.key LIKE 'p/%' THEN 'p' WHEN NEW.key LIKE 'a/%' THEN substr(NEW.key,1,66) ELSE substr(NEW.key,1,34) END".to_owned()];
        // The existing work queue durably completes local cache-age changes.
        // Bootstrap only copies physical facts; it must not renew an old age.
        let changed = if old {
            "AND (NEW.key LIKE 'a/%/read' AND NEW.register!=OLD.register OR NEW.key LIKE 'a/%/star' AND json_extract(NEW.register,'$.value.value') IS NOT json_extract(OLD.register,'$.value.value'))"
        } else {
            "AND (NEW.key LIKE 'a/%/read' OR NEW.key LIKE 'a/%/star')"
        };
        values.push(format!("CASE WHEN (SELECT initialized=1 FROM local_sync_control WHERE id=1) {changed} THEN 'retention/'||NEW.key END"));
        values.push("CASE WHEN NEW.key LIKE 's/%/feed_url' OR NEW.key LIKE 's/%/created' OR NEW.key LIKE 's/%/deleted' OR NEW.key LIKE 's/%/reactivate' OR NEW.key LIKE 's/%/folder' THEN 'source/'||substr(NEW.key,3,32) END".into());
        values.push(
            "CASE WHEN NEW.key LIKE 'g/%/deleted' THEN 'group/'||substr(NEW.key,3,32) END".into(),
        );
        values.push("CASE WHEN NEW.key LIKE 's/%/feed_url' THEN 'url/'||json_extract(NEW.register,'$.value.value') END".into());
        values.push("CASE WHEN NEW.key LIKE 's/%/folder' THEN 'group/'||json_extract(NEW.register,'$.value.value') END".into());
        if old {
            values.push("CASE WHEN OLD.key LIKE 's/%/feed_url' THEN 'url/'||json_extract(OLD.register,'$.value.value') END".into());
        }
        if old {
            values.push("CASE WHEN OLD.key LIKE 's/%/folder' THEN 'group/'||json_extract(OLD.register,'$.value.value') END".into());
        }
        let rows = values
            .iter()
            .map(|v| format!("SELECT {v} k"))
            .collect::<Vec<_>>()
            .join(" UNION ALL ");
        sql+=&format!("CREATE TRIGGER IF NOT EXISTS flx_logical_work_{suffix} AFTER {event} ON local_sync_fields WHEN (SELECT folding=0 FROM local_sync_control WHERE id=1) BEGIN
          INSERT INTO local_sync_work(job,key,revision,cursor) SELECT '',k,(SELECT logical_revision FROM local_sync_control WHERE id=1),CASE WHEN k LIKE 'retention/%' THEN json_object('since',CAST(strftime('%s','now') AS INTEGER),'after',0) ELSE '' END FROM ({rows}) WHERE k IS NOT NULL
          ON CONFLICT(job,key) DO UPDATE SET revision=excluded.revision,cursor=excluded.cursor; END;\n");
    }
    sql
}

pub fn revision(conn: &Connection) -> AppResult<i64> {
    Ok(conn.query_row(
        "SELECT logical_revision FROM local_sync_control WHERE id=1",
        [],
        |r| r.get(0),
    )?)
}
pub fn enabled(conn: &Connection) -> AppResult<bool> {
    Ok(conn.query_row(
        "SELECT capture=1 AND applying=0 FROM local_sync_control WHERE id=1",
        [],
        |r| r.get(0),
    )?)
}
pub fn query_requires_logical_state(conn: &Connection) -> AppResult<bool> {
    Ok(conn.query_row(
        "SELECT (
            (capture=1 AND initialized=0)
            OR accepted_job IS NOT NULL
            OR folding!=0
            OR applying!=0
            OR EXISTS(SELECT 1 FROM local_sync_work WHERE job='')
            OR EXISTS(SELECT 1 FROM local_sync_intents)
         ) FROM local_sync_control WHERE id=1",
        [],
        |r| r.get(0),
    )?)
}
pub fn frontier(conn: &Connection) -> AppResult<Frontier> {
    Ok(serde_json::from_str(&conn.query_row(
        "SELECT frontier FROM local_sync_control WHERE id=1",
        [],
        |r| r.get::<_, String>(0),
    )?)?)
}
pub fn completed(conn: &Connection) -> AppResult<Frontier> {
    Ok(serde_json::from_str(&conn.query_row(
        "SELECT completed_frontier FROM local_sync_control WHERE id=1",
        [],
        |r| r.get::<_, String>(0),
    )?)?)
}
fn get_from(conn: &Connection, relation: &str, key: &str) -> AppResult<Option<Register>> {
    conn.query_row(
        &format!("SELECT register FROM {relation} WHERE key=?1"),
        [key],
        |r| r.get::<_, String>(0),
    )
    .optional()?
    .map(|v| serde_json::from_str(&v).map_err(Into::into))
    .transpose()
}
pub fn get(conn: &Connection, key: &str) -> AppResult<Option<Register>> {
    get_from(conn, WINNERS, key)
}
fn entity_from(
    conn: &Connection,
    relation: &str,
    prefix: &str,
) -> AppResult<BTreeMap<String, Register>> {
    let start = format!("{prefix}/");
    let end = format!("{prefix}0");
    let mut stmt = conn.prepare(&format!(
        "SELECT key,register FROM {relation} WHERE key>=?1 AND key<?2 ORDER BY key"
    ))?;
    let mut fields = BTreeMap::new();
    for row in stmt.query_map(params![start, end], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })? {
        let (key, raw) = row?;
        fields.insert(key, serde_json::from_str(&raw)?);
    }
    Ok(fields)
}
pub fn entity(conn: &Connection, prefix: &str) -> AppResult<BTreeMap<String, Register>> {
    entity_from(conn, WINNERS, prefix)
}
fn prefix(key: &str) -> &str {
    key.rsplit_once('/').map_or(key, |(p, _)| p)
}
fn base_winners_are_unshadowed(conn: &Connection, relation: &str) -> AppResult<bool> {
    if relation != WINNERS {
        return Ok(false);
    }
    Ok(conn.query_row(
        "SELECT accepted_job IS NULL FROM local_sync_control WHERE id=1",
        [],
        |r| r.get(0),
    )?)
}
fn source_alias_ids(conn: &Connection, relation: &str, url: &str) -> AppResult<Vec<String>> {
    let sql = if base_winners_are_unshadowed(conn, relation)? {
        "SELECT substr(key,3,32) FROM local_sync_fields
         WHERE key LIKE 's/%/feed_url' AND json_extract(register,'$.value.value')=?1
         ORDER BY key"
            .to_owned()
    } else {
        format!(
            "SELECT substr(key,3,32) FROM {relation}
             WHERE key LIKE 's/%/feed_url' AND json_extract(register,'$.value.value')=?1
             ORDER BY key"
        )
    };
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map([url], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}
/// Alias lookup query. This must never join two references to the relation:
/// when it is the proposed view (a UNION ALL of winners and candidates),
/// SQLite materializes the whole view once per reference, which measured
/// ~0.5 s per call and made whole-library preflight quadratic at 50k articles
/// (25,825 s extrapolated on r9-i1-50k-03). The GUID equality is pushed into
/// both branches through their partial expression indexes instead, and
/// article_alias_targets resolves each matching source URL by key.
fn alias_lookup_sql(relation: &str, unshadowed: bool) -> String {
    if unshadowed {
        "SELECT key,register FROM local_sync_fields
         WHERE key LIKE 'a/%/meta'
           AND json_extract(register,'$.value.value.guid')=?1
         ORDER BY key"
            .to_owned()
    } else {
        format!(
            "SELECT key,register FROM {relation}
             WHERE key LIKE 'a/%/meta'
               AND json_extract(register,'$.value.value.guid')=?1
             ORDER BY key"
        )
    }
}

fn article_alias_targets(
    conn: &Connection,
    relation: &str,
    guid: &str,
    url: &str,
) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare(&alias_lookup_sql(
        relation,
        base_winners_are_unshadowed(conn, relation)?,
    ))?;
    let rows = stmt
        .query_map([guid], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);
    let mut targets = Vec::new();
    for (key, raw) in rows {
        let reg: Register = serde_json::from_str(&raw)?;
        let FieldValue::Set(value) = reg.value else {
            continue;
        };
        let meta: ArticleMeta = serde_json::from_value(value)?;
        let feed_url =
            get_from(conn, relation, &format!("s/{}/feed_url", meta.source))?.and_then(|reg| {
                match reg.value {
                    FieldValue::Set(Value::String(url)) => Some(url),
                    _ => None,
                }
            });
        if feed_url.as_deref() == Some(url) {
            targets.push(key.chars().take(66).collect());
        }
    }
    Ok(targets)
}

#[cfg(test)]
pub(super) fn article_alias_targets_for_test(
    conn: &Connection,
    relation: &str,
    guid: &str,
    url: &str,
) -> AppResult<Vec<String>> {
    article_alias_targets(conn, relation, guid, url)
}

#[cfg(test)]
pub(super) fn alias_lookup_sql_for_test(relation: &str, unshadowed: bool) -> String {
    alias_lookup_sql(relation, unshadowed)
}

#[cfg(test)]
pub(super) fn proposed_view_for_test(conn: &Connection, job: &str) -> AppResult<()> {
    proposed_view(conn, job)
}

/// Work keys an article metadata patch must queue, for the capture trigger.
/// Mirrors the `a/*/meta` branch of [`touch_field`]: the entry itself plus the
/// bounded display-alias set for the same GUID and source URL. Returns entry
/// keys, never `url/` keys — expanding one of those would fan out to the whole
/// source.
pub(super) fn meta_alias_work(conn: &Connection, patch: &FieldPatch) -> AppResult<Vec<String>> {
    let mut keys = BTreeSet::new();
    for key in patch.keys() {
        if !key.starts_with("a/") || !key.ends_with("/meta") {
            continue;
        }
        let Some(Register {
            value: FieldValue::Set(v),
            ..
        }) = get_from(conn, WINNERS, key)?
        else {
            continue;
        };
        let meta: ArticleMeta = serde_json::from_value(v)?;
        keys.insert(format!("a/{}", entry_key(&meta.source, &meta.guid)));
        if let Some(Register {
            value: FieldValue::Set(Value::String(url)),
            ..
        }) = get_from(conn, WINNERS, &format!("s/{}/feed_url", meta.source))?
        {
            for alias in article_alias_targets(conn, WINNERS, &meta.guid, &url)? {
                keys.insert(alias);
            }
        }
    }
    Ok(keys.into_iter().collect())
}
fn merge_frontier(current: &mut Frontier, incoming: &Frontier) {
    for (actor, seq) in incoming {
        let v = current.entry(actor.clone()).or_default();
        *v = (*v).max(*seq);
    }
}
pub fn register_actor(conn: &Connection, actor: &str) -> AppResult<()> {
    let _writer = db::writer_guard(conn)?;
    if !valid_hex(actor, 32) {
        return Err(invalid("设备身份无效"));
    }
    let known: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM local_sync_known_actors WHERE actor=?1)",
        [actor],
        |r| r.get(0),
    )?;
    if !known {
        let count: i64 =
            conn.query_row("SELECT COUNT(*) FROM local_sync_known_actors", [], |r| {
                r.get(0)
            })?;
        if count >= 64 {
            return Err(invalid("设备身份超过格式容量"));
        }
        conn.execute(
            "INSERT INTO local_sync_known_actors(actor) VALUES(?1)",
            [actor],
        )?;
        conn.execute(
            "UPDATE local_sync_control SET logical_revision=logical_revision+1 WHERE id=1",
            [],
        )?;
    }
    Ok(())
}

/// Work and causal facts are committed by the same writer. A cursor is never
/// evidence that a subsequently inserted lower key has been visited.
pub fn touch(conn: &Connection, job: &str, key: &str) -> AppResult<()> {
    let _writer = db::writer_guard(conn)?;
    conn.execute("INSERT INTO local_sync_work(job,key,revision) VALUES(?1,?2,(SELECT logical_revision FROM local_sync_control WHERE id=1)) ON CONFLICT(job,key) DO UPDATE SET revision=excluded.revision,cursor=''",params![job,key])?;
    Ok(())
}
pub fn touch_field(
    conn: &Connection,
    job: &str,
    key: &str,
    old: Option<&Register>,
    new: &Register,
) -> AppResult<()> {
    let _writer = db::writer_guard(conn)?;
    if old.is_some_and(|old| old.stamp >= new.stamp) {
        return Ok(());
    }
    if key.starts_with("a/")
        && (key.ends_with("/read")
            || (key.ends_with("/star") && old.map(|old| &old.value) != Some(&new.value)))
        && conn.query_row(
            "SELECT initialized FROM local_sync_control WHERE id=1",
            [],
            |r| r.get::<_, bool>(0),
        )?
    {
        // A local explicit intent is timed now, not again when its physical
        // projection eventually runs. Candidate timing starts only after accept.
        conn.execute("INSERT INTO local_sync_work(job,key,revision,cursor) VALUES(?1,?2,(SELECT logical_revision FROM local_sync_control WHERE id=1),CASE WHEN ?1='' THEN json_object('since',CAST(strftime('%s','now') AS INTEGER),'after',0) ELSE '' END) ON CONFLICT(job,key) DO UPDATE SET revision=excluded.revision,cursor=excluded.cursor",params![job,format!("retention/{key}")])?;
    }
    touch(
        conn,
        job,
        if key.starts_with("p/") {
            "p"
        } else {
            prefix(key)
        },
    )?;
    // Rules C and D of the fan-out table documented on [capture_sql].
    if key.starts_with("s/")
        && matches!(
            key.rsplit('/').next(),
            Some("feed_url" | "created" | "deleted" | "reactivate" | "folder")
        )
    {
        touch(conn, job, &format!("source/{}", &prefix(key)[2..]))?;
        if key.ends_with("/feed_url") {
            for reg in old.into_iter().chain(std::iter::once(new)) {
                if let FieldValue::Set(Value::String(url)) = &reg.value {
                    touch(conn, job, &format!("url/{url}"))?;
                }
            }
        }
        if key.ends_with("/folder") {
            for reg in old.into_iter().chain(std::iter::once(new)) {
                if let FieldValue::Set(Value::String(group)) = &reg.value {
                    touch(conn, job, &format!("group/{group}"))?;
                }
            }
        }
    }
    // Rule E of the fan-out table.
    if key.starts_with("g/") && key.ends_with("/deleted") {
        touch(conn, job, &format!("group/{}", &prefix(key)[2..]))?;
    }
    // Rules A and B: an article edit re-materializes its own entries and the
    // few display aliases that share its GUID and source URL. The alias set is
    // bounded by the number of sources publishing the same URL, so this stays
    // O(1) per edit. It must not go through url/: expanding a url key fans out
    // to source/<id> and would re-expand the whole source, which is exactly the
    // unbounded work this rule removes.
    if key.starts_with("a/") && key.ends_with("/meta") {
        for reg in old.into_iter().chain(std::iter::once(new)) {
            if let FieldValue::Set(v) = &reg.value {
                let meta: ArticleMeta = serde_json::from_value(v.clone())?;
                touch(
                    conn,
                    job,
                    &format!("a/{}", entry_key(&meta.source, &meta.guid)),
                )?;
                if let Some(Register {
                    value: FieldValue::Set(Value::String(url)),
                    ..
                }) = get(conn, &format!("s/{}/feed_url", meta.source))?
                {
                    for alias in article_alias_targets(conn, WINNERS, &meta.guid, &url)? {
                        touch(conn, job, &alias)?;
                    }
                }
            }
        }
    }
    Ok(())
}
fn put(conn: &Connection, key: &str, reg: &Register, job: &str, track_work: bool) -> AppResult<()> {
    let old = get(conn, key)?;
    if let Some(old) = &old {
        if old.stamp == reg.stamp && old.value != reg.value {
            return Err(invalid("同操作戳包含不同值"));
        }
        if old.stamp >= reg.stamp {
            return Ok(());
        }
    }
    if track_work {
        touch_field(conn, job, key, old.as_ref(), reg)?;
    }
    conn.execute("INSERT INTO local_sync_fields(key,register) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET register=excluded.register",params![key,serde_json::to_string(reg)?])?;
    Ok(())
}

/// Explicit user patch, including physical no-ops. Caller owns one writer and
/// suppresses physical-table capture while projecting its direct target.
pub fn capture_patch(conn: &Connection, patch: &FieldPatch) -> AppResult<()> {
    capture_patch_inner(conn, patch, true, true)
}

/// Initialization records a snapshot of physical rows which already exist.
/// Keep the journal/winner/clock semantics, but do not schedule those rows for
/// projection back onto themselves. `folding=1` also suppresses field triggers;
/// real edits outside this transaction continue to enqueue normal durable work.
pub(super) fn capture_bootstrap_patch(conn: &Connection, patch: &FieldPatch) -> AppResult<()> {
    if patch.is_empty() {
        return Ok(());
    }
    if conn.is_autocommit() {
        let tx = db::write_transaction(conn)?;
        capture_bootstrap_patch(&tx, patch)?;
        tx.commit()?;
        return Ok(());
    }
    let previous_folding: bool = conn.query_row(
        "SELECT folding FROM local_sync_control WHERE id=1",
        [],
        |row| row.get(0),
    )?;
    conn.execute("UPDATE local_sync_control SET folding=1 WHERE id=1", [])?;
    let result = capture_patch_inner(conn, patch, false, true);
    let restored = conn.execute(
        "UPDATE local_sync_control SET folding=?1 WHERE id=1",
        [previous_folding],
    );
    match (result, restored) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error.into()),
        (Ok(()), Ok(_)) => Ok(()),
    }
}

#[cfg(test)]
thread_local! {
    static BOOTSTRAP_SNAPSHOT_BATCH_COUNT: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
    static BOOTSTRAP_SNAPSHOT_MAX_ROWS: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(super) fn reset_bootstrap_snapshot_batch_contract() {
    BOOTSTRAP_SNAPSHOT_BATCH_COUNT.set(0);
    BOOTSTRAP_SNAPSHOT_MAX_ROWS.set(0);
}

#[cfg(test)]
pub(super) fn bootstrap_snapshot_batch_count() -> usize {
    BOOTSTRAP_SNAPSHOT_BATCH_COUNT.get()
}

#[cfg(test)]
pub(super) fn bootstrap_snapshot_max_rows() -> usize {
    BOOTSTRAP_SNAPSHOT_MAX_ROWS.get()
}

/// Article rows are already materialized by definition during bootstrap. They
/// only need one durable local sequence entry and fill-only winners; replaying
/// the full dependency/alias projection for every row is quadratic. The caller
/// supplies one bounded slice, so the clock/frontier batch path covers all rows
/// while retaining one journal entry (and therefore one operation stamp) each.
pub(super) fn capture_bootstrap_snapshot_patches(
    conn: &Connection,
    patches: &[FieldPatch],
) -> AppResult<()> {
    let patches: Vec<_> = patches
        .iter()
        .filter(|patch| !patch.is_empty())
        .cloned()
        .collect();
    if patches.is_empty() {
        return Ok(());
    }
    if conn.is_autocommit() {
        let tx = db::write_transaction(conn)?;
        capture_bootstrap_snapshot_patches(&tx, &patches)?;
        tx.commit()?;
        return Ok(());
    }
    #[cfg(test)]
    {
        BOOTSTRAP_SNAPSHOT_BATCH_COUNT.set(BOOTSTRAP_SNAPSHOT_BATCH_COUNT.get() + 1);
        BOOTSTRAP_SNAPSHOT_MAX_ROWS.set(BOOTSTRAP_SNAPSHOT_MAX_ROWS.get().max(patches.len()));
    }
    let previous_folding: bool = conn.query_row(
        "SELECT folding FROM local_sync_control WHERE id=1",
        [],
        |row| row.get(0),
    )?;
    conn.execute("UPDATE local_sync_control SET folding=1 WHERE id=1", [])?;
    let result = capture_bootstrap_snapshot_patches_inner(conn, &patches);
    let restored = conn.execute(
        "UPDATE local_sync_control SET folding=?1 WHERE id=1",
        [previous_folding],
    );
    match (result, restored) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error.into()),
        (Ok(()), Ok(_)) => Ok(()),
    }
}

fn capture_bootstrap_snapshot_patches_inner(
    conn: &Connection,
    patches: &[FieldPatch],
) -> AppResult<()> {
    let (actor, mut seq, mut lamport, library_id, mut logical_revision): (
        String,
        u64,
        u64,
        String,
        i64,
    ) = conn.query_row(
        "SELECT actor,seq,lamport,library_id,logical_revision
         FROM local_sync_control WHERE id=1",
        [],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    )?;
    let mut current_frontier = frontier(conn)?;
    let mut intents = Vec::with_capacity(patches.len());
    for patch in patches {
        seq = seq
            .checked_add(1)
            .filter(|value| *value <= i64::MAX as u64)
            .ok_or_else(|| invalid("本机操作序号已耗尽"))?;
        lamport = lamport
            .checked_add(1)
            .filter(|value| *value <= i64::MAX as u64)
            .ok_or_else(|| invalid("逻辑时钟已耗尽"))?;
        intents.push(Intent {
            seq,
            lamport,
            observed: current_frontier.clone(),
            patch: patch.clone(),
        });
        current_frontier.insert(actor.clone(), seq);
    }
    Batch {
        format: 2,
        library_id: library_id.clone(),
        actor: actor.clone(),
        intents: intents.clone(),
    }
    .validate()
    .map_err(transport_error)?;

    let mut insert_journal = conn.prepare_cached(
        "INSERT INTO local_sync_journal(seq,lamport,observed,patch)
         VALUES(?1,?2,?3,?4)",
    )?;
    let mut read_field =
        conn.prepare_cached("SELECT register FROM local_sync_winners WHERE key=?1")?;
    let mut write_field = conn.prepare_cached(
        "INSERT INTO local_sync_fields(key,register) VALUES(?1,?2)
         ON CONFLICT(key) DO UPDATE SET register=excluded.register",
    )?;
    for intent in intents {
        let mut covered = intent.observed.clone();
        covered.insert(actor.clone(), intent.seq);
        validate_checkpoint(&Checkpoint {
            format: 2,
            library_id: library_id.clone(),
            covered,
            max_lamport: intent.lamport,
            fields: intent
                .patch
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        Register {
                            stamp: Stamp {
                                actor: actor.clone(),
                                seq: intent.seq,
                                lamport: intent.lamport,
                            },
                            value: value.clone(),
                        },
                    )
                })
                .collect(),
        })?;
        insert_journal.execute(params![
            intent.seq,
            intent.lamport,
            serde_json::to_string(&intent.observed)?,
            serde_json::to_string(&intent.patch)?
        ])?;
        let stamp = Stamp {
            actor: actor.clone(),
            seq: intent.seq,
            lamport: intent.lamport,
        };
        for (key, value) in intent.patch {
            let old = read_field
                .query_row([&key], |row| row.get::<_, String>(0))
                .optional()?
                .map(|raw| serde_json::from_str::<Register>(&raw))
                .transpose()?;
            let register = Register {
                stamp: stamp.clone(),
                value,
            };
            if let Some(old) = &old {
                if old.stamp == register.stamp && old.value != register.value {
                    return Err(invalid("同操作戳包含不同值"));
                }
                if old.stamp >= register.stamp {
                    continue;
                }
            }
            write_field.execute(params![key, serde_json::to_string(&register)?])?;
        }
        logical_revision = logical_revision
            .checked_add(1)
            .ok_or_else(|| invalid("逻辑修订已耗尽"))?;
    }
    conn.execute(
        "UPDATE local_sync_control
         SET seq=?1,lamport=?2,frontier=?3,logical_revision=?4 WHERE id=1",
        params![
            seq,
            lamport,
            serde_json::to_string(&current_frontier)?,
            logical_revision
        ],
    )?;
    Ok(())
}

fn capture_patch_inner(
    conn: &Connection,
    patch: &FieldPatch,
    track_work: bool,
    validate: bool,
) -> AppResult<()> {
    if patch.is_empty() {
        return Ok(());
    }
    if conn.is_autocommit() {
        let tx = db::write_transaction(conn)?;
        capture_patch_inner(&tx, patch, track_work, validate)?;
        tx.commit()?;
        return Ok(());
    }
    let affected = if validate {
        Some(affected_before(conn, patch)?)
    } else {
        None
    };
    let entries: Vec<_> = patch.iter().collect();
    for chunk in entries.chunks(64) {
        let (actor, seq, clock): (String, u64, u64) = conn.query_row(
            "SELECT actor,seq,lamport FROM local_sync_control WHERE id=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let seq = seq
            .checked_add(1)
            .filter(|v| *v <= i64::MAX as u64)
            .ok_or_else(|| invalid("本机操作序号已耗尽"))?;
        let lamport = clock
            .checked_add(1)
            .filter(|v| *v <= i64::MAX as u64)
            .ok_or_else(|| invalid("逻辑时钟已耗尽"))?;
        let observed = frontier(conn)?;
        let patch: FieldPatch = chunk
            .iter()
            .map(|(k, v)| ((*k).clone(), (*v).clone()))
            .collect();
        let intent = Intent {
            seq,
            lamport,
            observed: observed.clone(),
            patch: patch.clone(),
        };
        let library_id: String = conn.query_row(
            "SELECT library_id FROM local_sync_control WHERE id=1",
            [],
            |r| r.get(0),
        )?;
        Batch {
            format: 2,
            library_id: library_id.clone(),
            actor: actor.clone(),
            intents: vec![intent],
        }
        .validate()
        .map_err(transport_error)?;
        let mut covered = observed.clone();
        covered.insert(actor.clone(), seq);
        validate_checkpoint(&Checkpoint {
            format: 2,
            library_id,
            covered,
            max_lamport: lamport,
            fields: patch
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        Register {
                            stamp: Stamp {
                                actor: actor.clone(),
                                seq,
                                lamport,
                            },
                            value: value.clone(),
                        },
                    )
                })
                .collect(),
        })?;
        conn.execute(
            "UPDATE local_sync_control SET logical_revision=logical_revision+1 WHERE id=1",
            [],
        )?;
        conn.execute(
            "INSERT INTO local_sync_journal(seq,lamport,observed,patch) VALUES(?1,?2,?3,?4)",
            params![
                seq,
                lamport,
                serde_json::to_string(&observed)?,
                serde_json::to_string(&patch)?
            ],
        )?;
        let stamp = Stamp {
            actor: actor.clone(),
            seq,
            lamport,
        };
        for (key, value) in patch {
            put(
                conn,
                &key,
                &Register {
                    stamp: stamp.clone(),
                    value,
                },
                "",
                track_work,
            )?;
        }
        let mut next = observed;
        next.insert(actor, seq);
        conn.execute(
            "UPDATE local_sync_control SET seq=?1,lamport=?2,frontier=?3 WHERE id=1",
            params![seq, lamport, serde_json::to_string(&next)?],
        )?;
    }
    if let Some(affected) = affected {
        validate_affected(conn, patch, &affected)?;
    }
    Ok(())
}

/// Dependency writes are validated against the same L used by materialization.
/// Caller owns the writer, so a failure rolls back registers, journal and clock
/// together; flags/settings do not pay a whole-library projection scan.
pub(super) fn affected_before(
    conn: &Connection,
    patch: &FieldPatch,
) -> AppResult<BTreeSet<String>> {
    affected_before_from(conn, WINNERS, patch)
}
fn affected_before_from(
    conn: &Connection,
    relation: &str,
    patch: &FieldPatch,
) -> AppResult<BTreeSet<String>> {
    // Only a source register owns a URL alias cluster. An article's metadata
    // value cannot change which sources share its URL, so a meta key must not
    // contribute one: capture turns this set into durable `url/` work, and
    // expanding a url fans out to `source/` (see the fan-out table on
    // [`capture_sql`]). That extra hop expanded a whole 50k source for every
    // single-article metadata edit.
    let mut urls = BTreeSet::new();
    for key in patch.keys() {
        if key.starts_with("s/") {
            if let Some(Register {
                value: FieldValue::Set(Value::String(url)),
                ..
            }) = get_from(conn, relation, &format!("{}/feed_url", prefix(key)))?
            {
                urls.insert(url);
            }
        }
    }
    Ok(urls)
}
fn validate_affected(
    conn: &Connection,
    patch: &FieldPatch,
    old_urls: &BTreeSet<String>,
) -> AppResult<()> {
    validate_affected_from(conn, WINNERS, patch, old_urls)
}
fn validate_article_flag_patch(
    conn: &Connection,
    relation: &str,
    patch: &FieldPatch,
) -> AppResult<()> {
    let mut sources = BTreeSet::new();
    for key in patch.keys() {
        if !key.starts_with("a/") || !(key.ends_with("/read") || key.ends_with("/star")) {
            return Err(invalid("文章读藏校验收到非读藏字段"));
        }
        let target = prefix(key);
        let Some(Register {
            value: FieldValue::Set(value),
            ..
        }) = get_from(conn, relation, &format!("{target}/meta"))?
        else {
            // Physical capture can arrive before bootstrap reaches this article's
            // meta. The flag register remains fill-only and the later meta seed
            // will pair with it; the previous projection path treated a missing
            // target as empty rather than rejecting the user's edit.
            continue;
        };
        sources.insert(serde_json::from_value::<ArticleMeta>(value)?.source);
    }
    for source in sources {
        let fields = source_projection(conn, relation, &source)?;
        let source_prefix = fields
            .keys()
            .next()
            .map(|key| prefix(key))
            .ok_or_else(|| invalid("文章来源缺少创建记录"))?;
        if value(&fields, source_prefix, "created") != Some(&Value::Bool(true)) {
            return Err(invalid("文章来源缺少显式创建"));
        }
        if let Some(Value::String(group)) = value(&fields, source_prefix, "folder") {
            let group_prefix = format!("g/{group}");
            let group_fields = entity_from(conn, relation, &group_prefix)?;
            if value(&group_fields, &group_prefix, "created") != Some(&Value::Bool(true)) {
                return Err(invalid("来源引用的分类缺少显式创建"));
            }
        }
    }
    Ok(())
}
fn validate_affected_from(
    conn: &Connection,
    relation: &str,
    patch: &FieldPatch,
    old_urls: &BTreeSet<String>,
) -> AppResult<()> {
    if patch
        .keys()
        .all(|key| key.starts_with("a/") && (key.ends_with("/read") || key.ends_with("/star")))
    {
        return validate_article_flag_patch(conn, relation, patch);
    }
    // An article's original key fixes its source/GUID. Its own projection
    // validates every same-URL/GUID alias and the source/group dependencies;
    // other GUIDs only need revalidation when a source changes, so the URL
    // scopes below are derived from source registers alone.
    let mut urls = BTreeSet::new();
    if patch.keys().any(|key| key.starts_with("s/")) {
        urls.extend(old_urls.iter().cloned());
        urls.extend(affected_before_from(conn, relation, patch)?);
    }
    let targets: BTreeSet<_> = patch
        .keys()
        .map(|key| {
            if key.starts_with("p/") {
                String::from("p")
            } else {
                prefix(key).to_owned()
            }
        })
        .collect();
    for target in targets {
        projection_from(conn, relation, &target)?;
    }
    for url in urls.iter() {
        let mut stmt=conn.prepare(&format!("SELECT target FROM (SELECT substr(key,1,34) target FROM {relation} WHERE key LIKE 's/%/feed_url' AND json_extract(register,'$.value.value')=?1 UNION SELECT substr(m.key,1,66) FROM {relation} m JOIN {relation} s ON s.key='s/'||json_extract(m.register,'$.value.value.source')||'/feed_url' WHERE m.key LIKE 'a/%/meta' AND json_extract(s.register,'$.value.value')=?1) ORDER BY target"))?;
        let mut rows = stmt.query([url])?;
        while let Some(row) = rows.next()? {
            let target: String = row.get(0)?;
            projection_from(conn, relation, &target)?;
        }
    }
    Ok(())
}

/// A capture trigger runs this after its complete field patch, while SQLite can
/// still abort the whole originating statement. The old L URL scopes were put
/// in the existing work queue before the patch, including accepted C's values.
pub(super) fn validate_captured_patch(conn: &Connection, patch: &FieldPatch) -> AppResult<()> {
    let mut stmt=conn.prepare("SELECT substr(key,5) FROM local_sync_work WHERE job='' AND key LIKE 'url/%' AND revision=(SELECT logical_revision FROM local_sync_control WHERE id=1)")?;
    let urls = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<BTreeSet<_>>>()?;
    validate_affected(conn, patch, &urls)
}

pub fn capture_article_flag(
    conn: &Connection,
    id: i64,
    field: &str,
    value: bool,
) -> AppResult<bool> {
    let _writer = db::writer_guard(conn)?;
    if !enabled(conn)? {
        return Ok(false);
    }
    if !matches!(field, "read" | "star") {
        return Err(invalid("未知读藏字段"));
    }
    let Some((source, guid)) = article_identity(conn, id)? else {
        return Ok(false);
    };
    if get(conn, &format!("a/{}/meta", entry_key(&source, &guid)))?.is_none() {
        if !conn.query_row(
            "SELECT initialized FROM local_sync_control WHERE id=1",
            [],
            |r| r.get::<_, bool>(0),
        )? {
            return Ok(false);
        }
        return Err(invalid("文章定位与逻辑身份不一致"));
    }
    let url = source_projection(conn, WINNERS, &source)?
        .into_iter()
        .find_map(|(key, reg)| {
            if key.ends_with("/feed_url") {
                match reg.value {
                    FieldValue::Set(Value::String(url)) => Some(url),
                    _ => None,
                }
            } else {
                None
            }
        })
        .ok_or_else(|| invalid("文章来源URL缺失"))?;
    let mut patch = FieldPatch::new();
    for target in article_alias_targets(conn, WINNERS, &guid, &url)? {
        patch.insert(format!("{target}/{field}"), FieldValue::Set(json!(value)));
    }
    capture_patch(conn, &patch)?;
    // Suppressing physical capture must not suppress durable body protection.
    // Use the current logical reference, and only hold bytes actually present.
    if let Some((read, star, Some(body))) = article_state(conn, &source, &guid)? {
        if (!read && source_active(conn, &source)?) || star {
            hold_body(conn, id, &entry_key(&source, &guid), &body)?;
        }
    }
    conn.execute("UPDATE local_sync_control SET applying=1 WHERE id=1", [])?;
    Ok(true)
}
pub fn capture_settings(
    conn: &Connection,
    patch: &serde_json::Map<String, Value>,
) -> AppResult<bool> {
    let _writer = db::writer_guard(conn)?;
    if !enabled(conn)? {
        return Ok(false);
    }
    let patch = patch
        .iter()
        .filter(|(k, _)| SETTINGS.contains(&k.as_str()))
        .map(|(k, v)| (format!("p/{k}"), FieldValue::Set(v.clone())))
        .collect();
    capture_patch(conn, &patch)?;
    conn.execute("UPDATE local_sync_control SET applying=1 WHERE id=1", [])?;
    Ok(true)
}
pub fn logical_settings(
    conn: &Connection,
    current: &mut serde_json::Map<String, Value>,
) -> AppResult<()> {
    for (key, reg) in entity(conn, "p")? {
        match reg.value {
            FieldValue::Set(v) => {
                current.insert(key[2..].to_owned(), v);
            }
            FieldValue::Delete => {
                current.remove(&key[2..]);
            }
        }
    }
    Ok(())
}
pub fn finish_explicit(conn: &Connection, suppressed: bool) -> AppResult<()> {
    let _writer = db::writer_guard(conn)?;
    if suppressed {
        conn.execute("UPDATE local_sync_control SET applying=0 WHERE id=1", [])?;
    }
    Ok(())
}
/// Explicit source/category commands share capture's transaction, including a
/// physical no-op. Projecting the target first also prevents an old application
/// row from supplying values for fields the caller did not mention.
pub fn capture_entity_edit(
    conn: &Connection,
    kind: &str,
    id: i64,
    values: &serde_json::Map<String, Value>,
) -> AppResult<bool> {
    let _writer = db::writer_guard(conn)?;
    if values.is_empty() || !enabled(conn)? {
        return Ok(false);
    }
    let sql = match kind {
        "s" => format!(
            "SELECT sync_id FROM feeds WHERE id=?1 AND {}",
            local_source("feeds")
        ),
        "g" => "SELECT sync_id FROM folders WHERE id=?1 AND origin='local'".to_owned(),
        _ => return Err(invalid("未知显式对象类型")),
    };
    let uid: Option<String> = conn.query_row(&sql, [id], |r| r.get(0)).optional()?;
    let Some(uid) = uid else { return Ok(false) };
    let target = format!("{kind}/{uid}");
    if get(conn, &format!("{target}/created"))?.is_none() {
        return Ok(false);
    }
    let patch = values
        .iter()
        .map(|(key, value)| (format!("{target}/{key}"), FieldValue::Set(value.clone())))
        .collect();
    capture_patch(conn, &patch)?;
    if !values.contains_key("deleted") {
        materialize(conn, &target)?;
    }
    conn.execute("UPDATE local_sync_control SET applying=1 WHERE id=1", [])?;
    Ok(true)
}
pub fn reactivation_epoch(conn: &Connection, id: i64) -> AppResult<Option<Value>> {
    if !enabled(conn)? {
        return Ok(None);
    }
    let source: Option<String> = conn
        .query_row(
            &format!(
                "SELECT sync_id FROM feeds WHERE id=?1 AND {}",
                local_source("feeds")
            ),
            [id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(source) = source else {
        return Ok(None);
    };
    if get(conn, &format!("s/{source}/created"))?.is_none() {
        return Ok(None);
    }
    let fields = source_projection(conn, WINNERS, &source)?;
    let p = fields
        .keys()
        .next()
        .map(|key| prefix(key))
        .ok_or_else(|| invalid("来源缺创建记录"))?;
    if active_source(&fields, p) {
        return Ok(None);
    }
    fields
        .get(&format!("{p}/deleted"))
        .map(|deleted| serde_json::to_value(&deleted.stamp).map_err(Into::into))
        .transpose()
}
/// Fetch start and commit must observe the logical lifecycle immediately, even
/// if the background worker has not reached this feed's physical row yet.
pub fn feed_active(conn: &Connection, id: i64) -> AppResult<bool> {
    let row: Option<(String, bool, bool)> = conn
        .query_row(
            &format!(
                "SELECT sync_id,active,{} FROM feeds WHERE id=?1",
                local_source("feeds")
            ),
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((source, physical, local)) = row else {
        return Ok(false);
    };
    if !local || get(conn, &format!("s/{source}/created"))?.is_none() {
        return Ok(physical);
    }
    source_active(conn, &source)
}
fn article_identity(conn: &Connection, id: i64) -> AppResult<Option<(String, String)>> {
    let row:Option<(Option<String>,String,String)>=conn.query_row(&format!("SELECT a.local_sync_source,f.sync_id,a.guid FROM articles a JOIN feeds f ON f.id=a.feed_id WHERE a.id=?1 AND {}",local_source("f")),[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    let Some((source, feed, guid)) = row else {
        return Ok(None);
    };
    let source = match source {
        Some(source) => source,
        None if !conn.query_row(
            "SELECT initialized FROM local_sync_control WHERE id=1",
            [],
            |r| r.get::<_, bool>(0),
        )? =>
        {
            feed
        }
        None => {
            return Err(invalid(
                "文章缺少已验证的物化定位；不能以当前订阅行猜测来源",
            ))
        }
    };
    if conn.query_row(
        "SELECT initialized FROM local_sync_control WHERE id=1",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        match get(conn, &format!("a/{}/meta", entry_key(&source, &guid)))? {
            Some(Register {
                value: FieldValue::Set(v),
                ..
            }) if v["source"] == source && v["guid"] == guid => {}
            _ => return Err(invalid("文章定位与逻辑身份不一致")),
        }
    }
    Ok(Some((source, guid)))
}
pub(super) fn invalid_locator_sql() -> String {
    format!(
        "SELECT EXISTS(
           SELECT 1
           FROM articles a
           JOIN feeds f ON f.id=a.feed_id
           LEFT JOIN local_sync_winners m
             ON m.key='a/'||flx_entry(COALESCE(a.local_sync_source,''),a.guid)||'/meta'
           WHERE {} AND (SELECT initialized FROM local_sync_control WHERE id=1)=1
             AND (m.key IS NULL
               OR json_extract(m.register,'$.value.value.source') IS NOT a.local_sync_source
               OR json_extract(m.register,'$.value.value.guid') IS NOT a.guid)
         )",
        local_source("f")
    )
}

pub(super) fn require_valid_locators(conn: &Connection) -> AppResult<()> {
    let invalid_locator: bool = conn.query_row(&invalid_locator_sql(), [], |row| row.get(0))?;
    if invalid_locator {
        return Err(invalid(
            "文章物化定位未通过逻辑身份校验，保留正文并暂停相关同步/导出",
        ));
    }
    Ok(())
}
pub(super) fn hold_body(conn: &Connection, id: i64, entry: &str, body: &BodyRef) -> AppResult<()> {
    let _writer = db::writer_guard(conn)?;
    let result = conn.execute("INSERT OR IGNORE INTO local_sync_body_hold(article_id,entry,body_ref) SELECT ?1,?2,?3 WHERE EXISTS(SELECT 1 FROM contents WHERE identity=?4 AND (COALESCE(content_html,'')!='' OR body_text!='')) AND (?5 IS NULL OR EXISTS(SELECT 1 FROM contents WHERE identity=?5 AND (COALESCE(content_html,'')!='' OR body_text!='')))",params![id,entry,serde_json::to_string(body)?,body.primary,body.rss]);
    result?;
    Ok(())
}
fn logical_article_meta(
    fields: &BTreeMap<String, Register>,
    article: &str,
) -> AppResult<ArticleMeta> {
    serde_json::from_value(
        value(fields, article, "meta")
            .cloned()
            .ok_or_else(|| invalid("读藏成员无元数据"))?,
    )
    .map_err(Into::into)
}
fn matches_read_group(
    fields: &BTreeMap<String, Register>,
    article: &str,
    target: &ArticleMeta,
    rss: Option<&str>,
) -> AppResult<bool> {
    let member = logical_article_meta(fields, article)?;
    if member.guid == target.guid {
        return Ok(true);
    }
    let Some(rss) = rss else {
        return Ok(false);
    };
    let same_rss = value(fields, article, "body")
        .and_then(|body| body.get("rss"))
        .and_then(Value::as_str)
        == Some(rss);
    Ok(same_rss
        && member.title == target.title
        && member.enclosure_url.as_deref().unwrap_or("")
            == target.enclosure_url.as_deref().unwrap_or("")
        && member.enclosure_mime.as_deref().unwrap_or("")
            == target.enclosure_mime.as_deref().unwrap_or(""))
}
fn read_target_keys(
    conn: &Connection,
    target: &str,
    target_meta: &ArticleMeta,
    url: &str,
    rss: Option<&str>,
) -> AppResult<BTreeSet<String>> {
    let mut targets = BTreeSet::new();
    targets.insert(target.to_owned());
    targets.extend(article_alias_targets(
        conn,
        WINNERS,
        &target_meta.guid,
        url,
    )?);
    let Some(rss) = rss else {
        return Ok(targets);
    };
    let base_sql = "SELECT substr(b.key,1,66)
        FROM local_sync_fields b
        JOIN local_sync_winners m ON m.key=substr(b.key,1,66)||'/meta'
        JOIN local_sync_winners s ON s.key='s/'||json_extract(m.register,'$.value.value.source')||'/feed_url'
        WHERE b.key LIKE 'a/%/body'
          AND json_extract(b.register,'$.value.value.rss')=?1
          AND json_extract(s.register,'$.value.value')=?2
          AND json_extract(m.register,'$.value.value.title')=?3
          AND COALESCE(json_extract(m.register,'$.value.value.enclosure_url'),'')=?4
          AND COALESCE(json_extract(m.register,'$.value.value.enclosure_mime'),'')=?5";
    let mut stmt = conn.prepare(base_sql)?;
    targets.extend(
        stmt.query_map(
            params![
                rss,
                url,
                target_meta.title,
                target_meta.enclosure_url.as_deref().unwrap_or(""),
                target_meta.enclosure_mime.as_deref().unwrap_or("")
            ],
            |row| row.get::<_, String>(0),
        )?
        .collect::<rusqlite::Result<BTreeSet<_>>>()?,
    );
    drop(stmt);

    let accepted_sql = "SELECT substr(b.key,1,66)
        FROM local_sync_candidates b
        JOIN local_sync_winners m ON m.key=substr(b.key,1,66)||'/meta'
        JOIN local_sync_winners s ON s.key='s/'||json_extract(m.register,'$.value.value.source')||'/feed_url'
        WHERE b.job=(SELECT accepted_job FROM local_sync_control WHERE id=1)
          AND b.key LIKE 'a/%/body'
          AND json_extract(b.register,'$.value.value.rss')=?1
          AND json_extract(s.register,'$.value.value')=?2
          AND json_extract(m.register,'$.value.value.title')=?3
          AND COALESCE(json_extract(m.register,'$.value.value.enclosure_url'),'')=?4
          AND COALESCE(json_extract(m.register,'$.value.value.enclosure_mime'),'')=?5";
    let mut stmt = conn.prepare(accepted_sql)?;
    targets.extend(
        stmt.query_map(
            params![
                rss,
                url,
                target_meta.title,
                target_meta.enclosure_url.as_deref().unwrap_or(""),
                target_meta.enclosure_mime.as_deref().unwrap_or("")
            ],
            |row| row.get::<_, String>(0),
        )?
        .collect::<rusqlite::Result<BTreeSet<_>>>()?,
    );
    Ok(targets)
}
pub fn read_targets(conn: &Connection, id: i64) -> AppResult<Option<Vec<(i64, i64, bool)>>> {
    if !enabled(conn)? {
        return Ok(None);
    }
    let Some((source, guid)) = article_identity(conn, id)? else {
        return Ok(None);
    };
    let target = format!("a/{}", entry_key(&source, &guid));
    if get(conn, &format!("{target}/meta"))?.is_none() {
        return Ok(None);
    }
    let target_fields = projected(conn, &target)?;
    let target_prefix = target_fields
        .keys()
        .find(|k| k.starts_with("a/"))
        .map(|k| prefix(k))
        .ok_or_else(|| invalid("读藏目标元数据缺失"))?;
    let meta: ArticleMeta = serde_json::from_value(
        value(&target_fields, target_prefix, "meta")
            .cloned()
            .ok_or_else(|| invalid("读藏目标缺身份"))?,
    )?;
    let source_fields = source_projection(conn, WINNERS, &source)?;
    let source_prefix = source_fields
        .keys()
        .next()
        .map(|k| prefix(k))
        .ok_or_else(|| invalid("读藏目标源缺失"))?;
    let url = value(&source_fields, source_prefix, "feed_url")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("读藏目标源URL缺失"))?;
    let rss = value(&target_fields, target_prefix, "body")
        .and_then(|v| v.get("rss"))
        .and_then(Value::as_str);
    let mut targets = BTreeMap::new();
    let keys = read_target_keys(conn, &target, &meta, url, rss)?;
    for p in keys {
        let fields = projected(conn, &p)?;
        let a = fields
            .keys()
            .find(|k| k.starts_with("a/"))
            .map(|k| prefix(k))
            .ok_or_else(|| invalid("读藏成员缺失"))?;
        if !matches_read_group(&fields, a, &meta, rss)? {
            continue;
        }
        let m = logical_article_meta(&fields, a)?;
        materialize(conn, &p)?;
        let row:Option<(i64,i64)>=conn.query_row("SELECT a.id,a.feed_id FROM articles a JOIN feeds f ON f.id=a.feed_id WHERE f.feed_url=?1 AND a.guid=?2",params![url,m.guid],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        if let Some((id, feed)) = row {
            targets.insert(
                id,
                (feed, value(&fields, a, "read") != Some(&Value::Bool(true))),
            );
        }
    }
    Ok(Some(
        targets
            .into_iter()
            .map(|(id, (feed, unread))| (id, feed, unread))
            .collect(),
    ))
}

fn source_projection(
    conn: &Connection,
    relation: &str,
    source: &str,
) -> AppResult<BTreeMap<String, Register>> {
    let reg = get_from(conn, relation, &format!("s/{source}/feed_url"))?
        .ok_or_else(|| invalid("来源缺少URL"))?;
    let FieldValue::Set(Value::String(url)) = reg.value else {
        return Err(invalid("来源URL类型无效"));
    };
    let mut out = Projection::default();
    let mut canonical = None;
    for id in source_alias_ids(conn, relation, &url)? {
        let target = canonical.get_or_insert_with(|| id.clone()).clone();
        for (key, reg) in entity_from(conn, relation, &format!("s/{id}"))? {
            out.select(
                format!("s/{target}/{}", key.rsplit('/').next().unwrap_or_default()),
                &key,
                reg,
            )?;
        }
    }
    Ok(out.into_fields())
}
/// At most one projected entity plus its source/group dependencies is retained.
fn projection_from(
    conn: &Connection,
    relation: &str,
    target: &str,
) -> AppResult<BTreeMap<String, Register>> {
    if target == "p" {
        return entity_from(conn, relation, "p");
    }
    if target.starts_with("g/") {
        let fields = entity_from(conn, relation, target)?;
        validate_group_entity(&fields, target)?;
        return Ok(fields);
    }
    let source = if let Some(source) = target.strip_prefix("s/") {
        source.to_owned()
    } else {
        let Some(reg) = get_from(conn, relation, &format!("{target}/meta"))? else {
            return Ok(BTreeMap::new());
        };
        let FieldValue::Set(v) = reg.value else {
            return Err(invalid("文章元数据无效"));
        };
        serde_json::from_value::<ArticleMeta>(v)?.source
    };
    let mut fields = source_projection(conn, relation, &source)?;
    let source_prefix = fields
        .keys()
        .next()
        .map(|k| prefix(k).to_owned())
        .ok_or_else(|| invalid("来源缺少创建字段"))?;
    if value(&fields, &source_prefix, "created") != Some(&Value::Bool(true)) {
        return Err(invalid("来源缺少显式创建"));
    }
    if let Some(Value::String(group)) = value(&fields, &source_prefix, "folder") {
        let group = group.clone();
        let group_prefix = format!("g/{group}");
        let group_fields = entity_from(conn, relation, &group_prefix)?;
        if value(&group_fields, &group_prefix, "created") != Some(&Value::Bool(true)) {
            return Err(invalid("来源引用的分类缺少显式创建"));
        }
        fields.extend(group_fields);
    }
    if target.starts_with("a/") {
        let Some(Register {
            value: FieldValue::Set(v),
            ..
        }) = get_from(conn, relation, &format!("{target}/meta"))?
        else {
            return Err(invalid("文章缺少元数据"));
        };
        let meta: ArticleMeta = serde_json::from_value(v)?;
        let url = value(&fields, &source_prefix, "feed_url")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("来源URL无效"))?
            .to_owned();
        let canonical = &source_prefix[2..];
        let projected = format!("a/{}", entry_key(canonical, &meta.guid));
        let mut articles = Projection::default();
        for target in article_alias_targets(conn, relation, &meta.guid, &url)? {
            for (key, mut reg) in entity_from(conn, relation, &target)? {
                if key.ends_with("/meta") {
                    if let FieldValue::Set(v) = &mut reg.value {
                        v["source"] = json!(canonical);
                    }
                }
                articles.select(
                    format!("{projected}/{}", key.rsplit('/').next().unwrap_or_default()),
                    &key,
                    reg,
                )?;
            }
        }
        fields.extend(articles.into_fields());
    }
    Ok(fields)
}
pub fn projected(conn: &Connection, target: &str) -> AppResult<BTreeMap<String, Register>> {
    projection_from(conn, WINNERS, target)
}
pub fn source_active(conn: &Connection, source: &str) -> AppResult<bool> {
    if get(conn, &format!("s/{source}/created"))?.is_none() {
        return Ok(true);
    }
    let fields = source_projection(conn, WINNERS, source)?;
    Ok(fields
        .keys()
        .next()
        .is_some_and(|k| active_source(&fields, prefix(k))))
}
pub fn article_state(
    conn: &Connection,
    source: &str,
    guid: &str,
) -> AppResult<Option<(bool, bool, Option<BodyRef>)>> {
    let target = format!("a/{}", entry_key(source, guid));
    if get(conn, &format!("{target}/meta"))?.is_none() {
        return Ok(None);
    }
    let fields = projected(conn, &target)?;
    let article = fields
        .keys()
        .find(|k| k.starts_with("a/"))
        .map(|k| prefix(k))
        .ok_or_else(|| invalid("文章投影缺失"))?;
    Ok(Some((
        value(&fields, article, "read") == Some(&Value::Bool(true)),
        value(&fields, article, "star") == Some(&Value::Bool(true)),
        value(&fields, article, "body")
            .cloned()
            .map(serde_json::from_value)
            .transpose()?,
    )))
}

/// Point lookup for cache maintenance. Avoid the global source-value window
/// views on every candidate; current projected L remains the authority.
pub(crate) fn article_retention_state(
    conn: &Connection,
    id: i64,
) -> AppResult<Option<(bool, bool, bool)>> {
    let physical: Option<(bool,bool,bool,bool,bool)> = conn.query_row(
        &format!("SELECT a.is_read,a.is_starred,f.active,{},(SELECT initialized FROM local_sync_control WHERE id=1) FROM articles a JOIN feeds f ON f.id=a.feed_id WHERE a.id=?1", local_source("f")), [id],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
    let Some((read, star, active, local, initialized)) = physical else {
        return Ok(None);
    };
    if !local || !initialized {
        return Ok(Some((read, star, active)));
    }
    let Some((source, guid)) = article_identity(conn, id)? else {
        return Ok(None);
    };
    let Some((read, star, _)) = article_state(conn, &source, &guid)? else {
        return Ok(None);
    };
    Ok(Some((read, star, source_active(conn, &source)?)))
}

/// Runs after ordinary projection and dependency expansion. The existing job
/// and cursor survive restart; GC stays paused until this last local-age work is
/// committed. No second winner/token/age is stored on article rows.
fn retention_work(conn: &Connection, job: &str, key: &str, cursor: &str) -> AppResult<bool> {
    let field = key
        .strip_prefix("retention/")
        .ok_or_else(|| invalid("正文保留工作键无效"))?;
    let expected = if job.is_empty() {
        get_from(conn, "local_sync_fields", field)?
    } else {
        conn.query_row(
            "SELECT register FROM local_sync_candidates WHERE job=?1 AND key=?2",
            params![job, field],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .map(|raw| serde_json::from_str::<Register>(&raw))
        .transpose()?
    };
    let Some(expected) = expected else {
        return Ok(true);
    };
    if get(conn, field)?.as_ref() != Some(&expected) {
        return Ok(true);
    }
    let target = prefix(field);
    let fields = projected(conn, target)?;
    let Some(projected) = fields
        .keys()
        .find(|key| key.starts_with("a/"))
        .map(|key| prefix(key))
    else {
        return Ok(true);
    };
    let name = field
        .rsplit('/')
        .next()
        .ok_or_else(|| invalid("正文保留字段无效"))?;
    if fields.get(&format!("{projected}/{name}")) != Some(&expected) {
        return Ok(true);
    }
    let Some(Register {
        value: FieldValue::Set(meta),
        ..
    }) = get(conn, &format!("{target}/meta"))?
    else {
        return Ok(true);
    };
    let meta: ArticleMeta = serde_json::from_value(meta)?;
    let Some(Register {
        value: FieldValue::Set(Value::String(url)),
        ..
    }) = get(conn, &format!("s/{}/feed_url", meta.source))?
    else {
        return Ok(true);
    };
    let (since, after) = if cursor.is_empty() {
        (chrono::Utc::now().timestamp(), 0)
    } else {
        let value: Value = serde_json::from_str(cursor)?;
        (
            value["since"]
                .as_i64()
                .filter(|v| *v >= 0)
                .ok_or_else(|| invalid("正文保留时间无效"))?,
            value["after"]
                .as_i64()
                .filter(|v| *v >= 0)
                .ok_or_else(|| invalid("正文保留游标无效"))?,
        )
    };
    let mut stmt=conn.prepare("SELECT id FROM (SELECT id FROM articles WHERE local_sync_source=?1 AND guid=?2 AND id>?4 UNION SELECT a.id FROM feeds f JOIN articles a ON a.feed_id=f.id AND a.guid=?2 WHERE f.feed_url=?3 AND a.id>?4) ORDER BY id LIMIT 64")?;
    let rows = stmt
        .query_map(params![meta.source, meta.guid, url, after], |r| {
            r.get::<_, i64>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);
    let started = Instant::now();
    let mut processed = 0;
    for id in &rows {
        // A restore or newer explicit action may already have a later local
        // origin. Completing old work must not shorten that new retention period.
        db::body_retention::complete_origin(conn, *id, since)?;
        conn.execute(
            "UPDATE local_sync_work SET cursor=?1 WHERE job=?2 AND key=?3",
            params![json!({"since":since,"after":id}).to_string(), job, key],
        )?;
        processed += 1;
        if started.elapsed() >= Duration::from_millis(15) {
            break;
        }
    }
    Ok(processed == rows.len() && rows.len() < ROWS)
}
pub fn materialize(conn: &Connection, target: &str) -> AppResult<()> {
    let _writer = db::writer_guard(conn)?;
    let fields = projected(conn, target)?;
    if fields.is_empty() {
        return Ok(());
    }
    let applying: bool = conn.query_row(
        "SELECT applying FROM local_sync_control WHERE id=1",
        [],
        |r| r.get(0),
    )?;
    conn.execute("UPDATE local_sync_control SET applying=1 WHERE id=1", [])?;
    let origin = if target.starts_with("a/") {
        get(conn, &format!("{target}/meta"))?
            .and_then(|reg| match reg.value {
                FieldValue::Set(value) => Some(value),
                _ => None,
            })
            .map(serde_json::from_value::<ArticleMeta>)
            .transpose()?
    } else {
        None
    };
    database::apply_projection(conn, &fields, origin.as_ref())?;
    if let Some(source) = target.strip_prefix("s/") {
        let old:Option<i64>=conn.query_row("SELECT f.id FROM feeds f JOIN local_sync_source_urls s ON s.source=f.sync_id WHERE f.sync_id=?1 AND f.feed_url!=s.url AND EXISTS(SELECT 1 FROM feeds target WHERE target.feed_url=s.url AND target.id!=f.id)",[source],|r|r.get(0)).optional()?;
        if let Some(old) = old {
            touch(conn, "", &format!("cleanup-feed/{old}"))?;
        }
    }
    conn.execute(
        "UPDATE local_sync_control SET applying=?1,data_revision=data_revision+1 WHERE id=1",
        [applying],
    )?;
    Ok(())
}

pub fn begin_receive(conn: &Connection, generation: i64, manifest: &Manifest) -> AppResult<String> {
    manifest.validate()?;
    let raw = serde_json::to_string(manifest)?;
    let id = digest(raw.as_bytes());
    let tx = db::write_transaction(conn)?;
    database::require_generation(&tx, generation)?;
    let library: String = tx.query_row(
        "SELECT library_id FROM local_sync_control WHERE id=1",
        [],
        |r| r.get(0),
    )?;
    if library != manifest.library_id {
        return Err(invalid("不同资料库的候选"));
    }
    let rejected:Option<String>=tx.query_row("SELECT rejected_reason FROM local_sync_receive WHERE id=?1 AND phase='rejected' AND local_actor=?2 AND library_id=?3",params![id,database::actor(&tx)?,library],|r|r.get(0)).optional()?;
    if let Some(reason) = rejected {
        return Err(AppError::new("localSyncRejected", reason));
    }
    register_actor(&tx, &manifest.actor)?;
    for actor in manifest.covered.keys() {
        register_actor(&tx, actor)?;
    }
    tx.execute("INSERT INTO local_sync_receive(id,generation,local_actor,library_id,manifest) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(id) DO NOTHING",params![id,generation,database::actor(&tx)?,library,raw])?;
    // A restarted connection may re-claim unaccepted durable bytes. Accepted
    // jobs belong to the library/actor, and continue without a network session.
    tx.execute("UPDATE local_sync_receive SET generation=?1 WHERE id=?2 AND phase='staging' AND library_id=?3 AND local_actor=?4",params![generation,id,library,database::actor(&tx)?])?;
    tx.commit()?;
    Ok(id)
}

/// Only a deterministic invalid immutable input can become rejected. IO, missing
/// pages and revision races keep staging retryable. No accepted candidate, other
/// pending intent or known actor is removed by this lifecycle transition.
pub fn reject_candidate(
    conn: &Connection,
    generation: i64,
    job: &str,
    error: &AppError,
) -> AppResult<bool> {
    reject_candidate_checked(conn, generation, job, error, None)
}
fn reject_candidate_checked(
    conn: &Connection,
    generation: i64,
    job: &str,
    error: &AppError,
    expected_revision: Option<i64>,
) -> AppResult<bool> {
    if !matches!(
        error.code.as_str(),
        "localSyncFormat" | "localSyncConflict" | "json"
    ) {
        return Ok(false);
    }
    let tx = db::write_transaction(conn)?;
    database::require_generation(&tx, generation)?;
    if let Some(expected) = expected_revision {
        if revision(&tx)? != expected {
            return Err(AppError::new(
                "localSyncPending",
                "拒绝判断期间逻辑输入已变化，保留候选重验",
            ));
        }
    }
    let (owner,library,raw,phase,epoch):(String,String,String,String,i64)=tx.query_row("SELECT local_actor,library_id,manifest,phase,generation FROM local_sync_receive WHERE id=?1",[job],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?;
    let identity:bool=tx.query_row("SELECT actor=?1 AND library_id=?2 AND (accepted_job IS NULL OR accepted_job!=?3) FROM local_sync_control WHERE id=1",params![owner,library,job],|r|r.get(0))?;
    if !identity || epoch != generation || digest(raw.as_bytes()) != job {
        return Err(AppError::new("staleLocalSync", "候选拒绝身份或状态已失效"));
    }
    if phase == "rejected" {
        return Ok(true);
    }
    if phase != "staging" {
        return Err(AppError::new("localSyncPending", "已接受候选不能丢弃"));
    }
    let reading:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM local_sync_roots WHERE kind='validation-reader' AND reference=?1)",[job],|r|r.get(0))?;
    if reading {
        return Err(AppError::new(
            "localSyncPending",
            "候选预验读者仍在使用数据",
        ));
    }
    tx.execute("UPDATE local_sync_receive SET phase='rejected',rejected_code=?1,rejected_reason=?2,validated_revision=NULL WHERE id=?3 AND phase='staging'",params![error.code,error.message,job])?;
    tx.execute(
        "DELETE FROM local_sync_roots WHERE owner=?1 AND kind='validation' AND reference=?2",
        params![format!("validation/{job}"), job],
    )?;
    tx.execute("DELETE FROM local_sync_work WHERE job=?1", [job])?;
    tx.execute("DELETE FROM local_sync_downloads WHERE manifest=?1", [raw])?;
    tx.commit()?;
    Ok(true)
}

pub(super) struct ValidationReader<'a> {
    conn: &'a Connection,
    owner: String,
    generation: i64,
}
impl<'a> ValidationReader<'a> {
    pub(super) fn new(conn: &'a Connection, job: &str) -> AppResult<Self> {
        let tx = db::write_transaction(conn)?;
        let (owner, library, phase, epoch): (String, String, String, i64) = tx.query_row(
            "SELECT local_actor,library_id,phase,generation FROM local_sync_receive WHERE id=?1",
            [job],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        let matches:bool=tx.query_row("SELECT actor=?1 AND library_id=?2 AND generation=?3 AND accepted_job IS NULL FROM local_sync_control WHERE id=1",params![owner,library,epoch],|r|r.get(0))?;
        if phase != "staging" || !matches {
            return Err(AppError::new("localSyncPending", "候选当前不可预验"));
        }
        let generation = database::generation(&tx)?;
        let owner = format!("validation-reader/{}", super::storage::random_id(&tx)?);
        tx.execute(
            "INSERT INTO local_sync_roots(owner,kind,reference) VALUES(?1,'validation-reader',?2)",
            params![owner, job],
        )?;
        tx.commit()?;
        Ok(Self {
            conn,
            owner,
            generation,
        })
    }
    fn close(&self) -> AppResult<()> {
        let _writer = db::writer_guard(self.conn)?;
        self.conn.execute(
            "DELETE FROM local_sync_roots WHERE owner=?1 AND kind='validation-reader'",
            [&self.owner],
        )?;
        Ok(())
    }
}
impl Drop for ValidationReader<'_> {
    fn drop(&mut self) {
        if let Err(error) = self.close() {
            log::warn!("candidate reader root retained: {}", error.message);
        }
    }
}

pub fn stage_page(
    conn: &Connection,
    generation: i64,
    job: &str,
    page: usize,
    fields: &BTreeMap<String, Register>,
) -> AppResult<()> {
    let result = stage_page_inner(conn, generation, job, page, fields);
    if let Err(error) = &result {
        reject_candidate(conn, generation, job, error)?;
    }
    result
}
fn stage_page_inner(
    conn: &Connection,
    generation: i64,
    job: &str,
    page: usize,
    fields: &BTreeMap<String, Register>,
) -> AppResult<()> {
    let raw: String = conn.query_row(
        "SELECT manifest FROM local_sync_receive WHERE id=?1",
        [job],
        |r| r.get(0),
    )?;
    let manifest: Manifest = serde_json::from_str(&raw)?;
    validate_checkpoint(&Checkpoint {
        format: 2,
        library_id: manifest.library_id,
        covered: manifest.covered,
        max_lamport: manifest.max_lamport,
        fields: fields.clone(),
    })?;
    let items: Vec<_> = fields.iter().collect();
    for chunk in items.chunks(ROWS) {
        let tx = db::write_transaction(conn)?;
        database::require_generation(&tx, generation)?;
        let (phase, next): (String, usize) = tx.query_row(
            "SELECT phase,next_page FROM local_sync_receive WHERE id=?1",
            [job],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if phase != "staging" || next > page {
            tx.commit()?;
            return Ok(());
        }
        if next != page {
            return Err(AppError::new(
                "localSyncPending",
                "候选页游标已变化，等待重新领取",
            ));
        }
        for (key, reg) in chunk {
            let old = tx
                .query_row(
                    "SELECT register FROM local_sync_candidates WHERE job=?1 AND key=?2",
                    params![job, key],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
                .map(|s| serde_json::from_str::<Register>(&s))
                .transpose()?;
            if let Some(old) = old {
                if old.stamp == reg.stamp && old.value != reg.value {
                    return Err(invalid("候选跨页同戳冲突"));
                }
                if old.stamp >= reg.stamp {
                    continue;
                }
            }
            tx.execute("INSERT INTO local_sync_candidates(job,key,register) VALUES(?1,?2,?3) ON CONFLICT(job,key) DO UPDATE SET register=excluded.register",params![job,key,serde_json::to_string(reg)?])?;
            // Prebuilt job work is inert until the accepted pointer selects it.
            touch_field(&tx, job, key, get(&tx, key)?.as_ref(), reg)?;
        }
        tx.commit()?;
        std::thread::yield_now();
    }
    let tx = db::write_transaction(conn)?;
    database::require_generation(&tx, generation)?;
    tx.execute("UPDATE local_sync_receive SET next_page=?1 WHERE id=?2 AND phase='staging' AND next_page=?3",params![page+1,job,page])?;
    tx.commit()?;
    Ok(())
}
fn proposed_view(conn: &Connection, job: &str) -> AppResult<()> {
    if !valid_hex(job, 64) {
        return Err(invalid("候选身份无效"));
    }
    conn.execute_batch(&format!("DROP VIEW IF EXISTS temp.local_sync_proposed;
      CREATE TEMP VIEW local_sync_proposed AS
      SELECT f.key,f.register FROM local_sync_winners f WHERE NOT EXISTS(SELECT 1 FROM local_sync_candidates c WHERE c.job='{job}' AND c.key=f.key AND (json_extract(c.register,'$.stamp.lamport'),json_extract(c.register,'$.stamp.actor'),json_extract(c.register,'$.stamp.seq'))>(json_extract(f.register,'$.stamp.lamport'),json_extract(f.register,'$.stamp.actor'),json_extract(f.register,'$.stamp.seq')))
      UNION ALL SELECT c.key,c.register FROM local_sync_candidates c WHERE c.job='{job}' AND NOT EXISTS(SELECT 1 FROM local_sync_winners f WHERE f.key=c.key AND (json_extract(f.register,'$.stamp.lamport'),json_extract(f.register,'$.stamp.actor'),json_extract(f.register,'$.stamp.seq'))>=(json_extract(c.register,'$.stamp.lamport'),json_extract(c.register,'$.stamp.actor'),json_extract(c.register,'$.stamp.seq')));"))?;
    Ok(())
}
/// Known pending dots covered by a checkpoint carry information that sparse
/// snapshots are allowed to omit. Join that information before acceptance, as
/// Replica::ingest_checkpoint does. Each durable mutation is at most one intent.
fn compensate_pending(conn: &Connection, job: &str, manifest: &Manifest) -> AppResult<()> {
    let mut after = (String::new(), 0_u64);
    loop {
        let tx = db::write_transaction(conn)?;
        let (phase,active):(String,Option<String>)=tx.query_row("SELECT r.phase,c.accepted_job FROM local_sync_receive r CROSS JOIN local_sync_control c WHERE r.id=?1 AND c.id=1",[job],|r|Ok((r.get(0)?,r.get(1)?)))?;
        if phase != "staging" || active.is_some() {
            return Err(AppError::new("localSyncPending", "候选当前不可预验"));
        }
        let row:Option<(String,u64,String)>=tx.query_row("SELECT actor,seq,intent FROM local_sync_intents WHERE (actor,seq)>(?1,?2) ORDER BY actor,seq LIMIT 1",params![after.0,after.1],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        let Some((actor, seq, raw)) = row else {
            tx.commit()?;
            return Ok(());
        };
        after = (actor.clone(), seq);
        let mut covered = frontier(&tx)?;
        merge_frontier(&mut covered, &manifest.covered);
        if covered.get(&actor).copied().unwrap_or(0) >= seq {
            let intent: Intent = serde_json::from_str(&raw)?;
            if intent
                .observed
                .iter()
                .any(|(a, s)| covered.get(a).copied().unwrap_or(0) < *s)
            {
                return Err(invalid("候选覆盖待处理意图但缺观察依赖"));
            }
            validate_clock(&tx, PROPOSED, &intent)?;
            let stamp = Stamp {
                actor,
                seq,
                lamport: intent.lamport,
            };
            for (key, value) in intent.patch {
                let reg = Register {
                    stamp: stamp.clone(),
                    value,
                };
                let old = get_from(&tx, PROPOSED, &key)?;
                if let Some(old) = &old {
                    if old.stamp == reg.stamp && old.value != reg.value {
                        return Err(invalid("已知待处理字段与候选同戳冲突"));
                    }
                    if old.stamp >= reg.stamp {
                        continue;
                    }
                }
                touch_field(&tx, job, &key, get(&tx, &key)?.as_ref(), &reg)?;
                tx.execute("INSERT INTO local_sync_candidates(job,key,register) VALUES(?1,?2,?3) ON CONFLICT(job,key) DO UPDATE SET register=excluded.register",params![job,key,serde_json::to_string(&reg)?])?;
            }
        }
        tx.commit()?;
        std::thread::yield_now();
    }
}
fn prepare_work(conn: &Connection, job: &str) -> AppResult<i64> {
    let expected = revision(conn)?;
    let mut cursor = String::new();
    loop {
        let tx = db::write_transaction(conn)?;
        if revision(&tx)? != expected {
            return Err(AppError::new(
                "localSyncPending",
                "候选依赖边变化，重建物化工作集",
            ));
        }
        let rows = {
            let mut stmt=tx.prepare("SELECT key,register FROM local_sync_candidates WHERE job=?1 AND key>?2 ORDER BY key LIMIT 64")?;
            let rows = stmt
                .query_map(params![job, cursor], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        for (key, raw) in &rows {
            let reg: Register = serde_json::from_str(raw)?;
            touch_field(&tx, job, key, get(&tx, key)?.as_ref(), &reg)?;
            cursor = key.clone();
        }
        let done = rows.len() < ROWS;
        tx.commit()?;
        if done {
            return Ok(expected);
        }
        std::thread::yield_now();
    }
}

fn affected_validation_targets(
    conn: &Connection,
    relation: &str,
    job: &str,
) -> AppResult<BTreeSet<String>> {
    let mut targets = BTreeSet::new();
    let mut sources = BTreeSet::new();
    let mut urls = BTreeSet::new();
    let mut groups = BTreeSet::new();
    const INITIAL_SQL: &str = "SELECT key FROM local_sync_candidates WHERE job=?1
         UNION
         SELECT key FROM local_sync_work WHERE job=?1";
    let mut stmt = conn.prepare(INITIAL_SQL)?;
    for key in stmt.query_map([job], |r| r.get::<_, String>(0))? {
        let key = key?;
        if let Some(source) = key.strip_prefix("source/") {
            sources.insert(source.to_owned());
        } else if let Some(url) = key.strip_prefix("url/") {
            urls.insert(url.to_owned());
        } else if let Some(group) = key.strip_prefix("group/") {
            groups.insert(group.to_owned());
        } else {
            let target = if key == "p" || key.starts_with("p/") {
                Some("p".to_owned())
            } else if key.starts_with("a/") || key.starts_with("s/") || key.starts_with("g/") {
                Some(prefix(&key).to_owned())
            } else {
                None
            };
            if let Some(target) = target {
                if let Some(source) = target.strip_prefix("s/") {
                    sources.insert(source.to_owned());
                } else if let Some(group) = target.strip_prefix("g/") {
                    groups.insert(group.to_owned());
                }
                targets.insert(target);
            }
        }
    }
    drop(stmt);

    for url in urls.iter() {
        let sql = format!(
            "SELECT substr(key,3,32) FROM {relation}
             WHERE key LIKE 's/%/feed_url'
               AND json_extract(register,'$.value.value')=?1"
        );
        let mut stmt = conn.prepare(&sql)?;
        for source in stmt.query_map([url], |r| r.get::<_, String>(0))? {
            sources.insert(source?);
        }
    }
    for group in groups.iter() {
        let sql = format!(
            "SELECT substr(key,3,32) FROM {relation}
             WHERE key LIKE 's/%/folder'
               AND json_extract(register,'$.value.value')=?1"
        );
        let mut stmt = conn.prepare(&sql)?;
        for source in stmt.query_map([group], |r| r.get::<_, String>(0))? {
            sources.insert(source?);
        }
    }
    for source in sources {
        targets.insert(format!("s/{source}"));
        let sql = format!(
            "SELECT substr(key,1,66) FROM {relation}
             WHERE key LIKE 'a/%/meta'
               AND json_extract(register,'$.value.value.source')=?1"
        );
        let mut stmt = conn.prepare(&sql)?;
        for target in stmt.query_map([source], |r| r.get::<_, String>(0))? {
            targets.insert(target?);
        }
    }
    Ok(targets)
}

#[cfg(test)]
pub(super) fn validation_targets_for_test(
    conn: &Connection,
    job: &str,
) -> AppResult<BTreeSet<String>> {
    proposed_view(conn, job)?;
    let raw: String = conn.query_row(
        "SELECT manifest FROM local_sync_receive WHERE id=?1",
        [job],
        |r| r.get(0),
    )?;
    let manifest: Manifest = serde_json::from_str(&raw)?;
    compensate_pending(conn, job, &manifest)?;
    prepare_work(conn, job)?;
    affected_validation_targets(conn, PROPOSED, job)
}

/// Returns the exact logical/pending revision checked. Acceptance never repeats
/// the entire library validation while holding the writer.
pub fn preflight(conn: &Connection, job: &str) -> AppResult<i64> {
    // Test-only timing for the whole-library apply window. The 50k restore
    // half records this segment separately from staging and materialization.
    #[cfg(test)]
    let started = std::time::Instant::now();
    let reader = ValidationReader::new(conn, job)?;
    let generation = reader.generation;
    let expected_revision = revision(conn)?;
    let result = preflight_inner(conn, job);
    reader.close()?;
    drop(reader);
    if let Err(error) = &result {
        reject_candidate_checked(conn, generation, job, error, Some(expected_revision))?;
    }
    #[cfg(test)]
    println!(
        "ARCHIVE_PERF preflight_ms={}",
        started.elapsed().as_millis()
    );
    result
}
fn preflight_inner(conn: &Connection, job: &str) -> AppResult<i64> {
    proposed_view(conn, job)?;
    let raw: String = conn.query_row(
        "SELECT manifest FROM local_sync_receive WHERE id=?1",
        [job],
        |r| r.get(0),
    )?;
    let manifest: Manifest = serde_json::from_str(&raw)?;
    // Pin the journal before taking the validation snapshot. A missed or
    // truncated tail always falls back to this complete validation pass.
    let owner = format!("validation/{job}");
    {
        let tx = db::write_transaction(conn)?;
        tx.execute("INSERT INTO local_sync_roots(owner,kind,reference) VALUES(?1,'validation',?2) ON CONFLICT(owner) DO NOTHING",params![owner,job])?;
        tx.commit()?;
    }
    compensate_pending(conn, job, &manifest)?;
    let prepared_revision = prepare_work(conn, job)?;
    let tx = conn.unchecked_transaction()?;
    let (raw, next): (String, usize) = tx.query_row(
        "SELECT manifest,next_page FROM local_sync_receive WHERE id=?1",
        [job],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let manifest: Manifest = serde_json::from_str(&raw)?;
    if next != manifest.shards.len() {
        return Err(AppError::new("localSyncPending", "候选分片未完整校验"));
    }
    let (r, actor, seq, active): (i64, String, u64, Option<String>) = tx.query_row(
        "SELECT logical_revision,actor,seq,accepted_job FROM local_sync_control WHERE id=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    if r != prepared_revision {
        return Err(AppError::new(
            "localSyncPending",
            "预验工作集已过时，等待重新校验",
        ));
    }
    if active.is_some() {
        return Err(AppError::new("localSyncPending", "前一候选仍在本地合并"));
    }
    if manifest.covered.get(&actor).copied().unwrap_or(0) > seq {
        return Err(invalid("候选声称未分配的本机序号"));
    }
    let count: i64 = tx.query_row(&format!("SELECT COUNT(*) FROM {PROPOSED}"), [], |r| {
        r.get(0)
    })?;
    if count > MAX_FIELDS as i64 {
        return Err(invalid("逻辑字段超过格式容量"));
    }
    let mut stmt=tx.prepare("SELECT c.key,c.register,f.register FROM local_sync_candidates c LEFT JOIN local_sync_winners f ON f.key=c.key WHERE c.job=?1 ORDER BY c.key")?;
    let mut clock = manifest.max_lamport;
    for row in stmt.query_map([job], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
        ))
    })? {
        let (_, raw, old) = row?;
        let reg: Register = serde_json::from_str(&raw)?;
        clock = clock.max(reg.stamp.lamport);
        if let Some(old) = old {
            let old: Register = serde_json::from_str(&old)?;
            if old.stamp == reg.stamp && old.value != reg.value {
                return Err(invalid("预验同操作戳冲突"));
            }
        }
    }
    drop(stmt);
    // Candidate fields and durable old/new work define a finite affected
    // closure. Expanding source/URL/group scopes still validates every alias
    // and explicit dependency that can change, without sorting or projecting
    // unrelated entities across the whole library.
    let affected = affected_validation_targets(&tx, PROPOSED, job)?;
    for target in affected {
        projection_from(&tx, PROPOSED, &target)?;
    }
    let mut joined_frontier = frontier(&tx)?;
    merge_frontier(&mut joined_frontier, &manifest.covered);
    // A concurrent new pending input changes the revision. If it arrived before
    // this snapshot but after compensation, retry instead of dropping its patch.
    let mut stmt = tx.prepare("SELECT actor,intent FROM local_sync_intents ORDER BY actor,seq")?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (actor, raw) = row?;
        let intent: Intent = serde_json::from_str(&raw)?;
        if joined_frontier.get(&actor).copied().unwrap_or(0) >= intent.seq {
            if intent
                .observed
                .iter()
                .any(|(a, s)| joined_frontier.get(a).copied().unwrap_or(0) < *s)
            {
                return Err(invalid("候选覆盖待处理意图但缺观察依赖"));
            }
            for (key, value) in &intent.patch {
                let Some(winner) = get_from(&tx, PROPOSED, key)? else {
                    return Err(AppError::new(
                        "localSyncPending",
                        "候选需先补偿已知待处理字段",
                    ));
                };
                let stamp = Stamp {
                    actor: actor.clone(),
                    seq: intent.seq,
                    lamport: intent.lamport,
                };
                if winner.stamp == stamp && winner.value != *value {
                    return Err(invalid("候选不能丢弃已知待处理字段"));
                }
                if winner.stamp < stamp {
                    return Err(AppError::new(
                        "localSyncPending",
                        "新增待处理字段需要重新预验",
                    ));
                }
            }
            validate_clock(&tx, PROPOSED, &intent)?;
        }
    }
    drop(stmt);
    // A candidate can satisfy retained intents it does not cover. Validate the
    // newly ready sequence against the exact candidate/frontier accepted by CAS.
    preflight_ready_intents(&tx, PROPOSED, &joined_frontier)?;
    tx.commit()?;
    let tx = db::write_transaction(conn)?;
    if revision(&tx)? == r {
        tx.execute("UPDATE local_sync_receive SET validated_revision=?1,validated_clock=?2 WHERE id=?3 AND phase='staging'",params![r,clock,job])?;
    }
    tx.commit()?;
    Ok(r)
}
pub fn accept(conn: &Connection, generation: i64, job: &str, checked: i64) -> AppResult<bool> {
    let tx = db::write_transaction(conn)?;
    database::require_generation(&tx, generation)?;
    let (r, actor, library, active): (i64, String, String, Option<String>) = tx.query_row(
        "SELECT logical_revision,actor,library_id,accepted_job FROM local_sync_control WHERE id=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    if r != checked || active.is_some() {
        tx.commit()?;
        return Ok(false);
    }
    let (raw,owner,lib,epoch,next,validated,clock,phase):(String,String,String,i64,usize,Option<i64>,u64,String)=tx.query_row("SELECT manifest,local_actor,library_id,generation,next_page,validated_revision,validated_clock,phase FROM local_sync_receive WHERE id=?1",[job],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?)))?;
    let manifest: Manifest = serde_json::from_str(&raw)?;
    if owner != actor || lib != library || epoch != generation || next != manifest.shards.len() {
        return Err(invalid("候选接受身份或完整性失效"));
    }
    if phase != "staging" || validated != Some(checked) {
        tx.commit()?;
        return Ok(false);
    }
    let local_seq: u64 =
        tx.query_row("SELECT seq FROM local_sync_control WHERE id=1", [], |r| {
            r.get(0)
        })?;
    if manifest.covered.get(&actor).copied().unwrap_or(0) > local_seq {
        return Err(invalid("候选声称未分配的本机序号"));
    }
    let mut covered = frontier(&tx)?;
    merge_frontier(&mut covered, &manifest.covered);
    tx.execute("UPDATE local_sync_control SET accepted_job=?1,frontier=?2,lamport=MAX(lamport,?3),logical_revision=logical_revision+1 WHERE id=1",params![job,serde_json::to_string(&covered)?,clock])?;
    tx.execute(
        "UPDATE local_sync_receive SET phase='accepted',validated_revision=?1 WHERE id=?2",
        params![checked, job],
    )?;
    tx.execute(
        "DELETE FROM local_sync_roots WHERE owner=?1",
        [format!("validation/{job}")],
    )?;
    tx.commit()?;
    Ok(true)
}
fn validate_clock(conn: &Connection, relation: &str, intent: &Intent) -> AppResult<()> {
    for (actor, seq) in &intent.observed {
        let pending_clock:Option<u64>=conn.query_row("SELECT json_extract(intent,'$.lamport') FROM local_sync_intents WHERE actor=?1 AND seq=?2",params![actor,seq],|r|r.get(0)).optional()?;
        if pending_clock.is_some_and(|clock| clock >= intent.lamport) {
            return Err(invalid("意图逻辑时钟未晚于待处理依赖"));
        }
        let bad:bool=conn.query_row(&format!("SELECT EXISTS(SELECT 1 FROM {relation} WHERE json_extract(register,'$.stamp.actor')=?1 AND json_extract(register,'$.stamp.seq')<=?2 AND json_extract(register,'$.stamp.lamport')>=?3)"),params![actor,seq,intent.lamport],|r|r.get(0))?;
        if bad {
            return Err(invalid("意图逻辑时钟未晚于已观察winner"));
        }
    }
    Ok(())
}

pub fn ingest_batch(conn: &Connection, generation: i64, batch: &Batch) -> AppResult<()> {
    receive_batch(conn, generation, batch, None)
}
pub(super) fn receive_batch(
    conn: &Connection,
    generation: i64,
    batch: &Batch,
    digest: Option<&str>,
) -> AppResult<()> {
    batch.validate().map_err(transport_error)?;
    let tx = db::write_transaction(conn)?;
    database::require_generation(&tx, generation)?;
    let library: String = tx.query_row(
        "SELECT library_id FROM local_sync_control WHERE id=1",
        [],
        |r| r.get(0),
    )?;
    if batch.library_id != library {
        return Err(invalid("快批属于另一资料库"));
    }
    register_actor(&tx, &batch.actor)?;
    let (local_actor, local_seq): (String, u64) = tx.query_row(
        "SELECT actor,seq FROM local_sync_control WHERE id=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if batch.actor == local_actor && batch.intents.iter().any(|intent| intent.seq > local_seq) {
        return Err(invalid("快批声称未分配的本机序号"));
    }
    for intent in &batch.intents {
        if intent.observed.get(&local_actor).copied().unwrap_or(0) > local_seq {
            return Err(invalid("快批观察了未分配的本机序号"));
        }
        for actor in intent.observed.keys() {
            register_actor(&tx, actor)?;
        }
        let stamp = Stamp {
            actor: batch.actor.clone(),
            seq: intent.seq,
            lamport: intent.lamport,
        };
        let covered: BTreeMap<_, _> = intent
            .observed
            .iter()
            .map(|(a, s)| (a.clone(), *s))
            .chain(std::iter::once((batch.actor.clone(), intent.seq)))
            .collect();
        validate_checkpoint(&Checkpoint {
            format: 2,
            library_id: library.clone(),
            covered,
            max_lamport: intent.lamport,
            fields: intent
                .patch
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        Register {
                            stamp: stamp.clone(),
                            value: v.clone(),
                        },
                    )
                })
                .collect(),
        })?;
        let raw = serde_json::to_string(intent)?;
        let old: Option<String> = tx
            .query_row(
                "SELECT intent FROM local_sync_intents WHERE actor=?1 AND seq=?2",
                params![batch.actor, intent.seq],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(old) = old {
            if old != raw {
                return Err(invalid("同快批dot出现不同意图"));
            }
            continue;
        }
        // Covered history follows Replica's bounded-history contract; retained
        // equal-stamp winners are still checked before accepting this input.
        for (key, value) in &intent.patch {
            if let Some(old) = get(&tx, key)? {
                if old.stamp == stamp && old.value != *value {
                    return Err(invalid("同戳快批字段冲突"));
                }
            }
        }
        if frontier(&tx)?.get(&batch.actor).copied().unwrap_or(0) >= intent.seq {
            continue;
        }
        tx.execute(
            "INSERT INTO local_sync_intents(actor,seq,intent) VALUES(?1,?2,?3)",
            params![batch.actor, intent.seq, raw],
        )?;
        tx.execute(
            "UPDATE local_sync_control SET logical_revision=logical_revision+1 WHERE id=1",
            [],
        )?;
    }
    let pending_count: usize =
        tx.query_row("SELECT COUNT(*) FROM local_sync_intents", [], |r| r.get(0))?;
    if pending_count > crate::webdav_transport::MAX_ACTIVE_INTENTS {
        return Err(invalid("待依赖意图超过容量；需先补齐检查点"));
    }
    for intent in &batch.intents {
        validate_clock(&tx, WINNERS, intent)?;
    }
    preflight_ready_intents(&tx, WINNERS, &frontier(&tx)?)?;
    if let Some(digest) = digest {
        tx.execute(
            "INSERT OR IGNORE INTO local_sync_seen_batches(actor,digest) VALUES(?1,?2)",
            params![batch.actor, digest],
        )?;
    }
    tx.commit()?;
    Ok(())
}
fn next_ready_intent(conn: &Connection) -> AppResult<Option<(String, Intent)>> {
    let covered = frontier(conn)?;
    let mut stmt =
        conn.prepare("SELECT actor,intent FROM local_sync_intents ORDER BY actor,seq")?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (actor, raw) = row?;
        let intent: Intent = serde_json::from_str(&raw)?;
        if covered.get(&actor).copied().unwrap_or(0).checked_add(1) == Some(intent.seq)
            && intent
                .observed
                .iter()
                .all(|(a, s)| covered.get(a).copied().unwrap_or(0) >= *s)
        {
            return Ok(Some((actor, intent)));
        }
    }
    Ok(None)
}

/// Actual application joins original wire registers, validates dependencies and
/// only then advances the frontier. Display aliases use Projection separately.
fn join_ready_intent(
    conn: &Connection,
    actor: &str,
    intent: &Intent,
) -> AppResult<BTreeSet<String>> {
    validate_clock(conn, WINNERS, intent)?;
    let affected = affected_before(conn, &intent.patch)?;
    conn.execute(
        "UPDATE local_sync_control SET logical_revision=logical_revision+1 WHERE id=1",
        [],
    )?;
    let stamp = Stamp {
        actor: actor.to_owned(),
        seq: intent.seq,
        lamport: intent.lamport,
    };
    let mut targets = BTreeSet::new();
    for (key, value) in &intent.patch {
        put(
            conn,
            key,
            &Register {
                stamp: stamp.clone(),
                value: value.clone(),
            },
            "",
            true,
        )?;
        targets.insert(if key.starts_with("p/") {
            String::from("p")
        } else {
            prefix(key).to_owned()
        });
    }
    validate_affected(conn, &intent.patch, &affected)?;
    let mut covered = frontier(conn)?;
    covered.insert(actor.to_owned(), intent.seq);
    conn.execute(
        "UPDATE local_sync_control SET frontier=?1,lamport=MAX(lamport,?2) WHERE id=1",
        params![serde_json::to_string(&covered)?, intent.lamport],
    )?;
    Ok(targets)
}

/// Check a causal sequence against a connection-private TEMP overlay. Main is
/// read-only throughout: no accepted pointer/frontier/work/holds are trial-written
/// and a WAL snapshot never upgrades to a writer. Real wire conflicts and missing
/// dependencies still reject before accepting pending input. The temporary size
/// is bounded by protocol intent/patch limits, not a promised time slice.
pub(super) fn preflight_ready_intents(
    conn: &Connection,
    base: &str,
    initial: &Frontier,
) -> AppResult<()> {
    const TRIAL: &str = "temp.local_sync_trial";
    conn.execute_batch(&format!("CREATE TEMP TABLE IF NOT EXISTS local_sync_trial_fields(key TEXT PRIMARY KEY,register TEXT NOT NULL); DELETE FROM temp.local_sync_trial_fields;
        DROP VIEW IF EXISTS temp.local_sync_trial;
        CREATE TEMP VIEW local_sync_trial AS SELECT b.key,b.register FROM {base} b WHERE NOT EXISTS(SELECT 1 FROM temp.local_sync_trial_fields t WHERE t.key=b.key)
        UNION ALL SELECT key,register FROM temp.local_sync_trial_fields;"))?;
    let check = (|| -> AppResult<()> {
        let mut covered = initial.clone();
        let mut pending = {
            let mut stmt =
                conn.prepare("SELECT actor,intent FROM local_sync_intents ORDER BY actor,seq")?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows.into_iter()
                .map(|(actor, raw)| Ok((actor, serde_json::from_str::<Intent>(&raw)?)))
                .collect::<AppResult<Vec<_>>>()?
        };
        while let Some(index) = pending.iter().position(|(actor, intent)| {
            covered.get(actor).copied().unwrap_or(0).checked_add(1) == Some(intent.seq)
                && intent
                    .observed
                    .iter()
                    .all(|(a, s)| covered.get(a).copied().unwrap_or(0) >= *s)
        }) {
            let (actor, intent) = pending.remove(index);
            validate_clock(conn, TRIAL, &intent)?;
            let affected = affected_before_from(conn, TRIAL, &intent.patch)?;
            let stamp = Stamp {
                actor: actor.clone(),
                seq: intent.seq,
                lamport: intent.lamport,
            };
            for (key, value) in &intent.patch {
                let register = Register {
                    stamp: stamp.clone(),
                    value: value.clone(),
                };
                if let Some(old) = get_from(conn, TRIAL, key)? {
                    if old.stamp == register.stamp && old.value != register.value {
                        return Err(invalid("同原始字段与操作戳包含不同值"));
                    }
                    if old.stamp >= register.stamp {
                        continue;
                    }
                }
                conn.execute("INSERT INTO temp.local_sync_trial_fields(key,register) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET register=excluded.register",params![key,serde_json::to_string(&register)?])?;
            }
            validate_affected_from(conn, TRIAL, &intent.patch, &affected)?;
            covered.insert(actor, intent.seq);
        }
        Ok(())
    })();
    conn.execute_batch(
        "DROP VIEW temp.local_sync_trial; DELETE FROM temp.local_sync_trial_fields;",
    )?;
    check
}

pub fn apply_ready_intent(conn: &Connection) -> AppResult<bool> {
    let tx = db::write_transaction(conn)?;
    let covered = frontier(&tx)?;
    // Acceptance already joined these known patches. Their disk rows can be
    // retired in bounded batches without changing logical or dependency input.
    let removed=tx.execute("DELETE FROM local_sync_intents WHERE (actor,seq) IN (SELECT actor,seq FROM local_sync_intents WHERE seq<=COALESCE((SELECT value FROM json_each(?1) WHERE key=actor),0) ORDER BY actor,seq LIMIT 64)",[serde_json::to_string(&covered)?])?;
    if removed > 0 {
        tx.commit()?;
        return Ok(true);
    }
    let Some((actor, intent)) = next_ready_intent(&tx)? else {
        tx.commit()?;
        return Ok(false);
    };
    let targets = join_ready_intent(&tx, &actor, &intent)?;
    for target in targets {
        materialize(&tx, &target)?;
        tx.execute(
            "DELETE FROM local_sync_work WHERE job='' AND key=?1",
            [target],
        )?;
    }
    tx.execute(
        "DELETE FROM local_sync_intents WHERE actor=?1 AND seq=?2",
        params![actor, intent.seq],
    )?;
    tx.commit()?;
    Ok(true)
}

fn expand_scope(conn: &Connection, job: &str, key: &str, cursor: &str) -> AppResult<bool> {
    let (sql, arg) = if let Some(source) = key.strip_prefix("source/") {
        (format!("SELECT target FROM (SELECT substr(key,1,66) target FROM {WINNERS} WHERE key LIKE 'a/%/meta' AND json_extract(register,'$.value.value.source')=?1 UNION SELECT 'a/'||flx_entry(COALESCE(a.local_sync_source,f.sync_id),a.guid) FROM articles a JOIN feeds f ON f.id=a.feed_id WHERE f.sync_id=?1 OR a.local_sync_source=?1) WHERE target>?2 ORDER BY target LIMIT {ROWS}"),source)
    } else if let Some(url) = key.strip_prefix("url/") {
        (format!("SELECT target FROM (SELECT substr(key,1,34) target FROM {WINNERS} WHERE key LIKE 's/%/feed_url' AND json_extract(register,'$.value.value')=?1 UNION SELECT 's/'||sync_id FROM feeds WHERE feed_url=?1) WHERE target>?2 ORDER BY target LIMIT {ROWS}"),url)
    } else if let Some(group) = key.strip_prefix("group/") {
        (format!("SELECT target FROM (SELECT substr(key,1,34) target FROM {WINNERS} WHERE key LIKE 's/%/folder' AND json_extract(register,'$.value.value')=?1 UNION SELECT 's/'||f.sync_id FROM feeds f JOIN folders g ON g.id=f.folder_id WHERE g.sync_id=?1) WHERE target>?2 ORDER BY target LIMIT {ROWS}"),group)
    } else {
        return Err(invalid("未知物化scope"));
    };
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params![arg, cursor], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for target in &rows {
        touch(conn, job, target)?;
        if key.starts_with("url/") {
            touch(conn, job, &format!("source/{}", &target[2..]))?;
        }
    }
    if let Some(last) = rows.last() {
        conn.execute(
            "UPDATE local_sync_work SET cursor=?1 WHERE job=?2 AND key=?3",
            params![last, job, key],
        )?;
    }
    Ok(rows.len() < ROWS)
}
/// Targets processed by one [`resume_slice`] before it commits.
const WORK_BATCH: usize = 16;
/// One work-queue row. `revision` is the CAS token: a row touched after being
/// picked keeps a newer revision and survives the batch delete.
pub(super) struct WorkRow {
    pub(super) job: String,
    pub(super) key: String,
    cursor: String,
    revision: i64,
}
/// Work buckets in processing order. A bucket is a half-open primary-key range
/// `prefix/` .. `prefix0` ('0' sorts directly after '/'), so picking the next
/// target is one index seek and never sorts the queue — the previous
/// `ORDER BY CASE ...` sorted every pending row on every slice, which is what
/// made expanding a 50k-member source quadratic in slice count.
///
/// The bucket order reproduces the old CASE partitions: settings (0), `g/*`
/// (1), `s/*` (2), the ELSE keys (3) in primary-key order (`a/`, `group/`,
/// `source/`, `url/`), feed cleanup (4), then retention (5). The old CASE had
/// no cross-job ordering; the picker deliberately adds one new choice — every
/// `job=''` target is selected before any accepted-job target. That job
/// preference is not covered by the reachability/CAS tests and can delay an
/// accepted background job behind continuous local writes; it is a scheduling
/// choice, not a per-key correctness claim.
///
/// Every key the fan-out table on [`capture_sql`] can enqueue belongs to exactly
/// one range. Prefixes that merely look alike are separate buckets:
/// `g/<gid>` (folder entity, produced by the ELSE branch of the trigger) is not
/// `group/<gid>` (folder fan-out), and neither is a prefix of the other —
/// `g/` sorts before `group/` because '/' < 'r', so a merged range would drop
/// one of them. Same for `s/` vs `source/` and `a/`. A key with no bucket is
/// never picked: the row would sit in the queue forever and the entity would
/// never materialize.
pub(super) const WORK_BUCKETS: [(&str, &str); 8] = [
    ("g/", "g0"),
    ("s/", "s0"),
    ("a/", "a0"),
    ("group/", "group0"),
    ("source/", "source0"),
    ("url/", "url0"),
    ("cleanup-feed/", "cleanup-feed0"),
    ("retention/", "retention0"),
];
/// The one bucket key that is not a prefix range: settings are a single row.
pub(super) const WORK_EXACT_KEY: &str = "p";
/// Exact-key picker for the single settings row. Kept as a constant so the
/// plan regression runs the production text, not a copy of it.
pub(super) const PICK_EXACT_SQL: &str =
    "SELECT job,key,cursor,revision FROM local_sync_work WHERE job=?1 AND key=?2";
/// Range picker: a primary-key seek inside one bucket. Never sorted in SQL.
pub(super) const PICK_RANGE_SQL: &str = "SELECT job,key,cursor,revision FROM local_sync_work
     WHERE job=?1 AND key>=?2 AND key<?3 ORDER BY key LIMIT 1";
/// Batch retire with the original `(job,key,revision)` CAS.
///
/// Two traps worth spelling out: the columns inside `EXISTS(SELECT ... json_each)`
/// must be qualified with the table name, because `json_each` exposes its own
/// `key`/value columns and would silently shadow the work row's `key`; and the
/// revision is compared as an integer because `json_each` yields JSON numbers.
/// Either mistake turns the CAS into "never matches", which leaves every
/// processed row queued and the drain loop permanently busy.
pub(super) const DELETE_COMPLETED_SQL: &str = "DELETE FROM local_sync_work
     WHERE job=?1 AND key>=?2 AND key<=?3
       AND EXISTS(SELECT 1 FROM json_each(?4) j
                  WHERE j.value->>0=local_sync_work.job
                    AND j.value->>1=local_sync_work.key
                    AND CAST(j.value->>2 AS INTEGER)=local_sync_work.revision)";
pub(super) fn pick_work(conn: &Connection) -> AppResult<Option<WorkRow>> {
    let mut jobs: Vec<String> = vec![String::new()];
    if let Some(job) = conn
        .query_row(
            "SELECT accepted_job FROM local_sync_control WHERE id=1",
            [],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten()
    {
        jobs.push(job);
    }
    for job in jobs {
        let mut stmt = conn.prepare_cached(PICK_EXACT_SQL)?;
        if let Some(row) = stmt
            .query_row(params![job, WORK_EXACT_KEY], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .optional()?
        {
            return Ok(Some(WorkRow {
                job: row.0,
                key: row.1,
                cursor: row.2,
                revision: row.3,
            }));
        }
        let mut stmt = conn.prepare_cached(PICK_RANGE_SQL)?;
        for (start, end) in WORK_BUCKETS {
            if let Some(row) = stmt
                .query_row(params![job, start, end], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })
                .optional()?
            {
                return Ok(Some(WorkRow {
                    job: row.0,
                    key: row.1,
                    cursor: row.2,
                    revision: row.3,
                }));
            }
        }
    }
    Ok(None)
}
/// Retire the batch. The delete is still the exact `(job,key,revision)` CAS:
/// a row touched after it was picked has a newer revision and stays queued for
/// the next slice. The key bound keeps the statement on the primary index, and
/// only rows actually picked can match the batch list.
fn delete_completed(conn: &Connection, complete: &[WorkRow]) -> AppResult<()> {
    if complete.is_empty() {
        return Ok(());
    }
    let payload: Vec<Value> = complete
        .iter()
        .map(|w| json!([w.job, w.key, w.revision]))
        .collect();
    let mut bounds: BTreeMap<&str, (String, String)> = BTreeMap::new();
    for row in complete {
        let entry = bounds
            .entry(row.job.as_str())
            .or_insert_with(|| (row.key.clone(), row.key.clone()));
        if row.key < entry.0 {
            entry.0 = row.key.clone();
        }
        if row.key > entry.1 {
            entry.1 = row.key.clone();
        }
    }
    for (job, (start, end)) in bounds {
        conn.execute(
            DELETE_COMPLETED_SQL,
            params![job, start, end, serde_json::to_string(&payload)?],
        )?;
    }
    Ok(())
}
/// True while an accepted candidate is mid-fold. That is the one state a
/// portable export cannot snapshot consistently: while `local_sync_candidates`
/// rows are being folded into `local_sync_fields`, the winners view resolves
/// each key against a partially advanced candidate table, so the read would mix
/// pre- and post-fold values. The fold slice commits every 15ms, so this clears
/// on its own.
///
/// `folding` alone needs no separate check: it is set to 1 only inside that same
/// fold slice, which runs only after `accept` has set `accepted_job`, and it is
/// reset to 0 before the slice's transaction commits (logical::resume_slice). So
/// a non-zero `folding` always implies a non-null `accepted_job`.
///
/// Publication channels are deliberately *not* part of this. A
/// `local_sync_pending` row, a build root, body holds and body receipts are
/// peer-protocol state for uploading this device's own batch and for restore;
/// the export reads `local_sync_winners`/`articles`/`contents`/`feeds`/
/// `folders`/`settings` and touches none of them. A sealed-but-unacked batch is
/// the normal steady state of a synced library (it waits for a peer that may be
/// offline for days), so waiting on it would turn a user-initiated export into
/// the same unbounded wait this change removes.
pub fn accepted_candidate(conn: &Connection) -> AppResult<bool> {
    Ok(conn.query_row(
        "SELECT accepted_job IS NOT NULL FROM local_sync_control WHERE id=1",
        [],
        |r| r.get::<_, bool>(0),
    )?)
}
/// One local work slice, independent of credentials and network generation.
pub fn resume_slice(conn: &Connection) -> AppResult<bool> {
    let applied = apply_ready_intent(conn)?;
    if applied {
        return Ok(true);
    }
    let tx = db::write_transaction(conn)?;
    let active: Option<String> = tx.query_row(
        "SELECT accepted_job FROM local_sync_control WHERE id=1",
        [],
        |r| r.get(0),
    )?;
    if let Some(job) = &active {
        let (owner, library, phase, cursor): (String, String, String, String) = tx.query_row(
            "SELECT local_actor,library_id,phase,fold_cursor FROM local_sync_receive WHERE id=?1",
            [job],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        let current_library: String = tx.query_row(
            "SELECT library_id FROM local_sync_control WHERE id=1",
            [],
            |r| r.get(0),
        )?;
        if owner != database::actor(&tx)? || library != current_library {
            return Err(invalid("本地恢复身份不匹配"));
        }
        if phase == "accepted" {
            let rows = {
                let mut s=tx.prepare("SELECT key,register FROM local_sync_candidates WHERE job=?1 AND key>?2 ORDER BY key LIMIT 64")?;
                let rows = s
                    .query_map(params![job, cursor], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            tx.execute("UPDATE local_sync_control SET folding=1 WHERE id=1", [])?;
            let start = Instant::now();
            let mut last = cursor.clone();
            let mut timed_out = false;
            for (key, raw) in &rows {
                let reg: Register = serde_json::from_str(raw)?;
                let old = tx
                    .query_row(
                        "SELECT register FROM local_sync_fields WHERE key=?1",
                        [key],
                        |r| r.get::<_, String>(0),
                    )
                    .optional()?
                    .map(|s| serde_json::from_str::<Register>(&s))
                    .transpose()?;
                if old
                    .as_ref()
                    .is_some_and(|o| o.stamp == reg.stamp && o.value != reg.value)
                {
                    return Err(invalid("已接受候选的同戳不变量被破坏"));
                }
                if old.as_ref().map_or(true, |o| o.stamp < reg.stamp) {
                    tx.execute("INSERT INTO local_sync_fields(key,register) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET register=excluded.register",params![key,raw])?;
                }
                last = key.clone();
                if start.elapsed() >= Duration::from_millis(15) {
                    timed_out = true;
                    break;
                }
            }
            tx.execute("UPDATE local_sync_control SET folding=0 WHERE id=1", [])?;
            let fold_complete = rows.is_empty()
                || (!timed_out && rows.len() < ROWS && rows.last().map(|r| &r.0) == Some(&last));
            tx.execute("UPDATE local_sync_receive SET fold_cursor=?1,phase=CASE WHEN ?2 THEN 'projecting' ELSE phase END WHERE id=?3",params![last,fold_complete,job])?;
            tx.commit()?;
            return Ok(true);
        }
    }
    let started = Instant::now();
    let mut processed = 0usize;
    let mut complete: Vec<WorkRow> = Vec::new();
    while processed < WORK_BATCH && started.elapsed() < Duration::from_millis(15) {
        let Some(work) = pick_work(&tx)? else { break };
        processed += 1;
        let done = if work.key.starts_with("source/")
            || work.key.starts_with("url/")
            || work.key.starts_with("group/")
        {
            expand_scope(&tx, &work.job, &work.key, &work.cursor)?
        } else if work.key.starts_with("retention/") {
            #[cfg(test)]
            let retention_started = Instant::now();
            let done = retention_work(&tx, &work.job, &work.key, &work.cursor)?;
            #[cfg(test)]
            RETENTION_DRAIN_MICROS.fetch_add(
                retention_started.elapsed().as_micros() as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
            done
        } else if let Some(id) = work.key.strip_prefix("cleanup-feed/") {
            let id = id.parse::<i64>().map_err(|_| invalid("来源清理定位无效"))?;
            tx.execute("UPDATE local_sync_control SET applying=1 WHERE id=1", [])?;
            database::cleanup_feed(&tx, id)?;
            tx.execute("UPDATE local_sync_control SET applying=0 WHERE id=1", [])?;
            true
        } else {
            materialize(&tx, &work.key)?;
            true
        };
        if done {
            complete.push(work);
        }
    }
    if processed > 0 {
        delete_completed(&tx, &complete)?;
        tx.commit()?;
        return Ok(true);
    }
    if let Some(job) = active {
        let raw: String = tx.query_row(
            "SELECT manifest FROM local_sync_receive WHERE id=?1",
            [&job],
            |r| r.get(0),
        )?;
        let manifest: Manifest = serde_json::from_str(&raw)?;
        db::set_setting(
            &tx,
            &format!("local_sync_peer_state_{}", manifest.actor),
            &raw,
        )?;
        tx.execute(
            "DELETE FROM local_sync_downloads WHERE actor=?1 AND kind='state' AND manifest=?2",
            params![manifest.actor, raw],
        )?;
        tx.execute(
            "UPDATE local_sync_receive SET phase='complete' WHERE id=?1",
            [&job],
        )?;
        tx.execute(
            "UPDATE local_sync_control SET accepted_job=NULL WHERE id=1 AND accepted_job=?1",
            [&job],
        )?;
    }
    let pending: bool =
        tx.query_row("SELECT EXISTS(SELECT 1 FROM local_sync_intents)", [], |r| {
            r.get(0)
        })?;
    if !pending {
        tx.execute(
            "UPDATE local_sync_control SET completed_frontier=frontier WHERE id=1",
            [],
        )?;
    }
    tx.commit()?;
    Ok(false)
}
pub fn settled(conn: &Connection) -> AppResult<bool> {
    Ok(conn.query_row("SELECT (capture=0 OR initialized=1) AND accepted_job IS NULL AND NOT EXISTS(SELECT 1 FROM local_sync_work WHERE job='') FROM local_sync_control WHERE id=1",[],|r|r.get(0))?)
}
pub fn drain_local(conn: &Connection) -> AppResult<()> {
    while database::resume_local_slice(conn)? {
        std::thread::yield_now();
    }
    Ok(())
}
