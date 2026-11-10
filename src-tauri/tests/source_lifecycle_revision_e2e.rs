//! B1–B5 regression coverage, adopted from the independent 2026-11-09 product audit.
mod common;
mod mock_greader;

use app_lib::{commands, db, ingestion, sync};
use mock_greader::MockGReader;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

fn open(name: &str) -> (common::TempDbGuard, rusqlite::Connection) {
    let path = common::unique_db_path(&format!("audit_backend_{name}"));
    let guard = common::TempDbGuard::new(path.clone());
    (guard, db::open(&path).unwrap())
}

fn seed(conn: &rusqlite::Connection, url: &str, body: &str) -> (i64, i64) {
    let folder = db::create_folder(conn, "audit folder", "social").unwrap();
    let feed = db::insert_feed(
        conn,
        url,
        None,
        "audit source",
        None,
        folder,
        "inherit",
        false,
        false,
    )
    .unwrap();
    let article = db::upsert_article_with_feed(conn, feed, &input(body), true)
        .unwrap()
        .0;
    (feed, article)
}

fn input(body: &str) -> db::NewArticle {
    db::NewArticle {
        guid: "stable-local-guid".into(),
        url: Some("https://audit.invalid/article".into()),
        title: "audit article".into(),
        author: None,
        summary: None,
        content_html: Some(format!("<p>{body}</p>")),
        body_text: body.into(),
        image_url: None,
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: Some("2026-11-01T00:00:00Z".into()),
        source: "direct".into(),
    }
}

fn configure(conn: &rusqlite::Connection, server: &MockGReader) {
    db::set_setting(conn, "greader_endpoint", &server.url()).unwrap();
    db::set_setting(conn, "greader_username", "audit").unwrap();
    db::set_setting(conn, "greader_password", "synthetic").unwrap();
    db::set_setting(conn, "sync_protocol", "greader").unwrap();
}

#[test]
fn direct_stable_guid_revision_positive_control() {
    let (_guard, conn) = open("direct_revision_control");
    let (feed, id) = seed(&conn, "https://audit.invalid/feed", "old body");
    let changed =
        db::upsert_article_with_feed_tracked(&conn, feed, &input("revised body"), true).unwrap();
    assert_eq!(changed.id, id);
    assert!(changed.content_changed);
    assert_eq!(
        db::get_article(&conn, id)
            .unwrap()
            .unwrap()
            .content_html
            .as_deref(),
        Some("<p>revised body</p>")
    );
}

#[tokio::test]
async fn remote_stable_id_revision_replaces_rss_content() {
    let (_guard, conn) = open("remote_revision");
    let server = MockGReader::start().await.unwrap();
    configure(&conn, &server);
    let (feed, _) = seed(&conn, "https://audit.invalid/feed", "unrelated local body");
    db::set_feed_remote_id(&conn, feed, 10).unwrap();
    let rid = server.add_entry_full(
        10,
        "https://audit.invalid/remote",
        "remote article",
        true,
        false,
        "<p>old remote body</p>",
        vec![],
    );
    let database = Arc::new(Mutex::new(conn));
    let http = ingestion::build_client(5);
    let first = sync::states_phase(&database, &http, true).await.unwrap();
    assert!(first.errors.is_empty(), "{:?}", first.errors);
    let id = db::article_by_remote_id(&*database.lock().await, rid)
        .unwrap()
        .unwrap();
    {
        let mut entries = server.entries.lock().unwrap();
        let entry = entries.iter_mut().find(|e| e.id == rid).unwrap();
        entry.title = "revised remote title".into();
        entry.content = "<p>revised remote body</p>".into();
    }
    let second = sync::states_phase(&database, &http, true).await.unwrap();
    assert!(second.errors.is_empty(), "{:?}", second.errors);
    let row = db::get_article(&*database.lock().await, id)
        .unwrap()
        .unwrap();
    assert_eq!(
        row.content_html.as_deref(),
        Some("<p>revised remote body</p>"),
        "normal revision of the same remote identity must update its RSS body"
    );
    assert_eq!(row.title, "revised remote title");
    assert!(second.entries_data_changed);
}

#[tokio::test]
async fn retained_favorite_survives_pull_already_in_flight() {
    let (_guard, conn) = open("unsubscribe_inflight");
    let server = MockGReader::start().await.unwrap();
    configure(&conn, &server);
    let (feed, id) = seed(&conn, "https://audit.invalid/feed", "Mock fetched content");
    db::set_feed_remote_id(&conn, feed, 10).unwrap();
    let rid = server.add_entry_ret(
        10,
        "https://audit.invalid/article",
        "audit article",
        true,
        false,
    );
    db::set_article_remote_id(&conn, id, rid).unwrap();
    db::set_starred(&conn, id, true).unwrap();
    let database = Arc::new(Mutex::new(conn));
    let barrier = server.arm_contents_barrier(1);
    let db_for_pull = database.clone();
    let pull = tokio::spawn(async move {
        sync::states_phase(&db_for_pull, &ingestion::build_client(5), true).await
    });
    tokio::time::timeout(Duration::from_secs(5), barrier.arrived)
        .await
        .unwrap()
        .unwrap();
    commands::record_feed_deletion(&*database.lock().await, feed).unwrap();
    barrier.release.send(()).unwrap();
    let report = tokio::time::timeout(Duration::from_secs(8), pull)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let row = db::get_article(&*database.lock().await, id)
        .unwrap()
        .unwrap();
    assert!(row.retained && row.is_starred, "the already-captured active mapping must not clear the favorite after unsubscribe: retained={}, starred={}", row.retained, row.is_starred);
    assert_eq!(
        row.content_html.as_deref(),
        Some("<p>Mock fetched content</p>")
    );
}

#[tokio::test]
async fn active_favorite_accepts_remote_unstar_positive_control() {
    let (_guard, conn) = open("active_remote_control");
    let server = MockGReader::start().await.unwrap();
    configure(&conn, &server);
    let (feed, id) = seed(&conn, "https://audit.invalid/feed", "Mock fetched content");
    db::set_feed_remote_id(&conn, feed, 10).unwrap();
    let rid = server.add_entry_ret(
        10,
        "https://audit.invalid/article",
        "audit article",
        true,
        false,
    );
    db::set_article_remote_id(&conn, id, rid).unwrap();
    db::set_starred(&conn, id, true).unwrap();
    let database = Arc::new(Mutex::new(conn));
    let report = sync::states_phase(&database, &ingestion::build_client(5), true)
        .await
        .unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(
        !db::get_article(&*database.lock().await, id)
            .unwrap()
            .unwrap()
            .is_starred
    );
}

#[test]
fn retired_local_sources_are_absent_from_first_connect_candidates() {
    let (_guard, conn) = open("retained_candidates");
    let (feed, id) = seed(&conn, "https://audit.invalid/retired-feed", "saved body");
    db::set_starred(&conn, id, true).unwrap();
    commands::record_feed_deletion(&conn, feed).unwrap();
    assert!(db::list_feeds(&conn).unwrap().is_empty());
    let candidates = db::list_unbound_local_feeds(&conn).unwrap();
    assert!(
        candidates.is_empty(),
        "a retained-only source must not be offered for subscription push: {candidates:?}"
    );
    assert_eq!(db::count_unbound_local_feeds(&conn).unwrap(), 0);
}

