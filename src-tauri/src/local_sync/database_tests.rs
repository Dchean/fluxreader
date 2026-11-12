//! Pure SQLite counterexamples. No sockets, application host or live credentials.
use super::{backup, database, storage};
use crate::db;
use rusqlite::{params, Connection};
use serde_json::json;
const LIB: &str = "11111111111111111111111111111111";
fn conn() -> Connection {
    let mut c = Connection::open_in_memory().unwrap();
    db::MIGRATIONS.to_latest(&mut c).unwrap();
    database::register_functions(&c).unwrap();
    c.pragma_update(None, "foreign_keys", true).unwrap();
    c
}
fn seed(c: &Connection, uid: &str, url: &str) -> (i64, i64) {
    let folder = db::create_folder_with_sync_id(
        c,
        "Group",
        "article",
        Some("22222222222222222222222222222222"),
    )
    .unwrap();
    let feed =
        db::insert_feed(c, url, None, "Feed", None, folder, "inherit", false, false).unwrap();
    c.execute(
        "UPDATE feeds SET sync_id=?1 WHERE id=?2",
        params![uid, feed],
    )
    .unwrap();
    let a = db::NewArticle {
        guid: "real-guid".into(),
        url: Some("https://example.test/post".into()),
        title: "Article".into(),
        author: None,
        summary: Some("body".into()),
        content_html: Some("<p>body</p>".into()),
        body_text: "body".into(),
        image_url: None,
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: Some("2026-11-09T00:00:00Z".into()),
        source: "direct".into(),
    };
    let (article, _) = db::upsert_article_with_feed(c, feed, &a, false).unwrap();
    (feed, article)
}
fn exchange(a: &Connection, b: &Connection) {
    let ca = database::checkpoint(a).unwrap();
    let cb = database::checkpoint(b).unwrap();
    database::merge_apply(a, 0, &cb).unwrap();
    database::merge_apply(b, 0, &ca).unwrap();
}
fn flags(c: &Connection) -> (bool, bool) {
    c.query_row("SELECT is_read,is_starred FROM articles LIMIT 1", [], |r| {
        Ok((r.get(0)?, r.get(1)?))
    })
    .unwrap()
}

/// Small production-SQL tests for the reviewed partial-materialization model.
/// These intentionally stop before drain_local, leaving the physical row stale.
pub(super) mod accepted_candidate_tests {
    use super::*;
    use crate::local_sync::{fast, logical, model};
    use crate::webdav_transport::{
        Batch, FieldValue, Intent, PreparedBatch, Register, Replica, Stamp,
    };

    pub(super) fn stage(c: &Connection, state: &crate::webdav_transport::Checkpoint) -> String {
        let bytes = serde_json::to_vec(&state.fields).unwrap();
        let hash = model::digest(&bytes);
        let manifest = storage::Manifest {
            format: model::FORMAT,
            library_id: state.library_id.clone(),
            actor: state.fields.values().next().unwrap().stamp.actor.clone(),
            publication: 1,
            kind: "state".into(),
            covered: state.covered.clone(),
            completed: Default::default(),
            max_lamport: state.max_lamport,
            deltas: 0,
            shards: vec![storage::ObjectRef {
                name: format!("{hash}-{}.bin", "f".repeat(32)),
                digest: hash,
                size: bytes.len(),
            }],
        };
        let job = logical::begin_receive(c, 0, &manifest).unwrap();
        logical::stage_page(c, 0, &job, 0, &state.fields).unwrap();
        job
    }
    fn pair() -> (Connection, Connection, i64, i64) {
        let a = conn();
        let b = conn();
        let (_, id) = seed(&a, &"a".repeat(32), "https://example.test/rss");
        database::initialize(&a, LIB).unwrap();
        database::initialize(&b, LIB).unwrap();
        database::merge_apply(&b, 0, &database::checkpoint(&a).unwrap()).unwrap();
        let bid = b
            .query_row("SELECT id FROM articles", [], |r| r.get(0))
            .unwrap();
        (a, b, id, bid)
    }

    #[test]
    fn accepted_logical_winners_drive_every_pending_query_surface_and_match_direct_state() {
        let source = "a".repeat(32);
        let populate = |db: &Connection| {
            let (feed, first) = seed(db, &source, "https://example.test/rss");
            let second = db::NewArticle {
                guid: "second-guid".into(),
                url: Some("https://example.test/post-2".into()),
                title: "Article".into(),
                author: None,
                summary: Some("body".into()),
                content_html: Some("<p>body</p>".into()),
                body_text: "body".into(),
                image_url: None,
                enclosure_url: None,
                enclosure_mime: None,
                duration_sec: None,
                published_at: Some("2026-11-09T00:00:00Z".into()),
                source: "direct".into(),
            };
            let (second, inserted) =
                db::upsert_article_with_feed(db, feed, &second, false).unwrap();
            assert!(inserted);
            db.execute(
                "UPDATE articles SET rss_content_id=(
                     SELECT content_id FROM articles WHERE guid='real-guid'
                 )",
                [],
            )
            .unwrap();
            (feed, first, second)
        };

        let pending_db = conn();
        let (feed, first, second) = populate(&pending_db);
        database::initialize(&pending_db, LIB).unwrap();
        logical::drain_local(&pending_db).unwrap();
        let (actor, library): (String, String) = pending_db
            .query_row(
                "SELECT actor,library_id FROM local_sync_control WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let job = "d".repeat(64);
        pending_db
            .execute(
                "INSERT INTO local_sync_receive(
                     id,generation,local_actor,library_id,manifest,phase
                 ) VALUES(?1,0,?2,?3,'{}','accepted')",
                params![job, actor.clone(), library],
            )
            .unwrap();
        let register = |value: bool| {
            serde_json::to_string(&Register {
                stamp: Stamp {
                    actor: actor.clone(),
                    seq: 1,
                    lamport: 1_000_000,
                },
                value: FieldValue::Set(json!(value)),
            })
            .unwrap()
        };
        for (guid, read, starred) in [("real-guid", true, true), ("second-guid", false, true)] {
            let entry = model::entry_key(&source, guid);
            pending_db
                .execute(
                    "INSERT INTO local_sync_candidates(job,key,register) VALUES(?1,?2,?3)",
                    params![job, format!("a/{entry}/read"), register(read)],
                )
                .unwrap();
            pending_db
                .execute(
                    "INSERT INTO local_sync_candidates(job,key,register) VALUES(?1,?2,?3)",
                    params![job, format!("a/{entry}/star"), register(starred)],
                )
                .unwrap();
        }
        pending_db
            .execute(
                "UPDATE local_sync_control
                    SET accepted_job=?1,logical_revision=logical_revision+1
                  WHERE id=1",
                [&job],
            )
            .unwrap();
        assert!(logical::query_requires_logical_state(&pending_db).unwrap());
        assert_eq!(
            pending_db
                .query_row(
                    "SELECT SUM(is_read),SUM(is_starred) FROM articles",
                    [],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
                )
                .unwrap(),
            (0, 0),
            "the accepted candidate remains physically pending"
        );

        let direct_db = conn();
        let (direct_feed, direct_first, direct_second) = populate(&direct_db);
        assert_eq!(
            (feed, first, second),
            (direct_feed, direct_first, direct_second)
        );
        direct_db
            .execute(
                "UPDATE articles SET is_starred=1,
                    is_read=CASE WHEN guid='real-guid' THEN 1 ELSE 0 END",
                [],
            )
            .unwrap();
        database::initialize(&direct_db, LIB).unwrap();
        logical::drain_local(&direct_db).unwrap();
        assert!(!logical::query_requires_logical_state(&direct_db).unwrap());

        let unread_query = db::ArticleQuery {
            feed_id: Some(feed),
            folder_id: None,
            only_unread: true,
            only_starred: false,
            only_today: false,
            newest_first: true,
            limit: 100,
            offset: 0,
            with_content: false,
            layout: None,
            last_published: None,
            last_id: None,
        };
        let starred_query = db::ArticleQuery {
            only_unread: false,
            only_starred: true,
            ..unread_query.clone()
        };
        let snapshot = |conn: &Connection| {
            let unread = db::list_articles(conn, &unread_query).unwrap();
            let starred = db::list_articles(conn, &starred_query).unwrap();
            let unread_members = db::list_article_member_ids(conn, &unread_query, None).unwrap();
            let starred_members = db::list_article_member_ids(conn, &starred_query, None).unwrap();
            let updates = db::get_article_updates(conn, &[first, second]).unwrap();
            let search = db::search_articles(conn, "Article", 100).unwrap();
            let feed_count = db::feed_counts(conn)
                .unwrap()
                .into_iter()
                .find(|row| row.feed_id == feed)
                .unwrap();
            let all_counts = db::scope_counts(conn, None).unwrap();
            let article_counts = db::scope_counts(conn, Some("article")).unwrap();
            json!({
                "unread_ids": unread.iter().map(|row| row.id).collect::<Vec<_>>(),
                "starred_ids": starred.iter().map(|row| row.id).collect::<Vec<_>>(),
                "unread_flags": unread.iter().map(|row| (row.is_read,row.is_starred)).collect::<Vec<_>>(),
                "starred_flags": starred.iter().map(|row| (row.is_read,row.is_starred)).collect::<Vec<_>>(),
                "unread_index_first": db::article_index(conn, &unread_query, first).unwrap(),
                "unread_index_second": db::article_index(conn, &unread_query, second).unwrap(),
                "starred_index_first": db::article_index(conn, &starred_query, first).unwrap(),
                "starred_index_second": db::article_index(conn, &starred_query, second).unwrap(),
                "unread_members": unread_members,
                "starred_members": starred_members,
                "updates": updates.items.iter().map(|row| (row.id,row.is_read,row.is_starred)).collect::<Vec<_>>(),
                "search": search.iter().map(|row| (row.id,row.is_read,row.is_starred)).collect::<Vec<_>>(),
                "feed": feed_count,
                "all_counts": all_counts,
                "article_counts": article_counts,
                "logical_unread_ids": db::list_unread_ids_scoped(
                    conn,None,None,false,None,None
                ).unwrap(),
                "read_target_ids": db::list_read_target_ids_scoped(
                    conn,None,None,false,None,None
                ).unwrap(),
            })
        };

        let pending = snapshot(&pending_db);
        assert_eq!(pending["unread_ids"], json!([second]));
        assert_eq!(pending["starred_ids"], json!([first]));
        assert_eq!(pending["unread_flags"], json!([(false, true)]));
        assert_eq!(pending["starred_flags"], json!([(false, true)]));
        assert_eq!(pending["unread_index_first"], serde_json::Value::Null);
        assert_eq!(pending["unread_index_second"], json!(0));
        assert_eq!(pending["starred_index_first"], json!(0));
        assert_eq!(pending["starred_index_second"], serde_json::Value::Null);
        assert_eq!(pending["unread_members"], json!([second]));
        assert_eq!(pending["starred_members"], json!([first]));
        assert_eq!(pending["search"], json!([(first, false, true)]));
        assert_eq!(pending["feed"]["unread"], json!(1));
        assert_eq!(pending["feed"]["starred"], json!(1));
        assert_eq!(pending["all_counts"]["all"]["unread"], json!(1));
        assert_eq!(pending["all_counts"]["all"]["starred"], json!(1));
        assert_eq!(pending["article_counts"]["all"]["unread"], json!(1));
        assert_eq!(pending["article_counts"]["all"]["starred"], json!(1));
        assert_eq!(pending["logical_unread_ids"], json!([second]));
        assert_eq!(pending["read_target_ids"], json!([first, second]));

        let direct = snapshot(&direct_db);
        assert_eq!(
            pending, direct,
            "pending logical and materialized direct query surfaces must be identical"
        );

