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
    pub articles: Vec<ParsedArticle>,
}

impl ParsedFeed {
    pub fn push_article(&mut self, article: NewArticle) {
        self.articles.push(article.into());
    }
}

/// 单条直连条目及其日期来源元数据。未来日期被钳制后，值不再是源发布时间；
/// 该标记必须随条目一起进入 DB，不能靠比较 `published_at` 与当前时间反推。
pub struct ParsedArticle {
    pub article: NewArticle,
    pub published_at_clamped: bool,
}

impl std::ops::Deref for ParsedArticle {
    type Target = NewArticle;

    fn deref(&self) -> &Self::Target {
        &self.article
    }
}

impl From<NewArticle> for ParsedArticle {
    fn from(article: NewArticle) -> Self {
        Self {
            article,
            published_at_clamped: false,
        }
    }
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

fn map_entry(e: &feed_rs::model::Entry, base: &str) -> Option<ParsedArticle> {
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

    let title_text = title.unwrap_or_else(|| "(untitled)".into());
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

    // 源日期与「是否缺失」是两件事：缺失时 published_at 保持 None，由 DB 用
    // first_seen_at 兜底并把来源标为 inferred_fetch；不能在这里写 Utc::now()，
    // 否则每次刷新都会把抓取时间伪装成发布时间、并污染无 id 条目的身份。
    let source_date = e.published.or(e.updated);
    let is_future = source_date
        .as_ref()
        .is_some_and(|d| *d > Utc::now() + chrono::Duration::hours(24));
    // 观察保留源的原始 UTC 日期。显示仍按旧规则 clamp，但 clamp/fetch 时刻
    // 不能替代原始日期进入空候选；DB 非空内容键不消费该观察 token。
    let observed_date = source_date.as_ref().map(|d| d.to_rfc3339());
    let published_at = source_date.map(|d| clamp_publish_date(d).to_rfc3339());

    // Note: 弱观察须同时保留URL和原始源日期，空字节不提供内容证明 — 见
    // .agents/notes/proposed/architecture/2026-11-09-源内等价组与持久条目状态.md
    let guid = if !e.id.trim().is_empty() && e.id != NO_STABLE_ID {
        e.id.clone()
    } else {
        crate::db::weak_observation_guid(url.as_deref(), &title_text, observed_date.as_deref())
    };

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
                .iter()
                .find(|thumbnail| {
                    crate::sanitize::cover_image_candidate(
                        &thumbnail.image.uri,
                        thumbnail.image.width,
                        thumbnail.image.height,
                        "",
                    )
                })
                .map(|t| t.image.uri.clone())
                .or_else(|| {
                    m.content.iter().find_map(|c| {
                        let is_img = c
                            .content_type
                            .as_ref()
                            .map(|t| t.ty().as_str() == "image")
                            .unwrap_or(false);
                        if is_img
                            && c.url.as_ref().is_some_and(|url| {
                                crate::sanitize::cover_image_candidate(
                                    url.as_str(),
                                    c.width,
                                    c.height,
                                    "",
                                )
                            })
                        {
                            c.url.as_ref().map(|u| u.to_string())
                        } else {
                            None
                        }
                    })
                })
        })
        .or_else(|| {
            crate::sanitize::first_image(&raw_html).and_then(|raw| {
                let resolved = url::Url::parse(&raw)
                    .or_else(|_| url::Url::parse(base).and_then(|base| base.join(&raw)))
                    .ok()?;
                matches!(resolved.scheme(), "http" | "https").then(|| resolved.to_string())
            })
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

    Some(ParsedArticle {
        article: NewArticle {
            guid,
            url,
            title: title_text,
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
        },
        published_at_clamped: is_future,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_selection_rejects_tiny_media_and_decorative_body_before_normal_photo() {
        let xml = r#"<rss version="2.0" xmlns:media="http://search.yahoo.com/mrss/"><channel><title>F</title><link>https://example.com</link><description>F</description><item><guid>x</guid><title>T</title><link>https://example.com/post</link><media:thumbnail url="https://example.com/tiny.jpg" width="1" height="1"/><media:content url="https://example.com/tiny-content.jpg" type="image/jpeg" width="32" height="32"/><description><![CDATA[<img class="author-avatar" src="https://example.com/avatar.jpg" width="120" height="120"><img src="/photo.jpg">]]></description></item></channel></rss>"#;
        let feed = parse_feed(xml.as_bytes(), "https://example.com/feed").unwrap();
        assert_eq!(
            feed.articles[0].image_url.as_deref(),
            Some("https://example.com/photo.jpg")
        );
        let xml = xml.replace("width=\"1\" height=\"1\"", "width=\"640\" height=\"360\"");
        assert_eq!(
            parse_feed(xml.as_bytes(), "https://example.com/feed")
                .unwrap()
                .articles[0]
                .image_url
                .as_deref(),
            Some("https://example.com/tiny.jpg"),
            "正常媒体缩略图仍优先"
        );
    }

    fn feed(items: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel>
<title>F</title><link>https://feed.example/</link><description>d</description>
{items}
</channel></rss>"#
        )
    }

    #[test]
    fn missing_date_is_none_and_observation_is_repeatable() {
        let xml = feed(r#"<item><title>Same</title><description>same body</description></item>"#);
        let first = parse_feed(xml.as_bytes(), "https://feed.example/rss").unwrap();
        let second = parse_feed(xml.as_bytes(), "https://feed.example/rss").unwrap();
        assert!(first.articles[0].published_at.is_none());
        assert_eq!(first.articles[0].guid, second.articles[0].guid);
        assert!(first.articles[0].guid.starts_with('\u{1}'));
    }

    #[test]
    fn future_date_observation_keeps_original_date_instead_of_clamp_time() {
        let xml = feed(
            r#"<item>
                 <title>Future</title>
                 <description>future body</description>
                 <pubDate>Wed, 24 Aug 2033 11:00:00 GMT</pubDate>
               </item>"#,
        );
        let first = parse_feed(xml.as_bytes(), "https://feed.example/rss").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let second = parse_feed(xml.as_bytes(), "https://feed.example/rss").unwrap();
        assert_eq!(
            first.articles[0].guid, second.articles[0].guid,
            "钳制后的当前时间不得进入持久身份"
        );
        let published = first.articles[0]
            .published_at
            .as_deref()
            .expect("源日期存在（已钳制）");
        assert!(published.starts_with("20"));
        assert!(
            first.articles[0].published_at_clamped,
            "钳制后的值必须携带独立来源标记"
        );
    }

    #[test]
    fn real_id_precedes_url_and_url_identity_is_synthetic() {
        let xml = feed(
            r#"<item>
                 <title>With id</title>
                 <guid isPermaLink="false">real-id</guid>
                 <link>https://e.example/a</link>
               </item>
               <item>
                 <title>URL only</title>
                 <link>https://e.example/a</link>
               </item>"#,
        );
        let parsed = parse_feed(xml.as_bytes(), "https://feed.example/rss").unwrap();
        assert_eq!(parsed.articles[0].guid, "real-id");
        assert!(parsed.articles[1].guid.starts_with('\u{1}'));
        assert_ne!(parsed.articles[0].guid, parsed.articles[1].guid);
    }

    #[test]
    fn empty_observations_keep_raw_dates_with_and_without_urls() {
        for link in ["", "<link>https://e.example/shared</link>"] {
            let parse = |date: &str| {
                let xml = feed(&format!("<item><title>empty</title>{link}{date}</item>"));
                parse_feed(xml.as_bytes(), "https://feed.example/rss")
                    .unwrap()
                    .articles
                    .remove(0)
            };
            let absent = parse("");
            assert!(absent.published_at.is_none());
            assert_eq!(absent.guid, parse("").guid);
            let first = parse("<pubDate>Sat, 01 Jan 2000 00:00:00 GMT</pubDate>");
            let same_utc = parse("<pubDate>Sat, 01 Jan 2000 08:00:00 +0800</pubDate>");
            let second = parse("<pubDate>Sun, 02 Jan 2000 00:00:00 GMT</pubDate>");
            assert_eq!(first.guid, same_utc.guid);
            assert_ne!(first.guid, second.guid);
            assert_ne!(first.guid, absent.guid);
            assert!(
                first.content_html.as_deref().unwrap_or("").is_empty()
                    && first.body_text.is_empty()
            );
            let future = parse("<pubDate>Wed, 24 Aug 2033 11:00:00 GMT</pubDate>");
            std::thread::sleep(std::time::Duration::from_millis(20));
            let replay = parse("<pubDate>Wed, 24 Aug 2033 11:00:00 GMT</pubDate>");
            assert!(future.published_at_clamped && replay.published_at_clamped);
            assert_ne!(future.published_at, replay.published_at);
            assert_eq!(future.guid, replay.guid);
            assert_ne!(
                future.guid,
                parse("<pubDate>Thu, 25 Aug 2033 11:00:00 GMT</pubDate>").guid
            );
            assert_ne!(future.guid, absent.guid);
        }
    }

    #[test]
    fn atom_updated_fallback_is_observed_before_clamping() {
        let parse = |updated: &str| {
            let xml = format!(
                r#"<feed xmlns="http://www.w3.org/2005/Atom"><title>F</title><id>feed</id><updated>2000-01-01T00:00:00Z</updated><entry><title>empty</title><link href="https://e.example/shared"/><updated>{updated}</updated></entry></feed>"#
            );
            parse_feed(xml.as_bytes(), "https://feed.example/rss")
                .unwrap()
                .articles
                .remove(0)
        };
        let first = parse("2033-08-24T11:00:00Z");
        assert!(first.published_at_clamped);
        assert_eq!(first.guid, parse("2033-08-24T19:00:00+08:00").guid);
        assert_ne!(first.guid, parse("2033-08-25T11:00:00Z").guid);
    }
}
