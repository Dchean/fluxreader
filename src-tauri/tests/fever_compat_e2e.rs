//! OPT-005：Fever 身份（长 id / 十进制数字字符串）与 max_id 完整历史回溯的**严格夹具**验证。
//!
//! 夹具按固定上游源码的实现形态建模（不是共享 mock_greader 的宽松形态）：
//! - FreshRSS `p/api/fever.php`（保存于 `tmp/optimization-20261008/upstream/`）：
//!   `id` 是 PHP numeric-string（64 位十进制，`json_encode` 输出 **JSON 字符串**），
//!   如 `"1791440000000000"`；`api_key` 只读 POST form body；`max_id` 语义为
//!   `id < max_id ORDER BY id DESC LIMIT 50`；`unread_item_ids`/`saved_item_ids`
//!   为逗号分隔字符串全集；`mark=item` 只接受单个 `ctype_digit` 的 id。
//! - Miniflux `internal/fever/handler.go`：同一能力，`id`/`feed_id` 为 JSON 数字，
//!   条目升序返回（夹具用 `max_id_asc` 模拟，锁「游标与页内顺序无关」）。
//!
//! 夹具只记录与校验请求；断言对象是**生产代码**（`FeverClient` / `sync::sync_now` /
//! `sync::sync_light`）发出的请求与产生的结果。本文件只声称「严格夹具 × 固定源码
//! 契约」验证，不代表任何真实部署服务端的实测（见 docs/sync-compat-matrix.md 的
//! 版本声明）。
//!
//! 运行：cargo test --test fever_compat_e2e

mod common;

use app_lib::db;
use app_lib::fever::FeverClient;
use app_lib::sync;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/* ============================================================
严格夹具服务端
============================================================ */

/// 夹具凭据（fixture 只认这一组）。
const USER: &str = "alice";
const PASS: &str = "secret";
/// md5("alice:secret") —— Fever api_key 公式不变（十六进制小写）。
const API_KEY: &str = "6f622058968bb90757e6c6ed79e5df81";

/// FreshRSS 长 id 用例的条目 id：16 位纯十进制，旧 `len==16` 十六进制猜测
/// 会把它解释成完全不同的数字（0x1791440000000000），必须按十进制字面值处理。
const LONG_ID: i64 = 1_791_440_000_000_000;
const LONG_FEED_ID: i64 = 2_000_000_000;

#[derive(Clone, Copy, Default)]
enum IdStyle {
    /// FreshRSS：`id` 序列化为 JSON 字符串（numeric-string）。
    #[default]
    DecimalString,
    /// Miniflux：`id` 序列化为 JSON 数字。
    JsonNumber,
}

#[derive(Default)]
struct FixtureOptions {
    id_style: IdStyle,
    /// feed_id 是否也渲染成字符串（契约：两种都接受）。
    feed_id_as_string: bool,
    /// **泛化鲁棒性对照**：假设某实现按升序返回 max_id 页。**不是** Miniflux
    /// 的事实——Miniflux 固定源码与 FreshRSS 同为 `ORDER BY id DESC`
    /// （见 fever.rs 模块头）；本开关只用来锁「游标取页内最小 id，与顺序无关」。
    max_id_asc_page: bool,
    /// true 时忽略 max_id（模拟不支持的服务器），返回与无参相同的最近 50 条。
    ignore_max_id: bool,
    /// true 时每个历史页把首个条目重复一遍（分页窗口重复），锁去重。
    dup_echo: bool,
}

#[derive(Clone)]
struct FEntry {
    id: i64,
    feed_id: i64,
    title: String,
    read: bool,
    saved: bool,
    /// OPT-008B：URL 覆盖（跨源同 URL 条目身份用例需要两条不同 feed 的
    /// entry 指向同一 URL；缺省仍按 `<feed>/<id>` 生成，既有用例逐字不变）。
    url: Option<String>,
}

/// 原样响应（HTTP 层用例：无长度分帧/压缩超限）。
#[derive(Clone)]
struct RawResponse {
    /// true = `Transfer-Encoding: chunked`（**不带** Content-Length）。
    chunked: bool,
    /// 附加响应头（如 `Content-Encoding: br`）。
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// 一条被夹具记录下来的请求。夹具用它做校验（拒绝不合法请求），
/// 测试用它断言生产客户端「发了什么」。
#[derive(Clone)]
struct Hit {
    method: String,
    path: String,
    query: String,
    form: Vec<(String, String)>,
}

impl Hit {
    fn form_first(&self, key: &str) -> Option<String> {
        self.form
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    }

    fn query_param(&self, key: &str) -> Option<String> {
        query_map(&self.query).get(key).cloned()
    }
}

struct Fixture {
    port: u16,
    /// 夹具只在此路径提供 Fever API（FreshRSS 形态），其余 404。
    endpoint_path: String,
    id_style: IdStyle,
    feed_id_as_string: bool,
    max_id_asc_page: bool,
    ignore_max_id: bool,
    dup_echo: bool,
    /// 第 N 次 max_id 调用返回 500（1 起算），用于分页失败可重试用例。
    max_id_fail_at: Mutex<Option<usize>>,
    max_id_calls: AtomicUsize,
    /// 第 N 次 since_id 调用返回 500（1 起算），用于增量分页失败用例。
    since_id_fail_at: Mutex<Option<usize>>,
    since_id_calls: AtomicUsize,
    /// 响应中省略 `items` 字段（协议错误用例）。
    omit_items: AtomicBool,
    /// 响应中省略 `unread_item_ids` / `saved_item_ids` 字段（协议错误用例）。
    omit_unread: AtomicBool,
    omit_saved: AtomicBool,
    /// 订阅层 action 各自要求的集合字段是否省略（OPT-019）。
    omit_feeds: AtomicBool,
    omit_groups: AtomicBool,
    omit_feeds_side_feeds_groups: AtomicBool,
    omit_groups_side_feeds_groups: AtomicBool,
    /// 覆盖条目的 id JSON 值（原文，如 `"\"0x1F\""`、`"1.5"`），非法 id 用例。
    raw_item_id_json: Mutex<Option<String>>,
    /// 覆盖 unread_item_ids 的原始 CSV 文本（非法集合用例）。
    raw_unread_csv: Mutex<Option<String>>,
    /// EntryBeforeDisplay 式**事后过滤**（LIMIT 之后按 id 丢弃）：
    /// 模拟 FreshRSS `fever.php` 先 `LIMIT 50` 再由扩展 hook 过滤的形态——
    /// 过滤后的非空短页**不代表没有后页**。R2 更正：hook 对**所有** items 分支
    /// 生效（含 with_ids，`getItems()` 统一走 hook）——被丢弃的条目对 Fever
    /// 整体不可见，不是「列表遗漏但 id 可取」。
    hook_drop_ids: Mutex<HashSet<i64>>,
    /// **合成形态（不代表 FreshRSS）**：仅从列表窗口（max_id/since_id/无参）
    /// 隐藏、with_ids 仍按 id 可取——模拟「服务端保留窗口/列表差异」，用于
    /// with_ids 补齐路径；不可冒充 hook 行为（见 `hook_drop_ids`）。
    listing_skip_ids: Mutex<HashSet<i64>>,
    /// >0 时条目 html 渲染为该字节数的超长串（响应体上限用例）。
    oversize_html_bytes: AtomicUsize,
    /// 原样响应（超限/压缩用例）：绕过 JSON 路由直接回字节/分帧。
    raw_response: Mutex<Option<RawResponse>>,
    /// 指定 action 返回 HTTP 200 非 JSON 正文（行为级 malformed JSON 用例）。
    malformed_json_action: Mutex<Option<String>>,
    entries: Mutex<Vec<FEntry>>,
    /// (feed_id, title, url)
    feeds: Mutex<Vec<(i64, String, String)>>,
    /// (group_id, title)
    groups: Mutex<Vec<(i64, String)>>,
    hits: Mutex<Vec<Hit>>,
}

impl Fixture {
    async fn start() -> Arc<Self> {
        Self::start_with(FixtureOptions::default()).await
    }

    async fn start_with(opts: FixtureOptions) -> Arc<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fixture");
        let port = listener.local_addr().unwrap().port();
        let server = Arc::new(Self {
            port,
            endpoint_path: "/api/fever.php".to_string(),
            id_style: opts.id_style,
            feed_id_as_string: opts.feed_id_as_string,
            max_id_asc_page: opts.max_id_asc_page,
            ignore_max_id: opts.ignore_max_id,
            dup_echo: opts.dup_echo,
            max_id_fail_at: Mutex::new(None),
            max_id_calls: AtomicUsize::new(0),
            since_id_fail_at: Mutex::new(None),
            since_id_calls: AtomicUsize::new(0),
            omit_items: AtomicBool::new(false),
            omit_unread: AtomicBool::new(false),
            omit_saved: AtomicBool::new(false),
            omit_feeds: AtomicBool::new(false),
            omit_groups: AtomicBool::new(false),
            omit_feeds_side_feeds_groups: AtomicBool::new(false),
            omit_groups_side_feeds_groups: AtomicBool::new(false),
            raw_item_id_json: Mutex::new(None),
            raw_unread_csv: Mutex::new(None),
            hook_drop_ids: Mutex::new(HashSet::new()),
            listing_skip_ids: Mutex::new(HashSet::new()),
            oversize_html_bytes: AtomicUsize::new(0),
            raw_response: Mutex::new(None),
            malformed_json_action: Mutex::new(None),
            entries: Mutex::new(Vec::new()),
            feeds: Mutex::new(Vec::new()),
            groups: Mutex::new(Vec::new()),
            hits: Mutex::new(Vec::new()),
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

    /// 生产 `FeverClient` 的已解析 base（跳过探测，直接打夹具端点）。
    fn api_root(&self) -> String {
        format!("{}{}", self.url(), self.endpoint_path)
    }

    fn add_entry(&self, id: i64, feed_id: i64, title: &str, read: bool, saved: bool) {
        self.add_entry_with_url(id, feed_id, title, read, saved, None);
    }

    /// OPT-008B：可指定 URL 的条目入口（None = 既有 `<feed>/<id>` 形态）。
    fn add_entry_with_url(
        &self,
        id: i64,
        feed_id: i64,
        title: &str,
        read: bool,
        saved: bool,
        url: Option<&str>,
    ) {
        self.entries.lock().unwrap().push(FEntry {
            id,
            feed_id,
            title: title.to_string(),
            read,
            saved,
            url: url.map(str::to_string),
        });
    }

    fn set_feeds(&self, feeds: Vec<(i64, &str, &str)>) {
        *self.feeds.lock().unwrap() = feeds
            .into_iter()
            .map(|(id, title, url)| (id, title.to_string(), url.to_string()))
            .collect();
    }

    fn set_groups(&self, groups: Vec<(i64, &str)>) {
        *self.groups.lock().unwrap() = groups
            .into_iter()
            .map(|(id, title)| (id, title.to_string()))
            .collect();
    }

    fn set_omit_items(&self, v: bool) {
        self.omit_items.store(v, Ordering::SeqCst);
    }

    fn set_omit_unread(&self, v: bool) {
        self.omit_unread.store(v, Ordering::SeqCst);
    }

    fn set_omit_saved(&self, v: bool) {
        self.omit_saved.store(v, Ordering::SeqCst);
    }

    fn set_omit_feeds(&self, v: bool) {
        self.omit_feeds.store(v, Ordering::SeqCst);
    }

    fn set_omit_groups(&self, v: bool) {
        self.omit_groups.store(v, Ordering::SeqCst);
    }

    fn set_omit_feeds_side_feeds_groups(&self, v: bool) {
        self.omit_feeds_side_feeds_groups.store(v, Ordering::SeqCst);
    }

    fn set_omit_groups_side_feeds_groups(&self, v: bool) {
        self.omit_groups_side_feeds_groups
            .store(v, Ordering::SeqCst);
    }

    fn set_raw_item_id_json(&self, raw: Option<&str>) {
        *self.raw_item_id_json.lock().unwrap() = raw.map(str::to_string);
    }

    fn set_raw_unread_csv(&self, raw: Option<&str>) {
        *self.raw_unread_csv.lock().unwrap() = raw.map(str::to_string);
    }

    /// LIMIT 之后按 id 丢弃（EntryBeforeDisplay 式过滤，含 with_ids 通道）
    fn hook_drop_entry(&self, id: i64) {
        self.hook_drop_ids.lock().unwrap().insert(id);
    }

    /// 合成：仅列表窗口隐藏（with_ids 仍可取；不代表 hook）
    fn synthetic_listing_skip(&self, id: i64) {
        self.listing_skip_ids.lock().unwrap().insert(id);
    }

    fn set_raw_response(&self, raw: Option<RawResponse>) {
        *self.raw_response.lock().unwrap() = raw;
    }

    fn set_malformed_json_action(&self, action: Option<&str>) {
        *self.malformed_json_action.lock().unwrap() = action.map(str::to_string);
    }

    fn set_max_id_fail_at(&self, n: usize) {
        *self.max_id_fail_at.lock().unwrap() = Some(n);
    }

    fn set_since_page_fail_at(&self, n: usize) {
        *self.since_id_fail_at.lock().unwrap() = Some(n);
    }

    fn set_oversize_html_bytes(&self, n: usize) {
        self.oversize_html_bytes.store(n, Ordering::SeqCst);
    }

    fn entry_read(&self, id: i64) -> bool {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.read)
            .unwrap_or(false)
    }

    fn entry_saved(&self, id: i64) -> bool {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.saved)
            .unwrap_or(false)
    }

    fn hits(&self) -> Vec<Hit> {
        self.hits.lock().unwrap().clone()
    }

    fn since_id_hits(&self) -> Vec<Hit> {
        self.hits()
            .into_iter()
            .filter(|h| h.query_param("since_id").is_some())
            .collect()
    }

    fn max_id_hits(&self) -> Vec<Hit> {
        self.hits()
            .into_iter()
            .filter(|h| h.query_param("max_id").is_some())
            .collect()
    }

    fn render_id_value(&self, id: i64) -> serde_json::Value {
        match self.id_style {
            IdStyle::DecimalString => serde_json::Value::String(id.to_string()),
            IdStyle::JsonNumber => serde_json::Value::from(id),
        }
    }

    fn render_feed_id(&self, id: i64) -> serde_json::Value {
        if self.feed_id_as_string {
            serde_json::Value::String(id.to_string())
        } else {
            serde_json::Value::from(id)
        }
    }

    fn render_item(&self, e: &FEntry) -> serde_json::Value {
        let id_value = match self.raw_item_id_json.lock().unwrap().clone() {
            Some(raw) => serde_json::from_str(&raw).expect("raw_item_id_json 必须是合法 JSON 文本"),
            None => self.render_id_value(e.id),
        };
        let oversize = self.oversize_html_bytes.load(Ordering::SeqCst);
        let html = if oversize > 0 {
            "x".repeat(oversize)
        } else {
            format!("<p>{}</p>", e.title)
        };
        serde_json::json!({
            "id": id_value,
            "feed_id": self.render_feed_id(e.feed_id),
            "title": e.title,
            "author": "a",
            "html": html,
            "url": e.url.clone().unwrap_or_else(|| format!("https://e.example/{}/{}", e.feed_id, e.id)),
            "is_saved": if e.saved { 1 } else { 0 },
            "is_read": if e.read { 1 } else { 0 },
            "created_on_time": 1_700_000_000 + (e.id % 1000),
        })
    }

    /// 权威未读集合（CSV，十进制字面值）。
    fn unread_csv(&self) -> String {
        if let Some(raw) = self.raw_unread_csv.lock().unwrap().clone() {
            return raw;
        }
        self.entries
            .lock()
            .unwrap()
            .iter()
            .filter(|e| !e.read)
            .map(|e| e.id.to_string())
            .collect::<Vec<_>>()
            .join(",")
    }

