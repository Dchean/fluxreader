//! 三段式刷新管线（refresh_feed_staged）的并发验证：
//! 自建慢速 HTTP server（每请求 sleep 300ms），4 个源并发抓取总耗时应显著小于
//! 串行（4×300ms）——证明 HTTP 在数据库锁外真正重叠。
//! 同时验证 304 分支保留旧条件头（写回旧 etag，不断条件 GET 链）。
//!
//! 全程本地回环 + 临时 DB，无需外部 server，可直接 `cargo test --test staged_refresh_e2e`。

use app_lib::db;
use app_lib::ingestion;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// 慢速 feed server：每个请求先 sleep 再回 RSS（item 数可配，用于 304 用例的 body 变体）。
/// 返回 (base_url, 命中计数)。
async fn spawn_slow_feed_server(delay_ms: u64, hits: Arc<AtomicUsize>) -> String {
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => return,
            };
            let hits = hits.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                // 读掉请求头（不关心内容）
                let _ = tokio::time::timeout(Duration::from_secs(2), socket.readable()).await;
                let _ = socket.try_read(&mut buf);
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                hits.fetch_add(1, Ordering::SeqCst);
                let body = r#"<?xml version="1.0"?>
<rss version="2.0"><channel>
<title>Slow Feed</title><link>http://127.0.0.1/</link><description>t</description>
<item><title>Item A</title><guid>a</guid><link>http://127.0.0.1/a</link></item>
</channel></rss>"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(resp.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    format!("http://{addr}/slow.xml")
}

fn temp_db(name: &str) -> std::path::PathBuf {
    let tmp = std::env::temp_dir().join(format!(
        "fluxreader_staged_{name}_{}.db",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&tmp);
    tmp
}

/// 4 源并发：每源 HTTP 300ms。三段式下 HTTP 在锁外重叠，
/// 总耗时应 < 1200ms（串行下限 4×300=1200ms + 锁排队会更长）。
/// 用宽松断言（< 1000ms）防 CI 抖动误报。
#[tokio::test]
async fn staged_refresh_http_overlaps_under_concurrency() {
    let hits = Arc::new(AtomicUsize::new(0));
    let url = spawn_slow_feed_server(300, hits.clone()).await;

    let tmp = temp_db("concurrent");
    let conn = db::open(&tmp).expect("open db");
    let db = Arc::new(Mutex::new(conn));
    let client = ingestion::build_client(30);

    {
        let conn = db.lock().await;
        db::create_folder(&conn, "并发", "article").unwrap();
        for i in 0..4 {
            db::insert_feed(&conn, &format!("{url}#{i}"), None, &format!("S{i}"), None, 1, "inherit", false, false).unwrap();
        }
    }

    let start = Instant::now();
    let mut handles = Vec::new();
    for id in 1..=4i64 {
        let db = db.clone();
        let client = client.clone();
        handles.push(tokio::spawn(async move {
            ingestion::refresh_feed_staged(&db, &client, id, false).await
        }));
    }
    let mut new_total = 0;
    for h in handles {
        new_total += h.await.unwrap().unwrap();
    }
    let elapsed = start.elapsed();

    assert_eq!(new_total, 4, "each feed ingests 1 article");
    assert_eq!(hits.load(Ordering::SeqCst), 4, "all 4 feeds hit the server");
    assert!(
        elapsed < Duration::from_millis(1000),
        "4 concurrent 300ms fetches took {elapsed:?} — HTTP is being serialized (lock held during network IO?)"
    );

    let _ = std::fs::remove_file(&tmp);
}

/// 304 分支：先抓一次拿 etag 入库，再刷一次（服务器不回 304 也没关系——
/// 关键断言是 NotModified 分支的写回不破坏既有 etag）。
/// 这里直接构造 Fetched::NotModified 调 apply_refresh_result 验证：
/// DB 里的 etag 保持抓取后的值（写回 old_etag 而非 None）。
#[tokio::test]
async fn staged_refresh_304_keeps_conditional_headers() {
    let tmp = temp_db("etag");
    let conn = db::open(&tmp).expect("open db");
    {
        db::create_folder(&conn, "Etag", "article").unwrap();
        db::insert_feed(&conn, "http://127.0.0.1:9/x.xml", None, "E", None, 1, "inherit", false, false).unwrap();
        db::set_feed_fetch_state(&conn, 1, false, None, Some("W/\"abc\""), Some("Wed, 21 Oct 2026 07:28:00 GMT")).unwrap();
    }

    let parsed = ingestion::ParsedFeed { title: None, site_url: None, icon: None, articles: Vec::new() };
    ingestion::apply_refresh_result(
        &conn, 1, &ingestion::Fetched::NotModified, &parsed, false,
        Some("W/\"abc\""), Some("Wed, 21 Oct 2026 07:28:00 GMT"),
    )
    .unwrap();

    let (etag, last_modified, failed): (Option<String>, Option<String>, i64) = conn
        .query_row(
            "SELECT etag, last_modified, fetch_failed FROM feeds WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(etag.as_deref(), Some("W/\"abc\""), "304 must keep existing etag");
    assert!(last_modified.is_some(), "304 must keep existing last_modified");
    assert_eq!(failed, 0, "304 is a success (clears failure state)");

    let _ = std::fs::remove_file(&tmp);
}

/// 失败分支：网络错误写回 fetch_failed + fetch_error，供 UI 与 Miniflux 兜底查询。
#[tokio::test]
async fn staged_refresh_failure_marks_feed() {
    let tmp = temp_db("fail");
    let conn = db::open(&tmp).expect("open db");
    let db = Arc::new(Mutex::new(conn));
    let client = ingestion::build_client(5); // 5s 超时；连接 127.0.0.1:1 立即拒绝

    {
        let conn = db.lock().await;
        db::create_folder(&conn, "Fail", "article").unwrap();
        db::insert_feed(&conn, "http://127.0.0.1:1/dead.xml", None, "D", None, 1, "inherit", false, false).unwrap();
    }

    let result = ingestion::refresh_feed_staged(&db, &client, 1, false).await;
    assert!(result.is_err(), "dead address must error");

    let (failed, error, fail_count): (i64, Option<String>, i64) = {
        let conn = db.lock().await;
        conn.query_row(
            "SELECT fetch_failed, fetch_error, fail_count FROM feeds WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
    };
    assert_eq!(failed, 1, "dead feed marked failed");
    assert!(error.is_some(), "failure reason recorded");
    assert_eq!(fail_count, 1, "first failure increments backoff counter");

    let _ = std::fs::remove_file(&tmp);
}
