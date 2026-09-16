use super::*;

/// ensure_uncategorized_folder：不存在时创建，已存在时返回现有 id
#[test]
fn ensure_uncategorized_folder_creates_or_returns_existing() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();

    // 首次调用：创建「未分类」
    let fid1 = ensure_uncategorized_folder(&conn).unwrap();
    assert!(fid1 > 0);

    // 再次调用：返回现有 id（不重复建）
    let fid2 = ensure_uncategorized_folder(&conn).unwrap();
    assert_eq!(fid1, fid2);

    // 验证数据库中确实只有一个「未分类」
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM folders WHERE name = '未分类'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

/// folder_exists：存在返回 true，不存在返回 false
#[test]
fn folder_exists_checks_correctly() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();

    let fid = create_folder(&conn, "测试分类", "article").unwrap();
    assert!(folder_exists(&conn, fid).unwrap());
    assert!(!folder_exists(&conn, 9999).unwrap());
}

/// list_unread_ids_scoped + mark_all_read：范围筛选一致性
#[test]
fn list_unread_ids_and_mark_all_read_scope_alignment() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();

    let f1 = create_folder(&conn, "F1", "article").unwrap();
    let f2 = create_folder(&conn, "F2", "article").unwrap();
    let feed1 = insert_feed(
        &conn,
        "http://a.example/f1",
        None,
        "feed1",
        None,
        f1,
        "inherit",
        false,
        false,
    )
    .unwrap();
    let feed2 = insert_feed(
        &conn,
        "http://a.example/f2",
        None,
        "feed2",
        None,
        f2,
        "inherit",
        false,
        false,
    )
    .unwrap();

    // 插入文章：feed1 两篇未读，feed2 一篇未读
    let art = |feed_id: i64, guid: &str| {
        let a = NewArticle {
            guid: guid.into(),
            url: None,
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
        };
        upsert_article_with_feed(&conn, feed_id, &a, false).unwrap();
    };
    art(feed1, "g1");
    art(feed1, "g2");
    art(feed2, "g3");

    // 全部文章未读
    let all_unread = list_unread_ids_scoped(&conn, None, None, false, None).unwrap();
    assert_eq!(all_unread.len(), 3);

    // feed1 范围
    let feed1_unread = list_unread_ids_scoped(&conn, Some(feed1), None, false, None).unwrap();
    assert_eq!(feed1_unread.len(), 2);

    // folder f1 范围
    let folder1_unread = list_unread_ids_scoped(&conn, None, Some(f1), false, None).unwrap();
    assert_eq!(folder1_unread.len(), 2);

    // 标读 feed1
    let n = mark_all_read(&conn, Some(feed1), None, false, None).unwrap();
    assert_eq!(n, 2);

    // 剩余未读应该只有 feed2 的一篇
    let remaining = list_unread_ids_scoped(&conn, None, None, false, None).unwrap();
    assert_eq!(remaining.len(), 1);
}

/// F8：mark_all_read / list_unread_ids_scoped 的视图口径过滤
/// （收藏视图只标收藏文章；今天视图只标边界之后的文章）——收集与标读同口径。
#[test]
fn mark_all_read_view_filters() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();

    let f = create_folder(&conn, "F", "article").unwrap();
    let feed = insert_feed(
        &conn,
        "http://a.example/scope",
        None,
        "feed",
        None,
        f,
        "inherit",
        false,
        false,
    )
    .unwrap();

    let mk = |guid: &str| {
        let a = NewArticle {
            guid: guid.into(),
            url: None,
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
        };
        upsert_article_with_feed(&conn, feed, &a, false).unwrap();
    };
    mk("star-old");
    mk("plain-recent");
    mk("plain-old");

    conn.execute(
        "UPDATE articles SET is_starred = 1 WHERE guid = 'star-old'",
        [],
    )
    .unwrap();
    let recent = chrono::Utc::now().to_rfc3339();
    let old = (chrono::Utc::now() - chrono::Duration::days(30)).to_rfc3339();
    conn.execute(
        "UPDATE articles SET published_at = ?1 WHERE guid = 'plain-recent'",
        [&recent],
    )
    .unwrap();
    conn.execute(
        "UPDATE articles SET published_at = ?1 WHERE guid IN ('star-old','plain-old')",
        [&old],
    )
    .unwrap();

    // 收藏视图：只标收藏文章（1 篇）
    let starred_ids = list_unread_ids_scoped(&conn, None, None, true, None).unwrap();
    assert_eq!(starred_ids.len(), 1, "收藏口径只应包含收藏文章");
    let n = mark_all_read(&conn, None, None, true, None).unwrap();
    assert_eq!(n, 1, "收藏视图只标 1 篇");

    // 今天视图：边界 = 1 小时前 → 只命中 recent 那篇
    let since_ms = (chrono::Utc::now() - chrono::Duration::hours(1)).timestamp_millis();
    let today_ids = list_unread_ids_scoped(&conn, None, None, false, Some(since_ms)).unwrap();
    assert_eq!(today_ids.len(), 1, "今天口径只应包含边界之后的文章");
    let n2 = mark_all_read(&conn, None, None, false, Some(since_ms)).unwrap();
    assert_eq!(n2, 1, "今天视图只标 1 篇");

    // 剩余未读：只有未收藏且较旧的那篇
    let left = list_unread_ids_scoped(&conn, None, None, false, None).unwrap();
    assert_eq!(left.len(), 1, "视图口径外的文章不应被标读");
}

