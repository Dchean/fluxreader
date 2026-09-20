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
    /// 收到的订阅编辑动作 (ac, s)：ac ∈ subscribe/unsubscribe/edit（edit 携带 t/a 参数由测试按需扩展）
    pub subscription_edits: Mutex<Vec<(String, String)>>,
    /// 故障注入：置位后 GET stream/items/ids 返回 500（C-1 对账跳过测试用）
    pub fail_stream_ids: std::sync::atomic::AtomicBool,
    /// 故障注入（TASK-060）：置位后 edit-tag 返回 500——推送失败、队列保留，
    /// 用于构造「本地变更已入队未推送（pending）+ 远端陈旧状态」的场景。
    pub fail_edit_tag: std::sync::atomic::AtomicBool,
    /// 故障注入（TASK-068）：置位后 stream/items/contents 返回 500——pull 分块失败，验证游标不推进守卫
    pub fail_item_contents: std::sync::atomic::AtomicBool,
    /// 故障注入（TASK-069 审查 F1）：置位后仅 reading-list 的 stream/items/ids 返回 500
    /// （read/starred 对账仍成功）——验证「id 列举失败同样不推进游标」
    pub fail_reading_list_ids: std::sync::atomic::AtomicBool,
    /// 故障注入（TASK-055）：置位后退订仍返回 200，但**服务端保留该订阅**——
    /// 模拟真实 GReader 后端在 token 失效/权限不足/目标不存在时「2xx + 未生效」的响应。
    pub unsubscribe_returns_2xx_without_removing: std::sync::atomic::AtomicBool,
    /// 最近一次 subscription/edit 请求的表单键值（A-2 断言 t=/a= 参数）
    pub last_subscription_edit_form: Mutex<Vec<(String, String)>>,
    /// 远端分类（GET tag/list 返回的 folder）
    pub folders: Mutex<Vec<String>>,
    pub next_feed_id: Mutex<i64>,
    pub next_entry_id: Mutex<i64>,
    /// TASK-059：GReader API 的前缀（"" = Miniflux 形态「在站点根」，
    /// "/api/greader.php" = FreshRSS 形态）。**非此前缀的请求一律 404**，
    /// 用于真实模拟「两种后端布局不同」，从而验证自动适配。
    pub greader_api_prefix: Mutex<String>,
    /// TASK-059：置位后 ClientLogin 返回 401（模拟凭据被拒，用于验证
    /// 「凭据错误不得被误报成找不到 API」）。
    pub reject_login: std::sync::atomic::AtomicBool,
    /// TASK-059：Fever 端点路径（"" = 走 `{base}/fever/?api` 的 Miniflux 形态；
    /// "/api/fever.php" = FreshRSS 形态）。非该路径的 Fever 请求返回 404。
    pub fever_endpoint: Mutex<String>,
    /// TASK-059：Fever 返回的 api_version（FreshRSS 实测为 4）。
    pub fever_api_version: Mutex<i64>,
    /// TASK-059：置位后 Fever 返回 auth=0（验证 auth 校验未因放宽版本而放松）。
    pub fever_reject_auth: std::sync::atomic::AtomicBool,
    /// TASK-059：收到的全部请求（`"{METHOD} {path}"`，含被前缀规则拒绝的）。
    /// 用于证明「探测有界」与「解析结果已缓存、后续同步不再探测」——
    /// 只看状态码无法区分「试了 1 次」与「试了 2 次」。
    pub requests: Mutex<Vec<String>>,
}

