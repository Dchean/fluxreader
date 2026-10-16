//! 封面补全回归：REQ-106① 负缓存时序/候选窗口推进/同 URL 分组/只填空 +
//! OPT-017D 覆盖面（服务端无图文章）/预算（失败缓存上限）/生命周期（可终止）/
//! 可观测（读写失败必须记录、不得压成空集合或 0 后标为已处理）。
//!
//! 历史缺陷（REQ-106①，修前）：`scheduler.rs` 每轮只取 `published_at DESC`
//! 的前 20 条候选，且在**发请求之前**就把 URL 写进进程内 tried 集合。最新 20
//! 条候选的文章页一旦全部拿不到 og:image（真实库实测：这 20 条全部属于实测
//! 403 的同一域名），`targets` 变空后循环只 sleep 并重复取同一批 ⇒ 第 21 条
//! 及以后**在本进程内永不被尝试**（真实库 52 条候选中 32 条从未被请求）。
//! 本文件用本地假文章站做确定性复现。
//!
//! OPT-017D 缺口：各后端（direct 走 `ingestion/parse.rs`；greader/miniflux、
//! fever 走 `sync/entries.rs`）入库时都用正文第一图兜底，但**正文里没有图片的
//! 服务端文章**既没有封面、又（修前）不在补全候选里（`articles_without_cover`
//! 只选 `source='direct'`）——本文件用真实管线夹具锁定两类现状，并断言扩展后
//! 服务端无图文章进入候选、由补全抓文章页 og:image 补上；已有封面（含服务端
//! 正文首图封面）不被重复抓取、不被覆盖。
//!
//! 运行：cargo test --test cover_backfill_e2e

use app_lib::db::{self, NewArticle};
use app_lib::scheduler::{cover_backfill_round, CoverBackfillState, ShutdownToken};
use mock_greader::MockGReader;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod common;
mod mock_greader;

/* ============================================================
进程级日志捕获：断言「失败被记录」而不是被静默当成已处理
============================================================ */

struct CaptureLogger;

static LOG_LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static LOG_INIT: std::sync::Once = std::sync::Once::new();

impl log::Log for CaptureLogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }
    fn log(&self, record: &log::Record) {
        // 只捕获本 crate（app_lib::*，即生产代码）的日志：第三方
        // （html5ever/reqwest 等）的 Debug 输出不参与断言，也不污染失败输出。
        if !(record.target() == "app_lib" || record.target().starts_with("app_lib::")) {
            return;
        }
        let line = format!("[{}] {}", record.level(), record.args());
        if let Ok(mut lines) = LOG_LINES.lock() {
            lines.push(line);
        }
    }
    fn flush(&self) {}
}

/// 安装进程级捕获 logger（Once；测试二进制内全局一次）。
fn install_log_capture() {
    LOG_INIT.call_once(|| {
        let _ = log::set_logger(&CaptureLogger);
        log::set_max_level(log::LevelFilter::Debug);
    });
}

fn log_mark() -> usize {
    LOG_LINES.lock().unwrap().len()
}

fn logs_since(mark: usize) -> Vec<String> {
    let lines = LOG_LINES.lock().unwrap();
    lines[mark.min(lines.len())..].to_vec()
}

/// 断言 mark 之后出现包含 needle 的日志行（失败必须可诊断）。
fn assert_logged_since(mark: usize, needle: &str) -> Vec<String> {
    let lines = logs_since(mark);
    assert!(
        lines.iter().any(|l| l.contains(needle)),
        "日志中应出现 {needle:?}；实际日志：{lines:#?}"
    );
    lines
}

/* ============================================================
假文章站：按路径返回不同响应，并统计每个路径被请求的次数
============================================================ */

