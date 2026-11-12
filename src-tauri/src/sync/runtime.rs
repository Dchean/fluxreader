use super::SyncReport;
use crate::diagnostics;
use crate::error::{AppError, AppResult};
use rusqlite::Connection;
use serde::Serialize;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;
use tauri::{Emitter, State};
use tokio::sync::{watch, Mutex as AsyncMutex};

type EventSink = Arc<dyn Fn(RuntimeEvent) + Send + Sync>;
type RuntimeRegistry = HashMap<usize, (Weak<AsyncMutex<Connection>>, Arc<SyncRuntime>)>;
static REGISTRY: OnceLock<Mutex<RuntimeRegistry>> = OnceLock::new();
static EVENT_SINK: OnceLock<EventSink> = OnceLock::new();
static SEQUENCE: AtomicU64 = AtomicU64::new(1);
// Allocate IDs and deliver events in one order, including across runtime lanes.
// Otherwise a paused old started event can arrive after a newer finished event.
static EVENT_ORDER: Mutex<()> = Mutex::new(());

tokio::task_local! {
    static ACTIVE: Active;
}

#[derive(Clone)]
struct Active {
    runtime: Arc<SyncRuntime>,
    id: u64,
    source: &'static str,
    generation: Arc<AtomicI64>,
    lane: SyncLane,
}

/// Fixed concurrency budget: at most one content, state read and immediate push
/// operation per database. Nested calls may only move to a later lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SyncLane {
    Content,
    State,
    Push,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeEvent {
    pub id: u64,
    pub sequence: u64,
    pub source: String,
    pub phase: String,
    pub stage: Option<String>,
    pub outcome: Option<String>,
    pub message: Option<String>,
}

pub struct SyncRuntime {
    gates: [AsyncMutex<()>; 3],
    admission: Mutex<()>,
    stopped: watch::Sender<bool>,
    active_count: watch::Sender<usize>,
    sink: EventSink,
}

pub trait RuntimeResult {
    fn runtime_outcome(&self) -> (&str, Option<String>);

    fn diagnostic_error(&self) -> Option<AppError> {
        None
    }
}

impl RuntimeResult for SyncReport {
    fn runtime_outcome(&self) -> (&str, Option<String>) {
        if self.errors.is_empty() {
            ("success", None)
        } else {
            ("partial", Some(self.errors.join("；")))
        }
    }

    fn diagnostic_error(&self) -> Option<AppError> {
        (!self.errors.is_empty())
            .then(|| AppError::new(if self.auth_failure { "auth" } else { "partial" }, ""))
    }
}

impl RuntimeResult for () {
    fn runtime_outcome(&self) -> (&str, Option<String>) {
        ("success", None)
    }
}

