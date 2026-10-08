//! HTML 消毒与文本抽取：所有 feed/网页来源的 HTML 进 reader webview 前必经。
//! 白名单式清洗（ammonia）+ 相对 URL 重写 + 惰性图片恢复 + 富媒体放行。

use ammonia::{Builder, UrlRelative};
use ego_tree::{NodeId, NodeMut, NodeRef, Tree};
use scraper::{Html, Node, Selector};
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
    IFRAME_HOSTS
        .iter()
        .any(|d| h == *d || h.ends_with(&format!(".{d}")))
}

// Note: 可渲染 HTML/译文的净化口径只有这一处（写入口与读边界共用同一 sanitize） — 见 .agents/notes/implemented/architecture/2026-10-08-可渲染正文安全边界.md
/// 消毒 feed HTML：安全渲染 + 相对 URL 以 base 重写为绝对。
/// 放行正文内嵌 `<video>/<audio>/<source>/<track>`，以及域名白名单内的
/// `<iframe>`（YouTube/B 站等嵌入播放器）——白名单外的 iframe 降级为外链。
pub fn sanitize(html: &str, base: Option<&str>) -> String {
    let html = preprocess_html(html);

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
            [
                "src", "poster", "controls", "preload", "width", "height", "muted", "loop",
            ],
        )
        .add_tag_attributes("audio", ["src", "controls", "preload", "loop"])
        .add_tag_attributes("source", ["src", "type", "media"])
        .add_tag_attributes("track", ["src", "kind", "srclang", "label"])
        // iframe 的 src 已在 preprocess_html 按域名预过滤，这里只放行展示属性
        .add_tag_attributes(
            "iframe",
            [
                "src",
                "width",
                "height",
                "allow",
                "allowfullscreen",
                "frameborder",
                "title",
            ],
        );

    if let Some(b) = base.and_then(|b| Url::parse(b).ok()) {
        builder.url_relative(UrlRelative::RewriteWithBase(b));
    }
    builder.clean(&html).to_string()
}

