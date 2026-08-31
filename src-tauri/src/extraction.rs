//! 全文提取：dom_smoothie（Readability 算法）从网页抽出正文，
//! 供「默认打开方式 = 自动全文」使用。
//!
//! dom_smoothie 的 reader 不是 Send，所以这里是纯同步函数 ——
//! 调用方须在 spawn_blocking 里跑，不能跨 .await。

use crate::error::{AppError, AppResult};
use crate::sanitize;
use dom_smoothie::Readability;
use scraper::{Html, Selector};
use std::sync::LazyLock;
use url::Url;

static LEAD_IMAGE_SELECTORS: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(
        r#"meta[property="og:image"], meta[name="og:image"],
           meta[property="twitter:image"], meta[name="twitter:image"],
           meta[itemprop="image"], link[rel="image_src"]"#,
    )
    .expect("lead image selector is valid")
});

/// 从整页 HTML 抽出正文（Readability）→ 消毒后返回。
pub fn extract_article(html: &str, url: &str) -> AppResult<String> {
    let mut readability = Readability::new(html, Some(url), None)
        .map_err(|e| AppError::internal(format!("readability init: {e}")))?;
    let article = readability
        .parse()
        .map_err(|e| AppError::internal(format!("readability parse: {e}")))?;
    let content = article.content.to_string();
    if content.trim().is_empty() {
        return Err(AppError::new("noExtractableContent", "页面没有可提取的正文"));
    }
    Ok(sanitize::sanitize(&content, Some(url)))
}

/// 从页面元数据取头图（og:image / twitter:image），相对 URL 按文章地址解析。
/// 摘要型 RSS（如少数派）不带 media 字段，但文章页有 og:image 可用作封面。
pub fn lead_image(html: &str, base: &str) -> Option<String> {
    let doc = Html::parse_document(html);
    doc.select(&LEAD_IMAGE_SELECTORS).find_map(|el| {
        let raw = el
            .value()
            .attr("content")
            .or_else(|| el.value().attr("href"))?
            .trim();
        resolve_http_url(raw, base)
    })
}

fn resolve_http_url(raw: &str, base: &str) -> Option<String> {
    if raw.is_empty() || raw.starts_with("data:") {
        return None;
    }
    let url = Url::parse(raw)
        .or_else(|_| Url::parse(base).and_then(|b| b.join(raw)))
        .ok()?;
    matches!(url.scheme(), "http" | "https").then(|| url.to_string())
}

#[cfg(test)]
mod tests {
    use super::lead_image;

    #[test]
    fn lead_image_reads_og_image() {
        let html = r#"<meta property="og:image" content="https://ex.com/a.jpg">"#;
        assert_eq!(
            lead_image(html, "https://site.test/post").as_deref(),
            Some("https://ex.com/a.jpg")
        );
    }

    #[test]
    fn lead_image_resolves_relative_urls() {
        let html = r#"<meta name="twitter:image" content="/img/a.jpg">"#;
        assert_eq!(
            lead_image(html, "https://site.test/post/1").as_deref(),
            Some("https://site.test/img/a.jpg")
        );
    }

    #[test]
    fn extract_article_pulls_main_content() {
        let html = r#"<html><body>
            <nav>导航 导航 导航</nav>
            <article><h1>标题</h1><p>这是正文第一段，长度足够让 Readability 认为它是主内容区域。</p>
            <p>第二段正文内容，继续保持足够的文本密度。</p></article>
            <footer>版权所有</footer>
        </body></html>"#;
        let out = super::extract_article(html, "https://site.test/post/1").unwrap();
        assert!(out.contains("正文第一段"), "extracted: {out}");
        assert!(!out.contains("版权所有"), "footer should be dropped: {out}");
    }
}
