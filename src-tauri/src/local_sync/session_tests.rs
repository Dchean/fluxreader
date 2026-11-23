//! Session-check failures retain their IPC shape without impersonating DAV leases.
use super::*;

async fn rejected_session(
    database: Arc<Mutex<Connection>>,
    session: Session,
    code: &str,
    message: &str,
) {
    let direct = ensure_session(&database, &session).await.unwrap_err();
    assert_eq!(direct.code, code);
    assert_eq!(direct.message, message);
    let original_message = direct.to_string();
    let guarded = transport_guard(&database, &session).await.unwrap_err();
    let kind = format!("{:?}", guarded.kind);
    assert_eq!(guarded.message, original_message);
    let public = transport_error(guarded);
    assert_eq!(
        serde_json::to_value(&public).unwrap(),
        serde_json::json!({
            "code": "localSyncLease", "message": original_message,
        })
    );
    assert_eq!(
        public.to_string(),
        format!("[localSyncLease] {original_message}")
    );
    assert_eq!(
        kind, "SessionCheckFailed",
        "a local session check is not a WebDAV lease expiry"
    );
}

#[tokio::test]
async fn x25_current_session_passes_the_real_database_guard() {
    let (conn, session) = segment2_schedule_tests::fixture();
    let database = Arc::new(Mutex::new(conn));
    ensure_session(&database, &session).await.unwrap();
    transport_guard(&database, &session).await.unwrap();
}

#[tokio::test]
async fn x25_stopped_session_keeps_cancelled_message_and_legacy_ipc() {
    let (conn, session) = segment2_schedule_tests::fixture();
    session.stopped.store(true, Ordering::Release);
    rejected_session(
        Arc::new(Mutex::new(conn)),
        session,
        "cancelled",
        "同步已停止，未完成操作保留待重试",
    )
    .await;
}

#[tokio::test]
async fn x25_generation_change_is_a_session_failure_with_unchanged_ipc() {
    let (conn, mut session) = segment2_schedule_tests::fixture();
    session.generation += 1;
    rejected_session(
        Arc::new(Mutex::new(conn)),
        session,
        "staleLocalSync",
        "同步会话身份已变化",
    )
    .await;
}

#[tokio::test]
async fn x25_library_change_is_a_session_failure_with_unchanged_ipc() {
    let (conn, mut session) = segment2_schedule_tests::fixture();
    session.library = "2".repeat(32);
    rejected_session(
        Arc::new(Mutex::new(conn)),
        session,
        "staleLocalSync",
        "同步会话身份已变化",
    )
    .await;
}

#[tokio::test]
async fn x25_actor_change_is_a_session_failure_with_unchanged_ipc() {
    let (conn, mut session) = segment2_schedule_tests::fixture();
    session.actor = if session.actor == "2".repeat(32) {
        "3"
    } else {
        "2"
    }
    .repeat(32);
    rejected_session(
        Arc::new(Mutex::new(conn)),
        session,
        "staleLocalSync",
        "同步会话身份已变化",
    )
    .await;
}

#[tokio::test]
async fn x25_database_check_failure_is_not_a_remote_lease_expiry() {
    let (conn, session) = segment2_schedule_tests::fixture();
    conn.execute("DROP TABLE local_sync_control", []).unwrap();
    rejected_session(
        Arc::new(Mutex::new(conn)),
        session,
        "db",
        "no such table: local_sync_control",
    )
    .await;
}

#[tokio::test]
async fn x25_guarded_write_rejects_stopped_session_before_http() {
    let (conn, session) = segment2_schedule_tests::fixture();
    let database = Arc::new(Mutex::new(conn));
    session.stopped.store(true, Ordering::Release);
    // The fixture points to a closed loopback port: reaching HTTP yields Network,
    // so this also detects moving the first session check after the first GET.
    let error = session
        .transport
        .library_store_guarded(
            ".fluxreader-v3/state/intent-session.json",
            b"sealed",
            || transport_guard(&database, &session),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.message,
        "[cancelled] 同步已停止，未完成操作保留待重试"
    );
    assert_eq!(format!("{:?}", error.kind), "SessionCheckFailed");
}

#[test]
fn x25_other_transport_errors_keep_public_codes_and_messages() {
    use crate::webdav_transport::{ErrorKind, TransportError};
    for (kind, code) in [
        (ErrorKind::Network, "localSyncNetwork"),
        (ErrorKind::OutcomeUnknown, "localSyncUnknown"),
        (ErrorKind::TooLarge, "localSyncCapacity"),
        (ErrorKind::Conflict, "localSyncConflict"),
        (ErrorKind::InvalidXml, "localSyncFormat"),
        (ErrorKind::InvalidState, "localSyncFormat"),
        (ErrorKind::InvalidInput, "validate"),
        (ErrorKind::Http(404), "localSyncNotFound"),
        (ErrorKind::Http(503), "localSyncHttp"),
    ] {
        let public = transport_error(TransportError {
            kind,
            message: "unchanged".to_owned(),
        });
        assert_eq!(
            serde_json::to_value(public).unwrap(),
            serde_json::json!({"code": code, "message": "unchanged"})
        );
    }
}
