//! Real loopback requests through run_fast_round; each fixture owns its socket,
//! database and joined handlers. These checks do not claim the full HTTP SLO.
use super::*;
use crate::webdav_transport::{Batch, FieldPatch, FieldValue, Intent};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::Mutex as StdMutex,
    thread,
};

const LIBRARY: &str = "11111111111111111111111111111111";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RequestKind {
    Marker,
    Actors,
    PeerList,
    PeerGet,
    OwnReadback,
    Other,
}
#[derive(Debug)]
struct Request {
    kind: RequestKind,
    path: String,
    finished: bool,
}
#[derive(Default)]
struct Delays {
    marker: bool,
    actors: bool,
    list: bool,
    get: bool,
    readback: bool,
}
struct Shared {
    objects: StdMutex<BTreeMap<String, Vec<u8>>>,
    requests: StdMutex<Vec<Request>>,
    peers: Vec<String>,
    delays: Delays,
    fail_next_get: AtomicBool,
}
struct Server {
    endpoint: String,
    shared: Arc<Shared>,
    stopped: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn start(peer_count: usize, batches: u64, delays: Delays) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let peers: Vec<_> = (0..peer_count)
            .map(|i| format!("{:032x}", i + 11))
            .collect();
        let marker = Marker {
            format: FORMAT,
            library_id: LIBRARY.into(),
            required: required_capabilities(),
        };
        let mut objects = BTreeMap::from([(
            format!("/{ROOT}library.json"),
            serde_json::to_vec(&marker).unwrap(),
        )]);
        for peer in &peers {
            for seq in 1..=batches {
                let batch = Batch {
                    format: 2,
                    library_id: LIBRARY.into(),
                    actor: peer.clone(),
                    intents: vec![Intent {
                        seq,
                        lamport: 1000 + seq,
                        observed: if seq > 1 {
                            BTreeMap::from([(peer.clone(), seq - 1)])
                        } else {
                            BTreeMap::new()
                        },
                        patch: FieldPatch::from([(
                            "p/fontSize".into(),
                            FieldValue::Set(serde_json::json!(18)),
                        )]),
                    }],
                };
                batch.validate().unwrap();
                let bytes = serde_json::to_vec(&batch).unwrap();
                let path = format!(
                    "/{ROOT}actors/{peer}/intents/{}-{}.json",
                    digest(&bytes),
                    "f".repeat(32)
                );
                objects.insert(path, bytes);
            }
        }
        let shared = Arc::new(Shared {
            objects: StdMutex::new(objects),
            requests: StdMutex::new(Vec::new()),
            peers,
            delays,
            fail_next_get: AtomicBool::new(false),
        });
        let stopped = Arc::new(AtomicBool::new(false));
        let (data, stop) = (shared.clone(), stopped.clone());
        let worker = thread::spawn(move || {
            let mut handlers = Vec::new();
            while !stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let data = data.clone();
                        handlers.push(thread::spawn(move || handle(stream, data)));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("fixture accept: {error}"),
                }
            }
            for handler in handlers {
                handler.join().unwrap();
            }
        });
        Self {
            endpoint,
            shared,
            stopped,
            worker: Some(worker),
        }
    }
    fn count(&self, kind: RequestKind) -> usize {
        self.shared
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.kind == kind)
            .count()
    }
    async fn wait_for(&self, kind: RequestKind) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.count(kind) == 0 {
            assert!(Instant::now() < deadline, "fixture did not reach {kind:?}");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    fn finish(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
            println!(
                "B3_HTTP endpoint={} requests={:?}",
                self.endpoint,
                self.shared.requests.lock().unwrap()
            );
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.finish();
    }
}

