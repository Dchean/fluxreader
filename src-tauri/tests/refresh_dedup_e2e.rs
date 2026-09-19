//! TASK-064 N3：手动「刷新全部」必须尊重 smartDedup。
//! 两个本地源（进程内 TcpListener HTTP server，复用 ai_e2e 模式）serve 同一份
//! RSS（同一 article link、不同 feed_id/guid 组合不被 UNIQUE(feed_id,guid) 拦）：
//! 开智能去重后 refresh_all 只应留 1 篇；关掉则 2 篇（开关语义双向锚定）。
//! 修前 refresh_all 把 dedup 写死 false → 两组断言中的 on 分支失败。
//! 不依赖外网与 Tauri（refresh_all 只吃 Arc<Mutex<Connection>> + Client）。

use app_lib::db;
use app_lib::scheduler;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use tokio::sync::Mutex;

const RSS: &str = r#"<?xml version="1.0"?><rss version="2.0"><channel><title>Dedup Feed</title>
<item><title>同一篇</title><link>https://example.com/post-1</link><guid>post-1</guid></item>
</channel></rss>"#;

/// 最小 HTTP feed mock：任何 GET 都回同一份 RSS（两个源用同 server 的不同路径区分）
fn start_feed_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut buf = [0u8; 4096];
            let mut req = String::new();
            loop {
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    return;
                }
                req.push_str(&String::from_utf8_lossy(&buf[..n]));
                if req.contains("\r\n\r\n") || req.len() > 8192 {
                    break;
                }
            }
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/rss+xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                RSS.len(),
                RSS
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    port
}

fn setup_db(port: u16) -> (Arc<Mutex<rusqlite::Connection>>, std::path::PathBuf) {
    let tmp = std::env::temp_dir().join(format!(
        "fluxreader_dedup_refresh_{}_{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();
    let f1 = db::create_folder(&conn, "A", "article").unwrap();
    let f2 = db::create_folder(&conn, "B", "article").unwrap();
    db::insert_feed(
        &conn,
        &format!("http://127.0.0.1:{port}/feedA"),
        None,
        "A",
        None,
        f1,
        "inherit",
        true,
        false,
    )
    .unwrap();
    db::insert_feed(
        &conn,
        &format!("http://127.0.0.1:{port}/feedB"),
        None,
        "B",
        None,
        f2,
        "inherit",
        true,
        false,
    )
    .unwrap();
    (Arc::new(Mutex::new(conn)), tmp)
}

fn set_smart_dedup(conn: &rusqlite::Connection, on: bool) {
    db::set_setting(conn, "app_settings", &format!(r#"{{"smartDedup":{on}}}"#)).unwrap();
}

async fn article_count(db: &Arc<Mutex<rusqlite::Connection>>) -> i64 {
    let conn = db.lock().await;
    conn.query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
        .unwrap()
}

#[tokio::test]
async fn manual_refresh_all_respects_smart_dedup_when_on() {
    let port = start_feed_server();
    let (db, tmp) = setup_db(port);
    let client = app_lib::ingestion::build_client(30);

    set_smart_dedup(&*db.lock().await, true);
    let (new_articles, failed) = scheduler::refresh_all(&db, &client).await;
    assert_eq!(failed, 0, "两个本地源都应抓取成功");
    assert_eq!(new_articles, 1, "smartDedup=on：跨源同文只应有 1 篇新增（N3 修前为 2）");
    assert_eq!(article_count(&db).await, 1);

    let _ = std::fs::remove_file(&tmp);
}

#[tokio::test]
async fn manual_refresh_all_keeps_both_when_dedup_off() {
    let port = start_feed_server();
    let (db, tmp) = setup_db(port);
    let client = app_lib::ingestion::build_client(30);

    set_smart_dedup(&*db.lock().await, false);
    let (new_articles, failed) = scheduler::refresh_all(&db, &client).await;
    assert_eq!(failed, 0);
    assert_eq!(new_articles, 2, "smartDedup=off：保持既有行为，两源各 1 篇");
    assert_eq!(article_count(&db).await, 2);

    let _ = std::fs::remove_file(&tmp);
}
