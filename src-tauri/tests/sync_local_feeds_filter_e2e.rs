//! `sync_local_feeds` 定向推送与全量推送的真实 IPC / 执行器端到端证据。
//!
//! 证据边界：定向用例用 Tauri mock runtime 调用生产命令（不是复刻入队/推送
//! 逻辑），DB 是真实 SQLite，HTTP 是自托管 MockGReader。夹具预置：
//! - legacy add_feed 队列项（existing-legacy）不得被定向推送；
//! - pending subscribe outbox（existing-outbox）不得被定向推送；
//! - pending edit/move outbox（existing-edit）不得被定向执行；
//! - pending unsubscribe outbox（remote-only）不得被定向执行、远端必须保留；
//! - 本次目标（new）必须创建、绑定并确认出队。
//!
//! 全量用例直接调用 `feeds_phase_at` 的 None scope 路径，证明通用同步仍执行
//! 全部预置动作，不因定向实现而丢动作。

mod common;
mod mock_greader;

use app_lib::db;
use app_lib::media::MediaHandle;
use app_lib::state::AppState;
use mock_greader::MockGReader;
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::webview::InvokeRequest;

const LEGACY_URL: &str = "http://127.0.0.1:1/existing-legacy.xml";
const OUTBOX_URL: &str = "http://127.0.0.1:1/existing-outbox.xml";
const EDIT_URL: &str = "http://example.com/existing-edit.xml";
const TARGET_URL: &str = "http://127.0.0.1:1/new.xml";
const UNSUBSCRIBE_URL: &str = "http://example.com/remote-only.xml";
const EDIT_REMOTE_ID: i64 = 12;
const EDIT_REMOTE_STREAM: &str = "feed/12";

#[derive(Clone)]
struct Fixture {
    target_feed_id: i64,
    legacy_queue_id: i64,
    existing_outbox_id: i64,
    edit_outbox_id: i64,
    unsubscribe_id: i64,
}