fn handle(mut stream: TcpStream, data: Arc<Shared>) {
    println!(
        "B3_DIAG accepted local={:?} peer={:?}",
        stream.local_addr(),
        stream.peer_addr()
    );
    // Windows accept inherits the listener's nonblocking mode. This handler
    // uses bounded blocking reads; a request head may arrive after accept.
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let end = loop {
        let n = match stream.read(&mut buffer) {
            Ok(0) => {
                println!(
                    "B3_DIAG read EOF bytes_so_far={} local={:?}",
                    bytes.len(),
                    stream.local_addr()
                );
                return;
            }
            Err(error) => {
                println!(
                    "B3_DIAG read error kind={:?} os={:?} message={} bytes_so_far={} local={:?}",
                    error.kind(),
                    error.raw_os_error(),
                    error,
                    bytes.len(),
                    stream.local_addr()
                );
                return;
            }
            Ok(n) => {
                println!(
                    "B3_DIAG read len={n} bytes_so_far={} local={:?}",
                    bytes.len(),
                    stream.local_addr()
                );
                n
            }
        };
        bytes.extend_from_slice(&buffer[..n]);
        if let Some(i) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
            break i + 4;
        }
        if bytes.len() > 65536 {
            return;
        }
    };
    let header = String::from_utf8_lossy(&bytes[..end]).into_owned();
    let mut first = header.lines().next().unwrap().split_whitespace();
    let method = first.next().unwrap();
    let path = first.next().unwrap();
    let length = header
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    while bytes.len() < end + length {
        let n = match stream.read(&mut buffer) {
            Ok(0) => {
                println!(
                    "B3_DIAG read EOF bytes_so_far={} local={:?}",
                    bytes.len(),
                    stream.local_addr()
                );
                return;
            }
            Err(error) => {
                println!(
                    "B3_DIAG read error kind={:?} os={:?} message={} bytes_so_far={} local={:?}",
                    error.kind(),
                    error.raw_os_error(),
                    error,
                    bytes.len(),
                    stream.local_addr()
                );
                return;
            }
            Ok(n) => {
                println!(
                    "B3_DIAG read len={n} bytes_so_far={} local={:?}",
                    bytes.len(),
                    stream.local_addr()
                );
                n
            }
        };
        bytes.extend_from_slice(&buffer[..n]);
    }
    let is_peer = data
        .peers
        .iter()
        .any(|p| path.starts_with(&format!("/{ROOT}actors/{p}/intents/")));
    let kind = match method {
        "GET" if path == format!("/{ROOT}library.json") => RequestKind::Marker,
        "PROPFIND" if path == format!("/{ROOT}actors/") => RequestKind::Actors,
        "PROPFIND" if is_peer => RequestKind::PeerList,
        "GET" if is_peer => RequestKind::PeerGet,
        "GET" if path.contains("/intents/") && data.objects.lock().unwrap().contains_key(path) => {
            RequestKind::OwnReadback
        }
        _ => RequestKind::Other,
    };
    let request_index = {
        let mut requests = data.requests.lock().unwrap();
        let index = requests.len();
        requests.push(Request {
            kind,
            path: path.into(),
            finished: false,
        });
        index
    };
    let delayed = match kind {
        RequestKind::Marker => data.delays.marker,
        RequestKind::Actors => data.delays.actors,
        RequestKind::PeerList => data.delays.list,
        RequestKind::PeerGet => data.delays.get,
        RequestKind::OwnReadback => data.delays.readback,
        RequestKind::Other => false,
    };
    if delayed {
        thread::sleep(Duration::from_millis(900));
    }
    let (status, body) = if kind == RequestKind::PeerGet
        && data.fail_next_get.swap(false, Ordering::AcqRel)
    {
        ("503 Service Unavailable", Vec::new())
    } else {
        match method {
            "GET" => match data.objects.lock().unwrap().get(path).cloned() {
                Some(bytes) => ("200 OK", bytes),
                None => ("404 Not Found", Vec::new()),
            },
            "PUT" => {
                data.objects
                    .lock()
                    .unwrap()
                    .insert(path.into(), bytes[end..end + length].to_vec());
                ("201 Created", Vec::new())
            }
            "MKCOL" => ("201 Created", Vec::new()),
            "PROPFIND" => {
                let prefix = format!("{}/", path.trim_end_matches('/'));
                let mut children = BTreeMap::<String, bool>::new();
                for key in data.objects.lock().unwrap().keys() {
                    if let Some(rest) = key.strip_prefix(&prefix) {
                        if rest.is_empty() {
                            continue;
                        }
                        let (name, collection) = rest
                            .split_once('/')
                            .map_or((rest, false), |(name, _)| (name, true));
                        children.insert(name.into(), collection);
                    }
                }
                let response = |href: &str, collection: bool| {
                    format!("<D:response><D:href>{href}</D:href><D:propstat><D:prop><D:resourcetype>{}</D:resourcetype></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>", if collection { "<D:collection/>" } else { "" })
                };
                let mut xml = format!("<D:multistatus xmlns:D='DAV:'>{}", response(&prefix, true));
                for (name, collection) in children {
                    xml += &response(
                        &format!("{prefix}{name}{}", if collection { "/" } else { "" }),
                        collection,
                    );
                }
                xml += "</D:multistatus>";
                ("207 Multi-Status", xml.into_bytes())
            }
            _ => ("405 Method Not Allowed", Vec::new()),
        }
    };
    let header = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: application/xml\r\n\r\n", body.len());
    println!(
        "B3_DIAG response status={status} path={path} bytes={}",
        body.len()
    );
    let _ = stream
        .write_all(header.as_bytes())
        .and_then(|()| stream.write_all(&body));
    data.requests.lock().unwrap()[request_index].finished = true;
}

