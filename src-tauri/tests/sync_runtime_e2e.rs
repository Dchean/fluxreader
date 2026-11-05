mod common;
mod mock_greader;

use app_lib::error::AppError;
use app_lib::sync::runtime::{self, RuntimeEvent, SyncRuntime};
use app_lib::{db, sync};
use mock_greader::MockGReader;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;

fn observed_runtime() -> (Arc<SyncRuntime>, Arc<Mutex<Vec<RuntimeEvent>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    (
        SyncRuntime::new(move |event| captured.lock().unwrap().push(event)),
        events,
    )
}

#[tokio::test]
async fn auto_focus_manual_rounds_are_serial_and_nested_phases_keep_one_activity() {
    let (runtime, events) = observed_runtime();
    let (started_tx, started_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let active = Arc::new(AtomicUsize::new(0));
    let maximum = Arc::new(AtomicUsize::new(0));
    let first = {
        let runtime = runtime.clone();
        let active = active.clone();
        let maximum = maximum.clone();
        tokio::spawn(async move {
            runtime
                .run("auto", async move {
                    maximum.fetch_max(active.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
                    started_tx.send(()).unwrap();
                    release_rx.await.unwrap();
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok(sync::SyncReport::default())
                })
                .await
        })
    };
    started_rx.await.unwrap();
    let second = {
        let runtime = runtime.clone();
        let active = active.clone();
        let maximum = maximum.clone();
        tokio::spawn(async move {
            runtime
                .run("focus", async move {
                    maximum.fetch_max(active.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok(sync::SyncReport::default())
                })
                .await
        })
    };
    let third = {
        let runtime = runtime.clone();
        let nested = runtime.clone();
        let active = active.clone();
        let maximum = maximum.clone();
        tokio::spawn(async move {
            runtime
                .run("manual", async move {
                    maximum.fetch_max(active.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
                    runtime::stage("feeds");
                    nested.run("feeds", async { Ok(()) }).await?;
                    runtime::stage("states");
                    nested.run("states", async { Ok(()) }).await?;
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok(sync::SyncReport::default())
                })
                .await
        })
    };
    tokio::task::yield_now().await;
    assert_eq!(
        events.lock().unwrap().len(),
        1,
        "等待闸门的轮次不得开始网络活动"
    );
    release_tx.send(()).unwrap();
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    third.await.unwrap().unwrap();
    assert_eq!(maximum.load(Ordering::SeqCst), 1);
    let events = events.lock().unwrap();
    let starts: Vec<_> = events
        .iter()
        .filter(|event| event.phase == "started")
        .collect();
    let finishes: Vec<_> = events
        .iter()
        .filter(|event| event.phase == "finished")
        .collect();
    assert_eq!(starts.len(), 3);
    assert_eq!(finishes.len(), 3);
    assert!(events
        .windows(2)
        .all(|pair| pair[0].sequence < pair[1].sequence));
    for (start, finish) in starts.iter().zip(finishes.iter()) {
        assert_eq!(start.id, finish.id);
        assert_eq!(finish.outcome.as_deref(), Some("success"));
    }
    let stages: Vec<_> = events
        .iter()
        .filter(|event| event.phase == "stage")
        .collect();
    assert_eq!(stages.len(), 2);
    assert_eq!(stages[0].id, stages[1].id);
    assert_eq!(stages[0].source, "manual");
}

#[tokio::test]
async fn caller_cancellation_does_not_drop_the_round_or_strand_the_gate() {
    let (runtime, events) = observed_runtime();
    let (started_tx, started_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let caller = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .run("manual", async move {
                    started_tx.send(()).unwrap();
                    release_rx.await.unwrap();
                    Ok(())
                })
                .await
        })
    };
    started_rx.await.unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert_eq!(events.lock().unwrap().len(), 1);
    release_tx.send(()).unwrap();
    tokio::time::timeout(
        Duration::from_secs(2),
        runtime.run("focus", async { Ok(()) }),
    )
    .await
    .unwrap()
    .unwrap();
    let events = events.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.phase == "finished")
            .count(),
        2
    );
}

