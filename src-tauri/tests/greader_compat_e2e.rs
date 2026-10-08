//! OPT-004：Google Reader 鉴权 / 分类对目标服务端契约的**严格夹具**验证。
//!
//! 两个独立夹具按固定上游源码的差异建模（不是共享 mock_greader 的宽松形态）：
//!
//! | 夹具 | GET 认证 | POST 认证 | /token |
//! | --- | --- | --- | --- |
//! | Miniflux 形态 | `Authorization: GoogleLogin auth=<auth>` | **只认表单 `T=<auth>`**（不读 Authorization） | 返回登录 auth 本身 |
//! | FreshRSS 形态 | `Authorization: GoogleLogin auth=<auth>`（所有请求） | 写端点只认 `T=<action-token>`（另一字符串） | 返回与 auth 不同的 token |
//!
//! 夹具只做记录与校验；断言对象是**生产代码**（`GReaderClient` / `sync::feeds_phase`）
//! 发出的请求与产生的结果。本文件只声称「严格夹具 × 固定源码契约」验证，
//! 不代表任何真实部署服务端的实测（见 docs/sync-compat-matrix.md 的版本声明）。

mod common;

use app_lib::db;
use app_lib::greader::GReaderClient;
use app_lib::sync;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/* ============================================================
严格夹具服务端
============================================================ */

/// 登录凭据（夹具只认这一组）。
const USER: &str = "alice";
const PASS: &str = "secret";
/// 登录 auth（ClientLogin 返回）。Miniflux 的 /token 返回它本身。
const AUTH: &str = "alice/abcdef123456";
/// FreshRSS 形态的 action token：与登录 auth **不同**的字符串。
const FRESH_ACTION_TOKEN: &str = "freshrss-action-token-57chars-padded-zzzzzzzzzzzzzzzzzzzzzz";

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Miniflux,
    FreshRss,
}

struct Entry {
    id: i64,
    feed_id: i64,
    title: String,
    read: bool,
    starred: bool,
}

struct Sub {
    id: String,
    title: String,
    url: String,
    /// 分类名（None = 无分类）
    category: Option<String>,
}

/// 一条被夹具记录下来的请求。夹具用它做校验（拒绝不合法请求），
/// 测试用它断言生产客户端「发了什么」。
#[derive(Clone)]
struct Hit {
    method: String,
    path: String,
    authorization: Option<String>,
    form: Vec<(String, String)>,
}

impl Hit {
    fn form_first(&self, key: &str) -> Option<String> {
        self.form
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    }
}

struct StrictServer {
    port: u16,
    mode: Mode,
    entries: Mutex<Vec<Entry>>,
    folders: Mutex<Vec<String>>,
    subscriptions: Mutex<Vec<Sub>>,
    hits: Mutex<Vec<Hit>>,
    /// `/token` 的响应覆盖（状态码 + 响应体）：故障注入用（401/500/404/空体/错误 token）。
    token_override: Mutex<Option<(u16, String)>>,
    /// edit-tag 的响应覆盖（状态码 + 响应体）：P2-1 故障注入用。
    /// 仅当覆盖为「200 + trim 后 OK」时才真正应用状态；其余覆盖返回响应但
    /// **不应用状态**（模拟真实「2xx 但写未生效」）。
    edit_tag_override: Mutex<Option<(u16, String)>>,
}

impl StrictServer {
    fn expected_token(&self) -> &str {
        match self.mode {
            Mode::Miniflux => AUTH,
            Mode::FreshRss => FRESH_ACTION_TOKEN,
        }
    }

    fn expected_authorization(&self) -> String {
        format!("GoogleLogin auth={AUTH}")
    }