/// - `/ok/*`    → 200，页面带 `og:image = https://img.example.com/{name}.jpg`
/// - `/fail/*`  → 403（抓取失败）
/// - `/nom/*`   → 200，页面**没有** og:image（抓取成功但无图）
/// - `/slow/*`  → 3s 后 200 + og:image（用于可控终止时序）
/// - 其它       → 404
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
                    // 计数发生在响应之前：慢路由的「已开始请求」也能被观察。
                    *hits.lock().unwrap().entry(path.clone()).or_insert(0) += 1;
                    let og_page = |name: &str| {
                        format!(
                            "<html><head><meta property=\"og:image\" content=\"https://img.example.com/{name}.jpg\"></head><body>x</body></html>"
                        )
                    };
                    let (status, body) = if let Some(name) = path.strip_prefix("/ok/") {
                        ("200 OK", og_page(name))
                    } else if path.starts_with("/fail/") {
                        ("403 Forbidden", "<html>no</html>".to_string())
                    } else if path.starts_with("/nom/") {
                        (
                            "200 OK",
                            "<html><head><title>no og image</title></head><body>x</body></html>"
                                .to_string(),
                        )
                    } else if let Some(name) = path.strip_prefix("/slow/") {
                        std::thread::sleep(Duration::from_secs(3));
                        ("200 OK", og_page(name))
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

    /// 所有路径被请求次数之和（用于「不再发起新请求」类断言）。
    fn hits_total(&self) -> usize {
        self.hits.lock().unwrap().values().sum()
    }
}

/* ============================================================
夹具与工具
============================================================ */

fn temp_db(name: &str) -> std::path::PathBuf {
    let path = common::unique_db_path(&format!("cover_backfill_{name}"));
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

async fn cover_of(db: &Arc<tokio::sync::Mutex<rusqlite::Connection>>, id: i64) -> Option<String> {
    let conn = db.lock().await;
    conn.query_row(
        "SELECT image_url FROM articles WHERE id = ?1",
        rusqlite::params![id],
        |r| r.get(0),
    )
    .unwrap()
}

/// 按原始 url 查询 (id, image_url, source)。
async fn row_by_url(
    db: &Arc<tokio::sync::Mutex<rusqlite::Connection>>,
    url: &str,
) -> (i64, Option<String>, String) {
    let conn = db.lock().await;
    conn.query_row(
        "SELECT id, image_url, source FROM articles WHERE url = ?1",
        rusqlite::params![url],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .unwrap()
}

/// 候选集合（一次取全：测试库规模小，limit 取 500 与分页语义无关）。
async fn candidates(db: &Arc<tokio::sync::Mutex<rusqlite::Connection>>) -> Vec<(i64, String)> {
    let conn = db.lock().await;
    db::articles_without_cover(&conn, 500, 0).unwrap()
}

fn new_state() -> Arc<tokio::sync::Mutex<CoverBackfillState>> {
    Arc::new(tokio::sync::Mutex::new(CoverBackfillState::new()))
}

fn new_state_with_cap(cap: usize) -> Arc<tokio::sync::Mutex<CoverBackfillState>> {
    Arc::new(tokio::sync::Mutex::new(CoverBackfillState::with_capacity(
        cap,
    )))
}

fn new_shutdown() -> ShutdownToken {
    ShutdownToken::new()
}

fn new_http() -> reqwest::Client {
    app_lib::ingestion::build_client(10)
}

/// 候选集合里的 id 列表。
fn ids_of(candidates: &[(i64, String)]) -> Vec<i64> {
    candidates.iter().map(|(id, _)| *id).collect()
}

/* ============================================================
R4：失效封面纠正通道（既有行为）
============================================================ */

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

    let http = new_http();
    let state = new_state();
    let shutdown = new_shutdown();
    let filled = cover_backfill_round(&db, &http, &state, &shutdown).await;
    assert_eq!(filled, 2, "清空的封面和原本无封面的文章都应被补全");
    assert_eq!(
        cover_of(&db, id).await.as_deref(),
        Some("https://img.example.com/replacement.jpg")
    );
    let remaining = candidates(&db).await;
    assert!(remaining.is_empty(), "补全后候选队列应为空：{remaining:?}");
}

/// 失效上报通道对**服务端来源**仍保守不清空（有意边界，OPT-017D 保留）：
/// 服务端文章的候选覆盖由下方矩阵测试锁定，本测试只锁 clear 的边界不回归。
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
        "SELECT a.image_url, a.feed_id, a.url, a.title, a.source, a.is_read, a.is_starred, c.content_html
         FROM articles a LEFT JOIN contents c ON c.id = a.content_id WHERE a.id = ?1",
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

/* ============================================================
R1 窗口推进与只填空（REQ-106① 回归）
============================================================ */

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
    let http = new_http();
    let state = new_state();
    let shutdown = new_shutdown();

    let filled_first = cover_backfill_round(&db, &http, &state, &shutdown).await;
    assert_eq!(filled_first, 0, "最新 20 条候选的文章页都拿不到 og:image");

    let filled_second = cover_backfill_round(&db, &http, &state, &shutdown).await;
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

    // 稳态：失败项都在负缓存中、有图项已离开队列 ⇒ 第三轮不再发起请求
    let before: usize = (0..25).map(|i| site.hits_of(&format!("/ok/{i}"))).sum();
    let filled_third = cover_backfill_round(&db, &http, &state, &shutdown).await;
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
    let http = new_http();
    let state = new_state();
    let shutdown = new_shutdown();

    let filled = cover_backfill_round(&db, &http, &state, &shutdown).await;
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
    let again = cover_backfill_round(&db, &http, &state, &shutdown).await;
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
    let http = new_http();
    let state = new_state();
    let shutdown = new_shutdown();

    let filled = cover_backfill_round(&db, &http, &state, &shutdown).await;
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

    let remaining = candidates(&db).await;
    assert!(
        remaining.is_empty(),
        "补全后队列应为空，实际仍有 {remaining:?}"
    );
}

