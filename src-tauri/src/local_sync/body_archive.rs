//! Body archives have a small paged catalog and independently hashed content
//! streams. Only identity/offset metadata scales with the library; payload memory
//! is one bounded stream. A read snapshot is spooled outside the primary DB, then
//! cached with short write transactions before its manifest is sealed.
use super::{
    database,
    model::*,
    storage::{self, Manifest, ObjectRef, Pending},
};
use crate::{
    error::{AppError, AppResult},
    webdav_transport::{FieldValue, Frontier, Register},
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
};

const CHUNK: usize = 1024 * 1024;
const STREAM_LIMIT: usize = 128 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub entry: String,
    pub body_ref: BodyRef,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stream {
    pub digest: String,
    pub bytes: usize,
    pub items: usize,
    pub parts: Vec<ObjectRef>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Row {
    Entry { entry: String, body_ref: BodyRef },
    Content { identity: String, stream: String },
    Stream(Stream),
}
#[derive(Default)]
pub struct Catalog {
    pub entries: BTreeMap<String, Entry>,
    pub contents: BTreeMap<String, String>,
    pub streams: BTreeMap<String, Stream>,
}
fn entry_id(entry: &Entry) -> AppResult<String> {
    Ok(digest(&serde_json::to_vec(entry)?))
}
fn valid_identity(identity: &str) -> bool {
    identity.starts_with("h|") && identity.len() < 128
}
impl Catalog {
    fn rows(self) -> BTreeMap<String, Row> {
        let mut rows = BTreeMap::new();
        for (id, e) in self.entries {
            rows.insert(
                format!("e/{id}"),
                Row::Entry {
                    entry: e.entry,
                    body_ref: e.body_ref,
                },
            );
        }
        for (identity, stream) in self.contents {
            rows.insert(format!("c/{identity}"), Row::Content { identity, stream });
        }
        for (hash, stream) in self.streams {
            rows.insert(format!("b/{hash}"), Row::Stream(stream));
        }
        rows
    }
    fn accept(&mut self, key: String, row: Row) -> AppResult<()> {
        let valid = match row {
            Row::Entry { entry, body_ref } => {
                let e = Entry { entry, body_ref };
                let id = entry_id(&e)?;
                let valid = valid_hex(&e.entry, 64)
                    && valid_identity(&e.body_ref.primary)
                    && e.body_ref.rss.as_deref().map_or(true, valid_identity)
                    && key == format!("e/{id}");
                valid && self.entries.insert(id, e).is_none()
            }
            Row::Content { identity, stream } => {
                let valid = valid_identity(&identity)
                    && valid_hex(&stream, 64)
                    && key == format!("c/{identity}");
                valid && self.contents.insert(identity, stream).is_none()
            }
            Row::Stream(stream) => {
                let mut size = 0usize;
                let mut valid = key == format!("b/{}", stream.digest)
                    && valid_hex(&stream.digest, 64)
                    && stream.bytes > 0
                    && stream.bytes <= STREAM_LIMIT
                    && stream.items > 0
                    && stream.items <= 128
                    && stream.parts.len() == stream.bytes.div_ceil(CHUNK);
                for part in &stream.parts {
                    storage::validate_object(part)?;
                    size = size
                        .checked_add(part.size)
                        .ok_or_else(|| invalid("正文流长度溢出"))?;
                }
                valid &= size == stream.bytes;
                valid && self.streams.insert(stream.digest.clone(), stream).is_none()
            }
        };
        if !valid {
            return Err(invalid("正文目录身份、重复记录或范围无效"));
        }
        Ok(())
    }
    fn validate(&self) -> AppResult<()> {
        let mut needed = BTreeSet::new();
        for entry in self.entries.values() {
            needed.insert(&entry.body_ref.primary);
            if let Some(rss) = &entry.body_ref.rss {
                needed.insert(rss);
            }
        }
        if needed.len() != self.contents.len()
            || needed
                .into_iter()
                .any(|identity| !self.contents.contains_key(identity))
        {
            return Err(invalid("正文目录引用不完整"));
        }
        let streams: BTreeSet<_> = self.contents.values().collect();
        if streams.len() != self.streams.len()
            || streams.into_iter().any(|id| !self.streams.contains_key(id))
        {
            return Err(invalid("正文目录分块引用不完整"));
        }
        Ok(())
    }
    pub fn objects(&self) -> Vec<ObjectRef> {
        let mut objects = BTreeMap::new();
        for stream in self.streams.values() {
            for part in &stream.parts {
                objects.insert(part.name.clone(), part.clone());
            }
        }
        objects.into_values().collect()
    }
    pub fn needed_objects(&self, refs: &[BodyRef]) -> Vec<ObjectRef> {
        let mut objects = BTreeMap::new();
        for body in refs {
            for identity in std::iter::once(&body.primary).chain(body.rss.iter()) {
                if let Some(stream) = self
                    .contents
                    .get(identity)
                    .and_then(|s| self.streams.get(s))
                {
                    for part in &stream.parts {
                        objects.insert(part.name.clone(), part.clone());
                    }
                }
            }
        }
        objects.into_values().collect()
    }
}

pub fn catalog(conn: &Connection, manifest: &Manifest) -> AppResult<Catalog> {
    manifest.validate()?;
    if manifest.kind != "body" {
        return Err(invalid("正文目录类型错误"));
    }
    let mut result = Catalog::default();
    for page in &manifest.shards {
        let bytes = storage::cached_object(conn, &manifest.actor, "body", page)?
            .ok_or_else(|| AppError::new("localSyncPending", "正文目录页仍在下载"))?;
        let rows: BTreeMap<String, Row> = serde_json::from_slice(&bytes)?;
        if rows.is_empty() {
            return Err(invalid("空正文目录页"));
        }
        for (key, row) in rows {
            result.accept(key, row)?;
            if result.entries.len() + result.contents.len() + result.streams.len() > MAX_FIELDS {
                return Err(AppError::new(
                    "localSyncCapacity",
                    "正文目录记录超过解析预算；本机数据保留待同步",
                ));
            }
        }
    }
    result.validate()?;
    Ok(result)
}
pub fn record_links(conn: &Connection, manifest: &Manifest) -> AppResult<()> {
    let _writer = crate::db::writer_guard(conn)?;
    for page in &manifest.shards {
        let bytes = storage::cached_object(conn, &manifest.actor, "body", page)?
            .ok_or_else(|| invalid("正文目录页缺失"))?;
        let rows: BTreeMap<String, Row> = serde_json::from_slice(&bytes)?;
        for row in rows.into_values() {
            if let Row::Stream(stream) = row {
                for part in stream.parts {
                    conn.execute("INSERT OR IGNORE INTO local_sync_object_links(actor,kind,parent,child) VALUES(?1,'body',?2,?3)",params![manifest.actor,page.digest,part.digest])?;
                }
            }
        }
    }
    Ok(())
}
fn load_stream(
    conn: &Connection,
    manifest: &Manifest,
    stream: &Stream,
) -> AppResult<BTreeMap<String, BodyContent>> {
    let mut bytes = Vec::with_capacity(stream.bytes);
    for part in &stream.parts {
        bytes.extend(
            storage::cached_object(conn, &manifest.actor, "body", part)?
                .ok_or_else(|| AppError::new("localSyncPending", "正文内容块仍在下载"))?,
        );
    }
    if bytes.len() != stream.bytes || digest(&bytes) != stream.digest {
        return Err(invalid("正文流完整性校验失败"));
    }
    let bodies: Vec<BodyContent> = serde_json::from_slice(&bytes)?;
    if bodies.len() != stream.items {
        return Err(invalid("正文流记录数不匹配"));
    }
    let mut result = BTreeMap::new();
    for body in bodies {
        body.validate()?;
        if result.insert(body.identity.clone(), body).is_some() {
            return Err(invalid("正文流身份重复"));
        }
    }
    Ok(result)
}

pub(super) struct Spool {
    file: Option<File>,
    path: PathBuf,
    objects: BTreeMap<String, (ObjectRef, u64)>,
}
impl Spool {
    pub(super) fn new(conn: &Connection) -> AppResult<Self> {
        let path = std::env::temp_dir().join(format!(
            "fluxreader-body-{}.stage",
            storage::random_id(conn)?
        ));
        let mut options = OpenOptions::new();
        options.create_new(true).read(true).write(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // FILE_FLAG_DELETE_ON_CLOSE, verified in windows 0.61.3 FileSystem:
            // the OS also removes this owned spool after process termination.
            options.custom_flags(0x0400_0000);
        }
        let file = options.open(&path).map_err(io)?;
        Ok(Self {
            file: Some(file),
            path,
            objects: BTreeMap::new(),
        })
    }
    pub(super) fn put(
        &mut self,
        conn: &Connection,
        bytes: &[u8],
        known: &BTreeMap<String, ObjectRef>,
    ) -> AppResult<ObjectRef> {
        let hash = digest(bytes);
        if let Some(object) = known.get(&hash) {
            return Ok(object.clone());
        }
        if let Some((object, _)) = self.objects.get(&hash) {
            return Ok(object.clone());
        }
        let attempt = storage::random_id(conn)?;
        let object = ObjectRef {
            name: format!("{hash}-{attempt}.bin"),
            digest: hash.clone(),
            size: bytes.len(),
        };
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| invalid("正文暂存文件已关闭"))?;
        let offset = file.stream_position().map_err(io)?;
        file.write_all(bytes).map_err(io)?;
        self.objects.insert(hash, (object.clone(), offset));
        Ok(object)
    }
    pub(super) fn persist(
        &mut self,
        conn: &Connection,
        generation: i64,
        actor: &str,
        owner: &str,
        kind: &str,
    ) -> AppResult<()> {
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| invalid("正文暂存文件已关闭"))?;
        file.flush().map_err(io)?;
        for (object, offset) in self.objects.values() {
            let mut bytes = vec![0; object.size];
            file.seek(SeekFrom::Start(*offset)).map_err(io)?;
            file.read_exact(&mut bytes).map_err(io)?;
            let tx = crate::db::write_transaction(conn)?;
            database::require_generation(&tx, generation)?;
            tx.execute("INSERT INTO local_sync_objects(actor,kind,digest,bytes) VALUES(?1,?2,?3,?4) ON CONFLICT(actor,kind,digest) DO NOTHING",params![actor,kind,object.digest,bytes])?;
            tx.execute("INSERT OR IGNORE INTO local_sync_object_pins(owner,actor,kind,digest) VALUES(?1,?2,?3,?4)",params![owner,actor,kind,object.digest])?;
            tx.commit()?;
            std::thread::yield_now();
        }
        Ok(())
    }
}
impl Drop for Spool {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = std::fs::remove_file(&self.path);
    }
}
fn io(error: std::io::Error) -> AppError {
    AppError::new("localSyncStorage", error.to_string())
}
fn flush_contents(
    conn: &Connection,
    batch: &mut Vec<BodyContent>,
    catalog: &mut Catalog,
    spool: &mut Spool,
    known: &BTreeMap<String, ObjectRef>,
) -> AppResult<()> {
    if batch.is_empty() {
        return Ok(());
    }
    let bytes = serde_json::to_vec(batch)?;
    if bytes.len() > STREAM_LIMIT {
        return Err(invalid("单篇正文超过编码流预算"));
    }
    let hash = digest(&bytes);
    let mut parts = Vec::new();
    for bytes in bytes.chunks(CHUNK) {
        parts.push(spool.put(conn, bytes, known)?);
    }
    for body in batch.iter() {
        catalog.contents.insert(body.identity.clone(), hash.clone());
    }
    catalog.streams.insert(
        hash.clone(),
        Stream {
            digest: hash,
            bytes: bytes.len(),
            items: batch.len(),
            parts,
        },
    );
    batch.clear();
    Ok(())
}
fn enqueue_content(
    conn: &Connection,
    body: BodyContent,
    batch: &mut Vec<BodyContent>,
    size: &mut usize,
    catalog: &mut Catalog,
    spool: &mut Spool,
    known: &BTreeMap<String, ObjectRef>,
) -> AppResult<()> {
    body.validate()?;
    let length = serde_json::to_vec(&body)?.len();
    if !batch.is_empty() && (*size + length > CHUNK || batch.len() >= 128) {
        flush_contents(conn, batch, catalog, spool, known)?;
        *size = 0;
    }
    *size += length + 1;
    batch.push(body);
    Ok(())
}
struct Prepared {
    shards: Vec<ObjectRef>,
    hold_ids: Vec<i64>,
}

