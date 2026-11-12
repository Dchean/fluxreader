//! Root-model regressions adapted from the independent A5 snapshot probes.
//! Cross-wire ties now converge; original rejection results stay in audit files.
use super::*;
use rusqlite::types::Value as SqlValue;
use serde_json::Value;
use std::collections::BTreeMap;

fn rows(c: &Connection, sql: &str) -> Vec<Vec<SqlValue>> {
    let mut stmt = c.prepare(sql).unwrap();
    let count = stmt.column_count();
    let result = stmt
        .query_map([], |row| (0..count).map(|i| row.get(i)).collect())
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    result
}
fn protected_snapshot(c: &Connection) -> BTreeMap<String, Vec<Vec<SqlValue>>> {
    let mut out = BTreeMap::new();
    for table in [
        "local_sync_fields",
        "local_sync_journal",
        "local_sync_body_hold",
        "articles",
        "feeds",
        "folders",
        "contents",
        "sync_queue",
    ] {
        out.insert(
            table.to_owned(),
            rows(c, &format!("SELECT rowid,* FROM {table} ORDER BY rowid")),
        );
    }
    out.insert("control".into(),rows(c,"SELECT actor,library_id,seq,lamport,frontier,completed_frontier,accepted_job,folding,applying FROM local_sync_control WHERE id=1"));
    out.insert(
        "ordinary_work".into(),
        rows(
            c,
            "SELECT job,key,revision,cursor FROM local_sync_work WHERE job='' ORDER BY key",
        ),
    );
    out
}
fn complete_snapshot(c: &Connection) -> BTreeMap<String, Vec<Vec<SqlValue>>> {
    let mut out = protected_snapshot(c);
    for table in [
        "local_sync_control",
        "local_sync_intents",
        "local_sync_seen_batches",
        "local_sync_known_actors",
    ] {
        out.insert(
            table.to_owned(),
            rows(c, &format!("SELECT rowid,* FROM {table} ORDER BY rowid")),
        );
    }
    out
}
fn pending_tied_url(c: &Connection, dependency: &str) -> Batch {
    let mut observed = logical::frontier(c).unwrap();
    observed.insert(dependency.to_owned(), 1);
    Batch {
        format: 2,
        library_id: LIB.into(),
        actor: "d".repeat(32),
        intents: vec![Intent {
            seq: 1,
            lamport: 2000,
            observed,
            patch: FieldPatch::from([(
                format!("s/{RIGHT}/feed_url"),
                FieldValue::Set(json!("https://example.test/left")),
            )]),
        }],
    }
}
fn add_dependency_state(c: &Connection) -> crate::webdav_transport::Checkpoint {
    let mut state = database::checkpoint(c).unwrap();
    state.fields.insert(
        "p/fontSize".into(),
        Register {
            stamp: Stamp {
                actor: "c".repeat(32),
                seq: 1,
                lamport: 1500,
            },
            value: FieldValue::Set(json!(18)),
        },
    );
    state.covered.insert("c".repeat(32), 1);
    state.max_lamport = 1500;
    state.validate().unwrap();
    state
}

