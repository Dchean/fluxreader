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
