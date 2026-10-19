//! OPT-006 屏障回归：账号代际与在途响应失效（F05/F06）。
//!
//! 每个场景都用**真实本地 HTTP mock + 阻塞屏障**（不靠 sleep）制造交错：
//! ① A 的订阅/分类响应在途 → 断开 → 释放：无 A 内容/绑定复活、无旧游标写回；
//! ② A 的编辑推送在途 → 切到 B → 释放：旧确认不落库、后续批次不再发出、
//!    B 的队列/绑定不变；
//! ③ A 的条目响应在途 → 断开 → 释放：无文章落库、无游标推进（GR 与 Fever）；
//! ④ 配置导入只落待确认建议：活动凭据不变、B 服务收不到 A 的密码；
//! ⑤ 在途 HTTP 不冻结本地 DB 读写；失败后无永久 busy/锁泄漏。
//!
//! 运行：cargo test --test sync_session_e2e

mod common;

use app_lib::db;
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

/// 屏障 + 请求记录：被门控路径的响应挂起，直到测试显式释放。
struct Gate {
    gated: StdMutex<Vec<String>>,
    parked_tx: mpsc::UnboundedSender<()>,
    parked_count: AtomicUsize,
    released: AtomicBool,
    release_notify: tokio::sync::Notify,
    requests: StdMutex<Vec<String>>,
    bodies: StdMutex<Vec<String>>,
}

impl Gate {
    fn new(tx: mpsc::UnboundedSender<()>) -> Self {
        Self {
            gated: StdMutex::new(Vec::new()),
            parked_tx: tx,
            parked_count: AtomicUsize::new(0),
            released: AtomicBool::new(false),
            release_notify: tokio::sync::Notify::new(),
            requests: StdMutex::new(Vec::new()),
            bodies: StdMutex::new(Vec::new()),
        }
    }

    fn arm(&self, path_fragment: &str) {
        self.gated.lock().unwrap().push(path_fragment.to_string());
    }

    fn is_gated(&self, path: &str) -> bool {
        self.gated
            .lock()
            .unwrap()
            .iter()
            .any(|f| path.contains(f.as_str()))
    }

    /// 挂起当前请求：先发「已停靠」信号，再等待释放（flag+notify 双条件防丢唤醒）。
    async fn park(&self) {
        self.parked_count.fetch_add(1, Ordering::SeqCst);
        let _ = self.parked_tx.send(());
        while !self.released.load(Ordering::SeqCst) {
            self.release_notify.notified().await;
        }
    }

    fn release(&self) {
        self.released.store(true, Ordering::SeqCst);
        self.release_notify.notify_waiters();
    }

    fn parked_count(&self) -> usize {
        self.parked_count.load(Ordering::SeqCst)
    }

    fn record(&self, line: &str, body: &str) {
        self.requests.lock().unwrap().push(line.to_string());
        self.bodies.lock().unwrap().push(body.to_string());
    }

    fn request_lines(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    fn bodies(&self) -> Vec<String> {
        self.bodies.lock().unwrap().clone()
    }

    fn edit_tag_count(&self) -> usize {
        self.request_lines()
            .iter()
            .filter(|r| r.contains("edit-tag"))
            .count()
    }
}

struct MockState {
    gate: Arc<Gate>,
    feed_url: String,
    entry_url: String,
}

struct MockServer {
    base: String,
    gate: Arc<Gate>,
    feed_url: String,
    entry_url: String,
    /// 每次有请求被屏障挂起时收到一个信号（select! 用，避免轮询）。
    parked_rx: mpsc::UnboundedReceiver<()>,
}

/// 启动最小 GReader（+ Fever 兼容）mock：认证/订阅/分类/条目/编辑路由。
async fn start_mock(tag: &str) -> MockServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, parked_rx) = mpsc::unbounded_channel();
    let gate = Arc::new(Gate::new(tx));
    let feed_url = format!("http://127.0.0.1:{port}/{tag}_feed.xml");
    let entry_url = format!("http://127.0.0.1:{port}/{tag}_entry");
    let state = Arc::new(MockState {
        gate: gate.clone(),
        feed_url: feed_url.clone(),
        entry_url: entry_url.clone(),
    });
    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => break,
            };
            let st = state.clone();
            tokio::spawn(async move {
                let _ = handle_conn(stream, st).await;
            });
        }
    });
    MockServer {
        base: format!("http://127.0.0.1:{port}"),
        gate,
        feed_url,
        entry_url,
        parked_rx,
    }
}