#[test]
fn alias_model_receive_preserves_snapshot_and_unlocks_existing_pending() {
    let c = fixture();
    let before = protected_snapshot(&c);
    let revision = logical::revision(&c).unwrap();
    let mut observed = logical::frontier(&c).unwrap();
    observed.insert("d".repeat(32), 1);
    let batch = Batch {
        format: 2,
        library_id: LIB.into(),
        actor: "d".repeat(32),
        intents: vec![
            Intent {
                seq: 1,
                lamport: 1000,
                observed: logical::frontier(&c).unwrap(),
                patch: FieldPatch::from([("p/themeMode".into(), FieldValue::Set(json!("dark")))]),
            },
            Intent {
                seq: 2,
                lamport: 1001,
                observed,
                patch: FieldPatch::from([("p/fontSize".into(), FieldValue::Set(json!(18)))]),
            },
        ],
    };
    batch.validate().unwrap();
    let hash = model::digest(&serde_json::to_vec(&batch).unwrap());
    logical::receive_batch(&c, 0, &batch, Some(&hash)).unwrap();
    assert_eq!(
        protected_snapshot(&c),
        before,
        "successful receive must roll back F/control/work/hold/physical trial effects"
    );
    assert_eq!(
        logical::revision(&c).unwrap(),
        revision + 3,
        "only one new actor and two inserted pending intents advance revision"
    );
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM local_sync_intents", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM local_sync_seen_batches WHERE digest=?1",
            [hash],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert!(logical::apply_ready_intent(&c).unwrap());
    assert!(logical::apply_ready_intent(&c).unwrap());
    assert_eq!(
        logical::frontier(&c).unwrap().get(&"d".repeat(32)),
        Some(&2)
    );

    let c = fixture();
    same_stamp_distinct_fields(&c, true);
    let pending = pending_tied_url(&c, &"c".repeat(32));
    pending.validate().unwrap();
    logical::ingest_batch(&c, 0, &pending).unwrap();
    let before = protected_snapshot(&c);
    let dependency = Batch {
        format: 2,
        library_id: LIB.into(),
        actor: "c".repeat(32),
        intents: vec![Intent {
            seq: 1,
            lamport: 1500,
            observed: logical::frontier(&c).unwrap(),
            patch: FieldPatch::from([("p/fontSize".into(), FieldValue::Set(json!(18)))]),
        }],
    };
    dependency.validate().unwrap();
    let result = logical::receive_batch(
        &c,
        0,
        &dependency,
        Some(&model::digest(&serde_json::to_vec(&dependency).unwrap())),
    );
    println!("SNAPSHOT new_batch_unlock_result={result:?}");
    result.unwrap();
    assert_eq!(
        protected_snapshot(&c),
        before,
        "receiving a dependency cannot trial-write F/frontier/work/holds/physical rows"
    );
    logical::drain_local(&c).unwrap();
    assert_eq!(
        logical::frontier(&c).unwrap().get(&"d".repeat(32)),
        Some(&1)
    );
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM local_sync_intents", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn alias_model_candidate_unlocks_ties_without_trial_main_writes() {
    for ties in [false, true] {
        let c = fixture();
        if ties {
            same_stamp_distinct_fields(&c, true);
        }
        let pending = if ties {
            pending_tied_url(&c, &"c".repeat(32))
        } else {
            let mut observed = logical::frontier(&c).unwrap();
            observed.insert("c".repeat(32), 1);
            Batch {
                format: 2,
                library_id: LIB.into(),
                actor: "d".repeat(32),
                intents: vec![Intent {
                    seq: 1,
                    lamport: 2000,
                    observed,
                    patch: FieldPatch::from([(
                        "p/themeMode".into(),
                        FieldValue::Set(json!("dark")),
                    )]),
                }],
            }
        };
        pending.validate().unwrap();
        logical::ingest_batch(&c, 0, &pending).unwrap();
        let state = add_dependency_state(&c);
        let job = super::super::accepted_candidate_tests::stage(&c, &state);
        let before = complete_snapshot(&c);
        let result = logical::preflight(&c, &job);
        println!("SNAPSHOT candidate ties={ties} result={result:?}");
        assert_eq!(complete_snapshot(&c),before,"candidate preview never writes accepted pointer/frontier/F/work; separate staging root/job work are outside this snapshot");
        let revision = result.unwrap();
        assert!(logical::accept(&c, 0, &job, revision).unwrap());
        assert!(logical::apply_ready_intent(&c).unwrap());
        logical::drain_local(&c).unwrap();
        assert_eq!(
            logical::frontier(&c).unwrap().get(&"d".repeat(32)),
            Some(&1)
        );
    }
}

