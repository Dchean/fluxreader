use super::{
    database,
    model::*,
    storage::{self, Budget, Manifest, Pending, ROOT},
    transport_error,
};
use crate::{
    db,
    error::{AppError, AppResult},
    state::AppState,
    webdav_transport::{Credentials, Transport},
};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tauri::{Emitter, Manager, State};
use tokio::sync::{Mutex, Notify};

#[derive(Default)]
pub struct LocalRuntime {
    pub state: Mutex<()>,
    pub body: Mutex<()>,
    pub portable: Mutex<()>,
    pub fast: Mutex<()>,
    fast_receive: Mutex<()>,
    marker_gate: Mutex<()>,
    actors_gate: Mutex<()>,
    pacing: Arc<Mutex<FastPacing>>,
    pub recovery: Mutex<()>,
    fast_wakeup: Notify,
    state_wakeup: Notify,
    stopped: Arc<AtomicBool>,
}
impl LocalRuntime {
    pub fn notify_local_commit(&self) {
        self.fast_wakeup.notify_one();
    }
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        self.fast_wakeup.notify_one();
    }
    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }
    pub async fn drain(&self, budget: Duration) -> bool {
        self.stop();
        tokio::time::timeout(budget, async {
            let _state = self.state.lock().await;
            let _body = self.body.lock().await;
            let _portable = self.portable.lock().await;
            let _fast = self.fast.lock().await;
            let _receive = self.fast_receive.lock().await;
            let _recovery = self.recovery.lock().await;
        })
        .await
        .is_ok()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    endpoint: String,
    username: String,
    mode: String,
    interval_minutes: u32,
}

#[derive(Clone)]
pub(super) struct Session {
    pub(super) generation: i64,
    pub(super) library: String,
    pub(super) actor: String,
    pub(super) transport: Transport,
    pub(super) stopped: Arc<AtomicBool>,
}

#[derive(Default)]
struct Due {
    next: Option<Instant>,
    failures: u32,
}
impl Due {
    fn ready(&self, now: Instant) -> bool {
        self.next.map_or(true, |next| now >= next)
    }
    fn success(&mut self, now: Instant, delay: Duration) {
        self.failures = 0;
        self.next = Some(now + delay);
    }
    fn failure(&mut self, now: Instant, salt: &str) {
        self.failures = self.failures.saturating_add(1);
        let seconds = (5_u64 << self.failures.saturating_sub(1).min(4)).min(60);
        let jitter = salt.bytes().map(u64::from).sum::<u64>() % 251;
        self.next = Some(now + Duration::from_millis(seconds * 1000 + jitter));
    }
}
#[derive(Default)]
struct FastPacing {
    scope: Option<(i64, String, String)>,
    marker: Due,
    marker_valid: bool,
    actors: Due,
    send: Due,
    peers: BTreeMap<String, Due>,
    peer_cursors: BTreeMap<String, super::fast::PeerCursor>,
    cursor: String,
    manifests: BTreeMap<(String, String), Instant>,
    urgent: BTreeSet<String>,
    directories_ready: bool,
}
const MAX_PENDING_PEERS: usize = 64;
impl FastPacing {
    fn observe_current(&mut self, conn: &Connection, session: &Session) -> AppResult<()> {
        check_session(conn, session)?;
        if self
            .scope
            .as_ref()
            .is_some_and(|scope| scope.0 > session.generation)
        {
            return Err(AppError::new("staleLocalSync", "同步会话身份已变化"));
        }
        self.observe(session);
        Ok(())
    }
    fn observe(&mut self, session: &Session) {
        let scope = (
            session.generation,
            session.library.clone(),
            session.actor.clone(),
        );
        if self.scope.as_ref() != Some(&scope) {
            *self = Self {
                scope: Some(scope),
                ..Self::default()
            };
        }
    }
    fn peers_due(&mut self, now: Instant) -> Vec<String> {
        let mut peers: Vec<_> = self
            .peers
            .iter()
            .filter(|(peer, due)| {
                due.ready(now)
                    && (self.peer_cursors.len() < MAX_PENDING_PEERS
                        || self.peer_cursors.contains_key(*peer))
            })
            .map(|(peer, _)| peer.clone())
            .collect();
        peers.sort_by_key(|peer| (peer <= &self.cursor, peer.clone()));
        peers.truncate(8);
        peers
    }
}

#[derive(Clone, Copy)]
pub enum FastPhase {
    Send,
    Discover,
}

/// A separate SQLite connection keeps CPU-heavy synchronization preparation off
/// the foreground async mutex. WAL readers keep browsing during background writes.
/// In-memory unit tests intentionally keep their one isolated connection.
pub(crate) async fn background_database(
    database: &Arc<Mutex<Connection>>,
) -> AppResult<Arc<Mutex<Connection>>> {
    let path = blocking(database.clone(), |conn| {
        Ok(conn
            .path()
            .filter(|path| !path.is_empty())
            .map(std::path::PathBuf::from))
    })
    .await?;
    let Some(path) = path else {
        return Ok(database.clone());
    };
    let connection = tokio::task::spawn_blocking(move || -> AppResult<Connection> {
        let conn = Connection::open(path)?;
        database::register_functions(&conn)?;
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        Ok(conn)
    })
    .await
    .map_err(|_| AppError::internal("同步数据库工作线程未完成"))??;
    Ok(Arc::new(Mutex::new(connection)))
}
pub(crate) async fn blocking<T: Send + 'static>(
    database: Arc<Mutex<Connection>>,
    work: impl FnOnce(&Connection) -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    crate::state::database_work(database, move |conn| work(conn)).await
}

