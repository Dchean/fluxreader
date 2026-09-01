//! 直连抓取管线（实施方案 §4.2 第一优先级）：
//! 条件 GET（ETag/If-Modified-Since）→ feed-rs 解析 → HTML 消毒 → upsert（source='direct'）。
//!
//! 失败时标记 feeds.fetch_failed=1 供 Miniflux 兜底路径查询。

use crate::db::{self, NewArticle};
use crate::error::{AppError, AppResult};
use chrono::{DateTime, Utc};
use reqwest::header::{CONTENT_TYPE, ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};
use reqwest::{Client, StatusCode};
use rusqlite::Connection;
use std::sync::Arc;
use std::time::Duration;

/// 桌面 RSS 客户端身份标识（避免被站点风控误伤为爬虫脚本）
pub const USER_AGENT: &str = "FluxReader/0.1 (+https://github.com/fluxreader; RSS reader)";

/// 响应体大小上限：feed 是文本，16 MiB 已很宽裕，防恶意/异常响应耗尽内存
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// fetch-rs 无稳定 id 时（feed-rs 默认会 hash link+title 合成 id，token 化链接
/// 每次抓取都变导致重复入库）的哨兵值。控制字符不可能出现在真实 guid 中。
const NO_STABLE_ID: &str = "\u{1}fluxreader:no-stable-id\u{1}";

/* ============================================================
   HTTP
   ============================================================ */

pub fn build_client(timeout_secs: u64) -> Client {
    // 直连源站需要能走用户代理（国内网络访问境外 feed 常见需求）。
    // reqwest 默认读取 HTTP_PROXY/HTTPS_PROXY/ALL_PROXY 环境变量。
    Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(timeout_secs.clamp(5, 300)))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .expect("failed to build reqwest client")
}

/// 条件 GET 结果
pub enum Fetched {
    NotModified,
    Body {
        bytes: Vec<u8>,
        content_type: Option<String>,
        etag: Option<String>,
        last_modified: Option<String>,
    },
}