#[test]
fn alias_model_local_merge_after_receive_keeps_ready_intent_processable() {
    let c = fixture();
    let before = database::checkpoint(&c).unwrap();
    let mut patch = FieldPatch::new();
    for source in [LEFT, RIGHT] {
        let key = format!("a/{}/meta", model::entry_key(source, "real-guid"));
        patch.insert(key.clone(), logical::get(&c, &key).unwrap().unwrap().value);
    }
    let batch = Batch {
        format: 2,
        library_id: LIB.into(),
        actor: "e".repeat(32),
        intents: vec![Intent {
            seq: 1,
            lamport: 1000,
            observed: before.covered.clone(),
            patch,
        }],
    };
    batch.validate().unwrap();
    logical::ingest_batch(&c, 0, &batch).unwrap();
    assert_eq!(database::checkpoint(&c).unwrap(), before);
    let local = logical::capture_patch(
        &c,
        &FieldPatch::from([(
            format!("s/{RIGHT}/feed_url"),
            FieldValue::Set(json!("https://example.test/left")),
        )]),
    );
    let first = logical::apply_ready_intent(&c);
    let second = logical::apply_ready_intent(&c);
    let pending: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM local_sync_intents WHERE actor=?1",
            ["e".repeat(32)],
            |r| r.get(0),
        )
        .unwrap();
    println!("SNAPSHOT interleave local={local:?} apply1={first:?} apply2={second:?} pending={pending} frontier={:?}",logical::frontier(&c).unwrap());
    assert!(local.is_ok());
    assert!(first.is_ok()&&second.is_ok(),"a valid durably received ready intent must remain processable across a later local dependency write; it may not poison the restore queue");
    assert_eq!(pending, 0);
}

#[test]
fn alias_model_same_wire_key_same_stamp_rejects_without_half_acceptance() {
    let c = fixture();
    same_stamp_distinct_fields(&c, true);
    let key = format!("a/{}/meta", model::entry_key(LEFT, "real-guid"));
    let mut bad = logical::get(&c, &key).unwrap().unwrap();
    if let FieldValue::Set(v) = &mut bad.value {
        v["title"] = json!("corrupt same wire key");
    }
    let batch = Batch {
        format: 2,
        library_id: LIB.into(),
        actor: bad.stamp.actor.clone(),
        intents: vec![Intent {
            seq: bad.stamp.seq,
            lamport: bad.stamp.lamport,
            observed: Default::default(),
            patch: FieldPatch::from([(key.clone(), bad.value.clone())]),
        }],
    };
    batch.validate().unwrap();
    let before = complete_snapshot(&c);
    assert!(logical::receive_batch(
        &c,
        0,
        &batch,
        Some(&model::digest(&serde_json::to_vec(&batch).unwrap()))
    )
    .is_err());
    assert_eq!(complete_snapshot(&c),before,"same-key rejection must restore F/frontier/seq/journal/pending/seen/work/holds/business rows");
    let mut corrupt = database::checkpoint(&c).unwrap();
    corrupt.fields.insert(key, bad);
    assert!(database::merge_apply(&c, 0, &corrupt).is_err());
    assert_eq!(
        complete_snapshot(&c),
        before,
        "candidate same-key rejection cannot half-accept; staging roots are separate"
    );
}