    fn saved_csv(&self) -> String {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.saved)
            .map(|e| e.id.to_string())
            .collect::<Vec<_>>()
            .join(",")
    }

    /// LIMIT 之后再应用 EntryBeforeDisplay 式过滤（列表窗口专用）。
    fn apply_hook(&self, page: Vec<FEntry>) -> Vec<FEntry> {
        let drop = self.hook_drop_ids.lock().unwrap();
        page.into_iter().filter(|e| !drop.contains(&e.id)).collect()
    }
}

async fn handle_conn(mut stream: TcpStream, srv: Arc<Fixture>) -> std::io::Result<()> {
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
    let query = path_query
        .split_once('?')
        .map(|(_, q)| q.to_string())
        .unwrap_or_default();

    let hit = Hit {
        method: method.clone(),
        path: path.clone(),
        query: query.clone(),
        form: if body.is_empty() {
            Vec::new()
        } else {
            parse_form(&body)
        },
    };
    srv.hits.lock().unwrap().push(hit.clone());

    // HTTP 层用例（超限/压缩）：原样回字节，绕过 JSON 路由。
    let raw = { srv.raw_response.lock().unwrap().clone() };
    if let Some(raw) = raw {
        return write_raw_resp(&mut stream, &raw).await;
    }

    let (status, response) = route(&srv, &hit);
    write_resp(&mut stream, status, &response).await
}

