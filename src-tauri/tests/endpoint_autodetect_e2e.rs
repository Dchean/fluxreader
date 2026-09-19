//! TASK-059：端点自动适配的端到端验证。
//!
//! 覆盖本任务最容易做错的边界——**「路径不存在」与「凭据错误」必须可区分**：
//! 若把 401 也当成「路径不对」继续尝试其余候选，用户在密码填错时会收到
//! 误导性的「找不到 API」，比改动前更糟。
//!
//! 运行：cargo test --test endpoint_autodetect_e2e

mod mock_greader;

use app_lib::endpoint_resolve::{fever_candidates, greader_candidates, path_exists};
use mock_greader::MockGReader;
use std::sync::Arc;

async fn setup(name: &str) -> (Arc<MockGReader>, reqwest::Client) {
    let server = MockGReader::start().await.expect("start mock server");
    let _ = name;
    (server, app_lib::ingestion::build_client(10))
}

/// (a) 纯域名 + FreshRSS 形态（API 在 /api/greader.php）⇒ 自动适配并登录成功。
#[tokio::test]
async fn bare_domain_resolves_freshrss_layout() {
    let (server, http) = setup("ad_freshrss").await;
    // FreshRSS 形态：根路径 404，API 在 /api/greader.php
    server.set_greader_api_prefix("/api/greader.php");

    let bare = server.url(); // 例：http://127.0.0.1:PORT（纯域名，无后缀）
    let client = app_lib::greader::GReaderClient::login(&bare, "u", "p", http)
        .await
        .expect("纯域名应能自动适配 FreshRSS 形态");
    // 解析出的 base 应指向 FreshRSS 的子路径
    assert!(
        client.resolved_base().ends_with("/api/greader.php"),
        "应解析到 FreshRSS 形态，实际 {}",
        client.resolved_base()
    );
    let subs = client.subscriptions().await.expect("拉订阅");
    assert!(!subs.is_empty(), "应能拉到订阅");
}

/// (b) 纯域名 + Miniflux 形态（API 就在站点根）⇒ 首个候选即命中。
#[tokio::test]
async fn bare_domain_resolves_miniflux_layout() {
    let (server, http) = setup("ad_miniflux").await;
    server.set_greader_api_prefix(""); // 根路径即 API

    let bare = server.url();
    let client = app_lib::greader::GReaderClient::login(&bare, "u", "p", http)
        .await
        .expect("纯域名应能适配 Miniflux 形态");
    assert_eq!(
        client.resolved_base(),
        bare.trim_end_matches('/'),
        "Miniflux 形态应保持站点根（首个候选命中）"
    );
}

/// (c) **向后兼容**：已填完整路径 ⇒ 首个候选即命中，行为不变。
#[tokio::test]
async fn full_path_still_works_first_try() {
    let (server, http) = setup("ad_fullpath").await;
    server.set_greader_api_prefix("/api/greader.php");

    let full = format!("{}/api/greader.php", server.url().trim_end_matches('/'));
    let client = app_lib::greader::GReaderClient::login(&full, "u", "p", http)
        .await
        .expect("完整路径应一次成功");
    assert_eq!(client.resolved_base(), full);
}

/// (d) **核心边界**：路径全错 ⇒ 报「找不到 API」；
///     路径正确但凭据错 ⇒ 报**凭据类**错误，且**不再继续尝试其它候选**。
#[tokio::test]
async fn missing_path_vs_bad_credentials_are_distinguished() {
    let (server, http) = setup("ad_distinguish").await;
    // 该 mock 上 /api/greader.php 不存在（prefix 为空表示 API 在根），
    // 故「填一个自带错误后缀的地址」会得到两个候选都 404 的情形。
    server.set_greader_api_prefix("");

    // 情形 1：完全找不到 API（两个候选都 404）
    let bogus = format!("{}/nope", server.url().trim_end_matches('/'));
    let err = app_lib::greader::GReaderClient::login(&bogus, "u", "p", http.clone())
        .await
        .expect_err("路径不存在时应报错");
    assert!(
        err.message.contains("找不到 GReader API"),
        "应为『找不到 API』类错误，实际：{}",
        err.message
    );

    // 情形 2：路径**正确**但凭据被拒（401）⇒ 必须报凭据/HTTP 类错误，
    // 且**不得**退化成「找不到 API」（即不得继续把剩余候选试完）
    server.set_reject_login(true);
    let good = server.url();
    let err2 = app_lib::greader::GReaderClient::login(&good, "u", "p", http)
        .await
        .expect_err("凭据被拒时应报错");
    assert!(
        !err2.message.contains("找不到 GReader API"),
        "凭据错误**不得**被报成『找不到 API』（这是本任务的核心边界），实际：{}",
        err2.message
    );
    assert!(
        err2.message.contains("401") || err2.message.contains("ClientLogin"),
        "应如实反映凭据/HTTP 层失败，实际：{}",
        err2.message
    );
}

