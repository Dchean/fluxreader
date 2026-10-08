//! OPT-002 正文安全边界与解析健壮性回归（审计 F07 / F20）：
//! ① 直接调用生产 sanitize()（不复制实现）锁定：残缺/多字节 iframe 不 panic、
//!    合法富媒体（白名单 iframe、大小写/属性空白写法）保留、白名单外降级外链；
//! ② 经真实 db API 回归：回填自身净化 + 相对 URL 按文章 url 重写 + 非空旧正文
//!    不被覆盖 + 回填错误正常上抛；
//! ③ 读边界（get_article / get_articles / with_content=true 列表）对存量污染行
//!    统一净化；只元数据的列表不携带正文列。
//!
//! 完整 Rust 测试由 Windows CI 执行（本机无 MSVC 链接器，见 AGENTS.md 门禁）；
//! 夹具全部是新建临时库，不触真实用户数据。

mod common;

use app_lib::db::{self, ArticleQuery, NewArticle};
use app_lib::sanitize::sanitize;

/// F07 反例原文（audit-2026-10-08）：任意 iframe + 事件属性 + script + 相对路径图 + 合法文本。
const POLLUTED: &str = r#"<iframe src="https://evil.invalid/embed"></iframe><img src="/x.png" onerror="x()"><script>alert(1)</script><p>保留</p>"#;

/// 译文列的污染形态（存量 AI 产物）：任意 iframe + 合法文本。
const POLLUTED_TRANSLATION: &str = r#"<iframe src="https://evil.invalid/t"></iframe><em>译文</em>"#;

fn open_db(base: &str) -> rusqlite::Connection {
    let tmp = common::unique_db_path(base);
    let _ = std::fs::remove_file(&tmp);
    db::open(&tmp).expect("open temp db")
}

/// 建一个源 + 一篇文章；content_html 以调用方给定形态直插
/// （模拟存量行/外部写入，绕过 sanitizer 的形态）。
fn seed_article(
    conn: &rusqlite::Connection,
    guid: &str,
    url: Option<&str>,
    content_html: Option<&str>,
) -> i64 {
    let folder = db::create_folder(conn, "F", "article").unwrap();
    let feed = db::insert_feed(
        conn,
        "https://feed.example/rss",
        None,
        "源",
        None,
        folder,
        "inherit",
        true,
        false,
    )
    .unwrap();
    let a = NewArticle {
        guid: guid.into(),
        url: url.map(str::to_string),
        title: "标题".into(),
        author: None,
        summary: None,
        content_html: content_html.map(str::to_string),
        body_text: "正文".into(),
        image_url: None,
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: Some("2026-01-01T00:00:00+00:00".into()),
        source: "direct".into(),
    };
    let (aid, _) = db::upsert_article_with_feed(conn, feed, &a, false).unwrap();
    aid
}

/// 直读库内原始 content_html（不经读边界净化，用于区分「写入已净化」与「读出才净化」）。
fn raw_content(conn: &rusqlite::Connection, aid: i64) -> String {
    conn.query_row(
        "SELECT COALESCE(content_html, '') FROM articles WHERE id = ?1",
        [aid],
        |r| r.get(0),
    )
    .unwrap()
}

fn list_q(with_content: bool) -> ArticleQuery {
    ArticleQuery {
        feed_id: None,
        folder_id: None,
        only_unread: false,
        only_starred: false,
        only_today: false,
        newest_first: true,
        limit: 10,
        offset: 0,
        with_content,
        layout: None,
        last_published: None,
        last_id: None,
    }
}

/* ============================================================
F20：残缺 / 多字节 iframe 不 panic
============================================================ */

