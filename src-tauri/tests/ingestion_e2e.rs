//! 集成测试：直连抓取全链路（fetch → parse → upsert → 查询）。
//! 自托管本地 HTTP feed 服务（OPT-016A）：测试自己绑定 127.0.0.1:0
//! （common::LocalFeedServer），固定 feed 内容与条件响应（ETag/Last-Modified、
//! 304）由本测试生成，结束时显式 stop 关闭 accept 循环；不依赖外网、不占用
//! 固定端口 8765、不借用任何用户现有服务。CI 默认运行（无 #[ignore]）。
//! 运行：cargo test --test ingestion_e2e -- --nocapture

use app_lib::db;
use app_lib::ingestion;

mod common;

#[tokio::test]
async fn direct_fetch_pipeline_end_to_end() {
    // 自托管服务：先绑定 127.0.0.1:0 拿到内核分配的端口，再把端口注入固定 feed 内容
    let server = common::LocalFeedServer::start_with_feed(common::local_feed_xml)
        .await
        .expect("start local feed server");
    let feed_url = server.url("/local_feed.xml");

    // 临时库守卫先于 Connection 声明：正常返回与 panic 展开都删除库文件（含 WAL 旁路）
    let tmp = common::unique_db_path("e2e_test");
    let _cleanup = common::TempDbGuard::new(tmp.clone());
    let conn = db::open(&tmp).expect("open db");

    // 1. 建分类 + 直连抓取验证（add_feed 命令的核心路径）
    let folder_id = db::create_folder(&conn, "技术开发", "article").unwrap();

    let client = ingestion::build_client(30);
    let fetched = ingestion::conditional_get(&client, &feed_url, None, None)
        .await
        .expect("direct fetch");
    let (bytes, etag, last_modified) = match fetched {
        ingestion::Fetched::NotModified => panic!("first fetch must return body"),
        ingestion::Fetched::Body {
            bytes,
            etag,
            last_modified,
            ..
        } => (bytes, etag, last_modified),
    };
    assert!(!bytes.is_empty(), "feed body should not be empty");
    assert!(etag.is_some(), "self-hosted server must send ETag");
    assert!(
        last_modified.is_some(),
        "self-hosted server must send Last-Modified"
    );

    let parsed = ingestion::parse_feed(&bytes, &feed_url).expect("parse feed");
    assert_eq!(parsed.title.as_deref(), Some("Local Test Feed"));
    assert_eq!(parsed.articles.len(), 2, "both entries parsed");
    assert!(
        parsed.icon.is_some(),
        "channel <image> parsed as icon（刷新管线因此不 spawn favicon 后台探测）"
    );
    println!("feed title: {:?}", parsed.title.as_deref());

    let feed_id = db::insert_feed(
        &conn,
        &feed_url,
        parsed.site_url.as_deref(),
        parsed.title.as_deref().unwrap_or(""),
        parsed.icon.as_deref(),
        folder_id,
        "inherit",
        true,
        false,
    )
    .unwrap();
    db::set_feed_fetch_state(
        &conn,
        feed_id,
        false,
        None,
        etag.as_deref(),
        last_modified.as_deref(),
    )
    .unwrap();

    // 2. 条目全部入库（source='direct'）+ 相对 URL 已解析为绝对
    for a in &parsed.articles {
        db::upsert_article_with_feed(&conn, feed_id, a, false).unwrap();
    }
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM articles WHERE feed_id = ?1",
            [feed_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 2, "all articles persisted");

    // 相对链接解析：entry link href="/post/1" 应变成绝对 URL（基址 = 自托管实际端口）
    let abs: String = conn
        .query_row(
            "SELECT url FROM articles WHERE feed_id = ?1 AND title LIKE 'Direct%'",
            [feed_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(abs, server.url("/post/1"), "relative link resolved");

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
            offset: 0,
            with_content: true,
            layout: None,
            /* TASK-121：TASK-117 给 ArticleQuery 增补 keyset 游标字段后，
             * 结构体字面量必须穷尽全字段；None/None = 既有 OFFSET 语义，
             * 本测试查询路径行为零变化。 */
            last_published: None,
            last_id: None,
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
        .query_row(
            "SELECT COUNT(*) FROM articles WHERE feed_id = ?1",
            [feed_id],
            |r| r.get(0),
        )
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
        re.content_html
            .as_deref()
            .unwrap_or("")
            .contains(&server.url("/img/a.png")),
        "relative img resolved in sanitized html"
    );
    println!("sanitized html ok");

    // 7. 消毒函数单点验证：事件处理器/js scheme 剥离，img src 重写
    let dirty =
        r#"<img src="/x.png" onerror="alert(1)"><a href="javascript:evil()">c</a><p>ok</p>"#;
    let base = server.url("/");
    let clean = app_lib::sanitize::sanitize(dirty, Some(&base));
    assert!(!clean.contains("onerror"), "event handler stripped");
    assert!(!clean.contains("javascript:"), "js scheme stripped");
    assert!(clean.contains(&server.url("/x.png")), "img src rewritten");
    println!("sanitize ok");

    // 8. 条件 GET 复请求：带自托管服务发出的验证器，应命中 304 未变更
    let r2 = ingestion::conditional_get(
        &client,
        &feed_url,
        etag.as_deref(),
        last_modified.as_deref(),
    )
    .await;
    assert!(r2.is_ok(), "conditional re-fetch path ok");
    assert!(
        matches!(r2, Ok(ingestion::Fetched::NotModified)),
        "matching validators must yield 304"
    );

    server.stop().await;
    println!("=== E2E PASS ===");
}
