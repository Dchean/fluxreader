//! GitHub 设备流登录 headless 测试（mock 服务器驱动）。
//! 覆盖：设备码签发（含 verification_uri/interval）→ pending 轮询 →
//! 批准后取 token → /user 账户验证。运行：cargo test --test github_auth_e2e

use app_lib::github_auth;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn device_flow_full_lifecycle() {
    let approved = Arc::new(AtomicBool::new(false));
    let mock = spawn_mock(approved.clone());

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();

    // 1. 发起：拿到 user_code / device_code / verification_uri / interval
    let (user_code, device_code, verification_uri, interval) =
        github_auth::device_code_request_for_test(&http, &mock.base, "cid_test").await.unwrap();
    assert_eq!(user_code, "ABCD-1234");
    assert_eq!(device_code, "dc_fixed");
    assert!(verification_uri.contains("/login/device"));
    assert!(interval >= 1, "interval 至少 1 秒");

    // 2. 未批准时轮询 → None（authorization_pending）
    let pending = github_auth::token_poll_once_for_test(&http, &mock.base, "cid_test", &device_code)
        .await
        .unwrap();
    assert!(pending.is_none());

    // 3. 批准后轮询 → token
    approved.store(true, Ordering::SeqCst);
    let token = github_auth::token_poll_once_for_test(&http, &mock.base, "cid_test", &device_code)
        .await
        .unwrap()
        .expect("批准后应有 token");
    assert_eq!(token, "gho_testtoken123");

    // 4. token 换账户
    let account = github_auth::fetch_account_for_test(&http, &mock.base, &token).await.unwrap();
    assert_eq!(account.login, "testuser");
}

#[tokio::test]
async fn bad_token_rejected_by_user_endpoint() {
    let approved = Arc::new(AtomicBool::new(true));
    let mock = spawn_mock(approved);
    let http = reqwest::Client::builder().timeout(Duration::from_secs(10)).build().unwrap();
    let bad = github_auth::fetch_account_for_test(&http, &mock.base, "gho_wrong").await;
    assert!(bad.is_err(), "错误 token 拉 /user 应 401 失败");
}

/* ---------------- mock ---------------- */

struct Mock {
    base: String,
    _keep: std::sync::mpsc::Sender<()>,
}

fn spawn_mock(approved: Arc<AtomicBool>) -> Mock {
    let (tx, _rx) = std::sync::mpsc::channel();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let approved = approved.clone();
            std::thread::spawn(move || handle(stream, approved));
        }
    });
    Mock { base: format!("http://127.0.0.1:{port}"), _keep: tx }
}

fn handle(mut stream: std::net::TcpStream, approved: Arc<AtomicBool>) {
    use std::io::{Read, Write};
    let mut buf = [0u8; 4096];
    let Ok(n) = stream.read(&mut buf) else { return };
    let req = String::from_utf8_lossy(&buf[..n]).to_string();
    let first_line = req.lines().next().unwrap_or("").to_string();
    let raw_path = first_line.split_whitespace().nth(1).unwrap_or("").to_string();
    let path = raw_path.split('?').next().unwrap_or("").to_string();

    let (status, json_out) = match path.as_str() {
        "/login/device/code" => (
            200,
            r#"{"device_code":"dc_fixed","user_code":"ABCD-1234","verification_uri":"https://github.com/login/device","expires_in":900,"interval":1}"#.to_string(),
        ),
        "/login/oauth/access_token" => {
            if approved.load(Ordering::SeqCst) {
                (200, r#"{"access_token":"gho_testtoken123","token_type":"bearer","scope":"gist"}"#.to_string())
            } else {
                (200, r#"{"error":"authorization_pending"}"#.to_string())
            }
        }
        "/user" => {
            if req.contains("gho_testtoken123") {
                (200, r#"{"login":"testuser","id":42,"name":"Test User"}"#.to_string())
            } else {
                (401, r#"{"message":"Bad credentials"}"#.to_string())
            }
        }
        _ => (404, r#"{"error":"not found"}"#.to_string()),
    };
    let reason = if status == 200 { "OK" } else if status == 401 { "Unauthorized" } else { "Not Found" };
    let resp = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status,
        reason,
        json_out.len(),
        json_out
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();
}
