//! OPML 导入导出 —— 订阅列表的可移植格式（各阅读器通吃的迁移载体）。

use crate::error::{AppError, AppResult};
use opml::{Head, Outline, OPML};
use std::collections::BTreeMap;

/// 从 OPML 解析出的一条订阅。
pub struct ImportedFeed {
    pub feed_url: String,
    pub title: String,
    pub folder: Option<String>,
}

/// 解析 OPML 文档 → 扁平的 (feed_url, title, folder) 列表。
///
/// 先过 [`tidy`]：真实世界的 OPML 导出（Readwise Reader 等）经常在 URL 和
/// 标题里裸写 `&`（如 `?type=etoc&feed=rss`）而不是规范的 `&amp;`，严格
/// XML 解析器遇到第一个就拒掉整份文档，没有这步整个导入静默失败。
pub fn parse(content: &str) -> AppResult<Vec<ImportedFeed>> {
    let doc = OPML::from_str(&tidy(content)).map_err(|e| AppError::new("opml", e.to_string()))?;
    // 真实导出常把同一 URL 重复放在多个目录（NetNewsWire 的 Today/All/Unread
    // 视图都指向同一源）。按 xml_url 去重，保留首次出现（原目录归属）。
    let mut seen = std::collections::HashSet::new();
    let mut feeds = Vec::new();
    let mut collect_into: Vec<ImportedFeed> = Vec::new();
    for outline in &doc.body.outlines {
        collect(outline, None, &mut collect_into);
    }
    for feed in collect_into {
        if seen.insert(feed.feed_url.clone()) {
            feeds.push(feed);
        }
    }
    Ok(feeds)
}

/// 把裸 `&`（未开启合法 XML 实体的）转义为 `&amp;`，让不规范文件可解析。
/// 已是合法实体的 `&` 原样保留（避免 `&amp;` → `&amp;amp;` 双重转义）。
fn tidy(content: &str) -> String {
    let mut out = String::with_capacity(content.len() + 32);
    for (idx, c) in content.char_indices() {
        if c == '&' && !starts_valid_entity(&content[idx + 1..]) {
            out.push_str("&amp;");
        } else {
            out.push(c);
        }
    }
    out
}

/// `&` 后紧跟的文本是否是合法 XML 实体（命名或数字）。
fn starts_valid_entity(rest: &str) -> bool {
    for name in ["amp;", "lt;", "gt;", "quot;", "apos;"] {
        if rest.starts_with(name) {
            return true;
        }
    }
    if let Some(after) = rest.strip_prefix('#') {
        let (body, hex) = match after.strip_prefix(['x', 'X']) {
            Some(b) => (b, true),
            None => (after, false),
        };
        if let Some(semi) = body.find(';') {
            return semi > 0
                && body[..semi]
                    .chars()
                    .all(|c| if hex { c.is_ascii_hexdigit() } else { c.is_ascii_digit() });
        }
    }
    false
}

/// outline 的人读标签：`text` 优先，回退 `title`。空白-only 视为缺失
/// （否则空名目录会让整份导入在 create_folder 上中断）。
fn outline_label(outline: &Outline) -> Option<&str> {
    let label = if !outline.text.trim().is_empty() {
        Some(outline.text.as_str())
    } else {
        outline.title.as_deref()
    };
    label.filter(|t| !t.trim().is_empty())
}

fn collect(outline: &Outline, folder: Option<&str>, out: &mut Vec<ImportedFeed>) {
    if let Some(url) = &outline.xml_url {
        let title = outline_label(outline).unwrap_or(url).to_string();
        out.push(ImportedFeed {
            feed_url: url.clone(),
            title,
            folder: folder.map(|s| s.to_string()),
        });
    }
    // 无 xml_url 且有子节点的 outline 充当目录；标签解析与 feed 一致。
    let child_folder = if outline.xml_url.is_none() && !outline.outlines.is_empty() {
        outline_label(outline).or(folder)
    } else {
        folder
    };
    for child in &outline.outlines {
        collect(child, child_folder, out);
    }
}