impl MockGReader {
    pub async fn start() -> std::io::Result<std::sync::Arc<Self>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let server = std::sync::Arc::new(Self {
            port,
            entries: Mutex::new(Vec::new()),
            subscription_edits: Mutex::new(Vec::new()),
            fail_stream_ids: std::sync::atomic::AtomicBool::new(false),
            fail_edit_tag: std::sync::atomic::AtomicBool::new(false),
            fail_item_contents: std::sync::atomic::AtomicBool::new(false),
            fail_reading_list_ids: std::sync::atomic::AtomicBool::new(false),
            unsubscribe_returns_2xx_without_removing: std::sync::atomic::AtomicBool::new(false),
            last_subscription_edit_form: Mutex::new(Vec::new()),
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
            greader_api_prefix: Mutex::new(String::new()),
            reject_login: std::sync::atomic::AtomicBool::new(false),
            fever_endpoint: Mutex::new(String::new()),
            fever_api_version: Mutex::new(3),
            fever_reject_auth: std::sync::atomic::AtomicBool::new(false),
            requests: Mutex::new(Vec::new()),
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

    /* ---------- TASK-059：端点布局与故障注入 ---------- */

    /// 设定 GReader API 前缀：`""` = Miniflux 形态（站点根）；
    /// `"/api/greader.php"` = FreshRSS 形态（子路径）。
    pub fn set_greader_api_prefix(&self, prefix: &str) {
        *self.greader_api_prefix.lock().unwrap() = prefix.to_string();
    }

    /// 置位后 edit-tag 返回 500（推送失败，客户端应保留队列待重试）。
    pub fn set_fail_edit_tag(&self, fail: bool) {
        self.fail_edit_tag
            .store(fail, std::sync::atomic::Ordering::SeqCst);
    }

    /// 置位后 stream/items/contents 返回 500（TASK-068：pull 分块失败，验证游标守卫）。
    pub fn set_fail_item_contents(&self, fail: bool) {
        self.fail_item_contents
            .store(fail, std::sync::atomic::Ordering::SeqCst);
    }

    /// 置位后仅 reading-list 的 stream/items/ids 返回 500（TASK-069 审查 F1：
    /// id 列举失败，验证游标守卫覆盖该路径）。
    pub fn set_fail_reading_list_ids(&self, fail: bool) {
        self.fail_reading_list_ids
            .store(fail, std::sync::atomic::Ordering::SeqCst);
    }

    /// 置位后 ClientLogin 一律 401（模拟凭据被拒）。
    pub fn set_reject_login(&self, reject: bool) {
        self.reject_login
            .store(reject, std::sync::atomic::Ordering::SeqCst);
    }

    /// 设定 Fever 端点路径：`""` = `/fever/?api` 形态；`"/api/fever.php"` = FreshRSS 形态。
    pub fn set_fever_endpoint(&self, endpoint: &str) {
        *self.fever_endpoint.lock().unwrap() = endpoint.to_string();
    }

    /// 设定 Fever 返回的 api_version（FreshRSS 实测为 4）。
    pub fn set_fever_api_version(&self, v: i64) {
        *self.fever_api_version.lock().unwrap() = v;
    }

    /// 置位后 Fever 返回 auth=0。
    pub fn set_fever_reject_auth(&self, reject: bool) {
        self.fever_reject_auth
            .store(reject, std::sync::atomic::Ordering::SeqCst);
    }

    /// 收到的请求记录（`"{METHOD} {path}"`）。
    pub fn request_log(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    /// 清空请求记录（用于「只看这一段有没有再探测」）。
    pub fn clear_request_log(&self) {
        self.requests.lock().unwrap().clear();
    }

    /// 命中 ClientLogin 的请求次数（探测次数的直接证据）。
    pub fn login_request_count(&self) -> usize {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.ends_with("/accounts/ClientLogin"))
            .count()
    }

    /// 旧便捷入口（status:&str 语义：unread/read）。
    #[allow(dead_code)]
    pub fn add_entry(&self, feed_id: i64, url: &str, title: &str, status: &str, starred: bool) {
        let read = status == "read";
        let _ = self.add_entry_ret(feed_id, url, title, read, starred);
    }

    /// 添加条目，返回 mock 分配的 entry id。
    pub fn add_entry_ret(
        &self,
        feed_id: i64,
        url: &str,
        title: &str,
        read: bool,
        starred: bool,
    ) -> i64 {
        self.add_entry_with_published(
            feed_id,
            url,
            title,
            read,
            starred,
            chrono::Utc::now().timestamp(),
        )
    }

    /// 同 add_entry_ret，但可指定 published（unix 秒，模拟「原文发布时间早、
    /// 但刚被抓取入库」的历史文章）。changed_at 固定为现在（抓取时刻），
    /// 与真实 Google Reader 语义一致：ot 游标按 crawl/change 时间过滤，而非 published。
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
            changed_at: chrono::Utc::now().timestamp(),
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
async fn handle_conn(
    mut stream: TcpStream,
    srv: std::sync::Arc<MockGReader>,
) -> std::io::Result<()> {
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

    // TASK-059：先记录请求，再做前缀判定——被拒绝的探测请求也要看得见，
    // 否则「探测有界」「不再重复探测」这两条断言无从取证。
    srv.requests.lock().unwrap().push(format!("{method} {path}"));

    // TASK-059：模拟「两种后端把 API 放在不同路径」。
    // 配置了前缀时，非该前缀下的 GReader/Fever 端点一律 404——
    // 这正是真实 FreshRSS 与 Miniflux 的差异，也是自动适配要解决的问题。
    if let Some(deny) = path_outside_configured_prefix(&srv, path) {
        return write_json(&mut stream, 404, &deny).await;
    }

    let (status, json) = route(&srv, method, path, path_query, &body, &head);
    write_json(&mut stream, status, &json).await
}

/// 若该路径落在已配置前缀之外，返回 Some(404 响应体)；否则 None（继续正常路由）。
///
/// 只拦截「受布局影响」的端点（ClientLogin / reader API / Fever），
/// 避免影响 mock 自身的其它路由。
fn path_outside_configured_prefix(srv: &MockGReader, path: &str) -> Option<String> {
    const GREADER_SUFFIXES: [&str; 9] = [
        "/accounts/ClientLogin",
        "/reader/api/0/subscription/list",
        "/reader/api/0/tag/list",
        "/reader/api/0/stream/items/ids",
        "/reader/api/0/stream/items/contents",
        "/reader/api/0/edit-tag",
        "/reader/api/0/subscription/edit",
        "/reader/api/0/subscription/quickadd",
        "/reader/api/0/mark-all-as-read",
    ];

    // GReader 家族：请求路径必须**恰好**是「已配置前缀 + 端点」。
    // 例如 prefix="" ⇒ 只认 /accounts/ClientLogin（根就是 API）；
    //     prefix="/api/greader.php" ⇒ 只认 /api/greader.php/accounts/ClientLogin。
    // 用「恰好相等」而不是 starts_with：prefix="" 时后者会让任何路径都通过，
    // 探测测试就失去了意义。
    if let Some(suffix) = GREADER_SUFFIXES.iter().find(|s| path.ends_with(**s)) {
        let prefix = srv.greader_api_prefix.lock().unwrap().clone();
        if path != format!("{prefix}{suffix}") {
            return Some(
                r#"{"error_message":"404 not found (path outside configured API prefix)"}"#.into(),
            );
        }
    }

    // Fever 家族：Miniflux 形态是 `/fever/`（其后再无路径段），
    // FreshRSS 形态是 `/api/fever.php`。未配置（""）时按 Miniflux 形态处理。
    //
    // 用**恰好相等**而非 starts_with：`starts_with` 会放过
    // `…/api/fever.php/fever/?api` 这种把两种形态叠起来的拼接错误
    // ——而那正是「写死 /fever/、忽略解析结果」会产生的 URL，mock 必须能抓到。
    if path.contains("fever") {
        let ep = srv.fever_endpoint.lock().unwrap().clone();
        let expected = if ep.is_empty() {
            "/fever/".to_string()
        } else {
            ep.clone()
        };
        if path != expected {
            return Some(r#"{"error_message":"404 not found (fever endpoint mismatch)"}"#.into());
        }
    }
    None
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
    /* 按字节解码 %XX 后再整体按 UTF-8 解释：逐个字节 `as char` 会把多字节
    UTF-8（中文分类名等）解成 Latin-1 乱码（A-2 的 a=目标分类 断言暴露）。 */
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
        // ClientLogin：默认任何 Email/Passwd 都返回固定 token；
        // TASK-059 故障注入下返回 401（模拟凭据被拒）
        ("POST", p) if p.ends_with("/accounts/ClientLogin") => {
            if srv.reject_login.load(std::sync::atomic::Ordering::SeqCst) {
                (401, r#"{"error_message":"Unauthorized"}"#.into())
            } else {
                (
                    200,
                    r#"{"SID":"mock/abc","LSID":"mock/abc","Auth":"mock/abc"}"#.into(),
                )
            }
        }
        // TASK-059：Fever 信封（Miniflux 的 /fever/?api 与 FreshRSS 的 /api/fever.php 共用）。
        // 带一份最小分组/订阅数据，使「解析出的端点能被真正使用」可验证——
        // 只回信封的话，客户端只能证明连接成功，证明不了后续调用打对了地址。
        ("POST", p) if p.contains("fever") => {
            let v = *srv.fever_api_version.lock().unwrap();
            let auth = if srv.fever_reject_auth.load(std::sync::atomic::Ordering::SeqCst) {
                0
            } else {
                1
            };
            let folders = srv.folders.lock().unwrap().clone();
            let groups: Vec<serde_json::Value> = folders
                .iter()
                .enumerate()
                .map(|(i, title)| serde_json::json!({ "id": (i as i64) + 1, "title": title }))
                .collect();
            let feeds: Vec<serde_json::Value> = srv
                .subscriptions
                .lock()
                .unwrap()
                .iter()
                .enumerate()
                // Fever 的 feed id 是数字（mock 的 GReader id 形如 "feed/10"，不能直接复用）
                .map(|(i, s)| {
                    serde_json::json!({ "id": (i as i64) + 1, "title": s.title, "url": s.url })
                })
                .collect();
            (
                200,
                serde_json::json!({
                    "api_version": v,
                    "auth": auth,
                    "groups": groups,
                    "feeds": feeds,
                })
                .to_string(),
            )
        }
        // 订阅列表
        ("GET", p) if p.ends_with("/reader/api/0/subscription/list") => {
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
        ("GET", p) if p.ends_with("/reader/api/0/tag/list") => {
            let folders = srv.folders.lock().unwrap();
            let mut tags: Vec<serde_json::Value> =
                vec![serde_json::json!({"id": "user/-/state/com.google/starred"})];
            for f in folders.iter() {
                tags.push(serde_json::json!({"id": format!("user/-/label/{f}"), "label": f, "type": "folder"}));
            }
            (200, serde_json::json!({ "tags": tags }).to_string())
        }
        // 条目 id 列表（reading-list 或 feed/数字）
        ("GET", p) if p.ends_with("/reader/api/0/stream/items/ids") => {
            if srv
                .fail_stream_ids
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                return (500, r#"{"error_message":"injected failure"}"#.into());
            }
            let q = parse_query(path_query);
            let stream = q.get("s").cloned().unwrap_or_default();
            // TASK-069 审查 F1：只让 reading-list 主列举失败（read/starred 对账照常成功），
            // 用于证明「id 列举失败也不得推进游标」，且不与 C-1 对账跳过语义互相干扰。
            if srv
                .fail_reading_list_ids
                .load(std::sync::atomic::Ordering::SeqCst)
                && stream.contains("reading-list")
            {
                return (500, r#"{"error_message":"injected reading-list failure"}"#.into());
            }
            let n: usize = q.get("n").and_then(|v| v.parse().ok()).unwrap_or(10000);
            let ot: i64 = q.get("ot").and_then(|v| v.parse().ok()).unwrap_or(0);
            // 过滤：feed/数字 按 feed_id；read/starred 按状态；否则全部（reading-list）
            let state_filter = match stream.as_str() {
                "user/-/state/com.google/read" => Some((true, false)), // 只看已读
                "user/-/state/com.google/starred" => Some((false, true)), // 只看收藏
                _ => None,
            };
            let feed_filter: Option<i64> =
                stream.strip_prefix("feed/").and_then(|s| s.parse().ok());
            let ids: Vec<i64> = srv
                .entries
                .lock()
                .unwrap()
                .iter()
                .filter(|e| feed_filter.map_or(true, |f| e.feed_id == f))
                .filter(|e| match state_filter {
                    Some((is_read, is_starred)) => (e.read == is_read) && (e.starred == is_starred),
                    None => true,
                })
                .filter(|e| e.changed_at >= ot)
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
            (
                200,
                serde_json::json!({ "itemRefs": item_refs, "continuation": continuation })
                    .to_string(),
            )
        }
        // 条目正文（POST，i 重复参数）
        ("POST", p) if p.ends_with("/reader/api/0/stream/items/contents") => {
            if srv.fail_item_contents.load(std::sync::atomic::Ordering::SeqCst) {
                // TASK-068 注入：pull 分块失败（500）
                return (500, "Internal Server Error".to_string());
            }
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
        ("POST", p) if p.ends_with("/reader/api/0/edit-tag") => {
            if srv.fail_edit_tag.load(std::sync::atomic::Ordering::SeqCst) {
                return (500, r#"{"error_message":"injected edit-tag failure"}"#.into());
            }
            let form = parse_form(body);
            let ids: Vec<i64> = form
                .get("i")
                .map(|v| v.iter().filter_map(|s| s.parse().ok()).collect())
                .unwrap_or_default();
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
                        srv.status_updates
                            .lock()
                            .unwrap()
                            .push((e.id, "read".into()));
                    }
                    if tag.ends_with("/starred") {
                        e.starred = true;
                        srv.bookmark_toggles.lock().unwrap().push(e.id);
                    }
                }
                for tag in &remove {
                    if tag.ends_with("/read") {
                        e.read = false;
                        srv.status_updates
                            .lock()
                            .unwrap()
                            .push((e.id, "unread".into()));
                    }
                    if tag.ends_with("/starred") {
                        e.starred = false;
                        srv.bookmark_toggles.lock().unwrap().push(e.id);
                    }
                }
            }
            (200, "OK".into())
        }
        // quickadd：订阅（幂等：已存在返回既有 id）
        ("POST", p) if p.ends_with("/reader/api/0/subscription/quickadd") => {
            let form = parse_form(body);
            let url = form
                .get("quickadd")
                .and_then(|v| v.first().cloned())
                .unwrap_or_default();
            let existing = srv
                .existing_feed_urls
                .lock()
                .unwrap()
                .iter()
                .find(|(u, _)| *u == url)
                .map(|(_, id)| *id);
            if let Some(id) = existing {
                (
                    200,
                    serde_json::json!({"numResults": 1, "streamId": format!("feed/{id}")})
                        .to_string(),
                )
            } else {
                srv.subscribed_urls.lock().unwrap().push(url.clone());
                srv.created_feeds.lock().unwrap().push((url.clone(), 1));
                let feed_id = {
                    let mut n = srv.next_feed_id.lock().unwrap();
                    *n += 1;
                    *n
                };
                (
                    200,
                    serde_json::json!({"numResults": 1, "streamId": format!("feed/{feed_id}")})
                        .to_string(),
                )
            }
        }
        // subscription/edit：订阅/退订/编辑
        ("POST", p) if p.ends_with("/reader/api/0/subscription/edit") => {
            let form = parse_form(body);
            let ac = form
                .get("ac")
                .and_then(|v| v.first().cloned())
                .unwrap_or_default();
            let s_val = form
                .get("s")
                .and_then(|v| v.first().cloned())
                .unwrap_or_default();
            srv.subscription_edits
                .lock()
                .unwrap()
                .push((ac.clone(), s_val));
            {
                let mut form_out = srv.last_subscription_edit_form.lock().unwrap();
                form_out.clear();
                for (k, v) in form.iter() {
                    form_out.push((k.clone(), v.first().cloned().unwrap_or_default()));
                }
            }
            match ac.as_str() {
                "unsubscribe" => {
                    let s_val = form
                        .get("s")
                        .and_then(|v| v.first().cloned())
                        .unwrap_or_default();
                    // 真实行为：退订后远端订阅列表不再包含该订阅。
                    // TASK-055 故障注入：置位时仍回 200 但保留订阅（「2xx 但未生效」）。
                    if !srv
                        .unsubscribe_returns_2xx_without_removing
                        .load(std::sync::atomic::Ordering::SeqCst)
                    {
                        srv.subscriptions
                            .lock()
                            .unwrap()
                            .retain(|sub| sub.id != s_val);
                    }
                    (200, "OK".into())
                }
                "subscribe" => {
                    let url = form
                        .get("s")
                        .and_then(|v| v.first().cloned())
                        .unwrap_or_default();
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

/// 最近一次 subscription/edit 请求的表单键值（A-2 断言 t=/a=）。
#[allow(dead_code)]
pub fn last_subscription_edit_form(srv: &MockGReader) -> Vec<(String, String)> {
    srv.last_subscription_edit_form.lock().unwrap().clone()
}

/// 供测试断言用的便捷读取（跨 test target 共享，未用的 target 会报 dead_code，显式豁免）
#[allow(dead_code)]
pub fn subscription_edit_actions(srv: &MockGReader) -> Vec<(String, String)> {
    srv.subscription_edits.lock().unwrap().clone()
}

/// 供测试断言用的便捷读取（跨 test target 共享，未用的 target 会报 dead_code，显式豁免）
#[allow(dead_code)]
pub fn status_updates_map(srv: &MockGReader) -> HashMap<i64, String> {
    srv.status_updates.lock().unwrap().iter().cloned().collect()
}
