//! 配置同步集成测试：本地 HTTP mock（WebDAV 语义）+ payload 构建/应用逻辑。
//! 覆盖：payload 构建含全部配置域、上传-下载往返、应用 upsert 语义
//! （新源导入/已存在跳过/分类合并/设置覆盖）。运行：cargo test --test config_sync_e2e

use app_lib::config_sync::{apply_payload, build_payload, SyncPayload};
use app_lib::db;
use rusqlite::Connection;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

/// 最小 WebDAV mock：PUT 存内容，GET 回内容，非 2xx 报错。
fn start_webdav_mock() -> (u16, std::sync::Arc<std::sync::Mutex<Option<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let store: std::sync::Arc<std::sync::Mutex<Option<String>>> = Default::default();
    let store2 = store.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut buf = [0u8; 8192];
            let mut req = String::new();
            // 读到 header 结束
            loop {
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 { return; }
                req.push_str(&String::from_utf8_lossy(&buf[..n]));
                if req.contains("\r\n\r\n") || req.len() > 1_048_576 { break; }
            }
            let head = req.clone();
            let is_put = head.starts_with("PUT");
            let is_get = head.starts_with("GET");
            // PUT body（header 之后的部分可能已在 buf 里）
            if is_put {
                let body = req.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
                // body 可能未读完：按 Content-Length 补读
                let cl: usize = head
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("content-length"))
                    .and_then(|l| l.split(':').nth(1))
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0);
                let mut body = body;
                while body.len() < cl {
                    let n = stream.read(&mut buf).unwrap_or(0);
                    if n == 0 { break; }
                    body.push_str(&String::from_utf8_lossy(&buf[..n]));
                }
                *store2.lock().unwrap() = Some(body);
                let resp = "HTTP/1.1 201 Created\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                stream.write_all(resp.as_bytes()).unwrap();
            } else if is_get {
                let guard = store2.lock().unwrap();
                match guard.as_ref() {
                    Some(body) => {
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(), body
                        );
                        stream.write_all(resp.as_bytes()).unwrap();
                    }
                    None => {
                        let resp = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        stream.write_all(resp.as_bytes()).unwrap();
                    }
                }
            } else {
                let resp = "HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                stream.write_all(resp.as_bytes()).unwrap();
            }
        }
    });
    (port, store)
}