/// F20 本体：`<iframe` 截断于 EOF、`<iframe` 后紧跟多字节字符、连续 `<<`
/// 都不得 panic（修前 `rest[7..8]` 越界，rustc 独立复现 exit 101），
/// 且输出里不得留下未清洗的 iframe 标签。
#[test]
fn sanitize_truncated_and_multibyte_iframe_does_not_panic() {
    for input in [
        "<p>正文</p><iframe",     // EOF 截断：修前 rest[7..8] 越界
        "<p>正文</p><iframe ",    // 截断 + 尾随空白
        "<iframe中>正文",         // 后继多字节字符：修前 8 非字符边界
        "<iframe 标题=\"测试\">", // 多字节属性值、无 src → 丢弃
        "<<iframe><p>x</p>",      // 连续 '<'（防死循环路径）
    ] {
        let out = sanitize(input, Some("https://real.example/post"));
        assert!(
            !out.contains("<iframe"),
            "输入 {input:?} 的输出不得含未清洗 iframe: {out}"
        );
    }
}

/// 合法白名单 iframe 不因中文属性丢失（顺带锁定多字节属性段不 panic 的修复不回退）。
#[test]
fn sanitize_keeps_allowlisted_iframe_with_multibyte_attrs() {
    let html = r#"<iframe 标题="视频：测试" src="https://www.youtube.com/embed/abc" allowfullscreen></iframe>"#;
    let out = sanitize(html, None);
    assert_eq!(
        out.matches("<iframe").count(),
        1,
        "合法 youtube iframe 不得因中文属性丢失: {out}"
    );
}

/* ============================================================
sanitize()：合法内容、大小写/空白变体、白名单外降级、二次清洗稳定
============================================================ */

/// 普通中文正文原样保留（不是移除所有 HTML）。
#[test]
fn sanitize_keeps_plain_chinese_content() {
    let out = sanitize("<p>你好，世界</p><p>第二段：保留</p>", None);
    assert!(out.contains("你好，世界"), "中文正文必须保留: {out}");
    assert!(out.contains("第二段：保留"), "第二段必须保留: {out}");
}

/// 大小写/属性空白等写法变体：合法嵌入保留、白名单外降级——安全判定不因写法差异失效。
#[test]
fn sanitize_handles_case_and_attribute_whitespace_variants() {
    // HTML 标签/属性名大小写不敏感：大写 SRC 的合法嵌入必须保留
    let upper = sanitize(
        r#"<IFRAME SRC="https://www.youtube.com/embed/a"></IFRAME>"#,
        None,
    );
    assert_eq!(
        upper.matches("<iframe").count(),
        1,
        "大写 SRC 的合法 iframe 必须保留: {upper}"
    );
    // `=` 两侧空白合法：不得因空白导致 src 解析失败而丢标签
    let spaced = sanitize(
        r#"<iframe  src = "https://player.bilibili.com/player.html?bvid=BV1xx"  ></iframe>"#,
        None,
    );
    assert_eq!(
        spaced.matches("<iframe").count(),
        1,
        "等号旁空白不得导致合法 iframe 丢失: {spaced}"
    );
    // 白名单外（含大写写法）必须降级为外链，不得以 iframe 形式出现
    let evil = sanitize(
        r#"<IFRAME SRC="https://evil.invalid/embed"></IFRAME>"#,
        None,
    );
    assert!(
        !evil.contains("<iframe"),
        "白名单外 iframe 不得以 iframe 形式出现: {evil}"
    );
    assert!(
        evil.contains("在浏览器打开嵌入内容"),
        "白名单外 iframe 必须降级为可点击外链: {evil}"
    );
}

