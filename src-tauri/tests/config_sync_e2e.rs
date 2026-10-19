//! 配置同步集成测试：本地 HTTP mock（WebDAV 语义）+ payload 构建/应用逻辑。
//! 覆盖：payload 构建含全部配置域、上传-下载往返、应用 upsert 语义
//! （新源导入/已存在跳过/分类合并/设置覆盖）、凭据边界（上传排除 + 导入保留）。
//!
//! OPT-010 增补：两段式 plan/apply（结构化预览、快照 hash + 配置版本 CAS、
//! 单事务全有全无）、镜像模式删除语义（仅删列出对象、不产生远端退订 outbox、
//! 不误删共享 contents、先移动后删目录）、schema2 稳定身份与 schema1 兼容。
//! 运行：cargo test --test config_sync_e2e

use app_lib::config_sync::{
    apply_payload, apply_payload_checked, build_payload, plan_payload, ApplyMode, FeedSpec,
    FolderSpec, SyncPayload,
};
use app_lib::db;
use rusqlite::Connection;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

mod common;

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
    let tmp = common::unique_db_path("cfgsync_test_cred");
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
fn apply_preserves_local_credentials_and_stores_pending_connection_only() {
    let tmp = common::unique_db_path("cfgsync_test_apply_cred");
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
    // 活动账号（身份 A）——导入 B 后必须原样
    db::set_setting(&conn, "sync_protocol", "greader").unwrap();
    db::set_setting(&conn, "greader_endpoint", "https://active-a.example").unwrap();
    db::set_setting(&conn, "greader_username", "active-user-a").unwrap();

    // 远端 payload 包含不同的非敏感配置和设置，但不包含凭据（模拟白名单构建）
    let payload = SyncPayload {
        schema: 1,
        uploaded_at: "2026-09-01T00:00:00Z".into(),
        folders: vec![],
        feeds: vec![],
        app_settings: Some(r#"{"themeMode":"dark","fontSize":20}"#.into()),
        connection_config: Some(app_lib::config_sync::ConnectionConfig {
            sync_protocol: Some("fever".into()),
            greader_endpoint: Some("https://remote-b.example".into()),
            greader_username: Some("remoteuser-b".into()),
        }),
    };

    let outcome = apply_payload(&conn, &payload).unwrap();

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

    // OPT-006 / F05：活动连接配置**不得**被导入直接覆盖——只落待确认建议。
    assert_eq!(
        db::get_setting(&conn, "sync_protocol").unwrap().unwrap(),
        "greader",
        "导入不得改写活动协议（修前被直接覆盖成 fever）"
    );
    assert_eq!(
        db::get_setting(&conn, "greader_endpoint").unwrap().unwrap(),
        "https://active-a.example",
        "导入不得改写活动地址（防「新地址 + 旧密码」拼接）"
    );
    assert_eq!(
        db::get_setting(&conn, "greader_username").unwrap().unwrap(),
        "active-user-a",
        "导入不得改写活动用户名"
    );
    let pending = outcome
        .pending_connection
        .expect("导入必须生成待确认连接建议（供 UI 展示并引导重新输入凭据）");
    assert!(pending.version >= 1, "建议必须带激活 CAS 版本号");
    assert_eq!(
        pending.connection.greader_endpoint.as_deref(),
        Some("https://remote-b.example")
    );
    assert_eq!(pending.connection.sync_protocol.as_deref(), Some("fever"));
    assert_eq!(
        pending.connection.greader_username.as_deref(),
        Some("remoteuser-b")
    );
    // 待确认建议已实际落库（settings），不是仅返回值
    let stored = db::get_setting(&conn, "pending_connection_config")
        .unwrap()
        .unwrap();
    assert!(stored.contains("remote-b.example"), "建议应落库待消费");

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

/// OPT-006 R2 ③：待确认建议的版本由**独立单调 serial** 发行，清空只清内容、
/// 不重置发行器（防 ABA）；同值重复导入 = ignore（不换 token），换值/清后重导
/// = 新 token。缺版本字段的旧形状兼容为版本 0。
#[test]
fn pending_suggestion_serial_is_monotonic_and_aba_safe() {
    fn payload_with(cc: app_lib::config_sync::ConnectionConfig) -> SyncPayload {
        SyncPayload {
            schema: 1,
            uploaded_at: "2026-09-01T00:00:00Z".into(),
            folders: vec![],
            feeds: vec![],
            app_settings: None,
            connection_config: Some(cc),
        }
    }
    fn stored_version(conn: &Connection) -> u64 {
        let raw = db::get_setting(conn, "pending_connection_config")
            .unwrap()
            .expect("建议应已落库");
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        v.get("version")
            .and_then(|n| n.as_u64())
            .unwrap_or_else(|| panic!("建议必须带版本号：{raw}"))
    }
    fn cc(endpoint: &str, user: &str) -> app_lib::config_sync::ConnectionConfig {
        app_lib::config_sync::ConnectionConfig {
            sync_protocol: Some("greader".into()),
            greader_endpoint: Some(endpoint.into()),
            greader_username: Some(user.into()),
        }
    }

    let tmp = common::unique_db_path("cfgsync_pending_serial");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();

    apply_payload(&conn, &payload_with(cc("http://b.example", "user-b"))).unwrap();
    let v1 = stored_version(&conn);
    assert!(v1 >= 1, "首次导入发行 token 从 1 起");

    // 同值重复导入：ignore——版本不变（不得刷新 token）
    apply_payload(&conn, &payload_with(cc("http://b.example", "user-b"))).unwrap();
    assert_eq!(stored_version(&conn), v1, "同值重复导入必须 ignore");

    // 换值：发新 token
    apply_payload(&conn, &payload_with(cc("http://c.example", "user-c"))).unwrap();
    let v2 = stored_version(&conn);
    assert!(v2 > v1, "换值必须发新 token：{v1} -> {v2}");

    // 清空（放弃建议）后重导同值：serial 不重置 → 新 token（ABA 修复核心）
    app_lib::config_sync::clear_pending_connection(&conn).unwrap();
    assert!(
        app_lib::config_sync::read_pending_suggestion(&conn)
            .unwrap()
            .is_none(),
        "清空后无建议"
    );
    apply_payload(&conn, &payload_with(cc("http://c.example", "user-c"))).unwrap();
    let v3 = stored_version(&conn);
    assert!(
        v3 > v2,
        "清空重导必须高于历史发行值（否则旧 token 会 ABA 命中）：{v2} -> {v3}"
    );

    // 旧平铺形状兼容为版本 0；不同值 store 继续用持久 serial（不倒退）
    db::set_setting(
        &conn,
        "pending_connection_config",
        r#"{"sync_protocol":"greader","greader_endpoint":"http://legacy.example","greader_username":"u"}"#,
    )
    .unwrap();
    let legacy = app_lib::config_sync::read_pending_suggestion(&conn)
        .unwrap()
        .expect("旧形状应可读");
    assert_eq!(legacy.version, 0, "旧形状 = 版本 0");
    let next = app_lib::config_sync::store_pending_connection(
        &conn,
        &cc("http://next.example", "user-next"),
    )
    .unwrap();
    assert!(next > v3, "持久 serial 不得因旧形状回退：{v3} -> {next}");

    let _ = std::fs::remove_file(&tmp);
}

/// OPT-006：无连接配置的导入不触碰待确认建议；空建议不落库。
#[test]
fn apply_without_connection_config_leaves_pending_untouched() {
    let tmp = common::unique_db_path("cfgsync_test_pending_none");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();
    db::set_setting(&conn, "pending_connection_config", "{}").unwrap();

    let payload = SyncPayload {
        schema: 1,
        uploaded_at: "2026-09-01T00:00:00Z".into(),
        folders: vec![],
        feeds: vec![],
        app_settings: None,
        connection_config: None,
    };
    let outcome = apply_payload(&conn, &payload).unwrap();
    assert!(outcome.pending_connection.is_none());
    assert_eq!(
        db::get_setting(&conn, "pending_connection_config")
            .unwrap()
            .unwrap(),
        "{}",
        "无连接配置的导入不得清掉既有建议"
    );

    // 空字段建议（全是 None/空白）不覆盖既有建议
    let payload2 = SyncPayload {
        connection_config: Some(app_lib::config_sync::ConnectionConfig {
            sync_protocol: Some("  ".into()),
            greader_endpoint: None,
            greader_username: Some("".into()),
        }),
        ..payload
    };
    let outcome2 = apply_payload(&conn, &payload2).unwrap();
    assert!(outcome2.pending_connection.is_none(), "空建议不落库");
    let _ = std::fs::remove_file(&tmp);
}

/// A①（TASK-074，DEC-req104-p2-12-config-delete-20260920）：远端删掉的
/// app_settings 白名单键必须在本地同步删除（修前只 upsert → 永远残留、
/// 下次上传还会把它带回远端）；本地专属字段 autoStart/closePromptShown
/// 即使远端没有也不得被删除。
#[test]
fn remote_missing_whitelist_setting_is_deleted_locally() {
    let tmp = common::unique_db_path("cfgsync_del");
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
    let tmp = common::unique_db_path("cfgsync_bad");
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
    let tmp = common::unique_db_path("cfgsync_test1");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();
    seed_db(&conn);

    let p = build_payload(&conn).unwrap();
    // OPT-010：schema 追加为 2（目录携带稳定 sync_id；源仍以 URL 为身份）。
    assert_eq!(p.schema, 2);
    assert_eq!(p.folders.len(), 2);
    assert!(
        p.folders
            .iter()
            .all(|f| f.sync_id.as_deref().is_some_and(|s| !s.is_empty())),
        "schema2 快照的每个分类必须携带稳定 sync_id: {:?}",
        p.folders
    );
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
    let tmp = common::unique_db_path("cfgsync_test2");
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
                sync_id: None,
            },
            app_lib::config_sync::FolderSpec {
                name: "新分类".into(),
                layout: "podcast".into(),
                auto_summary: false,
                auto_translate: true,
                position: 1,
                sync_id: None,
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
                // OPT-011：mode 是真值；该源本地为 off/off，快照为 on/inherit → updated
                summary_mode: Some("on".into()),
                translate_mode: Some("inherit".into()),
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
                summary_mode: Some("inherit".into()),
                translate_mode: Some("off".into()),
                site_url: None,
                favicon_url: None,
            },
            // 新源 + 新分类名 → 分类创建后导入（旧 schema1：无 mode 字段）
            app_lib::config_sync::FeedSpec {
                url: "https://pod.com/rss".into(),
                title: "播客源".into(),
                folder: "新分类".into(),
                layout: "inherit".into(),
                auto_summary: false,
                auto_translate: false,
                summary_mode: None,
                translate_mode: None,
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
                summary_mode: None,
                translate_mode: None,
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
    // OPT-011：mode 是真值——已存在源被快照的 mode 覆盖，bool 兼容列镜像 mode
    assert_eq!(
        (a.summary_mode.as_str(), a.translate_mode.as_str()),
        ("on", "inherit"),
        "快照带 mode 时必须按 mode 落库（不能忽略 synced 配置）"
    );
    assert!(a.auto_summary, "bool 兼容列为 mode==on 的镜像");
    assert!(
        !a.auto_translate,
        "inherit 镜像保留 0（保守，不诱使降级客户端烧 AI）"
    );
    let new_feed = feeds
        .iter()
        .find(|f| f.feed_url == "https://new.com/rss")
        .unwrap();
    assert_eq!(
        (
            new_feed.summary_mode.as_str(),
            new_feed.translate_mode.as_str()
        ),
        ("inherit", "off"),
        "新源导入按 mode 真值落库"
    );
    let legacy_feed = feeds
        .iter()
        .find(|f| f.feed_url == "https://pod.com/rss")
        .unwrap();
    assert_eq!(
        (
            legacy_feed.summary_mode.as_str(),
            legacy_feed.translate_mode.as_str()
        ),
        ("off", "off"),
        "旧 schema1（无 mode）按 bool 映射显式 on/off——绝不引入 inherit 开启"
    );

    // 设置被覆盖
    let s = db::get_setting(&conn, "app_settings").unwrap().unwrap();
    assert!(s.contains("\"fontSize\":18"));

    // OPT-011：同 payload 再应用一次 → 四个源全部按「内容一致」跳过。
    // mode 已进入变化判定：若只比 bool 镜像（inherit 与 off 同为 0），
    // translate_mode=inherit 的那条会被误判一致——本断言可判红该缺口。
    let outcome2 = apply_payload(&conn, &payload).unwrap();
    assert_eq!(
        (outcome2.imported, outcome2.updated, outcome2.skipped),
        (0, 0, 4),
        "重复应用必须幂等：mode 参与逐字段比较，全部跳过"
    );
    let _ = std::fs::remove_file(&tmp);
}

/// OPT-011：快照里的非法 AI mode 必须让整包导入失败（不悄悄回默认、不留半套）。
#[test]
fn apply_rejects_illegal_feed_mode_and_rolls_back() {
    let tmp = common::unique_db_path("cfgsync_test_badmode");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();

    let payload = SyncPayload {
        schema: 1,
        uploaded_at: "2026-09-01T00:00:00Z".into(),
        folders: vec![app_lib::config_sync::FolderSpec {
            name: "非法模式目录".into(),
            layout: "article".into(),
            auto_summary: false,
            auto_translate: false,
            position: 0,
            sync_id: None,
        }],
        feeds: vec![app_lib::config_sync::FeedSpec {
            url: "https://bad.example/rss".into(),
            title: "坏模式源".into(),
            folder: "非法模式目录".into(),
            layout: "article".into(),
            auto_summary: false,
            auto_translate: false,
            summary_mode: Some("maybe".into()),
            translate_mode: None,
            site_url: None,
            favicon_url: None,
        }],
        app_settings: None,
        connection_config: None,
    };

    let err = apply_payload(&conn, &payload).unwrap_err();
    assert!(
        err.message.contains("maybe"),
        "错误必须指出非法值（不悄悄回默认）: {err}"
    );
    let folder_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM folders", [], |r| r.get(0))
        .unwrap();
    let feed_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM feeds", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        (folder_count, feed_count),
        (0, 0),
        "非法 mode 必须整包回滚，不留半套"
    );
    let _ = std::fs::remove_file(&tmp);
}

/// OPT-011：远端 app_settings.refreshInterval 必须与本地写路径 / Rust scheduler
/// 同口径（5–720 整数）。非法值（30.5 / 4 / 721 / 字符串 / null）让整个 apply
/// 失败并回滚——本地既有 settings 与库内容原样，不静默把非法值写进本地
/// （修前 30.5 会原样落库：DB=30.5 / UI 保留旧值 / Rust 回默认，三方分叉）。
#[test]
fn apply_rejects_invalid_remote_refresh_interval_and_rolls_back() {
    for bad in ["30.5", "4", "721", "\"300\"", "null"] {
        let tmp = common::unique_db_path("cfgsync_test_badinterval");
        let _ = std::fs::remove_file(&tmp);
        let conn = db::open(&tmp).unwrap();
        // 本地既有设置与目录：失败后必须逐字原样
        db::set_setting(&conn, "app_settings", r#"{"refreshInterval":300}"#).unwrap();
        db::create_folder(&conn, "本地分类", "article").unwrap();

        let payload = SyncPayload {
            schema: 1,
            uploaded_at: "2026-09-01T00:00:00Z".into(),
            folders: vec![app_lib::config_sync::FolderSpec {
                name: "远端分类".into(),
                layout: "article".into(),
                auto_summary: false,
                auto_translate: false,
                position: 0,
                sync_id: None,
            }],
            feeds: vec![app_lib::config_sync::FeedSpec {
                url: "https://interval.example/rss".into(),
                title: "远端源".into(),
                folder: "远端分类".into(),
                layout: "article".into(),
                auto_summary: false,
                auto_translate: false,
                summary_mode: None,
                translate_mode: None,
                site_url: None,
                favicon_url: None,
            }],
            app_settings: Some(format!(r#"{{"refreshInterval":{bad}}}"#)),
            connection_config: None,
        };

        let err = apply_payload(&conn, &payload).unwrap_err();
        assert!(
            err.message.contains("refreshInterval"),
            "非法 refreshInterval（{bad}）必须被拒绝并说明字段: {err}"
        );
        assert_eq!(
            db::get_setting(&conn, "app_settings").unwrap().as_deref(),
            Some(r#"{"refreshInterval":300}"#),
            "非法 refreshInterval（{bad}）：本地设置必须原样（apply 事务整体回滚）"
        );
        let folders = db::list_folders(&conn).unwrap();
        assert!(
            folders.iter().all(|f| f.name != "远端分类"),
            "非法 refreshInterval（{bad}）：已应用的远端分类必须回滚"
        );
        assert!(
            db::list_feeds(&conn).unwrap().is_empty(),
            "非法 refreshInterval（{bad}）：已插入的远端源必须回滚"
        );
        let _ = std::fs::remove_file(&tmp);
    }
}

/// OPT-011：合法 refreshInterval 正常应用——整数原样；整数值小数（30.0）
/// 数值等价、归一为整数写回（与前端校验表 / Rust 读取同口径）。
#[test]
fn apply_accepts_integral_remote_refresh_interval_normalized() {
    let tmp = common::unique_db_path("cfgsync_test_okinterval");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();

    let apply_with = |raw: &str| {
        let payload = SyncPayload {
            schema: 1,
            uploaded_at: "2026-09-01T00:00:00Z".into(),
            folders: vec![],
            feeds: vec![],
            app_settings: Some(raw.to_string()),
            connection_config: None,
        };
        apply_payload(&conn, &payload).unwrap();
        let s = db::get_setting(&conn, "app_settings").unwrap().unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        v.get("refreshInterval").and_then(|x| x.as_i64())
    };

    assert_eq!(
        apply_with(r#"{"refreshInterval":300}"#),
        Some(300),
        "区间内整数原样应用"
    );
    assert_eq!(
        apply_with(r#"{"refreshInterval":30.0}"#),
        Some(30),
        "整数值小数数值等价，归一为整数写回（不写 30.0 的浮点形态）"
    );
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
    let tmp_a = common::unique_db_path("cfgsync_rt_a");
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
    let tmp_b = common::unique_db_path("cfgsync_rt_b");
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
    // OPT-010 P0 回归：稳定身份随快照往返并在导入端采用；同一快照重放幂等。
    let tech_a_uid = payload_a
        .folders
        .iter()
        .find(|f| f.name == "技术")
        .and_then(|f| f.sync_id.clone())
        .expect("设备A 的快照必须携带分类稳定身份");
    let tech_b = folders_b.iter().find(|f| f.name == "技术").unwrap();
    assert_eq!(
        tech_b.sync_id.as_deref(),
        Some(tech_a_uid.as_str()),
        "设备B 导入的分类必须采用设备A 的稳定身份（修前为随机身份占位）"
    );
    let replay = opt010_plan_json(&conn_b, &parsed, ApplyMode::Mirror);
    assert_eq!(
        (
            replay["additions"].as_array().unwrap().len(),
            replay["updates"].as_array().unwrap().len(),
            replay["removals"].as_array().unwrap().len()
        ),
        (0, 0, 0),
        "真实往返快照在设备B 重放必须幂等: {replay}"
    );
    // OPT-011：AI 开关三态随快照往返一致（设备B 拿到与设备A 相同的 mode）
    let a_b = db::list_feeds(&conn_b)
        .unwrap()
        .into_iter()
        .find(|f| f.feed_url == "https://a.com/rss")
        .unwrap();
    let b_b = db::list_feeds(&conn_b)
        .unwrap()
        .into_iter()
        .find(|f| f.feed_url == "https://b.com/feed")
        .unwrap();
    assert_eq!(
        (a_b.summary_mode.as_str(), a_b.translate_mode.as_str()),
        ("on", "off"),
        "mode 必须随配置快照往返（导出携带、导入生效）"
    );
    assert_eq!(
        (b_b.summary_mode.as_str(), b_b.translate_mode.as_str()),
        ("off", "on"),
        "mode 必须随配置快照往返（第二条源独立三态）"
    );
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
    let tmp = common::unique_db_path("cfgsync_test_rollback");
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
            sync_id: None,
        }],
        feeds: vec![app_lib::config_sync::FeedSpec {
            url: "https://rollback.example/rss".into(),
            title: "回滚源".into(),
            folder: "回滚目录".into(),
            layout: "article".into(),
            auto_summary: false,
            auto_translate: false,
            summary_mode: None,
            translate_mode: None,
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

/* ============================================================
OPT-010：两段式 plan/apply 与镜像应用

契约（见 docs/optimization/OPT-010.md 定版）：
- config_sync_plan(payload, mode) 返回结构化 additions/updates/removals
  （feed/folder + affectedArticleCount）、快照 hash 与当前配置版本签名；
  plan 只读，取消 = 不调用 apply，零写入。
- apply 在单事务内复核快照 hash 与配置版本后执行（全有全无）；预览后本地
  任何会改变删除后果的变更（新增/编辑源、新增文章、读藏翻转、pending 写回
  意图变化）都必须让旧预览失效（stalePreview，零写入）。
- 镜像删除只删 plan 列出的对象：不产生远端退订 outbox、不误删与保留对象
  共享的 contents、先移动保留源再删目录（不误级联）。
============================================================ */

fn opt010_folder(name: &str, sync_id: Option<&str>) -> FolderSpec {
    FolderSpec {
        name: name.into(),
        layout: "article".into(),
        auto_summary: false,
        auto_translate: false,
        position: 0,
        sync_id: sync_id.map(String::from),
    }
}

fn opt010_feed(url: &str, title: &str, folder: &str) -> FeedSpec {
    FeedSpec {
        url: url.into(),
        title: title.into(),
        folder: folder.into(),
        layout: "inherit".into(),
        auto_summary: false,
        auto_translate: false,
        summary_mode: None,
        translate_mode: None,
        site_url: None,
        favicon_url: None,
    }
}

fn opt010_payload(folders: Vec<FolderSpec>, feeds: Vec<FeedSpec>) -> SyncPayload {
    SyncPayload {
        schema: 2,
        uploaded_at: "2026-10-15T00:00:00Z".into(),
        folders,
        feeds,
        app_settings: None,
        connection_config: None,
    }
}

fn opt010_plan_json(conn: &Connection, p: &SyncPayload, mode: ApplyMode) -> serde_json::Value {
    serde_json::to_value(plan_payload(conn, p, mode).unwrap()).unwrap()
}

/// 走与前端相同的两段式：先 plan 取 hash/版本，再经 CAS 校验 apply。
fn opt010_planned_apply(
    conn: &Connection,
    p: &SyncPayload,
    mode: ApplyMode,
) -> app_lib::error::AppResult<app_lib::config_sync::ApplyOutcome> {
    let plan = plan_payload(conn, p, mode)?;
    apply_payload_checked(
        conn,
        p,
        mode,
        Some(&plan.snapshot_hash),
        Some(&plan.current_version),
    )
}

fn opt010_insert_article(conn: &Connection, feed_id: i64, guid: &str, read: bool) -> i64 {
    conn.execute(
        "INSERT INTO articles (feed_id, guid, title, is_read) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![feed_id, guid, format!("文章 {guid}"), read as i64],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn opt010_count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

/// 本地分类的稳定身份（真实 schema2 快照形态：build_payload 产物必带 sync_id）。
fn opt010_local_uid(conn: &Connection, name: &str) -> String {
    db::list_folders(conn)
        .unwrap()
        .into_iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("找不到本地分类 {name}"))
        .sync_id
        .expect("本地分类必须有稳定身份（v26 起创建即分配）")
}

fn opt010_item<'a>(
    plan: &'a serde_json::Value,
    list: &str,
    kind: &str,
    key: &str,
) -> &'a serde_json::Value {
    plan[list]
        .as_array()
        .unwrap_or_else(|| panic!("{list} 必须是数组：{plan}"))
        .iter()
        .find(|it| {
            it["kind"] == kind && {
                let field = if kind == "feed" { "url" } else { "name" };
                it[field] == key
            }
        })
        .unwrap_or_else(|| panic!("{list} 中找不到 {kind} {key}：{plan}"))
}

/// 合并保留本地独有对象；镜像列出 removal 与受影响文章数；plan 本身只读
/// （取消 = 不调用 apply，零写入）。
#[test]
fn plan_is_read_only_and_mirror_lists_removals_with_article_counts() {
    let tmp = common::unique_db_path("cfgsync_opt010_plan");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();
    let f1 = db::create_folder(&conn, "保留", "article").unwrap();
    let f2 = db::create_folder(&conn, "本地独有", "article").unwrap();
    let a = db::insert_feed(
        &conn,
        "https://a.example/rss",
        None,
        "源A",
        None,
        f1,
        "inherit",
        false,
        false,
    )
    .unwrap();
    let b = db::insert_feed(
        &conn,
        "https://b.example/rss",
        None,
        "源B",
        None,
        f2,
        "article",
        false,
        false,
    )
    .unwrap();
    opt010_insert_article(&conn, a, "a1", false);
    opt010_insert_article(&conn, b, "b1", false);
    opt010_insert_article(&conn, b, "b2", true);

    let keep_uid = opt010_local_uid(&conn, "保留");
    let payload = opt010_payload(
        vec![opt010_folder("保留", Some(&keep_uid))],
        vec![opt010_feed("https://a.example/rss", "源A", "保留")],
    );

    let before = (
        opt010_count(&conn, "SELECT COUNT(*) FROM feeds"),
        opt010_count(&conn, "SELECT COUNT(*) FROM folders"),
        opt010_count(&conn, "SELECT COUNT(*) FROM articles"),
        db::get_setting(&conn, "app_settings").unwrap(),
        opt010_count(&conn, "SELECT COUNT(*) FROM subscription_outbox"),
    );

    // 合并：不删除任何本地独有对象
    let merge = opt010_plan_json(&conn, &payload, ApplyMode::Merge);
    assert_eq!(merge["mode"], "merge");
    assert_eq!(
        merge["removals"].as_array().unwrap().len(),
        0,
        "合并模式不得列出 removal: {merge}"
    );
    assert_eq!(merge["affectedArticleCount"], 0, "合并模式受影响文章数为 0");

    // 镜像：列出 feed B 与目录「本地独有」及其文章数
    let mirror = opt010_plan_json(&conn, &payload, ApplyMode::Mirror);
    assert_eq!(mirror["mode"], "mirror");
    let b_item = opt010_item(&mirror, "removals", "feed", "https://b.example/rss");
    assert_eq!(
        b_item["affectedArticleCount"], 2,
        "feed B 有 2 篇文章: {mirror}"
    );
    assert_eq!(b_item["title"], "源B");
    let f2_item = opt010_item(&mirror, "removals", "folder", "本地独有");
    assert_eq!(
        f2_item["feedCount"], 1,
        "目录「本地独有」含 1 个被删源: {mirror}"
    );
    assert_eq!(
        f2_item["affectedArticleCount"], 2,
        "目录级受影响文章数 = 成员被删源的文章和"
    );
    assert_eq!(
        mirror["affectedArticleCount"], 2,
        "总受影响文章数按被删源计一次（目录不重复计）"
    );
    // 保留对象不在删除清单里
    assert!(
        mirror["removals"]
            .as_array()
            .unwrap()
            .iter()
            .all(|it| it["url"] != "https://a.example/rss" && it["name"] != "保留"),
        "保留对象绝不出现在删除清单: {mirror}"
    );

    // 两个 plan 都不产生任何写入（取消语义的基础）
    let after = (
        opt010_count(&conn, "SELECT COUNT(*) FROM feeds"),
        opt010_count(&conn, "SELECT COUNT(*) FROM folders"),
        opt010_count(&conn, "SELECT COUNT(*) FROM articles"),
        db::get_setting(&conn, "app_settings").unwrap(),
        opt010_count(&conn, "SELECT COUNT(*) FROM subscription_outbox"),
    );
    assert_eq!(before, after, "plan 必须只读：plan 前后本地状态逐项一致");
    let _ = std::fs::remove_file(&tmp);
}

/// 镜像 apply 只删除列出对象；已连接账号下**不得**产生远端退订 outbox；
/// 与保留文章共享的 contents 必须保留（只有最后引用消失的内容才回收）。
#[test]
fn mirror_apply_deletes_listed_only_without_unsubscribe_outbox_and_keeps_shared_contents() {
    let tmp = common::unique_db_path("cfgsync_opt010_mirror");
    let _ = std::fs::remove_file(&tmp);
    let mut conn = db::open(&tmp).unwrap();
    // 已连接账号：旧 delete_feed 路径在此会产生 unsubscribe outbox；镜像不得产生。
    app_lib::commands::save_account_for_test(
        &mut conn,
        "greader",
        "http://a.example",
        "user-a",
        "pw-a",
        "http://a.example",
    )
    .unwrap();

    let f1 = db::create_folder(&conn, "保留", "article").unwrap();
    let f2 = db::create_folder(&conn, "本地独有", "article").unwrap();
    let a = db::insert_feed(
        &conn,
        "https://a.example/rss",
        None,
        "源A",
        None,
        f1,
        "inherit",
        false,
        false,
    )
    .unwrap();
    let y = db::insert_feed(
        &conn,
        "https://y.example/rss",
        None,
        "源Y",
        None,
        f2,
        "article",
        false,
        false,
    )
    .unwrap();
    let a1 = opt010_insert_article(&conn, a, "a1", false);
    let y1 = opt010_insert_article(&conn, y, "y1", false);
    let _y2 = opt010_insert_article(&conn, y, "y2", true);

    // 共享内容：A1 与 Y1 引用同一 contents 行；另一行只有 Y2 引用（删 Y 后应回收）。
    conn.execute(
        "INSERT INTO contents (identity, url_norm, content_html, body_text, extraction_source, fulltext_extracted)
         VALUES ('h|shared|0|2', 'https://s.example', '<p>共</p>', '共', 'rss', 0)",
        [],
    )
    .unwrap();
    let shared_cid = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO contents (identity, url_norm, content_html, body_text, extraction_source, fulltext_extracted)
         VALUES ('h|only-y|0|2', 'https://o.example', '<p>独</p>', '独', 'rss', 0)",
        [],
    )
    .unwrap();
    let only_y_cid = conn.last_insert_rowid();
    conn.execute(
        "UPDATE articles SET content_id = ?1 WHERE id IN (?2, ?3)",
        rusqlite::params![shared_cid, a1, y1],
    )
    .unwrap();
    conn.execute(
        "UPDATE articles SET content_id = ?1 WHERE feed_id = ?2 AND guid = 'y2'",
        rusqlite::params![only_y_cid, y],
    )
    .unwrap();

    let keep_uid = opt010_local_uid(&conn, "保留");
    let payload = opt010_payload(
        vec![opt010_folder("保留", Some(&keep_uid))],
        vec![opt010_feed("https://a.example/rss", "源A", "保留")],
    );
    let outcome = opt010_planned_apply(&conn, &payload, ApplyMode::Mirror).unwrap();
    assert_eq!(
        (outcome.removed_feeds, outcome.removed_folders),
        (1, 1),
        "只删除列出的 1 源 1 目录"
    );
    assert_eq!(outcome.affected_article_count, 2, "受影响文章数如实回报");

    // 只删列出的对象：Y 与「本地独有」消失，A 与其文章原样
    let urls: Vec<String> = db::list_feeds(&conn)
        .unwrap()
        .into_iter()
        .map(|f| f.feed_url)
        .collect();
    assert_eq!(
        urls,
        vec!["https://a.example/rss".to_string()],
        "保留源必须在、被删源必须无"
    );
    let names: Vec<String> = db::list_folders(&conn)
        .unwrap()
        .into_iter()
        .map(|f| f.name)
        .collect();
    assert_eq!(
        names,
        vec!["保留".to_string()],
        "被删目录必须无、保留目录必须在"
    );
    assert_eq!(
        opt010_count(
            &conn,
            &format!("SELECT COUNT(*) FROM articles WHERE feed_id = {a}")
        ),
        1,
        "保留源的文章不被误删"
    );
    // 共享内容保留（A1 仍引用）；仅 Y2 引用的内容随最后引用消失回收
    assert_eq!(
        opt010_count(
            &conn,
            &format!("SELECT COUNT(*) FROM contents WHERE id = {shared_cid}")
        ),
        1,
        "与保留文章共享的 contents 绝不能被镜像删除（最后引用才回收）"
    );
    assert_eq!(
        opt010_count(
            &conn,
            &format!("SELECT COUNT(*) FROM contents WHERE id = {only_y_cid}")
        ),
        0,
        "只有被删源引用的内容随最后引用回收"
    );
    // 不产生远端退订 outbox（旧 delete_feed 路径在已连接账号下必然产生）
    assert_eq!(
        opt010_count(&conn, "SELECT COUNT(*) FROM subscription_outbox"),
        0,
        "镜像删除不得偷偷产生远端退订 outbox"
    );
    // 删除意图仍以墓碑落库（防 pull 把已删订阅复活），但它不是 outbox
    assert!(
        db::feed_url_tombstoned(&conn, "https://y.example/rss").unwrap(),
        "镜像删除的源必须落删除墓碑（防复活；与远端退订是两件事）"
    );
    assert!(
        !db::feed_url_tombstoned(&conn, "https://a.example/rss").unwrap(),
        "保留源不得被误写墓碑"
    );
    let _ = std::fs::remove_file(&tmp);
}

/// 预览后本地配置发生会影响删除后果的变化 → stalePreview 且零写入。
#[test]
fn stale_preview_invalidates_with_zero_writes() {
    fn base(tag: &str) -> (Connection, SyncPayload) {
        let tmp = common::unique_db_path(tag);
        let _ = std::fs::remove_file(&tmp);
        let conn = db::open(&tmp).unwrap();
        let f1 = db::create_folder(&conn, "保留", "article").unwrap();
        let f2 = db::create_folder(&conn, "本地独有", "article").unwrap();
        let a = db::insert_feed(
            &conn,
            "https://a.example/rss",
            None,
            "源A",
            None,
            f1,
            "inherit",
            false,
            false,
        )
        .unwrap();
        let b = db::insert_feed(
            &conn,
            "https://b.example/rss",
            None,
            "源B",
            None,
            f2,
            "article",
            false,
            false,
        )
        .unwrap();
        opt010_insert_article(&conn, a, "a1", false);
        opt010_insert_article(&conn, b, "b1", false);
        let keep_uid = opt010_local_uid(&conn, "保留");
        let payload = opt010_payload(
            vec![opt010_folder("保留", Some(&keep_uid))],
            vec![opt010_feed("https://a.example/rss", "源A", "保留")],
        );
        (conn, payload)
    }
    fn plan_hashes(conn: &Connection, p: &SyncPayload) -> (String, String) {
        let plan = plan_payload(conn, p, ApplyMode::Mirror).unwrap();
        (plan.snapshot_hash, plan.current_version)
    }

    // ① 预览后新增本地源 → 失效（新源会被新 plan 纳入删除集，旧预览数字不再代表后果）
    {
        let (conn, payload) = base("cfgsync_opt010_stale_newfeed");
        let (hash, version) = plan_hashes(&conn, &payload);
        let f1 = db::list_folders(&conn).unwrap()[0].id;
        db::insert_feed(
            &conn,
            "https://new.example/rss",
            None,
            "新源",
            None,
            f1,
            "inherit",
            false,
            false,
        )
        .unwrap();
        let err = apply_payload_checked(
            &conn,
            &payload,
            ApplyMode::Mirror,
            Some(&hash),
            Some(&version),
        )
        .unwrap_err();
        assert_eq!(err.code, "stalePreview", "新增源必须让旧预览失效: {err}");
        assert_eq!(
            opt010_count(&conn, "SELECT COUNT(*) FROM feeds"),
            3,
            "失效路径零写入（不得删 B）"
        );
    }

    // ② 预览后编辑源 → 失效，更新也不得发生
    {
        let (conn, payload) = base("cfgsync_opt010_stale_edit");
        let (hash, version) = plan_hashes(&conn, &payload);
        conn.execute(
            "UPDATE feeds SET title = '本地改名' WHERE feed_url = 'https://a.example/rss'",
            [],
        )
        .unwrap();
        let err = apply_payload_checked(
            &conn,
            &payload,
            ApplyMode::Mirror,
            Some(&hash),
            Some(&version),
        )
        .unwrap_err();
        assert_eq!(err.code, "stalePreview");
        let title: String = conn
            .query_row(
                "SELECT title FROM feeds WHERE feed_url = 'https://a.example/rss'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            title, "本地改名",
            "失效路径不得用旧预览覆盖本地编辑（任何更新都回滚/不发生）"
        );
        assert_eq!(
            opt010_count(&conn, "SELECT COUNT(*) FROM feeds"),
            2,
            "不得删除 B"
        );
    }

    // ③ 预览后删除集对象的文章新增 → 失效（受影响文章数已变）
    {
        let (conn, payload) = base("cfgsync_opt010_stale_article");
        let (hash, version) = plan_hashes(&conn, &payload);
        let b: i64 = conn
            .query_row(
                "SELECT id FROM feeds WHERE feed_url = 'https://b.example/rss'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        opt010_insert_article(&conn, b, "b2", false);
        let err = apply_payload_checked(
            &conn,
            &payload,
            ApplyMode::Mirror,
            Some(&hash),
            Some(&version),
        )
        .unwrap_err();
        assert_eq!(
            err.code, "stalePreview",
            "确认前新增文章必须让旧预览失效: {err}"
        );
        assert_eq!(
            opt010_count(&conn, "SELECT COUNT(*) FROM articles"),
            3,
            "零写入（不得删文章）"
        );
    }

    // ④ 预览后删除集对象的读藏变化 → 失效（状态会被级联删除）
    {
        let (conn, payload) = base("cfgsync_opt010_stale_read");
        let (hash, version) = plan_hashes(&conn, &payload);
        conn.execute("UPDATE articles SET is_read = 1 WHERE guid = 'b1'", [])
            .unwrap();
        let err = apply_payload_checked(
            &conn,
            &payload,
            ApplyMode::Mirror,
            Some(&hash),
            Some(&version),
        )
        .unwrap_err();
        assert_eq!(
            err.code, "stalePreview",
            "确认前读藏变化必须让旧预览失效: {err}"
        );
        assert_eq!(
            opt010_count(&conn, "SELECT COUNT(*) FROM feeds"),
            2,
            "零写入"
        );
    }

    // ⑤ 预览后删除集对象出现未推写回意图 → 失效（未推意图的后果已变）
    {
        let (conn, payload) = base("cfgsync_opt010_stale_outbox");
        let (hash, version) = plan_hashes(&conn, &payload);
        let gen = db::sync_generation(&conn).unwrap();
        // 未连接：直接以空 key 落一条同 URL 的 pending 意图（模拟另一窗口刚产生的意图）
        db::enqueue_subscribe(&conn, gen, "", "https://b.example/rss", None, None).unwrap();
        let err = apply_payload_checked(
            &conn,
            &payload,
            ApplyMode::Mirror,
            Some(&hash),
            Some(&version),
        )
        .unwrap_err();
        assert_eq!(
            err.code, "stalePreview",
            "确认前 pending 写回意图变化必须让旧预览失效: {err}"
        );
        assert_eq!(
            opt010_count(&conn, "SELECT COUNT(*) FROM subscription_outbox"),
            1,
            "零写入（意图保留）"
        );
    }

    // ⑥ 快照内容与预览不一致（hash 不符）→ 失效，零写入
    {
        let (conn, payload) = base("cfgsync_opt010_stale_hash");
        let (hash, version) = plan_hashes(&conn, &payload);
        let mut other = payload;
        other.uploaded_at = "2026-10-15T01:00:00Z".into();
        let err = apply_payload_checked(
            &conn,
            &other,
            ApplyMode::Mirror,
            Some(&hash),
            Some(&version),
        )
        .unwrap_err();
        assert_eq!(
            err.code, "stalePreview",
            "换了一份快照必须让旧预览失效: {err}"
        );
        assert_eq!(
            opt010_count(&conn, "SELECT COUNT(*) FROM feeds"),
            2,
            "零写入"
        );
    }
}

/// 镜像 apply 必须先有预览：缺哈希/版本一律拒绝、零写入。
#[test]
fn mirror_apply_requires_preview_hashes() {
    let tmp = common::unique_db_path("cfgsync_opt010_nopreview");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();
    let f1 = db::create_folder(&conn, "保留", "article").unwrap();
    db::insert_feed(
        &conn,
        "https://a.example/rss",
        None,
        "源A",
        None,
        f1,
        "inherit",
        false,
        false,
    )
    .unwrap();
    let keep_uid = opt010_local_uid(&conn, "保留");
    let payload = opt010_payload(
        vec![opt010_folder("保留", Some(&keep_uid))],
        vec![opt010_feed("https://a.example/rss", "源A", "保留")],
    );
    let err = apply_payload_checked(&conn, &payload, ApplyMode::Mirror, None, None).unwrap_err();
    assert_eq!(err.code, "validate", "镜像无预览必须拒绝: {err}");
    assert_eq!(
        opt010_count(&conn, "SELECT COUNT(*) FROM feeds"),
        1,
        "拒绝路径零写入"
    );
    let _ = std::fs::remove_file(&tmp);
}

/// 镜像幂等 + 保留源先移动后删目录：目录被删不级联掉已移动源的文章。
#[test]
fn mirror_is_idempotent_and_moves_kept_feeds_before_folder_removal() {
    let tmp = common::unique_db_path("cfgsync_opt010_move");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();
    let f_old = db::create_folder(&conn, "旧分类", "article").unwrap();
    let x = db::insert_feed(
        &conn,
        "https://x.example/rss",
        None,
        "源X",
        None,
        f_old,
        "inherit",
        false,
        false,
    )
    .unwrap();
    opt010_insert_article(&conn, x, "x1", false);
    opt010_insert_article(&conn, x, "x2", true);

    let payload = opt010_payload(
        vec![opt010_folder("新分类", Some("uid-move-new-cat"))],
        vec![opt010_feed("https://x.example/rss", "源X", "新分类")],
    );
    let outcome = opt010_planned_apply(&conn, &payload, ApplyMode::Mirror).unwrap();
    assert_eq!(
        (outcome.imported, outcome.updated, outcome.skipped),
        (0, 1, 0),
        "保留源记为移动更新"
    );
    assert_eq!(
        (outcome.removed_feeds, outcome.removed_folders),
        (0, 1),
        "旧目录被删、无源被删"
    );

    // 源 X 仍在、文章原样、归属新目录；旧目录消失
    let folders = db::list_folders(&conn).unwrap();
    assert_eq!(
        folders.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
        vec!["新分类"]
    );
    assert_eq!(
        folders[0].sync_id.as_deref(),
        Some("uid-move-new-cat"),
        "导入创建的分类必须采用远端身份（修前先随机分配、采用谓词恒 no-op）"
    );
    let feeds = db::list_feeds(&conn).unwrap();
    let x_row = feeds
        .iter()
        .find(|f| f.feed_url == "https://x.example/rss")
        .expect("源X必须保留");
    assert_eq!(x_row.folder_id, folders[0].id, "源X必须已移动到新目录");
    assert_eq!(
        opt010_count(
            &conn,
            &format!("SELECT COUNT(*) FROM articles WHERE feed_id = {x}")
        ),
        2,
        "目录删除不得级联删掉已移动保留源的文章"
    );

    // 再应用同一快照：幂等（无新增/更新/删除，全部跳过）
    let plan2 = opt010_plan_json(&conn, &payload, ApplyMode::Mirror);
    assert_eq!(
        (
            plan2["additions"].as_array().unwrap().len(),
            plan2["updates"].as_array().unwrap().len(),
            plan2["removals"].as_array().unwrap().len()
        ),
        (0, 0, 0),
        "镜像后重放必须幂等: {plan2}"
    );
    assert_eq!(plan2["affectedArticleCount"], 0);
    let again = opt010_planned_apply(&conn, &payload, ApplyMode::Mirror).unwrap();
    assert_eq!(
        (
            again.imported,
            again.updated,
            again.removed_feeds,
            again.removed_folders,
            again.affected_article_count
        ),
        (0, 0, 0, 0, 0)
    );
    assert_eq!(again.skipped, 1, "源X内容一致 → 跳过");
    let _ = std::fs::remove_file(&tmp);
}

/// mid-apply 注入失败（DELETE 触发器 RAISE）→ 整个镜像事务回滚（新增也不残留）。
#[test]
fn mid_apply_failure_rolls_back_whole_mirror() {
    let tmp = common::unique_db_path("cfgsync_opt010_midfail");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();
    let f1 = db::create_folder(&conn, "保留", "article").unwrap();
    let b = db::insert_feed(
        &conn,
        "https://b.example/rss",
        None,
        "源B",
        None,
        f1,
        "article",
        false,
        false,
    )
    .unwrap();
    opt010_insert_article(&conn, b, "b1", false);
    opt010_insert_article(&conn, b, "b2", false);

    let payload = opt010_payload(
        vec![opt010_folder("新增", Some("uid-midfail-new"))],
        vec![opt010_feed("https://new.example/rss", "新增源", "新增")],
    );
    conn.execute_batch(
        "CREATE TRIGGER opt010_fail BEFORE DELETE ON feeds WHEN old.feed_url = 'https://b.example/rss'
         BEGIN SELECT RAISE(ABORT, 'inject-delete-fail'); END;",
    )
    .unwrap();

    let err = opt010_planned_apply(&conn, &payload, ApplyMode::Mirror).unwrap_err();
    assert!(
        err.message.contains("inject-delete-fail"),
        "注入错误必须原样可见: {err}"
    );
    assert_eq!(
        opt010_count(&conn, "SELECT COUNT(*) FROM folders WHERE name = '新增'"),
        0,
        "中途失败必须回滚已建目录"
    );
    assert_eq!(
        opt010_count(
            &conn,
            "SELECT COUNT(*) FROM feeds WHERE feed_url = 'https://new.example/rss'"
        ),
        0,
        "中途失败必须回滚已插源"
    );
    assert_eq!(
        opt010_count(
            &conn,
            "SELECT COUNT(*) FROM feeds WHERE feed_url = 'https://b.example/rss'"
        ),
        1,
        "被删源保留"
    );
    assert_eq!(
        opt010_count(&conn, "SELECT COUNT(*) FROM articles"),
        2,
        "文章不被删"
    );
    let _ = std::fs::remove_file(&tmp);
}

/// 畸形快照（未知结构/枚举/URL 重复/范围）在任何写入之前整体拒绝；
/// 镜像对未知分类引用更严格（合并保留既有的「导入」兜底语义）。
#[test]
fn malformed_payloads_rejected_before_any_write() {
    let tmp = common::unique_db_path("cfgsync_opt010_bad");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();

    // schema 高于本客户端
    let mut future = opt010_payload(vec![], vec![]);
    future.schema = 99;
    let err = plan_payload(&conn, &future, ApplyMode::Merge).unwrap_err();
    assert!(
        err.message.contains("升级客户端"),
        "未来 schema 必须明确拒绝: {err}"
    );

    // 重复 URL（歧义快照）
    let dup = opt010_payload(
        vec![],
        vec![
            opt010_feed("https://d.example/rss", "一", ""),
            opt010_feed("https://d.example/rss", "二", ""),
        ],
    );
    let err = plan_payload(&conn, &dup, ApplyMode::Merge).unwrap_err();
    assert!(err.message.contains("重复"), "重复 URL 必须拒绝: {err}");

    // 空 URL
    let empty_url = opt010_payload(vec![], vec![opt010_feed("  ", "空", "")]);
    assert!(
        plan_payload(&conn, &empty_url, ApplyMode::Merge).is_err(),
        "空 URL 必须拒绝"
    );

    // 非法 AI 枚举
    let mut bad_mode = opt010_feed("https://m.example/rss", "坏", "");
    bad_mode.summary_mode = Some("maybe".into());
    let err = plan_payload(
        &conn,
        &opt010_payload(vec![], vec![bad_mode]),
        ApplyMode::Merge,
    )
    .unwrap_err();
    assert!(err.message.contains("maybe"), "非法枚举必须指出值: {err}");

    // 非法 refreshInterval 范围
    let mut bad_range = opt010_payload(vec![], vec![]);
    bad_range.app_settings = Some(r#"{"refreshInterval":721}"#.into());
    assert!(
        plan_payload(&conn, &bad_range, ApplyMode::Merge).is_err(),
        "越界值必须拒绝"
    );

    // 镜像：源引用了不存在的分类 → 拒绝（不留半套）；合并：同快照走既有「导入」兜底
    // 快照内重复的分类身份（歧义身份，跨设备匹配不可判定）→ 拒绝
    let dup_uid = opt010_payload(
        vec![
            opt010_folder("甲", Some("dup-uid")),
            opt010_folder("乙", Some("dup-uid")),
        ],
        vec![],
    );
    let err = plan_payload(&conn, &dup_uid, ApplyMode::Merge).unwrap_err();
    assert!(err.message.contains("重复"), "重复分类身份必须拒绝: {err}");

    let unknown_ref = opt010_payload(
        vec![opt010_folder("真实", Some("uid-real"))],
        vec![opt010_feed("https://u.example/rss", "无归属", "不存在")],
    );
    let err = plan_payload(&conn, &unknown_ref, ApplyMode::Mirror).unwrap_err();
    assert!(
        err.message.contains("不存在"),
        "镜像必须拒绝未知分类引用: {err}"
    );
    let merge_plan = opt010_plan_json(&conn, &unknown_ref, ApplyMode::Merge);
    let sink = opt010_item(&merge_plan, "additions", "feed", "https://u.example/rss");
    assert_eq!(sink["folder"], "导入", "合并保留「导入」兜底归属");
    let sink_folder = opt010_item(&merge_plan, "additions", "folder", "导入");
    assert_eq!(sink_folder["kind"], "folder", "「导入」目录作为新增列出");

    // 全部拒绝路径零写入
    assert_eq!(
        opt010_count(&conn, "SELECT COUNT(*) FROM folders"),
        0,
        "畸形快照不得写入任何目录"
    );
    assert_eq!(
        opt010_count(&conn, "SELECT COUNT(*) FROM feeds"),
        0,
        "畸形快照不得写入任何源"
    );
    assert_eq!(
        opt010_count(&conn, "SELECT COUNT(*) FROM subscription_outbox"),
        0,
        "畸形快照不得产生 outbox"
    );
    let _ = std::fs::remove_file(&tmp);
}

/// 预览携带结构化设置变化；敏感字段（AI 服务配置/密码）既不出网也不出现在预览里；
/// plan 不落待确认连接建议（apply 才落）。
#[test]
fn plan_exposes_settings_delta_and_never_echoes_secrets() {
    let tmp = common::unique_db_path("cfgsync_opt010_secrets");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();
    let f1 = db::create_folder(&conn, "技术", "article").unwrap();
    db::insert_feed(
        &conn,
        "https://a.example/rss",
        None,
        "源A",
        None,
        f1,
        "inherit",
        false,
        false,
    )
    .unwrap();
    db::set_setting(
        &conn,
        "app_settings",
        r#"{"themeMode":"light","fontSize":18,"autoStart":true}"#,
    )
    .unwrap();
    db::set_setting(
        &conn,
        "ai_config",
        r#"{"preset":"glm","apiKey":"top-secret-ai-key"}"#,
    )
    .unwrap();
    db::set_setting(&conn, "greader_password", "top-secret-password").unwrap();
    db::set_setting(
        &conn,
        "config_sync_credentials",
        r#"{"token":"top-secret-pat"}"#,
    )
    .unwrap();

    let upload = build_payload(&conn).unwrap();
    let upload_json = serde_json::to_string(&upload).unwrap();
    for secret in [
        "top-secret-ai-key",
        "top-secret-password",
        "top-secret-pat",
        "ai_config",
        "apiKey",
    ] {
        assert!(
            !upload_json.contains(secret),
            "上传快照不得包含敏感值/字段：{secret}"
        );
    }
    assert!(!upload_json.contains("autoStart"), "本地专属设置不出网");

    let tech_uid = opt010_local_uid(&conn, "技术");
    let mut payload = opt010_payload(
        vec![opt010_folder("技术", Some(&tech_uid))],
        vec![opt010_feed("https://a.example/rss", "源A", "技术")],
    );
    payload.app_settings = Some(r#"{"themeMode":"dark"}"#.into());
    payload.connection_config = Some(app_lib::config_sync::ConnectionConfig {
        sync_protocol: Some("fever".into()),
        greader_endpoint: Some("http://b.example".into()),
        greader_username: Some("user-b".into()),
    });

    let plan = opt010_plan_json(&conn, &payload, ApplyMode::Merge);
    assert_eq!(
        (
            plan["additions"].as_array().unwrap().len(),
            plan["updates"].as_array().unwrap().len()
        ),
        (0, 0),
        "带身份快照按身份命中既有分类/源：无新增无更新: {plan}"
    );
    assert_eq!(
        plan["settingsChangedKeys"],
        serde_json::json!(["themeMode"]),
        "远端存在的白名单键变化必须如实列出: {plan}"
    );
    assert_eq!(
        plan["settingsRemovedKeys"],
        serde_json::json!(["fontSize"]),
        "远端缺失的白名单键删除必须如实列出: {plan}"
    );
    assert_eq!(
        plan["pendingConnection"], true,
        "含连接建议的快照必须在预览里明示"
    );
    let plan_str = plan.to_string();
    for secret in [
        "top-secret-ai-key",
        "top-secret-password",
        "top-secret-pat",
        "ai_config",
        "apiKey",
    ] {
        assert!(!plan_str.contains(secret), "预览不得回显敏感值：{secret}");
    }
    assert!(
        !plan_str.contains("autoStart"),
        "预览的本地专属设置不参与同步"
    );
    // plan 不写待确认建议（apply 才落库）
    assert!(
        db::get_setting(&conn, "pending_connection_config")
            .unwrap()
            .is_none(),
        "plan 不得落库待确认建议"
    );

    // apply 后：白名单合并照常、待确认建议落库、活动凭据不动
    opt010_planned_apply(&conn, &payload, ApplyMode::Merge).unwrap();
    let settings = db::get_setting(&conn, "app_settings").unwrap().unwrap();
    let v: serde_json::Value = serde_json::from_str(&settings).unwrap();
    assert_eq!(v["themeMode"], "dark");
    assert!(
        v.get("fontSize").is_none(),
        "远端已删的白名单键本地同步删除"
    );
    assert_eq!(v["autoStart"], true, "本地专属设置保留");
    assert!(
        db::get_setting(&conn, "pending_connection_config")
            .unwrap()
            .is_some(),
        "apply 落待确认建议"
    );
    assert!(
        db::get_setting(&conn, "greader_endpoint")
            .unwrap()
            .is_none(),
        "导入不写活动连接配置"
    );
    let _ = std::fs::remove_file(&tmp);
}

/// 旧 schema1 快照（无稳定 ID / 无 mode）仍可合并；创建的对象获得本地稳定身份，
/// 重放幂等（不因缺少远端 sync_id 反复改写）。
#[test]
fn schema1_payload_still_merges_and_assigns_local_identity() {
    let tmp = common::unique_db_path("cfgsync_opt010_schema1");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();

    let mut payload = opt010_payload(
        vec![opt010_folder("分类", None)],
        vec![opt010_feed("https://s1.example/rss", "老源", "分类")],
    );
    payload.schema = 1;
    let outcome = apply_payload(&conn, &payload).unwrap();
    assert_eq!(outcome.imported, 1, "schema1 快照可合并导入");
    let folders = db::list_folders(&conn).unwrap();
    assert_eq!(folders.len(), 1);
    assert!(
        folders[0].sync_id.as_deref().is_some_and(|s| !s.is_empty()),
        "schema1 导入创建的目录也必须获得本地稳定身份（供后续快照携带）"
    );
    let outcome2 = apply_payload(&conn, &payload).unwrap();
    assert_eq!(
        (outcome2.imported, outcome2.updated, outcome2.skipped),
        (0, 0, 1),
        "schema1 重放幂等"
    );
    let plan = opt010_plan_json(&conn, &payload, ApplyMode::Merge);
    assert_eq!(
        (
            plan["additions"].as_array().unwrap().len(),
            plan["updates"].as_array().unwrap().len()
        ),
        (0, 0),
        "schema1 重放的预览也应无变化: {plan}"
    );
    let _ = std::fs::remove_file(&tmp);
}

/// 镜像的未推意图保留策略：已有 pending 写回意图一个字节不动（不替换、不清除、
/// 不新造），预览给出明确提醒；无意图的被删源不产生新行。
#[test]
fn mirror_keeps_existing_pending_intents_and_creates_none() {
    let tmp = common::unique_db_path("cfgsync_opt010_outbox_keep");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();
    let f1 = db::create_folder(&conn, "保留", "article").unwrap();
    let f2 = db::create_folder(&conn, "本地独有", "article").unwrap();
    db::insert_feed(
        &conn,
        "https://a.example/rss",
        None,
        "源A",
        None,
        f1,
        "inherit",
        false,
        false,
    )
    .unwrap();
    db::insert_feed(
        &conn,
        "https://y.example/rss",
        None,
        "源Y",
        None,
        f2,
        "article",
        false,
        false,
    )
    .unwrap();
    db::insert_feed(
        &conn,
        "https://z.example/rss",
        None,
        "源Z",
        None,
        f2,
        "article",
        false,
        false,
    )
    .unwrap();
    let gen = db::sync_generation(&conn).unwrap();
    db::enqueue_subscribe(&conn, gen, "acct|key", "https://y.example/rss", None, None).unwrap();
    let before: (i64, String) = conn
        .query_row(
            "SELECT id, status FROM subscription_outbox WHERE feed_url = 'https://y.example/rss'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();

    let keep_uid = opt010_local_uid(&conn, "保留");
    let payload = opt010_payload(
        vec![opt010_folder("保留", Some(&keep_uid))],
        vec![opt010_feed("https://a.example/rss", "源A", "保留")],
    );
    let plan = opt010_plan_json(&conn, &payload, ApplyMode::Mirror);
    assert!(
        plan["warnings"].as_array().is_some_and(|w| !w.is_empty()),
        "被删源存在未推送写回意图时预览必须提醒（保留策略明确）: {plan}"
    );
    opt010_planned_apply(&conn, &payload, ApplyMode::Mirror).unwrap();

    assert_eq!(
        opt010_count(&conn, "SELECT COUNT(*) FROM subscription_outbox"),
        1,
        "既有意图一个不差地保留"
    );
    let after: (i64, String) = conn
        .query_row(
            "SELECT id, status FROM subscription_outbox WHERE feed_url = 'https://y.example/rss'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        before, after,
        "既有 pending 行不得被镜像替换/清理（保留未推意图）"
    );
    assert_eq!(
        opt010_count(
            &conn,
            "SELECT COUNT(*) FROM subscription_outbox WHERE feed_url = 'https://z.example/rss'"
        ),
        0,
        "无意图的被删源不得新造 outbox 行"
    );
    let _ = std::fs::remove_file(&tmp);
}

/// 本地分类身份（可空形态；迁移前残留行/手工行为 None）。
fn opt010_local_uid_opt(conn: &Connection, name: &str) -> Option<String> {
    db::list_folders(conn)
        .unwrap()
        .into_iter()
        .find(|f| f.name == name)
        .and_then(|f| f.sync_id)
}

/* ============================================================
P0-1 回归：schema2 稳定身份的导入采用与重放幂等（r1 返工）

缺陷（REVIEW-r1 P0-1）：`FolderPlan::New` 先 `create_folder`（随机身份）再
`set_folder_sync_id`（只填空谓词 → 恒 no-op）——带 sync_id 的真实快照导入后，
本地身份是我方随机值；重放时远端身份 R 与本地随机值 W 双方非空且不等，落入
「同名不同身份」冲突：合并每次新增重复目录、镜像每次删建往复。
============================================================ */

/// 空库导入带身份 schema2 快照：目录身份必须等于远端值；同一快照重放两次
/// plan 全零（合并与镜像）；镜像重放 apply 零删除。
/// 判别力：修前导入目录身份 ≠ 远端值 → 本组断言必红。
#[test]
fn schema2_import_adopts_remote_folder_identity_and_replays_idempotently() {
    let tmp = common::unique_db_path("cfgsync_opt010_identity");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();

    let payload = opt010_payload(
        vec![opt010_folder("分类", Some("uid-remote-1"))],
        vec![opt010_feed("https://s2.example/rss", "源", "分类")],
    );
    let outcome = opt010_planned_apply(&conn, &payload, ApplyMode::Merge).unwrap();
    assert_eq!(outcome.imported, 1, "空库导入源");
    let folders = db::list_folders(&conn).unwrap();
    assert_eq!(folders.len(), 1);
    assert_eq!(
        folders[0].sync_id.as_deref(),
        Some("uid-remote-1"),
        "导入创建的分类必须采用远端稳定身份（修前被随机身份占位、采用谓词恒 no-op）"
    );

    // 重放：合并与镜像 plan 全零（防「同名不同身份」冲突被误触发）
    for mode in [ApplyMode::Merge, ApplyMode::Mirror] {
        let plan = opt010_plan_json(&conn, &payload, mode);
        assert_eq!(
            (
                plan["additions"].as_array().unwrap().len(),
                plan["updates"].as_array().unwrap().len(),
                plan["removals"].as_array().unwrap().len()
            ),
            (0, 0, 0),
            "带身份快照重放必须幂等（{mode:?}）: {plan}"
        );
    }
    let again = opt010_planned_apply(&conn, &payload, ApplyMode::Mirror).unwrap();
    assert_eq!(
        (
            again.imported,
            again.updated,
            again.skipped,
            again.removed_feeds,
            again.removed_folders
        ),
        (0, 0, 1, 0, 0),
        "镜像重放：removed_folders=0、源内容一致跳过"
    );
    let _ = std::fs::remove_file(&tmp);
}

/// 本地身份为空（v26 之前的残留行/手工行）时，名称命中必须采用远端身份
/// （只填空、绝不改写既有身份）；采用后按身份稳定匹配（重放全零）。
#[test]
fn local_folder_without_identity_adopts_remote_identity_by_name() {
    let tmp = common::unique_db_path("cfgsync_opt010_adopt");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();
    conn.execute(
        "INSERT INTO folders (name, position, layout, collapsed, sync_id)
         VALUES ('旧分类', 0, 'article', 1, NULL)",
        [],
    )
    .unwrap();
    assert!(
        opt010_local_uid_opt(&conn, "旧分类").is_none(),
        "前置：本地身份为空"
    );

    let payload = opt010_payload(vec![opt010_folder("旧分类", Some("uid-adopt"))], vec![]);
    let plan = opt010_plan_json(&conn, &payload, ApplyMode::Merge);
    assert_eq!(
        plan["additions"].as_array().unwrap().len(),
        0,
        "名称命中的本地空身份行不得被当作新增: {plan}"
    );
    opt010_planned_apply(&conn, &payload, ApplyMode::Merge).unwrap();
    assert_eq!(
        opt010_local_uid_opt(&conn, "旧分类").as_deref(),
        Some("uid-adopt"),
        "空身份必须被远端身份填空采用"
    );
    let plan2 = opt010_plan_json(&conn, &payload, ApplyMode::Merge);
    assert_eq!(
        (
            plan2["additions"].as_array().unwrap().len(),
            plan2["updates"].as_array().unwrap().len()
        ),
        (0, 0),
        "采用后重放按身份稳定匹配: {plan2}"
    );
    let _ = std::fs::remove_file(&tmp);
}

/// 同名但双方身份不同 = 不同对象（不采用、不猜）：
/// 合并 → 新增远端对象、本地身份原样、预览显式预警冲突；
/// 镜像 → 新增远端对象 + 删除本地对象（收敛到快照身份）。
#[test]
fn same_name_different_identity_is_conflict_not_adoption() {
    // 合并半边：不采用、不合并，两个对象并存
    {
        let tmp = common::unique_db_path("cfgsync_opt010_conflict_merge");
        let _ = std::fs::remove_file(&tmp);
        let conn = db::open(&tmp).unwrap();
        db::create_folder(&conn, "分类", "article").unwrap();
        let local_uid = opt010_local_uid(&conn, "分类");
        let payload = opt010_payload(
            vec![opt010_folder("分类", Some("uid-remote-other"))],
            vec![],
        );

        let plan = opt010_plan_json(&conn, &payload, ApplyMode::Merge);
        assert_eq!(
            plan["additions"].as_array().unwrap().len(),
            1,
            "同名不同身份在合并下按新增列出（不采用）: {plan}"
        );
        assert!(
            plan["warnings"].as_array().is_some_and(|w| w
                .iter()
                .any(|x| x.as_str().is_some_and(|t| t.contains("身份不同")))),
            "冲突必须显式预警（不猜同名即同对象）: {plan}"
        );
        opt010_planned_apply(&conn, &payload, ApplyMode::Merge).unwrap();
        let folders = db::list_folders(&conn).unwrap();
        assert_eq!(folders.len(), 2, "不同身份是两个对象（不采用、不合并）");
        assert!(
            folders
                .iter()
                .any(|f| f.sync_id.as_deref() == Some(local_uid.as_str())),
            "本地既有身份必须原样保留"
        );
        assert!(
            folders
                .iter()
                .any(|f| f.sync_id.as_deref() == Some("uid-remote-other")),
            "远端身份照新增建立"
        );
        let _ = std::fs::remove_file(&tmp);
    }

    // 镜像半边：删本地旧身份、建远端身份（显式冲突后收敛到快照）
    {
        let tmp = common::unique_db_path("cfgsync_opt010_conflict_mirror");
        let _ = std::fs::remove_file(&tmp);
        let conn = db::open(&tmp).unwrap();
        let local_id = db::create_folder(&conn, "分类", "article").unwrap();
        let payload = opt010_payload(
            vec![opt010_folder("分类", Some("uid-remote-other"))],
            vec![],
        );

        let plan = opt010_plan_json(&conn, &payload, ApplyMode::Mirror);
        assert_eq!(
            plan["additions"].as_array().unwrap().len(),
            1,
            "镜像新增远端对象: {plan}"
        );
        assert_eq!(
            plan["removals"].as_array().unwrap().len(),
            1,
            "镜像删除本地同名不同身份对象: {plan}"
        );
        opt010_planned_apply(&conn, &payload, ApplyMode::Mirror).unwrap();
        let folders = db::list_folders(&conn).unwrap();
        assert_eq!(folders.len(), 1, "镜像后收敛到快照的一份目录");
        assert_eq!(folders[0].sync_id.as_deref(), Some("uid-remote-other"));
        assert_ne!(folders[0].id, local_id, "是重建而不是改写既有行身份");
        let _ = std::fs::remove_file(&tmp);
    }
}

/// 远端身份已被本地其它对象占用（自相矛盾的混合快照：身份所属对象已被另一个
/// 无身份槽位按名称占用）→ 新建不得复用该身份：降级随机身份 + 显式预警；
/// 既有对象身份原样保留（身份唯一性纪律——两个对象绝不共享同一身份）。
#[test]
fn import_never_reuses_identity_taken_by_another_local_object() {
    let tmp = common::unique_db_path("cfgsync_opt010_uid_taken");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();
    conn.execute(
        "INSERT INTO folders (name, position, layout, collapsed, sync_id)
         VALUES ('甲', 0, 'article', 1, 'uid-taken')",
        [],
    )
    .unwrap();

    let payload = opt010_payload(
        vec![
            // 无身份槽位先按名称命中本地「甲」（身份 uid-taken 被它占用）
            opt010_folder("甲", None),
            // 后续槽位又声称自己是 uid-taken（矛盾快照）→ 不得复用该身份
            opt010_folder("乙", Some("uid-taken")),
        ],
        vec![],
    );
    let plan = opt010_plan_json(&conn, &payload, ApplyMode::Merge);
    assert!(
        plan["warnings"]
            .as_array()
            .is_some_and(|w| w.iter().any(|x| x
                .as_str()
                .is_some_and(|t| t.contains("已被本地其它对象占用")))),
        "身份占用冲突必须显式预警: {plan}"
    );
    opt010_planned_apply(&conn, &payload, ApplyMode::Merge).unwrap();
    let folders = db::list_folders(&conn).unwrap();
    assert_eq!(folders.len(), 2);
    assert_eq!(
        opt010_local_uid_opt(&conn, "甲").as_deref(),
        Some("uid-taken"),
        "既有对象的身份不得被改写/转走"
    );
    let yi_uid = opt010_local_uid_opt(&conn, "乙").unwrap();
    assert!(
        !yi_uid.trim().is_empty() && yi_uid != "uid-taken",
        "冲突身份降级为随机新身份（不复用）: {yi_uid}"
    );

    // P3-1（r2 复审登记，钉住「有意为之」的不收敛）：矛盾快照重放时该槽位仍是
    // 「冲突 → 降级新身份」——merge 每次确认都会新增一份空分类。唯一性优先于
    // 收敛是刻意取舍；过程不静默：新增项与预警每次都在预览里可见、需用户确认。
    let replay = opt010_plan_json(&conn, &payload, ApplyMode::Merge);
    assert_eq!(
        replay["additions"].as_array().unwrap().len(),
        1,
        "矛盾快照重放按契约仍列为新增（每次确认可见、需用户确认）: {replay}"
    );
    assert!(
        replay["warnings"].as_array().is_some_and(|w| !w.is_empty()),
        "重放仍伴随预警（不静默）: {replay}"
    );
    let _ = std::fs::remove_file(&tmp);
}

/// P2-1 回归：Match 采用路径的占用护栏——本地 NULL 身份行按名称命中时，若远端
/// 身份已被本地**另一对象**占用，不得采用（否则两个对象共享同一非空 sync_id，
/// 违反身份唯一性纪律）。
/// 场景（自相矛盾的混合快照）：槽0 无身份按名称消耗 L1；槽1 携带 L1 的身份，
/// 身份匹配失败（L1 已用）→ 名称命中 L2（NULL）→ 采用被拦截：L2 保持 NULL、
/// 出现预警、全库身份无重复。
/// 判别力：修前 adopt 无占用检查，L2 静默拿到 "uid-occupied"（两条断言必红）。
#[test]
fn match_adoption_never_shares_identity_with_another_local_object() {
    let tmp = common::unique_db_path("cfgsync_opt010_match_uid_taken");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();
    conn.execute_batch(
        "INSERT INTO folders (name, position, layout, collapsed, sync_id)
             VALUES ('甲', 0, 'article', 1, 'uid-occupied');
         INSERT INTO folders (name, position, layout, collapsed, sync_id)
             VALUES ('乙', 0, 'article', 1, NULL);",
    )
    .unwrap();

    let payload = opt010_payload(
        vec![
            // 无身份槽位先按名称命中本地「甲」→ 身份 uid-occupied 归属该行
            opt010_folder("甲", None),
            // 该槽声称自己是 uid-occupied：身份匹配甲失败（已被占用）→ 名称命中
            // 本地 NULL 身份行「乙」→ Match 空身份采用路径（本用例靶点）
            opt010_folder("乙", Some("uid-occupied")),
        ],
        vec![],
    );
    let plan = opt010_plan_json(&conn, &payload, ApplyMode::Merge);
    assert!(
        plan["warnings"]
            .as_array()
            .is_some_and(|w| w.iter().any(|x| x
                .as_str()
                .is_some_and(|t| t.contains("已被本地其它对象占用")))),
        "采用被占用身份必须显式预警（修前静默共享身份）: {plan}"
    );
    opt010_planned_apply(&conn, &payload, ApplyMode::Merge).unwrap();

    assert_eq!(
        opt010_local_uid_opt(&conn, "甲").as_deref(),
        Some("uid-occupied"),
        "占用方身份原样（不得被改写/转走）"
    );
    assert_eq!(
        opt010_local_uid_opt(&conn, "乙"),
        None,
        "被占用的身份绝不能被采用到另一对象（修前此处拿到 uid-occupied，产生共享身份）"
    );
    let dup = opt010_count(
        &conn,
        "SELECT COUNT(*) FROM (SELECT sync_id FROM folders
         WHERE sync_id IS NOT NULL AND trim(sync_id) != ''
         GROUP BY sync_id HAVING COUNT(*) > 1)",
    );
    assert_eq!(dup, 0, "全库不得存在重复的非空 sync_id");
    let _ = std::fs::remove_file(&tmp);
}