/* ============================================================
R1（OPT-017D）：覆盖面矩阵 —— direct / greader / miniflux / fever
============================================================ */

/// direct 后端（`ingestion/parse.rs`）：正文有图 → 入库封面来自正文首图，
/// 不进入候选、不被抓页面；正文无图 → 无封面、进入候选，由补全抓文章页补上。
#[tokio::test]
async fn cover_backfill_direct_ingestion_matrix() {
    install_log_capture();
    let site = ArticleSite::start();
    let body_img = site.url("/ok/direct-body-img");
    let page_cover = site.url("/ok/direct-page-cover");

    // 自托管 feed：一条正文带图（指向假文章站），一条正文无图（页面有 og:image）。
    let feed_server = common::LocalFeedServer::start_with_feed({
        let body_img = body_img.clone();
        let page_cover = page_cover.clone();
        move |port| {
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
<channel>
  <title>Direct Matrix Feed</title>
  <link>http://127.0.0.1:{port}/</link>
  <description>cover matrix fixture</description>
  <image>
    <url>http://127.0.0.1:{port}/icon.png</url>
    <title>Direct Matrix Feed</title>
    <link>http://127.0.0.1:{port}/</link>
  </image>
  <item>
    <title>正文有图</title>
    <link>{body_img}</link>
    <guid isPermaLink="false">matrix-direct-img</guid>
    <pubDate>Mon, 24 Aug 2026 11:00:00 GMT</pubDate>
    <description><![CDATA[<p>body</p><img src="{body_img}" />]]></description>
  </item>
  <item>
    <title>正文无图</title>
    <link>{page_cover}</link>
    <guid isPermaLink="false">matrix-direct-noimg</guid>
    <pubDate>Mon, 24 Aug 2026 10:00:00 GMT</pubDate>
    <description><![CDATA[<p>no image in body</p>]]></description>
  </item>
</channel>
</rss>
"#
            )
        }
    })
    .await
    .expect("start local feed server");

    let path = temp_db("direct_matrix");
    let conn = db::open(&path).expect("open db");
    let f = feed(
        &conn,
        folder(&conn),
        &feed_server.url("/local_feed.xml"),
        "直连矩阵源",
    );
    let db = Arc::new(tokio::sync::Mutex::new(conn));
    let http = new_http();

    let added = app_lib::ingestion::refresh_feed_staged(&db, &http, f, false)
        .await
        .expect("direct refresh");
    assert_eq!(added, 2, "两条条目入库");

    let (img_id, img_cover, img_source) = row_by_url(&db, &body_img).await;
    assert_eq!(
        img_cover.as_deref(),
        Some(body_img.as_str()),
        "direct 正文首图入库即为封面（ingestion/parse.rs）"
    );
    assert_eq!(img_source, "direct");
    let (page_id, page_cover_before, _) = row_by_url(&db, &page_cover).await;
    assert_eq!(
        page_cover_before, None,
        "direct 正文无图 → 入库无封面（补全要补的缺口）"
    );

    let before = candidates(&db).await;
    assert!(
        ids_of(&before).contains(&page_id),
        "正文无图的直连文章必须在候选集合里：{before:?}"
    );
    assert!(
        !ids_of(&before).contains(&img_id),
        "正文有图的文章不得进入候选（不重复抓页面）：{before:?}"
    );

    let state = new_state();
    let shutdown = new_shutdown();
    let filled = cover_backfill_round(&db, &http, &state, &shutdown).await;
    assert_eq!(filled, 1, "正文无图的直连文章应被补上 og:image 封面");
    assert_eq!(
        cover_of(&db, page_id).await.as_deref(),
        Some("https://img.example.com/direct-page-cover.jpg")
    );
    assert_eq!(
        site.hits_of("/ok/direct-body-img"),
        0,
        "正文已有图的文章不得被抓页面（有图不重复抓）"
    );
    assert_eq!(site.hits_of("/ok/direct-page-cover"), 1);
    assert!(candidates(&db).await.is_empty(), "补全后候选队列应为空");

    feed_server.stop().await;
}