fn seed_scope_case(
    name: &str,
    server: &MockGReader,
) -> (rusqlite::Connection, std::path::PathBuf, Fixture) {
    let tmp = common::unique_db_path(&format!("local_scope_{name}"));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).expect("open db");
    server
        .subscriptions
        .lock()
        .unwrap()
        .push(mock_greader::MockSubscription {
            id: EDIT_REMOTE_STREAM.into(),
            title: "Existing Edit".into(),
            url: EDIT_URL.into(),
            html_url: None,
            categories: vec![
                ("Old Cat".into(), "folder".into()),
                ("Keep Cat".into(), "folder".into()),
            ],
        });
    let folder = db::create_folder(&conn, "本地", "article").unwrap();
    let old_cat = db::create_folder(&conn, "Old Cat", "article").unwrap();
    db::create_folder(&conn, "Moved Cat", "article").unwrap();
    let existing_legacy = db::insert_feed(
        &conn,
        LEGACY_URL,
        None,
        "Existing Legacy",
        None,
        folder,
        "inherit",
        true,
        false,
    )
    .unwrap();
    let existing_outbox = db::insert_feed(
        &conn,
        OUTBOX_URL,
        None,
        "Existing Outbox",
        None,
        folder,
        "inherit",
        true,
        false,
    )
    .unwrap();
    let existing_edit = db::insert_feed(
        &conn,
        EDIT_URL,
        None,
        "Existing Edit",
        None,
        old_cat,
        "inherit",
        true,
        false,
    )
    .unwrap();
    db::set_feed_remote_id(&conn, existing_edit, EDIT_REMOTE_ID).unwrap();
    let target_feed_id = db::insert_feed(
        &conn,
        TARGET_URL,
        None,
        "New Local",
        None,
        folder,
        "inherit",
        true,
        false,
    )
    .unwrap();
    db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
    db::set_setting(&conn, "greader_username", "t").unwrap();
    db::set_setting(&conn, "greader_password", "pw").unwrap();
    db::set_setting(&conn, "sync_protocol", "greader").unwrap();

    let ctx = app_lib::sync::current_account(&conn)
        .unwrap()
        .expect("configured account");
    let payload = serde_json::json!({ "folder_id": folder }).to_string();
    db::enqueue_sync(&conn, None, Some(LEGACY_URL), "add_feed", Some(&payload)).unwrap();
    let existing_outbox_id =
        db::enqueue_subscribe(&conn, ctx.generation, &ctx.key, OUTBOX_URL, None, None).unwrap();
    db::enqueue_edit(
        &conn,
        ctx.generation,
        &ctx.key,
        EDIT_URL,
        EDIT_REMOTE_ID,
        Some("Existing Edit Updated"),
        Some("Moved Cat"),
        Some("Old Cat"),
    )
    .unwrap();
    assert!(db::enqueue_unsubscribe(
        &conn,
        ctx.generation,
        &ctx.key,
        UNSUBSCRIBE_URL,
        Some(11),
        None
    )
    .unwrap());
    let pending = db::pending_outbox(&conn).unwrap();
    let unsubscribe_id = pending
        .iter()
        .find(|item| item.action == db::ACTION_UNSUBSCRIBE && item.feed_url == UNSUBSCRIBE_URL)
        .expect("unsubscribe intent")
        .id;
    assert!(
        pending.iter().any(|item| item.id == existing_outbox_id),
        "pending subscribe fixture must exist"
    );
    let edit_outbox_id = pending
        .iter()
        .find(|item| item.action == db::ACTION_EDIT && item.feed_url == EDIT_URL)
        .expect("pending edit/move fixture")
        .id;
    let legacy_queue_id = db::take_sync_queue(&conn)
        .unwrap()
        .into_iter()
        .find(|item| item.action == "add_feed" && item.feed_url.as_deref() == Some(LEGACY_URL))
        .expect("legacy add_feed fixture")
        .id;

    for (id, url) in [(existing_legacy, LEGACY_URL), (existing_outbox, OUTBOX_URL)] {
        assert_eq!(
            db::feed_url_by_id(&conn, id).unwrap().as_deref(),
            Some(url),
            "fixture feed {id} must stay unbound"
        );
    }
    (
        conn,
        tmp,
        Fixture {
            target_feed_id,
            legacy_queue_id,
            existing_outbox_id,
            edit_outbox_id,
            unsubscribe_id,
        },
    )
}

