//! HTML 消毒与文本抽取：所有 feed/网页来源的 HTML 进 reader webview 前必经。
//! 白名单式清洗（ammonia）+ 相对 URL 重写 + 惰性图片恢复。

use ammonia::{Builder, UrlRelative};
use scraper::{Html, Selector};
use std::sync::LazyLock;
use url::Url;

/// 消毒 feed HTML：安全渲染 + 相对 URL 以 base 重写为绝对
pub fn sanitize(html: &str, base: Option<&str>) -> String {
    let html = promote_lazy_images(html);

    let mut builder = Builder::default();
    builder
        .link_rel(Some("noopener noreferrer nofollow"))
        .add_generic_attributes(["loading"])
        // 图片不携带 Referer（绕过常见图床防盗链）
        .set_tag_attribute_value("img", "referrerpolicy", "no-referrer");

    if let Some(b) = base.and_then(|b| Url::parse(b).ok()) {
        builder.url_relative(UrlRelative::RewriteWithBase(b));
    }
    builder.clean(&html).to_string()
}

/// 惰性图片恢复：`<img src="" data-src="https://…">` 的真实 URL 提升到 src，
/// 否则 ammonia 白名单会丢弃 data-* 属性导致图片无 URL 可加载。
fn promote_lazy_images(html: &str) -> String {
    let img_selector: LazyLock<Selector> =
        LazyLock::new(|| Selector::parse("img").expect("valid img selector"));
    let doc = Html::parse_document(html);
    if doc.select(&img_selector).next().is_none() {
        return html.to_string();
    }
    let mut out = html.to_string();
    for img in doc.select(&img_selector) {
        let real_src = img.value().attr("src").is_some_and(|s| {
            let s = s.trim();
            !s.is_empty() && !s.starts_with("data:")
        });
        if real_src {
            continue;
        }
        let recovered = ["data-src", "data-original", "data-lazy-src"]
            .iter()
            .find_map(|k| img.value().attr(k))
            .or_else(|| {
                img.value()
                    .attr("srcset")
                    .and_then(|ss| ss.split(',').next())
                    .and_then(|c| c.trim().split_whitespace().next())
            });
        if let Some(u) = recovered.filter(|u| !u.is_empty()) {
            let placeholder = img.value().attr("src").unwrap_or("");
            let from = format!("src=\"{placeholder}\"");
            let to = format!("src=\"{u}\"");
            out = out.replacen(&from, &to, 1);
        }
    }
    out
}

/// HTML → 纯文本（snippet / 正文文本列）
pub fn html_to_text(html: &str) -> String {
    let doc = Html::parse_document(html);
    let selector: LazyLock<Selector> =
        LazyLock::new(|| Selector::parse("body").unwrap_or_else(|_| unreachable!()));
    let text = doc
        .select(&selector)
        .next()
        .map(|b| b.text().collect::<String>())
        .unwrap_or_default();
    normalize_ws(&text)
}

/// 正文第一张图（卡片缩略图兜底）
pub fn first_image(html: &str) -> Option<String> {
    let img_selector: LazyLock<Selector> =
        LazyLock::new(|| Selector::parse("img[src]").expect("valid selector"));
    Html::parse_document(html)
        .select(&img_selector)
        .find_map(|i| {
            let src = i.value().attr("src")?;
            if src.starts_with("data:") {
                None
            } else {
                Some(src.to_string())
            }
        })
}

fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}