/// R1 P1 反例（reviewer Darwin）：其他属性引号值中的伪 `src=` 不得放行真实
/// 白名单外 iframe——属性必须按 HTML 解析器语义读取，而不是字符串扫描。
#[test]
fn iframe_decoy_src_in_other_attributes_does_not_bypass_allowlist() {
    let cases = [
        // 反例原文：双引号 title 内带空白伪 SRC，真实 src 是 evil
        r#"<iframe title="SRC = https://www.youtube.com/embed/a " src="https://evil.invalid/embed"></iframe>"#,
        // 无空白紧凑诱饵（旧版本同样会被欺骗）
        r#"<iframe title="src=https://www.youtube.com/embed/a" src="https://evil.invalid/embed"></iframe>"#,
        // 单引号属性值里再嵌双引号 URL
        r#"<iframe title='src = "https://youtube.com/embed/a"' src="https://evil.invalid/embed"></iframe>"#,
        // 诱饵在 data-* 属性里 + 真实 src 大小写混合
        r#"<iframe data-note="src = https://player.bilibili.com/player.html" SRC="https://evil.invalid/embed"></iframe>"#,
        // 诱饵在后、真实 src 在前
        r#"<iframe src="https://evil.invalid/embed" title="src = https://www.youtube.com/embed/a"></iframe>"#,
    ];
    for input in cases {
        let out = sanitize(input, None);
        assert!(
            !out.contains("<iframe"),
            "诱饵不得放行真实白名单外 src: {input} → {out}"
        );
        let again = sanitize(&out, None);
        assert!(!again.contains("<iframe"), "二次清洗不得复活: {again}");
    }
}

/// 容器语义：template 内容也在解析树内（iframe 不得漏网）；注释里的伪 iframe
/// 不是元素，由 ammonia 按注释剥离——两条路径都不允许危险 iframe 抵达渲染。
#[test]
fn iframe_inside_template_or_comment_follows_tree_semantics() {
    let tpl = sanitize(
        r#"<template><iframe src="https://evil.invalid/embed"></iframe></template><p>x</p>"#,
        None,
    );
    assert!(
        !tpl.contains("<iframe"),
        "template 内容里的 iframe 必须同样被处理: {tpl}"
    );
    let comment = sanitize(
        r#"<!-- <iframe src="https://evil.invalid/embed"></iframe> --><p>正文</p>"#,
        None,
    );
    assert!(
        !comment.contains("<iframe") && comment.contains("正文"),
        "注释里的伪 iframe 不得成为元素: {comment}"
    );
}

/// R2 反例原文：`data-src` 里实体编码的 iframe 标记——旧 `promote_lazy_images`
/// 把解码后的属性值裸 format 进 `src="…"` 拼回标记，绕过已完成的 iframe 策略。
const LAZY_IFRAME_PAYLOAD: &str = r#"<iframe></iframe><img src='' data-src='&quot;&gt;&lt;iframe src=&quot;https://evil.invalid/embed&quot;&gt;&lt;/iframe&gt;'>"#;

/// R2 组合边界：一次 sanitize 与二次清洗都不得借惰性图片恢复制造 iframe。
#[test]
fn lazy_image_promotion_cannot_reintroduce_iframe_markup() {
    let out = sanitize(LAZY_IFRAME_PAYLOAD, None);
    assert!(!out.contains("<iframe"), "组合变换不得制造 iframe: {out}");
    let again = sanitize(&out, None);
    assert!(!again.contains("<iframe"), "二次清洗不得复活: {again}");
}

