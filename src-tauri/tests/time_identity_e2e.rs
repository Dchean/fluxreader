//! 发布时间来源与持久条目身份回归（2026-11-03 审计 #P1-6）。
//!
//! 覆盖：
//! - 无源日期新条目：published_at_source=inferred_fetch，first_seen_at 兜底；
//! - 连续刷新：published_at / first_seen_at / guid 不变；
//! - 后续 feed 给出日期：可升级为 feed，之后不回退；
//! - 未来源日期：published_at_source=feed_clamped，后续真实日期可升级；
//! - 真实 id/URL 的优先级与同 feed 隔离；
//! - legacy_unknown、title+date、远程无日期与内容指纹边界。

use app_lib::db;
use app_lib::ingestion;
use rusqlite::Connection;

mod common;

struct TestDb {
    _guard: common::TempDbGuard,
    conn: Connection,
}

fn test_db(name: &str) -> TestDb {
    let path = common::unique_db_path(name);
    let _ = std::fs::remove_file(&path);
    let guard = common::TempDbGuard::new(path.clone());
    let conn = db::open(&path).expect("open temp db");
    TestDb {
        _guard: guard,
        conn,
    }
}

fn seed_feed(conn: &Connection, url: &str) -> i64 {
    let folder = db::create_folder(conn, "F", "article").unwrap();
    db::insert_feed(conn, url, None, "F", None, folder, "inherit", false, false).unwrap()
}

fn rss(items: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel>
<title>Identity Feed</title>
<link>https://feed.example/</link>
<description>fixture</description>
{items}
</channel></rss>"#
    )
}

fn upsert_feed(conn: &Connection, feed_id: i64, xml: &str) -> usize {
    let parsed = ingestion::parse_feed(xml.as_bytes(), "https://feed.example/rss")
        .expect("parse fixture feed");
    let mut inserted = 0;
    for article in &parsed.articles {
        let (_, is_new) = db::upsert_article_with_feed_source(
            conn,
            feed_id,
            &article.article,
            false,
            article.published_at_clamped,
        )
        .unwrap();
        inserted += usize::from(is_new);
    }
    inserted
}

#[derive(Debug, PartialEq, Eq)]
struct ArticleState {
    id: i64,
    guid: String,
    published_at: String,
    first_seen_at: Option<String>,
    published_at_source: String,
}

fn article_state(conn: &Connection, id: i64) -> ArticleState {
    conn.query_row(
        "SELECT id, guid, published_at, first_seen_at, published_at_source
           FROM articles WHERE id = ?1",
        [id],
        |r| {
            Ok(ArticleState {
                id: r.get(0)?,
                guid: r.get(1)?,
                published_at: r.get(2)?,
                first_seen_at: r.get(3)?,
                published_at_source: r.get(4)?,
            })
        },
    )
    .unwrap()
}

fn only_id(conn: &Connection) -> i64 {
    conn.query_row("SELECT id FROM articles", [], |r| r.get(0))
        .unwrap()
}

