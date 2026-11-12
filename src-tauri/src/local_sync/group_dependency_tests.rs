//! Unified group-creation dependency regressions, based on the A6 review inputs.
use super::{conn, LIB};
use crate::{
    local_sync::{database, logical, model},
    webdav_transport::{Batch, FieldPatch, FieldValue, Intent},
};
use rusqlite::{types::Value as SqlValue, Connection};
use serde_json::json;
use std::collections::BTreeMap;
const GROUP: &str = "cccccccccccccccccccccccccccccccc";
const PEER: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

fn snapshot(c: &Connection) -> BTreeMap<String, Vec<Vec<SqlValue>>> {
    let mut out = BTreeMap::new();
    for table in [
        "local_sync_control",
        "local_sync_fields",
        "local_sync_journal",
        "local_sync_work",
        "local_sync_body_hold",
        "local_sync_intents",
        "local_sync_seen_batches",
        "local_sync_known_actors",
        "folders",
        "feeds",
        "articles",
        "contents",
        "sync_queue",
    ] {
        let mut stmt = c
            .prepare(&format!("SELECT rowid,* FROM {table} ORDER BY rowid"))
            .unwrap();
        let n = stmt.column_count();
        let rows = stmt
            .query_map([], |r| (0..n).map(|i| r.get::<_, SqlValue>(i)).collect())
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        out.insert(table.into(), rows);
    }
    out
}
fn batch(c: &Connection, created: bool) -> Batch {
    let mut patch = FieldPatch::from([(
        format!("g/{GROUP}/name"),
        FieldValue::Set(json!("New remote group")),
    )]);
    if created {
        patch.insert(format!("g/{GROUP}/created"), FieldValue::Set(json!(true)));
    }
    Batch {
        format: 2,
        library_id: LIB.into(),
        actor: PEER.into(),
        intents: vec![Intent {
            seq: 1,
            lamport: 1000,
            observed: logical::frontier(c).unwrap(),
            patch,
        }],
    }
}

#[test]
fn a6_group_missing_creation_rejected_before_pending_acceptance() {
    let c = conn();
    database::initialize(&c, LIB).unwrap();
    logical::drain_local(&c).unwrap();
    let batch = batch(&c, false);
    batch.validate().unwrap();
    let before = snapshot(&c);
    let before_frontier = logical::frontier(&c).unwrap();
    let digest = model::digest(&serde_json::to_vec(&batch).unwrap());
    let received = logical::receive_batch(&c, 0, &batch, Some(&digest));
    println!("GROUP_DEP received={received:?}");
    if received.is_ok() {
        let first = logical::apply_ready_intent(&c);
        let second = logical::apply_ready_intent(&c);
        let pending: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM local_sync_intents WHERE actor=?1",
                [PEER],
                |r| r.get(0),
            )
            .unwrap();
        let seen: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM local_sync_seen_batches WHERE digest=?1",
                [digest],
                |r| r.get(0),
            )
            .unwrap();
        let known: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM local_sync_known_actors WHERE actor=?1",
                [PEER],
                |r| r.get(0),
            )
            .unwrap();
        println!("GROUP_DEP first={first:?} second={second:?} pending={pending} seen={seen} actor={known} frontier_before={before_frontier:?} frontier_after={:?}",logical::frontier(&c).unwrap());
    }
    assert!(received.is_err(),"satisfied causal frontier cannot substitute for the group's explicit created register; reject malformed dependency before durable pending/seen/actor acceptance");
    assert_eq!(
        snapshot(&c),
        before,
        "rejected creation dependency must leave no receive or physical effects"
    );
}

