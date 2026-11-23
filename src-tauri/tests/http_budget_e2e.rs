//! D7：生产 WebDAV / AI models / AI 错误收体的预算与正常对照。
use app_lib::{
    ai, ingestion,
    webdav_transport::{ErrorKind, Transport},
};
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::Duration,
};

fn response(status: &str, body: Vec<u8>, content_length: bool) -> (String, thread::JoinHandle<()>) {
    response_with_request(status, body, content_length, None)
}

fn response_with_request(
    status: &str,
    body: Vec<u8>,
    content_length: bool,
    expected_request: Option<String>,
) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let status = status.to_string();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 4096];
        while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            let n = stream.read(&mut buffer).unwrap();
            if n == 0 {
                return;
            }
            request.extend_from_slice(&buffer[..n]);
        }
        if let Some(expected) = &expected_request {
            let header_end = request
                .windows(4)
                .position(|bytes| bytes == b"\r\n\r\n")
                .unwrap()
                + 4;
            let request_line = String::from_utf8_lossy(
                request
                    .split(|byte| *byte == b'\n')
                    .next()
                    .expect("request line"),
            );
            assert_eq!(request_line.trim_end(), expected);
            let headers = String::from_utf8_lossy(&request[..header_end]);
            assert!(headers
                .lines()
                .any(|line| line.eq_ignore_ascii_case("Depth: 1")));
            let request_body = headers
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            while request.len() < header_end + request_body {
                let mut buffer = [0; 4096];
                let size = stream.read(&mut buffer).unwrap();
                assert_ne!(size, 0, "PROPFIND request body truncated");
                request.extend_from_slice(&buffer[..size]);
            }
        }
        let length = if content_length {
            format!("Content-Length: {}\r\n", body.len())
        } else {
            String::new()
        };
        let content_type = if expected_request.is_some() {
            "application/xml; charset=utf-8"
        } else {
            "application/json; charset=utf-8"
        };
        let header = format!("HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\n{length}Connection: close\r\n\r\n");
        if stream.write_all(header.as_bytes()).is_ok() {
            for chunk in body.chunks(16 * 1024) {
                if stream.write_all(chunk).is_err() {
                    break;
                }
            }
        }
    });
    (base, handle)
}

fn listing_response(
    path: &str,
    body: Vec<u8>,
    content_length: bool,
) -> (String, thread::JoinHandle<()>) {
    response_with_request(
        "207 Multi-Status",
        body,
        content_length,
        Some(format!("PROPFIND /{path} HTTP/1.1")),
    )
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
}

