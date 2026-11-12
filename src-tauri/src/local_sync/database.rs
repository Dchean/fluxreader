use super::model::*;
use crate::{
    db,
    error::{AppError, AppResult},
    webdav_transport::{Checkpoint, FieldPatch, FieldValue, Frontier, Register},
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const BOOTSTRAP_SLICE_ROWS: usize = 16;

pub fn register_functions(conn: &Connection) -> AppResult<()> {
    use rusqlite::functions::FunctionFlags;
    conn.create_scalar_function(
        "flx_bucket",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let key: String = ctx.get(0)?;
            Ok(i64::from(
                u8::from_str_radix(&digest(key.as_bytes())[..2], 16).unwrap_or(0) % 64,
            ))
        },
    )?;
    conn.create_scalar_function(
        "flx_entry",
        2,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let source: String = ctx.get(0)?;
            let guid: String = ctx.get(1)?;
            Ok(entry_key(&source, &guid))
        },
    )?;
    conn.create_scalar_function("flx_affected_urls", 1, FunctionFlags::SQLITE_UTF8, |ctx| {
        let raw: String = ctx.get(0)?;
        let result = (|| -> AppResult<String> {
            let patch: FieldPatch = serde_json::from_str(&raw)?;
            // SAFETY: rusqlite 0.32.1 binds this non-owning handle to ctx. This
            // synchronous callback only prepares/reads on the caller's thread;
            // neither the connection nor any borrowed row escapes the callback.
            let conn = unsafe { ctx.get_connection()? };
            Ok(serde_json::to_string(&super::logical::affected_before(
                &conn, &patch,
            )?)?)
        })();
        result.map_err(|error| rusqlite::Error::UserFunctionError(Box::new(error)))
    })?;
    conn.create_scalar_function(
        "flx_meta_alias_work",
        1,
        FunctionFlags::SQLITE_UTF8,
        |ctx| {
            let raw: String = ctx.get(0)?;
            let result = (|| -> AppResult<String> {
                let patch: FieldPatch = serde_json::from_str(&raw)?;
                // SAFETY: same ctx-bound, synchronous read-only use as above.
                let conn = unsafe { ctx.get_connection()? };
                Ok(serde_json::to_string(&super::logical::meta_alias_work(
                    &conn, &patch,
                )?)?)
            })();
            result.map_err(|error| rusqlite::Error::UserFunctionError(Box::new(error)))
        },
    )?;
    conn.create_scalar_function(
        "flx_validate_capture",
        1,
        FunctionFlags::SQLITE_UTF8,
        |ctx| {
            let raw: String = ctx.get(0)?;
            let result = (|| -> AppResult<i64> {
                let patch: FieldPatch = serde_json::from_str(&raw)?;
                // SAFETY: same ctx-bound, synchronous read-only use as above. SQL
                // callback errors propagate, aborting every effect of the statement.
                let conn = unsafe { ctx.get_connection()? };
                super::logical::validate_captured_patch(&conn, &patch)?;
                Ok(1)
            })();
            result.map_err(|error| rusqlite::Error::UserFunctionError(Box::new(error)))
        },
    )?;
    Ok(())
}

pub fn actor(conn: &Connection) -> AppResult<String> {
    Ok(
        conn.query_row("SELECT actor FROM local_sync_control WHERE id=1", [], |r| {
            r.get(0)
        })?,
    )
}
pub fn generation(conn: &Connection) -> AppResult<i64> {
    Ok(conn.query_row(
        "SELECT generation FROM local_sync_control WHERE id=1",
        [],
        |r| r.get(0),
    )?)
}
pub fn require_generation(conn: &Connection, expected: i64) -> AppResult<()> {
    if generation(conn)? != expected {
        return Err(AppError::new(
            "staleLocalSync",
            "本次同步连接已失效；迟到结果未应用",
        ));
    }
    Ok(())
}

pub fn checkpoint(conn: &Connection) -> AppResult<Checkpoint> {
    let (library, frontier, clock): (String, String, u64) = conn.query_row(
        "SELECT library_id,frontier,lamport FROM local_sync_control WHERE id=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let mut fields = BTreeMap::new();
    let mut stmt = conn.prepare("SELECT key,register FROM local_sync_winners ORDER BY key")?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (key, register) = row?;
        fields.insert(key, serde_json::from_str(&register)?);
    }
    let state = Checkpoint {
        format: 2,
        library_id: library,
        covered: serde_json::from_str(&frontier)?,
        max_lamport: clock,
        fields,
    };
    validate_checkpoint(&state)?;
    Ok(state)
}