fn seed_db(conn: &Connection) {
    let f1 = db::create_folder(conn, "技术", "article").unwrap();
    let f2 = db::create_folder(conn, "播客", "podcast").unwrap();
    db::set_folder_ai_flags(conn, f1, true, false).unwrap();
    db::insert_feed(conn, "https://a.com/rss", Some("https://a.com"), "源A", None, f1, "inherit", true, false).unwrap();
    db::insert_feed(conn, "https://b.com/feed", None, "源B", None, f2, "social", false, true).unwrap();
    db::set_setting(conn, "app_settings", r#"{"themeMode":"dark","fontSize":17}"#).unwrap();
    db::set_setting(conn, "ai_config", r#"{"preset":"glm"}"#).unwrap();
}

#[test]
fn payload_contains_all_config_domains() {
    let tmp = std::env::temp_dir().join("fluxreader_cfgsync_test1.db");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();
    seed_db(&conn);

    let p = build_payload(&conn).unwrap();
    assert_eq!(p.schema, 1);
    assert_eq!(p.folders.len(), 2);
    assert_eq!(p.feeds.len(), 2);
    // 分类 AI 标志带出
    let tech = p.folders.iter().find(|f| f.name == "技术").unwrap();
    assert!(tech.auto_summary && !tech.auto_translate);
    // 源归属正确映射
    let feed_b = p.feeds.iter().find(|f| f.url == "https://b.com/feed").unwrap();
    assert_eq!(feed_b.folder, "播客");
    assert_eq!(feed_b.layout, "social");
    // 设置原文带出
    assert!(p.app_settings.as_deref().unwrap().contains("fontSize"));
    assert!(p.ai_config.as_deref().unwrap().contains("glm"));
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn apply_upserts_feeds_and_overrides_settings() {
    let tmp = std::env::temp_dir().join("fluxreader_cfgsync_test2.db");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();
    // 本地已有：一个同名分类 + 一个同 URL 源
    let f1 = db::create_folder(&conn, "技术", "gallery").unwrap();
    db::insert_feed(&conn, "https://a.com/rss", None, "本地已有源A", None, f1, "inherit", false, false).unwrap();

    let payload = SyncPayload {
        schema: 1,
        uploaded_at: "2026-09-01T00:00:00Z".into(),
        folders: vec![
            app_lib::config_sync::FolderSpec {
                name: "技术".into(), layout: "article".into(), auto_summary: true, auto_translate: false,
            },
            app_lib::config_sync::FolderSpec {
                name: "新分类".into(), layout: "podcast".into(), auto_summary: false, auto_translate: true,
            },
        ],
        feeds: vec![
            // 已存在（同 URL）→ 跳过
            app_lib::config_sync::FeedSpec {
                url: "https://a.com/rss".into(), title: "源A".into(), folder: "技术".into(),
                layout: "inherit".into(), auto_summary: true, auto_translate: false,
            },
            // 新源 → 导入到已有分类
            app_lib::config_sync::FeedSpec {
                url: "https://new.com/rss".into(), title: "新源".into(), folder: "技术".into(),
                layout: "inherit".into(), auto_summary: false, auto_translate: false,
            },
            // 新源 + 新分类名 → 分类创建后导入
            app_lib::config_sync::FeedSpec {
                url: "https://pod.com/rss".into(), title: "播客源".into(), folder: "新分类".into(),
                layout: "inherit".into(), auto_summary: false, auto_translate: false,
            },
            // 未知分类 → 落「导入」分类
            app_lib::config_sync::FeedSpec {
                url: "https://x.com/rss".into(), title: "未知归属".into(), folder: "不存在".into(),
                layout: "inherit".into(), auto_summary: false, auto_translate: false,
            },
        ],
        app_settings: Some(r#"{"themeMode":"light","fontSize":18}"#.into()),
        ai_config: Some(r#"{"preset":"deepseek"}"#.into()),
    };

    let (imported, skipped) = apply_payload(&conn, &payload).unwrap();
    assert_eq!(imported, 3);
    assert_eq!(skipped, 1);

    // 分类 upsert：同名分类被更新布局+标志，新分类被创建
    let folders = db::list_folders(&conn).unwrap();
    assert_eq!(folders.len(), 3); // 技术（已有，被更新） + 新分类 + 导入
    let tech = folders.iter().find(|f| f.name == "技术").unwrap();
    assert_eq!(tech.layout, "article");
    assert!(tech.auto_summary);
    assert!(folders.iter().any(|f| f.name == "新分类"));
    assert!(folders.iter().any(|f| f.name == "导入"));

    // 源总数：原有 1 + 导入 3 = 4
    let feeds = db::list_feeds(&conn).unwrap();
    assert_eq!(feeds.len(), 4);
    // 已存在的源未被覆盖改名
    let a = feeds.iter().find(|f| f.feed_url == "https://a.com/rss").unwrap();
    assert_eq!(a.title, "本地已有源A");

    // 设置被覆盖
    let s = db::get_setting(&conn, "app_settings").unwrap().unwrap();
    assert!(s.contains("\"fontSize\":18"));
    let ai = db::get_setting(&conn, "ai_config").unwrap().unwrap();
    assert!(ai.contains("deepseek"));
    let _ = std::fs::remove_file(&tmp);
}

#[tokio::test]
async fn webdav_roundtrip_via_mock() {
    let (port, store) = start_webdav_mock();
    let http = app_lib::ingestion::build_client(30);

    // 模拟凭据与请求路径（WebDAV put/get 直接调用内部函数）
    let cred = app_lib::config_sync::SyncCredentials {
        backend: "webdav".into(),
        token: "pass".into(),
        server: format!("http://127.0.0.1:{port}"),
        username: "user".into(),
        gist_id: None,
    };

    // 上传（WebDAV PUT）
    app_lib::config_sync::webdav_put_for_test(&http, &cred, r#"{"schema":1,"feeds":[]}"#)
        .await
        .unwrap();
    assert_eq!(store.lock().unwrap().as_deref(), Some(r#"{"schema":1,"feeds":[]}"#));

    // 下载（WebDAV GET）→ 内容一致
    let got = app_lib::config_sync::webdav_get_for_test(&http, &cred).await.unwrap();
    assert_eq!(got, r#"{"schema":1,"feeds":[]}"#);
}

#[tokio::test]
async fn webdav_get_404_maps_to_error() {
    let (port, _store) = start_webdav_mock();
    let http = app_lib::ingestion::build_client(30);
    let cred = app_lib::config_sync::SyncCredentials {
        backend: "webdav".into(),
        token: "pass".into(),
        server: format!("http://127.0.0.1:{port}"),
        username: "user".into(),
        gist_id: None,
    };
    let r = app_lib::config_sync::webdav_get_for_test(&http, &cred).await;
    assert!(r.is_err());
}