fn all_consumers(c: &Connection) -> model::BodyRef {
    let state = database::checkpoint(c).unwrap();
    let pure = database::projection(&state).unwrap();
    let entry = model::entry_key(LEFT, "real-guid");
    let target = format!("a/{entry}");
    let logical = logical::projected(c, &target).unwrap();
    for (key, reg) in &logical {
        assert_eq!(
            pure.get(key),
            Some(reg),
            "pure and SQL-backed Rust projection differ at {key}"
        );
    }
    let source = format!("s/{LEFT}");
    let expected_body: model::BodyRef =
        serde_json::from_value(model::value(&logical, &target, "body").unwrap().clone()).unwrap();
    let expected_read = model::value(&logical, &target, "read")
        .unwrap()
        .as_bool()
        .unwrap();
    let expected_star = model::value(&logical, &target, "star")
        .unwrap()
        .as_bool()
        .unwrap();
    let sql: (bool, bool, String, bool) = c
        .query_row(
            "SELECT is_read,is_starred,body_ref,active FROM local_sync_article_state LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!((sql.0, sql.1), (expected_read, expected_star));
    assert_eq!(
        serde_json::from_str::<model::BodyRef>(&sql.2).unwrap(),
        expected_body
    );
    assert_eq!(sql.3, model::active_source(&logical, &source));
    assert_eq!(
        database::desired_body(c, "https://example.test/left", "real-guid").unwrap(),
        Some(expected_body.clone())
    );
    let sql_title:String=c.query_row("SELECT json_extract(register,'$.value.value') FROM local_sync_source_values WHERE url='https://example.test/left' AND field='title' AND winner=1",[],|r|r.get(0)).unwrap();
    assert_eq!(
        Some(&json!(sql_title)),
        model::value(&logical, &source, "title")
    );
    expected_body
}

#[test]
fn alias_model_ties_body_release_portable_and_split_share_one_order() {
    use crate::local_sync::{backup, body_archive};
    let c = fixture();
    let winner = [LEFT, RIGHT]
        .into_iter()
        .max_by_key(|s| model::entry_key(s, "real-guid"))
        .unwrap();
    let loser = if winner == LEFT { RIGHT } else { LEFT };
    let mut patch = FieldPatch::new();
    for source in [LEFT, RIGHT] {
        for key in [
            format!("s/{source}/title"),
            format!("a/{}/meta", model::entry_key(source, "real-guid")),
            format!("a/{}/body", model::entry_key(source, "real-guid")),
        ] {
            patch.insert(key.clone(), logical::get(&c, &key).unwrap().unwrap().value);
        }
        patch.insert(
            format!("a/{}/read", model::entry_key(source, "real-guid")),
            FieldValue::Set(json!(source == winner)),
        );
        patch.insert(
            format!("a/{}/star", model::entry_key(source, "real-guid")),
            FieldValue::Set(json!(source != winner)),
        );
    }
    let batch = Batch {
        format: 2,
        library_id: LIB.into(),
        actor: "e".repeat(32),
        intents: vec![Intent {
            seq: 1,
            lamport: 1000,
            observed: logical::frontier(&c).unwrap(),
            patch,
        }],
    };
    batch.validate().unwrap();
    logical::ingest_batch(&c, 0, &batch).unwrap();
    logical::drain_local(&c).unwrap();
    let raw = database::checkpoint(&c).unwrap();
    logical::capture_patch(
        &c,
        &FieldPatch::from([(
            format!("s/{RIGHT}/feed_url"),
            FieldValue::Set(json!("https://example.test/left")),
        )]),
    )
    .unwrap();
    logical::drain_local(&c).unwrap();
    let body = all_consumers(&c);
    let winner_body = logical::get(
        &c,
        &format!("a/{}/body", model::entry_key(winner, "real-guid")),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        winner_body.value,
        FieldValue::Set(serde_json::to_value(&body).unwrap())
    );
    for (key, reg) in &raw.fields {
        if key.starts_with("a/") {
            assert_eq!(logical::get(&c, key).unwrap().as_ref(), Some(reg));
        }
    }
    assert_eq!(
        c.query_row(
            "SELECT COUNT(DISTINCT body_ref) FROM local_sync_body_hold",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
    // Isolate the release proof from pending holds without committing their
    // removal. The losing member remains protected by its own current flags.
    c.execute_batch("SAVEPOINT release_proof; DELETE FROM local_sync_body_hold;")
        .unwrap();
    let mut release = body_archive::Release::default();
    let peers = vec![logical::frontier(&c).unwrap()];
    for (owner, expected) in [(winner, true), (loser, false)] {
        let entry = model::entry_key(owner, "real-guid");
        let reg = logical::get(&c, &format!("a/{entry}/body"))
            .unwrap()
            .unwrap();
        let FieldValue::Set(value) = reg.value else {
            panic!("body")
        };
        let entry = body_archive::Entry {
            entry,
            body_ref: serde_json::from_value(value).unwrap(),
        };
        assert_eq!(release.can_release(&c, &peers, &entry).unwrap(), expected);
    }
    c.execute_batch("ROLLBACK TO release_proof; RELEASE release_proof;")
        .unwrap();
    // Another field selects the opposite alias with a later stamp. Export must
    // preserve that priority together with the equal-stamp body group.
    logical::capture_patch(
        &c,
        &FieldPatch::from([(
            format!("a/{}/read", model::entry_key(loser, "real-guid")),
            FieldValue::Set(json!(false)),
        )]),
    )
    .unwrap();
    logical::drain_local(&c).unwrap();
    all_consumers(&c);
    let file = super::super::path("alias-tie-portable");
    let info = backup::export(&c, &file).unwrap();
    let restored = conn();
    let old_actor = database::actor(&c).unwrap();
    backup::restore(&restored, &file, &info.digest).unwrap();
    assert_ne!(database::actor(&restored).unwrap(), old_actor);
    assert_eq!(all_consumers(&restored), body);
    let a = logical::get(
        &restored,
        &format!("a/{}/body", model::entry_key(LEFT, "real-guid")),
    )
    .unwrap()
    .unwrap();
    let b = logical::get(
        &restored,
        &format!("a/{}/body", model::entry_key(RIGHT, "real-guid")),
    )
    .unwrap()
    .unwrap();
    assert_eq!(a.stamp, b.stamp);
    assert_eq!(
        model::value(
            &logical::projected(
                &restored,
                &format!("a/{}", model::entry_key(LEFT, "real-guid"))
            )
            .unwrap(),
            &format!("a/{}", model::entry_key(LEFT, "real-guid")),
            "read"
        ),
        Some(&json!(false))
    );
    logical::capture_patch(
        &restored,
        &FieldPatch::from([(
            format!("s/{RIGHT}/feed_url"),
            FieldValue::Set(json!("https://example.test/right")),
        )]),
    )
    .unwrap();
    logical::drain_local(&restored).unwrap();
    assert_eq!(
        restored
            .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    for source in [LEFT, RIGHT] {
        let key = format!("a/{}/meta", model::entry_key(source, "real-guid"));
        assert_eq!(
            logical::get(&restored, &key).unwrap().unwrap().value,
            raw.fields[&key].value
        );
    }
    std::fs::remove_file(file).unwrap();
}

#[test]
fn alias_model_epoch_ties_keep_exact_reference_after_portable_restore() {
    use crate::local_sync::backup;
    let c = fixture();
    logical::capture_patch(
        &c,
        &FieldPatch::from([(
            format!("s/{RIGHT}/feed_url"),
            FieldValue::Set(json!("https://example.test/left")),
        )]),
    )
    .unwrap();
    logical::drain_local(&c).unwrap();
    let mut state = database::checkpoint(&c).unwrap();
    let earlier = Stamp {
        actor: "e".repeat(32),
        seq: 1,
        lamport: 1000,
    };
    let latest = Stamp {
        actor: "e".repeat(32),
        seq: 2,
        lamport: 1001,
    };
    let reactivated = Stamp {
        actor: "d".repeat(32),
        seq: 1,
        lamport: 2000,
    };
    for (source, epoch) in [(LEFT, earlier), (RIGHT, latest.clone())] {
        state.fields.insert(
            format!("s/{source}/deleted"),
            Register {
                stamp: epoch.clone(),
                value: FieldValue::Set(json!(true)),
            },
        );
        state.fields.insert(
            format!("s/{source}/reactivate"),
            Register {
                stamp: reactivated.clone(),
                value: FieldValue::Set(serde_json::to_value(epoch).unwrap()),
            },
        );
    }
    state.covered.insert("e".repeat(32), 2);
    state.covered.insert("d".repeat(32), 1);
    state.max_lamport = 2000;
    state.validate().unwrap();
    database::merge_apply(&c, 0, &state).unwrap();
    assert!(logical::source_active(&c, LEFT).unwrap());
    let sql: bool = c
        .query_row(
            "SELECT active FROM local_sync_source_state WHERE source=?1",
            [LEFT],
            |r| r.get(0),
        )
        .unwrap();
    assert!(sql);
    assert!(model::active_source(
        &database::projection(&database::checkpoint(&c).unwrap()).unwrap(),
        &format!("s/{LEFT}")
    ));
    let file = super::super::path("alias-epoch-portable");
    let info = backup::export(&c, &file).unwrap();
    let restored = conn();
    backup::restore(&restored, &file, &info.digest).unwrap();
    let deletion = logical::get(&restored, &format!("s/{RIGHT}/deleted"))
        .unwrap()
        .unwrap();
    let activation = logical::get(&restored, &format!("s/{RIGHT}/reactivate"))
        .unwrap()
        .unwrap();
    assert_eq!(
        activation.value,
        FieldValue::Set(serde_json::to_value(&deletion.stamp).unwrap())
    );
    assert!(logical::source_active(&restored, LEFT).unwrap());
    assert!(restored
        .query_row(
            "SELECT active FROM local_sync_source_state WHERE source=?1",
            [LEFT],
            |r| r.get::<_, bool>(0)
        )
        .unwrap());
    let left_activation = logical::get(&restored, &format!("s/{LEFT}/reactivate"))
        .unwrap()
        .unwrap();
    assert_eq!(activation.stamp, left_activation.stamp);
    assert_ne!(activation.value, left_activation.value);
    std::fs::remove_file(file).unwrap();
}

#[test]
fn alias_model_trial_keeps_wal_snapshot_read_only_after_other_writer_commits() {
    let file = super::super::path("alias-wal-trial");
    let c = db::open(&file).unwrap();
    seed(&c, LEFT, "https://example.test/left");
    database::initialize(&c, LIB).unwrap();
    logical::drain_local(&c).unwrap();
    let batch = Batch {
        format: 2,
        library_id: LIB.into(),
        actor: "e".repeat(32),
        intents: vec![Intent {
            seq: 1,
            lamport: 1000,
            observed: logical::frontier(&c).unwrap(),
            patch: FieldPatch::from([("p/themeMode".into(), FieldValue::Set(json!("dark")))]),
        }],
    };
    logical::ingest_batch(&c, 0, &batch).unwrap();
    let writer = db::open(&file).unwrap();
    let tx = c.unchecked_transaction().unwrap();
    let before = complete_snapshot(&tx);
    let frontier = logical::frontier(&tx).unwrap();
    logical::capture_patch(
        &writer,
        &FieldPatch::from([("p/fontSize".into(), FieldValue::Set(json!(19)))]),
    )
    .unwrap();
    let committed = complete_snapshot(&writer);
    // The old main-table SAVEPOINT trial would now fail BUSY_SNAPSHOT. A TEMP
    // overlay can still validate the pinned main snapshot without upgrading it.
    logical::preflight_ready_intents(&tx, logical::WINNERS, &frontier).unwrap();
    assert_eq!(complete_snapshot(&tx), before);
    tx.commit().unwrap();
    assert_eq!(complete_snapshot(&c), committed);
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM temp.local_sync_trial_fields",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    logical::drain_local(&c).unwrap();
    assert_eq!(
        logical::frontier(&c).unwrap().get(&"e".repeat(32)),
        Some(&1)
    );
    drop(writer);
    drop(c);
    std::fs::remove_file(file).unwrap();
}

#[test]
fn alias_model_129_members_finish_old_feed_cleanup_and_split() {
    let c = fixture_with_extra(128);
    same_stamp_distinct_fields(&c, false);
    let old_feed: i64 = c
        .query_row("SELECT id FROM feeds WHERE sync_id=?1", [LEFT], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM articles WHERE feed_id=?1",
            [old_feed],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        129
    );
    let before = database::checkpoint(&c).unwrap();
    let bodies: i64 = c
        .query_row("SELECT COUNT(*) FROM contents", [], |r| r.get(0))
        .unwrap();
    let mut state = before.clone();
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
    let job = super::super::accepted_candidate_tests::stage(&c, &state);
    let revision = logical::preflight(&c, &job).unwrap();
    assert!(logical::accept(&c, 0, &job, revision).unwrap());
    c.execute(
        "UPDATE feeds SET feed_url='https://example.test/logical-target' WHERE sync_id=?1",
        [RIGHT],
    )
    .unwrap();
    let mut cursors = std::collections::BTreeSet::new();
    let mut finished = false;
    let mut slices = 0;
    for step in 0..2048 {
        let mut stmt = c
            .prepare("SELECT cursor FROM local_sync_work WHERE key=?1 AND cursor!=''")
            .unwrap();
        for cursor in stmt
            .query_map([format!("source/{LEFT}")], |r| r.get::<_, String>(0))
            .unwrap()
        {
            cursors.insert(cursor.unwrap());
        }
        drop(stmt);
        if !logical::resume_slice(&c).unwrap() {
            finished = true;
            slices = step;
            break;
        }
    }
    println!("CROSS_SLICE slices={slices} cursors={cursors:?}");
    assert!(finished, "129-member merge must make finite progress");
    assert!(
        cursors.len() >= 2,
        "scope must advance beyond one 64-member page"
    );
    assert!(!c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM feeds WHERE id=?1)",
            [old_feed],
            |r| r.get::<_, bool>(0)
        )
        .unwrap());
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM articles", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        129
    );
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM contents", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        bodies
    );
    assert!(c
        .query_row(
            "SELECT is_starred FROM articles WHERE guid='extra-064'",
            [],
            |r| r.get::<_, bool>(0)
        )
        .unwrap());
    logical::capture_patch(
        &c,
        &FieldPatch::from([(
            format!("s/{LEFT}/feed_url"),
            FieldValue::Set(json!("https://example.test/left")),
        )]),
    )
    .unwrap();
    let mut finished = false;
    for _ in 0..2048 {
        if !logical::resume_slice(&c).unwrap() {
            finished = true;
            break;
        }
    }
    assert!(finished, "split must also finish");
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM articles", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        130
    );
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM articles WHERE local_sync_source=?1",
            [LEFT],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        129
    );
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM contents", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        bodies
    );
    for (key, register) in before.fields {
        if key.starts_with("a/") {
            assert_eq!(logical::get(&c, &key).unwrap(), Some(register));
        }
    }
}

