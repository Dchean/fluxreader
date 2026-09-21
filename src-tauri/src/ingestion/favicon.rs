//! favicon 自动发现：站点 `<link rel~=icon>` 优先，退回 `<origin>/favicon.ico`。

use super::parse::resolve_url;
use reqwest::Client;

/// favicon 发现负缓存（feed_id 集合）：发现失败的源本进程生命周期内不重试。
/// 成功的 icon 已写库（feeds.favicon_url），不依赖此表。
pub(super) static FAVICON_TRIED: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<i64>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));

/// favicon 自动发现：先取站点 HTML 解析 `<link rel~=icon>`（svg/png 优先），
/// 失败或无 link 则退回 `<origin>/favicon.ico`。返回通过 GET 探活
/// 确认可达（200-299 且非 HTML）的图标 URL；任何失败返回 None（调用方静默）。
/// UA 用浏览器伪装——部分站点对非浏览器 UA 直接 403。
/// 超时收紧（首页 5s / 探活 3s）：favicon 是锦上添花，不值得长等——
/// 探测发生在刷新信号量内，超时越长并发刷新被拖得越久。
pub(super) async fn discover_favicon(client: &Client, site_url: &str) -> Option<String> {
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
    let link_icon = extract_icon_link(&html).map(|href| resolve_url(&href, &base));

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
        assert_eq!(
            extract_icon_link(h1).as_deref(),
            Some("/Icon.PNG"),
            "大小写保持原文"
        );
        // shortcut icon 变体 + 单引号
        let h2 = r#"<link rel='shortcut icon' href='/f.ico'>"#;
        assert_eq!(extract_icon_link(h2).as_deref(), Some("/f.ico"));
        // 属性顺序不固定（href 在前）
        let h3 = r#"<link href="https://cdn.example/a.svg" rel="icon" type="image/svg+xml">"#;
        assert_eq!(
            extract_icon_link(h3).as_deref(),
            Some("https://cdn.example/a.svg")
        );
        // 大写属性名 + 大小写混合 rel
        let h4 = r#"<LINK REL="Icon" HREF="https://x.example/i.png">"#;
        assert_eq!(
            extract_icon_link(h4).as_deref(),
            Some("https://x.example/i.png")
        );
        // 无 icon link → None
        assert_eq!(
            extract_icon_link(r#"<link rel="stylesheet" href="a.css">"#),
            None
        );
        // 非 icon 的 link 不误中（含 "icon" 子串的其他 rel）
        assert_eq!(extract_icon_link(r#"<link rel="iconx" href="a">"#), None);
    }

    #[test]
    fn extract_html_attr_all_quote_styles() {
        assert_eq!(
            extract_html_attr(r#"rel="icon" href="/a.png" "#, "href").as_deref(),
            Some("/a.png")
        );
        assert_eq!(
            extract_html_attr("href='/b.ico'", "href").as_deref(),
            Some("/b.ico")
        );
        assert_eq!(
            extract_html_attr("href=bare.ico ", "href").as_deref(),
            Some("bare.ico")
        );
        // 独立属性名判定（X-HREF 不得误中 href）
        assert_eq!(extract_html_attr("data-href='no'", "href"), None);
    }
}