/// 原样响应：支持 chunked（无 Content-Length）与附加头（Content-Encoding 等）。
async fn write_raw_resp(stream: &mut TcpStream, raw: &RawResponse) -> std::io::Result<()> {
    let mut head = String::from("HTTP/1.1 200 OK\r\nConnection: close\r\n");
    if raw.chunked {
        head.push_str("Transfer-Encoding: chunked\r\n");
    }
    for (k, v) in &raw.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    if !raw.chunked {
        head.push_str(&format!("Content-Length: {}\r\n", raw.body.len()));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    if raw.chunked {
        for piece in raw.body.chunks(16384) {
            stream
                .write_all(format!("{:x}\r\n", piece.len()).as_bytes())
                .await?;
            stream.write_all(piece).await?;
            stream.write_all(b"\r\n").await?;
        }
        stream.write_all(b"0\r\n\r\n").await?;
    } else {
        stream.write_all(&raw.body).await?;
    }
    stream.flush().await
}

/// 路由 + 严格校验。返回 (状态码, 响应体)。
fn route(srv: &Fixture, hit: &Hit) -> (u16, String) {
    // FreshRSS 的 fever.php 只处理 POST（api_key 在 $_POST）。
    if hit.method != "POST" {
        return (405, r#"{"error_message":"POST only"}"#.into());
    }
    if hit.path != srv.endpoint_path {
        return (404, r#"{"error_message":"not found"}"#.into());
    }
    // api_key 只读 POST form body（FreshRSS `p/api/fever.php:172`）；不匹配 → auth=0。
    let body_key = hit.form_first("api_key").unwrap_or_default();
    if body_key != API_KEY {
        return (
            200,
            serde_json::json!({ "api_version": 4, "auth": 0 }).to_string(),
        );
    }

    let q = query_map(&hit.query);
    if let Some(action) = srv.malformed_json_action.lock().unwrap().clone() {
        if q.contains_key(&action) {
            return (200, "not-json".into());
        }
    }
    let mut resp = serde_json::Map::new();
    resp.insert("api_version".into(), serde_json::Value::from(4));
    resp.insert("auth".into(), serde_json::Value::from(1));
    resp.insert(
        "last_refreshed_on_time".into(),
        serde_json::Value::from(1_700_000_500),
    );

    // groups / feeds：FreshRSS 两种 action 都带 feeds_groups。
    if q.contains_key("groups") || q.contains_key("feeds") {
        let groups: Vec<serde_json::Value> = srv
            .groups
            .lock()
            .unwrap()
            .iter()
            .map(
                |(id, title)| serde_json::json!({ "id": srv.render_id_value(*id), "title": title }),
            )
            .collect();
        let feeds: Vec<serde_json::Value> = srv
            .feeds
            .lock()
            .unwrap()
            .iter()
            .map(|(id, title, url)| {
                serde_json::json!({
                    "id": srv.render_feed_id(*id),
                    "title": title,
                    "url": url,
                    "site_url": "",
                })
            })
            .collect();
        // feeds_groups：把全部 feed 归入第一个 group（有 group 才有归属）。
        let feed_ids_csv = srv
            .feeds
            .lock()
            .unwrap()
            .iter()
            .map(|(id, _, _)| id.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let feeds_groups: Vec<serde_json::Value> = srv
            .groups
            .lock()
            .unwrap()
            .first()
            .map(|(gid, _)| {
                vec![serde_json::json!({
                    "group_id": srv.render_feed_id(*gid),
                    "feed_ids": feed_ids_csv,
                })]
            })
            .unwrap_or_default();
        if q.contains_key("groups") && !srv.omit_groups.load(Ordering::SeqCst) {
            resp.insert("groups".into(), serde_json::Value::Array(groups));
        }
        if q.contains_key("feeds") && !srv.omit_feeds.load(Ordering::SeqCst) {
            resp.insert("feeds".into(), serde_json::Value::Array(feeds));
        }
        if q.contains_key("groups") && !srv.omit_groups_side_feeds_groups.load(Ordering::SeqCst) {
            resp.insert(
                "feeds_groups".into(),
                serde_json::Value::Array(feeds_groups.clone()),
            );
        }
        if q.contains_key("feeds") && !srv.omit_feeds_side_feeds_groups.load(Ordering::SeqCst) {
            resp.insert(
                "feeds_groups".into(),
                serde_json::Value::Array(feeds_groups),
            );
        }
    }

    if q.contains_key("unread_item_ids") && !srv.omit_unread.load(Ordering::SeqCst) {
        resp.insert(
            "unread_item_ids".into(),
            serde_json::Value::String(srv.unread_csv()),
        );
    }
    if q.contains_key("saved_item_ids") && !srv.omit_saved.load(Ordering::SeqCst) {
        resp.insert(
            "saved_item_ids".into(),
            serde_json::Value::String(srv.saved_csv()),
        );
    }

    if q.contains_key("items") && !srv.omit_items.load(Ordering::SeqCst) {
        if let Some((status, body)) = items_failure(srv, &q) {
            return (status, body);
        }
        let page = items_page(srv, &q);
        let items: Vec<serde_json::Value> = page.iter().map(|e| srv.render_item(e)).collect();
        resp.insert("items".into(), serde_json::Value::Array(items));
        resp.insert(
            "total_items".into(),
            serde_json::Value::from(srv.entries.lock().unwrap().len()),
        );
    }

    // mark=item：FreshRSS 只接受单个 ctype_digit 的 id（逗号分隔无效）。
    if q.get("mark").map(|m| m == "item").unwrap_or(false) {
        let as_ = q.get("as").cloned().unwrap_or_default();
        let id_str = q.get("id").cloned().unwrap_or_default();
        if id_str.is_empty() || !id_str.chars().all(|c| c.is_ascii_digit()) {
            // 批量逗号 id / 非十进制 id 一律 400：生产实现若回退成批量会被此断言抓到。
            return (
                400,
                r#"{"error_message":"invalid id: Fever mark 只接受单个十进制 id"}"#.into(),
            );
        }
        let id: i64 = id_str.parse().unwrap();
        {
            let mut entries = srv.entries.lock().unwrap();
            if let Some(e) = entries.iter_mut().find(|e| e.id == id) {
                match as_.as_str() {
                    "read" => e.read = true,
                    "unread" => e.read = false,
                    "saved" => e.saved = true,
                    "unsaved" => e.saved = false,
                    _ => {}
                }
            }
        }
        match as_.as_str() {
            "read" | "unread" => {
                resp.insert(
                    "unread_item_ids".into(),
                    serde_json::Value::String(srv.unread_csv()),
                );
            }
            "saved" | "unsaved" => {
                resp.insert(
                    "saved_item_ids".into(),
                    serde_json::Value::String(srv.saved_csv()),
                );
            }
            _ => {}
        }
    }

    (200, serde_json::Value::Object(resp).to_string())
}

/// 分页故障注入：第 N 次 max_id / since_id 调用返回 500（1 起算）。
fn items_failure(srv: &Fixture, q: &HashMap<String, String>) -> Option<(u16, String)> {
    if let Some(raw) = q.get("max_id") {
        if raw.parse::<i64>().is_ok() {
            let n = srv.max_id_calls.fetch_add(1, Ordering::SeqCst) + 1;
            let fail_at = *srv.max_id_fail_at.lock().unwrap();
            if fail_at == Some(n) {
                return Some((500, r#"{"error_message":"injected max_id failure"}"#.into()));
            }
        }
    }
    if let Some(raw) = q.get("since_id") {
        if raw.parse::<i64>().is_ok() {
            let n = srv.since_id_calls.fetch_add(1, Ordering::SeqCst) + 1;
            let fail_at = *srv.since_id_fail_at.lock().unwrap();
            if fail_at == Some(n) {
                return Some((
                    500,
                    r#"{"error_message":"injected since_id failure"}"#.into(),
                ));
            }
        }
    }
    None
}

/// 计算 items 页。列表窗口严格按 FreshRSS 的顺序：先排序截断（`LIMIT 50`），
/// **再**应用 EntryBeforeDisplay 式过滤——过滤后的非空短页不代表没有后页。
fn items_page(srv: &Fixture, q: &HashMap<String, String>) -> Vec<FEntry> {
    if let Some(raw) = q.get("with_ids") {
        // 精确 id 集合查询（FreshRSS 的 id IN (...)）。R2 更正：`getItems()` 对
        // **所有**分支统一调用 EntryBeforeDisplay hook——with_ids 同样过 hook
        // （被 hook 丢弃的条目对 Fever 整体不可见）。
        let ids: HashSet<i64> = raw
            .split(',')
            .filter_map(|s| s.trim().parse::<i64>().ok())
            .collect();
        let mut out: Vec<FEntry> = srv
            .entries
            .lock()
            .unwrap()
            .iter()
            .filter(|e| ids.contains(&e.id))
            .cloned()
            .collect();
        out.sort_by_key(|e| e.id);
        out.truncate(50);
        return srv.apply_hook(out);
    }
    // 列表窗口：合成开关（listing_skip_ids）只在这里生效——用于「列表窗口与
    // id 通道存在差异」的**合成**用例，不代表 hook（hook 由 apply_hook 统一施加）。
    let skip = srv.listing_skip_ids.lock().unwrap().clone();
    let all: Vec<FEntry> = srv
        .entries
        .lock()
        .unwrap()
        .iter()
        .filter(|e| !skip.contains(&e.id))
        .cloned()
        .collect();
    if let Some(raw) = q.get("max_id") {
        if let Ok(mid) = raw.parse::<i64>() {
            if !srv.ignore_max_id {
                let mut candidates: Vec<FEntry> = all.into_iter().filter(|e| e.id < mid).collect();
                // 固定实现语义：取「紧邻 max_id 的最近 50 条」（两家源码均 DESC 选）。
                candidates.sort_by_key(|e| std::cmp::Reverse(e.id));
                candidates.truncate(50);
                if srv.max_id_asc_page {
                    candidates.reverse();
                }
                let mut page = srv.apply_hook(candidates);
                if srv.dup_echo && !page.is_empty() {
                    // 分页窗口重复条目（夹具缺陷注入）：锁生产侧去重。
                    let first = page[0].clone();
                    page.push(first);
                }
                return page;
            }
        }
    }
    if let Some(raw) = q.get("since_id") {
        if let Ok(sid) = raw.parse::<i64>() {
            let mut out: Vec<FEntry> = all.into_iter().filter(|e| e.id > sid).collect();
            out.sort_by_key(|e| e.id);
            out.truncate(50);
            return srv.apply_hook(out);
        }
    }
    // 无参（或 ignore_max_id）：最近 50 条。
    let mut page = all;
    page.sort_by_key(|e| std::cmp::Reverse(e.id));
    page.truncate(50);
    srv.apply_hook(page)
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

async fn write_resp(stream: &mut TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
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

fn query_map(query: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for kv in query.split('&') {
        if kv.is_empty() {
            continue;
        }
        match kv.split_once('=') {
            Some((k, v)) => {
                out.insert(k.to_string(), url_decode(v));
            }
            None => {
                out.insert(kv.to_string(), String::new());
            }
        }
    }
    out
}

fn url_decode(s: &str) -> String {
    // 与 mock_greader / greader_compat_e2e 同口径：逐字节解码 %XX 后整体按 UTF-8 解释。
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
共享测试脚手架
============================================================ */

type TestDb = Arc<tokio::sync::Mutex<rusqlite::Connection>>;

/// 建临时库 + Fever 账号设置，返回 (db, 临时路径)。
fn seed_db(tag: &str, srv: &Fixture) -> (TestDb, std::path::PathBuf) {
    let tmp = common::unique_db_path(tag);
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).expect("open db");
    db::set_setting(&conn, "sync_protocol", "fever").unwrap();
    db::set_setting(&conn, "greader_endpoint", &srv.url()).unwrap();
    db::set_setting(&conn, "greader_username", USER).unwrap();
    db::set_setting(&conn, "greader_password", PASS).unwrap();
    let account = app_lib::sync::current_account(&conn)
        .unwrap()
        .expect("test account");
    db::mark_zero_binding_discovery_attempt(&conn, &account.key, chrono::Utc::now().timestamp())
        .unwrap();
    (Arc::new(tokio::sync::Mutex::new(conn)), tmp)
}

fn http() -> reqwest::Client {
    app_lib::ingestion::build_client(10)
}

fn article_count(conn: &rusqlite::Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
        .unwrap()
}

fn article_state(conn: &rusqlite::Connection, remote_id: i64) -> Option<(bool, bool)> {
    conn.query_row(
        "SELECT is_read, is_starred FROM articles WHERE remote_id = ?1",
        [remote_id],
        |r| Ok((r.get::<_, i64>(0)? != 0, r.get::<_, i64>(1)? != 0)),
    )
    .ok()
}

fn bound_remote_id(conn: &rusqlite::Connection, remote_id: i64) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM articles WHERE remote_id = ?1",
        [remote_id],
        |r| r.get::<_, i64>(0),
    )
    .unwrap()
        == 1
}

/// 读 Fever 历史状态原始值（内部 settings，无 schema 版本）。
/// None = 键缺失（Unknown）；"pending:<id>" = 未完成；"complete" = 已取尽。
/// 测试直接断言原始编码，便于看清「状态没被提前当完成」。
fn history_state(conn: &rusqlite::Connection) -> Option<String> {
    conn.query_row(
        "SELECT value FROM settings WHERE key = 'fever_history_state'",
        [],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

/// 注入：settings 中指定 key 的写入失败（**真 DB trigger**）。settings 写入是
/// UPSERT，INSERT/UPDATE 两条触发路径都覆盖，确保任何写尝试都被 ABORT。
fn install_settings_write_failure(conn: &rusqlite::Connection, key: &str) {
    conn.execute_batch(&format!(
        "CREATE TRIGGER opt005_fail_settings_ins BEFORE INSERT ON settings \
         WHEN NEW.key = '{key}' \
         BEGIN SELECT RAISE(ABORT, 'injected settings write failure'); END;
         CREATE TRIGGER opt005_fail_settings_upd BEFORE UPDATE ON settings \
         WHEN NEW.key = '{key}' \
         BEGIN SELECT RAISE(ABORT, 'injected settings write failure'); END;"
    ))
    .unwrap();
}

fn remove_settings_write_failure(conn: &rusqlite::Connection) {
    conn.execute_batch(
        "DROP TRIGGER IF EXISTS opt005_fail_settings_ins;
         DROP TRIGGER IF EXISTS opt005_fail_settings_upd;",
    )
    .unwrap();
}

/// 注入：指定 guid 的文章 INSERT 失败（真 DB trigger）。
fn install_article_insert_failure(conn: &rusqlite::Connection, guid: &str) {
    conn.execute_batch(&format!(
        "CREATE TRIGGER opt005_fail_article_ins BEFORE INSERT ON articles \
         WHEN NEW.guid = '{guid}' \
         BEGIN SELECT RAISE(ABORT, 'injected article insert failure'); END;"
    ))
    .unwrap();
}

fn remove_article_insert_failure(conn: &rusqlite::Connection) {
    conn.execute_batch("DROP TRIGGER IF EXISTS opt005_fail_article_ins;")
        .unwrap();
}

/// 注入：指定 guid 的文章 UPDATE 失败（真 DB trigger）。R3 失败页标记测试用——
/// 重放/回填路径对既有行走 UPDATE（INSERT 注入打不到），触发器随页事务一起
/// 阻断并回滚。
fn install_article_update_failure(conn: &rusqlite::Connection, guid: &str) {
    conn.execute_batch(&format!(
        "CREATE TRIGGER opt007_fail_article_upd BEFORE UPDATE ON articles \
         WHEN OLD.guid = '{guid}' \
         BEGIN SELECT RAISE(ABORT, 'injected article update failure'); END;"
    ))
    .unwrap();
}

/// 极简 base64 解码（测试用；忽略 `=` 与空白）。载荷由 node zlib 预生成。
fn base64_decode(input: &str) -> Vec<u8> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::new();
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for &c in input.as_bytes() {
        let Some(v) = val(c) else { continue };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xFF) as u8);
        }
    }
    out
}

/// 预置一个已绑定远端 feed id=10 的本地源：增量用例无需先跑全量 feeds。
fn seed_bound_feed(conn: &rusqlite::Connection) {
    let folder = db::create_folder(conn, "分类", "article").unwrap();
    let feed = db::insert_feed(
        conn,
        "http://example.com/h.rss",
        None,
        "源",
        None,
        folder,
        "inherit",
        false,
        false,
    )
    .unwrap();
    db::set_feed_remote_id(conn, feed, 10).unwrap();
}

fn seed_remote_feed_and_article(conn: &rusqlite::Connection) -> (i64, i64) {
    let folder = db::create_folder(conn, "远端分类", "article").unwrap();
    let feed = db::insert_feed_origin(
        conn,
        "http://example.com/remote.rss",
        None,
        "远端源",
        None,
        folder,
        "inherit",
        false,
        false,
        "remote",
    )
    .unwrap();
    db::set_feed_remote_id(conn, feed, 10).unwrap();
    let article = db::NewArticle {
        guid: "remote-125".into(),
        url: Some("https://e.example/125".into()),
        title: "远端条目".into(),
        author: None,
        summary: None,
        content_html: None,
        body_text: "body".into(),
        image_url: None,
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: Some("2026-01-01T00:00:00+00:00".into()),
        source: "remote".into(),
    };
    let article_id = db::upsert_article_with_feed(conn, feed, &article, false)
        .unwrap()
        .0;
    db::set_article_remote_id(conn, article_id, 125).unwrap();
    (feed, article_id)
}

fn seed_unbound_remote_feed(conn: &rusqlite::Connection, feed_url: &str) -> i64 {
    let folder = db::create_folder(conn, "未绑定远端分类", "article").unwrap();
    db::insert_feed_origin(
        conn,
        feed_url,
        None,
        "未绑定远端源",
        None,
        folder,
        "inherit",
        false,
        false,
        "remote",
    )
    .unwrap()
}

fn seed_local_article(conn: &rusqlite::Connection) -> i64 {
    let folder = db::create_folder(conn, "本地分类", "article").unwrap();
    let feed = db::insert_feed(
        conn,
        "https://local.example/feed.xml",
        None,
        "本地源",
        None,
        folder,
        "inherit",
        false,
        false,
    )
    .unwrap();
    let article = db::NewArticle {
        guid: "local-125".into(),
        url: Some("https://local.example/125".into()),
        title: "本地条目".into(),
        author: None,
        summary: None,
        content_html: None,
        body_text: "body".into(),
        image_url: None,
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: Some("2026-01-01T00:00:00+00:00".into()),
        source: "direct".into(),
    };
    db::upsert_article_with_feed(conn, feed, &article, false)
        .unwrap()
        .0
}

fn feed_count(conn: &rusqlite::Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM feeds", [], |r| r.get(0))
        .unwrap()
}

/* ============================================================
A. FreshRSS 字符串长 id：读取、落库、mark 回传都是十进制字面值
============================================================ */

#[tokio::test]
async fn freshrss_string_long_ids_keep_decimal_literal_end_to_end() {
    let srv = Fixture::start_with(FixtureOptions {
        id_style: IdStyle::DecimalString,
        // 连 feed_id 也用字符串形态：契约要求两者都接受。
        feed_id_as_string: true,
        ..Default::default()
    })
    .await;
    srv.set_groups(vec![(1, "分类")]);
    srv.set_feeds(vec![(LONG_FEED_ID, "源", "http://example.com/f.rss")]);
    srv.add_entry(LONG_ID, LONG_FEED_ID, "长 id 条目", false, false);

    let (db, tmp) = seed_db("fever_compat_long_id", &srv);
    let report = sync::sync_now(&db, &http())
        .await
        .expect("full sync 应成功");
    assert!(
        report.errors.is_empty(),
        "字符串长 id 不得被当成协议错误：{:?}",
        report.errors
    );

    {
        let conn = db.lock().await;
        // 数据库 remote_id 必须是十进制字面值（旧 len==16 十六进制猜测会落另一个数）。
        assert!(
            bound_remote_id(&conn, LONG_ID),
            "remote_id 应为十进制 {LONG_ID}（误按十六进制会落成 0x{LONG_ID:x}）"
        );
        assert_eq!(article_count(&conn), 1);
        let (read, starred) = article_state(&conn, LONG_ID).expect("条目应已绑定");
        assert!(!read && !starred, "初始未读未收藏");
        let feed_remote: Option<i64> = conn
            .query_row(
                "SELECT remote_id FROM feeds WHERE feed_url = 'http://example.com/f.rss'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(feed_remote, Some(LONG_FEED_ID), "feed 数字 id 应绑定");
    }

    // 生产 FeverClient 直读：id 保持十进制字面值，状态 tag 一致。
    let client = FeverClient::new(&srv.url(), USER, PASS, http()).at_resolved(&srv.api_root());
    let items = client
        .items_with_ids(&[LONG_ID])
        .await
        .expect("with_ids 读取应成功");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, LONG_ID.to_string(), "id 必须是十进制字面值");
    assert!(
        !app_lib::greader::has_tag(&items[0].categories, "/com.google/read"),
        "初始应为未读"
    );

    // 本地标读 + 收藏 → push：wire 上的 id 必须是同一个十进制字面值。
    {
        let conn = db.lock().await;
        let aid: i64 = conn
            .query_row(
                "SELECT id FROM articles WHERE remote_id = ?1",
                [LONG_ID],
                |r| r.get(0),
            )
            .unwrap();
        db::set_read(&conn, aid, true).unwrap();
        db::enqueue_sync(&conn, Some(aid), None, "read", None).unwrap();
        db::set_starred(&conn, aid, true).unwrap();
        db::enqueue_sync(&conn, Some(aid), None, "star", None).unwrap();
    }
    let http_client = http();
    sync::push_states_now(&db, &http_client).await;

    assert!(srv.entry_read(LONG_ID), "标读必须真实到达夹具");
    assert!(srv.entry_saved(LONG_ID), "收藏必须真实到达夹具");
    {
        let conn = db.lock().await;
        let (read, starred) = article_state(&conn, LONG_ID).unwrap();
        assert!(read && starred, "本地状态应保持已读已收藏");
    }

    let marks: Vec<Hit> = srv
        .hits()
        .into_iter()
        .filter(|h| h.query_param("mark").as_deref() == Some("item"))
        .collect();
    assert!(!marks.is_empty(), "应有 mark 请求");
    for h in &marks {
        let id = h.query_param("id").expect("mark 必须带 id");
        assert_eq!(
            id,
            LONG_ID.to_string(),
            "mark 的 id 必须是十进制字面值（不得十六进制/不得逗号批量）"
        );
    }
    // 任何请求 query 都不得夹带 api_key（TASK-101 防回退）。
    assert!(
        srv.hits().iter().all(|h| !h.query.contains("api_key")),
        "api_key 不得出现在 query"
    );

    let _ = std::fs::remove_file(&tmp);
}

/// Miniflux 形态（JSON 数字）不回归；长 id 数字照常按十进制处理。
#[tokio::test]
async fn json_number_ids_accepted_and_bound() {
    let srv = Fixture::start_with(FixtureOptions {
        id_style: IdStyle::JsonNumber,
        ..Default::default()
    })
    .await;
    srv.set_feeds(vec![(10, "源", "http://example.com/n.rss")]);
    srv.add_entry(LONG_ID, 10, "数字 id", true, false);

    let (db, tmp) = seed_db("fever_compat_num_id", &srv);
    let report = sync::sync_now(&db, &http()).await.expect("sync");
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    {
        let conn = db.lock().await;
        assert!(bound_remote_id(&conn, LONG_ID));
        let (read, _) = article_state(&conn, LONG_ID).unwrap();
        assert!(read, "远端已读应合并");
    }
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
B. max_id 完整历史：125 篇全部可达 / 幂等 / 增量只走 since_id
============================================================ */

/// 构造 125 篇：1..=100 已读非收藏（其中 1..=75 在最近 50 以外，只能靠
/// max_id 回溯到达）、101..=125 未读。返回夹具。
async fn fixture_125() -> Arc<Fixture> {
    let srv = Fixture::start().await;
    srv.set_groups(vec![(1, "分类")]);
    srv.set_feeds(vec![(10, "源", "http://example.com/h.rss")]);
    for id in 1..=125 {
        let read = id <= 100;
        srv.add_entry(id, 10, &format!("条目{id}"), read, false);
    }
    srv
}

#[tokio::test]
async fn full_sync_reaches_all_125_including_read_non_saved_history() {
    // 正常样本（无 hook 过滤）：125 篇历史应全量可达。
    // R2 更正：hook 对 items 全分支（含 with_ids）生效，带 hook 的样本不能再
    // 冒充「列表遗漏但 id 可取」——该形态单独用合成开关在专用用例里锁。
    let srv = fixture_125().await;

    let (db, tmp) = seed_db("fever_compat_history", &srv);
    let http_client = http();

    let report = sync::sync_now(&db, &http_client)
        .await
        .expect("full sync 应成功");
    assert!(report.errors.is_empty(), "不应有错误：{:?}", report.errors);
    assert_eq!(
        report.pulled_entries, 125,
        "每条应恰好合并一次（重复/遗漏都会偏离 125）"
    );

    {
        let conn = db.lock().await;
        assert_eq!(article_count(&conn), 125, "125 篇历史必须全部可达");
        // 最近 50 以外的已读非收藏条目必须有。
        for id in [1_i64, 42, 75] {
            let (read, starred) = article_state(&conn, id)
                .unwrap_or_else(|| panic!("条目 {id} 应已落库（max_id 回溯）"));
            assert!(read, "条目 {id} 远端已读应合并");
            assert!(!starred, "条目 {id} 未收藏");
        }
        let (read_101, _) = article_state(&conn, 101).expect("101 应落库");
        assert!(!read_101, "101 远端未读应保持未读");
        assert_eq!(
            db::last_sync_entry_id(&conn).unwrap(),
            125,
            "游标应为最大 id"
        );
        assert_eq!(
            history_state(&conn).as_deref(),
            Some("complete"),
            "全量重放取尽后状态为 Complete"
        );
    }

    // 第二次 full：幂等（不重复建、不重复处理）。
    let report2 = sync::sync_now(&db, &http_client)
        .await
        .expect("第二次 full sync");
    assert!(report2.errors.is_empty(), "{:?}", report2.errors);
    assert_eq!(report2.pulled_entries, 125, "第二次仍是 125 条唯一合并");
    {
        let conn = db.lock().await;
        assert_eq!(article_count(&conn), 125, "幂等：不得重复");
    }

    // 服务端新增两条 → 轻量同步只走 since_id 增量。
    srv.add_entry(126, 10, "条目126", true, false);
    srv.add_entry(127, 10, "条目127", false, false);
    let before_light = srv.hits().len();
    let light = sync::sync_light(&db, &http_client)
        .await
        .expect("light sync");
    assert!(light.errors.is_empty(), "{:?}", light.errors);
    assert!(
        light.pulled_entries >= 2,
        "增量至少应合并新增两条（with_ids 补权威集合可能另有合并）"
    );
    {
        let conn = db.lock().await;
        assert_eq!(article_count(&conn), 127);
        assert!(bound_remote_id(&conn, 127));
        assert_eq!(db::last_sync_entry_id(&conn).unwrap(), 127);
    }
    let since_hits = srv.since_id_hits();
    assert!(!since_hits.is_empty(), "增量应有 since_id 请求");
    assert_eq!(
        since_hits[0].query_param("since_id").as_deref(),
        Some("125"),
        "增量应从上次游标 125 起"
    );
    assert!(
        since_hits
            .iter()
            .all(|h| h.query_param("since_id").as_deref() == Some("125")
                || h.query_param("since_id").as_deref() == Some("127")),
        "非空短页不停：增量页序应为 125 → 127 → 空页：{:?}",
        since_hits
            .iter()
            .map(|h| h.query_param("since_id"))
            .collect::<Vec<_>>()
    );
    let new_hits = srv.hits()[before_light..].to_vec();
    assert!(
        new_hits.iter().all(|h| h.query_param("max_id").is_none()),
        "增量不得再走 max_id 全历史回溯：{:?}",
        new_hits.iter().map(|h| h.query.clone()).collect::<Vec<_>>()
    );

    let _ = std::fs::remove_file(&tmp);
}

/// **泛化鲁棒性对照**（不是 Miniflux 事实——固定源码与 FreshRSS 同为 DESC）：
/// 假定某实现按升序返回 max_id 页；游标取页内最小 id，与顺序无关。
#[tokio::test]
async fn max_id_page_order_is_irrelevant_generic_robustness() {
    let srv = Fixture::start_with(FixtureOptions {
        max_id_asc_page: true,
        ..Default::default()
    })
    .await;
    srv.set_feeds(vec![(10, "源", "http://example.com/h.rss")]);
    for id in 1..=125 {
        srv.add_entry(id, 10, &format!("条目{id}"), id <= 100, false);
    }

    let (db, tmp) = seed_db("fever_compat_history_asc", &srv);
    let report = sync::sync_now(&db, &http()).await.expect("sync");
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    {
        let conn = db.lock().await;
        assert_eq!(article_count(&conn), 125);
    }
    let _ = std::fs::remove_file(&tmp);
}

#[tokio::test]
async fn overlapping_history_pages_are_deduplicated() {
    let srv = Fixture::start_with(FixtureOptions {
        dup_echo: true,
        ..Default::default()
    })
    .await;
    srv.set_feeds(vec![(10, "源", "http://example.com/h.rss")]);
    for id in 1..=125 {
        srv.add_entry(id, 10, &format!("条目{id}"), id <= 100, false);
    }

    let (db, tmp) = seed_db("fever_compat_dedup", &srv);
    let report = sync::sync_now(&db, &http()).await.expect("sync");
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(
        report.pulled_entries, 125,
        "重复页条目必须去重（无去重会 >125）"
    );
    {
        let conn = db.lock().await;
        assert_eq!(article_count(&conn), 125);
    }
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
C. 失败与边界：分页失败可重试 / 游标不前进显式失败 / 空库正常
============================================================ */

#[tokio::test]
async fn history_page_failure_is_explicit_and_retryable() {
    let srv = fixture_125().await;
    // 第 2 次 max_id 调用（即历史第 2 页）返回 500。
    srv.set_max_id_fail_at(2);

    let (db, tmp) = seed_db("fever_compat_retry", &srv);
    let http_client = http();
    let report = sync::sync_now(&db, &http_client)
        .await
        .expect("同步整体应完成（错误记入 report，不 panic）");
    assert!(
        report.errors.iter().any(|e| e.contains("历史")),
        "分页失败必须显式记录：{:?}",
        report.errors
    );
    {
        let conn = db.lock().await;
        // 第一页（最近 50 条，76..=125）已导入并保留——可恢复状态。
        assert_eq!(article_count(&conn), 50, "失败前已拉的页应保留");
        assert!(bound_remote_id(&conn, 125));
        assert!(
            !bound_remote_id(&conn, 1),
            "未拉到的历史在失败轮不得出现（也不得伪成功）"
        );
        assert_eq!(db::last_sync_entry_id(&conn).unwrap(), 125);
        // 未完成历史必须留下 pending 续取点（下一页起点 = 最后一页的最小 id）。
        assert_eq!(
            history_state(&conn).as_deref(),
            Some("pending:76"),
            "失败后必须保存历史续取状态"
        );
    }

    // 重试（故障只注入一次，full 全量重放）：全历史补齐，状态转 Complete。
    let report2 = sync::sync_now(&db, &http_client).await.expect("retry sync");
    assert!(report2.errors.is_empty(), "{:?}", report2.errors);
    {
        let conn = db.lock().await;
        assert_eq!(article_count(&conn), 125, "重试后全历史可达");
        assert_eq!(
            history_state(&conn).as_deref(),
            Some("complete"),
            "取尽后状态必须为 Complete（与未完成状态分清）"
        );
    }
    let _ = std::fs::remove_file(&tmp);
}

#[tokio::test]
async fn non_advancing_history_cursor_fails_fast_instead_of_looping() {
    let srv = Fixture::start_with(FixtureOptions {
        ignore_max_id: true,
        ..Default::default()
    })
    .await;
    srv.set_feeds(vec![(10, "源", "http://example.com/h.rss")]);
    for id in 1..=125 {
        srv.add_entry(id, 10, &format!("条目{id}"), id <= 100, false);
    }

    let (db, tmp) = seed_db("fever_compat_cursor", &srv);
    let report = sync::sync_now(&db, &http()).await.expect("sync");
    assert!(
        report.errors.iter().any(|e| e.contains("游标")),
        "游标不前进必须显式失败：{:?}",
        report.errors
    );
    // 恰两次 max_id 请求：第一次拿页、第二次发现游标不前进即中止（不是无限循环）。
    assert_eq!(
        srv.max_id_hits().len(),
        2,
        "游标不前进应立即中止：{:?}",
        srv.max_id_hits()
            .iter()
            .map(|h| h.query.clone())
            .collect::<Vec<_>>()
    );
    {
        let conn = db.lock().await;
        assert_eq!(article_count(&conn), 50, "只保留首屏数据");
        // 游标卡死仍要留下 pending 续取点（下次 light 从 76 继续尝试，不放弃历史）。
        assert_eq!(history_state(&conn).as_deref(), Some("pending:76"));
    }
    let _ = std::fs::remove_file(&tmp);
}

#[tokio::test]
async fn empty_account_sync_finishes_cleanly() {
    let srv = Fixture::start().await;

    let (db, tmp) = seed_db("fever_compat_empty", &srv);
    let report = sync::sync_now(&db, &http()).await.expect("sync");
    assert!(
        report.errors.is_empty(),
        "空库/空集合是合法状态：{:?}",
        report.errors
    );
    {
        let conn = db.lock().await;
        assert_eq!(article_count(&conn), 0);
        assert_eq!(db::last_sync_entry_id(&conn).unwrap(), 0);
    }
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
D. 缺字段 / 非法 ID / 缺集合：协议错误，不得当空集合
============================================================ */

#[derive(Clone, Copy)]
enum SubscriptionCollection {
    Feeds,
    Groups,
    FeedsGroupsOnFeeds,
    FeedsGroupsOnGroups,
}

impl SubscriptionCollection {
    fn field_name(self) -> &'static str {
        match self {
            Self::Feeds => "feeds",
            Self::Groups => "groups",
            Self::FeedsGroupsOnFeeds | Self::FeedsGroupsOnGroups => "feeds_groups",
        }
    }

    fn omit(self, srv: &Fixture) {
        match self {
            Self::Feeds => srv.set_omit_feeds(true),
            Self::Groups => srv.set_omit_groups(true),
            Self::FeedsGroupsOnFeeds => srv.set_omit_feeds_side_feeds_groups(true),
            Self::FeedsGroupsOnGroups => srv.set_omit_groups_side_feeds_groups(true),
        }
    }
}

async fn assert_missing_subscription_collection_fails_closed(
    tag: &str,
    omitted: SubscriptionCollection,
) {
    let srv = Fixture::start().await;
    srv.set_feeds(vec![(10, "远端源", "http://example.com/remote.rss")]);
    srv.set_groups(vec![(20, "分类")]);
    omitted.omit(&srv);

    let (db, tmp) = seed_db(tag, &srv);
    {
        let conn = db.lock().await;
        seed_remote_feed_and_article(&conn);
    }
    let report = sync::feeds_phase(&db, &http())
        .await
        .expect("protocol error is reported without aborting the phase");
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains(omitted.field_name()) || e.contains("协议")),
        "缺 {} 必须可见：{:?}",
        omitted.field_name(),
        report.errors
    );
    let conn = db.lock().await;
    assert_eq!(feed_count(&conn), 1, "缺字段不得删除远端源");
    assert_eq!(article_count(&conn), 1, "文章应随源保留");
    drop(conn);
    let _ = std::fs::remove_file(&tmp);
}

#[tokio::test]
async fn missing_fever_feeds_collection_fails_closed() {
    assert_missing_subscription_collection_fails_closed(
        "fever_compat_missing_feeds",
        SubscriptionCollection::Feeds,
    )
    .await;
}

#[tokio::test]
async fn missing_fever_groups_collection_fails_closed() {
    assert_missing_subscription_collection_fails_closed(
        "fever_compat_missing_groups",
        SubscriptionCollection::Groups,
    )
    .await;
}

#[tokio::test]
async fn missing_fever_feeds_side_feeds_groups_collection_fails_closed() {
    assert_missing_subscription_collection_fails_closed(
        "fever_compat_missing_feeds_side_feeds_groups",
        SubscriptionCollection::FeedsGroupsOnFeeds,
    )
    .await;
}

#[tokio::test]
async fn missing_fever_groups_side_feeds_groups_collection_fails_closed() {
    assert_missing_subscription_collection_fails_closed(
        "fever_compat_missing_groups_side_feeds_groups",
        SubscriptionCollection::FeedsGroupsOnGroups,
    )
    .await;
}

#[tokio::test]
async fn explicit_empty_fever_subscription_collections_are_legal() {
    let srv = Fixture::start().await;
    let (db, tmp) = seed_db("fever_compat_empty_subscription_collections", &srv);
    {
        let conn = db.lock().await;
        seed_remote_feed_and_article(&conn);
    }
    let report = sync::feeds_phase(&db, &http())
        .await
        .expect("explicit empty feeds/groups/feeds_groups are valid");
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let conn = db.lock().await;
    assert_eq!(
        feed_count(&conn),
        0,
        "显式三空数组仍是权威空集，必须执行远端源删除对账"
    );
    assert_eq!(
        article_count(&conn),
        0,
        "权威空订阅删除源时应级联删除其文章"
    );
    drop(conn);
    let _ = std::fs::remove_file(&tmp);
}

/// 零远端 feed 绑定的 light sync 不请求条目/权威集合、不推进两类游标，
/// 不写 history complete；预置本地文章保持。
#[tokio::test]
async fn fever_light_sync_without_remote_feed_binding_skips_pull_entirely() {
    let srv = Fixture::start().await;
    srv.add_entry(125, 10, "远端条目", false, false);
    let (db, tmp) = seed_db("fever_compat_zero_binding", &srv);
    {
        let conn = db.lock().await;
        seed_local_article(&conn);
        db::set_last_sync_entry_id(&conn, 100).unwrap();
        db::set_last_sync_ts(&conn, 777_777).unwrap();
        db::set_fever_history_pending(&conn, 55).unwrap();
    }

    let report = sync::sync_light(&db, &http())
        .await
        .expect("no remote feed binding is a benign skip");
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(
        srv.hits().iter().all(|h| {
            h.query_param("items").is_none()
                && h.query_param("since_id").is_none()
                && h.query_param("max_id").is_none()
                && h.query_param("unread_item_ids").is_none()
                && h.query_param("saved_item_ids").is_none()
        }),
        "零绑定不得请求条目或权威状态"
    );
    let conn = db.lock().await;
    assert_eq!(db::last_sync_entry_id(&conn).unwrap(), 100);
    assert_eq!(db::last_sync_ts(&conn).unwrap(), 777_777);
    assert_eq!(
        history_state(&conn).as_deref(),
        Some("pending:55"),
        "不得初始化/推进/完成历史状态"
    );
    assert_eq!(article_count(&conn), 1, "预置文章不得被消费");
    drop(conn);
    let _ = std::fs::remove_file(&tmp);
}

/// P2-2：一个 feed 已绑定、另一个 `origin='remote'` 源仍缺 remote_id 时，
/// Fever 不得消费条目窗口或推进 since/history checkpoint。
#[tokio::test]
async fn fever_partial_remote_feed_binding_keeps_cursor_and_history() {
    let srv = Fixture::start().await;
    srv.add_entry(125, 11, "未绑定源条目", false, false);
    let (db, tmp) = seed_db("fever_compat_partial_binding", &srv);
    {
        let conn = db.lock().await;
        seed_bound_feed(&conn);
        seed_unbound_remote_feed(&conn, "http://example.com/unbound.rss");
        db::set_last_sync_entry_id(&conn, 100).unwrap();
        db::set_last_sync_ts(&conn, 777_777).unwrap();
        db::set_fever_history_pending(&conn, 55).unwrap();
    }

    let report = sync::sync_light(&db, &http())
        .await
        .expect("partial binding is reported without aborting the phase");
    assert!(
        report.errors.iter().any(|e| {
            e.contains("remote_id") || e.contains("持久绑定不完整") || e.contains("尚无本地绑定")
        }),
        "部分绑定或持久 marker 必须显式可见：{:?}",
        report.errors
    );
    assert!(
        srv.hits().iter().all(|h| {
            h.query_param("items").is_none()
                && h.query_param("since_id").is_none()
                && h.query_param("max_id").is_none()
                && h.query_param("unread_item_ids").is_none()
                && h.query_param("saved_item_ids").is_none()
        }),
        "存在未绑定远端源时不得请求条目/权威集合"
    );
    let conn = db.lock().await;
    assert_eq!(db::last_sync_entry_id(&conn).unwrap(), 100);
    assert_eq!(db::last_sync_ts(&conn).unwrap(), 777_777);
    assert_eq!(history_state(&conn).as_deref(), Some("pending:55"));
    assert_eq!(article_count(&conn), 0);
    drop(conn);
    let _ = std::fs::remove_file(&tmp);
}

/// P2-2 的条目级分支：增量页出现本地未绑定 feed 时必须立即中止，不能继续
/// 走历史阶段并写 Complete。
#[tokio::test]
async fn fever_entry_feed_without_local_binding_keeps_cursor_and_history() {
    let srv = Fixture::start().await;
    srv.add_entry(125, 11, "unknown-feed", false, false);
    let (db, tmp) = seed_db("fever_compat_unknown_entry_feed", &srv);
    {
        let conn = db.lock().await;
        seed_bound_feed(&conn);
        db::set_last_sync_entry_id(&conn, 100).unwrap();
        db::set_last_sync_ts(&conn, 777_778).unwrap();
        db::reset_fever_history_state(&conn).unwrap();
    }

    let report = sync::sync_light(&db, &http())
        .await
        .expect("unknown entry feed is reported");
    assert!(
        report.errors.iter().any(|e| e.contains("尚无本地绑定")),
        "条目级未绑定必须显式可见：{:?}",
        report.errors
    );
    let conn = db.lock().await;
    assert_eq!(db::last_sync_entry_id(&conn).unwrap(), 100);
    assert_eq!(db::last_sync_ts(&conn).unwrap(), 777_778);
    assert_eq!(
        db::fever_history_state(&conn).unwrap(),
        db::FeverHistoryState::Unknown,
        "条目级绑定失败必须恢复同步前 Unknown 状态，不得残留初始化 Pending"
    );
    assert_eq!(article_count(&conn), 0);
    drop(conn);
    let _ = std::fs::remove_file(&tmp);
}

/// P2 返工：Fever 历史页遇到未绑定 feed 时也必须保持同步前 Unknown/history
/// 与条目游标，不得继续写 Complete。
#[tokio::test]
async fn fever_history_page_with_unknown_feed_keeps_history_unknown() {
    let srv = Fixture::start().await;
    srv.add_entry(125, 11, "unknown-history-feed", false, false);
    let (db, tmp) = seed_db("fever_compat_unknown_history_feed", &srv);
    {
        let conn = db.lock().await;
        seed_bound_feed(&conn);
        db::set_last_sync_entry_id(&conn, 100).unwrap();
        db::set_last_sync_ts(&conn, 777_779).unwrap();
        db::reset_fever_history_state(&conn).unwrap();
    }

    let report = sync::states_phase(&db, &http(), true)
        .await
        .expect("history page binding failure is reported");
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("历史页含尚无本地绑定")),
        "历史页未绑定分支必须显式可见：{:?}",
        report.errors
    );
    let conn = db.lock().await;
    assert_eq!(db::last_sync_entry_id(&conn).unwrap(), 100);
    assert_eq!(db::last_sync_ts(&conn).unwrap(), 777_779);
    assert_eq!(
        db::fever_history_state(&conn).unwrap(),
        db::FeverHistoryState::Unknown
    );
    drop(conn);
    drop(db);
    let _ = std::fs::remove_file(&tmp);
}

/// P1-A（Fever）：feeds 绑定失败持久化后，独立 states 调用不得请求条目/
/// 权威集合，也不得推进 since 或 history checkpoint。
#[tokio::test]
async fn fever_binding_failure_blocks_separate_states_phase() {
    let srv = Fixture::start().await;
    srv.set_feeds(vec![
        (10, "旧订阅", "http://example.com/a.rss"),
        (11, "待绑定订阅", "http://example.com/b.rss"),
    ]);
    srv.set_groups(vec![(1, "分类")]);
    let (db, tmp) = seed_db("fever_compat_binding_marker", &srv);
    {
        let conn = db.lock().await;
        let folder = db::create_folder(&conn, "已有分类", "article").unwrap();
        let feed = db::insert_feed(
            &conn,
            "http://example.com/a.rss",
            None,
            "旧订阅",
            None,
            folder,
            "inherit",
            false,
            false,
        )
        .unwrap();
        db::set_feed_remote_id(&conn, feed, 10).unwrap();
        db::set_last_sync_entry_id(&conn, 100).unwrap();
        db::set_last_sync_ts(&conn, 690_002).unwrap();
        db::set_fever_history_pending(&conn, 55).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER opt019_fever_fail_remote_11
             BEFORE UPDATE OF remote_id ON feeds
             WHEN NEW.remote_id = 11
             BEGIN SELECT RAISE(ABORT, 'injected Fever remote 11 failure'); END;",
        )
        .unwrap();
    }

    let feed_report = sync::feeds_phase(&db, &http()).await.expect("feeds phase");
    assert!(
        feed_report.errors.iter().any(|e| e.contains("绑定")),
        "{:?}",
        feed_report.errors
    );
    assert_eq!(feed_report.pulled_feeds, 0, "绑定失败不得计为发现成功");

    let item_state_hits = |srv: &Fixture| {
        srv.hits()
            .iter()
            .filter(|h| {
                h.query_param("items").is_some()
                    || h.query_param("since_id").is_some()
                    || h.query_param("max_id").is_some()
                    || h.query_param("unread_item_ids").is_some()
                    || h.query_param("saved_item_ids").is_some()
            })
            .count()
    };
    let before = item_state_hits(&srv);
    let states_report = sync::states_phase(&db, &http(), false)
        .await
        .expect("states phase");
    assert!(
        states_report
            .errors
            .iter()
            .any(|e| e.contains("持久绑定不完整")),
        "{:?}",
        states_report.errors
    );
    assert_eq!(item_state_hits(&srv), before);
    let conn = db.lock().await;
    assert_eq!(db::last_sync_entry_id(&conn).unwrap(), 100);
    assert_eq!(db::last_sync_ts(&conn).unwrap(), 690_002);
    assert_eq!(history_state(&conn).as_deref(), Some("pending:55"));
    let account = app_lib::sync::current_account(&conn).unwrap().unwrap();
    assert!(db::binding_incomplete(&conn, &account.key).unwrap());
    drop(conn);
    drop(db);
    let _ = std::fs::remove_file(&tmp);
}

#[tokio::test]
async fn missing_items_field_is_protocol_error() {
    let srv = Fixture::start().await;
    srv.set_feeds(vec![(10, "源", "http://example.com/items.rss")]);
    srv.add_entry(1, 10, "条目", false, false);
    srv.set_omit_items(true);

    let client = FeverClient::new(&srv.url(), USER, PASS, http()).at_resolved(&srv.api_root());
    let err = client
        .items_since(0)
        .await
        .expect_err("缺 items 字段必须报错");
    assert!(
        err.to_string().contains("items"),
        "错误应指明缺 items 字段：{err}"
    );
    let err = client
        .items_with_ids(&[1])
        .await
        .expect_err("缺 items 字段必须报错");
    assert!(err.to_string().contains("items"), "{err}");

    let (db, tmp) = seed_db("fever_compat_missing_items", &srv);
    let report = sync::sync_now(&db, &http()).await.expect("sync");
    assert!(
        report.errors.iter().any(|e| e.contains("items")),
        "同步报告必须可见缺字段错误：{:?}",
        report.errors
    );
    {
        let conn = db.lock().await;
        assert_eq!(article_count(&conn), 0, "缺 items 不得静默当空耗尽历史");
        assert_eq!(
            db::last_sync_entry_id(&conn).unwrap(),
            0,
            "缺 items 不得推进 Fever 条目游标"
        );
        assert_eq!(
            db::last_sync_ts(&conn).unwrap(),
            0,
            "缺 items 不得写完成时间"
        );
        assert_ne!(
            history_state(&conn).as_deref(),
            Some("complete"),
            "缺 items 不得伪装历史完成"
        );
    }
    let _ = std::fs::remove_file(&tmp);
}

/// P2 补强：Fever action 返回 HTTP 200 非 JSON 时，必须在生产 pull 中映射为
/// protocol，并保持条目游标/完成时间。
#[tokio::test]
async fn malformed_json_items_response_fails_closed_end_to_end() {
    let srv = Fixture::start().await;
    srv.add_entry(125, 10, "坏 JSON 条目", false, false);
    srv.set_malformed_json_action(Some("items"));
    let (db, tmp) = seed_db("fever_compat_malformed_json", &srv);
    {
        let conn = db.lock().await;
        seed_bound_feed(&conn);
        db::set_last_sync_entry_id(&conn, 100).unwrap();
        db::set_last_sync_ts(&conn, 890_001).unwrap();
        db::set_fever_history_complete(&conn).unwrap();
    }

    let report = sync::sync_light(&db, &http())
        .await
        .expect("malformed JSON is report-visible");
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("[protocol]") && e.contains("解析失败")),
        "{:?}",
        report.errors
    );
    let conn = db.lock().await;
    assert_eq!(db::last_sync_entry_id(&conn).unwrap(), 100);
    assert_eq!(db::last_sync_ts(&conn).unwrap(), 890_001);
    assert_eq!(
        db::fever_history_state(&conn).unwrap(),
        db::FeverHistoryState::Complete
    );
    drop(conn);
    drop(db);
    let _ = std::fs::remove_file(&tmp);
}