/// get_article_url：存在返回 url，不存在返回 None
#[test]
fn get_article_url_returns_url_or_none() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();

    let f = create_folder(&conn, "F", "article").unwrap();
    let feed = insert_feed(
        &conn,
        "http://a.example/f",
        None,
        "feed",
        None,
        f,
        "inherit",
        false,
        false,
    )
    .unwrap();

    let a = NewArticle {
        guid: "g1".into(),
        url: Some("http://example.com/a1".into()),
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
    };
    let (aid, _) = upsert_article_with_feed(&conn, feed, &a, false).unwrap();

    let url = get_article_url(&conn, aid).unwrap();
    assert_eq!(url, Some("http://example.com/a1".to_string()));

    let none_url = get_article_url(&conn, 9999).unwrap();
    assert_eq!(none_url, None);
}

/// feed_exists_by_url：URL 存在返回 true
#[test]
fn feed_exists_by_url_checks_correctly() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();

    let f = create_folder(&conn, "F", "article").unwrap();
    insert_feed(
        &conn,
        "http://example.com/feed",
        None,
        "feed",
        None,
        f,
        "inherit",
        false,
        false,
    )
    .unwrap();

    assert!(feed_exists_by_url(&conn, "http://example.com/feed").unwrap());
    assert!(!feed_exists_by_url(&conn, "http://other.example/feed").unwrap());
}

/// export_feeds_with_folders：返回 (title, url, folder_name) 元组列表
#[test]
fn export_feeds_with_folders_returns_tuples() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();

    let f1 = create_folder(&conn, "科技", "article").unwrap();
    let f2 = create_folder(&conn, "新闻", "article").unwrap();
    insert_feed(
        &conn,
        "http://a.example/tech",
        None,
        "科技源",
        None,
        f1,
        "inherit",
        false,
        false,
    )
    .unwrap();
    insert_feed(
        &conn,
        "http://b.example/news",
        None,
        "新闻源",
        None,
        f2,
        "inherit",
        false,
        false,
    )
    .unwrap();

    let rows = export_feeds_with_folders(&conn).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|(t, _, _)| t == "科技源"));
    assert!(rows
        .iter()
        .any(|(t, _, f)| t == "新闻源" && f.as_deref() == Some("新闻")));
}

/// count_unbound_local_feeds：统计未绑定本地源
#[test]
fn count_unbound_local_feeds_counts_correctly() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();

    let f = create_folder(&conn, "F", "article").unwrap();
    insert_feed(
        &conn,
        "http://a.example/f1",
        None,
        "f1",
        None,
        f,
        "inherit",
        false,
        false,
    )
    .unwrap();
    insert_feed(
        &conn,
        "http://a.example/f2",
        None,
        "f2",
        None,
        f,
        "inherit",
        false,
        false,
    )
    .unwrap();

    let count = count_unbound_local_feeds(&conn).unwrap();
    assert_eq!(count, 2);

    // 绑定一个
    let fid = feed_id_by_url(&conn, "http://a.example/f1")
        .unwrap()
        .unwrap();
    set_feed_remote_id(&conn, fid, 123).unwrap();

    let count2 = count_unbound_local_feeds(&conn).unwrap();
    assert_eq!(count2, 1);
}

/// list_unbound_local_feeds：返回 (id, url, folder_id) 元组列表
#[test]
fn list_unbound_local_feeds_returns_tuples() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();

    let f = create_folder(&conn, "F", "article").unwrap();
    insert_feed(
        &conn,
        "http://a.example/f1",
        None,
        "f1",
        None,
        f,
        "inherit",
        false,
        false,
    )
    .unwrap();

    let rows = list_unbound_local_feeds(&conn).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].1, "http://a.example/f1");
    assert_eq!(rows[0].2, Some(f));
}