/// 统一 DOM 预处理（R1/R2）：iframe 域名策略与惰性图片恢复在同一次解析里完成，
/// 元素级替换与属性赋值全部走解析树，**绝不把来自属性的字符串 format 成标记**
/// ——变换结束后不可能凭空出现新元素，最终 iframe 集合严格服从白名单域名；
/// ammonia 只做兜底清洗（scheme / 属性白名单 / 文本语义）。
/// 只有确实发生替换/提升时才重序列化，否则原样透传（字节保真、零额外成本）。
///
/// - iframe 策略：解析树读取实际 src（实体解码、引号/空白、大小写归一、重复属性
///   首个胜出，全部按 html5ever 语义）。白名单内保留；白名单外/无 src 降级为
///   「▶ 在浏览器打开」外链（无可用 src 整体移除）。R1 教训：字符串属性扫描会被
///   其他属性引号值里的伪 `src=` 诱骗放行；F20 的字节切片 panic 也随之消失。
/// - 惰性图片：`src` 为空/data: 时从 data-src / data-original / data-lazy-src /
///   srcset 首项取真实 URL，用 DOM 属性赋值写回 `src`。R2 教训：旧实现把解码后的
///   属性值 format 进 `src="…"` 再 replacen——等于在 iframe 策略之后重新向字符串
///   注入标记（data-src 可携带 `"><iframe …>` 制造新 iframe），且对单引号占位等
///   序列化形态不生效。
fn preprocess_html(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    if !lower.contains("<iframe") && !lower.contains("<img") {
        return html.to_string();
    }
    let mut doc = Html::parse_fragment(html);
    let mut changed = false;

    // ① iframe 域名策略：先只读遍历收集决策（值 = Option<降级外链 href>，
    //    None = 整体移除），再逐个原地替换/移除
    let victims: Vec<(NodeId, Option<String>)> = doc
        .tree
        .nodes()
        .filter_map(|n| {
            let el = n.value().as_element()?;
            if &*el.name.local != "iframe" {
                return None;
            }
            let src = el.attr("src");
            let keep = src
                .and_then(|s| Url::parse(s).ok())
                .and_then(|u| u.host_str().map(|h| h.to_string()))
                .is_some_and(|h| iframe_host_allowed(&h));
            if keep {
                return None;
            }
            // 仅 http 起点给出可点击外链；其余（相对/javascript:/空）整体移除
            Some((
                n.id(),
                src.filter(|s| s.starts_with("http")).map(str::to_string),
            ))
        })
        .collect();
    for (id, demote_src) in victims {
        let replacement_id = demote_src
            .as_deref()
            .and_then(demote_link_subtree)
            .map(|subtree| doc.tree.extend_tree(subtree).id());
        let Some(mut node) = doc.tree.get_mut(id) else {
            continue;
        };
        // 解析出的元素必有父（解析语义保证）；防御式检查避免任何 panic 路径
        if node.parent().is_none() {
            continue;
        }
        if let Some(pid) = replacement_id {
            node.insert_id_before(pid);
        }
        node.detach();
        changed = true;
    }

    // ② 惰性图片恢复：只更新既有 src 属性值；URL 经 DOM 赋值 + 序列化转义落地，
    //    不可能变成标记（也就不需要、不允许再做一遍 iframe 检查）
    let recoveries: Vec<(NodeId, String)> = doc
        .tree
        .nodes()
        .filter_map(|n| {
            let el = n.value().as_element()?;
            if &*el.name.local != "img" {
                return None;
            }
            let real_src = el.attr("src").is_some_and(|s| {
                let s = s.trim();
                !s.is_empty() && !s.starts_with("data:")
            });
            if real_src {
                return None;
            }
            let recovered = ["data-src", "data-original", "data-lazy-src"]
                .iter()
                .find_map(|k| el.attr(k))
                .or_else(|| {
                    el.attr("srcset")
                        .and_then(|ss| ss.split(',').next())
                        .and_then(|c| c.split_whitespace().next())
                })
                .filter(|u| !u.is_empty())?;
            Some((n.id(), recovered.to_string()))
        })
        .collect();
    for (id, url) in recoveries {
        let Some(mut node) = doc.tree.get_mut(id) else {
            continue;
        };
        let Node::Element(el) = node.value() else {
            continue;
        };
        // 只更新已存在的 src（旧实现对无 src 属性是 no-op；新写法同样不引入新属性）
        if let Some((_, v)) = el.attrs.iter_mut().find(|(k, _)| &*k.local == "src") {
            *v = url.into();
            changed = true;
        }
    }

    if !changed {
        return html.to_string(); // 无改动时保持原文，不做无谓规范化
    }
    doc.html()
}

/// 降级外链替换模板：href 由 DOM 属性赋值写入（模板本身不含任何插值）。
const DEMOTE_LINK_TEMPLATE: &str = r#"<p><a href="">▶ 在浏览器打开嵌入内容</a></p>"#;

/// 构造降级外链子树：解析常量模板 → 按标签名取出 `<p>` 子树克隆为独立树
/// （根即 `<p>`，不含解析器可能添加的包裹层）→ DOM 赋 href。
/// 返回树的根 id 在 `extend_tree` 后就是可插入的 `<p>` 节点 id。
fn demote_link_subtree(src: &str) -> Option<Tree<Node>> {
    let mut template = Html::parse_fragment(DEMOTE_LINK_TEMPLATE);
    let p_id = template
        .tree
        .nodes()
        .find(|n| {
            n.value()
                .as_element()
                .is_some_and(|e| &*e.name.local == "p")
        })?
        .id();
    let a_id = template
        .tree
        .nodes()
        .find(|n| {
            n.value()
                .as_element()
                .is_some_and(|e| &*e.name.local == "a")
        })?
        .id();
    {
        let mut a = template.tree.get_mut(a_id)?;
        let Node::Element(el) = a.value() else {
            return None;
        };
        let (_, v) = el.attrs.iter_mut().find(|(k, _)| &*k.local == "href")?;
        *v = src.into();
    }
    let p_ref = template.tree.get(p_id)?;
    let mut out = Tree::new(p_ref.value().clone());
    clone_children(p_ref, &mut out.root_mut());
    Some(out)
}

