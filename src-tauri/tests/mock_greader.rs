//! Mock Google Reader 服务器：最小 Google Reader API 形状
//! （/accounts/ClientLogin、/reader/api/0/subscription/list、tag/list、
//! stream/items/ids、stream/items/contents、edit-tag、subscription/edit/quickadd），
//! 用于同步引擎端到端测试（不依赖真实服务端）。

use std::collections::HashMap;
use std::sync::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone, serde::Serialize, serde::Deserialize, Debug)]
pub struct MockEnclosure {
    pub url: String,
    #[serde(default)]
    pub r#type: String,
    #[serde(default)]
    pub mime_type: String,
    #[serde(default)]
    pub duration: Option<i64>,
}

/// 模拟条目（Google Reader ItemContent 的最小形状）
#[derive(Clone, serde::Serialize, serde::Deserialize, Debug)]
pub struct MockEntry {
    pub id: i64,
    pub feed_id: i64,
    pub url: Option<String>,
    pub title: String,
    pub author: Option<String>,
    pub content: String,
    pub published: i64,
    pub read: bool,
    pub starred: bool,
    #[serde(default)]
    pub enclosures: Vec<MockEnclosure>,
    /// 兼容旧测试：changed_at（unix 秒）
    #[serde(default)]
    pub changed_at: i64,
}

/// 模拟订阅（Google Reader subscription/list 的最小形状）
#[derive(Clone, serde::Serialize, serde::Deserialize, Debug)]
pub struct MockSubscription {
    pub id: String, // "feed/10"
    pub title: String,
    pub url: String,
    pub html_url: Option<String>,
    pub categories: Vec<(String, String)>, // (label, type="folder")
}

pub struct MockGReader {
    pub port: u16,
    pub entries: Mutex<Vec<MockEntry>>,
    /// 收到的状态更新（entry_id → "read"/"unread"/"star"/"unstar"）
    pub status_updates: Mutex<Vec<(i64, String)>>,
    /// 收到的订阅请求（url）
    pub subscribed_urls: Mutex<Vec<String>>,
    /// 兼容旧测试断言：收到的 feed 创建请求 (url, category_id)
    pub created_feeds: Mutex<Vec<(String, i64)>>,
    /// 兼容旧测试断言：收到的收藏切换
    pub bookmark_toggles: Mutex<Vec<i64>>,
    /// 服务端"已有"订阅（url → feed_id）：quickadd 同 URL 返回既有 id
    pub existing_feed_urls: Mutex<Vec<(String, i64)>>,
    /// 远端订阅列表（GET subscription/list 返回）
    pub subscriptions: Mutex<Vec<MockSubscription>>,
    /// 远端分类（GET tag/list 返回的 folder）
    pub folders: Mutex<Vec<String>>,
    pub next_feed_id: Mutex<i64>,
    pub next_entry_id: Mutex<i64>,
}

