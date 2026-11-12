//! Original review inputs, adapted after the diagnosed cross-wire tie model fix.
//! The old rejection assertions and failed evidence remain in the audit snapshot.
use super::{conn, seed, LIB};
use crate::local_sync::{database, logical, model};
use crate::{
    db,
    webdav_transport::{Batch, FieldPatch, FieldValue, Intent, Register, Stamp},
};
use rusqlite::{params, Connection};
use serde_json::json;
const LEFT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const RIGHT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn fixture() -> Connection {
    fixture_with_extra(0)
}
fn fixture_with_extra(extra: usize) -> Connection {
    let c = conn();
    seed(&c, LEFT, "https://example.test/left");
    let folder: i64 = c
        .query_row("SELECT id FROM folders LIMIT 1", [], |r| r.get(0))
        .unwrap();
    let feed = db::insert_feed(
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
        params![RIGHT, feed],
    )
    .unwrap();
    let a = db::NewArticle {
        guid: "real-guid".into(),
        url: Some("https://example.test/right-post".into()),
        title: "Right article".into(),
        author: None,
        summary: Some("second body".into()),
        content_html: Some("<p>second body</p>".into()),
        body_text: "second body".into(),
        image_url: None,
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: Some("2026-11-09T00:00:00Z".into()),
        source: "direct".into(),
    };
    db::upsert_article_with_feed(&c, feed, &a, false).unwrap();
    let left: i64 = c
        .query_row("SELECT id FROM feeds WHERE sync_id=?1", [LEFT], |r| {
            r.get(0)
        })
        .unwrap();
    for index in 0..extra {
        let mut article = a.clone();
        article.guid = format!("extra-{index:03}");
        article.title = format!("Extra {index}");
        article.url = Some(format!("https://example.test/extra/{index}"));
        article.body_text = format!("extra body {index}");
        article.content_html = Some(format!("<p>extra body {index}</p>"));
        let id = db::upsert_article_with_feed(&c, left, &article, false)
            .unwrap()
            .0;
        if index == 64 {
            db::set_starred(&c, id, true).unwrap();
        }
    }
    database::initialize(&c, LIB).unwrap();
    logical::drain_local(&c).unwrap();
    c
}
fn same_stamp_distinct_fields(c: &Connection, meta: bool) {
    let mut patch = FieldPatch::new();
    for source in [LEFT, RIGHT] {
        let key = if meta {
            format!("a/{}/meta", model::entry_key(source, "real-guid"))
        } else {
            format!("s/{source}/title")
        };
        patch.insert(key.clone(), logical::get(c, &key).unwrap().unwrap().value);
    }
    let batch = Batch {
        format: 2,
        library_id: LIB.into(),
        actor: "e".repeat(32),
        intents: vec![Intent {
            seq: 1,
            lamport: 1000,
            observed: logical::frontier(c).unwrap(),
            patch,
        }],
    };
    batch.validate().unwrap();
    logical::ingest_batch(c, 0, &batch).unwrap();
    assert!(logical::apply_ready_intent(c).unwrap());
    logical::drain_local(c).unwrap();
}

#[test]
fn review_segment1_multientent_second_alias_tie_converges() {
    let c = fixture();
    same_stamp_distinct_fields(&c, true);
    let before = database::checkpoint(&c).unwrap();
    let mut observed = before.covered.clone();
    observed.insert("d".repeat(32), 1);
    let batch = Batch {
        format: 2,
        library_id: LIB.into(),
        actor: "d".repeat(32),
        intents: vec![
            Intent {
                seq: 1,
                lamport: 2000,
                observed: before.covered.clone(),
                patch: FieldPatch::from([("p/themeMode".into(), FieldValue::Set(json!("dark")))]),
            },
            Intent {
                seq: 2,
                lamport: 2001,
                observed,
                patch: FieldPatch::from([(
                    format!("s/{RIGHT}/feed_url"),
                    FieldValue::Set(json!("https://example.test/left")),
                )]),
            },
        ],
    };
    batch.validate().unwrap();
    let ingest = logical::ingest_batch(&c, 0, &batch);
    println!("PROBE multi_ingest_ok={}", ingest.is_ok());
    if ingest.is_ok() {
        assert_eq!(
            database::checkpoint(&c).unwrap(),
            before,
            "receive must not trial-write the logical state"
        );
        let first = logical::apply_ready_intent(&c);
        let second = logical::apply_ready_intent(&c);
        let third = logical::apply_ready_intent(&c);
        let pending: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM local_sync_intents WHERE actor=?1",
                ["d".repeat(32)],
                |r| r.get(0),
            )
            .unwrap();
        println!("PROBE first={first:?} second={second:?} third={third:?} pending={pending} frontier={:?}",logical::frontier(&c).unwrap());
    }
    assert!(
        ingest.is_ok(),
        "different wire fields with equal stamps are valid alias candidates"
    );
    logical::drain_local(&c).unwrap();
    assert_eq!(
        logical::frontier(&c).unwrap().get(&"d".repeat(32)),
        Some(&2)
    );
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM local_sync_intents", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM articles", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn review_segment1_legacy_sql_url_selects_source_tie_with_candidate() {
    let c = fixture();
    same_stamp_distinct_fields(&c, false);
    let mut state = database::checkpoint(&c).unwrap();
    state.fields.insert(
        format!("s/{LEFT}/feed_url"),
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
    state.validate().unwrap();
    model::validate_checkpoint(&state).unwrap();
    let job = super::accepted_candidate_tests::stage(&c, &state);
    let revision = logical::preflight(&c, &job).unwrap();
    assert!(logical::accept(&c, 0, &job, revision).unwrap());
    assert!(!c.query_row("SELECT EXISTS(SELECT 1 FROM feeds WHERE feed_url='https://example.test/logical-target')",[],|r|r.get::<_,bool>(0)).unwrap());
    let before = database::checkpoint(&c).unwrap();
    let update = c.execute(
        "UPDATE feeds SET feed_url='https://example.test/logical-target' WHERE sync_id=?1",
        [RIGHT],
    );
    let projection = logical::projected(&c, &format!("s/{RIGHT}"));
    println!(
        "PROBE legacy_update={update:?} projection={projection:?} logical_changed={}",
        database::checkpoint(&c).unwrap() != before
    );
    assert!(
        update.is_ok(),
        "different source keys can have equal stamps and select a deterministic display value"
    );
    let projection = projection.unwrap();
    assert_eq!(
        model::value(&projection, &format!("s/{LEFT}"), "title"),
        Some(&json!("Right"))
    );
    let mut finished = false;
    for step in 0..128 {
        if !logical::resume_slice(&c).unwrap() {
            finished = true;
            break;
        }
        if step == 15 || step == 127 {
            let mut stmt = c
                .prepare("SELECT job,key,revision,cursor FROM local_sync_work ORDER BY job,key")
                .unwrap();
            let work = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            println!("BOUNDED step={step} work={work:?}");
        }
    }
    assert!(finished,"two-source materialization must finish in 128 slices without re-enqueueing prerequisites forever");
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM feeds", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn review_segment1_no_sync_portable_existing_highwater_control() {
    super::portable_v4_deduplicates_content_and_restores_preferences_with_fresh_ids();
}

#[path = "alias_model_tests.rs"]
mod alias_model_tests;
