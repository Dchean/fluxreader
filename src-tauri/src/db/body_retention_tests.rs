//! Real SQLite behavior tests, including committed ordering and durable recovery.
//! No network, application windows, user database, or wall-clock sleeps.
use super::*;
use crate::local_sync::{database, logical, model, storage};
use crate::webdav_transport::{Checkpoint, FieldValue, Register, Stamp};
use serde_json::json;
use std::path::PathBuf;

const LIB: &str = "11111111111111111111111111111111";
const SOURCE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OLD: i64 = 1000;

struct TestDir(PathBuf);
impl TestDir {
    fn new() -> Self {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../tmp/body-retention-db-tests")
            .join(format!(
                "{}-{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap(),
                SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn db(&self) -> PathBuf {
        self.0.join("owned.db")
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn conn() -> Connection {
    let mut conn = Connection::open_in_memory().unwrap();
    super::super::MIGRATIONS.to_latest(&mut conn).unwrap();
    database::register_functions(&conn).unwrap();
    conn.pragma_update(None, "foreign_keys", true).unwrap();
    conn
}
fn input(guid: &str, body: &str) -> NewArticle {
    NewArticle {
        guid: guid.into(),
        url: Some(format!("https://example.test/{guid}")),
        title: guid.into(),
        author: None,
        summary: Some(body.into()),
        content_html: Some(format!("<p>{body}</p>")),
        body_text: body.into(),
        image_url: None,
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: Some("2000-01-01T00:00:00Z".into()),
        source: "direct".into(),
    }
}
fn feed(c: &Connection) -> i64 {
    let folder = super::super::create_folder(c, "Retention", "article").unwrap();
    let id = super::super::insert_feed(
        c,
        "https://example.test/retention.xml",
        None,
        "Feed",
        None,
        folder,
        "inherit",
        false,
        false,
    )
    .unwrap();
    c.execute(
        "UPDATE feeds SET sync_id=?1 WHERE id=?2",
        params![SOURCE, id],
    )
    .unwrap();
    id
}
fn seed(c: &Connection, f: i64, guid: &str, read: bool) -> i64 {
    let (id, _) = super::super::upsert_article_with_feed(c, f, &input(guid, guid), false).unwrap();
    super::super::sync_set_read_at_version(c, id, read, 0).unwrap();
    id
}
fn age(c: &Connection, id: i64, since: Option<i64>) {
    c.execute(
        "UPDATE articles SET body_retention_since=?1 WHERE id=?2",
        params![since, id],
    )
    .unwrap();
}
fn since(c: &Connection, id: i64) -> Option<i64> {
    c.query_row(
        "SELECT body_retention_since FROM articles WHERE id=?1",
        [id],
        |r| r.get(0),
    )
    .unwrap()
}
fn days(c: &Connection, days: serde_json::Value) {
    super::super::set_setting(
        c,
        "app_settings",
        &json!({"readBodyRetentionDays":days}).to_string(),
    )
    .unwrap();
}
fn run(c: &Connection, now: i64) -> RetentionReport {
    retire_read_bodies_slice(c, now, &mut RetentionCursor::default()).unwrap()
}
fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
fn initialized(c: &Connection) {
    database::initialize(c, LIB).unwrap();
    logical::drain_local(c).unwrap();
}
fn acknowledge_bodies(c: &Connection) {
    let generation = database::generation(c).unwrap();
    if let Some(pending) = storage::seal_bodies(c, generation).unwrap() {
        // Test fixture supplies the exact sealed manifest to the production ACK
        // boundary. No transport success or remote durability is claimed here.
        storage::acknowledge(c, generation, &pending.manifest).unwrap();
    }
    while storage::cleanup_slice(c).unwrap() {}
}
fn stage(c: &Connection, state: &Checkpoint) -> String {
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
    let revision = logical::preflight(c, &job).unwrap();
    assert!(logical::accept(c, 0, &job, revision).unwrap());
    job
}
fn remote_read(c: &Connection, guid: &str, read: bool) -> Checkpoint {
    let mut state = database::checkpoint(c).unwrap();
    let actor = "e".repeat(32);
    let seq = state.covered.get(&actor).copied().unwrap_or(0) + 1;
    state.max_lamport += 10;
    state.fields.insert(
        format!("a/{}/read", model::entry_key(SOURCE, guid)),
        Register {
            stamp: Stamp {
                actor: actor.clone(),
                seq,
                lamport: state.max_lamport,
            },
            value: FieldValue::Set(json!(read)),
        },
    );
    state.covered.insert(actor, seq);
    state
}

#[test]
fn retention_default_zero_invalid_and_integer_boundaries() {
    let c = conn();
    let f = feed(&c);
    let id = seed(&c, f, "permanent", true);
    age(&c, id, Some(OLD));
    assert_eq!(run(&c, now()).released, 0);
    days(&c, json!(0));
    assert_eq!(run(&c, now()).released, 0);
    for bad in [
        json!(-1),
        json!(3651),
        json!(1.5),
        json!("1"),
        json!(null),
        json!(true),
    ] {
        assert!(
            super::super::settings::validate_app_setting("readBodyRetentionDays", &bad).is_err()
        );
        days(&c, bad);
        assert!(retire_read_bodies_slice(&c, now(), &mut RetentionCursor::default()).is_err());
        assert!(has_body(&c, id).unwrap());
    }
    for good in [0, 1, 3650] {
        assert!(super::super::settings::validate_app_setting(
            "readBodyRetentionDays",
            &json!(good)
        )
        .is_ok());
    }
    days(&c, json!(3650));
    assert_eq!(run(&c, now()).released, 1);
}

#[test]
fn retention_provider_versions_same_values_and_explicit_local_read() {
    let c = conn();
    let f = feed(&c);
    let id = seed(&c, f, "provider-old-publication", false);
    assert_eq!(since(&c, id), None);
    let before = now();
    assert_eq!(
        super::super::sync_set_read_at_version(&c, id, true, 0).unwrap(),
        1
    );
    assert!(since(&c, id).unwrap() >= before);
    age(&c, id, Some(OLD));
    super::super::sync_set_read_at_version(&c, id, true, 0).unwrap();
    super::super::sync_set_star_at_version(&c, id, false, 0).unwrap();
    assert_eq!(
        since(&c, id),
        Some(OLD),
        "same provider snapshot does not renew"
    );
    super::super::set_read(&c, id, false).unwrap();
    assert_eq!(since(&c, id), None);
    assert_eq!(
        super::super::sync_set_read_at_version(&c, id, true, 0).unwrap(),
        0,
        "late pull version fails"
    );
    assert_eq!(since(&c, id), None);
    super::super::set_read(&c, id, true).unwrap();
    age(&c, id, Some(OLD));
    super::super::set_read(&c, id, true).unwrap();
    assert!(since(&c, id).unwrap() >= before);
    super::super::set_starred(&c, id, true).unwrap();
    assert_eq!(since(&c, id), None);
    super::super::set_starred(&c, id, false).unwrap();
    assert!(since(&c, id).unwrap() >= before);
}

#[test]
fn retention_bulk_mark_all_and_failed_enqueue_are_atomic() {
    let c = conn();
    let f = feed(&c);
    let a = seed(&c, f, "bulk-a", false);
    let b = seed(&c, f, "bulk-b", false);
    super::super::set_read_bulk_with_enqueue(&c, &[a, b], true).unwrap();
    assert!(since(&c, a).is_some() && since(&c, b).is_some());
    let d = seed(&c, f, "mark-all", false);
    super::super::mark_all_read_with_enqueue(&c, Some(f), None, false, None, None).unwrap();
    assert!(since(&c, d).is_some());
    let fail = seed(&c, f, "rollback", false);
    c.execute_batch("CREATE TRIGGER reject_retention_enqueue BEFORE INSERT ON sync_queue BEGIN SELECT RAISE(ABORT,'injected queue failure'); END;").unwrap();
    assert!(super::super::set_read_with_enqueue(&c, fail, true).is_err());
    assert_eq!(since(&c, fail), None);
    assert!(!c
        .query_row("SELECT is_read FROM articles WHERE id=?1", [fail], |r| r
            .get::<_, bool>(
            0
        ))
        .unwrap());
}

#[test]
fn retention_releases_payload_preserves_media_identity_and_rejects_old_ai() {
    let c = conn();
    let f = feed(&c);
    let id = seed(&c, f, "media", true);
    days(&c, json!(1));
    c.execute("UPDATE articles SET image_url='https://example.test/cover.jpg',image_source='rss',enclosure_url='https://example.test/podcast.mp3',enclosure_mime='audio/mpeg',duration_sec=123,ai_summary='ai',translated_content='translated',ai_summary_sig='s',translated_sig='t' WHERE id=?1",[id]).unwrap();
    c.execute(
        "UPDATE articles SET images_json=?2,image_count=3 WHERE id=?1",
        params![
            id,
            json!([
                "https://example.test/gallery-first.jpg",
                "data:image/png;base64,AAAA",
                "http://example.test/second.jpg"
            ])
            .to_string()
        ],
    )
    .unwrap();
    c.execute("INSERT INTO ai_segments(article_id,product,signature,segment_index,content) VALUES(?1,'translation','t',0,'segment')",[id]).unwrap();
    age(&c, id, Some(OLD));
    let token = super::super::article_ai_commit_token(&c, id)
        .unwrap()
        .unwrap();
    let fields: (String, Option<String>, i64, i64) = c
        .query_row(
            "SELECT guid,url,read_version,star_version FROM articles WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(run(&c, now()).released, 1);
    assert!(!has_body(&c, id).unwrap());
    assert_eq!(
        fields,
        c.query_row(
            "SELECT guid,url,read_version,star_version FROM articles WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        )
        .unwrap()
    );
    let media:(String,i64,String,String,String,i64)=c.query_row("SELECT images_json,image_count,image_url,image_source,enclosure_url,duration_sec FROM articles WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&media.0).unwrap(),
        json!([
            "https://example.test/gallery-first.jpg",
            "http://example.test/second.jpg"
        ])
    );
    assert_eq!(media.1, 2);
    assert!(media.2.ends_with("cover.jpg"));
    assert_eq!(media.3, "rss");
    assert!(media.4.ends_with("podcast.mp3"));
    assert_eq!(media.5, 123);
    assert_eq!(
        c.query_row("SELECT count(*) FROM ai_segments", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert!(!super::super::set_article_ai_fields_checked(
        &c,
        id,
        &token,
        AiProduct::Summary {
            text: "late",
            signature: "s"
        }
    )
    .unwrap());
    let epoch: i64 = c
        .query_row("SELECT ai_epoch FROM articles WHERE id=?1", [id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(run(&c, now()).released, 0);
    assert_eq!(
        epoch,
        c.query_row("SELECT ai_epoch FROM articles WHERE id=?1", [id], |r| r
            .get::<_, i64>(0))
            .unwrap()
    );
}

#[test]
fn retention_restore_success_restarts_but_refresh_and_stale_token_do_not() {
    let c = conn();
    let f = feed(&c);
    let id = seed(&c, f, "restore", true);
    days(&c, json!(1));
    age(&c, id, Some(OLD));
    let old = super::super::article_content_commit_token(&c, id)
        .unwrap()
        .unwrap()
        .0;
    assert_eq!(run(&c, now()).released, 1);
    assert!(
        super::super::replace_article_content_checked(&c, id, &old, "<p>late</p>")
            .unwrap()
            .is_none()
    );
    assert_eq!(since(&c, id), Some(OLD));
    let missing = super::super::article_content_commit_token(&c, id)
        .unwrap()
        .unwrap()
        .0;
    let start = now();
    assert!(super::super::replace_article_content_checked(
        &c,
        id,
        &missing,
        "<p>restored text</p>"
    )
    .unwrap()
    .is_some());
    assert!(since(&c, id).unwrap() >= start);
    assert_eq!(run(&c, now()).released, 0);
    age(&c, id, Some(OLD));
    super::super::upsert_article_with_feed(&c, f, &input("restore", "updated RSS"), false).unwrap();
    assert_eq!(
        since(&c, id),
        Some(OLD),
        "ordinary refresh of available content keeps age"
    );
    assert_eq!(run(&c, now()).released, 1);
    super::super::upsert_article_with_feed(&c, f, &input("restore", "updated RSS"), false).unwrap();
    assert!(
        since(&c, id).unwrap() >= start,
        "missing to available automatic refresh renews"
    );
}

#[test]
fn retention_null_future_clock_rollback_and_restart() {
    let dir = TestDir::new();
    let path = dir.db();
    let c = super::super::open(&path).unwrap();
    let f = feed(&c);
    let id = seed(&c, f, "restart", true);
    days(&c, json!(1));
    age(&c, id, None);
    assert_eq!(run(&c, now()).initialized, 1);
    assert!(has_body(&c, id).unwrap());
    let anchored = since(&c, id).unwrap();
    drop(c);
    let c = super::super::open(&path).unwrap();
    assert_eq!(since(&c, id), Some(anchored));
    assert_eq!(run(&c, anchored - 1).released, 0);
    assert_eq!(run(&c, anchored + 86400 - 1).released, 0);
    assert_eq!(run(&c, anchored + 86400).released, 1);
}

#[test]
fn retention_equivalent_unread_and_shared_content_protect_real_bytes() {
    let c = conn();
    let f = feed(&c);
    let mut first = input("equivalent-a", "shared");
    first.title = "same".into();
    let (a, _) = super::super::upsert_article_with_feed(&c, f, &first, false).unwrap();
    first.guid = "equivalent-b".into();
    let (b, _) = super::super::upsert_article_with_feed(&c, f, &first, false).unwrap();
    super::super::sync_set_read_at_version(&c, a, true, 0).unwrap();
    age(&c, a, Some(OLD));
    days(&c, json!(1));
    assert_eq!(run(&c, now()).released, 0);
    assert!(has_body(&c, a).unwrap() && has_body(&c, b).unwrap());
    c.execute("UPDATE articles SET title='independent' WHERE id=?1", [b])
        .unwrap();
    let cid: i64 = c
        .query_row("SELECT content_id FROM articles WHERE id=?1", [b], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(run(&c, now()).released, 1);
    assert!(has_body(&c, b).unwrap());
    assert!(c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM contents WHERE id=?1)",
            [cid],
            |r| r.get::<_, bool>(0)
        )
        .unwrap());
}

#[test]
fn retention_fairness_advances_beyond_pending_prefix_and_caps_each_slice() {
    let c = conn();
    let f = feed(&c);
    days(&c, json!(1));
    for i in 0..80 {
        let id = seed(&c, f, &format!("pending-{i}"), true);
        age(&c, id, Some(OLD));
        super::super::enqueue_sync(&c, Some(id), None, "read", None).unwrap();
    }
    let tail = seed(&c, f, "eligible-tail", true);
    age(&c, tail, Some(OLD));
    let mut cursor = RetentionCursor::default();
    let mut released = 0;
    for _ in 0..100 {
        let report = retire_read_bodies_slice(&c, now(), &mut cursor).unwrap();
        assert!(report.inspected <= 64 && report.released <= 8);
        released += report.released;
        if !has_body(&c, tail).unwrap() {
            break;
        }
    }
    assert_eq!(released, 1);
    assert!(!has_body(&c, tail).unwrap());
    assert_eq!(
        c.query_row("SELECT count(*) FROM sync_queue", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        80
    );
}

#[test]
fn retention_sql_candidates_and_holds_use_bounded_index_access() {
    let c = conn();
    for (sql, args, index) in [
        (
            EXPIRED_SQL,
            vec![-1, 0, now(), 32],
            "idx_articles_body_retention",
        ),
        (
            INITIALIZE_SQL,
            vec![0, 32],
            "idx_articles_body_retention_null",
        ),
    ] {
        let mut stmt = c.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        let plans = stmt
            .query_map(rusqlite::params_from_iter(args), |r| r.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .join("\n");
        assert!(plans.contains("SEARCH") && plans.contains(index), "{plans}");
        assert!(!plans.contains("TEMP B-TREE"), "{plans}");
    }
    for field in ["primary", "rss"] {
        let sql=format!("EXPLAIN QUERY PLAN SELECT 1 FROM local_sync_body_hold WHERE json_extract(body_ref,'$.{field}')=?1");
        let plan: String = c.query_row(&sql, ["x"], |r| r.get(3)).unwrap();
        assert!(plan.contains(&format!("local_sync_hold_{field}")), "{plan}");
    }
}

#[test]
fn retention_protection_first_and_gc_first_have_distinct_committed_results() {
    let dir = TestDir::new();
    let path = dir.db();
    let c = super::super::open(&path).unwrap();
    let f = feed(&c);
    let a = seed(&c, f, "protect-first", true);
    let b = seed(&c, f, "gc-first", true);
    days(&c, json!(1));
    initialized(&c);
    acknowledge_bodies(&c);
    age(&c, a, Some(OLD));
    age(&c, b, Some(OLD));
    let second = super::super::open(&path).unwrap();
    // This candidate was observed before the other writer committed protection.
    assert!(c
        .query_row(
            "SELECT content_id IS NOT NULL FROM articles WHERE id=?1",
            [a],
            |r| r.get::<_, bool>(0)
        )
        .unwrap());
    super::super::set_starred(&second, a, true).unwrap();
    logical::drain_local(&second).unwrap();
    assert!(has_body(&c, a).unwrap());
    let report = run(&c, now());
    assert_eq!(report.released, 1);
    assert!(has_body(&c, a).unwrap());
    assert!(!has_body(&c, b).unwrap());
    super::super::set_starred(&second, b, true).unwrap();
    logical::drain_local(&second).unwrap();
    assert!(
        !has_body(&c, b).unwrap(),
        "late favorite cannot preserve nonexistent bytes"
    );
    assert!(second
        .query_row("SELECT is_starred FROM articles WHERE id=?1", [b], |r| r
            .get::<_, bool>(
            0
        ))
        .unwrap());
    assert!(!second
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM local_sync_body_hold WHERE article_id=?1)",
            [b],
            |r| r.get::<_, bool>(0)
        )
        .unwrap());
}

#[test]
fn retention_accepted_l_disagreement_and_restart_pause_until_age_work_finishes() {
    let dir = TestDir::new();
    let path = dir.db();
    let c = super::super::open(&path).unwrap();
    let f = feed(&c);
    let id = seed(&c, f, "logical", true);
    days(&c, json!(1));
    initialized(&c);
    acknowledge_bodies(&c);
    age(&c, id, Some(OLD));
    let incoming = remote_read(&c, "logical", false);
    stage(&c, &incoming);
    assert!(c
        .query_row("SELECT is_read FROM articles WHERE id=?1", [id], |r| r
            .get::<_, bool>(0))
        .unwrap());
    assert_eq!(
        logical::article_retention_state(&c, id).unwrap(),
        Some((false, false, true))
    );
    assert_eq!(run(&c, now()).released, 0);
    // Persist an accepted/folding boundary and recover with a real reopened DB.
    assert!(logical::resume_slice(&c).unwrap());
    drop(c);
    let c = super::super::open(&path).unwrap();
    assert_eq!(run(&c, now()).released, 0);
    logical::drain_local(&c).unwrap();
    assert!(quiescent(&c).unwrap());
    assert_eq!(since(&c, id), None);
    assert!(has_body(&c, id).unwrap());
    let incoming = remote_read(&c, "logical", true);
    stage(&c, &incoming);
    assert_eq!(run(&c, now()).released, 0);
    logical::drain_local(&c).unwrap();
    assert!(since(&c, id).unwrap() >= now() - 2);
    age(&c, id, Some(OLD));
    database::merge_apply(&c, 0, &incoming).unwrap();
    assert_eq!(
        since(&c, id),
        Some(OLD),
        "same accepted operation does not renew on replay"
    );
    logical::materialize(&c, &format!("a/{}", model::entry_key(SOURCE, "logical"))).unwrap();
    assert_eq!(
        since(&c, id),
        Some(OLD),
        "same physical projection does not renew"
    );
}

#[test]
fn retention_new_same_value_read_register_renews_once_and_local_work_is_durable() {
    let c = conn();
    let f = feed(&c);
    let id = seed(&c, f, "same-register", true);
    days(&c, json!(1));
    initialized(&c);
    age(&c, id, Some(OLD));
    let incoming = remote_read(&c, "same-register", true);
    stage(&c, &incoming);
    assert_eq!(run(&c, now()).released, 0);
    logical::drain_local(&c).unwrap();
    assert!(since(&c, id).unwrap() > OLD);
    age(&c, id, Some(OLD));
    database::merge_apply(&c, 0, &incoming).unwrap();
    assert_eq!(since(&c, id), Some(OLD));
    let key = format!("a/{}/read", model::entry_key(SOURCE, "same-register"));
    for read in [false, true] {
        logical::capture_patch(
            &c,
            &[(key.clone(), FieldValue::Set(json!(read)))]
                .into_iter()
                .collect(),
        )
        .unwrap();
    }
    assert_eq!(run(&c, now()).released, 0);
    logical::drain_local(&c).unwrap();
    assert!(since(&c, id).unwrap() > OLD);
}

#[test]
fn retention_sealed_state_offline_can_progress_without_discarding_real_holds() {
    let c = conn();
    let f = feed(&c);
    let clear = seed(&c, f, "unheld", true);
    let later = seed(&c, f, "disconnected", true);
    let protected = seed(&c, f, "held", false);
    days(&c, json!(1));
    initialized(&c);
    acknowledge_bodies(&c);
    assert!(!c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM local_sync_body_hold WHERE article_id=?1)",
            [clear],
            |r| r.get::<_, bool>(0)
        )
        .unwrap());
    // A real explicit unread no-op captures protected bytes again without
    // touching the other article. Its body publication remains unacknowledged.
    super::super::set_read(&c, protected, false).unwrap();
    logical::drain_local(&c).unwrap();
    super::super::upsert_article_with_feed(&c, f, &input("held", "new protected body"), false)
        .unwrap();
    logical::drain_local(&c).unwrap();
    let state = storage::seal_state(&c, 0).unwrap().unwrap();
    let body = storage::seal_bodies(&c, 0).unwrap().unwrap();
    age(&c, clear, Some(OLD));
    assert_eq!(run(&c, now()).released, 1);
    assert!(has_body(&c, protected).unwrap());
    assert_eq!(
        storage::pending(&c, "state", 0)
            .unwrap()
            .unwrap()
            .manifest
            .file_name()
            .unwrap(),
        state.manifest.file_name().unwrap()
    );
    assert_eq!(
        storage::pending(&c, "body", 0)
            .unwrap()
            .unwrap()
            .manifest
            .file_name()
            .unwrap(),
        body.manifest.file_name().unwrap()
    );
    assert!(c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM local_sync_body_hold WHERE article_id=?1)",
            [protected],
            |r| r.get::<_, bool>(0)
        )
        .unwrap());
    let status = crate::local_sync::runtime::disconnect(&c).unwrap();
    assert!(!status.connected);
    age(&c, later, Some(OLD));
    assert_eq!(run(&c, now()).released, 1);
    assert!(has_body(&c, protected).unwrap());
}

#[test]
fn retention_retired_starred_and_locatorless_hold_preserve_bytes() {
    let c = conn();
    let f = feed(&c);
    let star = seed(&c, f, "retired-star", true);
    let held = seed(&c, f, "locatorless", true);
    days(&c, json!(1));
    super::super::set_starred(&c, star, true).unwrap();
    let identity: String = c
        .query_row(
            "SELECT c.identity FROM articles a JOIN contents c ON c.id=a.content_id WHERE a.id=?1",
            [held],
            |r| r.get(0),
        )
        .unwrap();
    c.execute(
        "INSERT INTO local_sync_body_hold(article_id,entry,body_ref) VALUES(NULL,?1,?2)",
        params![
            "b".repeat(64),
            json!({"primary":identity,"rss":null}).to_string()
        ],
    )
    .unwrap();
    age(&c, held, Some(OLD));
    assert_eq!(run(&c, now()).released, 0);
    c.execute("UPDATE feeds SET active=0 WHERE id=?1", [f])
        .unwrap();
    assert_eq!(run(&c, now()).released, 0);
    assert!(has_body(&c, star).unwrap());
    assert!(has_body(&c, held).unwrap());
    c.execute(
        "UPDATE articles SET rss_content_id=NULL WHERE id=?1",
        [held],
    )
    .unwrap();
    let token = super::super::article_content_commit_token(&c, held)
        .unwrap()
        .unwrap()
        .0;
    super::super::replace_article_content_checked(&c, held, &token, "<p>new display</p>").unwrap();
    assert!(
        c.query_row(
            "SELECT EXISTS(SELECT 1 FROM contents WHERE identity=?1)",
            [identity],
            |r| r.get::<_, bool>(0)
        )
        .unwrap(),
        "explicit recycle also honors locatorless hold"
    );
}

#[test]
fn retention_busy_writer_skips_and_restores_timeout() {
    let dir = TestDir::new();
    let path = dir.db();
    let c = super::super::open(&path).unwrap();
    let f = feed(&c);
    let id = seed(&c, f, "busy", true);
    days(&c, json!(1));
    age(&c, id, Some(OLD));
    c.busy_timeout(Duration::from_millis(4321)).unwrap();
    let second = super::super::open(&path).unwrap();
    let writer = super::super::write_transaction(&second).unwrap();
    let mut cursor = RetentionCursor {
        expired: (77, 88),
        initialize: 99,
        initialize_next: false,
    };
    let before = (cursor.expired, cursor.initialize, cursor.initialize_next);
    let start = Instant::now();
    let error = retire_read_bodies_slice(&c, now(), &mut cursor).unwrap_err();
    assert_eq!(error.code, "db");
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "maintenance must not wait 4s for the writer"
    );
    assert_eq!(
        (cursor.expired, cursor.initialize, cursor.initialize_next),
        before,
        "busy skip must not advance the maintenance cursor"
    );
    assert_eq!(
        c.pragma_query_value(None, "busy_timeout", |r| r.get::<_, i64>(0))
            .unwrap(),
        4321
    );
    assert!(has_body(&c, id).unwrap());
    writer.commit().unwrap();
    assert_eq!(run(&c, now()).released, 1);
}

#[test]
fn retention_manual_cleanup_uses_publication_age_but_shared_release_and_protection() {
    let mut c = conn();
    let f = feed(&c);
    let id = seed(&c, f, "manual", true);
    days(&c, json!(0));
    assert!(since(&c, id).unwrap() >= now() - 2);
    assert_eq!(run(&c, now()).released, 0);
    c.execute("UPDATE articles SET enclosure_url='https://example.test/file.mp3',duration_sec=12 WHERE id=?1",[id]).unwrap();
    assert_eq!(
        super::super::cleanup_cache(&mut c, 7, "articles")
            .unwrap()
            .0,
        1
    );
    assert!(!has_body(&c, id).unwrap());
    assert_eq!(
        c.query_row(
            "SELECT enclosure_url FROM articles WHERE id=?1",
            [id],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "https://example.test/file.mp3"
    );
}

#[test]
fn retention_old_development_database_is_rejected_without_any_write() {
    let dir = TestDir::new();
    let path = dir.db();
    let c = super::super::open(&path).unwrap();
    c.execute_batch("DROP TRIGGER trg_articles_retention_ai; DROP TRIGGER trg_articles_retention_state; DROP TRIGGER trg_articles_retention_restore; DROP INDEX idx_articles_body_retention; DROP INDEX idx_articles_body_retention_null; ALTER TABLE articles DROP COLUMN body_retention_since;").unwrap();
    drop(c);
    let before = std::fs::read(&path).unwrap();
    let error = super::super::open(&path).unwrap_err();
    assert_eq!(error.code, "unsupported_database");
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn retention_native_snapshot_keeps_local_age_portable_restore_does_not_transfer_it() {
    let dir = TestDir::new();
    let path = dir.db();
    let c = super::super::open(&path).unwrap();
    let f = feed(&c);
    let id = seed(&c, f, "backup-age", true);
    age(&c, id, Some(OLD));
    let snapshot = super::super::create_manual_backup(&c).unwrap();
    let snapshot = super::super::open(std::path::Path::new(&snapshot.path)).unwrap();
    assert_eq!(since(&snapshot, id), Some(OLD));
    drop(snapshot);
    let portable = dir.0.join("portable.flxb");
    crate::local_sync::backup::export(&c, &portable).unwrap();
    let preview = crate::local_sync::backup::preview(&portable).unwrap();
    crate::local_sync::backup::restore(&c, &portable, &preview.digest).unwrap();
    let restored: i64 = c
        .query_row("SELECT id FROM articles WHERE guid='backup-age'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(since(&c, restored).is_none_or(|since| since >= now() - 2));
}

/// Separate first-run counterexample requested by the controller. This is legal
/// current transport input, not a manually corrupted contents row. The initial
/// natural failure remains in first-check-01/counterexample-02.log; assertions
/// below are retained through retirement of the old lazy repair.
#[test]
fn held_imported_readability_survives_refresh_counterexample() {
    let c = conn();
    let f = feed(&c);
    let id = seed(&c, f, "imported-readability", false);
    initialized(&c);
    let html = crate::sanitize::sanitize("<p>original imported text</p>", None);
    let body = model::BodyContent {
        identity: super::super::portable_content_identity(
            "readability",
            Some(&html),
            "different provider text",
        ),
        html: Some(html),
        text: "different provider text".into(),
        source: "readability".into(),
        extracted: true,
    };
    body.validate().unwrap();
    let body_ref = model::BodyRef {
        primary: body.identity.clone(),
        rss: None,
    };
    let entry = model::entry_key(SOURCE, "imported-readability");
    logical::capture_patch(
        &c,
        &[(
            format!("a/{entry}/body"),
            FieldValue::Set(serde_json::to_value(&body_ref).unwrap()),
        )]
        .into_iter()
        .collect(),
    )
    .unwrap();
    logical::drain_local(&c).unwrap();
    let record = model::BodyRecord {
        entry,
        body_ref,
        primary: body.clone(),
        rss: None,
    };
    assert_eq!(database::apply_bodies(&c, 0, &[record]).unwrap(), 1);
    super::super::set_starred(&c, id, true).unwrap();
    logical::drain_local(&c).unwrap();
    assert!(c.query_row("SELECT EXISTS(SELECT 1 FROM local_sync_body_hold WHERE json_extract(body_ref,'$.primary')=?1)",[&body.identity],|r|r.get::<_,bool>(0)).unwrap());
    super::super::upsert_article_with_feed(
        &c,
        f,
        &input("imported-readability", "new RSS revision"),
        false,
    )
    .unwrap();
    let actual: Option<(Option<String>, String)> = c
        .query_row(
            "SELECT content_html,body_text FROM contents WHERE identity=?1",
            [&body.identity],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .unwrap();
    assert_eq!(
        actual,
        Some((body.html, body.text)),
        "held imported identity and exact bytes must survive a routine RSS refresh"
    );
}

fn install_current_import(c: &Connection, source: &str, guid: &str) -> model::BodyContent {
    let html = crate::sanitize::sanitize("<p>original imported text</p>", None);
    let body = model::BodyContent {
        identity: super::super::portable_content_identity(
            "readability",
            Some(&html),
            "different provider text",
        ),
        html: Some(html),
        text: "different provider text".into(),
        source: "readability".into(),
        extracted: true,
    };
    body.validate().unwrap();
    let entry = model::entry_key(source, guid);
    let body_ref = model::BodyRef {
        primary: body.identity.clone(),
        rss: None,
    };
    logical::capture_patch(
        c,
        &[(
            format!("a/{entry}/body"),
            FieldValue::Set(serde_json::to_value(&body_ref).unwrap()),
        )]
        .into_iter()
        .collect(),
    )
    .unwrap();
    logical::drain_local(c).unwrap();
    database::apply_bodies(
        c,
        0,
        &[model::BodyRecord {
            entry,
            body_ref,
            primary: body.clone(),
            rss: None,
        }],
    )
    .unwrap();
    body
}

fn immutable_body_snapshot(
    c: &Connection,
    identity: &str,
) -> (i64, Option<String>, String, i64, String, bool) {
    c.query_row("SELECT id,content_html,body_text,revision,extraction_source,fulltext_extracted FROM contents WHERE identity=?1", [identity],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).unwrap()
}

#[test]
fn immutable_imported_readability_without_hold_preserves_l_and_published_body_refs() {
    for action in ["refresh", "backfill"] {
        let c = conn();
        let f = feed(&c);
        let id = seed(&c, f, "unheld-import", false);
        initialized(&c);
        let body = install_current_import(&c, SOURCE, "unheld-import");
        super::super::set_starred(&c, id, true).unwrap();
        logical::drain_local(&c).unwrap();
        let published_body = storage::seal_bodies(&c, 0).unwrap().unwrap();
        storage::acknowledge(&c, 0, &published_body.manifest).unwrap();
        while storage::cleanup_slice(&c).unwrap() {}
        let published_state = storage::seal_state(&c, 0).unwrap().unwrap();
        storage::acknowledge(&c, 0, &published_state.manifest).unwrap();
        assert!(!c.query_row("SELECT EXISTS(SELECT 1 FROM local_sync_body_hold WHERE json_extract(body_ref,'$.primary')=?1)",[&body.identity],|r|r.get::<_,bool>(0)).unwrap(),"real exact receipt releases the hold before {action}");
        let before = immutable_body_snapshot(&c, &body.identity);
        if action == "refresh" {
            super::super::upsert_article_with_feed(
                &c,
                f,
                &input("unheld-import", "new RSS"),
                false,
            )
            .unwrap();
        } else {
            super::super::backfill_article_content(
                &c,
                id,
                "<p>backfill RSS</p>",
                "backfill RSS",
                Some("https://example.test/cover.png"),
                None,
                None,
            )
            .unwrap();
        }
        assert_eq!(
            immutable_body_snapshot(&c, &body.identity),
            before,
            "{action} cannot rewrite an unheld but referenced body"
        );
        let field = format!("a/{}/body", model::entry_key(SOURCE, "unheld-import"));
        let current = logical::get(&c, &field).unwrap().unwrap();
        let FieldValue::Set(current) = current.value else {
            panic!("current body reference disappeared")
        };
        assert_eq!(current["primary"], body.identity);
        let published: Register = serde_json::from_str(
            &c.query_row(
                "SELECT register FROM local_sync_selected_fields WHERE key=?1",
                [&field],
                |r| r.get::<_, String>(0),
            )
            .unwrap(),
        )
        .unwrap();
        let FieldValue::Set(published) = published.value else {
            panic!("published body reference disappeared")
        };
        assert_eq!(published["primary"], body.identity);
        let catalog =
            crate::local_sync::body_archive::catalog(&c, &published_body.manifest).unwrap();
        assert!(catalog
            .entries
            .values()
            .any(|entry| entry.body_ref.primary == body.identity));
        assert!(catalog.contents.contains_key(&body.identity));
        assert_eq!(
            super::super::article_content_commit_token(&c, id)
                .unwrap()
                .unwrap()
                .0
                .content_id,
            Some(before.0)
        );
    }
}

#[test]
fn immutable_imported_body_survives_merged_source_alias_refresh_and_backfill() {
    let a = conn();
    let b = conn();
    let af = feed(&a);
    let bf = feed(&b);
    let alias = "b".repeat(32);
    b.execute(
        "UPDATE feeds SET sync_id=?1 WHERE id=?2",
        params![alias, bf],
    )
    .unwrap();
    let aid = seed(&a, af, "shared-alias", false);
    seed(&b, bf, "shared-alias", false);
    initialized(&a);
    initialized(&b);
    let body = install_current_import(&a, SOURCE, "shared-alias");
    assert_eq!(
        install_current_import(&b, &alias, "shared-alias").identity,
        body.identity
    );
    let left = database::checkpoint(&a).unwrap();
    let right = database::checkpoint(&b).unwrap();
    database::merge_apply(&a, 0, &right).unwrap();
    database::merge_apply(&b, 0, &left).unwrap();
    acknowledge_bodies(&a);
    assert!(!a.query_row("SELECT EXISTS(SELECT 1 FROM local_sync_body_hold WHERE json_extract(body_ref,'$.primary')=?1)",[&body.identity],|r|r.get::<_,bool>(0)).unwrap());
    let before = immutable_body_snapshot(&a, &body.identity);
    assert_eq!(
        a.query_row(
            "SELECT COUNT(*) FROM articles WHERE guid='shared-alias'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1,
        "same URL/GUID aliases have one physical reader target"
    );
    super::super::upsert_article_with_feed(&a, af, &input("shared-alias", "new alias RSS"), false)
        .unwrap();
    super::super::backfill_article_content(
        &a,
        aid,
        "<p>alias backfill</p>",
        "alias backfill",
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(immutable_body_snapshot(&a, &body.identity), before);
    for source in [SOURCE, alias.as_str()] {
        let reg = logical::get(
            &a,
            &format!("a/{}/body", model::entry_key(source, "shared-alias")),
        )
        .unwrap()
        .unwrap();
        let FieldValue::Set(reference) = reg.value else {
            panic!("alias lost body reference")
        };
        assert_eq!(reference["primary"], body.identity);
    }
    assert_eq!(
        super::super::article_content_commit_token(&a, aid)
            .unwrap()
            .unwrap()
            .0
            .content_id,
        Some(before.0)
    );
}

/// RSS refresh changes rss_content_id before content_id. Real capture therefore
/// temporarily owns (old primary, new RSS); both exact byte records must survive
/// the last article reference moving, until normal archive/receipt ownership wins.
#[test]
fn rss_revision_mixed_reference_hold_preserves_bytes_until_exact_archive_ack() {
    let c = conn();
    let f = feed(&c);
    let id = seed(&c, f, "rss-mixed-hold", false);
    initialized(&c);
    acknowledge_bodies(&c);
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM local_sync_body_hold", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let old_identity: String = c
        .query_row(
            "SELECT identity FROM contents WHERE id=(SELECT content_id FROM articles WHERE id=?1)",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    let old_bytes = immutable_body_snapshot(&c, &old_identity);
    let input = input("rss-mixed-hold", "new ordinary RSS revision");
    super::super::upsert_article_with_feed(&c, f, &input, false).unwrap();
    logical::drain_local(&c).unwrap();
    let (primary, rss, read, star): (String,String,bool,bool) = c.query_row("SELECT p.identity,r.identity,a.is_read,a.is_starred FROM articles a JOIN contents p ON p.id=a.content_id JOIN contents r ON r.id=a.rss_content_id WHERE a.id=?1", [id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
    assert_eq!(primary, rss);
    assert_ne!(primary, old_identity);
    assert!(
        !read && !star,
        "ordinary refresh does not manufacture a read/star intent"
    );
    let entry = model::entry_key(SOURCE, "rss-mixed-hold");
    let desired = logical::get(&c, &format!("a/{entry}/body"))
        .unwrap()
        .unwrap();
    let FieldValue::Set(desired) = desired.value else {
        panic!("body register disappeared")
    };
    assert_eq!(
        desired["primary"], primary,
        "old primary is no longer selected by L"
    );
    let mixed: String = c.query_row("SELECT body_ref FROM local_sync_body_hold WHERE article_id=?1 AND json_extract(body_ref,'$.primary')=?2 AND json_extract(body_ref,'$.rss')=?3", params![id,old_identity,rss], |r| r.get(0)).unwrap();
    assert_eq!(
        immutable_body_snapshot(&c, &old_identity),
        old_bytes,
        "the intermediate real hold owns the old bytes even after article/L moved"
    );
    let held_before: Vec<String> = {
        let mut stmt = c
            .prepare("SELECT body_ref FROM local_sync_body_hold ORDER BY entry,body_ref")
            .unwrap();
        let rows = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        rows
    };
    let revision_before = logical::revision(&c).unwrap();
    super::super::upsert_article_with_feed(&c, f, &input, false).unwrap();
    logical::drain_local(&c).unwrap();
    let held_after: Vec<String> = {
        let mut stmt = c
            .prepare("SELECT body_ref FROM local_sync_body_hold ORDER BY entry,body_ref")
            .unwrap();
        let rows = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        rows
    };
    assert_eq!(
        held_after, held_before,
        "same GUID/content replay must not accumulate new permanent holds"
    );
    assert_eq!(
        logical::revision(&c).unwrap(),
        revision_before,
        "same bytes replay is not new logical state"
    );
    let pending = storage::seal_bodies(&c, 0).unwrap().unwrap();
    let catalog = crate::local_sync::body_archive::catalog(&c, &pending.manifest).unwrap();
    assert!(catalog.entries.values().any(|row| row.entry == entry
        && row.body_ref.primary == old_identity
        && row.body_ref.rss.as_deref() == Some(rss.as_str())));
    assert!(
        catalog.contents.contains_key(&old_identity) && catalog.contents.contains_key(&rss),
        "sealed archive contains every mixed-ref byte record"
    );
    assert_eq!(immutable_body_snapshot(&c, &old_identity), old_bytes);
    assert!(
        c.query_row(
            "SELECT EXISTS(SELECT 1 FROM local_sync_body_hold WHERE body_ref=?1)",
            [&mixed],
            |r| r.get::<_, bool>(0)
        )
        .unwrap(),
        "sealing alone is not a receipt"
    );
    storage::acknowledge(&c, 0, &pending.manifest).unwrap();
    // No manual hold deletion or forced retention. If the production ACK has
    // released this reference, exact current manifest evidence must exist.
    if !c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM local_sync_body_hold WHERE body_ref=?1)",
            [&mixed],
            |r| r.get::<_, bool>(0),
        )
        .unwrap()
    {
        assert!(c.query_row("SELECT EXISTS(SELECT 1 FROM local_sync_body_receipts WHERE manifest=?1 AND body_ref=?2)", params![pending.manifest.file_name().unwrap(),mixed], |r|r.get::<_,bool>(0)).unwrap());
    }
}