impl SyncRuntime {
    pub fn new(sink: impl Fn(RuntimeEvent) + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self {
            gates: std::array::from_fn(|_| AsyncMutex::new(())),
            admission: Mutex::new(()),
            stopped: watch::channel(false).0,
            active_count: watch::channel(0).0,
            sink: Arc::new(sink),
        })
    }

    pub fn stop(&self) {
        let _admission = self
            .admission
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.stopped.send_replace(true);
    }

    pub fn is_stopped(&self) -> bool {
        *self.stopped.borrow()
    }

    pub(crate) fn ensure_running(&self) -> AppResult<()> {
        if self.is_stopped() {
            Err(cancelled())
        } else {
            Ok(())
        }
    }

    pub async fn wait_for_stop(&self) {
        wait_stopped(&mut self.stopped.subscribe()).await;
    }

    pub async fn drain(&self, budget: Duration) -> bool {
        self.stop();
        let mut count = self.active_count.subscribe();
        tokio::time::timeout(budget, async {
            while *count.borrow_and_update() != 0 {
                if count.changed().await.is_err() {
                    break;
                }
            }
        })
        .await
        .is_ok()
    }

    pub fn run<T, F>(
        self: &Arc<Self>,
        source: &'static str,
        operation: F,
    ) -> Pin<Box<dyn Future<Output = AppResult<T>> + Send>>
    where
        T: RuntimeResult + Send + 'static,
        F: Future<Output = AppResult<T>> + Send + 'static,
    {
        self.run_in(SyncLane::Content, source, operation)
    }

    // Note: 生命周期共用，慢正文不持状态许可 — 见
    // .agents/notes/implemented/architecture/2026-11-05-同步运行生命周期与诊断.md
    pub fn run_in<T, F>(
        self: &Arc<Self>,
        lane: SyncLane,
        source: &'static str,
        operation: F,
    ) -> Pin<Box<dyn Future<Output = AppResult<T>> + Send>>
    where
        T: RuntimeResult + Send + 'static,
        F: Future<Output = AppResult<T>> + Send + 'static,
    {
        let runtime = self.clone();
        Box::pin(async move {
            let active = ACTIVE
                .try_with(Clone::clone)
                .ok()
                .filter(|active| Arc::ptr_eq(&active.runtime, &runtime));
            if let Some(mut active) = active {
                runtime.ensure_running()?;
                if active.lane == lane {
                    return operation.await;
                }
                if lane < active.lane {
                    return Err(AppError::internal("同步通道不能反向嵌套"));
                }
                let mut stopped = runtime.stopped.subscribe();
                let _permit = tokio::select! {
                    biased;
                    _ = wait_stopped(&mut stopped) => return Err(cancelled()),
                    permit = runtime.gates[lane as usize].lock() => permit,
                };
                runtime.ensure_running()?;
                active.lane = lane;
                // Reuse the parent activity and its Arc generation; a nested lane
                // must neither create an orphan activity nor reset the account.
                return ACTIVE.scope(active, operation).await;
            }
            if runtime.is_stopped() {
                return Err(cancelled());
            }
            tokio::spawn(async move {
                let mut stopped = runtime.stopped.subscribe();
                let permit = tokio::select! {
                    biased;
                    _ = wait_stopped(&mut stopped) => return Err(cancelled()),
                    permit = runtime.gates[lane as usize].lock() => permit,
                };
                let mut completion = Completion::new(runtime.clone(), source)?;
                let result = ACTIVE
                    .scope(
                        Active {
                            runtime: runtime.clone(),
                            id: completion.id,
                            source,
                            generation: Arc::new(AtomicI64::new(-1)),
                            lane,
                        },
                        async {
                            tokio::select! {
                                biased;
                                _ = wait_stopped(&mut stopped) => Err(cancelled()),
                                result = operation => result,
                            }
                        },
                    )
                    .await;
                completion.finish(&result);
                drop(permit);
                result
            })
            .await
            .map_err(|_| AppError::internal("同步任务异常结束，请重试"))?
        })
    }

    fn emit(&self, mut event: RuntimeEvent) {
        let _order = EVENT_ORDER
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        event.sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        (self.sink)(event);
    }
}

async fn wait_stopped(receiver: &mut watch::Receiver<bool>) {
    while !*receiver.borrow_and_update() {
        if receiver.changed().await.is_err() {
            break;
        }
    }
}

pub(crate) fn cancelled() -> AppError {
    AppError::new(
        "syncCancelled",
        "应用正在退出，同步已停止；未确认操作会保留",
    )
}

struct Completion {
    runtime: Arc<SyncRuntime>,
    id: u64,
    source: &'static str,
    finished: bool,
}

impl Completion {
    fn new(runtime: Arc<SyncRuntime>, source: &'static str) -> AppResult<Self> {
        {
            let _admission = runtime
                .admission
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            runtime.ensure_running()?;
            runtime.active_count.send_modify(|count| *count += 1);
        }
        let mut completion = Self {
            runtime,
            id: 0,
            source,
            finished: false,
        };
        {
            let _order = EVENT_ORDER
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            completion.id = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            (completion.runtime.sink)(RuntimeEvent {
                id: completion.id,
                sequence: SEQUENCE.fetch_add(1, Ordering::Relaxed),
                source: source.to_string(),
                phase: "started".into(),
                stage: None,
                outcome: None,
                message: None,
            });
        }
        diagnostics::record(completion.id, source, "started", None);
        Ok(completion)
    }

    fn event(&self, phase: &str, outcome: Option<&str>, message: Option<String>) {
        self.runtime.emit(RuntimeEvent {
            id: self.id,
            sequence: 0, // allocated under EVENT_ORDER by emit
            source: self.source.to_string(),
            phase: phase.to_string(),
            stage: None,
            outcome: outcome.map(str::to_string),
            message: message.map(|message| message.chars().take(600).collect()),
        });
    }

