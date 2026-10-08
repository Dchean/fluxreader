//! 三段式刷新管线（refresh_feed_staged）的并发验证：
//! 自建慢速 HTTP server（每请求 sleep 300ms），4 个源并发抓取总耗时应显著小于
//! 串行（4×300ms）——证明 HTTP 在数据库锁外真正重叠。
//! 同时验证 304 分支保留旧条件头（写回旧 etag，不断条件 GET 链）。
//!
//! 全程本地回环 + 临时 DB，无需外部 server，可直接 `cargo test --test staged_refresh_e2e`。

use app_lib::db;
use app_lib::ingestion;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

mod common;

/// 直连条目的测试夹具（字段口径与生产 NewArticle 一致）。
fn direct_article(
    guid: &str,
    url: &str,
    title: &str,
    content_html: &str,
    body_text: &str,
) -> db::NewArticle {
    db::NewArticle {
        guid: guid.into(),
        url: Some(url.into()),
        title: title.into(),
        author: None,
        summary: None,
        content_html: Some(content_html.into()),
        body_text: body_text.into(),
        image_url: None,
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: Some("2026-01-01T00:00:00Z".into()),
        source: "direct".into(),
    }
}

/// 慢速 feed server：每个请求先 sleep 再回 RSS（item 数可配，用于 304 用例的 body 变体）。
/// 返回 (base_url, 命中计数)。
async fn spawn_slow_feed_server(delay_ms: u64, hits: Arc<AtomicUsize>) -> String {
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => return,
            };
            let hits = hits.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                // 读掉请求头（不关心内容）
                let _ = tokio::time::timeout(Duration::from_secs(2), socket.readable()).await;
                let _ = socket.try_read(&mut buf);
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                hits.fetch_add(1, Ordering::SeqCst);
                let body = r#"<?xml version="1.0"?>
<rss version="2.0"><channel>
<title>Slow Feed</title><link>http://127.0.0.1/</link><description>t</description>
<item><title>Item A</title><guid>a</guid><link>http://127.0.0.1/a</link></item>
</channel></rss>"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(resp.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    format!("http://{addr}/slow.xml")
}

fn temp_db(name: &str) -> std::path::PathBuf {
    let tmp = common::unique_db_path(&format!("staged_{name}"));
    let _ = std::fs::remove_file(&tmp);
    tmp
}

/// 测试临时库 RAII（OPT-003 R1）：声明必须位于 `Connection` 之前——
/// Rust 逆序析构保证先关连接、后删文件；panic 展开路径同样走 Drop 清理。
/// Drop 内忽略错误、绝不 panic（展开期二次 panic 会 abort 进程）。
/// 清理范围仅本 helper 新建的三个文件（.db/-wal/-shm），不触碰历史别次产物。
struct TempDb {
    path: std::path::PathBuf,
}