/// The startup worker and disk recovery tests use this same credential-free
/// one-slice entry. Never call initialize's synchronous drain from this worker.
pub async fn recover_local_slice(
    database: &Arc<Mutex<Connection>>,
    runtime: &LocalRuntime,
) -> AppResult<bool> {
    let _guard = runtime.recovery.lock().await;
    let stop = runtime.stopped.clone();
    blocking(database.clone(), move |conn| {
        if stop.load(Ordering::Acquire) {
            return Err(AppError::new("cancelled", "同步已停止"));
        }
        db::with_write_cancellation(stop, || database::resume_local_slice(conn))
    })
    .await
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Marker {
    format: u32,
    library_id: String,
    required: BTreeSet<String>,
}
fn required_capabilities() -> BTreeSet<String> {
    ["intent-batches-v1", "complete-receipts-v1"]
        .into_iter()
        .map(str::to_owned)
        .collect()
}
impl Marker {
    fn validate(&self) -> AppResult<()> {
        if self.format != FORMAT
            || !valid_hex(&self.library_id, 32)
            || self.required != required_capabilities()
        {
            return Err(invalid("云端库能力不兼容；原文件已保留"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod capability_tests {
    use super::*;
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct OldMarker {
        #[serde(rename = "format")]
        _format: u32,
        #[serde(rename = "library_id")]
        _library_id: String,
    }
    #[test]
    fn r2_marker_requires_exact_capabilities_and_old_reader_rejects_them() {
        let marker = Marker {
            format: FORMAT,
            library_id: "1".repeat(32),
            required: required_capabilities(),
        };
        marker.validate().unwrap();
        let raw = serde_json::to_value(&marker).unwrap();
        assert!(serde_json::from_value::<OldMarker>(raw.clone()).is_err());
        let mut old = raw.clone();
        old.as_object_mut().unwrap().remove("required");
        assert!(serde_json::from_value::<Marker>(old).is_err());
        for required in [
            BTreeSet::new(),
            BTreeSet::from(["intent-batches-v1".to_owned()]),
            BTreeSet::from(["complete-receipts-v1".to_owned()]),
            BTreeSet::from([
                "intent-batches-v1".to_owned(),
                "complete-receipts-v1".to_owned(),
                "unknown".to_owned(),
            ]),
        ] {
            assert!(Marker {
                required,
                ..Marker {
                    format: FORMAT,
                    library_id: "1".repeat(32),
                    required: BTreeSet::new()
                }
            }
            .validate()
            .is_err());
        }
    }
    #[test]
    fn r2_manifest_rejects_missing_or_impossible_completion() {
        let mut manifest = Manifest {
            format: FORMAT,
            library_id: "1".repeat(32),
            actor: "2".repeat(32),
            publication: 1,
            kind: "state".into(),
            covered: std::collections::BTreeMap::from([("2".repeat(32), 10)]),
            completed: std::collections::BTreeMap::from([("2".repeat(32), 2)]),
            max_lamport: 20,
            shards: Vec::new(),
            deltas: 0,
        };
        manifest.validate().unwrap();
        let mut raw = serde_json::to_value(&manifest).unwrap();
        raw.as_object_mut().unwrap().remove("completed");
        assert!(serde_json::from_value::<Manifest>(raw).is_err());
        manifest.completed.insert("2".repeat(32), 11);
        assert!(manifest.validate().is_err());
        manifest.completed = std::collections::BTreeMap::from([("3".repeat(32), 1)]);
        assert!(manifest.validate().is_err());
    }
}

pub fn configured(conn: &Connection) -> AppResult<bool> {
    Ok(db::get_setting(conn, "local_sync_config")?.is_some_and(|v| !v.is_empty()))
}
fn read_config(conn: &Connection) -> AppResult<Option<Config>> {
    db::get_setting(conn, "local_sync_config")?
        .filter(|s| !s.is_empty())
        .map(|raw| serde_json::from_str(&raw).map_err(Into::into))
        .transpose()
}
fn session(conn: &Connection) -> AppResult<Option<Session>> {
    let Some(config) = read_config(conn)? else {
        return Ok(None);
    };
    if !conn.query_row(
        "SELECT initialized FROM local_sync_control WHERE id=1",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(AppError::new(
            "localSyncPending",
            "本机同步索引尚未建立完成",
        ));
    }
    if crate::sync::read_credentials(conn)?.is_some() {
        return Err(AppError::new(
            "localSyncMode",
            "WebDAV 仅支持纯本地模式；请先断开 RSS 服务连接",
        ));
    }
    let password = db::get_setting(conn, "local_sync_password")?
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::new("localSyncCredentials", "此设备需要重新填写 WebDAV 凭据"))?;
    let library: String = conn.query_row(
        "SELECT library_id FROM local_sync_control WHERE id=1",
        [],
        |r| r.get(0),
    )?;
    Ok(Some(Session {
        generation: database::generation(conn)?,
        library,
        actor: database::actor(conn)?,
        transport: Transport::new(
            &config.endpoint,
            Some(Credentials {
                username: config.username,
                password,
            }),
            Duration::from_secs(30),
        )
        .map_err(transport_error)?,
        stopped: Arc::new(AtomicBool::new(false)),
    }))
}
pub fn status(conn: &Connection) -> AppResult<LocalSyncStatus> {
    let config = read_config(conn)?;
    let (library,generation,last_state,last_body,next_retry,last_error):(String,i64,i64,i64,i64,Option<String>)=conn.query_row("SELECT library_id,generation,last_state_sync,last_body_sync,next_retry_at,last_error FROM local_sync_control WHERE id=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)))?;
    let pending: i64 =
        conn.query_row("SELECT COUNT(*) FROM local_sync_journal WHERE seq>(SELECT published_seq FROM local_sync_control WHERE id=1)", [], |r| r.get(0))?;
    let pending_publications: i64 =
        conn.query_row("SELECT COUNT(*) FROM local_sync_pending", [], |r| r.get(0))?;
    Ok(LocalSyncStatus {
        connected: config.is_some(),
        endpoint: config.as_ref().map(|c| c.endpoint.clone()),
        username: config.as_ref().map(|c| c.username.clone()),
        mode: config
            .as_ref()
            .map(|c| c.mode.clone())
            .unwrap_or_else(|| "scheduled".into()),
        interval_minutes: config.as_ref().map(|c| c.interval_minutes).unwrap_or(15),
        generation,
        library_id: library,
        pending_operations: pending,
        pending_bodies: database::pending_bodies(conn)?,
        pending_publications,
        last_state_sync: last_state,
        last_body_sync: last_body,
        next_retry_at: next_retry,
        last_error: db::get_setting(conn, "local_sync_recovery_error")?
            .or(db::get_setting(conn, "local_sync_fast_send_error")?)
            .or(db::get_setting(conn, "local_sync_fast_receive_error")?)
            .or(db::get_setting(conn, "local_sync_fast_discovery_error")?)
            .or(last_error),
    })
}
#[derive(Serialize)]
pub struct LocalSyncStatus {
    pub connected: bool,
    pub endpoint: Option<String>,
    pub username: Option<String>,
    pub mode: String,
    pub interval_minutes: u32,
    pub generation: i64,
    pub library_id: String,
    pub pending_operations: i64,
    pub pending_bodies: i64,
    pub pending_publications: i64,
    pub last_state_sync: i64,
    pub last_body_sync: i64,
    pub next_retry_at: i64,
    pub last_error: Option<String>,
}
#[tauri::command]
pub async fn local_sync_status(state: State<'_, AppState>) -> AppResult<LocalSyncStatus> {
    blocking(state.db.clone(), status).await
}

pub async fn connect(
    database: &Arc<Mutex<Connection>>,
    endpoint: String,
    username: String,
    password: String,
    mode: String,
    interval_minutes: u32,
) -> AppResult<LocalSyncStatus> {
    if !matches!(mode.as_str(), "realtime" | "scheduled")
        || !(1..=1440).contains(&interval_minutes)
        || password.is_empty()
    {
        return Err(AppError::new(
            "validate",
            "请选择同步方式、1–1440 分钟间隔并填写 WebDAV 密码",
        ));
    }
    let transport = Transport::new(
        &endpoint,
        Some(Credentials {
            username: username.clone(),
            password: password.clone(),
        }),
        Duration::from_secs(30),
    )
    .map_err(transport_error)?;
    let (expected, rss_generation, random) = blocking(database.clone(), |conn| {
        if crate::sync::read_credentials(conn)?.is_some() {
            return Err(AppError::new(
                "localSyncMode",
                "WebDAV 仅支持纯本地模式；请先断开 RSS 服务连接",
            ));
        }
        Ok((
            database::generation(conn)?,
            db::sync_generation(conn)?,
            storage::random_id(conn)?,
        ))
    })
    .await?;
    transport
        .library_mkdir(ROOT)
        .await
        .map_err(transport_error)?;
    let path = format!("{ROOT}library.json");
    let marker = match transport
        .library_get(&path, 4096)
        .await
        .map_err(transport_error)?
    {
        Some(bytes) => serde_json::from_slice::<Marker>(&bytes)?,
        None => {
            let marker = Marker {
                format: FORMAT,
                library_id: random,
                required: required_capabilities(),
            };
            let bytes = serde_json::to_vec(&marker)?;
            if transport.library_store(&path, &bytes).await.is_err() {
                let bytes = transport
                    .library_get(&path, 4096)
                    .await
                    .map_err(transport_error)?
                    .ok_or_else(|| {
                        AppError::new(
                            "localSyncUnknown",
                            "建库结果未确认；请重试连接，原数据保持不变",
                        )
                    })?;
                serde_json::from_slice::<Marker>(&bytes)?
            } else {
                marker
            }
        }
    };
    marker.validate()?;
    transport
        .library_mkdir(&format!("{ROOT}actors/"))
        .await
        .map_err(transport_error)?;
    let isolated = background_database(database).await?;
    blocking(isolated, move |conn| {
    database::require_generation(conn, expected)?;
    if db::sync_generation(conn)? != rss_generation
        || crate::sync::read_credentials(conn)?.is_some()
    {
        return Err(AppError::new(
            "staleLocalSync",
            "连接验证期间 RSS 账号已变化，本次 WebDAV 连接未保存",
        ));
    }
    // Initialize first, transactionally. Credential encryption occurs before any
    // connection commit, and its failure never stores plaintext or an active flag.
    let encrypted = crate::credentials::encrypt_secret(&password)?;
    database::initialize(conn, &marker.library_id)?;
    let tx = crate::db::write_transaction(conn)?;
    database::require_generation(&tx, expected)?;
    if db::sync_generation(&tx)? != rss_generation || crate::sync::read_credentials(&tx)?.is_some()
    {
        return Err(AppError::new(
            "staleLocalSync",
            "建立同步索引期间 RSS 账号已变化，本次 WebDAV 连接未保存",
        ));
    }
    db::set_setting(
        &tx,
        "local_sync_config",
        &serde_json::to_string(&Config {
            endpoint,
            username,
            mode,
            interval_minutes,
        })?,
    )?;
    tx.execute("INSERT INTO settings(key,value) VALUES('local_sync_password',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[encrypted])?;
    tx.execute("UPDATE local_sync_control SET generation=generation+1,failures=0,next_retry_at=0,last_error=NULL WHERE id=1",[])?;
    // Sealed operations still live in fields/journal. Old session's publication
    // attempts cannot commit after this generation changes.
    tx.execute("DELETE FROM local_sync_pending", [])?;
    tx.execute(
        "UPDATE local_sync_lanes SET last_sync=0,next_retry=0,failures=0,error=NULL",
        [],
    )?;
    tx.commit()?;
    status(conn)
    }).await
}
#[tauri::command]
pub async fn local_sync_connect(
    state: State<'_, AppState>,
    endpoint: String,
    username: String,
    password: String,
    mode: String,
    interval_minutes: u32,
) -> AppResult<LocalSyncStatus> {
    connect(
        &state.db,
        endpoint,
        username,
        password,
        mode,
        interval_minutes,
    )
    .await
}

pub fn disconnect(conn: &Connection) -> AppResult<LocalSyncStatus> {
    let tx = crate::db::write_transaction(conn)?;
    tx.execute(
        "DELETE FROM settings WHERE key IN ('local_sync_config','local_sync_password')",
        [],
    )?;
    tx.execute("UPDATE local_sync_control SET generation=generation+1,failures=0,next_retry_at=0,last_error=NULL WHERE id=1",[])?;
    tx.execute("DELETE FROM local_sync_pending", [])?;
    tx.execute(
        "UPDATE local_sync_lanes SET last_sync=0,next_retry=0,failures=0,error=NULL",
        [],
    )?;
    tx.commit()?;
    status(conn)
}
#[tauri::command]
pub async fn local_sync_disconnect(state: State<'_, AppState>) -> AppResult<LocalSyncStatus> {
    blocking(state.db.clone(), disconnect).await
}

pub(super) async fn ensure_session(
    database: &Arc<Mutex<Connection>>,
    session: &Session,
) -> AppResult<()> {
    let session = session.clone();
    blocking(database.clone(), move |conn| check_session(conn, &session)).await
}

// The original session is checked after both locks, including for HTTP result
// completion. No old round may reset a newer pacing scope or return its cursor.
async fn pacing_work<T: Send + 'static>(
    database: &Arc<Mutex<Connection>>,
    runtime: &LocalRuntime,
    session: &Session,
    work: impl FnOnce(&Connection, &mut FastPacing, &Session) -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    let pacing = runtime.pacing.clone();
    let session = session.clone();
    blocking(database.clone(), move |conn| {
        let mut pacing = pacing.blocking_lock();
        pacing.observe_current(conn, &session)?;
        work(conn, &mut pacing, &session)
    })
    .await
}
pub(super) fn check_session(conn: &Connection, session: &Session) -> AppResult<()> {
    if session.stopped.load(Ordering::Acquire) {
        return Err(AppError::new(
            "cancelled",
            "同步已停止，未完成操作保留待重试",
        ));
    }
    let valid: bool = conn.query_row(
        "SELECT generation=?1 AND library_id=?2 AND actor=?3 FROM local_sync_control WHERE id=1",
        params![session.generation, session.library, session.actor],
        |r| r.get(0),
    )?;
    if !valid {
        return Err(AppError::new("staleLocalSync", "同步会话身份已变化"));
    }
    Ok(())
}
pub(super) async fn transport_guard(
    database: &Arc<Mutex<Connection>>,
    session: &Session,
) -> crate::webdav_transport::Result<()> {
    ensure_session(database, session).await.map_err(|error| {
        crate::webdav_transport::TransportError {
            kind: crate::webdav_transport::ErrorKind::LeaseExpired,
            message: error.to_string(),
        }
    })
}
async fn verify_marker(session: &Session, budget: &mut Budget) -> AppResult<()> {
    budget.take(1)?;
    let bytes = session
        .transport
        .library_get(&format!("{ROOT}library.json"), 4096)
        .await
        .map_err(transport_error)?
        .ok_or_else(|| {
            AppError::new(
                "localSyncMissingLibrary",
                "云端库标识已丢失；保留本地数据，需恢复云端库后重试",
            )
        })?;
    let marker: Marker = serde_json::from_slice(&bytes)?;
    marker.validate()?;
    if marker.library_id != session.library {
        return Err(invalid("云端库标识已改变；本轮数据未应用"));
    }
    Ok(())
}

async fn marker_due(
    database: &Arc<Mutex<Connection>>,
    runtime: &LocalRuntime,
    session: &Session,
    budget: &mut Budget,
    deadline: Option<Instant>,
) -> AppResult<bool> {
    let _guard = runtime.marker_gate.lock().await;
    let cached = pacing_work(database, runtime, session, |_, pacing, _| {
        if !pacing.marker.ready(Instant::now()) {
            return if pacing.marker_valid {
                Ok(Some(true))
            } else {
                Err(AppError::new("localSyncPending", "云端标识等待重验"))
            };
        }
        Ok(None)
    })
    .await?;
    if let Some(valid) = cached {
        return Ok(valid);
    }
    ensure_session(database, session).await?;
    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
        return Ok(false);
    }
    // A scheduling slice controls admission of the next request, not the
    // lifetime of a request already admitted. Transport owns its 30s timeout.
    let result = verify_marker(session, budget).await;
    finish_marker(database, runtime, session, result).await
}

async fn finish_marker(
    database: &Arc<Mutex<Connection>>,
    runtime: &LocalRuntime,
    session: &Session,
    result: AppResult<()>,
) -> AppResult<bool> {
    pacing_work(database, runtime, session, move |_, pacing, session| {
        if result.is_ok() {
            pacing
                .marker
                .success(Instant::now(), Duration::from_secs(30));
            pacing.marker_valid = true;
        } else {
            pacing.marker.failure(Instant::now(), &session.actor);
            pacing.marker_valid = false;
        }
        result.map(|()| true)
    })
    .await
}

async fn save_fast_error(
    database: &Arc<Mutex<Connection>>,
    session: &Session,
    lane: &str,
    error: Option<&AppError>,
) -> AppResult<()> {
    ensure_session(database, session).await?;
    let session = session.clone();
    let key = format!("local_sync_fast_{lane}_error");
    let error = error.map(ToString::to_string);
    blocking(database.clone(), move |conn| {
        let _writer = db::writer_guard(conn)?;
        check_session(conn, &session)?;
        if let Some(error) = error {
            db::set_setting(conn, &key, &error)?;
        } else {
            conn.execute("DELETE FROM settings WHERE key=?1", [key])?;
        }
        Ok(())
    })
    .await
}

async fn actors_due(
    database: &Arc<Mutex<Connection>>,
    runtime: &LocalRuntime,
    session: &Session,
    budget: &mut Budget,
    deadline: Option<Instant>,
) -> AppResult<Vec<String>> {
    let _guard = runtime.actors_gate.lock().await;
    let cached = pacing_work(database, runtime, session, |conn, pacing, session| {
        let mut stmt = conn.prepare("SELECT actor FROM local_sync_known_actors ORDER BY actor")?;
        let known = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for peer in known {
            if peer != session.actor {
                pacing.peers.entry(peer).or_default();
            }
        }
        if !pacing.actors.ready(Instant::now()) {
            return Ok(Some(pacing.peers.keys().cloned().collect()));
        }
        Ok(None)
    })
    .await?;
    if let Some(peers) = cached {
        return Ok(peers);
    }
    ensure_session(database, session).await?;
    if deadline.is_some_and(|deadline| Instant::now() >= deadline) || budget.remaining() == 0 {
        return pacing_work(database, runtime, session, |_, pacing, _| {
            Ok(pacing.peers.keys().cloned().collect())
        })
        .await;
    }
    budget.take(1)?;
    let actor_path = format!("{ROOT}actors/");
    let result = session.transport.library_list(&actor_path).await;
    ensure_session(database, session).await?;
    let result = result.map_err(transport_error).and_then(|actors| {
        if actors.len() > 64 || actors.iter().any(|actor| !valid_hex(actor, 32)) {
            Err(invalid("设备目录数量或身份无效"))
        } else {
            Ok(actors)
        }
    });
    if let Ok(actors) = &result {
        let actors = actors.clone();
        let guard = session.clone();
        blocking(database.clone(), move |conn| {
            let tx = db::write_transaction(conn)?;
            check_session(&tx, &guard)?;
            for actor in &actors {
                super::logical::register_actor(&tx, actor)?;
            }
            tx.commit()?;
            Ok(())
        })
        .await?;
    }
    // DB → pacing, with no await while either lock is held. The original token
    // is checked after both locks, before an old round can replace a newer scope.
    let pacing = runtime.pacing.clone();
    let guard = session.clone();
    let (peers, result) = blocking(database.clone(), move |conn| {
        let mut pacing = pacing.blocking_lock();
        let peers = finish_actor_discovery(conn, &mut pacing, &guard, &result)?;
        Ok((peers, result))
    })
    .await?;
    save_fast_error(database, session, "discovery", result.as_ref().err()).await?;
    match result {
        Err(error) if peers.is_empty() => Err(error),
        _ => Ok(peers),
    }
}

fn finish_actor_discovery(
    conn: &Connection,
    pacing: &mut FastPacing,
    session: &Session,
    result: &AppResult<Vec<String>>,
) -> AppResult<Vec<String>> {
    pacing.observe_current(conn, session)?;
    match result {
        Ok(actors) => {
            for actor in actors {
                if actor != &session.actor {
                    pacing.peers.entry(actor.clone()).or_default();
                }
            }
            pacing
                .actors
                .success(Instant::now(), Duration::from_secs(15));
        }
        Err(_) => pacing.actors.failure(Instant::now(), &session.actor),
    }
    Ok(pacing.peers.keys().cloned().collect())
}

/// Shared production entry for native tasks, manual/state requests and the HTTP
/// fixture. Separate gates mean peer GET latency cannot consume send permission.
/// Cold/idle discovery uses the same 1s peer due interval as active operation.
pub async fn run_fast_round(
    database: &Arc<Mutex<Connection>>,
    runtime: &LocalRuntime,
    phase: FastPhase,
) -> AppResult<()> {
    let gate = match phase {
        FastPhase::Send => &runtime.fast,
        FastPhase::Discover => &runtime.fast_receive,
    };
    let Ok(_guard) = gate.try_lock() else {
        return Ok(());
    };
    let Some(mut session) = blocking(database.clone(), session).await? else {
        return Ok(());
    };
    session.stopped = runtime.stopped.clone();
    pacing_work(database, runtime, &session, |_, _, _| Ok(())).await?;
    let mut budget = Budget::limited(16);
    match phase {
        FastPhase::Send => {
            if !pacing_work(database, runtime, &session, |conn, _, _| {
                super::fast::has_outgoing(conn)
            })
            .await?
            {
                return Ok(());
            }
            let ready = pacing_work(database, runtime, &session, |_, pacing, _| {
                Ok(pacing.send.ready(Instant::now()))
            })
            .await?;
            if !ready {
                return Ok(());
            }
            let result = async {
                marker_due(database, runtime, &session, &mut budget, None).await?;
                let dirs = pacing_work(database, runtime, &session, |_, pacing, _| {
                    Ok(pacing.directories_ready)
                })
                .await?;
                super::fast::send(database, &session, &mut budget, dirs).await
            }
            .await;
            let result = finish_send(database, runtime, &session, result).await?;
            save_fast_error(database, &session, "send", result.as_ref().err()).await?;
            result.map(|_| ())
        }
        FastPhase::Discover => {
            let deadline = Instant::now() + Duration::from_millis(800);
            // A 100ms local wake checks these timestamps only. Healthy idle does
            // not call GET/PROPFIND until its explicit deadline is due.
            let due = pacing_work(database, runtime, &session, |_, pacing, _| {
                Ok(pacing.actors.ready(Instant::now())
                    || pacing.marker.ready(Instant::now())
                    || pacing.peers.values().any(|p| p.ready(Instant::now())))
            })
            .await?;
            if !due {
                return Ok(());
            }
            if pacing_work(database, runtime, &session, |_, pacing, _| {
                Ok(!pacing.marker_valid && !pacing.marker.ready(Instant::now()))
            })
            .await?
            {
                return Ok(());
            }
            match marker_due(database, runtime, &session, &mut budget, Some(deadline)).await {
                Ok(true) => {}
                Ok(false) => return Ok(()),
                Err(error) => {
                    save_fast_error(database, &session, "receive", Some(&error)).await?;
                    return Err(error);
                }
            }
            actors_due(database, runtime, &session, &mut budget, Some(deadline)).await?;
            let peers = pacing_work(database, runtime, &session, |_, pacing, _| {
                Ok(pacing.peers_due(Instant::now()))
            })
            .await?;
            let mut first_error = None;
            for peer in peers {
                if budget.remaining() == 0 || Instant::now() >= deadline {
                    break;
                }
                let cursor_peer = peer.clone();
                let cursor = pacing_work(database, runtime, &session, move |_, pacing, _| {
                    if pacing.peer_cursors.len() >= MAX_PENDING_PEERS
                        && !pacing.peer_cursors.contains_key(&cursor_peer)
                    {
                        return Ok(None);
                    }
                    Ok(Some(
                        pacing.peer_cursors.remove(&cursor_peer).unwrap_or_default(),
                    ))
                })
                .await?;
                let Some(mut cursor) = cursor else {
                    continue;
                };
                let result = super::fast::receive_peer(
                    database,
                    &session,
                    &peer,
                    &mut budget,
                    deadline,
                    &mut cursor,
                )
                .await;
                if let Err(error) =
                    finish_receive(database, runtime, &session, peer, cursor, result).await?
                {
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
            let urgent = pacing_work(database, runtime, &session, |conn, pacing, _| {
                pacing
                    .urgent
                    .extend(super::fast::missing_dependencies(conn)?);
                Ok(!pacing.urgent.is_empty())
            })
            .await?;
            if urgent {
                runtime.state_wakeup.notify_one();
            }
            save_fast_error(database, &session, "receive", first_error.as_ref()).await?;
            first_error.map_or(Ok(()), Err)
        }
    }
}

async fn finish_send(
    database: &Arc<Mutex<Connection>>,
    runtime: &LocalRuntime,
    session: &Session,
    result: AppResult<bool>,
) -> AppResult<AppResult<bool>> {
    pacing_work(database, runtime, session, move |_, pacing, session| {
        match &result {
            Ok(sent) => {
                pacing.send.success(Instant::now(), Duration::ZERO);
                pacing.directories_ready |= *sent;
            }
            Err(_) => {
                pacing.send.failure(Instant::now(), &session.actor);
                pacing.marker.next = None;
            }
        }
        Ok(result)
    })
    .await
}

async fn finish_receive(
    database: &Arc<Mutex<Connection>>,
    runtime: &LocalRuntime,
    session: &Session,
    peer: String,
    cursor: super::fast::PeerCursor,
    result: AppResult<bool>,
) -> AppResult<AppResult<()>> {
    pacing_work(database, runtime, session, move |_, pacing, _| {
        pacing.cursor = peer.clone();
        let pending = cursor.is_pending();
        if pending {
            // Bounded cursors belong only to this checked session.
            pacing.peer_cursors.insert(peer.clone(), cursor);
        }
        match result {
            Ok(stale) => {
                pacing.peers.entry(peer.clone()).or_default().success(
                    Instant::now(),
                    if pending {
                        Duration::ZERO
                    } else {
                        Duration::from_secs(1)
                    },
                );
                if stale {
                    pacing.urgent.insert(peer);
                }
                Ok(Ok(()))
            }
            Err(error) => {
                pacing
                    .peers
                    .entry(peer.clone())
                    .or_default()
                    .failure(Instant::now(), &peer);
                pacing.marker.next = None;
                Ok(Err(error))
            }
        }
    })
    .await
}

async fn fetch_objects(
    database: &Arc<Mutex<Connection>>,
    session: &Session,
    manifest: &Manifest,
    objects: &[storage::ObjectRef],
    budget: &mut Budget,
) -> AppResult<()> {
    if manifest.library_id != session.library {
        return Err(invalid("分片属于另一资料库"));
    }
    let generation = session.generation;
    let pinned = manifest.clone();
    blocking(database.clone(), move |conn| {
        let _writer = db::writer_guard(conn)?;
        database::require_generation(conn, generation)?;
        conn.execute("INSERT INTO local_sync_downloads(actor,kind,manifest) VALUES(?1,?2,?3) ON CONFLICT(actor,kind) DO UPDATE SET manifest=excluded.manifest",params![pinned.actor,pinned.kind,serde_json::to_string(&pinned)?])?;
        Ok(())
    }).await?;
    for object in objects {
        ensure_session(database, session).await?;
        let cached_manifest = manifest.clone();
        let cached_object = object.clone();
        if blocking(database.clone(), move |conn| {
            database::require_generation(conn, generation)?;
            Ok(storage::cached_object(
                conn,
                &cached_manifest.actor,
                &cached_manifest.kind,
                &cached_object,
            )?
            .is_some())
        })
        .await?
        {
            continue;
        }
        budget.take(1)?;
        let bytes = session
            .transport
            .library_get(&manifest.object_path(object), object.size)
            .await
            .map_err(transport_error)?
            .ok_or_else(|| {
                AppError::new(
                    "localSyncPending",
                    "远端分片尚未到齐，保留当前阅读数据并等待补齐",
                )
            })?;
        ensure_session(database, session).await?;
        let manifest = manifest.clone();
        let object = object.clone();
        blocking(database.clone(), move |conn| {
            storage::cache_object(conn, generation, &manifest, &object, &bytes)
        })
        .await?;
    }
    Ok(())
}
async fn pull(
    database: &Arc<Mutex<Connection>>,
    runtime: &LocalRuntime,
    session: &Session,
    kind: &str,
    budget: &mut Budget,
) -> AppResult<()> {
    let mut actors = actors_due(database, runtime, session, budget, None).await?;
    actors = pacing_work(database, runtime, session, move |_, pacing, _| {
        actors.sort_by_key(|actor| (!pacing.urgent.contains(actor), actor.clone()));
        Ok(actors)
    })
    .await?;
    for actor in actors {
        ensure_session(database, session).await?;
        if !valid_hex(&actor, 32) {
            return Err(invalid("设备目录身份无效"));
        }
        if actor == session.actor {
            continue;
        }
        let key = (actor.clone(), kind.to_owned());
        if !pacing_work(database, runtime, session, move |_, pacing, _| {
            let now = Instant::now();
            if pacing.manifests.get(&key).is_some_and(|next| *next > now) {
                return Ok(false);
            }
            let seconds = if key.1 == "state" { 1 } else { 10 };
            pacing
                .manifests
                .insert(key, now + Duration::from_secs(seconds));
            Ok(true)
        })
        .await?
        {
            continue;
        }
        let listing =
            storage::list_manifests_guarded(&session.transport, &actor, kind, budget, || {
                ensure_session(database, session)
            })
            .await;
        ensure_session(database, session).await?;
        let manifests = match listing {
            Err(e) if e.code == "localSyncNotFound" => continue,
            other => other?,
        };
        let Some((_, manifest)) = manifests.into_iter().next() else {
            continue;
        };
        let cache_key = format!("local_sync_peer_{kind}_{actor}");
        let generation = session.generation;
        let checked_key = cache_key.clone();
        let checked_manifest = manifest.clone();
        let is_state = kind == "state";
        let already_applied = blocking(database.clone(), move |conn| {
            database::require_generation(conn, generation)?;
            let known = db::get_setting(conn, &checked_key)?
                .map(|raw| serde_json::from_str::<Manifest>(&raw))
                .transpose()?;
            if let Some(known) = &known {
                if known.publication > checked_manifest.publication {
                    return Err(AppError::new(
                        "localSyncPending",
                        "云端设备清单暂时落后于已应用版本",
                    ));
                }
                if known.publication == checked_manifest.publication {
                    if known.file_name()? != checked_manifest.file_name()? {
                        return Err(invalid("相同设备发布编号出现不同内容"));
                    }
                    if is_state {
                        return Ok(true);
                    }
                }
            }
            Ok(false)
        })
        .await?;
        if already_applied {
            continue;
        }
        if kind == "body" && blocking(database.clone(), database::pending_bodies).await? == 0 {
            continue;
        }
        if kind == "state" {
            pull_state_candidate(database, session, &manifest, budget).await?;
            pacing_work(database, runtime, session, move |_, pacing, _| {
                pacing.urgent.remove(&actor);
                Ok(())
            })
            .await?;
            continue;
        }
        fetch_objects(database, session, &manifest, &manifest.shards, budget).await?;
        let body_catalog = if kind == "body" {
            let generation = session.generation;
            let manifest = manifest.clone();
            Some(
                blocking(database.clone(), move |conn| {
                    let catalog = super::body_archive::catalog(conn, &manifest)?;
                    let _writer = db::writer_guard(conn)?;
                    database::require_generation(conn, generation)?;
                    super::body_archive::record_links(conn, &manifest)?;
                    Ok(catalog)
                })
                .await?,
            )
        } else {
            None
        };
        if let Some(catalog) = &body_catalog {
            let targets = blocking(database.clone(), database::body_targets).await?;
            let references: Vec<_> = targets.into_iter().map(|(_, body)| body).collect();
            let objects = catalog.needed_objects(&references);
            fetch_objects(database, session, &manifest, &objects, budget).await?;
        }
        let generation = session.generation;
        let applied = manifest.clone();
        blocking(database.clone(), move |conn| {
            database::require_generation(conn, generation)?;
            super::body_archive::apply(
                conn,
                generation,
                &applied,
                body_catalog
                    .as_ref()
                    .ok_or_else(|| invalid("正文目录尚未校验"))?,
            )
        })
        .await?;
        blocking(database.clone(), move |conn| {
            let _writer = db::writer_guard(conn)?;
            database::require_generation(conn, generation)?;
            db::set_setting(conn, &cache_key, &serde_json::to_string(&manifest)?)?;
            conn.execute(
                "DELETE FROM local_sync_downloads WHERE actor=?1 AND kind=?2",
                params![manifest.actor, manifest.kind],
            )?;
            Ok(())
        })
        .await?;
    }
    Ok(())
}

async fn pull_state_candidate(
    database: &Arc<Mutex<Connection>>,
    session: &Session,
    manifest: &Manifest,
    budget: &mut Budget,
) -> AppResult<()> {
    ensure_session(database, session).await?;
    let generation = session.generation;
    let incoming = manifest.clone();
    let job = match blocking(database.clone(), move |conn| {
        super::logical::begin_receive(conn, generation, &incoming)
    })
    .await
    {
        Err(error) if error.code == "localSyncRejected" => return Ok(()),
        other => other?,
    };
    let phase_job = job.clone();
    let phase: String = blocking(database.clone(), move |conn| {
        Ok(conn.query_row(
            "SELECT phase FROM local_sync_receive WHERE id=?1",
            [&phase_job],
            |r| r.get(0),
        )?)
    })
    .await?;
    if phase != "staging" {
        return Ok(());
    }
    let result = async {
        fetch_objects(database, session, manifest, &manifest.shards, budget).await?;
        for (page, object) in manifest.shards.iter().enumerate() {
            ensure_session(database, session).await?;
            let actor = manifest.actor.clone();
            let object = object.clone();
            let target = job.clone();
            let guard = session.clone();
            blocking(database.clone(), move |conn| {
                check_session(conn, &guard)?;
                let bytes = storage::cached_object(conn, &actor, "state", &object)?
                    .ok_or_else(|| AppError::new("localSyncPending", "状态页待重新下载"))?;
                let fields = serde_json::from_slice(&bytes)
                    .map_err(|error| invalid(format!("状态页JSON无效: {error}")))?;
                super::logical::stage_page(conn, generation, &target, page, &fields)
            })
            .await?;
            tokio::task::yield_now().await;
        }
        let target = job.clone();
        let guard = session.clone();
        let checked = blocking(database.clone(), move |conn| {
            check_session(conn, &guard)?;
            super::logical::preflight(conn, &target)
        })
        .await?;
        ensure_session(database, session).await?;
        let target = job.clone();
        let guard = session.clone();
        if !blocking(database.clone(), move |conn| {
            check_session(conn, &guard)?;
            super::logical::accept(conn, generation, &target, checked)
        })
        .await?
        {
            return Err(AppError::new(
                "localSyncPending",
                "接收预验输入变化，等待下一工作片",
            ));
        }
        Ok(())
    }
    .await;
    if let Err(error) = &result {
        // Every awaited reader has finished before retiring a validation root.
        // Persist only an exact deterministic rejected identity; retries remain.
        let code = error.code.clone();
        let message = error.message.clone();
        blocking(database.clone(), move |conn| {
            super::logical::reject_candidate(conn, generation, &job, &AppError::new(&code, message))
        })
        .await?;
    }
    result
}
async fn mkdir_once(
    database: &Arc<Mutex<Connection>>,
    session: &Session,
    pending: &mut Pending,
    path: &str,
    budget: &mut Budget,
) -> AppResult<()> {
    if pending.ready_dirs.contains(path) {
        return Ok(());
    }
    budget.take(1)?;
    ensure_session(database, session).await?;
    session
        .transport
        .library_mkdir(path)
        .await
        .map_err(transport_error)?;
    ensure_session(database, session).await?;
    pending.ready_dirs.insert(path.to_owned());
    save_pending(database, session.generation, pending).await
}

async fn save_pending(
    database: &Arc<Mutex<Connection>>,
    generation: i64,
    pending: &Pending,
) -> AppResult<()> {
    let pending = pending.clone();
    blocking(database.clone(), move |conn| {
        storage::save_pending(conn, generation, &pending)
    })
    .await
}
async fn push(
    database: &Arc<Mutex<Connection>>,
    session: &Session,
    kind: &str,
    budget: &mut Budget,
) -> AppResult<()> {
    let generation = session.generation;
    let lane_kind = kind.to_owned();
    let actor = session.actor.clone();
    let (pending, previous) = blocking(database.clone(), move |conn| {
        database::require_generation(conn, generation)?;
        let pending = if lane_kind == "state" {
            storage::seal_state(conn, generation)?
        } else {
            storage::seal_bodies(conn, generation)?
        };
        Ok((
            pending,
            storage::previous_manifest(conn, &actor, &lane_kind)?,
        ))
    })
    .await?;
    let Some(mut pending) = pending else {
        return Ok(());
    };
    let actor_root = format!("{ROOT}actors/{}/", session.actor);
    let lane = format!("{actor_root}{kind}/");
    for directory in [
        &actor_root,
        &lane,
        &format!("{lane}objects/"),
        &format!("{lane}manifests/"),
    ] {
        mkdir_once(database, session, &mut pending, directory, budget).await?;
    }
    let publication = pending.manifest.clone();
    let (known, objects, previous) = blocking(database.clone(), move |conn| {
        let known: BTreeSet<String> = previous
            .as_ref()
            .map(|p| storage::publication_objects(conn, p))
            .transpose()?
            .unwrap_or_default()
            .into_iter()
            .map(|s| s.name)
            .collect();
        Ok((
            known,
            storage::publication_objects(conn, &publication)?,
            previous,
        ))
    })
    .await?;
    while pending.next_object < objects.len() {
        let object = objects[pending.next_object].clone();
        if !known.contains(&object.name) {
            let path = pending.manifest.object_path(&object);
            let directory = path
                .rsplit_once('/')
                .map(|v| format!("{}/", v.0))
                .ok_or_else(|| invalid("对象目录无效"))?;
            mkdir_once(database, session, &mut pending, &directory, budget).await?;
            budget.take(4)?;
            ensure_session(database, session).await?;
            let cache_actor = session.actor.clone();
            let cache_kind = kind.to_owned();
            let cache_object = object.clone();
            let bytes = blocking(database.clone(), move |conn| {
                storage::cached_object(conn, &cache_actor, &cache_kind, &cache_object)?
                    .ok_or_else(|| invalid("封批分片在本机缺失"))
            })
            .await?;
            let stored = session
                .transport
                .library_store_guarded(&path, &bytes, || transport_guard(database, session))
                .await;
            ensure_session(database, session).await?;
            if let Err(error) = stored {
                if error.kind == crate::webdav_transport::ErrorKind::Conflict {
                    blocking(database.clone(), move |conn| {
                        storage::retry_partial(conn, generation, &mut pending, Some(object))
                    })
                    .await?;
                    return Err(AppError::new(
                        "localSyncPending",
                        "上次传输未完整写入，已保留原操作并排队重传",
                    ));
                }
                return Err(transport_error(error));
            }
        }
        pending.next_object += 1;
        save_pending(database, generation, &pending).await?;
    }
    budget.take(4)?;
    ensure_session(database, session).await?;
    let stored = session
        .transport
        .library_store_guarded(
            &pending.manifest.manifest_path()?,
            &serde_json::to_vec(&pending.manifest)?,
            || transport_guard(database, session),
        )
        .await;
    ensure_session(database, session).await?;
    if let Err(error) = stored {
        if error.kind == crate::webdav_transport::ErrorKind::Conflict {
            blocking(database.clone(), move |conn| {
                storage::retry_partial(conn, generation, &mut pending, None)
            })
            .await?;
            return Err(AppError::new(
                "localSyncPending",
                "上次清单未完整写入，已保留原操作并排队重传",
            ));
        }
        return Err(transport_error(error));
    }
    ensure_session(database, session).await?;
    // Exact remote readback has completed. Prepare precise reclamation while the
    // old own-manifest still pins its catalog; ACK then advances that root.
    blocking(database.clone(), move |conn| {
        if let Some(previous) = previous {
            storage::enqueue_gc(conn, generation, &previous, &pending.manifest)?;
        }
        storage::acknowledge(conn, generation, &pending.manifest)?;
        Ok(())
    })
    .await
}
async fn collect_garbage(
    database: &Arc<Mutex<Connection>>,
    session: &Session,
    _budget: &mut Budget,
) -> AppResult<()> {
    ensure_session(database, session).await?;
    let generation = session.generation;
    blocking(database.clone(), move |conn| {
        maintain_local_gc(conn, generation)
    })
    .await
}

/// Remote DELETE is deliberately paused: local roots/ACKs do not prove that
/// unknown or offline readers have a safe replacement. Keep the durable remote
/// queue and all body obligations. This is loss prevention, not a space bound.
pub(super) fn maintain_local_gc(conn: &Connection, generation: i64) -> AppResult<()> {
    database::require_generation(conn, generation)?;
    storage::cleanup_slice(conn)?;
    storage::evict_cache(conn, generation)
}
pub async fn run_lane(
    database: &Arc<Mutex<Connection>>,
    runtime: &LocalRuntime,
    kind: &str,
) -> AppResult<()> {
    if runtime.stopped.load(Ordering::Acquire) {
        return Err(AppError::new("cancelled", "同步已停止"));
    }
    let gate = if kind == "state" {
        &runtime.state
    } else {
        &runtime.body
    };
    let Ok(_guard) = gate.try_lock() else {
        return Ok(());
    };
    let isolated = background_database(database).await?;
    let database = &isolated;
    let session = blocking(database.clone(), session).await?;
    let Some(mut session) = session else {
        return Ok(());
    };
    session.stopped = runtime.stopped.clone();
    let result = async {
        let mut budget = Budget::new();
        marker_due(database, runtime, &session, &mut budget, None).await?;
        if kind == "state" {
            run_fast_round(database, runtime, FastPhase::Send).await?;
        }
        pull(database, runtime, &session, kind, &mut budget).await?;
        push(database, &session, kind, &mut budget).await?;
        collect_garbage(database, &session, &mut budget).await?;
        if kind == "state" && !blocking(database.clone(), super::logical::settled).await? {
            return Err(AppError::new(
                "localSyncPending",
                "状态已接受，本地投影继续处理中",
            ));
        }
        if kind == "body" && blocking(database.clone(), database::pending_bodies).await? > 0 {
            return Err(AppError::new(
                "localSyncPending",
                "仍有未读或收藏正文未取得；保留待同步，请让持有正文的设备完成上传",
            ));
        }
        Ok(())
    }
    .await;
    let kind = kind.to_owned();
    blocking(database.clone(), move |conn| {
    let _writer = db::writer_guard(conn)?;
    if database::generation(conn)? == session.generation {
        match &result {
            Ok(()) => {
                let column = if kind == "state" {
                    "last_state_sync"
                } else {
                    "last_body_sync"
                };
                conn.execute("UPDATE local_sync_lanes SET last_sync=?1,next_retry=0,failures=0,error=NULL WHERE kind=?2",params![chrono::Utc::now().timestamp(),kind])?;
                conn.execute(
                    &format!("UPDATE local_sync_control SET {column}=?1 WHERE id=1"),
                    [chrono::Utc::now().timestamp()],
                )?;
            }
            Err(error) => {
                let pending = error.code == "localSyncPending";
                let failures: i64 = conn.query_row(
                    "SELECT failures FROM local_sync_lanes WHERE kind=?1",
                    [&kind],
                    |r| r.get(0),
                )?;
                let delay = if pending {
                    5
                } else {
                    (5_i64 * 2_i64.pow((failures as u32).min(8))).min(900)
                };
                conn.execute("UPDATE local_sync_lanes SET failures=failures+?1,next_retry=?2,error=?3 WHERE kind=?4",params![if pending{0}else{1},chrono::Utc::now().timestamp()+delay,error.message,kind])?;
            }
        }
        conn.execute("UPDATE local_sync_control SET next_retry_at=COALESCE((SELECT MIN(next_retry) FROM local_sync_lanes WHERE next_retry>0),0),last_error=(SELECT error FROM local_sync_lanes WHERE error IS NOT NULL ORDER BY kind DESC LIMIT 1) WHERE id=1",[])?;
    }
    result
    }).await
}
#[tauri::command]
pub async fn local_sync_run(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> AppResult<LocalSyncStatus> {
    run_and_notify(&app, &state.db, &state.local_sync, "state").await?;
    // Manual state completes promptly; body runs independently and status remains
    // explicit until all protected content is locally available.
    let db = state.db.clone();
    let runtime = state.local_sync.clone();
    tauri::async_runtime::spawn(async move {
        let _ = run_and_notify(&app, &db, &runtime, "body").await;
    });
    blocking(state.db.clone(), status).await
}
async fn data_revision(database: &Arc<Mutex<Connection>>) -> Option<i64> {
    blocking(database.clone(), |conn| {
        Ok(conn.query_row(
            "SELECT data_revision FROM local_sync_control WHERE id=1",
            [],
            |r| r.get(0),
        )?)
    })
    .await
    .ok()
}
async fn run_and_notify(
    app: &tauri::AppHandle,
    database: &Arc<Mutex<Connection>>,
    runtime: &LocalRuntime,
    kind: &str,
) -> AppResult<()> {
    let before = data_revision(database).await;
    let result = run_lane(database, runtime, kind).await;
    let after = data_revision(database).await;
    let _ = app.emit(
        "local-sync-updated",
        serde_json::json!({"kind":kind,"ok":result.is_ok()}),
    );
    if before.is_some() && after.is_some() && before != after {
        let _ = app.emit("feeds-updated", serde_json::json!({"source":"local-sync"}));
    }
    result
}
pub fn spawn(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let root = app.state::<AppState>().db.clone();
        let result = blocking(root, move |conn| {
                // Called once, before this runtime starts any lane. These roots
                // belonged to readers/builders in the previous process only.
                let tx = db::write_transaction(conn)?;
                tx.execute("DELETE FROM local_sync_object_pins WHERE owner GLOB 'state-build:*' OR owner GLOB 'body-build:*'",[])?;
                tx.execute("DELETE FROM local_sync_roots WHERE kind IN('state-build','body-build','snapshot','validation-reader')",[])?;
                tx.commit()?;
                Ok(())
        }).await;
        match result {
            Ok(()) => spawn_workers(app),
            Err(error) => {
                let _ = app.emit(
                    "local-sync-updated",
                    serde_json::json!({"kind":"state","ok":false,"error":error.message}),
                );
            }
        }
    });
}
fn spawn_workers(app: tauri::AppHandle) {
    // These tasks deliberately do not share the checkpoint HTTP gate. Recovery
    // is local and runs without credentials; fast intent traffic has its own
    // connection so a large state page/encoder cannot hold its async DB mutex.
    {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let root = app.state::<AppState>().db.clone();
            let runtime = app.state::<AppState>().local_sync.clone();
            let Ok(database) = background_database(&root).await else {
                return;
            };
            loop {
                if runtime.is_stopped() {
                    return;
                }
                let before = data_revision(&database).await;
                let result = recover_local_slice(&database, &runtime).await;
                if result.as_ref().is_ok_and(|worked| *worked) {
                    if before != data_revision(&database).await {
                        let _ =
                            app.emit("feeds-updated", serde_json::json!({"source":"local-sync"}));
                    }
                    tokio::task::yield_now().await;
                } else {
                    if let Err(error) = &result {
                        let message = error.message.clone();
                        let saved = blocking(database.clone(), move |conn| {
                            db::set_setting(conn, "local_sync_recovery_error", &message)
                        })
                        .await;
                        if let Err(error) = saved {
                            log::warn!("cannot persist local recovery error: {}", error.message);
                        }
                        let _ = app.emit(
                            "local-sync-updated",
                            serde_json::json!({"kind":"state","ok":false,"error":error.message}),
                        );
                    } else {
                        if let Err(error) = blocking(database.clone(), |conn| {
                            let _writer = db::writer_guard(conn)?;
                            conn.execute(
                                "DELETE FROM settings WHERE key='local_sync_recovery_error'",
                                [],
                            )?;
                            Ok(())
                        })
                        .await
                        {
                            log::warn!("cannot clear local recovery error: {}", error.message);
                        }
                    }
                    if let Err(error) = blocking(database.clone(), storage::cleanup_slice).await {
                        log::warn!("local sync cleanup deferred: {}", error.message);
                    }
                    tokio::time::sleep(if result.is_err() {
                        Duration::from_secs(5)
                    } else {
                        Duration::from_millis(250)
                    })
                    .await;
                }
            }
        });
    }
    for phase in [FastPhase::Send, FastPhase::Discover] {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let root = app.state::<AppState>().db.clone();
            let runtime = app.state::<AppState>().local_sync.clone();
            let Ok(database) = background_database(&root).await else {
                return;
            };
            loop {
                tokio::select! {_=runtime.fast_wakeup.notified(),if matches!(phase,FastPhase::Send)=>{},_=tokio::time::sleep(Duration::from_millis(100))=>{}}
                if runtime.is_stopped() {
                    return;
                }
                let enabled = blocking(database.clone(), |conn| {
                    Ok(read_config(conn)
                        .is_ok_and(|config| config.is_some_and(|c| c.mode == "realtime"))
                        && conn
                            .query_row(
                                "SELECT initialized FROM local_sync_control WHERE id=1",
                                [],
                                |r| r.get::<_, bool>(0),
                            )
                            .unwrap_or(false))
                })
                .await
                .unwrap_or(false);
                if !enabled {
                    continue;
                }
                let before = data_revision(&database).await;
                let result = run_fast_round(&database, &runtime, phase).await;
                if before != data_revision(&database).await {
                    let _ = app.emit("feeds-updated", serde_json::json!({"source":"local-sync"}));
                }
                if let Err(error) = result {
                    let _ = app.emit(
                        "local-sync-updated",
                        serde_json::json!({"kind":"state","ok":false,"error":error.message}),
                    );
                }
            }
        });
    }
    for kind in ["state", "body"] {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let db = app.state::<AppState>().db.clone();
            let runtime = app.state::<AppState>().local_sync.clone();
            let stop = crate::sync::runtime::for_db(&db);
            loop {
                tokio::select! {_=stop.wait_for_stop()=>return,_=runtime.state_wakeup.notified(),if kind=="state"=>{},_=tokio::time::sleep(Duration::from_secs(5))=>{}}
                if runtime.is_stopped() {
                    return;
                }
                let urgent = kind == "state" && !runtime.pacing.lock().await.urgent.is_empty();
                let due = blocking(db.clone(), move |conn| {
                    Ok(match read_config(conn) {
                        Ok(Some(config)) => {
                            let now = chrono::Utc::now().timestamp();
                            let time: rusqlite::Result<(i64, i64)> = conn.query_row(
                                "SELECT last_sync,next_retry FROM local_sync_lanes WHERE kind=?1",
                                [kind],
                                |r| Ok((r.get(0)?, r.get(1)?)),
                            );
                            time.is_ok_and(|(last, retry)| {
                                now >= retry
                                    && (urgent
                                        || now - last
                                            >= if config.mode == "realtime" {
                                                if kind == "state" {
                                                    5
                                                } else {
                                                    60
                                                }
                                            } else {
                                                i64::from(config.interval_minutes) * 60
                                            })
                            })
                        }
                        _ => false,
                    })
                })
                .await
                .unwrap_or(false);
                if due {
                    let _ = run_and_notify(&app, &db, &runtime, kind).await;
                }
            }
        });
    }
}

