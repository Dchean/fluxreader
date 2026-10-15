//! OPT-013 B2：翻译的长文 HTML 分段器（纯函数，无 IO）。
//!
//! 目标：把长正文切成**可重组**的片段——每段都是自包含的合法 HTML 片段
//! （标签配对、不断开 UTF-8 字符与 HTML 实体），最终产物按段拼接后保持链接、
//! 列表、表格与嵌套结构。
//!
//! 策略（确定性；任何变化必须同步递增 `ai::SEGMENT_POLICY_VERSION`）：
//! - 用既有依赖 `scraper`（html5ever）解析片段 DOM，按**节点序列化字节数**分组；
//!   节点整体放得下就整体入组，放不下就对它递归拆分；
//! - 元素拆分 = 「原样摘出该元素的开始/结束标签（截自自身序列化，属性转义与
//!   顺序不经手写重建） + 子内容按内层预算分组后重新包上这对标签」；因此拆出的
//!   每段标签都是配对的，`<ul>/<table>/<a>` 等结构在段内保持；
//! - 文本节点按字节预算切分，切点必须落在 UTF-8 字符边界且不在 HTML 实体
//!   （`&...;`）内部——保证每段不会出现半截字符或半截实体；
//! - 有界拆分下限（如实声明）：开始+结束标签本身已超过预算的节点（如超长
//!   属性值的 `<img>`）、注释/DOCTYPE 等非元素非文本节点、以及超过
//!   [`MAX_SPLIT_DEPTH`] 的深层嵌套，按原子保留（可能单段超预算，但不破坏
//!   结构、不丢内容）。
//!
//! 序列化口径（R1 复审 P3-3 如实声明）：输出是 html5ever 的**重新序列化**结果。
//! 规范序列化的输入逐字保持；非规范输入先被规范化（单引号属性→双引号、
//! 自闭合 `<br/>`→`<br>`、裸 `&`→`&amp;`、文本按序列化器规则转义等）。发送给
//! 模型的就是该输出——「发送前规范化」为有意行为（保证各段边界对下游可见）；
//! 缓存签名绑定的是**原始正文**（命令层快照），缓存身份不受规范化影响。
//!
//! 与签名的关系：本模块的输出只由 (输入 HTML, 预算, 算法版本) 决定；签名输入
//! 已含 `SEGMENT_POLICY_VERSION` 与整篇正文哈希，因此「同一签名的片段序列」是
//! 稳定可复现的——片段持久化只按签名与序号绑定，无需单独存段内容哈希。

use scraper::{ElementRef, Html, Node};

/// 段落/嵌套深度的防御上限：超过即原子保留（不无限递归）。
const MAX_SPLIT_DEPTH: usize = 64;

/// 把 HTML 切成长度不超过 `budget` 字节（尽量）的连续片段，拼接即为原文档的
/// 等价重组（结构保持；相邻段可直接相接，无需补标签）。
///
/// 空输入返回空 Vec（调用方对空正文与「无内容节点」的退化输入分别守卫）；未超
/// 预算的输入返回一个片段，内容为其 html5ever 序列化（规范序列化输入逐字，
/// 非规范输入先经规范化——见模块头注）。
pub fn split_html_segments(html: &str, budget: usize) -> Vec<String> {
    let budget = budget.max(1);
    let doc = Html::parse_fragment(html);
    let root = content_root(doc.tree.root());
    split_children(root, budget, 0)
}