fn database_for(server: &Server) -> Arc<Mutex<Connection>> {
    let mut conn = Connection::open_in_memory().unwrap();
    db::MIGRATIONS.to_latest(&mut conn).unwrap();
    database::register_functions(&conn).unwrap();
    database::initialize(&conn, LIBRARY).unwrap();
    super::super::logical::drain_local(&conn).unwrap();
    db::set_setting(
        &conn,
        "local_sync_config",
        &serde_json::to_string(&Config {
            endpoint: server.endpoint.clone(),
            username: "fixture".into(),
            mode: "realtime".into(),
            interval_minutes: 15,
        })
        .unwrap(),
    )
    .unwrap();
    db::set_setting(&conn, "local_sync_password", "owned-loopback-fixture").unwrap();
    Arc::new(Mutex::new(conn))
}
async fn capture_outgoing(database: &Arc<Mutex<Connection>>) {
    super::super::logical::capture_patch(
        &*database.lock().await,
        &FieldPatch::from([(
            "p/themeMode".into(),
            FieldValue::Set(serde_json::json!("dark")),
        )]),
    )
    .unwrap();
}
async fn invalidate(database: &Arc<Mutex<Connection>>, runtime: &LocalRuntime, stop: bool) {
    // Obtaining this same connection while HTTP is in flight also checks that
    // no database mutex or writer is held across the network await.
    let conn = tokio::time::timeout(Duration::from_millis(200), database.lock())
        .await
        .unwrap();
    if stop {
        runtime.stop();
    } else {
        conn.execute("UPDATE local_sync_control SET generation=generation+1", [])
            .unwrap();
    }
}

#[tokio::test]
async fn b3_slow_marker_and_actors_finish_once_across_discovery_slices() {
    let mut server = Server::start(
        1,
        1,
        Delays {
            marker: true,
            actors: true,
            ..Delays::default()
        },
    );
    let database = database_for(&server);
    let runtime = LocalRuntime::default();
    run_fast_round(&database, &runtime, FastPhase::Discover)
        .await
        .unwrap();
    assert!(runtime.pacing.lock().await.marker_valid);
    assert_eq!(server.count(RequestKind::Marker), 1);
    assert_eq!(
        server.count(RequestKind::Actors),
        0,
        "expired slice must not start another request"
    );
    run_fast_round(&database, &runtime, FastPhase::Discover)
        .await
        .unwrap();
    assert_eq!(server.count(RequestKind::Actors), 1);
    assert_eq!(server.count(RequestKind::PeerList), 0);
    run_fast_round(&database, &runtime, FastPhase::Discover)
        .await
        .unwrap();
    assert_eq!(server.count(RequestKind::Marker), 1);
    assert_eq!(server.count(RequestKind::Actors), 1);
    assert_eq!(
        super::super::logical::frontier(&*database.lock().await)
            .unwrap()
            .get(&server.shared.peers[0]),
        Some(&1)
    );
    server.finish();
}