#[tokio::test]
async fn missing_unread_field_does_not_mark_local_read() {
    let srv = Fixture::start().await;
    srv.set_feeds(vec![(10, "源", "http://example.com/u.rss")]);
    srv.add_entry(1, 10, "条目", false, false);
    srv.set_omit_unread(true);

    let client = FeverClient::new(&srv.url(), USER, PASS, http()).at_resolved(&srv.api_root());
    let err = client
        .unread_item_ids()
        .await
        .expect_err("缺 unread_item_ids 字段必须报错");
    assert!(
        err.to_string().contains("unread_item_ids"),
        "错误应指明缺字段：{err}"
    );

    let (db, tmp) = seed_db("fever_compat_missing_unread", &srv);
    // 预置一篇「已绑定远端 id 1、本地未读」的文章：若实现把缺字段当空集合，
    // 双向对账会把「未命中 = 已读」落到本地（数据损坏）。
    {
        let conn = db.lock().await;
        let folder = db::create_folder(&conn, "分类", "article").unwrap();
        let feed = db::insert_feed(
            &conn,
            "http://example.com/u.rss",
            None,
            "源",
            None,
            folder,
            "inherit",
            false,
            false,
        )
        .unwrap();
        let a = db::NewArticle {
            guid: "g1".into(),
            url: Some("https://e.example/1".into()),
            title: "本地".into(),
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
        let aid = db::upsert_article_with_feed(&conn, feed, &a, false)
            .unwrap()
            .0;
        db::set_article_remote_id(&conn, aid, 1).unwrap();
    }

    let report = sync::sync_now(&db, &http()).await.expect("sync");
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("unread_item_ids") || e.contains("状态对账")),
        "缺字段必须记为错误：{:?}",
        report.errors
    );
    {
        let conn = db.lock().await;
        let (read, _) = article_state(&conn, 1).expect("条目仍应存在");
        assert!(
            !read,
            "缺 unread_item_ids 时不得执行「未命中=已读」对账（本地状态被静默清掉）"
        );
    }
    let _ = std::fs::remove_file(&tmp);
}