    fn finish<T: RuntimeResult>(&mut self, result: &AppResult<T>) {
        let (outcome, message) = match result {
            Ok(report) => report.runtime_outcome(),
            Err(error) if error.code == "notConnected" => ("not_configured", None),
            Err(error) if error.code == "syncCancelled" => {
                ("cancelled", Some(error.message.clone()))
            }
            Err(error) => ("error", Some(error.message.clone())),
        };
        self.event("finished", Some(outcome), message);
        let reported_error = result
            .as_ref()
            .ok()
            .and_then(RuntimeResult::diagnostic_error);
        diagnostics::record(
            self.id,
            self.source,
            outcome,
            result.as_ref().err().or(reported_error.as_ref()),
        );
        self.finished = true;
    }
}

impl Drop for Completion {
    fn drop(&mut self) {
        if !self.finished {
            self.event(
                "finished",
                Some("error"),
                Some("同步任务异常结束，请重试".into()),
            );
            diagnostics::record(self.id, self.source, "error", Some(&AppError::internal("")));
        }
        self.runtime
            .active_count
            .send_modify(|count| *count = count.saturating_sub(1));
    }
}

pub fn for_db(db: &Arc<AsyncMutex<Connection>>) -> Arc<SyncRuntime> {
    let key = Arc::as_ptr(db) as usize;
    let mut registry = REGISTRY
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    registry.retain(|_, (database, _)| database.strong_count() > 0);
    registry
        .entry(key)
        .or_insert_with(|| {
            let runtime = SyncRuntime::new(|event| {
                if let Some(sink) = EVENT_SINK.get() {
                    sink(event);
                }
            });
            (Arc::downgrade(db), runtime)
        })
        .1
        .clone()
}

pub fn init_events(app: tauri::AppHandle) {
    let _ = EVENT_SINK.set(Arc::new(move |event| {
        if app.emit("sync-activity", &event).is_err() {
            diagnostics::record(event.id, &event.source, "event_failed", None);
        }
    }));
}

pub fn stage(stage: &str) {
    let _ = ACTIVE.try_with(|active| {
        active.runtime.emit(RuntimeEvent {
            id: active.id,
            sequence: 0,
            source: active.source.to_string(),
            phase: "stage".into(),
            stage: Some(stage.to_string()),
            outcome: None,
            message: None,
        });
    });
}

pub(super) fn accept_generation(generation: i64) -> AppResult<()> {
    ACTIVE
        .try_with(|active| {
            match active.generation.compare_exchange(
                -1,
                generation,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => Ok(()),
                Err(expected) if expected == generation => Ok(()),
                Err(_) => Err(super::SyncSession::stale_error()),
            }
        })
        .unwrap_or(Ok(()))
}

pub(super) async fn check_generation(db: &Arc<AsyncMutex<Connection>>) -> AppResult<()> {
    // accept_generation reads the async task-local round identity, so only the
    // SQL snapshot moves to the worker; acceptance remains on this task.
    let generation =
        crate::state::database_work(db.clone(), |conn| crate::db::sync_generation(conn)).await?;
    accept_generation(generation)
}

#[derive(Debug, Serialize)]
pub struct SyncRunError {
    pub stage: &'static str,
    pub code: String,
    pub message: String,
    #[serde(skip)]
    auth_failure: bool,
}