/// 正常懒加载不回归：data-src/data-original/data-lazy-src/srcset 提升到 src，
/// 相对地址仍按 base 重写；已有真实 src 的图片不被触碰。
#[test]
fn lazy_image_promotion_keeps_normal_behavior() {
    let out = sanitize(r#"<img src="" data-src="https://img.example/a.jpg">"#, None);
    assert!(out.contains("https://img.example/a.jpg"), "{out}");

    let original = sanitize(
        r#"<img src='' data-original="https://img.example/b.jpg">"#,
        None,
    );
    assert!(original.contains("https://img.example/b.jpg"), "{original}");

    let lazy = sanitize(
        r#"<img src="" data-lazy-src="https://img.example/c.jpg">"#,
        None,
    );
    assert!(lazy.contains("https://img.example/c.jpg"), "{lazy}");

    // srcset 兜底 + 相对地址按 base 重写
    let srcset = sanitize(
        r#"<img src="" srcset="/img/d.jpg 1x, /img/d2.jpg 2x">"#,
        Some("https://site.example/post/1"),
    );
    assert!(
        srcset.contains("https://site.example/img/d.jpg"),
        "{srcset}"
    );

    // 已有真实 src 的图片不被 data-src 覆盖；第二张图提升互不串位
    let real = sanitize(
        r#"<img src="https://img.example/real.jpg" data-src="https://img.example/other.jpg"><img src="" data-src="https://img.example/two.jpg">"#,
        None,
    );
    assert!(
        real.contains("src=\"https://img.example/real.jpg\""),
        "{real}"
    );
    assert!(
        real.contains("src=\"https://img.example/two.jpg\""),
        "第二张图必须提升到自己的 src: {real}"
    );
    assert!(
        !real.contains("other.jpg"),
        "已就位的 data-src 不参与提升（会随 data-* 属性被 ammonia 剥离）: {real}"
    );
}

/// 重复属性与实体编码按解析器语义判定：重复 src 取首个；实体解码后再判 host。
#[test]
fn iframe_duplicate_and_entity_encoded_src_follow_parser_semantics() {
    // 重复 src：解析器（html5ever，与浏览器一致）取第一个
    let first_allowed = sanitize(
        r#"<iframe src="https://www.youtube.com/embed/a" src="https://evil.invalid/embed"></iframe>"#,
        None,
    );
    assert_eq!(
        first_allowed.matches("<iframe").count(),
        1,
        "首个 src 在白名单 → 保留: {first_allowed}"
    );
    let first_evil = sanitize(
        r#"<iframe src="https://evil.invalid/embed" src="https://www.youtube.com/embed/a"></iframe>"#,
        None,
    );
    assert!(
        !first_evil.contains("<iframe"),
        "首个 src 在白名单外 → 降级/移除: {first_evil}"
    );

    // 实体解码后才判 host：&#x2F; = '/'
    let encoded_ok = sanitize(
        r#"<iframe src="https://www.youtube.com&#x2F;embed&#x2F;a"></iframe>"#,
        None,
    );
    assert_eq!(
        encoded_ok.matches("<iframe").count(),
        1,
        "实体编码的合法 host 必须保留: {encoded_ok}"
    );
    let encoded_evil = sanitize(
        r#"<iframe src="https://evil&#x2E;invalid/embed"></iframe>"#,
        None,
    );
    assert!(
        !encoded_evil.contains("<iframe"),
        "实体编码不得绕过白名单判定: {encoded_evil}"
    );
}

#[test]
fn sanitize_second_pass_preserves_clean_output() {
    let first = sanitize(POLLUTED, Some("https://real.example/posts/1"));
    let second = sanitize(&first, Some("https://real.example/posts/1"));
    assert!(
        !second.contains("<iframe") && !second.contains("onerror") && !second.contains("<script"),
        "二次清洗不得复活危险节点: {second}"
    );
    assert!(
        second.contains("保留") && second.contains("https://real.example/x.png"),
        "二次清洗不得破坏合法内容: {second}"
    );
    assert!(
        !second.contains("保留保留"),
        "二次清洗不得复制文本: {second}"
    );
}

/* ============================================================
F07：回填写入即净化
============================================================ */

/// F07 本体（卡片反例断言）：空正文补入的原始 HTML 必须经生产 sanitize() 清洗，
/// 相对 URL 以该文章 url 为基址重写。双重断言：
/// ① 库内原始值已安全（写入即净化，不是只靠读边界）；② 读出口同样安全。
#[test]
fn backfill_sanitizes_polluted_html_and_rewrites_relative_url() {
    let conn = open_db("opt002_backfill");
    let aid = seed_article(
        &conn,
        "g-backfill",
        Some("https://real.example/posts/1"),
        None,
    );

    db::backfill_article_content(&conn, aid, POLLUTED, "保留", None, None, None).unwrap();

    let raw = raw_content(&conn, aid);
    assert!(!raw.contains("<iframe"), "回填写入前就必须净化: {raw}");
    assert!(!raw.contains("onerror"), "事件属性必须被剥离: {raw}");
    assert!(!raw.contains("<script"), "script 必须被剥离: {raw}");
    assert!(
        raw.contains("https://real.example/x.png"),
        "相对 URL 必须按 article.url 重写: {raw}"
    );
    assert!(raw.contains("保留"), "合法文本必须保留: {raw}");

    let row = db::get_article(&conn, aid).unwrap().unwrap();
    let html = row.content_html.unwrap_or_default();
    assert!(!html.contains("<iframe") && !html.contains("onerror") && !html.contains("<script"));
    assert!(html.contains("https://real.example/x.png") && html.contains("保留"));
}

/// 合法富媒体不被「一律清空」误伤：白名单 iframe 保留、video 相对 src 按基址重写。
#[test]
fn backfill_keeps_allowlisted_media_and_absolutizes_relative_video() {
    let conn = open_db("opt002_backfill_media");
    let aid = seed_article(
        &conn,
        "g-backfill-media",
        Some("https://real.example/posts/1"),
        None,
    );

    let input = r#"<p>播客</p><iframe src="https://www.youtube.com/embed/abc"></iframe><video src="/v/a.mp4" controls></video>"#;
    db::backfill_article_content(&conn, aid, input, "播客", None, None, None).unwrap();

    let raw = raw_content(&conn, aid);
    assert!(
        raw.contains("<iframe") && raw.contains("youtube.com/embed/abc"),
        "白名单 iframe 必须保留: {raw}"
    );
    assert!(
        raw.contains("https://real.example/v/a.mp4"),
        "video 相对 src 必须按基址重写: {raw}"
    );
}

/// 非空旧正文不被回填覆盖（COALESCE/CASE 语义不回退）；封面空位仍按 COALESCE 补。
#[test]
fn backfill_does_not_overwrite_nonempty_content() {
    let conn = open_db("opt002_no_overwrite");
    let aid = seed_article(
        &conn,
        "g-keep",
        Some("https://real.example/posts/2"),
        Some("<p>旧正文</p>"),
    );

    db::backfill_article_content(
        &conn,
        aid,
        POLLUTED,
        "新正文",
        Some("https://real.example/new.jpg"),
        None,
        None,
    )
    .unwrap();

    assert_eq!(
        raw_content(&conn, aid),
        "<p>旧正文</p>",
        "非空旧正文不得被回填覆盖"
    );
    let img: Option<String> = conn
        .query_row("SELECT image_url FROM articles WHERE id = ?1", [aid], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        img.as_deref(),
        Some("https://real.example/new.jpg"),
        "空封面仍按 COALESCE 补上"
    );
}

/// 回填错误正常上抛（查询/写入失败不得被吞成成功后静默）。
#[test]
fn backfill_error_propagates() {
    let conn = open_db("opt002_err");
    let aid = seed_article(&conn, "g-err", Some("https://real.example/posts/3"), None);
    conn.execute_batch(
        "CREATE TRIGGER inject_backfill_fail BEFORE UPDATE ON articles
         BEGIN SELECT RAISE(ABORT, 'injected: backfill write failed'); END",
    )
    .unwrap();

    let err =
        db::backfill_article_content(&conn, aid, "<p>x</p>", "x", None, None, None).unwrap_err();
    assert!(
        err.to_string().contains("injected: backfill write failed"),
        "错误必须来自注入的写入失败并上抛: {err}"
    );
}

/* ============================================================
R1：诱饵 iframe 的全链路（回填写入 + 详情/批量/列表读取）与重复读取稳定
============================================================ */

/// R1 全链路回归：与 review 同形的诱饵 iframe 经回填写入与各读出口都不得放行，
/// 二次读取保持稳定（写/读边界都用同一解析器语义的 sanitize）。
#[test]
fn decoy_iframe_is_neutralized_through_backfill_and_read_boundary() {
    const DECOY: &str = r#"<p>保留</p><iframe title="SRC = https://www.youtube.com/embed/a " src="https://evil.invalid/embed"></iframe>"#;
    let conn = open_db("opt002_decoy_chain");
    let aid = seed_article(
        &conn,
        "g-decoy",
        Some("https://real.example/posts/11"),
        None,
    );
    db::backfill_article_content(&conn, aid, DECOY, "保留", None, None, None).unwrap();

    // ① 写入即净化
    let raw = raw_content(&conn, aid);
    assert!(!raw.contains("<iframe"), "回填写入即净化: {raw}");
    assert!(raw.contains("保留"), "合法文本保留: {raw}");

    // ② 读出口（详情/批量/with_content 列表）一致净化；重复读取稳定
    let first = db::get_article(&conn, aid)
        .unwrap()
        .unwrap()
        .content_html
        .unwrap_or_default();
    let second = db::get_article(&conn, aid)
        .unwrap()
        .unwrap()
        .content_html
        .unwrap_or_default();
    assert_eq!(first, second, "重复读取口径必须稳定");
    assert!(
        !first.contains("<iframe"),
        "详情不得放行诱饵 iframe: {first}"
    );
    let batch = db::get_articles(&conn, &[aid]).unwrap();
    assert_eq!(
        batch[0].content_html.clone().unwrap_or_default(),
        first,
        "批量详情口径一致"
    );
    let list = db::list_articles(&conn, &list_q(true)).unwrap();
    assert_eq!(
        list[0].content_html.clone().unwrap_or_default(),
        first,
        "with_content 列表口径一致"
    );
}

/* ============================================================
R2：组合变换边界（惰性图片恢复不得制造 iframe）——写入侧与存量读侧
============================================================ */

/// R2（写入侧）：组合 payload 经回填写入后，库内原始值与详情都不得含 iframe。
#[test]
fn composed_lazy_payload_stays_safe_through_backfill_storage() {
    let conn = open_db("opt002_lazy_combo_write");
    let aid = seed_article(
        &conn,
        "g-lazy-write",
        Some("https://real.example/posts/12"),
        None,
    );
    db::backfill_article_content(&conn, aid, LAZY_IFRAME_PAYLOAD, "正文", None, None, None)
        .unwrap();

    let raw = raw_content(&conn, aid);
    assert!(!raw.contains("<iframe"), "回填写入即净化: {raw}");
    let html = db::get_article(&conn, aid)
        .unwrap()
        .unwrap()
        .content_html
        .unwrap_or_default();
    assert!(!html.contains("<iframe"), "详情不得含 iframe: {html}");
}

/// R2（存量读侧）：旧库直插的组合 payload，详情/批量/with_content 列表读回
/// （正文与译文）都不得含 iframe。
#[test]
fn legacy_composed_lazy_payload_is_sanitized_on_read_boundary() {
    let conn = open_db("opt002_lazy_combo_read");
    let aid = seed_article(
        &conn,
        "g-lazy-legacy",
        Some("https://real.example/posts/13"),
        None,
    );
    conn.execute(
        "UPDATE articles SET content_html = ?1, translated_content = ?2 WHERE id = ?3",
        rusqlite::params![LAZY_IFRAME_PAYLOAD, LAZY_IFRAME_PAYLOAD, aid],
    )
    .unwrap();

    let row = db::get_article(&conn, aid).unwrap().unwrap();
    let html = row.content_html.unwrap_or_default();
    assert!(!html.contains("<iframe"), "详情读回不得含 iframe: {html}");
    let tr = row.translated_content.unwrap_or_default();
    assert!(!tr.contains("<iframe"), "译文读回不得含 iframe: {tr}");

    let batch = db::get_articles(&conn, &[aid]).unwrap();
    assert!(!batch[0]
        .content_html
        .clone()
        .unwrap_or_default()
        .contains("<iframe"));
    let list = db::list_articles(&conn, &list_q(true)).unwrap();
    assert!(!list[0]
        .content_html
        .clone()
        .unwrap_or_default()
        .contains("<iframe"));
}

/* ============================================================
读边界：存量污染行（正文 + 译文）统一净化
============================================================ */

/// 存量污染行：直接 UPDATE 原始 HTML（模拟历史回填绕过/历史 AI 产物），
/// get_article / get_articles 返回前必须净化可渲染字段；合法内容与
/// 相对 URL 重写（基址 = article.url）保持。
#[test]
fn get_article_and_get_articles_sanitize_legacy_polluted_fields() {
    let conn = open_db("opt002_legacy_read");
    let aid = seed_article(
        &conn,
        "g-legacy",
        Some("https://real.example/posts/9"),
        None,
    );
    conn.execute(
        "UPDATE articles SET content_html = ?1, translated_content = ?2 WHERE id = ?3",
        rusqlite::params![POLLUTED, POLLUTED_TRANSLATION, aid],
    )
    .unwrap();

    let row = db::get_article(&conn, aid).unwrap().unwrap();
    let html = row.content_html.unwrap_or_default();
    assert!(
        !html.contains("<iframe") && !html.contains("onerror") && !html.contains("<script"),
        "详情正文必须净化: {html}"
    );
    assert!(
        html.contains("https://real.example/x.png") && html.contains("保留"),
        "合法内容与相对 URL 重写必须保持: {html}"
    );
    let tr = row.translated_content.unwrap_or_default();
    assert!(
        !tr.contains("<iframe") && tr.contains("译文"),
        "详情译文必须净化且保留合法文本: {tr}"
    );

    let rows = db::get_articles(&conn, &[aid]).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].content_html.clone().unwrap_or_default(),
        html,
        "get_articles 与 get_article 的净化口径必须逐字一致"
    );
    assert_eq!(
        rows[0].translated_content.clone().unwrap_or_default(),
        tr,
        "批量详情译文口径一致"
    );
}