pub(super) fn body_rows_sql(logical_state: bool) -> String {
    let local = local_source("f");
    if logical_state {
        format!(
            "SELECT COALESCE(a.local_sync_source,f.sync_id),a.guid,c.id,c.identity,r.id,r.identity
             FROM articles a
             JOIN feeds f ON f.id=a.feed_id
             JOIN local_sync_article_state l ON l.id=a.id
             JOIN contents c ON c.id=a.content_id
             LEFT JOIN contents r ON r.id=a.rss_content_id
             WHERE {local} AND l.identity_valid=1
               AND (((l.is_read=0 AND l.active=1) OR l.is_starred=1)
                 AND (l.body_ref IS NULL OR (
                   json_extract(l.body_ref,'$.primary')=c.identity
                   AND COALESCE(json_extract(l.body_ref,'$.rss'),'')=COALESCE(r.identity,'')
                 ))
                 OR EXISTS(
                   SELECT 1 FROM local_sync_body_hold h
                   WHERE h.article_id=a.id
                     AND json_extract(h.body_ref,'$.primary')=c.identity
                     AND COALESCE(json_extract(h.body_ref,'$.rss'),'')=COALESCE(r.identity,'')
                 ))
               AND (COALESCE(c.content_html,'')!='' OR c.body_text!='')
             ORDER BY a.id"
        )
    } else {
        format!(
            "SELECT COALESCE(a.local_sync_source,f.sync_id),a.guid,c.id,c.identity,r.id,r.identity
             FROM articles a
             JOIN feeds f ON f.id=a.feed_id
             JOIN contents c ON c.id=a.content_id
             LEFT JOIN contents r ON r.id=a.rss_content_id
             WHERE {local}
               AND (((a.is_read=0 AND f.active=1) OR a.is_starred=1)
                 OR EXISTS(SELECT 1 FROM local_sync_body_hold h WHERE h.article_id=a.id))
               AND (COALESCE(c.content_html,'')!='' OR c.body_text!='')
             ORDER BY a.id"
        )
    }
}