async fn handle_conn(mut stream: tokio::net::TcpStream, st: Arc<MockState>) -> std::io::Result<()> {
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        if let Some(pos) = find_header_end(&buf) {
            break pos;
        }
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.len() > 1_048_576 {
            return Ok(());
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let content_length: usize = head
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("content-length"))
        .and_then(|l| l.split(':').nth(1))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    let mut body: Vec<u8> = buf[header_end + 4..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    let body = String::from_utf8_lossy(&body).to_string();
    let first_line = head.lines().next().unwrap_or("").to_string();
    let path = first_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .to_string();
    st.gate.record(&first_line, &body);

    if st.gate.is_gated(&path) {
        st.gate.park().await;
    }

    let (status, content_type, payload) = route(&path, &body, &st);
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    stream.write_all(resp.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn route(path: &str, body: &str, st: &Arc<MockState>) -> (&'static str, &'static str, String) {
    if path.contains("/accounts/ClientLogin") {
        return ("200 OK", "text/plain", "SID=s\nLSID=l\nAuth=tok\n".into());
    }
    if path.contains("/reader/api/0/token") {
        return ("200 OK", "text/plain", "write-token".into());
    }
    if path.contains("/reader/api/0/tag/list") {
        return (
            "200 OK",
            "application/json",
            r#"{"tags":[{"id":"user/-/label/RemoteCat","label":"RemoteCat","type":"folder"}]}"#
                .into(),
        );
    }
    if path.contains("/reader/api/0/subscription/list") {
        return (
            "200 OK",
            "application/json",
            format!(
                r#"{{"subscriptions":[{{"id":"feed/10","title":"Remote Feed","categories":[{{"id":"user/-/label/RemoteCat","label":"RemoteCat","type":"folder"}}],"url":"{}"}}]}}"#,
                st.feed_url
            ),
        );
    }
    if path.contains("/reader/api/0/stream/items/ids") {
        if path.contains("reading-list") {
            // ids 通道按数字 id 返回（消费方 `it.id.parse::<i64>()`）；正文里的长 id
            // （tag:...）经 parse_item_id 解析成同一个 500。
            return (
                "200 OK",
                "application/json",
                r#"{"itemRefs":[{"id":"500"}]}"#.into(),
            );
        }
        return ("200 OK", "application/json", r#"{"itemRefs":[]}"#.into());
    }
    if path.contains("/reader/api/0/stream/items/contents") {
        return (
            "200 OK",
            "application/json",
            format!(
                r#"{{"items":[{{"id":"tag:google.com,2005:reader/item/00000000000001f4","title":"Remote Entry","published":1700000000,"alternate":[{{"href":"{}"}}],"origin":{{"streamId":"feed/10"}},"categories":[]}}]}}"#,
                st.entry_url
            ),
        );
    }
    if path.contains("/reader/api/0/edit-tag")
        || path.contains("/reader/api/0/subscription/edit")
        || path.contains("/reader/api/0/subscription/quickadd")
    {
        return ("200 OK", "text/plain", "OK".into());
    }
    // Fever 兼容：任何带 api_key 的 POST（/fever/?api 或 /api/fever.php?api）都回信封。
    if body.contains("api_key") {
        if path.contains("items") {
            return (
                "200 OK",
                "application/json",
                r#"{"api_version":4,"auth":1,"items":[{"id":7001,"feed_id":1,"title":"E","is_saved":0,"is_read":0,"created_on_time":1700000000,"url":"http://127.0.0.1:1/e"}],"unread_item_ids":"","saved_item_ids":""}"#.into(),
            );
        }
        return (
            "200 OK",
            "application/json",
            r#"{"api_version":4,"auth":1,"feeds":[],"groups":[],"feeds_groups":[],"unread_item_ids":"","saved_item_ids":""}"#.into(),
        );
    }
    ("404 Not Found", "text/plain", String::new())
}

fn fresh_db(name: &str) -> (Connection, PathBuf) {
    let tmp = common::unique_db_path(&format!("opt006_{name}"));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).expect("open db");
    (conn, tmp)
}

fn seed_account(conn: &Connection, server: &MockServer, user: &str, pw: &str) {
    db::set_setting(conn, "sync_protocol", "greader").unwrap();
    db::set_setting(conn, "greader_endpoint", &server.base).unwrap();
    db::set_setting(conn, "greader_username", user).unwrap();
    db::set_setting(conn, "greader_password", pw).unwrap();
}

fn seed_local_bound_article(conn: &Connection, server: &MockServer) -> (i64, i64) {
    let folder = db::create_folder(conn, "本地", "article").unwrap();
    let feed = db::insert_feed(
        conn,
        &server.feed_url,
        None,
        "Local Feed",
        None,
        folder,
        "inherit",
        false,
        false,
    )
    .unwrap();
    let a = db::NewArticle {
        guid: "opt006-entry".into(),
        url: Some(server.entry_url.clone()),
        title: "Entry".into(),
        author: None,
        summary: None,
        content_html: None,
        body_text: "body".into(),
        image_url: None,
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: Some(chrono::Utc::now().to_rfc3339()),
        source: "direct".into(),
    };
    let (aid, _) = db::upsert_article_with_feed(conn, feed, &a, false).unwrap();
    (feed, aid)
}

/// 单个测试进程内串行化：PUSH_LOCK / 事件总线是进程级静态，
/// 屏障场景各自驱动完整阶段，串行避免跨测试互相阻塞。
/// （tokio Mutex：跨各测试 runtime 可用，且不会触发 await_holding_lock。）
async fn serialize() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    LOCK.lock().await
}

/* ============================================================
① A 的订阅/分类响应在途 → 断开 → 释放：无内容/绑定复活、无游标写回
============================================================ */

#[tokio::test]
async fn stale_subscription_response_after_disconnect_writes_nothing() {
    let _serial = serialize().await;
    let mut server = start_mock("stale_subs").await;
    let (conn, path) = fresh_db("stale_subs");
    // A 已连接且已有远端数据与游标（断开将清理；旧响应不得复活）
    seed_account(&conn, &server, "user-a", "pw-a");
    let folder = db::create_folder(&conn, "旧远端分类", "article").unwrap();
    let feed = db::insert_feed_origin(
        &conn,
        &server.feed_url,
        None,
        "Old Remote",
        None,
        folder,
        "inherit",
        false,
        false,
        "remote",
    )
    .unwrap();
    db::set_feed_remote_id(&conn, feed, 10).unwrap();
    db::set_last_sync_ts(&conn, 999).unwrap();

    let db = Arc::new(tokio::sync::Mutex::new(conn));
    let http = app_lib::ingestion::build_client(10);
    server.gate.arm("subscription/list");
    server.gate.arm("tag/list");

    let mut fut = Box::pin(app_lib::sync::feeds_phase(&db, &http));
    loop {
        if server.gate.parked_count() >= 2 {
            break;
        }
        tokio::select! {
            Some(_) = server.parked_rx.recv() => {},
            res = &mut fut => panic!("feeds_phase 在屏障前结束：{res:?}"),
        }
    }

    // 断开（生产同一事务内核）：清理 + 代际推进
    {
        let mut conn = db.lock().await;
        app_lib::commands::disconnect_for_test(&mut conn).unwrap();
        assert_eq!(db::sync_generation(&conn).unwrap(), 1);
    }
    server.gate.release();
    fut.await.expect("断开后旧响应应被丢弃，不返回错误假象");

    let conn = db::open(&path).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM feeds WHERE origin = 'remote'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0,
        "旧账号订阅不得复活"
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM feeds WHERE remote_id IS NOT NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0,
        "旧绑定不得复活"
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM folders WHERE name = 'RemoteCat'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0,
        "旧账号分类不得重建"
    );
    assert_eq!(db::last_sync_ts(&conn).unwrap(), 0, "旧游标不得写回");
    let _ = std::fs::remove_file(&path);
}

/* ============================================================
② A 的编辑推送在途 → 切到 B → 释放：后续批次不再发出、B 队列/绑定不变
============================================================ */

#[tokio::test]
async fn stale_push_confirmation_after_account_switch_does_not_touch_new_account() {
    let _serial = serialize().await;
    let mut server_a = start_mock("push_a").await;
    let server_b = start_mock("push_b").await;
    let (conn, path) = fresh_db("stale_push");

    // A：本地直连源已绑定远端 700 + 已读/收藏队列各一条
    seed_account(&conn, &server_a, "user-a", "pw-a");
    let (feed, aid) = seed_local_bound_article(&conn, &server_a);
    db::set_feed_remote_id(&conn, feed, 700).unwrap();
    db::set_article_remote_id(&conn, aid, 700).unwrap();
    db::enqueue_sync(&conn, Some(aid), None, "read", None).unwrap();
    db::enqueue_sync(&conn, Some(aid), None, "star", None).unwrap();

    let db = Arc::new(tokio::sync::Mutex::new(conn));
    let http = app_lib::ingestion::build_client(10);
    server_a.gate.arm("edit-tag");

    let mut fut = Box::pin(app_lib::sync::states_phase(&db, &http, false));
    loop {
        if server_a.gate.parked_count() >= 1 {
            break;
        }
        tokio::select! {
            Some(_) = server_a.parked_rx.recv() => {},
            res = &mut fut => panic!("states_phase 在屏障前结束：{res:?}"),
        }
    }

    // 切到 B（生产同一事务内核：清理 A、换凭据、代际推进），随后为 B 准备自己的
    // 远端绑定与待推队列——旧确认不得触碰它们。
    {
        let mut conn = db.lock().await;
        let generation = app_lib::commands::save_account_for_test(
            &mut conn,
            "greader",
            &server_b.base,
            "user-b",
            "pw-b",
            &server_b.base,
        )
        .unwrap();
        assert!(generation >= 1, "换号必须推进代际");
        db::set_feed_remote_id(&conn, feed, 700).unwrap();
        db::set_article_remote_id(&conn, aid, 700).unwrap();
        db::enqueue_sync(&conn, Some(aid), None, "read", None).unwrap();
    }
    server_a.gate.release();
    let result = fut.await;
    assert!(
        result.is_err(),
        "账号在途切换必须返回明确失效错误，而不是假成功"
    );
    assert_eq!(result.unwrap_err().code, "staleSession");

    // 已发出的首个批次不可撤回（计数 1）；后续批次（star）必须因代际丢失而不再发出。
    assert_eq!(
        server_a.gate.edit_tag_count(),
        1,
        "换号后不得再向旧账号发送后续推送批次：{:?}",
        server_a.gate.request_lines()
    );
    assert!(
        server_b.gate.request_lines().is_empty(),
        "整个旧会话不得接触新账号：{:?}",
        server_b.gate.request_lines()
    );

    let conn = db::open(&path).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM sync_queue", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1,
        "B 的待推队列必须原样（不得被旧确认剪除/标记）"
    );
    let bound: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM articles WHERE remote_id = 700",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(bound, 1, "B 的绑定必须原样");
    assert_eq!(db::last_sync_ts(&conn).unwrap(), 0, "旧会话不得推进游标");
    let _ = std::fs::remove_file(&path);
}

