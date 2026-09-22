//! TASK-068 N-硬3：行类型契约防漂移。
//! lib/api.ts 手写了 Rust Serialize 结构的 TS 镜像——Rust 侧改字段名/可空性，
//! TS 侧只会在运行时发现。本测试把规范的 FeedRow / ArticleListItem 序列化结果
//! 与检入的 fixture 逐字段比对：Rust 侧漂移在此处失败；TS 侧漂移由
//! frontend-regression 读同一 fixture 的映射断言捕获。

use app_lib::db;

#[test]
fn row_serialization_matches_fixture() {
    let fixture_raw = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/row_fixture.json"
    ))
    .expect("fixture 文件必须存在");
    let expected: serde_json::Value = serde_json::from_str(&fixture_raw).unwrap();

    let feed = db::FeedRow {
        id: 7,
        folder_id: 1,
        feed_url: "https://f.example/rss".into(),
        site_url: Some("https://f.example".into()),
        title: "Fixture Feed".into(),
        favicon_url: Some("https://f.example/favicon.ico".into()),
        layout: "article".into(),
        auto_summary: true,
        auto_translate: false,
        fetch_failed: false,
        fetch_error: None,
        last_fetched_at: None,
    };
    let article = db::ArticleListItem {
        id: 42,
        feed_id: 7,
        title: "Fixture Article".into(),
        author: Some("Fixture Author".into()),
        snippet: "snippet text".into(),
        image_url: Some("https://e.example/img.png".into()),
        enclosure_url: Some("https://e.example/audio.mp3".into()),
        enclosure_mime: Some("audio/mpeg".into()),
        duration_sec: Some(1234),
        ai_summary: Some("fixture summary".into()),
        source: "miniflux".into(),
        published_at: Some("2026-09-19T01:00:00+08:00".into()),
        is_read: false,
        is_starred: true,
        url: Some("https://e.example/a".into()),
        content_html: Some("<p>body</p>".into()),
        translated_content: Some("<p>translated</p>".into()),
        fulltext_extracted: false,
    };

    let feed_json = serde_json::to_value(&feed).expect("FeedRow 序列化");
    let article_json = serde_json::to_value(&article).expect("ArticleListItem 序列化");

    assert_eq!(
        feed_json, expected["feed_row"],
        "FeedRow 序列化漂移：Rust 侧字段变更必须同步 fixture 与 lib/api.ts 镜像"
    );
    assert_eq!(
        article_json, expected["article_list_item"],
        "ArticleListItem 序列化漂移：Rust 侧字段变更必须同步 fixture 与 lib/api.ts 镜像"
    );
}