fn insert_test_register(
    conn: &Connection,
    key: &str,
    actor: &str,
    seq: u64,
    lamport: u64,
    value: Value,
) {
    conn.execute(
        "INSERT INTO local_sync_fields(key,register) VALUES(?1,?2)",
        rusqlite::params![
            key,
            serde_json::to_string(&Register {
                stamp: Stamp {
                    actor: actor.into(),
                    seq,
                    lamport,
                },
                value: FieldValue::Set(value),
            })
            .unwrap()
        ],
    )
    .unwrap();
    let frontier: String = conn
        .query_row(
            "SELECT frontier FROM local_sync_control WHERE id=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let mut frontier: BTreeMap<String, u64> = serde_json::from_str(&frontier).unwrap();
    let covered = frontier.entry(actor.to_owned()).or_default();
    *covered = (*covered).max(seq);
    conn.execute(
        "UPDATE local_sync_control
         SET frontier=?1, lamport=MAX(lamport,?2) WHERE id=1",
        params![serde_json::to_string(&frontier).unwrap(), lamport],
    )
    .unwrap();
}

fn insert_test_source(conn: &Connection, source: &str, url: &str, actor: &str, seq: u64) {
    insert_test_register(
        conn,
        &format!("s/{source}/feed_url"),
        actor,
        seq,
        100 + seq,
        json!(url),
    );
    insert_test_register(
        conn,
        &format!("s/{source}/created"),
        actor,
        seq,
        100 + seq,
        json!(true),
    );
}

fn insert_test_article(
    conn: &Connection,
    key_source: &str,
    meta_source: &str,
    guid: &str,
    actor: &str,
    seq: u64,
) {
    insert_test_register(
        conn,
        &format!("a/{}/meta", model::entry_key(key_source, guid)),
        actor,
        seq,
        100 + seq,
        json!({
            "source": meta_source,
            "guid": guid,
            "url": null,
            "title": format!("article-{guid}"),
            "author": null,
            "published_at": null,
            "published_at_source": "legacy_unknown",
            "first_seen_at": null,
            "image_url": null,
            "image_source": "page",
            "enclosure_url": null,
            "enclosure_mime": null,
            "duration_sec": null
        }),
    );
}

fn stage_sparse_source_url(conn: &Connection, source: &str, url: &str, actor: &str) -> String {
    logical::register_actor(conn, actor).unwrap();
    let (library, covered, max_lamport) = conn
        .query_row(
            "SELECT library_id,frontier,lamport FROM local_sync_control WHERE id=1",
            [],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, u64>(2)?,
                ))
            },
        )
        .unwrap();
    // This fixture deliberately stages a sparse candidate while an existing
    // winner may be malformed for the rejection case. Only the validated header
    // comes from production state; candidate bytes are supplied below.
    let mut state = crate::webdav_transport::Checkpoint {
        format: 2,
        library_id: library,
        covered: serde_json::from_str(&covered).unwrap(),
        max_lamport,
        fields: BTreeMap::new(),
    };
    let lamport = state.max_lamport + 1_000;
    state.fields.insert(
        format!("s/{source}/feed_url"),
        Register {
            stamp: Stamp {
                actor: actor.into(),
                seq: 1,
                lamport,
            },
            value: FieldValue::Set(json!(url)),
        },
    );
    state.covered.insert(actor.into(), 1);
    state.max_lamport = lamport;
    state.validate().unwrap();
    crate::local_sync::database_tests::accepted_candidate_tests::stage(conn, &state)
}

