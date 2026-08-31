//! 集成测试：直连抓取全链路（fetch → parse → upsert → 查询）。
//! 用本地 HTTP feed 服务（127.0.0.1:8765）做确定性验证，不依赖外网
//! （国内网络对境外源站的 DNS/直连不稳定，那是环境问题不是管线问题）。
//! 运行：先 python -m http.server 8765 --bind 127.0.0.1（serve 含 local_feed.xml 的目录）
//! 然后 cargo test --test ingestion_e2e -- --ignored --nocapture

use app_lib::db;
use app_lib::ingestion;

const FEED_URL: &str = "http://127.0.0.1:8765/local_feed.xml";

#[tokio::test]
#[ignore = "requires local feed server on 127.0.0.1:8765"]
async fn direct_fetch_pipeline_end_to_end() {
    let tmp = std::env::temp_dir().join("fluxreader_e2e_test.db");
    let _ = std::fs::remove_file(&tmp);
    let mut conn = db::open(&tmp).expect("open db");

    // 1. 建分类 + 直连抓取验证（add_feed 命令的核心路径）
    let folder_id = db::create_folder(&conn, "技术开发", "article").unwrap();

    let client = ingestion::build_client(30);
    let fetched = ingestion::conditional_get(&client, FEED_URL, None, None)
        .await
        .expect("direct fetch");
    let (bytes, etag, last_modified) = match fetched {
        ingestion::Fetched::NotModified => panic!("first fetch must return body"),
        ingestion::Fetched::Body { bytes, etag, last_modified, .. } => (bytes, etag, last_modified),
    };
    assert!(!bytes.is_empty(), "feed body should not be empty");

    let parsed = ingestion::parse_feed(&bytes, FEED_URL).expect("parse feed");
    assert_eq!(parsed.title.as_deref(), Some("Local Test Feed"));
    assert_eq!(parsed.articles.len(), 2, "both entries parsed");
    println!("feed title: {:?}", parsed.title.as_deref());

    let feed_id = db::insert_feed(
        &conn,
        FEED_URL,
        parsed.site_url.as_deref(),
        parsed.title.as_deref().unwrap_or(""),
        parsed.icon.as_deref(),
        folder_id,
        "inherit",
        true,
        false,
    )
    .unwrap();
    db::set_feed_fetch_state(&conn, feed_id, false, None, etag.as_deref(), last_modified.as_deref()).unwrap();

    // 2. 条目全部入库（source='direct'）+ 相对 URL 已解析为绝对
    for a in &parsed.articles {
        db::upsert_article_with_feed(&conn, feed_id, a, false).unwrap();
    }
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM articles WHERE feed_id = ?1", [feed_id], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2, "all articles persisted");

    // 相对链接解析：entry link href="/post/1" 应变成绝对 URL
    let abs: String = conn
        .query_row(
            "SELECT url FROM articles WHERE feed_id = ?1 AND title LIKE 'Direct%'",
            [feed_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(abs, "http://127.0.0.1:8765/post/1", "relative link resolved");

    // 3. 列表查询路径（与前端 listArticles 同构）
    let items = db::list_articles(
        &conn,
        &db::ArticleQuery {
            feed_id: Some(feed_id),
            folder_id: None,
            only_unread: false,
            only_starred: false,
            only_today: false,
            newest_first: true,
            limit: 500,
        },
    )
    .unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].source, "direct");
    assert!(!items[0].is_read, "new articles start unread");
    // newest_first：11:00 的条目应排在 10:00 之前
    assert!(items[0].title.contains("Direct"), "newest first ordering");
    println!("newest: {}", items[0].title);

    // 4. 幂等重抓：同一 feed 再 upsert 不产生重复
    for a in &parsed.articles {
        db::upsert_article_with_feed(&conn, feed_id, a, false).unwrap();
    }
    let count2: i64 = conn
        .query_row("SELECT COUNT(*) FROM articles WHERE feed_id = ?1", [feed_id], |r| r.get(0))
        .unwrap();
    assert_eq!(count2, count, "re-upsert must not duplicate");

    // 5. 已读状态在重抓后保持（用户状态不被覆盖）
    db::set_read(&conn, items[0].id, true).unwrap();
    for a in &parsed.articles {
        db::upsert_article_with_feed(&conn, feed_id, a, false).unwrap();
    }
    let re = db::get_article(&conn, items[0].id).unwrap().unwrap();
    assert!(re.is_read, "read state must survive re-fetch");

    // 6. HTML 消毒：content 中的相对 img 已被 base 重写
    assert!(
        re.content_html.as_deref().unwrap_or("").contains("127.0.0.1:8765/img/a.png"),
        "relative img resolved in sanitized html"
    );
    println!("sanitized html ok");

    // 7. 消毒函数单点验证：事件处理器/js scheme 剥离，img src 重写
    let dirty = r#"<img src="/x.png" onerror="alert(1)"><a href="javascript:evil()">c</a><p>ok</p>"#;
    let clean = app_lib::sanitize::sanitize(dirty, Some("http://127.0.0.1:8765/"));
    assert!(!clean.contains("onerror"), "event handler stripped");
    assert!(!clean.contains("javascript:"), "js scheme stripped");
    assert!(clean.contains("http://127.0.0.1:8765/x.png"), "img src rewritten");
    println!("sanitize ok");

    // 8. 条件 GET 复请求路径不炸（本地 http.server 不回 ETag，仅验证请求路径）
    let r2 = ingestion::conditional_get(&client, FEED_URL, etag.as_deref(), last_modified.as_deref()).await;
    assert!(r2.is_ok(), "conditional re-fetch path ok");

    let _ = std::fs::remove_file(&tmp);
    println!("=== E2E PASS ===");
}