#[tokio::test]
async fn missing_saved_field_is_protocol_error() {
    let srv = Fixture::start().await;
    srv.add_entry(1, 10, "条目", false, true);
    srv.set_omit_saved(true);

    let client = FeverClient::new(&srv.url(), USER, PASS, http()).at_resolved(&srv.api_root());
    let err = client
        .saved_item_ids()
        .await
        .expect_err("缺 saved_item_ids 字段必须报错");
    assert!(err.to_string().contains("saved_item_ids"), "{err}");
}

#[tokio::test]
async fn invalid_item_id_values_are_explicit_errors() {
    let srv = Fixture::start().await;
    srv.add_entry(1, 10, "条目", false, false);
    let client = FeverClient::new(&srv.url(), USER, PASS, http()).at_resolved(&srv.api_root());

    for (raw, tag) in [
        ("\"0x1F\"", "十六进制字符串"),
        ("\"99999999999999999999\"", "溢出 i64"),
        ("\"-5\"", "负值"),
        ("1.5", "浮点数"),
    ] {
        srv.set_raw_item_id_json(Some(raw));
        let res = client.items_with_ids(&[1]).await;
        assert!(
            res.is_err(),
            "{tag}（{raw}）必须显式报错而不是静默：{res:?}"
        );
    }

    // 清掉注入后同一形态恢复正常：证明失败不是夹具自身坏了。
    srv.set_raw_item_id_json(None);
    let ok = client.items_with_ids(&[1]).await.expect("合法 id 应成功");
    assert_eq!(ok.len(), 1);
    assert_eq!(ok[0].id, "1");
}

#[tokio::test]
async fn invalid_authoritative_csv_is_explicit_error_not_silent_filter() {
    let srv = Fixture::start().await;
    srv.add_entry(1, 10, "条目", false, false);
    let client = FeverClient::new(&srv.url(), USER, PASS, http()).at_resolved(&srv.api_root());

    for bad in ["1,abc", "1,,2", "1,-2", "99999999999999999999"] {
        srv.set_raw_unread_csv(Some(bad));
        let res = client.unread_item_ids().await;
        assert!(
            res.is_err(),
            "非法 CSV {bad:?} 必须报错（不得静默过滤成伪空集合）：{res:?}"
        );
    }

    // 合法 CSV（含空集与空白容忍）仍正常。
    srv.set_raw_unread_csv(Some("1, 2"));
    let ok = client.unread_item_ids().await.expect("合法 CSV 应成功");
    assert_eq!(ok, vec![1, 2]);
    srv.set_raw_unread_csv(Some(""));
    assert!(client.unread_item_ids().await.unwrap().is_empty());
}

/* ============================================================
E. mark=item 逐条语义：单个十进制 id，逗号批量不成立
============================================================ */

#[tokio::test]
async fn mark_sends_one_decimal_id_per_request() {
    let srv = Fixture::start().await;
    srv.add_entry(5705, 10, "A", false, false);
    srv.add_entry(LONG_ID, 10, "B", false, false);

    let client = FeverClient::new(&srv.url(), USER, PASS, http()).at_resolved(&srv.api_root());
    client
        .mark_read(&[5705, LONG_ID])
        .await
        .expect("逐条 mark 应成功");

    let marks: Vec<Hit> = srv
        .hits()
        .into_iter()
        .filter(|h| h.query_param("mark").as_deref() == Some("item"))
        .collect();
    assert_eq!(
        marks.len(),
        2,
        "两个 id 必须是两次请求（Fever 不支持逗号批量）"
    );
    let mut ids: Vec<String> = marks
        .iter()
        .map(|h| h.query_param("id").expect("mark 带 id"))
        .collect();
    ids.sort();
    assert_eq!(ids, vec![LONG_ID.to_string(), "5705".to_string()]);
    for h in &marks {
        let id = h.query_param("id").unwrap();
        assert!(
            id.chars().all(|c| c.is_ascii_digit()),
            "mark id 必须是单个十进制：{id}"
        );
    }
    assert!(srv.entry_read(5705) && srv.entry_read(LONG_ID));
}

/* ============================================================
G. R2：页事务 / 显式状态 / 真 DB trigger / HTTP 硬界
============================================================ */

/// **合成形态（不代表 FreshRSS hook——hook 对 with_ids 同样生效）**：
/// 列表窗口覆盖不到、但按 id 可取的条目（模拟服务端保留/窗口差异），
/// with_ids 补齐路径必须把它取回。
#[tokio::test]
async fn synthetic_listing_window_miss_is_backfilled_via_with_ids() {
    let srv = Fixture::start().await;
    srv.set_feeds(vec![(10, "源", "http://example.com/h.rss")]);
    for id in 1..=20 {
        srv.add_entry(id, 10, &format!("条目{id}"), id != 7, false);
    }
    // 7 号（未读）只从列表窗口隐藏（合成），with_ids 仍可取。
    srv.synthetic_listing_skip(7);

    let (db, tmp) = seed_db("fever_compat_synth_miss", &srv);
    let report = sync::sync_now(&db, &http()).await.expect("sync");
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    {
        let conn = db.lock().await;
        assert_eq!(
            article_count(&conn),
            20,
            "列表遗漏的 7 号应经 with_ids 补齐"
        );
        assert!(bound_remote_id(&conn, 7));
        let (read, _) = article_state(&conn, 7).unwrap();
        assert!(!read, "7 号远端未读应保持未读");
    }
    assert!(
        srv.hits()
            .iter()
            .any(|h| h.query_param("with_ids").as_deref() == Some("7")),
        "应按 with_ids=7 补齐：{:?}",
        srv.hits()
            .iter()
            .filter_map(|h| h.query_param("with_ids"))
            .collect::<Vec<_>>()
    );
    let _ = std::fs::remove_file(&tmp);
}

/// R2-P1：checkpoint（历史状态）写失败 → 不得推进 since、不得开始拉取；
/// 修复 trigger 后 light 自动重试并取尽。
#[tokio::test]
async fn history_state_write_failure_blocks_since_and_light_retries() {
    let srv = fixture_125().await;
    let (db, tmp) = seed_db("fever_compat_state_fail", &srv);
    {
        let conn = db.lock().await;
        install_settings_write_failure(&conn, "fever_history_state");
    }
    let http_client = http();
    let report = sync::sync_now(&db, &http_client).await.expect("sync");
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("injected settings write failure") || e.contains("历史状态")),
        "状态写失败必须显式报错：{:?}",
        report.errors
    );
    {
        let conn = db.lock().await;
        assert_eq!(
            article_count(&conn),
            0,
            "Pending 初始化失败必须先于任何拉取（不允许先拉后记）"
        );
        assert_eq!(
            db::last_sync_entry_id(&conn).unwrap(),
            0,
            "状态初始化失败绝不允许更大的 since 落库"
        );
        assert_eq!(
            history_state(&conn),
            None,
            "状态键保持 Unknown（缺失），不得被当作 Complete"
        );
    }
    {
        let conn = db.lock().await;
        remove_settings_write_failure(&conn);
    }
    let light = sync::sync_light(&db, &http_client)
        .await
        .expect("light sync");
    assert!(light.errors.is_empty(), "{:?}", light.errors);
    {
        let conn = db.lock().await;
        assert_eq!(article_count(&conn), 125, "重试后全历史补齐");
        assert_eq!(history_state(&conn).as_deref(), Some("complete"));
        assert_eq!(db::last_sync_entry_id(&conn).unwrap(), 125);
    }
    let _ = std::fs::remove_file(&tmp);
}

/// R2-P1：页内 INSERT 失败（remote-100）→ 整页回滚、统计回滚、checkpoint 保持
/// 输入页游标（不提前 complete）；下一 light 补回 100 并在取尽后才 Complete。
#[tokio::test]
async fn page_insert_failure_rolls_back_page_with_pending_resume() {
    let srv = fixture_125().await;
    let (db, tmp) = seed_db("fever_compat_page_rollback", &srv);
    {
        let conn = db.lock().await;
        install_article_insert_failure(&conn, "remote-100");
    }
    let http_client = http();
    let report = sync::sync_now(&db, &http_client).await.expect("sync");
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("injected article insert failure")),
        "页失败必须带真实 DB 错误：{:?}",
        report.errors
    );
    {
        let conn = db.lock().await;
        // 首屏页（76..125）整页回滚：已读非收藏的 76..100 一条都不许留半提交。
        for id in [76_i64, 99, 100] {
            assert!(
                !bound_remote_id(&conn, id),
                "页事务失败后 {id} 不得半提交（统计/映射同样回滚）"
            );
        }
        // 页事务贡献 0；剩下的 25 条来自 with_ids 对未读集合（101..125）的合法补齐。
        assert_eq!(
            report.pulled_entries, 25,
            "页事务统计必须回滚（只允许 with_ids 补齐贡献计数）"
        );
        assert_eq!(article_count(&conn), 25, "只允许 with_ids 补齐的 101..125");
        assert_eq!(
            history_state(&conn).as_deref(),
            Some("pending:9223372036854775807"),
            "checkpoint 必须保持输入页游标，不得提前推进/完成"
        );
    }
    {
        let conn = db.lock().await;
        remove_article_insert_failure(&conn);
    }
    let light = sync::sync_light(&db, &http_client)
        .await
        .expect("light sync");
    assert!(light.errors.is_empty(), "{:?}", light.errors);
    {
        let conn = db.lock().await;
        assert!(
            bound_remote_id(&conn, 100),
            "下一 light 必须把 remote-100 补回"
        );
        assert_eq!(article_count(&conn), 125, "补回并走完后全历史 125");
        assert_eq!(
            history_state(&conn).as_deref(),
            Some("complete"),
            "只有真正取尽才允许 Complete"
        );
    }
    let _ = std::fs::remove_file(&tmp);
}