#[cfg(test)]
mod segment2_schedule_tests {
    use super::*;
    pub(super) fn fixture() -> (Connection, Session) {
        let mut conn = Connection::open_in_memory().unwrap();
        db::MIGRATIONS.to_latest(&mut conn).unwrap();
        database::register_functions(&conn).unwrap();
        database::initialize(&conn, &"1".repeat(32)).unwrap();
        let session = Session {
            generation: 0,
            library: "1".repeat(32),
            actor: database::actor(&conn).unwrap(),
            transport: Transport::new("http://127.0.0.1:9/", None, Duration::from_millis(100))
                .unwrap(),
            stopped: Arc::new(AtomicBool::new(false)),
        };
        (conn, session)
    }
    #[test]
    fn segment2_idle_due_round_robin_and_peer_failure_do_not_throttle_send() {
        let (_conn, session) = fixture();
        let mut pacing = FastPacing::default();
        pacing.observe(&session);
        let now = Instant::now();
        pacing.marker.success(now, Duration::from_secs(30));
        pacing.marker_valid = true;
        pacing.actors.success(now, Duration::from_secs(15));
        for i in 0..12 {
            pacing
                .peers
                .entry(format!("{i:032x}"))
                .or_default()
                .success(now, Duration::from_secs(1));
        }
        assert!(pacing
            .peers_due(now + Duration::from_millis(100))
            .is_empty());
        assert!(!pacing.marker.ready(now + Duration::from_secs(1)));
        assert!(!pacing.actors.ready(now + Duration::from_secs(1)));
        let first = pacing.peers_due(now + Duration::from_secs(1));
        assert_eq!(first.len(), 8);
        pacing.cursor = first[7].clone();
        for peer in &first {
            pacing
                .peers
                .get_mut(peer)
                .unwrap()
                .success(now + Duration::from_secs(1), Duration::from_secs(1));
        }
        let next = pacing.peers_due(now + Duration::from_millis(1100));
        assert_eq!(next.len(), 4);
        assert!(next.iter().all(|peer| !first.contains(peer)));
        pacing
            .peers
            .get_mut(&first[0])
            .unwrap()
            .failure(now, "peer");
        assert!(pacing.send.ready(now));
        assert!(!pacing.peers[&first[0]].ready(now + Duration::from_secs(1)));
        let mut changed = session.clone();
        changed.generation = 1;
        pacing.observe(&changed);
        assert!(pacing.marker.ready(now));
        assert!(!pacing.marker_valid);
    }
    #[tokio::test]
    async fn segment2_receive_gate_and_stop_are_independent_from_send_permission() {
        let (conn, session) = fixture();
        let runtime = LocalRuntime::default();
        let _peer = runtime.fast_receive.lock().await;
        assert!(runtime.fast.try_lock().is_ok());
        check_session(&conn, &session).unwrap();
        let mut stale = session.clone();
        stale.generation = 1;
        assert_eq!(
            check_session(&conn, &stale).unwrap_err().code,
            "staleLocalSync"
        );
        session.stopped.store(true, Ordering::Release);
        assert_eq!(
            check_session(&conn, &session).unwrap_err().code,
            "cancelled"
        );
    }
    #[tokio::test]
    async fn actor_discovery_completion_rejects_session_replaced_during_worker_wait() {
        let (conn, old) = fixture();
        let database = Arc::new(Mutex::new(conn));
        let runtime = LocalRuntime::default();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let late_actor = "2".repeat(32);
        let late = async {
            let guard = old.clone();
            blocking(database.clone(), move |conn| {
                check_session(conn, &guard)?;
                Ok(())
            })
            .await
            .unwrap();
            started_tx.send(()).unwrap();
            release_rx.await.unwrap();
            let pacing = runtime.pacing.clone();
            let guard = old.clone();
            let actors = vec![late_actor.clone()];
            blocking(database.clone(), move |conn| {
                let mut pacing = pacing.blocking_lock();
                finish_actor_discovery(conn, &mut pacing, &guard, &Ok(actors))
            })
            .await
        };
        let replace = async {
            started_rx.await.unwrap();
            blocking(database.clone(), |conn| {
                let tx = db::write_transaction(conn)?;
                tx.execute(
                    "UPDATE local_sync_control SET generation=generation+1 WHERE id=1",
                    [],
                )?;
                tx.commit()?;
                Ok(())
            })
            .await
            .unwrap();
            let mut newer = old.clone();
            newer.generation += 1;
            let mut pacing = runtime.pacing.lock().await;
            pacing.observe(&newer);
            pacing.peers.entry("3".repeat(32)).or_default();
            drop(pacing);
            release_tx.send(()).unwrap();
        };
        let (result, ()) = tokio::join!(late, replace);
        assert_eq!(result.unwrap_err().code, "staleLocalSync");
        let pacing = runtime.pacing.lock().await;
        assert_eq!(pacing.scope.as_ref().unwrap().0, old.generation + 1);
        assert!(pacing.peers.contains_key(&"3".repeat(32)));
        assert!(!pacing.peers.contains_key(&late_actor));
    }

