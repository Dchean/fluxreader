//! TASK-101：Fever 协议对 FreshRSS 的认证修复（api_key 必须走 POST form body）。
//!
//! 根因（tmp/task-101/fever-analysis.md，FreshRSS 官方源码逐行取证）：
//! FreshRSS `p/api/fever.php:172` 只读 `$_POST['api_key']`（POST form body），
//! 完全不读 query。旧实现把 api_key 拼在 URL query 上、POST 空 body，FreshRSS
//! 收到的 api_key 恒为空 → auth 恒 0 →「Fever 认证失败（api_key 不正确）」。
//!
//! 本文件的 mock 模拟 **FreshRSS 行为**：api_key 只认 POST form body，query 里
//! 带了也当无效（服务端根本不读 query 里的 api_key）。旧实现对这些用例全部失败
//! （修前红证据：tmp/task-101/fever-red.log），修复后必须全绿。
//!
//! 另锁定：
//! - 防回退断言：任何 Fever 请求的 query 中不得出现 api_key（兼消除其进
//!   服务器访问日志的泄露面）；
//! - Miniflux 兼容不回退：action 在 query、api_key 在 form body 的请求形状；
//! - 第三候选 `{base}/p/api/fever.php`（FreshRSS 新版布局）404 顺延；
//! - 认证失败文案含 FreshRSS「API 密码」指引（call_probe / mark_items 统一口径）。
//!
//! 运行：cargo test --test fever_freshrss_e2e

mod mock_greader;

use mock_greader::MockGReader;
use std::sync::Arc;

/// 与 fever.rs 单测一致的凭据：api_key = md5("REDACTED_USER:REDACTED_PASSWORD")
/// （公式本任务不改——修前即正确，错的是传参位置）。
const USER: &str = "REDACTED_USER";
const PASS: &str = "REDACTED_PASSWORD";
const EXPECTED_API_KEY: &str = "6ac0be0ba0aa8e8a4972b225d7cea926";

async fn setup() -> Arc<MockGReader> {
    MockGReader::start().await.expect("start mock server")
}

fn http() -> reqwest::Client {
    app_lib::ingestion::build_client(10)
}

/// 防回退断言：任何 Fever 请求的 query 都不得出现 api_key。
///
/// 旧实现把 api_key 拼 query（正是 FreshRSS 认证失败的根因，还会泄进服务器
/// 访问日志）；修复后 api_key 只能走 POST form body。记录覆盖路由/404 拦截
/// 之前的全部 Fever 请求（含被 404 拒掉的探测）。
fn assert_no_api_key_in_queries(server: &MockGReader) {
    let queries = server.fever_query_log();
    assert!(!queries.is_empty(), "应已发出 Fever 请求（否则断言无证据）");
    for q in &queries {
        assert!(
            !q.contains("api_key"),
            "请求 query 中不得出现 api_key（必须走 POST form body），实际 query：{q:?}"
        );
    }
}

/// (a) **核心修复**：FreshRSS 形态（api_key 只认 POST form body）下认证必须成功。
///
/// mock 模拟 FreshRSS：`/api/fever.php` 只读 `$_POST['api_key']`，query 里带了也当
/// 无效。旧实现（api_key 拼 query、POST 空 body）在该 mock 上恒 auth=0 → 认证失败
/// （修前红证据留档）；修复后 api_key 走 form body → auth=1。
#[tokio::test]
async fn freshrss_accepts_api_key_only_from_post_body() {
    let server = setup().await;
    server.set_fever_endpoint("/api/fever.php");
    server.set_fever_expected_api_key(EXPECTED_API_KEY);

    let client = app_lib::fever::FeverClient::new(&server.url(), USER, PASS, http());
    client
        .verify()
        .await
        .expect("api_key 走 POST form body 后应通过 FreshRSS 形态认证");

    assert_no_api_key_in_queries(&server);
}