/// R3：失败页回滚不得留下**假数据变化标记**。前置：全量同步完成（125 条、
/// history complete）。重放轮开始前做两处现场改造，二者都在回溯首页（=125..76，
/// 降序处理）且**先于**注入点 85 被合并：
/// - 删除条目 88（已读非收藏）→ 重放以**新行 INSERT** 真实入库（置标记）；
/// - 清空条目 90 的正文 → 重放 backfill 真实回填（置标记）。
/// 随后同页的条目 85 注入 UPDATE 失败 → 整页回滚。断言：错误可见、88 仍不存在、
/// 90 正文仍为空（页内真实写都被撤销）、标记与 has_data_change 为 false
/// （标记随页回滚，不得残留）。时序依据：mock 的 max_id 页为
/// `id < max_id ORDER BY id DESC LIMIT 50`（本文件夹具契约），88/90 先于 85 处理，
/// 故回滚前标记必然已被置起——本用例对「页回滚回退标记」有判别力。
/// 88/90 均不在 unread（已读）/saved 集合，回滚后 with_ids/reconcile 都不会再
/// 触及它们，终态可确定性断言。
#[tokio::test]
async fn failed_page_rollback_leaves_no_data_change_marker() {
    let srv = fixture_125().await;
    let (db, tmp) = seed_db("fever_compat_page_marker", &srv);
    let http_client = http();

    // 前置：全量成功（新条目 = 真实变化，标记必须为 true）。
    let first = sync::sync_now(&db, &http_client).await.expect("first sync");
    assert!(first.errors.is_empty(), "{:?}", first.errors);
    assert!(first.entries_data_changed, "前置：首轮全量确有新条目变化");
    {
        let conn = db.lock().await;
        assert_eq!(article_count(&conn), 125);
        assert_eq!(history_state(&conn).as_deref(), Some("complete"));
        // 现场改造 1：删除 88（重放会 INSERT 补回 → 页内真实变化）。
        conn.execute("DELETE FROM articles WHERE remote_id = 88", [])
            .unwrap();
        // 现场改造 2：清空 90 的正文（重放会 backfill 补回 → 页内真实变化）。
        conn.execute(
            "UPDATE contents SET content_html = NULL, body_text = ''
             WHERE id IN (SELECT content_id FROM articles WHERE remote_id = 90)",
            [],
        )
        .unwrap();
        // 触发顶部重放：历史状态重置为 Unknown（拉取仍从顶部走）。
        db::reset_fever_history_state(&conn).unwrap();
        // 注入：remote-85 的既有行 UPDATE 失败（重放/回填路径 = UPDATE）。
        install_article_update_failure(&conn, "remote-85");
    }

    let report = sync::sync_light(&db, &http_client)
        .await
        .expect("second sync");
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("injected article update failure")),
        "回放页必须真实失败并可见: {:?}",
        report.errors
    );
    {
        let conn = db.lock().await;
        assert_eq!(
            article_count(&conn),
            124,
            "失败页回滚后现场逐条不变（88 的重放 INSERT 也被回滚）"
        );
        assert!(
            !bound_remote_id(&conn, 88),
            "页内新行入库必须随页回滚（不得半提交）"
        );
        assert!(bound_remote_id(&conn, 85), "失败页条目不得半提交");
        let content_90: Option<String> = conn
            .query_row(
                "SELECT c.content_html FROM articles a LEFT JOIN contents c ON c.id = a.content_id WHERE a.remote_id = 90",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            content_90, None,
            "页内已真实发生的回填必须随页回滚（正文回到清空态）"
        );
    }
    assert!(
        !report.entries_data_changed,
        "失败页已回滚：不得留下假数据变化标记（页内新行/回填的标记必须一起回退）"
    );
    assert!(!report.has_data_change(), "本轮零实际变化：不得发数据事件");
    let _ = std::fs::remove_file(&tmp);
}

/// R2：旧库只留着 since（无状态键 = Unknown）→ light 必须自动补旧历史，
/// 而不是把 Unknown 当完成。
#[tokio::test]
async fn unknown_history_state_with_existing_since_auto_backfills() {
    let srv = fixture_125().await;
    let (db, tmp) = seed_db("fever_compat_unknown_state", &srv);
    {
        let conn = db.lock().await;
        seed_bound_feed(&conn);
        db::set_setting(&conn, "sync_last_entry_id", "100").unwrap();
        // 旧库形态：无 fever_history_state 键（Unknown）。
    }
    let report = sync::sync_light(&db, &http()).await.expect("light sync");
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    {
        let conn = db.lock().await;
        assert_eq!(
            article_count(&conn),
            125,
            "Unknown 必须自动补旧历史（1..75 已读非收藏也到）"
        );
        assert!(bound_remote_id(&conn, 1), "最老的条目也应补回");
        assert_eq!(history_state(&conn).as_deref(), Some("complete"));
    }
    let _ = std::fs::remove_file(&tmp);
}

/// R2：损坏的历史状态必须显式报错、不改游标、不擅自覆盖，也不得被当完成。
#[tokio::test]
async fn corrupt_history_state_errors_without_touching_cursor() {
    let srv = fixture_125().await;
    let (db, tmp) = seed_db("fever_compat_corrupt_state", &srv);
    {
        let conn = db.lock().await;
        seed_bound_feed(&conn);
        db::set_setting(&conn, "sync_last_entry_id", "100").unwrap();
        db::set_setting(&conn, "fever_history_state", "garbage-value").unwrap();
    }
    let report = sync::sync_light(&db, &http()).await.expect("sync");
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("fever_history_state") || e.contains("损坏")),
        "损坏状态必须显式报错：{:?}",
        report.errors
    );
    {
        let conn = db.lock().await;
        assert_eq!(
            db::last_sync_entry_id(&conn).unwrap(),
            100,
            "损坏状态不得改动 since 游标"
        );
        assert_eq!(article_count(&conn), 0, "状态不可判定时不得当完成继续拉取");
        assert_eq!(
            history_state(&conn).as_deref(),
            Some("garbage-value"),
            "不得擅自覆盖损坏值（留给显式修复/排查）"
        );
    }
    let _ = std::fs::remove_file(&tmp);
}

/// R2：chunked（**无 Content-Length**）响应必须按 chunk 累计设界。
#[tokio::test]
async fn chunked_response_without_content_length_is_capped() {
    let srv = Fixture::start().await;
    srv.set_raw_response(Some(RawResponse {
        chunked: true,
        headers: vec![("Content-Type".into(), "application/json".into())],
        body: vec![b'x'; 17 * 1024 * 1024],
    }));
    let client = FeverClient::new(&srv.url(), USER, PASS, http()).at_resolved(&srv.api_root());
    match client.items_since(0).await {
        Ok(items) => panic!("chunked 超限响应必须失败（实际返回 {} 条）", items.len()),
        Err(e) => assert!(e.to_string().contains("16 MiB"), "应指明 16 MiB：{e}"),
    }
}

/// 17 MiB 重复字节的 brotli 压缩流（node zlib 预生成，40 字节）。
const BROTLI_17MIB_B64: &str = "y///P/gl8OKxQCD3/o///3/wSwDEYRGA7v1f/f8/fgmAOCwA0L1/AA==";

/// R2：压缩（解压后超限）——线上仅 40 字节，解压后 17 MiB；
/// 上限必须按**解压后的 chunk 累计**生效。
#[tokio::test]
async fn brotli_oversized_response_is_capped_after_decompression() {
    let srv = Fixture::start().await;
    let body = base64_decode(BROTLI_17MIB_B64);
    assert!(
        body.len() < 1024,
        "线上载荷应为压缩后的小字节：{}",
        body.len()
    );
    srv.set_raw_response(Some(RawResponse {
        chunked: false,
        headers: vec![
            ("Content-Type".into(), "application/json".into()),
            ("Content-Encoding".into(), "br".into()),
        ],
        body,
    }));
    let client = FeverClient::new(&srv.url(), USER, PASS, http()).at_resolved(&srv.api_root());
    match client.items_since(0).await {
        Ok(items) => panic!("解压后超 16 MiB 的响应必须失败（实际 {} 条）", items.len()),
        Err(e) => assert!(
            e.to_string().contains("16 MiB"),
            "应指明按解压后累计的 16 MiB：{e}"
        ),
    }
}

/* ============================================================
F. R1：短页继续 / checkpoint 续取 / 游标隔离 / MAX 边界 / 预算 / 响应上限
============================================================ */

/// P1：since_id 短页（LIMIT 后 hook 过滤）**不是**结束信号。
///
/// 151..224 已读非收藏、225 未读。第一页 `id > 150` 由服务端取 151..200 再
/// 被 hook 丢弃 200 → 49 条非空短页。若把短页当结束，只能靠 with_ids(225)
/// 补到 225，151..224 会永久缺失。正确行为：非空页一律继续到空页。
#[tokio::test]
async fn since_short_page_after_hook_filter_continues_to_empty() {
    let srv = Fixture::start().await;
    srv.set_feeds(vec![(10, "源", "http://example.com/h.rss")]);
    for id in 151..=225 {
        srv.add_entry(id, 10, &format!("条目{id}"), id != 225, false);
    }
    srv.hook_drop_entry(200);

    let (db, tmp) = seed_db("fever_compat_short_page", &srv);
    {
        let conn = db.lock().await;
        seed_bound_feed(&conn);
        db::set_setting(&conn, "sync_last_entry_id", "150").unwrap();
        // 历史已完成：本用例只测增量短页继续（Unknown 会自动补历史，混淆前提）。
        db::set_setting(&conn, "fever_history_state", "complete").unwrap();
    }
    let report = sync::sync_light(&db, &http()).await.expect("light sync");
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    {
        let conn = db.lock().await;
        // hook 对**所有** items 分支（含 with_ids）持续过滤：200 对 Fever 整体
        // 不可见（且不在权威集合 → 不可达是夹具语义，符合 FreshRSS）。
        // 其余 151..199 + 201..225 共 74 条必须全部取到。
        assert_eq!(article_count(&conn), 74, "短页之后的整段必须继续拉完");
        assert!(bound_remote_id(&conn, 224), "151..224 已读非收藏不得漏");
        assert!(bound_remote_id(&conn, 225));
        assert!(
            !bound_remote_id(&conn, 200),
            "hook 过滤的 200 对列表恒不可见（夹具语义：它不是「后页」）"
        );
        assert_eq!(db::last_sync_entry_id(&conn).unwrap(), 225);
    }
    let since_hits = srv.since_id_hits();
    assert_eq!(
        since_hits[0].query_param("since_id").as_deref(),
        Some("150"),
        "增量应从 150 起"
    );
    assert!(
        since_hits
            .iter()
            .any(|h| h.query_param("since_id").as_deref() == Some("199")),
        "短页（49 条）后必须继续翻页：{:?}",
        since_hits
            .iter()
            .map(|h| h.query_param("since_id"))
            .collect::<Vec<_>>()
    );
    let _ = std::fs::remove_file(&tmp);
}

/// checkpoint：full 第二页失败后，**自动 light**（非手动 full）必须续取旧历史。
#[tokio::test]
async fn full_history_failure_then_light_resumes_automatically() {
    let srv = fixture_125().await;
    srv.set_max_id_fail_at(2);

    let (db, tmp) = seed_db("fever_compat_auto_resume", &srv);
    let http_client = http();
    let report = sync::sync_now(&db, &http_client).await.expect("full sync");
    assert!(
        report.errors.iter().any(|e| e.contains("历史")),
        "第二页失败必须显式记录：{:?}",
        report.errors
    );
    {
        let conn = db.lock().await;
        assert_eq!(article_count(&conn), 50, "失败前已拉页保留");
        assert_eq!(
            history_state(&conn).as_deref(),
            Some("pending:76"),
            "未完成历史必须保存 pending 状态（页事务与 checkpoint 同 commit）"
        );
    }

    // 不跑手动 full：普通 light 同步自动从 checkpoint 续取，直到取尽。
    let light = sync::sync_light(&db, &http_client)
        .await
        .expect("light sync");
    assert!(light.errors.is_empty(), "{:?}", light.errors);
    {
        let conn = db.lock().await;
        assert_eq!(article_count(&conn), 125, "light 自动补齐旧历史");
        assert_eq!(
            history_state(&conn).as_deref(),
            Some("complete"),
            "取尽后才允许进入 Complete（与 Pending 分清）"
        );
        assert_eq!(db::last_sync_entry_id(&conn).unwrap(), 125);
    }
    let _ = std::fs::remove_file(&tmp);
}

/// 游标隔离：增量第二页失败 + with_ids 补到大 id（收藏），
/// `last_sync_entry_id` 必须停在**连续成功范围**的末尾（150），
/// 不得被 with_ids 的大 id（225）推过未拉取的 151..224。
#[tokio::test]
async fn incremental_failure_cursor_not_pushed_by_with_ids_backfill() {
    let srv = Fixture::start().await;
    srv.set_feeds(vec![(10, "源", "http://example.com/h.rss")]);
    for id in 101..=225 {
        let saved = id == 225;
        srv.add_entry(id, 10, &format!("条目{id}"), true, saved);
    }
    srv.set_since_page_fail_at(2);

    let (db, tmp) = seed_db("fever_compat_cursor_isolation", &srv);
    {
        let conn = db.lock().await;
        seed_bound_feed(&conn);
        db::set_setting(&conn, "sync_last_entry_id", "100").unwrap();
        // 历史已完成：本用例只测增量游标隔离（Unknown 会自动补历史，会污染前提）。
        db::set_setting(&conn, "fever_history_state", "complete").unwrap();
    }
    let http_client = http();
    let report = sync::sync_light(&db, &http_client)
        .await
        .expect("light sync");
    assert!(
        report.errors.iter().any(|e| e.contains("增量")),
        "增量第二页失败必须记录：{:?}",
        report.errors
    );
    {
        let conn = db.lock().await;
        // 第一页 101..150 + with_ids 补齐的 225 = 51 篇。
        assert_eq!(article_count(&conn), 51, "第一页 + with_ids 补齐");
        assert!(
            bound_remote_id(&conn, 225),
            "收藏的 225 应经 with_ids 补齐（id 精确通道）"
        );
        assert!(
            !bound_remote_id(&conn, 151),
            "失败页之后的条目本轮不得出现（也不得伪成功）"
        );
        assert_eq!(
            db::last_sync_entry_id(&conn).unwrap(),
            150,
            "游标不得被 with_ids 的 225 推动（否则 151..224 永久缺失）"
        );
        assert_eq!(
            history_state(&conn).as_deref(),
            Some("complete"),
            "增量失败不得改动历史状态（原 Complete 保持 Complete）"
        );
    }

    // 清除注入后继续：从 150 起重新覆盖失败区间，全部 101..225 可达。
    srv.set_since_page_fail_at(9999);
    let again = sync::sync_light(&db, &http_client)
        .await
        .expect("light sync 继续");
    assert!(again.errors.is_empty(), "{:?}", again.errors);
    {
        let conn = db.lock().await;
        assert_eq!(article_count(&conn), 125);
        assert_eq!(db::last_sync_entry_id(&conn).unwrap(), 225);
    }
    let _ = std::fs::remove_file(&tmp);
}

