//! Production transport tests. Scripted HTTP is a fault injector, not a claim
//! about WebDAV deployment compatibility. The WsgiDAV case is explicitly ignored
//! until a coordinator grants a fresh loopback service/target reservation.

use app_lib::webdav_transport::{
    Batch, ErrorKind, FieldPatch, FieldValue, Intent, PreparedBatch, PreparedCheckpoint, ProbeId,
    Replica, StoreResolution, Transport,
};
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

fn id(value: u8) -> ProbeId {
    ProbeId([value; 16])
}

fn actor(value: u8) -> String {
    format!("{value:02x}").repeat(16)
}
fn prepared(who: u8, patches: &[(&str, bool)]) -> PreparedBatch {
    PreparedBatch::new(Batch {
        format: 2,
        library_id: actor(9),
        actor: actor(who),
        intents: vec![Intent {
            seq: 1,
            lamport: 1,
            observed: Default::default(),
            patch: patches
                .iter()
                .map(|(key, value)| (key.to_string(), FieldValue::Set(serde_json::json!(value))))
                .collect::<FieldPatch>(),
        }],
    })
    .unwrap()
}

struct Request {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn read_request(stream: &mut TcpStream) -> Request {
    // The accept loop is nonblocking for bounded shutdown. On Windows accepted
    // sockets can inherit that mode, so a timeout alone does not make read wait.
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut bytes = Vec::new();
    let end = loop {
        let mut buf = [0; 4096];
        let size = stream.read(&mut buf).unwrap();
        assert!(size > 0, "request ended before headers");
        bytes.extend_from_slice(&buf[..size]);
        assert!(bytes.len() <= 3 * 1024 * 1024, "test request too large");
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let header = String::from_utf8(bytes[..end].to_vec()).unwrap();
    let mut lines = header.lines();
    let mut first = lines.next().unwrap().split_whitespace();
    let method = first.next().unwrap().to_owned();
    let path = first.next().unwrap().to_owned();
    let headers: HashMap<_, _> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let length = headers
        .get("content-length")
        .map_or(0, |value| value.parse::<usize>().unwrap());
    while bytes.len() - end < length {
        let mut buf = [0; 4096];
        let size = stream.read(&mut buf).unwrap();
        assert!(size > 0);
        bytes.extend_from_slice(&buf[..size]);
    }
    Request {
        method,
        path,
        headers,
        body: bytes[end..end + length].to_vec(),
    }
}

fn respond(stream: &mut TcpStream, status: &str, headers: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{headers}Connection: close\r\n\r\n",
        body.len()
    );
    if stream.write_all(head.as_bytes()).is_ok() {
        let _ = stream.write_all(body);
    }
}

fn lock_xml(path: &str, seconds: u32) -> String {
    format!("<D:prop xmlns:D=\"DAV:\"><D:lockdiscovery><D:activelock><D:locktype><D:write/></D:locktype><D:lockscope><D:exclusive/></D:lockscope><D:depth>infinity</D:depth><D:timeout>Second-{seconds}</D:timeout><D:locktoken><D:href>urn:uuid:11111111-2222-3333-4444-555555555555</D:href></D:locktoken><D:lockroot><D:href>{path}</D:href></D:lockroot></D:activelock></D:lockdiscovery></D:prop>")
}

struct FaultServer {
    url: String,
    stop: Arc<AtomicBool>,
    moves: Arc<AtomicUsize>,
    worker: Option<thread::JoinHandle<()>>,
}

impl FaultServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let moves = Arc::new(AtomicUsize::new(0));
        let stopping = stop.clone();
        let move_counter = moves.clone();
        let worker = thread::spawn(move || {
            let mut files: HashMap<String, Vec<u8>> = HashMap::new();
            let mut collection = String::new();
            while !stopping.load(Ordering::SeqCst) {
                let (mut stream, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(e) => panic!("mock accept: {e}"),
                };
                let request = read_request(&mut stream);
                match request.method.as_str() {
                    "MKCOL" => {
                        collection = request.path;
                        respond(&mut stream, "201 Created", "", b"");
                    }
                    "LOCK" => respond(
                        &mut stream,
                        "200 OK",
                        "Lock-Token: <urn:uuid:11111111-2222-3333-4444-555555555555>\r\n",
                        lock_xml(&collection, 60).as_bytes(),
                    ),
                    "PROPFIND" => {
                        assert_eq!(request.headers.get("depth").map(String::as_str), Some("1"));
                        let mut xml = "<D:multistatus xmlns:D=\"DAV:\">".to_owned();
                        for path in files.keys() {
                            xml.push_str(&format!("<D:response><D:href>{path}</D:href><D:propstat><D:prop/><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>"));
                        }
                        xml.push_str("</D:multistatus>");
                        respond(&mut stream, "207 Multi-Status", "", xml.as_bytes());
                    }
                    "PUT" => {
                        assert_eq!(
                            request.headers.get("if-none-match").map(String::as_str),
                            Some("*")
                        );
                        // LOCK is optional; immutable batch identity supplies idempotence.
                        assert!(!files.contains_key(&request.path));
                        files.insert(request.path, request.body);
                        respond(&mut stream, "201 Created", "", b"");
                    }
                    "GET" => match files.get(&request.path) {
                        Some(body) => respond(&mut stream, "200 OK", "", body),
                        None => respond(&mut stream, "404 Not Found", "", b""),
                    },
                    "MOVE" => {
                        assert_eq!(
                            request.headers.get("overwrite").map(String::as_str),
                            Some("F")
                        );
                        // LOCK is optional; immutable batch identity supplies idempotence.
                        let destination =
                            url::Url::parse(request.headers.get("destination").unwrap()).unwrap();
                        let bytes = files.remove(&request.path).unwrap();
                        assert!(!files.contains_key(destination.path()));
                        files.insert(destination.path().to_owned(), bytes);
                        move_counter.fetch_add(1, Ordering::SeqCst);
                        // Deliberately drop the socket AFTER persisting the MOVE.
                        // Only a subsequent GET can establish the receipt.
                    }
                    "UNLOCK" => respond(&mut stream, "204 No Content", "", b""),
                    "DELETE" => {
                        files.clear();
                        respond(&mut stream, "204 No Content", "", b"");
                    }
                    _ => panic!("unexpected request {}", request.method),
                }
            }
        });
        Self {
            url,
            stop,
            moves,
            worker: Some(worker),
        }
    }
}