/// a/b：源无日期 → inferred_fetch，first_seen_at 兜底；连续刷新三者不变。
#[test]
fn missing_source_date_uses_first_seen_once_and_stays_stable() {
    let t = test_db("time_identity_missing_date");
    let feed = seed_feed(&t.conn, "https://feed.example/rss");
    let xml = rss(r#"<item>
             <title>No Date</title>
             <link>https://e.example/no-date</link>
             <description>same body</description>
           </item>"#);
    let parsed = ingestion::parse_feed(xml.as_bytes(), "https://feed.example/rss").expect("parse");
    assert!(
        parsed.articles[0].published_at.is_none(),
        "解析层必须保留「源未提供日期」的事实，不能写 Utc::now()"
    );

    assert_eq!(upsert_feed(&t.conn, feed, &xml), 1);
    let before = article_state(&t.conn, only_id(&t.conn));
    assert_eq!(before.published_at_source, "inferred_fetch");
    assert_eq!(
        before.first_seen_at.as_deref(),
        Some(before.published_at.as_str()),
        "无源日期时发布时间兜底必须等于首次见到时间"
    );
    assert!(before.first_seen_at.is_some());

    std::thread::sleep(std::time::Duration::from_millis(20));
    assert_eq!(upsert_feed(&t.conn, feed, &xml), 0);
    let after = article_state(&t.conn, before.id);
    assert_eq!(after, before, "连续刷新不得改发布时间/首次见到/身份");
}

/// c：后续 feed 给出日期 → 由 inferred_fetch 升级为 feed；再刷新不回退。
#[test]
fn later_source_date_upgrades_once_and_never_rolls_back() {
    let t = test_db("time_identity_upgrade");
    let feed = seed_feed(&t.conn, "https://feed.example/rss");
    let undated = rss(r#"<item><title>Undated</title><description>same body</description></item>"#);
    let dated = rss(r#"<item>
             <title>Undated</title>
             <description>same body</description>
             <pubDate>Mon, 24 Aug 2026 11:00:00 GMT</pubDate>
           </item>"#);

    assert_eq!(upsert_feed(&t.conn, feed, &undated), 1);
    let before = article_state(&t.conn, only_id(&t.conn));
    assert_eq!(before.published_at_source, "inferred_fetch");

    assert_eq!(upsert_feed(&t.conn, feed, &dated), 0);
    let upgraded = article_state(&t.conn, before.id);
    assert_eq!(upgraded.guid, before.guid, "补日期不得改变已建立的持久身份");
    assert_eq!(
        upgraded.first_seen_at, before.first_seen_at,
        "first_seen_at 永不覆盖"
    );
    assert_eq!(upgraded.published_at_source, "feed");
    assert_eq!(upgraded.published_at, "2026-08-24T11:00:00+00:00");

    std::thread::sleep(std::time::Duration::from_millis(20));
    assert_eq!(upsert_feed(&t.conn, feed, &dated), 0);
    assert_eq!(
        article_state(&t.conn, before.id),
        upgraded,
        "后续 feed 刷新不得再次改动"
    );

    // 上游又撤回日期：已有 feed 日期不得被推断值/抓取时间覆盖。
    assert_eq!(upsert_feed(&t.conn, feed, &undated), 0);
    assert_eq!(
        article_state(&t.conn, before.id),
        upgraded,
        "feed 日期建立后，缺失日期刷新不得回退为 inferred_fetch"
    );
}

/// 未来日期：值被钳制为当前时间，来源必须明确标 feed_clamped；后续真实日期
/// 可升级为 feed，且身份不得使用钳制时间。
#[test]
fn future_source_date_is_clamped_marked_and_identity_stays_stable() {
    let t = test_db("time_identity_future");
    let feed = seed_feed(&t.conn, "https://feed.example/rss");
    let future = rss(r#"<item>
             <title>Future</title>
             <description>future body</description>
             <pubDate>Wed, 24 Aug 2033 11:00:00 GMT</pubDate>
           </item>"#);
    assert_eq!(upsert_feed(&t.conn, feed, &future), 1);
    let before = article_state(&t.conn, only_id(&t.conn));
    assert_eq!(before.published_at_source, "feed_clamped");
    let published = chrono::DateTime::parse_from_rfc3339(&before.published_at).unwrap();
    let now = chrono::Utc::now();
    assert!(
        (now - published.with_timezone(&chrono::Utc))
            .num_seconds()
            .abs()
            < 10,
        "未来日期必须被钳制到抓取窗口: {}",
        before.published_at
    );

    std::thread::sleep(std::time::Duration::from_millis(20));
    assert_eq!(upsert_feed(&t.conn, feed, &future), 0);
    let after = article_state(&t.conn, before.id);
    assert_eq!(after, before, "钳制后的当前时间不得进入身份/刷新覆盖");

    let corrected = rss(r#"<item>
             <title>Future</title>
             <description>future body</description>
             <pubDate>Mon, 24 Aug 2026 11:00:00 GMT</pubDate>
           </item>"#);
    assert_eq!(upsert_feed(&t.conn, feed, &corrected), 0);
    let upgraded = article_state(&t.conn, before.id);
    assert_eq!(upgraded.guid, before.guid);
    assert_eq!(upgraded.first_seen_at, before.first_seen_at);
    assert_eq!(upgraded.published_at_source, "feed");
    assert_eq!(upgraded.published_at, "2026-08-24T11:00:00+00:00");

    // 再收到未来日期：已有真实 feed 日期不得降级为 feed_clamped/当前时间。
    assert_eq!(upsert_feed(&t.conn, feed, &future), 0);
    assert_eq!(article_state(&t.conn, before.id), upgraded);
}

/// d：真实 id 优先于 URL 改写；无 id 时同 feed URL 兜底；跨 feed 不共享身份。
#[test]
fn stable_id_then_url_order_and_cross_feed_isolation() {
    let t = test_db("time_identity_order");
    let feed1 = seed_feed(&t.conn, "https://feed1.example/rss");
    let feed2 = seed_feed(&t.conn, "https://feed2.example/rss");

    let first = rss(r#"<item>
             <title>Stable</title>
             <guid isPermaLink="false">stable-guid</guid>
             <link>https://e.example/original</link>
             <description>body</description>
           </item>"#);
    assert_eq!(upsert_feed(&t.conn, feed1, &first), 1);
    let id = only_id(&t.conn);
    let guid: String = t
        .conn
        .query_row("SELECT guid FROM articles WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(guid, "stable-guid");

    // 同 guid、不同 URL：guid 命中优先，仍复用同一行并刷新 URL。
    let moved = rss(r#"<item>
             <title>Stable</title>
             <guid isPermaLink="false">stable-guid</guid>
             <link>https://e.example/moved</link>
             <description>body</description>
           </item>"#);
    assert_eq!(upsert_feed(&t.conn, feed1, &moved), 0);
    let (stored_id, url): (i64, String) = t
        .conn
        .query_row(
            "SELECT id, url FROM articles WHERE guid = 'stable-guid'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(stored_id, id);
    assert_eq!(url, "https://e.example/moved");

    // 无 guid、同 URL：URL 兜底命中既有真实 id 行，不新增。
    let no_guid = rss(r#"<item>
             <title>Stable</title>
             <link>https://e.example/moved</link>
             <description>body</description>
           </item>"#);
    assert_eq!(upsert_feed(&t.conn, feed1, &no_guid), 0);
    assert_eq!(only_id(&t.conn), id, "URL 兜底必须复用同 feed 的行");
    let before_url_upgrade = article_state(&t.conn, id);
    assert_eq!(before_url_upgrade.published_at_source, "inferred_fetch");

    // URL 命中路径同样按 source 条件升级；不得回写 guid 或 first_seen_at。
    let dated_by_url = rss(r#"<item>
             <title>Stable</title>
             <link>https://e.example/moved</link>
             <description>body</description>
             <pubDate>Mon, 24 Aug 2026 11:00:00 GMT</pubDate>
           </item>"#);
    assert_eq!(upsert_feed(&t.conn, feed1, &dated_by_url), 0);
    let after_url_upgrade = article_state(&t.conn, id);
    assert_eq!(after_url_upgrade.guid, "stable-guid");
    assert_eq!(
        after_url_upgrade.first_seen_at,
        before_url_upgrade.first_seen_at
    );
    assert_eq!(after_url_upgrade.published_at_source, "feed");
    assert_eq!(after_url_upgrade.published_at, "2026-08-24T11:00:00+00:00");

    // 另一个 feed 的同 URL：feed-scoped，独立成行。
    assert_eq!(upsert_feed(&t.conn, feed2, &no_guid), 1);
    let total: i64 = t
        .conn
        .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total, 2, "不得跨 feed 复用身份");
}

/// URL 身份必须先于 title+date 弱身份：两者在同 feed 同时存在时，URL 命中
/// 已有真实 id 行，不能被同标题/同日期但无 URL 的弱行抢走。
#[test]
fn url_identity_precedes_title_date_weak_identity() {
    let t = test_db("time_identity_priority");
    let feed = seed_feed(&t.conn, "https://feed.example/rss");
    let with_id = rss(r#"<item>
             <title>Same Title</title>
             <guid isPermaLink="false">real-url-row</guid>
             <link>https://e.example/shared</link>
             <description>shared body</description>
             <pubDate>Mon, 24 Aug 2026 11:00:00 GMT</pubDate>
           </item>"#);
    assert_eq!(upsert_feed(&t.conn, feed, &with_id), 1);
    let real_id = only_id(&t.conn);

    // 同标题/日期、无 id/URL 的弱身份行（title+date）。
    let weak = rss(r#"<item>
             <title>Same Title</title>
             <description>shared body</description>
             <pubDate>Mon, 24 Aug 2026 11:00:00 GMT</pubDate>
           </item>"#);
    assert_eq!(upsert_feed(&t.conn, feed, &weak), 1);
    let weak_id: i64 = t
        .conn
        .query_row(
            "SELECT id FROM articles WHERE guid != 'real-url-row'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_ne!(weak_id, real_id);

    // 现在无 id 但有 URL：URL 必须命中 real-url-row，而不是 title+date 弱行。
    let url_only = rss(r#"<item>
             <title>Same Title</title>
             <link>https://e.example/shared</link>
             <description>shared body</description>
             <pubDate>Mon, 24 Aug 2026 11:00:00 GMT</pubDate>
           </item>"#);
    assert_eq!(upsert_feed(&t.conn, feed, &url_only), 0);
    let url_row: i64 = t
        .conn
        .query_row(
            "SELECT id FROM articles WHERE url = 'https://e.example/shared'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(url_row, real_id, "URL 身份必须优先于 title+date 弱身份");
    let weak_url: Option<String> = t
        .conn
        .query_row("SELECT url FROM articles WHERE id = ?1", [weak_id], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(weak_url.is_none(), "弱行不得被 URL 命中路径改写");
}

/// e：无 id/url/date 的标题+正文指纹可重复；正文变化按已记录边界产生新行，
/// 同一变化内容重复刷新稳定，A→B→A 的行数由不同内容指纹数界定。
#[test]
fn content_fingerprint_is_repeatable_and_content_change_boundary_is_explicit() {
    let t = test_db("time_identity_fingerprint");
    let feed = seed_feed(&t.conn, "https://feed.example/rss");
    let same_a = rss(r#"<item><title>Same</title><description>same body</description></item>"#);
    let same_b = rss(r#"<item><title>Same</title><description>same body</description></item>"#);
    let changed = rss(r#"<item><title>Same</title><description>changed body</description></item>"#);

    assert_eq!(upsert_feed(&t.conn, feed, &same_a), 1);
    let before = article_state(&t.conn, only_id(&t.conn));
    assert_eq!(
        upsert_feed(&t.conn, feed, &same_b),
        0,
        "标题+正文相同必须复用同一行（确定性指纹）"
    );
    assert_eq!(article_state(&t.conn, before.id), before);

    // 已记录边界：无 id/url/date 时指纹包含正文；正文真正变化 → 新身份/新行。
    assert_eq!(
        upsert_feed(&t.conn, feed, &changed),
        1,
        "正文变化产生新行是明确边界，不得假装有完美身份"
    );
    let total: i64 = t
        .conn
        .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total, 2);

    // 同一份已变化内容连续刷新：复用 B 行，不继续增行。
    assert_eq!(upsert_feed(&t.conn, feed, &changed), 0);
    let after_repeat: i64 = t
        .conn
        .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
        .unwrap();
    assert_eq!(after_repeat, 2);

    // A→B→A：回到旧内容时复用 A 行；总行数仍等于两个不同内容指纹。
    assert_eq!(upsert_feed(&t.conn, feed, &same_a), 0);
    assert_eq!(article_state(&t.conn, before.id), before);
    let after_aba: i64 = t
        .conn
        .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
        .unwrap();
    assert_eq!(after_aba, 2);

    // 再切回 B 仍复用；只有出现第三个不同正文指纹时才会新增第三行。
    assert_eq!(upsert_feed(&t.conn, feed, &changed), 0);
    assert_eq!(
        t.conn
            .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
}

/// legacy_unknown 只在下一次真实源日期出现时升级；缺日期刷新不得猜测或回退。
#[test]
fn legacy_unknown_upgrades_only_when_real_source_date_appears() {
    let t = test_db("time_identity_legacy");
    let feed = seed_feed(&t.conn, "https://legacy.example/rss");
    t.conn
        .execute(
            "INSERT INTO articles
               (feed_id, guid, title, url, url_norm, published_at, fetched_at,
                first_seen_at, published_at_source)
             VALUES (?1, 'legacy-guid', 'Legacy', 'https://legacy.example/a',
                     'https://legacy.example/a', '2024-01-01T00:00:00+00:00',
                     '2025-01-01 00:00:00', '2025-01-01T00:00:00+00:00',
                     'legacy_unknown')",
            [feed],
        )
        .unwrap();
    let legacy_id = only_id(&t.conn);

    let undated = rss(r#"<item>
             <title>Legacy</title>
             <guid isPermaLink="false">legacy-guid</guid>
             <link>https://legacy.example/a</link>
             <description>body</description>
           </item>"#);
    assert_eq!(upsert_feed(&t.conn, feed, &undated), 0);
    let before = article_state(&t.conn, legacy_id);
    assert_eq!(before.published_at_source, "legacy_unknown");
    assert_eq!(before.published_at, "2024-01-01T00:00:00+00:00");
    assert_eq!(
        before.first_seen_at.as_deref(),
        Some("2025-01-01T00:00:00+00:00")
    );

    let dated = rss(r#"<item>
             <title>Legacy</title>
             <guid isPermaLink="false">legacy-guid</guid>
             <link>https://legacy.example/a</link>
             <description>body</description>
             <pubDate>Tue, 02 Jan 2024 03:04:05 GMT</pubDate>
           </item>"#);
    assert_eq!(upsert_feed(&t.conn, feed, &dated), 0);
    let upgraded = article_state(&t.conn, legacy_id);
    assert_eq!(upgraded.published_at_source, "feed");
    assert_eq!(upgraded.published_at, "2024-01-02T03:04:05+00:00");
    assert_eq!(upgraded.first_seen_at, before.first_seen_at);
}

/// title+date 仅是弱候选；正文不同时保留两条，重放同一输入仍幂等。
#[test]
fn title_date_identity_keeps_distinct_bodies_and_replays_idempotently() {
    let t = test_db("time_identity_title_date");
    let feed = seed_feed(&t.conn, "https://titledate.example/rss");
    let feed2 = seed_feed(&t.conn, "https://titledate2.example/rss");
    let first = rss(r#"<item>
             <title>Daily</title>
             <description>body one</description>
             <pubDate>Mon, 24 Aug 2026 11:00:00 GMT</pubDate>
           </item>"#);
    let changed = rss(r#"<item>
             <title>Daily</title>
             <description>body two</description>
             <pubDate>Mon, 24 Aug 2026 11:00:00 GMT</pubDate>
           </item>"#);

    assert_eq!(upsert_feed(&t.conn, feed, &first), 1);
    let before = article_state(&t.conn, only_id(&t.conn));
    assert!(
        before.guid.contains("payload:"),
        "持久弱键不再携带可补全的日期候选形式"
    );
    assert_eq!(upsert_feed(&t.conn, feed, &changed), 1);
    assert_eq!(upsert_feed(&t.conn, feed, &changed), 0);
    assert_eq!(upsert_feed(&t.conn, feed, &first), 0);
    let after = article_state(&t.conn, before.id);
    assert_eq!(after, before, "title+date 身份不随正文变化");
    let body: String = t
        .conn
        .query_row(
            "SELECT c.body_text FROM articles a JOIN contents c ON c.id = a.content_id
              WHERE a.id = ?1",
            [before.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(body, "body one", "弱身份不能覆盖不同正文");
    let total: i64 = t
        .conn
        .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total, 2);

    assert_eq!(upsert_feed(&t.conn, feed2, &changed), 1);
    let total: i64 = t
        .conn
        .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total, 3, "title+date 必须 feed-scoped");
}

/// 远程协议没有日期：首次见到兜底，重复拉取稳定，后续真实日期升级为 feed。
#[test]
fn remote_protocol_missing_date_uses_first_seen_and_upgrades_later() {
    let t = test_db("time_identity_remote_missing");
    let feed = seed_feed(&t.conn, "https://remote.example/rss");
    let make = |published_at: Option<&str>| db::NewArticle {
        guid: "remote-9001".into(),
        url: Some("https://remote.example/a".into()),
        title: "Remote".into(),
        author: None,
        summary: None,
        content_html: Some("<p>body</p>".into()),
        body_text: "body".into(),
        image_url: None,
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: published_at.map(str::to_string),
        source: "miniflux".into(),
    };
    let first = db::upsert_remote_article(&t.conn, feed, &make(None), Some(9001)).unwrap();
    assert!(first.inserted);
    let before = article_state(&t.conn, first.id);
    assert_eq!(before.published_at_source, "inferred_fetch");
    assert_eq!(
        before.first_seen_at.as_deref(),
        Some(before.published_at.as_str())
    );

    std::thread::sleep(std::time::Duration::from_millis(20));
    let again = db::upsert_remote_article(&t.conn, feed, &make(None), Some(9001)).unwrap();
    assert!(!again.inserted);
    assert_eq!(article_state(&t.conn, first.id), before);

    let dated = make(Some("2026-08-24T11:00:00+00:00"));
    db::upsert_remote_article(&t.conn, feed, &dated, Some(9001)).unwrap();
    let upgraded = article_state(&t.conn, first.id);
    assert_eq!(upgraded.published_at, "2026-08-24T11:00:00+00:00");
    assert_eq!(upgraded.published_at_source, "feed");
    assert_eq!(upgraded.first_seen_at, before.first_seen_at);
}

/// 内容指纹包含标题 + HTML + 正文；只有 HTML 变化、提取正文不变也必须形成
/// 不同身份，重复同 HTML 仍稳定复用。
#[test]
fn content_fingerprint_includes_html_not_only_extracted_text() {
    let t = test_db("time_identity_html");
    let feed = seed_feed(&t.conn, "https://html.example/rss");
    let plain = rss(r#"<item>
             <title>Same</title>
             <description>same body</description>
           </item>"#);
    let html = rss(r#"<item>
             <title>Same</title>
             <description><p>same body</p></description>
           </item>"#);

    assert_eq!(upsert_feed(&t.conn, feed, &plain), 1);
    assert_eq!(
        upsert_feed(&t.conn, feed, &html),
        1,
        "HTML-only 变化必须进入内容指纹"
    );
    assert_eq!(upsert_feed(&t.conn, feed, &html), 0);
    let total: i64 = t
        .conn
        .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total, 2);
}