pub(super) fn body_rows_require_logical_state(conn: &Connection) -> AppResult<bool> {
    // Local captures update the physical row in the same transaction that records
    // their pending materialization/retention work, so those rows are already the
    // body snapshot's current truth. An accepted remote candidate and active
    // applying/folding remain logical until their projection finishes. The general
    // query surfaces deliberately use a wider predicate; applying that wider one
    // here makes an archive scan materialize every article state subquery.
    Ok(conn.query_row(
        "SELECT (capture=1 AND initialized=0)
                OR accepted_job IS NOT NULL OR folding!=0 OR applying!=0
         FROM local_sync_control WHERE id=1",
        [],
        |row| row.get(0),
    )?)
}

fn prepare(
    conn: &Connection,
    previous: Option<&Manifest>,
    spool: &mut Spool,
) -> AppResult<Prepared> {
    super::logical::require_valid_locators(conn)?;
    let mut catalog = previous
        .map(|m| self::catalog(conn, m))
        .transpose()?
        .unwrap_or_default();
    let mut known = BTreeMap::new();
    if let Some(previous) = previous {
        for object in previous.shards.iter().cloned().chain(catalog.objects()) {
            known.insert(object.digest.clone(), object);
        }
    }
    let mut current = BTreeMap::new();
    let mut ids = BTreeMap::new();
    let logical_state = body_rows_require_logical_state(conn)?;
    let mut stmt = conn.prepare(&body_rows_sql(logical_state))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let source: String = row.get(0)?;
        let guid: String = row.get(1)?;
        let primary: String = row.get(3)?;
        let rss: Option<String> = row.get(5)?;
        ids.insert(primary.clone(), row.get::<_, i64>(2)?);
        if let Some(rss) = &rss {
            ids.insert(rss.clone(), row.get::<_, i64>(4)?);
        }
        let entry = Entry {
            entry: entry_key(&source, &guid),
            body_ref: BodyRef { primary, rss },
        };
        current.insert(entry_id(&entry)?, entry);
    }
    drop(rows);
    drop(stmt);
    let hold_ids = {
        let mut stmt = conn.prepare("SELECT id FROM local_sync_body_hold ORDER BY id")?;
        let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    // A newer logical reference may detach an older protected payload before
    // that payload has its exact storage receipt. The hold references contents
    // by immutable identity, so it remains exportable after row projection.
    let mut stmt=conn.prepare("SELECT h.entry,c.id,c.identity,r.id,r.identity FROM local_sync_body_hold h JOIN contents c ON c.identity=json_extract(h.body_ref,'$.primary') LEFT JOIN contents r ON r.identity=json_extract(h.body_ref,'$.rss') ORDER BY h.entry,h.body_ref")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let owner: String = row.get(0)?;
        let primary: String = row.get(2)?;
        let rss: Option<String> = row.get(4)?;
        ids.insert(primary.clone(), row.get::<_, i64>(1)?);
        if let Some(rss) = &rss {
            ids.insert(rss.clone(), row.get::<_, i64>(3)?);
        }
        let entry = Entry {
            entry: owner,
            body_ref: BodyRef { primary, rss },
        };
        current.insert(entry_id(&entry)?, entry);
    }
    drop(rows);
    drop(stmt);
    let actor = database::actor(conn)?;
    let frontier: Frontier = serde_json::from_str(&conn.query_row(
        "SELECT frontier FROM local_sync_control WHERE id=1",
        [],
        |r| r.get::<_, String>(0),
    )?)?;
    let peers = storage::peer_frontiers(conn, &actor, &frontier)?;
    let mut release = Release::default();
    for (id, old) in std::mem::take(&mut catalog.entries) {
        if !current.contains_key(&id) && !release.can_release(conn, &peers, &old)? {
            current.insert(id, old);
        }
    }
    catalog.entries = current;
    let mut needed = BTreeSet::new();
    for entry in catalog.entries.values() {
        needed.insert(entry.body_ref.primary.clone());
        needed.extend(entry.body_ref.rss.iter().cloned());
    }
    catalog
        .contents
        .retain(|identity, _| needed.contains(identity));
    let live_streams: BTreeSet<_> = catalog.contents.values().cloned().collect();
    catalog.streams.retain(|id, _| live_streams.contains(id));
    let mut batch = Vec::new();
    let mut size = 0;
    for identity in needed
        .iter()
        .filter(|identity| !catalog.contents.contains_key(*identity))
        .cloned()
        .collect::<Vec<_>>()
    {
        let id = ids
            .get(&identity)
            .ok_or_else(|| invalid("正文归档失去尚未解除的保护内容"))?;
        let body =
            database::content(conn, Some(*id))?.ok_or_else(|| invalid("保护正文在读快照中缺失"))?;
        enqueue_content(
            conn,
            body,
            &mut batch,
            &mut size,
            &mut catalog,
            spool,
            &known,
        )?;
    }
    // Repack sparse shared streams only after the individual references were
    // proven releasable. At most one stream is decoded at a time.
    if let Some(previous) = previous {
        let mut counts = BTreeMap::<String, usize>::new();
        for stream in catalog.contents.values() {
            *counts.entry(stream.clone()).or_default() += 1;
        }
        let sparse: Vec<_> = catalog
            .streams
            .values()
            .filter(|s| counts.get(&s.digest).copied().unwrap_or(0) * 2 < s.items)
            .cloned()
            .collect();
        for stream in sparse {
            let bodies = load_stream(conn, previous, &stream)?;
            catalog.streams.remove(&stream.digest);
            for (identity, body) in bodies {
                if catalog.contents.get(&identity) == Some(&stream.digest) {
                    catalog.contents.remove(&identity);
                    enqueue_content(
                        conn,
                        body,
                        &mut batch,
                        &mut size,
                        &mut catalog,
                        spool,
                        &known,
                    )?;
                }
            }
        }
    }
    flush_contents(conn, &mut batch, &mut catalog, spool, &known)?;
    catalog.validate()?;
    let catalog_rows = catalog.rows();
    let mut buckets: BTreeMap<u8, Vec<(String, Row)>> = BTreeMap::new();
    for (key, row) in catalog_rows {
        let hash = digest(key.as_bytes());
        let bucket = u8::from_str_radix(&hash[..2], 16).map_err(|_| invalid("目录桶无效"))? % 64;
        buckets.entry(bucket).or_default().push((key, row));
    }
    let mut pages = Vec::new();
    for records in buckets.into_values() {
        let mut page = BTreeMap::new();
        let mut size = 2;
        for (key, row) in records {
            let len = serde_json::to_vec(&(&key, &row))?.len() + 2;
            if len > CHUNK {
                return Err(invalid("单条正文目录超过块预算"));
            }
            if size + len > CHUNK && !page.is_empty() {
                pages.push(spool.put(conn, &serde_json::to_vec(&page)?, &known)?);
                page.clear();
                size = 2;
            }
            size += len;
            page.insert(key, row);
        }
        if !page.is_empty() {
            pages.push(spool.put(conn, &serde_json::to_vec(&page)?, &known)?);
        }
    }
    Ok(Prepared {
        shards: pages,
        hold_ids,
    })
}