#[tokio::test]
async fn partial_panic_and_recovery_each_publish_a_real_terminal_result() {
    let (runtime, events) = observed_runtime();
    runtime
        .run("auto", async {
            Ok(sync::SyncReport {
                errors: vec!["一个源拉取失败".into()],
                ..Default::default()
            })
        })
        .await
        .unwrap();
    let error = runtime
        .run("focus", async {
            panic!("injected runtime task panic");
            #[allow(unreachable_code)]
            Ok(())
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, "internal");
    runtime.run("manual", async { Ok(()) }).await.unwrap();
    let outcomes: Vec<_> = events
        .lock()
        .unwrap()
        .iter()
        .filter(|event| event.phase == "finished")
        .map(|event| event.outcome.clone().unwrap())
        .collect();
    assert_eq!(outcomes, ["partial", "error", "success"]);
}

#[tokio::test]
async fn stop_cancels_inflight_and_rejects_queued_or_new_network_work() {
    let (runtime, events) = observed_runtime();
    let requests = Arc::new(AtomicUsize::new(0));
    let (started_tx, started_rx) = oneshot::channel();
    let first = {
        let runtime = runtime.clone();
        let requests = requests.clone();
        tokio::spawn(async move {
            runtime
                .run("auto", async move {
                    requests.fetch_add(1, Ordering::SeqCst);
                    started_tx.send(()).unwrap();
                    std::future::pending::<()>().await;
                    requests.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
                .await
        })
    };
    started_rx.await.unwrap();
    let queued = {
        let runtime = runtime.clone();
        let requests = requests.clone();
        tokio::spawn(async move {
            runtime
                .run("focus", async move {
                    requests.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
                .await
        })
    };
    assert!(runtime.drain(Duration::from_secs(1)).await);
    assert_eq!(first.await.unwrap().unwrap_err().code, "syncCancelled");
    assert_eq!(queued.await.unwrap().unwrap_err().code, "syncCancelled");
    assert_eq!(
        runtime
            .run("manual", async { Ok(()) })
            .await
            .unwrap_err()
            .code,
        "syncCancelled"
    );
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    assert_eq!(
        events.lock().unwrap().last().unwrap().outcome.as_deref(),
        Some("cancelled")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drain_budget_bounds_waiting_for_synchronous_cleanup() {
    struct SlowDrop(std::sync::mpsc::Receiver<()>);
    impl Drop for SlowDrop {
        fn drop(&mut self) {
            let _ = self.0.recv_timeout(Duration::from_secs(2));
        }
    }
    let (runtime, _) = observed_runtime();
    let (started_tx, started_rx) = oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .run("auto", async move {
                    let _cleanup = SlowDrop(release_rx);
                    started_tx.send(()).unwrap();
                    std::future::pending::<()>().await;
                    Ok(())
                })
                .await
        })
    };
    started_rx.await.unwrap();
    assert!(!runtime.drain(Duration::from_millis(20)).await);
    release_tx.send(()).unwrap();
    assert!(runtime.drain(Duration::from_secs(1)).await);
    assert_eq!(task.await.unwrap().unwrap_err().code, "syncCancelled");
}

fn test_database(
    name: &str,
    server: Option<&MockGReader>,
) -> (
    common::TempDbGuard,
    Arc<tokio::sync::Mutex<rusqlite::Connection>>,
) {
    let path = common::unique_db_path(name);
    let guard = common::TempDbGuard::new(path.clone());
    let conn = db::open(&path).unwrap();
    if let Some(server) = server {
        db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
        db::set_setting(&conn, "greader_username", "test").unwrap();
        db::set_setting(&conn, "greader_password", "test-token").unwrap();
    }
    (guard, Arc::new(tokio::sync::Mutex::new(conn)))
}

#[tokio::test]
async fn empty_queues_auth_failure_is_not_notconfigured_on_any_phase() {
    let server = MockGReader::start().await.unwrap();
    server.set_reject_login(true);
    let (_guard, db) = test_database("runtime_empty_auth", Some(&server));
    let http = app_lib::ingestion::build_client(2);
    let generation = db::sync_generation(&*db.lock().await).unwrap();
    let results = [
        sync::feeds_phase(&db, &http).await,
        sync::feeds_phase_at_scoped(&db, &http, generation, None).await,
        sync::states_phase(&db, &http, false).await,
        sync::sync_subscription_discovery(&db, &http).await,
    ];
    for result in results {
        let error = result.unwrap_err();
        assert_ne!(error.code, "notConnected");
        assert!(!error.message.is_empty());
    }
    let result = runtime::run_manual(&db, &http).await.unwrap();
    assert!(result.feeds.is_none() && result.states.is_none());
    let error = result.error.unwrap();
    assert_eq!(error.stage, "feeds");
    assert_ne!(error.code, "notConnected");
    assert_eq!(db::sync_queue_stats(&*db.lock().await).unwrap().waiting, 0);
}

#[tokio::test]
async fn no_configuration_is_distinct_and_network_failure_is_preserved() {
    let (_guard, db) = test_database("runtime_notconfigured", None);
    let http = app_lib::ingestion::build_client(1);
    assert_eq!(
        sync::feeds_phase(&db, &http).await.unwrap_err().code,
        "notConnected"
    );
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    {
        let conn = db.lock().await;
        db::set_setting(&conn, "greader_endpoint", &format!("http://{address}")).unwrap();
        db::set_setting(&conn, "greader_username", "test").unwrap();
        db::set_setting(&conn, "greader_password", "test-token").unwrap();
    }
    let result = runtime::run_manual(&db, &http).await.unwrap();
    assert_ne!(result.error.unwrap().code, "notConnected");
}

#[tokio::test]
async fn manual_run_preserves_both_phase_reports_and_partial_failure() {
    let server = MockGReader::start().await.unwrap();
    server.set_fail_subscription_list(true);
    let (_guard, db) = test_database("runtime_phase_reports", Some(&server));
    let http = app_lib::ingestion::build_client(2);
    let result = runtime::run_manual(&db, &http).await.unwrap();
    assert!(result.error.is_none());
    assert!(!result.feeds.unwrap().errors.is_empty());
    assert!(result.states.is_some());
    server.set_fail_subscription_list(false);
    let recovered = runtime::run_manual(&db, &http).await.unwrap();
    assert!(recovered.error.is_none());
    assert!(recovered.feeds.unwrap().errors.is_empty());
    assert!(recovered.states.unwrap().errors.is_empty());
}

#[tokio::test]
async fn a_generation_change_before_nested_states_rejects_before_new_account_auth() {
    let server = MockGReader::start().await.unwrap();
    let (_guard, db) = test_database("runtime_generation", Some(&server));
    let http = app_lib::ingestion::build_client(2);
    let runtime = runtime::for_db(&db);
    let round_db = db.clone();
    let round_server = server.clone();
    let error = runtime
        .run("manual", async move {
            sync::feeds_phase(&round_db, &http).await?;
            let login_count = round_server.login_request_count();
            {
                let conn = round_db.lock().await;
                db::bump_sync_generation(&conn)?;
                db::set_setting(&conn, "greader_username", "changed-account")?;
            }
            let error = sync::states_phase(&round_db, &http, true)
                .await
                .unwrap_err();
            assert_eq!(
                round_server.login_request_count(),
                login_count,
                "旧轮次不得为新账号发出认证请求"
            );
            Err::<(), _>(error)
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, "staleSession");
}

#[tokio::test]
async fn error_result_with_no_queue_still_emits_error_and_can_retry() {
    let (runtime, events) = observed_runtime();
    runtime
        .run("auto", async {
            Err::<(), _>(AppError::new("network", "端点无法连接"))
        })
        .await
        .unwrap_err();
    runtime.run("focus", async { Ok(()) }).await.unwrap();
    let outcomes: Vec<_> = events
        .lock()
        .unwrap()
        .iter()
        .filter(|event| event.phase == "finished")
        .map(|event| (event.outcome.clone().unwrap(), event.message.clone()))
        .collect();
    assert_eq!(outcomes[0], ("error".into(), Some("端点无法连接".into())));
    assert_eq!(outcomes[1], ("success".into(), None));
}
