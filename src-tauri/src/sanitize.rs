//! HTML 消毒与文本抽取：所有 feed/网页来源的 HTML 进 reader webview 前必经。
//! 白名单式清洗（ammonia）+ 相对 URL 重写 + 惰性图片恢复 + 富媒体放行。

use ammonia::{Builder, UrlRelative};
use scraper::{Html, Selector};
use std::sync::LazyLock;
use url::Url;

/// iframe 嵌入域名白名单（与 dom_smoothie 的 VIDEO_DOMAINS 对齐 + 常见音视频嵌入站）。
/// 后缀匹配（含子域）；白名单外的 iframe 一律降级为外链，杜绝任意站点嵌套。
const IFRAME_HOSTS: &[&str] = &[
    "youtube.com",
    "youtube-nocookie.com",
    "youtu.be",
    "player.vimeo.com",
    "dailymotion.com",
    "bilibili.com",
    "hdslb.com",
    "v.qq.com",
    "player.twitch.tv",
    "archive.org",
    "music.163.com",
    "open.spotify.com",
    "w.soundcloud.com",
    "bandcamp.com",
    "player.fireside.fm",
    "podcasts.apple.com",
    "player.cntv.cn",
    "v.cctv.com",
    "miaopai.com",
    "pearvideo.com",
    "npr.org",
    "player.tudou.com",
];

fn iframe_host_allowed(host: &str) -> bool {
    let h = host.trim_start_matches("www.").to_ascii_lowercase();
    IFRAME_HOSTS.iter().any(|d| h == *d || h.ends_with(&format!(".{d}")))
}

/// 消毒 feed HTML：安全渲染 + 相对 URL 以 base 重写为绝对。
/// 放行正文内嵌 `<video>/<audio>/<source>/<track>`，以及域名白名单内的
/// `<iframe>`（YouTube/B 站等嵌入播放器）——白名单外的 iframe 降级为外链。
pub fn sanitize(html: &str, base: Option<&str>) -> String {
    let html = promote_lazy_images(&filter_iframes(html));

    let mut builder = Builder::default();
    builder
        .link_rel(Some("noopener noreferrer nofollow"))
        .add_generic_attributes(["loading"])
        // 图片不携带 Referer（绕过常见图床防盗链）
        .set_tag_attribute_value("img", "referrerpolicy", "no-referrer")
        // 正文内嵌媒体：src 走默认 scheme 白名单（http/https）
        .add_tags(["video", "audio", "source", "track", "iframe"])
        .add_tag_attributes(
            "video",
            ["src", "poster", "controls", "preload", "width", "height", "muted", "loop"],
        )
        .add_tag_attributes("audio", ["src", "controls", "preload", "loop"])
        .add_tag_attributes("source", ["src", "type", "media"])
        .add_tag_attributes("track", ["src", "kind", "srclang", "label"])
        // iframe 的 src 已在 filter_iframes 按域名预过滤，这里只放行展示属性
        .add_tag_attributes(
            "iframe",
            ["src", "width", "height", "allow", "allowfullscreen", "frameborder", "title"],
        );

    if let Some(b) = base.and_then(|b| Url::parse(b).ok()) {
        builder.url_relative(UrlRelative::RewriteWithBase(b));
    }
    builder.clean(&html).to_string()
}

