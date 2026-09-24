//! 封面补全（REQ-106①）回归：负缓存时序、候选窗口推进、同 URL 分组与「只填空」语义。
//!
//! 缺陷（修前）：`scheduler.rs` 每轮只取 `published_at DESC` 的前 20 条候选，
//! 且在**发请求之前**就把 URL 写进进程内 tried 集合。最新 20 条候选的文章页
//! 一旦全部拿不到 og:image（真实库实测：这 20 条全部属于实测 403 的同一域名），
//! `targets` 变空后循环只 sleep 并重复取同一批 ⇒ 第 21 条及以后**在本进程内
//! 永不被尝试**（真实库 52 条候选中 32 条从未被请求）。
//!
//! 本文件用本地假文章站做确定性复现：最新 20 条候选的页面返回 403，其余返回
//! 带 `og:image` 的页面。断言「第 21 条起的候选在后续轮次被尝试并补上封面」。
//! 修前：第二轮 targets 仍为空 ⇒ 永不补全（断言失败）；修后：通过。
//!
//! 运行：cargo test --test cover_backfill_e2e

use app_lib::db::{self, NewArticle};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

/// 假文章站：按路径返回 403（/fail/*）或带 og:image 的 200 页面（/ok/*），
/// 并统计每个路径被请求的次数（用于断言负缓存与「一轮一次」语义）。
struct ArticleSite {
    base: String,
    hits: Arc<Mutex<HashMap<String, usize>>>,
}

impl ArticleSite {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock article site");
        let port = listener.local_addr().unwrap().port();
        let hits: Arc<Mutex<HashMap<String, usize>>> = Arc::new(Mutex::new(HashMap::new()));
        let hits_bg = hits.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let hits = hits_bg.clone();
                std::thread::spawn(move || {
                    let mut buf = [0u8; 4096];
                    let n = match stream.read(&mut buf) {
                        Ok(n) if n > 0 => n,
                        _ => return,
                    };
                    let head = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                    *hits.lock().unwrap().entry(path.clone()).or_insert(0) += 1;
                    let (status, body) = if path.starts_with("/ok/") {
                        let name = path.trim_start_matches("/ok/").to_string();
                        (
                            "200 OK",
                            format!(
                                "<html><head><meta property=\"og:image\" content=\"https://img.example.com/{name}.jpg\"></head><body>x</body></html>"
                            ),
                        )
                    } else if path.starts_with("/fail/") {
                        ("403 Forbidden", "<html>no</html>".to_string())
                    } else {
                        ("404 Not Found", "<html>nope</html>".to_string())
                    };
                    let resp = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.flush();
                });
            }
        });
        Self {
            base: format!("http://127.0.0.1:{port}"),
            hits,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    fn hits_of(&self, path: &str) -> usize {
        *self.hits.lock().unwrap().get(path).unwrap_or(&0)
    }
}

