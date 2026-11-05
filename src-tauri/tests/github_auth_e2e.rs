//! GitHub 设备流登录 headless 测试（mock 服务器驱动）。
//! 覆盖：设备码签发（含 verification_uri/interval）→ pending 轮询 →
//! 批准后取 token → /user 账户验证。运行：cargo test --test github_auth_e2e

use app_lib::db;
use app_lib::github_auth;
use app_lib::media::MediaHandle;
use app_lib::state::AppState;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

mod common;

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
        github_auth::device_code_request_for_test(&http, &mock.base, "cid_test")
            .await
            .unwrap();
    assert_eq!(user_code, "ABCD-1234");
    assert_eq!(device_code, "dc_fixed");
    assert!(verification_uri.contains("/login/device"));
    assert!(interval >= 1, "interval 至少 1 秒");

    // 2. 未批准时轮询 → None（authorization_pending）
    let pending =
        github_auth::token_poll_once_for_test(&http, &mock.base, "cid_test", &device_code)
            .await
            .unwrap();
    assert!(pending.is_none());

    // 3. 批准后轮询 → token
    approved.store(true, Ordering::SeqCst);
    let token = github_auth::token_poll_once_for_test(&http, &mock.base, "cid_test", &device_code)
        .await
        .unwrap()
        .expect("批准后应有 token");
    assert_eq!(token, MOCK_TOKEN);

    // 4. token 换账户
    let account = github_auth::fetch_account_for_test(&http, &mock.base, &token)
        .await
        .unwrap();
    assert_eq!(account.login, "testuser");
}

#[tokio::test]
async fn bad_token_rejected_by_user_endpoint() {
    let approved = Arc::new(AtomicBool::new(true));
    let mock = spawn_mock(approved);
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let bad = github_auth::fetch_account_for_test(&http, &mock.base, "gho_wrong").await;
    assert!(bad.is_err(), "错误 token 拉 /user 应 401 失败");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_poll_after_cancel_cannot_persist_account_or_clear_flow() {
    let approved = Arc::new(AtomicBool::new(true));
    let (mock, seen, release) = spawn_gated_mock(approved);
    let (db_guard, state) = test_state("github_cancel");
    let state = Arc::new(state);

    github_auth::github_login_start_for_test(
        &state,
        &mock.base,
        Some("cid_test".to_string()),
        Some(true),
    )
    .await
    .unwrap();
    let old_generation = github_auth::github_flow_generation_for_test(&state)
        .await
        .expect("start 后应有 flow");

    let poll_state = Arc::clone(&state);
    let poll_base = mock.base.clone();
    let poll = tokio::spawn(async move {
        github_auth::github_login_poll_for_test(&poll_state, &poll_base, &poll_base).await
    });
    seen.recv_timeout(Duration::from_secs(5))
        .expect("mock 应收到 token 轮询");

    let cancel = github_auth::github_login_cancel_for_test(&state)
        .await
        .unwrap();
    assert!(cancel.cancelled, "取消应命中旧发起 generation");
    assert_eq!(
        github_auth::github_flow_generation_for_test(&state).await,
        None,
        "取消后 state 不得留下旧 flow"
    );

    release_token(release);
    let err = poll
        .await
        .expect("poll task 不应 panic")
        .expect_err("取消后旧 poll 不得返回成功");
    assert_eq!(err.code, "cancelled", "旧 poll 必须明确返回 cancelled");
    assert_eq!(
        mock.user_hits.load(Ordering::SeqCst),
        0,
        "取消后旧 poll 不得请求 /user"
    );

    let conn = state.db.lock().await;
    assert_eq!(
        db::get_setting(&conn, "config_sync_credentials").unwrap(),
        None,
        "取消后旧 token 不得写入凭据"
    );
    assert_eq!(
        db::get_setting(&conn, "github_login_name").unwrap(),
        None,
        "取消后旧账号不得写入账户名"
    );
    drop(conn);
    drop(state);
    drop(db_guard);
    assert!(
        old_generation > 0,
        "发起必须绑定非零 generation（实际 {old_generation}）"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_poll_after_new_start_cannot_persist_old_account_or_clear_new_flow() {
    let approved = Arc::new(AtomicBool::new(true));
    let (mock, seen, release) = spawn_gated_mock(approved);
    let (db_guard, state) = test_state("github_new_start");
    let state = Arc::new(state);

    github_auth::github_login_start_for_test(
        &state,
        &mock.base,
        Some("cid_test".to_string()),
        Some(true),
    )
    .await
    .unwrap();
    let old_generation = github_auth::github_flow_generation_for_test(&state)
        .await
        .expect("首次 start 后应有 flow");

    let poll_state = Arc::clone(&state);
    let poll_base = mock.base.clone();
    let poll = tokio::spawn(async move {
        github_auth::github_login_poll_for_test(&poll_state, &poll_base, &poll_base).await
    });
    seen.recv_timeout(Duration::from_secs(5))
        .expect("mock 应收到旧 flow 的 token 轮询");

    github_auth::github_login_start_for_test(
        &state,
        &mock.base,
        Some("cid_test".to_string()),
        Some(true),
    )
    .await
    .unwrap();
    let new_generation = github_auth::github_flow_generation_for_test(&state)
        .await
        .expect("新 start 后应留下新 flow");
    assert!(
        new_generation > old_generation,
        "新 flow 必须使用更新的 generation（旧 {old_generation}，新 {new_generation}）"
    );

    release_token(release);
    let err = poll
        .await
        .expect("poll task 不应 panic")
        .expect_err("被新流替换后旧 poll 不得返回成功");
    assert_eq!(err.code, "stale", "旧 poll 必须明确返回 stale");
    assert_eq!(
        mock.user_hits.load(Ordering::SeqCst),
        0,
        "被新流替换后旧 poll 不得请求 /user"
    );
    assert_eq!(
        github_auth::github_flow_generation_for_test(&state).await,
        Some(new_generation),
        "旧 poll 不得清掉或替换新 flow"
    );

    let conn = state.db.lock().await;
    assert_eq!(
        db::get_setting(&conn, "config_sync_credentials").unwrap(),
        None,
        "旧 poll 不得写入凭据"
    );
    assert_eq!(
        db::get_setting(&conn, "github_login_name").unwrap(),
        None,
        "旧 poll 不得写入账户名"
    );
    drop(conn);
    drop(state);
    drop(db_guard);
}

#[tokio::test]
async fn cancel_without_active_flow_returns_false() {
    let (_db_guard, state) = test_state("github_cancel_idle");
    let result = github_auth::github_login_cancel_for_test(&state)
        .await
        .unwrap();
    assert!(
        !result.cancelled,
        "无活动流时 cancel 必须返回 cancelled=false"
    );
    assert_eq!(
        github_auth::github_flow_generation_for_test(&state).await,
        None,
        "空闲取消不得伪造活动 flow"
    );
}

/* ---------------- mock ---------------- */

/// 本地 mock 的假 token：仅在本测试内自洽使用（mock 签发、mock 校验、断言比对），
/// 不是任何真实凭据。定义成常量使「签发 / 校验 / 断言」三处共享同一事实来源；
/// 响应用 `json!` 构造而非整体字符串字面量，避免被凭据扫描器误判为硬编码凭据。
const MOCK_TOKEN: &str = "mock-access-token-not-a-real-credential";

struct Mock {
    base: String,
    user_hits: Arc<AtomicUsize>,
    _keep: std::sync::mpsc::Sender<()>,
}

fn spawn_mock(approved: Arc<AtomicBool>) -> Mock {
    spawn_server(approved, None)
}

#[derive(Clone)]
struct TokenGate {
    seen: std::sync::mpsc::Sender<()>,
    release: TokenRelease,
}

type TokenRelease = Arc<(Mutex<bool>, Condvar)>;
type GatedMock = (Mock, std::sync::mpsc::Receiver<()>, TokenRelease);

fn spawn_gated_mock(approved: Arc<AtomicBool>) -> GatedMock {
    let (seen_tx, seen_rx) = std::sync::mpsc::channel();
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let mock = spawn_server(
        approved,
        Some(TokenGate {
            seen: seen_tx,
            release: Arc::clone(&release),
        }),
    );
    (mock, seen_rx, release)
}

fn release_token(release: TokenRelease) {
    let (lock, cvar) = &*release;
    let mut released = lock.lock().unwrap();
    *released = true;
    cvar.notify_all();
}

fn spawn_server(approved: Arc<AtomicBool>, gate: Option<TokenGate>) -> Mock {
    let (tx, _rx) = std::sync::mpsc::channel();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let user_hits = Arc::new(AtomicUsize::new(0));
    let handler_user_hits = Arc::clone(&user_hits);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let approved = approved.clone();
            let gate = gate.clone();
            let user_hits = Arc::clone(&handler_user_hits);
            std::thread::spawn(move || handle(stream, approved, gate, user_hits));
        }
    });
    Mock {
        base: format!("http://127.0.0.1:{port}"),
        user_hits,
        _keep: tx,
    }
}