    async fn start(mode: Mode) -> Arc<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fixture");
        let port = listener.local_addr().unwrap().port();
        let server = Arc::new(Self {
            port,
            mode,
            entries: Mutex::new(Vec::new()),
            folders: Mutex::new(Vec::new()),
            subscriptions: Mutex::new(Vec::new()),
            hits: Mutex::new(Vec::new()),
            token_override: Mutex::new(None),
            edit_tag_override: Mutex::new(None),
        });
        let srv = server.clone();
        tokio::spawn(async move {
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(s) => s,
                    Err(_) => break,
                };
                let srv = srv.clone();
                tokio::spawn(async move {
                    let _ = handle_conn(stream, srv).await;
                });
            }
        });
        server
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn add_entry(&self, id: i64, feed_id: i64, title: &str) {
        self.entries.lock().unwrap().push(Entry {
            id,
            feed_id,
            title: title.to_string(),
            read: false,
            starred: false,
        });
    }

    fn is_read(&self, id: i64) -> bool {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.read)
            .unwrap_or(false)
    }

    fn is_starred(&self, id: i64) -> bool {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.starred)
            .unwrap_or(false)
    }

    fn set_folders(&self, folders: &[&str]) {
        *self.folders.lock().unwrap() = folders.iter().map(|f| f.to_string()).collect();
    }

    fn set_subscriptions(&self, subs: Vec<(&str, &str, &str, Option<&str>)>) {
        *self.subscriptions.lock().unwrap() = subs
            .into_iter()
            .map(|(id, title, url, cat)| Sub {
                id: id.to_string(),
                title: title.to_string(),
                url: url.to_string(),
                category: cat.map(str::to_string),
            })
            .collect();
    }

    fn hits(&self) -> Vec<Hit> {
        self.hits.lock().unwrap().clone()
    }

    fn token_request_count(&self) -> usize {
        self.hits
            .lock()
            .unwrap()
            .iter()
            .filter(|h| h.path.ends_with("/reader/api/0/token"))
            .count()
    }

    fn edit_tag_hits(&self) -> Vec<Hit> {
        self.hits
            .lock()
            .unwrap()
            .iter()
            .filter(|h| h.method == "POST" && h.path.ends_with("/reader/api/0/edit-tag"))
            .cloned()
            .collect()
    }

    fn set_token_override(&self, status: u16, body: &str) {
        *self.token_override.lock().unwrap() = Some((status, body.to_string()));
    }

    /// edit-tag 响应覆盖（P2-1 反例：200+FAIL / 200+空体 等）。
    fn set_edit_tag_override(&self, status: u16, body: &str) {
        *self.edit_tag_override.lock().unwrap() = Some((status, body.to_string()));
    }

    fn clear_edit_tag_override(&self) {
        *self.edit_tag_override.lock().unwrap() = None;
    }

    /// 远端订阅当前的分类名（None = 无分类/默认分类）。
    fn subscription_category(&self, id: &str) -> Option<String> {
        self.subscriptions
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.id == id)
            .and_then(|s| s.category.clone())
    }

    fn subscription_title(&self, id: &str) -> Option<String> {
        self.subscriptions
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.title.clone())
    }

    fn subscription_edit_hits(&self) -> Vec<Hit> {
        self.hits
            .lock()
            .unwrap()
            .iter()
            .filter(|h| h.method == "POST" && h.path.ends_with("/reader/api/0/subscription/edit"))
            .cloned()
            .collect()
    }
}

async fn handle_conn(mut stream: TcpStream, srv: Arc<StrictServer>) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = find_header_end(&buf) {
            let head = String::from_utf8_lossy(&buf[..pos]).to_string();
            let content_len = head
                .lines()
                .find(|l| l.to_ascii_lowercase().starts_with("content-length"))
                .and_then(|l| l.split(':').nth(1))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if buf.len() >= pos + 4 + content_len {
                break;
            }
        }
    }

    let header_end = find_header_end(&buf).unwrap_or(buf.len());
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let body = String::from_utf8_lossy(&buf[header_end + 4..]).to_string();

    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path_query = parts.next().unwrap_or("");
    let path = path_query.split('?').next().unwrap_or("").to_string();

    let mut headers: HashMap<String, String> = HashMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let authorization = headers.get("authorization").cloned();

    let hit = Hit {
        method: method.clone(),
        path: path.clone(),
        authorization,
        form: if body.is_empty() {
            Vec::new()
        } else {
            parse_form(&body)
        },
    };
    srv.hits.lock().unwrap().push(hit.clone());

    let (status, response) = route(&srv, &hit);
    write_resp(&mut stream, status, &response).await
}