/* ============================================================
③ A 的条目响应在途 → 断开 → 释放：无文章落库、无游标推进（GR 游标）
============================================================ */

#[tokio::test]
async fn stale_items_response_after_disconnect_writes_no_articles_no_cursor() {
    let _serial = serialize().await;
    let mut server = start_mock("stale_items").await;
    let (conn, path) = fresh_db("stale_items");
    seed_account(&conn, &server, "user-a", "pw-a");

    let db = Arc::new(tokio::sync::Mutex::new(conn));
    let http = app_lib::ingestion::build_client(10);
    server.gate.arm("stream/items/contents");

    let mut fut = Box::pin(app_lib::sync::states_phase(&db, &http, true));
    loop {
        if server.gate.parked_count() >= 1 {
            break;
        }
        tokio::select! {
            Some(_) = server.parked_rx.recv() => {},
            res = &mut fut => panic!("states_phase 在屏障前结束：{res:?}"),
        }
    }

    {
        let mut conn = db.lock().await;
        app_lib::commands::disconnect_for_test(&mut conn).unwrap();
    }
    server.gate.release();
    let err = fut.await.expect_err("旧条目响应必须被丢弃并返回失效错误");
    assert_eq!(err.code, "staleSession");

    let conn = db::open(&path).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM articles", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0,
        "旧账号条目不得落库"
    );
    assert_eq!(db::last_sync_ts(&conn).unwrap(), 0, "旧游标不得推进");
    assert_eq!(db::sync_generation(&conn).unwrap(), 1);
    let _ = std::fs::remove_file(&path);
}