#[tokio::test]
async fn b3_slow_lists_and_gets_resume_fairly_without_holding_send() {
    let mut server = Server::start(
        2,
        2,
        Delays {
            list: true,
            get: true,
            ..Delays::default()
        },
    );
    let database = database_for(&server);
    let runtime = LocalRuntime::default();
    for _ in 0..2 {
        run_fast_round(&database, &runtime, FastPhase::Discover)
            .await
            .unwrap();
    }
    assert_eq!(server.count(RequestKind::PeerList), 2);
    assert_eq!(server.count(RequestKind::PeerGet), 0);
    capture_outgoing(&database).await;
    let send = async {
        server.wait_for(RequestKind::PeerGet).await;
        run_fast_round(&database, &runtime, FastPhase::Send)
            .await
            .unwrap();
        assert!(
            !server
                .shared
                .requests
                .lock()
                .unwrap()
                .iter()
                .find(|r| r.kind == RequestKind::PeerGet)
                .unwrap()
                .finished,
            "send must finish while the admitted slow peer request remains in flight"
        );
    };
    let (received, ()) = tokio::join!(
        run_fast_round(&database, &runtime, FastPhase::Discover),
        send
    );
    received.unwrap();
    for _ in 0..3 {
        let before = server.count(RequestKind::PeerGet);
        run_fast_round(&database, &runtime, FastPhase::Discover)
            .await
            .unwrap();
        assert_eq!(
            server.count(RequestKind::PeerGet),
            before + 1,
            "one slow request may overrun the slice; no second one may start"
        );
    }
    {
        let requests = server.shared.requests.lock().unwrap();
        let peer_gets: Vec<_> = requests
            .iter()
            .filter(|r| r.kind == RequestKind::PeerGet)
            .collect();
        for (i, request) in peer_gets.iter().enumerate() {
            assert!(
                request.path.contains(&server.shared.peers[i % 2]),
                "unfinished peer must rotate fairly"
            );
        }
    }
    assert_eq!(
        server.count(RequestKind::PeerList),
        2,
        "resume saved names rather than repeating slow lists"
    );
    let conn = database.lock().await;
    let frontier = super::super::logical::frontier(&conn).unwrap();
    for peer in &server.shared.peers {
        assert_eq!(frontier.get(peer), Some(&2));
    }
    assert!(runtime.pacing.lock().await.peer_cursors.is_empty());
    let (stored, seq): (u64, u64) = conn
        .query_row(
            "SELECT fast_stored_seq,seq FROM local_sync_control",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(stored, seq);
    drop(conn);
    server.finish();
}

#[tokio::test]
async fn b3_healthy_requests_respect_round_and_peer_budgets() {
    let mut server = Server::start(4, 5, Delays::default());
    let database = database_for(&server);
    let runtime = LocalRuntime::default();
    for _ in 0..8 {
        let before = server.shared.requests.lock().unwrap().len();
        run_fast_round(&database, &runtime, FastPhase::Discover)
            .await
            .unwrap();
        {
            let requests = server.shared.requests.lock().unwrap();
            let round = &requests[before..];
            assert!(
                round.len() <= 16,
                "every actual HTTP request consumes the same round budget"
            );
            for peer in &server.shared.peers {
                assert!(
                    round
                        .iter()
                        .filter(|r| r.kind == RequestKind::PeerGet && r.path.contains(peer))
                        .count()
                        <= 4
                );
            }
        }
        if server.count(RequestKind::PeerGet) == 20 {
            break;
        }
    }
    assert_eq!(server.count(RequestKind::PeerList), 4);
    assert_eq!(
        server.count(RequestKind::PeerGet),
        20,
        "seen pending batches must not be downloaded again"
    );
    let conn = database.lock().await;
    let frontier = super::super::logical::frontier(&conn).unwrap();
    for peer in &server.shared.peers {
        assert_eq!(frontier.get(peer), Some(&5));
    }
    drop(conn);
    server.finish();
}

#[tokio::test]
async fn b3_failed_get_retains_the_listed_name_until_retry_succeeds() {
    let mut server = Server::start(1, 1, Delays::default());
    server.shared.fail_next_get.store(true, Ordering::Release);
    let database = database_for(&server);
    let runtime = LocalRuntime::default();
    assert!(run_fast_round(&database, &runtime, FastPhase::Discover)
        .await
        .is_err());
    let next = runtime.pacing.lock().await.peers[&server.shared.peers[0]]
        .next
        .unwrap();
    tokio::time::sleep(next.saturating_duration_since(Instant::now()) + Duration::from_millis(10))
        .await;
    run_fast_round(&database, &runtime, FastPhase::Discover)
        .await
        .unwrap();
    assert_eq!(server.count(RequestKind::PeerList), 1);
    assert_eq!(server.count(RequestKind::PeerGet), 2);
    assert_eq!(
        super::super::logical::frontier(&*database.lock().await)
            .unwrap()
            .get(&server.shared.peers[0]),
        Some(&1)
    );
    server.finish();
}

#[tokio::test]
async fn b3_late_receive_responses_reject_stop_and_generation_changes() {
    for kind in [
        RequestKind::Marker,
        RequestKind::Actors,
        RequestKind::PeerList,
        RequestKind::PeerGet,
    ] {
        for stop in [false, true] {
            let mut server = Server::start(
                1,
                1,
                Delays {
                    marker: kind == RequestKind::Marker,
                    actors: kind == RequestKind::Actors,
                    list: kind == RequestKind::PeerList,
                    get: kind == RequestKind::PeerGet,
                    ..Delays::default()
                },
            );
            let database = database_for(&server);
            let runtime = LocalRuntime::default();
            let change = async {
                server.wait_for(kind).await;
                invalidate(&database, &runtime, stop).await;
            };
            let (result, ()) = tokio::join!(
                run_fast_round(&database, &runtime, FastPhase::Discover),
                change
            );
            assert_eq!(
                result.unwrap_err().code,
                if stop { "cancelled" } else { "staleLocalSync" }
            );
            let conn = database.lock().await;
            let seen: i64 = conn
                .query_row("SELECT COUNT(*) FROM local_sync_seen_batches", [], |r| {
                    r.get(0)
                })
                .unwrap();
            let pending: i64 = conn
                .query_row("SELECT COUNT(*) FROM local_sync_intents", [], |r| r.get(0))
                .unwrap();
            assert_eq!((seen, pending), (0, 0));
            assert!(!super::super::logical::frontier(&conn)
                .unwrap()
                .contains_key(&server.shared.peers[0]));
            if kind == RequestKind::Actors {
                let peer_registered: bool = conn
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM local_sync_known_actors WHERE actor=?1)",
                        [&server.shared.peers[0]],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert!(
                    !peer_registered,
                    "late actor listing must not commit registration"
                );
            }
            drop(conn);
            let pacing = runtime.pacing.lock().await;
            assert!(
                pacing.peer_cursors.is_empty(),
                "late list/get must not install a resumable cursor"
            );
            if kind == RequestKind::Marker {
                assert!(!pacing.marker_valid);
            }
            drop(pacing);
            server.finish();
        }
    }
}