#[test]
fn retired_local_sources_are_absent_from_opml_export() {
    let (_guard, conn) = open("retained_opml");
    let (feed, id) = seed(&conn, "https://audit.invalid/retired-feed", "saved body");
    db::set_starred(&conn, id, true).unwrap();
    commands::record_feed_deletion(&conn, feed).unwrap();
    let export = db::export_feeds_with_folders(&conn).unwrap();
    assert!(
        export.is_empty(),
        "subscription export must not silently re-subscribe an inactive saved source: {export:?}"
    );
    seed(&conn, "https://audit.invalid/active-export", "active body");
    let export = db::export_feeds_with_folders(&conn).unwrap();
    assert_eq!(export.len(), 1);
    assert_eq!(export[0].1, "https://audit.invalid/active-export");
    assert_eq!(db::list_article_sources(&conn).unwrap().len(), 2);
}

#[tokio::test]
async fn remote_reappearing_subscription_reactivates_its_stable_source() {
    let (_guard, conn) = open("remote_resubscribe");
    let server = MockGReader::start().await.unwrap();
    configure(&conn, &server);
    let database = Arc::new(Mutex::new(conn));
    let http = ingestion::build_client(5);
    let first = sync::feeds_phase(&database, &http).await.unwrap();
    assert!(first.errors.is_empty(), "{:?}", first.errors);
    let before = db::list_feeds(&*database.lock().await).unwrap();
    let old = before
        .iter()
        .find(|f| f.feed_url == "http://example.com/remote-only.xml")
        .unwrap();
    let old_id = old.id;
    let old_sync_id = old.sync_id.clone();
    let removed = {
        let mut subscriptions = server.subscriptions.lock().unwrap();
        let index = subscriptions
            .iter()
            .position(|s| s.id == "feed/11")
            .unwrap();
        subscriptions.remove(index)
    };
    let second = sync::feeds_phase(&database, &http).await.unwrap();
    assert!(second.errors.is_empty(), "{:?}", second.errors);
    assert!(db::list_feeds(&*database.lock().await)
        .unwrap()
        .iter()
        .all(|f| f.id != old_id));
    server.subscriptions.lock().unwrap().push(removed);
    let third = sync::feeds_phase(&database, &http).await.unwrap();
    assert!(third.errors.is_empty(), "{:?}", third.errors);
    let active = db::list_feeds(&*database.lock().await).unwrap();
    let restored = active
        .iter()
        .find(|f| f.id == old_id)
        .expect("a source reappearing at the authoritative server must become active again");
    assert_eq!(restored.sync_id, old_sync_id);
    let lifecycle: i64 = database
        .lock()
        .await
        .query_row(
            "SELECT lifecycle_version FROM feeds WHERE id=?1",
            [old_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        lifecycle, 2,
        "remote removal and reappearance each advance the same source lifecycle"
    );
}

#[tokio::test]
async fn first_connect_command_does_not_send_retired_source_to_server() {
    use tauri::Manager;
    let (_guard, conn) = open("retired_first_connect_command");
    let server = MockGReader::start().await.unwrap();
    let retired_url = "https://audit.invalid/retired-feed";
    let (feed, id) = seed(&conn, retired_url, "saved body");
    db::set_starred(&conn, id, true).unwrap();
    commands::record_feed_deletion(&conn, feed).unwrap();
    let active_url = "https://audit.invalid/active-feed";
    seed(&conn, active_url, "active body");
    configure(&conn, &server);
    let app = tauri::test::mock_builder()
        .manage(app_lib::state::AppState::new(
            conn,
            ingestion::build_client(5),
            app_lib::media::MediaHandle::inactive(),
        ))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let response = commands::sync_local_feeds(app.state(), None, None)
        .await
        .unwrap();
    let response = serde_json::to_value(response).unwrap();
    let sent = server.subscribed_urls.lock().unwrap().clone();
    assert!(
        sent.iter().any(|url| url == active_url),
        "active subscription must still push: {sent:?}"
    );
    assert!(!sent.iter().any(|url| url == retired_url), "first-connect full push actually transmitted a retired subscription: sent={sent:?}, response={response}");
}

#[test]
fn missing_guid_shared_url_different_content_is_not_overwritten() {
    let (_guard, conn) = open("weak_url_identity");
    let (feed, _) = seed(&conn, "https://audit.invalid/feed", "unrelated body");
    let xml = r#"<rss version="2.0"><channel><title>audit</title><link>https://audit.invalid/</link><description>audit</description>
      <item><title>first event</title><link>https://audit.invalid/shared</link><description>distinct first content</description></item>
      <item><title>second event</title><link>https://audit.invalid/shared</link><description>distinct second content</description></item>
    </channel></rss>"#;
    let parsed = ingestion::parse_feed(xml.as_bytes(), "https://audit.invalid/feed").unwrap();
    assert_eq!(parsed.articles.len(), 2);
    let mut ids = Vec::new();
    for item in parsed.articles {
        ids.push(
            db::upsert_article_with_feed_source(
                &conn,
                feed,
                &item.article,
                true,
                item.published_at_clamped,
            )
            .unwrap()
            .0,
        );
    }
    assert_ne!(ids[0], ids[1], "synthetic URL key must not become strong identity after content proof rejected equivalence");
}

#[tokio::test]
async fn stale_exact_entry_cannot_cross_unsubscribe_resubscribe_lifecycle() {
    let (_guard, conn) = open("lifecycle_aba");
    let server = MockGReader::start().await.unwrap();
    configure(&conn, &server);
    let (feed, id) = seed(&conn, "https://audit.invalid/feed", "local retained body");
    db::set_feed_remote_id(&conn, feed, 10).unwrap();
    let rid = server.add_entry_ret(
        10,
        "https://audit.invalid/article",
        "old remote title",
        true,
        false,
    );
    db::set_article_remote_id(&conn, id, rid).unwrap();
    db::set_starred(&conn, id, true).unwrap();
    let database = Arc::new(Mutex::new(conn));
    let barrier = server.arm_contents_barrier(1);
    let db_for_pull = database.clone();
    let pull = tokio::spawn(async move {
        sync::states_phase(&db_for_pull, &ingestion::build_client(5), true).await
    });
    tokio::time::timeout(Duration::from_secs(5), barrier.arrived)
        .await
        .unwrap()
        .unwrap();
    {
        let conn = database.lock().await;
        commands::record_feed_deletion(&conn, feed).unwrap();
        let folder = db::create_folder(&conn, "readded", "article").unwrap();
        assert_eq!(
            db::insert_feed(
                &conn,
                "https://audit.invalid/feed",
                None,
                "readded",
                None,
                folder,
                "inherit",
                false,
                false
            )
            .unwrap(),
            feed
        );
    }
    barrier.release.send(()).unwrap();
    let report = tokio::time::timeout(Duration::from_secs(8), pull)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let row = db::get_article(&*database.lock().await, id)
        .unwrap()
        .unwrap();
    assert!(row.is_starred && !row.retained);
    assert_eq!(row.title, "audit article");
    assert_eq!(
        row.content_html.as_deref(),
        Some("<p>local retained body</p>")
    );
}

#[tokio::test]
async fn authority_response_after_unsubscribe_cannot_unstar_retained_article() {
    let (_guard, conn) = open("authority_inflight");
    let server = MockGReader::start().await.unwrap();
    configure(&conn, &server);
    let (feed, id) = seed(&conn, "https://audit.invalid/feed", "saved body");
    db::set_feed_remote_id(&conn, feed, 10).unwrap();
    let rid = server.add_entry_ret(
        10,
        "https://audit.invalid/article",
        "audit article",
        true,
        false,
    );
    db::set_article_remote_id(&conn, id, rid).unwrap();
    db::set_starred(&conn, id, true).unwrap();
    db::set_last_sync_ts(&conn, chrono::Utc::now().timestamp() + 3600).unwrap();
    let database = Arc::new(Mutex::new(conn));
    let barrier = server.arm_stream_ids_barrier("starred");
    let db_for_pull = database.clone();
    let pull = tokio::spawn(async move {
        sync::states_phase(&db_for_pull, &ingestion::build_client(5), false).await
    });
    tokio::time::timeout(Duration::from_secs(5), barrier.arrived)
        .await
        .unwrap()
        .unwrap();
    commands::record_feed_deletion(&*database.lock().await, feed).unwrap();
    barrier.release.send(()).unwrap();
    let report = tokio::time::timeout(Duration::from_secs(8), pull)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let row = db::get_article(&*database.lock().await, id)
        .unwrap()
        .unwrap();
    assert!(row.is_starred && row.retained);
}

#[tokio::test]
async fn local_tombstone_blocks_same_remote_identity_even_after_url_change() {
    let (_guard, conn) = open("local_tombstone");
    let server = MockGReader::start().await.unwrap();
    configure(&conn, &server);
    let database = Arc::new(Mutex::new(conn));
    let http = ingestion::build_client(5);
    assert!(sync::feeds_phase(&database, &http)
        .await
        .unwrap()
        .errors
        .is_empty());
    let old = db::list_feeds(&*database.lock().await)
        .unwrap()
        .into_iter()
        .find(|f| f.feed_url == "http://example.com/remote-only.xml")
        .unwrap();
    commands::record_feed_deletion(&*database.lock().await, old.id).unwrap();
    server
        .unsubscribe_returns_2xx_without_removing
        .store(true, std::sync::atomic::Ordering::SeqCst);
    server
        .subscriptions
        .lock()
        .unwrap()
        .iter_mut()
        .find(|s| s.id == "feed/11")
        .unwrap()
        .url = "https://audit.invalid/renamed.xml".into();
    let report = sync::feeds_phase(&database, &http).await.unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let conn = database.lock().await;
    assert!(db::list_feeds(&conn)
        .unwrap()
        .iter()
        .all(|f| f.id != old.id));
    assert!(db::feed_url_tombstoned(&conn, &old.feed_url).unwrap());
    assert_eq!(
        db::feed_url_by_id(&conn, old.id).unwrap(),
        Some(old.feed_url.clone())
    );
}

#[tokio::test]
async fn remote_revision_splits_rss_group_preserves_readability_and_pending() {
    let (_guard, conn) = open("remote_revision_group");
    let server = MockGReader::start().await.unwrap();
    configure(&conn, &server);
    let (feed, _) = seed(&conn, "https://audit.invalid/feed", "unrelated");
    db::set_feed_remote_id(&conn, feed, 10).unwrap();
    let first = server.add_entry_full(
        10,
        "https://audit.invalid/first",
        "shared title",
        false,
        false,
        "<p>shared body</p>",
        vec![],
    );
    let second = server.add_entry_full(
        10,
        "https://audit.invalid/second",
        "shared title",
        false,
        false,
        "<p>shared body</p>",
        vec![],
    );
    let database = Arc::new(Mutex::new(conn));
    let http = ingestion::build_client(5);
    assert!(sync::states_phase(&database, &http, true)
        .await
        .unwrap()
        .errors
        .is_empty());
    let (id, other, display_id, old_rss, token) = {
        let conn = database.lock().await;
        let id = db::article_by_remote_id(&conn, first).unwrap().unwrap();
        let other = db::article_by_remote_id(&conn, second).unwrap().unwrap();
        db::set_read_with_enqueue(&conn, id, true).unwrap();
        db::set_starred_with_enqueue(&conn, id, true).unwrap();
        let token = db::article_content_commit_token(&conn, id)
            .unwrap()
            .unwrap()
            .0;
        let display_id =
            db::replace_article_content_checked(&conn, id, &token, "<p>protected full article</p>")
                .unwrap()
                .unwrap()
                .0;
        let token = db::article_ai_commit_token(&conn, id).unwrap().unwrap();
        assert!(db::set_article_ai_fields_checked(
            &conn,
            id,
            &token,
            db::AiProduct::Summary {
                text: "cached",
                signature: "old"
            }
        )
        .unwrap());
        let old_rss: i64 = conn
            .query_row(
                "SELECT rss_content_id FROM articles WHERE id=?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        (id, other, display_id, old_rss, token)
    };
    {
        let mut entries = server.entries.lock().unwrap();
        let entry = entries.iter_mut().find(|e| e.id == first).unwrap();
        entry.title = "revised title".into();
        entry.content =
            "<p>revised RSS</p><img src=\"https://audit.invalid/new.jpg\"><script>bad()</script>"
                .into();
        entry.enclosures = vec![mock_greader::MockEnclosure {
            url: "https://audit.invalid/audio.mp3".into(),
            r#type: "audio/mpeg".into(),
            mime_type: "audio/mpeg".into(),
            duration: None,
        }];
    }
    server.set_fail_edit_tag(true);
    let report = sync::states_phase(&database, &http, true).await.unwrap();
    assert!(
        !report.errors.is_empty(),
        "the injected failed push must remain visible"
    );
    assert!(report.entries_data_changed);
    let conn = database.lock().await;
    let row = db::get_article(&conn, id).unwrap().unwrap();
    assert_eq!(row.content_id, Some(display_id));
    assert_eq!(
        row.content_html.as_deref(),
        Some("<p>protected full article</p>")
    );
    assert!(row.is_read && row.is_starred);
    assert!(row.ai_summary.is_none());
    assert!(row.media_revision > 0);
    assert_eq!(
        row.enclosure_url.as_deref(),
        Some("https://audit.invalid/audio.mp3")
    );
    let (rss,html):(i64,String)=conn.query_row("SELECT a.rss_content_id,c.content_html FROM articles a JOIN contents c ON c.id=a.rss_content_id WHERE a.id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_ne!(rss, old_rss);
    assert!(html.contains("revised RSS") && !html.contains("<script"));
    let other_rss: i64 = conn
        .query_row(
            "SELECT rss_content_id FROM articles WHERE id=?1",
            [other],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(other_rss, old_rss);
    assert!(!db::set_article_ai_fields_checked(
        &conn,
        id,
        &token,
        db::AiProduct::Summary {
            text: "late",
            signature: "old"
        }
    )
    .unwrap());
    assert!(db::article_group_has_pending(&conn, id, "read").unwrap());
    assert!(db::article_group_has_pending(&conn, id, "star").unwrap());
}

#[test]
fn sparse_remote_replay_preserves_body_and_ai_when_display_input_unchanged() {
    let (_guard, conn) = open("remote_sparse");
    let (_, id) = seed(&conn, "https://audit.invalid/feed", "body");
    let token = db::article_ai_commit_token(&conn, id).unwrap().unwrap();
    assert!(db::set_article_ai_fields_checked(
        &conn,
        id,
        &token,
        db::AiProduct::Summary {
            text: "cached",
            signature: "same"
        }
    )
    .unwrap());
    let mut sparse = input("");
    sparse.title = String::new();
    sparse.content_html = None;
    sparse.body_text = String::new();
    assert!(!db::refresh_bound_remote_article(&conn, id, &sparse).unwrap());
    let row = db::get_article(&conn, id).unwrap().unwrap();
    assert_eq!(row.ai_summary.as_deref(), Some("cached"));
    assert_eq!(row.content_html.as_deref(), Some("<p>body</p>"));
    assert_eq!(db::article_ai_commit_token(&conn, id).unwrap(), Some(token));
}

#[test]
fn weak_identity_replay_after_body_cleanup_keeps_read_state() {
    let (_guard, mut conn) = open("weak_cache_recovery");
    let (feed, _) = seed(&conn, "https://audit.invalid/feed", "unrelated");
    let xml = r#"<rss version="2.0"><channel><title>f</title><link>https://audit.invalid/</link><description>f</description><item><title>weak</title><link>https://audit.invalid/weak</link><description>replay body</description><pubDate>Sat, 01 Jan 2000 00:00:00 GMT</pubDate></item></channel></rss>"#;
    let parsed = ingestion::parse_feed(xml.as_bytes(), "https://audit.invalid/feed").unwrap();
    let a = &parsed.articles[0].article;
    let (id, inserted) = db::upsert_article_with_feed(&conn, feed, a, true).unwrap();
    assert!(inserted);
    db::set_read(&conn, id, true).unwrap();
    assert_eq!(db::cleanup_cache(&mut conn, 1, "articles").unwrap().0, 1);
    assert!(db::get_article(&conn, id).unwrap().unwrap().body_missing);
    let (replay, inserted) = db::upsert_article_with_feed(&conn, feed, a, true).unwrap();
    assert_eq!(replay, id);
    assert!(!inserted);
    let row = db::get_article(&conn, id).unwrap().unwrap();
    assert!(row.is_read && !row.body_missing);
}

#[test]
fn weak_title_date_empty_and_enclosure_variants_remain_distinct() {
    let (_guard, conn) = open("weak_empty_attachment");
    let (feed, _) = seed(&conn, "https://audit.invalid/feed", "unrelated");
    let xml = r#"<rss version="2.0"><channel><title>f</title><link>https://audit.invalid/</link><description>f</description>
    <item><title>weak</title><pubDate>Sat, 01 Jan 2000 00:00:00 GMT</pubDate><enclosure url="https://audit.invalid/a.mp3" length="1" type="audio/mpeg"/></item>
    <item><title>weak</title><pubDate>Sat, 01 Jan 2000 00:00:00 GMT</pubDate><enclosure url="https://audit.invalid/b.mp3" length="1" type="audio/mpeg"/></item>
    </channel></rss>"#;
    let parsed = ingestion::parse_feed(xml.as_bytes(), "https://audit.invalid/feed").unwrap();
    let first =
        db::upsert_article_with_feed(&conn, feed, &parsed.articles[0].article, true).unwrap();
    let second =
        db::upsert_article_with_feed(&conn, feed, &parsed.articles[1].article, true).unwrap();
    assert!(first.1 && second.1);
    assert_ne!(first.0, second.0);
    assert_eq!(
        db::upsert_article_with_feed(&conn, feed, &parsed.articles[0].article, true).unwrap(),
        (first.0, false)
    );
}

#[test]
fn remote_revision_failure_rolls_back_rss_metadata_ai_and_media() {
    let (_guard, conn) = open("revision_atomic_failure");
    let (_, id) = seed(&conn, "https://audit.invalid/feed", "original");
    let before = db::article_ai_commit_token(&conn, id).unwrap().unwrap();
    assert!(db::set_article_ai_fields_checked(
        &conn,
        id,
        &before,
        db::AiProduct::Summary {
            text: "cached",
            signature: "original"
        }
    )
    .unwrap());
    conn.execute_batch("CREATE TRIGGER fail_revision BEFORE UPDATE OF title ON articles WHEN NEW.title='revised' BEGIN SELECT RAISE(ABORT,'injected metadata failure'); END;").unwrap();
    let mut revised = input("replacement");
    revised.title = "revised".into();
    revised.image_url = Some("https://audit.invalid/new.png".into());
    assert!(db::refresh_bound_remote_article(&conn, id, &revised).is_err());
    assert_eq!(
        db::article_ai_commit_token(&conn, id).unwrap(),
        Some(before)
    );
    let row = db::get_article(&conn, id).unwrap().unwrap();
    assert_eq!(row.content_html.as_deref(), Some("<p>original</p>"));
    assert_eq!(row.ai_summary.as_deref(), Some("cached"));
    assert_eq!(row.media_revision, 0);
    let orphan:i64=conn.query_row("SELECT COUNT(*) FROM contents c WHERE NOT EXISTS(SELECT 1 FROM articles a WHERE a.content_id=c.id OR a.rss_content_id=c.id)",[],|r|r.get(0)).unwrap();
    assert_eq!(orphan, 0);
}

#[test]
fn protected_display_rss_only_revision_reports_group_change_without_invalidating_ai() {
    let (_guard, conn) = open("protected_rss_only");
    let (_, id) = seed(&conn, "https://audit.invalid/feed", "old RSS");
    let token = db::article_content_commit_token(&conn, id)
        .unwrap()
        .unwrap()
        .0;
    db::replace_article_content_checked(&conn, id, &token, "<p>display full text</p>")
        .unwrap()
        .unwrap();
    let before = db::article_ai_commit_token(&conn, id).unwrap().unwrap();
    assert!(db::set_article_ai_fields_checked(
        &conn,
        id,
        &before,
        db::AiProduct::Summary {
            text: "valid cached",
            signature: "display"
        }
    )
    .unwrap());
    assert!(db::refresh_bound_remote_article(&conn, id, &input("new RSS")).unwrap());
    assert_eq!(
        db::article_ai_commit_token(&conn, id).unwrap(),
        Some(before)
    );
    let row = db::get_article(&conn, id).unwrap().unwrap();
    assert_eq!(
        row.content_html.as_deref(),
        Some("<p>display full text</p>")
    );
    assert_eq!(row.ai_summary.as_deref(), Some("valid cached"));
    let (rss,orphan):(String,i64)=conn.query_row("SELECT c.content_html,(SELECT COUNT(*) FROM contents x WHERE NOT EXISTS(SELECT 1 FROM articles a WHERE a.content_id=x.id OR a.rss_content_id=x.id)) FROM articles a JOIN contents c ON c.id=a.rss_content_id WHERE a.id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!(rss, "<p>new RSS</p>");
    assert_eq!(orphan, 0);
    assert!(!db::refresh_bound_remote_article(&conn, id, &input("new RSS")).unwrap());
}

// R1 independent counterexamples and page-cover positive control, assertions preserved.
#[test]
fn weak_date_completion_then_cleanup_replay_keeps_the_proven_identity() {
    let (_guard, mut conn) = open("weak_date_cleanup_combo");
    let (feed, _) = seed(&conn, "https://audit.invalid/feed", "unrelated");
    let parse = |date: &str| {
        let xml = format!(
            r#"<rss version="2.0"><channel><title>f</title><link>https://audit.invalid/</link><description>f</description><item><title>weak no link</title><description>same stable payload</description>{date}</item></channel></rss>"#
        );
        ingestion::parse_feed(xml.as_bytes(), "https://audit.invalid/feed")
            .unwrap()
            .articles
            .remove(0)
    };
    let undated = parse("");
    let dated = parse("<pubDate>Sat, 01 Jan 2000 00:00:00 GMT</pubDate>");
    let first = db::upsert_article_with_feed_source(
        &conn,
        feed,
        &undated.article,
        true,
        undated.published_at_clamped,
    )
    .unwrap();
    let completed = db::upsert_article_with_feed_source(
        &conn,
        feed,
        &dated.article,
        true,
        dated.published_at_clamped,
    )
    .unwrap();
    assert_eq!(
        completed,
        (first.0, false),
        "while RSS bytes exist, completing the date must preserve the proven identity"
    );
    db::set_read(&conn, first.0, true).unwrap();
    assert_eq!(db::cleanup_cache(&mut conn, 1, "articles").unwrap().0, 1);
    assert!(
        db::get_article(&conn, first.0)
            .unwrap()
            .unwrap()
            .body_missing
    );
    let replay = db::upsert_article_with_feed_source(
        &conn,
        feed,
        &dated.article,
        true,
        dated.published_at_clamped,
    )
    .unwrap();
    let before_row = db::get_article(&conn, first.0).unwrap().unwrap();
    let after_row = db::get_article(&conn, replay.0).unwrap().unwrap();
    eprintln!("WEAK_DATE_REPLAY original_id={} original_read={} original_missing={} replay_id={} inserted={} replay_read={} replay_missing={}", first.0, before_row.is_read, before_row.body_missing, replay.0, replay.1, after_row.is_read, after_row.body_missing);
    assert_eq!(
        replay,
        (first.0, false),
        "cache cleanup must not forget the already-proven weak identity after date completion"
    );
    let row = db::get_article(&conn, replay.0).unwrap().unwrap();
    assert!(row.is_read && !row.body_missing);
}

#[tokio::test]
async fn remote_nonempty_revision_removing_rss_image_updates_gallery_version() {
    let (_guard, conn) = open("remote_remove_image");
    let server = MockGReader::start().await.unwrap();
    configure(&conn, &server);
    let (feed, _) = seed(&conn, "https://audit.invalid/feed", "unrelated");
    db::set_feed_remote_id(&conn, feed, 10).unwrap();
    let rid = server.add_entry_full(
        10,
        "https://audit.invalid/image-entry",
        "image entry",
        true,
        false,
        "<p>old body</p><img src=\"https://audit.invalid/old.png\">",
        vec![],
    );
    let database = Arc::new(Mutex::new(conn));
    let http = ingestion::build_client(5);
    let first_report = sync::states_phase(&database, &http, true).await.unwrap();
    assert!(first_report.errors.is_empty(), "{:?}", first_report.errors);
    let id = db::article_by_remote_id(&*database.lock().await, rid)
        .unwrap()
        .unwrap();
    let first = db::get_article(&*database.lock().await, id)
        .unwrap()
        .unwrap();
    assert_eq!(first.images, vec!["https://audit.invalid/old.png"]);
    server
        .entries
        .lock()
        .unwrap()
        .iter_mut()
        .find(|e| e.id == rid)
        .unwrap()
        .content = "<p>revised body without an image</p>".into();
    let report = sync::states_phase(&database, &http, true).await.unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let row = db::get_article(&*database.lock().await, id)
        .unwrap()
        .unwrap();
    assert_eq!(
        row.content_html.as_deref(),
        Some("<p>revised body without an image</p>")
    );
    eprintln!("RSS_IMAGE_REMOVAL old_media_revision={} new_media_revision={} image_count={} image_url={:?} gallery_first={:?} images={:?}", first.media_revision, row.media_revision, row.image_count, row.image_url, row.gallery_first_image_url, row.images);
    assert!(
        row.images.is_empty(),
        "the removed RSS image must not survive as a false fallback cover: {:?}",
        row.images
    );
    assert_eq!(row.image_count, 0);
    assert!(row.gallery_first_image_url.is_none());
    assert!(row.media_revision > first.media_revision);
}

#[test]
fn separate_page_cover_survives_text_only_rss_revision_positive_control() {
    let (_guard, conn) = open("separate_cover_control");
    let (_, id) = seed(
        &conn,
        "https://audit.invalid/feed",
        "old text without inline image",
    );
    db::backfill_article_content(
        &conn,
        id,
        "",
        "",
        Some("https://audit.invalid/page-cover.png"),
        None,
        None,
    )
    .unwrap();
    assert!(db::refresh_bound_remote_article(
        &conn,
        id,
        &input("revised text without inline image")
    )
    .unwrap());
    let row = db::get_article(&conn, id).unwrap().unwrap();
    assert_eq!(row.image_url.as_deref(), Some("https://audit.invalid/page-cover.png"), "separately discovered page covers must not be cleared simply because RSS has no inline image");
}

#[test]
fn weak_date_variants_remain_one_read_identity_across_cleanup_and_reopen() {
    let (_guard, mut conn) = open("weak_date_matrix");
    let path = db::database_path(&conn).unwrap();
    let (feed, _) = seed(&conn, "https://audit.invalid/feed", "unrelated");
    let parse = |date: &str, body: &str| {
        let xml = format!(
            r#"<rss version="2.0"><channel><title>f</title><link>https://audit.invalid/</link><description>f</description><item><title>weak no link</title><description>{body}</description>{date}</item></channel></rss>"#
        );
        ingestion::parse_feed(xml.as_bytes(), "https://audit.invalid/feed")
            .unwrap()
            .articles
            .remove(0)
    };
    let variants = [
        parse(
            "<pubDate>Wed, 24 Aug 2033 11:00:00 GMT</pubDate>",
            "stable body",
        ),
        parse(
            "<pubDate>Sat, 01 Jan 2000 00:00:00 GMT</pubDate>",
            "stable body",
        ),
        parse("", "stable body"),
    ];
    let first = db::upsert_article_with_feed_source(
        &conn,
        feed,
        &variants[0].article,
        true,
        variants[0].published_at_clamped,
    )
    .unwrap()
    .0;
    let guid: String = conn
        .query_row("SELECT guid FROM articles WHERE id=?1", [first], |r| {
            r.get(0)
        })
        .unwrap();
    db::set_read(&conn, first, true).unwrap();
    for a in &variants[1..] {
        assert_eq!(
            db::upsert_article_with_feed_source(
                &conn,
                feed,
                &a.article,
                true,
                a.published_at_clamped
            )
            .unwrap(),
            (first, false)
        );
    }
    for index in [1, 0, 2, 1] {
        assert_eq!(db::cleanup_cache(&mut conn, 1, "articles").unwrap().0, 1);
        drop(conn);
        conn = db::open(&path).unwrap();
        let a = &variants[index];
        assert_eq!(
            db::upsert_article_with_feed_source(
                &conn,
                feed,
                &a.article,
                true,
                a.published_at_clamped
            )
            .unwrap(),
            (first, false)
        );
        let row = db::get_article(&conn, first).unwrap().unwrap();
        assert!(row.is_read && !row.body_missing);
        assert_eq!(
            conn.query_row("SELECT guid FROM articles WHERE id=?1", [first], |r| r
                .get::<_, String>(
                0
            ))
            .unwrap(),
            guid
        );
    }
    let different = parse(
        "<pubDate>Sat, 01 Jan 2000 00:00:00 GMT</pubDate>",
        "different body",
    );
    let other = db::upsert_article_with_feed_source(
        &conn,
        feed,
        &different.article,
        true,
        different.published_at_clamped,
    )
    .unwrap();
    assert!(other.1);
    assert_ne!(other.0, first);
    assert!(!db::get_article(&conn, other.0).unwrap().unwrap().is_read);
    assert!(db::get_article(&conn, first).unwrap().unwrap().is_read);
}

#[test]
fn weak_date_completion_preserves_star_read_versions_and_pending_operations() {
    let (_guard, mut conn) = open("weak_date_pending");
    let (feed, _) = seed(&conn, "https://audit.invalid/feed", "unrelated");
    let parse = |date: &str| {
        let xml = format!(
            r#"<rss version="2.0"><channel><title>f</title><link>https://audit.invalid/</link><description>f</description><item><title>weak</title><description>unchanged</description>{date}</item></channel></rss>"#
        );
        ingestion::parse_feed(xml.as_bytes(), "https://audit.invalid/feed")
            .unwrap()
            .articles
            .remove(0)
    };
    let undated = parse("");
    let dated = parse("<pubDate>Sat, 01 Jan 2000 00:00:00 GMT</pubDate>");
    let id = db::upsert_article_with_feed(&conn, feed, &undated.article, true)
        .unwrap()
        .0;
    db::set_read_with_enqueue(&conn, id, true).unwrap();
    db::set_starred_with_enqueue(&conn, id, true).unwrap();
    let queued: Vec<i64> = db::take_sync_queue(&conn)
        .unwrap()
        .into_iter()
        .map(|q| q.id)
        .collect();
    let versions = db::article_field_versions(&conn, id).unwrap();
    assert_eq!(
        db::upsert_article_with_feed(&conn, feed, &dated.article, true).unwrap(),
        (id, false)
    );
    assert_eq!(
        db::take_sync_queue(&conn)
            .unwrap()
            .into_iter()
            .map(|q| q.id)
            .collect::<Vec<_>>(),
        queued
    );
    assert_eq!(db::article_field_versions(&conn, id).unwrap(), versions);
    let row = db::get_article(&conn, id).unwrap().unwrap();
    assert!(row.is_read && row.is_starred);
    assert_eq!(db::cleanup_cache(&mut conn, 1, "articles").unwrap().0, 0);
    assert!(!db::get_article(&conn, id).unwrap().unwrap().body_missing);
}

#[test]
fn empty_weak_payload_does_not_equate_distinct_dates() {
    let (_guard, conn) = open("weak_empty_dates");
    let (feed, _) = seed(&conn, "https://audit.invalid/feed", "unrelated");
    let mut ids = Vec::new();
    for date in [
        "Sat, 01 Jan 2000 00:00:00 GMT",
        "Sun, 02 Jan 2000 00:00:00 GMT",
    ] {
        let xml = format!(
            r#"<rss version="2.0"><channel><title>f</title><link>https://audit.invalid/</link><description>f</description><item><title>same</title><pubDate>{date}</pubDate></item></channel></rss>"#
        );
        let a = ingestion::parse_feed(xml.as_bytes(), "https://audit.invalid/feed")
            .unwrap()
            .articles
            .remove(0)
            .article;
        let result = db::upsert_article_with_feed(&conn, feed, &a, true).unwrap();
        assert!(result.1);
        ids.push(result.0);
        assert_eq!(
            db::upsert_article_with_feed(&conn, feed, &a, true).unwrap(),
            (result.0, false)
        );
    }
    assert_ne!(ids[0], ids[1]);
}

#[tokio::test]
async fn remote_rss_media_add_replace_remove_and_sparse_replay_share_revision_rules() {
    let (_guard, conn) = open("remote_media_matrix");
    let server = MockGReader::start().await.unwrap();
    configure(&conn, &server);
    let (feed, _) = seed(&conn, "https://audit.invalid/feed", "unrelated");
    db::set_feed_remote_id(&conn, feed, 10).unwrap();
    let rid = server.add_entry_full(
        10,
        "https://audit.invalid/media",
        "media",
        true,
        false,
        "<p>text</p>",
        vec![],
    );
    let database = Arc::new(Mutex::new(conn));
    let http = ingestion::build_client(5);
    assert!(sync::states_phase(&database, &http, true)
        .await
        .unwrap()
        .errors
        .is_empty());
    let id = db::article_by_remote_id(&*database.lock().await, rid)
        .unwrap()
        .unwrap();
    let mut previous = db::get_article(&*database.lock().await, id)
        .unwrap()
        .unwrap();
    for (html, expected, changes) in [
        (
            "<p>image A</p><img src=\"https://audit.invalid/a.png\">",
            Some("https://audit.invalid/a.png"),
            true,
        ),
        (
            "<p>image B</p><img src=\"https://audit.invalid/b.png\">",
            Some("https://audit.invalid/b.png"),
            true,
        ),
        ("", Some("https://audit.invalid/b.png"), false),
        ("<p>without image</p>", None, true),
        ("<p>without image</p>", None, false),
    ] {
        server
            .entries
            .lock()
            .unwrap()
            .iter_mut()
            .find(|e| e.id == rid)
            .unwrap()
            .content = html.into();
        let report = sync::states_phase(&database, &http, true).await.unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        let row = db::get_article(&*database.lock().await, id)
            .unwrap()
            .unwrap();
        assert_eq!(row.image_url.as_deref(), expected);
        assert_eq!(row.gallery_first_image_url.as_deref(), expected);
        assert_eq!(row.image_count, i64::from(expected.is_some()));
        assert_eq!(
            row.images,
            expected.map(|v| vec![v.to_string()]).unwrap_or_default()
        );
        if changes {
            assert!(row.media_revision > previous.media_revision);
        } else {
            assert_eq!(row.media_revision, previous.media_revision);
        }
        previous = row;
    }
}

#[test]
fn page_cover_survives_rss_images_and_fulltext_then_cache_clear_removes_media() {
    let (_guard, mut conn) = open("page_rss_fulltext_media");
    let (feed, id) = seed(&conn, "https://audit.invalid/feed", "plain RSS");
    db::update_article_image_if_empty(&conn, id, "https://audit.invalid/page.png").unwrap();
    let mut a = input("RSS image");
    a.content_html = Some("<p>RSS image</p><img src=\"https://audit.invalid/rss.png\">".into());
    a.image_url = Some("https://audit.invalid/rss.png".into());
    assert_eq!(
        db::upsert_article_with_feed(&conn, feed, &a, true)
            .unwrap()
            .0,
        id
    );
    let row = db::get_article(&conn, id).unwrap().unwrap();
    assert_eq!(
        row.image_url.as_deref(),
        Some("https://audit.invalid/page.png")
    );
    assert_eq!(
        row.gallery_first_image_url.as_deref(),
        Some("https://audit.invalid/rss.png")
    );
    let token = db::article_content_commit_token(&conn, id)
        .unwrap()
        .unwrap()
        .0;
    db::replace_article_content_checked(
        &conn,
        id,
        &token,
        "<p>full text</p><img src=\"https://audit.invalid/full.png\">",
    )
    .unwrap()
    .unwrap();
    let extracted = db::get_article(&conn, id).unwrap().unwrap();
    assert!(extracted.media_revision > row.media_revision);
    assert_eq!(
        extracted.gallery_first_image_url.as_deref(),
        Some("https://audit.invalid/full.png")
    );
    assert_eq!(extracted.image_url, row.image_url);
    assert!(db::refresh_bound_remote_article(&conn, id, &input("new plain RSS")).unwrap());
    let revised = db::get_article(&conn, id).unwrap().unwrap();
    assert_eq!(revised.images, extracted.images);
    assert_eq!(revised.media_revision, extracted.media_revision);
    assert_eq!(revised.image_url, extracted.image_url);
    conn.execute(
        "UPDATE articles SET published_at='2000-01-01T00:00:00Z' WHERE id=?1",
        [id],
    )
    .unwrap();
    db::set_read(&conn, id, true).unwrap();
    assert_eq!(db::cleanup_cache(&mut conn, 1, "articles").unwrap().0, 1);
    let cleared = db::get_article(&conn, id).unwrap().unwrap();
    assert!(
        cleared.images.is_empty()
            && cleared.image_url.is_none()
            && cleared.gallery_first_image_url.is_none()
    );
    assert_eq!(cleared.image_count, 0);
    assert!(cleared.media_revision > revised.media_revision);
}

#[test]
fn media_provenance_is_not_inferred_from_equal_urls_and_attachment_revision_is_uniform() {
    let (_guard, conn) = open("media_provenance_and_attachment");
    let (feed, id) = seed(&conn, "https://audit.invalid/feed", "plain");
    db::update_article_image_if_empty(&conn, id, "https://audit.invalid/shared.png").unwrap();
    let mut a = input("image");
    a.content_html = Some("<p>image</p><img src=\"https://audit.invalid/shared.png\">".into());
    a.image_url = Some("https://audit.invalid/shared.png".into());
    db::upsert_article_with_feed(&conn, feed, &a, true).unwrap();
    db::refresh_bound_remote_article(&conn, id, &input("no RSS images")).unwrap();
    let page = db::get_article(&conn, id).unwrap().unwrap();
    assert_eq!(
        page.image_url.as_deref(),
        Some("https://audit.invalid/shared.png")
    );
    assert_eq!(
        conn.query_row("SELECT image_source FROM articles WHERE id=?1", [id], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "page"
    );
    let mut podcast = input("no RSS images");
    podcast.enclosure_url = Some("https://audit.invalid/a.mp3".into());
    podcast.enclosure_mime = Some("audio/mpeg".into());
    podcast.duration_sec = Some(25);
    db::upsert_article_with_feed(&conn, feed, &podcast, true).unwrap();
    let audio = db::get_article(&conn, id).unwrap().unwrap();
    assert!(audio.media_revision > page.media_revision);
    db::upsert_article_with_feed(&conn, feed, &podcast, true).unwrap();
    assert_eq!(
        db::get_article(&conn, id).unwrap().unwrap().media_revision,
        audio.media_revision
    );
    podcast.duration_sec = Some(30);
    db::refresh_bound_remote_article(&conn, id, &podcast).unwrap();
    assert!(db::get_article(&conn, id).unwrap().unwrap().media_revision > audio.media_revision);
}

// R2 independent empty-body counterexample, original assertions retained.
#[test]
fn empty_shared_url_with_distinct_dates_keeps_distinct_candidates_and_replays_idempotently() {
    let (_guard, conn) = open("empty_url_dates");
    let (feed, _) = seed(&conn, "https://audit.invalid/feed", "unrelated");
    let mut ids = Vec::new();
    for date in [
        "Sat, 01 Jan 2000 00:00:00 GMT",
        "Sun, 02 Jan 2000 00:00:00 GMT",
    ] {
        let xml = format!(
            r#"<rss version="2.0"><channel><title>f</title><link>https://audit.invalid/</link><description>f</description><item><title>empty notification</title><link>https://audit.invalid/shared</link><pubDate>{date}</pubDate></item></channel></rss>"#
        );
        let parsed = ingestion::parse_feed(xml.as_bytes(), "https://audit.invalid/feed").unwrap();
        let a = &parsed.articles[0].article;
        assert!(a.content_html.as_deref().unwrap_or("").is_empty() && a.body_text.is_empty());
        let result = db::upsert_article_with_feed(&conn, feed, a, true).unwrap();
        ids.push(result.0);
        assert_eq!(
            db::upsert_article_with_feed(&conn, feed, a, true).unwrap(),
            (result.0, false),
            "identical empty observations must not grow without bound"
        );
        eprintln!(
            "EMPTY_URL_DATE observed_date={date} id={} inserted={} stored_published={:?}",
            result.0,
            result.1,
            db::get_article(&conn, result.0)
                .unwrap()
                .unwrap()
                .published_at
        );
    }
    assert_ne!(ids[0], ids[1], "different observable dates with no body proof must not be collapsed solely because their URLs and titles match");
}

#[test]
fn empty_observation_dates_are_stable_and_distinct_across_real_parse_and_reopen() {
    for link in ["", "<link>https://audit.invalid/shared</link>"] {
        let (_guard, mut conn) = open("empty_observation_matrix");
        let path = db::database_path(&conn).unwrap();
        let (feed, _) = seed(&conn, "https://audit.invalid/feed", "unrelated");
        let parse = |date: &str| {
            let xml = format!(
                r#"<rss version="2.0"><channel><title>f</title><link>https://audit.invalid/</link><description>f</description><item><title>empty</title>{link}{date}</item></channel></rss>"#
            );
            ingestion::parse_feed(xml.as_bytes(), "https://audit.invalid/feed")
                .unwrap()
                .articles
                .remove(0)
        };
        let dates = [
            "",
            "<pubDate>Sat, 01 Jan 2000 00:00:00 GMT</pubDate>",
            "<pubDate>Sun, 02 Jan 2000 00:00:00 GMT</pubDate>",
            "<pubDate>Wed, 24 Aug 2033 11:00:00 GMT</pubDate>",
            "<pubDate>Thu, 25 Aug 2033 11:00:00 GMT</pubDate>",
        ];
        let mut ids = std::collections::HashSet::new();
        for date in dates {
            let a = parse(date);
            let first = db::upsert_article_with_feed_source(
                &conn,
                feed,
                &a.article,
                true,
                a.published_at_clamped,
            )
            .unwrap();
            assert!(first.1);
            assert!(ids.insert(first.0));
            db::set_read_with_enqueue(&conn, first.0, true).unwrap();
            db::set_starred_with_enqueue(&conn, first.0, true).unwrap();
            let versions = db::article_field_versions(&conn, first.0).unwrap();
            let published = db::get_article(&conn, first.0)
                .unwrap()
                .unwrap()
                .published_at;
            drop(conn);
            conn = db::open(&path).unwrap();
            let repeat = parse(date);
            assert_eq!(
                db::upsert_article_with_feed_source(
                    &conn,
                    feed,
                    &repeat.article,
                    true,
                    repeat.published_at_clamped
                )
                .unwrap(),
                (first.0, false)
            );
            let row = db::get_article(&conn, first.0).unwrap().unwrap();
            assert!(row.is_read && row.is_starred && row.body_missing);
            assert_eq!(row.published_at, published);
            assert_eq!(
                db::article_field_versions(&conn, first.0).unwrap(),
                versions
            );
            assert!(db::article_group_has_pending(&conn, first.0, "read").unwrap());
            assert!(db::article_group_has_pending(&conn, first.0, "star").unwrap());
        }
        assert_eq!(ids.len(), 5);
    }
}

#[test]
fn weak_empty_and_filled_observations_do_not_inherit_state_or_grow_on_replay() {
    for filled_first in [false, true] {
        let (_guard, conn) = open("weak_empty_filled_boundary");
        let (feed, _) = seed(&conn, "https://audit.invalid/feed", "unrelated");
        let parse = |body: &str| {
            let xml = format!(
                r#"<rss version="2.0"><channel><title>f</title><link>https://audit.invalid/</link><description>f</description><item><title>notification</title><link>https://audit.invalid/notification</link><pubDate>Sat, 01 Jan 2000 00:00:00 GMT</pubDate>{body}</item></channel></rss>"#
            );
            ingestion::parse_feed(xml.as_bytes(), "https://audit.invalid/feed")
                .unwrap()
                .articles
                .remove(0)
                .article
        };
        let empty = parse("");
        let filled = parse("<description>confirmed body</description>");
        let first_input = if filled_first { &filled } else { &empty };
        let second_input = if filled_first { &empty } else { &filled };
        let first = db::upsert_article_with_feed(&conn, feed, first_input, true).unwrap();
        assert!(first.1);
        db::set_read_with_enqueue(&conn, first.0, true).unwrap();
        db::set_starred_with_enqueue(&conn, first.0, true).unwrap();
        let queued: Vec<_> = db::take_sync_queue(&conn)
            .unwrap()
            .into_iter()
            .map(|q| q.id)
            .collect();
        let versions = db::article_field_versions(&conn, first.0).unwrap();
        let second = db::upsert_article_with_feed(&conn, feed, second_input, true).unwrap();
        assert!(second.1);
        assert_ne!(first.0, second.0);
        for _ in 0..3 {
            assert_eq!(
                db::upsert_article_with_feed(&conn, feed, first_input, true).unwrap(),
                (first.0, false)
            );
            assert_eq!(
                db::upsert_article_with_feed(&conn, feed, second_input, true).unwrap(),
                (second.0, false)
            );
        }
        let retained = db::get_article(&conn, first.0).unwrap().unwrap();
        let separate = db::get_article(&conn, second.0).unwrap().unwrap();
        assert!(retained.is_read && retained.is_starred);
        assert!(!separate.is_read && !separate.is_starred);
        assert_eq!(retained.body_missing, !filled_first);
        assert_eq!(separate.body_missing, filled_first);
        assert_eq!(
            db::article_field_versions(&conn, first.0).unwrap(),
            versions
        );
        assert_eq!(
            db::take_sync_queue(&conn)
                .unwrap()
                .into_iter()
                .map(|q| q.id)
                .collect::<Vec<_>>(),
            queued
        );
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM articles WHERE feed_id=?1",
                [feed],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 3,
            "one seed plus one empty observation and one content identity"
        );
    }
}

#[test]
fn real_guid_empty_body_completion_keeps_identity_state_and_pending() {
    let (_guard, conn) = open("real_guid_empty_completion");
    let (feed, _) = seed(&conn, "https://audit.invalid/feed", "unrelated");
    let parse = |body: &str| {
        let xml = format!(
            r#"<rss version="2.0"><channel><title>f</title><link>https://audit.invalid/</link><description>f</description><item><guid isPermaLink="false">strong-guid</guid><title>notification</title><link>https://audit.invalid/notification</link>{body}</item></channel></rss>"#
        );
        ingestion::parse_feed(xml.as_bytes(), "https://audit.invalid/feed")
            .unwrap()
            .articles
            .remove(0)
            .article
    };
    let first = db::upsert_article_with_feed(&conn, feed, &parse(""), true).unwrap();
    assert!(first.1);
    db::set_read_with_enqueue(&conn, first.0, true).unwrap();
    db::set_starred_with_enqueue(&conn, first.0, true).unwrap();
    let queue: Vec<_> = db::take_sync_queue(&conn)
        .unwrap()
        .into_iter()
        .map(|q| q.id)
        .collect();
    assert_eq!(
        db::upsert_article_with_feed(
            &conn,
            feed,
            &parse("<description>filled strong body</description>"),
            true
        )
        .unwrap(),
        (first.0, false)
    );
    let row = db::get_article(&conn, first.0).unwrap().unwrap();
    assert!(row.is_read && row.is_starred && !row.body_missing);
    assert!(row.content_html.unwrap().contains("filled strong body"));
    assert_eq!(
        db::take_sync_queue(&conn)
            .unwrap()
            .into_iter()
            .map(|q| q.id)
            .collect::<Vec<_>>(),
        queue
    );
}

#[tokio::test]
async fn remote_stable_id_empty_body_completion_keeps_identity_and_local_pending() {
    let (_guard, conn) = open("remote_empty_completion");
    let server = MockGReader::start().await.unwrap();
    configure(&conn, &server);
    let (feed, _) = seed(&conn, "https://audit.invalid/feed", "unrelated");
    db::set_feed_remote_id(&conn, feed, 10).unwrap();
    let rid = server.add_entry_full(
        10,
        "https://audit.invalid/remote-empty",
        "remote empty",
        false,
        false,
        "",
        vec![],
    );
    let database = Arc::new(Mutex::new(conn));
    let http = ingestion::build_client(5);
    assert!(sync::states_phase(&database, &http, true)
        .await
        .unwrap()
        .errors
        .is_empty());
    let (id, queued) = {
        let conn = database.lock().await;
        let id = db::article_by_remote_id(&conn, rid).unwrap().unwrap();
        assert!(db::get_article(&conn, id).unwrap().unwrap().body_missing);
        db::set_read_with_enqueue(&conn, id, true).unwrap();
        db::set_starred_with_enqueue(&conn, id, true).unwrap();
        (
            id,
            db::take_sync_queue(&conn)
                .unwrap()
                .into_iter()
                .map(|q| q.id)
                .collect::<Vec<_>>(),
        )
    };
    server.set_fail_edit_tag(true);
    server
        .entries
        .lock()
        .unwrap()
        .iter_mut()
        .find(|e| e.id == rid)
        .unwrap()
        .content = "<p>remote body completed</p>".into();
    let report = sync::states_phase(&database, &http, true).await.unwrap();
    assert!(
        !report.errors.is_empty(),
        "the injected failed push remains visible"
    );
    assert!(report.entries_data_changed);
    let conn = database.lock().await;
    assert_eq!(db::article_by_remote_id(&conn, rid).unwrap(), Some(id));
    let row = db::get_article(&conn, id).unwrap().unwrap();
    assert!(row.is_read && row.is_starred && !row.body_missing);
    assert_eq!(
        row.content_html.as_deref(),
        Some("<p>remote body completed</p>")
    );
    assert_eq!(
        db::take_sync_queue(&conn)
            .unwrap()
            .into_iter()
            .map(|q| q.id)
            .collect::<Vec<_>>(),
        queued
    );
}
