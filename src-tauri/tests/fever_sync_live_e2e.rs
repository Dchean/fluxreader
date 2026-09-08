//! sync 引擎走 Fever 协议连真实 Miniflux 的端到端同步验证（`--ignored`）。
//!
//! 测试账号：test / testtest（Fever 集成凭据），后端 https://rss.chean.top/。
//! 用临时空库做一次 full 同步（feeds + states），验证订阅/条目能通过 Fever 协议
//! 完整拉下来并落库——这是「Fever 协议调整完毕」的端到端证据（不只客户端 API）。
//! 空库无推送队列，不会向服务端写入状态。

use app_lib::{db, sync};

fn test_creds() -> (String, String, String) {
    let endpoint = std::env::var("FLUXREADER_TEST_ENDPOINT")
        .unwrap_or_else(|_| "https://rss.chean.top".to_string());
    let username = std::env::var("FLUXREADER_TEST_USER").unwrap_or_else(|_| "test".to_string());
    let password = std::env::var("FLUXREADER_TEST_PASS").unwrap_or_else(|_| "testtest".to_string());
    (endpoint, username, password)
}

#[tokio::test]
#[ignore = "需真实 Miniflux 测试账号 + 网络"]
async fn fever_sync_end_to_end_on_live_backend() {
    let (endpoint, username, password) = test_creds();

    let tmp = std::env::temp_dir().join("fluxreader_fever_sync_live.db");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).expect("open db");
    let db = std::sync::Arc::new(tokio::sync::Mutex::new(conn));
    {
        let conn = db.lock().await;
        db::set_setting(&conn, "sync_protocol", "fever").unwrap();
        db::set_setting(&conn, "greader_endpoint", &endpoint).unwrap();
        db::set_setting(&conn, "greader_username", &username).unwrap();
        db::set_setting(&conn, "greader_password", &password).unwrap();
        db::set_setting(&conn, "sync_last_sync", "0").unwrap();
        db::set_setting(&conn, "sync_last_entry_id", "0").unwrap();
    }

    let http = reqwest::Client::new();
    let report = sync::sync_now(&db, &http).await.expect("Fever full sync");

    assert!(
        report.errors.is_empty(),
        "Fever 同步不应有错误：{:?}",
        report.errors
    );
    assert!(report.pulled_feeds > 0, "应拉取到订阅源");
    assert!(report.pulled_entries > 0, "应拉取到条目");

    let conn = db.lock().await;
    let feed_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM feeds WHERE origin = 'remote'", [], |r| r.get(0))
        .unwrap();
    let article_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
        .unwrap();
    assert!(feed_count > 0, "远端订阅应落库");
    assert!(article_count > 0, "远端条目应落库");

    // Fever 增量游标应已推进到最大条目 id
    let last_entry_id = db::last_sync_entry_id(&conn).unwrap();
    assert!(last_entry_id > 0, "Fever since_id 游标应已推进");

    println!(
        "Fever 端到端同步通过：feeds={feed_count}, articles={article_count}, since_id={last_entry_id}"
    );

    let _ = std::fs::remove_file(&tmp);
}