//! A bare remote ID is not evidence of its source. Exercise the production
//! Content -> provenance -> State/Push boundary, including in-flight invalidation.
mod common;
mod mock_greader;

use app_lib::{db, sync};
use mock_greader::MockGReader;
use rusqlite::Connection;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

async fn cold_setup(
    name: &str,
) -> (
    common::TempDbGuard,
    Arc<Mutex<Connection>>,
    reqwest::Client,
    Arc<MockGReader>,
) {
    let server = MockGReader::start().await.unwrap();
    assert_eq!(
        server.add_entry_ret(10, "https://origin.example/501", "Origin", false, true),
        501
    );
    let path = common::unique_db_path(name);
    let guard = common::TempDbGuard::new(path.clone());
    let conn = db::open(&path).unwrap();
    db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
    db::set_setting(&conn, "greader_username", "test").unwrap();
    db::set_setting(&conn, "greader_password", "test-token").unwrap();
    conn.execute_batch(
        "INSERT INTO feeds(id,feed_url,title,remote_id) VALUES(1,'http://127.0.0.1:8765/local_feed.xml','Origin',10);
         INSERT INTO articles(id,feed_id,guid,title,url,remote_id,is_read)
         VALUES(1,1,'origin','Origin','https://origin.example/501',501,1);"
    ).unwrap();
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    (guard, Arc::new(Mutex::new(conn)), http, server)
}

fn flags(conn: &Connection) -> (bool, bool) {
    conn.query_row(
        "SELECT is_read,is_starred FROM articles WHERE id=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .unwrap()
}

#[tokio::test]
async fn cold_identity_cannot_receive_or_push_until_actual_content_proves_origin() {
    let (_guard, database, http, server) = cold_setup("origin_cold").await;
    {
        let conn = database.lock().await;
        // Explicit local intent must be retained even though its bare ID exists.
        db::set_read_with_enqueue(&conn, 1, false).unwrap();
        db::set_read_with_enqueue(&conn, 1, true).unwrap();
        assert!(db::verified_remote_binding(&conn, 1).unwrap().is_none());
    }
    let state = sync::sync_states_now(&database, &http).await.unwrap();
    assert!(state.errors.is_empty(), "{:?}", state.errors);
    sync::push_states_now(&database, &http).await;
    {
        let conn = database.lock().await;
        assert_eq!(flags(&conn), (true, false));
        let queue = db::take_sync_queue(&conn).unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].status, db::STATUS_BINDABLE_RETRY);
    }
    assert!(server.status_updates.lock().unwrap().is_empty());
    assert_eq!(
        server.contents_calls.load(Ordering::SeqCst),
        0,
        "State never waits for cold origin proof"
    );

    let complete = sync::states_phase(&database, &http, true).await.unwrap();
    assert!(complete.errors.is_empty(), "{:?}", complete.errors);
    {
        let conn = database.lock().await;
        let proof = db::verified_remote_binding(&conn, 1).unwrap().unwrap();
        assert_eq!((proof.remote_id, proof.origin_feed_id), (501, 10));
        assert!(db::take_sync_queue(&conn).unwrap().is_empty());
        assert_eq!(flags(&conn), (true, true));
    }
    assert!(server
        .status_updates
        .lock()
        .unwrap()
        .iter()
        .any(|(id, action)| *id == 501 && action == "read"));
    {
        let mut entries = server.entries.lock().unwrap();
        entries[0].read = false;
        entries[0].starred = false;
    }
    let before = server.contents_calls.load(Ordering::SeqCst);
    let state = sync::sync_states_now(&database, &http).await.unwrap();
    assert!(state.errors.is_empty(), "{:?}", state.errors);
    assert_eq!(flags(&*database.lock().await), (false, false));
    assert_eq!(server.contents_calls.load(Ordering::SeqCst), before,
        "proven unread/star changes need no origin/body request; the separate GR read-existence probe is not exercised here");
    assert!(
        sync::runtime::for_db(&database)
            .drain(Duration::from_secs(2))
            .await
    );
}

