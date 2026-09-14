use super::*;
use rusqlite::params;

/// get_article_remote_id：返回文章的 remote_id（未绑定返回 None）
#[test]
fn get_article_remote_id_returns_id_or_none() {
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

    // 初始未绑定
    let remote_id = get_article_remote_id(&conn, aid).unwrap();
    assert_eq!(remote_id, None);

    // 绑定后返回 id
    set_article_remote_id(&conn, aid, 123).unwrap();
    let remote_id = get_article_remote_id(&conn, aid).unwrap();
    assert_eq!(remote_id, Some(123));
}

/// find_folder_by_name：按名称查询 folder（不存在返回 None）
#[test]
fn find_folder_by_name_returns_id_or_none() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();

    let fid = create_folder(&conn, "科技", "article").unwrap();
    assert_eq!(find_folder_by_name(&conn, "科技").unwrap(), Some(fid));
    assert_eq!(find_folder_by_name(&conn, "不存在").unwrap(), None);
}

/// get_first_folder_id：返回第一个 folder（空库返回 None）
#[test]
fn get_first_folder_id_returns_first_or_none() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();

    // 空库
    assert_eq!(get_first_folder_id(&conn).unwrap(), None);

    // 有 folder
    let fid = create_folder(&conn, "F1", "article").unwrap();
    assert_eq!(get_first_folder_id(&conn).unwrap(), Some(fid));
}

/// update_feed_title_if_empty：仅当本地标题等于 feed_url 时回填
#[test]
fn update_feed_title_if_empty_only_when_title_equals_url() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();

    let f = create_folder(&conn, "F", "article").unwrap();
    let feed1 = insert_feed(
        &conn,
        "http://a.example/f1",
        None,
        "http://a.example/f1", // 标题等于 URL（未抓取成功过）
        None,
        f,
        "inherit",
        false,
        false,
    )
    .unwrap();
    let feed2 = insert_feed(
        &conn,
        "http://a.example/f2",
        None,
        "用户自定义标题",
        None,
        f,
        "inherit",
        false,
        false,
    )
    .unwrap();

    // feed1：标题等于 URL，应回填
    update_feed_title_if_empty(&conn, feed1, "远端标题1", Some("http://site1.example")).unwrap();
    let row1: String = conn
        .query_row(
            "SELECT title FROM feeds WHERE id = ?1",
            params![feed1],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(row1, "远端标题1");

    // feed2：已有用户标题，不应覆盖
    update_feed_title_if_empty(&conn, feed2, "远端标题2", Some("http://site2.example")).unwrap();
    let row2: String = conn
        .query_row(
            "SELECT title FROM feeds WHERE id = ?1",
            params![feed2],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(row2, "用户自定义标题");
}

/// sync_set_article_status：无条件更新已读和收藏状态
#[test]
fn sync_set_article_status_updates_unconditionally() {
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

    // 初始未读、未收藏
    let (is_read, is_starred): (i64, i64) = conn
        .query_row(
            "SELECT is_read, is_starred FROM articles WHERE id = ?1",
            params![aid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((is_read, is_starred), (0, 0));

    // 同步设置为已读+收藏
    sync_set_article_status(&conn, aid, true, true).unwrap();
    let (is_read, is_starred): (i64, i64) = conn
        .query_row(
            "SELECT is_read, is_starred FROM articles WHERE id = ?1",
            params![aid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((is_read, is_starred), (1, 1));
}

/// sync_mark_read_if_unread：仅未读时标读，返回影响行数
#[test]
fn sync_mark_read_if_unread_only_when_unread() {
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

    // 初始未读 → 标读成功
    let n = sync_mark_read_if_unread(&conn, aid).unwrap();
    assert_eq!(n, 1);

    // 已读 → 无影响
    let n = sync_mark_read_if_unread(&conn, aid).unwrap();
    assert_eq!(n, 0);
}

/// sync_mark_unread_if_read：仅已读时标未读，返回影响行数
#[test]
fn sync_mark_unread_if_read_only_when_read() {
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

    // 初始未读 → 无影响
    let n = sync_mark_unread_if_read(&conn, aid).unwrap();
    assert_eq!(n, 0);

    // 先标读
    set_read(&conn, aid, true).unwrap();

    // 已读 → 标未读成功
    let n = sync_mark_unread_if_read(&conn, aid).unwrap();
    assert_eq!(n, 1);
}

/// sync_mark_starred/unstarred：收藏切换返回影响行数
#[test]
fn sync_starred_toggles_correctly() {
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

    // 初始未收藏 → 收藏成功
    let n = sync_mark_starred_if_unstarred(&conn, aid).unwrap();
    assert_eq!(n, 1);

    // 已收藏 → 无影响
    let n = sync_mark_starred_if_unstarred(&conn, aid).unwrap();
    assert_eq!(n, 0);

    // 取消收藏成功
    let n = sync_mark_unstarred_if_starred(&conn, aid).unwrap();
    assert_eq!(n, 1);

    // 已取消 → 无影响
    let n = sync_mark_unstarred_if_starred(&conn, aid).unwrap();
    assert_eq!(n, 0);
}

/// backfill_article_content：本地为空才补，已有内容不覆盖
#[test]
fn backfill_article_content_only_when_empty() {
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
        content_html: None, // 正文为空
        body_text: "".into(),
        image_url: None,
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: None,
        source: "direct".into(),
    };
    let (aid, _) = upsert_article_with_feed(&conn, feed, &a, false).unwrap();

    // 首次回填：正文为空，应成功
    backfill_article_content(
        &conn,
        aid,
        "<p>远端正文</p>",
        "远端正文",
        Some("http://example.com/img1.jpg"),
        Some("http://example.com/audio.mp3"),
        Some("audio/mpeg"),
    )
    .unwrap();

    let (html, body, img, enc_url): (String, String, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT content_html, body_text, image_url, enclosure_url FROM articles WHERE id = ?1",
            params![aid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(html, "<p>远端正文</p>");
    assert_eq!(body, "远端正文");
    assert_eq!(img, Some("http://example.com/img1.jpg".to_string()));
    assert_eq!(enc_url, Some("http://example.com/audio.mp3".to_string()));

    // 再次回填：正文已有，不应覆盖
    backfill_article_content(
        &conn,
        aid,
        "<p>新正文</p>",
        "新正文",
        Some("http://example.com/img2.jpg"),
        Some("http://example.com/audio2.mp3"),
        Some("audio/mpeg"),
    )
    .unwrap();

    let (html2, body2, img2, enc_url2): (String, String, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT content_html, body_text, image_url, enclosure_url FROM articles WHERE id = ?1",
            params![aid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    // 正文不覆盖
    assert_eq!(html2, "<p>远端正文</p>");
    assert_eq!(body2, "远端正文");
    // 封面/enclosure 不覆盖（COALESCE 保留首值）
    assert_eq!(img2, Some("http://example.com/img1.jpg".to_string()));
    assert_eq!(enc_url2, Some("http://example.com/audio.mp3".to_string()));
}