/// 列表边界：with_content=true 的列表行与详情同口径净化；
/// 只元数据的列表（with_content=false）不携带正文列，不执行正文清洗。
#[test]
fn list_with_content_sanitizes_renderable_fields_meta_only_does_not() {
    let conn = open_db("opt002_list");
    let aid = seed_article(&conn, "g-list", Some("https://real.example/posts/10"), None);
    conn.execute(
        "UPDATE articles SET content_html = ?1, translated_content = ?2 WHERE id = ?3",
        rusqlite::params![POLLUTED, POLLUTED_TRANSLATION, aid],
    )
    .unwrap();

    let with = db::list_articles(&conn, &list_q(true)).unwrap();
    assert_eq!(with.len(), 1);
    let html = with[0].content_html.clone().unwrap_or_default();
    assert!(
        !html.contains("<iframe") && !html.contains("onerror") && !html.contains("<script"),
        "with_content 列表正文必须净化: {html}"
    );
    assert!(
        html.contains("https://real.example/x.png") && html.contains("保留"),
        "合法内容与相对 URL 重写必须保持: {html}"
    );
    let tr = with[0].translated_content.clone().unwrap_or_default();
    assert!(
        !tr.contains("<iframe") && tr.contains("译文"),
        "with_content 列表译文必须净化: {tr}"
    );

    let meta = db::list_articles(&conn, &list_q(false)).unwrap();
    assert_eq!(meta.len(), 1);
    assert!(
        meta[0].content_html.is_none(),
        "只元数据的列表不携带正文（也不产生清洗成本）"
    );
    assert!(
        meta[0].translated_content.is_none(),
        "只元数据的列表不携带译文"
    );
}
