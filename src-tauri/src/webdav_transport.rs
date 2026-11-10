//! Small, independently testable WebDAV transport. No application DB or sync policy.
//!
//! Immutable field-intent batches may coexist. LOCK never establishes a global
//! order. Stored receipts prove exact bytes, while pure per-field merging decides
//! current values using stamps fixed when local intents were created.

use quick_xml::{events::Event, name::ResolveResult, reader::NsReader};
use reqwest::{header, Client, Method, Response, StatusCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    time::{Duration, Instant},
};
use url::Url;

const XML_LIMIT: usize = 256 * 1024;
const STATE_LIMIT: usize = 2 * 1024 * 1024;
const XML_NODES: usize = 4096;
const XML_DEPTH: usize = 32;
const DAV: &str = "DAV:";
const LOCK_BODY: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?><D:lockinfo xmlns:D=\"DAV:\"><D:lockscope><D:exclusive/></D:lockscope><D:locktype><D:write/></D:locktype></D:lockinfo>";
const PROPFIND_BODY: &str =
    "<D:propfind xmlns:D=\"DAV:\"><D:prop><D:resourcetype/><D:getetag/></D:prop></D:propfind>";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    InvalidInput,
    Network,
    Http(u16),
    TooLarge,
    InvalidXml,
    InvalidState,
    Conflict,
    LeaseExpired,
    OutcomeUnknown,
}

