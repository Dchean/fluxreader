//! Segment-two lifecycle tests. All ordinary cases use owned small disk/SQLite
//! fixtures; network timing and capacity are separate reserved, ignored work.
use super::{conn, seed, LIB};
use crate::{
    db,
    local_sync::{database, logical, model, runtime, storage},
    webdav_transport::{Batch, FieldPatch, FieldValue, Intent, Register, Stamp},
};
use rusqlite::{params, Connection};
use serde_json::json;
use std::{sync::Arc, time::Instant};
use tokio::sync::Mutex;

fn disk(label: &str) -> (std::path::PathBuf, Connection) {
    let path = super::path(label);
    let c = db::open(&path).unwrap();
    (path, c)
}
fn initialized(c: &Connection) {
    seed(c, &"a".repeat(32), "https://example.test/segment2");
    database::initialize(c, LIB).unwrap();
    logical::drain_local(c).unwrap();
}
fn stage(c: &Connection, state: &crate::webdav_transport::Checkpoint) -> String {
    super::accepted_candidate_tests::stage(c, state)
}

#[test]
fn segment2_rejected_candidate_survives_restart_without_roots_or_lost_pending() {
    let (path, c) = disk("segment2-reject");
    initialized(&c);
    let pending = Batch {
        format: 2,
        library_id: LIB.into(),
        actor: "d".repeat(32),
        intents: vec![Intent {
            seq: 1,
            lamport: 3000,
            observed: std::collections::BTreeMap::from([("f".repeat(32), 1)]),
            patch: FieldPatch::from([("p/themeMode".into(), FieldValue::Set(json!("dark")))]),
        }],
    };
    logical::ingest_batch(&c, 0, &pending).unwrap();
    let before = database::checkpoint(&c).unwrap();
    let mut bad = before.clone();
    for i in 0..64 {
        bad.fields.insert(
            format!("g/{:032x}/name", i + 100),
            Register {
                stamp: Stamp {
                    actor: "e".repeat(32),
                    seq: 1,
                    lamport: 1000,
                },
                value: FieldValue::Set(json!(format!("missing-created-{i}"))),
            },
        );
    }
    bad.covered.insert("e".repeat(32), 1);
    bad.max_lamport = 1000;
    let job = stage(&c, &bad);
    let known: i64 = c
        .query_row("SELECT COUNT(*) FROM local_sync_known_actors", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(logical::preflight(&c, &job).is_err());
    assert_eq!(database::checkpoint(&c).unwrap(), before);
    assert_eq!(
        c.query_row(
            "SELECT phase FROM local_sync_receive WHERE id=?1",
            [&job],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "rejected"
    );
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM local_sync_roots WHERE reference=?1",
            [&job],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM local_sync_work WHERE job=?1",
            [&job],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    let raw: String = c
        .query_row(
            "SELECT manifest FROM local_sync_receive WHERE id=?1",
            [&job],
            |r| r.get(0),
        )
        .unwrap();
    drop(c);
    let c = db::open(&path).unwrap();
    let manifest: storage::Manifest = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        logical::begin_receive(&c, 0, &manifest).unwrap_err().code,
        "localSyncRejected"
    );
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM local_sync_intents", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM local_sync_known_actors", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        known
    );
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM local_sync_roots", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    drop(c);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn segment2_candidate_reader_transient_and_accepted_boundaries() {
    let c = conn();
    initialized(&c);
    let state = database::checkpoint(&c).unwrap();
    let job = stage(&c, &state);
    let reader = logical::ValidationReader::new(&c, &job).unwrap();
    assert_eq!(
        logical::reject_candidate(&c, 0, &job, &model::invalid("bad candidate"))
            .unwrap_err()
            .code,
        "localSyncPending"
    );
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM local_sync_roots WHERE reference=?1",
            [&job],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    drop(reader);
    assert!(!logical::reject_candidate(
        &c,
        0,
        &job,
        &crate::error::AppError::new("localSyncPending", "missing page")
    )
    .unwrap());
    assert!(!logical::reject_candidate(
        &c,
        0,
        &job,
        &crate::error::AppError::new("localSyncNetwork", "temporary IO")
    )
    .unwrap());
    let revision = logical::preflight(&c, &job).unwrap();
    logical::capture_patch(
        &c,
        &FieldPatch::from([("p/themeMode".into(), FieldValue::Set(json!("dark")))]),
    )
    .unwrap();
    assert!(!logical::accept(&c, 0, &job, revision).unwrap());
    assert_eq!(
        c.query_row(
            "SELECT phase FROM local_sync_receive WHERE id=?1",
            [&job],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "staging"
    );
    let revision = logical::preflight(&c, &job).unwrap();
    assert!(logical::accept(&c, 0, &job, revision).unwrap());
    let before = database::checkpoint(&c).unwrap();
    assert!(logical::reject_candidate(&c, 0, &job, &model::invalid("late bad notice")).is_err());
    assert_eq!(database::checkpoint(&c).unwrap(), before);
    assert_eq!(
        c.query_row("SELECT accepted_job FROM local_sync_control", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        job
    );
}

#[test]
fn segment2_missing_page_retries_but_rejected_rows_cleanup_is_bounded() {
    let c = conn();
    initialized(&c);
    let state = database::checkpoint(&c).unwrap();
    let staged = stage(&c, &state);
    let manifest: storage::Manifest = serde_json::from_str(
        &c.query_row::<String, _, _>(
            "SELECT manifest FROM local_sync_receive WHERE id=?1",
            [&staged],
            |r| r.get(0),
        )
        .unwrap(),
    )
    .unwrap();
    c.execute(
        "UPDATE local_sync_receive SET next_page=0 WHERE id=?1",
        [&staged],
    )
    .unwrap();
    assert_eq!(
        logical::preflight(&c, &staged).unwrap_err().code,
        "localSyncPending"
    );
    assert_eq!(logical::begin_receive(&c, 0, &manifest).unwrap(), staged);
    logical::stage_page(&c, 0, &staged, 0, &state.fields).unwrap();
    assert!(
        logical::reject_candidate(&c, 0, &staged, &model::invalid("confirmed invalid bytes"))
            .unwrap()
    );
    let rows: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM local_sync_candidates WHERE job=?1",
            [&staged],
            |r| r.get(0),
        )
        .unwrap();
    storage::cleanup_slice(&c).unwrap();
    let after: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM local_sync_candidates WHERE job=?1",
            [&staged],
            |r| r.get(0),
        )
        .unwrap();
    assert!(rows - after <= 128);
    assert_eq!(after, 0);
    assert_eq!(
        logical::begin_receive(&c, 0, &manifest).unwrap_err().code,
        "localSyncRejected"
    );
}