#[test]
fn a6_group_explicit_creation_and_name_applies() {
    let c = conn();
    database::initialize(&c, LIB).unwrap();
    logical::drain_local(&c).unwrap();
    let batch = batch(&c, true);
    batch.validate().unwrap();
    logical::ingest_batch(&c, 0, &batch).unwrap();
    assert!(logical::apply_ready_intent(&c).unwrap());
    assert!(!logical::apply_ready_intent(&c).unwrap());
    assert_eq!(
        c.query_row("SELECT name FROM folders WHERE sync_id=?1", [GROUP], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "New remote group"
    );
    assert_eq!(logical::frontier(&c).unwrap().get(PEER), Some(&1));
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM local_sync_intents", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn a6_group_capture_and_sql_share_validation_existing_updates_work() {
    let c = conn();
    database::initialize(&c, LIB).unwrap();
    logical::drain_local(&c).unwrap();
    let bad = FieldPatch::from([(
        format!("g/{GROUP}/name"),
        FieldValue::Set(json!("Missing creation")),
    )]);
    let before = snapshot(&c);
    assert!(logical::capture_patch(&c, &bad).is_err());
    assert_eq!(snapshot(&c), before);
    // Deliberately orphan a physical local row while capture is disabled. The
    // ordinary SQL update must still use the shared entity check, not its row.
    c.execute_batch("UPDATE local_sync_control SET capture=0 WHERE id=1")
        .unwrap();
    c.execute(
        "INSERT INTO folders(sync_id,name) VALUES(?1,'physical only')",
        [GROUP],
    )
    .unwrap();
    c.execute_batch("UPDATE local_sync_control SET capture=1 WHERE id=1")
        .unwrap();
    let before = snapshot(&c);
    let result = c.execute(
        "UPDATE folders SET name='invalid update' WHERE sync_id=?1",
        [GROUP],
    );
    assert!(result.is_err());
    assert_eq!(
        snapshot(&c),
        before,
        "UDF failure aborts physical update and capture effects"
    );
    logical::capture_patch(
        &c,
        &FieldPatch::from([
            (format!("g/{GROUP}/created"), FieldValue::Set(json!(true))),
            (format!("g/{GROUP}/name"), FieldValue::Set(json!("Created"))),
        ]),
    )
    .unwrap();
    logical::drain_local(&c).unwrap();
    c.execute(
        "UPDATE folders SET name='SQL updated' WHERE sync_id=?1",
        [GROUP],
    )
    .unwrap();
    logical::capture_patch(
        &c,
        &FieldPatch::from([(
            format!("g/{GROUP}/name"),
            FieldValue::Set(json!("Explicit updated")),
        )]),
    )
    .unwrap();
    logical::drain_local(&c).unwrap();
    assert_eq!(
        c.query_row("SELECT name FROM folders WHERE sync_id=?1", [GROUP], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "Explicit updated"
    );
}

#[test]
fn a6_group_standalone_tombstone_survives_later_name_without_resurrection() {
    let c = conn();
    database::initialize(&c, LIB).unwrap();
    logical::drain_local(&c).unwrap();
    let batch = Batch {
        format: 2,
        library_id: LIB.into(),
        actor: PEER.into(),
        intents: vec![Intent {
            seq: 1,
            lamport: 1000,
            observed: logical::frontier(&c).unwrap(),
            patch: FieldPatch::from([(format!("g/{GROUP}/deleted"), FieldValue::Set(json!(true)))]),
        }],
    };
    batch.validate().unwrap();
    logical::receive_batch(
        &c,
        0,
        &batch,
        Some(&model::digest(&serde_json::to_vec(&batch).unwrap())),
    )
    .unwrap();
    assert!(logical::apply_ready_intent(&c).unwrap());
    assert!(!logical::apply_ready_intent(&c).unwrap());
    logical::capture_patch(
        &c,
        &FieldPatch::from([(
            format!("g/{GROUP}/name"),
            FieldValue::Set(json!("Stale ordinary edit")),
        )]),
    )
    .unwrap();
    logical::drain_local(&c).unwrap();
    assert!(logical::get(&c, &format!("g/{GROUP}/created"))
        .unwrap()
        .is_none());
    assert_eq!(
        logical::get(&c, &format!("g/{GROUP}/deleted"))
            .unwrap()
            .unwrap()
            .value,
        FieldValue::Set(json!(true))
    );
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM folders WHERE sync_id=?1",
            [GROUP],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn a6_group_candidate_and_pure_projection_use_same_creation_contract() {
    use crate::webdav_transport::{Register, Stamp};
    for kind in ["missing", "created", "deleted"] {
        let c = conn();
        database::initialize(&c, LIB).unwrap();
        logical::drain_local(&c).unwrap();
        let mut state = database::checkpoint(&c).unwrap();
        let stamp = Stamp {
            actor: PEER.into(),
            seq: 1,
            lamport: 1000,
        };
        if kind != "deleted" {
            state.fields.insert(
                format!("g/{GROUP}/name"),
                Register {
                    stamp: stamp.clone(),
                    value: FieldValue::Set(json!("Candidate group")),
                },
            );
        }
        if kind != "missing" {
            state.fields.insert(
                format!("g/{GROUP}/{kind}"),
                Register {
                    stamp,
                    value: FieldValue::Set(json!(true)),
                },
            );
        }
        state.covered.insert(PEER.into(), 1);
        state.max_lamport = 1000;
        state.validate().unwrap();
        model::validate_checkpoint(&state).unwrap();
        let pure = database::projection(&state);
        let job = super::accepted_candidate_tests::stage(&c, &state);
        let before = database::checkpoint(&c).unwrap();
        let mut protected = snapshot(&c);
        protected.remove("local_sync_work");
        let checked = logical::preflight(&c, &job);
        let mut after = snapshot(&c);
        after.remove("local_sync_work");
        assert_eq!(
            after, protected,
            "candidate staging work is separate; main state cannot half-accept"
        );
        assert_eq!(database::checkpoint(&c).unwrap(), before);
        if kind == "missing" {
            assert!(pure.is_err());
            assert!(checked.is_err());
            assert_eq!(
                c.query_row("SELECT accepted_job FROM local_sync_control", [], |r| {
                    r.get::<_, Option<String>>(0)
                })
                .unwrap(),
                None
            );
        } else {
            pure.unwrap();
            assert!(logical::accept(&c, 0, &job, checked.unwrap()).unwrap());
            logical::drain_local(&c).unwrap();
            assert_eq!(
                c.query_row(
                    "SELECT COUNT(*) FROM folders WHERE sync_id=?1",
                    [GROUP],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                i64::from(kind == "created")
            );
        }
    }
}