fn temp_db(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "fluxreader_cover_backfill_{name}_{}_{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

fn folder(conn: &rusqlite::Connection) -> i64 {
    db::create_folder(conn, "封面", "article").unwrap()
}

fn feed(conn: &rusqlite::Connection, folder_id: i64, url: &str, title: &str) -> i64 {
    db::insert_feed(
        conn, url, None, title, None, folder_id, "inherit", false, false,
    )
    .unwrap()
}

/// 造一条文章；`index` 同时决定发布时间（越大越旧）与 guid。
fn article(
    conn: &rusqlite::Connection,
    feed_id: i64,
    index: i64,
    url: &str,
    image_url: Option<&str>,
) -> i64 {
    article_with_source(conn, feed_id, index, url, image_url, "direct")
}

fn article_with_source(
    conn: &rusqlite::Connection,
    feed_id: i64,
    index: i64,
    url: &str,
    image_url: Option<&str>,
    source: &str,
) -> i64 {
    let a = NewArticle {
        guid: format!("cover-{feed_id}-{index}"),
        url: Some(url.to_string()),
        title: format!("第 {index} 篇"),
        author: None,
        summary: None,
        content_html: Some("<p>正文无图</p>".into()),
        body_text: "正文无图".into(),
        image_url: image_url.map(str::to_string),
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: Some((chrono::Utc::now() - chrono::Duration::minutes(index)).to_rfc3339()),
        source: source.into(),
    };
    db::upsert_article_with_feed(conn, feed_id, &a, false)
        .unwrap()
        .0
}

/// 失效封面纠正通道：只清空 direct 源仍持有相同 URL 的非空封面，且清空后
/// 该文章必须重新进入补全队列并能由下一轮写回新封面。
#[tokio::test]
async fn broken_cover_report_clears_matching_direct_cover_and_backfills_it() {
    let site = ArticleSite::start();
    let path = temp_db("broken_cover");
    let conn = db::open(&path).expect("open db");
    let f = feed(&conn, folder(&conn), "http://cover.test/broken.xml", "源");
    let page = site.url("/ok/replacement");
    let stale = "https://img.example.com/stale.jpg";
    let id = article(&conn, f, 0, &page, Some(stale));
    let neighbor = article(&conn, f, 2, &site.url("/ok/neighbor"), Some(stale));
    let empty_id = article(&conn, f, 1, &site.url("/ok/empty"), None);
    let db = Arc::new(tokio::sync::Mutex::new(conn));

    {
        let conn = db.lock().await;
        conn.execute(
            "UPDATE articles SET title = '保留标题', is_read = 1, is_starred = 1 WHERE id = ?1",
            [id],
        )
        .unwrap();
        let target_before = article_snapshot(&conn, id);
        let neighbor_before = article_snapshot(&conn, neighbor);
        assert_eq!(
            db::clear_article_cover_if_matches(&conn, id, "https://img.example.com/other.jpg")
                .unwrap(),
            0,
            "URL 不一致时不得清空封面"
        );
        assert_eq!(
            db::clear_article_cover_if_matches(&conn, empty_id, &page).unwrap(),
            0,
            "本已无封面的文章不得被当作成功清空"
        );
        assert_eq!(
            conn.query_row(
                "SELECT image_url FROM articles WHERE id = ?1",
                [id],
                |row| row.get::<_, Option<String>>(0),
            )
            .unwrap(),
            Some(stale.to_string())
        );
        assert_eq!(
            db::clear_article_cover_if_matches(&conn, i64::MAX, stale).unwrap(),
            0,
            "不存在的文章不得报告清空成功"
        );
        assert_eq!(
            db::clear_article_cover_if_matches(&conn, id, stale).unwrap(),
            1,
            "URL 完全匹配的 direct 封面应被清空"
        );
        assert_eq!(
            article_snapshot(&conn, id),
            ArticleSnapshot {
                image_url: None,
                ..target_before
            },
            "清空封面只能改变 image_url，目标文章其它字段必须保持不变"
        );
        assert_eq!(
            article_snapshot(&conn, neighbor),
            neighbor_before,
            "即使其它文章持有相同封面 URL，也不得被一并清空或改变其它字段"
        );
        assert_eq!(
            db::clear_article_cover_if_matches(&conn, id, stale).unwrap(),
            0,
            "重复上报空值不得再次报告清空"
        );
    }
    assert_eq!(cover_of(&db, id).await, None);

    let http = app_lib::ingestion::build_client(10);
    let tried = new_tried();
    let filled = app_lib::scheduler::cover_backfill_round(&db, &http, &tried).await;
    assert_eq!(filled, 2, "清空的封面和原本无封面的文章都应被补全");
    assert_eq!(
        cover_of(&db, id).await.as_deref(),
        Some("https://img.example.com/replacement.jpg")
    );
    let remaining = {
        let conn = db.lock().await;
        db::articles_without_cover(&conn, 50, 0).unwrap()
    };
    assert!(remaining.is_empty(), "补全后候选队列应为空：{remaining:?}");
}

/// Miniflux 文章不进入封面补全队列，报告图片失败也不得把其已有封面清掉。
#[tokio::test]
async fn broken_cover_report_keeps_miniflux_cover() {
    let path = temp_db("broken_miniflux");
    let conn = db::open(&path).expect("open db");
    let f = feed(
        &conn,
        folder(&conn),
        "http://cover.test/miniflux.xml",
        "远端源",
    );
    let url = "https://img.example.com/miniflux.jpg";
    let id = article_with_source(
        &conn,
        f,
        0,
        "https://article.example.com/miniflux",
        Some(url),
        "miniflux",
    );

    assert_eq!(
        db::clear_article_cover_if_matches(&conn, id, url).unwrap(),
        0,
        "非 direct 源不得清空封面"
    );
    assert_eq!(
        conn.query_row(
            "SELECT image_url, source FROM articles WHERE id = ?1",
            [id],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, String>(1)?)),
        )
        .unwrap(),
        (Some(url.to_string()), "miniflux".to_string())
    );
}