/// 递归克隆子树的孩子（值克隆；不依赖解析器的文档包裹层结构）。
fn clone_children(src: NodeRef<'_, Node>, dst: &mut NodeMut<'_, Node>) {
    let mut child = src.first_child();
    while let Some(c) = child {
        let mut n = dst.append(c.value().clone());
        clone_children(c, &mut n);
        child = c.next_sibling();
    }
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
        assert_eq!(
            out.matches("<iframe").count(),
            2,
            "only allowlisted iframes: {out}"
        );
        assert_eq!(
            out.matches("在浏览器打开嵌入内容").count(),
            2,
            "demoted to links: {out}"
        );
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

    /// R1：iframe 属性一律按 HTML 解析器语义读取（旧 `extract_attr` 字符串扫描
    /// 已删除）。覆盖：引号风格、无引号值、`data-src` 不冒充 `src`、
    /// 重复属性（解析器取首个）、其他属性值中的伪 `src=` 诱饵。
    #[test]
    fn iframe_attributes_follow_parser_semantics() {
        // 无引号值
        let bare = sanitize(
            r#"<iframe src=https://www.youtube.com/embed/bare></iframe>"#,
            None,
        );
        assert_eq!(
            bare.matches("<iframe").count(),
            1,
            "无引号合法 src 必须保留: {bare}"
        );
        // data-src 不冒充 src → 无 src 整体移除
        let data_only = sanitize(
            r#"<iframe data-src="https://www.youtube.com/embed/a"></iframe>"#,
            None,
        );
        assert!(
            !data_only.contains("<iframe"),
            "data-src 不得冒充 src: {data_only}"
        );
        // 重复 src：解析器取首个（与浏览器一致）
        let dup_first_ok = sanitize(
            r#"<iframe src="https://www.youtube.com/embed/a" src="https://evil.invalid/x"></iframe>"#,
            None,
        );
        assert_eq!(
            dup_first_ok.matches("<iframe").count(),
            1,
            "首个 src 在白名单 → 保留: {dup_first_ok}"
        );
        let dup_first_evil = sanitize(
            r#"<iframe src="https://evil.invalid/x" src="https://www.youtube.com/embed/a"></iframe>"#,
            None,
        );
        assert!(
            !dup_first_evil.contains("<iframe"),
            "首个 src 在白名单外 → 降级/移除: {dup_first_evil}"
        );
        // 其他属性引号值中的伪 src= 诱饵不得影响真实判定
        let decoy = sanitize(
            r#"<iframe title="SRC = https://www.youtube.com/embed/a " src="https://evil.invalid/x"></iframe>"#,
            None,
        );
        assert!(
            !decoy.contains("<iframe"),
            "诱饵不得放行真实白名单外 src: {decoy}"
        );
    }

    /// 回归：含中文的属性值 / 中文正文 / 残缺与多字节 iframe 不 panic
    /// （旧 `extract_attr` 曾因字节切片落在多字节字符中间 panic；
    /// F20 的 `rest[7..8]` 越界已随字符串扫描移除，由解析器吸收）。
    #[test]
    fn sanitize_handles_multibyte_and_truncated_iframes_without_panic() {
        // 整链：含中文 feed HTML —— 合法嵌入保留、白名单外降级、不 panic
        let html = r#"<p>频道名：科技频道</p><iframe src="https://www.youtube.com/embed/x" title="视频：测试"></iframe><iframe src="https://恶意.例子.com/e"></iframe>"#;
        let out = sanitize(html, None);
        assert!(out.contains("youtube.com/embed"), "{out}");
        assert_eq!(out.matches("<iframe").count(), 1, "{out}");
        assert!(
            out.contains("在浏览器打开嵌入内容"),
            "白名单外降级为外链: {out}"
        );
        // 残缺/多字节非 iframe 形态：不 panic、不放行
        for input in ["<p>正文</p><iframe", "<iframe中>正文", "<<iframe><p>x</p>"] {
            let out = sanitize(input, None);
            assert!(!out.contains("<iframe"), "{input} → {out}");
        }
    }
}
