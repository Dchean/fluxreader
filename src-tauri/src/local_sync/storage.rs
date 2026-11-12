//! Bounded immutable publications. Content shards are reused across publications;
//! a manifest does not assert a global order or replace another actor's state.
use super::{database, model::*, transport_error};
use crate::{
    error::{AppError, AppResult},
    webdav_transport::{Checkpoint, Frontier, Register, Transport},
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const ROOT: &str = ".fluxreader-v3/";
const CHUNK: usize = 1024 * 1024;
const MAX_SHARDS: usize = 4096;
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const BODY_HOLD_RELEASE_BATCH: i64 = 128;
pub const REQUESTS_PER_ROUND: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ObjectRef {
    pub name: String,
    pub digest: String,
    pub size: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: u32,
    pub library_id: String,
    pub actor: String,
    pub publication: u64,
    pub kind: String,
    pub covered: Frontier,
    pub completed: Frontier,
    pub max_lamport: u64,
    pub shards: Vec<ObjectRef>,
    pub deltas: usize,
}
impl Manifest {
    pub fn file_name(&self) -> AppResult<String> {
        Ok(format!(
            "{:020}-{}.json",
            self.publication,
            digest(&serde_json::to_vec(self)?)
        ))
    }
    pub fn validate(&self) -> AppResult<()> {
        if self.format != FORMAT
            || !valid_hex(&self.library_id, 32)
            || !valid_hex(&self.actor, 32)
            || self.publication == 0
            || !matches!(self.kind.as_str(), "state" | "body")
            || self.shards.len() > MAX_SHARDS
            || self.deltas > 32
            || self.completed.iter().any(|(actor, seq)| {
                *seq == 0 || *seq > self.covered.get(actor).copied().unwrap_or(0)
            })
        {
            return Err(invalid("库清单版本、身份或分片数不受支持"));
        }
        let mut names = BTreeSet::new();
        for shard in &self.shards {
            validate_object(shard)?;
            if !names.insert(&shard.name) {
                return Err(invalid("目录分片重复引用"));
            }
        }
        if serde_json::to_vec(self)?.len() > MAX_MANIFEST_BYTES {
            return Err(AppError::new(
                "localSyncCapacity",
                "发布目录超过单次解析预算；本地数据保留待同步",
            ));
        }
        // Reuse the audited clock/actor validation even before field pages arrive.
        Checkpoint {
            format: 2,
            library_id: self.library_id.clone(),
            covered: self.covered.clone(),
            max_lamport: self.max_lamport,
            fields: BTreeMap::new(),
        }
        .validate()
        .map_err(transport_error)?;
        Ok(())
    }
    pub fn object_path(&self, object: &ObjectRef) -> String {
        format!(
            "{ROOT}actors/{}/{}/objects/{}/{}",
            self.actor,
            self.kind,
            bucket(&object.digest),
            object.name
        )
    }
    pub fn manifest_dir(&self) -> String {
        format!("{ROOT}actors/{}/{}/manifests/", self.actor, self.kind)
    }
    pub fn manifest_path(&self) -> AppResult<String> {
        Ok(format!("{}{}", self.manifest_dir(), self.file_name()?))
    }
}
pub(super) fn validate_object(object: &ObjectRef) -> AppResult<()> {
    let valid = object
        .name
        .strip_suffix(".bin")
        .and_then(|s| s.split_once('-'))
        .is_some_and(|(hash, attempt)| hash == object.digest && valid_hex(attempt, 32));
    if !valid || !valid_hex(&object.digest, 64) || object.size == 0 || object.size > CHUNK {
        return Err(invalid("库对象名称或长度无效"));
    }
    Ok(())
}
fn bucket(hash: &str) -> String {
    format!(
        "{:02x}",
        u8::from_str_radix(&hash[..2], 16).unwrap_or(0) % 64
    )
}
pub fn random_id(conn: &Connection) -> AppResult<String> {
    Ok(conn.query_row("SELECT lower(hex(randomblob(16)))", [], |r| r.get(0))?)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pending {
    pub manifest: Manifest,
    pub next_object: usize,
    pub ready_dirs: BTreeSet<String>,
    pub superseded_attempts: Vec<String>,
    pub body_inputs: Option<String>,
    pub baseline: Option<String>,
}

pub fn previous_manifest(
    conn: &Connection,
    actor: &str,
    kind: &str,
) -> AppResult<Option<Manifest>> {
    let key = format!("local_sync_own_{kind}");
    let raw: Option<String> = conn
        .query_row("SELECT value FROM settings WHERE key=?1", [key], |r| {
            r.get(0)
        })
        .optional()?;
    let previous = raw
        .map(|s| serde_json::from_str::<Manifest>(&s))
        .transpose()?;
    Ok(previous.filter(|m| m.actor == actor))
}

/// Pins the exact SQL read/build lifetime before its snapshot is opened. All
/// destructive cleanup honors these roots; an interrupted process may leave a
/// conservative root, which startup recovery can retire before new work starts.
pub(super) struct BuildRoot<'a> {
    conn: &'a Connection,
    pub owner: String,
}
impl<'a> BuildRoot<'a> {
    pub fn new(conn: &'a Connection, kind: &str) -> AppResult<Self> {
        let owner = format!("{kind}:{}", random_id(conn)?);
        let tx = crate::db::write_transaction(conn)?;
        tx.execute("INSERT INTO local_sync_roots(owner,kind,reference) SELECT ?1,?2,COALESCE(selected_baseline,'') FROM local_sync_control WHERE id=1",params![owner,kind])?;
        tx.commit()?;
        Ok(Self { conn, owner })
    }
}
impl Drop for BuildRoot<'_> {
    fn drop(&mut self) {
        let result = (|| -> AppResult<()> {
            let tx = crate::db::write_transaction(self.conn)?;
            tx.execute(
                "DELETE FROM local_sync_object_pins WHERE owner=?1",
                [&self.owner],
            )?;
            tx.execute("DELETE FROM local_sync_roots WHERE owner=?1", [&self.owner])?;
            tx.commit()?;
            Ok(())
        })();
        if let Err(error) = result {
            log::warn!(
                "local sync build root retained after cleanup failure: {}",
                error.message
            );
        }
    }
}