/// Called inside the same transaction which seeds/changes application data.
fn fill_only_patches(
    conn: &Connection,
    patches: impl IntoIterator<Item = FieldPatch>,
) -> AppResult<Vec<FieldPatch>> {
    // Bootstrap fills only absent winners. Triggers are already live, so a user
    // edit committed between seed chunks is never replaced by seed data. One
    // indexed lookup covers a whole snapshot slice instead of one query per
    // field or article.
    let patches: Vec<_> = patches.into_iter().collect();
    let keys: BTreeSet<_> = patches
        .iter()
        .flat_map(|patch| patch.keys().cloned())
        .collect();
    if keys.is_empty() {
        return Ok(patches);
    }
    let placeholders = std::iter::repeat("?")
        .take(keys.len())
        .collect::<Vec<_>>()
        .join(",");
    let mut stmt = conn.prepare(&format!(
        "SELECT key FROM local_sync_winners WHERE key IN ({placeholders})"
    ))?;
    let existing = stmt
        .query_map(rusqlite::params_from_iter(keys.iter()), |row| {
            row.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<BTreeSet<_>>>()?;
    Ok(patches
        .into_iter()
        .map(|patch| {
            patch
                .into_iter()
                .filter(|(key, _)| !existing.contains(key))
                .collect()
        })
        .collect())
}
fn fill_only_patch(conn: &Connection, patch: &FieldPatch) -> AppResult<FieldPatch> {
    Ok(fill_only_patches(conn, [patch.clone()])?
        .into_iter()
        .next()
        .unwrap_or_default())
}
fn record(conn: &Connection, patch: FieldPatch) -> AppResult<()> {
    let patch = fill_only_patch(conn, &patch)?;
    if patch.is_empty() {
        return Ok(());
    }
    super::logical::capture_bootstrap_patch(conn, &patch)
}
fn capture_article_snapshots(conn: &Connection, snapshots: &mut Vec<FieldPatch>) -> AppResult<()> {
    if snapshots.is_empty() {
        return Ok(());
    }
    let snapshots = fill_only_patches(conn, std::mem::take(snapshots))?;
    super::logical::capture_bootstrap_snapshot_patches(conn, &snapshots)?;
    Ok(())
}
fn fields(prefix: &str, data: Value) -> AppResult<FieldPatch> {
    let object = data
        .as_object()
        .ok_or_else(|| invalid("同步对象必须为映射"))?;
    Ok(object
        .iter()
        .map(|(key, value)| (format!("{prefix}/{key}"), FieldValue::Set(value.clone())))
        .collect())
}

pub fn initialize(conn: &Connection, library: &str) -> AppResult<()> {
    begin_initialize(conn, library)?;
    while !conn.query_row(
        "SELECT initialized FROM local_sync_control WHERE id=1",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        resume_local_slice(conn)?;
        std::thread::yield_now();
    }
    Ok(())
}

/// Persist only the start boundary. Startup resumes this exact library/actor a
/// slice at a time, without credentials or another network connection attempt.
pub(super) fn begin_initialize(conn: &Connection, library: &str) -> AppResult<()> {
    if !valid_hex(library, 32) {
        return Err(invalid("库标识无效"));
    }
    register_functions(conn)?;
    let tx = crate::db::write_transaction(conn)?;
    // The v1 format is intentionally non-upgrading, but these indexes were
    // added after that format boundary. Create them once for an existing
    // current-format library before deciding initialization is done.
    tx.execute_batch(
        "CREATE INDEX IF NOT EXISTS local_sync_fields_body_rss
           ON local_sync_fields(json_extract(register,'$.value.value.rss'))
           WHERE key LIKE 'a/%/body';
         CREATE INDEX IF NOT EXISTS local_sync_candidates_body_rss
           ON local_sync_candidates(job,json_extract(register,'$.value.value.rss'),key)
           WHERE key LIKE 'a/%/body';
         CREATE INDEX IF NOT EXISTS local_sync_receipt_hold_id
           ON local_sync_body_receipt_holds(hold_id);",
    )?;
    let (capture, initialized, current): (bool, bool, String) = tx.query_row(
        "SELECT capture,initialized,library_id FROM local_sync_control WHERE id=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    if capture && initialized && current == library {
        tx.commit()?;
        return Ok(());
    }
    if capture
        && !current.is_empty()
        && current != library
        && db::get_setting(&tx, "local_sync_detached")?.as_deref() != Some("1")
    {
        return Err(AppError::new(
            "differentLocalLibrary",
            "此设备已有另一云端库的待同步状态；请使用备份恢复到新库，不能把两个库的操作身份混用",
        ));
    }
    tx.execute(
        "UPDATE local_sync_control SET library_id=?1,capture=1,applying=0 WHERE id=1",
        [library],
    )?;
    tx.execute("DELETE FROM settings WHERE key='local_sync_detached'", [])?;
    tx.execute_batch(&super::capture::sql())?;
    super::logical::register_actor(&tx, &actor(&tx)?)?;
    tx.commit()?;
    Ok(())
}

pub fn resume_local_slice(conn: &Connection) -> AppResult<bool> {
    let (started, initialized, library, actor, generation):(bool,bool,String,String,i64)=conn.query_row(
        "SELECT capture,initialized,library_id,actor,generation FROM local_sync_control WHERE id=1",[],
        |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?;
    if started && !initialized {
        if !valid_hex(&library, 32) {
            return Err(invalid("已开始的初始化缺少资料库身份"));
        }
        bootstrap_slice(conn, generation, &library, &actor)?;
        // Finishing the bootstrap phase is itself progress; a caller draining
        // local work must still get the next opportunity to project its fields.
        return Ok(true);
    }
    super::logical::resume_slice(conn)
}

fn bootstrap_slice(
    conn: &Connection,
    expected: i64,
    library: &str,
    owner: &str,
) -> AppResult<bool> {
    let tx = crate::db::write_transaction(conn)?;
    require_generation(&tx, expected)?;
    let matches: bool = tx.query_row(
        "SELECT capture=1 AND library_id=?1 AND actor=?2 FROM local_sync_control WHERE id=1",
        params![library, owner],
        |r| r.get(0),
    )?;
    if !matches {
        return Err(AppError::new(
            "staleLocalSync",
            "初始化身份已变化，旧工作片未写入",
        ));
    }
    let (phase, mut cursor, initialized): (String, i64, bool) = tx.query_row(
        "SELECT bootstrap_phase,bootstrap_cursor,initialized FROM local_sync_control WHERE id=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    if initialized {
        tx.commit()?;
        return Ok(false);
    }
    let started = std::time::Instant::now();
    let mut count = 0;
    let mut article_snapshots = Vec::new();
    while count < BOOTSTRAP_SLICE_ROWS && started.elapsed() < std::time::Duration::from_millis(15) {
        let row:Option<(i64,String,Value,bool)>=match phase.as_str() {
            "groups"=>tx.query_row("SELECT id,sync_id,name,layout,summary_mode,translate_mode,position,collapsed FROM folders WHERE origin='local' AND id>?1 ORDER BY id LIMIT 1",[cursor],|r|Ok((r.get(0)?,r.get(1)?,json!({"created":true,"name":r.get::<_,String>(2)?,"layout":r.get::<_,String>(3)?,"summary_mode":r.get::<_,String>(4)?,"translate_mode":r.get::<_,String>(5)?,"position":r.get::<_,i64>(6)?,"collapsed":r.get::<_,bool>(7)?}),true))).optional()?,
            "sources"=>tx.query_row(&format!("SELECT id,sync_id,feed_url,title,site_url,favicon_url,layout,summary_mode,translate_mode,(SELECT sync_id FROM folders WHERE id=feeds.folder_id),active,title_custom,fetched_title FROM feeds WHERE {} AND id>?1 ORDER BY id LIMIT 1",local_source("feeds")),[cursor],|r|Ok((r.get(0)?,r.get(1)?,json!({"feed_url":r.get::<_,String>(2)?,"title":r.get::<_,String>(3)?,"site_url":r.get::<_,Option<String>>(4)?,"favicon_url":r.get::<_,Option<String>>(5)?,"layout":r.get::<_,String>(6)?,"summary_mode":r.get::<_,String>(7)?,"translate_mode":r.get::<_,String>(8)?,"folder":r.get::<_,Option<String>>(9)?,"created":true,"title_custom":r.get::<_,Option<String>>(11)?,"fetched_title":r.get::<_,String>(12)?}),r.get(10)?))).optional()?,
            "articles"=>tx.query_row(&format!("SELECT a.id,COALESCE(a.local_sync_source,f.sync_id),a.guid FROM articles a JOIN feeds f ON f.id=a.feed_id WHERE {} AND a.id>?1 ORDER BY a.id LIMIT 1",local_source("f")),[cursor],|r|Ok((r.get(0)?,r.get(1)?,json!(r.get::<_,String>(2)?),true))).optional()?,
            "settings"=>None,
            _=>return Err(invalid("初始化游标阶段损坏")),
        };
        let Some((id, source, data, active)) = row else {
            capture_article_snapshots(&tx, &mut article_snapshots)?;
            let next = match phase.as_str() {
                "groups" => "sources",
                "sources" => "articles",
                "articles" => "settings",
                _ => "done",
            };
            if phase == "settings" {
                if let Some(raw) = db::get_setting(&tx, "app_settings")? {
                    let settings: Value = serde_json::from_str(&raw)?;
                    let patch = SETTINGS
                        .iter()
                        .filter_map(|key| {
                            settings
                                .get(*key)
                                .map(|value| (format!("p/{key}"), FieldValue::Set(value.clone())))
                        })
                        .collect();
                    record(&tx, patch)?;
                }
                tx.execute("UPDATE local_sync_control SET initialized=1 WHERE id=1", [])?;
            }
            tx.execute(
                "UPDATE local_sync_control SET bootstrap_phase=?1,bootstrap_cursor=0 WHERE id=1",
                [next],
            )?;
            tx.commit()?;
            return Ok(next != "done");
        };
        cursor = id;
        count += 1;
        if phase == "articles" {
            let guid = data.as_str().ok_or_else(|| invalid("初始化文章身份无效"))?;
            let updated = tx.execute("UPDATE articles SET local_sync_source=?1 WHERE id=?2 AND local_sync_source IS NULL",params![source,id])?;
            if updated != 1 {
                return Err(invalid("初始化文章定位未被单行更新"));
            }
            let meta = article_meta(&tx, id, &source)?;
            let (read, star): (bool, bool) = tx.query_row(
                "SELECT is_read,is_starred FROM articles WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let mut patch = fields(
                &format!("a/{}", entry_key(&source, guid)),
                json!({"meta":meta,"read":read,"star":star}),
            )?;
            if let Some(body) = body_reference(&tx, id)? {
                patch.insert(
                    format!("a/{}/body", entry_key(&source, guid)),
                    FieldValue::Set(serde_json::to_value(&body)?),
                );
                if !read || star {
                    super::logical::hold_body(&tx, id, &entry_key(&source, guid), &body)?;
                }
            }
            article_snapshots.push(patch);
        } else {
            let p = format!("{}/{source}", if phase == "groups" { "g" } else { "s" });
            record(&tx, fields(&p, data)?)?;
            if !active {
                record(&tx, fields(&p, json!({"deleted":true}))?)?;
            }
        }
    }
    capture_article_snapshots(&tx, &mut article_snapshots)?;
    tx.execute(
        "UPDATE local_sync_control SET bootstrap_cursor=?1 WHERE id=1",
        [cursor],
    )?;
    tx.commit()?;
    Ok(true)
}

pub(super) fn article_meta(conn: &Connection, id: i64, source: &str) -> AppResult<ArticleMeta> {
    let meta = conn.query_row(
        "SELECT guid,url,title,author,published_at,published_at_source,first_seen_at,image_url,image_source,enclosure_url,enclosure_mime,duration_sec FROM articles WHERE id=?1",
        [id],
        |r| Ok(ArticleMeta {
            source: source.to_owned(),
            guid: r.get(0)?, url: r.get(1)?, title: r.get(2)?, author: r.get(3)?,
            published_at: r.get(4)?, published_at_source: r.get(5)?, first_seen_at: r.get(6)?,
            image_url: r.get(7)?, image_source: r.get(8)?, enclosure_url: r.get(9)?,
            enclosure_mime: r.get(10)?, duration_sec: r.get(11)?,
        }),
    )?;
    Ok(meta)
}
pub(super) fn content(conn: &Connection, id: Option<i64>) -> AppResult<Option<BodyContent>> {
    let Some(id) = id else { return Ok(None) };
    let body=conn.query_row("SELECT identity,content_html,body_text,extraction_source,fulltext_extracted FROM contents WHERE id=?1",[id],|r|Ok(BodyContent{identity:r.get(0)?,html:r.get(1)?,text:r.get(2)?,source:r.get(3)?,extracted:r.get(4)?})).optional()?;
    Ok(body.filter(|b| db::content_non_empty(b.html.as_deref(), &b.text)))
}
fn body_reference(conn: &Connection, id: i64) -> AppResult<Option<BodyRef>> {
    Ok(conn.query_row("SELECT c.identity,r.identity FROM articles a JOIN contents c ON c.id=a.content_id LEFT JOIN contents r ON r.id=a.rss_content_id AND (COALESCE(r.content_html,'')!='' OR r.body_text!='') WHERE a.id=?1 AND (COALESCE(c.content_html,'')!='' OR c.body_text!='')",[id],|r|Ok(BodyRef{primary:r.get(0)?,rss:r.get(1)?})).optional()?)
}
pub fn read_body(conn: &Connection, id: i64) -> AppResult<Option<BodyRecord>> {
    let (source,guid,primary,rss):(String,String,Option<i64>,Option<i64>)=conn.query_row("SELECT COALESCE(a.local_sync_source,f.sync_id),a.guid,a.content_id,a.rss_content_id FROM articles a JOIN feeds f ON f.id=a.feed_id WHERE a.id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
    let Some(primary) = content(conn, primary)? else {
        return Ok(None);
    };
    let rss = content(conn, rss)?;
    let body = BodyRecord {
        entry: entry_key(&source, &guid),
        body_ref: BodyRef {
            primary: primary.identity.clone(),
            rss: rss.as_ref().map(|b| b.identity.clone()),
        },
        primary,
        rss,
    };
    body.validate()?;
    Ok(Some(body))
}
pub fn protected_bodies(conn: &Connection) -> AppResult<Vec<BodyRecord>> {
    let mut stmt=conn.prepare(&format!("SELECT a.id FROM articles a JOIN feeds f ON f.id=a.feed_id JOIN local_sync_article_state l ON l.id=a.id WHERE {} AND ((l.is_read=0 AND l.active=1) OR l.is_starred=1 OR EXISTS(SELECT 1 FROM local_sync_body_hold h WHERE h.article_id=a.id)) ORDER BY a.id",local_source("f")))?;
    let ids = stmt
        .query_map([], |r| r.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ids.into_iter()
        .filter_map(|id| match read_body(conn, id) {
            Ok(Some(body)) => Some(Ok(body)),
            Ok(None) => None,
            Err(error) => Some(Err(error)),
        })
        .collect()
}

/// Alias derivation is the sole URL identity bridge. It never normalizes URLs or
/// mutates wire keys/stamps. Existing local feed.sync_id remains its original ID.
pub fn projection(state: &Checkpoint) -> AppResult<BTreeMap<String, Register>> {
    let mut urls: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (key, reg) in &state.fields {
        if key.starts_with("s/") && key.ends_with("/feed_url") {
            if let FieldValue::Set(Value::String(url)) = &reg.value {
                urls.entry(url.clone())
                    .or_default()
                    .push(key.split('/').nth(1).unwrap_or_default().to_owned());
            }
        }
    }
    let mut aliases = BTreeMap::new();
    for ids in urls.values_mut() {
        ids.sort();
        for id in ids.iter() {
            aliases.insert(id.clone(), ids[0].clone());
        }
    }
    let mut entries = BTreeMap::new();
    for (key, reg) in &state.fields {
        if key.starts_with("a/") && key.ends_with("/meta") {
            if let FieldValue::Set(v) = &reg.value {
                let meta: ArticleMeta = serde_json::from_value(v.clone())?;
                let source = aliases.get(&meta.source).unwrap_or(&meta.source);
                entries.insert(
                    key.split('/').nth(1).unwrap_or_default().to_owned(),
                    (entry_key(source, &meta.guid), source.clone()),
                );
            }
        }
    }
    let mut projected = Projection::default();
    for (key, reg) in &state.fields {
        let parts: Vec<&str> = key.split('/').collect();
        let mut reg = reg.clone();
        let target = if parts.len() == 3 && parts[0] == "s" {
            format!(
                "s/{}/{}",
                aliases
                    .get(parts[1])
                    .map(String::as_str)
                    .unwrap_or(parts[1]),
                parts[2]
            )
        } else if parts.len() == 3 && parts[0] == "a" {
            let Some((entry, source)) = entries.get(parts[1]) else {
                return Err(invalid("文章字段缺少元数据身份"));
            };
            if parts[2] == "meta" {
                if let FieldValue::Set(v) = &mut reg.value {
                    v["source"] = json!(source);
                }
            }
            format!("a/{entry}/{}", parts[2])
        } else {
            key.clone()
        };
        projected.select(target, key, reg)?;
    }
    let fields = projected.into_fields();
    let groups: BTreeSet<_> = fields
        .keys()
        .filter(|key| key.starts_with("g/"))
        .filter_map(|key| key.rsplit_once('/').map(|(prefix, _)| prefix))
        .collect();
    for prefix in groups {
        validate_group_entity(&fields, prefix)?;
    }
    Ok(fields)
}

/// Synchronous checkpoint boundary for callers that need all projection complete.
/// Production HTTP stages cached pages and yields between local work slices.
pub fn merge_apply(conn: &Connection, expected: i64, incoming: &Checkpoint) -> AppResult<bool> {
    validate_checkpoint(incoming)?;
    let before = super::logical::revision(conn)?;
    let mut pages = Vec::new();
    let entries: Vec<_> = incoming.fields.iter().collect();
    for chunk in entries.chunks(64) {
        let fields: BTreeMap<String, Register> = chunk
            .iter()
            .map(|(k, v)| ((*k).clone(), (*v).clone()))
            .collect();
        pages.push(serde_json::to_vec(&fields)?);
    }
    let actor = incoming
        .covered
        .keys()
        .next()
        .cloned()
        .unwrap_or(actor(conn)?);
    let manifest = super::storage::Manifest {
        format: FORMAT,
        library_id: incoming.library_id.clone(),
        actor,
        publication: 1,
        kind: "state".into(),
        covered: incoming.covered.clone(),
        completed: Frontier::new(),
        max_lamport: incoming.max_lamport,
        deltas: 0,
        shards: pages
            .iter()
            .map(|bytes| {
                let hash = digest(bytes);
                super::storage::ObjectRef {
                    name: format!("{hash}-{}.bin", "0".repeat(32)),
                    digest: hash,
                    size: bytes.len(),
                }
            })
            .collect(),
    };
    let job = super::logical::begin_receive(conn, expected, &manifest)?;
    let phase: String = conn.query_row(
        "SELECT phase FROM local_sync_receive WHERE id=?1",
        [&job],
        |r| r.get(0),
    )?;
    if phase == "complete" {
        return Ok(false);
    }
    if phase == "accepted" || phase == "projecting" {
        super::logical::drain_local(conn)?;
        return Ok(true);
    }
    for (page, bytes) in pages.iter().enumerate() {
        super::logical::stage_page(conn, expected, &job, page, &serde_json::from_slice(bytes)?)?;
    }
    let revision = super::logical::preflight(conn, &job)?;
    if !super::logical::accept(conn, expected, &job, revision)? {
        return Err(AppError::new(
            "localSyncPending",
            "候选预验输入已变化，等待重新校验",
        ));
    }
    super::logical::drain_local(conn)?;
    Ok(super::logical::revision(conn)? != before)
}
pub fn merge_small(
    conn: &Connection,
    expected: i64,
    incoming: &Checkpoint,
) -> AppResult<Option<bool>> {
    merge_apply(conn, expected, incoming).map(Some)
}
pub fn store_checkpoint(conn: &Connection, state: &Checkpoint) -> AppResult<()> {
    merge_apply(conn, generation(conn)?, state).map(|_| ())
}
fn string<'a>(
    fields: &'a BTreeMap<String, Register>,
    prefix: &str,
    key: &str,
    default: &'a str,
) -> &'a str {
    value(fields, prefix, key)
        .and_then(Value::as_str)
        .unwrap_or(default)
}
fn optional(fields: &BTreeMap<String, Register>, prefix: &str, key: &str) -> Option<String> {
    value(fields, prefix, key)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

// Keep the GUID parameter on the same physical row and in the logical validity
// predicate. Expanding the locator view via a second articles alias prevents
// SQLite from using the full physical source/GUID key before validity checks.
// The winner relation still includes accepted C; feeds only proves row existence.
pub(super) const ARTICLE_RECONCILE_SQL: &str = "SELECT a.id,a.feed_id,a.local_sync_source FROM articles a JOIN feeds f ON f.id=a.feed_id JOIN local_sync_source_urls s ON s.source=a.local_sync_source WHERE s.url=?1 AND a.guid=?2 AND EXISTS(SELECT 1 FROM local_sync_winners m WHERE m.key LIKE 'a/%/meta' AND json_extract(m.register,'$.value.value.source')=a.local_sync_source AND json_extract(m.register,'$.value.value.guid')=?2) ORDER BY CASE WHEN a.feed_id=?3 THEN 0 ELSE 1 END,a.id";

/// Fold physical duplicates only after their immutable payload obligations have
/// been detached from row lifetime. Original wire entries remain exclusively in L.
fn reconcile_article_rows(
    conn: &Connection,
    feed: i64,
    url: &str,
    guid: &str,
    source: &str,
) -> AppResult<()> {
    let rows = {
        let mut stmt = conn.prepare(ARTICLE_RECONCILE_SQL)?;
        let rows = stmt
            .query_map(params![url, guid, feed], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let Some((keep, _, _)) = rows.first() else {
        return Ok(());
    };
    let keep = *keep;
    for (id, _, owner) in &rows {
        if let Some(body) = body_reference(conn, *id)? {
            super::logical::hold_body(conn, *id, &entry_key(owner, guid), &body)?;
        }
    }
    for (id, old_feed, _) in &rows {
        if *id == keep {
            continue;
        }
        // RSS state queues retain their individual intent records; their row
        // locator follows the retained display row. No remote IDs are invented.
        conn.execute(
            "UPDATE sync_queue SET article_id=?1 WHERE article_id=?2",
            params![keep, id],
        )?;
        conn.execute(
            "UPDATE local_sync_body_hold SET article_id=?1 WHERE article_id=?2",
            params![keep, id],
        )?;
        conn.execute(
            "UPDATE deduped_urls SET kept_aid=?1 WHERE kept_aid=?2",
            params![keep, id],
        )?;
        conn.execute("DELETE FROM articles WHERE id=?1", [id])?;
        super::logical::touch(conn, "", &format!("cleanup-feed/{old_feed}"))?;
    }
    conn.execute(
        "UPDATE articles SET feed_id=?1 WHERE id=?2 AND feed_id!=?1",
        params![feed, keep],
    )?;
    // Keep the retained row's original valid wire owner. Only a new row adopts
    // the current materialization target; source is not a canonical alias ID.
    let _ = source;
    for (_, old_feed, _) in rows {
        if old_feed != feed {
            super::logical::touch(conn, "", &format!("cleanup-feed/{old_feed}"))?;
        }
    }
    Ok(())
}
pub(super) fn cleanup_feed(conn: &Connection, id: i64) -> AppResult<()> {
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT sync_id,feed_url FROM feeds WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((source, url)) = row else {
        return Ok(());
    };
    let Some(Register {
        value: FieldValue::Set(Value::String(logical_url)),
        ..
    }) = super::logical::get(conn, &format!("s/{source}/feed_url"))?
    else {
        return Ok(());
    };
    let replacement: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM feeds WHERE id!=?1 AND feed_url=?2)",
        params![id, logical_url],
        |r| r.get(0),
    )?;
    if replacement {
        conn.execute(
            "DELETE FROM feeds WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM articles WHERE feed_id=?1)",
            [id],
        )?;
    } else if url != logical_url {
        super::logical::touch(conn, "", &format!("s/{source}"))?;
    }
    Ok(())
}

pub(super) fn apply_projection(
    conn: &Connection,
    fields: &BTreeMap<String, Register>,
    origin: Option<&ArticleMeta>,
) -> AppResult<()> {
    let prefixes: BTreeSet<String> = fields
        .keys()
        .filter_map(|k| k.rsplit_once('/').map(|v| v.0.to_owned()))
        .collect();
    for prefix in prefixes.iter().filter(|k| k.starts_with("g/")) {
        let uid = &prefix[2..];
        validate_group_entity(fields, prefix)?;
        if fields.contains_key(&format!("{prefix}/deleted")) {
            continue;
        }
        let name = string(fields, prefix, "name", "");
        let layout = string(fields, prefix, "layout", "article");
        let existing: Option<i64> = conn
            .query_row("SELECT id FROM folders WHERE sync_id=?1", [uid], |r| {
                r.get(0)
            })
            .optional()?;
        let id = match existing {
            Some(id) => id,
            None => db::create_folder_with_sync_id(conn, name, layout, Some(uid))?,
        };
        let collapsed = value(fields, prefix, "collapsed")
            .map(|v| v.as_bool().unwrap_or(v.as_i64() == Some(1)))
            .unwrap_or(true);
        conn.execute("UPDATE folders SET origin='local',name=?1,layout=?2,summary_mode=?3,translate_mode=?4,position=?5,collapsed=?6 WHERE id=?7",params![name,layout,string(fields,prefix,"summary_mode","inherit"),string(fields,prefix,"translate_mode","inherit"),value(fields,prefix,"position").and_then(Value::as_i64).unwrap_or(0),collapsed,id])?;
    }
    let mut sources = BTreeMap::new();
    for prefix in prefixes.iter().filter(|k| k.starts_with("s/")) {
        let source = &prefix[2..];
        let url = string(fields, prefix, "feed_url", "");
        if url.is_empty() {
            return Err(invalid("来源缺少订阅URL"));
        }
        let occupant: Option<(i64, String)> = conn
            .query_row(
                "SELECT id,sync_id FROM feeds WHERE feed_url=?1",
                [url],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((id, owner)) = occupant {
            if let Some(Register {
                value: FieldValue::Set(Value::String(current_url)),
                ..
            }) = super::logical::get(conn, &format!("s/{owner}/feed_url"))?
            {
                if current_url != url {
                    // A URL split/swap can arrive before either physical source
                    // moves. Free only its locator; consumers read the real URL
                    // from L, and the owner's work restores this row in a slice.
                    conn.execute(
                        "UPDATE feeds SET feed_url=?1 WHERE id=?2",
                        params![format!("urn:fluxreader:materializing:{owner}"), id],
                    )?;
                    super::logical::touch(conn, "", &format!("s/{owner}"))?;
                    super::logical::touch(conn, "", &format!("source/{owner}"))?;
                }
            }
        }
        let existing: Option<i64> = conn
            .query_row("SELECT id FROM feeds WHERE feed_url=?1", [url], |r| {
                r.get(0)
            })
            .optional()?;
        let own: Option<i64> = conn
            .query_row("SELECT id FROM feeds WHERE sync_id=?1", [source], |r| {
                r.get(0)
            })
            .optional()?;
        let folder = optional(fields, prefix, "folder");
        let folder_id: Option<i64> = match folder {
            Some(folder) => conn
                .query_row("SELECT id FROM folders WHERE sync_id=?1", [folder], |r| {
                    r.get(0)
                })
                .optional()?,
            None => None,
        };
        let active = active_source(fields, prefix);
        let id = match existing.or(own) {
            Some(id) => id,
            None => {
                conn.execute("INSERT INTO feeds(sync_id,feed_url,title,folder_id,active) VALUES(?1,?2,?3,?4,?5)",params![source,url,string(fields,prefix,"title",url),folder_id,active])?;
                conn.last_insert_rowid()
            }
        };
        if let Some(old) = own.filter(|old| *old != id) {
            // Article projection also resolves this source. Once the old feed
            // is empty, re-enqueueing its scope would keep recreating article
            // work ahead of cleanup forever. Only actual unmigrated rows create
            // this prerequisite; the same writer still schedules checked cleanup.
            conn.execute("INSERT OR IGNORE INTO local_sync_work(job,key,revision) SELECT '',?1,(SELECT logical_revision FROM local_sync_control WHERE id=1) WHERE EXISTS(SELECT 1 FROM articles WHERE feed_id=?2)",params![format!("source/{source}"),old])?;
            super::logical::touch(conn, "", &format!("cleanup-feed/{old}"))?;
        }
        // Do not change sync_id when a smaller alias is discovered: existing wire keys remain stable.
        let custom = optional(fields, prefix, "title_custom").filter(|s| !s.trim().is_empty());
        let fetched = string(fields, prefix, "fetched_title", "");
        let title = custom.as_deref().unwrap_or_else(|| {
            if fetched.trim().is_empty() {
                string(fields, prefix, "title", url)
            } else {
                fetched
            }
        });
        // Cache only this fixed statement on its owning connection. Every call
        // rebinds all twelve parameters; winner/projection results stay uncached.
        conn.prepare_cached("UPDATE feeds SET origin='local',feed_url=?12,title=?1,title_custom=?10,fetched_title=?11,site_url=?2,favicon_url=?3,folder_id=?4,layout=?5,summary_mode=?6,translate_mode=?7,active=?8,fetch_generation=fetch_generation+CASE WHEN active!=?8 OR feed_url!=?12 THEN 1 ELSE 0 END WHERE id=?9 AND (origin!='local' OR feed_url IS NOT ?12 OR title IS NOT ?1 OR title_custom IS NOT ?10 OR fetched_title IS NOT ?11 OR site_url IS NOT ?2 OR favicon_url IS NOT ?3 OR folder_id IS NOT ?4 OR layout IS NOT ?5 OR summary_mode IS NOT ?6 OR translate_mode IS NOT ?7 OR active IS NOT ?8)")?.execute(params![title,optional(fields,prefix,"site_url"),optional(fields,prefix,"favicon_url"),folder_id,string(fields,prefix,"layout","inherit"),string(fields,prefix,"summary_mode","inherit"),string(fields,prefix,"translate_mode","inherit"),active,id,optional(fields,prefix,"title_custom"),string(fields,prefix,"fetched_title",""),url])?;
        if let Some(deleted) = fields.get(&format!("{prefix}/deleted")) {
            let stamp = serde_json::to_string(&deleted.stamp)?;
            let local_source: String =
                conn.query_row("SELECT sync_id FROM feeds WHERE id=?1", [id], |r| r.get(0))?;
            conn.execute("INSERT INTO local_sync_epochs(source_key,stamp) VALUES(?1,?2) ON CONFLICT(source_key) DO UPDATE SET stamp=excluded.stamp",params![local_source,stamp])?;
            if !active {
                conn.execute("INSERT INTO source_tombstones(source_id,lifecycle_version,feed_url) VALUES(?1,?2,?3) ON CONFLICT(source_id) DO UPDATE SET lifecycle_version=excluded.lifecycle_version,feed_url=excluded.feed_url",params![local_source,deleted.stamp.lamport,url])?;
            }
        }
        sources.insert(source.to_owned(), (id, active));
    }
    for prefix in prefixes.iter().filter(|k| k.starts_with("a/")) {
        let Some(v) = value(fields, prefix, "meta") else {
            return Err(invalid("文章缺少身份"));
        };
        let meta: ArticleMeta = serde_json::from_value(v.clone())?;
        let Some((feed_id, active)) = sources.get(&meta.source) else {
            return Err(invalid("文章引用了缺失来源"));
        };
        let read = value(fields, prefix, "read")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let star = value(fields, prefix, "star")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let source = origin.map_or(meta.source.as_str(), |origin| origin.source.as_str());
        let url = string(fields, &format!("s/{}", meta.source), "feed_url", "");
        reconcile_article_rows(conn, *feed_id, url, &meta.guid, source)?;
        let existing: Option<i64> = conn
            .query_row(
                "SELECT id FROM articles WHERE feed_id=?1 AND guid=?2",
                params![feed_id, meta.guid],
                |r| r.get(0),
            )
            .optional()?;
        if existing.is_none() {
            // This is a physical locator only. The caller resolved source/GUID
            // from L before this move; it never creates or renames a wire key.
            let owned:Option<i64>=conn.query_row("SELECT id FROM articles WHERE local_sync_source=?1 AND guid=?2 ORDER BY id LIMIT 1",params![source,meta.guid],|r|r.get(0)).optional()?;
            if let Some(id) = owned {
                conn.execute(
                    "UPDATE articles SET feed_id=?1 WHERE id=?2",
                    params![feed_id, id],
                )?;
            }
        }
        if !active && !star {
            conn.execute(
                "DELETE FROM articles WHERE feed_id=?1 AND guid=?2 AND NOT EXISTS(SELECT 1 FROM local_sync_body_hold WHERE article_id=articles.id) AND NOT EXISTS(SELECT 1 FROM sync_queue WHERE article_id=articles.id)",
                params![feed_id, meta.guid],
            )?;
            continue;
        }
        conn.execute("INSERT INTO articles(feed_id,guid,title,is_read,is_starred,source,local_sync_source) VALUES(?1,?2,?3,?4,?5,'direct',?6) ON CONFLICT(feed_id,guid) DO NOTHING",params![feed_id,meta.guid,meta.title,read,star,source])?;
        conn.execute("UPDATE articles SET read_version=read_version+CASE WHEN is_read!=?1 THEN 1 ELSE 0 END,star_version=star_version+CASE WHEN is_starred!=?2 THEN 1 ELSE 0 END,is_read=?1,is_starred=?2 WHERE feed_id=?3 AND guid=?4 AND (is_read!=?1 OR is_starred!=?2)",params![read,star,feed_id,meta.guid])?;
        conn.execute("UPDATE articles SET url=?1,url_norm=?2,title=?3,author=?4,published_at=COALESCE(?5,published_at),published_at_source=?6,first_seen_at=COALESCE(?7,first_seen_at),image_url=?8,image_source=?9,enclosure_url=?10,enclosure_mime=?11,duration_sec=?12 WHERE feed_id=?13 AND guid=?14 AND (url IS NOT ?1 OR url_norm IS NOT ?2 OR title IS NOT ?3 OR author IS NOT ?4 OR published_at IS NOT COALESCE(?5,published_at) OR published_at_source IS NOT ?6 OR first_seen_at IS NOT COALESCE(?7,first_seen_at) OR image_url IS NOT ?8 OR image_source IS NOT ?9 OR enclosure_url IS NOT ?10 OR enclosure_mime IS NOT ?11 OR duration_sec IS NOT ?12)",params![meta.url,meta.url.as_deref().map(db::normalize_url),meta.title,meta.author,meta.published_at,meta.published_at_source,meta.first_seen_at,meta.image_url,meta.image_source,meta.enclosure_url,meta.enclosure_mime,meta.duration_sec,feed_id,meta.guid])?;
        if let Some(desired) = value(fields, prefix, "body") {
            let desired: BodyRef = serde_json::from_value(desired.clone())?;
            let id: i64 = conn.query_row(
                "SELECT id FROM articles WHERE feed_id=?1 AND guid=?2",
                params![feed_id, meta.guid],
                |r| r.get(0),
            )?;
            if let Some(body) = body_reference(conn, id)? {
                super::logical::hold_body(conn, id, &entry_key(source, &meta.guid), &body)?;
            }
            conn.execute("UPDATE articles SET content_id=NULL,rss_content_id=NULL WHERE feed_id=?1 AND guid=?2 AND content_id IS NOT NULL AND (COALESCE((SELECT identity FROM contents WHERE id=articles.content_id),'')!=?3 OR COALESCE((SELECT identity FROM contents WHERE id=articles.rss_content_id),'')!=?4)",params![feed_id,meta.guid,desired.primary,desired.rss.as_deref().unwrap_or("")])?;
            conn.execute("UPDATE articles SET content_id=(SELECT id FROM contents WHERE identity=?1),rss_content_id=(SELECT id FROM contents WHERE identity=?2),summary=(SELECT substr(body_text,1,400) FROM contents WHERE identity=?1) WHERE id=?3 AND content_id IS NULL AND EXISTS(SELECT 1 FROM contents WHERE identity=?1) AND (?2 IS NULL OR EXISTS(SELECT 1 FROM contents WHERE identity=?2))",params![desired.primary,desired.rss,id])?;
        }
    }
    for prefix in prefixes
        .iter()
        .filter(|k| k.starts_with("g/") && fields.contains_key(&format!("{k}/deleted")))
    {
        // Source lifecycle was already projected independently. Deleting a stale group
        // must not implicitly delete/reactivate sources after their own user actions.
        conn.execute("UPDATE feeds SET folder_id=NULL WHERE folder_id IN(SELECT id FROM folders WHERE sync_id=?1)",[&prefix[2..]])?;
        conn.execute("DELETE FROM folders WHERE sync_id=?1", [&prefix[2..]])?;
    }
    if !fields.keys().any(|key| key.starts_with("p/")) {
        return Ok(());
    }
    let raw = db::get_setting(conn, "app_settings")?.unwrap_or_else(|| "{}".to_owned());
    let mut settings: Value = serde_json::from_str(&raw)?;
    let object = settings
        .as_object_mut()
        .ok_or_else(|| invalid("本机设置结构损坏"))?;
    for (key, reg) in fields.iter().filter(|(key, _)| key.starts_with("p/")) {
        match &reg.value {
            FieldValue::Set(v) => {
                object.insert(key[2..].to_owned(), v.clone());
            }
            FieldValue::Delete => {
                object.remove(&key[2..]);
            }
        }
    }
    db::set_setting(conn, "app_settings", &serde_json::to_string(&settings)?)?;
    Ok(())
}

pub fn apply_bodies(conn: &Connection, expected: i64, bodies: &[BodyRecord]) -> AppResult<usize> {
    let tx = crate::db::write_transaction(conn)?;
    let count = apply_bodies_in(&tx, expected, bodies)?;
    tx.commit()?;
    Ok(count)
}
pub(super) fn apply_bodies_in(
    conn: &Connection,
    expected: i64,
    bodies: &[BodyRecord],
) -> AppResult<usize> {
    require_generation(conn, expected)?;
    for body in bodies {
        body.validate()?;
    }
    let mut by_ref = BTreeMap::new();
    for body in bodies {
        by_ref.insert(serde_json::to_string(&body.body_ref)?, body);
    }
    let tx = conn;
    let previous: bool = conn.query_row(
        "SELECT applying FROM local_sync_control WHERE id=1",
        [],
        |r| r.get(0),
    )?;
    tx.execute("UPDATE local_sync_control SET applying=1 WHERE id=1", [])?;
    let mut count = 0;
    let mut stmt =
        conn.prepare("SELECT key FROM local_sync_winners WHERE key LIKE 'a/%/body' ORDER BY key")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let key: String = row.get(0)?;
        let prefix = key.trim_end_matches("/body");
        let fields = super::logical::projected(conn, prefix)?;
        let Some(projected_prefix) = fields
            .keys()
            .find(|key| key.starts_with("a/"))
            .and_then(|key| key.rsplit_once('/').map(|(p, _)| p))
        else {
            continue;
        };
        let prefix = projected_prefix;
        let Some(body_ref) = value(&fields, prefix, "body") else {
            continue;
        };
        let body_ref: BodyRef = serde_json::from_value(body_ref.clone())?;
        let Some(body) = by_ref.get(&serde_json::to_string(&body_ref)?) else {
            continue;
        };
        let Some(meta) = value(&fields, prefix, "meta") else {
            continue;
        };
        let meta: ArticleMeta = serde_json::from_value(meta.clone())?;
        if value(&fields, prefix, "star") != Some(&Value::Bool(true))
            && (value(&fields, prefix, "read") == Some(&Value::Bool(true))
                || !active_source(&fields, &format!("s/{}", meta.source)))
        {
            continue;
        }
        super::logical::materialize(conn, key.trim_end_matches("/body"))?;
        let url = string(&fields, &format!("s/{}", meta.source), "feed_url", "");
        let id:Option<i64>=tx.query_row("SELECT a.id FROM articles a JOIN feeds f ON f.id=a.feed_id WHERE f.feed_url=?1 AND a.guid=?2",params![url,meta.guid],|r|r.get(0)).optional()?;
        let Some(id) = id else { continue };
        let primary = install_content(tx, id, &body.primary, meta.url.as_deref())?;
        let rss = body
            .rss
            .as_ref()
            .map(|b| install_content(tx, id, b, meta.url.as_deref()))
            .transpose()?;
        count+=tx.execute("UPDATE articles SET content_id=?1,rss_content_id=?2,summary=?3 WHERE id=?4 AND (content_id IS NOT ?1 OR rss_content_id IS NOT ?2)",params![primary,rss,body.primary.text.chars().take(400).collect::<String>(),id])?;
        tx.execute(
            "DELETE FROM local_sync_body_hold WHERE article_id=?1 AND body_ref=?2",
            params![id, serde_json::to_string(&body_ref)?],
        )?;
    }
    tx.execute(
        "UPDATE local_sync_control SET applying=?1,data_revision=data_revision+?2 WHERE id=1",
        params![previous, count > 0],
    )?;
    Ok(count)
}
fn install_content(
    conn: &Connection,
    id: i64,
    body: &BodyContent,
    url: Option<&str>,
) -> AppResult<i64> {
    let html = body
        .html
        .as_deref()
        .map(|html| crate::sanitize::sanitize(html, url));
    // Payload bytes have been verified; sanitizer is still required at the untrusted boundary.
    // Reject a non-canonical body instead of acknowledging bytes we did not install.
    if html != body.html {
        return Err(invalid("远端正文未通过安全规范化，保留待补状态"));
    }
    let (id, _, _) = db::portable_ensure_content(
        conn,
        &db::PortableContentInput {
            content_html: html,
            body_text: body.text.clone(),
            extraction_source: body.source.clone(),
            fulltext_extracted: body.extracted,
            url_norm: url.map(db::normalize_url),
        },
        Some(id),
    )?;
    Ok(id)
}

pub fn pending_bodies(conn: &Connection) -> AppResult<i64> {
    super::logical::require_valid_locators(conn)?;
    Ok(conn.query_row(&format!("SELECT COUNT(*) FROM articles a JOIN feeds f ON f.id=a.feed_id JOIN local_sync_article_state l ON l.id=a.id LEFT JOIN contents c ON c.id=a.content_id WHERE {} AND l.identity_valid=1 AND ((l.is_read=0 AND l.active=1) OR l.is_starred=1) AND (c.id IS NULL OR (COALESCE(c.content_html,'')='' AND c.body_text='') OR (l.body_ref IS NOT NULL AND (json_extract(l.body_ref,'$.primary') IS NOT c.identity OR COALESCE(json_extract(l.body_ref,'$.rss'),'') IS NOT COALESCE((SELECT identity FROM contents WHERE id=a.rss_content_id),''))))",local_source("f")),[],|r|r.get(0))?)
}

/// Resolve only aliases of this exact raw URL/GUID using the same display order.
/// This avoids rebuilding a full 50k-entry checkpoint for every payload block.
pub(super) fn desired_body(conn: &Connection, url: &str, guid: &str) -> AppResult<Option<BodyRef>> {
    let mut stmt=conn.prepare("SELECT b.key,b.register FROM local_sync_winners m JOIN local_sync_winners s ON s.key='s/'||json_extract(m.register,'$.value.value.source')||'/feed_url' JOIN local_sync_winners b ON b.key=substr(m.key,1,length(m.key)-5)||'/body' WHERE m.key LIKE 'a/%/meta' AND json_extract(m.register,'$.value.value.guid')=?1 AND json_extract(s.register,'$.value.value')=?2")?;
    let mut projected = Projection::default();
    for row in stmt.query_map(params![guid, url], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })? {
        let (key, raw) = row?;
        projected.select("body".into(), &key, serde_json::from_str(&raw)?)?;
    }
    match projected.into_fields().remove("body").map(|r| r.value) {
        Some(FieldValue::Set(value)) => Ok(Some(serde_json::from_value(value)?)),
        _ => Ok(None),
    }
}
pub(super) fn body_targets(conn: &Connection) -> AppResult<Vec<(i64, BodyRef)>> {
    let mut stmt=conn.prepare(&format!("SELECT a.id,l.url,a.guid FROM articles a JOIN feeds f ON f.id=a.feed_id JOIN local_sync_article_state l ON l.id=a.id LEFT JOIN contents c ON c.id=a.content_id WHERE {} AND l.identity_valid=1 AND ((l.is_read=0 AND l.active=1) OR l.is_starred=1) AND (c.id IS NULL OR (COALESCE(c.content_html,'')='' AND c.body_text='') OR (l.body_ref IS NOT NULL AND (json_extract(l.body_ref,'$.primary') IS NOT c.identity OR COALESCE(json_extract(l.body_ref,'$.rss'),'') IS NOT COALESCE((SELECT identity FROM contents WHERE id=a.rss_content_id),'')))) ORDER BY a.id",local_source("f")))?;
    let mut rows = stmt.query([])?;
    let mut result = Vec::new();
    while let Some(row) = rows.next()? {
        let id = row.get(0)?;
        let url: String = row.get(1)?;
        let guid: String = row.get(2)?;
        if let Some(body) = desired_body(conn, &url, &guid)? {
            result.push((id, body));
        }
    }
    Ok(result)
}
pub(super) fn apply_body_targets(
    conn: &Connection,
    expected: i64,
    targets: &[(i64, BodyRef)],
    bodies: &BTreeMap<String, BodyContent>,
) -> AppResult<usize> {
    let tx = crate::db::write_transaction(conn)?;
    require_generation(&tx, expected)?;
    let previous: bool = tx.query_row(
        "SELECT applying FROM local_sync_control WHERE id=1",
        [],
        |r| r.get(0),
    )?;
    tx.execute("UPDATE local_sync_control SET applying=1 WHERE id=1", [])?;
    let mut count = 0;
    let mut installed = BTreeMap::new();
    for (id, body_ref) in targets {
        let target:Option<(String,String,Option<String>)>=tx.query_row("SELECT l.url,a.guid,a.url FROM articles a JOIN local_sync_article_state l ON l.id=a.id WHERE a.id=?1 AND l.identity_valid=1 AND ((l.is_read=0 AND l.active=1) OR l.is_starred=1)",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        let Some((feed, guid, url)) = target else {
            continue;
        };
        if desired_body(&tx, &feed, &guid)?.as_ref() != Some(body_ref) {
            continue;
        }
        let primary = bodies
            .get(&body_ref.primary)
            .ok_or_else(|| invalid("目录引用的主正文不在对应流中"))?;
        let primary_id = match installed.get(&body_ref.primary) {
            Some(id) => *id,
            None => {
                let cid = install_content(&tx, *id, primary, url.as_deref())?;
                installed.insert(body_ref.primary.clone(), cid);
                cid
            }
        };
        let rss_id = body_ref
            .rss
            .as_ref()
            .map(|identity| {
                bodies
                    .get(identity)
                    .ok_or_else(|| invalid("目录引用的RSS正文不在对应流中"))
                    .and_then(|body| match installed.get(identity) {
                        Some(id) => Ok(*id),
                        None => {
                            let cid = install_content(&tx, *id, body, url.as_deref())?;
                            installed.insert(identity.clone(), cid);
                            Ok(cid)
                        }
                    })
            })
            .transpose()?;
        count+=tx.execute("UPDATE articles SET content_id=?1,rss_content_id=?2,summary=?3 WHERE id=?4 AND (content_id IS NOT ?1 OR rss_content_id IS NOT ?2)",params![primary_id,rss_id,primary.text.chars().take(400).collect::<String>(),id])?;
        tx.execute(
            "DELETE FROM local_sync_body_hold WHERE article_id=?1 AND body_ref=?2",
            params![id, serde_json::to_string(body_ref)?],
        )?;
    }
    tx.execute(
        "UPDATE local_sync_control SET applying=?1,data_revision=data_revision+?2 WHERE id=1",
        params![previous, count > 0],
    )?;
    tx.commit()?;
    Ok(count)
}