/* ============================================================
③b Fever：历史页在途 → 断开 → 释放：历史/条目游标不继承
============================================================ */

#[tokio::test]
async fn stale_fever_page_after_disconnect_keeps_history_unknown() {
    let _serial = serialize().await;
    let mut server = start_mock("stale_fever").await;
    let (conn, path) = fresh_db("stale_fever");
    db::set_setting(&conn, "sync_protocol", "fever").unwrap();
    db::set_setting(&conn, "greader_endpoint", &server.base).unwrap();
    db::set_setting(&conn, "greader_username", "user-f").unwrap();
    db::set_setting(&conn, "greader_password", "pw-f").unwrap();
    db::set_setting(&conn, "sync_last_entry_id", "7000").unwrap();

    let db = Arc::new(tokio::sync::Mutex::new(conn));
    let http = app_lib::ingestion::build_client(10);
    server.gate.arm("with_ids");

    let mut fut = Box::pin(app_lib::sync::states_phase(&db, &http, true));
    loop {
        if server.gate.parked_count() >= 1 {
            break;
        }
        tokio::select! {
            Some(_) = server.parked_rx.recv() => {},
            res = &mut fut => panic!("states_phase 在屏障前结束：{res:?}"),
        }
    }

    {
        let mut conn = db.lock().await;
        app_lib::commands::disconnect_for_test(&mut conn).unwrap();
    }
    server.gate.release();
    let err = fut.await.expect_err("Fever 历史页在断开后必须被丢弃");
    assert_eq!(err.code, "staleSession");

    let conn = db::open(&path).unwrap();
    assert_eq!(
        db::last_sync_entry_id(&conn).unwrap(),
        0,
        "Fever 条目游标重置"
    );
    assert_eq!(
        db::fever_history_state(&conn).unwrap(),
        db::FeverHistoryState::Unknown,
        "新账号不得继承旧 Pending/Complete"
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM articles", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    let _ = std::fs::remove_file(&path);
}

/* ============================================================
④ 配置导入：只落待确认建议；活动凭据不变；B 服务收不到 A 的密码
============================================================ */

#[tokio::test]
async fn config_import_pending_never_sends_old_password_to_new_server() {
    let _serial = serialize().await;
    let server_a = start_mock("import_a").await;
    let server_b = start_mock("import_b").await;
    let (conn, path) = fresh_db("import_pending");
    seed_account(&conn, &server_a, "user-a", "pw-a");

    let payload = app_lib::config_sync::SyncPayload {
        schema: 1,
        uploaded_at: "2026-09-01T00:00:00Z".into(),
        folders: vec![],
        feeds: vec![],
        app_settings: None,
        connection_config: Some(app_lib::config_sync::ConnectionConfig {
            sync_protocol: Some("greader".into()),
            greader_endpoint: Some(server_b.base.clone()),
            greader_username: Some("user-b".into()),
        }),
    };
    let outcome = app_lib::config_sync::apply_payload(&conn, &payload).unwrap();
    assert!(outcome.pending_connection.is_some(), "导入必须落待确认建议");

    // 活动凭据不变（B 地址只存在于建议里）
    assert_eq!(
        db::get_setting(&conn, "greader_endpoint").unwrap().unwrap(),
        server_a.base
    );
    assert_eq!(
        db::get_setting(&conn, "greader_password").unwrap().unwrap(),
        "pw-a"
    );
    let stored = db::get_setting(&conn, "pending_connection_config")
        .unwrap()
        .unwrap();
    assert!(stored.contains("user-b"), "建议已落库待 UI 消费：{stored}");

    // 同步仍走 A：B 服务零请求，且从未见到 A 的密码
    let db = Arc::new(tokio::sync::Mutex::new(conn));
    let http = app_lib::ingestion::build_client(10);
    app_lib::sync::feeds_phase(&db, &http).await.unwrap();
    assert!(
        server_b.gate.request_lines().is_empty(),
        "导入建议不得让同步改道到 B：{:?}",
        server_b.gate.request_lines()
    );
    assert!(
        !server_b.gate.bodies().iter().any(|b| b.contains("pw-a")),
        "B 服务不得收到 A 的密码"
    );

    // 激活入口 = 专用保存流程（重新输入凭据 + 携带建议版本做 CAS）：
    // 代际推进、建议被消费、同步走 B
    let activate_version = {
        let conn = db.lock().await;
        app_lib::config_sync::read_pending_suggestion(&conn)
            .unwrap()
            .expect("建议应可读")
            .version
    };
    app_lib::commands::sync_save_for_test(
        &db,
        &http,
        "greader",
        &server_b.base,
        "user-b",
        "pw-b",
        Some(activate_version),
    )
    .await
    .unwrap();
    {
        let conn = db.lock().await;
        let pending = db::get_setting(&conn, "pending_connection_config")
            .unwrap()
            .unwrap_or_default();
        assert!(
            pending.trim().is_empty(),
            "激活 CAS 命中后必须消费待确认建议：{pending}"
        );
    }
    app_lib::sync::feeds_phase(&db, &http).await.unwrap();
    let b_bodies = server_b.gate.bodies();
    assert!(
        !b_bodies.iter().any(|b| b.contains("pw-a")),
        "激活 B 后 B 服务不得收到 A 的旧密码：{b_bodies:?}"
    );
    assert!(
        b_bodies.iter().any(|b| b.contains("pw-b")),
        "激活 B 后登录应使用新输入的凭据"
    );
    let _ = std::fs::remove_file(&path);
}

/* ============================================================
⑤ 在途 HTTP 不冻结本地读写；失败后无永久 busy/锁泄漏
============================================================ */

#[tokio::test]
async fn in_flight_http_does_not_freeze_local_writes_and_failure_leaves_no_busy() {
    let _serial = serialize().await;
    let mut server = start_mock("freeze").await;
    let (conn, path) = fresh_db("no_freeze");
    seed_account(&conn, &server, "user-a", "pw-a");

    let db = Arc::new(tokio::sync::Mutex::new(conn));
    // 1s 超时 client：屏障不释放 → 有界失败（不靠 sleep 同步）
    let http = app_lib::ingestion::build_client(1);
    server.gate.arm("subscription/list");

    let mut fut = Box::pin(app_lib::sync::feeds_phase(&db, &http));
    loop {
        if server.gate.parked_count() >= 1 {
            break;
        }
        tokio::select! {
            Some(_) = server.parked_rx.recv() => {},
            res = &mut fut => panic!("feeds_phase 在屏障前结束：{res:?}"),
        }
    }

    // HTTP 在飞期间本地写仍可用（锁未跨 await 持有）
    {
        let conn = db.lock().await;
        db::set_setting(&conn, "app_settings", r#"{"themeMode":"dark"}"#).unwrap();
    }

    // 不释放 → 请求 1s 超时；阶段应如实报告失败（不是假成功），且不再有内容落库
    let report = fut
        .await
        .expect("网络失败经 report.errors 上报，返回 Ok(report)");
    assert!(
        report.errors.iter().any(|e| e.contains("拉取订阅失败")),
        "在途失败必须可见：{:?}",
        report.errors
    );
    {
        let conn = db.lock().await;
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM feeds", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0,
            "失败轮不得留下半套订阅"
        );
        // DB 锁没有泄漏：立即可再取用
        db::set_setting(&conn, "app_settings", r#"{"themeMode":"light"}"#).unwrap();
    }
    server.gate.release();

    // 失败之后仍可正常同步（无永久 busy）：干净服务完成一次拉取
    let server2 = start_mock("freeze_ok").await;
    {
        let conn = db.lock().await;
        db::set_setting(&conn, "greader_endpoint", &server2.base).unwrap();
    }
    let report2 = app_lib::sync::feeds_phase(&db, &http).await.unwrap();
    assert!(report2.errors.is_empty(), "服务恢复后同步应正常");
    let _ = std::fs::remove_file(&path);
}

/* ============================================================
⑥ R1 专用激活：fresh password 边界 / 建议版本 CAS / 坏代际
============================================================ */

fn import_pending(
    conn: &rusqlite::Connection,
    server: &MockServer,
    user: &str,
) -> (u64, app_lib::config_sync::ConnectionConfig) {
    let cc = app_lib::config_sync::ConnectionConfig {
        sync_protocol: Some("greader".into()),
        greader_endpoint: Some(server.base.clone()),
        greader_username: Some(user.into()),
    };
    let payload = app_lib::config_sync::SyncPayload {
        schema: 1,
        uploaded_at: "2026-09-01T00:00:00Z".into(),
        folders: vec![],
        feeds: vec![],
        app_settings: None,
        connection_config: Some(cc.clone()),
    };
    app_lib::config_sync::apply_payload(conn, &payload).unwrap();
    let version = app_lib::config_sync::read_pending_suggestion(conn)
        .unwrap()
        .expect("建议应已落库")
        .version;
    (version, cc)
}

/// P1 验收：A 已连接 + **空密码**改换新身份（地址/用户名变化）→ HTTP 前拒绝，
/// 新服务零请求、活动配置不变（旧密码绝不发给新地址）。
#[tokio::test]
async fn empty_password_new_identity_sends_no_request_to_new_server() {
    let _serial = serialize().await;
    let server_a = start_mock("empty_a").await;
    let server_b = start_mock("empty_b").await;
    let (conn, path) = fresh_db("empty_pw_identity");
    seed_account(&conn, &server_a, "user-a", "pw-a");
    let db = Arc::new(tokio::sync::Mutex::new(conn));
    let http = app_lib::ingestion::build_client(10);

    // 直接改 endpoint/用户名 + 空密码（普通保存路径，不携带激活 token）
    let err = app_lib::commands::sync_save_for_test(
        &db,
        &http,
        "greader",
        &server_b.base,
        "user-b",
        "",
        None,
    )
    .await
    .expect_err("新身份空密码必须拒绝");
    assert_eq!(err.code, "validate");
    assert!(err.message.contains("密码"), "{err}");

    // 携带激活 token 时同样拒绝（fresh password 是硬条件）
    let (version, _) = {
        let conn = db.lock().await;
        let (v, cc) = import_pending(&conn, &server_b, "user-b");
        (v, cc)
    };
    let err2 = app_lib::commands::sync_save_for_test(
        &db,
        &http,
        "greader",
        &server_b.base,
        "user-b",
        "",
        Some(version),
    )
    .await
    .expect_err("激活新身份同样必须重新输入密码");
    assert_eq!(err2.code, "validate");

    assert!(
        server_b.gate.request_lines().is_empty(),
        "被拒绝的保存不得向新服务发出任何请求：{:?}",
        server_b.gate.request_lines()
    );
    let conn = db.lock().await;
    assert_eq!(
        db::get_setting(&conn, "greader_endpoint").unwrap().unwrap(),
        server_a.base,
        "活动配置必须保持 A"
    );
    assert_eq!(db::sync_generation(&conn).unwrap(), 0, "拒绝不得推进代际");
    drop(conn);
    let _ = std::fs::remove_file(&path);
}

/// P2 验收：激活 token 的版本与当前建议不符（B 之后又导入 C）→ HTTP 前拒绝：
/// 新服务零请求、新建议 C 保持、活动配置保持 A。
#[tokio::test]
async fn activation_stale_version_rejected_before_http() {
    let _serial = serialize().await;
    let server_a = start_mock("cas_a").await;
    let server_b = start_mock("cas_b").await;
    let (conn, path) = fresh_db("activation_stale");
    seed_account(&conn, &server_a, "user-a", "pw-a");
    let (v_b, _) = import_pending(&conn, &server_b, "user-b");
    // 期间又导入 C（新版本）
    let server_c = start_mock("cas_c").await;
    let (v_c, _) = import_pending(&conn, &server_c, "user-c");
    assert!(v_c > v_b);

    let db = Arc::new(tokio::sync::Mutex::new(conn));
    let http = app_lib::ingestion::build_client(10);
    let err = app_lib::commands::sync_save_for_test(
        &db,
        &http,
        "greader",
        &server_b.base,
        "user-b",
        "pw-b",
        Some(v_b), // 迟到 B 激活
    )
    .await
    .expect_err("迟到激活必须整体拒绝");
    assert_eq!(err.code, "staleActivation");

    assert!(
        server_b.gate.request_lines().is_empty(),
        "版本校验必须在 HTTP 前完成：{:?}",
        server_b.gate.request_lines()
    );
    let conn = db.lock().await;
    let suggestion = app_lib::config_sync::read_pending_suggestion(&conn)
        .unwrap()
        .expect("C 应仍在");
    assert_eq!(suggestion.version, v_c, "新建议 C 不得被消费");
    assert_eq!(
        suggestion.connection.greader_username.as_deref(),
        Some("user-c")
    );
    assert_eq!(
        db::get_setting(&conn, "greader_endpoint").unwrap().unwrap(),
        server_a.base,
        "活动配置保持 A"
    );
    drop(conn);
    let _ = std::fs::remove_file(&path);
}

/// P2 验收（在途交错）：B 激活验证进行中导入 C → CAS 失败整体拒绝：
/// C 保留、A 保留（旧 active 配置不被 B 的迟到提交覆盖）。
#[tokio::test]
async fn late_activation_after_replacement_during_http_keeps_newer_suggestion() {
    let _serial = serialize().await;
    let server_a = start_mock("late_a").await;
    let mut server_b = start_mock("late_b").await;
    let server_c = start_mock("late_c").await;
    let (conn, path) = fresh_db("activation_late");
    seed_account(&conn, &server_a, "user-a", "pw-a");
    let (v_b, _) = import_pending(&conn, &server_b, "user-b");

    let db = Arc::new(tokio::sync::Mutex::new(conn));
    let http = app_lib::ingestion::build_client(10);
    // 屏障：挂起 B 的 ClientLogin——「B 验证中」
    server_b.gate.arm("accounts/ClientLogin");

    let mut fut = Box::pin(app_lib::commands::sync_save_for_test(
        &db,
        &http,
        "greader",
        &server_b.base,
        "user-b",
        "pw-b",
        Some(v_b),
    ));
    loop {
        if server_b.gate.parked_count() >= 1 {
            break;
        }
        tokio::select! {
            Some(_) = server_b.parked_rx.recv() => {},
            res = &mut fut => panic!("激活在屏障前结束：{res:?}"),
        }
    }

    // 验证在途：导入 C（替换建议）
    let v_c = {
        let conn = db.lock().await;
        let (v_c, _) = import_pending(&conn, &server_c, "user-c");
        v_c
    };
    assert!(v_c > v_b);
    server_b.gate.release();

    let err = fut.await.expect_err("CAS 必须拒绝迟到激活");
    assert_eq!(err.code, "staleActivation");

    let conn = db.lock().await;
    let suggestion = app_lib::config_sync::read_pending_suggestion(&conn)
        .unwrap()
        .expect("C 应仍在");
    assert_eq!(suggestion.version, v_c, "C 不得被 B 的迟到提交消费");
    assert_eq!(
        db::get_setting(&conn, "greader_endpoint").unwrap().unwrap(),
        server_a.base,
        "活动配置必须保持 A"
    );
    assert_eq!(db::sync_generation(&conn).unwrap(), 0, "拒绝不得推进代际");
    drop(conn);
    let _ = std::fs::remove_file(&path);
}

/// P2 三态验收：**现存但非法**的代际是可见错误，不做任何 HTTP 请求、
/// 不按 0 与会话假匹配；修好后才恢复同步。
#[tokio::test]
async fn corrupt_generation_is_visible_error_with_zero_requests() {
    let _serial = serialize().await;
    let server = start_mock("bad_gen").await;
    let (conn, path) = fresh_db("bad_generation");
    seed_account(&conn, &server, "user-a", "pw-a");
    db::set_setting(&conn, "sync_generation", "garbage").unwrap();

    let db = Arc::new(tokio::sync::Mutex::new(conn));
    let http = app_lib::ingestion::build_client(10);

    let err = app_lib::sync::feeds_phase(&db, &http)
        .await
        .expect_err("坏代际必须可见失败");
    assert_eq!(err.code, "protocol", "{err}");
    let err2 = app_lib::sync::states_phase(&db, &http, false)
        .await
        .expect_err("坏代际必须阻断 states 阶段");
    assert_eq!(err2.code, "protocol", "{err2}");
    assert!(
        server.gate.request_lines().is_empty(),
        "坏代际不得发起任何 HTTP 请求：{:?}",
        server.gate.request_lines()
    );

    // 账号保存也必须被坏代际阻断（提交前复核不通过）
    {
        let mut conn = db.lock().await;
        let save_err = app_lib::commands::save_account_for_test(
            &mut conn,
            "greader",
            &server.base,
            "user-a",
            "pw-a",
            &server.base,
        )
        .expect_err("坏代际必须阻断账号保存");
        assert_eq!(save_err.code, "protocol");
    }

    // 修复后恢复
    {
        let conn = db.lock().await;
        db::set_setting(&conn, "sync_generation", "2").unwrap();
    }
    let report = app_lib::sync::feeds_phase(&db, &http).await.unwrap();
    assert!(report.errors.is_empty(), "修复代际后同步恢复：{report:?}");
    let _ = std::fs::remove_file(&path);
}

/* ============================================================
⑦ R2 定点：端口属于身份 / 激活必须 fresh 密码 / serial 防 ABA
============================================================ */

fn host_of(base: &str) -> String {
    // "http://127.0.0.1:PORT" → "127.0.0.1"
    let rest = base.split("//").nth(1).unwrap_or(base);
    rest.rsplit_once(':')
        .map(|(h, _)| h)
        .unwrap_or(rest)
        .to_string()
}

fn port_of(base: &str) -> u16 {
    base.rsplit(':').next().unwrap().parse().unwrap()
}

/// R2 ①：**同一 host、仅端口不同**（= 两台不同服务）也是换号——空密码的
/// 普通保存与显式激活都必须在 HTTP 前拒绝，新端口侧零请求、活动配置不变。
#[tokio::test]
async fn port_only_change_with_empty_password_sends_nothing_to_new_port() {
    let _serial = serialize().await;
    let server_a = start_mock("port_a").await;
    let server_b = start_mock("port_b").await;
    // 前置条件：host 相同、端口不同（正是本轮收紧的身份维度）
    assert_eq!(host_of(&server_a.base), host_of(&server_b.base));
    assert_ne!(port_of(&server_a.base), port_of(&server_b.base));

    let (conn, path) = fresh_db("port_identity");
    seed_account(&conn, &server_a, "user-a", "pw-a");
    let db = Arc::new(tokio::sync::Mutex::new(conn));
    let http = app_lib::ingestion::build_client(10);

    // 普通保存：同用户名、仅端口不同 + 空密码 → HTTP 前拒绝
    let err = app_lib::commands::sync_save_for_test(
        &db,
        &http,
        "greader",
        &server_b.base,
        "user-a",
        "",
        None,
    )
    .await
    .expect_err("仅端口变化 + 空密码必须拒绝");
    assert_eq!(err.code, "validate");
    assert!(
        server_b.gate.request_lines().is_empty(),
        "被拒绝的保存不得向新端口发出任何请求：{:?}",
        server_b.gate.request_lines()
    );

    // 显式激活同一条仅端口不同的建议：同样拒绝、零请求
    let version = {
        let conn = db.lock().await;
        import_pending(&conn, &server_b, "user-a").0
    };
    let err2 = app_lib::commands::sync_save_for_test(
        &db,
        &http,
        "greader",
        &server_b.base,
        "user-a",
        "",
        Some(version),
    )
    .await
    .expect_err("激活仅端口变化的新身份 + 空密码必须拒绝");
    assert_eq!(err2.code, "validate");
    assert!(
        server_b.gate.request_lines().is_empty(),
        "激活被拒后新端口仍须零请求：{:?}",
        server_b.gate.request_lines()
    );

    let conn = db.lock().await;
    assert_eq!(
        db::get_setting(&conn, "greader_endpoint").unwrap().unwrap(),
        server_a.base,
        "活动配置保持 A"
    );
    assert_eq!(db::sync_generation(&conn).unwrap(), 0);
    drop(conn);
    let _ = std::fs::remove_file(&path);
}

/// R2 ①：**显式激活即使身份完全相同也必须重新输入密码**（激活凭 fresh
/// credential）；对照——真正的同身份普通保存空密码仍可复用旧密码。
#[tokio::test]
async fn activation_same_identity_still_requires_fresh_password() {
    let _serial = serialize().await;
    let server_a = start_mock("fresh_same").await;
    let (conn, path) = fresh_db("fresh_same_identity");
    seed_account(&conn, &server_a, "user-a", "pw-a");
    let db = Arc::new(tokio::sync::Mutex::new(conn));
    let http = app_lib::ingestion::build_client(10);

    // 建议指向 A 自己（身份相同）
    let version = {
        let conn = db.lock().await;
        import_pending(&conn, &server_a, "user-a").0
    };
    let before = server_a.gate.request_lines().len();
    let err = app_lib::commands::sync_save_for_test(
        &db,
        &http,
        "greader",
        &server_a.base,
        "user-a",
        "",
        Some(version),
    )
    .await
    .expect_err("显式激活必须携带 fresh 密码");
    assert_eq!(err.code, "validate");
    assert!(err.message.contains("密码"), "{err}");
    assert_eq!(
        server_a.gate.request_lines().len(),
        before,
        "被拒激活不得发出请求"
    );
    // 建议保留（激活未成功，不得消费）
    {
        let conn = db.lock().await;
        assert!(
            app_lib::config_sync::read_pending_suggestion(&conn)
                .unwrap()
                .is_some(),
            "被拒激活不得消费建议"
        );
    }

    // 对照：同身份普通保存 + 空密码 → 允许（复用已存密码，正常发请求）
    let ok = app_lib::commands::sync_save_for_test(
        &db,
        &http,
        "greader",
        &server_a.base,
        "user-a",
        "",
        None,
    )
    .await;
    assert!(ok.is_ok(), "同身份普通保存空密码应复用旧密码：{ok:?}");
    let _ = std::fs::remove_file(&path);
}

/// R2 ③（ABA 屏障）：B 激活验证在途时 放弃建议 + 重导同值 —— serial 不重置，
/// 重导发行新 token；迟到激活的旧 token CAS 失败：新建议保留、活动 A 保留。
#[tokio::test]
async fn pending_serial_prevents_aba_with_inflight_activation() {
    let _serial = serialize().await;
    let server_a = start_mock("aba_a").await;
    let mut server_b = start_mock("aba_b").await;
    let (conn, path) = fresh_db("pending_aba");
    seed_account(&conn, &server_a, "user-a", "pw-a");
    let v1 = import_pending(&conn, &server_b, "user-b").0;

    let db = Arc::new(tokio::sync::Mutex::new(conn));
    let http = app_lib::ingestion::build_client(10);
    server_b.gate.arm("accounts/ClientLogin");

    let mut fut = Box::pin(app_lib::commands::sync_save_for_test(
        &db,
        &http,
        "greader",
        &server_b.base,
        "user-b",
        "pw-b",
        Some(v1),
    ));
    loop {
        if server_b.gate.parked_count() >= 1 {
            break;
        }
        tokio::select! {
            Some(_) = server_b.parked_rx.recv() => {},
            res = &mut fut => panic!("激活在屏障前结束：{res:?}"),
        }
    }

    // 验证在途：用户放弃建议，随后重导**同值**建议
    let v2 = {
        let conn = db.lock().await;
        app_lib::config_sync::clear_pending_connection(&conn).unwrap();
        import_pending(&conn, &server_b, "user-b").0
    };
    assert!(
        v2 > v1,
        "清空重导必须发行新 token（serial 不重置，防 ABA）：{v1} -> {v2}"
    );
    server_b.gate.release();

    let err = fut.await.expect_err("旧 token 的迟到激活必须被 CAS 拒绝");
    assert_eq!(err.code, "staleActivation");

    let conn = db.lock().await;
    let suggestion = app_lib::config_sync::read_pending_suggestion(&conn)
        .unwrap()
        .expect("重导的新建议必须保留");
    assert_eq!(suggestion.version, v2, "新 token 的建议不得被旧激活消费");
    assert_eq!(
        db::get_setting(&conn, "greader_endpoint").unwrap().unwrap(),
        server_a.base,
        "活动配置必须保持 A"
    );
    assert_eq!(db::sync_generation(&conn).unwrap(), 0, "拒绝不得推进代际");
    drop(conn);
    let _ = std::fs::remove_file(&path);
}