pub fn seal_state(conn: &Connection, generation: i64) -> AppResult<Option<Pending>> {
    if let Some(pending) = pending(conn, "state", generation)? {
        return Ok(Some(pending));
    }
    database::require_generation(conn, generation)?;
    let build_root = BuildRoot::new(conn, "state-build")?;
    // TEMP rows are disk-backed staging for the exact read snapshot. Neither
    // page bytes nor a full baseline map are retained in process memory.
    conn.execute_batch("PRAGMA temp_store=FILE; CREATE TEMP TABLE IF NOT EXISTS local_sync_baseline_stage(key TEXT PRIMARY KEY,register TEXT NOT NULL); DELETE FROM local_sync_baseline_stage;")?;
    let mut spool = super::body_archive::Spool::new(conn)?;
    let tx = conn.unchecked_transaction()?;
    let actor = database::actor(&tx)?;
    let previous = previous_manifest(&tx, &actor, "state")?;
    let (library,raw,clock,selected,base):(String,String,u64,Option<String>,Option<String>)=tx.query_row("SELECT library_id,frontier,lamport,selected_baseline,selected_base FROM local_sync_control WHERE id=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?;
    let covered: Frontier = serde_json::from_str(&raw)?;
    let completed = super::logical::completed(&tx)?;
    if previous
        .as_ref()
        .is_some_and(|p| p.covered == covered && p.max_lamport == clock && p.completed == completed)
    {
        tx.commit()?;
        return Ok(None);
    }
    let known: BTreeMap<String, ObjectRef> = previous
        .as_ref()
        .map(|m| {
            m.shards
                .iter()
                .map(|s| (s.digest.clone(), s.clone()))
                .collect()
        })
        .unwrap_or_default();
    let receipt_only = previous
        .as_ref()
        .is_some_and(|p| p.covered == covered && p.max_lamport == clock)
        && selected.is_some();
    let mut shards = Vec::new();
    let mut delta = false;
    if !receipt_only && selected.is_some() && previous.as_ref().is_some_and(|p| p.deltas < 32) {
        let mut fields = BTreeMap::<String, Register>::new();
        let mut size = 2;
        let mut stmt=tx.prepare("SELECT f.key,f.register FROM local_sync_winners f LEFT JOIN local_sync_selected_fields p ON p.key=f.key WHERE p.register IS NULL OR p.register!=f.register ORDER BY f.key")?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (key, raw) = row?;
            size += key.len() + raw.len() + 8;
            if size > 64 * 1024 {
                break;
            }
            fields.insert(key, serde_json::from_str(&raw)?);
        }
        if size <= 64 * 1024 {
            delta = true;
            if !fields.is_empty() {
                shards.push(spool.put(&tx, &serde_json::to_vec(&fields)?, &known)?);
                for (key, reg) in fields {
                    tx.execute(
                        "INSERT INTO local_sync_baseline_stage(key,register) VALUES(?1,?2)",
                        params![key, serde_json::to_string(&reg)?],
                    )?;
                }
            }
        }
    }
    if !receipt_only && !delta {
        let mut stmt=tx.prepare("SELECT key,register,flx_bucket(key) FROM local_sync_winners ORDER BY flx_bucket(key),key")?;
        let mut fields = BTreeMap::<String, Register>::new();
        let mut size = 2;
        let mut prior_bucket = -1;
        for row in stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })? {
            let (key, raw, bucket) = row?;
            let length = key.len() + raw.len() + 8;
            if length > CHUNK {
                return Err(invalid("单字段超过状态页预算"));
            }
            if !fields.is_empty() && (bucket != prior_bucket || size + length > CHUNK) {
                shards.push(spool.put(&tx, &serde_json::to_vec(&fields)?, &known)?);
                fields.clear();
                size = 2;
            }
            prior_bucket = bucket;
            size += length;
            fields.insert(key.clone(), serde_json::from_str(&raw)?);
            tx.execute(
                "INSERT INTO local_sync_baseline_stage(key,register) VALUES(?1,?2)",
                params![key, raw],
            )?;
        }
        if !fields.is_empty() {
            shards.push(spool.put(&tx, &serde_json::to_vec(&fields)?, &known)?);
        }
    }
    let deltas = if receipt_only {
        previous.as_ref().map_or(0, |p| p.deltas)
    } else if delta {
        previous.as_ref().map_or(0, |p| p.deltas) + usize::from(!shards.is_empty())
    } else {
        0
    };
    if receipt_only || delta {
        let mut all = previous
            .as_ref()
            .map(|p| p.shards.clone())
            .unwrap_or_default();
        all.append(&mut shards);
        shards = all;
    }
    tx.commit()?;
    let owner = build_root.owner.clone();
    let baseline = if receipt_only {
        selected.clone()
    } else {
        Some(random_id(conn)?)
    };
    // A build root bridges snapshot staging, cache persistence and final seal.
    {
        let tx = crate::db::write_transaction(conn)?;
        database::require_generation(&tx, generation)?;
        tx.execute(
            "UPDATE local_sync_roots SET reference=?2 WHERE owner=?1",
            params![owner, baseline],
        )?;
        if !receipt_only {
            let id = baseline.as_deref().ok_or_else(|| invalid("缺少基线版本"))?;
            tx.execute(
                "INSERT INTO local_sync_baselines(id,base,parent) VALUES(?1,?2,?3)",
                params![
                    id,
                    if delta {
                        base.as_deref().unwrap_or(id)
                    } else {
                        id
                    },
                    if delta { selected.as_deref() } else { None }
                ],
            )?;
        }
        tx.commit()?;
    }
    spool.persist(conn, generation, &actor, &owner, "state")?;
    if !receipt_only {
        let id = baseline.as_deref().ok_or_else(|| invalid("缺少基线版本"))?;
        let mut last = String::new();
        loop {
            let rows = {
                let mut stmt=conn.prepare("SELECT key,register FROM local_sync_baseline_stage WHERE key>?1 ORDER BY key LIMIT 128")?;
                let rows = stmt
                    .query_map([&last], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            if rows.is_empty() {
                break;
            }
            let tx = crate::db::write_transaction(conn)?;
            database::require_generation(&tx, generation)?;
            {
                let mut insert = tx.prepare(
                    "INSERT INTO local_sync_baseline_fields(version,key,register) VALUES(?1,?2,?3)",
                )?;
                for (key, raw) in &rows {
                    insert.execute(params![id, key, raw])?;
                    last = key.clone();
                }
            }
            tx.commit()?;
            std::thread::yield_now();
        }
    }
    let tx = crate::db::write_transaction(conn)?;
    database::require_generation(&tx, generation)?;
    if let Some(existing) = pending(&tx, "state", generation)? {
        tx.commit()?;
        return Ok(Some(existing));
    }
    let publication: u64 = tx.query_row(
        "SELECT publication+1 FROM local_sync_control WHERE id=1",
        [],
        |r| r.get(0),
    )?;
    let manifest = Manifest {
        format: FORMAT,
        library_id: library,
        actor,
        publication,
        kind: "state".into(),
        covered,
        completed,
        max_lamport: clock,
        shards,
        deltas,
    };
    manifest.validate()?;
    if let Some(id) = &baseline {
        tx.execute("UPDATE local_sync_baselines SET ready=1 WHERE id=?1", [id])?;
    }
    let pending = Pending {
        manifest,
        next_object: 0,
        ready_dirs: BTreeSet::new(),
        superseded_attempts: Vec::new(),
        body_inputs: None,
        baseline,
    };
    tx.execute(
        "UPDATE local_sync_control SET publication=?1 WHERE id=1",
        [publication],
    )?;
    tx.execute("INSERT INTO local_sync_pending(lane,generation,publication,payload) VALUES('state',?1,?2,?3)",params![generation,publication,serde_json::to_string(&pending)?])?;
    tx.execute("DELETE FROM local_sync_roots WHERE owner=?1", [&owner])?;
    tx.execute(
        "DELETE FROM local_sync_object_pins WHERE owner=?1",
        [&owner],
    )?;
    tx.commit()?;
    Ok(Some(pending))
}
pub fn seal_bodies(conn: &Connection, generation: i64) -> AppResult<Option<Pending>> {
    super::body_archive::seal(conn, generation)
}
pub(super) fn peer_frontiers(
    conn: &Connection,
    actor: &str,
    current: &Frontier,
) -> AppResult<Vec<Frontier>> {
    let actors = {
        let mut stmt = conn.prepare("SELECT actor FROM local_sync_known_actors ORDER BY actor")?;
        let actors = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        actors
    };
    let _ = current;
    let mut peers = vec![super::logical::completed(conn)?];
    for peer in actors.into_iter().filter(|peer| peer != actor) {
        let manifest = crate::db::get_setting(conn, &format!("local_sync_peer_state_{peer}"))?
            .map(|raw| serde_json::from_str::<Manifest>(&raw))
            .transpose()?;
        peers.push(manifest.map(|m| m.completed).unwrap_or_default());
    }
    Ok(peers)
}
pub(super) fn body_inputs_digest(
    conn: &Connection,
    actor: &str,
    covered: &Frontier,
) -> AppResult<String> {
    let revision: i64 = conn.query_row(
        "SELECT data_revision FROM local_sync_control WHERE id=1",
        [],
        |r| r.get(0),
    )?;
    let mut value =
        serde_json::to_vec(&(covered, revision, peer_frontiers(conn, actor, covered)?))?;
    let mut stmt = conn
        .prepare("SELECT id,entry,body_ref FROM local_sync_body_hold ORDER BY id,entry,body_ref")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        value.extend(row.get::<_, i64>(0)?.to_be_bytes());
        for part in [row.get::<_, String>(1)?, row.get::<_, String>(2)?] {
            value.extend((part.len() as u64).to_be_bytes());
            value.extend(part.as_bytes());
        }
    }
    let mut hasher = Sha256::new();
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
    Ok(format!("{:x}", hasher.finalize()))
}
pub fn pending(conn: &Connection, lane: &str, generation: i64) -> AppResult<Option<Pending>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT payload FROM local_sync_pending WHERE lane=?1 AND generation=?2",
            params![lane, generation],
            |r| r.get(0),
        )
        .optional()?;
    raw.map(|v| serde_json::from_str(&v).map_err(Into::into))
        .transpose()
}
pub fn save_pending(conn: &Connection, generation: i64, pending: &Pending) -> AppResult<()> {
    let _writer = crate::db::writer_guard(conn)?;
    database::require_generation(conn, generation)?;
    let n=conn.execute("UPDATE local_sync_pending SET payload=?1 WHERE lane=?2 AND generation=?3 AND publication=?4",params![serde_json::to_string(pending)?,pending.manifest.kind,generation,pending.manifest.publication])?;
    if n != 1 {
        return Err(AppError::new("staleLocalSync", "待发布批次已被新连接取代"));
    }
    Ok(())
}