/// (e) 候选表有界且形态正确（纯函数，无需网络）。
#[test]
fn candidates_are_bounded_and_shaped_correctly() {
    assert_eq!(
        greader_candidates("https://demo.freshrss.org"),
        vec![
            "https://demo.freshrss.org".to_string(),
            "https://demo.freshrss.org/api/greader.php".to_string()
        ]
    );
    // 已带后缀时不追加重复候选
    assert_eq!(greader_candidates("https://x/api/greader.php").len(), 1);
    // Fever 同构
    assert_eq!(
        fever_candidates("https://demo.freshrss.org"),
        vec![
            "https://demo.freshrss.org".to_string(),
            "https://demo.freshrss.org/api/fever.php".to_string()
        ]
    );
    assert_eq!(fever_candidates("https://x/api/fever.php").len(), 1);

    // 只有 404 代表「路径不存在」；凭据类状态码绝不能算作路径缺失
    assert!(!path_exists(404));
    for s in [200, 400, 401, 403, 405, 410, 500, 503] {
        assert!(path_exists(s), "status {s} 必须视为路径存在");
    }
}

/// (f) Fever：纯域名 + FreshRSS 形态（`/api/fever.php`）⇒ 自动适配成功。
#[tokio::test]
async fn fever_bare_domain_resolves_freshrss_endpoint() {
    let (server, http) = setup("ad_fever").await;
    // FreshRSS 形态：/fever/ 不存在，端点在 /api/fever.php
    server.set_fever_endpoint("/api/fever.php");

    let bare = server.url();
    let client = app_lib::fever::FeverClient::new(&bare, "u", "p", http);
    client
        .verify()
        .await
        .expect("Fever 纯域名应能适配 FreshRSS 形态");
}

/// (g) Fever：接受 api_version >= 3（owner 授权放宽；FreshRSS 实测返回 4）。
#[tokio::test]
async fn fever_accepts_api_version_4() {
    let (server, http) = setup("ad_fever_v4").await;
    server.set_fever_endpoint("");
    server.set_fever_api_version(4);

    let client = app_lib::fever::FeverClient::new(&server.url(), "u", "p", http);
    client
        .verify()
        .await
        .expect("api_version=4 应被接受（FreshRSS 实测形态）");
}

/// (h) Fever：版本低于 3 仍须拒绝（放宽不等于不校验）。
#[tokio::test]
async fn fever_rejects_api_version_below_3() {
    let (server, http) = setup("ad_fever_v2").await;
    server.set_fever_endpoint("");
    server.set_fever_api_version(2);

    let client = app_lib::fever::FeverClient::new(&server.url(), "u", "p", http);
    let err = client.verify().await.expect_err("api_version=2 应被拒绝");
    assert!(
        err.message.contains("不支持的 Fever API 版本"),
        "应报版本不受支持，实际：{}",
        err.message
    );
}

/// (i) Fever：`auth != 1` 仍须报认证失败（**不得**因放宽版本而放松 auth）。
#[tokio::test]
async fn fever_still_rejects_bad_auth() {
    let (server, http) = setup("ad_fever_auth").await;
    server.set_fever_endpoint("");
    server.set_fever_api_version(4);
    server.set_fever_reject_auth(true);

    let client = app_lib::fever::FeverClient::new(&server.url(), "u", "p", http);
    let err = client.verify().await.expect_err("auth=0 应被拒绝");
    assert!(
        err.message.contains("认证失败"),
        "应报认证失败，实际：{}",
        err.message
    );
}

