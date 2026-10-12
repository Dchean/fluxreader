//! OPT-008B 第一阶段迁移（原 TASK-064 N3）：手动「刷新全部」时 smartDedup 的
//! 语义从「入库丢身份」改为「显示层聚合」——
//! 两个本地源 serve 同一份 RSS（同一 article link）：**无论开关都各自入库**
//! （条目是订阅源/远端 ID 的独立实体，`refresh_all` 报出的新增数 = 实际入库行数）；
//! 开关只改变 `list_articles` 的显示集合（开：全部视图折叠为 1；关：逐行可见），
//! 且单源视图两个开关下都能看到自己的那条（不隐藏当前源）。
//! 旧断言「开=1 篇新增」是旧模型的产物（跨源副本被 dedup 丢弃），已按新语义迁移。
//! 修前 refresh_all 把 dedup 写死 false → 现在无关（身份不再丢），保留的回归价值：
//! 刷新必须真的走完两个源、显示集合必须随开关变化。

use app_lib::db;
use app_lib::scheduler;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use tokio::sync::Mutex;

mod common;

const RSS: &str = r#"<?xml version="1.0"?><rss version="2.0"><channel><title>Dedup Feed</title>
<item><title>同一篇</title><link>https://example.com/post-1</link><guid>post-1</guid></item>
</channel></rss>"#;

/// 最小 HTTP feed mock：任何 GET 都回同一份 RSS（两个源用同 server 的不同路径区分）
fn start_feed_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            /* TASK-088：单个连接出错（对端提前断开等）不得 panic 掉**整个服务器线程**——
            此前 `stream.unwrap()` 一旦 panic，该线程即退出，本测试内后续所有请求都会
            连不上，症状会伪装成「抓取失败」而非「测试基建故障」，极难定位。
            改为跳过这一个连接、继续 accept 下一个。
            TASK-096 补充：出错留痕——eprintln 进 stderr（cargo test 捕获，
            失败时可见），不再静默吞掉连接错误。 */
            let Ok(mut stream) = stream else {
                eprintln!("[refresh_dedup mock] accept 失败，跳过该连接，继续服务");
                continue;
            };
            let mut buf = [0u8; 4096];
            let mut req = String::new();
            loop {
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break;
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
            // 写回失败同样只影响这一个连接：忽略并在下一轮继续服务
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    port
}

fn setup_db(port: u16) -> (Arc<Mutex<rusqlite::Connection>>, std::path::PathBuf) {
    let tmp = common::unique_db_path("dedup_refresh");
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

fn all_scope_len(conn: &rusqlite::Connection) -> usize {
    let q = db::ArticleQuery {
        feed_id: None,
        folder_id: None,
        only_unread: false,
        only_starred: false,
        only_today: false,
        newest_first: true,
        limit: 500,
        offset: 0,
        with_content: false,
        layout: None,
        last_published: None,
        last_id: None,
    };
    db::list_articles(conn, &q).unwrap().len()
}

#[tokio::test]
async fn manual_refresh_all_keeps_identity_and_display_dedup_collapses() {
    let port = start_feed_server();
    let (db, tmp) = setup_db(port);
    let client = app_lib::ingestion::build_client(30);

    set_smart_dedup(&*db.lock().await, true);
    let (new_articles, failed) = scheduler::refresh_all(&db, &client).await;
    assert_eq!(failed, 0, "两个本地源都应抓取成功");
    assert_eq!(
        new_articles, 2,
        "smartDedup=on：两源各自入库（身份保留；旧断言「只留 1 篇」已迁移）"
    );
    assert_eq!(article_count(&db).await, 2, "条目身份不丢");
    {
        let conn = db.lock().await;
        assert_eq!(
            all_scope_len(&conn),
            1,
            "显示层聚合：全部视图折叠为 1（开关的可见效果）"
        );
    }

    // 关闭开关：全部逐行可见（同一份数据，显示集合回退）
    set_smart_dedup(&*db.lock().await, false);
    {
        let conn = db.lock().await;
        assert_eq!(all_scope_len(&conn), 2, "关闭后跨源两行都可见");
    }

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
    assert_eq!(new_articles, 2, "smartDedup=off：两源各 1 篇");
    assert_eq!(article_count(&db).await, 2);

    let _ = std::fs::remove_file(&tmp);
}
