//! Must be run by the reserved WsgiDAV harness. Both clients use production
//! connect/run_lane and disk-backed SQLite, with disposable synthetic data only.
use app_lib::{
    db,
    local_sync::{
        database,
        runtime::{self, LocalRuntime},
        storage,
    },
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Mutex;
fn env() -> (String, PathBuf) {
    let url = std::env::var("FLUXREADER_LOCAL_SYNC_TEST_URL").expect("reserved service required");
    let root = PathBuf::from(std::env::var("FLUXREADER_LOCAL_SYNC_EVIDENCE").expect("owned root"));
    assert!(root.starts_with(Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()));
    (url, root)
}
fn open(path: &Path) -> Arc<Mutex<rusqlite::Connection>> {
    Arc::new(Mutex::new(db::open(path).unwrap()))
}
async fn seed(database: &Arc<Mutex<rusqlite::Connection>>) -> i64 {
    let conn = database.lock().await;
    let group = db::create_folder(&conn, "Group", "article").unwrap();
    let feed = db::insert_feed(
        &conn,
        "https://example.test/rss",
        None,
        "Feed",
        None,
        group,
        "inherit",
        false,
        false,
    )
    .unwrap();
    let a = db::NewArticle {
        guid: "real-guid".into(),
        url: Some("https://example.test/article".into()),
        title: "Saved article".into(),
        author: None,
        summary: Some("full saved body".into()),
        content_html: Some("<p>full saved body</p>".into()),
        body_text: "full saved body".into(),
        image_url: None,
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: Some("2026-11-09T00:00:00Z".into()),
        source: "direct".into(),
    };
    db::upsert_article_with_feed(&conn, feed, &a, false)
        .unwrap()
        .0
}
async fn sync(db: &Arc<Mutex<rusqlite::Connection>>, rt: &LocalRuntime, lane: &str) {
    for _ in 0..10 {
        if lane == "state" {
            for _ in 0..256 {
                if !runtime::recover_local_slice(db, rt).await.unwrap() {
                    break;
                }
            }
        }
        match runtime::run_lane(db, rt, lane).await {
            Ok(()) => return,
            Err(e) if e.code == "localSyncPending" => {}
            Err(e) => panic!("{lane}: {e}"),
        }
    }
    panic!("bounded rounds did not complete")
}
async fn connect(db: &Arc<Mutex<rusqlite::Connection>>, url: &str) {
    runtime::connect(
        db,
        url.into(),
        "synthetic".into(),
        "synthetic-password".into(),
        "realtime".into(),
        1,
    )
    .await
    .unwrap();
}
async fn wait(path: &Path) {
    let start = std::time::Instant::now();
    while !path.exists() {
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "provider barrier never entered"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires coordinator-granted real WebDAV fast timing window"]
async fn real_fast_round_same_dependency_commit_seal_store_and_visibility() {
    use runtime::FastPhase;
    let (url, root) = env();
    let a = open(&root.join("fast-a.db"));
    let b = open(&root.join("fast-b.db"));
    let aid = seed(&a).await;
    let ra = Arc::new(LocalRuntime::default());
    let rb = LocalRuntime::default();
    connect(&a, &url).await;
    connect(&b, &url).await;
    sync(&a, &ra, "state").await;
    sync(&b, &rb, "state").await;
    for _ in 0..256 {
        if !runtime::recover_local_slice(&b, &rb).await.unwrap() {
            break;
        }
    }
    let bid: i64 = b
        .lock()
        .await
        .query_row("SELECT id FROM articles WHERE guid='real-guid'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let started = std::time::Instant::now();
    let target = {
        let c = a.lock().await;
        db::set_read_with_enqueue(&c, aid, true).unwrap();
        c.query_row("SELECT seq FROM local_sync_control", [], |r| {
            r.get::<_, u64>(0)
        })
        .unwrap()
    };
    ra.notify_local_commit();
    let sender = a.clone();
    let runtime = ra.clone();
    let send =
        tokio::spawn(
            async move { runtime::run_fast_round(&sender, &runtime, FastPhase::Send).await },
        );
    loop {
        let sealed: bool = a
            .lock()
            .await
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM local_sync_fast_objects WHERE last_seq>=?1)",
                [target],
                |r| r.get(0),
            )
            .unwrap();
        if sealed {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_millis(250),
            "commit to durable fast seal exceeded fixed 250ms"
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    let sealed_ms = started.elapsed().as_secs_f64() * 1000.0;
    assert!(
        sealed_ms <= 250.0,
        "commit to observed durable fast seal exceeded fixed 250ms"
    );
    send.await.unwrap().unwrap();
    let stored_ms = started.elapsed().as_secs_f64() * 1000.0;
    assert!(
        stored_ms <= 1000.0,
        "exact stored receipt exceeded fixed 1s"
    );
    loop {
        runtime::run_fast_round(&b, &rb, FastPhase::Discover)
            .await
            .unwrap();
        let visible: bool = b
            .lock()
            .await
            .query_row("SELECT is_read FROM articles WHERE id=?1", [bid], |r| {
                r.get(0)
            })
            .unwrap();
        if visible {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "same-dependency visibility exceeded fixed 2s"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let visible_ms = started.elapsed().as_secs_f64() * 1000.0;
    assert!(
        visible_ms <= 2000.0,
        "same-dependency visibility exceeded fixed 2s"
    );
    std::fs::write(root.join("fast-timing.json"),serde_json::to_vec_pretty(&serde_json::json!({"target_seq":target,"seal_observed_ms":sealed_ms,"exact_stored_ms":stored_ms,"visible_ms":visible_ms,"scope":"same dependencies; both production run_fast_round branches; no full-checkpoint pause or missing-dependency scenario claimed"})).unwrap()).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires coordinator-granted real WebDAV window"]
async fn real_two_clients_offline_restart_unknown_reply_and_body_restore() {
    let (url, root) = env();
    let a_path = root.join("a.db");
    let b_path = root.join("b.db");
    let a = open(&a_path);
    let b = open(&b_path);
    let aid = seed(&a).await;
    let ra = Arc::new(LocalRuntime::default());
    let rb = LocalRuntime::default();
    connect(&a, &url).await;
    connect(&b, &url).await;
    sync(&a, &ra, "state").await;
    sync(&a, &ra, "body").await;
    sync(&b, &rb, "state").await;
    sync(&b, &rb, "body").await;
    let bid = {
        let conn = b.lock().await;
        assert_eq!(database::pending_bodies(&conn).unwrap(), 0);
        let id = conn
            .query_row("SELECT id FROM articles", [], |r| r.get::<_, i64>(0))
            .unwrap();
        assert_eq!(
            db::get_article(&conn, id)
                .unwrap()
                .unwrap()
                .content_html
                .as_deref(),
            Some("<p>full saved body</p>")
        );
        id
    };
    db::set_read(&*a.lock().await, aid, true).unwrap();
    db::set_starred(&*b.lock().await, bid, true).unwrap();
    sync(&a, &ra, "state").await;
    sync(&b, &rb, "state").await;
    sync(&a, &ra, "state").await;
    for database in [&a, &b] {
        let conn = database.lock().await;
        let flags = conn
            .query_row("SELECT is_read,is_starred FROM articles", [], |r| {
                Ok((r.get::<_, bool>(0)?, r.get::<_, bool>(1)?))
            })
            .unwrap();
        assert_eq!(flags, (true, true));
    }
    // Server writes a complete immutable PUT, then withholds only its HTTP reply.
    // Cancelling the client models process loss after remote storage, not fake success.
    db::set_read(&*a.lock().await, aid, false).unwrap();
    let actor = database::actor(&*a.lock().await).unwrap();
    std::fs::write(root.join("pause-next-put"), &actor).unwrap();
    let task = {
        let a = a.clone();
        let ra = ra.clone();
        tokio::spawn(async move { runtime::run_lane(&a, &ra, "state").await })
    };
    wait(&root.join("put-stored")).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let sealed = {
        let conn = a.lock().await;
        storage::pending(&conn, "state", database::generation(&conn).unwrap())
            .unwrap()
            .unwrap()
    };
    std::fs::write(root.join("release-put"), "resume").unwrap();
    drop(a);
    let a = open(&a_path);
    {
        let conn = a.lock().await;
        assert_eq!(
            storage::pending(&conn, "state", database::generation(&conn).unwrap())
                .unwrap()
                .unwrap()
                .manifest
                .file_name()
                .unwrap(),
            sealed.manifest.file_name().unwrap()
        );
    }
    sync(&a, &ra, "state").await;
    sync(&b, &rb, "state").await;
    assert!(!b
        .lock()
        .await
        .query_row("SELECT is_read FROM articles", [], |r| r.get::<_, bool>(0))
        .unwrap());
    // Retired saved source and body remain readable on a third fresh database.
    {
        let conn = a.lock().await;
        let feed = conn
            .query_row("SELECT id FROM feeds", [], |r| r.get::<_, i64>(0))
            .unwrap();
        db::delete_feed(&conn, feed).unwrap();
    }
    sync(&a, &ra, "state").await;
    sync(&a, &ra, "body").await;
    sync(&b, &rb, "state").await;
    let c = open(&root.join("c.db"));
    let rc = LocalRuntime::default();
    connect(&c, &url).await;
    sync(&c, &rc, "state").await;
    sync(&c, &rc, "body").await;
    {
        let conn = c.lock().await;
        assert!(!conn
            .query_row("SELECT active FROM feeds", [], |r| r.get::<_, bool>(0))
            .unwrap());
        assert_eq!(database::pending_bodies(&conn).unwrap(), 0);
        assert!(conn
            .query_row("SELECT is_starred FROM articles", [], |r| r
                .get::<_, bool>(0))
            .unwrap());
    }
    // A delayed successful response must not revive a disconnected generation.
    for name in ["put-stored", "release-put", "pause-consumed"] {
        std::fs::remove_file(root.join(name)).unwrap();
    }
    {
        let conn = a.lock().await;
        db::set_setting(&conn, "app_settings", r#"{"themeMode":"dark"}"#).unwrap();
    }
    std::fs::write(
        root.join("pause-next-put"),
        database::actor(&*a.lock().await).unwrap(),
    )
    .unwrap();
    let delayed = {
        let a = a.clone();
        let ra = ra.clone();
        tokio::spawn(async move { runtime::run_lane(&a, &ra, "state").await })
    };
    wait(&root.join("put-stored")).await;
    runtime::disconnect(&*a.lock().await).unwrap();
    std::fs::write(root.join("release-put"), "resume").unwrap();
    assert_eq!(delayed.await.unwrap().unwrap_err().code, "staleLocalSync");
    assert!(!runtime::status(&*a.lock().await).unwrap().connected);
    connect(&a, &url).await;
    sync(&a, &ra, "state").await;
    // A referenced missing block is pending, not an empty or complete success.
    let own = {
        let conn = a.lock().await;
        storage::previous_manifest(&conn, &database::actor(&conn).unwrap(), "state")
            .unwrap()
            .unwrap()
    };
    let object = own.shards.last().unwrap();
    let file = root.join("data").join(own.object_path(object));
    let bytes = std::fs::read(&file).unwrap();
    std::fs::remove_file(&file).unwrap();
    let d = open(&root.join("d.db"));
    let rd = LocalRuntime::default();
    connect(&d, &url).await;
    let error = runtime::run_lane(&d, &rd, "state").await.unwrap_err();
    assert_eq!(error.code, "localSyncPending");
    std::fs::write(&file, bytes).unwrap();
    sync(&d, &rd, "state").await;
    // Ordinary read articles carry only identity/state and no archived body.
    let normal = open(&root.join("normal.db"));
    let rn = LocalRuntime::default();
    let nid = seed(&normal).await;
    {
        let conn = normal.lock().await;
        conn.execute(
            "UPDATE feeds SET feed_url='https://normal.example.test/rss'",
            [],
        )
        .unwrap();
        conn.execute("UPDATE articles SET guid='normal-only'", [])
            .unwrap();
        db::set_read(&conn, nid, true).unwrap();
    }
    connect(&normal, &url).await;
    sync(&normal, &rn, "state").await;
    sync(&normal, &rn, "body").await;
    {
        let conn = normal.lock().await;
        let manifest = storage::previous_manifest(&conn, &database::actor(&conn).unwrap(), "body")
            .unwrap()
            .unwrap();
        assert!(app_lib::local_sync::body_archive::catalog(&conn,&manifest).unwrap().entries.values().all(|record|record.entry!=app_lib::local_sync::model::entry_key(&conn.query_row("SELECT sync_id FROM feeds WHERE feed_url='https://normal.example.test/rss'",[],|r|r.get::<_,String>(0)).unwrap(),"normal-only")));
    }
    // A server failure must not be swallowed as an absent lane/success.
    std::fs::write(root.join("fail-propfind"), "503").unwrap();
    let e = runtime::run_lane(&b, &rb, "state").await.unwrap_err();
    assert_eq!(e.code, "localSyncHttp");
    std::fs::remove_file(root.join("fail-propfind")).unwrap();
    std::fs::write(root.join("rust-result.json"),serde_json::to_vec_pretty(&serde_json::json!({"two_clients":true,"new_device_retired_starred_body":true,"unknown_reply_restart_same_sealed_publication":true,"server_503_visible":true,"late_disconnect_response_rejected":true,"missing_block_pending_then_recovers":true})).unwrap()).unwrap();
}