/// (j) **解析结果必须被采用**：`resolve()` 返回的客户端必须已指向解析出的端点。
///
/// 只验证「连接成功」（`verify() -> ()`）是不够的——旧实现把解析结果丢掉，
/// 同步侧仍拿纯域名去拼 `{域名}/fever/?api`，在 FreshRSS 上依旧 404。
#[tokio::test]
async fn fever_resolved_client_actually_uses_resolved_endpoint() {
    let (server, http) = setup("ad_fever_adopt").await;
    server.set_fever_endpoint("/api/fever.php");

    let bare = server.url();
    let resolved = app_lib::fever::FeverClient::new(&bare, "u", "p", http)
        .resolve()
        .await
        .expect("应解析成功");
    assert!(
        resolved.resolved_base().ends_with("/api/fever.php"),
        "解析结果应指向 FreshRSS 端点，实际 {}",
        resolved.resolved_base()
    );

    // 采用解析结果的客户端能真正拉到数据（这才是同步实际走的路径）
    server.clear_request_log();
    let tags = resolved.tags().await.expect("用解析后的端点拉分组");
    assert!(!tags.is_empty(), "应能拉到分组");
    let log = server.request_log();
    assert!(
        log.iter().all(|r| r.contains("/api/fever.php")),
        "解析后所有请求都应打在解析出的端点上，实际：{log:?}"
    );
}

/// (j2) **写路径也必须走解析出的端点**（TASK-059 独立审查发现的缺陷）。
///
/// `mark_items` 曾**绕过** `api_entry()` 写死 `{base}/fever/?api`，于是 FreshRSS 形态下
/// 拼成 `…/api/fever.php/fever/?api` → 404：**「Fever + FreshRSS」拉得到、推不出去**，
/// 同步报告会一直累积「Fever mark … → 404」。本测试锁定写路径与读路径一致。
#[tokio::test]
async fn fever_write_path_uses_resolved_endpoint_too() {
    let (server, http) = setup("ad_fever_mark").await;
    server.set_fever_endpoint("/api/fever.php");

    let resolved = app_lib::fever::FeverClient::new(&server.url(), "u", "p", http)
        .resolve()
        .await
        .expect("应解析成功");

    server.clear_request_log();
    resolved.mark_read(&[1001]).await.expect("标记已读应成功");
    resolved.mark_starred(&[1001]).await.expect("标记收藏应成功");

    let log = server.request_log();
    assert!(!log.is_empty(), "应发出标记请求");
    assert!(
        log.iter().all(|r| r.contains("/api/fever.php")),
        "写路径也必须打在解析出的端点上，实际：{log:?}"
    );
    assert!(
        !log.iter().any(|r| r.contains("/api/fever.php/fever/")),
        "不得把两种形态叠起来拼接（写死 /fever/ 的典型症状），实际：{log:?}"
    );
}

/// (k) **探测有界**：纯域名 + FreshRSS 形态最多试 2 个候选；
///     完整路径则**首个候选即命中**，不产生多余探测。
#[tokio::test]
async fn probing_is_bounded_and_full_path_does_not_extra_probe() {
    let (server, http) = setup("ad_bounded").await;
    server.set_greader_api_prefix("/api/greader.php");

    // 纯域名：两个候选（根 404 → /api/greader.php 命中）
    let bare = server.url();
    let _ = app_lib::greader::GReaderClient::login(&bare, "u", "p", http.clone())
        .await
        .expect("纯域名应解析成功");
    let bare_probes = server.login_request_count();
    assert_eq!(
        bare_probes, 2,
        "纯域名 + FreshRSS 形态应恰好探测 2 次（候选表有界），实际 {bare_probes}"
    );

    // 完整路径：首个候选即命中，1 次
    server.clear_request_log();
    let full = format!("{}/api/greader.php", server.url().trim_end_matches('/'));
    let _ = app_lib::greader::GReaderClient::login(&full, "u", "p", http)
        .await
        .expect("完整路径应一次成功");
    let full_probes = server.login_request_count();
    assert_eq!(
        full_probes, 1,
        "完整路径应首个候选即命中、无多余探测，实际 {full_probes}"
    );
}