/// A partially written immutable object cannot be overwritten. Retarget only its
/// transport attempt/publication; captured field stamps/frontier and bytes stay
/// fixed. Delayed old attempts are harmless older publications, never new intents.
pub fn retry_partial(
    conn: &Connection,
    generation: i64,
    pending: &mut Pending,
    object: Option<ObjectRef>,
) -> AppResult<()> {
    if pending.superseded_attempts.len() >= 128 {
        return Err(AppError::new(
            "localSyncCapacity",
            "连续传输损坏达到重试预算；原操作与正文保留，修复云端存储后可继续",
        ));
    }
    let tx = crate::db::write_transaction(conn)?;
    database::require_generation(&tx, generation)?;
    let old = pending.manifest.publication;
    pending
        .superseded_attempts
        .push(pending.manifest.manifest_path()?);
    if let Some(object) = object {
        pending
            .superseded_attempts
            .push(pending.manifest.object_path(&object));
        if pending.manifest.kind == "body" {
            super::body_archive::retry_object(&tx, pending, &object)?;
        } else {
            let index = pending
                .manifest
                .shards
                .iter()
                .position(|old| old == &object)
                .ok_or_else(|| invalid("部分对象不属于封批"))?;
            let target = &mut pending.manifest.shards[index];
            target.name = format!("{}-{}.bin", target.digest, random_id(&tx)?);
            pending.next_object = index;
        }
    }
    let publication: u64 = tx.query_row(
        "SELECT publication+1 FROM local_sync_control WHERE id=1",
        [],
        |r| r.get(0),
    )?;
    pending.manifest.publication = publication;
    pending.manifest.validate()?;
    let changed=tx.execute("UPDATE local_sync_pending SET publication=?1,payload=?2 WHERE lane=?3 AND generation=?4 AND publication=?5",params![publication,serde_json::to_string(pending)?,pending.manifest.kind,generation,old])?;
    if changed != 1 {
        return Err(AppError::new(
            "staleLocalSync",
            "部分发布重试已被新会话取代",
        ));
    }
    tx.execute(
        "UPDATE local_sync_control SET publication=?1 WHERE id=1",
        [publication],
    )?;
    // Retried bytes keep their exact operation stamps. Obsolete immutable
    // attempts are queued now but deletion remains blocked by pending roots.
    for path in &pending.superseded_attempts {
        tx.execute(
            "INSERT OR IGNORE INTO local_sync_gc(path,generation) VALUES(?1,?2)",
            params![path, generation],
        )?;
    }
    tx.commit()?;
    Ok(())
}
pub fn cached_object(
    conn: &Connection,
    actor: &str,
    kind: &str,
    object: &ObjectRef,
) -> AppResult<Option<Vec<u8>>> {
    let bytes: Option<Vec<u8>> = conn
        .query_row(
            "SELECT bytes FROM local_sync_objects WHERE actor=?1 AND kind=?2 AND digest=?3",
            params![actor, kind, object.digest],
            |r| r.get(0),
        )
        .optional()?;
    if bytes
        .as_ref()
        .is_some_and(|bytes| bytes.len() != object.size || digest(bytes) != object.digest)
    {
        return Err(invalid("本机分片缓存校验失败"));
    }
    Ok(bytes)
}
pub fn cache_object(
    conn: &Connection,
    generation: i64,
    manifest: &Manifest,
    object: &ObjectRef,
    bytes: &[u8],
) -> AppResult<()> {
    let _writer = crate::db::writer_guard(conn)?;
    database::require_generation(conn, generation)?;
    if bytes.len() != object.size || digest(bytes) != object.digest {
        return Err(invalid("远端分片哈希或长度不匹配；保留本机状态"));
    }
    conn.execute("INSERT INTO local_sync_objects(actor,kind,digest,bytes) VALUES(?1,?2,?3,?4) ON CONFLICT(actor,kind,digest) DO NOTHING",params![manifest.actor,manifest.kind,object.digest,bytes])?;
    Ok(())
}
pub fn reassemble(conn: &Connection, manifest: &Manifest) -> AppResult<Checkpoint> {
    manifest.validate()?;
    let mut state = Checkpoint {
        format: 2,
        library_id: manifest.library_id.clone(),
        covered: manifest.covered.clone(),
        max_lamport: manifest.max_lamport,
        fields: BTreeMap::new(),
    };
    for object in &manifest.shards {
        let bytes = cached_object(conn, &manifest.actor, &manifest.kind, object)?
            .ok_or_else(|| AppError::new("localSyncPending", "库分片仍在下载"))?;
        let part: BTreeMap<String, Register> = serde_json::from_slice(&bytes)?;
        state.join_fields(part).map_err(transport_error)?;
    }
    validate_checkpoint(&state)?;
    Ok(state)
}
pub fn small_delta(
    conn: &Connection,
    known: &Manifest,
    new: &Manifest,
) -> AppResult<Option<Checkpoint>> {
    if new.deltas == 0
        || new.deltas <= known.deltas
        || known.library_id != new.library_id
        || known.actor != new.actor
        || !known
            .shards
            .iter()
            .all(|object| new.shards.contains(object))
        || known
            .covered
            .iter()
            .any(|(actor, seq)| new.covered.get(actor).copied().unwrap_or(0) < *seq)
    {
        return Ok(None);
    }
    let mut state = Checkpoint {
        format: 2,
        library_id: new.library_id.clone(),
        covered: new.covered.clone(),
        max_lamport: new.max_lamport,
        fields: BTreeMap::new(),
    };
    for object in new
        .shards
        .iter()
        .filter(|object| !known.shards.contains(object))
    {
        let bytes = cached_object(conn, &new.actor, "state", object)?
            .ok_or_else(|| invalid("已验证增量分片缺失"))?;
        state
            .join_fields(serde_json::from_slice(&bytes)?)
            .map_err(transport_error)?;
    }
    validate_checkpoint(&state)?;
    Ok(Some(state))
}
/// Root catalog pages plus the content streams they name; payload bytes are not loaded.
pub fn publication_objects(conn: &Connection, manifest: &Manifest) -> AppResult<Vec<ObjectRef>> {
    if manifest.kind == "state" {
        return Ok(manifest.shards.clone());
    }
    let mut objects = super::body_archive::catalog(conn, manifest)?.objects();
    objects.extend(manifest.shards.iter().cloned());
    Ok(objects)
}