/// MAX 边界：`items&max_id=i64::MAX` 按严格 `<` 不会返回恰好 i64::MAX 的条目。
/// 已读非收藏的边界条目不在权威集合里，必须由顶覆盖（with_ids(MAX)）取得。
#[tokio::test]
async fn history_covers_item_at_i64_max_boundary() {
    let srv = Fixture::start().await;
    srv.set_feeds(vec![(10, "源", "http://example.com/h.rss")]);
    srv.add_entry(5, 10, "普通已读", true, false);
    srv.add_entry(i64::MAX, 10, "边界已读非收藏", true, false);

    let (db, tmp) = seed_db("fever_compat_max_boundary", &srv);
    let report = sync::sync_now(&db, &http()).await.expect("sync");
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    {
        let conn = db.lock().await;
        assert_eq!(article_count(&conn), 2);
        assert!(
            bound_remote_id(&conn, i64::MAX),
            "恰好 i64::MAX 的已读非收藏条目必须被顶覆盖取到"
        );
        let (read, _) = article_state(&conn, i64::MAX).unwrap();
        assert!(read);
        assert_eq!(db::last_sync_entry_id(&conn).unwrap(), i64::MAX);
    }
    // 顶覆盖走 with_ids(MAX)：wire 上是十进制字面值。
    assert!(
        srv.hits()
            .iter()
            .any(|h| h.query_param("with_ids").as_deref() == Some("9223372036854775807")),
        "应按 with_ids=9223372036854775807 做顶覆盖：{:?}",
        srv.hits()
            .iter()
            .filter_map(|h| h.query_param("with_ids"))
            .collect::<Vec<_>>()
    );
    let _ = std::fs::remove_file(&tmp);
}

/// 200 页预算：单轮最多 200 页（1 万条），到达即**明确未完成** + 保存 checkpoint；
/// 下一次 light 自动续取剩余部分（不是永久只留一万条，也不是伪成功）。
#[tokio::test]
async fn history_budget_200_pages_defers_and_light_resumes() {
    let srv = Fixture::start().await;
    srv.set_feeds(vec![(10, "源", "http://example.com/h.rss")]);
    for id in 1..=10050 {
        srv.add_entry(id, 10, &format!("条目{id}"), true, false);
    }

    let (db, tmp) = seed_db("fever_compat_budget", &srv);
    let http_client = http();
    let report = sync::sync_now(&db, &http_client).await.expect("full sync");
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("预算") && e.contains("未完成")),
        "预算到达必须返回明确未完成：{:?}",
        report.errors
    );
    {
        let conn = db.lock().await;
        // 200 页 × 50 条 = 10000 条：完全按页落库，不是「固定只留一万条」的上限。
        assert_eq!(article_count(&conn), 10000, "本轮应落库 200 页");
        assert_eq!(
            history_state(&conn).as_deref(),
            Some("pending:51"),
            "pending 续取点应为下一页起点（10050-10000+1）"
        );
        assert_eq!(
            db::last_sync_ts(&conn).unwrap(),
            0,
            "未完成不得推进「完成时间」游标"
        );
        assert_eq!(db::last_sync_entry_id(&conn).unwrap(), 10050);
    }

    // 下一次普通 light 自动续取剩余 50 条并完成。
    let light = sync::sync_light(&db, &http_client)
        .await
        .expect("light sync");
    assert!(light.errors.is_empty(), "{:?}", light.errors);
    {
        let conn = db.lock().await;
        assert_eq!(article_count(&conn), 10050, "续取后全历史可达");
        assert_eq!(
            history_state(&conn).as_deref(),
            Some("complete"),
            "取尽后状态转 Complete"
        );
    }
    let _ = std::fs::remove_file(&tmp);
}

/// 响应体 16 MiB 界限：超限响应必须显式失败，而不是整段读入内存再解析。
#[tokio::test]
async fn oversized_response_is_rejected_by_16mib_cap() {
    let srv = Fixture::start().await;
    srv.add_entry(1, 10, "超大条目", false, false);
    srv.set_oversize_html_bytes(17 * 1024 * 1024);

    let client = FeverClient::new(&srv.url(), USER, PASS, http()).at_resolved(&srv.api_root());
    match client.items_since(0).await {
        Ok(items) => panic!(
            "超过 16 MiB 的响应必须显式失败（实际返回 {} 条）",
            items.len()
        ),
        Err(e) => assert!(
            e.to_string().contains("16 MiB"),
            "错误应指明 16 MiB 上限：{e}"
        ),
    }
}

/* ============================================================
H. R3：map 读取失败绝不退化成空映射（错误停止传播）
============================================================ */