/// (b) **防回退（读 + 写两条路径）**：解析、拉分组、拉未读集合、mark 写入的
/// 请求 query 中都不得出现 api_key；action/mark/as/id 参数仍留在 query
/// （FreshRSS 全部走 $_REQUEST，Miniflux 走 FormValue，query 可用——不动）。
#[tokio::test]
async fn request_query_never_carries_api_key_on_read_and_write() {
    let server = setup().await;
    server.set_fever_endpoint("/api/fever.php");
    server.set_fever_expected_api_key(EXPECTED_API_KEY);

    let resolved = app_lib::fever::FeverClient::new(&server.url(), USER, PASS, http())
        .resolve()
        .await
        .expect("解析并认证成功");
    resolved.tags().await.expect("读路径：拉分组");
    resolved.unread_item_ids().await.expect("读路径：未读集合");
    resolved.mark_read(&[42]).await.expect("写路径：标记已读");

    assert_no_api_key_in_queries(&server);

    // 形状锁定：action 与 mark/as/id 仍在 query（禁止顺手全搬进 body 造成
    // Miniflux 的「action 不能放 form body」回退）
    let queries = server.fever_query_log();
    assert!(
        queries.iter().any(|q| q.contains("groups")),
        "groups action 应留在 query：{queries:?}"
    );
    assert!(
        queries
            .iter()
            .any(|q| q.contains("mark=item") && q.contains("as=read") && q.contains("id=42")),
        "mark/as/id 参数应留在 query：{queries:?}"
    );
}

/// (c) **Miniflux 兼容不回退**：Miniflux 形态（`{base}/fever/`，mock 默认）下
/// 同样必须 api_key 走 form body、action 留 query 的形状（Miniflux 的
/// r.FormValue 对 query 与 body 都收，form body 是两种后端的公共交集）。
#[tokio::test]
async fn miniflux_shape_action_in_query_api_key_in_body() {
    let server = setup().await;
    server.set_fever_expected_api_key(EXPECTED_API_KEY);

    let client = app_lib::fever::FeverClient::new(&server.url(), USER, PASS, http());
    client
        .verify()
        .await
        .expect("Miniflux 形态在 api_key 走 form body 后应继续工作");

    assert_no_api_key_in_queries(&server);
    // 探测请求形状：{base}/fever/?api —— query 仅含 `api`（无值 action 缺省），
    // api_key 不在 query
    let queries = server.fever_query_log();
    assert!(
        queries.iter().all(|q| q == "api"),
        "探测请求的 query 应仅含 api 参数，实际：{queries:?}"
    );
}

/// (d) **第三候选**：FreshRSS 新版布局把 fever.php 移到 `p/api/` 下——
/// 前两个候选（`{base}/fever/`、`{base}/api/fever.php`）404 后顺延尝试并命中
/// `{base}/p/api/fever.php`。「非 404 = 路径存在立即停」的既有纪律不变
/// （由 endpoint_autodetect_e2e 的既有用例锁定）。
#[tokio::test]
async fn freshrss_new_layout_p_api_candidate_resolves_after_404() {
    let server = setup().await;
    // mock 只认 /p/api/fever.php；/fever/ 与 /api/fever.php 一律 404
    server.set_fever_endpoint("/p/api/fever.php");
    server.set_fever_expected_api_key(EXPECTED_API_KEY);

    let resolved = app_lib::fever::FeverClient::new(&server.url(), USER, PASS, http())
        .resolve()
        .await
        .expect("第三候选 p/api/fever.php 应在 404 顺延后被命中");
    assert!(
        resolved.resolved_base().ends_with("/p/api/fever.php"),
        "应解析到 FreshRSS 新版布局端点，实际 {}",
        resolved.resolved_base()
    );

    // 解析出的端点能真正拉到数据（不只是连上）
    let tags = resolved.tags().await.expect("用解析后的端点拉分组");
    assert!(!tags.is_empty(), "应能拉到分组");
    assert_no_api_key_in_queries(&server);
}

/// (e) 认证失败文案：统一口径补 FreshRSS「API 密码」指引（call_probe 与
/// mark_items 两处共用；FreshRSS 的 Fever 与 GReader 都用个人设置里的 API 密码，
/// 提示用户别拿登录密码试 Fever）。
#[tokio::test]
async fn auth_failure_message_mentions_freshrss_api_password() {
    let server = setup().await;
    // 期望 key 与客户端算出的不一致 → auth=0（模拟 FreshRSS 密码错/未设 API 密码）
    server.set_fever_expected_api_key("00000000000000000000000000000000");

    let client = app_lib::fever::FeverClient::new(&server.url(), USER, PASS, http());
    let err = client.verify().await.expect_err("api_key 不匹配应认证失败");
    assert!(
        err.message.contains("认证失败"),
        "应报认证失败，实际：{}",
        err.message
    );
    assert!(
        err.message.contains("API 密码"),
        "认证失败文案应含 FreshRSS「API 密码」指引，实际：{}",
        err.message
    );
}