/// 解析器为片段补出的容器（`html`/`body`）不是输入内容的一部分：单子节点且为
/// 容器时下钻，让分段作用于输入的顶层节点（否则分段结果会被额外包一层）。
fn content_root(root: ego_tree::NodeRef<'_, Node>) -> ego_tree::NodeRef<'_, Node> {
    let mut node = root;
    loop {
        let mut children = node.children();
        let Some(child) = children.next() else {
            return node;
        };
        if children.next().is_some() {
            return node;
        }
        match child.value() {
            Node::Element(el) if el.name() == "html" || el.name() == "body" => node = child,
            _ => return node,
        }
    }
}

/// 拆分一个节点的子序列（返回 ≥1 个「若干子节点连成的组」，每组 ≤ budget）。
fn split_children(node: ego_tree::NodeRef<'_, Node>, budget: usize, depth: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    for child in node.children() {
        let html = node_html(child);
        if html.is_empty() {
            continue;
        }
        if html.len() <= budget {
            push_group(&mut out, &mut current, &html, budget);
            continue;
        }
        // 单个子节点超预算 → 递归拆分（可能返回多个自包含片段）
        for part in split_node(child, budget, depth + 1) {
            push_group(&mut out, &mut current, &part, budget);
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// 节点自身的 HTML 序列化：元素走 html5ever 序列化器（属性转义/顺序保真）；
/// 文本节点按 html5ever 同款转义规则（`&`/`<`/`>`/nbsp）——这样实体边界对
/// 下游（切分点保护、模型、sanitize）都可见。
fn node_html(node: ego_tree::NodeRef<'_, Node>) -> String {
    match node.value() {
        Node::Element(_) => ElementRef::wrap(node).map(|e| e.html()).unwrap_or_default(),
        Node::Text(t) => escape_text(t),
        Node::Comment(c) => format!("<!--{}-->", &**c),
        Node::Doctype(d) => format!("<!DOCTYPE {}>", d.name()),
        // 文档/片段/处理指令：正文片段场景不可达；空串由调用方跳过
        _ => String::new(),
    }
}

/// html5ever 序列化器的文本转义规则（与 ElementRef::html 的输出口径一致）。
fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\u{00A0}' => out.push_str("&nbsp;"),
            _ => out.push(c),
        }
    }
    out
}

/// 追加一片到当前组；装满即落盘开新组。
fn push_group(out: &mut Vec<String>, current: &mut String, part: &str, budget: usize) {
    if !current.is_empty() && current.len() + part.len() > budget {
        out.push(std::mem::take(current));
    }
    current.push_str(part);
}

/// 拆分单个节点：`html` 为该节点的完整序列化（文本节点为转义文本）。
fn split_node(node: ego_tree::NodeRef<'_, Node>, budget: usize, depth: usize) -> Vec<String> {
    let html = node_html(node);
    if html.len() <= budget || depth > MAX_SPLIT_DEPTH {
        return vec![html];
    }
    match node.value() {
        Node::Text(_) => split_escaped_text(&html, budget),
        Node::Element(el) => {
            // 从自身的序列化里摘开始/结束标签（不经手写重建，属性转义/顺序保真）
            let Some((open, close)) = split_outer_tag(&html, el.name()) else {
                return vec![html]; // 无结束标签（void 元素/自闭合）→ 原子保留
            };
            let overhead = open.len() + close.len();
            if overhead >= budget {
                // 标签本身超预算：原子保留（有界拆分下限，见模块头注）
                return vec![html];
            }
            let inner = &html[open.len()..html.len() - close.len()];
            if inner.trim().is_empty() {
                return vec![html]; // 无子内容可拆（如超长属性值的 img）→ 原子保留
            }
            let inner_budget = budget - overhead;
            let parts = split_inner(inner, node, inner_budget, depth + 1);
            if parts.len() <= 1 {
                return vec![html];
            }
            parts
                .into_iter()
                .map(|p| format!("{open}{p}{close}"))
                .collect()
        }
        // 注释/DOCTYPE/处理指令等：原子保留（可能单段超预算）
        _ => vec![html],
    }
}

