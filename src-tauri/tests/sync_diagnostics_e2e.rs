mod common;
mod mock_greader;

use app_lib::sync::runtime::{self, RuntimeResult};
use app_lib::{db, diagnostics, ingestion, sync};
use mock_greader::MockGReader;
use std::sync::Arc;

#[tokio::test]
async fn manual_client_login_401_keeps_auth_diagnostics_and_compatible_ipc() {
    let directory = common::unique_db_path("manual_auth_diagnostics").with_extension("logs");
    diagnostics::install(&directory).unwrap();
    let server = MockGReader::start().await.unwrap();
    server.set_reject_login(true);
    let conn = db::open(std::path::Path::new(":memory:")).unwrap();
    db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
    db::set_setting(&conn, "greader_username", "diagnostics-test").unwrap();
    db::set_setting(&conn, "greader_password", "loopback-only").unwrap();
    let database = Arc::new(tokio::sync::Mutex::new(conn));
    let http = ingestion::build_client(3);

    let direct_error = sync::feeds_phase(&database, &http).await.unwrap_err();
    assert_eq!(direct_error.code, "network");
    assert!(direct_error.is_auth_failure());
    let result = runtime::run_manual(&database, &http).await.unwrap();
    let classified = result.diagnostic_error().unwrap();
    let returned = result.error.as_ref().unwrap();
    assert_eq!(
        serde_json::to_value(returned).unwrap(),
        serde_json::json!({
            "stage": "feeds",
            "code": "network",
            "message": returned.message,
        })
    );
    assert_eq!(server.login_request_count(), 2);

    let text = std::fs::read_to_string(directory.join("sync-0.log")).unwrap();
    let records: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    std::fs::remove_file(directory.join("sync-0.log")).unwrap();
    std::fs::remove_dir(&directory).unwrap();
    let direct = records
        .iter()
        .find(|entry| entry["source"] == "feeds" && entry["outcome"] == "error")
        .unwrap();
    assert_eq!(direct["error_class"], "auth");
    let manual = records
        .iter()
        .find(|entry| entry["activity_id"] == result.activity_id && entry["outcome"] == "error")
        .unwrap();
    assert_eq!(manual["source"], "manual");
    assert_eq!(manual["error_class"], "auth", "{text}");
    assert_eq!(classified.code, "network");
    assert!(classified.is_auth_failure());
    assert!(classified.message.is_empty());
}