impl TempDb {
    fn new(name: &str) -> Self {
        let path = common::unique_db_path(&format!("staged_{name}"));
        // 唯一路径理论上无残留；若有同名（同进程重跑复用），先清本测试的文件
        let _ = std::fs::remove_file(&path);
        Self { path }
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// 本测试相关的三个 sidecar 路径：.db / -wal / -shm。
    fn files(&self) -> [std::path::PathBuf; 3] {
        [
            self.path.clone(),
            self.path.with_extension("db-wal"),
            self.path.with_extension("db-shm"),
        ]
    }

    /// 正常路径显式收尾：先关连接，再删除文件，并断言无残留。
    /// panic 路径不会走到这里，由 Drop 兜底（同一份删除逻辑）。
    fn finish(self, conn: rusqlite::Connection) {
        let files = self.files();
        drop(conn);
        drop(self);
        assert!(
            files.iter().all(|f| !f.exists()),
            "临时库文件未清理：{files:?}"
        );
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        for f in self.files() {
            let _ = std::fs::remove_file(f);
        }
    }
}

/// 4 源并发：每源 HTTP 300ms。三段式下 HTTP 在锁外重叠，
/// 总耗时应 < 1200ms（串行下限 4×300=1200ms + 锁排队会更长）。
/// 用宽松断言（< 1000ms）防 CI 抖动误报。
#[tokio::test]
async fn staged_refresh_http_overlaps_under_concurrency() {
    let hits = Arc::new(AtomicUsize::new(0));
    let url = spawn_slow_feed_server(300, hits.clone()).await;

    let tmp = temp_db("concurrent");
    let conn = db::open(&tmp).expect("open db");
    let db = Arc::new(Mutex::new(conn));
    let client = ingestion::build_client(30);

    {
        let conn = db.lock().await;
        db::create_folder(&conn, "并发", "article").unwrap();
        for i in 0..4 {
            db::insert_feed(
                &conn,
                &format!("{url}#{i}"),
                None,
                &format!("S{i}"),
                None,
                1,
                "inherit",
                false,
                false,
            )
            .unwrap();
        }
    }

    let start = Instant::now();
    let mut handles = Vec::new();
    for id in 1..=4i64 {
        let db = db.clone();
        let client = client.clone();
        handles.push(tokio::spawn(async move {
            ingestion::refresh_feed_staged(&db, &client, id, false).await
        }));
    }
    let mut new_total = 0;
    for h in handles {
        new_total += h.await.unwrap().unwrap();
    }
    let elapsed = start.elapsed();

    assert_eq!(new_total, 4, "each feed ingests 1 article");
    assert_eq!(hits.load(Ordering::SeqCst), 4, "all 4 feeds hit the server");
    assert!(
        elapsed < Duration::from_millis(1000),
        "4 concurrent 300ms fetches took {elapsed:?} — HTTP is being serialized (lock held during network IO?)"
    );

    let _ = std::fs::remove_file(&tmp);
}

/// 304 分支：先抓一次拿 etag 入库，再刷一次（服务器不回 304 也没关系——
/// 关键断言是 NotModified 分支的写回不破坏既有 etag）。
/// 这里直接构造 Fetched::NotModified 调 apply_refresh_result 验证：
/// DB 里的 etag 保持抓取后的值（写回 old_etag 而非 None）。
#[tokio::test]
async fn staged_refresh_304_keeps_conditional_headers() {
    let tmp = temp_db("etag");
    let conn = db::open(&tmp).expect("open db");
    {
        db::create_folder(&conn, "Etag", "article").unwrap();
        db::insert_feed(
            &conn,
            "http://127.0.0.1:9/x.xml",
            None,
            "E",
            None,
            1,
            "inherit",
            false,
            false,
        )
        .unwrap();
        db::set_feed_fetch_state(
            &conn,
            1,
            false,
            None,
            Some("W/\"abc\""),
            Some("Wed, 21 Oct 2026 07:28:00 GMT"),
        )
        .unwrap();
        // OPT-003：把 last_fetched_at 钉到固定旧值，验证 304 也会推进到当前时间
        conn.execute(
            "UPDATE feeds SET last_fetched_at = '2001-01-01 00:00:00' WHERE id = 1",
            [],
        )
        .unwrap();
    }

    let parsed = ingestion::ParsedFeed {
        title: None,
        site_url: None,
        icon: None,
        articles: Vec::new(),
    };
    ingestion::apply_refresh_result(
        &conn,
        1,
        &ingestion::Fetched::NotModified,
        &parsed,
        false,
        Some("W/\"abc\""),
        Some("Wed, 21 Oct 2026 07:28:00 GMT"),
    )
    .unwrap();

    let (etag, last_modified, failed, fetched_at): (
        Option<String>,
        Option<String>,
        i64,
        Option<String>,
    ) = conn
        .query_row(
            "SELECT etag, last_modified, fetch_failed, last_fetched_at FROM feeds WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        etag.as_deref(),
        Some("W/\"abc\""),
        "304 must keep existing etag"
    );
    assert!(
        last_modified.is_some(),
        "304 must keep existing last_modified"
    );
    assert_eq!(failed, 0, "304 is a success (clears failure state)");
    // datetime('now') 为 UTC 形态；与当前时刻同源比较（容忍分钟级时钟误差）
    let parsed_time = chrono::NaiveDateTime::parse_from_str(
        fetched_at
            .as_deref()
            .expect("304 must advance last_fetched_at"),
        "%Y-%m-%d %H:%M:%S",
    )
    .expect("last_fetched_at 为 datetime('now') 形态");
    let now = chrono::Utc::now().naive_utc();
    assert!(
        (now - parsed_time).num_seconds().abs() < 300,
        "304 必须把 last_fetched_at 推进到当前时间（实际: {fetched_at:?}）"
    );

    let _ = std::fs::remove_file(&tmp);
}

/// 失败分支：网络错误写回 fetch_failed + fetch_error，供 UI 错误标志与指数退避重试使用。
#[tokio::test]
async fn staged_refresh_failure_marks_feed() {
    let tmp = temp_db("fail");
    let conn = db::open(&tmp).expect("open db");
    let db = Arc::new(Mutex::new(conn));
    let client = ingestion::build_client(5); // 5s 超时；连接 127.0.0.1:1 立即拒绝

    {
        let conn = db.lock().await;
        db::create_folder(&conn, "Fail", "article").unwrap();
        db::insert_feed(
            &conn,
            "http://127.0.0.1:1/dead.xml",
            None,
            "D",
            None,
            1,
            "inherit",
            false,
            false,
        )
        .unwrap();
    }

    let result = ingestion::refresh_feed_staged(&db, &client, 1, false).await;
    assert!(result.is_err(), "dead address must error");

    let (failed, error, fail_count): (i64, Option<String>, i64) = {
        let conn = db.lock().await;
        conn.query_row(
            "SELECT fetch_failed, fetch_error, fail_count FROM feeds WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
    };
    assert_eq!(failed, 1, "dead feed marked failed");
    assert!(error.is_some(), "failure reason recorded");
    assert_eq!(fail_count, 1, "first failure increments backoff counter");

    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
OPT-003：Body 写回的原子性与全文保护回归
============================================================ */

/// OPT-003 反例①（故障注入，RED 锚）：给 articles 加 BEFORE INSERT 触发器，
/// 第二篇标题命中固定串时 RAISE(ABORT)。生产 apply_refresh_result 必须整体
/// 失败且不半提交——第一篇条目、源标题、etag/last_modified、
/// fail_count/last_fetched_at 全部保持旧值；去掉触发器后同一结果重跑必须
/// 完整成功，且 ETag 只在成功后才更新。
#[test]
fn staged_refresh_rolls_back_atomically_on_article_insert_failure() {
    let tmp = TempDb::new("atomic");
    let conn = db::open(tmp.path()).expect("open db");
    db::create_folder(&conn, "原子", "article").unwrap();
    db::insert_feed(
        &conn,
        "http://127.0.0.1:9/atomic.xml",
        None,
        "原始标题",
        None,
        1,
        "inherit",
        false,
        false,
    )
    .unwrap();
    // 旧失败态 + 旧条件头：写回若半提交，下面每个观察点都有具体旧值可比
    db::set_feed_fetch_state(
        &conn,
        1,
        true,
        Some("旧错误"),
        Some("W/\"old\""),
        Some("Mon, 01 Jan 2024 00:00:00 GMT"),
    )
    .unwrap();
    conn.execute(
        "UPDATE feeds SET last_fetched_at = '2001-01-01 00:00:00' WHERE id = 1",
        [],
    )
    .unwrap();
    // 已存文章（guid 与 RSS 内不重叠，保证两篇都走 INSERT 而非 UPDATE 分支）
    let seed = direct_article(
        "seed",
        "http://127.0.0.1/seed",
        "已存",
        "<p>已存</p>",
        "已存",
    );
    db::upsert_article_with_feed(&conn, 1, &seed, false).unwrap();

    // 真实解析：两篇不同 guid 的有效 RSS
    let rss = r#"<?xml version="1.0"?>
<rss version="2.0"><channel>
<title>原子源</title><link>http://127.0.0.1/</link><description>t</description>
<item><title>第一篇</title><guid>g1</guid><link>http://127.0.0.1/one</link><description>one</description></item>
<item><title>触发器命中</title><guid>g2</guid><link>http://127.0.0.1/two</link><description>two</description></item>
</channel></rss>"#;
    let parsed = ingestion::parse_feed(rss.as_bytes(), "http://127.0.0.1:9/atomic.xml").unwrap();
    assert_eq!(parsed.articles.len(), 2, "解析两篇不同 guid 的有效 RSS");
    assert_eq!(parsed.articles[0].guid, "g1");
    assert_eq!(parsed.articles[1].guid, "g2");

    // 注入：第二篇（g2 / 标题「触发器命中」）插入即 ABORT
    conn.execute_batch(
        "CREATE TRIGGER test_abort_second_article BEFORE INSERT ON articles
         WHEN NEW.title = '触发器命中'
         BEGIN SELECT RAISE(ABORT, 'inject: second article'); END;",
    )
    .unwrap();

    let fetched = ingestion::Fetched::Body {
        bytes: rss.as_bytes().to_vec(),
        content_type: Some("application/rss+xml".into()),
        etag: Some("W/\"new\"".into()),
        last_modified: Some("Tue, 02 Jan 2024 00:00:00 GMT".into()),
    };
    let err = ingestion::apply_refresh_result(
        &conn,
        1,
        &fetched,
        &parsed,
        false,
        Some("W/\"old\""),
        Some("Mon, 01 Jan 2024 00:00:00 GMT"),
    )
    .expect_err("触发器 ABORT 必须让整次写回失败");
    assert!(
        err.message.contains("inject"),
        "错误必须来自注入的触发器，实际: {err}"
    );

    // 回滚断言：第一篇不得半提交，源行不得带任何新值
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total, 1, "回滚后只应有预置文章；第一篇 g1 不得半提交");
    let first: i64 = conn
        .query_row("SELECT COUNT(*) FROM articles WHERE guid = 'g1'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(first, 0, "g1 必须随事务回滚（修复前会留下已提交的第一篇）");
    let (title, etag, lm): (String, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT title, etag, last_modified FROM feeds WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    let (failed, error, fail_count, retry, fetched_at): (
        i64,
        Option<String>,
        i64,
        Option<String>,
        Option<String>,
    ) = conn
        .query_row(
            "SELECT fetch_failed, fetch_error, fail_count, next_retry_at, last_fetched_at
             FROM feeds WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(title, "原始标题", "失败时源标题不得被新标题覆盖");
    assert_eq!(etag.as_deref(), Some("W/\"old\""), "失败时 etag 不得更新");
    assert_eq!(
        lm.as_deref(),
        Some("Mon, 01 Jan 2024 00:00:00 GMT"),
        "失败时 last_modified 不得更新"
    );
    assert_eq!(failed, 1, "失败时旧失败标记必须原样保留");
    assert_eq!(error.as_deref(), Some("旧错误"));
    assert_eq!(fail_count, 1, "失败时 fail_count 不得被清零");
    assert!(retry.is_some(), "失败时 next_retry_at 不得被清");
    assert_eq!(
        fetched_at.as_deref(),
        Some("2001-01-01 00:00:00"),
        "失败时 last_fetched_at 不得被推进"
    );

    // 去掉触发器，同一结果重跑：两篇均成功，ETag 只在成功后更新
    conn.execute_batch("DROP TRIGGER test_abort_second_article;")
        .unwrap();
    let new_count = ingestion::apply_refresh_result(
        &conn,
        1,
        &fetched,
        &parsed,
        false,
        Some("W/\"old\""),
        Some("Mon, 01 Jan 2024 00:00:00 GMT"),
    )
    .unwrap();
    assert_eq!(new_count, 2, "触发器移除后两篇均入库");
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total, 3, "预置 1 篇 + 本次 2 篇");
    let (title, etag, failed, fail_count, fetched_at): (
        String,
        Option<String>,
        i64,
        i64,
        Option<String>,
    ) = conn
        .query_row(
            "SELECT title, etag, fetch_failed, fail_count, last_fetched_at FROM feeds WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(title, "原子源", "成功后标题更新");
    assert_eq!(etag.as_deref(), Some("W/\"new\""), "成功后 etag 才更新");
    assert_eq!(failed, 0, "成功清失败标记");
    assert_eq!(fail_count, 0, "成功清零退避计数");
    assert_ne!(
        fetched_at.as_deref(),
        Some("2001-01-01 00:00:00"),
        "成功后 last_fetched_at 推进"
    );

    // 正常路径显式收尾：关连接 → 删临时库 → 断言无残留
    tmp.finish(conn);
}

/// OPT-003 R1 反例（写入边界 + guid 命中刷新）：真实全文写入边界
/// `update_article_fulltext` 必须 HTML/body_text 同源（旧实现只写 HTML+flag，
/// 纯文本残留 RSS 文本）；随后同 guid 直连刷新，RSS 摘要不得替换任何一列。
#[test]
fn staged_refresh_fulltext_boundary_and_guid_refresh_stay_same_source() {
    let tmp = TempDb::new("fulltext");
    let conn = db::open(tmp.path()).expect("open db");
    db::create_folder(&conn, "全文", "article").unwrap();
    db::insert_feed(
        &conn,
        "http://127.0.0.1:9/ft.xml",
        None,
        "全文源",
        None,
        1,
        "inherit",
        false,
        false,
    )
    .unwrap();

    let (aid, _) = db::upsert_article_with_feed(
        &conn,
        1,
        &direct_article(
            "ft-guid",
            "http://127.0.0.1/ft1",
            "标题一",
            "<p>旧RSS正文一</p>",
            "旧RSS正文一",
        ),
        false,
    )
    .unwrap();

    // 真实提取写入边界：HTML 与纯文本必须同一来源（RED 锚：旧实现残留 RSS 文本）
    db::update_article_fulltext(&conn, aid, "<p>提取的全文字一</p>", true).unwrap();
    let (html, body, flag): (Option<String>, String, i64) = conn
        .query_row(
            "SELECT content_html, body_text, fulltext_extracted FROM articles WHERE id = ?1",
            [aid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        html.as_deref(),
        Some("<p>提取的全文字一</p>"),
        "提取写入必须覆盖正文 HTML"
    );
    assert_eq!(
        body, "提取的全文字一",
        "提取写入必须同源更新 body_text（不得残留旧 RSS 文本）"
    );
    assert_eq!(flag, 1, "提取标志置位");

    // 直连刷新：同 guid、RSS 只带摘要 → 两列都不得被摘要替换
    let parsed = ingestion::ParsedFeed {
        title: Some("全文源".into()),
        site_url: None,
        icon: None,
        articles: vec![direct_article(
            "ft-guid",
            "http://127.0.0.1/ft1",
            "标题一",
            "<p>新RSS摘要一</p>",
            "新RSS摘要一",
        )],
    };
    let fetched = ingestion::Fetched::Body {
        bytes: Vec::new(),
        content_type: None,
        etag: None,
        last_modified: None,
    };
    ingestion::apply_refresh_result(&conn, 1, &fetched, &parsed, false, None, None).unwrap();

    let (html, body, flag): (Option<String>, String, i64) = conn
        .query_row(
            "SELECT content_html, body_text, fulltext_extracted FROM articles WHERE id = ?1",
            [aid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        html.as_deref(),
        Some("<p>提取的全文字一</p>"),
        "同 guid 刷新：提取正文不得被 RSS 摘要替换"
    );
    assert_eq!(
        body, "提取的全文字一",
        "同 guid 刷新：body_text 必须保持提取全文文本"
    );
    assert_eq!(flag, 1, "同 guid 刷新：全文标志必须与正文状态一致");

    tmp.finish(conn);
}

/// OPT-003 R1 反例（历史 flag1 错配 + 改 guid 同 URL 刷新）：旧版提取只写
/// HTML+flag、body_text 残留 RSS 文本的历史行，刷新保护必须按当前保留的
/// HTML 重算 body_text（恢复同源），而不是冻结错配值。
#[test]
fn staged_refresh_recomputes_stale_body_text_on_historical_fulltext_refresh() {
    let tmp = TempDb::new("fulltext_history");
    let conn = db::open(tmp.path()).expect("open db");
    db::create_folder(&conn, "历史", "article").unwrap();
    db::insert_feed(
        &conn,
        "http://127.0.0.1:9/ft2.xml",
        None,
        "历史源",
        None,
        1,
        "inherit",
        false,
        false,
    )
    .unwrap();

    let (aid, _) = db::upsert_article_with_feed(
        &conn,
        1,
        &direct_article(
            "ft-old",
            "http://127.0.0.1/ft2",
            "标题二",
            "<p>旧RSS正文二</p>",
            "旧RSS正文二",
        ),
        false,
    )
    .unwrap();
    // 模拟历史错配：只写 HTML+flag（旧 update_article_fulltext 的写入形态），
    // body_text 保持旧 RSS 文本 —— 不允许刷新后继续冻在这个错配上
    conn.execute(
        "UPDATE articles SET content_html = '<p>提取的全文字二</p>', fulltext_extracted = 1 WHERE id = ?1",
        [aid],
    )
    .unwrap();

    // 直连刷新：guid 变化但规范化 URL 命中，RSS 只带摘要
    let parsed = ingestion::ParsedFeed {
        title: Some("历史源".into()),
        site_url: None,
        icon: None,
        articles: vec![direct_article(
            "ft-new",
            "http://127.0.0.1/ft2",
            "标题二",
            "<p>新RSS摘要二</p>",
            "新RSS摘要二",
        )],
    };
    let fetched = ingestion::Fetched::Body {
        bytes: Vec::new(),
        content_type: None,
        etag: None,
        last_modified: None,
    };
    ingestion::apply_refresh_result(&conn, 1, &fetched, &parsed, false, None, None).unwrap();

    let (html, body, flag): (Option<String>, String, i64) = conn
        .query_row(
            "SELECT content_html, body_text, fulltext_extracted FROM articles WHERE id = ?1",
            [aid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        html.as_deref(),
        Some("<p>提取的全文字二</p>"),
        "改 guid 同 URL 刷新：提取正文不得被 RSS 摘要替换"
    );
    assert_eq!(
        body, "提取的全文字二",
        "历史 flag1 错配：刷新必须按保留的 HTML 重算 body_text"
    );
    assert_eq!(flag, 1, "改 guid 同 URL 刷新：全文标志保持");

    tmp.finish(conn);
}

/// OPT-003 反例③：空有效 feed（0 条目）是成功——清失败标记、推进时间、
/// 更新标题，不得误报失败。
#[test]
fn staged_refresh_empty_valid_feed_is_success_not_failure() {
    let tmp = TempDb::new("empty");
    let conn = db::open(tmp.path()).expect("open db");
    db::create_folder(&conn, "空", "article").unwrap();
    db::insert_feed(
        &conn,
        "http://127.0.0.1:9/empty.xml",
        None,
        "旧名",
        None,
        1,
        "inherit",
        false,
        false,
    )
    .unwrap();
    db::set_feed_fetch_state(&conn, 1, true, Some("旧错误"), Some("W/\"old\""), None).unwrap();
    conn.execute(
        "UPDATE feeds SET last_fetched_at = '2001-01-01 00:00:00' WHERE id = 1",
        [],
    )
    .unwrap();

    let parsed = ingestion::ParsedFeed {
        title: Some("空源".into()),
        site_url: None,
        icon: None,
        articles: Vec::new(),
    };
    let fetched = ingestion::Fetched::Body {
        bytes: Vec::new(),
        content_type: None,
        etag: None,
        last_modified: None,
    };
    let n =
        ingestion::apply_refresh_result(&conn, 1, &fetched, &parsed, false, None, None).unwrap();
    assert_eq!(n, 0, "空 feed 是 0 条新增，不是错误");

    let (failed, error, title, fetched_at): (i64, Option<String>, String, Option<String>) = conn
        .query_row(
            "SELECT fetch_failed, fetch_error, title, last_fetched_at FROM feeds WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(failed, 0, "空有效 feed 必须清失败标记");
    assert_eq!(error, None, "空有效 feed 不得留错误说明");
    assert_eq!(title, "空源", "空有效 feed 的标题照常更新");
    assert_ne!(
        fetched_at.as_deref(),
        Some("2001-01-01 00:00:00"),
        "空有效 feed 照常推进 last_fetched_at"
    );

    tmp.finish(conn);
}

/// OPT-003 反例③：dedup 开关行为保持——开启时跨源同 URL 仍被拦并记账墓碑，
/// 关闭时按既有语义入库；事务化不得改变去重语义。
#[test]
fn staged_refresh_dedup_flag_still_blocks_cross_feed_duplicate() {
    let tmp = TempDb::new("dedup");
    let conn = db::open(tmp.path()).expect("open db");
    db::create_folder(&conn, "D", "article").unwrap();
    db::insert_feed(
        &conn,
        "http://127.0.0.1:9/a.xml",
        None,
        "A",
        None,
        1,
        "inherit",
        false,
        false,
    )
    .unwrap();
    db::insert_feed(
        &conn,
        "http://127.0.0.1:9/b.xml",
        None,
        "B",
        None,
        1,
        "inherit",
        false,
        false,
    )
    .unwrap();

    let (kept_aid, _) = db::upsert_article_with_feed(
        &conn,
        1,
        &direct_article("a1", "https://d.example/x", "A 篇", "<p>A</p>", "A"),
        false,
    )
    .unwrap();

    let parsed = ingestion::ParsedFeed {
        title: Some("B".into()),
        site_url: None,
        icon: None,
        articles: vec![direct_article(
            "b1",
            "https://d.example/x",
            "B 篇",
            "<p>B</p>",
            "B",
        )],
    };
    let fetched = ingestion::Fetched::Body {
        bytes: Vec::new(),
        content_type: None,
        etag: None,
        last_modified: None,
    };

    let n = ingestion::apply_refresh_result(&conn, 2, &fetched, &parsed, true, None, None).unwrap();
    assert_eq!(n, 0, "dedup 开启时同 URL 不得新增");
    let in_b: i64 = conn
        .query_row("SELECT COUNT(*) FROM articles WHERE feed_id = 2", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(in_b, 0, "被去重的条目不得落库");
    let (url, kept): (String, i64) = conn
        .query_row("SELECT url, kept_aid FROM deduped_urls", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(url, db::normalize_url("https://d.example/x"));
    assert_eq!(kept, kept_aid, "墓碑必须指向保留的那篇");

    let n2 =
        ingestion::apply_refresh_result(&conn, 2, &fetched, &parsed, false, None, None).unwrap();
    assert_eq!(n2, 1, "dedup 关闭时同 URL 按既有语义入库");
    let in_b2: i64 = conn
        .query_row("SELECT COUNT(*) FROM articles WHERE feed_id = 2", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(in_b2, 1);

    tmp.finish(conn);
}