pub fn seal(conn: &Connection, generation: i64) -> AppResult<Option<Pending>> {
    let actor = database::actor(conn)?;
    {
        let tx = crate::db::write_transaction(conn)?;
        database::require_generation(&tx, generation)?;
        tx.execute(
            "DELETE FROM local_sync_object_pins WHERE actor=?1 AND owner GLOB 'body-build:*'",
            [&actor],
        )?;
        tx.commit()?;
    }
    if let Some(pending) = storage::pending(conn, "body", generation)? {
        return Ok(Some(pending));
    }
    let build_root = storage::BuildRoot::new(conn, "body-build")?;
    let owner = build_root.owner.clone();
    let mut spool = Spool::new(conn)?;
    let tx = conn.unchecked_transaction()?;
    database::require_generation(&tx, generation)?;
    let previous = storage::previous_manifest(&tx, &actor, "body")?;
    let (library, raw, clock): (String, String, u64) = tx.query_row(
        "SELECT library_id,frontier,lamport FROM local_sync_control WHERE id=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let covered = serde_json::from_str(&raw)?;
    // covered and completed are evidence from the same immutable read snapshot.
    // Taking completed later at seal can otherwise exceed this covered frontier.
    let completed = super::logical::completed(&tx)?;
    let inputs = storage::body_inputs_digest(&tx, &actor, &covered)?;
    if previous.is_some()
        && crate::db::get_setting(&tx, "local_sync_body_inputs")?.as_deref() == Some(&inputs)
    {
        tx.commit()?;
        return Ok(None);
    }
    let prepared = prepare(&tx, previous.as_ref(), &mut spool)?;
    tx.commit()?;
    if previous
        .as_ref()
        .is_some_and(|m| m.shards == prepared.shards)
        && prepared.hold_ids.is_empty()
    {
        database::require_generation(conn, generation)?;
        crate::db::set_setting(conn, "local_sync_body_inputs", &inputs)?;
        return Ok(None);
    }
    let Prepared { shards, hold_ids } = prepared;
    let result = (|| {
        spool.persist(conn, generation, &actor, &owner, "body")?;
        let manifest = {
            let tx = crate::db::write_transaction(conn)?;
            database::require_generation(&tx, generation)?;
            let publication: u64 = tx.query_row(
                "SELECT publication+1 FROM local_sync_control WHERE id=1",
                [],
                |r| r.get(0),
            )?;
            let manifest = Manifest {
                format: FORMAT,
                library_id: library,
                actor: actor.clone(),
                publication,
                kind: "body".into(),
                covered,
                completed,
                max_lamport: clock,
                shards,
                deltas: 0,
            };
            manifest.validate()?;
            tx.execute(
                "UPDATE local_sync_control SET publication=?1 WHERE id=1",
                [publication],
            )?;
            tx.commit()?;
            manifest
        };
        let identity = manifest.file_name()?;
        for batch in hold_ids.chunks(128) {
            let tx = crate::db::write_transaction(conn)?;
            database::require_generation(&tx, generation)?;
            let batch = serde_json::to_string(batch)?;
            tx.execute(
                "INSERT INTO local_sync_body_receipt_holds(manifest,hold_id)
                 SELECT ?1,CAST(value AS INTEGER) FROM json_each(?2)",
                params![&identity, batch],
            )?;
            tx.commit()?;
            std::thread::yield_now();
        }
        let tx = crate::db::write_transaction(conn)?;
        database::require_generation(&tx, generation)?;
        record_links(&tx, &manifest)?;
        let publication = manifest.publication;
        let pending = Pending {
            manifest,
            next_object: 0,
            ready_dirs: BTreeSet::new(),
            superseded_attempts: Vec::new(),
            body_inputs: Some(inputs.clone()),
            baseline: None,
        };
        tx.execute("INSERT INTO local_sync_pending(lane,generation,publication,payload) VALUES('body',?1,?2,?3)",params![generation,publication,serde_json::to_string(&pending)?])?;
        tx.execute(
            "DELETE FROM local_sync_object_pins WHERE owner=?1",
            [&owner],
        )?;
        tx.commit()?;
        Ok(Some(pending))
    })();
    if result.is_err() {
        let cleanup = (|| -> AppResult<()> {
            let _writer = crate::db::writer_guard(conn)?;
            conn.execute("DELETE FROM local_sync_object_pins WHERE owner=?1", [owner])?;
            Ok(())
        })();
        if let Err(error) = cleanup {
            log::warn!("body publication pins retained after cleanup failure: {error}");
        }
    }
    result
}

#[derive(Default)]
pub(super) struct Release {
    sources: BTreeMap<String, (Vec<String>, Projection)>,
}
fn register(conn: &Connection, key: &str) -> AppResult<Option<Register>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT register FROM local_sync_winners WHERE key=?1",
            [key],
            |r| r.get(0),
        )
        .optional()?;
    raw.map(|v| serde_json::from_str(&v).map_err(Into::into))
        .transpose()
}
impl Release {
    pub(super) fn can_release(
        &mut self,
        conn: &Connection,
        peers: &[Frontier],
        entry: &Entry,
    ) -> AppResult<bool> {
        let pending:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM local_sync_article_locator WHERE managed=1 AND valid=0) OR EXISTS(SELECT 1 FROM local_sync_intents) OR EXISTS(SELECT 1 FROM local_sync_body_hold) OR EXISTS(SELECT 1 FROM local_sync_work WHERE job='') OR (SELECT accepted_job IS NOT NULL FROM local_sync_control WHERE id=1)",[],|r|r.get(0))?;
        if pending {
            return Ok(false);
        }
        let Some(meta) = register(conn, &format!("a/{}/meta", entry.entry))? else {
            return Ok(false);
        };
        let FieldValue::Set(meta) = meta.value else {
            return Ok(false);
        };
        let meta: ArticleMeta = serde_json::from_value(meta)?;
        // Losing a display tie does not release the original member's current
        // protected bytes. Holds additionally retain displaced references until
        // their exact receipt; neither proof follows from display selection.
        let own_article = super::logical::entity(conn, &format!("a/{}", entry.entry))?;
        let own_source = super::logical::entity(conn, &format!("s/{}", meta.source))?;
        let own_prefix = format!("a/{}", entry.entry);
        if value(&own_article, &own_prefix, "body") == Some(&serde_json::to_value(&entry.body_ref)?)
            && (value(&own_article, &own_prefix, "star") == Some(&serde_json::Value::Bool(true))
                || (active_source(&own_source, &format!("s/{}", meta.source))
                    && value(&own_article, &own_prefix, "read")
                        != Some(&serde_json::Value::Bool(true))))
        {
            return Ok(false);
        }
        if !self.sources.contains_key(&meta.source) {
            let Some(url) = register(conn, &format!("s/{}/feed_url", meta.source))? else {
                return Ok(false);
            };
            let FieldValue::Set(url) = url.value else {
                return Ok(false);
            };
            let Some(url) = url.as_str() else {
                return Ok(false);
            };
            let mut stmt=conn.prepare("SELECT substr(key,3,32) FROM local_sync_winners WHERE key LIKE 's/%/feed_url' AND json_extract(register,'$.value.value')=?1")?;
            let aliases = stmt
                .query_map([url], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut fields = Projection::default();
            for alias in &aliases {
                for name in ["created", "deleted", "reactivate"] {
                    let key = format!("s/{alias}/{name}");
                    if let Some(reg) = register(conn, &key)? {
                        fields.select(format!("s/source/{name}"), &key, reg)?;
                    }
                }
            }
            self.sources.insert(meta.source.clone(), (aliases, fields));
        }
        let (aliases, source) = &self.sources[&meta.source];
        let mut fields = source.clone();
        for alias in aliases {
            let key = entry_key(alias, &meta.guid);
            for name in ["read", "star"] {
                let wire_key = format!("a/{key}/{name}");
                if let Some(reg) = register(conn, &wire_key)? {
                    fields.select(format!("a/entry/{name}"), &wire_key, reg)?;
                }
            }
        }
        let fields = fields.into_fields();
        let active = active_source(&fields, "s/source");
        if value(&fields, "a/entry", "star") == Some(&serde_json::Value::Bool(true))
            || (active && value(&fields, "a/entry", "read") != Some(&serde_json::Value::Bool(true)))
        {
            return Ok(false);
        }
        let Some(star) = fields.get("a/entry/star") else {
            return Ok(false);
        };
        let Some(release) = fields.get(if active {
            "a/entry/read"
        } else {
            "s/source/deleted"
        }) else {
            return Ok(false);
        };
        Ok(peers.iter().all(|p| {
            p.get(&release.stamp.actor).copied().unwrap_or(0) >= release.stamp.seq
                && p.get(&star.stamp.actor).copied().unwrap_or(0) >= star.stamp.seq
        }))
    }
}