pub fn acknowledge(conn: &Connection, generation: i64, manifest: &Manifest) -> AppResult<()> {
    database::require_generation(conn, generation)?;
    let identity = manifest.file_name()?;
    let already_acknowledged = if manifest.kind == "body" {
        let already = body_ack_already_applied(conn, generation, manifest)?;
        // Prepare exact immutable body-reference evidence page by page. Only
        // selecting this manifest in the final ACK activates these rows.
        if !already {
            for page in &manifest.shards {
                let bytes = cached_object(conn, &manifest.actor, "body", page)?
                    .ok_or_else(|| invalid("正文ACK目录页缺失"))?;
                let records: BTreeMap<String, serde_json::Value> = serde_json::from_slice(&bytes)?;
                let refs = records
                    .values()
                    .filter(|row| row["kind"] == "entry")
                    .map(|row| {
                        let entry = row["entry"]
                            .as_str()
                            .ok_or_else(|| invalid("正文ACK缺少条目身份"))?
                            .to_owned();
                        let body = serde_json::from_value::<BodyRef>(row["body_ref"].clone())?;
                        Ok((entry, serde_json::to_string(&body)?))
                    })
                    .collect::<AppResult<Vec<_>>>()?;
                for batch in refs.chunks(128) {
                    let tx = crate::db::write_transaction(conn)?;
                    database::require_generation(&tx, generation)?;
                    {
                        let mut insert=tx.prepare("INSERT OR IGNORE INTO local_sync_body_receipts(manifest,entry,body_ref) VALUES(?1,?2,?3)")?;
                        for (entry, body) in batch {
                            insert.execute(params![identity, entry, body])?;
                        }
                    }
                    tx.commit()?;
                    std::thread::yield_now();
                }
            }
        }
        Some(already)
    } else {
        None
    };
    if already_acknowledged == Some(true) {
        release_body_holds(conn, generation, &identity)?;
        return Ok(());
    }
    let tx = crate::db::write_transaction(conn)?;
    database::require_generation(&tx, generation)?;
    let pending = match pending(&tx, &manifest.kind, generation)? {
        Some(pending) if pending.manifest.file_name()? == manifest.file_name()? => pending,
        None if previous_manifest(&tx, &manifest.actor, &manifest.kind)?
            .is_some_and(|own| own.file_name().ok() == manifest.file_name().ok()) =>
        {
            tx.commit()?;
            if manifest.kind == "body" {
                release_body_holds(conn, generation, &identity)?;
            }
            return Ok(());
        }
        _ => return Err(AppError::new("staleLocalSync", "回执不属于当前精确封批")),
    };
    if let Some(inputs) = &pending.body_inputs {
        db_set(&tx, "local_sync_body_inputs", inputs)?;
    }
    let key = format!("local_sync_own_{}", manifest.kind);
    db_set(&tx, &key, &serde_json::to_string(manifest)?)?;
    tx.execute(
        "DELETE FROM local_sync_pending WHERE lane=?1 AND generation=?2 AND publication=?3",
        params![manifest.kind, generation, manifest.publication],
    )?;
    if manifest.kind == "state" {
        let baseline = pending
            .baseline
            .as_deref()
            .ok_or_else(|| invalid("ACK缺少精确封批基线"))?;
        let (base, ready): (String, bool) = tx.query_row(
            "SELECT base,ready FROM local_sync_baselines WHERE id=?1",
            [baseline],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if !ready {
            return Err(invalid("ACK基线尚未完整持久化"));
        }
        tx.execute(
            "UPDATE local_sync_control SET selected_baseline=?1,selected_base=?2 WHERE id=1",
            params![baseline, base],
        )?;
        let seq = manifest.covered.get(&manifest.actor).copied().unwrap_or(0);
        // Ack only the captured local sequence, never later edits which arrived during HTTP.
        tx.execute("UPDATE local_sync_control SET published_seq=MAX(published_seq,?1),last_state_sync=?2 WHERE id=1",params![seq,chrono::Utc::now().timestamp()])?;
        // Physical deletion is a separate root-aware cleanup slice. ACK does
        // not delete validation-tail, pending intent or accepted-candidate roots.
    } else {
        tx.execute(
            "UPDATE local_sync_control SET last_body_sync=?1 WHERE id=1",
            [chrono::Utc::now().timestamp()],
        )?;
    }
    tx.commit()?;
    if manifest.kind == "body" {
        release_body_holds(conn, generation, &identity)?;
    }
    Ok(())
}

fn body_ack_already_applied(
    conn: &Connection,
    generation: i64,
    manifest: &Manifest,
) -> AppResult<bool> {
    let identity = manifest.file_name()?;
    if let Some(pending) = pending(conn, "body", generation)? {
        if pending.manifest.file_name()? == identity {
            return Ok(false);
        }
        return Err(AppError::new("staleLocalSync", "回执不属于当前精确封批"));
    }
    let own = previous_manifest(conn, &manifest.actor, "body")?
        .map(|own| own.file_name())
        .transpose()?;
    if own.as_deref() == Some(identity.as_str()) {
        Ok(true)
    } else {
        Err(AppError::new("staleLocalSync", "回执不属于当前精确封批"))
    }
}

fn release_body_holds(conn: &Connection, generation: i64, manifest: &str) -> AppResult<()> {
    loop {
        let tx = crate::db::write_transaction(conn)?;
        database::require_generation(&tx, generation)?;
        let hold_ids = {
            let mut stmt = tx.prepare(
                "SELECT hold_id FROM local_sync_body_receipt_holds
                 WHERE manifest=?1 ORDER BY hold_id LIMIT ?2",
            )?;
            let rows =
                stmt.query_map(params![manifest, BODY_HOLD_RELEASE_BATCH], |row| row.get(0))?;
            rows.collect::<rusqlite::Result<Vec<i64>>>()?
        };
        if hold_ids.is_empty() {
            tx.commit()?;
            break;
        }
        let hold_ids = serde_json::to_string(&hold_ids)?;
        // Remove receipt authority first so deleting the parent hold has no
        // cascade work to perform inside this bounded write transaction.
        tx.execute(
            "DELETE FROM local_sync_body_receipt_holds
             WHERE manifest=?1 AND hold_id IN(
                 SELECT CAST(value AS INTEGER) FROM json_each(?2)
             )",
            params![manifest, &hold_ids],
        )?;
        tx.execute(
            "DELETE FROM local_sync_body_hold
             WHERE id IN(SELECT CAST(value AS INTEGER) FROM json_each(?1))",
            [&hold_ids],
        )?;
        tx.commit()?;
        std::thread::yield_now();
    }
    Ok(())
}
fn db_set(conn: &Connection, key: &str, value: &str) -> AppResult<()> {
    crate::db::set_setting(conn, key, value)
}

fn selected_dominates_intent(
    conn: &Connection,
    manifest: &Manifest,
    actor: &str,
    intent: &crate::webdav_transport::Intent,
) -> AppResult<bool> {
    if manifest.covered.get(actor).copied().unwrap_or(0) < intent.seq
        || intent
            .observed
            .iter()
            .any(|(peer, seq)| manifest.covered.get(peer).copied().unwrap_or(0) < *seq)
    {
        return Ok(false);
    }
    let stamp = crate::webdav_transport::Stamp {
        actor: actor.to_owned(),
        seq: intent.seq,
        lamport: intent.lamport,
    };
    for (key, value) in &intent.patch {
        let raw: Option<String> = conn
            .query_row(
                "SELECT register FROM local_sync_selected_fields WHERE key=?1",
                [key],
                |r| r.get(0),
            )
            .optional()?;
        let Some(raw) = raw else { return Ok(false) };
        let winner: Register = serde_json::from_str(&raw)?;
        if winner.stamp < stamp || (winner.stamp == stamp && winner.value != *value) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Physical reclamation has no authority to advance logical state. Every slice
/// preserves validation/read/build roots, the active candidate, exact pending
/// bytes and later local operations, even after an earlier checkpoint ACK.
pub fn cleanup_slice(conn: &Connection) -> AppResult<bool> {
    let tx = crate::db::write_transaction(conn)?;
    let actor = database::actor(&tx)?;
    let mut changed = 0;
    let rooted:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM local_sync_roots) OR EXISTS(SELECT 1 FROM local_sync_object_pins) OR EXISTS(SELECT 1 FROM local_sync_pending) OR EXISTS(SELECT 1 FROM local_sync_fast_pending) OR EXISTS(SELECT 1 FROM local_sync_intents) OR (SELECT accepted_job IS NOT NULL FROM local_sync_control WHERE id=1)",[],|r|r.get(0))?;
    if rooted {
        tx.commit()?;
        return Ok(changed > 0);
    }
    let own_body = previous_manifest(&tx, &actor, "body")?
        .map(|body| body.file_name())
        .transpose()?
        .unwrap_or_default();
    // ACK commits its metadata and pending removal first; release_body_holds
    // then deletes the exact receipt-hold authority in separate bounded
    // transactions. A crash in that window leaves the mapping and its holds on
    // disk. The old cleanup collected only non-own mappings and never their
    // holds, so the own ACKed body publication stayed protected until the next
    // publication. Repair a bounded batch for manifests that are provably
    // finished: the own ACKed body manifest, or a manifest with committed ACK
    // receipts. Their bodies were already handed to peers by that ACK, so the
    // leftover hold is stale. A mapping without ACK evidence is only detached
    // (a build may re-bind the same hold), and a shared hold that another
    // still-present mapping references (late re-pin) is never deleted.
    let finished: Vec<(i64, i64)> = {
        let mut stmt = tx.prepare(
            "SELECT h.rowid,h.hold_id FROM local_sync_body_receipt_holds h
             WHERE h.manifest=?1
                OR EXISTS(SELECT 1 FROM local_sync_body_receipts r WHERE r.manifest=h.manifest)
             ORDER BY h.manifest,h.hold_id LIMIT 128",
        )?;
        let rows = stmt.query_map([&own_body], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    if !finished.is_empty() {
        let rowids =
            serde_json::to_string(&finished.iter().map(|(rowid, _)| *rowid).collect::<Vec<_>>())?;
        let holds =
            serde_json::to_string(&finished.iter().map(|(_, hold)| *hold).collect::<Vec<_>>())?;
        changed += tx.execute(
            "DELETE FROM local_sync_body_receipt_holds
             WHERE rowid IN(SELECT CAST(value AS INTEGER) FROM json_each(?1))",
            [&rowids],
        )?;
        changed += tx.execute(
            "DELETE FROM local_sync_body_hold
             WHERE id IN(SELECT CAST(value AS INTEGER) FROM json_each(?1))
               AND NOT EXISTS(SELECT 1 FROM local_sync_body_receipt_holds r WHERE r.hold_id=local_sync_body_hold.id)",
            [&holds],
        )?;
    }
    changed+=tx.execute("DELETE FROM local_sync_body_receipt_holds WHERE rowid IN(SELECT h.rowid FROM local_sync_body_receipt_holds h WHERE h.manifest!=?1 AND NOT EXISTS(SELECT 1 FROM local_sync_body_receipts r WHERE r.manifest=h.manifest) AND NOT EXISTS(SELECT 1 FROM local_sync_pending WHERE lane='body') ORDER BY h.manifest,h.hold_id LIMIT 128)",[&own_body])?;
    changed+=tx.execute("DELETE FROM local_sync_body_receipts WHERE rowid IN(SELECT r.rowid FROM local_sync_body_receipts r WHERE r.manifest!=?1 AND NOT EXISTS(SELECT 1 FROM local_sync_body_receipt_holds h WHERE h.manifest=r.manifest) AND NOT EXISTS(SELECT 1 FROM local_sync_pending WHERE lane='body') ORDER BY r.manifest LIMIT 128)",[&own_body])?;
    let own = previous_manifest(&tx, &actor, "state")?;
    if let Some(own) = &own {
        let journal = {
            let mut stmt=tx.prepare("SELECT seq,lamport,observed,patch FROM local_sync_journal WHERE seq<=(SELECT published_seq FROM local_sync_control WHERE id=1) ORDER BY seq LIMIT 64")?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, u64>(0)?,
                        r.get::<_, u64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        for (seq, lamport, observed, patch) in journal {
            let intent = crate::webdav_transport::Intent {
                seq,
                lamport,
                observed: serde_json::from_str(&observed)?,
                patch: serde_json::from_str(&patch)?,
            };
            if selected_dominates_intent(&tx, own, &actor, &intent)? {
                changed += tx.execute("DELETE FROM local_sync_journal WHERE seq=?1", [seq])?;
            }
        }
        let batch:Option<(String,Vec<u8>,i64)>=tx.query_row("SELECT path,bytes,generation FROM local_sync_fast_objects WHERE actor=?1 ORDER BY first_seq,path LIMIT 1",[&actor],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        if let Some((path, bytes, generation)) = batch {
            let batch: crate::webdav_transport::Batch = serde_json::from_slice(&bytes)?;
            batch.validate().map_err(transport_error)?;
            let mut safe = true;
            for intent in &batch.intents {
                safe &= selected_dominates_intent(&tx, own, &actor, intent)?;
            }
            if safe {
                // Exact checkpoint domination, not fast_stored_seq, grants the
                // remote object deletion. Changed connection generations retain
                // old endpoint paths locally rather than deleting another server.
                if generation == database::generation(&tx)? {
                    tx.execute(
                        "INSERT OR IGNORE INTO local_sync_gc(path,generation) VALUES(?1,?2)",
                        params![path, generation],
                    )?;
                }
                changed +=
                    tx.execute("DELETE FROM local_sync_fast_objects WHERE path=?1", [path])?;
            }
        }
    }
    changed+=tx.execute("DELETE FROM local_sync_candidates WHERE rowid IN(SELECT c.rowid FROM local_sync_candidates c JOIN local_sync_receive r ON r.id=c.job WHERE r.phase IN('complete','rejected') ORDER BY c.job,c.key LIMIT 128)",[])?;
    tx.execute("DELETE FROM local_sync_work WHERE job IN(SELECT id FROM local_sync_receive WHERE phase IN('complete','rejected'))",[])?;
    // Keep tiny receive receipts to recognize repeated manifests; their bulky
    // candidate rows and cached pages have independent bounded lifetimes.
    tx.execute_batch("CREATE TEMP TABLE IF NOT EXISTS local_sync_baseline_roots(id TEXT PRIMARY KEY); DELETE FROM local_sync_baseline_roots;
      INSERT OR IGNORE INTO local_sync_baseline_roots WITH RECURSIVE chain(id,parent) AS (
      SELECT id,parent FROM local_sync_baselines WHERE id=(SELECT selected_baseline FROM local_sync_control WHERE id=1)
      UNION ALL SELECT b.id,b.parent FROM local_sync_baselines b JOIN chain c ON b.id=c.parent) SELECT id FROM chain;")?;
    changed+=tx.execute("DELETE FROM local_sync_baseline_fields WHERE rowid IN(SELECT f.rowid FROM local_sync_baseline_fields f WHERE NOT EXISTS(SELECT 1 FROM local_sync_baseline_roots r WHERE r.id=f.version) ORDER BY f.version,f.key LIMIT 128)",[])?;
    changed+=tx.execute("DELETE FROM local_sync_baselines WHERE id IN(SELECT b.id FROM local_sync_baselines b WHERE NOT EXISTS(SELECT 1 FROM local_sync_baseline_roots r WHERE r.id=b.id) AND NOT EXISTS(SELECT 1 FROM local_sync_baseline_fields f WHERE f.version=b.id) LIMIT 64)",[])?;
    tx.commit()?;
    Ok(changed > 0)
}

/// Recheck all durable references at the actual delete boundary, not merely
/// when an obsolete publication was first queued. Unknown obligations defer GC.
pub fn can_remove_path(conn: &Connection, generation: i64, path: &str) -> AppResult<bool> {
    database::require_generation(conn, generation)?;
    let actor = database::actor(conn)?;
    if !path.starts_with(&format!("{ROOT}actors/{actor}/")) {
        return Err(invalid("拒绝回收其他设备路径"));
    }
    let busy:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM local_sync_roots) OR EXISTS(SELECT 1 FROM local_sync_object_pins) OR EXISTS(SELECT 1 FROM local_sync_intents) OR EXISTS(SELECT 1 FROM local_sync_pending) OR EXISTS(SELECT 1 FROM local_sync_fast_pending) OR (SELECT accepted_job IS NOT NULL FROM local_sync_control WHERE id=1)",[],|r|r.get(0))?;
    if busy {
        return Ok(false);
    }
    let mut stmt=conn.prepare("SELECT value FROM settings WHERE key GLOB 'local_sync_own_*' OR key GLOB 'local_sync_peer_*' UNION ALL SELECT manifest FROM local_sync_downloads UNION ALL SELECT manifest FROM local_sync_receive WHERE phase NOT IN('complete','rejected')")?;
    for row in stmt.query_map([], |r| r.get::<_, String>(0))? {
        let manifest: Manifest = serde_json::from_str(&row?)?;
        if manifest.actor != actor {
            continue;
        }
        if manifest.manifest_path()? == path
            || publication_objects(conn, &manifest)?
                .iter()
                .any(|object| manifest.object_path(object) == path)
        {
            return Ok(false);
        }
    }
    if path.contains("/body/") {
        let hold:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM local_sync_body_hold) OR EXISTS(SELECT 1 FROM local_sync_work WHERE job='')",[],|r|r.get(0))?;
        if hold {
            return Ok(false);
        }
        let covered = super::logical::frontier(conn)?;
        let current = body_inputs_digest(conn, &actor, &covered)?;
        if crate::db::get_setting(conn, "local_sync_body_inputs")?.as_deref() != Some(&current) {
            return Ok(false);
        }
    }
    Ok(true)
}

pub fn evict_cache(conn: &Connection, generation: i64) -> AppResult<()> {
    let tx = crate::db::write_transaction(conn)?;
    database::require_generation(&tx, generation)?;
    let pinned:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM local_sync_roots WHERE kind IN ('state-build','body-build','snapshot'))",[],|r|r.get(0))?;
    if pinned {
        tx.commit()?;
        return Ok(());
    }
    tx.execute_batch("CREATE TEMP TABLE IF NOT EXISTS local_sync_cache_roots(actor TEXT,kind TEXT,digest TEXT,PRIMARY KEY(actor,kind,digest)); DELETE FROM local_sync_cache_roots;
      INSERT OR IGNORE INTO local_sync_cache_roots SELECT json_extract(s.value,'$.actor'),json_extract(s.value,'$.kind'),json_extract(j.value,'$.digest') FROM settings s,json_each(CASE WHEN json_valid(s.value) THEN s.value ELSE '{}' END,'$.shards') j WHERE s.key GLOB 'local_sync_own_*' OR s.key GLOB 'local_sync_peer_*';
      INSERT OR IGNORE INTO local_sync_cache_roots SELECT json_extract(p.payload,'$.manifest.actor'),p.lane,json_extract(j.value,'$.digest') FROM local_sync_pending p,json_each(p.payload,'$.manifest.shards') j;
      INSERT OR IGNORE INTO local_sync_cache_roots SELECT d.actor,d.kind,json_extract(j.value,'$.digest') FROM local_sync_downloads d,json_each(d.manifest,'$.shards') j;
      INSERT OR IGNORE INTO local_sync_cache_roots SELECT json_extract(d.manifest,'$.actor'),json_extract(d.manifest,'$.kind'),json_extract(j.value,'$.digest') FROM local_sync_receive d,json_each(d.manifest,'$.shards') j WHERE d.phase NOT IN('complete','rejected') OR d.id=(SELECT accepted_job FROM local_sync_control WHERE id=1) OR EXISTS(SELECT 1 FROM local_sync_roots r WHERE r.reference=d.id);
      DELETE FROM local_sync_objects WHERE rowid IN(SELECT o.rowid FROM local_sync_objects o WHERE NOT EXISTS(SELECT 1 FROM local_sync_cache_roots r WHERE r.actor=o.actor AND r.kind=o.kind AND r.digest=o.digest) AND NOT EXISTS(SELECT 1 FROM local_sync_object_links l JOIN local_sync_cache_roots r ON r.actor=l.actor AND r.kind=l.kind AND r.digest=l.parent WHERE l.actor=o.actor AND l.kind=o.kind AND l.child=o.digest) AND NOT EXISTS(SELECT 1 FROM local_sync_object_pins p WHERE p.actor=o.actor AND p.kind=o.kind AND p.digest=o.digest) LIMIT 256);
      DELETE FROM local_sync_object_links WHERE rowid IN(SELECT l.rowid FROM local_sync_object_links l WHERE NOT EXISTS(SELECT 1 FROM local_sync_objects o WHERE o.actor=l.actor AND o.kind=l.kind AND o.digest=l.parent) LIMIT 256);")?;
    tx.commit()?;
    Ok(())
}

/// Enqueue precise immutable paths only after the replacement was read back and
/// acknowledged. Ordinary deltas retain every prior page; compacted state needs
/// full domination. Body preparation retained every unproven release obligation.
fn state_dominates_streaming(conn: &Connection, new: &Manifest, old: &Manifest) -> AppResult<bool> {
    if old
        .covered
        .iter()
        .any(|(actor, seq)| new.covered.get(actor).copied().unwrap_or(0) < *seq)
        || new.max_lamport < old.max_lamport
    {
        return Ok(false);
    }
    conn.execute_batch("PRAGMA temp_store=FILE; CREATE TEMP TABLE IF NOT EXISTS local_sync_gc_state(key TEXT PRIMARY KEY,register TEXT NOT NULL); DELETE FROM local_sync_gc_state;")?;
    let tx = conn.unchecked_transaction()?;
    for object in &new.shards {
        let bytes = cached_object(&tx, &new.actor, "state", object)?
            .ok_or_else(|| invalid("新检查点缺少回收校验页"))?;
        let fields: BTreeMap<String, Register> = serde_json::from_slice(&bytes)?;
        for (key, reg) in fields {
            let old: Option<String> = tx
                .query_row(
                    "SELECT register FROM local_sync_gc_state WHERE key=?1",
                    [&key],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(old) = old {
                let old: Register = serde_json::from_str(&old)?;
                if old.stamp == reg.stamp && old.value != reg.value {
                    return Err(invalid("新检查点页间同戳冲突"));
                }
                if old.stamp >= reg.stamp {
                    continue;
                }
            }
            tx.execute("INSERT INTO local_sync_gc_state(key,register) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET register=excluded.register",params![key,serde_json::to_string(&reg)?])?;
        }
    }
    for object in &old.shards {
        let bytes = cached_object(&tx, &old.actor, "state", object)?
            .ok_or_else(|| invalid("旧检查点缺少回收校验页"))?;
        let fields: BTreeMap<String, Register> = serde_json::from_slice(&bytes)?;
        for (key, reg) in fields {
            let raw: Option<String> = tx
                .query_row(
                    "SELECT register FROM local_sync_gc_state WHERE key=?1",
                    [key],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(raw) = raw else { return Ok(false) };
            let winner: Register = serde_json::from_str(&raw)?;
            if winner.stamp < reg.stamp || (winner.stamp == reg.stamp && winner.value != reg.value)
            {
                return Ok(false);
            }
        }
    }
    tx.commit()?;
    Ok(true)
}
pub fn enqueue_gc(
    conn: &Connection,
    generation: i64,
    old: &Manifest,
    new: &Manifest,
) -> AppResult<()> {
    database::require_generation(conn, generation)?;
    let _root = BuildRoot::new(conn, "snapshot")?;
    if old.actor != new.actor || old.kind != new.kind || old.publication >= new.publication {
        return Err(invalid("非法回收替代关系"));
    }
    let carries_all = old.shards.iter().all(|object| new.shards.contains(object));
    let safe = if new.kind == "state" {
        carries_all || state_dominates_streaming(conn, new, old)?
    } else {
        true
    };
    if !safe {
        return Err(invalid("新发布未支配旧状态，拒绝回收"));
    }
    let next: BTreeSet<_> = publication_objects(conn, new)?
        .into_iter()
        .map(|o| o.name)
        .collect();
    let objects = publication_objects(conn, old)?;
    let tx = crate::db::write_transaction(conn)?;
    database::require_generation(&tx, generation)?;
    tx.execute(
        "INSERT OR IGNORE INTO local_sync_gc(path,generation) VALUES(?1,?2)",
        params![old.manifest_path()?, generation],
    )?;
    tx.commit()?;
    for batch in objects.chunks(64) {
        let tx = crate::db::write_transaction(conn)?;
        database::require_generation(&tx, generation)?;
        for object in batch {
            if !next.contains(&object.name) {
                tx.execute(
                    "INSERT OR IGNORE INTO local_sync_gc(path,generation) VALUES(?1,?2)",
                    params![old.object_path(object), generation],
                )?;
            }
        }
        tx.commit()?;
        std::thread::yield_now();
    }
    Ok(())
}

pub struct Budget {
    remaining: usize,
}
impl Default for Budget {
    fn default() -> Self {
        Self::new()
    }
}
impl Budget {
    pub fn limited(remaining: usize) -> Self {
        Self { remaining }
    }
    pub fn remaining(&self) -> usize {
        self.remaining
    }
    pub fn new() -> Self {
        Self {
            remaining: REQUESTS_PER_ROUND,
        }
    }
    pub fn take(&mut self, n: usize) -> AppResult<()> {
        if self.remaining < n {
            return Err(AppError::new(
                "localSyncPending",
                "本轮传输预算已用完，剩余分片继续排队",
            ));
        }
        self.remaining -= n;
        Ok(())
    }
}

pub async fn list_manifests(
    transport: &Transport,
    actor: &str,
    kind: &str,
    budget: &mut Budget,
) -> AppResult<Vec<(String, Manifest)>> {
    list_manifests_guarded(transport, actor, kind, budget, || async { Ok(()) }).await
}
pub async fn list_manifests_guarded<F, Fut>(
    transport: &Transport,
    actor: &str,
    kind: &str,
    budget: &mut Budget,
    mut check: F,
) -> AppResult<Vec<(String, Manifest)>>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = AppResult<()>>,
{
    let path = format!("{ROOT}actors/{actor}/{kind}/manifests/");
    budget.take(1)?;
    check().await?;
    let mut names = transport
        .library_list(&path)
        .await
        .map_err(transport_error)?;
    names.sort();
    names.reverse();
    let mut result = Vec::new();
    // One manifest contains the full causal state of this actor. A newer manifest
    // missing pages is pending; it is never silently reported as an empty library.
    for name in names.into_iter().take(1) {
        let Some((seq, hash)) = name.strip_suffix(".json").and_then(|n| n.split_once('-')) else {
            return Err(invalid("发布清单文件名无效"));
        };
        if seq.len() != 20 || !seq.bytes().all(|b| b.is_ascii_digit()) || !valid_hex(hash, 64) {
            return Err(invalid("发布清单序号或哈希无效"));
        }
        budget.take(1)?;
        check().await?;
        let bytes = transport
            .library_get(&format!("{path}{name}"), MAX_MANIFEST_BYTES)
            .await
            .map_err(transport_error)?
            .ok_or_else(|| AppError::new("localSyncPending", "清单正在压缩或尚未完整到达"))?;
        check().await?;
        if digest(&bytes) != hash {
            return Err(invalid("发布清单哈希不匹配"));
        }
        let manifest: Manifest = serde_json::from_slice(&bytes)?;
        manifest.validate()?;
        if manifest.actor != actor
            || manifest.kind != kind
            || manifest.file_name().ok().as_deref() != Some(name.as_str())
        {
            return Err(invalid("发布清单身份或编号不匹配"));
        }
        result.push((name, manifest));
    }
    Ok(result)
}