#[tokio::test]
async fn b3_receive_rechecks_session_after_each_apply_step_before_next_request() {
    let server = Server::start(1, 2, Delays::default());
    let database = database_for(&server);
    {
        let conn = database.lock().await;
        conn.execute_batch(
            "CREATE TRIGGER b3_invalidate_after_apply
             AFTER DELETE ON local_sync_intents
             BEGIN
               UPDATE local_sync_control SET generation=generation+1 WHERE id=1;
             END;",
        )
        .unwrap();
    }
    let mut session = blocking(database.clone(), session).await.unwrap().unwrap();
    session.stopped = Arc::new(AtomicBool::new(false));
    let peer = server.shared.peers[0].clone();
    let mut cursor = crate::local_sync::fast::PeerCursor::default();
    let mut budget = storage::Budget::limited(64);
    let result = crate::local_sync::fast::receive_peer(
        &database,
        &session,
        &peer,
        &mut budget,
        Instant::now() + Duration::from_secs(3),
        &mut cursor,
    )
    .await;

    assert_eq!(result.unwrap_err().code, "staleLocalSync");
    assert_eq!(
        server.count(RequestKind::PeerGet),
        1,
        "generation changed by the first applied intent must stop the next GET"
    );
    let conn = database.lock().await;
    let (intents, generation, applied): (i64, i64, bool) = conn
        .query_row(
            "SELECT
               (SELECT COUNT(*) FROM local_sync_intents),
               (SELECT generation FROM local_sync_control WHERE id=1),
               EXISTS(
                 SELECT 1 FROM local_sync_winners
                 WHERE key='p/fontSize'
                   AND json_extract(register,'$.stamp.actor')=?1
                   AND json_extract(register,'$.stamp.seq')=1
                   AND json_extract(register,'$.value.value')=18
               )",
            [&peer],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        intents, 0,
        "the first intent is retired after its transaction commits"
    );
    assert_eq!(
        generation,
        session.generation + 1,
        "the first apply must commit and advance the generation once"
    );
    assert!(applied, "the first intent's winner must survive the commit");
}