/// 服务端形态枚举：greader（Miniflux 形态端点）、greader（FreshRSS 子路径形态）、
/// fever。三者共用 `sync/entries.rs` 的入库路径（正文首图兜底 + COALESCE 回填），
/// 差异只在鉴权与端点布局；封面缺口语义应逐字一致。
#[derive(Clone, Copy, PartialEq)]
enum ServerShape {
    /// Miniflux 形态：API 在站点根（`greader_api_prefix = ""`）。
    Miniflux,
    /// FreshRSS 形态：API 在 `/api/greader.php` 子路径。
    GReaderFreshRss,
    /// Fever 协议（Fever 信封服务条目）。
    Fever,
}

impl ServerShape {
    fn tag(self) -> &'static str {
        match self {
            ServerShape::Miniflux => "miniflux",
            ServerShape::GReaderFreshRss => "greader_freshrss",
            ServerShape::Fever => "fever",
        }
    }
}

/// 服务端矩阵断言主体（三种形态共用）。
async fn server_backend_matrix(shape: ServerShape) {
    install_log_capture();
    let site = ArticleSite::start();
    let server = MockGReader::start().await.expect("start mock server");
    match shape {
        ServerShape::Miniflux => {}
        ServerShape::GReaderFreshRss => server.set_greader_api_prefix("/api/greader.php"),
        ServerShape::Fever => server.set_fever_serve_entries(true),
    }

    let path = temp_db(&format!("server_matrix_{}", shape.tag()));
    let conn = db::open(&path).expect("open db");
    if shape == ServerShape::Fever {
        db::set_setting(&conn, "sync_protocol", "fever").unwrap();
    }
    db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
    db::set_setting(&conn, "greader_username", "test").unwrap();
    db::set_setting(&conn, "greader_password", "test-token").unwrap();
    let db = Arc::new(tokio::sync::Mutex::new(conn));
    let http = new_http();

    // 先把远端订阅拉进本地（feed/11 → origin='remote'）。
    app_lib::sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase");

    // 服务端 feed 11 上加两条：正文有图 / 正文无图（页面有 og:image）。
    let body_img = site.url("/ok/srv-body-img");
    let page_cover = site.url("/ok/srv-page-cover");
    server.add_entry_full(
        11,
        &body_img,
        "服务端正文有图",
        false,
        false,
        &format!(r#"<p>x</p><img src="{body_img}" />"#),
        Vec::new(),
    );
    server.add_entry_full(
        11,
        &page_cover,
        "服务端正文无图",
        false,
        false,
        "<p>no image in body</p>",
        Vec::new(),
    );
    app_lib::sync::sync_light(&db, &http)
        .await
        .expect("light sync");

    let (img_id, img_cover, img_source) = row_by_url(&db, &body_img).await;
    assert_eq!(
        img_cover.as_deref(),
        Some(body_img.as_str()),
        "[{}] 服务端入库已用正文首图兜底（sync/entries.rs）",
        shape.tag()
    );
    assert_eq!(img_source, "miniflux");
    let (page_id, page_cover_before, _) = row_by_url(&db, &page_cover).await;
    assert_eq!(
        page_cover_before,
        None,
        "[{}] 服务端正文无图 → 无任何封面（OPT-017D 要补的缺口）",
        shape.tag()
    );

    let before = candidates(&db).await;
    assert!(
        ids_of(&before).contains(&page_id),
        "[{}] 服务端正文无图文章必须进入补全候选：{before:?}",
        shape.tag()
    );
    assert!(
        !ids_of(&before).contains(&img_id),
        "[{}] 服务端正文有图文章（已有封面）不得进入候选：{before:?}",
        shape.tag()
    );

    let state = new_state();
    let shutdown = new_shutdown();
    let filled = cover_backfill_round(&db, &http, &state, &shutdown).await;
    assert_eq!(
        filled,
        1,
        "[{}] 服务端正文无图文章应被补上文章页 og:image 封面",
        shape.tag()
    );
    assert_eq!(
        cover_of(&db, page_id).await.as_deref(),
        Some("https://img.example.com/srv-page-cover.jpg"),
        "[{}] 补上的封面来自文章页 og:image",
        shape.tag()
    );
    assert_eq!(
        site.hits_of("/ok/srv-body-img"),
        0,
        "[{}] 已有封面（正文首图）的服务端文章不得被重复抓取",
        shape.tag()
    );
    assert_eq!(
        site.hits_of("/ok/srv-page-cover"),
        1,
        "[{}] 缺封面文章的文章页恰被抓一次",
        shape.tag()
    );
    // 服务端已有封面不被覆盖：整轮结束后仍是入库时的正文首图。
    assert_eq!(
        cover_of(&db, img_id).await.as_deref(),
        Some(body_img.as_str()),
        "[{}] 已有封面不得被补全覆盖",
        shape.tag()
    );
    assert!(candidates(&db).await.is_empty(), "补全后候选队列应为空");
}

#[tokio::test]
async fn cover_backfill_server_matrix_miniflux() {
    server_backend_matrix(ServerShape::Miniflux).await;
}

#[tokio::test]
async fn cover_backfill_server_matrix_greader_freshrss() {
    server_backend_matrix(ServerShape::GReaderFreshRss).await;
}

#[tokio::test]
async fn cover_backfill_server_matrix_fever() {
    server_backend_matrix(ServerShape::Fever).await;
}

/* ============================================================
R2（OPT-017D）：失败缓存上限与「失败被记录」
============================================================ */

/// 抓取失败的三种形态（403 / 200 无 og:image / 404）都被记录且本进程不重试；
/// 「已尝试」负缓存有明确上限。
#[tokio::test]
async fn cover_backfill_failures_are_cached_and_logged() {
    install_log_capture();
    let site = ArticleSite::start();
    let path = temp_db("fail_modes");
    let conn = db::open(&path).expect("open db");
    let f = feed(&conn, folder(&conn), "http://cover.test/d.xml", "源D");
    let forbidden = article(&conn, f, 0, &site.url("/fail/forbidden"), None);
    let no_og = article(&conn, f, 1, &site.url("/nom/no-og"), None);
    let missing = article(&conn, f, 2, &site.url("/gone/missing"), None);
    let db = Arc::new(tokio::sync::Mutex::new(conn));
    let http = new_http();
    let state = new_state();
    let shutdown = new_shutdown();

    let mark = log_mark();
    let filled = cover_backfill_round(&db, &http, &state, &shutdown).await;
    assert_eq!(filled, 0, "三种失败形态都不产出封面");

    // 失败被记录（可诊断），带 URL 与原因，而不是静默当作已处理。
    let forbidden_lines = assert_logged_since(mark, "/fail/forbidden");
    assert!(
        forbidden_lines
            .iter()
            .any(|l| l.contains("/fail/forbidden") && l.contains("403")),
        "403 失败必须连同状态码记录：{forbidden_lines:#?}"
    );
    let no_og_lines = assert_logged_since(mark, "/nom/no-og");
    assert!(
        no_og_lines
            .iter()
            .any(|l| l.contains("/nom/no-og") && l.contains("无 og:image")),
        "200 但无 og:image 必须连同原因记录：{no_og_lines:#?}"
    );
    assert_logged_since(mark, "/gone/missing");

    // 同一会话内不无限重试。
    let before = site.hits_total();
    let filled2 = cover_backfill_round(&db, &http, &state, &shutdown).await;
    assert_eq!(filled2, 0);
    assert_eq!(
        site.hits_total(),
        before,
        "已尝试过的失败 URL 本进程内不得重试"
    );
    assert_eq!(
        state.lock().await.attempts().len(),
        3,
        "三个 URL 都应在负缓存里（这是不重试的依据）"
    );
    assert_eq!(cover_of(&db, forbidden).await, None);
    assert_eq!(cover_of(&db, no_og).await, None);
    assert_eq!(cover_of(&db, missing).await, None);
}

/// 负缓存容量有上限：容量满后按插入序逐出最旧——逐出只把「本进程永不重试」
/// 放宽为「至少再经历 CAP 条其它尝试后才可能重试」，每轮请求量仍有界。
#[tokio::test]
async fn cover_backfill_attempt_cache_cap_bounds_retry() {
    let site = ArticleSite::start();
    let path = temp_db("cap");
    let conn = db::open(&path).expect("open db");
    let f = feed(&conn, folder(&conn), "http://cover.test/e.xml", "源E");
    article(&conn, f, 0, &site.url("/fail/cap-0"), None);
    article(&conn, f, 1, &site.url("/fail/cap-1"), None);
    let db = Arc::new(tokio::sync::Mutex::new(conn));
    let http = new_http();
    // 容量 1：第二轮必然重试两条中被逐出的那一条。
    let state = new_state_with_cap(1);
    let shutdown = new_shutdown();

    let filled1 = cover_backfill_round(&db, &http, &state, &shutdown).await;
    assert_eq!(filled1, 0);
    assert_eq!(site.hits_total(), 2, "第一轮两条都尝试过");
    assert_eq!(
        state.lock().await.attempts().len(),
        1,
        "容量 1 的负缓存只保留最近一条（上限生效）"
    );

    let filled2 = cover_backfill_round(&db, &http, &state, &shutdown).await;
    assert_eq!(filled2, 0);
    assert!(
        site.hits_total() > 2,
        "被逐出的 URL 允许重试（有界：每轮最多一批），但不存在的『永久不重试』被上限放宽"
    );
    assert!(
        state.lock().await.attempts().len() <= 1,
        "任何时刻负缓存不得超过上限"
    );
}

/// R1-P2-2（窗口冻结）：候选 > 80 条、前 80 条（最新、占满单轮扫描窗 4×20）
/// **全部已被负缓存命中**（无图/失败，留在队列且不产生新尝试 → 负缓存也不再
/// 逐出）。没有持久扫描游标时每轮都从 offset 0 重扫同样 80 行：更老的候选
/// 在本进程内冻结到重启。断言游标推进（负缓存命中也算扫描过）且后续轮次
/// 必须请求到第一窗之外的候选。
#[tokio::test]
async fn cover_backfill_scan_cursor_reaches_candidates_beyond_first_window() {
    let site = ArticleSite::start();
    let path = temp_db("cursor");
    let conn = db::open(&path).expect("open db");
    let f = feed(&conn, folder(&conn), "http://cover.test/i.xml", "源I");
    // 100 条候选：最新 80 条（先扫到）指向必然失败的页面，更老 20 条可补。
    let mut urls = Vec::new();
    for i in 0..100i64 {
        let url = if i < 80 {
            site.url(&format!("/fail/cursor-{i}"))
        } else {
            site.url(&format!("/ok/cursor-{i}"))
        };
        article(&conn, f, i, &url, None);
        urls.push(url);
    }
    let db = Arc::new(tokio::sync::Mutex::new(conn));
    let http = new_http();
    let state = new_state();
    let shutdown = new_shutdown();

    // 预置负缓存：最新 80 条（占满第一窗）标记为「已尝试」。
    {
        let mut s = state.lock().await;
        for url in urls.iter().take(80) {
            s.attempts_mut().insert(url.clone());
        }
    }

    // 第一轮：窗口内全部命中负缓存 → 不产生任何请求。
    let filled1 = cover_backfill_round(&db, &http, &state, &shutdown).await;
    assert_eq!(filled1, 0);
    assert_eq!(site.hits_total(), 0, "第一窗全被负缓存命中时本轮不发请求");
    assert!(
        state.lock().await.scan_offset() > 0,
        "负缓存命中也要推进扫描游标（修前每轮从 0 重扫，窗口冻结）"
    );

    // 第二轮：游标已越过第一窗 → 必须请求到第一窗之外的候选并补上。
    let filled2 = cover_backfill_round(&db, &http, &state, &shutdown).await;
    assert_eq!(
        filled2, 20,
        "第一窗之外的 20 条候选必须被覆盖到（修前 picked 恒空、永不请求）"
    );
    let remaining = candidates(&db).await;
    assert_eq!(
        remaining.len(),
        80,
        "被负缓存跳过的最新 80 条仍在队列里（无封面、未到逐出）"
    );
}

/// 候选查询失败必须可见：不得压成空集合后静默当作「没有候选」。
#[tokio::test]
async fn cover_backfill_candidate_read_failure_is_logged() {
    install_log_capture();
    // 无 schema 的连接：候选查询必然失败（no such table: articles）。
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let db = Arc::new(tokio::sync::Mutex::new(conn));
    let http = new_http();
    let state = new_state();
    let shutdown = new_shutdown();

    let mark = log_mark();
    let filled = cover_backfill_round(&db, &http, &state, &shutdown).await;
    assert_eq!(filled, 0);
    assert_logged_since(mark, "候选查询失败");
    assert!(
        state.lock().await.attempts().is_empty(),
        "读失败不得把任何条目标记为已处理"
    );
}

/// DB 写失败必须可见且不得把该 URL 标记为已处理：下一轮仍会重试（有界）；
/// 恢复后同一条候选能被正常补上。
#[tokio::test]
async fn cover_backfill_db_write_failure_is_retried_not_marked_processed() {
    install_log_capture();
    let site = ArticleSite::start();
    let path = temp_db("db_write_fail");
    let conn = db::open(&path).expect("open db");
    let f = feed(&conn, folder(&conn), "http://cover.test/f.xml", "源F");
    let url = site.url("/ok/write-fail");
    article(&conn, f, 0, &url, None);
    let db = Arc::new(tokio::sync::Mutex::new(conn));
    let http = new_http();
    let state = new_state();
    let shutdown = new_shutdown();

    // 真 DB 触发器：UPDATE image_url 一律 ABORT（模拟写库失败）。
    {
        let conn = db.lock().await;
        conn.execute_batch(
            "CREATE TRIGGER cover_write_failure BEFORE UPDATE OF image_url ON articles
             BEGIN SELECT RAISE(ABORT, 'injected cover write failure'); END;",
        )
        .unwrap();
    }

    let mark = log_mark();
    let filled1 = cover_backfill_round(&db, &http, &state, &shutdown).await;
    assert_eq!(filled1, 0, "写库失败不产出封面");
    assert_logged_since(mark, "写库失败");
    assert_eq!(site.hits_of("/ok/write-fail"), 1);
    assert!(
        !state.lock().await.attempts().contains(&url),
        "写库失败不得把 URL 标记为已尝试（那不是『已处理』）"
    );

    // 下一轮重试（有界：每轮批次上限；这里同一条候选重新被尝试一次）。
    let _ = cover_backfill_round(&db, &http, &state, &shutdown).await;
    assert_eq!(
        site.hits_of("/ok/write-fail"),
        2,
        "写失败后下一轮必须重试而不是永久跳过"
    );

    // 恢复 DB → 同一候选被正常补上。
    {
        let conn = db.lock().await;
        conn.execute_batch("DROP TRIGGER cover_write_failure;")
            .unwrap();
    }
    let filled3 = cover_backfill_round(&db, &http, &state, &shutdown).await;
    assert_eq!(filled3, 1, "恢复后应能补上封面");
    assert_eq!(
        cover_of(&db, article_id_by_url(&db, &url).await)
            .await
            .as_deref(),
        Some("https://img.example.com/write-fail.jpg")
    );
}

async fn article_id_by_url(db: &Arc<tokio::sync::Mutex<rusqlite::Connection>>, url: &str) -> i64 {
    row_by_url(db, url).await.0
}

/* ============================================================
R3（OPT-017D）：退出可终止 —— 触发后不再发起新请求
============================================================ */

/// 终止信号触发后：不再发起新请求；在途请求被取消；慢请求不会拖住本轮返回。
#[tokio::test]
async fn cover_backfill_stops_on_shutdown_signal() {
    install_log_capture();
    let site = ArticleSite::start();
    let path = temp_db("shutdown");
    let conn = db::open(&path).expect("open db");
    let f = feed(&conn, folder(&conn), "http://cover.test/g.xml", "源G");
    article(&conn, f, 0, &site.url("/ok/sd-0"), None);
    article(&conn, f, 1, &site.url("/ok/sd-1"), None);
    let db = Arc::new(tokio::sync::Mutex::new(conn));
    let http = new_http();

    // (a) 触发后再跑一轮：一个新请求都不发。
    let state_a = new_state();
    let shutdown_a = new_shutdown();
    shutdown_a.trigger();
    let filled_a = cover_backfill_round(&db, &http, &state_a, &shutdown_a).await;
    assert_eq!(filled_a, 0);
    assert_eq!(site.hits_total(), 0, "终止信号触发后不得发起任何新请求");
    assert!(state_a.lock().await.attempts().is_empty());

    // (b) 中途触发：两个慢请求在途（并发上限 2），触发后其余候选不再发出；
    // 在途请求被取消，本轮在远小于慢响应（3s）的时间内返回。
    let path_b = temp_db("shutdown_mid");
    let conn_b = db::open(&path_b).expect("open db");
    let f_b = feed(&conn_b, folder(&conn_b), "http://cover.test/h.xml", "源H");
    for i in 0..4 {
        article(&conn_b, f_b, i, &site.url(&format!("/slow/{i}")), None);
    }
    let db_b = Arc::new(tokio::sync::Mutex::new(conn_b));
    let state_b = new_state();
    let shutdown_b = new_shutdown();

    let round = tokio::spawn({
        let db = db_b.clone();
        let http = http.clone();
        let state = state_b.clone();
        let shutdown = shutdown_b.clone();
        async move { cover_backfill_round(&db, &http, &state, &shutdown).await }
    });

    // 等到并发窗口满（恰 2 个慢请求在途）。
    let deadline = Instant::now() + Duration::from_secs(5);
    while site.hits_total() < 2 {
        assert!(
            Instant::now() < deadline,
            "等待慢请求在途超时（当前 {} 个请求）",
            site.hits_total()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let triggered_at = Instant::now();
    shutdown_b.trigger();
    let filled_b = tokio::time::timeout(Duration::from_secs(5), round)
        .await
        .expect("终止信号后本轮必须在有限时间内返回")
        .expect("round task join");
    let elapsed = triggered_at.elapsed();

    assert_eq!(filled_b, 0, "退出挂起中不产出封面");
    assert_eq!(
        site.hits_total(),
        2,
        "触发后不得再发起新请求（进入在途的 2 个请求之外不再增加）"
    );
    assert!(
        elapsed < Duration::from_millis(1500),
        "在途慢请求必须被取消而不是等满 3s（实际 {elapsed:?}）"
    );
    assert!(
        state_b.lock().await.attempts().is_empty(),
        "被终止中断的尝试不得记入负缓存"
    );
}