async fn cover_of(db: &Arc<tokio::sync::Mutex<rusqlite::Connection>>, id: i64) -> Option<String> {
    let conn = db.lock().await;
    conn.query_row(
        "SELECT image_url FROM articles WHERE id = ?1",
        rusqlite::params![id],
        |r| r.get(0),
    )
    .unwrap()
}

#[derive(Debug, PartialEq)]
struct ArticleSnapshot {
    image_url: Option<String>,
    feed_id: i64,
    url: Option<String>,
    title: String,
    source: String,
    is_read: i64,
    is_starred: i64,
    content_html: Option<String>,
}

fn article_snapshot(conn: &rusqlite::Connection, id: i64) -> ArticleSnapshot {
    conn.query_row(
        "SELECT image_url, feed_id, url, title, source, is_read, is_starred, content_html
         FROM articles WHERE id = ?1",
        [id],
        |row| {
            Ok(ArticleSnapshot {
                image_url: row.get(0)?,
                feed_id: row.get(1)?,
                url: row.get(2)?,
                title: row.get(3)?,
                source: row.get(4)?,
                is_read: row.get(5)?,
                is_starred: row.get(6)?,
                content_html: row.get(7)?,
            })
        },
    )
    .unwrap()
}

fn new_tried() -> Arc<tokio::sync::Mutex<std::collections::HashSet<String>>> {
    Arc::new(tokio::sync::Mutex::new(std::collections::HashSet::new()))
}

/// 菜单：最新 `failing` 条候选（/fail/*）全部失败，其余（/ok/*）有图。
fn menu(site: &ArticleSite, total: usize, failing: usize) -> Vec<String> {
    (0..total)
        .map(|i| {
            if i < failing {
                site.url(&format!("/fail/{i}"))
            } else {
                site.url(&format!("/ok/{i}"))
            }
        })
        .collect()
}

/// 窗口推进：最新 20 条候选全失败后，第 21 条起仍必须被尝试并补上封面。
#[tokio::test]
async fn cover_backfill_advances_window_past_failing_candidates() {
    let site = ArticleSite::start();
    let path = temp_db("window");
    let conn = db::open(&path).expect("open db");
    let f = feed(
        &conn,
        folder(&conn),
        "http://cover.test/feed.xml",
        "封面测试源",
    );
    let urls = menu(&site, 25, 20);
    let ids: Vec<i64> = urls
        .iter()
        .enumerate()
        .map(|(i, u)| article(&conn, f, i as i64, u, None))
        .collect();
    let db = Arc::new(tokio::sync::Mutex::new(conn));
    let http = app_lib::ingestion::build_client(10);
    let tried = new_tried();

    let filled_first = app_lib::scheduler::cover_backfill_round(&db, &http, &tried).await;
    assert_eq!(filled_first, 0, "最新 20 条候选的文章页都拿不到 og:image");

    let filled_second = app_lib::scheduler::cover_backfill_round(&db, &http, &tried).await;
    assert!(
        filled_second > 0,
        "窗口必须推进到第 21 条起的候选：修前第二轮 targets 为空，老候选在本进程内永不被尝试"
    );

    // 第 21 条（下标 20）必须真的拿到封面
    assert_eq!(
        cover_of(&db, ids[20]).await.as_deref(),
        Some("https://img.example.com/20.jpg"),
        "第 21 条候选应被尝试并写入 og:image 封面"
    );

    // 失败候选的负缓存仍然有效：两轮之后 /fail/0 只请求过一次（不反复轰炸源站）
    assert_eq!(
        site.hits_of("/fail/0"),
        1,
        "已尝试过的失败候选本进程内不重复请求"
    );

    // 稳态：失败项都在 tried 中、有图项已离开队列 ⇒ 第三轮不再发起请求
    let before: usize = (0..25).map(|i| site.hits_of(&format!("/ok/{i}"))).sum();
    let filled_third = app_lib::scheduler::cover_backfill_round(&db, &http, &tried).await;
    let after: usize = (0..25).map(|i| site.hits_of(&format!("/ok/{i}"))).sum();
    assert_eq!(filled_third, 0, "第三轮没有新可补的候选");
    assert_eq!(before, after, "第三轮不应重复请求已尝试过的候选");
}