fn handle(
    mut stream: std::net::TcpStream,
    approved: Arc<AtomicBool>,
    gate: Option<TokenGate>,
    user_hits: Arc<AtomicUsize>,
) {
    use std::io::{Read, Write};
    let mut buf = [0u8; 4096];
    let Ok(n) = stream.read(&mut buf) else { return };
    let req = String::from_utf8_lossy(&buf[..n]).to_string();
    let first_line = req.lines().next().unwrap_or("").to_string();
    let raw_path = first_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("")
        .to_string();
    let path = raw_path.split('?').next().unwrap_or("").to_string();

    let (status, json_out) = match path.as_str() {
        "/login/device/code" => (
            200,
            r#"{"device_code":"dc_fixed","user_code":"ABCD-1234","verification_uri":"https://github.com/login/device","expires_in":900,"interval":1}"#.to_string(),
        ),
        "/login/oauth/access_token" => {
            if approved.load(Ordering::SeqCst) {
                if let Some(gate) = gate {
                    let _ = gate.seen.send(());
                    let (lock, cvar) = &*gate.release;
                    let mut released = lock.lock().unwrap();
                    while !*released {
                        released = cvar.wait(released).unwrap();
                    }
                }
                (200, serde_json::json!({
                    "access_token": MOCK_TOKEN,
                    "token_type": "bearer",
                    "scope": "gist"
                }).to_string())
            } else {
                (200, r#"{"error":"authorization_pending"}"#.to_string())
            }
        }
        "/user" => {
            user_hits.fetch_add(1, Ordering::SeqCst);
            if req.contains(MOCK_TOKEN) {
                (200, r#"{"login":"testuser","id":42,"name":"Test User"}"#.to_string())
            } else {
                (401, r#"{"message":"Bad credentials"}"#.to_string())
            }
        }
        _ => (404, r#"{"error":"not found"}"#.to_string()),
    };
    let reason = if status == 200 {
        "OK"
    } else if status == 401 {
        "Unauthorized"
    } else {
        "Not Found"
    };
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

fn test_state(name: &str) -> (common::TempDbGuard, AppState) {
    let path = common::unique_db_path(name);
    let _ = std::fs::remove_file(&path);
    let guard = common::TempDbGuard::new(path.clone());
    let conn = db::open(&path).expect("open test db");
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    (guard, AppState::new(conn, http, MediaHandle::inactive()))
}