#[tokio::test]
async fn source_invalidation_during_ack_keeps_queue_and_blocks_later_writes() {
    let (_guard, database, http, server) = cold_setup("origin_late_ack").await;
    let complete = sync::states_phase(&database, &http, true).await.unwrap();
    assert!(complete.errors.is_empty(), "{:?}", complete.errors);
    {
        let conn = database.lock().await;
        db::set_read_with_enqueue(&conn, 1, true).unwrap();
        db::set_starred_with_enqueue(&conn, 1, false).unwrap();
    }
    let barrier = server.arm_contents_barrier(server.contents_calls.load(Ordering::SeqCst) + 1);
    let pushing = {
        let database = database.clone();
        let http = http.clone();
        tokio::spawn(async move { sync::push_states_now(&database, &http).await })
    };
    tokio::time::timeout(Duration::from_secs(10), barrier.arrived)
        .await
        .unwrap()
        .unwrap();
    // The first write was legitimately sent. Invalidation cannot undo it, but
    // its delayed ACK must not confirm this source incarnation or send unstar.
    assert!(server.entries.lock().unwrap()[0].read);
    database
        .lock()
        .await
        .execute_batch(
            "UPDATE feeds SET active=0,lifecycle_version=lifecycle_version+1 WHERE id=1;",
        )
        .unwrap();
    barrier.release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), pushing)
        .await
        .unwrap()
        .unwrap();
    let conn = database.lock().await;
    assert!(db::verified_remote_binding(&conn, 1).unwrap().is_none());
    let queue = db::take_sync_queue(&conn).unwrap();
    assert_eq!(queue.len(), 2);
    assert!(queue
        .iter()
        .all(|item| item.status == db::STATUS_BINDABLE_RETRY));
    assert!(
        server.entries.lock().unwrap()[0].starred,
        "later unstar must not be sent"
    );
    drop(conn);
    assert!(
        sync::runtime::for_db(&database)
            .drain(Duration::from_secs(2))
            .await
    );
}