impl SyncRunError {
    fn from_error(stage: &'static str, error: AppError) -> Self {
        let auth_failure = error.is_auth_failure();
        Self {
            stage,
            code: error.code,
            message: error.message,
            auth_failure,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SyncRunResult {
    pub activity_id: u64,
    pub feeds: Option<SyncReport>,
    pub states: Option<SyncReport>,
    pub error: Option<SyncRunError>,
}

impl RuntimeResult for SyncRunResult {
    fn runtime_outcome(&self) -> (&str, Option<String>) {
        if let Some(error) = &self.error {
            return if error.code == "notConnected" {
                ("not_configured", None)
            } else {
                ("error", Some(error.message.clone()))
            };
        }
        let errors: Vec<_> = self
            .feeds
            .iter()
            .chain(self.states.iter())
            .flat_map(|report| report.errors.iter().cloned())
            .collect();
        if errors.is_empty() {
            ("success", None)
        } else {
            ("partial", Some(errors.join("；")))
        }
    }

    fn diagnostic_error(&self) -> Option<AppError> {
        if let Some(error) = &self.error {
            return Some(AppError::new(&error.code, "").auth_if(error.auth_failure));
        }
        self.feeds
            .iter()
            .chain(self.states.iter())
            .filter_map(RuntimeResult::diagnostic_error)
            .max_by_key(|error| error.code == "auth")
    }
}

// Note: 一次 IPC 拥有完整 feeds→states 生命周期，旧 phase 入口仍受同一闸门保护 — 见 .agents/notes/implemented/architecture/2026-11-05-同步运行生命周期与诊断.md
pub async fn run_manual(
    db: &Arc<AsyncMutex<Connection>>,
    http: &reqwest::Client,
) -> AppResult<SyncRunResult> {
    let runtime = for_db(db);
    let db = db.clone();
    let http = http.clone();
    runtime
        .run("manual", async move {
            let mut result = SyncRunResult {
                activity_id: ACTIVE.with(|active| active.id),
                feeds: None,
                states: None,
                error: None,
            };
            stage("feeds");
            match super::feeds_phase(&db, &http).await {
                Ok(report) => result.feeds = Some(report),
                Err(error) => {
                    result.error = Some(SyncRunError::from_error("feeds", error));
                    return Ok(result);
                }
            }
            stage("states");
            match super::states_phase(&db, &http, true).await {
                Ok(report) => result.states = Some(report),
                Err(error) => result.error = Some(SyncRunError::from_error("states", error)),
            }
            Ok(result)
        })
        .await
}

#[tauri::command]
pub async fn sync_run(state: State<'_, crate::state::AppState>) -> AppResult<SyncRunResult> {
    run_manual(&state.db, &state.http).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn nested_lane_keeps_the_parent_account_generation() {
        let runtime = SyncRuntime::new(|_| {});
        let nested = runtime.clone();
        let error = runtime
            .run("manual", async move {
                accept_generation(4)?;
                nested
                    .run_in(SyncLane::State, "states", async {
                        accept_generation(5)?;
                        Ok(())
                    })
                    .await
            })
            .await
            .unwrap_err();
        assert_eq!(error.code, "staleSession");
        runtime
            .run_in(SyncLane::State, "states", async {
                accept_generation(5)?;
                Ok(())
            })
            .await
            .unwrap();
    }

    #[test]
    fn returned_phase_failures_and_partial_reports_keep_diagnostic_classification() {
        let mut result = SyncRunResult {
            activity_id: 1,
            feeds: Some(SyncReport {
                errors: vec!["untrusted partial text".into()],
                ..Default::default()
            }),
            states: Some(SyncReport {
                errors: vec!["untrusted authentication text".into()],
                auth_failure: true,
                ..Default::default()
            }),
            error: None,
        };
        let error = result.diagnostic_error().unwrap();
        assert_eq!(error.code, "auth");
        assert!(error.message.is_empty());
        result.error = Some(SyncRunError::from_error(
            "states",
            AppError::network("https://user:secret@example.test/?token=private"),
        ));
        let error = result.diagnostic_error().unwrap();
        assert_eq!(error.code, "network");
        assert!(!error.is_auth_failure());
        assert!(error.message.is_empty());
        result.error = Some(SyncRunError::from_error(
            "states",
            AppError::network("ClientLogin returned 401").auth(),
        ));
        let error = result.diagnostic_error().unwrap();
        assert_eq!(error.code, "network");
        assert!(error.is_auth_failure());
        assert!(error.message.is_empty());
    }

    #[tokio::test]
    async fn a_round_rejects_generation_change_between_phases_and_next_round_recovers() {
        let runtime = SyncRuntime::new(|_| {});
        let db = Arc::new(AsyncMutex::new(Connection::open_in_memory().unwrap()));
        db.lock()
            .await
            .execute_batch("CREATE TABLE settings(key TEXT PRIMARY KEY, value TEXT);")
            .unwrap();
        let round_db = db.clone();
        let error = runtime
            .run("manual", async move {
                check_generation(&round_db).await?;
                crate::db::bump_sync_generation(&*round_db.lock().await).unwrap();
                check_generation(&round_db).await?;
                Ok(())
            })
            .await
            .unwrap_err();
        assert_eq!(error.code, "staleSession");
        runtime
            .run("manual", async move {
                check_generation(&db).await?;
                accept_generation(1)?;
                Ok(())
            })
            .await
            .unwrap();
    }
}