    #[tokio::test]
    async fn shared_pacing_work_rejects_older_session_before_mutating_new_scope() {
        let (conn, old) = fixture();
        let database = Arc::new(Mutex::new(conn));
        let runtime = LocalRuntime::default();
        let newer_peer = "3".repeat(32);
        let late_peer = "2".repeat(32);
        {
            let mut newer = old.clone();
            newer.generation += 1;
            let mut pacing = runtime.pacing.lock().await;
            pacing.observe(&newer);
            pacing.peers.entry(newer_peer.clone()).or_default();
        }

        let result = pacing_work(&database, &runtime, &old, move |_, pacing, _| {
            pacing.peers.entry(late_peer.clone()).or_default();
            pacing.cursor = late_peer;
            Ok(())
        })
        .await;

        assert_eq!(result.unwrap_err().code, "staleLocalSync");
        let pacing = runtime.pacing.lock().await;
        assert_eq!(pacing.scope.as_ref().unwrap().0, old.generation + 1);
        assert!(pacing.peers.contains_key(&newer_peer));
        assert!(!pacing.peers.contains_key(&"2".repeat(32)));
    }

    #[tokio::test]
    async fn segment2_production_gc_round_uses_zero_network_budget() {
        let (conn, session) = fixture();
        let path = format!("{ROOT}actors/{}/state/objects/obsolete.bin", session.actor);
        conn.execute(
            "INSERT INTO local_sync_gc(path,generation) VALUES(?1,0)",
            [&path],
        )
        .unwrap();
        let database = Arc::new(Mutex::new(conn));
        let mut budget = Budget::limited(0);
        // The production function must succeed even with no network budget and
        // an unreachable transport, retaining the exact durable DELETE queue.
        collect_garbage(&database, &session, &mut budget)
            .await
            .unwrap();
        assert_eq!(
            database
                .lock()
                .await
                .query_row("SELECT path FROM local_sync_gc", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            path
        );
        assert_eq!(budget.remaining(), 0);
    }
}

#[cfg(test)]
#[path = "fast_network_tests.rs"]
mod fast_network_tests;