/// 分块读取响应体，超过上限即中止（防 Content-Length 撒谎的流式响应）
async fn read_capped(mut resp: reqwest::Response) -> AppResult<Vec<u8>> {
    if resp.content_length().is_some_and(|n| n > MAX_BODY_BYTES as u64) {
        return Err(AppError::new("responseTooLarge", "feed body exceeds 16 MiB"));
    }
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        if buf.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(AppError::new("responseTooLarge", "feed body exceeds 16 MiB"));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

/// 条件 GET：有 ETag/Last-Modified 时携带，304 直接返回未变更
pub async fn conditional_get(
    client: &Client,
    url: &str,
    etag: Option<&str>,
    last_modified: Option<&str>,
) -> AppResult<Fetched> {
    let mut req = client.get(url);
    if let Some(e) = etag {
        req = req.header(IF_NONE_MATCH, e);
    }
    if let Some(lm) = last_modified {
        req = req.header(IF_MODIFIED_SINCE, lm);
    }
    let resp = req.send().await?;
    if resp.status() == StatusCode::NOT_MODIFIED {
        return Ok(Fetched::NotModified);
    }
    let resp = resp.error_for_status()?;
    let header = |name: reqwest::header::HeaderName| {
        resp.headers()
            .get(&name)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
    };
    let etag = header(ETAG);
    let last_modified = header(LAST_MODIFIED);
    let content_type = header(CONTENT_TYPE);
    let bytes = read_capped(resp).await?;
    Ok(Fetched::Body { bytes, content_type, etag, last_modified })
}

/* ============================================================
   解析（feed-rs → NewArticle）
   ============================================================ */

/// 单次抓取解析出的 feed 元数据 + 条目
pub struct ParsedFeed {
    pub title: Option<String>,
    pub site_url: Option<String>,
    pub icon: Option<String>,
    pub articles: Vec<NewArticle>,
}

pub fn parse_feed(bytes: &[u8], base_url: &str) -> AppResult<ParsedFeed> {
    let raw = feed_rs::parser::Builder::new()
        .id_generator(|_links, _title, _uri| NO_STABLE_ID.to_string())
        .build()
        .parse(bytes)?;

    let site_url = raw
        .links
        .iter()
        .find(|l| l.rel.as_deref() == Some("alternate"))
        .or_else(|| raw.links.first())
        .map(|l| l.href.clone())
        .or_else(|| Some(base_url.to_string()));

    let base = site_url.as_deref().unwrap_or(base_url);
    let articles = raw
        .entries
        .iter()
        .filter_map(|e| map_entry(e, base))
        .collect();

    Ok(ParsedFeed {
        title: raw.title.map(|t| t.content),
        site_url,
        icon: raw.icon.or(raw.logo).map(|i| i.uri),
        articles,
    })
}

/// 相对链接解析为绝对 URL（Atom 相对 href 会破坏去重键与"打开原文"）
fn resolve_url(href: &str, base: &str) -> String {
    match url::Url::parse(href) {
        Ok(_) => href.to_string(),
        Err(_) => url::Url::parse(base)
            .ok()
            .and_then(|b| b.join(href).ok())
            .map(|u| u.to_string())
            .unwrap_or_else(|| href.to_string()),
    }
}

/// 未来时间钳制：脏 feed 常带未来日期，会把条目永久钉在列表顶部
fn clamp_publish_date(date: DateTime<Utc>) -> DateTime<Utc> {
    let now = Utc::now();
    if date > now + chrono::Duration::hours(24) {
        now
    } else {
        date
    }
}

fn map_entry(e: &feed_rs::model::Entry, base: &str) -> Option<NewArticle> {
    let url = e
        .links
        .iter()
        .find(|l| l.rel.as_deref() == Some("alternate"))
        .or_else(|| e.links.first())
        .map(|l| resolve_url(&l.href, base));

    let title = e
        .title
        .as_ref()
        .map(|t| t.content.trim().to_string())
        .filter(|t| !t.is_empty());

    let published_at = e
        .published
        .or(e.updated)
        .map(clamp_publish_date)
        .map(|d| d.to_rfc3339());

    // 去重键优先级：真实 guid → title+日期 → URL
    let guid = if !e.id.trim().is_empty() && e.id != NO_STABLE_ID {
        e.id.clone()
    } else if let Some(t) = title.as_deref() {
        format!("{t}\u{1f}{}", published_at.as_deref().unwrap_or(""))
    } else {
        url.clone()?
    };

    let raw_html = e
        .content
        .as_ref()
        .and_then(|c| c.body.clone())
        .or_else(|| e.summary.as_ref().map(|s| s.content.clone()))
        .unwrap_or_default();

    let content_html = if raw_html.is_empty() {
        None
    } else {
        Some(crate::sanitize::sanitize(&raw_html, Some(base)))
    };
    let body_text = crate::sanitize::html_to_text(&raw_html);

    let summary = e
        .summary
        .as_ref()
        .map(|s| crate::sanitize::html_to_text(&s.content))
        .filter(|s| !s.is_empty());

    // 图片：媒体缩略图 → 媒体内容 → 正文第一图
    let image_url = e
        .media
        .iter()
        .find_map(|m| {
            m.thumbnails
                .first()
                .map(|t| t.image.uri.clone())
                .or_else(|| {
                    m.content.iter().find_map(|c| {
                        let is_img = c
                            .content_type
                            .as_ref()
                            .map(|t| t.ty().as_str() == "image")
                            .unwrap_or(false);
                        if is_img {
                            c.url.as_ref().map(|u| u.to_string())
                        } else {
                            None
                        }
                    })
                })
        })
        .or_else(|| content_html.as_deref().and_then(crate::sanitize::first_image));

    // 播客 enclosure：音频/视频媒体（type 缺失时按扩展名推断）
    let enclosure = e
        .media
        .iter()
        .flat_map(|m| m.content.iter())
        .find_map(|c| {
            let u = c.url.as_ref()?.to_string();
            let declared = c
                .content_type
                .as_ref()
                .map(|t| t.to_string().to_ascii_lowercase());
            let mime = declared.or_else(|| mime_from_url(&u).map(String::from));
            let is_av = mime
                .as_deref()
                .map(|m| m.starts_with("audio") || m.starts_with("video"))
                .unwrap_or(false);
            if is_av {
                Some((u, mime, c.size.map(|s| s as i64)))
            } else {
                None
            }
        });

    // 时长（秒）：itunes:duration / media:content duration。注意 enclosure 的
    // size 是文件字节数，不是时长——播客卡片把它显示成 25:00 就是这个混淆。
    let duration_sec = e
        .media
        .iter()
        .find_map(|m| m.duration)
        .map(|d| d.as_secs().min(i64::MAX as u64) as i64)
        .filter(|d| *d > 0);

    Some(NewArticle {
        guid,
        url,
        title: title.unwrap_or_else(|| "(untitled)".into()),
        author: e.authors.first().map(|p| p.name.clone()),
        summary,
        content_html,
        body_text,
        image_url,
        enclosure_url: enclosure.as_ref().map(|(u, _, _)| u.clone()),
        enclosure_mime: enclosure.as_ref().and_then(|(_, m, _)| m.clone()),
        duration_sec,
        published_at,
        source: "direct".into(),
    })
}

/// 从 URL 扩展名推断媒体 MIME（enclosure 无 type 属性时兜底）
fn mime_from_url(url: &str) -> Option<&'static str> {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let ext = path.rsplit('.').next()?.to_ascii_lowercase();
    match ext.as_str() {
        "mp3" => Some("audio/mpeg"),
        "m4a" | "aac" => Some("audio/aac"),
        "ogg" | "oga" | "opus" => Some("audio/ogg"),
        "wav" => Some("audio/wav"),
        "flac" => Some("audio/flac"),
        "mp4" | "m4v" => Some("video/mp4"),
        "webm" => Some("video/webm"),
        "mov" => Some("video/quicktime"),
        _ => None,
    }
}