pub fn retry_object(conn: &Connection, pending: &mut Pending, object: &ObjectRef) -> AppResult<()> {
    let _writer = crate::db::writer_guard(conn)?;
    let replacement = ObjectRef {
        name: format!("{}-{}.bin", object.digest, storage::random_id(conn)?),
        ..object.clone()
    };
    let original = pending.manifest.clone();
    let mut changed = false;
    for page in &mut pending.manifest.shards {
        if page == object {
            *page = replacement.clone();
            changed = true;
            continue;
        }
        let bytes = storage::cached_object(conn, &pending.manifest.actor, "body", page)?
            .ok_or_else(|| invalid("重试正文目录缺失"))?;
        let mut rows: BTreeMap<String, Row> = serde_json::from_slice(&bytes)?;
        let mut page_changed = false;
        for row in rows.values_mut() {
            if let Row::Stream(stream) = row {
                for part in &mut stream.parts {
                    if part == object {
                        *part = replacement.clone();
                        page_changed = true;
                    }
                }
            }
        }
        if page_changed {
            pending.superseded_attempts.push(original.object_path(page));
            let bytes = serde_json::to_vec(&rows)?;
            let hash = digest(&bytes);
            *page = ObjectRef {
                name: format!("{hash}-{}.bin", storage::random_id(conn)?),
                digest: hash.clone(),
                size: bytes.len(),
            };
            conn.execute("INSERT INTO local_sync_objects(actor,kind,digest,bytes) VALUES(?1,'body',?2,?3) ON CONFLICT(actor,kind,digest) DO NOTHING",params![pending.manifest.actor,hash,bytes])?;
            changed = true;
        }
    }
    if !changed {
        return Err(invalid("部分正文对象不属于封批目录"));
    }
    record_links(conn, &pending.manifest)?;
    pending.next_object = 0;
    Ok(())
}