/* ============================================================
解析缓存：不重复探测 + 输入变更即失效
============================================================ */

/// 建一个临时库（缓存读写需要 settings 表）。
fn temp_db(name: &str) -> rusqlite::Connection {
    let tmp = std::env::temp_dir().join(format!(
        "fluxreader_endpoint_cache_{}_{}_{}.db",
        name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&tmp);
    app_lib::db::open(&tmp).expect("open db")
}

/// (l) 缓存写入后可命中；**输入变了就不命中**（避免「改了地址仍按旧地址同步」）。
#[test]
fn cache_hits_on_same_input_and_invalidates_on_change() {
    let conn = temp_db("invalidate");
    let base = "https://demo.freshrss.org/api/greader.php";

    app_lib::endpoint_resolve::remember_base(&conn, "greader", "https://demo.freshrss.org", base)
        .expect("写入缓存");

    assert_eq!(
        app_lib::endpoint_resolve::cached_base(&conn, "greader", "https://demo.freshrss.org"),
        Some(base.to_string()),
        "同一输入应命中缓存"
    );
    // 尾斜杠/空白差异视为同一输入（归一化）
    assert_eq!(
        app_lib::endpoint_resolve::cached_base(&conn, "greader", "  https://demo.freshrss.org/  "),
        Some(base.to_string()),
        "归一化后应命中"
    );
    // 用户改了地址 → 必须失效并重新解析
    assert_eq!(
        app_lib::endpoint_resolve::cached_base(&conn, "greader", "https://other.example.org"),
        None,
        "改地址后缓存必须失效"
    );
    // 换了协议 → 同样失效（两个协议的端点形态不同）
    assert_eq!(
        app_lib::endpoint_resolve::cached_base(&conn, "fever", "https://demo.freshrss.org"),
        None,
        "换协议后缓存必须失效"
    );

    let _ = std::fs::remove_file(conn.path().unwrap());
}

/// (m) **缓存生效的实证**：首轮同步探测并落库，次轮同步**不再发探测请求**。
///
/// `build_client` 在 feeds/states/调度同步中被反复调用——不缓存就会每轮重探。
#[tokio::test]
async fn second_sync_reuses_cached_endpoint_without_probing() {
    let (server, http) = setup("ad_cache").await;
    // FreshRSS 形态：纯域名必须靠探测才能找到 /api/greader.php
    server.set_greader_api_prefix("/api/greader.php");

    let conn = temp_db("cache_reuse");
    app_lib::db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
    app_lib::db::set_setting(&conn, "greader_username", "test").unwrap();
    app_lib::db::set_setting(&conn, "greader_password", "test-token").unwrap();
    let db = std::sync::Arc::new(tokio::sync::Mutex::new(conn));

    // 首轮：应发生探测（根 404 → 子路径命中），并把结果落库
    app_lib::sync::feeds_phase(&db, &http)
        .await
        .expect("首轮同步成功");
    let first_probes = server.login_request_count();
    assert_eq!(first_probes, 2, "首轮应探测 2 次，实际 {first_probes}");
    {
        let conn = db.lock().await;
        assert_eq!(
            app_lib::endpoint_resolve::cached_base(&conn, "greader", &server.url()),
            Some(format!("{}/api/greader.php", server.url().trim_end_matches('/'))),
            "首轮同步后应已把解析结果落库"
        );
    }

    // 次轮：命中缓存，**不再探测**——登录只打解析出的地址
    server.clear_request_log();
    app_lib::sync::feeds_phase(&db, &http)
        .await
        .expect("次轮同步成功");
    let log = server.request_log();
    let logins: Vec<&String> = log
        .iter()
        .filter(|r| r.ends_with("/accounts/ClientLogin"))
        .collect();
    assert_eq!(
        logins.len(),
        1,
        "次轮应复用缓存、恰好登录 1 次（不重复探测），实际请求：{log:?}"
    );
    assert!(
        logins[0].contains("/api/greader.php"),
        "次轮应直接打解析出的端点，实际：{}",
        logins[0]
    );

    let guard = db.lock().await;
    let _ = std::fs::remove_file(guard.path().unwrap());
}
