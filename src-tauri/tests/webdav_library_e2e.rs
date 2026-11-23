//! Active immutable WebDAV library operations, exercised against a loopback fault server.
use app_lib::webdav_transport::{ErrorKind, Transport};
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

struct Request {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn read_request(stream: &mut TcpStream) -> Request {
    // Windows may carry the listener's nonblocking mode into accepted sockets.
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut bytes = Vec::new();
    let end = loop {
        let mut chunk = [0; 4096];
        let count = stream.read(&mut chunk).unwrap();
        assert_ne!(count, 0, "request ended before its headers");
        bytes.extend_from_slice(&chunk[..count]);
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
        let mut chunk = [0; 4096];
        let count = stream.read(&mut chunk).unwrap();
        assert_ne!(count, 0, "request body truncated");
        bytes.extend_from_slice(&chunk[..count]);
    }
    Request {
        method,
        path,
        headers,
        body: bytes[end..end + length].to_vec(),
    }
}

fn respond(stream: &mut TcpStream, status: &str, body: &[u8]) {
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    if stream.write_all(header.as_bytes()).is_ok() {
        let _ = stream.write_all(body);
    }
}

fn propfind_response(body: Vec<u8>, content_length: bool) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request = read_request(&mut stream);
        assert_eq!(request.method, "PROPFIND");
        assert_eq!(request.path, "/.fluxreader-v3/actors/");
        assert_eq!(request.headers.get("depth").map(String::as_str), Some("1"));
        let length = if content_length {
            format!("Content-Length: {}\r\n", body.len())
        } else {
            String::new()
        };
        let header = format!("HTTP/1.1 207 Multi-Status\r\n{length}Connection: close\r\n\r\n");
        stream.write_all(header.as_bytes()).unwrap();
        let _ = stream.write_all(&body);
    });
    (url, worker)
}

fn padded_empty_listing(length: usize) -> Vec<u8> {
    let mut body = br#"<D:multistatus xmlns:D="DAV:"></D:multistatus>"#.to_vec();
    body.resize(length, b' ');
    body
}

async fn check_library_list_xml_budget(content_length: bool) {
    const LIMIT: usize = 256 * 1024;
    let over = padded_empty_listing(LIMIT + 1);
    let (url, worker) = propfind_response(over, content_length);
    let transport = Transport::new(&url, None, Duration::from_secs(3)).unwrap();
    let error = transport
        .library_list(".fluxreader-v3/actors/")
        .await
        .unwrap_err();
    worker.join().unwrap();
    assert_eq!(error.kind, ErrorKind::TooLarge);

    let exact = padded_empty_listing(LIMIT);
    let (url, worker) = propfind_response(exact, content_length);
    let transport = Transport::new(&url, None, Duration::from_secs(3)).unwrap();
    assert!(transport
        .library_list(".fluxreader-v3/actors/")
        .await
        .unwrap()
        .is_empty());
    worker.join().unwrap();
}

struct FaultServer {
    url: String,
    stop: Arc<AtomicBool>,
    puts: Arc<AtomicUsize>,
    bytes: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl FaultServer {
    fn start() -> Self {
        Self::start_with(false)
    }

    fn start_with(corrupt_readback: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let puts = Arc::new(AtomicUsize::new(0));
        let bytes: Arc<Mutex<HashMap<String, Vec<u8>>>> = Arc::new(Mutex::new(HashMap::new()));
        let (stopping, put_count, stored) = (stop.clone(), puts.clone(), bytes.clone());
        let worker = thread::spawn(move || {
            while !stopping.load(Ordering::Acquire) {
                let (mut stream, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("loopback accept: {error}"),
                };
                let request = read_request(&mut stream);
                match request.method.as_str() {
                    "GET" => match stored.lock().unwrap().get(&request.path) {
                        Some(body) => {
                            let mut reply = body.clone();
                            if corrupt_readback && !reply.is_empty() {
                                reply.pop();
                            }
                            respond(&mut stream, "200 OK", &reply);
                        }
                        None => respond(&mut stream, "404 Not Found", b""),
                    },
                    "PUT" => {
                        assert_eq!(
                            request.headers.get("if-none-match").map(String::as_str),
                            Some("*")
                        );
                        let count = put_count.fetch_add(1, Ordering::AcqRel);
                        let mut files = stored.lock().unwrap();
                        assert!(
                            !files.contains_key(&request.path),
                            "immutable PUT must not overwrite existing bytes"
                        );
                        files.insert(request.path, request.body);
                        drop(files);
                        if count != 0 || corrupt_readback {
                            respond(&mut stream, "201 Created", b"");
                        }
                        // Lost-response mode persists the bytes but drops the first reply.
                    }
                    method => panic!("unexpected method: {method}"),
                }
            }
        });
        Self {
            url,
            stop,
            puts,
            bytes,
            worker: Some(worker),
        }
    }
}

impl Drop for FaultServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}

#[tokio::test]
async fn lost_immutable_put_response_retries_exact_bytes_without_new_put() {
    let server = FaultServer::start();
    let transport = Transport::new(&server.url, None, Duration::from_secs(3)).unwrap();
    let path = ".fluxreader-v3/state/intent-immutable.json";
    let remote_path = format!("/{path}");
    let sealed = br#"{"format":2,"fields":{"nullable":{"op":"set","value":null},"removed":{"op":"delete"}}}"#;

    let first = transport.library_store(path, sealed).await.unwrap_err();
    assert_eq!(first.kind, ErrorKind::OutcomeUnknown);
    assert_eq!(server.puts.load(Ordering::Acquire), 1);
    assert_eq!(
        server.bytes.lock().unwrap().get(&remote_path).unwrap(),
        sealed
    );

    transport.library_store(path, sealed).await.unwrap();
    assert_eq!(server.puts.load(Ordering::Acquire), 1);
    assert_eq!(
        server.bytes.lock().unwrap().get(&remote_path).unwrap(),
        sealed
    );

    let conflict = transport
        .library_store(path, br#"{"different":true}"#)
        .await
        .unwrap_err();
    assert_eq!(conflict.kind, ErrorKind::Conflict);
    assert_eq!(server.puts.load(Ordering::Acquire), 1);
    assert_eq!(
        server.bytes.lock().unwrap().get(&remote_path).unwrap(),
        sealed
    );
}

#[tokio::test]
async fn active_library_list_keeps_256_kib_xml_content_length_budget() {
    check_library_list_xml_budget(true).await;
}

#[tokio::test]
async fn active_library_list_keeps_256_kib_xml_eof_budget() {
    check_library_list_xml_budget(false).await;
}

#[tokio::test]
async fn exact_readback_mismatch_is_outcome_unknown_and_keeps_sealed_bytes() {
    let server = FaultServer::start_with(true);
    let transport = Transport::new(&server.url, None, Duration::from_secs(3)).unwrap();
    let path = ".fluxreader-v3/state/intent-readback.json";
    let remote_path = format!("/{path}");
    let sealed = br#"{"format":2,"fields":{"nullable":{"op":"set","value":null}}}"#;

    let error = transport.library_store(path, sealed).await.unwrap_err();
    assert_eq!(error.kind, ErrorKind::OutcomeUnknown);
    assert!(
        error.message.contains("readback"),
        "a mismatched readback must not claim success: {}",
        error.message
    );
    assert_eq!(server.puts.load(Ordering::Acquire), 1);
    assert_eq!(
        server.bytes.lock().unwrap().get(&remote_path).unwrap(),
        sealed,
        "the accepted PUT bytes stay sealed even when the readback disagrees"
    );
}
