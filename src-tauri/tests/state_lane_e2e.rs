//! Production entrypoint regressions for state work blocked behind a slow content pull.
//! The HTTP barrier fixes the old response before the action; no sleep guesses its phase.
mod common;
mod mock_greader;

use app_lib::{commands, db, sync};
use mock_greader::MockGReader;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

const FAST_BUDGET: Duration = Duration::from_secs(3);
const FINISH_BUDGET: Duration = Duration::from_secs(10);

async fn setup(
    name: &str,
) -> (
    common::TempDbGuard,
    Arc<Mutex<rusqlite::Connection>>,
    reqwest::Client,
    Arc<MockGReader>,
    i64,
) {
    let server = MockGReader::start().await.unwrap();
    let path = common::unique_db_path(name);
    let guard = common::TempDbGuard::new(path.clone());
    let conn = db::open(&path).unwrap();
    db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
    db::set_setting(&conn, "greader_username", "test").unwrap();
    db::set_setting(&conn, "greader_password", "test-token").unwrap();
    // An existing, bound entry. Fixture writes do not simulate a user mutation or ACK.
    conn.execute_batch(
        "INSERT INTO feeds(id,feed_url,title,remote_id)
         VALUES(1,'http://127.0.0.1:8765/local_feed.xml','State lane',10);
         INSERT INTO articles(id,feed_id,guid,title,url,remote_id)
         VALUES(1,1,'state-lane','State lane','https://state-lane.invalid/item',501);",
    )
    .unwrap();
    let remote_id = server.add_entry_with_published(
        10,
        "https://state-lane.invalid/item",
        "State lane",
        false,
        false,
        1_600_000_000,
    );
    assert_eq!(remote_id, 501);
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    (guard, Arc::new(Mutex::new(conn)), http, server, remote_id)
}

