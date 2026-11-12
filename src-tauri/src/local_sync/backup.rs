//! Portable v4: bounded row batches, unique content records, per-block hashes and
//! a final container digest. No opaque DB, credentials, sender identity or journal.
use super::{database, model::*, storage};
use crate::webdav_transport::{FieldValue, Register, Stamp};
use crate::{
    db,
    error::{AppError, AppResult},
    state::AppState,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs::{File, OpenOptions},
    io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::Path,
    time::{Duration, Instant},
};
use tauri::{Emitter, State};
use tauri_plugin_dialog::DialogExt;
const MAGIC: &[u8; 16] = b"FLUXREADER-BK4\0\0";
const BACKUP_FORMAT: u32 = 4;
const BLOCK_TARGET: usize = 1024 * 1024;
const BLOCK_LIMIT: usize = 128 * 1024 * 1024;
const ROW_LIMIT: usize = 1_000_000;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    format: u32,
    created_at: String,
    settings: BTreeMap<String, Value>,
    sync_mode: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Folder {
    id: String,
    name: String,
    layout: String,
    summary_mode: String,
    translate_mode: String,
    position: i64,
    collapsed: bool,
    #[serde(default)]
    registers: BTreeMap<String, PortableRegister>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    id: String,
    url: String,
    title: String,
    site_url: Option<String>,
    favicon_url: Option<String>,
    folder: Option<String>,
    layout: String,
    summary_mode: String,
    translate_mode: String,
    active: bool,
    title_custom: String,
    fetched_title: String,
    #[serde(default)]
    registers: BTreeMap<String, PortableRegister>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Article {
    entry: String,
    meta: ArticleMeta,
    read: bool,
    star: bool,
    body: Option<BodyRef>,
    #[serde(default)]
    registers: BTreeMap<String, PortableRegister>,
}
/// Relative total order preserves Stamp order and equality groups. Original wire
/// keys stay in each source/entry record, so Projection's equal-Stamp key tie is
/// unchanged after fresh-actor restore; no sender actor/frontier/journal travels.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PortableRegister {
    order: u64,
    value: FieldValue,
}
#[derive(Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum Record {
    Header(Header),
    Folder(Folder),
    Source(Source),
    Article(Article),
    Content(BodyContent),
}
#[derive(Serialize)]
pub struct PortableBackupPreview {
    pub digest: String,
    pub format: u32,
    pub created_at: String,
    pub folders: usize,
    pub sources: usize,
    pub articles: usize,
    pub starred: usize,
    pub protected_bodies: usize,
    pub missing_bodies: usize,
    pub replaces_current_library: bool,
    pub disconnects_services: bool,
    #[serde(skip)]
    max_order: u64,
}
#[derive(Serialize)]
pub struct PortableBackupInfo {
    pub path: String,
    pub size_bytes: u64,
    pub digest: String,
    pub sources: usize,
    pub articles: usize,
    pub starred: usize,
    pub missing_bodies: usize,
}
fn io(error: std::io::Error) -> AppError {
    AppError::new("backup", error.to_string())
}
struct Writer {
    file: BufWriter<File>,
    hash: Sha256,
    records: Vec<Record>,
    estimate: usize,
    size: u64,
}
impl Writer {
    fn new(path: &Path) -> AppResult<Self> {
        let mut file = BufWriter::new(
            OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(path)
                .map_err(io)?,
        );
        file.write_all(MAGIC).map_err(io)?;
        let mut hash = Sha256::new();
        hash.update(MAGIC);
        Ok(Self {
            file,
            hash,
            records: Vec::new(),
            estimate: 0,
            size: 16,
        })
    }
    fn push(&mut self, record: Record) -> AppResult<()> {
        let size = serde_json::to_vec(&record)?.len();
        if size > BLOCK_LIMIT - 2 {
            return Err(invalid("单条备份记录超过格式字节预算"));
        }
        if !self.records.is_empty()
            && (self.estimate + size > BLOCK_TARGET || self.records.len() >= 128)
        {
            self.flush()?;
        }
        self.estimate += size + 1;
        self.records.push(record);
        Ok(())
    }
    fn flush(&mut self) -> AppResult<()> {
        if self.records.is_empty() {
            return Ok(());
        }
        let bytes = serde_json::to_vec(&self.records)?;
        let mut header = [0u8; 41];
        header[0] = 1;
        header[1..9].copy_from_slice(&(bytes.len() as u64).to_le_bytes());
        header[9..].copy_from_slice(&Sha256::digest(&bytes));
        self.file.write_all(&header).map_err(io)?;
        self.file.write_all(&bytes).map_err(io)?;
        self.hash.update(header);
        self.hash.update(&bytes);
        self.size += 41 + bytes.len() as u64;
        self.records.clear();
        self.estimate = 0;
        Ok(())
    }
    fn finish(mut self) -> AppResult<(String, u64)> {
        self.flush()?;
        let hash = self.hash.finalize();
        let mut footer = [0u8; 41];
        footer[0] = 255;
        footer[9..].copy_from_slice(&hash);
        self.file.write_all(&footer).map_err(io)?;
        self.file.flush().map_err(io)?;
        self.file.get_ref().sync_all().map_err(io)?;
        Ok((format!("{hash:x}"), self.size + 41))
    }
}
fn open_locked(path: &Path) -> AppResult<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1);
    }
    options.open(path).map_err(io)
}
/// Each pass hashes the actual parsed bytes. One Windows handle refuses write/delete sharing.
fn read(file: &mut File, mut visit: impl FnMut(Record) -> AppResult<()>) -> AppResult<String> {
    file.seek(SeekFrom::Start(0)).map_err(io)?;
    let mut reader = BufReader::new(file);
    let mut magic = [0; 16];
    reader.read_exact(&mut magic).map_err(io)?;
    if &magic != MAGIC {
        return Err(AppError::new(
            "unsupportedBackup",
            "此文件不是 v4 便携备份；旧文件与当前资料库保持原样",
        ));
    }
    let mut hash = Sha256::new();
    hash.update(magic);
    let mut rows = 0;
    loop {
        let mut header = [0; 41];
        reader
            .read_exact(&mut header)
            .map_err(|_| invalid("备份被截断或缺少完整结尾"))?;
        let len = u64::from_le_bytes(
            header[1..9]
                .try_into()
                .map_err(|_| invalid("备份块长度无效"))?,
        );
        if header[0] == 255 {
            if len != 0 || hash.clone().finalize().as_slice() != &header[9..] {
                return Err(invalid("备份最终完整性校验失败；当前资料库未修改"));
            }
            let mut extra = [0];
            if reader.read(&mut extra).map_err(io)? != 0 {
                return Err(invalid("备份结尾存在额外数据"));
            }
            return Ok(format!("{:x}", hash.finalize()));
        }
        if header[0] != 1 || len == 0 || len > BLOCK_LIMIT as u64 {
            return Err(invalid("备份块类型或大小无效"));
        }
        let mut bytes = vec![0; len as usize];
        reader
            .read_exact(&mut bytes)
            .map_err(|_| invalid("备份内容块被截断"))?;
        if Sha256::digest(&bytes).as_slice() != &header[9..] {
            return Err(invalid("备份内容块完整性校验失败"));
        }
        hash.update(header);
        hash.update(&bytes);
        let records: Vec<Record> = serde_json::from_slice(&bytes)?;
        if records.is_empty() || records.len() > 128 {
            return Err(invalid("备份块记录数无效"));
        }
        rows += records.len();
        if rows > ROW_LIMIT {
            return Err(invalid("备份记录数超过当前格式容量"));
        }
        for record in records {
            visit(record)?;
        }
    }
}
fn layout(value: &str, inherit: bool) -> bool {
    matches!(
        value,
        "article" | "social" | "image" | "podcast" | "notification"
    ) || (inherit && value == "inherit")
}
fn mode(value: &str) -> bool {
    matches!(value, "inherit" | "on" | "off")
}
fn portable_registers(
    conn: &Connection,
    target: &str,
) -> AppResult<BTreeMap<String, PortableRegister>> {
    let mut out = BTreeMap::new();
    for (key, reg) in super::logical::entity(conn, target)? {
        let stamp = serde_json::to_string(&reg.stamp)?;
        let order: u64 = conn.query_row(
            "SELECT priority FROM local_sync_backup_order WHERE stamp=?1",
            [stamp],
            |r| r.get(0),
        )?;
        let mut value = reg.value;
        if key.ends_with("/reactivate") {
            if let FieldValue::Set(v) = value {
                let epoch: Stamp = serde_json::from_value(v)?;
                let rank: Option<u64> = conn
                    .query_row(
                        "SELECT priority FROM local_sync_backup_order WHERE stamp=?1",
                        [serde_json::to_string(&epoch)?],
                        |r| r.get(0),
                    )
                    .optional()?;
                // A stale reference can never reactivate the newest deletion.
                // Retain a distinct zero rank rather than inventing a match.
                value = FieldValue::Set(json!({"order":rank.unwrap_or(0)}));
            }
        }
        out.insert(
            key.rsplit('/').next().unwrap_or_default().to_owned(),
            PortableRegister { order, value },
        );
    }
    Ok(out)
}
fn restore_registers(
    conn: &Connection,
    target: &str,
    values: &BTreeMap<String, PortableRegister>,
) -> AppResult<()> {
    let actor = database::actor(conn)?;
    for (name, value) in values {
        let mut field = value.value.clone();
        if name == "reactivate" {
            if let FieldValue::Set(v) = field {
                let rank = v["order"]
                    .as_u64()
                    .ok_or_else(|| invalid("备份恢复epoch优先级无效"))?;
                field = FieldValue::Set(serde_json::to_value(Stamp {
                    actor: actor.clone(),
                    seq: rank,
                    lamport: rank,
                })?);
            }
        }
        let reg = Register {
            stamp: Stamp {
                actor: actor.clone(),
                seq: value.order,
                lamport: value.order,
            },
            value: field,
        };
        conn.execute(
            "INSERT INTO local_sync_fields(key,register) VALUES(?1,?2)",
            params![format!("{target}/{name}"), serde_json::to_string(&reg)?],
        )?;
        conn.execute(
            "UPDATE local_sync_control SET seq=MAX(seq,?1),lamport=MAX(lamport,?1) WHERE id=1",
            [value.order],
        )?;
    }
    Ok(())
}
fn seed_restored(conn: &Connection, target: &str, data: Value) -> AppResult<()> {
    let order: u64 =
        conn.query_row("SELECT seq+1 FROM local_sync_control WHERE id=1", [], |r| {
            r.get(0)
        })?;
    let values = data
        .as_object()
        .ok_or_else(|| invalid("恢复seed必须是对象"))?
        .iter()
        .map(|(name, value)| {
            (
                name.clone(),
                PortableRegister {
                    order,
                    value: FieldValue::Set(value.clone()),
                },
            )
        })
        .collect();
    restore_registers(conn, target, &values)
}
fn validate_registers(target: &str, values: &BTreeMap<String, PortableRegister>) -> AppResult<()> {
    let actor = "0".repeat(32);
    let mut fields = BTreeMap::new();
    let mut max = 0;
    for (name, reg) in values {
        if reg.order == 0 || reg.order > i64::MAX as u64 {
            return Err(invalid("备份字段优先级无效"));
        }
        max = max.max(reg.order);
        let mut value = reg.value.clone();
        if name == "reactivate" {
            if let FieldValue::Set(v) = value {
                let rank = v["order"]
                    .as_u64()
                    .filter(|v| *v <= i64::MAX as u64)
                    .ok_or_else(|| invalid("备份恢复epoch优先级无效"))?;
                value = FieldValue::Set(serde_json::to_value(Stamp {
                    actor: actor.clone(),
                    seq: rank,
                    lamport: rank,
                })?);
            }
        }
        fields.insert(
            format!("{target}/{name}"),
            Register {
                stamp: Stamp {
                    actor: actor.clone(),
                    seq: reg.order,
                    lamport: reg.order,
                },
                value,
            },
        );
    }
    if !fields.is_empty() {
        validate_checkpoint(&crate::webdav_transport::Checkpoint {
            format: 2,
            library_id: "0".repeat(32),
            covered: BTreeMap::from([(actor, max)]),
            max_lamport: max,
            fields,
        })?;
    }
    Ok(())
}
fn matches_portable(
    values: &BTreeMap<String, PortableRegister>,
    name: &str,
    expected: Value,
) -> AppResult<()> {
    if !values.is_empty() && values.get(name).map(|r| &r.value) != Some(&FieldValue::Set(expected))
    {
        return Err(invalid(format!("备份字段{name}与成员记录不一致")));
    }
    Ok(())
}
fn export_body(
    conn: &Connection,
    desired: Option<BodyRef>,
    contents: &mut BTreeSet<i64>,
) -> AppResult<Option<BodyRef>> {
    let Some(body) = desired else { return Ok(None) };
    let mut ids = Vec::new();
    for identity in std::iter::once(&body.primary).chain(body.rss.iter()) {
        let id:Option<i64>=conn.query_row("SELECT id FROM contents WHERE identity=?1 AND (COALESCE(content_html,'')!='' OR body_text!='')",[identity],|r|r.get(0)).optional()?;
        let Some(id) = id else { return Ok(None) };
        ids.push(id);
    }
    contents.extend(ids);
    Ok(Some(body))
}
struct Check {
    header: Option<Header>,
    phase: u8,
    folders: BTreeSet<String>,
    sources: HashMap<String, bool>,
    entries: BTreeSet<String>,
    needed: BTreeSet<String>,
    contents: BTreeSet<String>,
    stars: usize,
    bodies: usize,
    missing: usize,
    max_order: u64,
}
impl Check {
    fn new() -> Self {
        Self {
            header: None,
            phase: 0,
            folders: BTreeSet::new(),
            sources: HashMap::new(),
            entries: BTreeSet::new(),
            needed: BTreeSet::new(),
            contents: BTreeSet::new(),
            stars: 0,
            bodies: 0,
            missing: 0,
            max_order: 0,
        }
    }
    fn accept(&mut self, record: Record) -> AppResult<()> {
        match record {
            Record::Header(header) => {
                if self.header.is_some() || self.phase != 0 || header.format != BACKUP_FORMAT {
                    return Err(invalid("便携备份版本或头顺序无效"));
                }
                for (k, v) in &header.settings {
                    validate_setting(k, v)?;
                }
                if let Some(mode) = &header.sync_mode {
                    db::validate_app_setting("syncMode", &json!(mode))
                        .map_err(|error| invalid(error.message))?;
                }
                self.header = Some(header);
                self.phase = 1;
            }
            Record::Folder(f) => {
                validate_registers(&format!("g/{}", f.id), &f.registers)?;
                matches_portable(&f.registers, "created", json!(true))?;
                matches_portable(&f.registers, "name", json!(f.name))?;
                self.max_order = self
                    .max_order
                    .max(f.registers.values().map(|r| r.order).max().unwrap_or(0));
                if self.phase > 1
                    || self.header.is_none()
                    || !valid_hex(&f.id, 32)
                    || f.name.trim().is_empty()
                    || f.name.len() > 16384
                    || !layout(&f.layout, false)
                    || !mode(&f.summary_mode)
                    || !mode(&f.translate_mode)
                    || !self.folders.insert(f.id)
                {
                    return Err(invalid("备份分类重复、顺序或字段无效"));
                }
            }
            Record::Source(f) => {
                validate_registers(&format!("s/{}", f.id), &f.registers)?;
                matches_portable(&f.registers, "created", json!(true))?;
                matches_portable(&f.registers, "feed_url", json!(f.url))?;
                matches_portable(&f.registers, "title", json!(f.title))?;
                self.max_order = self
                    .max_order
                    .max(f.registers.values().map(|r| r.order).max().unwrap_or(0));
                if self.phase > 2
                    || self.header.is_none()
                    || !valid_hex(&f.id, 32)
                    || f.title.trim().is_empty()
                    || f.title.len() > 16_384
                    || f.url.len() > 65_536
                    || f.title_custom.len() > 16_384
                    || f.fetched_title.len() > 16_384
                    || !layout(&f.layout, true)
                    || !mode(&f.summary_mode)
                    || !mode(&f.translate_mode)
                    || f.folder
                        .as_ref()
                        .is_some_and(|id| !self.folders.contains(id))
                    || !url::Url::parse(&f.url).is_ok_and(|u| {
                        matches!(u.scheme(), "http" | "https")
                            && u.host_str().is_some()
                            && u.username().is_empty()
                            && u.password().is_none()
                    })
                    || self.sources.insert(f.id, f.active).is_some()
                {
                    return Err(invalid("备份来源重复、引用或字段无效"));
                }
                self.phase = 2;
            }
            Record::Article(a) => {
                validate_registers(&format!("a/{}", a.entry), &a.registers)?;
                matches_portable(&a.registers, "meta", serde_json::to_value(&a.meta)?)?;
                matches_portable(&a.registers, "read", json!(a.read))?;
                matches_portable(&a.registers, "star", json!(a.star))?;
                if let Some(body) = &a.body {
                    matches_portable(&a.registers, "body", serde_json::to_value(body)?)?;
                }
                self.max_order = self
                    .max_order
                    .max(a.registers.values().map(|r| r.order).max().unwrap_or(0));
                if self.phase > 3
                    || self.header.is_none()
                    || !self.sources.contains_key(&a.meta.source)
                    || (!self.sources[&a.meta.source] && !a.star && a.registers.is_empty())
                    || a.meta.guid.is_empty()
                    || a.meta.guid.len() > 65536
                    || a.entry != entry_key(&a.meta.source, &a.meta.guid)
                    || !self.entries.insert(a.entry)
                    || !matches!(a.meta.image_source.as_str(), "rss" | "page")
                    || !["feed", "feed_clamped", "inferred_fetch", "legacy_unknown"]
                        .contains(&a.meta.published_at_source.as_str())
                {
                    return Err(invalid("备份文章身份、引用或字段无效"));
                }
                self.phase = 3;
                self.stars += usize::from(a.star);
                let required = a.star || (!a.read && self.sources[&a.meta.source]);
                match a.body {
                    Some(body) if required || !a.registers.is_empty() => {
                        self.bodies += 1;
                        self.needed.insert(body.primary);
                        if let Some(rss) = body.rss {
                            self.needed.insert(rss);
                        }
                    }
                    None if required => self.missing += 1,
                    Some(_) => return Err(invalid("普通已读文章不应携带正文")),
                    None => {}
                }
            }
            Record::Content(body) => {
                if self.phase < 2 {
                    return Err(invalid("正文先于来源或文章记录"));
                }
                self.phase = 4;
                body.validate()?;
                if !self.needed.contains(&body.identity) || !self.contents.insert(body.identity) {
                    return Err(invalid("备份正文重复或没有对应保护引用"));
                }
            }
        }
        Ok(())
    }
    fn finish(self, hash: String) -> AppResult<PortableBackupPreview> {
        if self.contents != self.needed {
            return Err(invalid("备份缺少被引用的正文块；文件不完整"));
        }
        let header = self.header.ok_or_else(|| invalid("备份没有头记录"))?;
        Ok(PortableBackupPreview {
            digest: hash,
            format: BACKUP_FORMAT,
            created_at: header.created_at,
            folders: self.folders.len(),
            sources: self.sources.len(),
            articles: self.entries.len(),
            starred: self.stars,
            protected_bodies: self.bodies,
            missing_bodies: self.missing,
            replaces_current_library: true,
            disconnects_services: true,
            max_order: self.max_order,
        })
    }
}
fn verify(file: &mut File) -> AppResult<PortableBackupPreview> {
    let mut check = Check::new();
    let hash = read(file, |record| check.accept(record))?;
    check.finish(hash)
}
pub fn preview(path: &Path) -> AppResult<PortableBackupPreview> {
    verify(&mut open_locked(path)?)
}
/// Test-only: the article record this export wrote for one entry. Production
/// restore always goes through [`verify`] first; a regression needs to assert
/// what the file actually contains without materialising a second library.
#[cfg(test)]
pub(super) fn read_exported_article(
    path: &Path,
    entry: &str,
) -> AppResult<Option<(bool, bool, Value)>> {
    let mut found = None;
    read(&mut open_locked(path)?, |record| {
        if let Record::Article(a) = &record {
            if a.entry == entry {
                found = Some((a.read, a.star, serde_json::to_value(&a.meta)?));
            }
        }
        Ok(())
    })?;
    Ok(found)
}
/// Portable export publishes one committed logical revision, not "the queue is
/// empty". A user may export while the foreground keeps writing; waiting for
/// `local_sync::logical::settled` would never return, because retention work is
/// re-enqueued by every foreground read/star and body holds are permanent.
///
/// The only thing worth waiting for is an accepted candidate that is mid-fold:
/// while it is being folded into `local_sync_fields`, the winners view resolves
/// each key against a partially advanced candidate table, so a read there would
/// mix pre- and post-fold values. That window is committed in 15ms slices, so it
/// clears by itself.
///
/// Publication is deliberately not waited for. `local_sync_pending`,
/// `local_sync_roots`, `local_sync_body_hold` and the body receipt tables belong
/// to the peer protocol (uploading this device's own batch) and to restore; the
/// export reads winners/articles/contents/feeds/folders/settings and none of
/// them. A sealed-but-unacked batch is the ordinary steady state of a synced
/// library — it may wait days for a peer — and this device usually has no peer at
/// all (the archive/portable tests seal and ACK by hand). Waiting for it would
/// just replace the previous deadlock with a different one.
///
/// The wait is bounded so a corrupt producer cannot hang the UI.
pub fn export(conn: &Connection, path: &Path) -> AppResult<PortableBackupInfo> {
    const GATE: Duration = Duration::from_secs(300);
    let deadline = Instant::now() + GATE;
    while super::logical::accepted_candidate(conn)? {
        if Instant::now() >= deadline {
            return Err(invalid("远端状态正在应用到本地，请稍后重试便携导出"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // Deferred: the first statement below pins the WAL read snapshot. Everything
    // after it (validation, ordering table, records) reads that one revision, so
    // foreground writes committed during the export cannot enter the file.
    let tx = conn.unchecked_transaction()?;
    let _snapshot_revision = super::logical::revision(&tx)?;
    tx.execute_batch("CREATE TEMP TABLE IF NOT EXISTS local_sync_backup_order(stamp TEXT PRIMARY KEY,priority INTEGER NOT NULL); DELETE FROM local_sync_backup_order;
      INSERT INTO local_sync_backup_order SELECT stamp,dense_rank() OVER(ORDER BY json_extract(stamp,'$.lamport'),json_extract(stamp,'$.actor') COLLATE BINARY,json_extract(stamp,'$.seq')) FROM (SELECT DISTINCT json_extract(register,'$.stamp') stamp FROM local_sync_winners);")?;
    super::logical::require_valid_locators(&tx)?;
    let mut writer = Writer::new(path)?;
    let mut settings = BTreeMap::new();
    let raw = db::get_setting(&tx, "app_settings")?.unwrap_or_else(|| "{}".into());
    let all: Value = serde_json::from_str(&raw)?;
    for key in SETTINGS {
        if let Some(value) = all.get(*key) {
            validate_setting(key, value)?;
            settings.insert((*key).to_owned(), value.clone());
        }
    }
    writer.push(Record::Header(Header {
        format: BACKUP_FORMAT,
        created_at: chrono::Utc::now().to_rfc3339(),
        settings,
        sync_mode: all
            .get("syncMode")
            .and_then(Value::as_str)
            .map(str::to_owned),
    }))?;
    let mut stmt = tx.prepare(
        "SELECT substr(key,1,34) FROM local_sync_winners WHERE key LIKE 'g/%/created' ORDER BY key",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let p: String = row.get(0)?;
        let fields = super::logical::entity(&tx, &p)?;
        let text = |name: &str, default: &str| {
            value(&fields, &p, name)
                .and_then(Value::as_str)
                .unwrap_or(default)
                .to_owned()
        };
        let collapsed = value(&fields, &p, "collapsed")
            .map(|value| value.as_bool().unwrap_or(value.as_i64() == Some(1)))
            .unwrap_or(true);
        writer.push(Record::Folder(Folder {
            id: p[2..].to_owned(),
            name: text("name", "Restored group"),
            layout: text("layout", "article"),
            summary_mode: text("summary_mode", "inherit"),
            translate_mode: text("translate_mode", "inherit"),
            position: value(&fields, &p, "position")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            collapsed,
            registers: portable_registers(&tx, &p)?,
        }))?;
    }
    drop(rows);
    drop(stmt);
    for f in db::list_folders(&tx)? {
        let id = f.sync_id.ok_or_else(|| invalid("分类缺少稳定身份"))?;
        if super::logical::get(&tx, &format!("g/{id}/created"))?.is_some() {
            continue;
        }
        writer.push(Record::Folder(Folder {
            registers: portable_registers(&tx, &format!("g/{id}"))?,
            id,
            name: f.name,
            layout: f.layout,
            summary_mode: f.summary_mode,
            translate_mode: f.translate_mode,
            position: f.position,
            collapsed: f.collapsed,
        }))?;
    }
    let mut sources = 0;
    let mut stmt = tx.prepare(
        "SELECT substr(key,1,34) FROM local_sync_winners WHERE key LIKE 's/%/created' ORDER BY key",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let p: String = row.get(0)?;
        let fields = super::logical::entity(&tx, &p)?;
        let text = |name: &str, default: &str| {
            value(&fields, &p, name)
                .and_then(Value::as_str)
                .unwrap_or(default)
                .to_owned()
        };
        let optional = |name: &str| {
            value(&fields, &p, name)
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        let folder = optional("folder");
        writer.push(Record::Source(Source {
            id: p[2..].to_owned(),
            url: text("feed_url", ""),
            title: text("title", ""),
            site_url: optional("site_url"),
            favicon_url: optional("favicon_url"),
            folder,
            layout: text("layout", "inherit"),
            summary_mode: text("summary_mode", "inherit"),
            translate_mode: text("translate_mode", "inherit"),
            active: active_source(&fields, &p),
            title_custom: text("title_custom", ""),
            fetched_title: text("fetched_title", ""),
            registers: portable_registers(&tx, &p)?,
        }))?;
        sources += 1;
    }
    drop(rows);
    drop(stmt);
    let mut stmt=tx.prepare("SELECT f.sync_id,f.feed_url,f.title,f.site_url,f.favicon_url,(SELECT sync_id FROM folders WHERE id=f.folder_id),f.layout,f.summary_mode,f.translate_mode,f.active,COALESCE(f.title_custom,''),f.fetched_title FROM feeds f WHERE NOT EXISTS(SELECT 1 FROM local_sync_winners WHERE key='s/'||f.sync_id||'/created') ORDER BY f.id")?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        writer.push(Record::Source(Source {
            id: r.get(0)?,
            url: r.get(1)?,
            title: r.get(2)?,
            site_url: r.get(3)?,
            favicon_url: r.get(4)?,
            folder: r.get(5)?,
            layout: r.get(6)?,
            summary_mode: r.get(7)?,
            translate_mode: r.get(8)?,
            active: r.get(9)?,
            title_custom: r.get(10)?,
            fetched_title: r.get(11)?,
            registers: BTreeMap::new(),
        }))?;
        sources += 1;
    }
    drop(rows);
    drop(stmt);
    let mut contents = BTreeSet::new();
    let (mut articles, mut starred, mut missing) = (0, 0, 0);
    let mut stmt=tx.prepare("SELECT substr(key,1,66),register FROM local_sync_winners WHERE key LIKE 'a/%/meta' ORDER BY key")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let p: String = row.get(0)?;
        let reg: Register = serde_json::from_str(&row.get::<_, String>(1)?)?;
        let FieldValue::Set(meta) = reg.value else {
            return Err(invalid("文章缺少meta"));
        };
        let meta: ArticleMeta = serde_json::from_value(meta)?;
        let fields = super::logical::entity(&tx, &p)?;
        let read = value(&fields, &p, "read") == Some(&Value::Bool(true));
        let star = value(&fields, &p, "star") == Some(&Value::Bool(true));
        let source_fields = super::logical::entity(&tx, &format!("s/{}", meta.source))?;
        let active = active_source(&source_fields, &format!("s/{}", meta.source));
        let desired = value(&fields, &p, "body")
            .cloned()
            .map(serde_json::from_value)
            .transpose()?;
        let body = export_body(&tx, desired, &mut contents)?;
        if body.is_none() && (star || (!read && active)) {
            missing += 1;
        }
        writer.push(Record::Article(Article {
            entry: p[2..].to_owned(),
            meta,
            read,
            star,
            body,
            registers: portable_registers(&tx, &p)?,
        }))?;
        articles += 1;
        starred += usize::from(star);
    }
    drop(rows);
    drop(stmt);
    let mut stmt=tx.prepare("SELECT a.id,f.sync_id,a.is_read,a.is_starred,f.active,a.content_id,a.rss_content_id FROM articles a JOIN feeds f ON f.id=a.feed_id WHERE (f.active=1 OR a.is_starred=1) AND NOT EXISTS(SELECT 1 FROM local_sync_winners WHERE key='s/'||f.sync_id||'/created') ORDER BY a.id")?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let id = r.get(0)?;
        let source = r.get::<_, String>(1)?;
        let read = r.get::<_, bool>(2)?;
        let star = r.get::<_, bool>(3)?;
        let active = r.get::<_, bool>(4)?;
        let primary = r.get::<_, Option<i64>>(5)?;
        let rss = r.get::<_, Option<i64>>(6)?;
        let meta = database::article_meta(&tx, id, &source)?;
        let mut identity = |id: Option<i64>| -> AppResult<Option<String>> {
            let Some(id) = id else { return Ok(None) };
            let value=tx.query_row("SELECT identity FROM contents WHERE id=?1 AND (COALESCE(content_html,'')!='' OR body_text!='')",[id],|r|r.get::<_,String>(0)).optional()?;
            if value.is_some() {
                contents.insert(id);
            }
            Ok(value)
        };
        let body = if star || (!read && active) {
            match identity(primary)? {
                Some(primary) => Some(BodyRef {
                    primary,
                    rss: identity(rss)?,
                }),
                None => {
                    missing += 1;
                    None
                }
            }
        } else {
            None
        };
        writer.push(Record::Article(Article {
            entry: entry_key(&source, &meta.guid),
            meta,
            read,
            star,
            body,
            registers: BTreeMap::new(),
        }))?;
        articles += 1;
        starred += usize::from(star);
    }
    drop(rows);
    drop(stmt);
    let mut seen = BTreeSet::new();
    for id in contents {
        if let Some(body) = database::content(&tx, Some(id))? {
            body.validate()?;
            if seen.insert(body.identity.clone()) {
                writer.push(Record::Content(body))?;
            }
        }
    }
    let (hash, size) = writer.finish()?;
    tx.commit()?;
    Ok(PortableBackupInfo {
        path: path.to_string_lossy().into_owned(),
        size_bytes: size,
        digest: hash,
        sources,
        articles,
        starred,
        missing_bodies: missing,
    })
}
fn clear_for_restore(conn: &Connection) -> AppResult<()> {
    conn.execute("UPDATE local_sync_control SET applying=1,generation=generation+1,capture=0,initialized=0 WHERE id=1",[])?;
    for table in [
        "ai_segments",
        "sync_queue",
        "subscription_outbox",
        "articles",
        "contents",
        "feeds",
        "folders",
        "source_tombstones",
        "local_sync_candidates",
        "local_sync_receive",
        "local_sync_work",
        "local_sync_intents",
        "local_sync_fast_pending",
        "local_sync_seen_batches",
        "local_sync_fast_objects",
        "local_sync_known_actors",
        "local_sync_roots",
        "local_sync_baseline_fields",
        "local_sync_baselines",
        "local_sync_fields",
        "local_sync_published_fields",
        "local_sync_journal",
        "local_sync_epochs",
        "local_sync_pending",
        "local_sync_objects",
        "local_sync_object_links",
        "local_sync_object_pins",
        "local_sync_gc",
        "local_sync_downloads",
        "local_sync_body_receipt_holds",
        "local_sync_body_hold",
        "local_sync_body_receipts",
    ] {
        conn.execute(&format!("DELETE FROM {table}"), [])?;
    }
    conn.execute("DELETE FROM settings WHERE key IN ('sync_protocol','greader_endpoint','greader_username','greader_password','miniflux_token','ai_config','pending_connection_config','config_sync_credentials') OR key GLOB 'local_sync_*'",[])?;
    db::bump_sync_generation(conn)?;
    db::reset_fever_history_state(conn)?;
    db::clear_zero_binding_discovery_attempts(conn)?;
    db::clear_binding_incomplete_attempts(conn)?;
    db::clear_feed_tombstones(conn)?;
    crate::endpoint_resolve::clear_cached_base(conn)?;
    conn.execute("DELETE FROM settings WHERE key='sync_last_disconnected_account_key' OR key GLOB 'folder_tombstones*'",[])?;
    db::set_setting(conn, "sync_last_sync", "0")?;
    db::set_setting(conn, "sync_last_entry_id", "0")?;
    conn.execute(
        "UPDATE local_sync_lanes SET last_sync=0,next_retry=0,failures=0,error=NULL",
        [],
    )?;
    conn.execute("UPDATE local_sync_control SET actor=?1,library_id='',seq=0,lamport=0,frontier='{}',completed_frontier='{}',accepted_job=NULL,logical_revision=0,folding=0,bootstrap_cursor=0,bootstrap_phase='groups',selected_baseline=NULL,selected_base=NULL,fast_stored_seq=0,published_seq=0,publication=0,last_state_sync=0,last_body_sync=0,next_retry_at=0,failures=0,last_error=NULL WHERE id=1",[storage::random_id(conn)?])?;
    Ok(())
}
pub fn restore(conn: &Connection, path: &Path, expected: &str) -> AppResult<PortableBackupInfo> {
    let mut file = open_locked(path)?;
    let preview = verify(&mut file)?;
    if preview.digest != expected {
        return Err(AppError::new(
            "staleBackupPreview",
            "备份文件在预览后发生变化；当前资料库未修改",
        ));
    }
    database::register_functions(conn)?;
    let tx = crate::db::write_transaction(conn)?;
    // Preserve row-ID freshness against in-flight fetch/AI work from the old library.
    let _next_folder: i64 =
        tx.query_row("SELECT MAX(COALESCE((SELECT MAX(id) FROM folders),0),COALESCE((SELECT seq FROM sqlite_sequence WHERE name='folders'),0))+1", [], |r| {
            r.get(0)
        })?;
    let _next_source: i64 =
        tx.query_row("SELECT MAX(COALESCE((SELECT MAX(id) FROM feeds),0),COALESCE((SELECT seq FROM sqlite_sequence WHERE name='feeds'),0))+1", [], |r| r.get(0))?;
    let _next_article: i64 =
        tx.query_row("SELECT MAX(COALESCE((SELECT MAX(id) FROM articles),0),COALESCE((SELECT seq FROM sqlite_sequence WHERE name='articles'),0))+1", [], |r| {
            r.get(0)
        })?;
    clear_for_restore(&tx)?;
    tx.execute(
        "UPDATE local_sync_control SET seq=?1,lamport=?1 WHERE id=1",
        [preview.max_order],
    )?;
    let mut pending = Vec::new();
    let hash = read(&mut file, |record| {
        match record {
            Record::Header(h) => {
                let mut local: Value = serde_json::from_str(
                    &db::get_setting(&tx, "app_settings")?.unwrap_or_else(|| "{}".into()),
                )?;
                let object = local
                    .as_object_mut()
                    .ok_or_else(|| invalid("本机设置结构损坏"))?;
                for key in SETTINGS {
                    object.remove(*key);
                }
                let ordinary = h.settings;
                seed_restored(&tx, "p", serde_json::to_value(&ordinary)?)?;
                object.extend(ordinary);
                object.insert(
                    "syncMode".into(),
                    json!(h.sync_mode.unwrap_or_else(|| "direct".into())),
                );
                db::set_setting(&tx, "app_settings", &serde_json::to_string(&local)?)?;
            }
            Record::Folder(f) => {
                let p = format!("g/{}", f.id);
                if f.registers.is_empty() {
                    seed_restored(
                        &tx,
                        &p,
                        json!({"created":true,"name":f.name,"layout":f.layout,"summary_mode":f.summary_mode,"translate_mode":f.translate_mode,"position":f.position,"collapsed":f.collapsed}),
                    )?;
                } else {
                    restore_registers(&tx, &p, &f.registers)?;
                }
            }
            Record::Source(f) => {
                let p = format!("s/{}", f.id);
                if f.registers.is_empty() {
                    let mut data = json!({"created":true,"feed_url":f.url,"title":f.title,"site_url":f.site_url,"favicon_url":f.favicon_url,"folder":f.folder,"layout":f.layout,"summary_mode":f.summary_mode,"translate_mode":f.translate_mode,"title_custom":f.title_custom,"fetched_title":f.fetched_title});
                    if !f.active {
                        data["deleted"] = json!(true);
                    }
                    seed_restored(&tx, &p, data)?;
                } else {
                    restore_registers(&tx, &p, &f.registers)?;
                }
            }
            Record::Article(a) => {
                let p = format!("a/{}", a.entry);
                if a.registers.is_empty() {
                    let mut data = json!({"meta":a.meta,"read":a.read,"star":a.star});
                    if let Some(body) = &a.body {
                        data["body"] = serde_json::to_value(body)?;
                    }
                    seed_restored(&tx, &p, data)?;
                } else {
                    restore_registers(&tx, &p, &a.registers)?;
                }
                if let Some(body) = a.body {
                    pending.push((a.entry, body));
                }
            }
            Record::Content(body) => {
                body.validate()?;
                let html = body
                    .html
                    .as_deref()
                    .map(|html| crate::sanitize::sanitize(html, None));
                if html != body.html {
                    return Err(invalid("正文安全规范化失败，当前资料库保留"));
                }
                db::portable_ensure_content(
                    &tx,
                    &db::PortableContentInput {
                        content_html: body.html,
                        body_text: body.text,
                        extraction_source: body.source,
                        fulltext_extracted: body.extracted,
                        url_norm: None,
                    },
                    None,
                )?;
            }
        }
        Ok(())
    })?;
    if hash != preview.digest {
        return Err(invalid("备份读取过程中发生变化；原资料库保留"));
    }
    for (entry, body) in pending {
        tx.execute(
            "INSERT INTO local_sync_body_hold(article_id,entry,body_ref) VALUES(NULL,?1,?2)",
            params![entry, serde_json::to_string(&body)?],
        )?;
    }
    // Imported relative ranks are re-stamped with this new actor. Rebuild the
    // same L once, then project each entity; URL/GUID aliases never INSERT two
    // conflicting application rows or choose winners by record traversal order.
    tx.execute("UPDATE local_sync_control SET library_id=lower(hex(randomblob(16))),frontier=json_object(actor,seq),capture=1,initialized=1,bootstrap_phase='done',logical_revision=logical_revision+1 WHERE id=1",[])?;
    tx.execute(
        "UPDATE local_sync_control SET completed_frontier=frontier WHERE id=1",
        [],
    )?;
    db::set_setting(
        &tx,
        "local_sync_seed_seq",
        &tx.query_row("SELECT seq FROM local_sync_control WHERE id=1", [], |r| {
            r.get::<_, u64>(0)
        })?
        .to_string(),
    )?;
    db::set_setting(&tx, "local_sync_detached", "1")?;
    for kind in ["g", "s", "a"] {
        let suffix = match kind {
            "g" | "s" => "created",
            _ => "meta",
        };
        let mut after = String::new();
        loop {
            let key:Option<String>=tx.query_row("SELECT key FROM local_sync_fields WHERE key LIKE ?1 AND key>?2 ORDER BY key LIMIT 1",params![format!("{kind}/%/{suffix}"),after],|r|r.get(0)).optional()?;
            let Some(key) = key else { break };
            after = key.clone();
            let target = key
                .rsplit_once('/')
                .ok_or_else(|| invalid("恢复字段身份无效"))?
                .0;
            super::logical::materialize(&tx, target)?;
        }
    }
    tx.execute("UPDATE local_sync_body_hold SET article_id=(SELECT a.id FROM articles a JOIN feeds f ON f.id=a.feed_id JOIN local_sync_winners m ON m.key='a/'||local_sync_body_hold.entry||'/meta' JOIN local_sync_source_urls s ON s.source=json_extract(m.register,'$.value.value.source') WHERE f.feed_url=s.url AND a.guid=json_extract(m.register,'$.value.value.guid') LIMIT 1)",[])?;
    tx.execute("DELETE FROM local_sync_work", [])?;
    tx.execute_batch(&super::capture::sql())?;
    // Lightweight read entries still keep their recorded cover/media projection.
    tx.execute("UPDATE articles SET image_url=image_url WHERE content_id IS NULL AND image_url IS NOT NULL",[])?;
    let errors: i64 = tx.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| {
        r.get(0)
    })?;
    if errors != 0 {
        return Err(invalid("恢复后的引用完整性校验失败；原资料库保留"));
    }
    tx.execute(
        "UPDATE local_sync_control SET applying=0,data_revision=data_revision+1 WHERE id=1",
        [],
    )?;
    tx.commit()?;
    Ok(PortableBackupInfo {
        path: path.to_string_lossy().into_owned(),
        size_bytes: file.metadata().map_err(io)?.len(),
        digest: preview.digest,
        sources: preview.sources,
        articles: preview.articles,
        starred: preview.starred,
        missing_bodies: preview.missing_bodies,
    })
}
#[tauri::command]
pub async fn export_portable_backup(
    state: State<'_, AppState>,
    path: String,
) -> AppResult<PortableBackupInfo> {
    let database = super::runtime::background_database(&state.db).await?;
    let runtime = state.local_sync.clone();
    tokio::task::spawn_blocking(move || {
        let _operation = runtime.portable.blocking_lock();
        if runtime.is_stopped() {
            return Err(AppError::new("cancelled", "应用正在退出，备份未开始"));
        }
        export(&database.blocking_lock(), Path::new(&path))
    })
    .await
    .map_err(|_| AppError::internal("备份工作线程未完成"))?
}
#[tauri::command]
pub async fn preview_portable_backup(
    state: State<'_, AppState>,
    path: String,
) -> AppResult<PortableBackupPreview> {
    let runtime = state.local_sync.clone();
    tokio::task::spawn_blocking(move || {
        let _operation = runtime.portable.blocking_lock();
        if runtime.is_stopped() {
            return Err(AppError::new("cancelled", "应用正在退出，预览未开始"));
        }
        preview(Path::new(&path))
    })
    .await
    .map_err(|_| AppError::internal("备份预览工作线程未完成"))?
}
#[tauri::command]
pub async fn restore_portable_backup(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    path: String,
    expected_digest: String,
) -> AppResult<PortableBackupInfo> {
    let database = state.db.clone();
    let runtime = state.local_sync.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _operation = runtime.portable.blocking_lock();
        if runtime.is_stopped() {
            return Err(AppError::new("cancelled", "应用正在退出，恢复未开始"));
        }
        let conn = database.blocking_lock();
        let result = restore(&conn, Path::new(&path), &expected_digest)?;
        crate::scheduler::set_tray_refresh_mode(&db::app_settings_str(&conn, "syncMode", "direct"));
        Ok(result)
    })
    .await
    .map_err(|_| AppError::internal("备份恢复工作线程未完成"))??;
    let _ = app.emit("feeds-updated", json!({"source":"portable-restore"}));
    let _ = app.emit("local-sync-updated", json!({"kind":"restore","ok":true}));
    Ok(result)
}
#[tauri::command]
pub async fn choose_portable_backup_file(
    app: tauri::AppHandle,
    save: bool,
) -> AppResult<Option<String>> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let dialog = app
        .dialog()
        .file()
        .add_filter("FluxReader 便携备份", &["flxb"]);
    if save {
        dialog
            .set_file_name(format!(
                "FluxReader-{}.flxb",
                chrono::Local::now().format("%Y%m%d-%H%M%S")
            ))
            .save_file(move |file| {
                let _ = sender.send(file);
            });
    } else {
        dialog.pick_file(move |file| {
            let _ = sender.send(file);
        });
    }
    receiver
        .await
        .map_err(|_| AppError::new("backupDialog", "文件选择器未返回结果"))?
        .map(|file| {
            file.into_path()
                .map(|path| path.to_string_lossy().into_owned())
                .map_err(|_| AppError::new("backupDialog", "请选择此电脑中的普通文件"))
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn file(label: &str) -> std::path::PathBuf {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("tmp/portable-tests");
        std::fs::create_dir_all(&root).unwrap();
        let db = Connection::open_in_memory().unwrap();
        root.join(format!("{label}-{}.flxb", storage::random_id(&db).unwrap()))
    }
    fn records() -> Vec<Record> {
        let source = "a".repeat(32);
        let content = BodyContent {
            identity: db::portable_content_identity("rss", Some("<p>body</p>"), "body"),
            html: Some("<p>body</p>".into()),
            text: "body".into(),
            source: "rss".into(),
            extracted: false,
        };
        vec![
            Record::Header(Header {
                format: 4,
                created_at: "2026-11-09T00:00:00Z".into(),
                settings: BTreeMap::new(),
                sync_mode: None,
            }),
            Record::Source(Source {
                id: source.clone(),
                url: "https://example.test/rss".into(),
                title: "Feed".into(),
                site_url: None,
                favicon_url: None,
                folder: None,
                layout: "inherit".into(),
                summary_mode: "inherit".into(),
                translate_mode: "inherit".into(),
                active: true,
                title_custom: String::new(),
                fetched_title: "Feed".into(),
                registers: BTreeMap::new(),
            }),
            Record::Article(Article {
                entry: entry_key(&source, "guid"),
                meta: ArticleMeta {
                    source,
                    guid: "guid".into(),
                    url: None,
                    title: "Article".into(),
                    author: None,
                    published_at: None,
                    published_at_source: "legacy_unknown".into(),
                    first_seen_at: None,
                    image_url: None,
                    image_source: "page".into(),
                    enclosure_url: None,
                    enclosure_mime: None,
                    duration_sec: None,
                },
                read: false,
                star: false,
                body: Some(BodyRef {
                    primary: content.identity.clone(),
                    rss: Some(content.identity.clone()),
                }),
                registers: BTreeMap::new(),
            }),
            Record::Content(content),
        ]
    }
    fn write(path: &Path, records: Vec<Record>) {
        let mut writer = Writer::new(path).unwrap();
        for record in records {
            writer.push(record).unwrap();
        }
        writer.finish().unwrap();
    }
    #[test]
    fn portable_v4_validates_references_order_and_container_integrity() {
        let good = file("valid");
        write(&good, records());
        let valid = preview(&good).unwrap();
        assert_eq!(valid.protected_bodies, 1);
        assert_eq!(valid.missing_bodies, 0);
        let mut missing_ref = records();
        missing_ref.pop();
        let missing = file("missing-ref");
        write(&missing, missing_ref);
        assert!(preview(&missing).is_err());
        let mut declared_missing = records();
        declared_missing.pop();
        if let Record::Article(a) = &mut declared_missing[2] {
            a.body = None;
        }
        let declared = file("declared-missing");
        write(&declared, declared_missing);
        assert_eq!(preview(&declared).unwrap().missing_bodies, 1);
        let mut duplicate_id = records();
        let copied: Source = match &duplicate_id[1] {
            Record::Source(f) => serde_json::from_value(serde_json::to_value(f).unwrap()).unwrap(),
            _ => unreachable!(),
        };
        duplicate_id.insert(2, Record::Source(copied));
        let duplicate_source = file("duplicate-source-id");
        write(&duplicate_source, duplicate_id);
        assert!(
            preview(&duplicate_source).is_err(),
            "the same source id remains a structural error"
        );
        let mut shared_url = records();
        let mut copied: Source = match &shared_url[1] {
            Record::Source(f) => serde_json::from_value(serde_json::to_value(f).unwrap()).unwrap(),
            _ => unreachable!(),
        };
        copied.id = "b".repeat(32);
        shared_url.insert(2, Record::Source(copied));
        let shared = file("shared-url");
        write(&shared, shared_url);
        let shared_preview = preview(&shared).unwrap();
        assert_eq!(
            shared_preview.sources, 2,
            "different source ids may share a raw URL and remain distinct wire aliases"
        );
        let mut retired = records();
        if let Record::Source(f) = &mut retired[1] {
            f.active = false;
        }
        let inactive = file("inactive");
        write(&inactive, retired);
        assert!(preview(&inactive).is_err());
        let bytes = std::fs::read(&good).unwrap();
        let damaged = file("damaged");
        let mut changed = bytes.clone();
        changed[60] ^= 1;
        std::fs::write(&damaged, changed).unwrap();
        assert!(preview(&damaged).is_err());
        let truncated = file("truncated");
        std::fs::write(&truncated, &bytes[..bytes.len() - 20]).unwrap();
        assert!(preview(&truncated).is_err());
        let trailing = file("trailing");
        let mut changed = bytes;
        changed.push(0);
        std::fs::write(&trailing, changed).unwrap();
        assert!(preview(&trailing).is_err());
        for path in [
            good,
            missing,
            declared,
            duplicate_source,
            shared,
            inactive,
            damaged,
            truncated,
            trailing,
        ] {
            std::fs::remove_file(path).unwrap();
        }
    }
    #[test]
    fn portable_v4_same_handle_refuses_write_and_old_format_is_preserved() {
        let good = file("locked");
        write(&good, records());
        let mut held = open_locked(&good).unwrap();
        let before = verify(&mut held).unwrap().digest;
        #[cfg(windows)]
        {
            assert!(
                OpenOptions::new().write(true).open(&good).is_err(),
                "Windows denies mutation while preview and restore share the same read handle"
            );
            assert!(
                std::fs::remove_file(&good).is_err(),
                "Windows denies replacing the held backup"
            );
        }
        assert_eq!(verify(&mut held).unwrap().digest, before);
        drop(held);
        let old = file("v3");
        let bytes = b"{\"format\":3,\"old\":true}";
        std::fs::write(&old, bytes).unwrap();
        assert_eq!(preview(&old).err().unwrap().code, "unsupportedBackup");
        assert_eq!(std::fs::read(&old).unwrap(), bytes);
        std::fs::remove_file(good).unwrap();
        std::fs::remove_file(old).unwrap();
    }
}