impl MockGReader {
    pub async fn start() -> std::io::Result<std::sync::Arc<Self>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let server = std::sync::Arc::new(Self {
            port,
            entries: Mutex::new(Vec::new()),
            status_updates: Mutex::new(Vec::new()),
            subscribed_urls: Mutex::new(Vec::new()),
            created_feeds: Mutex::new(Vec::new()),
            bookmark_toggles: Mutex::new(Vec::new()),
            existing_feed_urls: Mutex::new(vec![(
                "http://127.0.0.1:8765/local_feed.xml".into(),
                10,
            )]),
            subscriptions: Mutex::new(vec![
                MockSubscription {
                    id: "feed/10".into(),
                    title: "Remote Collision Feed".into(),
                    url: "http://127.0.0.1:8765/local_feed.xml".into(),
                    html_url: None,
                    categories: vec![("Default".into(), "folder".into())],
                },
                MockSubscription {
                    id: "feed/11".into(),
                    title: "Remote Only Feed".into(),
                    url: "http://example.com/remote-only.xml".into(),
                    html_url: None,
                    categories: vec![("Remote Cat".into(), "folder".into())],
                },
            ]),
            folders: Mutex::new(vec!["Default".into(), "Remote Cat".into()]),
            next_feed_id: Mutex::new(100),
            next_entry_id: Mutex::new(500),
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
        Ok(server)
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// 旧便捷入口（status:&str 语义：unread/read）。
    #[allow(dead_code)]
    pub fn add_entry(&self, feed_id: i64, url: &str, title: &str, status: &str, starred: bool) {
        let read = status == "read";
        let _ = self.add_entry_ret(feed_id, url, title, read, starred);
    }

    /// 添加条目，返回 mock 分配的 entry id。
    pub fn add_entry_ret(&self, feed_id: i64, url: &str, title: &str, read: bool, starred: bool) -> i64 {
        self.add_entry_with_published(feed_id, url, title, read, starred, chrono::Utc::now().timestamp())
    }

    /// 同 add_entry_ret，但可指定 published（unix 秒，模拟历史文章）。
    pub fn add_entry_with_published(
        &self,
        feed_id: i64,
        url: &str,
        title: &str,
        read: bool,
        starred: bool,
        published: i64,
    ) -> i64 {
        let id = {
            let mut n = self.next_entry_id.lock().unwrap();
            *n += 1;
            *n
        };
        self.entries.lock().unwrap().push(MockEntry {
            id,
            feed_id,
            url: Some(url.to_string()),
            title: title.to_string(),
            author: Some("Mock Author".into()),
            content: "<p>Mock fetched content</p>".into(),
            published,
            read,
            starred,
            enclosures: Vec::new(),
            changed_at: published,
        });
        id
    }

    /// 同 add_entry_ret，但可指定正文 HTML 与 enclosures（播客/封面回归用）。
    #[allow(dead_code)]
    #[allow(clippy::too_many_arguments)]
    pub fn add_entry_full(
        &self,
        feed_id: i64,
        url: &str,
        title: &str,
        read: bool,
        starred: bool,
        content: &str,
        enclosures: Vec<MockEnclosure>,
    ) -> i64 {
        let id = {
            let mut n = self.next_entry_id.lock().unwrap();
            *n += 1;
            *n
        };
        self.entries.lock().unwrap().push(MockEntry {
            id,
            feed_id,
            url: Some(url.to_string()),
            title: title.to_string(),
            author: Some("Mock Author".into()),
            content: content.to_string(),
            published: chrono::Utc::now().timestamp(),
            read,
            starred,
            enclosures,
            changed_at: chrono::Utc::now().timestamp(),
        });
        id
    }
}

/// 极简 HTTP/1.1 解析：读请求头 + 可选 body，路由，写 JSON 响应
async fn handle_conn(mut stream: TcpStream, srv: std::sync::Arc<MockGReader>) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = find_header_end(&buf) {
            let headers = String::from_utf8_lossy(&buf[..pos]).to_string();
            let content_len = headers
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
    let method = parts.next().unwrap_or("");
    let path_query = parts.next().unwrap_or("");
    let path = path_query.split('?').next().unwrap_or("");

    let (status, json) = route(&srv, method, path, path_query, &body, &head);
    write_json(&mut stream, status, &json).await
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

async fn write_json(stream: &mut TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        401 => "Unauthorized",
        _ => "Error",
    };
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(resp.as_bytes()).await?;
    stream.flush().await
}

/// 从 query string 解析参数（path_query 是 "path?k=v&k2=v2"）
fn parse_query(path_query: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Some(q) = path_query.split('?').nth(1) {
        for kv in q.split('&') {
            if let Some((k, v)) = kv.split_once('=') {
                map.insert(k.to_string(), v.to_string());
            }
        }
    }
    map
}

/// 从 form body 解析参数（url-encoded 或已解析的 form）
fn parse_form(body: &str) -> HashMap<String, Vec<String>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for kv in body.split('&') {
        if let Some((k, v)) = kv.split_once('=') {
            let v = url_decode(v);
            map.entry(k.to_string()).or_default().push(v);
        }
    }
    map
}

fn url_decode(s: &str) -> String {
    let mut out = String::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v as char);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(' ');
        } else {
            out.push(bytes[i] as char);
        }
        i += 1;
    }
    out
}