pub fn apply(
    conn: &Connection,
    generation: i64,
    manifest: &Manifest,
    catalog: &Catalog,
) -> AppResult<usize> {
    let targets = database::body_targets(conn)?;
    type BodyTargetsByStream = BTreeMap<(String, Option<String>), Vec<(i64, BodyRef)>>;
    let mut groups: BodyTargetsByStream = BTreeMap::new();
    for (id, body) in targets {
        let Some(primary) = catalog.contents.get(&body.primary) else {
            continue;
        };
        let rss = match &body.rss {
            None => None,
            Some(identity) => {
                let Some(stream) = catalog.contents.get(identity) else {
                    continue;
                };
                Some(stream.clone())
            }
        };
        groups
            .entry((primary.clone(), rss))
            .or_default()
            .push((id, body));
    }
    let mut count = 0;
    for ((primary, rss), targets) in groups {
        let mut bodies = load_stream(conn, manifest, &catalog.streams[&primary])?;
        if let Some(rss) = rss.filter(|rss| rss != &primary) {
            bodies.extend(load_stream(conn, manifest, &catalog.streams[&rss])?);
        }
        for targets in targets.chunks(128) {
            count += database::apply_body_targets(conn, generation, targets, &bodies)?;
            std::thread::yield_now();
        }
    }
    Ok(count)
}

