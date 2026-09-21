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
        return Err(AppError::new(
            "noExtractableContent",
            "页面没有可提取的正文",
        ));
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

/// 是否应当**放弃**提取结果、保留原正文（「智能防退化」判定）。
///
/// TASK-076（P2-10 后半，DEC-req104-p2-10b-fulltext-degraded-20260920）：把判定从
/// 命令里抽成纯函数，使两条降级形态都能被直接断言，且让「为何降级」有明确文案。
/// 返回值：`None` = 采用提取结果；`Some(原因)` = 保留原文并说明原因。
///
/// 两种降级形态：
///   ① 提取结果为空（网页非 HTML、纯脚本页、解析不到主内容）；
///   ② 提取结果显著更短（不足原文 80%）——原文本身可能已是全文，换成更短的是退化。
pub fn degradation_reason(original_html: &str, extracted_html: &str) -> Option<&'static str> {
    let orig_len = sanitize::html_to_text(original_html).trim().len();
    let extracted_len = sanitize::html_to_text(extracted_html).trim().len();
    if extracted_len == 0 {
        return Some("提取结果为空：网页中未找到可用的正文内容");
    }
    if extracted_len * 5 < orig_len * 4 {
        return Some("提取结果比原正文更短，已保留原正文（原文可能已是全文）");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{degradation_reason, lead_image};

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

    /* ---------- TASK-076（P2-10 后半）：降级判定与原因 ---------- */

    /// 形态①：提取结果为空 → 必须降级并给出「提取为空」的原因。
    #[test]
    fn degradation_when_extraction_is_empty() {
        let original = "<p>RSS 摘要正文，长度足够。</p>";
        let reason = degradation_reason(original, "").expect("空提取结果必须判定为降级");
        assert!(
            reason.contains("为空"),
            "原因应说明提取结果为空，实际: {reason}"
        );
    }

    /// 形态①变体：提取结果只有空白（仍属空）。
    #[test]
    fn degradation_when_extraction_is_whitespace_only() {
        let original = "<p>RSS 摘要正文。</p>";
        assert!(degradation_reason(original, "   \n\t ").is_some());
    }

    /// 形态②：提取结果显著更短（不足原文 80%）→ 必须降级并保留原文。
    #[test]
    fn degradation_when_extraction_is_much_shorter() {
        let original = "<p>这是一段本来就已经是全文的正文，长度远远超过提取结果。</p>";
        let extracted = "<p>短</p>";
        let reason = degradation_reason(original, extracted).expect("显著更短必须判定为降级");
        assert!(
            reason.contains("更短") || reason.contains("保留原正文"),
            "原因应说明比原文更短，实际: {reason}"
        );
    }

    /// 成功路径：提取结果不短于原文 → 不降级（返回 None，调用方采用提取结果）。
    #[test]
    fn no_degradation_when_extraction_is_not_shorter() {
        let original = "<p>短摘要</p>";
        let extracted = "<p>这是从网页提取出来的完整正文，明显比原来的 RSS 摘要长得多。</p>";
        assert_eq!(
            degradation_reason(original, extracted),
            None,
            "更长的提取结果不得被判定为降级"
        );
    }

    /// 边界：原文为空时，任何非空提取结果都应被采用（不因 0 长度而误判退化）。
    #[test]
    fn empty_original_never_degrades_a_nonempty_extraction() {
        let extracted = "<p>网页正文</p>";
        assert_eq!(degradation_reason("", extracted), None);
    }
}