impl Drop for FaultServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            if let Err(panic) = worker.join() {
                // Preserve the primary assertion/network failure rather than
                // aborting the whole test process with a destructor panic. If
                // the test itself has not failed, propagate the fixture panic.
                if !thread::panicking() {
                    std::panic::resume_unwind(panic);
                }
            }
        }
    }
}

#[tokio::test]
async fn mock_lost_move_response_is_stored_not_global_winner_and_retry_is_idempotent() {
    let server = FaultServer::start();
    let transport = Transport::new(&server.url, None, Duration::from_secs(3)).unwrap();
    let collection = transport.create_probe_collection(id(1)).await.unwrap();
    let batch = prepared(1, &[("entry/read", true)]);
    let mut nullable_batch = batch.batch().clone();
    nullable_batch.intents[0].patch.insert(
        "settings/nullable".to_owned(),
        FieldValue::Set(serde_json::Value::Null),
    );
    nullable_batch.intents[0]
        .patch
        .insert("settings/removed".to_owned(), FieldValue::Delete);
    let batch = PreparedBatch::new(nullable_batch).unwrap();
    assert_eq!(
        transport
            .store_batch(&collection, &batch, id(3), None)
            .await
            .unwrap_err()
            .kind,
        ErrorKind::OutcomeUnknown
    );
    assert!(matches!(
        transport.resolve_batch(&collection, &batch).await.unwrap(),
        StoreResolution::Stored(_)
    ));
    transport
        .store_batch(&collection, &batch, id(4), None)
        .await
        .unwrap();
    assert_eq!(server.moves.load(Ordering::SeqCst), 1);
    let scan = transport
        .scan(&collection, &Replica::new(&actor(9)).unwrap())
        .await
        .unwrap();
    assert_eq!(
        scan.replica.checkpoint().unwrap().fields["entry/read"].value,
        FieldValue::Set(serde_json::json!(true))
    );
    assert_eq!(
        scan.replica.checkpoint().unwrap().fields["settings/nullable"].value,
        FieldValue::Set(serde_json::Value::Null)
    );
    assert_eq!(
        scan.replica.checkpoint().unwrap().fields["settings/removed"].value,
        FieldValue::Delete
    );
    let mut local = Replica::new(&actor(9)).unwrap();
    local.ingest_batch(batch.batch()).unwrap();
    assert_eq!(
        transport
            .scan(&collection, &local)
            .await
            .unwrap()
            .replica
            .checkpoint()
            .unwrap(),
        local.checkpoint().unwrap(),
        "own uploaded null must not become a different dot payload"
    );
    transport.cleanup_probe(&collection).await.unwrap();
}

