//! Protocol operation full-coverage live tests (phase 0 wrap-up): per-op x per-server.
//! Covers every protocol operation the engine actually triggers (non-dead code), aligned to server source semantics.
//! Run: cargo test --test protocol_coverage_live -- --ignored --nocapture

use app_lib::backend::{resolve_api_base, Protocol, ServerKind};
use app_lib::fever::FeverClient;
use app_lib::greader::{self, GReaderClient};
use std::collections::HashSet;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Flavor { Miniflux, FreshRss }

impl Flavor {
    fn kind(self) -> ServerKind {
        match self { Flavor::Miniflux => ServerKind::Miniflux, Flavor::FreshRss => ServerKind::FreshRss }
    }
    fn name(self) -> &'static str {
        match self { Flavor::Miniflux => "Miniflux", Flavor::FreshRss => "FreshRSS" }
    }
}

fn creds_for(flavor: Flavor) -> (String, String, String) {
    let (ep_var, user_var, pass_var, ep_default) = match flavor {
        Flavor::Miniflux => ("FLUXREADER_TEST_ENDPOINT", "FLUXREADER_TEST_USER", "FLUXREADER_TEST_PASS", "https://rss.chean.top"),
        Flavor::FreshRss => ("FLUXREADER_FRESHRSS_ENDPOINT", "FLUXREADER_FRESHRSS_USER", "FLUXREADER_FRESHRSS_PASS", "https://rss.ceaion.com"),
    };
    let endpoint = std::env::var(ep_var).unwrap_or_else(|_| ep_default.to_string());
    let username = std::env::var(user_var).unwrap_or_else(|_| "test".to_string());
    let password = std::env::var(pass_var).unwrap_or_else(|_| "testtest".to_string());
    (endpoint, username, password)
}

async fn greader_client(flavor: Flavor) -> GReaderClient {
    let (root, user, pass) = creds_for(flavor);
    let base = resolve_api_base(flavor.kind(), Protocol::GReader, &root);
    let http = reqwest::Client::new();
    GReaderClient::login(&base, &user, &pass, http).await.unwrap()
}

fn fever_client(flavor: Flavor) -> FeverClient {
    let (root, user, pass) = creds_for(flavor);
    let base = resolve_api_base(flavor.kind(), Protocol::Fever, &root);
    let http = reqwest::Client::new();
    FeverClient::new(&base, &user, &pass, http)
}

async fn pick_any_greader_id(client: &GReaderClient) -> i64 {
    let ids = client.item_ids("user/-/state/com.google/reading-list", None, None, Some(5), None).await.unwrap();
    ids.item_refs[0]
        .id
        .parse::<i64>()
        .expect("reading-list should have entries")
}

// ---- GReader ----

async fn greader_login_subs_tags(flavor: Flavor) {
    let client = greader_client(flavor).await;
    let subs = client.subscriptions().await.unwrap();
    assert!(!subs.is_empty(), "[{}] subscriptions non-empty", flavor.name());
    for s in &subs {
        assert!(greader::parse_feed_numeric_id(&s.id).is_some(), "[{}] feed id should be feed/<num>: {}", flavor.name(), s.id);
    }
    let tags = client.tags().await.unwrap();
    assert!(!tags.is_empty(), "[{}] tag/list non-empty", flavor.name());
    assert!(tags.iter().any(|t| t.id.ends_with("/com.google/starred")), "[{}] tag/list should contain starred", flavor.name());
    println!("  [{}] subs={} tags={}", flavor.name(), subs.len(), tags.len());
}

async fn greader_item_ids_ot_and_continuation(flavor: Flavor) {
    let client = greader_client(flavor).await;
    let future = chrono::Utc::now().timestamp() + 3600;
    let r = client.item_ids("user/-/state/com.google/reading-list", Some(future), None, Some(100), None).await.unwrap();
    assert!(r.item_refs.is_empty(), "[{}] ot=future should return empty", flavor.name());

    let mut seen: Vec<i64> = Vec::new();
    let mut cont: Option<String> = None;
    let mut pages = 0;
    loop {
        let page = client.item_ids("user/-/state/com.google/reading-list", None, None, Some(1), cont.as_deref()).await.unwrap();
        for it in &page.item_refs {
            if let Ok(id) = it.id.parse::<i64>() { seen.push(id); }
        }
        pages += 1;
        match &page.continuation {
            Some(c) if !c.is_empty() && pages < 10 => cont = Some(c.clone()),
            _ => break,
        }
    }
    assert!(!seen.is_empty(), "[{}] continuation should yield entries", flavor.name());
    let uniq: HashSet<i64> = seen.iter().copied().collect();
    assert_eq!(seen.len(), uniq.len(), "[{}] continuation must not duplicate", flavor.name());
    println!("  [{}] continuation {} pages {} entries", flavor.name(), pages, seen.len());
}