fn ai_config(base: String) -> ai::AiConfig {
    ai::AiConfig {
        base_url: base,
        api_key: "synthetic".into(),
        model: "test".into(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn webdav_body_cap_rejects_length_and_eof_oversize_and_accepts_normal_json() {
    const LIMIT: usize = 4 * 1024 * 1024;
    for content_length in [true, false] {
        let (base, server) = response("200 OK", vec![b'x'; LIMIT + 1], content_length);
        let transport = Transport::new(&base, None, Duration::from_secs(10)).unwrap();
        let error = transport
            .library_get(".fluxreader-v3/test.bin", LIMIT)
            .await
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::TooLarge);
        server.join().unwrap();
    }
    let (base, server) = response("200 OK", br#"{"format":3}"#.to_vec(), true);
    let transport = Transport::new(&base, None, Duration::from_secs(10)).unwrap();
    assert_eq!(
        transport
            .library_get(".fluxreader-v3/test.bin", LIMIT)
            .await
            .unwrap()
            .unwrap(),
        br#"{"format":3}"#
    );
    server.join().unwrap();
    let (base, server) = response("200 OK", vec![b'x'; LIMIT], true);
    let transport = Transport::new(&base, None, Duration::from_secs(10)).unwrap();
    assert_eq!(
        transport
            .library_get(".fluxreader-v3/test.bin", LIMIT)
            .await
            .unwrap()
            .unwrap()
            .len(),
        LIMIT
    );
    server.join().unwrap();
}

fn listing(count: usize) -> Vec<u8> {
    let mut xml = "<D:multistatus xmlns:D='DAV:'>".to_owned();
    for index in 0..count {
        xml.push_str(&format!("<D:response><D:href>{index}.json</D:href><D:propstat><D:prop><D:resourcetype/></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>"));
    }
    xml.push_str("</D:multistatus>");
    xml.into_bytes()
}

#[tokio::test]
async fn s4b_d7_directory_capacity_identifies_each_directory_and_256_limit() {
    for directory in ["state/manifests/", "body/manifests/", "intents/"] {
        let path = format!(".fluxreader-v3/actors/{}/{directory}", "1".repeat(32));
        for count in [0, 256, 257] {
            let xml = listing(count);
            assert!(
                xml.len() < 256 * 1024,
                "count={count}, path={path}: byte cap"
            );
            let (base, server) = listing_response(&path, xml, true);
            let transport = Transport::new(&base, None, Duration::from_secs(10)).unwrap();
            let result = transport.library_list(&path).await;
            server.join().unwrap();
            if count <= 256 {
                assert_eq!(result.unwrap().len(), count, "count={count}, path={path}");
            } else {
                let error = result.unwrap_err();
                assert_eq!(
                    error.kind,
                    ErrorKind::TooLarge,
                    "count={count}, path={path}"
                );
                assert!(
                    error.message.contains("256 项上限"),
                    "count={count}, path={path}, error={}",
                    error.message
                );
                assert!(
                    error.message.contains("单目录最多 256 项"),
                    "count={count}, path={path}, error={}",
                    error.message
                );
                assert!(
                    error.message.contains(&path),
                    "count={count}, path={path}, error={}",
                    error.message
                );
                assert!(
                    !error.message.contains("XML"),
                    "count={count}, path={path}, error={}",
                    error.message
                );
                assert!(
                    error.message.contains("不会减少远端对象数"),
                    "count={count}, path={path}, error={}",
                    error.message
                );
            }
        }
    }
}

#[tokio::test]
async fn s4b_d7_xml_256kib_length_and_eof_are_distinct_from_directory_count() {
    const XML_LIMIT: usize = 256 * 1024;
    let path = ".fluxreader-v3/actors/";
    for content_length in [true, false] {
        let mut xml = listing(0);
        xml.resize(XML_LIMIT + 1, b' ');
        let (base, server) = listing_response(path, xml, content_length);
        let transport = Transport::new(&base, None, Duration::from_secs(10)).unwrap();
        let error = transport.library_list(path).await.unwrap_err();
        server.join().unwrap();
        assert_eq!(error.kind, ErrorKind::TooLarge);
        assert!(
            error.message.contains(if content_length {
                "Content-Length"
            } else {
                "流式接收"
            }),
            "{}",
            error.message
        );
        assert!(error.message.contains("XML"), "{}", error.message);
        assert!(error.message.contains("256 KiB"), "{}", error.message);
        assert!(error.message.contains(path), "{}", error.message);
        assert!(
            !error.message.contains("256 个"),
            "byte budget is not a file count"
        );

        let mut exact = listing(0);
        exact.resize(XML_LIMIT, b' ');
        let (base, server) = listing_response(path, exact, content_length);
        let transport = Transport::new(&base, None, Duration::from_secs(10)).unwrap();
        assert!(transport.library_list(path).await.unwrap().is_empty());
        server.join().unwrap();
    }
}

#[tokio::test]
async fn s4b_d7_xml_256kib_eof_error_identifies_byte_budget() {
    const XML_LIMIT: usize = 256 * 1024;
    let path = ".fluxreader-v3/actors/";
    let mut xml = listing(0);
    xml.resize(XML_LIMIT + 1, b' ');
    let (base, server) = listing_response(path, xml, false);
    let transport = Transport::new(&base, None, Duration::from_secs(10)).unwrap();
    let error = transport.library_list(path).await.unwrap_err();
    server.join().unwrap();
    assert_eq!(error.kind, ErrorKind::TooLarge);
    assert!(error.message.contains("流式接收"), "{}", error.message);
    assert!(error.message.contains("XML"), "{}", error.message);
    assert!(error.message.contains("256 KiB"), "{}", error.message);
    assert!(error.message.contains(path), "{}", error.message);
    assert!(
        error.message.contains("字节上限"),
        "byte budget must name the byte limit: {}",
        error.message
    );
    assert!(
        !error.message.contains("对象数") && !error.message.contains("项上限"),
        "a 256 KiB byte overrun must not borrow the directory object-count wording: {}",
        error.message
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn models_body_is_bounded_before_json_parse_and_normal_list_still_works() {
    for content_length in [true, false] {
        let body = format!(
            r#"{{"data":[{{"id":"{}"}}]}}"#,
            "x".repeat(ingestion::MAX_BODY_BYTES + 1)
        );
        let (base, server) = response("200 OK", body.into_bytes(), content_length);
        assert_eq!(
            ai::list_models(&client(), &ai_config(base))
                .await
                .unwrap_err()
                .code,
            "responseTooLarge"
        );
        server.join().unwrap();
    }
    let (base, server) = response(
        "200 OK",
        br#"{"data":[{"id":"b"},{"id":"a"}]}"#.to_vec(),
        true,
    );
    assert_eq!(
        ai::list_models(&client(), &ai_config(base)).await.unwrap(),
        ["a", "b"]
    );
    server.join().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ai_error_responses_keep_bounded_prefix_and_original_status() {
    for content_length in [true, false] {
        for models in [false, true] {
            let (base, server) = response(
                "503 Service Unavailable",
                "故障详情".repeat(2 * 1024 * 1024).into_bytes(),
                content_length,
            );
            let cfg = ai_config(base);
            let error = if models {
                ai::list_models(&client(), &cfg).await.unwrap_err()
            } else {
                ai::stream_chat(&client(), &cfg, "system", "user", &mut |_| true, 16)
                    .await
                    .err()
                    .expect("503 must fail")
            };
            assert!(error.message.contains("503"));
            assert!(error.message.contains("已截断"));
            assert!(
                error.message.len() < 8500,
                "error must not carry a 16MiB body over IPC"
            );
            assert_eq!(
                error.code,
                if models { "aiConnectivity" } else { "aiStream" }
            );
            server.join().unwrap();
        }
    }
}