/* ============================================================
   单源刷新（direct 优先写库）
   ============================================================ */

/// 刷新单个源：304 → 不动；成功 → 清除失败标记 + 更新元数据 + upsert 条目；
/// 失败 → 标记 fetch_failed（Miniflux 兜底路径会查这张表）。
/// 返回本次新增条目数。dedup：同 URL 跨源去重（智能去重开关）。
///
/// 注意：此签名在**锁外**调用没有意义——conn 借用即持锁。仅适合
/// `refresh_feed` 命令（单源、调用方一次只抓一个）与既有测试复用。
pub async fn refresh_feed(conn: &mut Connection, client: &Client, feed_id: i64, dedup: bool) -> AppResult<usize> {
    // feed 行（URL + 条件 GET 头）
    let (feed_url, etag, last_modified): (String, Option<String>, Option<String>) = {
        conn.query_row(
            "SELECT feed_url, etag, last_modified FROM feeds WHERE id = ?1",
            rusqlite::params![feed_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|_| AppError::not_found(format!("feed {feed_id} not found")))?
    };

    let fetched = conditional_get(client, &feed_url, etag.as_deref(), last_modified.as_deref()).await;

    match fetched {
        Ok(Fetched::NotModified) => {
            db::set_feed_fetch_state(conn, feed_id, false, None, etag.as_deref(), last_modified.as_deref())?;
            Ok(0)
        }
        Ok(Fetched::Body { bytes, content_type, etag, last_modified }) => {
            let parsed = parse_feed(&bytes, &feed_url)?;
            let _ = content_type; // feed-rs 自带编码探测，无需手动解码

            db::set_feed_title_and_icon(conn, feed_id, parsed.title.as_deref(), parsed.icon.as_deref(), parsed.site_url.as_deref())?;
            db::set_feed_fetch_state(conn, feed_id, false, None, etag.as_deref(), last_modified.as_deref())?;

            let mut new_count = 0;
            for a in &parsed.articles {
                let (_, was_new) = db::upsert_article_with_feed(conn, feed_id, a, dedup)?;
                if was_new {
                    new_count += 1;
                }
            }
            Ok(new_count)
        }
        Err(e) => {
            db::set_feed_fetch_state(conn, feed_id, true, Some(&e.message), etag.as_deref(), last_modified.as_deref())?;
            Err(e)
        }
    }
}

/* ============================================================
   三段式刷新管线（并发安全版）
   ============================================================ */

/// 单源刷新的第一阶段：锁内读 feed 行（URL + 条件 GET 头）。
/// 读到的快照交给锁外的 `fetch_and_parse`，HTTP 期间不占数据库锁——
/// 这是并发抓取能真正并行（而非被 Mutex 串行化）的前提。
pub fn read_feed_for_refresh(conn: &Connection, feed_id: i64) -> AppResult<(String, Option<String>, Option<String>)> {
    conn.query_row(
        "SELECT feed_url, etag, last_modified FROM feeds WHERE id = ?1",
        rusqlite::params![feed_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .map_err(|_| AppError::not_found(format!("feed {feed_id} not found")))
}

/// 第二阶段：锁外条件 GET + 解析。HTTP 失败与解析失败统一为 Err，
/// 调用方据此走失败写回；304 与 200-带-body 的区分在第三阶段处理。
pub async fn fetch_and_parse(
    client: &Client,
    feed_url: &str,
    etag: Option<&str>,
    last_modified: Option<&str>,
) -> AppResult<(Fetched, ParsedFeed)> {
    let fetched = conditional_get(client, feed_url, etag, last_modified).await?;
    match &fetched {
        Fetched::NotModified => Ok((fetched, ParsedFeed { title: None, site_url: None, icon: None, articles: Vec::new() })),
        Fetched::Body { bytes, .. } => {
            let parsed = parse_feed(bytes, feed_url)?;
            Ok((fetched, parsed))
        }
    }
}

/// 第三阶段：锁内写回。成功清失败标记 + 更新元数据 + upsert 条目；
/// 失败标记 fetch_failed（指数退避由 db 层维护）。
/// 返回本次新增条目数。old_etag/old_last_modified：DB 里读到的旧条件头，
/// 304 分支写回时复用（304 响应不带新头，写 None 会断掉条件 GET 链）。
pub fn apply_refresh_result(
    conn: &Connection,
    feed_id: i64,
    fetched: &Fetched,
    parsed: &ParsedFeed,
    dedup: bool,
    old_etag: Option<&str>,
    old_last_modified: Option<&str>,
) -> AppResult<usize> {
    match fetched {
        Fetched::NotModified => {
            db::set_feed_fetch_state(conn, feed_id, false, None, old_etag, old_last_modified)?;
            Ok(0)
        }
        Fetched::Body { etag, last_modified, .. } => {
            db::set_feed_title_and_icon(conn, feed_id, parsed.title.as_deref(), parsed.icon.as_deref(), parsed.site_url.as_deref())?;
            db::set_feed_fetch_state(conn, feed_id, false, None, etag.as_deref(), last_modified.as_deref())?;
            let mut new_count = 0;
            for a in &parsed.articles {
                let (_, was_new) = db::upsert_article_with_feed(conn, feed_id, a, dedup)?;
                if was_new {
                    new_count += 1;
                }
            }
            Ok(new_count)
        }
    }
}

/// 三段式整合入口：读快照 →（锁外 HTTP+解析）→ 写回。
/// 语义与旧 `refresh_feed` 一致，但 HTTP 期间不持数据库锁，
/// 多任务并发时网络等待真正重叠（旧版被外层 Mutex 串行化）。
pub async fn refresh_feed_staged(
    db: &Arc<tokio::sync::Mutex<Connection>>,
    client: &Client,
    feed_id: i64,
    dedup: bool,
) -> AppResult<usize> {
    let (feed_url, etag, last_modified) = {
        let conn = db.lock().await;
        read_feed_for_refresh(&conn, feed_id)?
    };

    match fetch_and_parse(client, &feed_url, etag.as_deref(), last_modified.as_deref()).await {
        Ok((fetched, parsed)) => {
            /* favicon 后台发现：feed 未带 icon 且 DB 无缓存且本进程未尝试过 →
               spawn 独立任务（不占刷新信号量、不拖慢刷新关键路径——favicon 是
               锦上添花）。负缓存防无 favicon 的站点每轮重付探测超时。 */
            if parsed.icon.is_none() {
                let (existing_icon, already_tried): (Option<String>, bool) = {
                    let conn = db.lock().await;
                    let icon = conn
                        .query_row(
                            "SELECT favicon_url FROM feeds WHERE id = ?1",
                            rusqlite::params![feed_id],
                            |r| r.get(0),
                        )
                        .ok()
                        .flatten();
                    (icon, FAVICON_TRIED.lock().unwrap().contains(&feed_id))
                };
                if existing_icon.is_none() && !already_tried {
                    FAVICON_TRIED.lock().unwrap().insert(feed_id);
                    let site = parsed.site_url.clone().or_else(|| Some(feed_url.clone()));
                    let db = db.clone();
                    let client = client.clone();
                    /* tokio::spawn（非 tauri::async_runtime）：本函数在测试里
                       无 Tauri 运行时也能跑；调度器/命令均在 tokio 上下文调用 */
                    tokio::spawn(async move {
                        let discovered = match site.as_deref() {
                            Some(s) => discover_favicon(&client, s).await,
                            None => None,
                        };
                        if let Some(icon) = discovered {
                            let conn = db.lock().await;
                            let _ = conn.execute(
                                "UPDATE feeds SET favicon_url = ?1 WHERE id = ?2 AND (favicon_url IS NULL OR favicon_url = '')",
                                rusqlite::params![icon, feed_id],
                            );
                        }
                        /* 失败留在 FAVICON_TRIED（本进程不再重试）；
                           前端下次 reload 拿到新 favicon（如有） */
                    });
                }
            }
            let conn = db.lock().await;
            apply_refresh_result(&conn, feed_id, &fetched, &parsed, dedup, etag.as_deref(), last_modified.as_deref())
        }
        Err(e) => {
            let conn = db.lock().await;
            let _ = db::set_feed_fetch_state(&conn, feed_id, true, Some(&e.message), etag.as_deref(), last_modified.as_deref());
            Err(e)
        }
    }
}

/// favicon 发现负缓存（feed_id 集合）：发现失败的源本进程生命周期内不重试。
/// 成功的 icon 已写库（feeds.favicon_url），不依赖此表。
static FAVICON_TRIED: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<i64>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));

/// favicon 自动发现：先取站点 HTML 解析 `<link rel~=icon>`（svg/png 优先），
/// 失败或无 link 则退回 `<origin>/favicon.ico`。返回通过 GET 探活
/// 确认可达（200-299 且非 HTML）的图标 URL；任何失败返回 None（调用方静默）。
/// UA 用浏览器伪装——部分站点对非浏览器 UA 直接 403。
/// 超时收紧（首页 5s / 探活 3s）：favicon 是锦上添花，不值得长等——
/// 探测发生在刷新信号量内，超时越长并发刷新被拖得越久。
async fn discover_favicon(client: &Client, site_url: &str) -> Option<String> {
    let origin = url::Url::parse(site_url).ok()?;
    let base = format!("{}://{}", origin.scheme(), origin.host_str()?);

    // ① 站点首页 HTML 里的 <link rel="icon">（rel 包含 icon 的各种变体）
    let html = client
        .get(&base)
        .header("User-Agent", BROWSER_UA)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .ok()?
        .text()
        .await
        .ok()?;
    let link_icon = extract_icon_link(&html)
        .map(|href| resolve_url(&href, &base));

    // ② 兜底 /favicon.ico
    let fallback = format!("{base}/favicon.ico");
    let candidates = link_icon.into_iter().chain(std::iter::once(fallback));
    for url in candidates {
        if let Ok(resp) = client
            .get(&url)
            .header("User-Agent", BROWSER_UA)
            .timeout(std::time::Duration::from_secs(3))
            .send()
            .await
        {
            if resp.status().is_success() {
                let ct = resp
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if !ct.starts_with("text/html") {
                    return Some(url);
                }
            }
        }
    }
    None
}

/// 从 HTML 提取 `<link rel="…icon…">` 的 href（首个命中）。
/// rel 判定在小写副本上（属性名/值大小写容错）；href 从**原文**提取
/// ——URL path 大小写敏感，小写化会拿到错误地址。
fn extract_icon_link(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let mut search = 0usize;
    while let Some(rel) = lower[search..].find("<link ") {
        let start = search + rel;
        let end = match lower[start..].find('>') {
            Some(e) => start + e,
            None => break,
        };
        let tag_lower = &lower[start..end];
        if rel_is_icon(tag_lower) {
            if let Some(href) = extract_html_attr(&html[start..end], "href") {
                if !href.is_empty() {
                    return Some(href);
                }
            }
        }
        search = end + 1;
    }
    None
}

/// rel 属性值是否为 icon 类。词级判定：rel="shortcut icon" 按空白拆词后
/// `icon` 完整相等，或 image/x-icon / mask-icon 等 `-icon` / `/icon` 后缀词
/// ——排除 `iconx` 子串误中。
fn rel_is_icon(tag_lower: &str) -> bool {
    let rel_val = extract_html_attr(tag_lower, "rel").unwrap_or_default();
    rel_val
        .split_whitespace()
        .map(|w| w.trim_matches('"').trim_matches('\''))
        .any(|w| w == "icon" || w.ends_with("/icon") || w.ends_with("-icon"))
}

/// 从标签原文提取 `name="…"` / `name='…'` / `name=裸值` 的值。
/// 属性名匹配容错大小写（Link HREF= 合法）；值保持原文（URL 大小写敏感）。
fn extract_html_attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let pat = format!("{name}=");
    let mut search = 0usize;
    while let Some(rel) = lower[search..].find(&pat) {
        let idx = search + rel;
        // 独立属性名：前一字符不是 [A-Za-z0-9-_.]
        let prev_ok = idx == 0
            || lower
                .as_bytes()
                .get(idx.wrapping_sub(1))
                .map(|c| !c.is_ascii_alphanumeric() && *c != b'-' && *c != b'_' && *c != b'.')
                .unwrap_or(true);
        if prev_ok {
            let vstart = idx + pat.len();
            let rest = tag.get(vstart..)?;
            let quote = rest.chars().next()?;
            return match quote {
                '"' | '\'' => {
                    let end = rest[1..].find(quote)?;
                    Some(rest[1..1 + end].to_string())
                }
                _ => Some(rest.split_whitespace().next()?.to_string()),
            };
        }
        search = idx + pat.len();
    }
    None
}

