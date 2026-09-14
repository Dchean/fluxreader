use super::*;

fn new_article(url: &str, guid: &str) -> NewArticle {
    NewArticle {
        guid: guid.into(),
        url: Some(url.into()),
        title: "t".into(),
        author: None,
        summary: None,
        content_html: None,
        body_text: "b".into(),
        image_url: None,
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: None,
        source: "direct".into(),
    }
}

/// 智能去重：同 URL 跨源只留首个；不同 URL 互不影响；同源 guid 冲突仍走更新。
#[test]
fn smart_dedup_blocks_cross_feed_same_url() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    let f1 = create_folder(&conn, "A", "article").unwrap();
    let f2 = create_folder(&conn, "B", "article").unwrap();
    let feed1 = insert_feed(
        &conn,
        "https://x.example/f1",
        None,
        "f1",
        None,
        f1,
        "inherit",
        true,
        false,
    )
    .unwrap();
    let feed2 = insert_feed(
        &conn,
        "https://x.example/f2",
        None,
        "f2",
        None,
        f2,
        "inherit",
        true,
        false,
    )
    .unwrap();

    // feed1 首个入库
    let (id1, new1) = upsert_article_with_feed(
        &conn,
        feed1,
        &new_article("https://n.example/a", "g1"),
        true,
    )
    .unwrap();
    assert!(new1);

    // feed2 推来同 URL（不同 guid）→ dedup 拦截
    let (_, new2) = upsert_article_with_feed(
        &conn,
        feed2,
        &new_article("https://n.example/a", "g2"),
        true,
    )
    .unwrap();
    assert!(!new2, "same URL cross-feed must be blocked by dedup");

    // feed2 不同 URL → 正常入库
    let (_, new3) = upsert_article_with_feed(
        &conn,
        feed2,
        &new_article("https://n.example/b", "g3"),
        true,
    )
    .unwrap();
    assert!(new3);

    // dedup 关闭时同 URL 也会入库（保持既有行为）
    let (_, new4) = upsert_article_with_feed(
        &conn,
        feed2,
        &new_article("https://n.example/a", "g4"),
        false,
    )
    .unwrap();
    assert!(new4, "dedup off must not block");

    // 同源 guid 冲突 → 更新而非插入（was_new=false）
    let (_, new5) = upsert_article_with_feed(
        &conn,
        feed1,
        &new_article("https://n.example/a", "g1"),
        true,
    )
    .unwrap();
    assert!(!new5);
    let _ = id1;
}
/// 搜索（LIKE 子串）：中文子串、多词 AND、通配符转义。
/// 修复背景：unicode61 FTS 把整段中文当一个 token，搜「科技」匹配不到
/// 「科技公司新闻」——改为子串匹配后语义对任意语言正确。
#[test]
fn search_finds_chinese_substring_and_multi_term_and() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    let f = create_folder(&conn, "F", "article").unwrap();
    let feed = insert_feed(
        &conn,
        "https://x.example/f",
        None,
        "f",
        None,
        f,
        "inherit",
        true,
        false,
    )
    .unwrap();

    let art = |url: &str, guid: &str, title: &str, body: &str| {
        let mut a = new_article(url, guid);
        a.title = title.into();
        a.body_text = body.into();
        let _ = upsert_article_with_feed(&conn, feed, &a, false).unwrap();
    };
    art(
        "https://n.example/1",
        "g1",
        "科技公司新闻",
        "今天发布了新产品",
    );
    art(
        "https://n.example/2",
        "g2",
        "无关标题",
        "正文提到了科技公司",
    );
    art("https://n.example/3", "g3", "另一个", "完全没有相关内容");

    // 中文子串：标题或正文含「科技」都命中（FTS 时代这条是失败的）
    let r1 = search_articles(&conn, "科技", 50).unwrap();
    assert_eq!(
        r1.len(),
        2,
        "chinese substring must match both: {:?}",
        r1.iter().map(|a| &a.title).collect::<Vec<_>>()
    );

    // 多词 AND：两个词都命中才返回
    let r2 = search_articles(&conn, "科技 产品", 50).unwrap();
    assert_eq!(
        r2.len(),
        1,
        "AND semantics: {:?}",
        r2.iter().map(|a| &a.title).collect::<Vec<_>>()
    );
    assert_eq!(r2[0].title, "科技公司新闻");

    // 无命中
    let r3 = search_articles(&conn, "不存在的词", 50).unwrap();
    assert!(r3.is_empty());

    // LIKE 通配符按字面转义：存 % 和 _ 的标题不被 % 通配误命中
    art("https://n.example/4", "g4", "100%_安全", "特殊字符");
    let r4 = search_articles(&conn, "100%", 50).unwrap();
    assert_eq!(r4.len(), 1, "literal %% must match: {}", r4.len());
    assert_eq!(r4[0].title, "100%_安全");
    // 单下划线词不当作通配符命中任意单字符
    let r5 = search_articles(&conn, "100X_安全", 50).unwrap();
    assert!(r5.is_empty(), "underscore must be literal, not wildcard");

    // 含 FTS 特殊字符的词安全
    art(
        "https://n.example/5",
        "g5",
        "node.js 指南",
        "C++ 与 Rust 对比",
    );
    let r6 = search_articles(&conn, "node.js", 50).unwrap();
    assert_eq!(r6.len(), 1);
    let r7 = search_articles(&conn, "C++", 50).unwrap();
    assert_eq!(r7.len(), 1);
}