#[tokio::test]
async fn b3_late_send_readback_keeps_pending_without_ack_after_invalidation() {
    for stop in [false, true] {
        let mut server = Server::start(
            0,
            0,
            Delays {
                readback: true,
                ..Delays::default()
            },
        );
        let database = database_for(&server);
        let runtime = LocalRuntime::default();
        capture_outgoing(&database).await;
        {
            let conn = database.lock().await;
            println!(
                "B3_DIAG send_precondition has_outgoing={:?} seq_stored={:?}",
                super::super::fast::has_outgoing(&conn),
                conn.query_row(
                    "SELECT seq,fast_stored_seq FROM local_sync_control",
                    [],
                    |r| Ok((r.get::<_, u64>(0)?, r.get::<_, u64>(1)?))
                )
            );
        }
        let change = async {
            server.wait_for(RequestKind::OwnReadback).await;
            invalidate(&database, &runtime, stop).await;
        };
        let (result, ()) = tokio::join!(
            async {
                let result = run_fast_round(&database, &runtime, FastPhase::Send).await;
                println!("B3_DIAG send_result={result:?}");
                result
            },
            change
        );
        assert_eq!(
            result.unwrap_err().code,
            if stop { "cancelled" } else { "staleLocalSync" }
        );
        let conn = database.lock().await;
        let stored: u64 = conn
            .query_row("SELECT fast_stored_seq FROM local_sync_control", [], |r| {
                r.get(0)
            })
            .unwrap();
        let pending: Vec<u8> = conn
            .query_row("SELECT bytes FROM local_sync_fast_pending", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(stored, 0);
        assert!(server.shared.objects.lock().unwrap().values().any(|bytes| bytes == &pending),
            "PUT may already have completed remotely; keep exact sealed bytes without accepting a late ACK");
        drop(conn);
        server.finish();
    }
}

#[test]
fn b3_pending_peer_capacity_and_generation_scope_are_bounded() {
    let (_conn, session) = super::segment2_schedule_tests::fixture();
    let mut pacing = FastPacing::default();
    pacing.observe(&session);
    for i in 0..(MAX_PENDING_PEERS + 8) {
        let peer = format!("{i:032x}");
        pacing.peers.insert(peer.clone(), Due::default());
        if i < MAX_PENDING_PEERS {
            pacing.peer_cursors.insert(peer, Default::default());
        }
    }
    assert!(pacing
        .peers_due(Instant::now())
        .iter()
        .all(|peer| pacing.peer_cursors.contains_key(peer)));
    let mut changed = session;
    changed.generation += 1;
    pacing.observe(&changed);
    assert!(pacing.peer_cursors.is_empty());
}