#[tokio::test]
async fn segment2_startup_resumes_started_bootstrap_without_credentials() {
    let (path, c) = disk("segment2-bootstrap");
    let (feed, _) = seed(&c, &"a".repeat(32), "https://example.test/bootstrap");
    for i in 1..128 {
        c.execute("INSERT INTO articles(feed_id,guid,title,published_at,first_seen_at) VALUES(?1,?2,?2,'2026-11-10T00:00:00Z','2026-11-10T00:00:00Z')",params![feed,format!("bootstrap-{i}")]).unwrap();
    }
    database::begin_initialize(&c, LIB).unwrap();
    assert!(!logical::settled(&c).unwrap());
    database::resume_local_slice(&c).unwrap();
    runtime::disconnect(&c).unwrap();
    let owner = database::actor(&c).unwrap();
    drop(c);
    let database = Arc::new(Mutex::new(db::open(&path).unwrap()));
    let rt = runtime::LocalRuntime::default();
    let mut done = false;
    for _ in 0..2048 {
        if !runtime::recover_local_slice(&database, &rt).await.unwrap() {
            done = true;
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(done);
    let c = database.lock().await;
    assert!(logical::settled(&c).unwrap());
    assert_eq!(database::actor(&c).unwrap(), owner);
    assert!(db::get_setting(&c, "local_sync_password")
        .unwrap()
        .is_none());
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM local_sync_winners WHERE key LIKE 'a/%/meta'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        128
    );
    drop(c);
    drop(database);
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn segment2_ordinary_unstarted_library_is_not_bootstrapped() {
    let (path, c) = disk("segment2-ordinary");
    seed(&c, &"a".repeat(32), "https://example.test/ordinary");
    let database = Arc::new(Mutex::new(c));
    let rt = runtime::LocalRuntime::default();
    assert!(!runtime::recover_local_slice(&database, &rt).await.unwrap());
    let c = database.lock().await;
    assert!(logical::settled(&c).unwrap());
    assert_eq!(
        c.query_row(
            "SELECT capture+initialized+seq FROM local_sync_control",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM local_sync_fields", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    drop(c);
    drop(database);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn segment2_paused_remote_gc_preserves_queue_and_body_obligations() {
    let c = conn();
    initialized(&c);
    let owner = database::actor(&c).unwrap();
    let path = format!("{}actors/{owner}/state/objects/old.bin", storage::ROOT);
    c.execute(
        "INSERT INTO local_sync_gc(path,generation) VALUES(?1,0)",
        [&path],
    )
    .unwrap();
    let holds: i64 = c
        .query_row("SELECT COUNT(*) FROM local_sync_body_hold", [], |r| {
            r.get(0)
        })
        .unwrap();
    runtime::maintain_local_gc(&c, 0).unwrap();
    assert_eq!(
        c.query_row("SELECT path FROM local_sync_gc", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        path
    );
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM local_sync_body_hold", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        holds
    );
}

/// Prepared evidence only; the coordinator must grant the separate small-disk
/// profiling window. No timeout/latency assertions are relaxed here.
#[test]
#[ignore = "requires coordinator-granted small disk writer/EXPLAIN profile"]
fn segment2_profile_129_phases_and_product_sqlite_plans() {
    let (path, c) = disk("segment2-profile129");
    let mut phases = std::collections::BTreeMap::new();
    let mut slices = Vec::new();
    let start = Instant::now();
    let left = "a".repeat(32);
    let right = "b".repeat(32);
    let (feed, _) = seed(&c, &left, "https://example.test/left");
    let folder: i64 = c
        .query_row("SELECT folder_id FROM feeds WHERE id=?1", [feed], |r| {
            r.get(0)
        })
        .unwrap();
    let other = db::insert_feed(
        &c,
        "https://example.test/right",
        None,
        "Right",
        None,
        folder,
        "inherit",
        false,
        false,
    )
    .unwrap();
    c.execute(
        "UPDATE feeds SET sync_id=?1 WHERE id=?2",
        params![right, other],
    )
    .unwrap();
    for i in 0..129 {
        let guid = if i == 0 {
            "real-guid".to_owned()
        } else {
            format!("extra-{:03}", i - 1)
        };
        let target = if i == 0 { other } else { feed };
        let body = format!("distinct profile body {i}");
        let article = db::NewArticle {
            guid,
            url: Some(format!("https://example.test/profile/{i}")),
            title: format!("Profile {i}"),
            author: None,
            summary: Some(body.clone()),
            content_html: Some(format!("<p>{body}</p>")),
            body_text: body,
            image_url: None,
            enclosure_url: None,
            enclosure_mime: None,
            duration_sec: None,
            published_at: Some("2026-11-10T00:00:00Z".into()),
            source: "direct".into(),
        };
        let id = db::upsert_article_with_feed(&c, target, &article, false)
            .unwrap()
            .0;
        if i == 65 {
            db::set_starred(&c, id, true).unwrap();
        }
    }
    phases.insert("seed", start.elapsed().as_secs_f64() * 1000.0);
    let drain = |phase: &str, slices: &mut Vec<serde_json::Value>| {
        let phase_start = Instant::now();
        for index in 0..4096 {
            let start = Instant::now();
            let work = database::resume_local_slice(&c).unwrap();
            slices.push(json!({"phase":phase,"index":index,"slice_wall_ms":start.elapsed().as_secs_f64()*1000.0}));
            if !work {
                return phase_start.elapsed().as_secs_f64() * 1000.0;
            }
        }
        panic!("profile did not finish in bounded slices")
    };
    database::begin_initialize(&c, LIB).unwrap();
    phases.insert(
        "bootstrap_projection",
        drain("bootstrap_projection", &mut slices),
    );
    let start = Instant::now();
    let batch = Batch {
        format: 2,
        library_id: LIB.into(),
        actor: "e".repeat(32),
        intents: vec![Intent {
            seq: 1,
            lamport: 1000,
            observed: logical::frontier(&c).unwrap(),
            patch: FieldPatch::from([
                (format!("s/{left}/title"), FieldValue::Set(json!("Left"))),
                (format!("s/{right}/title"), FieldValue::Set(json!("Right"))),
            ]),
        }],
    };
    logical::ingest_batch(&c, 0, &batch).unwrap();
    drain("same_stamp_titles", &mut slices);
    phases.insert("same_stamp_titles", start.elapsed().as_secs_f64() * 1000.0);
    let start = Instant::now();
    let mut state = database::checkpoint(&c).unwrap();
    state.fields.insert(
        format!("s/{left}/feed_url"),
        Register {
            stamp: Stamp {
                actor: "e".repeat(32),
                seq: 2,
                lamport: 1001,
            },
            value: FieldValue::Set(json!("https://example.test/logical-target")),
        },
    );
    state.covered.insert("e".repeat(32), 2);
    state.max_lamport = 1001;
    let job = stage(&c, &state);
    let checked = logical::preflight(&c, &job).unwrap();
    assert!(logical::accept(&c, 0, &job, checked).unwrap());
    phases.insert(
        "candidate_stage_preflight_accept",
        start.elapsed().as_secs_f64() * 1000.0,
    );
    let start = Instant::now();
    c.execute(
        "UPDATE feeds SET feed_url='https://example.test/logical-target' WHERE sync_id=?1",
        [&right],
    )
    .unwrap();
    phases.insert("sql_merge_capture", start.elapsed().as_secs_f64() * 1000.0);
    phases.insert("merge_projection", drain("merge_projection", &mut slices));
    let start = Instant::now();
    logical::capture_patch(
        &c,
        &FieldPatch::from([(
            format!("s/{left}/feed_url"),
            FieldValue::Set(json!("https://example.test/left")),
        )]),
    )
    .unwrap();
    phases.insert("split_capture", start.elapsed().as_secs_f64() * 1000.0);
    phases.insert("split_projection", drain("split_projection", &mut slices));
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM articles", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        130
    );
    let mut plans = Vec::new();
    for sql in ["SELECT id FROM local_sync_article_state WHERE is_read=0 AND active=1","SELECT job,key,cursor,revision FROM local_sync_work WHERE job='' ORDER BY key LIMIT 1","SELECT substr(key,1,66) FROM local_sync_winners WHERE key LIKE 'a/%/meta' AND json_extract(register,'$.value.value.source')='aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' ORDER BY key LIMIT 64"] {
        let mut stmt=c.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();let rows=stmt.query_map([],|r|r.get::<_,String>(3)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();plans.push(json!({"sql":sql,"plan":rows}));
    }
    let version: String = c
        .query_row("SELECT sqlite_version()", [], |r| r.get(0))
        .unwrap();
    println!(
        "PROFILE {}",
        json!({"sqlite":version,"original_source_members":129,"phases_ms":phases,"slices":slices,"plans":plans,"scope":"owned disk, distinct bodies, production bootstrap/merge/split; slice wall time includes transaction work and is not lock-only timing; no comparison claim before measurement"})
    );
    drop(c);
    std::fs::remove_file(path).unwrap();
}