async fn creation_response(body: Vec<u8>, length: bool) -> ErrorKind {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        read_request(&mut stream);
        let header = if length {
            format!("Content-Length: {}\r\n", body.len())
        } else {
            String::new()
        };
        let head = format!("HTTP/1.1 201 Created\r\n{header}Connection: close\r\n\r\n");
        stream.write_all(head.as_bytes()).unwrap();
        let _ = stream.write_all(&body);
    });
    let transport = Transport::new(&url, None, Duration::from_secs(3)).unwrap();
    let result = transport
        .create_probe_collection(id(5))
        .await
        .unwrap_err()
        .kind;
    worker.join().unwrap();
    result
}

#[tokio::test]
async fn mock_response_budget_checks_length_and_streamed_eof() {
    for length in [true, false] {
        assert_eq!(
            creation_response(vec![b'x'; 256 * 1024 + 1], length).await,
            ErrorKind::TooLarge
        );
    }
}

fn live_id() -> ProbeId {
    let raw = std::env::var("FLUXREADER_WEBDAV_PROBE_ID")
        .expect("runner must supply random 128-bit probe id");
    assert_eq!(raw.len(), 32);
    let mut value = [0; 16];
    for (index, byte) in value.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&raw[index * 2..index * 2 + 2], 16).unwrap();
    }
    ProbeId(value)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "coordinator must reserve real WsgiDAV with unchanged post-check MOVE barrier"]