/// 从 (title, feed_url, folder) 元组构建 OPML 文档。
/// 空 URL 跳过；空/空白目录名归入未分组（与 parse 对称）。
pub fn build(feeds: &[(String, String, Option<String>)]) -> AppResult<String> {
    let mut doc = OPML {
        head: Some(Head {
            title: Some("FluxReader Subscriptions".to_string()),
            ..Head::default()
        }),
        ..OPML::default()
    };

    let mut by_folder: BTreeMap<Option<String>, Vec<Outline>> = BTreeMap::new();
    for (title, url, folder) in feeds {
        let trimmed_url = url.trim();
        if trimmed_url.is_empty() {
            continue;
        }
        let normalised_folder = folder
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let outline = Outline {
            text: title.clone(),
            xml_url: Some(trimmed_url.to_string()),
            r#type: Some("rss".to_string()),
            ..Outline::default()
        };
        by_folder.entry(normalised_folder).or_default().push(outline);
    }

    for (folder, outlines) in by_folder {
        match folder {
            Some(name) => doc.body.outlines.push(Outline {
                text: name,
                outlines,
                ..Outline::default()
            }),
            None => doc.body.outlines.extend(outlines),
        }
    }

    doc.to_string().map_err(|e| AppError::new("opml", e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{build, parse};

    #[test]
    fn parses_flat_feed_list() {
        let xml = r#"<opml version="2.0"><head/><body>
            <outline text="Blog A" xmlUrl="https://a.example/feed.xml"/>
            <outline text="Blog B" xmlUrl="https://b.example/feed.xml"/>
        </body></opml>"#;
        let feeds = parse(xml).expect("parse");
        assert_eq!(feeds.len(), 2);
        assert_eq!(feeds[0].title, "Blog A");
        assert!(feeds[0].folder.is_none());
    }

    #[test]
    fn parses_feeds_nested_in_a_folder() {
        let xml = r#"<opml version="2.0"><head/><body>
            <outline text="Tech">
                <outline text="Feed 1" xmlUrl="https://1.example/f"/>
                <outline text="Feed 2" xmlUrl="https://2.example/f"/>
            </outline>
        </body></opml>"#;
        let feeds = parse(xml).expect("parse");
        assert_eq!(feeds.len(), 2);
        assert!(feeds.iter().all(|f| f.folder.as_deref() == Some("Tech")));
    }

    #[test]
    fn build_round_trips_through_parse() {
        let input = vec![
            ("Folderless".to_string(), "https://x.example/f".to_string(), None),
            ("In Folder".to_string(), "https://y.example/f".to_string(), Some("Tech".to_string())),
        ];
        let xml = build(&input).expect("build");
        let feeds = parse(&xml).expect("re-parse");
        assert_eq!(feeds.len(), 2);
        let foldered = feeds.iter().find(|f| f.feed_url == "https://y.example/f").unwrap();
        assert_eq!(foldered.folder.as_deref(), Some("Tech"));
    }

    #[test]
    fn bare_ampersands_are_tolerated() {
        let xml = r#"<opml version="1.0"><head/><body>
            <outline title="Cell Death & Disease" type="rss"
                     xmlUrl="https://www.science.org/action/showFeed?type=etoc&feed=rss&jc=stm"/>
        </body></opml>"#;
        let feeds = parse(xml).expect("bare & must not fail the parse");
        assert_eq!(feeds[0].title, "Cell Death & Disease");
    }

    #[test]
    fn duplicate_feed_url_is_collapsed() {
        let xml = r#"<opml version="1.0"><head/><body>
            <outline title="Today" xmlUrl="https://x.example/feed"/>
            <outline title="All Articles" xmlUrl="https://x.example/feed"/>
            <outline title="Other" xmlUrl="https://y.example/feed"/>
        </body></opml>"#;
        let feeds = parse(xml).expect("parse");
        assert_eq!(feeds.len(), 2);
    }

    #[test]
    fn parse_rejects_malformed_document() {
        assert!(parse("not opml at all").is_err());
    }
}