/// 元素的子内容拆分：优先按已解析的子节点（保留原序列化）；无子节点时按
/// 内层字符串兜底拆分（解析器把内容保序放在子节点上，此路径是防御）。
fn split_inner(
    inner: &str,
    node: ego_tree::NodeRef<'_, Node>,
    inner_budget: usize,
    depth: usize,
) -> Vec<String> {
    let children: Vec<_> = node.children().collect();
    if children.is_empty() {
        return split_escaped_text(inner, inner_budget);
    }
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    for child in children {
        let html = node_html(child);
        if html.is_empty() {
            continue;
        }
        if html.len() <= inner_budget {
            push_group(&mut out, &mut current, &html, inner_budget);
            continue;
        }
        for part in split_node(child, inner_budget, depth + 1) {
            push_group(&mut out, &mut current, &part, inner_budget);
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    if out.is_empty() {
        vec![inner.to_string()]
    } else {
        out
    }
}

/// 从元素自身的序列化里摘出（开始标签, 结束标签）切片。
///
/// 开始标签 = 从 0 到第一个**不在引号内**的 `>`（含）；结束标签 = 与标签名
/// 匹配的末尾 `</name>`。无匹配结束标签（void 元素/非常规序列化）返回 None；
/// 超长属性值里带 `>` 的引号内字符不会误判（引号状态跟踪）。
fn split_outer_tag<'a>(outer: &'a str, tag: &str) -> Option<(&'a str, &'a str)> {
    let bytes = outer.as_bytes();
    if bytes.first() != Some(&b'<') {
        return None;
    }
    let mut quote: Option<u8> = None;
    let mut open_end = None;
    for (i, &b) in bytes.iter().enumerate() {
        match quote {
            Some(q) => {
                if b == q {
                    quote = None;
                }
            }
            None => match b {
                b'"' | b'\'' => quote = Some(b),
                b'>' => {
                    open_end = Some(i + 1);
                    break;
                }
                _ => {}
            },
        }
    }
    let open_end = open_end?;
    let close_tag = format!("</{tag}>");
    if outer.len() < open_end + close_tag.len() || !outer.ends_with(&close_tag) {
        return None;
    }
    Some((&outer[..open_end], &outer[outer.len() - close_tag.len()..]))
}

/// 拆分很长的（已转义的）文本：按字节预算切分，切点落在字符边界且不切断实体。
fn split_escaped_text(text: &str, budget: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut start = 0usize;
    while start < text.len() {
        let remaining = &text[start..];
        if remaining.len() <= budget {
            out.push(remaining.to_string());
            break;
        }
        let cut = safe_cut(remaining, budget);
        out.push(remaining[..cut].to_string());
        start += cut;
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// 安全切点：≤ max 字节、UTF-8 字符边界、不落在 `&...;` 实体内部。
fn safe_cut(s: &str, max: usize) -> usize {
    let mut cut = floor_char_boundary(s, max);
    if cut == 0 {
        // 首个字符就超过预算（异常预算）：至少切一个字符，避免死循环
        cut = ceil_char_boundary(s, max.max(1));
    }
    if let Some((amp, semi)) = entity_containing(s, cut) {
        cut = if amp > 0 { amp } else { semi + 1 };
        cut = floor_char_boundary(s, cut);
        if cut == 0 {
            cut = ceil_char_boundary(s, semi + 1);
        }
    }
    cut
}

/// 若 `cut` 落在某个 `&...;` 实体内部，返回该实体的 (amt, 分号) 字节位置。
fn entity_containing(s: &str, cut: usize) -> Option<(usize, usize)> {
    let bytes = s.as_bytes();
    let window_start = cut.saturating_sub(32);
    let mut i = cut;
    while i > window_start {
        i -= 1;
        if bytes[i] != b'&' {
            continue;
        }
        let mut j = i + 1;
        while j < bytes.len() && j <= i + 32 {
            match bytes[j] {
                b';' => {
                    // 实体在 cut 之前就结束 → 不在内部
                    return (cut <= j).then_some((i, j));
                }
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'#' => j += 1,
                _ => return None, // 普通 &（非实体形态）
            }
        }
        return None;
    }
    None
}

/// 向下取整到字符边界（≤ i）。
fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// 向上取整到字符边界（≥ i；i 超长时取串尾）。
fn ceil_char_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// 朴素标签平衡检查：栈式扫描（跳过注释/void 元素），返回未配对的标签名。
///
/// 测试专用（`#[cfg(test)]`）：ai_segments 单测与 commands 层的「实际发送的
/// 每段都是合法 HTML 片段」断言共用同一口径（不是第二份实现）。
#[cfg(test)]
pub(crate) fn unbalanced_tags(html: &str) -> Vec<String> {
    const VOID: &[&str] = &[
        "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param",
        "source", "track", "wbr",
    ];
    let mut stack: Vec<String> = Vec::new();
    let bytes = html.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        if html[i..].starts_with("<!--") {
            if let Some(end) = html[i + 4..].find("-->") {
                i += 4 + end + 3;
                continue;
            }
            break;
        }
        let Some(rel_end) = html[i..].find('>') else {
            stack.push("<未闭合的标签起始".into());
            break;
        };
        let tag = &html[i..i + rel_end];
        let inner = tag[1..].trim();
        if let Some(name) = inner.strip_prefix('/') {
            let name = name.trim().to_ascii_lowercase();
            match stack.pop() {
                Some(top) if top == name => {}
                other => stack.push(format!("</{name}> 与栈顶 {other:?} 不配对")),
            }
        } else {
            let name = inner
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_end_matches('/')
                .to_ascii_lowercase();
            let self_closed = inner.ends_with('/') || VOID.contains(&name.as_str());
            if !self_closed && !name.is_empty() {
                stack.push(name);
            }
        }
        i += rel_end + 1;
    }
    stack
}

#[cfg(test)]
mod tests {
    use super::*;
    use scraper::Html;

    /// 把 HTML 的可见文本抽出来（与拼接后逐段文本比较，防丢字/截断实体）。
    fn text_of(html: &str) -> String {
        Html::parse_fragment(html)
            .tree
            .root()
            .descendants()
            .filter_map(|n| match n.value() {
                Node::Text(t) => Some(t.text.to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    fn article_paragraphs(count: usize, filler: &str) -> String {
        (0..count)
            .map(|i| format!("<p>段落{i}：{filler}</p>"))
            .collect::<Vec<_>>()
            .join("")
    }

    #[test]
    fn short_input_is_a_single_segment_for_canonical_input() {
        let html = "<p>你好<strong>世界</strong></p>";
        let segs = split_html_segments(html, 8192);
        assert_eq!(
            segs,
            vec![html.to_string()],
            "规范序列化输入未超预算时逐字保持（不重写）"
        );
    }

    /// R1 复审 P3-3：非规范短输入在发送前经 html5ever 规范化——单引号属性→
    /// 双引号、自闭合 `<br/>`→`<br>`、裸 `&`→`&amp;`。这是**有意行为**（发送
    /// 给模型的就是分段器输出）；缓存签名绑定原始正文，缓存身份不受影响。
    #[test]
    fn short_non_canonical_input_is_normalized_before_send() {
        let html = "<p class='x'>a & b<br/>c</p>";
        let segs = split_html_segments(html, 8192);
        assert_eq!(segs.len(), 1);
        assert_eq!(
            segs[0], "<p class=\"x\">a &amp; b<br>c</p>",
            "非规范短输入必须经序列化规范化（且只规范化这一种形态差异）"
        );
    }

    /// R1 复审 P2-1 的退化输入：`body.trim()` 非空但解析后没有任何内容节点
    /// （`<html></html>` 这类容器标签被片段解析器吸收）→ **无段**。命令层据此
    /// 显式 `aiIncomplete`（见 commands::ai 的守卫与 `degenerate_input_...` 用例），
    /// 不得空拼串提交空产物。
    #[test]
    fn input_without_content_nodes_yields_no_segments() {
        assert!(
            split_html_segments("<html></html>", 8192).is_empty(),
            "无内容节点的正文不得产出空段/伪段"
        );
    }

    #[test]
    fn long_input_segments_are_balanced_and_ordered_and_preserve_text() {
        let html = article_paragraphs(30, "内容内容内容内容内容内容内容内容");
        let budget = 200usize;
        let segs = split_html_segments(&html, budget);
        assert!(
            segs.len() > 1,
            "超长输入必须切多段（实际 {} 段）",
            segs.len()
        );
        for (i, seg) in segs.iter().enumerate() {
            assert!(
                unbalanced_tags(seg).is_empty(),
                "第 {i} 段标签必须配对：{}",
                seg
            );
        }
        assert_eq!(
            text_of(&html),
            segs.iter().map(|s| text_of(s)).collect::<String>(),
            "拼接后的可见文本与原文一致（不丢字、不截断实体）"
        );
        // 确定性/顺序稳定：同一输入两次结果逐字相同
        assert_eq!(segs, split_html_segments(&html, budget));
    }

    #[test]
    fn segments_do_not_exceed_budget_except_atomic_fallbacks() {
        let html = article_paragraphs(40, "字");
        let budget = 160usize;
        for seg in split_html_segments(&html, budget) {
            assert!(
                seg.len() <= budget,
                "普通节点拆分后不得超预算：{}",
                seg.len()
            );
        }
    }

    /// 超长单节点（一个巨型 <p>）必须递归拆分：拆成多个仍为 <p> 的段，
    /// 每段标签闭合、文本内容完整。
    #[test]
    fn oversized_single_node_is_split_recursively() {
        let body: String = (0..80).map(|i| format!("第{i}句内容")).collect();
        let html = format!("<div><p>{body}</p><p>尾巴</p></div>");
        let budget = 128usize;
        let segs = split_html_segments(&html, budget);
        assert!(
            segs.len() > 2,
            "超长单节点必须有界拆分（实际 {} 段）",
            segs.len()
        );
        for seg in &segs {
            assert!(
                unbalanced_tags(seg).is_empty(),
                "递归拆分段必须标签配对：{seg}"
            );
        }
        assert_eq!(
            text_of(&html),
            segs.iter().map(|s| text_of(s)).collect::<String>()
        );
        let rejoined = segs.join("");
        assert!(rejoined.contains("<p>"), "保留 <p> 结构：{rejoined}");
    }

    /// 结构与链接/列表/表格保持：拆分后每段内这些元素仍成对出现，最终拼接
    /// 里各元素数量不减。
    #[test]
    fn links_lists_tables_stay_paired_in_every_segment() {
        let rows: String = (0..20)
            .map(|i| format!("<tr><td>单元格{i}</td><td><a href=\"/l{i}\">链接{i}</a></td></tr>"))
            .collect();
        let html = format!(
            "<p><a href=\"https://example.com/x\">开篇链接</a></p><ul><li>甲</li><li>乙</li></ul><table><tbody>{rows}</tbody></table>"
        );
        let segs = split_html_segments(&html, 300);
        assert!(segs.len() > 1);
        for seg in &segs {
            assert!(unbalanced_tags(seg).is_empty(), "结构必须逐段配对：{seg}");
        }
        let all = segs.join("");
        assert_eq!(
            all.matches("<a ").count(),
            html.matches("<a ").count(),
            "链接不丢"
        );
        assert_eq!(
            all.matches("<td>").count(),
            html.matches("<td>").count(),
            "单元格不丢"
        );
        assert!(
            all.contains("<ul><li>甲</li><li>乙</li></ul>"),
            "列表保持成对"
        );
    }

    /// 实体与 UTF-8 不截断：把预算卡在实体中间——每段不得出现半截实体，
    /// 也不能丢多字节字符。
    #[test]
    fn entity_and_utf8_boundaries_are_respected() {
        // 让实体刚好跨越切点：长前缀 + 实体 + 长后缀，预算取在实体中段
        let text = format!("{}{}继续写一点内容", "汉".repeat(20), "&nbsp;");
        let html = format!("<p>{text}</p>");
        let entity_start = format!("<p>{}</p>", "汉".repeat(20)).len() - "</p>".len();
        let budget = entity_start + 3; // 落在 "&nbsp;" 内部（&n 处）
        let segs = split_html_segments(&html, budget);
        assert!(segs.len() > 1, "预算切在实体处也必须分成多段");
        for seg in &segs {
            assert!(
                unbalanced_tags(seg).is_empty(),
                "实体切点不得破坏标签：{seg}"
            );
            // 段内不允许出现「& 开头但没有分号收尾」的半截实体
            let mut rest = seg.as_str();
            while let Some(pos) = rest.find('&') {
                let tail = &rest[pos..];
                assert!(
                    tail.find(';').is_some(),
                    "出现半截实体（段内 & 无分号收尾）：{seg}"
                );
                rest = &tail[1..];
            }
        }
        let joined_text = segs.iter().map(|s| text_of(s)).collect::<String>();
        assert_eq!(text_of(&html), joined_text, "实体与多字节字符不得丢");
    }

    /// 有界下限的如实行为：开始/结束标签本身超预算（超长属性）→ 原子保留，
    /// 不丢内容也不产生半截标签。
    #[test]
    fn oversized_tag_overhead_is_kept_atomic() {
        let long_attr = "x".repeat(400);
        let html = format!("<p><img src=\"https://e.example/{long_attr}\">短文本</p>");
        let segs = split_html_segments(&html, 64);
        let all = segs.join("");
        assert!(all.contains("<img"), "超长属性节点不得被丢弃：{all}");
        for seg in &segs {
            assert!(
                unbalanced_tags(seg).is_empty(),
                "原子保留也必须自平衡：{seg}"
            );
        }
        assert_eq!(
            text_of(&html),
            segs.iter().map(|s| text_of(s)).collect::<String>()
        );
    }

    #[test]
    fn empty_and_whitespace_inputs() {
        assert!(split_html_segments("", 128).is_empty());
        assert_eq!(split_html_segments("   ", 128).len(), 1);
    }

    /// 预算退化（1 字节）不得 panic / 死循环（有界拆分下限兜底）。
    #[test]
    fn degenerate_budget_terminates() {
        let html = "<p>很长很长很长的一个段落</p><p>再来一段</p>";
        let segs = split_html_segments(html, 1);
        assert!(!segs.is_empty());
        assert_eq!(
            text_of(html),
            segs.iter().map(|s| text_of(s)).collect::<String>()
        );
    }
}