/// get_article_for_summary：返回 (title, body_text, ai_summary)
#[test]
fn get_article_for_summary_returns_tuple() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();

    let f = create_folder(&conn, "F", "article").unwrap();
    let feed = insert_feed(
        &conn,
        "http://a.example/f",
        None,
        "feed",
        None,
        f,
        "inherit",
        false,
        false,
    )
    .unwrap();

    let a = NewArticle {
        guid: "g1".into(),
        url: None,
        title: "测试标题".into(),
        author: None,
        summary: None,
        content_html: None,
        body_text: "测试正文".into(),
        image_url: None,
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: None,
        source: "direct".into(),
    };
    let (aid, _) = upsert_article_with_feed(&conn, feed, &a, false).unwrap();

    let (title, body, summary) = get_article_for_summary(&conn, aid).unwrap().unwrap();
    assert_eq!(title, "测试标题");
    assert_eq!(body, "测试正文");
    assert_eq!(summary, None);

    // 设置 AI 摘要后再查
    set_article_ai_fields(&conn, aid, Some("AI 摘要"), None).unwrap();
    let (_, _, summary2) = get_article_for_summary(&conn, aid).unwrap().unwrap();
    assert_eq!(summary2, Some("AI 摘要".to_string()));
}

/// get_article_for_translation：返回 (title, content_html, translated_content)
#[test]
fn get_article_for_translation_returns_tuple() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();

    let f = create_folder(&conn, "F", "article").unwrap();
    let feed = insert_feed(
        &conn,
        "http://a.example/f",
        None,
        "feed",
        None,
        f,
        "inherit",
        false,
        false,
    )
    .unwrap();

    let a = NewArticle {
        guid: "g1".into(),
        url: None,
        title: "Test Title".into(),
        author: None,
        summary: None,
        content_html: Some("<p>Test content</p>".into()),
        body_text: "Test content".into(),
        image_url: None,
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: None,
        source: "direct".into(),
    };
    let (aid, _) = upsert_article_with_feed(&conn, feed, &a, false).unwrap();

    let (title, html, translated) = get_article_for_translation(&conn, aid).unwrap().unwrap();
    assert_eq!(title, "Test Title");
    assert_eq!(html, "<p>Test content</p>");
    assert_eq!(translated, None);
}

/// update_article_fulltext：更新正文与提取标志
#[test]
fn update_article_fulltext_updates_correctly() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();

    let f = create_folder(&conn, "F", "article").unwrap();
    let feed = insert_feed(
        &conn,
        "http://a.example/f",
        None,
        "feed",
        None,
        f,
        "inherit",
        false,
        false,
    )
    .unwrap();

    let a = NewArticle {
        guid: "g1".into(),
        url: None,
        title: "t".into(),
        author: None,
        summary: None,
        content_html: Some("<p>原始</p>".into()),
        body_text: "原始".into(),
        image_url: None,
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: None,
        source: "direct".into(),
    };
    let (aid, _) = upsert_article_with_feed(&conn, feed, &a, false).unwrap();

    update_article_fulltext(&conn, aid, "<p>全文</p>", true).unwrap();

    let row = get_article(&conn, aid).unwrap().unwrap();
    assert_eq!(row.content_html, Some("<p>全文</p>".to_string()));
    assert!(row.fulltext_extracted);
}

/// update_article_image_if_empty：仅在封面为空时更新
#[test]
fn update_article_image_if_empty_only_when_empty() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();

    let f = create_folder(&conn, "F", "article").unwrap();
    let feed = insert_feed(
        &conn,
        "http://a.example/f",
        None,
        "feed",
        None,
        f,
        "inherit",
        false,
        false,
    )
    .unwrap();

    let a = NewArticle {
        guid: "g1".into(),
        url: None,
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
    };
    let (aid, _) = upsert_article_with_feed(&conn, feed, &a, false).unwrap();

    // 首次更新：封面为空，应成功
    update_article_image_if_empty(&conn, aid, "http://example.com/img1.jpg").unwrap();
    let row1 = get_article(&conn, aid).unwrap().unwrap();
    assert_eq!(
        row1.image_url,
        Some("http://example.com/img1.jpg".to_string())
    );

    // 再次更新：封面已有，不应覆盖
    update_article_image_if_empty(&conn, aid, "http://example.com/img2.jpg").unwrap();
    let row2 = get_article(&conn, aid).unwrap().unwrap();
    assert_eq!(
        row2.image_url,
        Some("http://example.com/img1.jpg".to_string())
    );
}

/// get_article_content_html：返回正文 HTML
#[test]
fn get_article_content_html_returns_html() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();

    let f = create_folder(&conn, "F", "article").unwrap();
    let feed = insert_feed(
        &conn,
        "http://a.example/f",
        None,
        "feed",
        None,
        f,
        "inherit",
        false,
        false,
    )
    .unwrap();

    let a = NewArticle {
        guid: "g1".into(),
        url: None,
        title: "t".into(),
        author: None,
        summary: None,
        content_html: Some("<p>内容</p>".into()),
        body_text: "内容".into(),
        image_url: None,
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: None,
        source: "direct".into(),
    };
    let (aid, _) = upsert_article_with_feed(&conn, feed, &a, false).unwrap();

    let html = get_article_content_html(&conn, aid).unwrap();
    assert_eq!(html, "<p>内容</p>");
}