/// 同一 URL 被两个源各收录一篇：一轮只请求一次页面，但**两篇都要写封面**
/// （只写第一篇会让第二篇永久空缺——该 URL 已进负缓存，本进程内不会重试）。
#[tokio::test]
async fn cover_backfill_fills_all_rows_sharing_a_url_in_one_request() {
    let site = ArticleSite::start();
    let path = temp_db("dedupe");
    let conn = db::open(&path).expect("open db");
    let folder_id = folder(&conn);
    let feed_a = feed(&conn, folder_id, "http://cover.test/a.xml", "源A");
    let feed_b = feed(&conn, folder_id, "http://cover.test/b.xml", "源B");
    let shared = site.url("/ok/shared");
    let other = site.url("/ok/other");
    let a1 = article(&conn, feed_a, 0, &shared, None);
    let a2 = article(&conn, feed_a, 1, &other, None);
    let b1 = article(&conn, feed_b, 2, &shared, None);
    let db = Arc::new(tokio::sync::Mutex::new(conn));
    let http = app_lib::ingestion::build_client(10);
    let tried = new_tried();

    let filled = app_lib::scheduler::cover_backfill_round(&db, &http, &tried).await;
    assert_eq!(filled, 3, "两个源的同 URL 条目 + 另一篇都要补上封面");
    assert_eq!(
        site.hits_of("/ok/shared"),
        1,
        "同一 URL 一轮只请求一次（不同源收录同一篇原文）"
    );

    assert_eq!(
        cover_of(&db, a1).await.as_deref(),
        Some("https://img.example.com/shared.jpg")
    );
    assert_eq!(
        cover_of(&db, a2).await.as_deref(),
        Some("https://img.example.com/other.jpg")
    );
    assert_eq!(
        cover_of(&db, b1).await.as_deref(),
        Some("https://img.example.com/shared.jpg"),
        "同 URL 的第二篇也必须写入封面，而不是等下次启动"
    );

    // 全部补上后队列为空；第二轮不再请求
    let before = site.hits_of("/ok/shared") + site.hits_of("/ok/other");
    let again = app_lib::scheduler::cover_backfill_round(&db, &http, &tried).await;
    assert_eq!(again, 0);
    assert_eq!(
        before,
        site.hits_of("/ok/shared") + site.hits_of("/ok/other")
    );
}

/// 空串封面（image_url = ''）同样在候选队列里，必须能被补上；已有封面不被覆盖。
#[tokio::test]
async fn cover_backfill_fills_empty_string_cover_and_keeps_existing() {
    let site = ArticleSite::start();
    let path = temp_db("empty");
    let conn = db::open(&path).expect("open db");
    let f = feed(&conn, folder(&conn), "http://cover.test/c.xml", "源C");
    let empty = article(&conn, f, 0, &site.url("/ok/empty"), Some(""));
    let kept = article(
        &conn,
        f,
        1,
        &site.url("/ok/kept"),
        Some("https://already.example.com/keep.jpg"),
    );
    let db = Arc::new(tokio::sync::Mutex::new(conn));
    let http = app_lib::ingestion::build_client(10);
    let tried = new_tried();

    let filled = app_lib::scheduler::cover_backfill_round(&db, &http, &tried).await;
    assert_eq!(filled, 1, "只有空串封面那条需要补");
    assert_eq!(
        cover_of(&db, empty).await.as_deref(),
        Some("https://img.example.com/empty.jpg"),
        "空串封面必须能被补上（COALESCE 只判 NULL，需要 NULLIF）"
    );
    assert_eq!(
        cover_of(&db, kept).await.as_deref(),
        Some("https://already.example.com/keep.jpg"),
        "已有封面绝不覆盖（只填空）"
    );
    assert_eq!(
        site.hits_of("/ok/kept"),
        0,
        "已有封面的条目不在候选队列里，不会被请求"
    );

    let remaining = {
        let conn = db.lock().await;
        db::articles_without_cover(&conn, 50, 0).unwrap()
    };
    assert!(
        remaining.is_empty(),
        "补全后队列应为空，实际仍有 {remaining:?}"
    );
}
