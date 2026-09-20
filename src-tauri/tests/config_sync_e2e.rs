//! 配置同步集成测试：本地 HTTP mock（WebDAV 语义）+ payload 构建/应用逻辑。
//! 覆盖：payload 构建含全部配置域、上传-下载往返、应用 upsert 语义
//! （新源导入/已存在跳过/分类合并/设置覆盖）、凭据边界（上传排除 + 导入保留）。
//! 运行：cargo test --test config_sync_e2e

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
                if n == 0 {
                    return;
                }
                req.push_str(&String::from_utf8_lossy(&buf[..n]));
                if req.contains("\r\n\r\n") || req.len() > 1_048_576 {
                    break;
                }
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
                    if n == 0 {
                        break;
                    }
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
    db::insert_feed(
        conn,
        "https://a.com/rss",
        Some("https://a.com"),
        "源A",
        None,
        f1,
        "inherit",
        true,
        false,
    )
    .unwrap();
    db::insert_feed(
        conn,
        "https://b.com/feed",
        Some("https://b.com"),
        "源B",
        Some("https://b.com/favicon.ico"),
        f2,
        "social",
        false,
        true,
    )
    .unwrap();
    db::set_setting(
        conn,
        "app_settings",
        r#"{"themeMode":"dark","fontSize":17,"autoStart":true,"closePromptShown":true}"#,
    )
    .unwrap();
    db::set_setting(
        conn,
        "ai_config",
        r#"{"preset":"glm","apiKey":"secret123"}"#,
    )
    .unwrap();
    db::set_setting(conn, "sync_protocol", "greader").unwrap();
    db::set_setting(conn, "greader_endpoint", "https://example.com").unwrap();
    db::set_setting(conn, "greader_username", "testuser").unwrap();
    db::set_setting(conn, "greader_password", "secret_password").unwrap();
}