#[test]
fn proof_is_invalidated_by_identity_and_source_changes_but_not_same_value_writes() {
    let path = common::unique_db_path("origin_invalidation");
    let _guard = common::TempDbGuard::new(path.clone());
    let conn = db::open(&path).unwrap();
    conn.execute_batch("INSERT INTO feeds(id,feed_url,title,remote_id) VALUES(1,'https://origin.example/feed','Origin',10),(2,'https://origin.example/other','Other',10);
        INSERT INTO articles(id,feed_id,guid,title,remote_id) VALUES(1,1,'origin','Origin',501);").unwrap();
    assert!(
        !db::confirm_article_remote_origin(&conn, 1, 501, 20).unwrap(),
        "a conflicting observed origin cannot prove a dirty binding"
    );
    assert!(db::verified_remote_binding(&conn, 1).unwrap().is_none());
    assert!(db::confirm_article_remote_origin(&conn, 1, 501, 10).unwrap());
    assert!(!db::set_article_remote_id(&conn, 1, 501).unwrap());
    conn.execute("UPDATE feeds SET remote_id=10 WHERE id=1", [])
        .unwrap();
    assert!(db::verified_remote_binding(&conn, 1).unwrap().is_some());
    assert_eq!(
        conn.query_row("SELECT lifecycle_version FROM feeds WHERE id=1", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap(),
        0,
        "same-value remote identity writes do not advance the lifecycle"
    );
    for change in [
        "UPDATE articles SET remote_id=502 WHERE id=1",
        "UPDATE articles SET feed_id=2 WHERE id=1",
        "UPDATE feeds SET remote_id=20 WHERE id=1",
        "UPDATE feeds SET sync_id='new-origin-source' WHERE id=1",
        "UPDATE feeds SET lifecycle_version=lifecycle_version+1 WHERE id=1",
        "UPDATE feeds SET active=0 WHERE id=1",
    ] {
        conn.execute_batch("SAVEPOINT origin_case").unwrap();
        conn.execute_batch(change).unwrap();
        assert!(
            db::verified_remote_binding(&conn, 1).unwrap().is_none(),
            "{change}"
        );
        let origin: Option<i64> = conn
            .query_row(
                "SELECT remote_origin_feed_id FROM articles WHERE id=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(origin, None, "{change}");
        conn.execute_batch("ROLLBACK TO origin_case; RELEASE origin_case")
            .unwrap();
    }
    conn.execute("UPDATE feeds SET remote_id=NULL WHERE id=1", [])
        .unwrap();
    assert!(
        db::confirm_article_remote_origin(&conn, 1, 501, 10).unwrap(),
        "exact ID with unknown local remote feed remains compatible"
    );
    assert!(db::verified_remote_binding(&conn, 1).unwrap().is_some());
    conn.execute(
        "UPDATE feeds SET lifecycle_version=9223372036854775807 WHERE id=1",
        [],
    )
    .unwrap();
    assert!(db::confirm_article_remote_origin(&conn, 1, 501, 10).unwrap());
    assert!(conn
        .execute("UPDATE feeds SET remote_id=20 WHERE id=1", [])
        .is_err());
    assert_eq!(
        conn.query_row("SELECT remote_id FROM feeds WHERE id=1", [], |r| r
            .get::<_, Option<i64>>(0))
            .unwrap(),
        None,
        "overflow aborts the complete identity update"
    );
    assert!(db::verified_remote_binding(&conn, 1).unwrap().is_some());
}

#[tokio::test]
async fn old_content_cannot_reprove_origin_after_source_remote_binding_aba() {
    let (_guard, database, http, server) = cold_setup("origin_source_aba").await;
    db::set_last_sync_ts(&*database.lock().await, 100).unwrap();
    let barrier = server.arm_contents_barrier(1);
    let complete = {
        let database = database.clone();
        let http = http.clone();
        tokio::spawn(async move { sync::states_phase(&database, &http, true).await })
    };
    tokio::time::timeout(Duration::from_secs(10), barrier.arrived)
        .await
        .unwrap()
        .unwrap();
    let previous: i64 = database
        .lock()
        .await
        .query_row("SELECT lifecycle_version FROM feeds WHERE id=1", [], |r| {
            r.get(0)
        })
        .unwrap();
    database
        .lock()
        .await
        .execute_batch(
            "UPDATE feeds SET remote_id=NULL WHERE id=1; UPDATE feeds SET remote_id=10 WHERE id=1;",
        )
        .unwrap();
    barrier.release.send(()).unwrap();
    let report = tokio::time::timeout(Duration::from_secs(10), complete)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let conn = database.lock().await;
    assert!(report.binding_incomplete);
    assert_eq!(
        conn.query_row("SELECT lifecycle_version FROM feeds WHERE id=1", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap(),
        previous + 2
    );
    assert!(db::verified_remote_binding(&conn, 1).unwrap().is_none());
    assert_eq!(flags(&conn), (true, false));
    assert_eq!(db::last_sync_ts(&conn).unwrap(), 100);
    drop(conn);
    assert!(
        sync::runtime::for_db(&database)
            .drain(Duration::from_secs(2))
            .await
    );
}

#[tokio::test]
async fn cold_origin_budget_rotates_and_proven_bulk_star_changes_need_no_origin_requests() {
    let (_guard, database, http, server) = cold_setup("origin_budget").await;
    // Keep every remote article outside the ordinary publication-time window.
    server.entries.lock().unwrap()[0].published = 1;
    for index in 1..2001 {
        server.add_entry_with_published(
            10,
            &format!("https://origin.example/{index}"),
            "Origin",
            false,
            true,
            1,
        );
    }
    {
        let conn = database.lock().await;
        conn.execute("INSERT INTO contents(identity,content_html,body_text) VALUES('origin-budget','<p>existing</p>','existing')", []).unwrap();
        let content = conn.last_insert_rowid();
        conn.execute("UPDATE articles SET content_id=?1 WHERE id=1", [content])
            .unwrap();
        for id in 502..=2501 {
            conn.execute("INSERT INTO articles(feed_id,guid,title,remote_id,is_read,content_id) VALUES(1,?1,'Origin',?2,1,?3)", rusqlite::params![format!("cold-{id}"), id, content]).unwrap();
        }
        db::set_last_sync_ts(&conn, chrono::Utc::now().timestamp()).unwrap();
    }
    let first = sync::states_phase(&database, &http, false).await.unwrap();
    assert!(first.errors.is_empty(), "{:?}", first.errors);
    assert_eq!(
        server.contents_calls.load(Ordering::SeqCst),
        20,
        "2000 cold IDs / 100 IDs per request"
    );
    let proven: i64 = database
        .lock()
        .await
        .query_row(
            "SELECT count(*) FROM articles WHERE remote_origin_feed_id IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(proven, 2000);
    let second = sync::states_phase(&database, &http, false).await.unwrap();
    assert!(second.errors.is_empty(), "{:?}", second.errors);
    assert_eq!(
        server.contents_calls.load(Ordering::SeqCst),
        21,
        "next round reaches the previously unproved tail"
    );
    for entry in server.entries.lock().unwrap().iter_mut() {
        entry.starred = false;
    }
    let states = sync::sync_states_now(&database, &http).await.unwrap();
    assert!(states.errors.is_empty(), "{:?}", states.errors);
    assert_eq!(
        server.contents_calls.load(Ordering::SeqCst),
        21,
        "2001 proven star changes add zero origin requests"
    );
    let starred: i64 = database
        .lock()
        .await
        .query_row(
            "SELECT count(*) FROM articles WHERE is_starred=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(starred, 0);
    assert!(
        sync::runtime::for_db(&database)
            .drain(Duration::from_secs(2))
            .await
    );
}

#[tokio::test]
async fn fever_source_invalidation_between_toggle_probe_and_mark_blocks_the_write() {
    let (_guard, database, http, server) = cold_setup("origin_fever_probe").await;
    server.set_fever_serve_entries(true);
    server.set_fever_api_version(3);
    {
        let conn = database.lock().await;
        db::set_setting(&conn, "sync_protocol", "fever").unwrap();
        conn.execute("UPDATE feeds SET remote_id=1 WHERE id=1", [])
            .unwrap();
    }
    let complete = sync::states_phase(&database, &http, true).await.unwrap();
    assert!(complete.errors.is_empty(), "{:?}", complete.errors);
    db::set_starred_with_enqueue(&*database.lock().await, 1, false).unwrap();
    let barrier =
        server.arm_fever_items_barrier(server.fever_items_calls.load(Ordering::SeqCst) + 1);
    let pushing = {
        let database = database.clone();
        let http = http.clone();
        tokio::spawn(async move { sync::push_states_now(&database, &http).await })
    };
    tokio::time::timeout(Duration::from_secs(10), barrier.arrived)
        .await
        .unwrap()
        .unwrap();
    database
        .lock()
        .await
        .execute(
            "UPDATE feeds SET lifecycle_version=lifecycle_version+1 WHERE id=1",
            [],
        )
        .unwrap();
    barrier.release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), pushing)
        .await
        .unwrap()
        .unwrap();
    assert!(server.entries.lock().unwrap()[0].starred);
    assert!(
        !server
            .fever_query_strings
            .lock()
            .unwrap()
            .iter()
            .any(|query| query.contains("mark=item")),
        "source changed after probe: no mark may follow"
    );
    let queue = db::take_sync_queue(&*database.lock().await).unwrap();
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0].status, db::STATUS_BINDABLE_RETRY);
    assert!(
        sync::runtime::for_db(&database)
            .drain(Duration::from_secs(2))
            .await
    );
}

#[tokio::test]
async fn fever_read_batch_preserves_fifty_id_ack_chunks() {
    let (_guard, database, http, server) = cold_setup("origin_fever_batch").await;
    server.set_fever_serve_entries(true);
    server.set_fever_api_version(4);
    for index in 1..51 {
        server.add_entry_ret(
            10,
            &format!("https://origin.example/batch/{index}"),
            "Batch",
            false,
            false,
        );
    }
    {
        let conn = database.lock().await;
        db::set_setting(&conn, "sync_protocol", "fever").unwrap();
        conn.execute("UPDATE feeds SET remote_id=1 WHERE id=1", [])
            .unwrap();
    }
    let content = sync::states_phase(&database, &http, true).await.unwrap();
    assert!(content.errors.is_empty(), "{:?}", content.errors);
    {
        let conn = database.lock().await;
        let mut statement = conn
            .prepare(
                "SELECT id FROM articles WHERE remote_id BETWEEN 501 AND 551 ORDER BY remote_id",
            )
            .unwrap();
        let ids: Vec<i64> = statement
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(ids.len(), 51);
        for aid in ids {
            db::set_read_with_enqueue(&conn, aid, true).unwrap();
        }
        assert_eq!(db::take_sync_queue(&conn).unwrap().len(), 51);
    }
    let before = server.fever_query_strings.lock().unwrap().len();
    sync::push_states_now(&database, &http).await;
    let queries = server.fever_query_strings.lock().unwrap()[before..].to_vec();
    assert_eq!(
        queries
            .iter()
            .filter(|query| query.contains("mark=item"))
            .count(),
        51
    );
    assert_eq!(
        queries
            .iter()
            .filter(|query| query.contains("with_ids="))
            .count(),
        2,
        "51 per-ID marks still use two readback requests, not 51"
    );
    assert!(db::take_sync_queue(&*database.lock().await)
        .unwrap()
        .is_empty());
    assert!(
        sync::runtime::for_db(&database)
            .drain(Duration::from_secs(2))
            .await
    );
}

#[tokio::test]
async fn fever_unwritten_second_source_is_not_confirmed_by_an_already_matching_remote_flag() {
    let (_guard, database, http, server) = cold_setup("origin_fever_between_marks").await;
    server.set_fever_serve_entries(true);
    server.set_fever_api_version(4);
    assert_eq!(
        server.add_entry_ret(11, "https://origin.example/other", "Other", true, false),
        502
    );
    {
        let conn = database.lock().await;
        db::set_setting(&conn, "sync_protocol", "fever").unwrap();
        conn.execute("UPDATE feeds SET remote_id=1 WHERE id=1", [])
            .unwrap();
    }
    let content = sync::sync_now(&database, &http).await.unwrap();
    assert!(content.errors.is_empty(), "{:?}", content.errors);
    let second = {
        let conn = database.lock().await;
        let first = db::article_by_remote_id(&conn, 501).unwrap().unwrap();
        let second = db::article_by_remote_id(&conn, 502).unwrap().unwrap();
        db::set_read_with_enqueue(&conn, first, true).unwrap();
        db::set_read_with_enqueue(&conn, second, false).unwrap();
        db::set_read_with_enqueue(&conn, second, true).unwrap();
        assert_eq!(db::take_sync_queue(&conn).unwrap().len(), 2);
        second
    };
    let before = server.fever_query_strings.lock().unwrap().len();
    let barrier = server.arm_fever_mark_barrier(server.fever_mark_calls.load(Ordering::SeqCst) + 1);
    let pushing = {
        let database = database.clone();
        let http = http.clone();
        tokio::spawn(async move { sync::push_states_now(&database, &http).await })
    };
    tokio::time::timeout(Duration::from_secs(10), barrier.arrived)
        .await
        .unwrap()
        .unwrap();
    database.lock().await.execute("UPDATE feeds SET active=0,lifecycle_version=lifecycle_version+1 WHERE id=(SELECT feed_id FROM articles WHERE id=?1)", [second]).unwrap();
    barrier.release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), pushing)
        .await
        .unwrap()
        .unwrap();
    let queries = server.fever_query_strings.lock().unwrap()[before..].to_vec();
    assert_eq!(
        queries
            .iter()
            .filter(|query| query.contains("mark=item"))
            .count(),
        1
    );
    let readback: Vec<_> = queries
        .iter()
        .filter(|query| query.contains("with_ids="))
        .collect();
    assert_eq!(readback.len(), 1);
    assert!(readback[0].contains("501") && !readback[0].contains("502"));
    let queue = db::take_sync_queue(&*database.lock().await).unwrap();
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0].article_id, Some(second));
    assert_eq!(queue[0].status, db::STATUS_BINDABLE_RETRY);
    assert!(
        sync::runtime::for_db(&database)
            .drain(Duration::from_secs(2))
            .await
    );
}