async fn flags(database: &Arc<Mutex<rusqlite::Connection>>) -> (bool, bool) {
    database
        .lock()
        .await
        .query_row(
            "SELECT is_read,is_starred FROM articles WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
}

async fn wait_for_ack(
    database: &Arc<Mutex<rusqlite::Connection>>,
    server: &MockGReader,
    remote_id: i64,
) {
    loop {
        let accepted = server
            .status_updates
            .lock()
            .unwrap()
            .iter()
            .any(|(id, action)| *id == remote_id && action == "read");
        let drained = db::take_sync_queue(&*database.lock().await)
            .unwrap()
            .is_empty();
        if accepted && drained {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn slow_full_pull_does_not_block_production_state_push() {
    let (_guard, database, http, server, remote_id) = setup("state_lane_push").await;
    let barrier = server.arm_contents_barrier(server.contents_calls.load(Ordering::SeqCst) + 1);
    let slow = {
        let database = database.clone();
        let http = http.clone();
        tokio::spawn(async move { sync::states_phase(&database, &http, true).await })
    };
    tokio::time::timeout(FINISH_BUDGET, barrier.arrived)
        .await
        .expect("full pull did not reach the controlled contents response")
        .unwrap();
    assert!(
        !slow.is_finished(),
        "the old content response must remain held"
    );
    db::set_read_with_enqueue(&*database.lock().await, 1, true).unwrap();
    assert_eq!(
        db::take_sync_queue(&*database.lock().await).unwrap().len(),
        1
    );
    commands::schedule_state_push_for_db(&database, &http);
    let ack_before_release =
        tokio::time::timeout(FAST_BUDGET, wait_for_ack(&database, &server, remote_id))
            .await
            .is_ok();
    let slow_still_held = !slow.is_finished();
    let waiting_before_release = db::take_sync_queue(&*database.lock().await).unwrap().len();
    eprintln!(
        "STATE_LANE_PUSH before_release ack={ack_before_release} slow_held={slow_still_held} queue={waiting_before_release}"
    );

    // Always release and finish the known work before asserting the regression.
    barrier.release.send(()).unwrap();
    let report = tokio::time::timeout(FINISH_BUDGET, slow)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let recovered =
        tokio::time::timeout(FINISH_BUDGET, wait_for_ack(&database, &server, remote_id))
            .await
            .is_ok();
    let final_flags = flags(&database).await;
    let drained = sync::runtime::for_db(&database)
        .drain(Duration::from_secs(2))
        .await;
    eprintln!("STATE_LANE_PUSH after_release recovered={recovered} flags={final_flags:?}");
    assert!(drained, "runtime cleanup must finish");
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(
        recovered,
        "released baseline must prove HTTP and ACK are functional"
    );
    assert!(
        final_flags.0,
        "old content may not undo the local intent after ACK"
    );
    assert!(slow_still_held);
    assert!(
        ack_before_release,
        "new read must reach the server and ACK before slow content is released"
    );
}

#[tokio::test]
async fn slow_full_pull_does_not_block_new_remote_flags_or_allow_late_rollback() {
    let (_guard, database, http, server, remote_id) = setup("state_lane_receive").await;
    database.lock().await.execute_batch(
        "CREATE TABLE state_lane_trace(seq INTEGER PRIMARY KEY,is_read INTEGER,is_starred INTEGER);
         CREATE TRIGGER state_lane_trace_flags AFTER UPDATE OF is_read,is_starred ON articles
         WHEN NEW.id=1 AND (OLD.is_read!=NEW.is_read OR OLD.is_starred!=NEW.is_starred)
         BEGIN INSERT INTO state_lane_trace(is_read,is_starred) VALUES(NEW.is_read,NEW.is_starred); END;",
    ).unwrap();
    let barrier = server.arm_contents_barrier(server.contents_calls.load(Ordering::SeqCst) + 1);
    let slow = {
        let database = database.clone();
        let http = http.clone();
        tokio::spawn(async move { sync::states_phase(&database, &http, true).await })
    };
    tokio::time::timeout(FINISH_BUDGET, barrier.arrived)
        .await
        .unwrap()
        .unwrap();
    {
        let mut entries = server.entries.lock().unwrap();
        let entry = entries
            .iter_mut()
            .find(|entry| entry.id == remote_id)
            .unwrap();
        entry.read = true;
        entry.starred = true;
        entry.changed_at += 1;
    }
    let incoming = {
        let database = database.clone();
        let http = http.clone();
        tokio::spawn(async move { sync::sync_states_now(&database, &http).await })
    };
    let received_before_release = tokio::time::timeout(FAST_BUDGET, async {
        while flags(&database).await != (true, true) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .is_ok();
    let marker: i64 = database
        .lock()
        .await
        .query_row(
            "SELECT COALESCE(MAX(seq),0) FROM state_lane_trace",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let old_held = !slow.is_finished();
    let versions = db::article_field_versions(&*database.lock().await, 1).unwrap();
    eprintln!("STATE_LANE_RECEIVE before_release received={received_before_release} slow_held={old_held} versions={versions:?}");
    barrier.release.send(()).unwrap();
    let old_report = tokio::time::timeout(FINISH_BUDGET, slow)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let new_report = tokio::time::timeout(FINISH_BUDGET, incoming)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let rolled_back: bool = database.lock().await.query_row(
        "SELECT EXISTS(SELECT 1 FROM state_lane_trace WHERE seq>?1 AND (is_read=0 OR is_starred=0))",
        [marker], |row| row.get(0),
    ).unwrap();
    let final_flags = flags(&database).await;
    let drained = sync::runtime::for_db(&database)
        .drain(Duration::from_secs(2))
        .await;
    eprintln!("STATE_LANE_RECEIVE after_release flags={final_flags:?} rollback={rolled_back}");
    assert!(drained);
    assert!(old_report.errors.is_empty(), "{:?}", old_report.errors);
    assert!(new_report.errors.is_empty(), "{:?}", new_report.errors);
    assert_eq!(
        versions,
        Some((0, 0)),
        "no local field-version change may mask the race"
    );
    assert_eq!(
        final_flags,
        (true, true),
        "released baseline must eventually receive remote state"
    );
    assert!(old_held);
    assert!(
        received_before_release,
        "remote state must be applied while slow content is still held"
    );
    assert!(
        !rolled_back,
        "old content may not transiently undo newer remote flags"
    );
}

#[tokio::test]
async fn fever_state_and_local_push_continue_while_history_response_is_held() {
    let (_guard, database, http, server, remote_id) = setup("state_lane_fever").await;
    server.set_fever_serve_entries(true);
    server.set_fever_api_version(4);
    {
        let conn = database.lock().await;
        db::set_setting(&conn, "sync_protocol", "fever").unwrap();
        // The shared mock's Fever feed IDs are subscription positions, not GR IDs.
        conn.execute("UPDATE feeds SET remote_id=1 WHERE id=1", [])
            .unwrap();
    }
    let barrier = server.arm_fever_items_barrier(1);
    let slow = {
        let database = database.clone();
        let http = http.clone();
        tokio::spawn(async move { sync::states_phase(&database, &http, true).await })
    };
    tokio::time::timeout(FINISH_BUDGET, barrier.arrived)
        .await
        .unwrap()
        .unwrap();
    {
        let mut entries = server.entries.lock().unwrap();
        let entry = entries
            .iter_mut()
            .find(|entry| entry.id == remote_id)
            .unwrap();
        entry.read = true;
        entry.starred = true;
    }
    let incoming = tokio::time::timeout(FAST_BUDGET, sync::sync_states_now(&database, &http))
        .await
        .expect("Fever authority read must not wait for history")
        .unwrap();
    assert!(incoming.errors.is_empty(), "{:?}", incoming.errors);
    assert_eq!(flags(&database).await, (true, true));
    // Both inverse local actions must be accepted/ACKed while the first history
    // page is still held. Newer item verification uses its own request slot.
    {
        let conn = database.lock().await;
        db::set_read_with_enqueue(&conn, 1, false).unwrap();
        db::set_starred_with_enqueue(&conn, 1, false).unwrap();
    }
    let pushed = tokio::time::timeout(FAST_BUDGET, sync::push_states_now(&database, &http))
        .await
        .is_ok();
    let acked = db::take_sync_queue(&*database.lock().await)
        .unwrap()
        .is_empty();
    let old_held = !slow.is_finished();
    barrier.release.send(()).unwrap();
    let report = tokio::time::timeout(FINISH_BUDGET, slow)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let final_flags = flags(&database).await;
    let drained = sync::runtime::for_db(&database)
        .drain(Duration::from_secs(2))
        .await;
    assert!(drained);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(old_held && pushed && acked);
    assert_eq!(final_flags, (false, false));
    let updates = server.status_updates.lock().unwrap();
    for action in ["unread", "unstar"] {
        assert!(updates
            .iter()
            .any(|(id, seen)| *id == remote_id && seen == action));
    }
}

#[tokio::test]
async fn state_snapshot_cannot_undo_a_local_operation_after_real_ack() {
    let (_guard, database, http, server, remote_id) = setup("state_lane_state_ack").await;
    let barrier = server.arm_stream_ids_barrier("reading-list");
    let state = {
        let database = database.clone();
        let http = http.clone();
        tokio::spawn(async move { sync::sync_states_now(&database, &http).await })
    };
    tokio::time::timeout(FINISH_BUDGET, barrier.arrived)
        .await
        .unwrap()
        .unwrap();
    db::set_read_with_enqueue(&*database.lock().await, 1, true).unwrap();
    let pushed = tokio::time::timeout(FAST_BUDGET, sync::push_states_now(&database, &http))
        .await
        .is_ok();
    let acked = db::take_sync_queue(&*database.lock().await)
        .unwrap()
        .is_empty();
    barrier.release.send(()).unwrap();
    let report = tokio::time::timeout(FINISH_BUDGET, state)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let local = flags(&database).await;
    let drained = sync::runtime::for_db(&database)
        .drain(Duration::from_secs(2))
        .await;
    assert!(drained && pushed && acked);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(
        local.0,
        "version guard survives an empty queue after the real ACK"
    );
    assert!(server
        .status_updates
        .lock()
        .unwrap()
        .iter()
        .any(|(id, action)| *id == remote_id && action == "read"));
}

#[tokio::test]
async fn state_lane_rejects_old_account_snapshot_after_generation_change() {
    let (_guard, database, http, server, _remote_id) = setup("state_lane_generation").await;
    let barrier = server.arm_stream_ids_barrier("reading-list");
    let state = {
        let database = database.clone();
        let http = http.clone();
        tokio::spawn(async move { sync::sync_states_now(&database, &http).await })
    };
    tokio::time::timeout(FINISH_BUDGET, barrier.arrived)
        .await
        .unwrap()
        .unwrap();
    {
        let conn = database.lock().await;
        db::bump_sync_generation(&conn).unwrap();
        db::set_read_with_enqueue(&conn, 1, true).unwrap();
    }
    barrier.release.send(()).unwrap();
    let error = tokio::time::timeout(FINISH_BUDGET, state)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    let local = flags(&database).await;
    let pending = db::take_sync_queue(&*database.lock().await).unwrap().len();
    assert!(
        sync::runtime::for_db(&database)
            .drain(Duration::from_secs(2))
            .await
    );
    assert_eq!(error.code, "staleSession");
    assert!(local.0);
    assert_eq!(pending, 1);
}

#[tokio::test]
async fn final_state_write_failure_keeps_content_cursor_and_committed_change_report() {
    let (_guard, database, http, server, remote_id) = setup("state_lane_late_failure").await;
    db::set_last_sync_ts(&*database.lock().await, 100).unwrap();
    let barrier = server.arm_contents_barrier(1);
    let full = {
        let database = database.clone();
        let http = http.clone();
        tokio::spawn(async move { sync::states_phase(&database, &http, true).await })
    };
    tokio::time::timeout(FINISH_BUDGET, barrier.arrived)
        .await
        .unwrap()
        .unwrap();
    {
        let mut entries = server.entries.lock().unwrap();
        let entry = entries
            .iter_mut()
            .find(|entry| entry.id == remote_id)
            .unwrap();
        entry.read = true;
        entry.starred = true;
    }
    database
        .lock()
        .await
        .execute_batch(
            "CREATE TRIGGER state_lane_late_star BEFORE UPDATE OF is_starred ON articles
         WHEN NEW.is_starred=1 AND OLD.is_starred=0
         BEGIN SELECT RAISE(ABORT,'late state write failure'); END;",
        )
        .unwrap();
    barrier.release.send(()).unwrap();
    let report = tokio::time::timeout(FINISH_BUDGET, full)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let local = flags(&database).await;
    let cursor = db::last_sync_ts(&*database.lock().await).unwrap();
    assert!(
        sync::runtime::for_db(&database)
            .drain(Duration::from_secs(2))
            .await
    );
    assert_eq!(
        local,
        (true, false),
        "read committed before the independent star write failed"
    );
    assert!(report
        .errors
        .iter()
        .any(|error| error.contains("late state write failure")));
    assert!(
        report.has_data_change(),
        "failure cannot erase a prior committed change"
    );
    assert_eq!(
        cursor, 100,
        "late state failure cannot acknowledge the complete content window"
    );
}

// A1/A2: independent audit counterexamples promoted with their original assertions.
#[tokio::test]
async fn fever_partial_commit_retains_refresh_signal() {
    let (_guard, database, http, server, _) = setup("audit_fever_partial_commit").await;
    server.set_fever_serve_entries(true);
    server.set_fever_api_version(4);
    {
        let conn = database.lock().await;
        db::set_setting(&conn, "sync_protocol", "fever").unwrap();
        conn.execute("UPDATE feeds SET remote_id=1 WHERE id=1", [])
            .unwrap();
        conn.execute_batch(
            "CREATE TRIGGER audit_fail_star BEFORE UPDATE OF is_starred ON articles
             WHEN NEW.id=1 AND OLD.is_starred=0 AND NEW.is_starred=1
             BEGIN SELECT RAISE(ABORT,'audit star write rejected'); END;",
        )
        .unwrap();
    }
    {
        let mut entries = server.entries.lock().unwrap();
        entries[0].read = true;
        entries[0].starred = true;
    }
    let report = sync::sync_states_now(&database, &http).await.unwrap();
    let flags: (bool, bool) = database
        .lock()
        .await
        .query_row(
            "SELECT is_read,is_starred FROM articles WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let drained = sync::runtime::for_db(&database)
        .drain(Duration::from_secs(2))
        .await;
    eprintln!(
        "AUDIT_FEVER_PARTIAL flags={flags:?} entries_changed={} merged={} has_change={} incomplete={} errors={:?}",
        report.entries_data_changed,
        report.merged_states,
        report.has_data_change(),
        report.states_incomplete,
        report.errors,
    );
    assert!(drained);
    assert_eq!(
        flags,
        (true, false),
        "read really committed before star failed"
    );
    assert!(report
        .errors
        .iter()
        .any(|error| error.contains("audit star write rejected")));
    assert!(
        report.has_data_change(),
        "the committed read must still cause the fast path to publish feeds-updated"
    );
}

#[tokio::test]
async fn discovery_retry_does_not_skip_new_source_then_consume_window() {
    let (_guard, database, http, server, _) = setup("audit_new_source_window").await;
    let new_remote_id = server.add_entry_with_published(
        11,
        "https://audit.invalid/new-source-entry",
        "Read unstarred entry from a newly discovered source",
        true,
        false,
        1_600_000_000,
    );
    db::set_last_sync_ts(&*database.lock().await, 100).unwrap();
    let full = sync::states_phase(&database, &http, true).await.unwrap();
    let (new_source_bound, imported_after_full, cursor_after_full): (bool, bool, i64) = {
        let conn = database.lock().await;
        (
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM feeds WHERE remote_id=11 AND active=1)",
                [],
                |row| row.get(0),
            )
            .unwrap(),
            db::article_by_remote_id(&conn, new_remote_id)
                .unwrap()
                .is_some(),
            db::last_sync_ts(&conn).unwrap(),
        )
    };
    let incremental = sync::states_phase(&database, &http, false).await.unwrap();
    let imported_after_incremental =
        db::article_by_remote_id(&*database.lock().await, new_remote_id)
            .unwrap()
            .is_some();
    let drained = sync::runtime::for_db(&database)
        .drain(Duration::from_secs(2))
        .await;
    eprintln!(
        "AUDIT_NEW_SOURCE bound={new_source_bound} imported_after_full={imported_after_full} cursor_after_full={cursor_after_full} imported_after_incremental={imported_after_incremental} full_binding_incomplete={} full_errors={:?} incremental_errors={:?}",
        full.binding_incomplete, full.errors, incremental.errors,
    );
    assert!(drained);
    assert!(
        new_source_bound,
        "the same full round did discover and bind the source"
    );
    assert!(
        imported_after_full || cursor_after_full == 100,
        "a skipped newly discovered source cannot consume the complete content window"
    );
    assert!(
        imported_after_incremental,
        "a normal subsequent incremental round must still import the skipped entry"
    );
}

async fn discovery_receipt_keeps_old_source_lifecycle(protocol: &str) {
    let (_guard, database, http, server, old_remote_id) = setup("state_lane_discovery_aba").await;
    let new_remote_id = server.add_entry_with_published(
        11,
        "https://state-lane.invalid/new-source",
        "New source item",
        true,
        false,
        1_600_000_000,
    );
    {
        let mut entries = server.entries.lock().unwrap();
        let old = entries
            .iter_mut()
            .find(|entry| entry.id == old_remote_id)
            .unwrap();
        old.title = "old response must not rewrite the retained item".into();
        old.starred = true;
    }
    {
        let conn = database.lock().await;
        // Preserve the known row across retirement; this is a real local version.
        db::set_starred(&conn, 1, true).unwrap();
        db::set_last_sync_ts(&conn, 100).unwrap();
        if protocol == "fever" {
            db::set_setting(&conn, "sync_protocol", "fever").unwrap();
            conn.execute("UPDATE feeds SET remote_id=1 WHERE id=1", [])
                .unwrap();
            server.set_fever_serve_entries(true);
            server.set_fever_api_version(4);
        }
    }
    let barrier = if protocol == "fever" {
        server.arm_fever_items_barrier(1)
    } else {
        server.arm_contents_barrier(1)
    };
    let full = {
        let database = database.clone();
        let http = http.clone();
        tokio::spawn(async move { sync::states_phase(&database, &http, true).await })
    };
    tokio::time::timeout(FINISH_BUDGET, barrier.arrived)
        .await
        .unwrap()
        .unwrap();
    {
        let conn = database.lock().await;
        db::delete_feed(&conn, 1).unwrap();
        let folder = db::create_folder(&conn, "resubscribed", "article").unwrap();
        assert_eq!(
            db::insert_feed(
                &conn,
                "http://127.0.0.1:8765/local_feed.xml",
                None,
                "resubscribed",
                None,
                folder,
                "inherit",
                false,
                false
            )
            .unwrap(),
            1
        );
        // The already-captured source now has a new lifecycle, and an in-round
        // local intent. Importing a different source must not refresh either.
        db::set_read_with_enqueue(&conn, 1, true).unwrap();
    }
    barrier.release.send(()).unwrap();
    let report = tokio::time::timeout(FINISH_BUDGET, full)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let (old_title, old_read, old_star, version, cursor, imported): (
        String,
        bool,
        bool,
        i64,
        i64,
        bool,
    ) = {
        let conn = database.lock().await;
        let (title, read, star): (String, bool, bool) = conn
            .query_row(
                "SELECT title,is_read,is_starred FROM articles WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        (
            title,
            read,
            star,
            conn.query_row(
                "SELECT lifecycle_version FROM feeds WHERE id=1",
                [],
                |row| row.get(0),
            )
            .unwrap(),
            db::last_sync_ts(&conn).unwrap(),
            db::article_by_remote_id(&conn, new_remote_id)
                .unwrap()
                .is_some(),
        )
    };
    let next = sync::states_phase(&database, &http, false).await.unwrap();
    let eventually_imported = db::article_by_remote_id(&*database.lock().await, new_remote_id)
        .unwrap()
        .is_some();
    assert!(
        sync::runtime::for_db(&database)
            .drain(Duration::from_secs(2))
            .await
    );
    assert_eq!(version, 2);
    assert_eq!(
        old_title, "State lane",
        "the old source token must never be replaced by discovery"
    );
    assert!(
        old_read && old_star,
        "old source flags/local intent must survive the retry"
    );
    assert_eq!(
        cursor, 100,
        "a rejected old lifecycle must not consume the time window"
    );
    assert!(
        report.binding_incomplete,
        "rejected source evidence remains an incomplete window"
    );
    assert!(
        report.discovered_sources.keys().all(|id| *id != 1),
        "reactivation is never a first-creation receipt"
    );
    assert!(
        imported || eventually_imported,
        "new-source backfill must remain recoverable"
    );
    assert!(
        eventually_imported,
        "the next normal round can consume its new, valid baseline: {:?}",
        next.errors
    );
}

#[tokio::test]
async fn greader_discovery_admits_new_source_without_refreshing_reactivated_source() {
    discovery_receipt_keeps_old_source_lifecycle("greader").await;
}

#[tokio::test]
async fn fever_discovery_admits_new_source_without_refreshing_reactivated_source() {
    discovery_receipt_keeps_old_source_lifecycle("fever").await;
}
