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
        let length = if content_length {
            format!("Content-Length: {}\r\n", body.len())
        } else {
            String::new()
        };
        let header = format!("HTTP/1.1 {status}\r\nContent-Type: application/json; charset=utf-8\r\n{length}Connection: close\r\n\r\n");
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