#[cfg(test)]
pub fn records(conn: &Connection, manifest: &Manifest) -> AppResult<Vec<BodyRecord>> {
    let catalog = catalog(conn, manifest)?;
    let mut bodies = BTreeMap::new();
    for stream in catalog.streams.values() {
        bodies.extend(load_stream(conn, manifest, stream)?);
    }
    catalog
        .entries
        .into_values()
        .map(|entry| {
            let primary = bodies
                .get(&entry.body_ref.primary)
                .cloned()
                .ok_or_else(|| invalid("正文缺失"))?;
            let rss = entry
                .body_ref
                .rss
                .as_ref()
                .map(|id| {
                    bodies
                        .get(id)
                        .cloned()
                        .ok_or_else(|| invalid("RSS正文缺失"))
                })
                .transpose()?;
            Ok(BodyRecord {
                entry: entry.entry,
                body_ref: entry.body_ref,
                primary,
                rss,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn body_archive_catalog_has_no_one_gib_payload_total_limit() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::MIGRATIONS.to_latest(&mut conn).unwrap();
        let mut catalog = Catalog::default();
        for n in 0..17 {
            let identity = format!("h|{:032x}|1|1", n);
            let stream_digest = digest(format!("stream-{n}").as_bytes());
            let entry = Entry {
                entry: digest(format!("entry-{n}").as_bytes()),
                body_ref: BodyRef {
                    primary: identity.clone(),
                    rss: None,
                },
            };
            catalog.entries.insert(entry_id(&entry).unwrap(), entry);
            catalog.contents.insert(identity, stream_digest.clone());
            let parts = (0..64)
                .map(|piece| {
                    let hash = digest(format!("piece-{n}-{piece}").as_bytes());
                    ObjectRef {
                        name: format!("{hash}-{}.bin", "a".repeat(32)),
                        digest: hash,
                        size: CHUNK,
                    }
                })
                .collect();
            catalog.streams.insert(
                stream_digest.clone(),
                Stream {
                    digest: stream_digest,
                    bytes: 64 * CHUNK,
                    items: 1,
                    parts,
                },
            );
        }
        let bytes = serde_json::to_vec(&catalog.rows()).unwrap();
        assert!(bytes.len() < CHUNK);
        let hash = digest(&bytes);
        let page = ObjectRef {
            name: format!("{hash}-{}.bin", "b".repeat(32)),
            digest: hash,
            size: bytes.len(),
        };
        let manifest = Manifest {
            format: FORMAT,
            library_id: "a".repeat(32),
            actor: "b".repeat(32),
            publication: 1,
            kind: "body".into(),
            covered: Frontier::new(),
            completed: Frontier::new(),
            max_lamport: 0,
            shards: vec![page.clone()],
            deltas: 0,
        };
        storage::cache_object(&conn, 0, &manifest, &page, &bytes).unwrap();
        let decoded = super::catalog(&conn, &manifest).unwrap();
        assert!(decoded.objects().iter().map(|o| o.size as u64).sum::<u64>() > 1024 * 1024 * 1024);
    }
    #[test]
    fn body_archive_owned_spool_is_removed_at_handle_close() {
        let conn = Connection::open_in_memory().unwrap();
        let spool = Spool::new(&conn).unwrap();
        let path = spool.path.clone();
        assert!(path.exists());
        drop(spool);
        assert!(!path.exists());
    }
}