/// 十进制 id → Google Reader 长格式 item id（tag:google.com,2005:reader/item/<16位hex>）
fn long_item_id(id: i64) -> String {
    format!("tag:google.com,2005:reader/item/{id:016x}")
}

fn route(
    srv: &MockGReader,
    method: &str,
    path: &str,
    path_query: &str,
    body: &str,
    _head: &str,
) -> (u16, String) {
    match (method, path) {
        // ClientLogin：任何 Email/Passwd 都返回固定 token
        ("POST", "/accounts/ClientLogin") => (
            200,
            r#"{"SID":"mock/abc","LSID":"mock/abc","Auth":"mock/abc"}"#.into(),
        ),
        // 订阅列表
        ("GET", "/reader/api/0/subscription/list") => {
            let subs = srv.subscriptions.lock().unwrap();
            let arr: Vec<serde_json::Value> = subs
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "id": s.id,
                        "title": s.title,
                        "url": s.url,
                        "htmlUrl": s.html_url,
                        "categories": s.categories.iter().map(|(l, t)| serde_json::json!({"id": format!("user/-/label/{l}"), "label": l, "type": t})).collect::<Vec<_>>(),
                    })
                })
                .collect();
            (200, serde_json::json!({ "subscriptions": arr }).to_string())
        }
        // 标签列表（分类 + starred）
        ("GET", "/reader/api/0/tag/list") => {
            let folders = srv.folders.lock().unwrap();
            let mut tags: Vec<serde_json::Value> = vec![serde_json::json!({"id": "user/-/state/com.google/starred"})];
            for f in folders.iter() {
                tags.push(serde_json::json!({"id": format!("user/-/label/{f}"), "label": f, "type": "folder"}));
            }
            (200, serde_json::json!({ "tags": tags }).to_string())
        }
        // 条目 id 列表（reading-list 或 feed/数字）
        ("GET", "/reader/api/0/stream/items/ids") => {
            let q = parse_query(path_query);
            let stream = q.get("s").cloned().unwrap_or_default();
            let n: usize = q.get("n").and_then(|v| v.parse().ok()).unwrap_or(10000);
            let ot: i64 = q.get("ot").and_then(|v| v.parse().ok()).unwrap_or(0);
            // 过滤：stream 是 feed/数字 时按 feed_id；否则全部（reading-list）
            let feed_filter: Option<i64> = stream.strip_prefix("feed/").and_then(|s| s.parse().ok());
            let ids: Vec<i64> = srv
                .entries
                .lock()
                .unwrap()
                .iter()
                .filter(|e| feed_filter.map_or(true, |f| e.feed_id == f))
                .filter(|e| e.published >= ot)
                .map(|e| e.id)
                .collect();
            let total = ids.len();
            let page: Vec<i64> = ids.into_iter().take(n).collect();
            let item_refs: Vec<serde_json::Value> = page
                .iter()
                .map(|id| serde_json::json!({"id": id.to_string()}))
                .collect();
            let continuation = if total > page.len() {
                Some(page.len().to_string())
            } else {
                None
            };
            (200, serde_json::json!({ "itemRefs": item_refs, "continuation": continuation }).to_string())
        }
        // 条目正文（POST，i 重复参数）
        ("POST", "/reader/api/0/stream/items/contents") => {
            let form = parse_form(body);
            // i 参数可能来自 body 或 query
            let mut all_ids: Vec<i64> = Vec::new();
            if let Some(ids) = form.get("i") {
                all_ids.extend(ids.iter().filter_map(|s| s.parse::<i64>().ok()));
            }
            // 也可能在 query 里（output=json&i=123）
            let q = parse_query(path_query);
            if all_ids.is_empty() {
                if let Some(i) = q.get("i") {
                    all_ids.extend(i.split(',').filter_map(|s| s.parse::<i64>().ok()));
                }
            }
            let entries = srv.entries.lock().unwrap();
            let items: Vec<serde_json::Value> = entries
                .iter()
                .filter(|e| all_ids.contains(&e.id))
                .map(|e| {
                    let mut cats = vec!["user/-/state/com.google/reading-list".to_string()];
                    if e.read {
                        cats.push("user/-/state/com.google/read".to_string());
                    }
                    if e.starred {
                        cats.push("user/-/state/com.google/starred".to_string());
                    }
                    serde_json::json!({
                        "id": long_item_id(e.id),
                        "categories": cats,
                        "title": e.title,
                        "author": e.author,
                        "published": e.published,
                        "alternate": [{"href": e.url, "type": "text/html"}],
                        "summary": {"content": e.content},
                        "content": {"content": e.content},
                        "origin": {"streamId": format!("feed/{}", e.feed_id)},
                        "enclosure": e.enclosures.iter().map(|enc| serde_json::json!({"url": enc.url, "type": enc.r#type})).collect::<Vec<_>>(),
                    })
                })
                .collect();
            (200, serde_json::json!({ "items": items }).to_string())
        }
        // edit-tag：标读/收藏（a=加 tag, r=删 tag）
        ("POST", "/reader/api/0/edit-tag") => {
            let form = parse_form(body);
            let ids: Vec<i64> = form.get("i").map(|v| v.iter().filter_map(|s| s.parse().ok()).collect()).unwrap_or_default();
            let add: Vec<String> = form.get("a").cloned().unwrap_or_default();
            let remove: Vec<String> = form.get("r").cloned().unwrap_or_default();
            let mut entries = srv.entries.lock().unwrap();
            for e in entries.iter_mut() {
                if !ids.contains(&e.id) {
                    continue;
                }
                for tag in &add {
                    if tag.ends_with("/read") {
                        e.read = true;
                        srv.status_updates.lock().unwrap().push((e.id, "read".into()));
                    }
                    if tag.ends_with("/starred") {
                        e.starred = true;
                        srv.status_updates.lock().unwrap().push((e.id, "star".into()));
                    }
                }
                for tag in &remove {
                    if tag.ends_with("/read") {
                        e.read = false;
                        srv.status_updates.lock().unwrap().push((e.id, "unread".into()));
                    }
                    if tag.ends_with("/starred") {
                        e.starred = false;
                        srv.status_updates.lock().unwrap().push((e.id, "unstar".into()));
                    }
                }
            }
            (200, "OK".into())
        }
        // quickadd：订阅（幂等：已存在返回既有 id）
        ("POST", "/reader/api/0/subscription/quickadd") => {
            let form = parse_form(body);
            let url = form.get("quickadd").and_then(|v| v.first().cloned()).unwrap_or_default();
            let existing = srv
                .existing_feed_urls
                .lock()
                .unwrap()
                .iter()
                .find(|(u, _)| *u == url)
                .map(|(_, id)| *id);
            if let Some(id) = existing {
                (200, serde_json::json!({"numResults": 1, "streamId": format!("feed/{id}")}).to_string())
            } else {
                srv.subscribed_urls.lock().unwrap().push(url.clone());
                srv.created_feeds.lock().unwrap().push((url.clone(), 1));
                let feed_id = {
                    let mut n = srv.next_feed_id.lock().unwrap();
                    *n += 1;
                    *n
                };
                (200, serde_json::json!({"numResults": 1, "streamId": format!("feed/{feed_id}")}).to_string())
            }
        }
        // subscription/edit：订阅/退订/编辑
        ("POST", "/reader/api/0/subscription/edit") => {
            let form = parse_form(body);
            let ac = form.get("ac").and_then(|v| v.first().cloned()).unwrap_or_default();
            match ac.as_str() {
                "subscribe" => {
                    let url = form.get("s").and_then(|v| v.first().cloned()).unwrap_or_default();
                    let url = url.strip_prefix("feed/").unwrap_or(&url).to_string();
                    srv.subscribed_urls.lock().unwrap().push(url);
                    (200, "OK".into())
                }
                _ => (200, "OK".into()),
            }
        }
        _ => (404, r#"{"error_message":"not found"}"#.into()),
    }
}

/// 供测试断言用的便捷读取（跨 test target 共享，未用的 target 会报 dead_code，显式豁免）
#[allow(dead_code)]
pub fn status_updates_map(srv: &MockGReader) -> HashMap<i64, String> {
    srv.status_updates
        .lock()
        .unwrap()
        .iter()
        .cloned()
        .collect()
}
