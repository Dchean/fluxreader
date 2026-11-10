//! White-box prospective race: only the old call bypasses orchestration locks.
//! This is not a claim that the baseline public API can overlap two pulls.
//! Both sides execute real production pull code against controlled loopback HTTP.
#[path = "../../tests/mock_greader.rs"]
mod mock_greader;

use super::*;
use crate::db;
use mock_greader::MockGReader;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

async fn old_content_must_not_undo_new_remote_flags(initial: bool) {
    let server = MockGReader::start().await.unwrap();
    let remote_id = server.add_entry_with_published(
        10,
        "https://state-lane.invalid/overlap",
        "Overlap",
        initial,
        initial,
        1_600_000_000,
    );
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    db::MIGRATIONS.to_latest(&mut conn).unwrap();
    db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
    db::set_setting(&conn, "greader_username", "test").unwrap();
    db::set_setting(&conn, "greader_password", "test-token").unwrap();
    conn.execute_batch(
        "INSERT INTO feeds(id,feed_url,title,remote_id)
         VALUES(1,'http://127.0.0.1:8765/local_feed.xml','Overlap',10);
         INSERT INTO articles(id,feed_id,guid,title,url,remote_id,remote_origin_feed_id)
         VALUES(1,1,'overlap','Overlap','https://state-lane.invalid/overlap',501,10);",
    )
    .unwrap();
    assert_eq!(remote_id, 501);
    // This steady-state race starts after an earlier Content response proved
    // feed/10. The held response is a later revision, not first identity proof.
    conn.execute(
        "UPDATE articles SET is_read=?1,is_starred=?1 WHERE id=1",
        [initial],
    )
    .unwrap();
    // Only content full pulls see this old article; the incoming light path must use
    // its real authority/diff logic, not a second full-body refresh.
    db::set_last_sync_ts(&conn, chrono::Utc::now().timestamp() + 3600).unwrap();
    let database = Arc::new(Mutex::new(conn));
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let lease = match acquire_session(&database, &http).await {
        Ok(lease) => lease,
        Err(_) => panic!("configured fixture session must be built"),
    };
    let barrier = server.arm_contents_barrier(1);
    let old = {
        let database = database.clone();
        let session = lease.session.clone();
        tokio::spawn(async move {
            let mut report = SyncReport::default();
            let Backend::GReader(client) = &session.client else {
                panic!("GReader fixture")
            };
            // Intentionally model the concurrency created by a future lane split;
            // do not change production runtime/PULL_LOCK to make this test possible.
            pull_entries_greader(&database, &session, client, &mut report, true, None).await?;
            Ok::<_, crate::error::AppError>(report)
        })
    };
    tokio::time::timeout(Duration::from_secs(10), barrier.arrived)
        .await
        .unwrap()
        .unwrap();
    {
        let mut entries = server.entries.lock().unwrap();
        let entry = entries
            .iter_mut()
            .find(|entry| entry.id == remote_id)
            .unwrap();
        entry.read = !initial;
        entry.starred = !initial;
        entry.changed_at += 1;
    }
    let incoming =
        tokio::time::timeout(Duration::from_secs(5), sync_states_now(&database, &http)).await;
    let before: (bool, bool, i64, i64) = database
        .lock()
        .await
        .query_row(
            "SELECT is_read,is_starred,read_version,star_version FROM articles WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    barrier.release.send(()).unwrap();
    let old_report = tokio::time::timeout(Duration::from_secs(10), old)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let after: (bool, bool, i64, i64) = database
        .lock()
        .await
        .query_row(
            "SELECT is_read,is_starred,read_version,star_version FROM articles WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    let drained = runtime::for_db(&database)
        .drain(Duration::from_secs(2))
        .await;
    eprintln!(
        "STATE_LANE_WHITEBOX initial={initial} before_release={before:?} after_release={after:?}"
    );
    assert!(drained);
    let new_report = incoming
        .expect("new remote round must finish while old response is held")
        .unwrap();
    assert!(new_report.errors.is_empty(), "{:?}", new_report.errors);
    assert!(old_report.errors.is_empty(), "{:?}", old_report.errors);
    assert_eq!(
        before,
        (!initial, !initial, 0, 0),
        "new remote flags must be accepted without any local version bump"
    );
    assert_eq!(
        after, before,
        "old content response must not overwrite newer remote flags"
    );
}

#[tokio::test]
async fn whitebox_old_content_cannot_rollback_new_remote_read_and_star() {
    old_content_must_not_undo_new_remote_flags(false).await;
}

#[tokio::test]
async fn whitebox_old_content_cannot_rollback_new_remote_unread_and_unstar() {
    old_content_must_not_undo_new_remote_flags(true).await;
}