async fn real_t1_expired_move_branches_converge_via_production_clients() {
    let base = std::env::var("FLUXREADER_WEBDAV_TEST_URL").expect("isolated server required");
    let url = url::Url::parse(&base).unwrap();
    assert_eq!(url.scheme(), "http");
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    let evidence =
        std::path::PathBuf::from(std::env::var("FLUXREADER_WEBDAV_TEST_EVIDENCE").unwrap());
    assert!(evidence.is_absolute());
    let a = Transport::new(&base, None, Duration::from_secs(15)).unwrap();
    let b = Transport::new(&base, None, Duration::from_secs(15)).unwrap();
    let collection = a.create_probe_collection(live_id()).await.unwrap();
    let batch_a = prepared(1, &[("entry/read", true), ("entry/a-only", true)]);
    let batch_b = prepared(2, &[("entry/read", false), ("entry/star", true)]);
    let lease_a = a.acquire_lock(&collection, 1).await.unwrap();
    assert!(lease_a.duration() <= Duration::from_secs(3));
    let duration = lease_a.duration();
    let a_clone = a.clone();
    let c_clone = collection.clone();
    let ba_clone = batch_a.clone();
    let moving = tokio::spawn(async move {
        a_clone
            .store_batch(&c_clone, &ba_clone, id(11), Some(&lease_a))
            .await
    });
    let entered = evidence.join("a-entered");
    let started = std::time::Instant::now();
    while !entered.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "A did not reach original provider post-check barrier"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(duration + Duration::from_millis(200)).await;
    let lease_b = b.acquire_lock(&collection, 10).await.unwrap();
    b.store_batch(&collection, &batch_b, id(12), Some(&lease_b))
        .await
        .unwrap();
    assert!(matches!(
        b.resolve_batch(&collection, &batch_b).await.unwrap(),
        StoreResolution::Stored(_)
    ));
    let while_paused = b
        .scan(&collection, &Replica::new(&actor(9)).unwrap())
        .await
        .unwrap();
    assert!(!while_paused
        .replica
        .checkpoint()
        .unwrap()
        .fields
        .contains_key("entry/a-only"));
    std::fs::write(evidence.join("b-stored-before-a"), b"yes").unwrap();
    // B keeps the replacement lock. Release A at the same provider position
    // as the independent counterexample; server's original MOVE still runs.
    std::fs::write(evidence.join("resume-a"), b"resume").unwrap();
    moving.await.unwrap().unwrap();
    let scan_a = a
        .scan(&collection, &Replica::new(&actor(9)).unwrap())
        .await
        .unwrap();
    let scan_b = b
        .scan(&collection, &Replica::new(&actor(9)).unwrap())
        .await
        .unwrap();
    let cp_a = scan_a.replica.checkpoint().unwrap();
    let cp_b = scan_b.replica.checkpoint().unwrap();
    assert_eq!(
        cp_a, cp_b,
        "both clients must merge both branches identically"
    );
    assert!(
        scan_a.pending_objects.is_empty() && scan_a.replica.project().unwrap().pending.is_empty()
    );
    assert_eq!(
        cp_a.fields["entry/read"].value,
        FieldValue::Set(serde_json::json!(false)),
        "same logical clock: actor2 wins independent of arrival"
    );
    assert_eq!(
        cp_a.fields["entry/star"].value,
        FieldValue::Set(serde_json::json!(true))
    );
    assert_eq!(
        cp_a.fields["entry/a-only"].value,
        FieldValue::Set(serde_json::json!(true)),
        "independent late field must survive"
    );
    let readback = serde_json::json!({"actor_a":actor(1),"actor_b":actor(2),"both_clients_equal":true,
        "b_stored_before_a_resumed":true,"projection":cp_a});
    std::fs::write(
        evidence.join("rust-convergence.json"),
        serde_json::to_vec_pretty(&readback).unwrap(),
    )
    .unwrap();
    b.release_lock(&lease_b).await.unwrap();
    // Stored old A is not promoted by retry; it cannot beat B after arriving last.
    a.store_batch(&collection, &batch_a, id(13), None)
        .await
        .unwrap();
    assert_eq!(
        a.scan(&collection, &Replica::new(&actor(9)).unwrap())
            .await
            .unwrap()
            .replica
            .checkpoint()
            .unwrap(),
        cp_b
    );
    // Partial objects remain pending; valid independent inputs remain usable.
    let partial = prepared(3, &[("entry/partial", true)]);
    a.probe_write(
        &collection,
        &partial.file_name(id(14)),
        &partial.bytes()[..10],
        None,
    )
    .await
    .unwrap();
    let pending = a
        .scan(&collection, &Replica::new(&actor(9)).unwrap())
        .await
        .unwrap();
    assert_eq!(pending.pending_objects.len(), 1);
    assert!(!pending
        .replica
        .checkpoint()
        .unwrap()
        .fields
        .contains_key("entry/partial"));
    let checkpoint = PreparedCheckpoint::new(cp_b.clone()).unwrap();
    let stored = a
        .store_checkpoint(&collection, &checkpoint, id(15), None)
        .await
        .unwrap();
    assert_eq!(
        a.remove_covered_batches(&collection, &checkpoint, &stored, &scan_a)
            .await
            .unwrap(),
        2
    );
    let after_gc = b
        .scan(&collection, &Replica::new(&actor(9)).unwrap())
        .await
        .unwrap();
    assert_eq!(
        after_gc.replica.checkpoint().unwrap(),
        cp_b,
        "checkpoint carries both branches after exact covered cleanup"
    );
    assert_eq!(
        after_gc.pending_objects.len(),
        1,
        "uncovered partial file is not deleted as covered"
    );
    a.cleanup_probe(&collection).await.unwrap();
}