/// 搜索命中 AI 摘要/翻译（SRH-2）：正文/标题都不含关键词、仅 AI 产物含时也要命中。
#[test]
fn search_finds_ai_summary_and_translation() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    let f = create_folder(&conn, "F", "article").unwrap();
    let feed = insert_feed(
        &conn,
        "https://x.example/f",
        None,
        "f",
        None,
        f,
        "inherit",
        true,
        false,
    )
    .unwrap();

    let mut a = new_article("https://n.example/1", "g1");
    a.title = "普通标题".into();
    a.body_text = "正文不含关键词".into();
    let (id, _) = upsert_article_with_feed(&conn, feed, &a, false).unwrap();

    // 仅摘要含关键词
    set_article_ai_fields(&conn, id, Some("这篇讲的是量子计算的突破"), None).unwrap();
    let r1 = search_articles(&conn, "量子计算", 50).unwrap();
    assert_eq!(r1.len(), 1, "ai_summary 命中");

    // 仅译文含关键词
    set_article_ai_fields(&conn, id, None, Some("译文里提到了深度学习模型")).unwrap();
    let r2 = search_articles(&conn, "深度学习", 50).unwrap();
    assert_eq!(r2.len(), 1, "translated_content 命中");

    // 无关词不命中
    let r3 = search_articles(&conn, "不存在的词", 50).unwrap();
    assert!(r3.is_empty());
}

/// feed_id_by_url_normalized：Miniflux 返回的 feed_url 与本地直连添加时
/// 有协议/www./尾斜杠/跟踪参数差异时，必须规范化为同一 feed（否则同一
/// 订阅出现两个本地 feed → 文章翻倍、状态分裂、数量对不齐）。
#[test]
fn feed_url_normalized_matching_collides_differently_decorated_urls() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    let f = create_folder(&conn, "F", "article").unwrap();
    // 本地直连添加：https + www + 尾斜杠 + 跟踪参数
    let fid = insert_feed(
        &conn,
        "https://www.example.com/feed/?utm_source=x",
        None,
        "f",
        None,
        f,
        "inherit",
        false,
        false,
    )
    .unwrap();

    // Miniflux 返回：http + 无 www + 无尾斜杠 + 无跟踪参数 → 必须匹配到同一 feed
    let matched = feed_id_by_url_normalized(&conn, "http://example.com/feed").unwrap();
    assert_eq!(
        matched,
        Some(fid),
        "规范化后不同饰的 feed_url 必须匹配同一本地 feed"
    );

    // 精确匹配（旧函数）对这种情况会漏判——保持旧函数不变，仅新函数规范化
    let exact = feed_id_by_url(&conn, "http://example.com/feed").unwrap();
    assert!(
        exact.is_none(),
        "精确匹配对规范化差异应返回 None（这正是修复前漏判的根因）"
    );
}

/// article_index 与 list_articles 位置对齐：某篇文章的绝对位置 = list_articles
/// 用该 offset 拉取时的第一条。验证双向分页锚定（搜索/深层打开文章）的正确性。
#[test]
fn article_index_positions_align_with_list() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    let f = create_folder(&conn, "F", "article").unwrap();
    let feed = insert_feed(
        &conn,
        "https://x.example/f",
        None,
        "f",
        None,
        f,
        "inherit",
        true,
        false,
    )
    .unwrap();

    // 插入 5 篇，published_at 递增（最新在最前，newest_first=true）
    let mut ids = Vec::new();
    for i in 0..5 {
        let mut a = new_article(&format!("https://n.example/{i}"), &format!("g{i}"));
        a.published_at = Some(format!("2026-01-0{}T00:00:00Z", i + 1));
        let (id, _) = upsert_article_with_feed(&conn, feed, &a, false).unwrap();
        ids.push(id);
    }

    let q = ArticleQuery {
        feed_id: Some(feed),
        folder_id: None,
        only_unread: false,
        only_starred: false,
        only_today: false,
        newest_first: true,
        limit: 500,
        offset: 0,
        with_content: false,
    };

    // 全量列表顺序：最新（i=4）在最前
    let all = list_articles(&conn, &q).unwrap();
    assert_eq!(all.len(), 5);
    // 位置 0 = i=4（最新），位置 4 = i=0（最旧）
    assert_eq!(all[0].id, ids[4], "newest first: ids[4] at pos 0");
    assert_eq!(all[4].id, ids[0], "newest first: ids[0] at pos 4");

    // 每篇的 article_index 应与它在列表中的位置一致
    for (pos, row) in all.iter().enumerate() {
        let idx = article_index(&conn, &q, row.id).unwrap();
        assert_eq!(
            idx,
            Some(pos as i64),
            "article {} should be at pos {}",
            row.id,
            pos
        );
    }

    // 从某个 offset 拉取，第一条应是 article_index 等于该 offset 的文章
    let target = all[2].id; // 位置 2 的文章
    let idx = article_index(&conn, &q, target).unwrap().unwrap();
    assert_eq!(idx, 2);
    let page = list_articles(
        &conn,
        &ArticleQuery {
            offset: idx,
            ..q.clone()
        },
    )
    .unwrap();
    assert_eq!(page[0].id, target, "offset={} 的第一条应是目标文章", idx);
}