        assert_eq!(
            db::mark_all_read_with_enqueue(&pending_db, None, None, false, None, None).unwrap(),
            1,
            "only the logical unread member contributes a new read flip"
        );
        assert_eq!(
            pending_db
                .query_row("SELECT SUM(is_read) FROM articles", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            2,
            "mark-all must materialize the complete adjacent closure"
        );
    }
    #[test]
    fn t2_explicit_same_physical_read_beats_accepted_opposite_without_other_fields() {
        let (a, b, aid, bid) = pair();
        db::set_read_with_enqueue(&a, aid, true).unwrap();
        database::merge_apply(&b, 0, &database::checkpoint(&a).unwrap()).unwrap();
        db::set_read_with_enqueue(&a, aid, false).unwrap();
        let state = database::checkpoint(&a).unwrap();
        let job = stage(&b, &state);
        let checked = logical::preflight(&b, &job).unwrap();
        assert!(logical::accept(&b, 0, &job, checked).unwrap());
        assert!(flags(&b).0, "physical row is deliberately not projected");
        let before: u64 = b
            .query_row("SELECT seq FROM local_sync_control", [], |r| r.get(0))
            .unwrap();
        db::set_read_with_enqueue(&b, bid, true).unwrap();
        let (seq, patch): (u64, String) = b
            .query_row(
                "SELECT seq,patch FROM local_sync_journal ORDER BY seq DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(seq, before + 1);
        let patch: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(&patch).unwrap();
        assert_eq!(patch.len(), 1);
        assert!(patch.keys().all(|k| k.ends_with("/read")));
        logical::drain_local(&b).unwrap();
        assert!(flags(&b).0);
    }
    #[test]
    fn t1_fast_intent_applies_with_candidate_projection_paused_and_matches_replica() {
        let (a, b, aid, bid) = pair();
        // Both have accepted the same dependency but B's table fold stays paused.
        db::set_read_with_enqueue(&a, aid, true).unwrap();
        let state = database::checkpoint(&a).unwrap();
        let job = stage(&b, &state);
        let checked = logical::preflight(&b, &job).unwrap();
        assert!(logical::accept(&b, 0, &job, checked).unwrap());
        let base = database::checkpoint(&b).unwrap();
        db::set_starred_with_enqueue(&a, aid, true).unwrap();
        let (seq,lamport,observed,patch):(u64,u64,String,String)=a.query_row("SELECT seq,lamport,observed,patch FROM local_sync_journal ORDER BY seq DESC LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
        let batch = Batch {
            format: 2,
            library_id: LIB.into(),
            actor: database::actor(&a).unwrap(),
            intents: vec![Intent {
                seq,
                lamport,
                observed: serde_json::from_str(&observed).unwrap(),
                patch: serde_json::from_str(&patch).unwrap(),
            }],
        };
        PreparedBatch::new(batch.clone()).unwrap();
        let mut replica = Replica::from_checkpoint(base).unwrap();
        replica.ingest_batch(&batch).unwrap();
        logical::ingest_batch(&b, 0, &batch).unwrap();
        assert!(logical::apply_ready_intent(&b).unwrap());
        assert!(b
            .query_row("SELECT is_starred FROM articles WHERE id=?1", [bid], |r| {
                r.get::<_, bool>(0)
            })
            .unwrap());
        assert!(b
            .query_row(
                "SELECT accepted_job IS NOT NULL FROM local_sync_control",
                [],
                |r| r.get::<_, bool>(0)
            )
            .unwrap());
        assert_eq!(
            database::checkpoint(&b).unwrap(),
            replica.checkpoint().unwrap()
        );
        let outgoing = fast::prepare(&a, 0).unwrap().unwrap();
        let pending: Batch = serde_json::from_slice(&outgoing.bytes).unwrap();
        assert_eq!(pending.actor, batch.actor);
    }
    #[test]
    fn t3_acceptance_rechecks_revision_before_any_candidate_visibility() {
        let (a, b, aid, bid) = pair();
        db::set_starred_with_enqueue(&a, aid, true).unwrap();
        let job = stage(&b, &database::checkpoint(&a).unwrap());
        let checked = logical::preflight(&b, &job).unwrap();
        let before = logical::frontier(&b).unwrap();
        db::set_read_with_enqueue(&b, bid, true).unwrap();
        assert!(!logical::accept(&b, 0, &job, checked).unwrap());
        assert!(!flags(&b).1);
        assert!(b
            .query_row(
                "SELECT accepted_job IS NULL FROM local_sync_control",
                [],
                |r| r.get::<_, bool>(0)
            )
            .unwrap());
        assert_eq!(
            logical::frontier(&b)
                .unwrap()
                .get(&database::actor(&a).unwrap()),
            before.get(&database::actor(&a).unwrap())
        );
    }
    #[test]
    fn r1_completed_only_change_publishes_once_without_field_rewrite() {
        let (a, _, _, _) = pair();
        let first = storage::seal_state(&a, 0).unwrap().unwrap();
        storage::acknowledge(&a, 0, &first.manifest).unwrap();
        logical::drain_local(&a).unwrap();
        let second = storage::seal_state(&a, 0).unwrap().unwrap();
        assert_eq!(first.manifest.covered, second.manifest.covered);
        assert_eq!(first.manifest.max_lamport, second.manifest.max_lamport);
        assert_ne!(first.manifest.completed, second.manifest.completed);
        assert_eq!(first.manifest.shards, second.manifest.shards);
        assert_eq!(first.baseline, second.baseline);
        assert_ne!(
            first.manifest.file_name().unwrap(),
            second.manifest.file_name().unwrap()
        );
        storage::acknowledge(&a, 0, &second.manifest).unwrap();
        assert!(storage::seal_state(&a, 0).unwrap().is_none());
        assert!(storage::acknowledge(&a, 0, &first.manifest).is_err());
    }
    #[test]
    fn r3_unknown_pending_actor_is_durable_and_blocks_completed_proof() {
        let (a, _, _, _) = pair();
        logical::drain_local(&a).unwrap();
        let before = logical::revision(&a).unwrap();
        let complete = logical::completed(&a).unwrap();
        let actor = "e".repeat(32);
        let dependency = "d".repeat(32);
        let batch = Batch {
            format: 2,
            library_id: LIB.into(),
            actor: actor.clone(),
            intents: vec![Intent {
                seq: 1,
                lamport: 100,
                observed: std::collections::BTreeMap::from([(dependency.clone(), 1)]),
                patch: std::collections::BTreeMap::from([(
                    "p/themeMode".into(),
                    crate::webdav_transport::FieldValue::Set(json!("dark")),
                )]),
            }],
        };
        logical::ingest_batch(&a, 0, &batch).unwrap();
        assert!(logical::revision(&a).unwrap() > before);
        assert!(!logical::apply_ready_intent(&a).unwrap());
        assert_eq!(
            a.query_row(
                "SELECT COUNT(*) FROM local_sync_known_actors WHERE actor IN (?1,?2)",
                params![actor, dependency],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            2
        );
        logical::drain_local(&a).unwrap();
        assert_eq!(logical::completed(&a).unwrap(), complete);
    }
    #[test]
    fn t3_checkpoint_compensates_known_pending_patch_and_clock_like_replica() {
        let (_, b, _, _) = pair();
        let base = database::checkpoint(&b).unwrap();
        let remote = "e".repeat(32);
        let mut observed = base.covered.clone();
        observed.insert(remote.clone(), 1);
        let batch = Batch {
            format: 2,
            library_id: LIB.into(),
            actor: remote.clone(),
            intents: vec![Intent {
                seq: 2,
                lamport: base.max_lamport + 10,
                observed,
                patch: std::collections::BTreeMap::from([(
                    "p/themeMode".into(),
                    crate::webdav_transport::FieldValue::Set(json!("dark")),
                )]),
            }],
        };
        let mut replica = Replica::from_checkpoint(base.clone()).unwrap();
        replica.ingest_batch(&batch).unwrap();
        logical::ingest_batch(&b, 0, &batch).unwrap();
        assert!(!logical::apply_ready_intent(&b).unwrap());
        let mut sparse = base;
        sparse.covered.insert(remote, 2);
        replica.ingest_checkpoint(&sparse).unwrap();
        let job = stage(&b, &sparse);
        let checked = logical::preflight(&b, &job).unwrap();
        assert!(logical::accept(&b, 0, &job, checked).unwrap());
        assert_eq!(
            database::checkpoint(&b).unwrap(),
            replica.checkpoint().unwrap()
        );
        let accepted = database::checkpoint(&b).unwrap();
        logical::drain_local(&b).unwrap();
        assert_eq!(
            database::checkpoint(&b).unwrap(),
            accepted,
            "fold and retirement preserve L"
        );
        assert_eq!(
            b.query_row("SELECT COUNT(*) FROM local_sync_intents", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    #[test]
    fn t3_remote_conflict_after_preflight_never_accepts_partial_candidate() {
        let (_, b, _, _) = pair();
        let mut incoming = database::checkpoint(&b).unwrap();
        let remote = "e".repeat(32);
        let clock = incoming.max_lamport + 10;
        incoming.covered.insert(remote.clone(), 1);
        incoming.max_lamport = clock;
        incoming.fields.insert(
            "p/themeMode".into(),
            crate::webdav_transport::Register {
                stamp: crate::webdav_transport::Stamp {
                    actor: remote.clone(),
                    seq: 1,
                    lamport: clock,
                },
                value: crate::webdav_transport::FieldValue::Set(json!("dark")),
            },
        );
        let job = stage(&b, &incoming);
        let checked = logical::preflight(&b, &job).unwrap();
        let batch = Batch {
            format: 2,
            library_id: LIB.into(),
            actor: remote,
            intents: vec![Intent {
                seq: 1,
                lamport: clock,
                observed: Default::default(),
                patch: std::collections::BTreeMap::from([(
                    "p/themeMode".into(),
                    crate::webdav_transport::FieldValue::Set(json!("light")),
                )]),
            }],
        };
        logical::ingest_batch(&b, 0, &batch).unwrap();
        assert!(logical::apply_ready_intent(&b).unwrap());
        let accepted = database::checkpoint(&b).unwrap();
        assert!(!logical::accept(&b, 0, &job, checked).unwrap());
        assert!(logical::preflight(&b, &job).is_err());
        assert_eq!(database::checkpoint(&b).unwrap(), accepted);
    }
    #[test]
    fn t2_explicit_star_holds_present_body_while_projection_capture_is_suppressed() {
        let (a, b, aid, bid) = pair();
        db::set_read_with_enqueue(&a, aid, true).unwrap();
        db::set_starred_with_enqueue(&a, aid, true).unwrap();
        database::merge_apply(&b, 0, &database::checkpoint(&a).unwrap()).unwrap();
        database::apply_bodies(&b, 0, &database::protected_bodies(&a).unwrap()).unwrap();
        db::set_starred_with_enqueue(&a, aid, false).unwrap();
        let state = database::checkpoint(&a).unwrap();
        let job = stage(&b, &state);
        let checked = logical::preflight(&b, &job).unwrap();
        assert!(logical::accept(&b, 0, &job, checked).unwrap());
        assert!(flags(&b).1);
        db::set_starred(&b, bid, true).unwrap();
        assert!(b
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM local_sync_body_hold WHERE article_id=?1)",
                [bid],
                |r| r.get::<_, bool>(0)
            )
            .unwrap());
        let patch: String = b
            .query_row(
                "SELECT patch FROM local_sync_journal ORDER BY seq DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let patch: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(&patch).unwrap();
        assert_eq!(patch.len(), 1);
        assert!(patch.keys().all(|key| key.ends_with("/star")));
        logical::drain_local(&b).unwrap();
        assert!(flags(&b).1);
    }
    #[test]
    fn t2_settings_capture_only_named_key_and_reactivate_latest_logical_epoch() {
        let (a, b, _, _) = pair();
        crate::commands::apply_app_settings_patch(&a, &json!({"themeMode":"dark"})).unwrap();
        database::merge_apply(&b, 0, &database::checkpoint(&a).unwrap()).unwrap();
        crate::commands::apply_app_settings_patch(&a, &json!({"themeMode":"light","fontSize":18}))
            .unwrap();
        let afeed: i64 = a
            .query_row("SELECT id FROM feeds", [], |r| r.get(0))
            .unwrap();
        db::delete_feed(&a, afeed).unwrap();
        let incoming = database::checkpoint(&a).unwrap();
        let job = stage(&b, &incoming);
        let checked = logical::preflight(&b, &job).unwrap();
        assert!(logical::accept(&b, 0, &job, checked).unwrap());
        let source: String = b
            .query_row("SELECT sync_id FROM feeds", [], |r| r.get(0))
            .unwrap();
        let expected_epoch = logical::get(&b, &format!("s/{source}/deleted"))
            .unwrap()
            .unwrap()
            .stamp;
        assert!(b
            .query_row("SELECT active FROM feeds", [], |r| r.get::<_, bool>(0))
            .unwrap());
        let settings =
            crate::commands::apply_app_settings_patch(&b, &json!({"themeMode":"dark"})).unwrap();
        let settings: serde_json::Value = serde_json::from_str(&settings).unwrap();
        assert_eq!(settings["themeMode"], json!("dark"));
        assert_eq!(settings["fontSize"], json!(18));
        let patch: String = b
            .query_row(
                "SELECT patch FROM local_sync_journal ORDER BY seq DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let patch: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(&patch).unwrap();
        assert_eq!(patch.len(), 1);
        assert!(patch.contains_key("p/themeMode"));
        let folder: i64 = b
            .query_row("SELECT id FROM folders", [], |r| r.get(0))
            .unwrap();
        db::insert_feed(
            &b,
            "https://example.test/rss",
            None,
            "Feed",
            None,
            folder,
            "inherit",
            false,
            false,
        )
        .unwrap();
        assert_eq!(
            logical::get(&b, &format!("s/{source}/reactivate"))
                .unwrap()
                .unwrap()
                .value,
            crate::webdav_transport::FieldValue::Set(serde_json::to_value(expected_epoch).unwrap())
        );
        logical::drain_local(&b).unwrap();
        assert!(logical::source_active(&b, &source).unwrap());
    }
    #[test]
    fn t3_future_own_sequence_and_corrupt_final_page_are_rejected_before_acceptance() {
        let (_, b, _, _) = pair();
        let before = database::checkpoint(&b).unwrap();
        let mut incoming = before.clone();
        let own = database::actor(&b).unwrap();
        incoming.covered.insert(
            own.clone(),
            before.covered.get(&own).copied().unwrap_or(0) + 1,
        );
        let job = stage(&b, &incoming);
        assert!(logical::preflight(&b, &job).is_err());
        assert_eq!(database::checkpoint(&b).unwrap(), before);
        let mut corrupt = before.clone();
        let reg = corrupt.fields.values().next().unwrap().clone();
        corrupt.fields.insert("p/notAllowed".into(), reg);
        let bytes = serde_json::to_vec(&corrupt.fields).unwrap();
        let hash = model::digest(&bytes);
        let manifest = storage::Manifest {
            format: model::FORMAT,
            library_id: LIB.into(),
            actor: "e".repeat(32),
            publication: 2,
            kind: "state".into(),
            covered: before.covered.clone(),
            completed: Default::default(),
            max_lamport: before.max_lamport,
            deltas: 0,
            shards: vec![storage::ObjectRef {
                name: format!("{hash}-{}.bin", "f".repeat(32)),
                digest: hash,
                size: bytes.len(),
            }],
        };
        let job = logical::begin_receive(&b, 0, &manifest).unwrap();
        assert!(logical::stage_page(&b, 0, &job, 0, &corrupt.fields).is_err());
        assert!(logical::preflight(&b, &job).is_err());
        assert_eq!(database::checkpoint(&b).unwrap(), before);
    }
    #[test]
    fn r4_exact_ack_and_cleanup_preserve_candidate_validation_and_later_sequence_roots() {
        let (a, b, aid, bid) = pair();
        let first = storage::seal_state(&b, 0).unwrap().unwrap();
        storage::acknowledge(&b, 0, &first.manifest).unwrap();
        db::set_starred_with_enqueue(&a, aid, true).unwrap();
        let job = stage(&b, &database::checkpoint(&a).unwrap());
        let checked = logical::preflight(&b, &job).unwrap();
        assert!(logical::accept(&b, 0, &job, checked).unwrap());
        db::set_read_with_enqueue(&b, bid, true).unwrap();
        let sealed = storage::seal_state(&b, 0).unwrap().unwrap();
        storage::acknowledge(&b, 0, &sealed.manifest).unwrap();
        let candidate_count: i64 = b
            .query_row(
                "SELECT COUNT(*) FROM local_sync_candidates WHERE job=?1",
                [&job],
                |r| r.get(0),
            )
            .unwrap();
        assert!(candidate_count > 0);
        assert!(!storage::cleanup_slice(&b).unwrap());
        assert_eq!(
            b.query_row(
                "SELECT COUNT(*) FROM local_sync_candidates WHERE job=?1",
                [&job],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            candidate_count
        );
        logical::drain_local(&b).unwrap();
        db::set_read_with_enqueue(&b, bid, false).unwrap();
        let later: u64 = b
            .query_row("SELECT seq FROM local_sync_control", [], |r| r.get(0))
            .unwrap();
        b.execute("INSERT INTO local_sync_roots(owner,kind,reference) VALUES('test-tail','validation',?1)",[&job]).unwrap();
        let journal: i64 = b
            .query_row("SELECT COUNT(*) FROM local_sync_journal", [], |r| r.get(0))
            .unwrap();
        assert!(!storage::cleanup_slice(&b).unwrap());
        assert_eq!(
            b.query_row("SELECT COUNT(*) FROM local_sync_journal", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            journal
        );
        b.execute("DELETE FROM local_sync_roots WHERE owner='test-tail'", [])
            .unwrap();
        for _ in 0..32 {
            if !storage::cleanup_slice(&b).unwrap() {
                break;
            }
        }
        assert!(b
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM local_sync_journal WHERE seq=?1)",
                [later],
                |r| r.get::<_, bool>(0)
            )
            .unwrap());
        let next = storage::seal_state(&b, 0).unwrap().unwrap();
        assert!(storage::acknowledge(&b, 0, &first.manifest).is_err());
        assert!(storage::acknowledge(&b, 0, &sealed.manifest).is_err());
        assert_eq!(
            storage::pending(&b, "state", 0)
                .unwrap()
                .unwrap()
                .manifest
                .file_name()
                .unwrap(),
            next.manifest.file_name().unwrap()
        );
    }
    #[test]
    fn r4_fast_storage_is_not_checkpoint_ack_and_gc_needs_patch_domination() {
        let (a, _, id, _) = pair();
        db::set_read_with_enqueue(&a, id, true).unwrap();
        let out = fast::prepare(&a, 0).unwrap().unwrap();
        fast::acknowledge(&a, 0, &out).unwrap();
        assert_eq!(
            a.query_row("SELECT published_seq FROM local_sync_control", [], |r| r
                .get::<_, u64>(0))
                .unwrap(),
            0
        );
        logical::drain_local(&a).unwrap();
        storage::cleanup_slice(&a).unwrap();
        assert_eq!(
            a.query_row("SELECT COUNT(*) FROM local_sync_fast_objects", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        let publication = storage::seal_state(&a, 0).unwrap().unwrap();
        storage::acknowledge(&a, 0, &publication.manifest).unwrap();
        for _ in 0..32 {
            if !storage::cleanup_slice(&a).unwrap() {
                break;
            }
        }
        assert_eq!(
            a.query_row("SELECT COUNT(*) FROM local_sync_fast_objects", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert!(a
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM local_sync_gc WHERE path=?1)",
                [out.path],
                |r| r.get::<_, bool>(0)
            )
            .unwrap());
    }
    #[test]
    fn t5_disconnect_disk_reopen_local_recovery_and_portable_include_accepted_favorite() {
        let a = conn();
        let (_, aid) = seed(&a, &"a".repeat(32), "https://example.test/rss");
        database::initialize(&a, LIB).unwrap();
        let file = path("accepted-restart").with_extension("db");
        let b = db::open(&file).unwrap();
        database::initialize(&b, LIB).unwrap();
        database::merge_apply(&b, 0, &database::checkpoint(&a).unwrap()).unwrap();
        db::set_starred_with_enqueue(&a, aid, true).unwrap();
        let job = stage(&b, &database::checkpoint(&a).unwrap());
        let checked = logical::preflight(&b, &job).unwrap();
        assert!(logical::accept(&b, 0, &job, checked).unwrap());
        crate::local_sync::runtime::disconnect(&b).unwrap();
        drop(b);
        // No Transport exists in this test: all accepted bytes are local.
        let b = db::open(&file).unwrap();
        assert!(!crate::local_sync::runtime::configured(&b).unwrap());
        logical::drain_local(&b).unwrap();
        assert!(flags(&b).1);
        let backup_path = path("accepted-backup");
        let info = backup::export(&b, &backup_path).unwrap();
        assert_eq!(backup::preview(&backup_path).unwrap().starred, 1);
        let restored = conn();
        let actor = database::actor(&restored).unwrap();
        backup::restore(&restored, &backup_path, &info.digest).unwrap();
        assert!(flags(&restored).1);
        assert_ne!(actor, database::actor(&restored).unwrap());
        drop(b);
        std::fs::remove_file(file).unwrap();
        std::fs::remove_file(backup_path).unwrap();
    }
    #[test]
    fn t4_alias_split_keeps_wire_owner_guid_and_row_id_after_cursor_reset() {
        let a = conn();
        let b = conn();
        let (_, aid) = seed(&a, &"f".repeat(32), "https://example.test/rss");
        let (bfeed, bid) = seed(&b, &"a".repeat(32), "https://example.test/rss");
        b.execute("UPDATE articles SET guid='only-second' WHERE id=?1", [bid])
            .unwrap();
        database::initialize(&a, LIB).unwrap();
        database::initialize(&b, LIB).unwrap();
        exchange(&a, &b);
        let second: i64 = a
            .query_row(
                "SELECT id FROM articles WHERE guid='only-second'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            a.query_row(
                "SELECT local_sync_source FROM articles WHERE id=?1",
                [second],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "a".repeat(32)
        );
        // A prior source scope has already walked beyond the new lower key.
        a.execute("INSERT INTO local_sync_work(job,key,revision,cursor) VALUES('','source/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',0,'a/ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff') ON CONFLICT(job,key) DO UPDATE SET cursor=excluded.cursor",[]).unwrap();
        b.execute(
            "UPDATE feeds SET feed_url='https://example.test/second' WHERE id=?1",
            [bfeed],
        )
        .unwrap();
        db::set_starred_with_enqueue(&b, bid, true).unwrap();
        let job = stage(&a, &database::checkpoint(&b).unwrap());
        let checked = logical::preflight(&a, &job).unwrap();
        assert!(logical::accept(&a, 0, &job, checked).unwrap());
        logical::drain_local(&a).unwrap();
        assert_eq!(a.query_row("SELECT a.id FROM articles a JOIN feeds f ON f.id=a.feed_id WHERE a.guid='only-second' AND f.feed_url='https://example.test/second'",[],|r|r.get::<_,i64>(0)).unwrap(),second);
        assert_eq!(
            a.query_row("SELECT guid FROM articles WHERE id=?1", [aid], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "real-guid"
        );
        assert!(a
            .query_row(
                "SELECT is_starred FROM articles WHERE id=?1",
                [second],
                |r| r.get::<_, bool>(0)
            )
            .unwrap());
        assert!(logical::settled(&a).unwrap());
        assert_eq!(
            a.query_row("SELECT COUNT(*) FROM articles", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        let before = database::checkpoint(&a).unwrap();
        a.execute(
            "UPDATE articles SET local_sync_source=NULL WHERE id=?1",
            [second],
        )
        .unwrap();
        assert!(db::set_read_with_enqueue(&a, second, true).is_err());
        assert_eq!(
            database::checkpoint(&a).unwrap(),
            before,
            "missing locator must not invent a feed-owned wire member"
        );
        a.execute(
            "UPDATE articles SET local_sync_source=?1 WHERE id=?2",
            params!["a".repeat(32), second],
        )
        .unwrap();
        let file = path("alias-split-backup");
        let info = backup::export(&a, &file).unwrap();
        let restored = conn();
        backup::restore(&restored, &file, &info.digest).unwrap();
        assert_eq!(
            restored
                .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert!(restored.query_row("SELECT a.is_starred FROM articles a JOIN feeds f ON f.id=a.feed_id WHERE a.guid='only-second' AND f.feed_url='https://example.test/second'",[],|r|r.get::<_,bool>(0)).unwrap());
        std::fs::remove_file(file).unwrap();
    }
}

/// A1–A5: wire identities and their portable form survive physical folding.
mod identity_portable_tests {
    use super::*;
    use crate::local_sync::{logical, model};
    use crate::webdav_transport::{Batch, FieldPatch, FieldValue, Intent, Register, Stamp};
    const LEFT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const RIGHT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    fn fixture() -> (Connection, i64, i64) {
        let c = conn();
        let (_, left) = seed(&c, LEFT, "https://example.test/left");
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
        let right = db::upsert_article_with_feed(&c, feed, &a, false).unwrap().0;
        db::set_starred(&c, left, true).unwrap();
        db::set_starred(&c, right, true).unwrap();
        database::initialize(&c, LIB).unwrap();
        logical::drain_local(&c).unwrap();
        (c, left, right)
    }
    fn url(c: &Connection, source: &str, url: &str) {
        logical::capture_patch(
            c,
            &FieldPatch::from([(format!("s/{source}/feed_url"), FieldValue::Set(json!(url)))]),
        )
        .unwrap();
        logical::drain_local(c).unwrap();
    }
    fn flag(c: &Connection, source: &str, name: &str) -> Register {
        logical::get(
            c,
            &format!("a/{}/{name}", model::entry_key(source, "real-guid")),
        )
        .unwrap()
        .unwrap()
    }
    #[test]
    fn a1_same_guid_flags_cover_wire_members_merge_split_and_rollback() {
        let (c, left, _) = fixture();
        let before = database::checkpoint(&c).unwrap();
        url(&c, RIGHT, "https://example.test/left");
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM feeds", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM articles", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            c.query_row("SELECT id FROM articles", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            left
        );
        assert_eq!(
            c.query_row(
                "SELECT COUNT(DISTINCT body_ref) FROM local_sync_body_hold",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            2
        );
        assert_eq!(
            c.query_row(
                "SELECT COUNT(*) FROM contents WHERE COALESCE(content_html,'')!=''",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            2
        );
        let star_before = [flag(&c, LEFT, "star"), flag(&c, RIGHT, "star")];
        db::set_read_with_enqueue(&c, left, true).unwrap();
        assert_eq!(flag(&c, LEFT, "read").value, FieldValue::Set(json!(true)));
        assert_eq!(flag(&c, RIGHT, "read").value, FieldValue::Set(json!(true)));
        assert_eq!(flag(&c, LEFT, "star"), star_before[0]);
        assert_eq!(flag(&c, RIGHT, "star"), star_before[1]);
        for source in [LEFT, RIGHT] {
            let meta = format!("a/{}/meta", model::entry_key(source, "real-guid"));
            assert_eq!(
                logical::get(&c, &meta).unwrap().unwrap(),
                before.fields[&meta]
            );
        }
        let snapshot = database::checkpoint(&c).unwrap();
        c.execute_batch("CREATE TRIGGER fail_wire_flag BEFORE INSERT ON sync_queue BEGIN SELECT RAISE(ABORT,'injected queue failure'); END;").unwrap();
        assert!(db::set_starred_with_enqueue(&c, left, false).is_err());
        assert_eq!(database::checkpoint(&c).unwrap(), snapshot);
        c.execute_batch("DROP TRIGGER fail_wire_flag").unwrap();
        db::set_starred_with_enqueue(&c, left, true).unwrap();
        assert_eq!(flag(&c, LEFT, "star").stamp, flag(&c, RIGHT, "star").stamp);
        url(&c, RIGHT, "https://example.test/right");
        assert_eq!(
            c.query_row(
                "SELECT COUNT(*) FROM articles WHERE is_read=1 AND is_starred=1",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            2
        );
        assert_eq!(
            c.query_row(
                "SELECT id FROM articles WHERE local_sync_source=?1",
                [LEFT],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            left
        );
        url(&c, RIGHT, "https://example.test/left");
        url(&c, RIGHT, "https://example.test/right");
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM articles", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            c.query_row(
                "SELECT COUNT(*) FROM contents WHERE COALESCE(content_html,'')!=''",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            2
        );
    }
    #[test]
    fn a4_empty_fill_rebuild_has_valid_locator_unknown_locator_preserves_bytes() {
        let (c, left, _) = fixture();
        let meta = logical::get(
            &c,
            &format!("a/{}/meta", model::entry_key(LEFT, "real-guid")),
        )
        .unwrap()
        .unwrap();
        let (published, first_seen): (String, String) = c
            .query_row(
                "SELECT published_at,first_seen_at FROM articles WHERE id=?1",
                [left],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        c.execute("DELETE FROM articles WHERE id=?1", [left])
            .unwrap();
        let feed: i64 = c
            .query_row("SELECT id FROM feeds WHERE sync_id=?1", [LEFT], |r| {
                r.get(0)
            })
            .unwrap();
        let seq: i64 = c
            .query_row("SELECT seq FROM local_sync_control", [], |r| r.get(0))
            .unwrap();
        // Like production upsert, supply normalized dates: fallback triggers
        // perform real metadata updates, which are not an empty-fill insertion.
        c.execute("INSERT INTO articles(feed_id,guid,title,is_starred,published_at,first_seen_at) VALUES(?1,'real-guid','physical rebuild',1,?2,?3)",params![feed,published,first_seen]).unwrap();
        let id = c.last_insert_rowid();
        assert_eq!(
            c.query_row(
                "SELECT local_sync_source FROM articles WHERE id=?1",
                [id],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            LEFT
        );
        assert_eq!(
            c.query_row("SELECT seq FROM local_sync_control", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            seq
        );
        assert_eq!(
            logical::get(
                &c,
                &format!("a/{}/meta", model::entry_key(LEFT, "real-guid"))
            )
            .unwrap()
            .unwrap(),
            meta
        );
        db::set_read_with_enqueue(&c, id, true).unwrap();
        c.execute(
            "UPDATE articles SET local_sync_source=?1 WHERE id=?2",
            params!["c".repeat(32), id],
        )
        .unwrap();
        assert!(db::set_read_with_enqueue(&c, id, false).is_err());
        assert!(!c
            .query_row(
                "SELECT identity_valid FROM local_sync_article_state WHERE id=?1",
                [id],
                |r| r.get::<_, bool>(0)
            )
            .unwrap());
        assert_eq!(
            c.query_row(
                "SELECT is_read FROM local_sync_article_state WHERE id=?1",
                [id],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0,
            "unknown locator cannot authorize cleanup"
        );
    }
    #[test]
    fn a5_fast_url_merge_selects_cross_wire_ties_with_candidate() {
        for accepted in [false, true] {
            let (c, _, _) = fixture();
            let mut state = database::checkpoint(&c).unwrap();
            let remote = "e".repeat(32);
            let stamp = Stamp {
                actor: remote.clone(),
                seq: 1,
                lamport: 1000,
            };
            for source in [LEFT, RIGHT] {
                state
                    .fields
                    .get_mut(&format!("a/{}/meta", model::entry_key(source, "real-guid")))
                    .unwrap()
                    .stamp = stamp.clone();
            }
            state.covered.insert(remote, 1);
            state.max_lamport = 1000;
            if accepted {
                let job = accepted_candidate_tests::stage(&c, &state);
                let r = logical::preflight(&c, &job).unwrap();
                assert!(logical::accept(&c, 0, &job, r).unwrap());
            } else {
                database::merge_apply(&c, 0, &state).unwrap();
            }
            let before = database::checkpoint(&c).unwrap();
            let rows: i64 = c
                .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
                .unwrap();
            let batch = Batch {
                format: 2,
                library_id: LIB.into(),
                actor: "d".repeat(32),
                intents: vec![Intent {
                    seq: 1,
                    lamport: 2000,
                    observed: before.covered.clone(),
                    patch: FieldPatch::from([(
                        format!("s/{RIGHT}/feed_url"),
                        FieldValue::Set(json!("https://example.test/left")),
                    )]),
                }],
            };
            // Different original keys may legitimately carry one intent's same
            // stamp and different values; display folding has a total tie order.
            logical::ingest_batch(&c, 0, &batch).unwrap();
            assert_eq!(database::checkpoint(&c).unwrap(), before);
            assert_eq!(
                c.query_row("SELECT COUNT(*) FROM articles", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                rows
            );
            assert!(logical::apply_ready_intent(&c).unwrap());
            logical::drain_local(&c).unwrap();
            assert_eq!(
                c.query_row("SELECT COUNT(*) FROM articles", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                1
            );
            let display =
                logical::projected(&c, &format!("a/{}", model::entry_key(LEFT, "real-guid")))
                    .unwrap();
            let winner = [LEFT, RIGHT]
                .into_iter()
                .max_by_key(|s| model::entry_key(s, "real-guid"))
                .unwrap();
            let expected = logical::get(
                &c,
                &format!("a/{}/meta", model::entry_key(winner, "real-guid")),
            )
            .unwrap()
            .unwrap();
            assert_eq!(
                display
                    .values()
                    .find_map(|r| match &r.value {
                        FieldValue::Set(v) if v.get("guid").is_some() => Some(&v["title"]),
                        _ => None,
                    })
                    .unwrap(),
                match &expected.value {
                    FieldValue::Set(v) => &v["title"],
                    _ => unreachable!(),
                }
            );
            assert_eq!(
                c.query_row(
                    "SELECT COUNT(*) FROM local_sync_intents WHERE actor=?1",
                    ["d".repeat(32)],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                0
            );
        }
    }
    #[test]
    fn a3_portable_merged_aliases_preserve_field_priority_and_split_after_restore() {
        let (c, left, _) = fixture();
        logical::capture_patch(
            &c,
            &FieldPatch::from([(
                format!("a/{}/read", model::entry_key(LEFT, "real-guid")),
                FieldValue::Set(json!(true)),
            )]),
        )
        .unwrap();
        logical::capture_patch(
            &c,
            &FieldPatch::from([(
                format!("a/{}/star", model::entry_key(RIGHT, "real-guid")),
                FieldValue::Set(json!(false)),
            )]),
        )
        .unwrap();
        url(&c, RIGHT, "https://example.test/left");
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM articles", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        let file = path("merged-wire-backup");
        let info = backup::export(&c, &file).unwrap();
        let preview = backup::preview(&file).unwrap();
        assert_eq!(preview.sources, 2);
        assert_eq!(preview.articles, 2);
        let restored = conn();
        let (_, old) = seed(&restored, &"c".repeat(32), "https://example.test/previous");
        let actor = database::actor(&restored).unwrap();
        restored.execute_batch("CREATE TRIGGER fail_alias_restore BEFORE INSERT ON feeds BEGIN SELECT RAISE(ABORT,'injected restore failure'); END;").unwrap();
        assert!(backup::restore(&restored, &file, &info.digest).is_err());
        assert_eq!(database::actor(&restored).unwrap(), actor);
        restored
            .execute_batch("DROP TRIGGER fail_alias_restore")
            .unwrap();
        backup::restore(&restored, &file, &info.digest).unwrap();
        assert_ne!(database::actor(&restored).unwrap(), actor);
        assert_eq!(flags(&restored), (true, false));
        assert!(
            restored
                .query_row("SELECT MIN(id) FROM articles", [], |r| r.get::<_, i64>(0))
                .unwrap()
                > old
        );
        assert_eq!(
            flag(&restored, LEFT, "read").value,
            flag(&c, LEFT, "read").value
        );
        assert_eq!(
            flag(&restored, RIGHT, "read").value,
            flag(&c, RIGHT, "read").value
        );
        assert_eq!(
            flag(&restored, LEFT, "star").value,
            flag(&c, LEFT, "star").value
        );
        assert_eq!(
            flag(&restored, RIGHT, "star").value,
            flag(&c, RIGHT, "star").value
        );
        url(&restored, RIGHT, "https://example.test/right");
        assert_eq!(
            restored
                .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            restored
                .query_row(
                    "SELECT COUNT(*) FROM contents WHERE COALESCE(content_html,'')!=''",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            2
        );
        assert!(db::get_setting(&restored, "local_sync_password")
            .unwrap()
            .is_none());
        std::fs::remove_file(file).unwrap();
        let _ = left;
    }
}

#[test]
fn two_databases_offline_fields_converge_without_echo_and_causal_successor_wins() {
    let a = conn();
    let b = conn();
    let (_, aid) = seed(&a, &"a".repeat(32), "https://example.test/rss");
    database::initialize(&a, LIB).unwrap();
    database::initialize(&b, LIB).unwrap();
    exchange(&a, &b);
    let bid = b
        .query_row("SELECT id FROM articles", [], |r| r.get::<_, i64>(0))
        .unwrap();
    db::set_read(&a, aid, true).unwrap();
    db::set_starred(&b, bid, true).unwrap();
    let count = |c: &Connection| {
        c.query_row("SELECT COUNT(*) FROM local_sync_journal", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap()
    };
    let before = (count(&a), count(&b));
    exchange(&a, &b);
    assert_eq!(flags(&a), (true, true));
    assert_eq!(flags(&b), (true, true));
    assert_eq!((count(&a), count(&b)), before);
    db::set_read(&b, bid, false).unwrap();
    exchange(&a, &b);
    assert_eq!(flags(&a), (false, true));
    assert_eq!(flags(&b), (false, true));
    let body = database::protected_bodies(&a).unwrap();
    database::apply_bodies(&b, 0, &body).unwrap();
    assert_eq!(database::pending_bodies(&b).unwrap(), 0);
}

#[test]
fn exact_url_alias_smaller_late_identity_preserves_original_wire_ids_and_pending() {
    let a = conn();
    let b = conn();
    let (_, aid) = seed(&a, &"f".repeat(32), "https://example.test/rss");
    let (_, bid) = seed(&b, &"a".repeat(32), "https://example.test/rss");
    database::initialize(&a, LIB).unwrap();
    database::initialize(&b, LIB).unwrap();
    db::set_read(&a, aid, true).unwrap();
    db::set_starred(&b, bid, true).unwrap();
    let pending = storage::seal_state(&a, 0).unwrap().unwrap();
    let before = database::checkpoint(&a).unwrap();
    exchange(&a, &b);
    assert_eq!(flags(&a), (true, true));
    assert_eq!(flags(&b), (true, true));
    assert_eq!(
        a.query_row("SELECT sync_id FROM feeds", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "f".repeat(32)
    );
    let after = database::checkpoint(&a).unwrap();
    for (key, reg) in before.fields {
        assert!(after.fields.get(&key).is_some_and(|r| r.stamp >= reg.stamp));
    }
    assert_eq!(
        storage::pending(&a, "state", 0)
            .unwrap()
            .unwrap()
            .manifest
            .file_name()
            .unwrap(),
        pending.manifest.file_name().unwrap()
    );
    db::set_read(&a, aid, false).unwrap();
    exchange(&a, &b);
    assert_eq!(flags(&b), (false, true));
}

#[test]
fn delete_epoch_survives_old_snapshot_and_only_observed_reactivation_restores() {
    let a = conn();
    let b = conn();
    let (feed, article) = seed(&a, &"a".repeat(32), "https://example.test/rss");
    db::set_starred(&a, article, true).unwrap();
    database::initialize(&a, LIB).unwrap();
    database::initialize(&b, LIB).unwrap();
    exchange(&a, &b);
    let old = database::checkpoint(&b).unwrap();
    db::delete_feed(&a, feed).unwrap();
    exchange(&a, &b);
    database::merge_apply(&a, 0, &old).unwrap();
    assert!(!a
        .query_row("SELECT active FROM feeds", [], |r| r.get::<_, bool>(0))
        .unwrap());
    assert_eq!(flags(&a), (false, true));
    let folder = db::list_folders(&b).unwrap()[0].id;
    db::insert_feed(
        &b,
        "https://example.test/rss",
        None,
        "Feed",
        None,
        folder,
        "inherit",
        false,
        false,
    )
    .unwrap();
    exchange(&a, &b);
    assert!(a
        .query_row("SELECT active FROM feeds", [], |r| r.get::<_, bool>(0))
        .unwrap());
}

#[test]
fn settings_whitelist_keeps_independent_keys_and_ignores_machine_secrets() {
    let a = conn();
    let b = conn();
    db::set_setting(
        &a,
        "app_settings",
        r#"{"fontSize":16,"themeMode":"light","articleAutoSummary":false,"autoStart":true}"#,
    )
    .unwrap();
    database::initialize(&a, LIB).unwrap();
    database::initialize(&b, LIB).unwrap();
    exchange(&a, &b);
    db::set_setting(
        &a,
        "app_settings",
        r#"{"fontSize":18,"themeMode":"light","articleAutoSummary":false,"autoStart":true}"#,
    )
    .unwrap();
    db::set_setting(
        &b,
        "app_settings",
        r#"{"fontSize":16,"themeMode":"dark","articleAutoSummary":true}"#,
    )
    .unwrap();
    exchange(&a, &b);
    for c in [&a, &b] {
        let raw = db::get_setting(c, "app_settings").unwrap().unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["fontSize"], 18);
        assert_eq!(v["themeMode"], "dark");
        assert_eq!(v["articleAutoSummary"], true);
        assert!(!database::checkpoint(c)
            .unwrap()
            .fields
            .contains_key("p/autoStart"));
    }
    let before = database::checkpoint(&a).unwrap();
    let raw = db::get_setting(&a, "app_settings").unwrap().unwrap();
    db::set_setting(&a, "app_settings", &raw).unwrap();
    assert_eq!(before, database::checkpoint(&a).unwrap());
}

#[test]
fn local_scope_remote_creation_local_membership_and_purge_preserve_local_objects() {
    let mut c = conn();
    let (local, article) = seed(&c, &"a".repeat(32), "https://local.test/rss");
    c.execute("UPDATE folders SET remote_id=99", []).unwrap();
    c.execute("UPDATE feeds SET remote_id=88 WHERE id=?1", [local])
        .unwrap();
    let remote_group = db::create_remote_folder(&c, "Remote labels", "article").unwrap();
    let remote = db::insert_feed_origin(
        &c,
        "https://remote.test/rss",
        None,
        "Remote",
        None,
        remote_group,
        "inherit",
        false,
        false,
        "remote",
    )
    .unwrap();
    c.execute("INSERT INTO articles(feed_id,guid,title,is_starred,content_id,rss_content_id) SELECT ?1,'remote-favorite','Favorite',1,content_id,rss_content_id FROM articles WHERE id=?2",params![remote,article]).unwrap();
    let remote_source: String = c
        .query_row("SELECT sync_id FROM feeds WHERE id=?1", [remote], |r| {
            r.get(0)
        })
        .unwrap();
    let remote_uid: String = c
        .query_row(
            "SELECT sync_id FROM folders WHERE id=?1",
            [remote_group],
            |r| r.get(0),
        )
        .unwrap();
    database::initialize(&c, LIB).unwrap();
    let initial = database::checkpoint(&c).unwrap();
    assert!(!initial
        .fields
        .contains_key(&format!("s/{remote_source}/created")));
    assert!(!initial
        .fields
        .contains_key(&format!("g/{remote_uid}/created")));
    assert!(initial
        .fields
        .contains_key(&format!("s/{}/created", "a".repeat(32))));
    let later = db::create_remote_folder(&c, "Remote after init", "article").unwrap();
    let later_uid: String = c
        .query_row("SELECT sync_id FROM folders WHERE id=?1", [later], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(
        !database::checkpoint(&c)
            .unwrap()
            .fields
            .contains_key(&format!("g/{later_uid}/created")),
        "remote INSERT never briefly enters local journal"
    );
    let before = database::checkpoint(&c).unwrap();
    {
        let tx = c.unchecked_transaction().unwrap();
        tx.execute(
            "UPDATE feeds SET folder_id=?1 WHERE id=?2",
            params![later, local],
        )
        .unwrap();
        assert_eq!(
            tx.query_row("SELECT origin FROM folders WHERE id=?1", [later], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
            "local"
        );
        tx.rollback().unwrap();
    }
    assert_eq!(database::checkpoint(&c).unwrap(), before);
    assert_eq!(
        c.query_row("SELECT origin FROM folders WHERE id=?1", [later], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "remote"
    );
    c.execute(
        "UPDATE feeds SET folder_id=?1 WHERE id=?2",
        params![later, local],
    )
    .unwrap();
    assert!(database::checkpoint(&c)
        .unwrap()
        .fields
        .contains_key(&format!("g/{later_uid}/created")));
    let backup_file = path("all-favorites");
    let backup = backup::export(&c, &backup_file).unwrap();
    assert_eq!(
        backup.starred, 1,
        "portable backup includes RSS favorites independent of WebDAV scope"
    );
    std::fs::remove_file(backup_file).unwrap();
    let local_state = database::checkpoint(&c).unwrap();
    db::purge_remote_data(&mut c).unwrap();
    database::initialize(&c, LIB).unwrap();
    database::merge_apply(&c, 0, &local_state).unwrap();
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM feeds WHERE feed_url='https://remote.test/rss'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM folders WHERE id=?1",
            [remote_group],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM folders WHERE id=?1", [later], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        1
    );
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM folders WHERE id=1", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1,
        "local category with remote binding survives purge even when empty"
    );
    assert!(c
        .execute(
            "INSERT INTO folders(name,origin) VALUES('invalid','other')",
            []
        )
        .is_err());
}

#[test]
fn local_scope_physical_delete_tombstone_defeats_old_snapshot_and_local_history_remains() {
    let mut c = conn();
    let (feed, article) = seed(&c, &"a".repeat(32), "https://local.test/rss");
    database::initialize(&c, LIB).unwrap();
    let old = database::checkpoint(&c).unwrap();
    c.execute("UPDATE feeds SET origin='remote' WHERE id=?1", [feed])
        .unwrap();
    assert_eq!(
        c.query_row(
            "SELECT origin_was_local FROM feeds WHERE id=?1",
            [feed],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    db::set_starred(&c, article, true).unwrap();
    db::purge_remote_data(&mut c).unwrap();
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM feeds WHERE id=?1", [feed], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    c.execute("DELETE FROM feeds WHERE id=?1", [feed]).unwrap();
    let deleted = database::checkpoint(&c).unwrap();
    assert!(!super::model::active_source(
        &deleted.fields,
        &format!("s/{}", "a".repeat(32))
    ));
    database::merge_apply(&c, 0, &old).unwrap();
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM feeds WHERE active=1", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert!(!super::model::active_source(
        &database::checkpoint(&c).unwrap().fields,
        &format!("s/{}", "a".repeat(32))
    ));
}

#[test]
fn local_scope_remote_default_reuses_local_group_and_adoption_captures_existing_entries() {
    let c = conn();
    let local_default = db::ensure_uncategorized_folder(&c).unwrap();
    database::initialize(&c, LIB).unwrap();
    assert_eq!(
        db::ensure_remote_uncategorized_folder(&c).unwrap(),
        local_default
    );
    assert_eq!(
        c.query_row(
            "SELECT origin FROM folders WHERE id=?1",
            [local_default],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "local"
    );
    let group = db::create_remote_folder(&c, "Remote", "article").unwrap();
    let feed = db::insert_feed_origin(
        &c,
        "https://adopt.test/rss",
        None,
        "Remote",
        None,
        group,
        "inherit",
        false,
        false,
        "remote",
    )
    .unwrap();
    c.execute(
        "INSERT INTO articles(feed_id,guid,title,is_starred) VALUES(?1,'existing','Existing',1)",
        [feed],
    )
    .unwrap();
    c.execute("UPDATE feeds SET origin='local' WHERE id=?1", [feed])
        .unwrap();
    let state = database::checkpoint(&c).unwrap();
    let source: String = c
        .query_row("SELECT sync_id FROM feeds WHERE id=?1", [feed], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(state.fields.contains_key(&format!("s/{source}/created")));
    assert!(state.fields.contains_key(&format!(
        "a/{}/meta",
        super::model::entry_key(&source, "existing")
    )));
    assert_eq!(
        c.query_row("SELECT origin FROM folders WHERE id=?1", [group], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "local"
    );
}

fn path(name: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tmp")
        .join("portable-tests");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(format!(
        "{name}-{}.flxb",
        storage::random_id(&conn()).unwrap()
    ))
}
#[test]
fn portable_restore_keeps_starred_retired_body_new_actor_and_rejects_corruption_atomically() {
    let a = conn();
    let b = conn();
    let (feed, article) = seed(&a, &"a".repeat(32), "https://example.test/rss");
    db::set_starred(&a, article, true).unwrap();
    db::set_read(&a, article, true).unwrap();
    db::delete_feed(&a, feed).unwrap();
    db::set_setting(&a, "ai_config", r#"{"api_key":"secret-should-not-export"}"#).unwrap();
    db::set_setting(&a, "local_sync_password", "dav-secret-should-not-export").unwrap();
    database::initialize(&a, LIB).unwrap();
    let file = path("restore");
    let info = backup::export(&a, &file).unwrap();
    let bytes = std::fs::read(&file).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(!text.contains("secret-should-not-export"));
    assert!(!text.contains(&database::actor(&a).unwrap()));
    let prior = database::actor(&b).unwrap();
    let preview = backup::preview(&file).unwrap();
    assert_eq!(preview.starred, 1);
    assert_eq!(preview.missing_bodies, 0);
    backup::restore(&b, &file, &info.digest).unwrap();
    assert_ne!(database::actor(&b).unwrap(), prior);
    assert_ne!(database::actor(&b).unwrap(), database::actor(&a).unwrap());
    assert_eq!(flags(&b), (true, true));
    assert_eq!(database::pending_bodies(&b).unwrap(), 0);
    assert!(!b
        .query_row("SELECT active FROM feeds", [], |r| r.get::<_, bool>(0))
        .unwrap());
    let mut bad = bytes;
    let last = bad.len() - 1;
    bad[last] ^= 1;
    let corrupted = path("corrupt");
    std::fs::write(&corrupted, bad).unwrap();
    assert!(backup::restore(&b, &corrupted, &info.digest).is_err());
    assert_eq!(flags(&b), (true, true));
    b.execute_batch("CREATE TRIGGER reject_restore BEFORE INSERT ON feeds BEGIN SELECT RAISE(ABORT,'injected restore failure'); END;").unwrap();
    assert!(backup::restore(&b, &file, &info.digest).is_err());
    assert_eq!(flags(&b), (true, true));
    assert!(!b
        .query_row("SELECT applying FROM local_sync_control", [], |r| r
            .get::<_, bool>(0))
        .unwrap());
    std::fs::remove_file(file).unwrap();
    std::fs::remove_file(corrupted).unwrap();
}

#[test]
fn portable_v4_deduplicates_content_and_restores_preferences_with_fresh_ids() {
    let a = conn();
    let b = conn();
    let (feed, article) = seed(&a, &"a".repeat(32), "https://example.test/rss");
    a.execute(
        "UPDATE feeds SET fetched_title='Fetched',title_custom='Custom',title='Custom'",
        [],
    )
    .unwrap();
    a.execute("UPDATE articles SET rss_content_id=content_id", [])
        .unwrap();
    a.execute("INSERT INTO articles(feed_id,guid,title,content_id,rss_content_id) SELECT feed_id,'second-guid','Second',content_id,content_id FROM articles WHERE id=?1", [article]).unwrap();
    db::set_setting(
        &a,
        "app_settings",
        r#"{"syncMode":"hybrid","articleAutoSummary":true,"fontSize":18,"autoStart":true}"#,
    )
    .unwrap();
    seed(&b, &"b".repeat(32), "https://old.test/rss");
    b.execute("UPDATE articles SET id=1000", []).unwrap();
    b.execute(
        "INSERT INTO folders(id,name) VALUES(100,'removed group')",
        [],
    )
    .unwrap();
    b.execute("INSERT INTO feeds(id,feed_url,title,folder_id) VALUES(200,'https://removed.test/rss','removed source',100)",[]).unwrap();
    b.execute(
        "INSERT INTO articles(id,feed_id,guid,title) VALUES(3000,200,'removed-highest','Removed')",
        [],
    )
    .unwrap();
    b.execute("DELETE FROM folders WHERE id=100", []).unwrap();
    db::set_setting(
        &b,
        "app_settings",
        r#"{"syncMode":"direct","fontSize":14,"autoStart":false}"#,
    )
    .unwrap();
    db::set_setting(&b, "greader_endpoint", "https://old.test").unwrap();
    db::set_setting(&b, "greader_password", "not-portable").unwrap();
    db::set_setting(&b, "ai_config", "not-portable").unwrap();
    db::set_setting(&b, "local_sync_config", r#"{"endpoint":"https://old.test","username":"old","mode":"scheduled","interval_minutes":15}"#).unwrap();
    let file = path("v4-dedup");
    let exported = backup::export(&a, &file).unwrap();
    let bytes = std::fs::read(&file).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&bytes)
            .matches("\"kind\":\"content\"")
            .count(),
        1,
        "primary, RSS and shared entries carry one content record"
    );
    assert_eq!(backup::preview(&file).unwrap().format, 4);
    backup::restore(&b, &file, &exported.digest).unwrap();
    let minimum: i64 = b
        .query_row("SELECT MIN(id) FROM articles", [], |r| r.get(0))
        .unwrap();
    assert!(
        minimum > 3000,
        "old asynchronous article IDs cannot target the replacement library"
    );
    let sources: i64 = b
        .query_row("SELECT MIN(id) FROM feeds", [], |r| r.get(0))
        .unwrap();
    assert!(sources > 200 && sources > feed);
    let groups: i64 = b
        .query_row("SELECT MIN(id) FROM folders", [], |r| r.get(0))
        .unwrap();
    assert!(
        groups > 100,
        "old asynchronous group IDs cannot target the replacement library"
    );
    let settings: serde_json::Value =
        serde_json::from_str(&db::get_setting(&b, "app_settings").unwrap().unwrap()).unwrap();
    assert_eq!(settings["syncMode"], "hybrid");
    assert_eq!(settings["fontSize"], 18);
    assert_eq!(settings["autoStart"], false);
    assert_eq!(settings["articleAutoSummary"], true);
    for key in [
        "greader_endpoint",
        "greader_password",
        "ai_config",
        "local_sync_config",
    ] {
        assert!(db::get_setting(&b, key).unwrap().is_none());
    }
    assert_eq!(
        b.query_row("SELECT title_custom,fetched_title FROM feeds", [], |r| Ok(
            (r.get::<_, String>(0)?, r.get::<_, String>(1)?)
        ))
        .unwrap(),
        ("Custom".into(), "Fetched".into())
    );
    assert!(!super::runtime::status(&b).unwrap().connected);
    std::fs::remove_file(file).unwrap();
}

#[test]
fn portable_restore_highwater_overflow_and_failed_transaction_preserve_sequence() {
    let a = conn();
    seed(&a, &"a".repeat(32), "https://example.test/rss");
    let file = path("highwater");
    let info = backup::export(&a, &file).unwrap();
    for table in ["folders", "feeds", "articles"] {
        let b = conn();
        seed(&b, &"b".repeat(32), "https://current.test/rss");
        b.execute(
            "UPDATE sqlite_sequence SET seq=?1 WHERE name=?2",
            params![i64::MAX, table],
        )
        .unwrap();
        assert!(backup::restore(&b, &file, &info.digest).is_err());
        assert_eq!(
            b.query_row("SELECT feed_url FROM feeds", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "https://current.test/rss"
        );
        assert_eq!(
            b.query_row(
                "SELECT seq FROM sqlite_sequence WHERE name=?1",
                [table],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            i64::MAX
        );
    }
    let b = conn();
    seed(&b, &"b".repeat(32), "https://current.test/rss");
    b.execute_batch("CREATE TRIGGER fail_restored_article BEFORE INSERT ON articles BEGIN SELECT RAISE(ABORT,'injected failure after new source id'); END;").unwrap();
    assert!(backup::restore(&b, &file, &info.digest).is_err());
    for table in ["folders", "feeds", "articles"] {
        assert_eq!(
            b.query_row(
                "SELECT seq FROM sqlite_sequence WHERE name=?1",
                [table],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
    }
    std::fs::remove_file(file).unwrap();
}

#[test]
fn missing_body_remains_explicit_and_stale_ack_does_not_clear_later_intent() {
    let a = conn();
    let (_, article) = seed(&a, &"a".repeat(32), "https://example.test/rss");
    database::initialize(&a, LIB).unwrap();
    let batch = storage::seal_state(&a, 0).unwrap().unwrap();
    let journal_before: i64 = a
        .query_row("SELECT COUNT(*) FROM local_sync_journal", [], |r| r.get(0))
        .unwrap();
    db::set_read(&a, article, true).unwrap();
    storage::acknowledge(&a, 0, &batch.manifest).unwrap();
    assert_eq!(
        a.query_row("SELECT COUNT(*) FROM local_sync_journal", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        journal_before + 1,
        "the test must compare the new intent with the initialized journal instead of treating bootstrap entries as later intent"
    );
    a.execute("DELETE FROM local_sync_body_hold", []).unwrap();
    a.execute(
        "UPDATE articles SET content_id=NULL,rss_content_id=NULL",
        [],
    )
    .unwrap();
    db::set_starred(&a, article, true).unwrap();
    assert_eq!(database::pending_bodies(&a).unwrap(), 1);
    let later_state = database::checkpoint(&a).unwrap();
    a.execute("UPDATE local_sync_control SET generation=generation+1", [])
        .unwrap();
    assert!(storage::acknowledge(&a, 0, &batch.manifest).is_err());
    assert_eq!(
        database::checkpoint(&a).unwrap(),
        later_state,
        "the stale ACK must not clear the later star intent"
    );
    assert_eq!(database::pending_bodies(&a).unwrap(), 1);
}

#[test]
fn bootstrap_large_existing_library_does_not_queue_per_article_projection_work() {
    let c = conn();
    let (feed, _) = seed(&c, &"a".repeat(32), "https://example.test/rss");
    c.execute(
        "WITH RECURSIVE seq(n) AS(SELECT 1 UNION ALL SELECT n+1 FROM seq WHERE n<2000)
         INSERT INTO articles(feed_id,guid,title,published_at,is_read)
         SELECT ?1,'bootstrap-'||n,'Bootstrap '||n,'2026-11-09T00:00:00Z',n%2 FROM seq",
        [feed],
    )
    .unwrap();
    database::initialize(&c, LIB).unwrap();
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM local_sync_work WHERE job=''",
            [],
            |row| { row.get::<_, i64>(0) }
        )
        .unwrap(),
        0,
        "seeding already-materialized physical rows must not create projection work"
    );
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM local_sync_fields
             WHERE key LIKE 'a/%/meta'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        2001,
        "bootstrap must still publish every existing article winner"
    );
}

#[test]
fn unstar_cache_cleanup_is_bounded_by_affected_article_closure() {
    let file = path("unstar-bounded-cleanup").with_extension("db");
    let c = db::open(&file).unwrap();
    let (feed, first) = seed(&c, &"a".repeat(32), "https://example.test/rss");
    let content: i64 = c
        .query_row(
            "SELECT content_id FROM articles WHERE id=?1",
            [first],
            |row| row.get(0),
        )
        .unwrap();
    c.execute(
        "UPDATE articles SET is_starred=1,rss_content_id=content_id,published_at='2100-01-01T00:00:00Z' WHERE id=?1",
        [first],
    )
    .unwrap();
    c.execute(
        "WITH RECURSIVE seq(n) AS(SELECT 1 UNION ALL SELECT n+1 FROM seq WHERE n<2047)
         INSERT INTO articles(
             feed_id,guid,title,published_at,is_starred,content_id,rss_content_id
         )
         SELECT ?1,'cleanup-'||n,'Cleanup '||n,'2026-11-09T00:00:00Z',1,?2,?2
         FROM seq",
        params![feed, content],
    )
    .unwrap();
    database::initialize(&c, LIB).unwrap();

    let mark_started = std::time::Instant::now();
    assert_eq!(
        db::mark_all_read_with_enqueue(&c, Some(feed), None, false, Some(4_102_444_799_000), None,)
            .unwrap(),
        1
    );
    assert!(
        mark_started.elapsed() < std::time::Duration::from_secs(2),
        "a timestamp-scoped mark-all must use the physical candidate window"
    );
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM articles WHERE is_read=1", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        1,
        "only the article in the requested time scope may become read"
    );

    let target = first + 1;
    let started = std::time::Instant::now();
    db::set_starred_with_enqueue(&c, target, false).unwrap();
    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "one unstar must be bounded by its affected target closure, took {elapsed:?} over 2048 articles"
    );
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM local_sync_body_hold WHERE article_id=?1",
            [target],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1,
        "pre-ACK body protection must survive unstar"
    );

    drop(c);
    for cleanup in [
        file.clone(),
        file.with_extension("db-wal"),
        file.with_extension("db-shm"),
    ] {
        let _ = std::fs::remove_file(cleanup);
    }
}

#[test]
fn bootstrap_distinct_body_snapshot_uses_bounded_capture_batches() {
    let c = conn();
    let (feed, _) = seed(&c, &"a".repeat(32), "https://example.test/rss");
    let added = 384_i64;
    let tx = c.unchecked_transaction().unwrap();
    for index in 0..added {
        let text = format!("{index:08} {}", "content ".repeat(511));
        let html = format!("<p>{text}</p>");
        let identity = db::portable_content_identity("rss", Some(&html), &text);
        tx.execute(
            "INSERT INTO contents(identity,content_html,body_text,extraction_source)
             VALUES(?1,?2,?3,'rss')",
            params![identity, html, text],
        )
        .unwrap();
        let content = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO articles(feed_id,guid,title,published_at,is_starred,content_id,rss_content_id)
             VALUES(?1,?2,'Saved','2026-11-09T00:00:00Z',1,?3,?3)",
            params![feed, format!("bounded-{index}"), content],
        )
        .unwrap();
    }
    tx.commit().unwrap();

    super::logical::reset_bootstrap_snapshot_batch_contract();
    database::initialize(&c, LIB).unwrap();
    let articles = added + 1;
    let capture_batches = super::logical::bootstrap_snapshot_batch_count();
    assert!(capture_batches > 0);
    assert!(
        capture_batches < articles as usize,
        "article snapshot capture must aggregate rows instead of replaying one call per article, got {capture_batches} calls for {articles} articles"
    );
    let max_rows = super::logical::bootstrap_snapshot_max_rows();
    assert!(
        (2..=16).contains(&max_rows),
        "each bootstrap transaction must capture a bounded batch of at most 16 rows, got {max_rows}"
    );
    let expected_entries: std::collections::BTreeSet<_> = c
        .prepare(
            "SELECT COALESCE(a.local_sync_source,f.sync_id),a.guid
             FROM articles a JOIN feeds f ON f.id=a.feed_id",
        )
        .unwrap()
        .query_map([], |row| {
            Ok(super::model::entry_key(
                &row.get::<_, String>(0)?,
                &row.get::<_, String>(1)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let captured_entries: std::collections::BTreeSet<_> = c
        .prepare("SELECT substr(key,3,64) FROM local_sync_fields WHERE key LIKE 'a/%/meta'")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        captured_entries, expected_entries,
        "bootstrap must publish every existing article winner"
    );
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM local_sync_fields WHERE key LIKE 'a/%/body'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        articles
    );
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM local_sync_body_hold", [], |row| row
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        articles
    );
    assert!(
        c.query_row("SELECT COUNT(*) FROM local_sync_journal", [], |row| row
            .get::<_, i64>(0))
            .unwrap()
            >= articles
    );
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM local_sync_work WHERE job=''",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn bootstrap_preserves_and_materializes_real_edit_work() {
    let c = conn();
    let (_, article) = seed(&c, &"a".repeat(32), "https://example.test/rss");
    database::begin_initialize(&c, LIB).unwrap();
    assert!(database::resume_local_slice(&c).unwrap());
    let initialized: bool = c
        .query_row(
            "SELECT initialized FROM local_sync_control WHERE id=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!initialized);

    db::set_read(&c, article, true).unwrap();
    let target = format!(
        "a/{}",
        super::model::entry_key(&"a".repeat(32), "real-guid")
    );
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM local_sync_work WHERE job='' AND key=?1",
            [&target],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1,
        "a real edit during bootstrap must retain its durable materialization work"
    );

    database::initialize(&c, LIB).unwrap();
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM local_sync_work WHERE job='' AND key=?1",
            [&target],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1,
        "finishing bootstrap must not suppress or clear the real edit"
    );
    super::logical::drain_local(&c).unwrap();
    assert!(!c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM local_sync_work WHERE job='' AND key=?1)",
            [&target],
            |row| row.get::<_, bool>(0)
        )
        .unwrap());
    let (read, star, _) = super::logical::article_state(&c, &"a".repeat(32), "real-guid")
        .unwrap()
        .unwrap();
    assert!(read);
    assert!(!star);
    assert_eq!(flags(&c), (true, false));
}

#[test]
fn fifty_thousand_articles_initial_shards_and_single_action_delta_budget() {
    let c = conn();
    let (feed, _) = seed(&c, &"a".repeat(32), "https://example.test/rss");
    c.execute("WITH RECURSIVE seq(n) AS(SELECT 1 UNION ALL SELECT n+1 FROM seq WHERE n<49999) INSERT INTO articles(feed_id,guid,title,published_at,is_read) SELECT ?1,'large-'||n,'Title '||n,'2026-11-09T00:00:00Z',1 FROM seq",[feed]).unwrap();
    let start = std::time::Instant::now();
    println!("PERF phase=initialize start");
    database::initialize(&c, LIB).unwrap();
    println!("PERF initialize_ms={}", start.elapsed().as_millis());
    let start = std::time::Instant::now();
    println!("PERF phase=seal start");
    let initial = storage::seal_state(&c, 0).unwrap().unwrap();
    println!("PERF seal_ms={}", start.elapsed().as_millis());
    let bytes: usize = initial.manifest.shards.iter().map(|s| s.size).sum();
    assert!(initial
        .manifest
        .shards
        .iter()
        .all(|s| s.size <= 1024 * 1024));
    storage::acknowledge(&c, 0, &initial.manifest).unwrap();
    let start = std::time::Instant::now();
    println!("PERF phase=click start");
    c.execute(
        "UPDATE articles SET is_starred=1 WHERE guid='large-49999'",
        [],
    )
    .unwrap();
    println!("PERF click_us={}", start.elapsed().as_micros());
    let start = std::time::Instant::now();
    println!("PERF phase=delta start");
    let delta = storage::seal_state(&c, 0).unwrap().unwrap();
    println!("PERF delta_ms={}", start.elapsed().as_millis());
    assert_eq!(delta.manifest.deltas, 1);
    let fast = storage::small_delta(&c, &initial.manifest, &delta.manifest)
        .unwrap()
        .unwrap();
    let start = std::time::Instant::now();
    database::merge_small(&c, 0, &fast).unwrap().unwrap();
    println!("PERF fast_apply_us={}", start.elapsed().as_micros());
    storage::acknowledge(&c, 0, &delta.manifest).unwrap();
    let completion = storage::seal_state(&c, 0)
        .unwrap()
        .expect("completed local work needs one completed-only publication");
    assert_eq!(completion.manifest.shards, delta.manifest.shards);
    assert!(
        completion.manifest.publication > delta.manifest.publication,
        "completed-only publication must advance"
    );
    storage::acknowledge(&c, 0, &completion.manifest).unwrap();
    let start = std::time::Instant::now();
    assert!(storage::seal_state(&c, 0).unwrap().is_none());
    println!("PERF idle_us={}", start.elapsed().as_micros());
    let new: Vec<_> = delta
        .manifest
        .shards
        .iter()
        .filter(|s| {
            !initial
                .manifest
                .shards
                .iter()
                .any(|old| old.digest == s.digest)
        })
        .collect();
    assert_eq!(new.len(), 1);
    assert!(new[0].size < 4096);
    assert!(storage::reassemble(&c, &delta.manifest)
        .unwrap()
        .dominates(&storage::reassemble(&c, &initial.manifest).unwrap())
        .unwrap());
    println!(
        "50k state bytes={bytes}, shards={}, steady_delta_bytes={}, manifest_bytes={}",
        initial.manifest.shards.len(),
        new[0].size,
        serde_json::to_vec(&delta.manifest).unwrap().len()
    );
}

#[test]
fn similar_normalized_source_urls_stay_distinct() {
    let c = conn();
    let urls = [
        "https://example.test/rss?utm_source=a",
        "https://example.test/rss?utm_source=b",
        "http://example.test/rss?utm_source=a",
        "https://m.example.test/rss?utm_source=a",
    ];
    let folder = db::create_folder(&c, "Group", "article").unwrap();
    for url in urls {
        db::insert_feed(&c, url, None, "Feed", None, folder, "inherit", false, false).unwrap();
    }
    database::initialize(&c, LIB).unwrap();
    let projected = database::projection(&database::checkpoint(&c).unwrap()).unwrap();
    assert_eq!(
        projected
            .keys()
            .filter(|k| k.starts_with("s/") && k.ends_with("/feed_url"))
            .count(),
        4
    );
    assert_eq!(json!(db::list_feeds(&c).unwrap().len()), 4);
}

#[test]
fn settings_production_entry_updates_tray_label_without_a_native_window() {
    static LABEL: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
    fn write(label: &str) {
        *LABEL.lock().unwrap() = label.to_owned();
    }
    crate::scheduler::set_tray_text_writer_for_test(Some(write));
    let c = conn();
    crate::commands::apply_app_settings_patch(&c, &json!({"syncMode":"hybrid"})).unwrap();
    assert_eq!(
        crate::scheduler::take_last_tray_refresh_mode_for_test().as_deref(),
        Some("hybrid")
    );
    assert_eq!(*LABEL.lock().unwrap(), "刷新本地订阅源");
    crate::commands::apply_app_settings_patch(&c, &json!({"syncMode":"direct"})).unwrap();
    assert_eq!(*LABEL.lock().unwrap(), "刷新全部订阅源");
    crate::scheduler::set_tray_text_writer_for_test(None);
}

#[test]
fn settings_patch_preserves_unobserved_remote_keys_and_rejects_partial_invalid_updates() {
    let a = conn();
    let b = conn();
    db::set_setting(
        &a,
        "app_settings",
        r#"{"fontSize":16,"themeMode":"light","autoStart":false}"#,
    )
    .unwrap();
    database::initialize(&a, LIB).unwrap();
    database::initialize(&b, LIB).unwrap();
    exchange(&a, &b);
    crate::commands::apply_app_settings_patch(
        &b,
        &json!({"themeMode":"dark","readBodyRetentionDays":30}),
    )
    .unwrap();
    database::merge_apply(&a, 0, &database::checkpoint(&b).unwrap()).unwrap();
    // The frontend may still cache the old light theme. Its explicit font-size
    // intent contains no theme key; only that intent is committed atomically.
    let saved: serde_json::Value = serde_json::from_str(
        &crate::commands::apply_app_settings_patch(
            &a,
            &json!({"fontSize":18,"closePromptShown":true}),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(saved["themeMode"], "dark");
    assert_eq!(saved["readBodyRetentionDays"], 30);
    assert_eq!(saved["fontSize"], 18);
    assert_eq!(saved["autoStart"], false);
    let before = db::get_setting(&a, "app_settings").unwrap();
    let checkpoint = database::checkpoint(&a).unwrap();
    for patch in [
        json!({"fontSize":20,"local_sync_password":"secret"}),
        json!({"fontSize":-1}),
        json!({"readBodyRetentionDays":1.5}),
        json!({"readBodyRetentionDays":3651}),
        json!({"syncMode":"wrong"}),
    ] {
        assert_eq!(
            crate::commands::apply_app_settings_patch(&a, &patch)
                .unwrap_err()
                .code,
            "validate"
        );
        assert_eq!(db::get_setting(&a, "app_settings").unwrap(), before);
        assert_eq!(database::checkpoint(&a).unwrap(), checkpoint);
    }
    assert_eq!(
        crate::commands::apply_setting(&a, "app_settings", r#"{"fontSize":15}"#)
            .unwrap_err()
            .code,
        "protectedSetting"
    );
    assert!(!checkpoint.fields.contains_key("p/closePromptShown"));
    let c = conn();
    for raw in ["not-json", "[]"] {
        db::set_setting(&c, "app_settings", raw).unwrap();
        assert_eq!(
            crate::commands::apply_app_settings_patch(&c, &json!({"fontSize":18}))
                .unwrap_err()
                .code,
            "invalidSettings"
        );
        assert_eq!(
            db::get_setting(&c, "app_settings").unwrap().as_deref(),
            Some(raw)
        );
    }
    crate::commands::apply_app_settings_patch(&a, &json!({"readBodyRetentionDays":0})).unwrap();
}

#[test]
fn settings_patch_and_nested_write_savepoint_roll_back_without_poisoning_outer_transaction() {
    let c = conn();
    db::set_setting(&c, "app_settings", "{}").unwrap();
    let (feed, article) = seed(&c, &"a".repeat(32), "https://example.test/rss");
    database::initialize(&c, LIB).unwrap();
    let before = database::checkpoint(&c).unwrap();
    c.execute_batch("CREATE TRIGGER reject_settings BEFORE UPDATE ON settings WHEN NEW.key='app_settings' BEGIN SELECT RAISE(ABORT,'injected patch failure'); END;").unwrap();
    assert!(crate::commands::apply_app_settings_patch(&c, &json!({"fontSize":18})).is_err());
    assert_eq!(database::checkpoint(&c).unwrap(), before);
    assert_eq!(
        db::get_setting(&c, "app_settings").unwrap().as_deref(),
        Some("{}")
    );
    c.execute_batch("DROP TRIGGER reject_settings;").unwrap();
    let tx = db::write_transaction(&c).unwrap();
    tx.execute("UPDATE feeds SET title='outer' WHERE id=?1", [feed])
        .unwrap();
    tx.execute_batch("CREATE TRIGGER reject_content BEFORE INSERT ON contents BEGIN SELECT RAISE(ABORT,'injected nested failure'); END;").unwrap();
    let (token, _) = db::article_content_commit_token(&tx, article)
        .unwrap()
        .unwrap();
    assert!(
        db::replace_article_content_checked(&tx, article, &token, "<p>new nested content</p>")
            .is_err()
    );
    tx.execute_batch("DROP TRIGGER reject_content;").unwrap();
    tx.commit().unwrap();
    assert_eq!(
        c.query_row("SELECT title FROM feeds WHERE id=?1", [feed], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "outer"
    );
    assert_eq!(
        db::get_article(&c, article)
            .unwrap()
            .unwrap()
            .content_html
            .as_deref(),
        Some("<p>body</p>")
    );
}

#[test]
fn body_archive_wal_upgrade_counterexample_and_bounded_production_writer() {
    use std::sync::{Arc, Barrier};
    let file = path("wal-upgrade").with_extension("db");
    let c = db::open(&file).unwrap();
    let (feed, article) = seed(&c, &"a".repeat(32), "https://example.test/rss");
    c.execute("WITH RECURSIVE n(x) AS(SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<300) INSERT INTO articles(feed_id,guid,title,content_id,rss_content_id) SELECT ?1,'wal-'||x,'Saved-'||x,a.content_id,a.content_id FROM n JOIN articles a ON a.id=?2",params![feed,article]).unwrap();
    database::initialize(&c, LIB).unwrap();
    let other = db::open(&file).unwrap();
    let read = c.unchecked_transaction().unwrap();
    let _: i64 = read
        .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
        .unwrap();
    db::set_read_with_enqueue(&other, article, true).unwrap();
    let failure = read
        .execute(
            "UPDATE settings SET value=value WHERE key='app_settings'",
            [],
        )
        .unwrap_err();
    assert!(
        matches!(failure,rusqlite::Error::SqliteFailure(code,_) if code.extended_code==517),
        "WAL snapshot upgrade must reproduce SQLITE_BUSY_SNAPSHOT"
    );
    drop(read);
    drop(other);
    let pending = storage::seal_bodies(&c, 0).unwrap().unwrap();
    let ready = Arc::new(Barrier::new(2));
    let worker_ready = ready.clone();
    let target = file.clone();
    let samples = std::thread::spawn(move || {
        let db = db::open(&target).unwrap();
        let mut samples = Vec::new();
        worker_ready.wait();
        for n in 0..40 {
            let start = std::time::Instant::now();
            let outcome = match n % 4 {
                0 => db::set_read_with_enqueue(&db, article, n % 2 == 0).map(|_| ()),
                1 => db::set_starred_with_enqueue(&db, article, true),
                2 => db::set_read_bulk_with_enqueue(&db, &[article], false).map(|_| ()),
                _ => db::mark_all_read_with_enqueue(&db, None, None, false, None, None).map(|_| ()),
            };
            samples.push((
                start.elapsed().as_micros() as u64,
                outcome.map_err(|e| e.code),
            ));
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        samples
    });
    ready.wait();
    let ack = storage::acknowledge(&c, 0, &pending.manifest);
    let results = samples.join().unwrap();
    println!("WAL_FOREGROUND samples={results:?}");
    ack.unwrap();
    assert_eq!(results.len(), 40);
    assert!(results.iter().all(|(_, result)| result.is_ok()));
    drop(c);
    std::fs::remove_file(file).unwrap();
}

#[test]
fn body_archive_stable_rows_use_direct_plan_and_match_logical_snapshot() {
    let c = conn();
    let (feed, article) = seed(&c, &"a".repeat(32), "https://example.test/rss");
    c.execute(
        "WITH RECURSIVE n(x) AS(SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<300)
         INSERT INTO articles(feed_id,guid,title,content_id,rss_content_id)
         SELECT ?1,'body-plan-'||x,'Saved-'||x,a.content_id,a.content_id
         FROM n JOIN articles a ON a.id=?2",
        params![feed, article],
    )
    .unwrap();
    database::initialize(&c, LIB).unwrap();
    assert!(!super::logical::query_requires_logical_state(&c).unwrap());

    let summarize = |sql: &str| {
        let mut stmt = c.prepare(sql).unwrap();
        stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
    };
    let plan = |sql: &str| {
        let mut stmt = c.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        rows.join("\n")
    };

    let logical_sql = super::body_archive::body_rows_sql(true);
    let direct_sql = super::body_archive::body_rows_sql(false);
    assert_eq!(summarize(&logical_sql), summarize(&direct_sql));
    let logical_plan = plan(&logical_sql);
    let direct_plan = plan(&direct_sql);
    assert!(
        logical_plan.contains("local_sync_source_state") && logical_plan.contains("TEMP B-TREE"),
        "{logical_plan}"
    );
    assert!(
        !direct_plan.contains("MATERIALIZE local_sync_article_state")
            && !direct_plan.contains("local_sync_source_state")
            && !direct_plan.contains("TEMP B-TREE"),
        "{direct_plan}"
    );

    db::set_starred_with_enqueue(&c, article, true).unwrap();
    assert!(super::logical::query_requires_logical_state(&c).unwrap());
    assert!(!super::body_archive::body_rows_require_logical_state(&c).unwrap());
    assert_eq!(summarize(&logical_sql), summarize(&direct_sql));

    let invalid_plan = plan(&super::logical::invalid_locator_sql());
    assert!(
        !invalid_plan.contains("local_sync_article_locator"),
        "{invalid_plan}"
    );
    let meta_key: String = c
        .query_row(
            "SELECT key FROM local_sync_fields WHERE key LIKE 'a/%/meta' LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    c.execute("DELETE FROM local_sync_fields WHERE key=?1", [meta_key])
        .unwrap();
    assert!(super::logical::require_valid_locators(&c).is_err());

    c.execute("UPDATE local_sync_control SET applying=1", [])
        .unwrap();
    assert!(super::logical::query_requires_logical_state(&c).unwrap());
    assert!(super::body_archive::body_rows_require_logical_state(&c).unwrap());
}

#[test]
fn bootstrap_interruption_reopens_without_overwriting_between_chunk_edits() {
    let file = path("bootstrap").with_extension("db");
    let c = db::open(&file).unwrap();
    let (feed, _) = seed(&c, &"a".repeat(32), "https://example.test/rss");
    c.execute("WITH RECURSIVE n(x) AS(SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<600) INSERT INTO articles(feed_id,guid,title,is_read) SELECT ?1,'seed-'||x,'seed',1 FROM n",[feed]).unwrap();
    c.execute_batch("CREATE TRIGGER interrupt_seed BEFORE INSERT ON local_sync_journal WHEN NEW.seq>300 BEGIN SELECT RAISE(ABORT,'injected power-loss boundary'); END;").unwrap();
    assert!(database::initialize(&c, LIB).is_err());
    assert!(!c
        .query_row("SELECT initialized FROM local_sync_control", [], |r| r
            .get::<_, bool>(0))
        .unwrap());
    assert!(!c
        .query_row("SELECT applying FROM local_sync_control", [], |r| r
            .get::<_, bool>(0))
        .unwrap());
    c.execute_batch("DROP TRIGGER interrupt_seed;").unwrap();
    c.execute("UPDATE articles SET is_read=0 WHERE guid='seed-600'", [])
        .unwrap();
    drop(c);
    let c = db::open(&file).unwrap();
    database::initialize(&c, LIB).unwrap();
    assert!(!c
        .query_row(
            "SELECT is_read FROM articles WHERE guid='seed-600'",
            [],
            |r| r.get::<_, bool>(0)
        )
        .unwrap());
    let state = database::checkpoint(&c).unwrap();
    let key = format!(
        "a/{}/read",
        super::model::entry_key(&"a".repeat(32), "seed-600")
    );
    assert_eq!(
        state.fields[&key].value,
        crate::webdav_transport::FieldValue::Set(json!(false))
    );
    drop(c);
    std::fs::remove_file(file).unwrap();
}

#[test]
fn body_gc_requires_all_known_peer_release_coverage_and_retains_late_protection() {
    let c = conn();
    let source = "a".repeat(32);
    let (feed, article) = seed(&c, &source, "https://example.test/rss");
    database::initialize(&c, LIB).unwrap();
    let body = storage::seal_bodies(&c, 0).unwrap().unwrap();
    storage::acknowledge(&c, 0, &body.manifest).unwrap();
    let body_ref = super::body_archive::catalog(&c, &body.manifest)
        .unwrap()
        .entries
        .into_values()
        .next()
        .unwrap()
        .body_ref;
    let first_entry = super::model::entry_key(&source, "real-guid");
    let body_json = serde_json::to_string(&body_ref).unwrap();
    let peer = "d".repeat(32);
    super::logical::register_actor(&c, &peer).unwrap();
    db::set_read(&c, article, true).unwrap();
    super::logical::drain_local(&c).unwrap();
    let first_receipt = storage::seal_bodies(&c, 0).unwrap().unwrap();
    let old_digests = body
        .manifest
        .shards
        .iter()
        .map(|object| object.digest.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let receipt_digests = first_receipt
        .manifest
        .shards
        .iter()
        .map(|object| object.digest.clone())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(
        old_digests.is_subset(&receipt_digests),
        "receipt-only publication must retain every original body object"
    );
    assert!(
        (old_digests.len()..=old_digests.len() + 1).contains(&receipt_digests.len()),
        "receipt/catalog paging may reuse all old objects or add at most one bounded object"
    );
    let first_receipt_entries = super::body_archive::catalog(&c, &first_receipt.manifest)
        .unwrap()
        .entries
        .into_values()
        .map(|entry| (entry.entry, serde_json::to_string(&entry.body_ref).unwrap()))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        first_receipt_entries,
        std::collections::BTreeSet::from([(first_entry.clone(), body_json.clone())]),
        "the first receipt must cover exactly the first article's hold"
    );
    assert!(
        first_receipt.manifest.publication > body.manifest.publication,
        "receipt-only publication must advance"
    );

    let second = db::NewArticle {
        guid: "already-read-guid".into(),
        url: Some("https://example.test/second".into()),
        title: "Second".into(),
        author: None,
        summary: Some("body".into()),
        content_html: Some("<p>body</p>".into()),
        body_text: "body".into(),
        image_url: None,
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: Some("2026-11-09T00:00:00Z".into()),
        source: "direct".into(),
    };
    let (already_read, inserted) = db::upsert_article_with_feed(&c, feed, &second, false).unwrap();
    assert!(inserted);
    db::set_read(&c, already_read, true).unwrap();
    super::logical::drain_local(&c).unwrap();
    let second_entry = super::model::entry_key(&source, "already-read-guid");
    let holds = |c: &Connection| {
        let mut stmt = c
            .prepare("SELECT entry,body_ref FROM local_sync_body_hold ORDER BY entry,body_ref")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<std::collections::BTreeSet<_>>>()
            .unwrap()
    };
    assert_eq!(
        holds(&c),
        std::collections::BTreeSet::from([
            (first_entry.clone(), body_json.clone()),
            (second_entry.clone(), body_json.clone()),
        ])
    );

    let state = storage::seal_state(&c, 0).unwrap().unwrap();
    let mut peer_manifest = state.manifest.clone();
    peer_manifest.actor = peer.clone();
    peer_manifest.completed = peer_manifest.covered.clone();
    db::set_setting(
        &c,
        &format!("local_sync_peer_state_{peer}"),
        &serde_json::to_string(&peer_manifest).unwrap(),
    )
    .unwrap();
    let assert_release_blocked = |manifest: &storage::Manifest| {
        let catalog = super::body_archive::catalog(&c, manifest).unwrap();
        let covered = super::logical::frontier(&c).unwrap();
        let peers = storage::peer_frontiers(&c, &database::actor(&c).unwrap(), &covered).unwrap();
        let mut release = super::body_archive::Release::default();
        assert!(
            catalog
                .entries
                .values()
                .all(|entry| !release.can_release(&c, &peers, entry).unwrap()),
            "pending body receipt/hold must block release"
        );
    };
    assert_release_blocked(&first_receipt.manifest);
    assert_eq!(
        storage::pending(&c, "body", 0)
            .unwrap()
            .unwrap()
            .manifest
            .file_name()
            .unwrap(),
        first_receipt.manifest.file_name().unwrap()
    );

    storage::acknowledge(&c, 0, &first_receipt.manifest).unwrap();
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM local_sync_body_receipts WHERE manifest=?1",
            [first_receipt.manifest.file_name().unwrap()],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1,
        "ACK must persist one exact receipt row for the single pending entry"
    );
    assert_eq!(
        holds(&c),
        std::collections::BTreeSet::from([(second_entry.clone(), body_json.clone())]),
        "an earlier receipt may clear only its exact (entry, BodyRef) hold"
    );

    let second_receipt = storage::seal_bodies(&c, 0).unwrap().unwrap();
    let second_digests = second_receipt
        .manifest
        .shards
        .iter()
        .map(|object| object.digest.clone())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(old_digests.is_subset(&second_digests));
    assert!(
        (old_digests.len()..=old_digests.len() + 1).contains(&second_digests.len()),
        "second receipt/catalog paging may reuse all old objects or add at most one bounded object"
    );
    let second_receipt_entries = super::body_archive::catalog(&c, &second_receipt.manifest)
        .unwrap()
        .entries
        .into_values()
        .map(|entry| (entry.entry, serde_json::to_string(&entry.body_ref).unwrap()))
        .collect::<std::collections::BTreeSet<_>>();
    let allowed_second_receipt_entries = std::collections::BTreeSet::from([
        (first_entry.clone(), body_json.clone()),
        (second_entry.clone(), body_json.clone()),
    ]);
    assert!(
        second_receipt_entries.contains(&(second_entry.clone(), body_json.clone())),
        "the second receipt must contain the new hold"
    );
    assert!(
        second_receipt_entries.is_subset(&allowed_second_receipt_entries),
        "the second receipt may retain the first entry but cannot add another body reference"
    );
    assert!(
        second_receipt.manifest.publication > first_receipt.manifest.publication,
        "the hold created after the first receipt snapshot needs its own publication"
    );
    assert_release_blocked(&second_receipt.manifest);
    storage::acknowledge(&c, 0, &second_receipt.manifest).unwrap();
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM local_sync_body_receipts WHERE manifest=?1",
            [second_receipt.manifest.file_name().unwrap()],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        second_receipt_entries.len() as i64
    );
    let mut receipt_statement = c
        .prepare(
            "SELECT entry,body_ref FROM local_sync_body_receipts
             WHERE manifest=?1 ORDER BY entry,body_ref",
        )
        .unwrap();
    let receipt_entries = receipt_statement
        .query_map([second_receipt.manifest.file_name().unwrap()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<std::collections::BTreeSet<_>>>()
        .unwrap();
    assert_eq!(
        receipt_entries, second_receipt_entries,
        "ACK must persist exactly the catalog's (entry, BodyRef) receipt set"
    );
    assert!(holds(&c).is_empty());

    let released = storage::seal_bodies(&c, 0).unwrap().unwrap();
    assert!(
        released.manifest.shards.is_empty(),
        "only after exact body receipts cover every hold may release proceed"
    );
    assert!(
        released.manifest.publication > second_receipt.manifest.publication,
        "release must be a new publication"
    );
    storage::acknowledge(&c, 0, &released.manifest).unwrap();
    assert!(
        storage::seal_bodies(&c, 0).unwrap().is_none(),
        "an acknowledged release with unchanged inputs is idle"
    );

    db::set_starred(&c, article, true).unwrap();
    super::logical::drain_local(&c).unwrap();
    let repinned = storage::seal_bodies(&c, 0).unwrap().unwrap();
    assert!(
        !repinned.manifest.shards.is_empty(),
        "a later pin recreates a body obligation after old release ACK"
    );
}

#[test]
fn acknowledged_body_receipt_cleanup_never_releases_a_later_same_tuple_hold() {
    let c = conn();
    let source = "a".repeat(32);
    let (_feed, article) = seed(&c, &source, "https://example.test/rss");
    database::initialize(&c, LIB).unwrap();

    let first = storage::seal_bodies(&c, 0).unwrap().unwrap();
    let (old_hold_id, tuple) = c
        .query_row(
            "SELECT id,entry||char(0)||body_ref FROM local_sync_body_hold LIMIT 1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .unwrap();
    let first_manifest = first.manifest.file_name().unwrap();
    storage::acknowledge(&c, 0, &first.manifest).unwrap();
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM local_sync_body_hold", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM local_sync_body_receipts WHERE manifest=?1",
            [&first_manifest],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1,
        "the first receipt remains as historical ACK evidence"
    );

    db::set_starred(&c, article, true).unwrap();
    super::logical::drain_local(&c).unwrap();
    let (new_hold_id, new_tuple) = c
        .query_row(
            "SELECT id,entry||char(0)||body_ref FROM local_sync_body_hold LIMIT 1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .unwrap();
    assert_eq!(
        new_tuple, tuple,
        "the logical hold tuple is intentionally identical"
    );
    assert_ne!(
        new_hold_id, old_hold_id,
        "a later obligation has a new durable hold identity"
    );

    storage::acknowledge(&c, 0, &first.manifest).unwrap();
    assert_eq!(
        c.query_row("SELECT id FROM local_sync_body_hold", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        new_hold_id,
        "repeating an old ACK must not release the later hold generation"
    );

    storage::cleanup_slice(&c).unwrap();
    assert_eq!(
        c.query_row("SELECT id FROM local_sync_body_hold", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        new_hold_id,
        "an old receipt may not delete a later hold with the same tuple"
    );

    let next = storage::seal_bodies(&c, 0).unwrap().unwrap();
    assert!(
        next.manifest.publication > first.manifest.publication,
        "the later hold identity must force a new receipt-only publication"
    );
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM local_sync_body_receipt_holds WHERE manifest=?1 AND hold_id=?2",
            params![next.manifest.file_name().unwrap(), new_hold_id],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1,
        "the new manifest must bind the exact later hold generation"
    );

    storage::acknowledge(&c, 0, &next.manifest).unwrap();
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM local_sync_body_hold", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        0,
        "the hold is released only by its own exact receipt"
    );
    assert!(
        storage::seal_bodies(&c, 0).unwrap().is_none(),
        "unchanged inputs after the exact receipt ACK are idle"
    );
}

#[test]
fn body_receipt_hold_index_and_fifty_thousand_cascade_are_exact() {
    let c = conn();
    let index_columns = || {
        let mut stmt = c
            .prepare("PRAGMA index_info(local_sync_receipt_hold_id)")
            .unwrap();
        stmt.query_map([], |row| row.get::<_, String>(2))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    assert_eq!(
        index_columns(),
        vec!["hold_id"],
        "new databases need a leading index for the receipt-to-hold foreign key"
    );

    c.execute("DROP INDEX local_sync_receipt_hold_id", [])
        .unwrap();
    assert!(index_columns().is_empty());
    database::begin_initialize(&c, LIB).unwrap();
    assert_eq!(
        index_columns(),
        vec!["hold_id"],
        "opening an existing v1 library must restore the foreign-key index"
    );

    c.execute(
        "WITH RECURSIVE n(x) AS (
             SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<50000
         )
         INSERT INTO local_sync_body_hold(entry,body_ref)
         SELECT printf('entry-%05d',x),
                json_object('primary',printf('body-%05d',x),'rss',NULL)
         FROM n",
        [],
    )
    .unwrap();
    c.execute(
        "INSERT INTO local_sync_body_receipt_holds(manifest,hold_id)
         SELECT 'receipt',id FROM local_sync_body_hold",
        [],
    )
    .unwrap();
    c.execute(
        "INSERT INTO local_sync_body_hold(entry,body_ref)
         VALUES('later-hold',json_object('primary','later-body','rss',NULL))",
        [],
    )
    .unwrap();
    let later_id = c.last_insert_rowid();

    let started = std::time::Instant::now();
    c.execute(
        "DELETE FROM local_sync_body_hold
         WHERE id IN(SELECT hold_id FROM local_sync_body_receipt_holds)",
        [],
    )
    .unwrap();
    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(30),
        "50k receipt-bound holds took {elapsed:?} to cascade"
    );
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM local_sync_body_receipt_holds",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0,
        "deleting the bound holds must remove their exact receipt edges"
    );
    assert_eq!(
        c.query_row("SELECT id FROM local_sync_body_hold", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        later_id,
        "an unbound later hold must survive the prior receipt generation"
    );
}

#[test]
fn body_ack_metadata_survives_release_failure_and_retry_is_idempotent() {
    let c = conn();
    seed(&c, &"a".repeat(32), "https://example.test/rss");
    database::initialize(&c, LIB).unwrap();
    let pending = storage::seal_bodies(&c, 0).unwrap().unwrap();
    let identity = pending.manifest.file_name().unwrap();

    c.execute_batch(
        "CREATE TRIGGER reject_body_hold_release
         BEFORE DELETE ON local_sync_body_hold
         BEGIN SELECT RAISE(ABORT,'injected release failure'); END;",
    )
    .unwrap();
    assert!(storage::acknowledge(&c, 0, &pending.manifest).is_err());
    assert!(
        storage::pending(&c, "body", 0).unwrap().is_none(),
        "the confirmed manifest must be committed before physical hold release"
    );
    assert_eq!(
        storage::previous_manifest(&c, &pending.manifest.actor, "body")
            .unwrap()
            .unwrap()
            .file_name()
            .unwrap(),
        identity
    );
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM local_sync_body_hold", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        1
    );
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM local_sync_body_receipt_holds WHERE manifest=?1",
            [&identity],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );

    c.execute_batch("DROP TRIGGER reject_body_hold_release;")
        .unwrap();
    storage::acknowledge(&c, 0, &pending.manifest).unwrap();
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM local_sync_body_hold", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM local_sync_body_receipt_holds WHERE manifest=?1",
            [&identity],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0,
        "the idempotent retry must finish exact physical receipt cleanup"
    );
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM local_sync_body_receipts WHERE manifest=?1",
            [identity],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1,
        "physical release must not erase historical receipt evidence"
    );
}

#[test]
fn body_receipt_staging_is_not_cleaned_while_build_root_is_active() {
    let c = conn();
    database::initialize(&c, LIB).unwrap();
    let staged_hold = c
        .execute(
            "INSERT INTO local_sync_body_hold(entry,body_ref)
             VALUES('staged-entry',json_object('primary','staged-body','rss',NULL))",
            [],
        )
        .unwrap();
    assert_eq!(staged_hold, 1);
    let hold_id = c.last_insert_rowid();
    let root = storage::BuildRoot::new(&c, "body-build").unwrap();
    c.execute(
        "INSERT INTO local_sync_body_receipt_holds(manifest,hold_id)
         VALUES('staged-manifest',?1)",
        [hold_id],
    )
    .unwrap();

    assert!(!storage::cleanup_slice(&c).unwrap());
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM local_sync_body_receipt_holds
             WHERE manifest='staged-manifest'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1,
        "cleanup must not erase receipt metadata before the build-rooted seal binds its pending manifest"
    );

    drop(root);
    assert!(storage::cleanup_slice(&c).unwrap());
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM local_sync_body_receipt_holds
             WHERE manifest='staged-manifest'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0,
        "orphaned staging metadata is collectable once no root or pending remains"
    );
}

/// An ACK commits the publication first and then releases the exact receipt
/// holds in separate bounded transactions. A crash in that window used to leave
/// the hold and its mapping until a later body publication. Reopening the file
/// must now heal them through the maintenance slice, while the historical ACK
/// receipt and any late re-pin protection stay untouched.
#[test]
fn ack_committed_before_hold_release_self_heals_on_reopen() {
    let file = path("ack-crash-recovery").with_extension("db");
    let identity = {
        let c = db::open(&file).unwrap();
        c.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
        seed(&c, &"a".repeat(32), "https://example.test/rss");
        database::initialize(&c, LIB).unwrap();
        let pending = storage::seal_bodies(&c, 0).unwrap().unwrap();
        let identity = pending.manifest.file_name().unwrap();
        c.execute_batch(
            "CREATE TRIGGER reject_body_hold_release
             BEFORE DELETE ON local_sync_body_hold
             BEGIN SELECT RAISE(ABORT,'injected release failure'); END;",
        )
        .unwrap();
        assert!(storage::acknowledge(&c, 0, &pending.manifest).is_err());
        assert!(
            storage::pending(&c, "body", 0).unwrap().is_none(),
            "the ACK metadata and pending removal must be committed before physical release"
        );
        let holds: i64 = c
            .query_row("SELECT COUNT(*) FROM local_sync_body_hold", [], |r| {
                r.get(0)
            })
            .unwrap();
        let mappings: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM local_sync_body_receipt_holds WHERE manifest=?1",
                [&identity],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!((holds, mappings), (1, 1));
        // Drop the injected failure before closing: a real crash would not leave
        // a trigger behind, only the committed ACK with its stale hold.
        c.execute_batch("DROP TRIGGER reject_body_hold_release;")
            .unwrap();
        drop(c);
        identity
    };
    let c = db::open(&file).unwrap();
    c.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
    let holds: i64 = c
        .query_row("SELECT COUNT(*) FROM local_sync_body_hold", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(holds, 1, "the committed ACK left a stale physical hold");
    assert!(storage::cleanup_slice(&c).unwrap());
    let holds: i64 = c
        .query_row("SELECT COUNT(*) FROM local_sync_body_hold", [], |r| {
            r.get(0)
        })
        .unwrap();
    let mappings: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM local_sync_body_receipt_holds WHERE manifest=?1",
            [&identity],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        (holds, mappings),
        (0, 0),
        "reopen/maintenance must release the stale hold and its exact receipt edge"
    );
    assert!(
        storage::seal_bodies(&c, 0).unwrap().is_none(),
        "no body input changed, so the repaired library stays idle"
    );
    let receipts: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM local_sync_body_receipts WHERE manifest=?1",
            [&identity],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        receipts, 1,
        "historical ACK receipts must survive the repair"
    );
    drop(c);
    std::fs::remove_file(&file).unwrap();
}

#[test]
fn body_archive_unique_payload_small_increment_retry_and_download_pins() {
    let a = conn();
    let b = conn();
    let (feed, first) = seed(&a, &"a".repeat(32), "https://example.test/rss");
    a.execute("UPDATE articles SET rss_content_id=content_id", [])
        .unwrap();
    a.execute("WITH RECURSIVE n(x) AS(SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<399) INSERT INTO articles(feed_id,guid,title,content_id,rss_content_id) SELECT ?1,'archive-'||x,'Saved',a.content_id,a.content_id FROM n JOIN articles a ON a.id=?2",params![feed,first]).unwrap();
    database::initialize(&a, LIB).unwrap();
    database::initialize(&b, LIB).unwrap();
    exchange(&a, &b);
    let initial = storage::seal_bodies(&a, 0).unwrap().unwrap();
    let catalog = super::body_archive::catalog(&a, &initial.manifest).unwrap();
    assert_eq!(catalog.entries.len(), 400);
    assert_eq!(catalog.contents.len(), 1);
    assert_eq!(catalog.streams.len(), 1);
    let old_objects = storage::publication_objects(&a, &initial.manifest).unwrap();
    for object in &old_objects {
        let bytes = storage::cached_object(&a, &initial.manifest.actor, "body", object)
            .unwrap()
            .unwrap();
        storage::cache_object(&b, 0, &initial.manifest, object, &bytes).unwrap();
    }
    b.execute(
        "INSERT INTO local_sync_downloads(actor,kind,manifest) VALUES(?1,'body',?2)",
        params![
            initial.manifest.actor,
            serde_json::to_string(&initial.manifest).unwrap()
        ],
    )
    .unwrap();
    super::body_archive::record_links(&b, &initial.manifest).unwrap();
    storage::evict_cache(&b, 0).unwrap();
    assert!(
        old_objects.iter().all(
            |o| storage::cached_object(&b, &initial.manifest.actor, "body", o)
                .unwrap()
                .is_some()
        ),
        "catalog's content descendants stay pinned during another lane's eviction"
    );
    assert_eq!(
        super::body_archive::apply(&b, 0, &initial.manifest, &catalog).unwrap(),
        400
    );
    assert_eq!(database::pending_bodies(&b).unwrap(), 0);
    storage::acknowledge(&a, 0, &initial.manifest).unwrap();
    assert!(storage::seal_bodies(&a, 0).unwrap().is_none());
    let mut next = db::NewArticle {
        guid: "new-archive".into(),
        url: Some("https://example.test/new".into()),
        title: "New".into(),
        author: None,
        summary: None,
        content_html: Some("<p>new content</p>".into()),
        body_text: "new content".into(),
        image_url: None,
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: None,
        source: "direct".into(),
    };
    db::upsert_article_with_feed(&a, feed, &next, false).unwrap();
    next.guid = "second-shared-new".into();
    db::upsert_article_with_feed(&a, feed, &next, false).unwrap();
    let mut incremental = storage::seal_bodies(&a, 0).unwrap().unwrap();
    let objects = storage::publication_objects(&a, &incremental.manifest).unwrap();
    let changed: Vec<_> = objects
        .iter()
        .filter(|o| !old_objects.contains(o))
        .collect();
    let bytes: usize = changed.iter().map(|o| o.size).sum();
    assert!(
        changed.len() < 8 && bytes < 65536,
        "a shared new body must not rewrite old payloads: objects={} bytes={bytes}",
        changed.len()
    );
    let before = database::checkpoint(&a).unwrap();
    let corrupt = (*changed[0]).clone();
    storage::retry_partial(&a, 0, &mut incremental, Some(corrupt.clone())).unwrap();
    assert_eq!(
        before,
        database::checkpoint(&a).unwrap(),
        "retry changes transport attempt only"
    );
    assert!(!storage::publication_objects(&a, &incremental.manifest)
        .unwrap()
        .contains(&corrupt));
    assert_eq!(
        super::body_archive::records(&a, &incremental.manifest)
            .unwrap()
            .len(),
        402
    );
    storage::acknowledge(&a, 0, &incremental.manifest).unwrap();
    storage::enqueue_gc(&a, 0, &initial.manifest, &incremental.manifest).unwrap();
    assert!(
        a.query_row("SELECT COUNT(*) FROM local_sync_gc", [], |r| r
            .get::<_, i64>(0))
            .unwrap()
            > 0
    );
    assert_eq!(database::pending_bodies(&b).unwrap(), 0);
}

#[test]
fn body_archive_missing_payload_is_pending_and_failed_install_rolls_back_batch() {
    let a = conn();
    let b = conn();
    seed(&a, &"a".repeat(32), "https://example.test/rss");
    database::initialize(&a, LIB).unwrap();
    database::initialize(&b, LIB).unwrap();
    exchange(&a, &b);
    let pending = storage::seal_bodies(&a, 0).unwrap().unwrap();
    for page in &pending.manifest.shards {
        let bytes = storage::cached_object(&a, &pending.manifest.actor, "body", page)
            .unwrap()
            .unwrap();
        storage::cache_object(&b, 0, &pending.manifest, page, &bytes).unwrap();
    }
    let catalog = super::body_archive::catalog(&b, &pending.manifest).unwrap();
    assert_eq!(
        super::body_archive::apply(&b, 0, &pending.manifest, &catalog)
            .err()
            .unwrap()
            .code,
        "localSyncPending"
    );
    assert_eq!(database::pending_bodies(&b).unwrap(), 1);
    for object in catalog.objects() {
        let bytes = storage::cached_object(&a, &pending.manifest.actor, "body", &object)
            .unwrap()
            .unwrap();
        storage::cache_object(&b, 0, &pending.manifest, &object, &bytes).unwrap();
    }
    b.execute_batch("CREATE TRIGGER reject_body BEFORE UPDATE OF content_id ON articles BEGIN SELECT RAISE(ABORT,'injected install failure'); END;").unwrap();
    assert!(super::body_archive::apply(&b, 0, &pending.manifest, &catalog).is_err());
    assert_eq!(database::pending_bodies(&b).unwrap(), 1);
    assert!(!b
        .query_row("SELECT applying FROM local_sync_control", [], |r| r
            .get::<_, bool>(0))
        .unwrap());
    b.execute_batch("DROP TRIGGER reject_body;").unwrap();
    assert_eq!(
        super::body_archive::apply(&b, 0, &pending.manifest, &catalog).unwrap(),
        1
    );
}

#[test]
fn body_archive_restart_releases_abandoned_pins_preserves_pending_and_rejects_stale_eviction() {
    let file = path("archive-reopen").with_extension("db");
    let c = db::open(&file).unwrap();
    seed(&c, &"a".repeat(32), "https://example.test/rss");
    database::initialize(&c, LIB).unwrap();
    let mut sealed = storage::seal_bodies(&c, 0).unwrap().unwrap();
    sealed.next_object = 1;
    storage::save_pending(&c, 0, &sealed).unwrap();
    let name = sealed.manifest.file_name().unwrap();
    let objects = storage::publication_objects(&c, &sealed.manifest).unwrap();
    c.execute("INSERT INTO local_sync_object_pins(owner,actor,kind,digest) VALUES('body-build:crashed',?1,'body',?2)",params![sealed.manifest.actor,objects[0].digest]).unwrap();
    drop(c);
    let c = db::open(&file).unwrap();
    let recovered = storage::seal_bodies(&c, 0).unwrap().unwrap();
    assert_eq!(recovered.manifest.file_name().unwrap(), name);
    assert_eq!(recovered.next_object, 1);
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM local_sync_object_pins", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    storage::evict_cache(&c, 0).unwrap();
    assert!(objects.iter().all(
        |o| storage::cached_object(&c, &sealed.manifest.actor, "body", o)
            .unwrap()
            .is_some()
    ));
    c.execute("UPDATE local_sync_control SET generation=1", [])
        .unwrap();
    assert_eq!(
        storage::evict_cache(&c, 0).err().unwrap().code,
        "staleLocalSync"
    );
    assert!(objects.iter().all(
        |o| storage::cached_object(&c, &sealed.manifest.actor, "body", o)
            .unwrap()
            .is_some()
    ));
    assert!(storage::acknowledge(&c, 0, &sealed.manifest).is_err());
    assert!(storage::pending(&c, "body", 0).unwrap().is_some());
    drop(c);
    std::fs::remove_file(file).unwrap();
}

#[test]
fn body_archive_discarded_attempt_does_not_skip_reseal_and_future_actor_sees_missing() {
    let a = conn();
    let b = conn();
    let (_, article) = seed(&a, &"a".repeat(32), "https://example.test/rss");
    database::initialize(&a, LIB).unwrap();
    let first = storage::seal_bodies(&a, 0).unwrap().unwrap();
    a.execute("DELETE FROM local_sync_pending", []).unwrap();
    let second = storage::seal_bodies(&a, 0).unwrap().unwrap();
    assert!(
        second.manifest.publication > first.manifest.publication,
        "unacknowledged attempt cannot set the idle-success signature"
    );
    storage::acknowledge(&a, 0, &second.manifest).unwrap();
    db::set_read(&a, article, true).unwrap();
    super::logical::drain_local(&a).unwrap();
    let receipt = storage::seal_bodies(&a, 0).unwrap().unwrap();
    assert!(
        receipt.manifest.publication > second.manifest.publication,
        "the new body hold needs its own receipt publication"
    );
    storage::acknowledge(&a, 0, &receipt.manifest).unwrap();
    let released = storage::seal_bodies(&a, 0).unwrap().unwrap();
    assert!(released.manifest.shards.is_empty());
    storage::acknowledge(&a, 0, &released.manifest).unwrap();
    database::initialize(&b, LIB).unwrap();
    exchange(&a, &b);
    let id = b
        .query_row("SELECT id FROM articles", [], |r| r.get::<_, i64>(0))
        .unwrap();
    db::set_starred(&b, id, true).unwrap();
    assert_eq!(database::pending_bodies(&b).unwrap(),1,"a previously unknown device cannot revive physically collected content merely by starring metadata");
    let empty = super::body_archive::catalog(&a, &released.manifest).unwrap();
    assert_eq!(
        super::body_archive::apply(&b, 0, &released.manifest, &empty).unwrap(),
        0
    );
    assert_eq!(database::pending_bodies(&b).unwrap(), 1);
    exchange(&a, &b);
    let repinned = storage::seal_bodies(&a, 0).unwrap().unwrap();
    assert!(
        !repinned.manifest.shards.is_empty(),
        "a device still holding local bytes can fulfill the late protection"
    );
}

#[test]
#[ignore = "explicit 50k body capacity measurement; pure SQLite and owned file only"]
fn portable_fifty_thousand_four_kib_bodies_capacity() {
    let c = conn();
    let (feed, article) = seed(&c, &"a".repeat(32), "https://example.test/rss");
    let text = "content ".repeat(512);
    let html = format!("<p>{text}</p>");
    let input = db::PortableContentInput {
        content_html: Some(html),
        body_text: text,
        extraction_source: "rss".into(),
        fulltext_extracted: false,
        url_norm: None,
    };
    let (content, _, _) = db::portable_ensure_content(&c, &input, Some(article)).unwrap();
    c.execute(
        "UPDATE articles SET content_id=?1,rss_content_id=?1",
        [content],
    )
    .unwrap();
    c.execute("WITH RECURSIVE n(x) AS(SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<49999) INSERT INTO articles(feed_id,guid,title,published_at,content_id,rss_content_id,is_starred) SELECT ?1,'body-'||x,'Saved body','2026-11-09T00:00:00Z',?2,?2,1 FROM n",params![feed,content]).unwrap();
    let file = path("50k-bodies");
    println!("BODY_PERF phase=export start");
    let start = std::time::Instant::now();
    let exported = backup::export(&c, &file).unwrap();
    println!(
        "BODY_PERF export_ms={} bytes={}",
        start.elapsed().as_millis(),
        exported.size_bytes
    );
    println!("BODY_PERF phase=preview start");
    let start = std::time::Instant::now();
    let preview = backup::preview(&file).unwrap();
    assert_eq!(preview.articles, 50000);
    assert_eq!(preview.missing_bodies, 0);
    println!("BODY_PERF preview_ms={}", start.elapsed().as_millis());
    std::fs::remove_file(file).unwrap();
}

#[test]
fn body_archive_two_thousand_distinct_bodies_seal_has_bounded_prepare() {
    const ARTICLES: usize = 2_048;
    let file = path("archive-bounded-2k").with_extension("db");
    let c = db::open(&file).unwrap();
    let (feed, first) = seed(&c, &"a".repeat(32), "https://example.test/rss");
    let tx = c.unchecked_transaction().unwrap();
    for n in 0..ARTICLES {
        let text = format!("{n:08} {}", "content ".repeat(511));
        let html = format!("<p>{text}</p>");
        let identity = db::portable_content_identity("rss", Some(&html), &text);
        tx.execute(
            "INSERT INTO contents(identity,content_html,body_text,extraction_source)
             VALUES(?1,?2,?3,'rss')",
            params![identity, html, text],
        )
        .unwrap();
        let content = tx.last_insert_rowid();
        if n == 0 {
            tx.execute(
                "UPDATE articles SET content_id=?1,rss_content_id=?1,is_starred=1
                 WHERE id=?2",
                params![content, first],
            )
            .unwrap();
        } else {
            tx.execute(
                "INSERT INTO articles(
                     feed_id,guid,title,published_at,is_starred,content_id,rss_content_id
                 ) VALUES(?1,?2,'Saved','2026-11-09T00:00:00Z',1,?3,?3)",
                params![feed, format!("bounded-2k-{n}"), content],
            )
            .unwrap();
        }
    }
    tx.commit().unwrap();
    database::initialize(&c, LIB).unwrap();
    assert!(!super::logical::query_requires_logical_state(&c).unwrap());
    db::set_starred_with_enqueue(&c, first, false).unwrap();
    assert!(super::logical::query_requires_logical_state(&c).unwrap());
    assert!(!super::body_archive::body_rows_require_logical_state(&c).unwrap());

    let started = std::time::Instant::now();
    let sealed = storage::seal_bodies(&c, 0).unwrap();
    let elapsed = started.elapsed();
    assert!(sealed.is_some());
    assert!(
        elapsed < std::time::Duration::from_secs(60),
        "2k distinct-body seal took {elapsed:?}"
    );
}

/// Ignored capacity fixture only. Drop always stops/joins the owned foreground
/// writer and persists phase/operation summaries, even when the measured code panics.
struct CapacityForeground {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    worker: Option<std::thread::JoinHandle<usize>>,
    path: std::path::PathBuf,
    feed: i64,
    first: i64,
    errors: usize,
}
impl CapacityForeground {
    fn start(path: &std::path::Path, feed: i64, first: i64) -> Self {
        Self::start_phase(path, feed, first, 0)
    }
    fn start_phase(path: &std::path::Path, feed: i64, first: i64, number: usize) -> Self {
        use std::sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Arc,
        };
        let stop = Arc::new(AtomicBool::new(false));
        let phase = Arc::new(AtomicUsize::new(number));
        let worker_stop = stop.clone();
        let worker_phase = phase.clone();
        let file = path.to_path_buf();
        let ready = Arc::new(std::sync::Barrier::new(2));
        let worker_ready = ready.clone();
        let worker = std::thread::spawn(move || {
            use std::io::Write;
            let ui = db::open(&file).unwrap();
            ui.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
            let sample_path = file.with_extension(format!("phase-{number}.foreground.ndjson"));
            let summary_path =
                file.with_extension(format!("phase-{number}.foreground-summary.json"));
            println!("ARCHIVE_PERF foreground_samples={} foreground_summary={} markall_scope=one_article bulk_scope=three_articles cadence_ms=50",sample_path.display(),summary_path.display());
            let mut log = std::io::BufWriter::new(std::fs::File::create(&sample_path).unwrap());
            let mut groups = std::collections::BTreeMap::<(usize, &str), Vec<u64>>::new();
            let mut flushes = Vec::new();
            let mut errors = Vec::new();
            let clock = std::time::Instant::now();
            let mut sequence = 0;
            worker_ready.wait();
            while !worker_stop.load(Ordering::Acquire) {
                let phase = worker_phase.load(Ordering::Acquire);
                let start = std::time::Instant::now();
                let operation = match sequence % 4 {
                    0 => "read",
                    1 => "star",
                    2 => "bulk3",
                    _ => "markall1",
                };
                let outcome = match sequence % 4 {
                    0 => (|| -> crate::error::AppResult<()> {
                        let title: String =
                            ui.query_row("SELECT title FROM articles WHERE id=?1", [first], |r| {
                                r.get(0)
                            })?;
                        assert!(!title.is_empty());
                        db::set_read_with_enqueue(&ui, first, (sequence / 4) % 2 == 0)?;
                        Ok(())
                    })(),
                    1 => db::set_starred_with_enqueue(&ui, first + 3, (sequence / 4) % 2 == 0),
                    2 => db::set_read_bulk_with_enqueue(&ui, &[first, first + 1, first + 2], false)
                        .map(|_| ()),
                    _ => db::mark_all_read_with_enqueue(
                        &ui,
                        Some(feed),
                        None,
                        false,
                        Some(4_102_444_799_000),
                        None,
                    )
                    .map(|_| ()),
                };
                let micros = start.elapsed().as_micros() as u64;
                groups.entry((phase, operation)).or_default().push(micros);
                let error = outcome
                    .err()
                    .map(|e| json!({"code":e.code,"message":e.message}));
                if let Some(error) = &error {
                    errors.push(json!({"phase":phase,"operation":operation,"error":error}));
                }
                let record = json!({"sequence":sequence,"seconds":clock.elapsed().as_secs_f64(),"phase":phase,"operation":operation,"operation_us":micros,"error":error});
                let flush_start = std::time::Instant::now();
                writeln!(log, "{record}").unwrap();
                log.flush().unwrap();
                flushes.push(flush_start.elapsed().as_micros() as u64);
                sequence += 1;
                if error.is_some() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            let summary:Vec<_>=groups.into_iter().map(|((phase,operation),mut values)|{values.sort_unstable();json!({"phase":phase,"operation":operation,"samples":values.len(),"p50_us":values[values.len()/2],"p95_us":values[values.len()*95/100],"max_us":values.last()})}).collect();
            flushes.sort_unstable();
            let output = json!({"groups":summary,"errors":errors,"log_write_flush":{"samples":flushes.len(),"p95_us":flushes.get(flushes.len()*95/100),"max_us":flushes.last()},"writer_stopped":true});
            std::fs::write(&summary_path, serde_json::to_vec_pretty(&output).unwrap()).unwrap();
            println!("ARCHIVE_PERF foreground_summary={output}");
            errors.len()
        });
        ready.wait();
        Self {
            stop,
            worker: Some(worker),
            path: path.to_path_buf(),
            feed,
            first,
            errors: 0,
        }
    }
    fn phase(&mut self, phase: usize) {
        let errors = self.stop();
        let mut next = Self::start_phase(&self.path, self.feed, self.first, phase);
        next.errors = errors;
        *self = next;
    }
    fn stop(&mut self) -> usize {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        self.errors += self
            .worker
            .take()
            .map(|worker| worker.join().unwrap_or(1))
            .unwrap_or(0);
        self.errors
    }
}

fn capacity_foreground_max_us(path: &std::path::Path, phases: &[usize]) -> u64 {
    phases
        .iter()
        .map(|phase| {
            let summary_path =
                path.with_extension(format!("phase-{phase}.foreground-summary.json"));
            let summary: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&summary_path).unwrap()).unwrap();
            summary["groups"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|group| group["max_us"].as_u64())
                .max()
                .unwrap_or(0)
        })
        .max()
        .unwrap_or(0)
}

impl Drop for CapacityForeground {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[test]
#[ignore = "explicit reserved 10k distinct-body disk/latency measurement (50k capacity remains unverified); no HTTP or native UI"]
fn body_archive_ten_thousand_distinct_four_kib_capacity_and_foreground_latency() {
    use std::time::{Duration, Instant};
    let file = path("archive-10k").with_extension("db");
    let c = db::open(&file).unwrap();
    c.busy_timeout(Duration::from_secs(5)).unwrap();
    let (feed, first) = seed(&c, &"a".repeat(32), "https://example.test/rss");
    println!("ARCHIVE_PERF phase=fixture path={}", file.display());
    let tx = c.unchecked_transaction().unwrap();
    for n in 0..10000 {
        let text = format!("{n:08} {}", "content ".repeat(511));
        let html = format!("<p>{text}</p>");
        let identity = db::portable_content_identity("rss", Some(&html), &text);
        tx.execute("INSERT INTO contents(identity,content_html,body_text,extraction_source) VALUES(?1,?2,?3,'rss')",params![identity,html,text]).unwrap();
        let content = tx.last_insert_rowid();
        if n == 0 {
            tx.execute("UPDATE articles SET content_id=?1,rss_content_id=?1,is_starred=1,published_at='2100-01-01T00:00:00Z' WHERE id=?2",params![content,first]).unwrap();
        } else {
            tx.execute("INSERT INTO articles(feed_id,guid,title,published_at,is_starred,content_id,rss_content_id) VALUES(?1,?2,'Saved','2026-11-09T00:00:00Z',1,?3,?3)",params![feed,format!("distinct-{n}"),content]).unwrap();
        }
    }
    tx.execute(
        "UPDATE articles SET is_starred=1,rss_content_id=content_id WHERE id=?1",
        [first],
    )
    .unwrap();
    tx.commit().unwrap();
    let mut foreground = CapacityForeground::start(&file, feed, first);
    foreground.phase(1);
    println!("ARCHIVE_PERF phase=initialize start");
    let start = Instant::now();
    database::initialize(&c, LIB).unwrap();
    println!("ARCHIVE_PERF initialize_ms={}", start.elapsed().as_millis());
    foreground.phase(7);
    println!("ARCHIVE_PERF phase=state_seal start");
    let start = Instant::now();
    let state = storage::seal_state(&c, 0).unwrap().unwrap();
    println!(
        "ARCHIVE_PERF state_seal_ms={} state_bytes={} state_objects={}",
        start.elapsed().as_millis(),
        state.manifest.shards.iter().map(|s| s.size).sum::<usize>(),
        state.manifest.shards.len()
    );
    foreground.phase(8);
    println!("ARCHIVE_PERF phase=state_ack start");
    let start = Instant::now();
    storage::acknowledge(&c, 0, &state.manifest).unwrap();
    println!("ARCHIVE_PERF state_ack_ms={}", start.elapsed().as_millis());
    drop(state);
    foreground.phase(2);
    println!("ARCHIVE_PERF phase=seal start");
    let start = Instant::now();
    let initial = storage::seal_bodies(&c, 0).unwrap().unwrap();
    println!("ARCHIVE_PERF seal_ms={}", start.elapsed().as_millis());
    let objects = storage::publication_objects(&c, &initial.manifest).unwrap();
    let bytes: usize = objects.iter().map(|o| o.size).sum();
    let catalog = super::body_archive::catalog(&c, &initial.manifest).unwrap();
    assert_eq!(catalog.entries.len(), 10000);
    assert_eq!(catalog.contents.len(), 10000);
    println!("ARCHIVE_PERF payload_and_catalog_bytes={bytes} objects={} catalog_pages={} manifest_bytes={}",objects.len(),initial.manifest.shards.len(),serde_json::to_vec(&initial.manifest).unwrap().len());
    foreground.phase(3);
    println!("ARCHIVE_PERF phase=ack start");
    let start = Instant::now();
    storage::acknowledge(&c, 0, &initial.manifest).unwrap();
    println!("ARCHIVE_PERF ack_ms={}", start.elapsed().as_millis());
    let text = "one new body ".repeat(300);
    let new = db::NewArticle {
        guid: "late-distinct".into(),
        url: Some("https://example.test/late".into()),
        title: "Late".into(),
        author: None,
        summary: None,
        content_html: Some(format!("<p>{text}</p>")),
        body_text: text,
        image_url: None,
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: None,
        source: "direct".into(),
    };
    db::upsert_article_with_feed(&c, feed, &new, false).unwrap();
    foreground.phase(4);
    println!("ARCHIVE_PERF phase=delta start");
    let start = Instant::now();
    let next = storage::seal_bodies(&c, 0).unwrap().unwrap();
    let changed: Vec<_> = storage::publication_objects(&c, &next.manifest)
        .unwrap()
        .into_iter()
        .filter(|o| !objects.contains(o))
        .collect();
    println!(
        "ARCHIVE_PERF delta_ms={} objects={} bytes={} manifest_bytes={}",
        start.elapsed().as_millis(),
        changed.len(),
        changed.iter().map(|o| o.size).sum::<usize>(),
        serde_json::to_vec(&next.manifest).unwrap().len()
    );
    assert!(changed.len() < 8);
    assert!(changed.iter().map(|o| o.size).sum::<usize>() < 4 * 1024 * 1024);
    foreground.phase(5);
    println!("ARCHIVE_PERF phase=portable_export start");
    let portable = path("archive-10k-portable");
    let start = Instant::now();
    let exported = backup::export(&c, &portable).unwrap();
    println!(
        "ARCHIVE_PERF portable_export_ms={} bytes={}",
        start.elapsed().as_millis(),
        exported.size_bytes
    );
    foreground.phase(6);
    println!("ARCHIVE_PERF phase=portable_preview start");
    let start = Instant::now();
    assert_eq!(backup::preview(&portable).unwrap().articles, 10001);
    println!(
        "ARCHIVE_PERF portable_preview_ms={}",
        start.elapsed().as_millis()
    );
    let source_errors = foreground.stop();
    let target_file = path("state-apply-10k").with_extension("db");
    let target = db::open(&target_file).unwrap();
    let (target_feed, target_first) = seed(&target, &"a".repeat(32), "https://example.test/rss");
    target
        .execute(
            "UPDATE articles SET published_at='2100-01-01T00:00:00Z' WHERE id=?1",
            [target_first],
        )
        .unwrap();
    for n in 1..=3 {
        target
            .execute(
                "INSERT INTO articles(feed_id,guid,title) VALUES(?1,?2,'Saved')",
                params![target_feed, format!("distinct-{n}")],
            )
            .unwrap();
    }
    database::initialize(&target, LIB).unwrap();
    let source = database::checkpoint(&c).unwrap();
    // The whole-library state apply below targets a NEW library through the
    // merge_apply test entry; it is not the portable restore path and not the
    // production sync entry. Production restore is backup::restore with its own
    // exclusive lock and capture=0 (src/App.tsx:393 `inert={libraryRestoreBusy}`,
    // :442 the blocking overlay). Production sync state application is
    // runtime::pull_state_candidate (begin_receive -> stage_page -> preflight ->
    // accept). Both whole-library paths require a stable logical revision while
    // candidates fold (`prepare_work`): a permanent 20-writes-per-second load
    // makes them fail retryably with `localSyncPending`, and they converge once
    // the writer stops (see
    // `production_receive_path_reports_retryable_pending_and_converges_after_the_writer_stops`
    // for the production entry and
    // `merge_apply_reports_retryable_pending_under_concurrent_writes` for this
    // test entry). The source half above keeps its continuous foreground writer.
    println!(
        "ARCHIVE_PERF phase=state_first_apply target={} start",
        target_file.display()
    );
    let start = Instant::now();
    database::merge_apply(&target, 0, &source).unwrap();
    println!(
        "ARCHIVE_PERF state_first_apply_ms={}",
        start.elapsed().as_millis()
    );
    println!(
        "ARCHIVE_PERF retention_drain_ms={}",
        super::logical::retention_drain_take_ms()
    );
    drop(source);
    c.execute(
        "UPDATE articles SET title='Changed metadata' WHERE id=?1",
        [first + 10],
    )
    .unwrap();
    let source = database::checkpoint(&c).unwrap();
    println!("ARCHIVE_PERF phase=state_metadata_apply start");
    let start = Instant::now();
    database::merge_apply(&target, 0, &source).unwrap();
    println!(
        "ARCHIVE_PERF state_metadata_apply_ms={}",
        start.elapsed().as_millis()
    );
    drop(source);
    assert_eq!(source_errors,0,"foreground failures are retained per phase in NDJSON/summary evidence; both databases retained on failure");
    let source_max_us = capacity_foreground_max_us(&file, &[0, 1, 2, 3, 4, 5, 6, 7, 8]);
    assert!(
        source_max_us <= 2_000_000,
        "a 10k source foreground operation exceeded 2s: {source_max_us}us"
    );
    drop(target);
    drop(c);
    std::fs::remove_file(portable).unwrap();
    std::fs::remove_file(file).unwrap();
    std::fs::remove_file(target_file).unwrap();
}

#[test]
fn small_delta_alias_join_obeys_field_winners_and_rejects_same_stamp_corruption_atomically() {
    let a = conn();
    let b = conn();
    let (_, aid) = seed(&a, &"f".repeat(32), "https://example.test/rss");
    let (_, bid) = seed(&b, &"a".repeat(32), "https://example.test/rss");
    database::initialize(&a, LIB).unwrap();
    database::initialize(&b, LIB).unwrap();
    exchange(&a, &b);
    db::set_read(&a, aid, true).unwrap();
    db::set_starred(&b, bid, true).unwrap();
    let mut delta = database::checkpoint(&b).unwrap();
    delta.fields.retain(|key, _| key.ends_with("/star"));
    assert_eq!(database::merge_small(&a, 0, &delta).unwrap(), Some(true));
    assert_eq!(flags(&a), (true, true));
    let before = database::checkpoint(&a).unwrap();
    let key = delta.fields.keys().next().unwrap().clone();
    delta.fields.get_mut(&key).unwrap().value =
        crate::webdav_transport::FieldValue::Set(json!(false));
    assert!(database::merge_small(&a, 0, &delta).is_err());
    assert_eq!(before, database::checkpoint(&a).unwrap());
    assert_eq!(flags(&a), (true, true));
}

fn fanout_keys(c: &Connection) -> std::collections::BTreeSet<String> {
    let mut stmt = c
        .prepare("SELECT key FROM local_sync_work WHERE job=''")
        .unwrap();
    stmt.query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}
fn added_keys(c: &Connection, before: &std::collections::BTreeSet<String>) -> Vec<String> {
    let mut added = fanout_keys(c)
        .difference(before)
        .cloned()
        .collect::<Vec<_>>();
    added.sort();
    added
}
/// The fan-out table on logical::capture_sql has two implementations: the
/// capture triggers and touch_field. This pins them together, so removing a
/// rule from one side (for example the source-wide expansion an article
/// metadata edit used to trigger) cannot silently survive in the other. Each
/// case runs on its own initialized library, so a case can never pass because
/// a previous case left the queue in a convenient state.
#[test]
fn capture_trigger_and_explicit_patch_enqueue_the_same_fanout_keys() {
    use crate::local_sync::logical;
    use crate::webdav_transport::{FieldPatch, FieldValue};

    let library = |uid: &str| {
        let c = conn();
        let seeded = seed(&c, uid, "https://example.test/rss");
        database::initialize(&c, LIB).unwrap();
        logical::drain_local(&c).unwrap();
        assert!(
            c.query_row("SELECT COUNT(*) FROM local_sync_work", [], |r| r
                .get::<_, i64>(0))
                .unwrap()
                == 0,
            "the fixture must start settled"
        );
        (c, seeded.0, seeded.1)
    };

    // A: an ordinary metadata value change is not a source-wide event, and the
    // explicit (capture_patch) path must enqueue exactly the same key.
    {
        let (c, _, article) = library(&"a".repeat(32));
        let before = fanout_keys(&c);
        c.execute("UPDATE articles SET title='Changed' WHERE id=?1", [article])
            .unwrap();
        let trigger_keys = added_keys(&c, &before);
        let entry = format!(
            "a/{}",
            super::model::entry_key(&"a".repeat(32), "real-guid")
        );
        assert_eq!(trigger_keys, vec![entry.clone()], "{trigger_keys:?}");
    }
    {
        let (c, _, article) = library(&"a".repeat(32));
        let before = fanout_keys(&c);
        let entry = format!(
            "a/{}",
            super::model::entry_key(&"a".repeat(32), "real-guid")
        );
        let mut meta = database::article_meta(&c, article, &"a".repeat(32)).unwrap();
        meta.title = "Changed".into();
        logical::capture_patch(
            &c,
            &FieldPatch::from([(
                format!("{entry}/meta"),
                FieldValue::Set(serde_json::to_value(&meta).unwrap()),
            )]),
        )
        .unwrap();
        let patch_keys = added_keys(&c, &before);
        assert_eq!(patch_keys, vec![entry], "{patch_keys:?}");
    }

    // B: rewriting an article's own identity in place is not a wire operation —
    // the validator rejects it and, importantly, leaves no work behind.
    {
        let (c, _, article) = library(&"a".repeat(32));
        let before = fanout_keys(&c);
        let entry = format!(
            "a/{}",
            super::model::entry_key(&"a".repeat(32), "real-guid")
        );
        let mut meta = database::article_meta(&c, article, &"a".repeat(32)).unwrap();
        meta.guid = "renamed-guid".into();
        let rejected = logical::capture_patch(
            &c,
            &FieldPatch::from([(
                format!("{entry}/meta"),
                FieldValue::Set(serde_json::to_value(&meta).unwrap()),
            )]),
        );
        assert!(
            rejected.is_err(),
            "an in-place GUID rewrite must be rejected"
        );
        assert!(
            added_keys(&c, &before).is_empty(),
            "a rejected patch writes no work"
        );
    }

    // C: a feed URL change keeps the source expansion and both URL aliases.
    {
        let (c, feed, _) = library(&"a".repeat(32));
        let before = fanout_keys(&c);
        c.execute(
            "UPDATE feeds SET feed_url='https://example.test/rss2' WHERE id=?1",
            [feed],
        )
        .unwrap();
        let keys = added_keys(&c, &before);
        assert!(
            keys.iter()
                .any(|key| key == "source/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            "{keys:?}"
        );
        assert!(
            keys.iter().any(|key| key == "url/https://example.test/rss"),
            "{keys:?}"
        );
        assert!(
            keys.iter()
                .any(|key| key == "url/https://example.test/rss2"),
            "{keys:?}"
        );
    }

    // C again: retiring a source still fans out to its members and its group.
    {
        let (c, feed, _) = library(&"a".repeat(32));
        db::delete_feed(&c, feed).unwrap();
        let keys = fanout_keys(&c);
        assert!(
            keys.iter()
                .any(|key| key == "source/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            "{keys:?}"
        );
    }

    // D: folder membership expands the source and both groups.
    {
        let (c, feed, _) = library(&"a".repeat(32));
        let before = fanout_keys(&c);
        c.execute("UPDATE feeds SET folder_id=NULL WHERE id=?1", [feed])
            .unwrap();
        let keys = added_keys(&c, &before);
        assert!(
            keys.iter()
                .any(|key| key == "source/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            "{keys:?}"
        );
        assert!(
            keys.iter()
                .any(|key| key == "group/22222222222222222222222222222222"),
            "{keys:?}"
        );
    }

    // E: deleting the group expands the group scope.
    {
        let (c, feed, _) = library(&"a".repeat(32));
        c.execute("UPDATE feeds SET folder_id=NULL WHERE id=?1", [feed])
            .unwrap();
        logical::drain_local(&c).unwrap();
        let before = fanout_keys(&c);
        c.execute("DELETE FROM folders", []).unwrap();
        let keys = added_keys(&c, &before);
        assert!(
            keys.iter()
                .any(|key| key == "group/22222222222222222222222222222222"),
            "{keys:?}"
        );
    }
}

/// Every key the capture triggers or touch_field can enqueue must be reachable
/// by the picker. A prefix bucket that misses a whole family (as an \`s/\` bucket
/// misses \`source/\`) leaves the row queued forever while the slice loop reports
/// "nothing to do" — the entity then never materializes and no error is raised.
/// This walks the real picker over one row of every family.
#[test]
fn every_enqueued_work_key_is_reachable_by_the_picker() {
    use crate::local_sync::logical;
    let c = conn();
    database::initialize(&c, LIB).unwrap();
    logical::drain_local(&c).unwrap();
    c.execute("DELETE FROM local_sync_work", []).unwrap();
    let entry = |n: u8| format!("a/{}", (b'a' + n) as char).repeat(2) + &"c".repeat(62);
    let keys = vec![
        "p".to_owned(),
        format!("g/{}", "1".repeat(32)),
        format!("s/{}", "2".repeat(32)),
        entry(0),
        format!("group/{}", "3".repeat(32)),
        format!("source/{}", "4".repeat(32)),
        "url/https://example.test/rss".to_owned(),
        "cleanup-feed/7".to_owned(),
        format!("retention/{}/read", entry(1)),
    ];
    for key in &keys {
        c.execute(
            "INSERT INTO local_sync_work(job,key,revision) VALUES('',?1,1)",
            [key],
        )
        .unwrap();
    }
    let mut picked = Vec::new();
    while let Some(row) = logical::pick_work(&c).unwrap() {
        picked.push(row.key.clone());
        c.execute("DELETE FROM local_sync_work WHERE key=?1", [&row.key])
            .unwrap();
    }
    picked.sort();
    let mut expected = keys.clone();
    expected.sort();
    assert_eq!(picked, expected, "the picker must reach every queued key");
    assert!(logical::pick_work(&c).unwrap().is_none());
}

/// A new folder is captured as \`g/<gid>/*\` fields; its entity key is the only
/// entry point that materializes it into \`folders\`. Before the \`g/\` bucket
/// existed that key was unpickable, so the folder never appeared.
#[test]
fn folder_created_and_renamed_through_capture_are_picked_and_materialized() {
    use crate::local_sync::logical;
    let c = conn();
    database::initialize(&c, LIB).unwrap();
    logical::drain_local(&c).unwrap();
    let gid = "d".repeat(32);
    c.execute(
        "INSERT INTO folders(origin,sync_id,name,layout) VALUES('local',?1,'Fresh','article')",
        [&gid],
    )
    .unwrap();
    assert!(
        fanout_keys(&c).contains(&format!("g/{gid}")),
        "creating a folder must queue its entity key: {:?}",
        fanout_keys(&c)
    );
    let mut slices = 0;
    while logical::resume_slice(&c).unwrap() {
        slices += 1;
        assert!(
            slices < 16,
            "folder materialization must be bounded: {:?}",
            fanout_keys(&c)
        );
    }
    // The projection wrote the logical name back onto the physical row.
    c.execute("UPDATE folders SET name='Renamed' WHERE sync_id=?1", [&gid])
        .unwrap();
    assert!(fanout_keys(&c).contains(&format!("g/{gid}")));
    while logical::resume_slice(&c).unwrap() {}
    assert_eq!(
        c.query_row("SELECT name FROM folders WHERE sync_id=?1", [&gid], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "Renamed"
    );
    assert!(logical::settled(&c).unwrap());
}

/// Capacity counterpart: on a real 50k-member source a metadata edit must
/// enqueue O(1) work and the slice loop must retire it promptly.
#[test]
fn article_meta_change_on_fifty_thousand_member_source_queues_only_direct_work() {
    use crate::local_sync::logical;
    let file = path("fanout-50k").with_extension("db");
    let c = db::open(&file).unwrap();
    c.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
    let (feed, first) = seed(&c, &"a".repeat(32), "https://example.test/rss");
    let tx = c.unchecked_transaction().unwrap();
    tx.execute(
        "WITH RECURSIVE n(x) AS(SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<49999)
         INSERT INTO articles(feed_id,guid,title,is_read)
         SELECT ?1,'fan-'||x,'Fan',1 FROM n",
        [feed],
    )
    .unwrap();
    tx.commit().unwrap();
    let start = std::time::Instant::now();
    database::initialize(&c, LIB).unwrap();
    println!(
        "ARCHIVE_PERF fanout_initialize_ms={}",
        start.elapsed().as_millis()
    );
    logical::drain_local(&c).unwrap();
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM local_sync_fields WHERE key LIKE 'a/%/meta'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        50_000
    );

    let before = fanout_keys(&c);
    let start = std::time::Instant::now();
    c.execute(
        "UPDATE articles SET title='Changed metadata' WHERE id=?1",
        [first],
    )
    .unwrap();
    let update_ms = start.elapsed().as_millis();
    let keys = added_keys(&c, &before);
    println!("ARCHIVE_PERF fanout_update_ms={update_ms} fanout_keys={keys:?}");
    assert!(
        keys.iter().all(|key| !key.starts_with("source/")),
        "50k member source must not be expanded by a metadata edit: {keys:?}"
    );
    assert_eq!(keys.len(), 1, "{keys:?}");

    let mut slices = 0;
    while logical::resume_slice(&c).unwrap() {
        slices += 1;
        assert!(
            slices < 64,
            "a single metadata edit must not need a source-wide drain"
        );
    }
    println!("ARCHIVE_PERF fanout_drain_slices={slices}");
    assert!(logical::settled(&c).unwrap());
    drop(c);
    std::fs::remove_file(&file).unwrap();
}

/// The production picker runs on the primary-key index; no slice may sort the
/// whole queue. Shapes match a materialized 20k and a 50k library.
#[test]
fn work_picker_uses_primary_key_seeks_without_a_sort() {
    use crate::local_sync::logical;
    let c = conn();
    let (_feed, _) = seed(&c, &"a".repeat(32), "https://example.test/rss");
    database::initialize(&c, LIB).unwrap();
    logical::drain_local(&c).unwrap();
    let plan = |sql: &str, binds: &[&dyn rusqlite::ToSql]| {
        let mut stmt = c.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        let rows = stmt
            .query_map(binds, |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        rows.join("\n")
    };
    let job = String::new();
    for members in [20_000i64, 50_000] {
        c.execute("DELETE FROM local_sync_work", []).unwrap();
        c.execute(
            "WITH RECURSIVE n(x) AS(SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<?1)
             INSERT INTO local_sync_work(job,key,revision)
             SELECT '',printf('a/%064x',x),1 FROM n",
            [members],
        )
        .unwrap();
        c.execute(
            "WITH RECURSIVE n(x) AS(SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<?1)
             INSERT INTO local_sync_work(job,key,revision)
             SELECT '',printf('retention/a/%064x/read',x),1 FROM n",
            [members / 2],
        )
        .unwrap();
        for (start, end) in logical::WORK_BUCKETS {
            let text = plan(logical::PICK_RANGE_SQL, &[&job, &start, &end]);
            assert!(
                !text.to_uppercase().contains("TEMP B-TREE"),
                "bucket {start}: {text}"
            );
            assert!(
                text.contains("sqlite_autoindex_local_sync_work"),
                "bucket {start} must seek the queue primary key: {text}"
            );
        }
        let exact = plan(logical::PICK_EXACT_SQL, &[&job, &"p"]);
        assert!(!exact.to_uppercase().contains("TEMP B-TREE"), "{exact}");
        assert!(
            exact.contains("sqlite_autoindex_local_sync_work"),
            "{exact}"
        );
        let payload = json!([["", format!("a/{}", "0".repeat(64)), 1]]).to_string();
        let text = plan(
            logical::DELETE_COMPLETED_SQL,
            &[&job, &"a/", &"a0", &payload],
        );
        assert!(!text.to_uppercase().contains("TEMP B-TREE"), "{text}");
        assert!(
            text.contains("sqlite_autoindex_local_sync_work"),
            "batch retire must stay on the primary key: {text}"
        );
    }
    // The batch delete still applies the exact (job,key,revision) CAS: a row
    // touched after being picked keeps its newer revision and stays queued.
    c.execute("DELETE FROM local_sync_work", []).unwrap();
    c.execute(
        "INSERT INTO local_sync_work(job,key,revision) VALUES('','a/keep',1),('','a/touched',1),('','a/other',1)",
        [],
    )
    .unwrap();
    c.execute(
        "UPDATE local_sync_work SET revision=2 WHERE key='a/touched'",
        [],
    )
    .unwrap();
    let payload = json!([["", "a/keep", 1], ["", "a/touched", 1]]).to_string();
    c.execute(
        logical::DELETE_COMPLETED_SQL,
        params!["", "a/keep", "a/touched", payload],
    )
    .unwrap();
    let remaining = fanout_keys(&c).into_iter().collect::<Vec<_>>();
    assert_eq!(
        remaining,
        vec!["a/other".to_owned(), "a/touched".to_owned()]
    );
}

/// Whole-library state application requires a stable logical revision while its
/// candidates fold. A local write inside that window must surface as the
/// retryable \`localSyncPending\` — not a partial accept and not a corrupt
/// library — and a retry on a quiet library must succeed and be idempotent.
/// Production treats this code as retryable (runtime.rs reschedules it after 5s
/// without counting a failure); this pins that contract instead of pretending
/// unlimited concurrent editing is supported. The drift is injected with a
/// trigger so the case is deterministic rather than a timing race.
#[test]
fn merge_apply_reports_retryable_pending_under_concurrent_writes() {
    use crate::local_sync::logical;
    let a = conn();
    let (_, article) = seed(&a, &"a".repeat(32), "https://example.test/rss");
    database::initialize(&a, LIB).unwrap();
    for n in 1..=300 {
        a.execute(
            "INSERT INTO articles(feed_id,guid,title,is_read)
             SELECT feed_id,?1,'Bulk',1 FROM articles WHERE id=?2",
            params![format!("bulk-{n:04}"), article],
        )
        .unwrap();
    }
    logical::drain_local(&a).unwrap();
    let state = database::checkpoint(&a).unwrap();
    assert!(
        state.fields.len() > 256,
        "the fixture must need several candidate pages: {}",
        state.fields.len()
    );

    let b = conn();
    let (_, other) = seed(&b, &"a".repeat(32), "https://example.test/rss");
    database::initialize(&b, LIB).unwrap();
    let before = logical::revision(&b).unwrap();
    // A local write landing while the candidate folds moves logical_revision,
    // which is exactly what prepare_work refuses to tolerate. The fold window is
    // prepare_work's own queue maintenance. touch() inserts a work row first and
    // then hits the ON CONFLICT UPDATE path, so an AFTER INSERT trigger never
    // fires there; the update trigger is the deterministic fold-window probe.
    // (Staging a candidate page is a different window: preflight captures its
    // revision after staging, and per-field stamps already order the older
    // candidate against newer local writes.)
    b.execute_batch(
        "CREATE TRIGGER test_drift_during_fold AFTER UPDATE ON local_sync_work
         BEGIN UPDATE local_sync_control SET logical_revision=logical_revision+1 WHERE id=1; END;",
    )
    .unwrap();
    let rejected = database::merge_apply(&b, 0, &state);
    b.execute_batch("DROP TRIGGER test_drift_during_fold;")
        .unwrap();
    let code = rejected.as_ref().err().map(|error| error.code.clone());
    assert_eq!(
        code.as_deref(),
        Some("localSyncPending"),
        "a write during the fold must be reported as retryable, not accepted: {rejected:?}"
    );

    // The rejected attempt must not have half-accepted the candidate.
    let accepted: Option<String> = b
        .query_row(
            "SELECT accepted_job FROM local_sync_control WHERE id=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        accepted.is_none(),
        "a rejected merge leaves no accepted job"
    );
    assert!(logical::revision(&b).unwrap() >= before);

    // Retrying on a quiet library succeeds, and repeating it changes nothing.
    database::merge_apply(&b, 0, &state).expect("retry on a quiet library");
    logical::drain_local(&b).unwrap();
    let settled_revision = logical::revision(&b).unwrap();
    let title: String = b
        .query_row("SELECT title FROM articles WHERE id=?1", [other], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(title, "Article");
    database::merge_apply(&b, 0, &state).expect("second apply");
    assert_eq!(
        logical::revision(&b).unwrap(),
        settled_revision,
        "reapplying a converged state is idempotent"
    );
    assert!(logical::settled(&b).unwrap());
}

/// The production sync receive path is begin_receive -> stage_page -> preflight
/// -> accept (runtime::pull_state_candidate), not merge_apply. It shares the
/// same stable-revision requirement while candidates fold: one local write in
/// that window is retryable, and validation/acceptance converges after the
/// writer stops. This is product semantics, not a relaxed test.
#[test]
fn production_receive_path_reports_retryable_pending_and_converges_after_the_writer_stops() {
    use crate::local_sync::logical;
    let a = conn();
    let (feed, _) = seed(&a, &"a".repeat(32), "https://example.test/rss");
    a.execute(
        "WITH RECURSIVE n(x) AS(SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<299)
         INSERT INTO articles(feed_id,guid,title,is_read)
         SELECT ?1,'bulk-'||x,'Bulk',1 FROM n",
        [feed],
    )
    .unwrap();
    database::initialize(&a, LIB).unwrap();
    logical::drain_local(&a).unwrap();
    let state = database::checkpoint(&a).unwrap();
    assert!(
        state.fields.len() > 256,
        "several candidate pages are required: {}",
        state.fields.len()
    );

    let b = conn();
    seed(&b, &"a".repeat(32), "https://example.test/rss");
    database::initialize(&b, LIB).unwrap();
    b.execute_batch(
        "CREATE TRIGGER test_drift_during_fold AFTER UPDATE ON local_sync_work
         BEGIN UPDATE local_sync_control SET logical_revision=logical_revision+1 WHERE id=1; END;",
    )
    .unwrap();
    let job = accepted_candidate_tests::stage(&b, &state);
    let error = logical::preflight(&b, &job).unwrap_err();
    assert_eq!(
        error.code, "localSyncPending",
        "a local write during the fold must be reported retryable at the production receive entry: {error:?}"
    );

    // The writer stops; the same receive row can now validate and accept.
    b.execute_batch("DROP TRIGGER test_drift_during_fold;")
        .unwrap();
    let checked = logical::preflight(&b, &job).unwrap();
    assert!(logical::accept(&b, 0, &job, checked).unwrap());
    logical::drain_local(&b).unwrap();
    assert_eq!(
        b.query_row("SELECT COUNT(*) FROM articles", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        300,
        "the retried whole-library state must converge after the writer stops"
    );
    assert!(logical::settled(&b).unwrap());
}

/// A sealed-but-unacked body publication is the normal steady state: this device
/// sealed a batch and is waiting for a peer that may never come (the archive
/// tests ACK by hand). The export reads the local logical view, so it must
/// complete without waiting for that channel, and must still be correct while
/// the foreground keeps writing.
#[test]
fn portable_export_finishes_with_an_unacked_body_publication() {
    use crate::local_sync::logical;
    let file = path("export-pending").with_extension("db");
    let c = db::open(&file).unwrap();
    c.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
    let (feed, _) = seed(&c, &"a".repeat(32), "https://example.test/rss");
    for n in 1..=40 {
        let text = format!("{n:04} {}", "body ".repeat(200));
        let html = format!("<p>{text}</p>");
        let identity = db::portable_content_identity("rss", Some(&html), &text);
        c.execute(
            "INSERT INTO contents(identity,content_html,body_text,extraction_source) VALUES(?1,?2,?3,'rss')",
            params![identity, html, text],
        )
        .unwrap();
        let content = c.last_insert_rowid();
        c.execute(
            "INSERT INTO articles(feed_id,guid,title,is_read,content_id,rss_content_id)
             VALUES(?1,?2,'Saved',1,?3,?3)",
            params![feed, format!("pending-{n:03}"), content],
        )
        .unwrap();
    }
    database::initialize(&c, LIB).unwrap();
    logical::drain_local(&c).unwrap();
    // Seal a body batch and never acknowledge it: local_sync_pending keeps the row.
    let sealed = storage::seal_bodies(&c, 0).unwrap().expect("body batch");
    let pending: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM local_sync_pending WHERE lane='body'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        pending, 1,
        "the fixture must hold an unacked body publication"
    );

    let export_path = path("export-pending-out");
    let started = std::time::Instant::now();
    let exported = backup::export(&c, &export_path).expect("export must not wait for the ACK");
    println!(
        "ARCHIVE_PERF export_with_pending_ms={} bytes={}",
        started.elapsed().as_millis(),
        exported.size_bytes
    );
    assert_eq!(exported.articles, 41, "every article is exported");
    assert_eq!(backup::preview(&export_path).unwrap().articles, 41);

    // Foreground writes continue while that channel is still unacked.
    db::set_read_with_enqueue(&c, 1, true).unwrap();
    let seq_before: i64 = c
        .query_row(
            "SELECT COALESCE(MAX(seq),0) FROM local_sync_journal",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(seq_before > 0, "the foreground write committed");
    let second_path = path("export-pending-out-2");
    let again = backup::export(&c, &second_path).expect("second export");
    assert_eq!(again.articles, 41);
    std::fs::remove_file(&second_path).unwrap();
    let still_pending: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM local_sync_pending WHERE lane='body'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        still_pending, 1,
        "the publication channel is untouched by export"
    );
    drop(sealed);
    drop(c);
    std::fs::remove_file(&export_path).unwrap();
    std::fs::remove_file(&file).unwrap();
}

/// Observable cut point for the export snapshot: a write committed while the
/// export runs must not appear in the file, while the live library keeps it.
/// This is the assertion; "it finished without an error" is not.
#[test]
fn portable_export_snapshot_excludes_writes_committed_while_it_runs() {
    use crate::local_sync::logical;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let file = path("export-cut").with_extension("db");
    let c = db::open(&file).unwrap();
    c.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
    let (feed, _) = seed(&c, &"a".repeat(32), "https://example.test/rss");
    // Enough later-sorting entries that the inspected article is written late
    // in the export, so the concurrent edit lands long before it.
    let tx = c.unchecked_transaction().unwrap();
    for n in 0..2000 {
        let text = format!("{n:08} {}", "content ".repeat(511));
        let html = format!("<p>{text}</p>");
        let identity = db::portable_content_identity("rss", Some(&html), &text);
        tx.execute(
            "INSERT INTO contents(identity,content_html,body_text,extraction_source) VALUES(?1,?2,?3,'rss')",
            params![identity, html, text],
        )
        .unwrap();
        let content = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO articles(feed_id,guid,title,is_read,is_starred,content_id,rss_content_id)
             VALUES(?1,?2,'Saved',1,0,?3,?3)",
            params![feed, format!("zzz-sentinel-{n:05}"), content],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    database::initialize(&c, LIB).unwrap();
    logical::drain_local(&c).unwrap();

    let entry: String = c
        .query_row(
            "SELECT substr(key,1,66) FROM local_sync_fields
             WHERE key LIKE 'a/%/meta' ORDER BY key DESC LIMIT 1",
            [],
            |r| r.get::<_, String>(0),
        )
        .unwrap();
    let guid: String = c
        .query_row(
            "SELECT json_extract(register,'$.value.value.guid') FROM local_sync_fields WHERE key=?1",
            [format!("{entry}/meta")],
            |r| r.get(0),
        )
        .unwrap();
    assert!(guid.starts_with("zzz-sentinel-"), "{guid}");
    let article: i64 = c
        .query_row("SELECT id FROM articles WHERE guid=?1", [&guid], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(!c
        .query_row(
            "SELECT is_starred FROM articles WHERE id=?1",
            [article],
            |r| { r.get::<_, bool>(0) }
        )
        .unwrap());

    let export_path = path("export-cut-out");
    let done = Arc::new(AtomicBool::new(false));
    let worker = {
        let path = export_path.clone();
        let db_path = file.clone();
        let done = done.clone();
        std::thread::spawn(move || {
            let reader = db::open(&db_path).unwrap();
            let result = backup::export(&reader, &path);
            done.store(true, Ordering::SeqCst);
            result
        })
    };
    while !export_path.exists() && !done.load(Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(export_path.exists(), "export never produced an output file");
    let seq_before: i64 = c
        .query_row(
            "SELECT COALESCE(MAX(seq),0) FROM local_sync_journal",
            [],
            |r| r.get(0),
        )
        .unwrap();
    db::set_starred_with_enqueue(&c, article, true).unwrap();
    let seq_after: i64 = c
        .query_row(
            "SELECT COALESCE(MAX(seq),0) FROM local_sync_journal",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        seq_after > seq_before,
        "the concurrent write did not commit a journal entry"
    );
    assert!(
        !done.load(Ordering::SeqCst),
        "export finished before the concurrent write; the fixture no longer proves the cut"
    );
    let info = worker.join().unwrap().unwrap();
    assert!(info.size_bytes > 0);

    let (_, exported_star, exported_meta) =
        backup::read_exported_article(&export_path, &entry[2..])
            .unwrap()
            .expect("the exported article record is present");
    assert!(
        !exported_star,
        "a write committed after the export snapshot must not enter the file"
    );
    assert_eq!(exported_meta["guid"], json!(guid));
    assert!(c
        .query_row(
            "SELECT is_starred FROM articles WHERE id=?1",
            [article],
            |r| { r.get::<_, bool>(0) }
        )
        .unwrap());
    drop(c);
    std::fs::remove_file(&export_path).unwrap();
    std::fs::remove_file(&file).unwrap();
}

#[path = "review_segment1_tests.rs"]
mod review_segment1_tests;

#[path = "group_dependency_tests.rs"]
mod group_dependency_tests;

#[path = "segment2_tests.rs"]
mod segment2_tests;

#[path = "perf_boundary_tests.rs"]
mod perf_boundary_tests;