/// 造一篇带 URL 的本地锚文章（url_norm 初始为正常文本），返回 aid。
/// 供 R3 污染 trigger 在同步中途改成 BLOB，制造真实的列类型读取错误。
fn seed_anchor_article(conn: &rusqlite::Connection) -> i64 {
    let folder = db::create_folder(conn, "锚分类", "article").unwrap();
    let feed = db::insert_feed(
        conn,
        "http://example.com/anchor.rss",
        None,
        "锚",
        None,
        folder,
        "inherit",
        false,
        false,
    )
    .unwrap();
    let a = db::NewArticle {
        guid: "r3-anchor".into(),
        url: Some("https://e.example/anchor".into()),
        title: "锚".into(),
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
    let aid = db::upsert_article_with_feed(conn, feed, &a, false)
        .unwrap()
        .0;
    // 初始读取必须成功（正常文本）；污染只在同步中途由 trigger 注入。
    assert!(
        conn.query_row(
            "SELECT COUNT(*) FROM articles WHERE id = ?1 AND url_norm IS NOT NULL",
            [aid],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
            == 1,
        "锚文章的 url_norm 应是可正常读取的文本"
    );
    aid
}

/// R3-P1：初次 `sync_match_maps` 读取失败（真实列类型错误：url_norm 为 BLOB）
/// 必须立即终止本轮——**绝不**退化成空映射继续拉取。
/// 空 feed 映射会让每条 merge「合法跳过」，却照常确认 since/checkpoint 直到
/// Complete：本地 0 篇而游标认为历史取尽（永久数据丢失）。
#[tokio::test]
async fn map_read_error_aborts_round_without_fake_completion() {
    let srv = fixture_125().await;
    let (db, tmp) = seed_db("fever_compat_map_read_error", &srv);
    {
        let conn = db.lock().await;
        seed_bound_feed(&conn);
        let anchor = seed_anchor_article(&conn);
        // 真实 DB 列类型错误：url_norm 写 BLOB，sync_match_maps 以 String 读取。
        conn.execute(
            "UPDATE articles SET url_norm = x'00' WHERE id = ?1",
            [anchor],
        )
        .unwrap();
        // map 构建发生在历史 Pending 初始化之前；预先写入哨兵状态，断言失败
        // 不会凭空覆盖它。
        db::set_fever_history_pending(&conn, 77).unwrap();
    }
    let report = sync::sync_now(&db, &http()).await.expect("sync");
    assert!(
        report.errors.iter().any(|e| e.contains("匹配映射构建失败")),
        "map 读取失败必须显式记录：{:?}",
        report.errors
    );
    {
        let conn = db.lock().await;
        assert_eq!(
            article_count(&conn),
            1,
            "只允许预置的锚文章（不得合入任何条目）"
        );
        assert_eq!(
            history_state(&conn).as_deref(),
            Some("pending:77"),
            "map 读取失败必须保留同步前历史状态（不初始化、不假 Complete）"
        );
        assert_eq!(db::last_sync_entry_id(&conn).unwrap(), 0, "since 不得推进");
        assert_eq!(db::last_sync_ts(&conn).unwrap(), 0, "完成时间不得推进");
    }
    assert_eq!(srv.max_id_hits().len(), 0, "map 读取失败后不得执行历史回溯");
    assert_eq!(
        srv.since_id_hits().len(),
        0,
        "map 读取失败后不得执行增量翻页"
    );
    assert_eq!(
        srv.hits()
            .iter()
            .filter(|h| h.query_param("with_ids").is_some())
            .count(),
        0,
        "map 读取失败后不得执行 with_ids 补齐"
    );
    let _ = std::fs::remove_file(&tmp);
}

/// R3-P1：页失败后的 map 重建失败同样必须终止整轮（真实列类型错误：页 1 成功
/// 提交的 AFTER INSERT trigger 把锚文章 url_norm 污染成 BLOB，重建读取命中）。
/// 旧行为退化成空映射继续执行顶部历史回溯——空洞区间（101..150）未合入却被
/// 伪确认（since 推到 150 + Complete），永久数据丢失。
#[tokio::test]
async fn page_failure_then_rebuild_map_error_terminates_round() {
    // 51..=150 共 100 篇已读非收藏：增量 since=50 → 页 1=51..100（成功），
    // 页 2=101..150 在 120 处失败。历史状态 Unknown（旧库形态）→ light 本应
    // 在增量后启动顶部回溯（这正是旧行为伪确认的路径）。
    let srv = Fixture::start().await;
    srv.set_feeds(vec![(10, "源", "http://example.com/h.rss")]);
    for id in 51..=150 {
        srv.add_entry(id, 10, &format!("条目{id}"), true, false);
    }
    let (db, tmp) = seed_db("fever_compat_r3_rebuild_error", &srv);
    {
        let conn = db.lock().await;
        seed_bound_feed(&conn);
        db::set_setting(&conn, "sync_last_entry_id", "50").unwrap();
        // 页 2 的 120 号 INSERT 失败（真 DB trigger）。
        install_article_insert_failure(&conn, "remote-120");
        // 锚文章：url_norm 初始正常（首读成功），页 1 首条（51）落库的 AFTER
        // INSERT 把它污染成 BLOB → 页 2 失败后的重建读取命中列类型错误。
        let anchor = seed_anchor_article(&conn);
        conn.execute_batch(&format!(
            "CREATE TRIGGER opt005_r3_pollute AFTER INSERT ON articles \
             WHEN NEW.guid = 'remote-51' \
             BEGIN UPDATE articles SET url_norm = x'00' WHERE id = {anchor}; END;"
        ))
        .unwrap();
    }
    let report = sync::sync_light(&db, &http()).await.expect("light sync");
    assert!(
        report.errors.iter().any(|e| e.contains("增量页合并失败")),
        "页失败必须记录：{:?}",
        report.errors
    );
    assert!(
        report.errors.iter().any(|e| e.contains("重建匹配映射失败")),
        "重建失败必须显式记录并终止本轮：{:?}",
        report.errors
    );
    {
        let conn = db.lock().await;
        // 页 1（51..100）已提交；页 2（101..150）在 120 处整页回滚。
        assert_eq!(
            article_count(&conn),
            51,
            "锚 1 篇 + 页 1 的 50 篇；页 2 不得半提交"
        );
        assert!(bound_remote_id(&conn, 100), "页 1 最后一条应已提交");
        assert!(!bound_remote_id(&conn, 101), "页 2 首条不得半提交");
        assert!(!bound_remote_id(&conn, 150), "未到达的条目不得出现");
        assert_eq!(
            db::last_sync_entry_id(&conn).unwrap(),
            100,
            "since 必须停在连续成功范围（100）——不得被后续顶部回溯伪推进到 150"
        );
        assert_eq!(
            history_state(&conn).as_deref(),
            Some("pending:9223372036854775807"),
            "Pending 输入游标必须保留（绝不假 Complete）"
        );
        assert_eq!(db::last_sync_ts(&conn).unwrap(), 0, "完成时间不得推进");
    }
    assert_eq!(srv.max_id_hits().len(), 0, "重建失败后不得执行历史回溯");
    assert_eq!(
        srv.hits()
            .iter()
            .filter(|h| h.query_param("with_ids").is_some())
            .count(),
        0,
        "重建失败后不得执行 with_ids 补齐"
    );
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
I. OPT-008B 第一阶段：Fever 跨源同 URL 的独立条目身份
============================================================ */

/// Fever：两个 feed 的 entry 指向同一 URL、id 不同 —— 必须各有独立条目。
/// 身份层：两条 entry 各自绑定自己的行（不因 URL 相同被吞成 CSV/单行）。
/// 读政策层（N4 裁定，两协议统一）：远端任一副本已读时，同 URL 兄弟按
/// read-anywhere-wins 广播一并标读，且**本轮最终 Fever 权威对账不得立刻
/// 撤销同一已接受的广播**；下一轮远端显式 unread（无同轮 read 信号）时
/// 双向 unread 权威照常生效——历史广播读不永久阻挡后续远端显式 unread。
/// 实现前：URL 兜底匹配（全局）把第二条 entry 记进 CSV/直接吞掉，断言必红。
#[tokio::test]
async fn fever_cross_feed_same_url_keeps_independent_entries() {
    const SHARED: &str = "http://127.0.0.1:8765/post/fever-shared";
    let srv = Fixture::start().await;
    srv.set_groups(vec![(1, "G")]);
    srv.set_feeds(vec![
        (10, "F10", "http://example.com/f10.rss"),
        (11, "F11", "http://example.com/f11.rss"),
    ]);
    srv.add_entry_with_url(7001, 10, "copy-10", false, false, Some(SHARED));
    srv.add_entry_with_url(7002, 11, "copy-11", false, false, Some(SHARED));

    let (db, tmp) = seed_db("fever_identity_cross", &srv);
    {
        let conn = db.lock().await;
        // 两个本地源：sync_now 的 feeds 阶段按 URL 与远端 feed 完成绑定
        let folder = db::create_folder(&conn, "分类", "article").unwrap();
        let feed10 = db::insert_feed(
            &conn,
            "http://example.com/f10.rss",
            None,
            "F10",
            None,
            folder,
            "inherit",
            false,
            false,
        )
        .unwrap();
        let feed11 = db::insert_feed(
            &conn,
            "http://example.com/f11.rss",
            None,
            "F11",
            None,
            folder,
            "inherit",
            false,
            false,
        )
        .unwrap();
        // 直连风格两行：同 URL、各自源、均未绑定——full pull 必须把每条 entry
        // 绑到**自己源的那一行**（旧行为：URL 兜底只命中最小 id 行，第二条 entry
        // 被记成副本、行永不绑定）。
        let article = |guid: &str| db::NewArticle {
            guid: guid.into(),
            url: Some(SHARED.into()),
            title: "direct copy".into(),
            author: None,
            summary: None,
            content_html: Some("<p>c</p>".into()),
            body_text: "c".into(),
            image_url: None,
            enclosure_url: None,
            enclosure_mime: None,
            duration_sec: None,
            published_at: Some(chrono::Utc::now().to_rfc3339()),
            source: "direct".into(),
        };
        db::upsert_article_with_feed(&conn, feed10, &article("g-f10"), false).unwrap();
        db::upsert_article_with_feed(&conn, feed11, &article("g-f11"), false).unwrap();
    }
    let report = sync::sync_now(&db, &http()).await.expect("sync");
    assert!(report.errors.is_empty(), "{:?}", report.errors);

    {
        let conn = db.lock().await;
        let total: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM articles WHERE url_norm = ?1",
                [db::normalize_url(SHARED)],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            total, 2,
            "Fever 跨源同 URL 必须独立成行（修前被全局 URL 匹配吞成 1 行）"
        );
        assert!(
            bound_remote_id(&conn, 7001) && bound_remote_id(&conn, 7002),
            "两条 entry 必须各自绑定到自己的 feed 行"
        );
    }

    // 远端把 feed 10 的副本（7001）置为已读 → 7001 按自己的条目收敛已读；
    // 同 URL 的 7002 按 read-anywhere-wins 广播一并标读，且轮终 Fever 权威
    // 对账**不得立刻撤销**同一轮已接受的广播（N4：与 GR 同轮政策统一）。
    {
        let mut es = srv.entries.lock().unwrap();
        if let Some(e) = es.iter_mut().find(|e| e.id == 7001) {
            e.read = true;
        }
    }
    sync::sync_now(&db, &http()).await.expect("sync 2");
    {
        let conn = db.lock().await;
        assert_eq!(
            article_state(&conn, 7001),
            Some((true, false)),
            "A 远端已读按自己的条目收敛"
        );
        assert_eq!(
            article_state(&conn, 7002),
            Some((true, false)),
            "B 按同轮读广播一并标读（read-anywhere-wins；N4 起两协议同轮终值一致），而非 B 自己的远端 unread 立即撤销"
        );
        let total: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM articles WHERE url_norm = ?1",
                [db::normalize_url(SHARED)],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(total, 2, "读广播作用于已存在的兄弟行，不得合并/新增条目");
        assert!(
            bound_remote_id(&conn, 7001) && bound_remote_id(&conn, 7002),
            "广播后两条 entry 仍各自绑定的自己的行（身份独立未被读广播破坏）"
        );
    }

    // 下一轮远端把两条都显式置回未读（无同轮 read 信号）→ Fever 双向 unread
    // 权威恢复：本地已读（含上一轮广播落下的）被显式 unread 复活，不被历史
    // 读态永久阻挡（N4 裁定后半）。
    {
        let mut es = srv.entries.lock().unwrap();
        for e in es.iter_mut() {
            if e.id == 7001 || e.id == 7002 {
                e.read = false;
            }
        }
    }
    sync::sync_now(&db, &http()).await.expect("sync 3");
    {
        let conn = db.lock().await;
        assert_eq!(
            article_state(&conn, 7001),
            Some((false, false)),
            "远端显式 unread（无同轮读信号）复活本地未读"
        );
        assert_eq!(
            article_state(&conn, 7002),
            Some((false, false)),
            "历史广播读不得永久阻挡后续远端显式 unread"
        );
    }
    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
N4/R2 扩展反例（主控裁决：两协议同轮政策 + 失败恢复 + 清队版本）

- 排列覆盖：既有用例是「小 id 读、大 id 未读」（页序里未读行先处理、
  读行后广播命中它）；本段补**大 id 读、小 id 未读**（读行先处理、广播先于
  本人 own unread 落定）——两种页序都必须收敛到 read-anywhere-wins。
- 集合失败恢复：unread 集合拉取失败必须跳过对账（不伪空成功、不推完成时间），
  失败轮里由 items 合并产生的已读广播不回滚；下一轮集合恢复后：
  读信号仍在的轮不得撤销广播，双方显式 unread（无读信号）的轮照常复活。
- 清队版本：真实网络 push（Fever mark=item 端点）确认后队列清空；下一轮远端
  显式 unread 必须能穿透字段版本守卫（本地写已把 read_version 推到 1，远端
  合并不递增版本——版本守卫不得把「已确认的本地意图」当永久阻塞）。
  在途旧快照与确认清队的交错方向（响应屏障）由 OPT-007
  `pull_field_versions_e2e`（GR mock 屏障）覆盖；本段不重复屏障夹具，也不以
  直接 `prune_sync` 模拟冒充网络 push：push 经真实 HTTP 后再断言清队。
============================================================ */

/// R2 扩展夹具：两源同 URL 各一条远端 entry（id 分别指定），本地两源各自
/// 直连一行（未绑定）；`sync_now`（feeds→states 全量）按 URL 把每条 entry
/// 绑到自己源的那一行。返回 (db, tmp)。
async fn seed_cross_feed_pair(
    tag: &str,
    srv: &Arc<Fixture>,
    url: &str,
    remote_small: i64,
    remote_large: i64,
) -> (TestDb, std::path::PathBuf) {
    srv.set_groups(vec![(1, "G")]);
    srv.set_feeds(vec![
        (10, "F10", "http://example.com/f10.rss"),
        (11, "F11", "http://example.com/f11.rss"),
    ]);
    srv.add_entry_with_url(remote_small, 10, "copy-10", false, false, Some(url));
    srv.add_entry_with_url(remote_large, 11, "copy-11", false, false, Some(url));

    let (db, tmp) = seed_db(tag, srv);
    {
        let conn = db.lock().await;
        let folder = db::create_folder(&conn, "分类", "article").unwrap();
        let feed10 = db::insert_feed(
            &conn,
            "http://example.com/f10.rss",
            None,
            "F10",
            None,
            folder,
            "inherit",
            false,
            false,
        )
        .unwrap();
        let feed11 = db::insert_feed(
            &conn,
            "http://example.com/f11.rss",
            None,
            "F11",
            None,
            folder,
            "inherit",
            false,
            false,
        )
        .unwrap();
        let article = |guid: &str| db::NewArticle {
            guid: guid.into(),
            url: Some(url.into()),
            title: "direct copy".into(),
            author: None,
            summary: None,
            content_html: Some("<p>c</p>".into()),
            body_text: "c".into(),
            image_url: None,
            enclosure_url: None,
            enclosure_mime: None,
            duration_sec: None,
            published_at: Some(chrono::Utc::now().to_rfc3339()),
            source: "direct".into(),
        };
        db::upsert_article_with_feed(&conn, feed10, &article("g-f10"), false).unwrap();
        db::upsert_article_with_feed(&conn, feed11, &article("g-f11"), false).unwrap();
    }
    (db, tmp)
}

/// 队列里 `read` 动作的队项数。
fn queued_read_count(conn: &rusqlite::Connection) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM sync_queue WHERE action = 'read'",
        [],
        |r| r.get(0),
    )
    .unwrap()
}

/// 某行的 read_version（版本守卫断言：本地写递增、远端合并不递增）。
fn read_version_of(conn: &rusqlite::Connection, remote_id: i64) -> i64 {
    conn.query_row(
        "SELECT read_version FROM articles WHERE remote_id = ?1",
        [remote_id],
        |r| r.get(0),
    )
    .unwrap()
}

/// N4 排列二（大 id 读、小 id 未读）：页序按 id 降序 → 读行先落定并广播，
/// 尚未被自己条目合并的未读行必须先被广播标读；随后该行 own unread 处理时，
/// 同轮已读信号（显式集合）不得被推翻。轮终 Fever 权威对账同样不得撤销。
#[tokio::test]
async fn fever_same_round_large_id_read_small_id_unread_stays_read() {
    const SHARED: &str = "http://127.0.0.1:8765/post/fever-perm-large-read";
    let srv = Fixture::start().await;
    let (db, tmp) = seed_cross_feed_pair("fever_perm_large_read", &srv, SHARED, 7101, 7102).await;
    let report = sync::sync_now(&db, &http()).await.expect("sync bind");
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    {
        let conn = db.lock().await;
        assert!(
            bound_remote_id(&conn, 7101) && bound_remote_id(&conn, 7102),
            "两源同 URL 各自成行并绑定"
        );
        assert_eq!(article_state(&conn, 7101), Some((false, false)));
        assert_eq!(article_state(&conn, 7102), Some((false, false)));
    }

    // 远端只把**大 id**（7102）置读 → 页序 [7102, 7101]：读行先处理。
    {
        let mut es = srv.entries.lock().unwrap();
        if let Some(e) = es.iter_mut().find(|e| e.id == 7102) {
            e.read = true;
        }
    }
    sync::sync_now(&db, &http()).await.expect("sync 2");
    {
        let conn = db.lock().await;
        assert_eq!(
            article_state(&conn, 7102),
            Some((true, false)),
            "大 id 读行按自己的条目收敛"
        );
        assert_eq!(
            article_state(&conn, 7101),
            Some((true, false)),
            "小 id 未读行被同轮广播先标读；own unread 与轮终对账都不得推翻（顺序无关）"
        );
    }
    let _ = std::fs::remove_file(&tmp);
}

/// N4 集合失败与恢复（可解释链）：失败轮跳过对账但保留 items 合并的已读广播、
/// 不推完成时间；恢复轮里读信号仍在 → 广播不被撤销；双方显式 unread（无读
/// 信号）轮 → 双向 unread 权威恢复。
#[tokio::test]
async fn fever_collection_failure_round_and_next_round_recovery_are_explainable() {
    const SHARED: &str = "http://127.0.0.1:8765/post/fever-collection-failure";
    let srv = Fixture::start().await;
    let (db, tmp) =
        seed_cross_feed_pair("fever_collection_failure", &srv, SHARED, 7201, 7202).await;
    let r1 = sync::sync_now(&db, &http()).await.expect("sync bind");
    assert!(r1.errors.is_empty(), "{:?}", r1.errors);
    let ts1 = {
        let conn = db.lock().await;
        db::last_sync_ts(&conn).unwrap()
    };

    // 失败轮：7101 远端已读 + unread 集合拉取失败（响应缺字段）。
    {
        let mut es = srv.entries.lock().unwrap();
        if let Some(e) = es.iter_mut().find(|e| e.id == 7201) {
            e.read = true;
        }
    }
    srv.set_omit_unread(true);
    let r2 = sync::sync_now(&db, &http())
        .await
        .expect("sync 2 (collection failure)");
    assert!(
        r2.errors.iter().any(|e| e.contains("状态对账跳过")),
        "集合拉取失败必须显式报错（不得伪空成功）：{:?}",
        r2.errors
    );
    {
        let conn = db.lock().await;
        assert_eq!(
            db::last_sync_ts(&conn).unwrap(),
            ts1,
            "集合没拿全 → 完成时间不得推进（切回 greader 不跳窗口）"
        );
        assert_eq!(
            article_state(&conn, 7201),
            Some((true, false)),
            "失败轮 items 合并仍生效：读行自身收敛"
        );
        assert_eq!(
            article_state(&conn, 7202),
            Some((true, false)),
            "失败轮跳过对账不等于撤销 items 合并产生的已读广播"
        );
    }

    // 恢复轮：集合恢复（7201 仍读、7202 仍未读）。读信号仍在 → 广播不被撤销。
    srv.set_omit_unread(false);
    let r3 = sync::sync_now(&db, &http())
        .await
        .expect("sync 3 (recovered)");
    assert!(r3.errors.is_empty(), "{:?}", r3.errors);
    {
        let conn = db.lock().await;
        assert_eq!(
            article_state(&conn, 7202),
            Some((true, false)),
            "恢复轮里同轮已读广播继续有效：own 远端 unread 不得撤销（N4 统一政策）"
        );
    }

    // 显式 unread 轮：远端把两条都置回未读（无任何读信号）→ 双向权威恢复。
    {
        let mut es = srv.entries.lock().unwrap();
        for e in es.iter_mut() {
            if e.id == 7201 || e.id == 7202 {
                e.read = false;
            }
        }
    }
    sync::sync_now(&db, &http()).await.expect("sync 4");
    {
        let conn = db.lock().await;
        assert_eq!(article_state(&conn, 7201), Some((false, false)));
        assert_eq!(
            article_state(&conn, 7202),
            Some((false, false)),
            "失败轮与广播读都不得永久阻挡后续远端显式 unread"
        );
    }
    let _ = std::fs::remove_file(&tmp);
}

/// N4 清队版本（真实网络 push）：本地标读经闭包把同文两行标读+入队 →
/// `sync_now` 走 Fever `mark=item` 端点真实推送 → 服务端状态已读、队列被
/// 确认清空（不是直接 prune 模拟）→ 下一轮远端显式 unread 穿透字段版本守卫
/// （read_version=1 的本地意图 + 无 pending + 快照等于当前版本）恢复未读，
/// 且远端合并不递增版本。
#[tokio::test]
async fn fever_push_confirmed_read_then_next_round_explicit_unread_revives() {
    const SHARED: &str = "http://127.0.0.1:8765/post/fever-push-confirm";
    let srv = Fixture::start().await;
    let (db, tmp) = seed_cross_feed_pair("fever_push_confirm", &srv, SHARED, 7301, 7302).await;
    let r1 = sync::sync_now(&db, &http()).await.expect("sync bind");
    assert!(r1.errors.is_empty(), "{:?}", r1.errors);
    let aid_small = {
        let conn = db.lock().await;
        assert!(bound_remote_id(&conn, 7301) && bound_remote_id(&conn, 7302));
        assert_eq!(queued_read_count(&conn), 0);
        conn.query_row("SELECT id FROM articles WHERE remote_id = 7301", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap()
    };

    // 本地对 7301 行标读：读闭包应把同文兄弟（7302 行）一并标读并各自入队；
    // 回执是**真实翻转行**（两条都在内）。
    let receipts = {
        let conn = db.lock().await;
        db::set_read_with_enqueue(&conn, aid_small, true).unwrap()
    };
    assert_eq!(
        receipts.len(),
        2,
        "读闭包回执应包含两条真实翻转行：{receipts:?}"
    );
    {
        let conn = db.lock().await;
        assert_eq!(queued_read_count(&conn), 2, "两行各自入队 read（本地半边）");
        assert_eq!(read_version_of(&conn, 7301), 1);
        assert_eq!(read_version_of(&conn, 7302), 1);
    }

    // 真实网络 push：Fever mark=item 端点被命中，服务端两条变已读，队列清空。
    let r2 = sync::sync_now(&db, &http()).await.expect("sync push");
    assert!(r2.errors.is_empty(), "{:?}", r2.errors);
    assert!(
        r2.pushed_states >= 2,
        "两条 read 意图都应经网络推送（pushed_states={}）",
        r2.pushed_states
    );
    {
        let es = srv.entries.lock().unwrap();
        let read_of = |id: i64| es.iter().find(|e| e.id == id).map(|e| e.read);
        assert_eq!(read_of(7301), Some(true), "真实 push 后服务端 7301 已读");
        assert_eq!(read_of(7302), Some(true), "读广播同样经网络到达 7302");
    }
    {
        let conn = db.lock().await;
        assert_eq!(
            queued_read_count(&conn),
            0,
            "push 确认成功 → 队项被真实确认清空（非模拟 prune）"
        );
        assert_eq!(article_state(&conn, 7301), Some((true, false)));
        assert_eq!(article_state(&conn, 7302), Some((true, false)));
    }

    // 下一轮远端显式 unread（无同轮读信号）：版本守卫必须放行远端合并
    // （pending 已清、快照=当前版本），本地恢复未读；远端合并不递增版本。
    {
        let mut es = srv.entries.lock().unwrap();
        for e in es.iter_mut() {
            if e.id == 7301 || e.id == 7302 {
                e.read = false;
            }
        }
    }
    let r3 = sync::sync_now(&db, &http()).await.expect("sync 3");
    assert!(r3.errors.is_empty(), "{:?}", r3.errors);
    {
        let conn = db.lock().await;
        assert_eq!(
            article_state(&conn, 7301),
            Some((false, false)),
            "清队后远端显式 unread 穿透版本守卫（未被本地已读意图永久阻塞）"
        );
        assert_eq!(article_state(&conn, 7302), Some((false, false)));
        assert_eq!(
            read_version_of(&conn, 7301),
            1,
            "远端合并不递增版本（版本只记录本地意图）"
        );
        assert_eq!(queued_read_count(&conn), 0, "远端 unread 不产生推送队项");
    }
    let _ = std::fs::remove_file(&tmp);
}