/// 浏览器伪装 UA（favicon 发现专用）
const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

#[cfg(test)]
mod favicon_tests {
    use super::*;

    #[test]
    fn extract_icon_link_finds_variants() {
        // 常规写法
        let h1 = r#"<head><link rel="icon" href="/Icon.PNG"><title>x</title></head>"#;
        assert_eq!(extract_icon_link(h1).as_deref(), Some("/Icon.PNG"), "大小写保持原文");
        // shortcut icon 变体 + 单引号
        let h2 = r#"<link rel='shortcut icon' href='/f.ico'>"#;
        assert_eq!(extract_icon_link(h2).as_deref(), Some("/f.ico"));
        // 属性顺序不固定（href 在前）
        let h3 = r#"<link href="https://cdn.example/a.svg" rel="icon" type="image/svg+xml">"#;
        assert_eq!(extract_icon_link(h3).as_deref(), Some("https://cdn.example/a.svg"));
        // 大写属性名 + 大小写混合 rel
        let h4 = r#"<LINK REL="Icon" HREF="https://x.example/i.png">"#;
        assert_eq!(extract_icon_link(h4).as_deref(), Some("https://x.example/i.png"));
        // 无 icon link → None
        assert_eq!(extract_icon_link(r#"<link rel="stylesheet" href="a.css">"#), None);
        // 非 icon 的 link 不误中（含 "icon" 子串的其他 rel）
        assert_eq!(extract_icon_link(r#"<link rel="iconx" href="a">"#), None);
    }

    #[test]
    fn extract_html_attr_all_quote_styles() {
        assert_eq!(extract_html_attr(r#"rel="icon" href="/a.png" "#, "href").as_deref(), Some("/a.png"));
        assert_eq!(extract_html_attr("href='/b.ico'", "href").as_deref(), Some("/b.ico"));
        assert_eq!(extract_html_attr("href=bare.ico ", "href").as_deref(), Some("bare.ico"));
        // 独立属性名判定（X-HREF 不得误中 href）
        assert_eq!(extract_html_attr("data-href='no'", "href"), None);
    }
}