async fn greader_item_contents(flavor: Flavor) {
    let client = greader_client(flavor).await;
    let ids = client.item_ids("user/-/state/com.google/reading-list", None, None, Some(3), None).await.unwrap();
    let numeric: Vec<i64> = ids.item_refs.iter().filter_map(|r| r.id.parse().ok()).collect();
    assert!(!numeric.is_empty(), "[{}] should have entries", flavor.name());
    let contents = client.item_contents(&numeric).await.unwrap();
    assert_eq!(contents.len(), numeric.len(), "[{}] contents count == ids count", flavor.name());
    for c in &contents {
        assert!(greader::parse_item_id(&c.id).is_some(), "[{}] long id parseable: {}", flavor.name(), c.id);
        assert!(!c.title.is_empty(), "[{}] entry has title", flavor.name());
    }
    println!("  [{}] fetched {} contents", flavor.name(), contents.len());
}

async fn greader_edit_tag_full_roundtrip(flavor: Flavor) {
    let client = greader_client(flavor).await;
    let id = pick_any_greader_id(&client).await;
    let before = client.item_contents(&[id]).await.unwrap();
    let before_read = greader::has_tag(&before[0].categories, "/com.google/read");
    let before_starred = greader::has_tag(&before[0].categories, "/com.google/starred");

    // 先标未读（无论初始态）→ 验证 read 被移除
    client.mark_unread(&[id]).await.unwrap();
    let after = client.item_contents(&[id]).await.unwrap();
    assert!(!greader::has_tag(&after[0].categories, "/com.google/read"), "[{}] mark_unread should remove read", flavor.name());

    client.mark_read(&[id]).await.unwrap();
    let after = client.item_contents(&[id]).await.unwrap();
    assert!(greader::has_tag(&after[0].categories, "/com.google/read"), "[{}] mark_read should add read", flavor.name());

    client.mark_starred(&[id]).await.unwrap();
    let after = client.item_contents(&[id]).await.unwrap();
    assert!(greader::has_tag(&after[0].categories, "/com.google/starred"), "[{}] mark_starred should add starred", flavor.name());

    client.mark_unstarred(&[id]).await.unwrap();
    let after = client.item_contents(&[id]).await.unwrap();
    assert!(!greader::has_tag(&after[0].categories, "/com.google/starred"), "[{}] mark_unstarred should remove starred", flavor.name());

    // 恢复初始状态
    if before_read { client.mark_read(&[id]).await.unwrap(); } else { client.mark_unread(&[id]).await.unwrap(); }
    if before_starred { client.mark_starred(&[id]).await.unwrap(); }
    println!("  [{}] edit-tag read/unread/star/unstar roundtrip OK (id {})", flavor.name(), id);
}

async fn greader_quickadd(flavor: Flavor) {
    let client = greader_client(flavor).await;
    let subs = client.subscriptions().await.unwrap();
    let subscribed_urls: HashSet<String> = subs.iter().map(|s| s.url.clone()).collect();

    // 用「已订阅」的 URL 验证 quickadd 重复订阅是可诊断的、且不新增订阅。
    // 源码对齐：
    // - Miniflux quickAdd 会重新 fetch feed 再 subscribe，已订阅 → duplicate 500（mf_handler.go:372-379）；
    // - FreshRSS quickadd 对已存在 feed 抛异常 → 返回 {numResults:0, error}（greader.php:502-514）。
    let first_url = subs[0].url.clone();
    match client.quick_add(&first_url).await {
        Ok(r) => {
            // FreshRSS：重复 → numResults=0（stream_id 为 None），不报错但也不新增
            assert!(
                r.stream_id.is_none(),
                "[{}] quickadd(dup) 应返回 numResults=0（stream_id=None）：{:?}",
                flavor.name(),
                r.stream_id
            );
        }
        Err(e) => {
            // Miniflux：duplicate → 500，是可诊断的「服务端已存在」信号，非协议错误
            assert!(
                e.code == "network",
                "[{}] quickadd(dup) 应是可诊断错误：{}",
                flavor.name(),
                e
            );
        }
    }
    // 断言没有把「已订阅」重新当成新订阅（订阅集合不变）
    let subs_after = client.subscriptions().await.unwrap();
    let after_urls: HashSet<String> = subs_after.iter().map(|s| s.url.clone()).collect();
    assert_eq!(subscribed_urls, after_urls, "[{}] quickadd 已存在源不得新增订阅", flavor.name());
    println!("  [{}] quickadd duplicate handled OK ({})", flavor.name(), first_url);
}

// ---- Fever ----