#[test]
fn source_url_preflight_closure_includes_old_and_new_alias_sources_and_articles() {
    let c = conn();
    database::initialize(&c, LIB).unwrap();
    let s1 = "1".repeat(32);
    let s2 = "2".repeat(32);
    let s3 = "3".repeat(32);
    let unrelated = "6".repeat(32);
    let base = "4".repeat(32);
    insert_test_source(&c, &s1, "https://example.test/old", &base, 1);
    insert_test_source(&c, &s2, "https://example.test/old", &base, 2);
    insert_test_source(&c, &s3, "https://example.test/new", &base, 3);
    insert_test_source(&c, &unrelated, "https://example.test/unrelated", &base, 4);
    insert_test_article(&c, &s1, &s1, "shared-guid", &base, 4);
    insert_test_article(&c, &s2, &s2, "old-only-guid", &base, 5);
    insert_test_article(&c, &s2, &s2, "old-second-guid", &base, 6);
    insert_test_article(&c, &s3, &s3, "shared-guid", &base, 6);
    insert_test_article(&c, &unrelated, &unrelated, "unrelated-guid", &base, 7);
    c.execute("DELETE FROM local_sync_work", []).unwrap();
    c.execute(
        "UPDATE local_sync_control SET logical_revision=logical_revision+1",
        [],
    )
    .unwrap();

    let job = stage_sparse_source_url(&c, &s1, "https://example.test/new", &"5".repeat(32));
    let targets = logical::validation_targets_for_test(&c, &job).unwrap();
    for target in [
        format!("s/{s1}"),
        format!("s/{s2}"),
        format!("s/{s3}"),
        format!("a/{}", model::entry_key(&s1, "shared-guid")),
        format!("a/{}", model::entry_key(&s2, "old-only-guid")),
        format!("a/{}", model::entry_key(&s2, "old-second-guid")),
        format!("a/{}", model::entry_key(&s3, "shared-guid")),
    ] {
        assert!(
            targets.contains(&target),
            "affected preflight closure omitted {target}: {targets:?}"
        );
    }
    assert!(
        !targets.contains(&format!("s/{unrelated}"))
            && !targets.contains(&format!(
                "a/{}",
                model::entry_key(&unrelated, "unrelated-guid")
            )),
        "closure must not include an unrelated source or article: {targets:?}"
    );
}