#[test]
fn payload_excludes_all_credential_fields() {
    let tmp = std::env::temp_dir().join(format!(
        "fluxreader_cfgsync_test_cred_{}_{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();
    seed_db(&conn);

    let p = build_payload(&conn).unwrap();
    let json = serde_json::to_string(&p).unwrap();

    // 验证：载荷不含任何凭据字段
    assert!(!json.contains("ai_config"), "payload 不应包含 ai_config");
    assert!(!json.contains("apiKey"), "payload 不应包含 AI API key");
    assert!(!json.contains("secret123"), "payload 不应包含凭据值");
    assert!(
        !json.contains("greader_password"),
        "payload 不应包含 greader_password"
    );
    assert!(!json.contains("secret_password"), "payload 不应包含密码");
    assert!(
        !json.contains("config_sync_credentials"),
        "payload 不应包含 config_sync_credentials"
    );
    assert!(
        !json.contains("miniflux_token"),
        "payload 不应包含 miniflux_token"
    );

    // 验证：载荷包含非敏感连接配置
    assert!(
        json.contains("greader_endpoint"),
        "payload 应包含 greader_endpoint"
    );
    assert!(json.contains("example.com"), "payload 应包含服务器地址");
    assert!(
        json.contains("greader_username"),
        "payload 应包含 greader_username"
    );
    assert!(json.contains("testuser"), "payload 应包含用户名");

    // 验证：app_settings 不含本地特定字段
    assert!(!json.contains("autoStart"), "payload 不应包含 autoStart");
    assert!(
        !json.contains("closePromptShown"),
        "payload 不应包含 closePromptShown"
    );
    assert!(json.contains("themeMode"), "payload 应包含 themeMode");

    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn apply_preserves_local_credentials_and_updates_allowed_fields() {
    let tmp = std::env::temp_dir().join(format!(
        "fluxreader_cfgsync_test_apply_cred_{}_{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();

    // 本地有凭据和本地特定设置
    db::set_setting(
        &conn,
        "ai_config",
        r#"{"preset":"local_model","apiKey":"local_secret"}"#,
    )
    .unwrap();
    db::set_setting(&conn, "greader_password", "local_password").unwrap();
    db::set_setting(
        &conn,
        "config_sync_credentials",
        r#"{"token":"local_gist_pat"}"#,
    )
    .unwrap();
    db::set_setting(
        &conn,
        "app_settings",
        r#"{"themeMode":"light","autoStart":false,"closePromptShown":true}"#,
    )
    .unwrap();

    // 远端 payload 包含不同的非敏感配置和设置，但不包含凭据（模拟白名单构建）
    let payload = SyncPayload {
        schema: 1,
        uploaded_at: "2026-09-01T00:00:00Z".into(),
        folders: vec![],
        feeds: vec![],
        app_settings: Some(r#"{"themeMode":"dark","fontSize":20}"#.into()),
        connection_config: Some(app_lib::config_sync::ConnectionConfig {
            sync_protocol: Some("fever".into()),
            greader_endpoint: Some("https://remote.com".into()),
            greader_username: Some("remoteuser".into()),
        }),
    };

    apply_payload(&conn, &payload).unwrap();

    // 验证：本地凭据保持不变
    let ai_config = db::get_setting(&conn, "ai_config").unwrap().unwrap();
    assert!(
        ai_config.contains("local_secret"),
        "本地 ai_config 应保持不变"
    );
    assert!(ai_config.contains("local_model"), "本地 AI 预设应保持不变");

    let password = db::get_setting(&conn, "greader_password").unwrap().unwrap();
    assert_eq!(password, "local_password", "本地密码应保持不变");

    let sync_cred = db::get_setting(&conn, "config_sync_credentials")
        .unwrap()
        .unwrap();
    assert!(
        sync_cred.contains("local_gist_pat"),
        "本地同步凭据应保持不变"
    );

    // 验证：非敏感连接配置被更新
    let protocol = db::get_setting(&conn, "sync_protocol").unwrap().unwrap();
    assert_eq!(protocol, "fever", "sync_protocol 应被远端更新");

    let endpoint = db::get_setting(&conn, "greader_endpoint").unwrap().unwrap();
    assert_eq!(
        endpoint, "https://remote.com",
        "greader_endpoint 应被远端更新"
    );

    let username = db::get_setting(&conn, "greader_username").unwrap().unwrap();
    assert_eq!(username, "remoteuser", "greader_username 应被远端更新");

    // 验证：app_settings 白名单字段更新，本地特定字段保留
    let settings = db::get_setting(&conn, "app_settings").unwrap().unwrap();
    let settings_obj: serde_json::Value = serde_json::from_str(&settings).unwrap();
    assert_eq!(settings_obj["themeMode"], "dark", "themeMode 应被远端更新");
    assert_eq!(settings_obj["fontSize"], 20, "fontSize 应被远端更新");
    assert_eq!(settings_obj["autoStart"], false, "autoStart 应保持本地值");
    assert_eq!(
        settings_obj["closePromptShown"], true,
        "closePromptShown 应保持本地值"
    );

    let _ = std::fs::remove_file(&tmp);
}

/// A①（TASK-074，DEC-req104-p2-12-config-delete-20260920）：远端删掉的
/// app_settings 白名单键必须在本地同步删除（修前只 upsert → 永远残留、
/// 下次上传还会把它带回远端）；本地专属字段 autoStart/closePromptShown
/// 即使远端没有也不得被删除。
#[test]
fn remote_missing_whitelist_setting_is_deleted_locally() {
    let tmp = std::env::temp_dir().join(format!(
        "fluxreader_cfgsync_del_{}_{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();

    // 本地有三个键：themeMode（远端将有）、fontSize（远端将删）、autoStart（本地专属）
    db::set_setting(
        &conn,
        "app_settings",
        r#"{"themeMode":"light","fontSize":18,"autoStart":true,"closePromptShown":true}"#,
    )
    .unwrap();

    // 远端 payload 只带 themeMode —— 即「用户在服务端删掉了 fontSize」
    let payload = SyncPayload {
        schema: 1,
        uploaded_at: "2026-09-01T00:00:00Z".into(),
        folders: vec![],
        feeds: vec![],
        app_settings: Some(r#"{"themeMode":"dark"}"#.into()),
        connection_config: None,
    };
    apply_payload(&conn, &payload).unwrap();

    let raw = db::get_setting(&conn, "app_settings").unwrap().unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(v["themeMode"], "dark", "远端存在的白名单键应覆盖本地");
    assert!(
        v.get("fontSize").is_none(),
        "远端已删除的白名单键必须本地同步删除（修前会残留 18）"
    );
    assert_eq!(v["autoStart"], true, "本地专属字段不得因远端缺失被删除");
    assert_eq!(
        v["closePromptShown"], true,
        "本地专属字段不得因远端缺失被删除"
    );

    let _ = std::fs::remove_file(&tmp);
}

/// A① 对照组：远端 app_settings 为畸形/非对象时，不做删除——避免一次坏
/// payload 把本地设置整批清空（删除语义的失败路径保护）。
#[test]
fn malformed_remote_settings_does_not_wipe_local() {
    let tmp = std::env::temp_dir().join(format!(
        "fluxreader_cfgsync_bad_{}_{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();
    db::set_setting(
        &conn,
        "app_settings",
        r#"{"themeMode":"light","fontSize":18}"#,
    )
    .unwrap();

    // 非对象（字符串）→ 解析成 Value 但 as_object() 为 None
    let payload = SyncPayload {
        schema: 1,
        uploaded_at: "2026-09-01T00:00:00Z".into(),
        folders: vec![],
        feeds: vec![],
        app_settings: Some(r#""not-an-object""#.into()),
        connection_config: None,
    };
    apply_payload(&conn, &payload).unwrap();

    let raw = db::get_setting(&conn, "app_settings").unwrap().unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(v["themeMode"], "light", "畸形远端设置不得清掉本地键");
    assert_eq!(v["fontSize"], 18, "畸形远端设置不得清掉本地键");

    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn payload_contains_all_config_domains() {
    let tmp = std::env::temp_dir().join(format!(
        "fluxreader_cfgsync_test1_{}_{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
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
    let feed_b = p
        .feeds
        .iter()
        .find(|f| f.url == "https://b.com/feed")
        .unwrap();
    assert_eq!(feed_b.folder, "播客");
    assert_eq!(feed_b.layout, "social");
    assert_eq!(feed_b.site_url, Some("https://b.com".to_string()));
    assert_eq!(
        feed_b.favicon_url,
        Some("https://b.com/favicon.ico".to_string())
    );
    // 设置原文带出（过滤后）
    assert!(p.app_settings.as_deref().unwrap().contains("fontSize"));
    // 连接配置带出
    assert_eq!(
        p.connection_config.as_ref().unwrap().sync_protocol,
        Some("greader".to_string())
    );
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn apply_upserts_feeds_and_overrides_settings() {
    let tmp = std::env::temp_dir().join(format!(
        "fluxreader_cfgsync_test2_{}_{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();
    // 本地已有：一个同名分类 + 一个同 URL 源
    let f1 = db::create_folder(&conn, "技术", "gallery").unwrap();
    db::insert_feed(
        &conn,
        "https://a.com/rss",
        None,
        "本地已有源A",
        None,
        f1,
        "inherit",
        false,
        false,
    )
    .unwrap();

    let payload = SyncPayload {
        schema: 1,
        uploaded_at: "2026-09-01T00:00:00Z".into(),
        folders: vec![
            app_lib::config_sync::FolderSpec {
                name: "技术".into(),
                layout: "article".into(),
                auto_summary: true,
                auto_translate: false,
                position: 0,
            },
            app_lib::config_sync::FolderSpec {
                name: "新分类".into(),
                layout: "podcast".into(),
                auto_summary: false,
                auto_translate: true,
                position: 1,
            },
        ],
        feeds: vec![
            // 已存在（同 URL）→ 跳过
            app_lib::config_sync::FeedSpec {
                url: "https://a.com/rss".into(),
                title: "源A".into(),
                folder: "技术".into(),
                layout: "inherit".into(),
                auto_summary: true,
                auto_translate: false,
                site_url: Some("https://a.com".into()),
                favicon_url: None,
            },
            // 新源 → 导入到已有分类
            app_lib::config_sync::FeedSpec {
                url: "https://new.com/rss".into(),
                title: "新源".into(),
                folder: "技术".into(),
                layout: "inherit".into(),
                auto_summary: false,
                auto_translate: false,
                site_url: None,
                favicon_url: None,
            },
            // 新源 + 新分类名 → 分类创建后导入
            app_lib::config_sync::FeedSpec {
                url: "https://pod.com/rss".into(),
                title: "播客源".into(),
                folder: "新分类".into(),
                layout: "inherit".into(),
                auto_summary: false,
                auto_translate: false,
                site_url: None,
                favicon_url: None,
            },
            // 未知分类 → 落「导入」分类
            app_lib::config_sync::FeedSpec {
                url: "https://x.com/rss".into(),
                title: "未知归属".into(),
                folder: "不存在".into(),
                layout: "inherit".into(),
                auto_summary: false,
                auto_translate: false,
                site_url: None,
                favicon_url: None,
            },
        ],
        app_settings: Some(r#"{"themeMode":"light","fontSize":18}"#.into()),
        connection_config: None,
    };

    // TASK-074：imported / updated / skipped 三口径分离。
    // 已存在的那条（https://a.com/rss）若白名单字段与远端一致才算 skipped；
    // 有差异则计入 updated —— 下列断言按实际语义逐项核对，而不是沿用
    // 旧的「skipped=已存在数」口径。
    let outcome = apply_payload(&conn, &payload).unwrap();
    assert_eq!(outcome.imported, 3, "三条新源：new/pod/未知归属");
    assert_eq!(
        outcome.updated + outcome.skipped,
        1,
        "已存在的 a.com/rss 必落 updated 或 skipped 之一"
    );
    // 该源的 title/folder/layout/AI 标志/site_url 与本地种子值不同 → 应判为 updated
    assert_eq!(
        outcome.updated, 1,
        "白名单字段确有变化：必须计入 updated（旧口径把它错记为 skipped）"
    );
    assert_eq!(outcome.skipped, 0, "本次没有内容完全一致的源");

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
    // 已存在的源标题应被更新
    let a = feeds
        .iter()
        .find(|f| f.feed_url == "https://a.com/rss")
        .unwrap();
    assert_eq!(a.title, "源A");
    assert_eq!(a.site_url, Some("https://a.com".to_string()));

    // 设置被覆盖
    let s = db::get_setting(&conn, "app_settings").unwrap().unwrap();
    assert!(s.contains("\"fontSize\":18"));
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
    assert_eq!(
        store.lock().unwrap().as_deref(),
        Some(r#"{"schema":1,"feeds":[]}"#)
    );

    // 下载（WebDAV GET）→ 内容一致
    let got = app_lib::config_sync::webdav_get_for_test(&http, &cred)
        .await
        .unwrap();
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

/// 命令级端到端：build → webdav_put → webdav_get → apply 全链路（对本地 mock）。
#[tokio::test]
async fn full_roundtrip_upload_download_apply() {
    let (port, _store) = start_webdav_mock();
    let http = app_lib::ingestion::build_client(30);
    let cred = app_lib::config_sync::SyncCredentials {
        backend: "webdav".into(),
        token: "pass".into(),
        server: format!("http://127.0.0.1:{port}"),
        username: "user".into(),
        gist_id: None,
    };

    // 设备A：本地库构建 payload 并上传
    let tmp_a = std::env::temp_dir().join(format!(
        "fluxreader_cfgsync_rt_a_{}_{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&tmp_a);
    let conn_a = db::open(&tmp_a).unwrap();
    seed_db(&conn_a);
    let payload_a = build_payload(&conn_a).unwrap();
    app_lib::config_sync::webdav_put_for_test(
        &http,
        &cred,
        &serde_json::to_string(&payload_a).unwrap(),
    )
    .await
    .unwrap();

    // 设备B：空库下载同一配置并应用
    let tmp_b = std::env::temp_dir().join(format!(
        "fluxreader_cfgsync_rt_b_{}_{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&tmp_b);
    let conn_b = db::open(&tmp_b).unwrap();
    let downloaded = app_lib::config_sync::webdav_get_for_test(&http, &cred)
        .await
        .unwrap();
    let parsed: SyncPayload = serde_json::from_str(&downloaded).unwrap();
    let outcome = apply_payload(&conn_b, &parsed).unwrap();

    assert_eq!(outcome.imported, 2, "空库应导入全部 2 个源");
    assert_eq!(outcome.updated, 0, "空库没有已存在的源可更新");
    assert_eq!(outcome.skipped, 0, "空库没有可跳过的源");
    // 设备B 拿到与设备A 相同的分类结构
    let folders_b = db::list_folders(&conn_b).unwrap();
    assert!(folders_b
        .iter()
        .any(|f| f.name == "技术" && f.layout == "article" && f.auto_summary));
    assert!(folders_b
        .iter()
        .any(|f| f.name == "播客" && f.layout == "podcast"));
    // 设备B 拿到设备A 的设置（但不包含凭据）
    let s = db::get_setting(&conn_b, "app_settings").unwrap().unwrap();
    assert!(s.contains("fontSize"));

    // 验证设备B 未获得设备A 的凭据
    let ai_config_b = db::get_setting(&conn_b, "ai_config").unwrap();
    assert!(ai_config_b.is_none(), "设备B 不应获得设备A 的 ai_config");

    let password_b = db::get_setting(&conn_b, "greader_password").unwrap();
    assert!(password_b.is_none(), "设备B 不应获得设备A 的密码");

    let _ = std::fs::remove_file(&tmp_a);
    let _ = std::fs::remove_file(&tmp_b);
}

/// TASK-064 N6：apply_payload 原子性——中途失败必须全量回滚，不留半套已应用
/// 配置（此前无事务：已建的 folders / 已插的 feeds 残留，重试得到叠加结果）。
/// 失败注入点：app_settings 传非法 JSON（merge_app_settings 的 serde 解析必失败），
/// 此时 folders/feeds 已应用完毕——回滚后两者都必须为零。
#[test]
fn apply_failure_rolls_back_the_whole_payload() {
    let tmp = std::env::temp_dir().join(format!(
        "fluxreader_cfgsync_test_rollback_{}_{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();

    let payload = SyncPayload {
        schema: 1,
        uploaded_at: "2026-09-01T00:00:00Z".into(),
        folders: vec![app_lib::config_sync::FolderSpec {
            name: "回滚目录".into(),
            layout: "article".into(),
            auto_summary: false,
            auto_translate: false,
            position: 0,
        }],
        feeds: vec![app_lib::config_sync::FeedSpec {
            url: "https://rollback.example/rss".into(),
            title: "回滚源".into(),
            folder: "回滚目录".into(),
            layout: "article".into(),
            auto_summary: false,
            auto_translate: false,
            site_url: None,
            favicon_url: None,
        }],
        app_settings: Some("{invalid json".into()),
        connection_config: None,
    };

    let err = apply_payload(&conn, &payload).unwrap_err();
    let _ = err; // 失败即可，错误文案不锁死

    let folder_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM folders", [], |r| r.get(0))
        .unwrap();
    let feed_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM feeds", [], |r| r.get(0))
        .unwrap();
    assert_eq!(folder_count, 0, "中途失败必须回滚：不得残留已建目录");
    assert_eq!(feed_count, 0, "中途失败必须回滚：不得残留已插源");

    let _ = std::fs::remove_file(&tmp);
}