async fn fever_subs_groups_sets(flavor: Flavor) {
    let client = fever_client(flavor);
    client.verify().await.unwrap();
    let subs = client.subscriptions().await.unwrap();
    assert!(!subs.is_empty(), "[{}] Fever subs non-empty", flavor.name());
    for s in &subs {
        assert!(greader::parse_feed_numeric_id(&s.id).is_some(), "[{}] Fever feed id feed/<num>", flavor.name());
    }
    let tags = client.tags().await.unwrap();
    assert!(!tags.is_empty(), "[{}] Fever groups non-empty", flavor.name());
    let unread = client.unread_item_ids().await.unwrap();
    let saved = client.saved_item_ids().await.unwrap();
    assert!(unread.iter().all(|id| *id > 0), "[{}] unread ids positive", flavor.name());
    println!("  [{}] subs={} groups={} unread={} saved={}", flavor.name(), subs.len(), tags.len(), unread.len(), saved.len());
}

async fn fever_items_all_modes(flavor: Flavor) {
    let client = fever_client(flavor);
    let recent = client.items_recent().await.unwrap();
    assert!(!recent.is_empty(), "[{}] items no-arg non-empty", flavor.name());
    let since0 = client.items_since(0).await.unwrap();
    assert!(!since0.is_empty(), "[{}] items since_id=0 non-empty", flavor.name());
    let target_ids: Vec<i64> = recent.iter().filter_map(|c| greader::parse_item_id(&c.id)).take(2).collect();
    let with = client.items_with_ids(&target_ids).await.unwrap();
    assert_eq!(with.len(), target_ids.len(), "[{}] with_ids exact count", flavor.name());
    let got: HashSet<i64> = with.iter().filter_map(|c| greader::parse_item_id(&c.id)).collect();
    for id in &target_ids {
        assert!(got.contains(id), "[{}] with_ids contains {}", flavor.name(), id);
    }
    println!("  [{}] items no-arg={} since_id={} with_ids={}", flavor.name(), recent.len(), since0.len(), with.len());
}

async fn fever_mark_full_roundtrip(flavor: Flavor) {
    let client = fever_client(flavor);
    let unread = client.unread_item_ids().await.unwrap();
    let id = unread[0];

    client.mark_read(&[id]).await.unwrap();
    let u = client.unread_item_ids().await.unwrap();
    assert!(!u.contains(&id), "[{}] mark_read removes from unread", flavor.name());

    client.mark_starred(&[id]).await.unwrap();
    let s = client.saved_item_ids().await.unwrap();
    assert!(s.contains(&id), "[{}] mark_starred adds to saved", flavor.name());

    client.mark_unstarred(&[id]).await.unwrap();
    let s2 = client.saved_item_ids().await.unwrap();
    assert!(!s2.contains(&id), "[{}] mark_unstarred removes from saved", flavor.name());

    client.mark_unread(&[id]).await.unwrap();
    let u2 = client.unread_item_ids().await.unwrap();
    assert!(u2.contains(&id), "[{}] mark_unread adds back to unread", flavor.name());

    println!("  [{}] Fever mark read/unread/saved/unsaved roundtrip OK (id {})", flavor.name(), id);
}

#[tokio::test]
#[ignore = "needs real Miniflux + network"]
async fn miniflux_greader_full_coverage() {
    greader_login_subs_tags(Flavor::Miniflux).await;
    greader_item_ids_ot_and_continuation(Flavor::Miniflux).await;
    greader_item_contents(Flavor::Miniflux).await;
    greader_edit_tag_full_roundtrip(Flavor::Miniflux).await;
    greader_quickadd(Flavor::Miniflux).await;
}

#[tokio::test]
#[ignore = "needs real Miniflux + network"]
async fn miniflux_fever_full_coverage() {
    fever_subs_groups_sets(Flavor::Miniflux).await;
    fever_items_all_modes(Flavor::Miniflux).await;
    fever_mark_full_roundtrip(Flavor::Miniflux).await;
}

#[tokio::test]
#[ignore = "needs real FreshRSS + network"]
async fn freshrss_greader_full_coverage() {
    greader_login_subs_tags(Flavor::FreshRss).await;
    greader_item_ids_ot_and_continuation(Flavor::FreshRss).await;
    greader_item_contents(Flavor::FreshRss).await;
    greader_edit_tag_full_roundtrip(Flavor::FreshRss).await;
    greader_quickadd(Flavor::FreshRss).await;
}

#[tokio::test]
#[ignore = "needs real FreshRSS + network"]
async fn freshrss_fever_full_coverage() {
    fever_subs_groups_sets(Flavor::FreshRss).await;
    fever_items_all_modes(Flavor::FreshRss).await;
    fever_mark_full_roundtrip(Flavor::FreshRss).await;
}