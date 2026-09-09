//! 临时 live 验证：FreshRSS（https://rss.ceaion.com/，test/testtest）
//! 验证阶段 0 的协议修复（G1/G2/G3/G4 + ServerKind::FreshRss）。
//! 运行：cargo test --test freshrss_live_check -- --ignored --nocapture

use app_lib::backend::{resolve_api_base, Protocol, ServerKind};
use app_lib::fever::FeverClient;
use app_lib::greader::GReaderClient;
use app_lib::sync::test_connection;

const ROOT: &str = "https://rss.ceaion.com";
const USER: &str = "test";
const PASS: &str = "testtest";

#[tokio::test]
#[ignore = "需真实 FreshRSS + 网络"]
async fn freshrss_greader_connect_and_list() {
    let http = reqwest::Client::new();

    let base = resolve_api_base(ServerKind::FreshRss, Protocol::GReader, ROOT);
    println!("api_base = {base}");

    // test_connection（走 ClientLogin + subscriptions）
    let (msg, user) = test_connection(ServerKind::FreshRss, "greader", ROOT, USER, PASS, &http)
        .await
        .unwrap_or_else(|e| panic!("test_connection 失败: {e}"));
    println!("test_connection: {msg} (user={user})");

    // 直接 ClientLogin + 拉订阅
    let client = GReaderClient::login(&base, USER, PASS, http).await.unwrap();
    let subs = client.subscriptions().await.unwrap();
    println!("订阅数 = {}", subs.len());
    for s in subs.iter().take(5) {
        println!("  feed: id={} title={} url={}", s.id, s.title, s.url);
    }
    assert!(!subs.is_empty(), "FreshRSS 应有订阅");

    // 拉条目 id（reading-list，验证 Authorization 头 + continuation 字符串化）
    let ids = client
        .item_ids("user/-/state/com.google/reading-list", None, None, Some(10), None)
        .await
        .unwrap();
    println!("条目 id 数（首 10）= {}", ids.item_refs.len());
}

/// 完整同步（feeds + states）against FreshRSS：验证阶段 0 全链路。
#[tokio::test]
#[ignore = "需真实 FreshRSS + 网络"]
async fn freshrss_full_sync_end_to_end() {
    let http = reqwest::Client::new();

    let tmp = std::env::temp_dir().join("fluxreader_freshrss_sync.db");
    let _ = std::fs::remove_file(&tmp);
    let conn = app_lib::db::open(&tmp).unwrap();
    let db = std::sync::Arc::new(tokio::sync::Mutex::new(conn));
    {
        let conn = db.lock().await;
        app_lib::db::set_setting(&conn, "sync_server_kind", "freshrss").unwrap();
        app_lib::db::set_setting(&conn, "sync_protocol", "greader").unwrap();
        app_lib::db::set_setting(&conn, "greader_endpoint", ROOT).unwrap();
        app_lib::db::set_setting(&conn, "greader_username", USER).unwrap();
        app_lib::db::set_setting(&conn, "greader_password", PASS).unwrap();
        app_lib::db::set_setting(&conn, "sync_last_sync", "0").unwrap();
        app_lib::db::set_setting(&conn, "sync_last_entry_id", "0").unwrap();
    }

    let report = app_lib::sync::sync_now(&db, &http).await.unwrap();
    println!("sync_now: pulled_feeds={} pulled_entries={} errors={:?}",
        report.pulled_feeds, report.pulled_entries, report.errors);
    assert!(!report.aborted, "不应中止");
    assert!(report.pulled_feeds > 0, "应拉取到订阅");

    let conn = db.lock().await;
    let feed_count: i64 = conn.query_row("SELECT COUNT(*) FROM feeds WHERE origin='remote'", [], |r| r.get(0)).unwrap();
    let article_count: i64 = conn.query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0)).unwrap();
    println!("落库：remote feeds={feed_count}, articles={article_count}");
    assert!(feed_count > 0);

    let _ = std::fs::remove_file(&tmp);
}