#[test]
fn source_url_preflight_rejects_bad_dependency_on_non_candidate_old_alias_page() {
    let c = conn();
    database::initialize(&c, LIB).unwrap();
    let s1 = "6".repeat(32);
    let s2 = "7".repeat(32);
    insert_test_source(&c, &s1, "https://example.test/old", &"8".repeat(32), 1);
    insert_test_source(&c, &s2, "https://example.test/old", &"8".repeat(32), 2);
    insert_test_article(&c, &s2, &s2, "bad-dependency", &"8".repeat(32), 3);
    // Keep the old URL so this non-candidate source remains in the URL alias
    // closure; its later stamp makes the invalid explicit-creation value win
    // over the candidate source's valid register.
    c.execute(
        "UPDATE local_sync_fields
            SET register=json_set(register,'$.value.value',json('false'))
          WHERE key=?1",
        [format!("s/{s2}/created")],
    )
    .unwrap();
    c.execute("DELETE FROM local_sync_work", []).unwrap();
    c.execute(
        "UPDATE local_sync_control SET logical_revision=logical_revision+1",
        [],
    )
    .unwrap();

    let job = stage_sparse_source_url(&c, &s1, "https://example.test/new", &"a".repeat(32));
    let error = logical::preflight(&c, &job).unwrap_err();
    assert!(
        error.message.contains("缺少显式创建"),
        "bad dependency on a non-candidate old-alias page must reject: {error:?}"
    );
}
