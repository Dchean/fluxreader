//! greader.rs 连真实 Miniflux 的端到端测试（`--ignored`，需网络 + 测试账号）。
//!
//! 测试账号：test / testtest（Google Reader 集成凭据），后端 https://rss.chean.top/
//! 这是「证据优先」(R1) 的关键验证——用真实后端实证 greader 客户端的正确性。
//! 不跑在 CI（依赖外部服务），本地 `cargo test -- --ignored` 手动跑。

use app_lib::greader::{self, GReaderClient};

/// 从环境变量读测试凭据（缺省用文档里公开的测试账号）。
fn test_creds() -> (String, String, String) {
    let endpoint = std::env::var("FLUXREADER_TEST_ENDPOINT")
        .unwrap_or_else(|_| "https://rss.chean.top".to_string());
    let username = std::env::var("FLUXREADER_TEST_USER").unwrap_or_else(|_| "test".to_string());
    let password = std::env::var("FLUXREADER_TEST_PASS").unwrap_or_else(|_| "testtest".to_string());
    (endpoint, username, password)
}

#[tokio::test]
#[ignore = "需真实 Miniflux 测试账号 + 网络"]
async fn greader_login_and_subscriptions() {
    let (endpoint, username, password) = test_creds();
    let http = reqwest::Client::new();
    let client = GReaderClient::login(&endpoint, &username, &password, http).await.unwrap();

    let subs = client.subscriptions().await.unwrap();
    assert!(!subs.is_empty(), "测试账号应有订阅源");
    // 每个订阅应有 feed/数字 id
    for s in &subs {
        assert!(greader::parse_feed_numeric_id(&s.id).is_some(), "订阅 id 应是 feed/数字：{}", s.id);
    }
    println!("订阅数: {}", subs.len());
}

#[tokio::test]
#[ignore = "需真实 Miniflux 测试账号 + 网络"]
async fn greader_item_ids_and_contents() {
    let (endpoint, username, password) = test_creds();
    let http = reqwest::Client::new();
    let client = GReaderClient::login(&endpoint, &username, &password, http).await.unwrap();

    // 拉 reading-list 前 5 条 id
    let ids = client
        .item_ids("user/-/state/com.google/reading-list", None, None, Some(5), None)
        .await
        .unwrap();
    assert!(!ids.item_refs.is_empty(), "reading-list 应有条目");
    let numeric: Vec<i64> = ids.item_refs.iter().filter_map(|r| r.id.parse().ok()).collect();
    assert!(!numeric.is_empty());

    // 拉正文
    let contents = client.item_contents(&numeric).await.unwrap();
    assert_eq!(contents.len(), numeric.len(), "正文数应与 id 数一致");
    for c in &contents {
        assert!(!c.title.is_empty(), "条目应有标题");
    }
    println!("拉取 {} 条正文", contents.len());
}

#[tokio::test]
#[ignore = "需真实 Miniflux 测试账号 + 网络"]
async fn greader_edit_tag_roundtrip() {
    let (endpoint, username, password) = test_creds();
    let http = reqwest::Client::new();
    let client = GReaderClient::login(&endpoint, &username, &password, http).await.unwrap();

    // 拿一条真实条目
    let ids = client
        .item_ids("user/-/state/com.google/reading-list", None, None, Some(1), None)
        .await
        .unwrap();
    let id: i64 = ids.item_refs[0].id.parse().unwrap();

    // 读初始 starred 状态
    let before = client.item_contents(&[id]).await.unwrap();
    let before_starred = greader::has_tag(&before[0].categories, "/com.google/starred");

    // 收藏 → 验证 → 恢复
    client.mark_starred(&[id]).await.unwrap();
    let after = client.item_contents(&[id]).await.unwrap();
    assert!(greader::has_tag(&after[0].categories, "/com.google/starred"), "应已收藏");

    // 恢复原状
    if before_starred {
        client.mark_starred(&[id]).await.unwrap();
    } else {
        client.mark_unstarred(&[id]).await.unwrap();
    }
    println!("edit-tag 往返验证通过（id {id}）");
}