/// GReader 状态写入（edit-tag）往返：验证 G3（POST 带 Authorization 头）+ G4
/// （T=action token）修复。桌面标读 → FreshRSS 可见；再标未读恢复。
#[tokio::test]
#[ignore = "需真实 FreshRSS + 网络"]
async fn freshrss_greader_mark_read_roundtrip() {
    let http = reqwest::Client::new();
    let base = resolve_api_base(ServerKind::FreshRss, Protocol::GReader, ROOT);
    let client = GReaderClient::login(&base, USER, PASS, http).await.unwrap();

    // 拉一条未读条目
    let ids = client
        .item_ids("user/-/state/com.google/reading-list", None, None, Some(5), None)
        .await
        .unwrap();
    let id = ids.item_refs[0].id.parse::<i64>().unwrap();
    println!("target entry id = {id}");

    // 标已读
    client.mark_read(&[id]).await.unwrap();
    let read_ids = client
        .item_ids("user/-/state/com.google/read", Some(0), None, Some(1000), None)
        .await
        .unwrap();
    let read_set: std::collections::HashSet<i64> = read_ids
        .item_refs
        .iter()
        .filter_map(|r| r.id.parse::<i64>().ok())
        .collect();
    assert!(read_set.contains(&id), "标读后应出现在 read 集合");

    // 恢复未读
    client.mark_unread(&[id]).await.unwrap();
    let read_ids2 = client
        .item_ids("user/-/state/com.google/read", Some(0), None, Some(1000), None)
        .await
        .unwrap();
    let read_set2: std::collections::HashSet<i64> = read_ids2
        .item_refs
        .iter()
        .filter_map(|r| r.id.parse::<i64>().ok())
        .collect();
    assert!(!read_set2.contains(&id), "标未读后应从 read 集合移除");
    println!("GReader mark-read 往返验证通过（id {id}）");
}

/// FreshRSS Fever 端点连通 + 订阅/条目拉取（阶段 0 live 手测 Fever 行）。
#[tokio::test]
#[ignore = "需真实 FreshRSS + 网络"]
async fn freshrss_fever_connect_and_list() {
    let http = reqwest::Client::new();
    let base = resolve_api_base(ServerKind::FreshRss, Protocol::Fever, ROOT);
    println!("fever api_base = {base}");

    // test_connection 走 Fever
    let (msg, _user) = test_connection(ServerKind::FreshRss, "fever", ROOT, USER, PASS, &http)
        .await
        .unwrap_or_else(|e| panic!("Fever test_connection 失败: {e}"));
    println!("fever test_connection: {msg}");

    let client = FeverClient::new(&base, USER, PASS, http);
    let subs = client.subscriptions().await.unwrap();
    println!("Fever 订阅数 = {}", subs.len());
    for s in subs.iter() {
        println!("  feed: id={} title={} url={}", s.id, s.title, s.url);
    }

    let unread = client.unread_item_ids().await.unwrap();
    let saved = client.saved_item_ids().await.unwrap();
    println!("Fever 未读 {} 条，收藏 {} 条", unread.len(), saved.len());
}

/// 通过 Google Reader quickadd 向 FreshRSS 测试账号添加国内可访问的订阅源，
/// 制造测试数据供后续同步验证（Fever 协议无订阅端点，故走 greader quickadd）。
#[tokio::test]
#[ignore = "需真实 FreshRSS + 网络"]
async fn freshrss_add_test_feeds() {
    let http = reqwest::Client::new();
    let base = resolve_api_base(ServerKind::FreshRss, Protocol::GReader, ROOT);
    let client = GReaderClient::login(&base, USER, PASS, http).await.unwrap();

    // 国内可直连的源（避开境外无法访问的源，防止误判为协议问题）
    let feeds = [
        ("https://sspai.com/feed", "少数派"),
        ("http://www.ruanyifeng.com/blog/atom.xml", "阮一峰的网络日志"),
        ("https://www.solidot.org/index.rss", "Solidot"),
    ];
    for (url, name) in feeds {
        match client.quick_add(url).await {
            Ok(r) => println!("quickadd {name}: stream_id={:?}", r.stream_id),
            Err(e) => println!("quickadd {name} 失败: {e}"),
        }
    }

    // 回读订阅列表确认
    let subs = client.subscriptions().await.unwrap();
    println!("添加后订阅数 = {}", subs.len());
    for s in subs.iter() {
        println!("  feed: id={} title={} url={}", s.id, s.title, s.url);
    }
}
