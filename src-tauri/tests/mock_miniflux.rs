//! Mock Miniflux 服务器：最小 REST 形状（/v1/me、/v1/categories、/v1/feeds、
//! /v1/entries、PUT 状态/收藏），用于同步引擎端到端测试（不依赖真实服务端）。

use std::collections::HashMap;
use std::sync::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone, serde::Serialize, serde::Deserialize, Debug)]
pub struct MockEntry {
    pub id: i64,
    pub feed_id: i64,
    pub url: Option<String>,
    pub title: String,
    pub author: Option<String>,
    pub content: String,
    pub published_at: String,
    pub changed_at: String,
    pub status: String,
    pub starred: bool,
}

pub struct MockMiniflux {
    pub port: u16,
    pub entries: Mutex<Vec<MockEntry>>,
    /// 收到的状态更新（entry_id → status）
    pub status_updates: Mutex<Vec<(i64, String)>>,
    /// 收到的收藏切换
    pub bookmark_toggles: Mutex<Vec<i64>>,
    /// 收到的 feed 创建请求 (url, category_id)
    pub created_feeds: Mutex<Vec<(String, i64)>>,
    pub next_feed_id: Mutex<i64>,
    pub next_entry_id: Mutex<i64>,
}

impl MockMiniflux {
    pub async fn start() -> std::io::Result<std::sync::Arc<Self>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let server = std::sync::Arc::new(Self {
            port,
            entries: Mutex::new(Vec::new()),
            status_updates: Mutex::new(Vec::new()),
            bookmark_toggles: Mutex::new(Vec::new()),
            created_feeds: Mutex::new(Vec::new()),
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

    /// 旧便捷入口（sync_e2e 使用；dual_client 用 add_entry_ret 拿真实 id）。
    /// 共享模块跨 test target 编译，未用的 target 会报 dead_code，显式豁免。
    #[allow(dead_code)]
    pub fn add_entry(&self, feed_id: i64, url: &str, title: &str, status: &str, starred: bool) {
        let _ = self.add_entry_ret(feed_id, url, title, status, starred);
    }

    /// 同 add_entry，返回 mock 分配的 entry id（绑定场景需要真实 id）
    pub fn add_entry_ret(&self, feed_id: i64, url: &str, title: &str, status: &str, starred: bool) -> i64 {
        let id = {
            let mut n = self.next_entry_id.lock().unwrap();
            *n += 1;
            *n
        };
        let now = chrono::Utc::now().to_rfc3339();
        self.entries.lock().unwrap().push(MockEntry {
            id,
            feed_id,
            url: Some(url.to_string()),
            title: title.to_string(),
            author: Some("Miniflux Author".into()),
            content: "<p>Miniflux fetched content</p>".into(),
            published_at: now.clone(),
            changed_at: now,
            status: status.to_string(),
            starred,
        });
        id
    }
}

/// 极简 HTTP/1.1 解析：读请求头 + 可选 body，路由，写 JSON 响应
async fn handle_conn(mut stream: TcpStream, srv: std::sync::Arc<MockMiniflux>) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    // 读到 \r\n\r\n（头结束）
    loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = find_header_end(&buf) {
            // 有 body 时按 Content-Length 继续读
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
    eprintln!("[mock] raw request:\n{head}\n--- body: {body}");
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path_query = parts.next().unwrap_or("");
    let path = path_query.split('?').next().unwrap_or("");

    // Token 认证：无 token 头返回 401（hyper 统一小写 header 名）
    if !head.to_ascii_lowercase().contains("x-auth-token") {
        return write_json(&mut stream, 401, r#"{"error_message":"unauthorized"}"#).await;
    }

    let (status, json) = route(&srv, method, path, path_query, &body);
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

fn route(srv: &MockMiniflux, method: &str, path: &str, path_query: &str, body: &str) -> (u16, String) {
    match (method, path) {
        ("GET", "/v1/me") => (200, r#"{"id":1,"username":"mockuser"}"#.into()),
        ("GET", "/v1/categories") => (
            200,
            r#"[{"id":1,"title":"Default"},{"id":2,"title":"Remote Cat"}]"#.into(),
        ),
        ("GET", "/v1/feeds") => {
            // 两个远端 feed：一个与本地 URL 碰撞，一个是远端独有
            let json = r#"{"total":2,"feeds":[
                {"id":10,"feed_url":"http://127.0.0.1:8765/local_feed.xml","site_url":null,"title":"Remote Collision Feed","icon_url":null,"category":{"id":1,"title":"Default"}},
                {"id":11,"feed_url":"http://example.com/remote-only.xml","site_url":null,"title":"Remote Only Feed","icon_url":null,"category":{"id":2,"title":"Remote Cat"}}
            ]}"#;
            (200, json.into())
        }
        ("GET", "/v1/entries") => {
            // 解析 after/changed_after/offset（毫秒）
            let mut after_ms: i64 = 0;
            let mut offset: usize = 0;
            let mut changed = false;
            for kv in path_query.split('&').skip(1) {
                let (k, v) = kv.split_once('=').unwrap_or(("", ""));
                match k {
                    "after" => after_ms = v.parse().unwrap_or(0),
                    "changed_after" => {
                        changed = true;
                        after_ms = v.parse().unwrap_or(0)
                    }
                    "offset" => offset = v.parse().unwrap_or(0),
                    _ => {}
                }
            }
            // changed_after 也过滤 published_at（简化：测试里时间都是现在）
            let entries: Vec<MockEntry> = srv
                .entries
                .lock()
                .unwrap()
                .iter()
                .filter(|_| !changed || chrono::Utc::now().timestamp_millis() >= after_ms)
                .cloned()
                .collect();
            let total = entries.len();
            let page: Vec<MockEntry> = entries.into_iter().skip(offset).take(100).collect();
            let json = serde_json::json!({ "total": total, "entries": page }).to_string();
            (200, json)
        }
        ("PUT", "/v1/entries") => {
            let v: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
            let status = v.get("status").and_then(|s| s.as_str()).unwrap_or("read").to_string();
            let ids: Vec<i64> = v
                .get("entry_ids")
                .and_then(|a| a.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_i64()).collect())
                .unwrap_or_default();
            // 真实回写：同步轮次间状态持久（模拟服务端行为，双向流测试依赖）
            srv.entries.lock().unwrap().iter_mut().for_each(|e| {
                if ids.contains(&e.id) {
                    e.status = status.clone();
                }
            });
            for id in &ids {
                srv.status_updates.lock().unwrap().push((*id, status.clone()));
            }
            (204, String::new())
        }
        ("POST", "/v1/feeds") => {
            let v: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
            let url = v.get("feed_url").and_then(|s| s.as_str()).unwrap_or("").to_string();
            let cat = v.get("category_id").and_then(|s| s.as_i64()).unwrap_or(1);
            srv.created_feeds.lock().unwrap().push((url.clone(), cat));
            let feed_id = {
                let mut n = srv.next_feed_id.lock().unwrap();
                *n += 1;
                *n
            };
            (200, serde_json::json!({ "feed_id": feed_id }).to_string())
        }
        (_, p) if p.starts_with("/v1/entries/") && p.ends_with("/bookmark") => {
            let id: i64 = p
                .trim_start_matches("/v1/entries/")
                .trim_end_matches("/bookmark")
                .parse()
                .unwrap_or(0);
            srv.bookmark_toggles.lock().unwrap().push(id);
            (204, String::new())
        }
        ("POST", "/v1/categories") => (200, r#"{"id":3,"title":"New"}"#.into()),
        _ => (404, r#"{"error_message":"not found"}"#.into()),
    }
}

/// 供测试断言用的便捷读取
pub fn status_updates_map(srv: &MockMiniflux) -> HashMap<i64, String> {
    srv.status_updates
        .lock()
        .unwrap()
        .iter()
        .cloned()
        .collect()
}