#[derive(Debug)]
pub struct TransportError {
    pub kind: ErrorKind,
    pub message: String,
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for TransportError {}

pub type Result<T> = std::result::Result<T, TransportError>;

fn error(kind: ErrorKind, message: &str) -> TransportError {
    TransportError {
        kind,
        message: message.to_owned(),
    }
}

/// Supplied by the caller from 128 bits of fresh randomness. Transport does not
/// invent a clock-based identifier or open a database to obtain randomness.
#[derive(Clone, Copy)]
pub struct ProbeId(pub [u8; 16]);

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn valid_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Intentionally not Debug: callers must not log passwords.
#[derive(Clone)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

#[derive(Clone)]
pub struct Transport {
    http: Client,
    base: Url,
    credentials: Option<Credentials>,
}

/// Only returned after this transport created a fresh, dedicated collection.
/// Cleanup is restricted to that exact collection, never the configured root.
#[derive(Debug, Clone)]
pub struct ProbeCollection {
    url: Url,
    base: Url,
}

impl ProbeCollection {
    pub fn url(&self) -> &Url {
        &self.url
    }
}

/// Token is private and deliberately excluded from Debug/serialization.
pub struct Lease {
    collection: ProbeCollection,
    token: String,
    expires_at: Instant,
    duration: Duration,
}

impl Lease {
    pub fn duration(&self) -> Duration {
        self.duration
    }
    pub fn remaining(&self) -> Duration {
        self.expires_at.saturating_duration_since(Instant::now())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct Dot {
    pub actor: String,
    pub seq: u64,
}

/// A logical clock, not a timestamp or WebDAV receipt order. Set at the local
/// transaction which creates the intent, and never changed by upload/retry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct Stamp {
    pub lamport: u64,
    pub actor: String,
    pub seq: u64,
}

pub type Frontier = std::collections::BTreeMap<String, u64>;

/// A JSON null is a valid setting value and is distinct from deleting a field.
/// Explicit wire tags preserve that distinction in both batches and checkpoints.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(
    tag = "op",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum FieldValue {
    Set(serde_json::Value),
    Delete,
}

pub type FieldPatch = std::collections::BTreeMap<String, FieldValue>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Intent {
    pub seq: u64,
    pub lamport: u64,
    pub observed: Frontier,
    /// Delete is an explicit tombstone; absent keys are unchanged. Set(null)
    /// remains a present field whose business value is JSON null.
    pub patch: FieldPatch,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Batch {
    pub format: u32,
    pub library_id: String,
    pub actor: String,
    pub intents: Vec<Intent>,
}

const BATCH_LIMIT: usize = 256 * 1024;
const MAX_INTENTS: usize = 256;
const MAX_ACTORS: usize = 64;
const MAX_FIELDS: usize = 16_384;
const MAX_ACTIVE_INTENTS: usize = 4096;

fn validate_frontier(frontier: &Frontier) -> Result<()> {
    if frontier.len() > MAX_ACTORS
        || frontier
            .iter()
            .any(|(actor, seq)| !valid_id(actor) || *seq == 0)
    {
        return Err(error(ErrorKind::InvalidState, "invalid causal frontier"));
    }
    Ok(())
}

fn frontier_seq(frontier: &Frontier, actor: &str) -> u64 {
    frontier.get(actor).copied().unwrap_or(0)
}

fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 512 && !key.chars().any(char::is_control)
}

impl Batch {
    pub fn validate(&self) -> Result<()> {
        if self.format != 2
            || !valid_id(&self.library_id)
            || !valid_id(&self.actor)
            || self.intents.is_empty()
            || self.intents.len() > MAX_INTENTS
        {
            return Err(error(
                ErrorKind::InvalidState,
                "invalid batch header or size",
            ));
        }
        let mut previous: Option<&Intent> = None;
        for intent in &self.intents {
            validate_frontier(&intent.observed)?;
            if intent.seq == 0
                || intent.lamport < intent.seq
                || frontier_seq(&intent.observed, &self.actor) != intent.seq - 1
                || intent.patch.is_empty()
                || intent.patch.len() > 64
                || intent.patch.keys().any(|key| !valid_key(key))
            {
                return Err(error(
                    ErrorKind::InvalidState,
                    "invalid intent identity/context/patch",
                ));
            }
            if let Some(old) = previous {
                if old.seq.checked_add(1) != Some(intent.seq)
                    || old.lamport >= intent.lamport
                    || old
                        .observed
                        .iter()
                        .any(|(actor, seq)| frontier_seq(&intent.observed, actor) < *seq)
                {
                    return Err(error(
                        ErrorKind::InvalidState,
                        "batch is not a contiguous immutable actor sequence",
                    ));
                }
            }
            previous = Some(intent);
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct PreparedBatch {
    batch: Batch,
    bytes: Vec<u8>,
    id: String,
}

impl PreparedBatch {
    pub fn new(batch: Batch) -> Result<Self> {
        batch.validate()?;
        let bytes = serde_json::to_vec(&batch)
            .map_err(|_| error(ErrorKind::InvalidState, "batch serialization failed"))?;
        if bytes.len() > BATCH_LIMIT {
            return Err(error(ErrorKind::TooLarge, "batch budget exceeded"));
        }
        let id = hex(&Sha256::digest(&bytes));
        Ok(Self { batch, bytes, id })
    }
    pub fn batch(&self) -> &Batch {
        &self.batch
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn file_name(&self, attempt: ProbeId) -> String {
        immutable_name("batch", &self.id, attempt)
    }
}

/// Stored is transport durability evidence for these exact bytes, NOT a global
/// winning value or a unique server commit order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredReceipt {
    pub batch_id: String,
    pub actor: String,
    pub first_seq: u64,
    pub last_seq: u64,
}

impl PreparedBatch {
    fn receipt(&self) -> StoredReceipt {
        StoredReceipt {
            batch_id: self.id.clone(),
            actor: self.batch.actor.clone(),
            first_seq: self.batch.intents[0].seq,
            last_seq: self
                .batch
                .intents
                .last()
                .expect("validated nonempty batch")
                .seq,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreResolution {
    Stored(StoredReceipt),
    Pending,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Register {
    pub stamp: Stamp,
    pub value: FieldValue,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    pub format: u32,
    pub library_id: String,
    pub covered: Frontier,
    pub max_lamport: u64,
    pub fields: std::collections::BTreeMap<String, Register>,
}

impl Checkpoint {
    pub fn empty(library_id: &str) -> Result<Self> {
        if !valid_id(library_id) {
            return Err(error(ErrorKind::InvalidInput, "invalid library identity"));
        }
        Ok(Self {
            format: 2,
            library_id: library_id.to_owned(),
            covered: Frontier::new(),
            max_lamport: 0,
            fields: std::collections::BTreeMap::new(),
        })
    }
    pub fn validate(&self) -> Result<()> {
        validate_frontier(&self.covered)?;
        if self.format != 2
            || !valid_id(&self.library_id)
            || self.fields.len() > MAX_FIELDS
            || self.covered.values().any(|seq| *seq > self.max_lamport)
            || self.fields.iter().any(|(key, value)| {
                !valid_key(key)
                    || !valid_id(&value.stamp.actor)
                    || value.stamp.seq == 0
                    || value.stamp.lamport < value.stamp.seq
                    || value.stamp.seq > frontier_seq(&self.covered, &value.stamp.actor)
                    || value.stamp.lamport > self.max_lamport
            })
        {
            return Err(error(
                ErrorKind::InvalidState,
                "invalid checkpoint coverage/stamp",
            ));
        }
        Ok(())
    }
    /// Pure join: checkpoint creation does not allocate new user operation stamps.
    pub fn merge(&self, other: &Self) -> Result<Self> {
        self.validate()?;
        other.validate()?;
        if self.library_id != other.library_id {
            return Err(error(ErrorKind::Conflict, "different libraries"));
        }
        let mut joined = self.clone();
        for (actor, seq) in &other.covered {
            let slot = joined.covered.entry(actor.clone()).or_default();
            *slot = (*slot).max(*seq);
        }
        joined.max_lamport = joined.max_lamport.max(other.max_lamport);
        for (key, register) in &other.fields {
            join_register(&mut joined.fields, key, register)?;
        }
        joined.validate()?;
        Ok(joined)
    }
    pub fn covers(&self, dot: &Dot) -> bool {
        dot.seq <= frontier_seq(&self.covered, &dot.actor)
    }
    /// Field tombstones participate, so a removed setting cannot reappear from
    /// an older checkpoint. A frontier alone is not state domination evidence.
    pub fn dominates(&self, other: &Self) -> Result<bool> {
        Ok(self.merge(other)? == *self)
    }
}

fn join_register(
    fields: &mut std::collections::BTreeMap<String, Register>,
    key: &str,
    incoming: &Register,
) -> Result<()> {
    match fields.get(key) {
        Some(old) if old.stamp == incoming.stamp && old.value != incoming.value => Err(error(
            ErrorKind::Conflict,
            "same operation stamp has different field contents",
        )),
        Some(old) if old.stamp >= incoming.stamp => Ok(()),
        _ => {
            fields.insert(key.to_owned(), incoming.clone());
            Ok(())
        }
    }
}

#[derive(Debug, Clone)]
pub struct Replica {
    base: Checkpoint,
    active: std::collections::BTreeMap<Dot, Intent>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Projection {
    pub checkpoint: Checkpoint,
    pub pending: Vec<Dot>,
}

impl Replica {
    pub fn new(library_id: &str) -> Result<Self> {
        Ok(Self {
            base: Checkpoint::empty(library_id)?,
            active: std::collections::BTreeMap::new(),
        })
    }
    pub fn from_checkpoint(base: Checkpoint) -> Result<Self> {
        base.validate()?;
        Ok(Self {
            base,
            active: std::collections::BTreeMap::new(),
        })
    }
    /// Candidate is computed first; conflicts cannot partially mutate the replica.
    pub fn ingest_batch(&mut self, batch: &Batch) -> Result<()> {
        batch.validate()?;
        if batch.library_id != self.base.library_id {
            return Err(error(
                ErrorKind::Conflict,
                "batch belongs to another library",
            ));
        }
        let mut candidate = self.clone();
        for intent in &batch.intents {
            let dot = Dot {
                actor: batch.actor.clone(),
                seq: intent.seq,
            };
            // Deliberate bounded history contract: covered old inputs never
            // reapply. We do not claim to prove their historical payload identity.
            if candidate.base.covers(&dot) {
                continue;
            }
            if let Some(old) = candidate.active.get(&dot) {
                if old != intent {
                    return Err(error(
                        ErrorKind::Conflict,
                        "active dot has different contents",
                    ));
                }
            } else {
                candidate.active.insert(dot, intent.clone());
            }
        }
        if candidate.active.len() > MAX_ACTIVE_INTENTS {
            return Err(error(
                ErrorKind::TooLarge,
                "active intent budget; compact covered state before continuing",
            ));
        }
        candidate.project()?;
        *self = candidate;
        Ok(())
    }
    pub fn ingest_checkpoint(&mut self, checkpoint: &Checkpoint) -> Result<()> {
        let mut candidate = self.clone();
        let current = candidate.project()?;
        // The joined state is the new base, not merely a validation temporary.
        // Coverage cannot erase locally known winners, even when an incoming
        // checkpoint omits a field or carries an older value for that field.
        let mut joined = current.checkpoint.merge(checkpoint)?;
        for dot in &current.pending {
            if !joined.covers(dot) {
                continue;
            }
            let intent = &candidate.active[dot];
            // A checkpoint may supply missing dependencies, but must not ACK a
            // known pending intent while omitting its required causal frontier.
            if intent
                .observed
                .iter()
                .any(|(actor, seq)| frontier_seq(&joined.covered, actor) < *seq)
            {
                return Err(error(
                    ErrorKind::InvalidState,
                    "checkpoint covers a pending intent without its observed dependencies",
                ));
            }
            candidate.validate_observed_clock(intent, &joined)?;
            let stamp = Stamp {
                lamport: intent.lamport,
                actor: dot.actor.clone(),
                seq: dot.seq,
            };
            for (key, value) in &intent.patch {
                join_register(
                    &mut joined.fields,
                    key,
                    &Register {
                        stamp: stamp.clone(),
                        value: value.clone(),
                    },
                )?;
            }
            joined.max_lamport = joined.max_lamport.max(intent.lamport);
        }
        joined.validate()?;
        candidate.base = joined;
        candidate
            .active
            .retain(|dot, _| !candidate.base.covers(dot));
        candidate.project()?;
        *self = candidate;
        Ok(())
    }
    /// Missing actor prefixes or observed dependencies remain pending, not ACKed.
    pub fn project(&self) -> Result<Projection> {
        let mut checkpoint = self.base.clone();
        let mut remaining = self.active.clone();
        loop {
            let ready = remaining
                .iter()
                .find(|(dot, intent)| {
                    frontier_seq(&checkpoint.covered, &dot.actor).checked_add(1) == Some(dot.seq)
                        && intent
                            .observed
                            .iter()
                            .all(|(actor, seq)| frontier_seq(&checkpoint.covered, actor) >= *seq)
                })
                .map(|(dot, intent)| (dot.clone(), intent.clone()));
            let Some((dot, intent)) = ready else { break };
            self.validate_observed_clock(&intent, &checkpoint)?;
            let stamp = Stamp {
                lamport: intent.lamport,
                actor: dot.actor.clone(),
                seq: dot.seq,
            };
            for (key, value) in &intent.patch {
                join_register(
                    &mut checkpoint.fields,
                    key,
                    &Register {
                        stamp: stamp.clone(),
                        value: value.clone(),
                    },
                )?;
            }
            checkpoint.covered.insert(dot.actor.clone(), dot.seq);
            checkpoint.max_lamport = checkpoint.max_lamport.max(intent.lamport);
            remaining.remove(&dot);
        }
        checkpoint.validate()?;
        Ok(Projection {
            checkpoint,
            pending: remaining.keys().cloned().collect(),
        })
    }
    fn validate_observed_clock(&self, intent: &Intent, checkpoint: &Checkpoint) -> Result<()> {
        // Share the same causal validation between normal projection and
        // checkpoint-assisted projection. Unrelated high clocks are not bounds.
        for (actor, seq) in &intent.observed {
            if let Some(prior) = self.active.get(&Dot {
                actor: actor.clone(),
                seq: *seq,
            }) {
                if prior.lamport >= intent.lamport {
                    return Err(error(
                        ErrorKind::InvalidState,
                        "logical clock does not follow its observed dependency",
                    ));
                }
            }
        }
        if self
            .base
            .fields
            .values()
            .chain(checkpoint.fields.values())
            .any(|register| {
                register.stamp.seq <= frontier_seq(&intent.observed, &register.stamp.actor)
                    && register.stamp.lamport >= intent.lamport
            })
        {
            return Err(error(
                ErrorKind::InvalidState,
                "logical clock does not follow an observed retained winner",
            ));
        }
        Ok(())
    }
    /// Caller persists this intent and its sequence in the SAME business DB
    /// transaction before any network send. Retry reuses the sealed bytes.
    pub fn next_intent(&self, actor: &str, patch: FieldPatch) -> Result<Intent> {
        if !valid_id(actor) {
            return Err(error(ErrorKind::InvalidInput, "invalid actor"));
        }
        let projection = self.project()?;
        if self
            .active
            .keys()
            .any(|dot| dot.actor == actor && !projection.checkpoint.covers(dot))
        {
            return Err(error(
                ErrorKind::Conflict,
                "local actor has a missing sequence; cannot reuse it",
            ));
        }
        let seq = frontier_seq(&projection.checkpoint.covered, actor)
            .checked_add(1)
            .ok_or_else(|| error(ErrorKind::InvalidState, "sequence exhausted"))?;
        let lamport = projection
            .checkpoint
            .max_lamport
            .checked_add(1)
            .ok_or_else(|| error(ErrorKind::InvalidState, "logical clock exhausted"))?;
        let intent = Intent {
            seq,
            lamport,
            observed: projection.checkpoint.covered,
            patch,
        };
        Batch {
            format: 2,
            library_id: self.base.library_id.clone(),
            actor: actor.to_owned(),
            intents: vec![intent.clone()],
        }
        .validate()?;
        Ok(intent)
    }
    pub fn checkpoint(&self) -> Result<Checkpoint> {
        Ok(self.project()?.checkpoint)
    }
    /// Drops per-dot history already represented by field stamps/frontier. Missing
    /// dependency intents remain active. Thus normal steady-state space converges.
    pub fn compact(&mut self) -> Result<Checkpoint> {
        let checkpoint = self.checkpoint()?;
        self.ingest_checkpoint(&checkpoint)?;
        Ok(checkpoint)
    }
    pub fn active_count(&self) -> usize {
        self.active.len()
    }
}

#[derive(Debug, Clone)]
pub struct PreparedCheckpoint {
    checkpoint: Checkpoint,
    bytes: Vec<u8>,
    id: String,
}

impl PreparedCheckpoint {
    pub fn new(checkpoint: Checkpoint) -> Result<Self> {
        checkpoint.validate()?;
        let bytes = serde_json::to_vec(&checkpoint)
            .map_err(|_| error(ErrorKind::InvalidState, "checkpoint serialization failed"))?;
        if bytes.len() > STATE_LIMIT {
            return Err(error(ErrorKind::TooLarge, "checkpoint budget exceeded"));
        }
        let id = hex(&Sha256::digest(&bytes));
        Ok(Self {
            checkpoint,
            bytes,
            id,
        })
    }
    pub fn checkpoint(&self) -> &Checkpoint {
        &self.checkpoint
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn file_name(&self, attempt: ProbeId) -> String {
        immutable_name("checkpoint", &self.id, attempt)
    }
}

#[derive(Debug, Clone)]
pub struct StoredCheckpoint {
    id: String,
    file_name: String,
    collection_url: Url,
}

#[derive(Debug, Clone)]
pub struct PendingObject {
    pub file_name: String,
    pub reason: ErrorKind,
}

#[derive(Debug, Clone)]
struct SeenBatch {
    file_name: String,
    prepared: PreparedBatch,
}

#[derive(Debug, Clone)]
pub struct Scan {
    pub replica: Replica,
    pub pending_objects: Vec<PendingObject>,
    batches: Vec<SeenBatch>,
    collection_url: Url,
}

/// Plans only exact batch URIs from a completed scan whose effects are covered
/// by the stored checkpoint. No generation/age-based deletion, no checkpoint GC.
pub fn covered_batch_names(
    checkpoint: &PreparedCheckpoint,
    stored: &StoredCheckpoint,
    scan: &Scan,
) -> Result<Vec<String>> {
    if stored.id != checkpoint.id
        || stored.collection_url != scan.collection_url
        || !checkpoint
            .checkpoint
            .dominates(&scan.replica.checkpoint()?)?
    {
        return Err(error(
            ErrorKind::Conflict,
            "checkpoint does not cover the observed merged state",
        ));
    }
    Ok(scan
        .batches
        .iter()
        .filter(|seen| {
            seen.prepared.batch.intents.iter().all(|intent| {
                checkpoint.checkpoint.covers(&Dot {
                    actor: seen.prepared.batch.actor.clone(),
                    seq: intent.seq,
                })
            })
        })
        .map(|seen| seen.file_name.clone())
        .collect())
}

fn immutable_name(kind: &str, id: &str, attempt: ProbeId) -> String {
    format!("{kind}-{id}-{}.json", hex(&attempt.0))
}

fn object_id(name: &str, kind: &str) -> Option<String> {
    let rest = name
        .strip_prefix(kind)?
        .strip_prefix('-')?
        .strip_suffix(".json")?;
    let (id, attempt) = rest.split_once('-')?;
    if id.len() != 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        || !valid_id(attempt)
    {
        return None;
    }
    Some(id.to_owned())
}

impl Transport {
    pub fn new(base: &str, credentials: Option<Credentials>, timeout: Duration) -> Result<Self> {
        let mut base = Url::parse(base)
            .map_err(|_| error(ErrorKind::InvalidInput, "invalid WebDAV collection URL"))?;
        if !matches!(base.scheme(), "http" | "https")
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
            || timeout.is_zero()
            || timeout > Duration::from_secs(120)
        {
            return Err(error(
                ErrorKind::InvalidInput,
                "unsupported endpoint or timeout",
            ));
        }
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(timeout)
            .build()
            .map_err(|_| error(ErrorKind::InvalidInput, "HTTP client setup failed"))?;
        Ok(Self {
            http,
            base,
            credentials,
        })
    }

    fn request(&self, method: &str, url: Url) -> Result<reqwest::RequestBuilder> {
        if url.origin() != self.base.origin() || !url.path().starts_with(self.base.path()) {
            return Err(error(
                ErrorKind::InvalidInput,
                "request escaped owned endpoint",
            ));
        }
        let method = Method::from_bytes(method.as_bytes())
            .map_err(|_| error(ErrorKind::InvalidInput, "invalid HTTP method"))?;
        let mut request = self.http.request(method, url);
        if let Some(credentials) = &self.credentials {
            request = request.basic_auth(&credentials.username, Some(&credentials.password));
        }
        Ok(request)
    }

    fn owns(&self, collection: &ProbeCollection) -> Result<()> {
        if collection.base != self.base {
            return Err(error(
                ErrorKind::InvalidInput,
                "collection belongs to another endpoint",
            ));
        }
        Ok(())
    }

    fn resource(&self, collection: &ProbeCollection, name: &str) -> Result<Url> {
        self.owns(collection)?;
        if name.is_empty()
            || name.len() > 180
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
            || name == "."
            || name == ".."
        {
            return Err(error(ErrorKind::InvalidInput, "invalid resource name"));
        }
        collection
            .url
            .join(name)
            .map_err(|_| error(ErrorKind::InvalidInput, "invalid resource URL"))
    }

    pub async fn create_probe_collection(&self, id: ProbeId) -> Result<ProbeCollection> {
        let url = self
            .base
            .join(&format!(".fluxreader-probe-{}/", hex(&id.0)))
            .map_err(|_| error(ErrorKind::InvalidInput, "invalid probe URL"))?;
        let response = self
            .request("MKCOL", url.clone())?
            .send()
            .await
            .map_err(|_| {
                error(
                    ErrorKind::OutcomeUnknown,
                    "collection creation outcome unknown; do not claim existing data",
                )
            })?;
        require_status(response.status(), &[StatusCode::CREATED])?;
        read_bounded(response, XML_LIMIT).await?;
        Ok(ProbeCollection {
            url,
            base: self.base.clone(),
        })
    }

    pub async fn cleanup_probe(&self, collection: &ProbeCollection) -> Result<()> {
        self.owns(collection)?;
        let response = self
            .request("DELETE", collection.url.clone())?
            .send()
            .await
            .map_err(|_| error(ErrorKind::OutcomeUnknown, "probe cleanup outcome unknown"))?;
        require_status(
            response.status(),
            &[
                StatusCode::NO_CONTENT,
                StatusCode::OK,
                StatusCode::NOT_FOUND,
            ],
        )?;
        read_bounded(response, XML_LIMIT).await?;
        Ok(())
    }

    pub async fn acquire_lock(&self, collection: &ProbeCollection, seconds: u32) -> Result<Lease> {
        self.owns(collection)?;
        if !(1..=120).contains(&seconds) {
            return Err(error(
                ErrorKind::InvalidInput,
                "lock duration outside probe budget",
            ));
        }
        let started = Instant::now();
        let response = self
            .request("LOCK", collection.url.clone())?
            .header("Depth", "infinity")
            .header("Timeout", format!("Second-{seconds}"))
            .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
            .body(LOCK_BODY)
            .send()
            .await
            .map_err(|_| {
                error(
                    ErrorKind::OutcomeUnknown,
                    "LOCK outcome unknown; wait for lease expiration",
                )
            })?;
        require_status(response.status(), &[StatusCode::OK, StatusCode::CREATED])?;
        let token = response
            .headers()
            .get("Lock-Token")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| error(ErrorKind::InvalidXml, "missing Lock-Token"))?
            .to_owned();
        let token = parse_token(&token)?;
        let body = read_bounded(response, XML_LIMIT).await?;
        let duration = parse_lock(&body, &token, &collection.url)?;
        let expires_at = started
            .checked_add(duration)
            .ok_or_else(|| error(ErrorKind::InvalidXml, "invalid lease duration"))?;
        if expires_at <= Instant::now() {
            return Err(error(
                ErrorKind::LeaseExpired,
                "lease expired before response arrived",
            ));
        }
        Ok(Lease {
            collection: collection.clone(),
            token,
            duration,
            expires_at,
        })
    }

    pub async fn renew_lock(&self, lease: &mut Lease, seconds: u32) -> Result<()> {
        self.check_lease(lease)?;
        if !(1..=120).contains(&seconds) {
            return Err(error(
                ErrorKind::InvalidInput,
                "lock duration outside probe budget",
            ));
        }
        let started = Instant::now();
        let response = self
            .request("LOCK", lease.collection.url.clone())?
            .header("If", lease_condition(lease))
            .header("Timeout", format!("Second-{seconds}"))
            .send()
            .await
            .map_err(|_| error(ErrorKind::OutcomeUnknown, "lock renewal outcome unknown"))?;
        require_status(response.status(), &[StatusCode::OK])?;
        let duration = parse_lock(
            &read_bounded(response, XML_LIMIT).await?,
            &lease.token,
            &lease.collection.url,
        )?;
        lease.duration = duration;
        lease.expires_at = started
            .checked_add(duration)
            .ok_or_else(|| error(ErrorKind::InvalidXml, "invalid lease duration"))?;
        self.check_lease(lease)
    }

    pub async fn release_lock(&self, lease: &Lease) -> Result<()> {
        self.owns(&lease.collection)?;
        let response = self
            .request("UNLOCK", lease.collection.url.clone())?
            .header("Lock-Token", format!("<{}>", lease.token))
            .send()
            .await
            .map_err(|_| error(ErrorKind::OutcomeUnknown, "unlock outcome unknown"))?;
        require_status(response.status(), &[StatusCode::NO_CONTENT, StatusCode::OK])?;
        read_bounded(response, XML_LIMIT).await?;
        Ok(())
    }

    fn check_lease(&self, lease: &Lease) -> Result<()> {
        self.owns(&lease.collection)?;
        if lease.remaining().is_zero() {
            return Err(error(
                ErrorKind::LeaseExpired,
                "lease expired; acquire and re-read before writing",
            ));
        }
        Ok(())
    }

    /// Probe-only write. `Some(lease)` deliberately sends even an expired token
    /// so capability tests can verify the SERVER rejects it. Normal publication
    /// always calls check_lease first. Only the dedicated probe collection is writable.
    pub async fn probe_write(
        &self,
        collection: &ProbeCollection,
        name: &str,
        bytes: &[u8],
        lease: Option<&Lease>,
    ) -> Result<()> {
        if bytes.len() > STATE_LIMIT {
            return Err(error(ErrorKind::TooLarge, "probe write exceeds budget"));
        }
        let mut request = self
            .request("PUT", self.resource(collection, name)?)?
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .body(bytes.to_owned());
        if let Some(lease) = lease {
            if lease.collection.url != collection.url {
                return Err(error(
                    ErrorKind::InvalidInput,
                    "lock belongs to another collection",
                ));
            }
            request = request.header("If", lease_condition(lease));
        }
        let response = request
            .send()
            .await
            .map_err(|_| error(ErrorKind::OutcomeUnknown, "probe PUT outcome unknown"))?;
        require_status(
            response.status(),
            &[StatusCode::OK, StatusCode::CREATED, StatusCode::NO_CONTENT],
        )?;
        read_bounded(response, XML_LIMIT).await?;
        Ok(())
    }

    async fn get(&self, collection: &ProbeCollection, name: &str) -> Result<Option<Vec<u8>>> {
        let response = self
            .request("GET", self.resource(collection, name)?)?
            .send()
            .await
            .map_err(|_| error(ErrorKind::Network, "GET failed"))?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        require_status(response.status(), &[StatusCode::OK])?;
        Ok(Some(read_bounded(response, STATE_LIMIT).await?))
    }

    async fn names(&self, collection: &ProbeCollection) -> Result<Vec<String>> {
        self.owns(collection)?;
        let response = self
            .request("PROPFIND", collection.url.clone())?
            .header("Depth", "1")
            .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
            .body(PROPFIND_BODY)
            .send()
            .await
            .map_err(|_| error(ErrorKind::Network, "PROPFIND failed"))?;
        require_status(response.status(), &[StatusCode::MULTI_STATUS])?;
        parse_listing(&read_bounded(response, XML_LIMIT).await?, &collection.url)
    }

    /// Finds exact immutable bytes, never infers that their field values won.
    pub async fn resolve_batch(
        &self,
        collection: &ProbeCollection,
        batch: &PreparedBatch,
    ) -> Result<StoreResolution> {
        for name in self.names(collection).await? {
            if object_id(&name, "batch").as_deref() != Some(batch.id()) {
                continue;
            }
            if self.get(collection, &name).await?.as_deref() == Some(batch.bytes()) {
                return Ok(StoreResolution::Stored(batch.receipt()));
            }
        }
        Ok(StoreResolution::Pending)
    }

    /// LOCK is optional contention reduction, never the correctness boundary.
    /// Every attempt has its own name. A delayed previous attempt may also finish;
    /// both encode the same dots and are idempotent inputs, not competing heads.
    pub async fn store_batch(
        &self,
        collection: &ProbeCollection,
        batch: &PreparedBatch,
        attempt: ProbeId,
        lease: Option<&Lease>,
    ) -> Result<StoredReceipt> {
        if let StoreResolution::Stored(receipt) = self.resolve_batch(collection, batch).await? {
            return Ok(receipt);
        }
        self.store_object(
            collection,
            "batch",
            batch.id(),
            batch.bytes(),
            attempt,
            lease,
        )
        .await?;
        Ok(batch.receipt())
    }

    pub async fn store_checkpoint(
        &self,
        collection: &ProbeCollection,
        checkpoint: &PreparedCheckpoint,
        attempt: ProbeId,
        lease: Option<&Lease>,
    ) -> Result<StoredCheckpoint> {
        let file_name = self
            .store_object(
                collection,
                "checkpoint",
                checkpoint.id(),
                checkpoint.bytes(),
                attempt,
                lease,
            )
            .await?;
        Ok(StoredCheckpoint {
            id: checkpoint.id.clone(),
            file_name,
            collection_url: collection.url.clone(),
        })
    }

    async fn store_object(
        &self,
        collection: &ProbeCollection,
        kind: &str,
        id: &str,
        bytes: &[u8],
        attempt: ProbeId,
        lease: Option<&Lease>,
    ) -> Result<String> {
        self.owns(collection)?;
        if let Some(lease) = lease {
            self.check_lease(lease)?;
            if lease.collection.url != collection.url {
                return Err(error(
                    ErrorKind::InvalidInput,
                    "lease belongs to another collection",
                ));
            }
        }
        let file_name = immutable_name(kind, id, attempt);
        if let Some(existing) = self.get(collection, &file_name).await? {
            if existing == bytes {
                return Ok(file_name);
            }
            return Err(error(
                ErrorKind::Conflict,
                "attempt name already exists; use a new attempt without overwriting",
            ));
        }
        let stage_name = format!("stage-{kind}-{id}-{}", hex(&attempt.0));
        let stage = self.resource(collection, &stage_name)?;
        let mut request = self
            .request("PUT", stage.clone())?
            .header(header::IF_NONE_MATCH, "*")
            .header(header::CONTENT_TYPE, "application/json")
            .body(bytes.to_owned());
        if let Some(lease) = lease {
            request = request.header("If", lease_condition(lease));
        }
        let response = request.send().await.map_err(|_| {
            error(
                ErrorKind::OutcomeUnknown,
                "staging PUT outcome unknown; retain sealed batch",
            )
        })?;
        require_status(response.status(), &[StatusCode::CREATED])?;
        read_bounded(response, XML_LIMIT).await?;
        if self.get(collection, &stage_name).await?.as_deref() != Some(bytes) {
            return Err(error(
                ErrorKind::InvalidState,
                "staging readback did not match",
            ));
        }
        if let Some(lease) = lease {
            self.check_lease(lease)?;
        }
        let destination = self.resource(collection, &file_name)?;
        let mut request = self
            .request("MOVE", stage)?
            .header("Destination", destination.as_str())
            .header("Overwrite", "F");
        if let Some(lease) = lease {
            request = request.header("If", lease_condition(lease));
        }
        let response = request.send().await.map_err(|_| {
            error(
                ErrorKind::OutcomeUnknown,
                "MOVE outcome unknown; query batch hash without assigning a new operation",
            )
        })?;
        require_status(response.status(), &[StatusCode::CREATED])?;
        read_bounded(response, XML_LIMIT)
            .await
            .map_err(|_| error(ErrorKind::OutcomeUnknown, "MOVE response incomplete"))?;
        match self.get(collection, &file_name).await {
            Ok(Some(readback)) if readback == bytes => Ok(file_name),
            _ => Err(error(
                ErrorKind::OutcomeUnknown,
                "immutable bytes not yet confirmed",
            )),
        }
    }

    /// Reuses known local state: a missing listing entry never rolls it backward.
    /// Concurrent branches are ordinary inputs. Incomplete objects are pending,
    /// never applied or counted as an acknowledged sequence.
    pub async fn scan(&self, collection: &ProbeCollection, known: &Replica) -> Result<Scan> {
        let names = self.names(collection).await?;
        let mut replica = known.clone();
        let mut pending_objects = Vec::new();
        let mut checkpoints = Vec::new();
        let mut batches = Vec::new();
        for name in names {
            let (kind, id) = if let Some(id) = object_id(&name, "checkpoint") {
                ("checkpoint", id)
            } else if let Some(id) = object_id(&name, "batch") {
                ("batch", id)
            } else if name.starts_with("stage-") {
                continue;
            } else if name.starts_with("batch-") || name.starts_with("checkpoint-") {
                return Err(error(
                    ErrorKind::InvalidState,
                    "invalid immutable object name",
                ));
            } else {
                continue;
            };
            let bytes = match self.get(collection, &name).await? {
                Some(bytes) => bytes,
                None => {
                    pending_objects.push(PendingObject {
                        file_name: name,
                        reason: ErrorKind::Network,
                    });
                    continue;
                }
            };
            if hex(&Sha256::digest(&bytes)) != id {
                pending_objects.push(PendingObject {
                    file_name: name,
                    reason: ErrorKind::InvalidState,
                });
                continue;
            }
            if kind == "checkpoint" {
                let checkpoint: Checkpoint = serde_json::from_slice(&bytes)
                    .map_err(|_| error(ErrorKind::InvalidState, "invalid complete checkpoint"))?;
                checkpoint.validate()?;
                checkpoints.push(checkpoint);
            } else {
                let batch: Batch = serde_json::from_slice(&bytes)
                    .map_err(|_| error(ErrorKind::InvalidState, "invalid complete batch"))?;
                let prepared = PreparedBatch::new(batch)?;
                if prepared.bytes != bytes {
                    return Err(error(
                        ErrorKind::InvalidState,
                        "batch encoding is not canonical",
                    ));
                }
                batches.push(SeenBatch {
                    file_name: name,
                    prepared,
                });
            }
        }
        // Covered old seq is ignored by contract, independent of listing order.
        for checkpoint in checkpoints {
            replica.ingest_checkpoint(&checkpoint)?;
        }
        for seen in &batches {
            replica.ingest_batch(seen.prepared.batch())?;
        }
        Ok(Scan {
            replica,
            pending_objects,
            batches,
            collection_url: collection.url.clone(),
        })
    }

    /// Narrow GC: only exact, fully validated batches proven covered by a stored
    /// checkpoint. This phase deliberately does not delete checkpoints/archives.
    pub async fn remove_covered_batches(
        &self,
        collection: &ProbeCollection,
        checkpoint: &PreparedCheckpoint,
        stored: &StoredCheckpoint,
        scan: &Scan,
    ) -> Result<usize> {
        if stored.collection_url != collection.url || scan.collection_url != collection.url {
            return Err(error(
                ErrorKind::InvalidInput,
                "cleanup proof belongs to another collection",
            ));
        }
        if self.get(collection, &stored.file_name).await?.as_deref() != Some(checkpoint.bytes()) {
            return Err(error(
                ErrorKind::Conflict,
                "covering checkpoint is not readable; retain batches",
            ));
        }
        let names = covered_batch_names(checkpoint, stored, scan)?;
        let mut removed = 0;
        for name in names {
            let response = self
                .request("DELETE", self.resource(collection, &name)?)?
                .send()
                .await
                .map_err(|_| {
                    error(
                        ErrorKind::OutcomeUnknown,
                        "covered batch cleanup outcome unknown",
                    )
                })?;
            require_status(
                response.status(),
                &[
                    StatusCode::NO_CONTENT,
                    StatusCode::OK,
                    StatusCode::NOT_FOUND,
                ],
            )?;
            read_bounded(response, XML_LIMIT).await?;
            removed += 1;
        }
        Ok(removed)
    }
}

fn lease_condition(lease: &Lease) -> String {
    format!("(<{}>)", lease.token)
}

fn require_status(actual: StatusCode, allowed: &[StatusCode]) -> Result<()> {
    if allowed.contains(&actual) {
        Ok(())
    } else {
        Err(error(
            ErrorKind::Http(actual.as_u16()),
            "unexpected WebDAV status",
        ))
    }
}

async fn read_bounded(mut response: Response, limit: usize) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|size| size > limit as u64)
    {
        return Err(error(
            ErrorKind::TooLarge,
            "response Content-Length exceeds budget",
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| error(ErrorKind::Network, "response body incomplete"))?
    {
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err(error(ErrorKind::TooLarge, "response body exceeds budget"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn parse_token(raw: &str) -> Result<String> {
    let token = raw
        .strip_prefix('<')
        .and_then(|value| value.strip_suffix('>'))
        .ok_or_else(|| error(ErrorKind::InvalidXml, "invalid lock token header"))?;
    if token.is_empty()
        || token.len() > 512
        || !token.is_ascii()
        || token
            .bytes()
            .any(|b| b <= 0x20 || matches!(b, b'<' | b'>' | b'(' | b')' | b'"'))
        || Url::parse(token).is_err()
    {
        return Err(error(ErrorKind::InvalidXml, "invalid lock token value"));
    }
    Ok(token.to_owned())
}

#[derive(Debug)]
struct XmlNode {
    namespace: String,
    name: String,
    text: String,
    children: Vec<usize>,
}

struct XmlDoc {
    nodes: Vec<XmlNode>,
}

impl XmlDoc {
    fn child(&self, parent: usize, name: &str) -> Result<usize> {
        let mut matches = self.nodes[parent]
            .children
            .iter()
            .copied()
            .filter(|&i| self.nodes[i].namespace == DAV && self.nodes[i].name == name);
        let value = matches
            .next()
            .ok_or_else(|| error(ErrorKind::InvalidXml, "required DAV element missing"))?;
        if matches.next().is_some() {
            return Err(error(ErrorKind::InvalidXml, "duplicate DAV element"));
        }
        Ok(value)
    }

    fn text(&self, index: usize) -> Result<&str> {
        if !self.nodes[index].children.is_empty() {
            return Err(error(
                ErrorKind::InvalidXml,
                "text element contains nested markup",
            ));
        }
        Ok(self.nodes[index].text.trim())
    }

    fn root(&self, name: &str) -> Result<usize> {
        if self
            .nodes
            .first()
            .is_some_and(|node| node.namespace == DAV && node.name == name)
        {
            Ok(0)
        } else {
            Err(error(
                ErrorKind::InvalidXml,
                "wrong DAV root namespace or name",
            ))
        }
    }
}

fn parse_xml(bytes: &[u8]) -> Result<XmlDoc> {
    if bytes.len() > XML_LIMIT {
        return Err(error(ErrorKind::TooLarge, "XML exceeds byte budget"));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| error(ErrorKind::InvalidXml, "XML must be UTF-8"))?;
    let mut reader = NsReader::from_str(text);
    reader.config_mut().expand_empty_elements = true;
    reader.config_mut().check_comments = true;
    let mut nodes: Vec<XmlNode> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut events = 0;
    loop {
        events += 1;
        if events > XML_NODES * 4 {
            return Err(error(ErrorKind::TooLarge, "XML event budget exceeded"));
        }
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|_| error(ErrorKind::InvalidXml, "malformed XML"))?;
        match event {
            Event::Start(element) => {
                let namespace = match namespace {
                    ResolveResult::Bound(ns) => std::str::from_utf8(ns.as_ref())
                        .map_err(|_| error(ErrorKind::InvalidXml, "invalid namespace"))?
                        .to_owned(),
                    ResolveResult::Unbound => String::new(),
                    ResolveResult::Unknown(_) => {
                        return Err(error(ErrorKind::InvalidXml, "unbound XML prefix"))
                    }
                };
                if stack.len() >= XML_DEPTH
                    || nodes.len() >= XML_NODES
                    || element.name().as_ref().len() > 128
                {
                    return Err(error(ErrorKind::TooLarge, "XML structure budget exceeded"));
                }
                for (count, attr) in element.attributes().enumerate() {
                    let attr =
                        attr.map_err(|_| error(ErrorKind::InvalidXml, "malformed XML attribute"))?;
                    if count >= 32 || attr.value.len() > 4096 {
                        return Err(error(ErrorKind::TooLarge, "XML attribute budget exceeded"));
                    }
                    attr.decoded_and_normalized_value(
                        quick_xml::XmlVersion::Explicit1_0,
                        reader.decoder(),
                    )
                    .map_err(|_| error(ErrorKind::InvalidXml, "unknown attribute entity"))?;
                }
                if stack.is_empty() && !nodes.is_empty() {
                    return Err(error(ErrorKind::InvalidXml, "multiple XML roots"));
                }
                let index = nodes.len();
                if let Some(&parent) = stack.last() {
                    nodes[parent].children.push(index);
                }
                nodes.push(XmlNode {
                    namespace,
                    name: std::str::from_utf8(element.local_name().as_ref())
                        .map_err(|_| error(ErrorKind::InvalidXml, "invalid element name"))?
                        .to_owned(),
                    text: String::new(),
                    children: Vec::new(),
                });
                stack.push(index);
            }
            Event::End(_) => {
                stack
                    .pop()
                    .ok_or_else(|| error(ErrorKind::InvalidXml, "unbalanced XML"))?;
            }
            Event::Text(value) => {
                let value = value
                    .decode()
                    .map_err(|_| error(ErrorKind::InvalidXml, "invalid XML text"))?;
                append_xml_text(&mut nodes, &stack, &value)?;
            }
            Event::CData(value) => {
                let value = value
                    .decode()
                    .map_err(|_| error(ErrorKind::InvalidXml, "invalid CDATA"))?;
                append_xml_text(&mut nodes, &stack, &value)?;
            }
            Event::GeneralRef(value) => {
                let name = value
                    .decode()
                    .map_err(|_| error(ErrorKind::InvalidXml, "invalid entity"))?;
                let decoded = match name.as_ref() {
                    "amp" => "&".to_owned(),
                    "lt" => "<".to_owned(),
                    "gt" => ">".to_owned(),
                    "quot" => "\"".to_owned(),
                    "apos" => "'".to_owned(),
                    _ => value
                        .resolve_char_ref()
                        .ok()
                        .flatten()
                        .map(|c| c.to_string())
                        .ok_or_else(|| {
                            error(ErrorKind::InvalidXml, "custom/external entities forbidden")
                        })?,
                };
                append_xml_text(&mut nodes, &stack, &decoded)?;
            }
            Event::Decl(declaration) => {
                if !nodes.is_empty()
                    || declaration.version().ok().as_deref() != Some(b"1.0")
                    || declaration.encoding().is_some_and(|value| {
                        value.map_or(true, |enc| !enc.eq_ignore_ascii_case(b"utf-8"))
                    })
                {
                    return Err(error(ErrorKind::InvalidXml, "unsupported XML declaration"));
                }
            }
            Event::DocType(_) | Event::PI(_) => {
                return Err(error(
                    ErrorKind::InvalidXml,
                    "DTD and processing instructions forbidden",
                ))
            }
            Event::Comment(_) => {}
            Event::Eof => break,
            Event::Empty(_) => {
                return Err(error(
                    ErrorKind::InvalidXml,
                    "unexpected unexpanded element",
                ))
            }
        }
    }
    if nodes.is_empty() || !stack.is_empty() {
        return Err(error(ErrorKind::InvalidXml, "incomplete XML"));
    }
    Ok(XmlDoc { nodes })
}

fn append_xml_text(nodes: &mut [XmlNode], stack: &[usize], value: &str) -> Result<()> {
    if value.chars().any(|c| {
        !matches!(c, '\t' | '\n' | '\r') && (c < '\u{20}' || matches!(c, '\u{fffe}' | '\u{ffff}'))
    }) {
        return Err(error(ErrorKind::InvalidXml, "invalid XML character"));
    }
    if let Some(&index) = stack.last() {
        if nodes[index].text.len() + value.len() > 64 * 1024 {
            return Err(error(ErrorKind::TooLarge, "XML text budget exceeded"));
        }
        nodes[index].text.push_str(value);
    } else if !value.trim().is_empty() {
        return Err(error(ErrorKind::InvalidXml, "text outside root"));
    }
    Ok(())
}

fn parse_lock(bytes: &[u8], token: &str, collection: &Url) -> Result<Duration> {
    let doc = parse_xml(bytes)?;
    let root = doc.root("prop")?;
    let discovery = doc.child(root, "lockdiscovery")?;
    let active = doc.child(discovery, "activelock")?;
    let scope = doc.child(active, "lockscope")?;
    doc.child(scope, "exclusive")?;
    let kind = doc.child(active, "locktype")?;
    doc.child(kind, "write")?;
    if doc.text(doc.child(active, "depth")?)? != "infinity"
        || doc.text(doc.child(doc.child(active, "locktoken")?, "href")?)? != token
    {
        return Err(error(ErrorKind::InvalidXml, "lock scope/token mismatch"));
    }
    let root_href = doc.text(doc.child(doc.child(active, "lockroot")?, "href")?)?;
    let lock_root = collection
        .join(root_href)
        .map_err(|_| error(ErrorKind::InvalidXml, "invalid lock root"))?;
    if &lock_root != collection {
        return Err(error(ErrorKind::InvalidXml, "lock root mismatch"));
    }
    let timeout = doc.text(doc.child(active, "timeout")?)?;
    let seconds = timeout
        .strip_prefix("Second-")
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|&seconds| (1..=3600).contains(&seconds))
        .ok_or_else(|| error(ErrorKind::InvalidXml, "finite lock timeout required"))?;
    Ok(Duration::from_secs(seconds))
}

fn parse_listing(bytes: &[u8], collection: &Url) -> Result<Vec<String>> {
    let doc = parse_xml(bytes)?;
    let root = doc.root("multistatus")?;
    let mut names = std::collections::BTreeSet::new();
    for &index in &doc.nodes[root].children {
        if doc.nodes[index].namespace != DAV || doc.nodes[index].name != "response" {
            return Err(error(
                ErrorKind::InvalidXml,
                "unexpected multistatus element",
            ));
        }
        let href = doc.text(doc.child(index, "href")?)?;
        let url = collection
            .join(href)
            .map_err(|_| error(ErrorKind::InvalidXml, "invalid listing href"))?;
        if url.origin() != collection.origin()
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(error(ErrorKind::InvalidXml, "listing escaped collection"));
        }
        if url == *collection {
            continue;
        }
        let name = url
            .path()
            .strip_prefix(collection.path())
            .filter(|name| !name.is_empty() && !name.contains('/') && !name.contains('%'))
            .ok_or_else(|| error(ErrorKind::InvalidXml, "listing is not Depth:1"))?;
        if !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        {
            return Err(error(ErrorKind::InvalidXml, "invalid listed file name"));
        }
        let propstat = doc.child(index, "propstat")?;
        let status = doc.text(doc.child(propstat, "status")?)?;
        if !matches!(status, "HTTP/1.1 200 OK" | "HTTP/1.0 200 OK") {
            return Err(error(
                ErrorKind::InvalidXml,
                "listing property request failed",
            ));
        }
        if !names.insert(name.to_owned()) {
            return Err(error(ErrorKind::InvalidXml, "duplicate listing href"));
        }
        if names.len() > 128 {
            return Err(error(
                ErrorKind::TooLarge,
                "listing resource budget exceeded",
            ));
        }
    }
    Ok(names.into_iter().collect())
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn actor(n: u8) -> String {
        format!("{n:02x}").repeat(16)
    }
    fn patch(key: &str, value: FieldValue) -> FieldPatch {
        BTreeMap::from([(key.to_owned(), value)])
    }
    fn batch(actor_id: u8, seq: u64, clock: u64, patch: FieldPatch) -> Batch {
        Batch {
            format: 2,
            library_id: actor(9),
            actor: actor(actor_id),
            intents: vec![Intent {
                seq,
                lamport: clock,
                observed: if seq > 1 {
                    BTreeMap::from([(actor(actor_id), seq - 1)])
                } else {
                    BTreeMap::new()
                },
                patch,
            }],
        }
    }
    fn merged(inputs: &[Batch]) -> Checkpoint {
        let mut replica = Replica::new(&actor(9)).unwrap();
        for input in inputs {
            replica.ingest_batch(input).unwrap();
        }
        replica.checkpoint().unwrap()
    }

    #[test]
    fn join_is_commutative_associative_and_idempotent_per_field() {
        let a = batch(
            1,
            1,
            1,
            patch("entry/read", FieldValue::Set(serde_json::json!(true))),
        );
        let b = batch(
            2,
            1,
            1,
            patch("entry/star", FieldValue::Set(serde_json::json!(true))),
        );
        let c = batch(
            3,
            1,
            1,
            patch("entry/read", FieldValue::Set(serde_json::json!(false))),
        );
        let abc = merged(&[a.clone(), b.clone(), c.clone()]);
        for order in [
            vec![c.clone(), a.clone(), b.clone()],
            vec![b.clone(), c.clone(), a.clone()],
            vec![a.clone(), a.clone(), b.clone(), c.clone(), c.clone()],
        ] {
            assert_eq!(merged(&order), abc);
        }
        let ca = merged(&[a]);
        let cb = merged(&[b]);
        let cc = merged(&[c]);
        assert_eq!(
            ca.merge(&cb).unwrap().merge(&cc).unwrap(),
            ca.merge(&cb.merge(&cc).unwrap()).unwrap()
        );
        assert_eq!(abc.merge(&abc).unwrap(), abc);
        assert_eq!(
            abc.fields["entry/read"].value,
            FieldValue::Set(serde_json::json!(false))
        );
        assert_eq!(
            abc.fields["entry/star"].value,
            FieldValue::Set(serde_json::json!(true))
        );
    }

    #[test]
    fn local_sequence_and_observed_remote_followup_win_without_upload_clock() {
        let first = batch(
            1,
            1,
            1,
            patch("entry/read", FieldValue::Set(serde_json::json!(true))),
        );
        let second = batch(
            1,
            2,
            2,
            patch("entry/read", FieldValue::Set(serde_json::json!(false))),
        );
        let mut replica = Replica::new(&actor(9)).unwrap();
        replica.ingest_batch(&second).unwrap();
        assert_eq!(replica.project().unwrap().pending.len(), 1);
        assert!(replica.checkpoint().unwrap().fields.is_empty());
        replica.ingest_batch(&first).unwrap();
        assert!(replica.project().unwrap().pending.is_empty());
        assert_eq!(
            replica.checkpoint().unwrap().fields["entry/read"].value,
            FieldValue::Set(serde_json::json!(false))
        );
        let followup = replica
            .next_intent(
                &actor(2),
                patch("entry/read", FieldValue::Set(serde_json::json!(true))),
            )
            .unwrap();
        assert_eq!(followup.lamport, 3);
        let third = Batch {
            format: 2,
            library_id: actor(9),
            actor: actor(2),
            intents: vec![followup],
        };
        replica.ingest_batch(&third).unwrap();
        replica.ingest_batch(&first).unwrap(); // late retry is not a new operation
        assert_eq!(
            replica.checkpoint().unwrap().fields["entry/read"]
                .stamp
                .actor,
            actor(2)
        );
        let mut other = Replica::new(&actor(9)).unwrap();
        other.ingest_batch(&third).unwrap();
        assert!(other.checkpoint().unwrap().fields.is_empty());
        other.ingest_batch(&second).unwrap();
        other.ingest_batch(&first).unwrap();
        assert_eq!(other.checkpoint().unwrap(), replica.checkpoint().unwrap());
    }

    #[test]
    fn active_dot_conflict_is_atomic_but_covered_old_seq_is_stale() {
        let original = batch(
            1,
            1,
            1,
            patch("entry/read", FieldValue::Set(serde_json::json!(true))),
        );
        let mut different = original.clone();
        different.intents[0].patch = patch("entry/read", FieldValue::Set(serde_json::json!(false)));
        let mut replica = Replica::new(&actor(9)).unwrap();
        replica.ingest_batch(&original).unwrap();
        let before = replica.checkpoint().unwrap();
        assert_eq!(
            replica.ingest_batch(&different).unwrap_err().kind,
            ErrorKind::Conflict
        );
        assert_eq!(replica.checkpoint().unwrap(), before);
        replica.compact().unwrap();
        assert_eq!(replica.active_count(), 0);
        different.intents[0].lamport = u64::MAX;
        replica.ingest_batch(&different).unwrap();
        assert_eq!(
            replica.checkpoint().unwrap(),
            before,
            "covered old seq must not reapply even if payload/clock differs"
        );
    }

    #[test]
    fn compacted_checkpoints_merge_without_losing_tombstones_or_other_fields() {
        let a = batch(
            1,
            1,
            1,
            patch("settings/font", FieldValue::Set(serde_json::json!(17))),
        );
        let deletion = batch(1, 2, 2, patch("settings/font", FieldValue::Delete));
        let b = batch(
            2,
            1,
            1,
            patch(
                "source/title",
                FieldValue::Set(serde_json::json!("retained")),
            ),
        );
        let left = merged(&[a.clone(), deletion.clone()]);
        let right = merged(std::slice::from_ref(&b));
        let mut replica = Replica::from_checkpoint(left.clone()).unwrap();
        replica.ingest_checkpoint(&right).unwrap();
        replica.ingest_batch(&a).unwrap();
        assert_eq!(replica.checkpoint().unwrap(), merged(&[a, deletion, b]));
        assert_eq!(
            replica.checkpoint().unwrap().fields["settings/font"].value,
            FieldValue::Delete
        );
        assert!(replica.checkpoint().unwrap().dominates(&left).unwrap());
    }

    #[test]
    fn invalid_clock_or_missing_dependency_does_not_advance_frontier() {
        let mut malformed = batch(2, 1, 1, patch("x", FieldValue::Set(serde_json::json!(1))));
        malformed.intents[0].observed.insert(actor(1), 1);
        let prior = batch(1, 1, 2, patch("a", FieldValue::Set(serde_json::json!(1))));
        let mut replica = Replica::new(&actor(9)).unwrap();
        replica.ingest_batch(&prior).unwrap();
        let before = replica.checkpoint().unwrap();
        assert_eq!(
            replica.ingest_batch(&malformed).unwrap_err().kind,
            ErrorKind::InvalidState
        );
        assert_eq!(replica.checkpoint().unwrap(), before);
        let gap = batch(1, 3, 4, patch("x", FieldValue::Delete));
        replica.ingest_batch(&gap).unwrap();
        assert_eq!(
            frontier_seq(&replica.checkpoint().unwrap().covered, &actor(1)),
            1
        );
        assert_eq!(replica.project().unwrap().pending.len(), 1);
    }

    #[test]
    fn checkpoint_space_converges_to_fields_and_actor_frontier() {
        let mut replica = Replica::new(&actor(9)).unwrap();
        for i in 0..1000 {
            let intent = replica
                .next_intent(
                    &actor(1),
                    patch("entry/read", FieldValue::Set(serde_json::json!(i % 2 == 0))),
                )
                .unwrap();
            replica
                .ingest_batch(&Batch {
                    format: 2,
                    library_id: actor(9),
                    actor: actor(1),
                    intents: vec![intent],
                })
                .unwrap();
            replica.compact().unwrap();
        }
        assert_eq!(replica.active_count(), 0);
        assert_eq!(replica.checkpoint().unwrap().fields.len(), 1);
        assert_eq!(replica.checkpoint().unwrap().covered.len(), 1);
        assert!(
            PreparedCheckpoint::new(replica.checkpoint().unwrap())
                .unwrap()
                .bytes()
                .len()
                < 512
        );
    }

    #[test]
    fn checkpoint_same_stamp_conflict_and_uncovered_cleanup_are_rejected() {
        let a = batch(
            1,
            1,
            1,
            patch("read", FieldValue::Set(serde_json::json!(true))),
        );
        let mut replica = Replica::new(&actor(9)).unwrap();
        replica.ingest_batch(&a).unwrap();
        let checkpoint = PreparedCheckpoint::new(replica.checkpoint().unwrap()).unwrap();
        let mut conflict = checkpoint.checkpoint().clone();
        conflict.fields.get_mut("read").unwrap().value = FieldValue::Set(serde_json::json!(false));
        assert_eq!(
            replica.ingest_checkpoint(&conflict).unwrap_err().kind,
            ErrorKind::Conflict
        );
        let url = Url::parse("http://127.0.0.1/probe/").unwrap();
        let stored = StoredCheckpoint {
            id: checkpoint.id().to_owned(),
            file_name: checkpoint.file_name(ProbeId([1; 16])),
            collection_url: url.clone(),
        };
        let prepared = PreparedBatch::new(a).unwrap();
        let first_name = prepared.file_name(ProbeId([2; 16]));
        let mut scan = Scan {
            replica,
            pending_objects: Vec::new(),
            batches: vec![SeenBatch {
                file_name: first_name.clone(),
                prepared,
            }],
            collection_url: url,
        };
        assert_eq!(
            covered_batch_names(&checkpoint, &stored, &scan).unwrap(),
            [first_name]
        );
        let b = batch(
            2,
            1,
            1,
            patch("star", FieldValue::Set(serde_json::json!(true))),
        );
        scan.replica.ingest_batch(&b).unwrap();
        assert_eq!(
            covered_batch_names(&checkpoint, &stored, &scan)
                .unwrap_err()
                .kind,
            ErrorKind::Conflict
        );
    }

    #[test]
    fn checkpoint_checks_live_winner_before_discarding_active_and_failure_is_atomic() {
        let original = batch(
            1,
            1,
            1,
            patch("entry/read", FieldValue::Set(serde_json::json!(true))),
        );
        let mut replica = Replica::new(&actor(9)).unwrap();
        replica.ingest_batch(&original).unwrap();
        let before = replica.project().unwrap();
        let mut conflicting = before.checkpoint.clone();
        conflicting.fields.get_mut("entry/read").unwrap().value =
            FieldValue::Set(serde_json::json!(false));
        conflicting.fields.insert(
            "pollution".to_owned(),
            Register {
                stamp: conflicting.fields["entry/read"].stamp.clone(),
                value: FieldValue::Set(serde_json::json!("must not land")),
            },
        );
        conflicting.validate().unwrap();
        assert_eq!(
            replica.ingest_checkpoint(&conflicting).unwrap_err().kind,
            ErrorKind::Conflict
        );
        assert_eq!(replica.project().unwrap(), before);
        assert_eq!(replica.active_count(), 1);
        replica.ingest_checkpoint(&before.checkpoint).unwrap();
        assert_eq!(replica.checkpoint().unwrap(), before.checkpoint);
        assert_eq!(
            replica.active_count(),
            0,
            "equivalent checkpoint can compact the active dot"
        );
    }

    #[test]
    fn sparse_checkpoint_coverage_preserves_all_known_applied_fields() {
        let first = batch(
            1,
            1,
            1,
            [
                ("x".to_owned(), FieldValue::Set(serde_json::json!(true))),
                ("y".to_owned(), FieldValue::Set(serde_json::Value::Null)),
                ("removed".to_owned(), FieldValue::Delete),
            ]
            .into_iter()
            .collect(),
        );
        for retained_keys in [vec![], vec!["x"], vec!["removed", "y"]] {
            let mut replica = Replica::new(&actor(9)).unwrap();
            replica.ingest_batch(&first).unwrap();
            let before = replica.checkpoint().unwrap();
            let mut sparse = before.clone();
            sparse
                .fields
                .retain(|key, _| retained_keys.contains(&key.as_str()));
            sparse.validate().unwrap();
            replica.ingest_checkpoint(&sparse).unwrap();
            assert_eq!(
                replica.checkpoint().unwrap(),
                before,
                "coverage is not a deletion of omitted fields or tombstones"
            );
            assert_eq!(replica.active_count(), 0);
            assert!(replica.project().unwrap().pending.is_empty());
        }
    }

    #[test]
    fn older_checkpoint_fields_cannot_replace_current_winners_or_other_keys() {
        let first = batch(1, 1, 1, patch("x", FieldValue::Set(serde_json::json!(1))));
        let second = batch(
            1,
            2,
            2,
            [
                ("x".to_owned(), FieldValue::Set(serde_json::json!(2))),
                (
                    "local-only".to_owned(),
                    FieldValue::Set(serde_json::json!(true)),
                ),
            ]
            .into_iter()
            .collect(),
        );
        let remote = batch(
            2,
            1,
            1,
            patch("remote-only", FieldValue::Set(serde_json::json!(true))),
        );
        for claims_newer_coverage in [false, true] {
            let mut replica = Replica::new(&actor(9)).unwrap();
            replica.ingest_batch(&first).unwrap();
            replica.ingest_batch(&second).unwrap();
            let mut older = merged(&[first.clone(), remote.clone()]);
            if claims_newer_coverage {
                older.covered.insert(actor(1), 2);
                older.max_lamport = 2;
            }
            older.validate().unwrap();
            replica.ingest_checkpoint(&older).unwrap();
            assert_eq!(
                replica.checkpoint().unwrap(),
                merged(&[first.clone(), second.clone(), remote.clone()])
            );
            assert_eq!(
                replica.checkpoint().unwrap().fields["x"].value,
                FieldValue::Set(serde_json::json!(2))
            );
        }
    }

    #[test]
    fn checkpoint_join_keeps_pending_until_its_actual_dependencies_arrive() {
        let first = batch(1, 1, 1, patch("x", FieldValue::Set(serde_json::json!(1))));
        let remote = batch(
            2,
            1,
            2,
            patch("dependency", FieldValue::Set(serde_json::json!(true))),
        );
        let mut pending = batch(1, 2, 3, patch("x", FieldValue::Set(serde_json::json!(2))));
        pending.intents[0].observed.insert(actor(2), 1);
        for covers_pending in [false, true] {
            let mut replica = Replica::new(&actor(9)).unwrap();
            replica.ingest_batch(&first).unwrap();
            replica.ingest_batch(&pending).unwrap();
            let mut sparse_old = merged(std::slice::from_ref(&first));
            sparse_old.fields.clear();
            replica.ingest_checkpoint(&sparse_old).unwrap();
            assert_eq!(
                replica.checkpoint().unwrap().fields["x"].value,
                FieldValue::Set(serde_json::json!(1))
            );
            assert_eq!(replica.checkpoint().unwrap().covered[&actor(1)], 1);
            assert_eq!(
                replica.project().unwrap().pending,
                [Dot {
                    actor: actor(1),
                    seq: 2
                }]
            );
            assert_eq!(replica.active_count(), 1);
            let mut supplied = merged(std::slice::from_ref(&remote));
            if covers_pending {
                supplied.covered.insert(actor(1), 2);
                supplied.max_lamport = 3;
                // Local pending bytes are still known even if this checkpoint
                // claims their coverage without carrying the pending field.
            }
            replica.ingest_checkpoint(&supplied).unwrap();
            assert!(replica.project().unwrap().pending.is_empty());
            assert_eq!(
                replica.checkpoint().unwrap(),
                merged(&[first.clone(), remote.clone(), pending.clone()])
            );
        }
    }

    #[test]
    fn checkpoint_cannot_ack_known_pending_dot_without_observed_frontier() {
        let first = batch(
            1,
            1,
            1,
            patch("x", FieldValue::Set(serde_json::json!(true))),
        );
        let mut pending = batch(1, 2, 3, patch("x", FieldValue::Delete));
        pending.intents[0].observed.insert(actor(2), 1);
        let mut replica = Replica::new(&actor(9)).unwrap();
        replica.ingest_batch(&first).unwrap();
        replica.ingest_batch(&pending).unwrap();
        let before = replica.project().unwrap();
        let active_before = replica.active_count();
        let mut missing_dependency = before.checkpoint.clone();
        missing_dependency.covered.insert(actor(1), 2);
        missing_dependency.max_lamport = 3;
        missing_dependency.fields.clear();
        missing_dependency.validate().unwrap();
        assert_eq!(
            replica
                .ingest_checkpoint(&missing_dependency)
                .unwrap_err()
                .kind,
            ErrorKind::InvalidState
        );
        assert_eq!(replica.project().unwrap(), before);
        assert_eq!(replica.active_count(), active_before);
    }

    #[test]
    fn checkpoint_assisted_pending_join_keeps_clock_and_value_conflicts_atomic() {
        let first = batch(
            1,
            1,
            1,
            patch("x", FieldValue::Set(serde_json::json!(true))),
        );
        let mut pending = batch(1, 2, 3, patch("x", FieldValue::Delete));
        pending.intents[0].observed.insert(actor(2), 1);
        for wrong_clock in [false, true] {
            let mut replica = Replica::new(&actor(9)).unwrap();
            replica.ingest_batch(&first).unwrap();
            replica.ingest_batch(&pending).unwrap();
            let before = replica.project().unwrap();
            let mut incoming = merged(&[batch(
                2,
                1,
                if wrong_clock { 100 } else { 2 },
                patch("dependency", FieldValue::Set(serde_json::json!(true))),
            )]);
            incoming.covered.insert(actor(1), 2);
            incoming.max_lamport = incoming.max_lamport.max(3);
            if !wrong_clock {
                incoming.fields.insert(
                    "x".to_owned(),
                    Register {
                        stamp: Stamp {
                            actor: actor(1),
                            seq: 2,
                            lamport: 3,
                        },
                        value: FieldValue::Set(serde_json::json!(false)),
                    },
                );
            }
            incoming.validate().unwrap();
            assert_eq!(
                replica.ingest_checkpoint(&incoming).unwrap_err().kind,
                if wrong_clock {
                    ErrorKind::InvalidState
                } else {
                    ErrorKind::Conflict
                }
            );
            assert_eq!(replica.project().unwrap(), before);
            assert_eq!(replica.active_count(), 2);
        }
    }

    #[test]
    fn explicit_null_and_delete_batches_round_trip_to_distinct_bytes_and_ids() {
        let original = batch(
            1,
            1,
            1,
            patch(
                "settings/nullable",
                FieldValue::Set(serde_json::Value::Null),
            ),
        );
        let deletion = batch(1, 1, 1, patch("settings/nullable", FieldValue::Delete));
        let set = PreparedBatch::new(original.clone()).unwrap();
        let remove = PreparedBatch::new(deletion.clone()).unwrap();
        assert_ne!(set.bytes(), remove.bytes());
        assert_ne!(set.id(), remove.id());
        let decoded: Batch = serde_json::from_slice(set.bytes()).unwrap();
        let decoded_deletion: Batch = serde_json::from_slice(remove.bytes()).unwrap();
        assert_eq!(decoded, original);
        assert_eq!(decoded_deletion, deletion);
        assert_eq!(
            PreparedBatch::new(decoded.clone()).unwrap().bytes(),
            set.bytes()
        );
        assert_eq!(PreparedBatch::new(decoded.clone()).unwrap().id(), set.id());
        let mut local = Replica::new(&actor(9)).unwrap();
        local.ingest_batch(&original).unwrap();
        local.ingest_batch(&decoded).unwrap();
        assert_eq!(
            local.checkpoint().unwrap().fields["settings/nullable"].value,
            FieldValue::Set(serde_json::Value::Null)
        );
        assert!(
            serde_json::from_value::<FieldValue>(serde_json::Value::Null).is_err(),
            "untagged legacy null is ambiguous"
        );
    }

    #[test]
    fn explicit_null_and_delete_checkpoint_values_round_trip() {
        let values = [
            (
                "nullable".to_owned(),
                FieldValue::Set(serde_json::Value::Null),
            ),
            ("removed".to_owned(), FieldValue::Delete),
            (
                "nested".to_owned(),
                FieldValue::Set(serde_json::json!({"value":null})),
            ),
        ]
        .into_iter()
        .collect();
        let original = batch(1, 1, 1, values);
        let checkpoint = merged(&[original]);
        let prepared = PreparedCheckpoint::new(checkpoint.clone()).unwrap();
        let decoded: Checkpoint = serde_json::from_slice(prepared.bytes()).unwrap();
        assert_eq!(decoded, checkpoint);
        let prepared_again = PreparedCheckpoint::new(decoded.clone()).unwrap();
        assert_eq!(prepared_again.bytes(), prepared.bytes());
        assert_eq!(prepared_again.id(), prepared.id());
        let restored = Replica::from_checkpoint(decoded).unwrap();
        assert_eq!(
            restored.checkpoint().unwrap().fields["nullable"].value,
            FieldValue::Set(serde_json::Value::Null)
        );
        assert_eq!(
            restored.checkpoint().unwrap().fields["removed"].value,
            FieldValue::Delete
        );
    }

    #[test]
    fn observed_winner_rejects_bad_new_clock_before_and_after_compaction() {
        let first = batch(
            1,
            1,
            100,
            patch("read", FieldValue::Set(serde_json::json!(true))),
        );
        let invalid = batch(
            1,
            2,
            2,
            [
                ("read".to_owned(), FieldValue::Set(serde_json::json!(false))),
                (
                    "pollution".to_owned(),
                    FieldValue::Set(serde_json::json!(true)),
                ),
            ]
            .into_iter()
            .collect(),
        );
        invalid.validate().unwrap();
        for compact in [false, true] {
            let mut replica = Replica::new(&actor(9)).unwrap();
            replica.ingest_batch(&first).unwrap();
            if compact {
                replica.compact().unwrap();
            }
            let before = replica.project().unwrap();
            let active_before = replica.active_count();
            assert_eq!(
                replica.ingest_batch(&invalid).unwrap_err().kind,
                ErrorKind::InvalidState
            );
            assert_eq!(replica.project().unwrap(), before);
            assert_eq!(replica.active_count(), active_before);
            assert_eq!(
                frontier_seq(&replica.checkpoint().unwrap().covered, &actor(1)),
                1
            );
            assert!(!replica
                .checkpoint()
                .unwrap()
                .fields
                .contains_key("pollution"));
        }
    }

    #[test]
    fn retained_causal_evidence_is_checked_without_rejecting_lower_concurrent_clocks() {
        let first = batch(
            1,
            1,
            100,
            patch("read", FieldValue::Set(serde_json::json!(true))),
        );
        let valid_concurrent = batch(
            2,
            1,
            1,
            patch("star", FieldValue::Set(serde_json::json!(true))),
        );
        for compact in [false, true] {
            let mut replica = Replica::new(&actor(9)).unwrap();
            replica.ingest_batch(&first).unwrap();
            if compact {
                replica.compact().unwrap();
            }
            replica.ingest_batch(&valid_concurrent).unwrap();
            assert_eq!(
                replica.checkpoint().unwrap().fields["star"].stamp.lamport,
                1
            );
            let mut invalid = batch(
                3,
                1,
                2,
                patch("pollution", FieldValue::Set(serde_json::json!(true))),
            );
            invalid.intents[0].observed.insert(actor(1), 1);
            let before = replica.project().unwrap();
            assert_eq!(
                replica.ingest_batch(&invalid).unwrap_err().kind,
                ErrorKind::InvalidState
            );
            assert_eq!(replica.project().unwrap(), before);
        }
    }

    #[test]
    fn xml_namespaces_entities_and_budgets_fail_closed() {
        for invalid in [
            "<!DOCTYPE x [<!ENTITY a SYSTEM 'file:///secret'>]><D:prop xmlns:D='DAV:'>&a;</D:prop>"
                .to_owned(),
            "<D:prop xmlns:D='DAV:'>&custom;</D:prop>".to_owned(),
            "<D:prop xmlns:D='DAV:'>&#0;</D:prop>".to_owned(),
            "<D:prop xmlns:D='DAV:'/><D:prop xmlns:D='DAV:'/>".to_owned(),
            "<D:prop xmlns:D='DAV:'><?external data?></D:prop>".to_owned(),
            "<D:prop xmlns:D='DAV:'>".repeat(33) + &"</D:prop>".repeat(33),
            "<D:prop xmlns:D='DAV:'>".to_owned() + &"<D:x/>".repeat(XML_NODES) + "</D:prop>",
        ] {
            assert!(parse_xml(invalid.as_bytes()).is_err());
        }
        let doc =
            parse_xml(b"<a:prop xmlns:a='DAV:'><a:href>a&amp;b&#47;c</a:href></a:prop>").unwrap();
        assert_eq!(
            doc.text(doc.child(doc.root("prop").unwrap(), "href").unwrap())
                .unwrap(),
            "a&b/c"
        );
        assert!(parse_xml(b"<a:prop xmlns:a='urn:wrong'/>")
            .unwrap()
            .root("prop")
            .is_err());
        assert!(parse_xml(b"<a:prop/>").is_err());
    }
}