/// iframe 预过滤：逐个扫描 `<iframe …>` 开标签——
/// - src 域名在白名单内 → 原样保留（后续 ammonia 只放行标签与展示属性）
/// - 白名单外 / src 解析失败 / 无 src → 替换为「▶ 在浏览器打开」外链（无 src 直接丢弃）
///
/// 手写字符串扫描（scraper 树不可变更）；属性值里含 `>` 的极端写法会误判，
/// 但误判方向是多降级一个 iframe，不构成安全问题（ammonia 兜底仍会清洗）。
fn filter_iframes(html: &str) -> String {
    if !html.to_ascii_lowercase().contains("<iframe") {
        return html.to_string();
    }
    let bytes = html.as_bytes();
    let mut out = String::with_capacity(html.len());
    let mut i = 0usize;
    while i < bytes.len() {
        // 找下一个 '<'，原样拷贝到 '<' 前
        let Some(tag_start) = html[i..].find('<') else {
            out.push_str(&html[i..]);
            break;
        };
        let tag_start = i + tag_start;
        out.push_str(&html[i..tag_start]);
        let rest = &html[tag_start..];
        // 判定 iframe 开标签——纯 ASCII 前缀比较，多字节安全：
        // is_char_boundary 保证切片点不会落在 UTF-8 字符中间
        let is_iframe = rest.len() >= 7
            && rest.is_char_boundary(7)
            && rest[..7].eq_ignore_ascii_case("<iframe")
            && !rest[7..8].starts_with(|c: char| c.is_ascii_alphanumeric());
        if !is_iframe {
            // 非 iframe 开标签：拷一个字符继续（避免 '<<' 死循环）
            out.push('<');
            i = tag_start + 1;
            continue;
        }
        // iframe 开标签：截到 '>'（'>' 是 ASCII 单字节，find 返回的位置必是字符边界）
        let Some(gt) = rest.find('>') else {
            out.push_str(rest); // 未闭合的残缺标签，原样交给 ammonia 处理
            break;
        };
        let tag = &rest[..=gt]; // 含 '<' … '>'
        // tag[..7] = "<iframe"（纯 ASCII）；gt 是 '>' 字节位置（边界安全）
        let attrs = tag.get(7..gt).unwrap_or("");
        let src = extract_attr(attrs, "src");
        let keep = src
            .as_deref()
            .and_then(|s| Url::parse(s).ok())
            .and_then(|u| u.host_str().map(|h| h.to_string()))
            .is_some_and(|h| iframe_host_allowed(&h));
        if keep {
            out.push_str(tag);
        } else if let Some(s) = src.as_deref().filter(|s| s.starts_with("http")) {
            out.push_str(&format!(
                "<p><a href=\"{}\">▶ 在浏览器打开嵌入内容</a></p>",
                escape_attr(s)
            ));
        } // 无可用 src → 整个标签丢弃
        i = tag_start + tag.len();
    }
    out
}

/// 从开标签属性段提取 `name="…"` / `name='…'` / `name=裸值` 的值。
/// 纯字符迭代（不做字节切片算术——attrs 可能含任意 UTF-8，
/// 字节位置推进可能落在多字节字符中间导致 panic）。
fn extract_attr(attrs: &str, name: &str) -> Option<String> {
    let pat = format!("{name}=");
    let chars: Vec<char> = attrs.chars().collect();
    let pat_chars: Vec<char> = pat.chars().collect();
    let mut i = 0usize;
    while i + pat_chars.len() <= chars.len() {
        if chars[i..i + pat_chars.len()] == pat_chars[..] {
            // 独立属性名：前一个字符不是 [A-Za-z0-9-_]
            let prev_ok = i == 0
                || !chars[i - 1].is_ascii_alphanumeric()
                    && chars[i - 1] != '-'
                    && chars[i - 1] != '_';
            if prev_ok {
                let mut v = chars[i + pat_chars.len()..].iter().copied();
                return match v.next() {
                    Some(q @ ('"' | '\'')) => {
                        let mut value = String::new();
                        for c in v {
                            if c == q {
                                break;
                            }
                            value.push(c);
                        }
                        Some(value)
                    }
                    Some(c) if !c.is_whitespace() => {
                        // 裸值：到下一个空白符
                        let mut value = String::new();
                        value.push(c);
                        for c in v {
                            if c.is_whitespace() {
                                break;
                            }
                            value.push(c);
                        }
                        Some(value)
                    }
                    _ => None,
                };
            }
        }
        i += 1;
    }
    None
}

fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('"', "&quot;")
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
                    .and_then(|c| c.split_whitespace().next())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_audio_survive_with_attributes() {
        let html = r#"<video src="https://v.example.com/a.mp4" poster="https://v.example.com/p.jpg" controls preload="metadata"></video>
                      <audio src="https://v.example.com/a.mp3" controls loop></audio>
                      <video controls><source src="https://v.example.com/b.webm" type="video/webm"></video>"#;
        let out = sanitize(html, None);
        assert!(out.contains("<video"), "video tag kept: {out}");
        assert!(out.contains("poster"), "poster kept");
        assert!(out.contains("controls"), "controls kept");
        assert!(out.contains("<audio"), "audio kept");
        assert!(out.contains("<source"), "source kept");
    }

    #[test]
    fn javascript_src_media_stripped() {
        let html = r#"<video src="javascript:alert(1)" controls></video><audio src="javascript:alert(2)"></audio>"#;
        let out = sanitize(html, None);
        assert!(!out.contains("javascript:"));
    }

    #[test]
    fn iframe_allowlist_kept_others_demoted_to_link() {
        let html = r#"<p>before</p>
            <iframe src="https://www.youtube.com/embed/abc123" width="560" height="315" allowfullscreen></iframe>
            <iframe src="https://player.bilibili.com/player.html?bvid=BV1xx411c7mD"></iframe>
            <iframe src="https://evil.example.com/embed/x"></iframe>
            <iframe src="https://evil.example.com/clickjacking"></iframe>"#;
        let out = sanitize(html, None);
        assert!(out.contains("youtube.com/embed"), "youtube kept: {out}");
        assert!(out.contains("bilibili.com/player"), "bilibili kept: {out}");
        assert_eq!(out.matches("<iframe").count(), 2, "only allowlisted iframes: {out}");
        assert_eq!(out.matches("在浏览器打开嵌入内容").count(), 2, "demoted to links: {out}");
        // 未放行的 src 不得再以 iframe 形式出现（仅存在于降级外链 href 里）
        assert!(!out.contains("<iframe src=\"https://evil"));
        // 降级链接的 href 指向原地址（用户仍可去浏览器看）
        assert!(out.contains("href=\"https://evil.example.com/clickjacking\""));
    }

    #[test]
    fn iframe_without_src_dropped_entirely() {
        let html = r#"x<iframe width="300"></iframe>y"#;
        let out = sanitize(html, None);
        assert!(!out.contains("<iframe"));
        assert!(!out.contains("在浏览器打开"));
        assert!(out.contains('x') && out.contains('y'));
    }

    #[test]
    fn iframe_relative_src_demoted_not_absolutized() {
        // 相对 src 不在 http 白名单入口（可能被 base 改写指向任意站）→ 降级丢弃
        let html = r#"<iframe src="/embed/local"></iframe>"#;
        let out = sanitize(html, Some("https://example.com/feed"));
        assert!(!out.contains("<iframe"));
    }

    #[test]
    fn script_still_stripped_with_media_enabled() {
        let html = r#"<video src="https://v.example.com/a.mp4"></video><script>alert(1)</script><img src="https://v.example.com/i.jpg" onerror="alert(2)">"#;
        let out = sanitize(html, None);
        assert!(!out.contains("script"));
        assert!(!out.contains("onerror"));
        assert!(out.contains("<video"));
    }

    #[test]
    fn extract_attr_finds_value_in_all_quote_styles() {
        assert_eq!(extract_attr(r#" src="https://a/b.mp4" "#, "src").as_deref(), Some("https://a/b.mp4"));
        assert_eq!(extract_attr(" src='x' ", "src").as_deref(), Some("x"));
        assert_eq!(extract_attr(" src=bare ", "src").as_deref(), Some("bare"));
        assert_eq!(extract_attr(" data-src=\"y\"", "src"), None);
        assert_eq!(extract_attr(" nope", "src"), None);
    }

    /// 回归：含中文的属性段曾因字节切片落在多字节字符中间而 panic
    /// （sanitize.rs:96 end byte index not a char boundary —— add_feed
    /// 抓取含中文标题的 feed 时命令任务 panic 永不返回）。
    #[test]
    fn extract_attr_with_multibyte_attrs_does_not_panic() {
        let attrs = r#" title="媒体测试源" src="http://127.0.0.1:8799/x.mp4" alt="视频说明""#;
        assert_eq!(
            extract_attr(attrs, "src").as_deref(),
            Some("http://127.0.0.1:8799/x.mp4")
        );
        assert_eq!(extract_attr(attrs, "title").as_deref(), Some("媒体测试源"));
        assert_eq!(extract_attr(attrs, "alt").as_deref(), Some("视频说明"));
        // 中文值里再找英文属性（跨多字节推进路径）
        let attrs2 = r#" 描述="说明文字一" src='https://例子/视频.mp4'"#;
        assert_eq!(extract_attr(attrs2, "src").as_deref(), Some("https://例子/视频.mp4"));
        assert_eq!(extract_attr(attrs2, "描述").as_deref(), Some("说明文字一"));
        // 整链：含中文 feed HTML 过 filter_iframes + sanitize 不 panic
        let html = r#"<p>频道名：科技频道</p><iframe src="https://www.youtube.com/embed/x" title="视频：测试"></iframe><iframe src="https://恶意.例子.com/e"></iframe>"#;
        let out = sanitize(html, None);
        assert!(out.contains("youtube.com/embed"));
        // 白名单外 iframe 降级为外链（href 带原地址），不再以 iframe 形式存在
        assert_eq!(out.matches("<iframe").count(), 1);
        assert!(out.contains("在浏览器打开嵌入内容"));
    }
}