/// 路由 + 严格校验。返回 (状态码, 响应体)。
fn route(srv: &StrictServer, hit: &Hit) -> (u16, String) {
    let method = hit.method.as_str();
    let path = hit.path.as_str();

    // ClientLogin：表单凭据换取 auth；两种形态的响应体格式不同。
    if method == "POST" && path.ends_with("/accounts/ClientLogin") {
        let email = hit.form_first("Email").unwrap_or_default();
        let passwd = hit.form_first("Passwd").unwrap_or_default();
        if email == USER && passwd == PASS {
            return match srv.mode {
                Mode::Miniflux => (
                    200,
                    format!(r#"{{"SID":"{AUTH}","LSID":"{AUTH}","Auth":"{AUTH}"}}"#),
                ),
                Mode::FreshRss => (200, format!("SID={AUTH}\nLSID=null\nAuth={AUTH}\n")),
            };
        }
        return (401, "Unauthorized!".into());
    }

    // 其余端点一律要求 Authorization 头（两种形态都靠它设用户上下文；
    // Miniflux 的 POST 真实实现虽不读它，但客户端契约要求所有请求都带——
    // 这里强制要求，缺失即失败，防止客户端实现回退）。
    if hit.authorization.as_deref() != Some(srv.expected_authorization().as_str()) {
        return (401, "Unauthorized!".into());
    }

    // action token 端点。覆盖注入优先（故障/空体/错误 token 用）。
    if method == "GET" && path.ends_with("/reader/api/0/token") {
        if let Some((status, body)) = srv.token_override.lock().unwrap().clone() {
            return (status, body);
        }
        return (200, srv.expected_token().to_string());
    }

    // 写端点的 T 校验：Miniflux 认登录 auth，FreshRSS 认 /token 换来的 action token。
    let is_write = path.ends_with("/reader/api/0/edit-tag")
        || path.ends_with("/reader/api/0/subscription/edit");
    if method == "POST" && is_write && hit.form_first("T").as_deref() != Some(srv.expected_token())
    {
        return (401, "Unauthorized!".into());
    }
    // Miniflux 对**所有** POST 都要求 T=auth（middleware 先于路由）；
    // FreshRSS 只在校验写端点时看 T，item_contents 忽略 T。
    if method == "POST"
        && srv.mode == Mode::Miniflux
        && !path.ends_with("/accounts/ClientLogin")
        && hit.form_first("T").as_deref() != Some(AUTH)
    {
        return (401, "Unauthorized!".into());
    }

    /* ---------- 读端点 ---------- */
    if method == "GET" && path.ends_with("/reader/api/0/subscription/list") {
        let subs = srv.subscriptions.lock().unwrap();
        let arr: Vec<serde_json::Value> = subs
            .iter()
            .map(|s| {
                let categories: Vec<serde_json::Value> = match (&s.category, srv.mode) {
                    (Some(cat), Mode::Miniflux) => {
                        serde_json::json!([{ "id": format!("user/-/label/{cat}"), "label": cat, "type": "folder" }])
                            .as_array()
                            .cloned()
                            .unwrap_or_default()
                    }
                    // FreshRSS 形态：只有 id + label，没有 type（固定源码 219eaf58 的 subscriptionList）
                    (Some(cat), Mode::FreshRss) => {
                        serde_json::json!([{ "id": format!("user/-/label/{cat}"), "label": cat }])
                            .as_array()
                            .cloned()
                            .unwrap_or_default()
                    }
                    (None, _) => Vec::new(),
                };
                serde_json::json!({
                    "id": s.id,
                    "title": s.title,
                    "url": s.url,
                    "categories": categories,
                })
            })
            .collect();
        return (200, serde_json::json!({ "subscriptions": arr }).to_string());
    }
    if method == "GET" && path.ends_with("/reader/api/0/tag/list") {
        let folders = srv.folders.lock().unwrap();
        // 状态 tag 两种形态都存在且都**没有 type**——绝不能变成目录。
        let mut tags: Vec<serde_json::Value> = vec![
            serde_json::json!({"id": "user/-/state/com.google/starred"}),
            serde_json::json!({"id": "user/-/state/com.google/reading-list"}),
        ];
        if srv.mode == Mode::FreshRss {
            tags.push(serde_json::json!({"id": "user/-/state/org.freshrss/main"}));
        }
        for f in folders.iter() {
            tags.push(match srv.mode {
                // Miniflux：folder tag 带 label
                Mode::Miniflux => serde_json::json!({
                    "id": format!("user/-/label/{f}"), "label": f, "type": "folder"
                }),
                // FreshRSS：folder tag 只有 {id, type}，**没有 label**（从 id 后缀取名）
                Mode::FreshRss => serde_json::json!({
                    "id": format!("user/-/label/{f}"), "type": "folder"
                }),
            });
        }
        return (200, serde_json::json!({ "tags": tags }).to_string());
    }

    /* ---------- 条目端点 ---------- */
    if method == "POST" && path.ends_with("/reader/api/0/stream/items/contents") {
        let ids: Vec<i64> = hit
            .form
            .iter()
            .filter(|(k, _)| k == "i")
            .filter_map(|(_, v)| v.parse::<i64>().ok())
            .collect();
        let entries = srv.entries.lock().unwrap();
        let items: Vec<serde_json::Value> = entries
            .iter()
            .filter(|e| ids.contains(&e.id))
            .map(|e| {
                let mut cats = vec!["user/-/state/com.google/reading-list".to_string()];
                if e.read {
                    cats.push("user/-/state/com.google/read".to_string());
                }
                if e.starred {
                    cats.push("user/-/state/com.google/starred".to_string());
                }
                serde_json::json!({
                    "id": format!("tag:google.com,2005:reader/item/{:016x}", e.id),
                    "categories": cats,
                    "title": e.title,
                    "published": 1700000000,
                    "alternate": [{"href": format!("http://example.com/e/{}", e.id), "type": "text/html"}],
                    "summary": {"content": "fixture"},
                    "content": {"content": "fixture"},
                    "origin": {"streamId": format!("feed/{}", e.feed_id)},
                })
            })
            .collect();
        return (200, serde_json::json!({ "items": items }).to_string());
    }

    /* ---------- 写端点 ---------- */
    if method == "POST" && path.ends_with("/reader/api/0/edit-tag") {
        // P2-1 覆盖注入：只有「200 + trim 后 OK」才继续应用状态；
        // 其余（200+FAIL、200+空体、非 200）返回覆盖但**不应用状态**——
        // 模拟真实「2xx 但写未生效」，供测试断言客户端必须报错且队列不 prune。
        let overridden = srv.edit_tag_override.lock().unwrap().clone();
        if let Some((status, body)) = &overridden {
            if *status != 200 || body.trim() != "OK" {
                return (*status, body.clone());
            }
        }
        let ids: Vec<i64> = hit
            .form
            .iter()
            .filter(|(k, _)| k == "i")
            .filter_map(|(_, v)| v.parse::<i64>().ok())
            .collect();
        let add: Vec<String> = hit
            .form
            .iter()
            .filter(|(k, _)| k == "a")
            .map(|(_, v)| v.clone())
            .collect();
        let remove: Vec<String> = hit
            .form
            .iter()
            .filter(|(k, _)| k == "r")
            .map(|(_, v)| v.clone())
            .collect();
        let mut entries = srv.entries.lock().unwrap();
        for e in entries.iter_mut() {
            if !ids.contains(&e.id) {
                continue;
            }
            for tag in &add {
                if tag.ends_with("/read") {
                    e.read = true;
                }
                if tag.ends_with("/starred") {
                    e.starred = true;
                }
            }
            for tag in &remove {
                if tag.ends_with("/read") {
                    e.read = false;
                }
                if tag.ends_with("/starred") {
                    e.starred = false;
                }
            }
        }
        return match overridden {
            Some((_, body)) => (200, body),
            None => (200, "OK".into()),
        };
    }
    if method == "POST" && path.ends_with("/reader/api/0/subscription/quickadd") {
        return (
            200,
            serde_json::json!({"numResults": 1, "streamId": "feed/99"}).to_string(),
        );
    }
    if method == "POST" && path.ends_with("/reader/api/0/subscription/edit") {
        let ac = hit.form_first("ac").unwrap_or_default();
        if ac == "edit" {
            let s = hit.form_first("s").unwrap_or_default();
            let mut subs = srv.subscriptions.lock().unwrap();
            if let Some(sub) = subs.iter_mut().find(|sub| sub.id == s) {
                if let Some(title) = hit.form_first("t").filter(|t| !t.is_empty()) {
                    sub.title = title;
                }
                if let Some(a) = hit.form_first("a") {
                    // 真实 wire 规范：`a` 是 **stream id**（FreshRSS 按
                    // `user/-/label/` 或 `user/<user>/label/` 前缀解析名字）。
                    // 裸名不是有效 stream id——FreshRSS 会落到默认分类，
                    // 夹具据此不把它当成目标分类（旧实现发裸名时断言必红）。
                    sub.category = label_name_from_stream_id(&a).map(str::to_string);
                }
            }
        }
        return (200, "OK".into());
    }

    (404, r#"{"error_message":"not found"}"#.into())
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

async fn write_resp(stream: &mut TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        401 => "Unauthorized",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Error",
    };
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(resp.as_bytes()).await?;
    stream.flush().await
}

fn parse_form(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for kv in body.split('&') {
        if let Some((k, v)) = kv.split_once('=') {
            out.push((k.to_string(), url_decode(v)));
        }
    }
    out
}

/// 从 label stream id（`user/.../label/<名>`）解析目录名；裸名/其它前缀返回 None。
/// 与生产 `greader::category_label_from_id` 同规则（夹具不依赖生产代码，独立实现）。
fn label_name_from_stream_id(id: &str) -> Option<&str> {
    let rest = id.strip_prefix("user/")?;
    let (_, name) = rest.split_once("/label/")?;
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

fn url_decode(s: &str) -> String {
    // 逐字节解码 %XX 后整体按 UTF-8 解释（中文分类名等），与 mock_greader 同口径。
    let mut out: Vec<u8> = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/* ============================================================
Miniflux 形态（POST 只认 T=登录 auth）
============================================================ */

/// 生产客户端在 Miniflux 严格夹具上的完整写/读往返：
/// item_contents + 四个状态写操作都真实到达；Authorization 头全程携带；
/// /token 只取一次；POST 的 T 是登录 auth 本身。
#[tokio::test]
async fn miniflux_strict_reads_and_writes_reach_server_with_single_token_fetch() {
    let srv = StrictServer::start(Mode::Miniflux).await;
    srv.add_entry(1, 10, "A");
    srv.add_entry(2, 10, "B");
    srv.set_subscriptions(vec![("feed/10", "A 源", "http://example.com/a.xml", None)]);
    let http = reqwest::Client::new();
    let client = GReaderClient::login_resolved(&srv.url(), USER, PASS, http)
        .await
        .expect("Miniflux 形态登录应成功");

    let items = client.item_contents(&[1, 2]).await.expect("拉正文应成功");
    assert_eq!(items.len(), 2, "两条条目都应返回");

    client.mark_read(&[1]).await.expect("标读应成功");
    assert!(srv.is_read(1), "标读必须真实到达服务端");
    client.mark_unread(&[1]).await.expect("标未读应成功");
    assert!(!srv.is_read(1), "标未读必须真实到达服务端");
    client.mark_starred(&[2]).await.expect("收藏应成功");
    assert!(srv.is_starred(2), "收藏必须真实到达服务端");
    client.mark_unstarred(&[2]).await.expect("取消收藏应成功");
    assert!(!srv.is_starred(2), "取消收藏必须真实到达服务端");

    // P2-2：写分类的 `a` 是完整 stream id，夹具按真实 wire 规范实际移动分类。
    client
        .edit_subscription(10, Some("重命名"), Some("科技"))
        .await
        .expect("移动分类应成功");
    assert_eq!(
        srv.subscription_category("feed/10").as_deref(),
        Some("科技"),
        "夹具必须实际改分类（仅 200+OK 不足以证明写生效）"
    );
    let sub_edits = srv.subscription_edit_hits();
    assert_eq!(sub_edits.len(), 1);
    assert_eq!(
        sub_edits[0].form_first("a").as_deref(),
        Some("user/-/label/科技"),
        "Miniflux 同样要求完整 stream id"
    );
    assert_eq!(sub_edits[0].form_first("T").as_deref(), Some(AUTH));
    assert_eq!(srv.subscription_title("feed/10").as_deref(), Some("重命名"));

    // 同一客户端 6 次 POST（contents + 4 次 edit-tag + subscription/edit）只取一次 token。
    assert_eq!(
        srv.token_request_count(),
        1,
        "同一客户端只应获取一次 action token"
    );

    // 所有非登录请求都带 Authorization（生产契约要求，Mock 只记录不判定）。
    let expected = srv.expected_authorization();
    for h in srv
        .hits()
        .iter()
        .filter(|h| !h.path.ends_with("/accounts/ClientLogin"))
    {
        assert_eq!(
            h.authorization.as_deref(),
            Some(expected.as_str()),
            "{} {} 应携带 Authorization",
            h.method,
            h.path
        );
    }
    // Miniflux 的 T = 登录 auth 本身（/token 返回 auth）。
    for h in srv.edit_tag_hits() {
        assert_eq!(
            h.form_first("T").as_deref(),
            Some(AUTH),
            "Miniflux 写请求的 T 应等于登录 auth"
        );
    }
}

/// 错 token 必须失败且不得假成功：auth 与 T 都不对时服务端 401，
/// 客户端报错、夹具不产生任何状态变更。
#[tokio::test]
async fn miniflux_strict_wrong_credentials_fail_without_state_change() {
    let srv = StrictServer::start(Mode::Miniflux).await;
    srv.add_entry(1, 10, "A");
    let http = reqwest::Client::new();
    let bad = GReaderClient::new(&srv.url(), "alice/wrong-token", http.clone());

    let err = bad.item_contents(&[1]).await.unwrap_err();
    assert!(
        err.to_string().contains("action token"),
        "错误应指明 action token 获取失败，实际：{err}"
    );
    let err = bad.mark_read(&[1]).await.unwrap_err();
    assert!(err.to_string().contains("action token"), "实际：{err}");
    assert!(!srv.is_read(1), "认证失败不得在服务端产生状态变更");
    assert!(
        srv.edit_tag_hits().is_empty(),
        "未认证的写请求不应到达 edit-tag"
    );

    // 服务端返回错误 token（≠auth）时：客户端忠实用它做 T → 读端点 401 → 必须报错，
    // 不得回退成 auth 再试或把 401 吞成成功。
    srv.set_token_override(200, "bogus-token");
    let good_auth = GReaderClient::new(&srv.url(), AUTH, http);
    let err = good_auth.mark_read(&[1]).await.unwrap_err();
    assert!(
        err.to_string().contains("401"),
        "错 token 的写请求应报 401，实际：{err}"
    );
    let edits = srv.edit_tag_hits();
    assert_eq!(edits.len(), 1, "应恰有一次被拒的写尝试");
    assert_eq!(
        edits[0].form_first("T").as_deref(),
        Some("bogus-token"),
        "客户端应使用 /token 返回的 token，而不是自行改猜"
    );
    assert!(!srv.is_read(1), "被拒的写不得产生状态变更");
}

/// token 获取失败的边界：401/500/空体/网络失败一律明确报错且不回退；
/// **只有明确 404** 才回退用登录 auth 当 T。
#[tokio::test]
async fn token_fetch_failure_modes_and_404_only_fallback() {
    let srv = StrictServer::start(Mode::Miniflux).await;
    srv.add_entry(1, 10, "A");
    let http = reqwest::Client::new();
    let client = GReaderClient::new(&srv.url(), AUTH, http.clone());

    // 401：认证问题 → 报错，不碰写端点
    srv.set_token_override(401, "Unauthorized!");
    let err = client.mark_read(&[1]).await.unwrap_err();
    assert!(err.to_string().contains("action token"), "实际：{err}");
    assert!(err.to_string().contains("401"), "应带状态码，实际：{err}");
    assert!(srv.edit_tag_hits().is_empty(), "token 失败不得继续发写请求");

    // 500：服务端错误 → 报错，不回退
    srv.set_token_override(500, "boom");
    let err = client.mark_read(&[1]).await.unwrap_err();
    assert!(err.to_string().contains("500"), "实际：{err}");
    assert!(srv.edit_tag_hits().is_empty());

    // 200 但空体 → 明确「空 token」错误，不得发空 T
    srv.set_token_override(200, "");
    let err = client.mark_read(&[1]).await.unwrap_err();
    assert!(err.to_string().contains("空 token"), "实际：{err}");
    assert!(srv.edit_tag_hits().is_empty());

    // 404：实现没有 /token → 唯一允许的回退：用登录 auth 当 T
    srv.set_token_override(404, "not found");
    client
        .mark_read(&[1])
        .await
        .expect("404 应回退用 auth 并成功");
    assert!(srv.is_read(1), "回退后的写必须真实到达");
    let edits = srv.edit_tag_hits();
    assert_eq!(edits.len(), 1);
    assert_eq!(
        edits[0].form_first("T").as_deref(),
        Some(AUTH),
        "404 回退应使用登录 auth 当 T"
    );

    // 网络失败：端口无人监听 → 明确网络错误（同样不静默、不回退）。
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let dead_port = listener.local_addr().unwrap().port();
    drop(listener);
    let offline = GReaderClient::new(&format!("http://127.0.0.1:{dead_port}"), AUTH, http);
    let err = offline.mark_read(&[1]).await.unwrap_err();
    assert!(
        err.to_string().contains("action token") && err.to_string().contains("网络错误"),
        "实际：{err}"
    );
}

/* ============================================================
FreshRSS 形态（所有请求要 Auth，写要 T=action token）
============================================================ */

/// FreshRSS 严格夹具：/token 返回与 auth 不同的 action token；
/// 写请求的 T 必须是它——夹具对不合法 T 一律 401，所以成功即证明职责分离生效。
#[tokio::test]
async fn freshrss_strict_writes_use_action_token_from_token_endpoint() {
    let srv = StrictServer::start(Mode::FreshRss).await;
    srv.add_entry(1, 10, "A");
    srv.add_entry(2, 10, "B");
    let http = reqwest::Client::new();
    // FreshRSS 的 ClientLogin 是经典文本形态（忽略 output=json）。
    let client = GReaderClient::login_resolved(&srv.url(), USER, PASS, http)
        .await
        .expect("FreshRSS 文本形态登录应成功");

    assert!(
        client.resolved_base().starts_with("http://127.0.0.1"),
        "解析后的 base 应为夹具地址"
    );

    let items = client.item_contents(&[1, 2]).await.expect("拉正文应成功");
    assert_eq!(items.len(), 2);

    client.mark_read(&[1]).await.expect("标读应成功");
    assert!(srv.is_read(1));
    client.mark_unread(&[1]).await.expect("标未读应成功");
    assert!(!srv.is_read(1));
    client.mark_starred(&[2]).await.expect("收藏应成功");
    assert!(srv.is_starred(2));
    client.mark_unstarred(&[2]).await.expect("取消收藏应成功");
    assert!(!srv.is_starred(2));

    assert_eq!(srv.token_request_count(), 1, "action token 只应取一次");

    // 写请求的 T = /token 返回的 action token（≠ 登录 auth）——这正是职责分离的锁。
    let edits = srv.edit_tag_hits();
    assert_eq!(edits.len(), 4, "四次状态写都应到达 edit-tag");
    for h in edits {
        assert_eq!(
            h.form_first("T").as_deref(),
            Some(FRESH_ACTION_TOKEN),
            "FreshRSS 写请求的 T 应等于 /token 换来的 action token"
        );
        assert_ne!(
            h.form_first("T").as_deref(),
            Some(AUTH),
            "T 不得是登录 auth"
        );
    }
    // 所有请求（含 POST）都带 Authorization——夹具对缺失一律 401，通过即证明。
    for h in srv.hits() {
        if h.path.ends_with("/accounts/ClientLogin") {
            continue;
        }
        assert!(
            h.authorization.as_deref() == Some(srv.expected_authorization().as_str()),
            "{} {} 应携带 Authorization",
            h.method,
            h.path
        );
    }

    // 凭据错误（auth 放错）→ 401 失败，不产生状态变更。
    let bad = GReaderClient::new(&srv.url(), "alice/wrong-token", reqwest::Client::new());
    let err = bad.mark_read(&[2]).await.unwrap_err();
    assert!(err.to_string().contains("action token"), "实际：{err}");
    assert!(!srv.is_read(2), "认证失败不得产生状态变更");
}

/// FreshRSS 形态的分类数据（tag 只有 {id,type:folder}、subscription category
/// 只有 {id,label} 无 type）经 `sync::feeds_phase` 同步后：
/// 分类名（含中文与斜杠）完整、归属正确、state tag 不建目录。
#[tokio::test]
async fn freshrss_shape_categories_sync_correctly_without_state_tag_pollution() {
    let srv = StrictServer::start(Mode::FreshRss).await;
    srv.set_folders(&["技术/阅读", "空目录", "备用"]);
    srv.set_subscriptions(vec![
        (
            "feed/10",
            "A 源",
            "http://example.com/a.xml",
            Some("技术/阅读"),
        ),
        (
            "feed/11",
            "B 源",
            "http://example.com/b.xml",
            Some("空目录"),
        ),
        ("feed/12", "C 源", "http://example.com/c.xml", None),
    ]);

    let tmp = common::unique_db_path("compat_freshrss_cat");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).expect("open db");
    db::set_setting(&conn, "greader_endpoint", &srv.url()).unwrap();
    db::set_setting(&conn, "greader_username", USER).unwrap();
    db::set_setting(&conn, "greader_password", PASS).unwrap();
    let db = Arc::new(tokio::sync::Mutex::new(conn));
    let http = reqwest::Client::new();

    let report = sync::feeds_phase(&db, &http)
        .await
        .expect("feeds 阶段应成功");
    assert!(
        report.errors.is_empty(),
        "拉取分类/订阅不应有错误：{:?}",
        report.errors
    );
    assert_eq!(report.pulled_feeds, 3, "三个订阅都应建本地");

    let conn = db.lock().await;
    // 分类：标签里的三个目录（含 tag/list 独有、无订阅挂载的「备用」）都被创建，
    // 斜杠与中文原样保留（一个目录，不是 技术 / 阅读 两层）。
    let fid_tech = db::find_folder_by_name(&conn, "技术/阅读")
        .unwrap()
        .expect("「技术/阅读」应作为单个目录存在");
    let fid_empty = db::find_folder_by_name(&conn, "空目录")
        .unwrap()
        .expect("「空目录」");
    let fid_backup = db::find_folder_by_name(&conn, "备用")
        .unwrap()
        .expect("「备用」");
    assert!(
        db::find_folder_by_name(&conn, "技术").unwrap().is_none(),
        "斜杠不得被拆层"
    );
    assert!(
        db::find_folder_by_name(&conn, "阅读").unwrap().is_none(),
        "斜杠不得被拆层"
    );

    // state tag 不得污染目录：starred / reading-list / org.freshrss/main 都不建目录。
    let bad_folders: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM folders WHERE name IN ('starred','reading-list','org.freshrss','main')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(bad_folders, 0, "state tag 不得创建目录");

    // 归属：A→技术/阅读、B→空目录、C→未分类。
    let folder_of = |url: &str| -> i64 {
        conn.query_row(
            "SELECT folder_id FROM feeds WHERE feed_url = ?1",
            [url],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(folder_of("http://example.com/a.xml"), fid_tech);
    assert_eq!(folder_of("http://example.com/b.xml"), fid_empty);
    let fid_uncat = db::find_folder_by_name(&conn, "未分类")
        .unwrap()
        .expect("无分类订阅应落入「未分类」");
    assert_eq!(folder_of("http://example.com/c.xml"), fid_uncat);

    // tag/list 独有目录「备用」确有目录 id；远端 feed 数字 id 已绑定。
    assert!(fid_backup > 0);
    let remote_id: Option<i64> = conn
        .query_row(
            "SELECT remote_id FROM feeds WHERE feed_url = ?1",
            ["http://example.com/a.xml"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(remote_id, Some(10), "远端 feed/10 应绑定到本地源");

    drop(conn);
    let _ = std::fs::remove_file(&tmp);
}

/// Miniflux 原形态回归：folder tag 带 label + type、subscription category 带 type，
/// 经 feeds_phase 后分类与归属不变。
#[tokio::test]
async fn miniflux_shape_categories_no_regression() {
    let srv = StrictServer::start(Mode::Miniflux).await;
    srv.set_folders(&["Default"]);
    srv.set_subscriptions(vec![(
        "feed/10",
        "A 源",
        "http://example.com/a.xml",
        Some("Default"),
    )]);

    let tmp = common::unique_db_path("compat_miniflux_cat");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).expect("open db");
    db::set_setting(&conn, "greader_endpoint", &srv.url()).unwrap();
    db::set_setting(&conn, "greader_username", USER).unwrap();
    db::set_setting(&conn, "greader_password", PASS).unwrap();
    let db = Arc::new(tokio::sync::Mutex::new(conn));
    let http = reqwest::Client::new();

    let report = sync::feeds_phase(&db, &http)
        .await
        .expect("feeds 阶段应成功");
    assert!(report.errors.is_empty(), "{:?}", report.errors);

    let conn = db.lock().await;
    let fid = db::find_folder_by_name(&conn, "Default")
        .unwrap()
        .expect("Miniflux folder 应按 label 建目录");
    let feed_folder: i64 = conn
        .query_row(
            "SELECT folder_id FROM feeds WHERE feed_url = ?1",
            ["http://example.com/a.xml"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(feed_folder, fid, "订阅应归属 Default 分类");
    let polluted: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM folders WHERE name IN ('starred','reading-list')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(polluted, 0, "state tag 不得建目录");

    drop(conn);
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
R1 返工反例（P2-1 写响应体 / P2-2 完整 stream id）
============================================================ */

/// P2-1：写端点回 200 但正文不是 OK（FAIL / 空体）时，客户端必须报错（脱敏），
/// 且经 `push_states_now` 验证「失败不假成功、队列不被 prune」。
#[tokio::test]
async fn write_2xx_non_ok_body_fails_and_queue_survives() {
    let srv = StrictServer::start(Mode::Miniflux).await;
    srv.add_entry(1, 10, "A");
    srv.set_edit_tag_override(200, "FAIL");

    // DB：绑定远端 entry 1 的文章 + 一条待推送的 read。
    let tmp = common::unique_db_path("compat_write_body");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).expect("open db");
    db::set_setting(&conn, "greader_endpoint", &srv.url()).unwrap();
    db::set_setting(&conn, "greader_username", USER).unwrap();
    db::set_setting(&conn, "greader_password", PASS).unwrap();
    let _aid = {
        let folder = db::create_folder(&conn, "分类", "article").unwrap();
        let feed_id = db::insert_feed(
            &conn,
            "http://example.com/w.xml",
            None,
            "W",
            None,
            folder,
            "inherit",
            true,
            false,
        )
        .unwrap();
        let a = db::NewArticle {
            guid: "g-w".into(),
            url: Some("http://example.com/w/1".into()),
            title: "W".into(),
            author: None,
            summary: None,
            content_html: None,
            body_text: String::new(),
            image_url: None,
            enclosure_url: None,
            enclosure_mime: None,
            duration_sec: None,
            published_at: None,
            source: "remote".into(),
        };
        let id = db::upsert_article_with_feed(&conn, feed_id, &a, false)
            .unwrap()
            .0;
        db::set_article_remote_id(&conn, id, 1).unwrap();
        db::enqueue_sync(&conn, Some(id), None, "read", None).unwrap();
        id
    };
    let db = Arc::new(tokio::sync::Mutex::new(conn));
    let http = reqwest::Client::new();

    sync::push_states_now(&db, &http).await;

    assert_eq!(srv.edit_tag_hits().len(), 1, "应发出一次 edit-tag");
    assert!(!srv.is_read(1), "200+FAIL 不得被当成写成功应用");
    {
        let conn = db.lock().await;
        let left: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_queue WHERE action='read'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(left, 1, "写失败时队列不得被 prune（否则用户意图静默丢失）");
    }

    // 直接客户端面：错误必须是脱敏协议错误（不把响应体正文带进错误）。
    let client = GReaderClient::new(&srv.url(), AUTH, reqwest::Client::new());
    let err = client.mark_read(&[1]).await.unwrap_err();
    let text = err.to_string();
    assert!(text.contains("OK"), "应说明响应体不是 OK，实际：{text}");
    assert!(!text.contains("FAIL"), "错误不得回显响应体：{text}");

    // 200 + 空体：同样失败、同样不应用状态。
    srv.set_edit_tag_override(200, "");
    let err = client.mark_read(&[1]).await.unwrap_err();
    assert!(
        err.to_string().contains("OK"),
        "空体应报协议错误，实际：{err}"
    );
    assert!(!srv.is_read(1));

    // 允许周围空白：夹具按 wire 规范仍应用状态。
    srv.set_edit_tag_override(200, " OK \n");
    client.mark_read(&[1]).await.expect("trim 后为 OK 应成功");
    assert!(srv.is_read(1));

    // 清除覆盖走真实成功路径：队列应被 prune（严格校验不破坏正常成功语义）。
    srv.clear_edit_tag_override();
    sync::push_states_now(&db, &http).await;
    {
        let conn = db.lock().await;
        let left: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_queue WHERE action='read'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(left, 0, "真实成功路径仍应 prune");
    }

    let _ = std::fs::remove_file(&tmp);
}

/// P2-2：`a` 必须是完整 stream id（`user/-/label/<名>`）；夹具按真实 wire 规范
/// **实际移动分类**——中文/斜杠名可断言；目录名本身带 `user/-/label/` 字面量时
/// 仍只前置一次（不得按字面判定「已格式化」）。
#[tokio::test]
async fn freshrss_subscription_edit_moves_category_with_full_stream_id() {
    let srv = StrictServer::start(Mode::FreshRss).await;
    srv.set_subscriptions(vec![(
        "feed/10",
        "A 源",
        "http://example.com/a.xml",
        Some("旧分类"),
    )]);
    let client = GReaderClient::login_resolved(&srv.url(), USER, PASS, reqwest::Client::new())
        .await
        .expect("登录应成功");

    client
        .edit_subscription(10, Some("新标题"), Some("技术/阅读"))
        .await
        .expect("移动分类应成功");
    assert_eq!(
        srv.subscription_category("feed/10").as_deref(),
        Some("技术/阅读"),
        "夹具必须实际改分类（仅回 OK 不足以证明写生效）"
    );
    assert_eq!(srv.subscription_title("feed/10").as_deref(), Some("新标题"));
    let hits = srv.subscription_edit_hits();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].form_first("ac").as_deref(), Some("edit"));
    assert_eq!(hits[0].form_first("s").as_deref(), Some("feed/10"));
    assert_eq!(
        hits[0].form_first("a").as_deref(),
        Some("user/-/label/技术/阅读"),
        "a 必须是完整 stream id 而不是裸目录名"
    );
    assert_eq!(hits[0].form_first("T").as_deref(), Some(FRESH_ACTION_TOKEN));

    // 目录名本身就是 `user/-/label/` 字面量：仍按裸名前置一次。
    client
        .edit_subscription(10, None, Some("user/-/label/伪装"))
        .await
        .expect("应成功");
    assert_eq!(
        srv.subscription_category("feed/10").as_deref(),
        Some("user/-/label/伪装"),
        "字面量名字不得被误判为「已格式化」而少前置"
    );
    let hits = srv.subscription_edit_hits();
    assert_eq!(
        hits[1].form_first("a").as_deref(),
        Some("user/-/label/user/-/label/伪装")
    );
}