fn invoke_sync_local_feeds(
    conn: rusqlite::Connection,
    path: std::path::PathBuf,
    selector: serde_json::Value,
    fixture: &Fixture,
    server: &MockGReader,
) {
    let app = tauri::test::mock_builder()
        .manage(AppState::new(
            conn,
            app_lib::ingestion::build_client(10),
            MediaHandle::inactive(),
        ))
        .invoke_handler(tauri::generate_handler![
            app_lib::commands::sync_local_feeds
        ])
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build mock app");
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("build mock webview");
    let mut body = serde_json::Map::new();
    for (key, value) in selector.as_object().cloned().unwrap_or_default() {
        body.insert(key, value);
    }
    let response = tauri::test::get_ipc_response(
        &webview,
        InvokeRequest {
            cmd: "sync_local_feeds".into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: if cfg!(any(windows, target_os = "android")) {
                "http://tauri.localhost"
            } else {
                "tauri://localhost"
            }
            .parse()
            .unwrap(),
            body: InvokeBody::Json(serde_json::Value::Object(body)),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_string(),
        },
    )
    .expect("sync_local_feeds command response");
    let value = response.deserialize::<serde_json::Value>().unwrap();
    assert_eq!(value["ok"], true, "定向同步应成功: {value}");
    assert_eq!(value["queued"], 1, "只入队本次目标: {value}");
    assert_eq!(value["pushed"], 1, "只推送本次目标: {value}");
    assert_eq!(value["failed"], 0, "定向推送不得带入其它失败: {value}");

    drop(app);
    let conn = db::open(&path).unwrap();
    let pending = db::pending_outbox(&conn).unwrap();
    assert!(
        pending
            .iter()
            .any(|item| item.id == fixture.existing_outbox_id),
        "existing pending subscribe must remain pending"
    );
    assert!(
        pending.iter().any(|item| item.id == fixture.unsubscribe_id),
        "unrelated unsubscribe must remain pending"
    );
    assert!(
        pending.iter().any(|item| item.id == fixture.edit_outbox_id),
        "unrelated edit/move must remain pending"
    );
    assert!(
        pending.iter().all(|item| item.feed_url != TARGET_URL),
        "target subscribe must be confirmed and removed"
    );
    let queue = db::take_sync_queue(&conn).unwrap();
    assert!(
        queue.iter().any(|item| item.id == fixture.legacy_queue_id),
        "existing legacy add_feed must remain queued"
    );
    let existing_left: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM feeds WHERE feed_url IN (?1, ?2) AND remote_id IS NULL",
            rusqlite::params![LEGACY_URL, OUTBOX_URL],
            |r| r.get(0),
        )
        .unwrap();
    let target_bound: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM feeds WHERE id = ?1 AND remote_id IS NOT NULL",
            rusqlite::params![fixture.target_feed_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(existing_left, 2, "existing local feeds must stay unbound");
    assert_eq!(target_bound, 1, "target should be remotely bound");
    let _ = std::fs::remove_file(&path);

    let created = server.created_feeds.lock().unwrap().clone();
    assert_eq!(
        created.len(),
        1,
        "only the target may be created: {created:?}"
    );
    assert!(
        created[0].0.ends_with("/new.xml"),
        "created feed must be the target: {created:?}"
    );
    assert!(
        server
            .subscriptions
            .lock()
            .unwrap()
            .iter()
            .find(|sub| sub.id == EDIT_REMOTE_STREAM)
            .is_some_and(|sub| {
                sub.title == "Existing Edit"
                    && sub.categories.iter().any(|(label, _)| label == "Old Cat")
                    && !sub.categories.iter().any(|(label, _)| label == "Moved Cat")
            }),
        "unrelated edit/move must not reach the remote"
    );
    assert!(
        !server
            .subscription_edits
            .lock()
            .unwrap()
            .iter()
            .any(|(action, stream)| action == "edit" && stream == EDIT_REMOTE_STREAM),
        "targeted sync must not send an edit request for unrelated feeds"
    );
    assert!(
        server
            .subscriptions
            .lock()
            .unwrap()
            .iter()
            .any(|sub| sub.id == "feed/11"),
        "unrelated unsubscribe must not reach the remote"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn feed_id_filter_scopes_legacy_and_outbox_push_to_target() {
    let server = MockGReader::start().await.expect("start mock");
    let (conn, path, fixture) = seed_scope_case("id", &server);
    let selector = serde_json::json!({ "feedId": fixture.target_feed_id });
    let server = server.clone();
    tokio::task::spawn_blocking(move || {
        invoke_sync_local_feeds(conn, path, selector, &fixture, &server)
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn feed_url_filter_scopes_legacy_and_outbox_push_to_target() {
    let server = MockGReader::start().await.expect("start mock");
    let (conn, path, fixture) = seed_scope_case("url", &server);
    let selector = serde_json::json!({ "feedUrl": TARGET_URL });
    let server = server.clone();
    tokio::task::spawn_blocking(move || {
        invoke_sync_local_feeds(conn, path, selector, &fixture, &server)
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn empty_feed_url_is_rejected_without_remote_writes() {
    let server = MockGReader::start().await.expect("start mock");
    let (conn, path, _fixture) = seed_scope_case("empty", &server);
    let app = tauri::test::mock_builder()
        .manage(AppState::new(
            conn,
            app_lib::ingestion::build_client(10),
            MediaHandle::inactive(),
        ))
        .invoke_handler(tauri::generate_handler![
            app_lib::commands::sync_local_feeds
        ])
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build mock app");
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("build mock webview");
    let response = tauri::test::get_ipc_response(
        &webview,
        InvokeRequest {
            cmd: "sync_local_feeds".into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: if cfg!(any(windows, target_os = "android")) {
                "http://tauri.localhost"
            } else {
                "tauri://localhost"
            }
            .parse()
            .unwrap(),
            body: InvokeBody::Json(serde_json::json!({ "feedUrl": "   " })),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_string(),
        },
    );
    assert!(response.is_err(), "empty feedUrl must be rejected");
    let message = format!("{:?}", response.unwrap_err());
    assert!(
        message.contains("feedUrl 不能为空"),
        "error should explain the empty selector: {message}"
    );
    drop(app);
    assert!(
        server.created_feeds.lock().unwrap().is_empty(),
        "empty selector must not create any remote feed"
    );
    let _ = std::fs::remove_file(&path);
}

#[tokio::test(flavor = "multi_thread")]
async fn full_feeds_phase_still_executes_all_preexisting_intents() {
    let server = MockGReader::start().await.expect("start mock");
    let (conn, path, fixture) = seed_scope_case("full", &server);
    let generation = db::sync_generation(&conn).unwrap();
    let db = std::sync::Arc::new(tokio::sync::Mutex::new(conn));
    let http = app_lib::ingestion::build_client(10);
    let report = app_lib::sync::feeds_phase_at(&db, &http, generation)
        .await
        .expect("full feeds phase");
    assert_eq!(
        report.pushed_feeds, 2,
        "legacy + outbox subscribe both pushed"
    );

    drop(db);
    let conn = db::open(&path).unwrap();
    let queue = db::take_sync_queue(&conn).unwrap();
    assert!(
        queue.iter().all(|item| item.id != fixture.legacy_queue_id),
        "full sync must consume legacy add_feed"
    );
    let pending = db::pending_outbox(&conn).unwrap();
    assert!(
        pending
            .iter()
            .all(|item| item.id != fixture.existing_outbox_id),
        "full sync must confirm outbox subscribe"
    );
    assert!(
        pending.iter().all(|item| item.id != fixture.unsubscribe_id),
        "full sync must execute unrelated unsubscribe intent"
    );
    assert!(
        pending.iter().all(|item| item.id != fixture.edit_outbox_id),
        "full sync must execute unrelated edit/move intent"
    );
    assert!(
        server
            .subscription_edits
            .lock()
            .unwrap()
            .iter()
            .any(|(action, stream)| action == "edit" && stream == EDIT_REMOTE_STREAM),
        "full sync must send edit/move to the remote"
    );
    let edited_remote = server
        .subscriptions
        .lock()
        .unwrap()
        .iter()
        .find(|sub| sub.id == EDIT_REMOTE_STREAM)
        .cloned()
        .expect("edited remote subscription");
    assert_eq!(edited_remote.title, "Existing Edit Updated");
    assert!(
        edited_remote
            .categories
            .iter()
            .any(|(label, _)| label == "Moved Cat"),
        "full sync must add the destination label: {:?}",
        edited_remote.categories
    );
    assert!(
        !edited_remote
            .categories
            .iter()
            .any(|(label, _)| label == "Old Cat"),
        "full sync must remove the old label: {:?}",
        edited_remote.categories
    );
    assert!(
        edited_remote
            .categories
            .iter()
            .any(|(label, _)| label == "Keep Cat"),
        "full sync must preserve non-target labels: {:?}",
        edited_remote.categories
    );
    assert!(
        server
            .subscriptions
            .lock()
            .unwrap()
            .iter()
            .all(|sub| sub.id != "feed/11"),
        "full sync must remove the remote feed for unsubscribe"
    );
    let created = server.created_feeds.lock().unwrap().clone();
    assert_eq!(
        created.len(),
        2,
        "both preexisting intents create remote feeds"
    );
    assert!(created.iter().any(|(url, _)| url == LEGACY_URL));
    assert!(created.iter().any(|(url, _)| url == OUTBOX_URL));
    let _ = std::fs::remove_file(&path);
}
