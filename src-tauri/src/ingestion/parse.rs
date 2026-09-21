//! feed-rs 解析层：响应字节 → ParsedFeed（feed 元数据 + 条目）。

use crate::db::NewArticle;
use crate::error::AppResult;
use chrono::{DateTime, Utc};

/// fetch-rs 无稳定 id 时（feed-rs 默认会 hash link+title 合成 id，token 化链接
/// 每次抓取都变导致重复入库）的哨兵值。控制字符不可能出现在真实 guid 中。
const NO_STABLE_ID: &str = "\u{1}fluxreader:no-stable-id\u{1}";


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
pub(super) fn resolve_url(href: &str, base: &str) -> String {
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

    // 发布时间：源未提供 pubDate/updated 时，回退为抓取时间（否则 published_at
    // 为 NULL，前端 publishedAt=0 显示成 1970-01-01，且「今天」过滤/排序都失准）。
    // 有真实 guid 的条目去重键不依赖时间，兜底不影响去重。
    let published_at = e
        .published
        .or(e.updated)
        .map(clamp_publish_date)
        .unwrap_or_else(Utc::now)
        .to_rfc3339();
    let published_at = Some(published_at);

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
        .or_else(|| {
            content_html
                .as_deref()
                .and_then(crate::sanitize::first_image)
        });

    // 播客 enclosure：音频/视频媒体（type 缺失时按扩展名推断）
    let enclosure = e.media.iter().flat_map(|m| m.content.iter()).find_map(|c| {
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
