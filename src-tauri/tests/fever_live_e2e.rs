//! fever.rs 连真实 Miniflux 的端到端测试（`--ignored`，需网络 + 测试账号）。
//!
//! 测试账号：test / testtest（Fever 与 Google Reader 共用「集成」凭据），
//! 后端 https://rss.chean.top/。这是「Fever 协议落地」的关键证据——实证
//! api_key 认证、feeds/groups、unread/saved 集合、items 分页、mark 写入均在
//! 真实 Miniflux 上工作。不跑在 CI，本地 `cargo test -- --ignored` 手动跑。

use app_lib::backend::{resolve_api_base, Protocol, ServerKind};
use app_lib::fever::FeverClient;
use app_lib::greader::{self, has_tag};

/// 从环境变量读测试凭据（缺省用文档里公开的测试账号）。
fn test_creds() -> (String, String, String) {
    let endpoint = std::env::var("FLUXREADER_TEST_ENDPOINT")
        .unwrap_or_else(|_| "https://rss.chean.top".to_string());
    let username = std::env::var("FLUXREADER_TEST_USER").unwrap_or_else(|_| "test".to_string());
    let password = std::env::var("FLUXREADER_TEST_PASS").unwrap_or_else(|_| "testtest".to_string());
    (endpoint, username, password)
}

/// 测试账号按 Miniflux 处理：Fever API base = {root}/fever/
fn fever_base(endpoint: &str) -> String {
    resolve_api_base(ServerKind::Miniflux, Protocol::Fever, endpoint)
}

#[tokio::test]
#[ignore = "需真实 Miniflux 测试账号 + 网络"]
async fn fever_auth_and_subscriptions() {
    let (endpoint, username, password) = test_creds();
    let client = FeverClient::new(&fever_base(&endpoint), &username, &password, reqwest::Client::new());

    client.verify().await.unwrap();

    let subs = client.subscriptions().await.unwrap();
    assert!(!subs.is_empty(), "测试账号应有订阅源");
    for s in &subs {
        assert!(
            greader::parse_feed_numeric_id(&s.id).is_some(),
            "订阅 id 应是 feed/数字：{}",
            s.id
        );
    }

    let tags = client.tags().await.unwrap();
    assert!(!tags.is_empty(), "测试账号应有分类");
    assert!(
        tags.iter().all(|t| t.r#type.as_deref() == Some("folder")),
        "Fever groups 映射为 folder 类型"
    );
    println!("Fever 认证通过：订阅 {} 个，分类 {} 个", subs.len(), tags.len());
}

#[tokio::test]
#[ignore = "需真实 Miniflux 测试账号 + 网络"]
async fn fever_unread_saved_and_items() {
    let (endpoint, username, password) = test_creds();
    let client = FeverClient::new(&fever_base(&endpoint), &username, &password, reqwest::Client::new());

    let unread = client.unread_item_ids().await.unwrap();
    let saved = client.saved_item_ids().await.unwrap();
    assert!(!unread.is_empty(), "测试账号应有未读条目");
    assert!(unread.iter().all(|id| *id > 0), "item id 应为正整数");
    println!("未读 {} 条，收藏 {} 条", unread.len(), saved.len());

    // since_id 分页拉取（从 0 开始，至少拉到 1 页 50 条）
    let page = client.items_since(0).await.unwrap();
    assert!(!page.is_empty());
    for c in &page {
        assert!(!c.title.is_empty(), "条目应有标题");
        assert!(
            greader::parse_item_id(&c.id).is_some(),
            "Fever item id 应可解析为十进制：{}",
            c.id
        );
        assert!(
            c.origin
                .as_ref()
                .and_then(|o| greader::parse_feed_numeric_id(&o.stream_id))
                .is_some(),
            "origin.stream_id 应为 feed/数字"
        );
    }
    println!("items since_id=0 拉到 {} 条", page.len());
}

#[tokio::test]
#[ignore = "需真实 Miniflux 测试账号 + 网络"]
async fn fever_mark_roundtrip() {
    let (endpoint, username, password) = test_creds();
    let client = FeverClient::new(&fever_base(&endpoint), &username, &password, reqwest::Client::new());

    // 拿一条真实条目
    let unread = client.unread_item_ids().await.unwrap();
    let mut id = unread[0];
    // 若拿到的条目在收藏集合里，换个 id 避免影响星标断言
    let saved = client.saved_item_ids().await.unwrap();
    if saved.contains(&id) && unread.len() > 1 {
        id = unread[1];
    }

    // 初始状态
    let before = client.items_with_ids(&[id]).await.unwrap();
    let before_read = has_tag(&before[0].categories, "/com.google/read");
    let before_starred = has_tag(&before[0].categories, "/com.google/starred");
    assert!(!before_read, "测试条目应初始未读");

    // 标已读 → 验证 → 恢复未读
    client.mark_read(&[id]).await.unwrap();
    let after = client.items_with_ids(&[id]).await.unwrap();
    assert!(has_tag(&after[0].categories, "/com.google/read"), "应已读");

    // 收藏 → 验证
    client.mark_starred(&[id]).await.unwrap();
    let starred = client.items_with_ids(&[id]).await.unwrap();
    assert!(has_tag(&starred[0].categories, "/com.google/starred"), "应已收藏");

    // 恢复原状（未读 + 原星标）
    client.mark_unread(&[id]).await.unwrap();
    if before_starred {
        client.mark_starred(&[id]).await.unwrap();
    } else {
        client.mark_unstarred(&[id]).await.unwrap();
    }
    println!("Fever mark 往返验证通过（id {id}）");
}