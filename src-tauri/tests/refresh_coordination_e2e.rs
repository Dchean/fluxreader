//! 刷新编排回归：手动全刷 single-flight、同源在途去重、批内并发上限、
//! hybrid 源集合、generation CAS 与用户标题保护。全程本地回环 + 临时库。

use app_lib::db;
use app_lib::ingestion;
use app_lib::scheduler;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex as AsyncMutex;

mod common;

struct FeedServer {
    port: u16,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    join: Option<tokio::task::JoinHandle<()>>,
    hits: Arc<StdMutex<HashMap<String, usize>>>,
    max_active: Arc<AtomicUsize>,
}

#[derive(Clone)]
enum FeedGate {
    Delay(Duration),
    FirstWave {
        barrier: Arc<tokio::sync::Barrier>,
        arrivals: Arc<AtomicUsize>,
    },
}

impl FeedServer {
    async fn start(delay_ms: u64) -> Self {
        Self::start_with_gate(FeedGate::Delay(Duration::from_millis(delay_ms))).await
    }

    async fn start_first_wave_rendezvous() -> Self {
        Self::start_with_gate(FeedGate::FirstWave {
            barrier: Arc::new(tokio::sync::Barrier::new(2)),
            arrivals: Arc::new(AtomicUsize::new(0)),
        })
        .await
    }

    async fn start_with_gate(gate: FeedGate) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(StdMutex::new(HashMap::new()));
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
        let join = {
            let hits = hits.clone();
            let active = active.clone();
            let max_active = max_active.clone();
            let gate = gate.clone();
            tokio::spawn(async move {
                loop {
                    let accepted = tokio::select! {
                        _ = &mut stop_rx => break,
                        r = listener.accept() => r,
                    };
                    let Ok((stream, _)) = accepted else {
                        continue;
                    };
                    let hits = hits.clone();
                    let active = active.clone();
                    let max_active = max_active.clone();
                    let gate = gate.clone();
                    tokio::spawn(async move {
                        serve_feed(stream, gate, hits, active, max_active).await;
                    });
                }
            })
        };
        Self {
            port,
            stop: Some(stop_tx),
            join: Some(join),
            hits,
            max_active,
        }
    }

    fn url(&self, name: &str) -> String {
        format!("http://127.0.0.1:{}/feed/{name}.xml", self.port)
    }

    fn hits(&self, name: &str) -> usize {
        *self
            .hits
            .lock()
            .unwrap()
            .get(&format!("/feed/{name}.xml"))
            .unwrap_or(&0)
    }

    fn max_active(&self) -> usize {
        self.max_active.load(Ordering::SeqCst)
    }

    async fn stop(mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        if let Some(join) = self.join.take() {
            let _ = join.await;
        }
    }
}

impl Drop for FeedServer {
    fn drop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        if let Some(join) = self.join.take() {
            join.abort();
        }
    }
}

async fn serve_feed(
    mut stream: tokio::net::TcpStream,
    gate: FeedGate,
    hits: Arc<StdMutex<HashMap<String, usize>>>,
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
) {
    let mut buf = Vec::new();
    let head_end = loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos;
        }
        let mut chunk = [0u8; 2048];
        let Ok(n) = stream.read(&mut chunk).await else {
            return;
        };
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > 16 * 1024 {
            return;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]);
    let path = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or_default()
        .split('?')
        .next()
        .unwrap_or_default();
    let Some(name) = path
        .strip_prefix("/feed/")
        .and_then(|p| p.strip_suffix(".xml"))
    else {
        let _ = write_response(&mut stream, "404 Not Found", "").await;
        return;
    };
    let hit_count = {
        let mut hits = hits.lock().unwrap();
        let count = hits.entry(path.to_string()).or_insert(0);
        *count += 1;
        *count
    };

    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
    let mut highest = max_active.load(Ordering::SeqCst);
    while current > highest {
        match max_active.compare_exchange(highest, current, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => break,
            Err(observed) => highest = observed,
        }
    }
    match gate {
        FeedGate::Delay(delay) => tokio::time::sleep(delay).await,
        FeedGate::FirstWave { barrier, arrivals } => {
            if arrivals.fetch_add(1, Ordering::SeqCst) < 2
                && tokio::time::timeout(Duration::from_secs(5), barrier.wait())
                    .await
                    .is_err()
            {
                let _ =
                    write_response(&mut stream, "503 Service Unavailable", "gate timeout").await;
                active.fetch_sub(1, Ordering::SeqCst);
                return;
            }
        }
    }

    if name == "fail" || (name == "fail-once" && hit_count == 1) {
        let _ = write_response(&mut stream, "500 Internal Server Error", "injected").await;
        active.fetch_sub(1, Ordering::SeqCst);
        return;
    }

    let body = format!(
        r#"<?xml version="1.0"?>
<rss version="2.0"><channel>
<title>Fetched {name}</title><link>http://127.0.0.1/</link><description>t</description>
<item><title>{name} item</title><guid>{name}-1</guid><link>http://127.0.0.1/{name}/1</link></item>
</channel></rss>"#
    );
    let _ = write_response(&mut stream, "200 OK", &body).await;
    active.fetch_sub(1, Ordering::SeqCst);
}

async fn write_response(
    stream: &mut tokio::net::TcpStream,
    status: &str,
    body: &str,
) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/rss+xml; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

async fn setup(name: &str) -> (common::TempDbGuard, Arc<AsyncMutex<rusqlite::Connection>>) {
    let path = common::unique_db_path(name);
    let guard = common::TempDbGuard::new(path.clone());
    let conn = db::open(&path).unwrap();
    db::create_folder(&conn, "测试", "article").unwrap();
    (guard, Arc::new(AsyncMutex::new(conn)))
}

fn insert_feed(conn: &rusqlite::Connection, url: &str, title: &str, origin: &str) -> i64 {
    db::insert_feed_origin(
        conn, url, None, title, None, 1, "inherit", false, false, origin,
    )
    .unwrap()
}

fn direct_article(guid: &str, title: &str) -> db::NewArticle {
    db::NewArticle {
        guid: guid.into(),
        url: Some(format!("http://127.0.0.1/{guid}")),
        title: title.into(),
        author: None,
        summary: None,
        content_html: Some(format!("<p>{title}</p>")),
        body_text: title.into(),
        image_url: None,
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: Some("2026-01-01T00:00:00Z".into()),
        source: "direct".into(),
    }
}

#[tokio::test]
async fn concurrent_manual_full_refresh_coalesces_to_one_fetch() {
    let server = FeedServer::start(180).await;
    let (_guard, db) = setup("refresh_single_flight").await;
    let feed_id = {
        let conn = db.lock().await;
        insert_feed(&conn, &server.url("a"), "A", "local")
    };
    let client = ingestion::build_client(10);

    let (first, second) = tokio::join!(
        scheduler::refresh_all(&db, &client),
        scheduler::refresh_all(&db, &client),
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert_eq!(first, (1, 0));
    assert_eq!(second, first, "后到全刷必须复用领跑者结果");
    assert_eq!(server.hits("a"), 1, "两个并发全刷只能发一次请求");
    let count: i64 = {
        let conn = db.lock().await;
        conn.query_row(
            "SELECT COUNT(*) FROM articles WHERE feed_id = ?1",
            [feed_id],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(count, 1);
    server.stop().await;
}

#[tokio::test]
async fn same_feed_inflight_is_deduplicated() {
    let server = FeedServer::start(180).await;
    let (_guard, db) = setup("refresh_same_feed").await;
    let feed_id = {
        let conn = db.lock().await;
        insert_feed(&conn, &server.url("same"), "Same", "local")
    };
    let client = ingestion::build_client(10);

    let (first, second) = tokio::join!(
        ingestion::refresh_feed_staged(&db, &client, feed_id, false),
        ingestion::refresh_feed_staged(&db, &client, feed_id, false),
    );
    let outcomes = [first.unwrap(), second.unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, ingestion::FeedRefreshOutcome::SkippedInFlight))
            .count(),
        1,
        "同源在途的第二次请求必须明确标记为 SkippedInFlight"
    );
    assert_eq!(
        outcomes
            .iter()
            .filter_map(|outcome| match outcome {
                ingestion::FeedRefreshOutcome::Applied { new_articles, .. } => Some(*new_articles),
                _ => None,
            })
            .sum::<usize>(),
        1,
        "只有领跑者真正抓取并写回 1 条"
    );
    assert_eq!(server.hits("same"), 1);
    server.stop().await;
}

#[tokio::test]
async fn distinct_feeds_share_one_bounded_batch() {
    let server = FeedServer::start_first_wave_rendezvous().await;
    let (_guard, db) = setup("refresh_bounded").await;
    {
        let conn = db.lock().await;
        for name in ["a", "b", "c"] {
            insert_feed(&conn, &server.url(name), name, "local");
        }
        db::set_setting(&conn, "app_settings", r#"{"fetchConcurrency":2}"#).unwrap();
    }
    let client = ingestion::build_client(10);

    assert_eq!(scheduler::refresh_all(&db, &client).await.unwrap(), (3, 0));
    assert_eq!(server.hits("a"), 1);
    assert_eq!(server.hits("b"), 1);
    assert_eq!(server.hits("c"), 1);
    assert_eq!(
        server.max_active(),
        2,
        "批内必须按 fetchConcurrency 限并发，且不同源仍可并行"
    );
    server.stop().await;
}

#[tokio::test]
async fn hybrid_full_refresh_is_local_only_and_direct_includes_remote() {
    let server = FeedServer::start(40).await;
    let (_guard, db) = setup("refresh_hybrid_scope").await;
    {
        let conn = db.lock().await;
        insert_feed(&conn, &server.url("local"), "Local", "local");
        insert_feed(&conn, &server.url("remote"), "Remote", "remote");
        db::set_setting(
            &conn,
            "app_settings",
            r#"{"fetchConcurrency":1,"syncMode":"hybrid"}"#,
        )
        .unwrap();
    }
    let client = ingestion::build_client(10);

    assert_eq!(scheduler::refresh_all(&db, &client).await.unwrap(), (1, 0));
    assert_eq!(server.hits("local"), 1);
    assert_eq!(server.hits("remote"), 0, "hybrid 全刷不得直连远端源");

    {
        let conn = db.lock().await;
        db::set_setting(
            &conn,
            "app_settings",
            r#"{"fetchConcurrency":1,"syncMode":"direct"}"#,
        )
        .unwrap();
    }
    assert_eq!(scheduler::refresh_all(&db, &client).await.unwrap(), (1, 0));
    assert_eq!(server.hits("remote"), 1, "direct 全刷必须包含远端来源");
    server.stop().await;
}

#[tokio::test]
async fn due_then_manual_explicitly_forces_refresh_and_retries_failures() {
    let server = FeedServer::start(30).await;
    let (_guard, db) = setup("refresh_due_then_manual").await;
    {
        let conn = db.lock().await;
        insert_feed(&conn, &server.url("good"), "Good", "local");
        insert_feed(&conn, &server.url("fail"), "Fail", "local");
        db::set_setting(
            &conn,
            "app_settings",
            r#"{"fetchConcurrency":2,"refreshInterval":5}"#,
        )
        .unwrap();
    }
    let client = ingestion::build_client(10);

    assert_eq!(
        scheduler::refresh_due_once(&db, &client).await.unwrap(),
        (1, 1),
        "到期批：好源成功、坏源失败"
    );
    assert_eq!(server.hits("good"), 1);
    assert_eq!(server.hits("fail"), 1);

    assert_eq!(
        scheduler::refresh_all(&db, &client).await.unwrap(),
        (0, 1),
        "手动全刷是显式行为，必须绕过 footprint；失败源同样重试"
    );
    assert_eq!(
        server.hits("good"),
        2,
        "due→manual 可再抓一次：manual 不静默跳过刚成功的源"
    );
    assert_eq!(server.hits("fail"), 2, "失败源必须允许下一批立即重试");

    assert_eq!(
        scheduler::refresh_all(&db, &client).await.unwrap(),
        (0, 1),
        "完成后的下一次 manual 调用仍是新的显式批次"
    );
    assert_eq!(
        server.hits("good"),
        3,
        "只有并发 manual 复用 flight；顺序 manual 必须重新抓取"
    );
    assert_eq!(server.hits("fail"), 3);
    server.stop().await;
}

#[tokio::test]
async fn manual_then_due_skips_recent_success_even_when_due_query_would_select_it() {
    let server = FeedServer::start(30).await;
    let (_guard, db) = setup("refresh_manual_then_due").await;
    let feed_id = {
        let conn = db.lock().await;
        insert_feed(&conn, &server.url("a"), "A", "local")
    };
    let client = ingestion::build_client(10);

    assert_eq!(scheduler::refresh_all(&db, &client).await.unwrap(), (1, 0));
    {
        let conn = db.lock().await;
        // 人为把 last_fetched_at 推旧，确保 due 查询本身会选中它；跨批次
        // footprint 仍须阻止紧随的重复抓取。
        conn.execute(
            "UPDATE feeds SET last_fetched_at = datetime('now', '-1 day') WHERE id = ?1",
            [feed_id],
        )
        .unwrap();
        db::set_setting(
            &conn,
            "app_settings",
            r#"{"fetchConcurrency":1,"refreshInterval":5}"#,
        )
        .unwrap();
    }
    assert_eq!(
        scheduler::refresh_due_once(&db, &client).await.unwrap(),
        (0, 0)
    );
    assert_eq!(server.hits("a"), 1);
    server.stop().await;
}

#[tokio::test]
async fn inflight_skip_then_failure_does_not_enter_footprint_and_due_retry_succeeds() {
    // Keep the first request visibly in flight under the parallel test gate too.
    let server = FeedServer::start(2_000).await;
    let (_guard, db) = setup("refresh_inflight_failure_retry").await;
    let feed_id = {
        let conn = db.lock().await;
        let feed_id = insert_feed(&conn, &server.url("fail-once"), "Fail Once", "local");
        db::set_setting(
            &conn,
            "app_settings",
            r#"{"fetchConcurrency":1,"refreshInterval":5}"#,
        )
        .unwrap();
        feed_id
    };
    let client = ingestion::build_client(10);

    let mut claim_observer =
        ingestion::refresh_claim_hook::ClaimObserver::install(&db, &server.url("fail-once"));
    let first_db = db.clone();
    let first_client = client.clone();
    let first = tokio::spawn(async move {
        ingestion::refresh_feed_staged(&first_db, &first_client, feed_id, false).await
    });
    claim_observer
        .wait_for_claim()
        .await
        .expect("首轮必须先在进程内建立 in-flight claim");
    tokio::time::timeout(Duration::from_secs(2), async {
        while server.hits("fail-once") == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("首轮应已发起 HTTP 请求");

    assert_eq!(
        scheduler::refresh_due_once(&db, &client).await.unwrap(),
        (0, 0),
        "同源在途时 due 批必须明确跳过，而不是把跳过的源记为成功"
    );
    assert_eq!(server.hits("fail-once"), 1);

    first
        .await
        .unwrap()
        .expect_err("首轮 fail-once 应真实失败，而不是被 in-flight skip 掩盖");
    {
        let conn = db.lock().await;
        conn.execute(
            "UPDATE feeds SET last_fetched_at = datetime('now', '-1 hour'),
                              next_retry_at = datetime('now', '-1 minute')
             WHERE id = ?1",
            [feed_id],
        )
        .unwrap();
    }

    assert_eq!(
        scheduler::refresh_due_once(&db, &client).await.unwrap(),
        (1, 0),
        "失败后下一次 due 必须可重试；若 in-flight skip 记入 footprint，这里会被静默过滤"
    );
    assert_eq!(server.hits("fail-once"), 2);
    server.stop().await;
}

#[tokio::test]
async fn footprint_uses_normalized_url_not_reusable_sqlite_id() {
    let server = FeedServer::start(30).await;
    let (_guard, db) = setup("refresh_footprint_id_reuse").await;
    let old_id = {
        let conn = db.lock().await;
        let old_id = insert_feed(&conn, &server.url("old"), "Old", "local");
        db::set_setting(
            &conn,
            "app_settings",
            r#"{"fetchConcurrency":1,"refreshInterval":5}"#,
        )
        .unwrap();
        old_id
    };
    let client = ingestion::build_client(10);

    assert_eq!(
        scheduler::refresh_due_once(&db, &client).await.unwrap(),
        (1, 0)
    );
    assert_eq!(server.hits("old"), 1);

    let new_id = {
        let conn = db.lock().await;
        // Intentional physical rowid reuse probe; ordinary unsubscribe now archives identity.
        conn.execute("DELETE FROM feeds WHERE id=?1", [old_id])
            .unwrap();
        conn.execute("DELETE FROM sqlite_sequence WHERE name='feeds'", [])
            .unwrap();
        insert_feed(&conn, &server.url("new"), "New", "local")
    };
    assert_eq!(
        new_id, old_id,
        "测试前提：SQLite 在删除最大 id 后应复用该 rowid"
    );

    assert_eq!(
        scheduler::refresh_due_once(&db, &client).await.unwrap(),
        (1, 0),
        "新 URL 即使复用旧 rowid，也不得继承旧 URL 的 footprint"
    );
    assert_eq!(server.hits("new"), 1);
    server.stop().await;
}

#[tokio::test]
async fn reused_rowid_while_old_url_inflight_refreshes_new_source() {
    let server = FeedServer::start(300).await;
    let (_guard, db) = setup("refresh_inflight_rowid_reuse").await;
    let old_id = {
        let conn = db.lock().await;
        insert_feed(&conn, &server.url("old"), "Old", "local")
    };
    let client = ingestion::build_client(10);

    let old_db = db.clone();
    let old_client = client.clone();
    let old = tokio::spawn(async move {
        ingestion::refresh_feed_staged(&old_db, &old_client, old_id, false).await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while server.hits("old") == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("旧源应已发起 HTTP 请求");

    let new_url = server.url("new");
    let new_id = {
        let conn = db.lock().await;
        // Intentional physical rowid reuse probe; ordinary unsubscribe now archives identity.
        conn.execute("DELETE FROM feeds WHERE id=?1", [old_id])
            .unwrap();
        conn.execute("DELETE FROM sqlite_sequence WHERE name='feeds'", [])
            .unwrap();
        insert_feed(&conn, &new_url, "New", "local")
    };
    assert_eq!(
        new_id, old_id,
        "测试前提：SQLite 在删除最大 id 后应复用该 rowid"
    );

    let new_outcome = tokio::time::timeout(
        Duration::from_secs(3),
        ingestion::refresh_feed_staged(&db, &client, new_id, false),
    )
    .await
    .expect("复用 rowid 的新源不得等待旧请求")
    .unwrap();
    assert_eq!(
        new_outcome,
        ingestion::FeedRefreshOutcome::Applied {
            new_articles: 1,
            data_changed: true,
            feed_url: new_url.clone(),
        },
        "在途键必须按 URL 区分，不能让新 URL 被旧 rowid 误跳过"
    );
    assert_eq!(server.hits("new"), 1);

    let old_outcome = tokio::time::timeout(Duration::from_secs(3), old)
        .await
        .expect("旧请求应在测试超时前结束")
        .expect("旧请求任务不得 panic")
        .unwrap();
    assert_eq!(
        old_outcome,
        ingestion::FeedRefreshOutcome::Superseded,
        "旧 URL 的结果不得写进复用 rowid 的新源"
    );

    let conn = db.lock().await;
    let (stored_url, title, article_count, old_article_count): (String, String, i64, i64) = conn
        .query_row(
            "SELECT f.feed_url, f.title,
                    (SELECT COUNT(*) FROM articles WHERE feed_id = f.id),
                    (SELECT COUNT(*) FROM articles WHERE feed_id = f.id AND guid = 'old-1')
               FROM feeds f WHERE f.id = ?1",
            [new_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(stored_url, new_url);
    assert_eq!(title, "Fetched new");
    assert_eq!(article_count, 1, "只允许新源自己的条目落库");
    assert_eq!(old_article_count, 0, "旧请求不得污染新源文章");
    server.stop().await;
}

#[tokio::test]
async fn cancelled_refresh_caller_does_not_strand_flight() {
    let server = FeedServer::start(350).await;
    let (_guard, db) = setup("refresh_cancelled_caller").await;
    {
        let conn = db.lock().await;
        insert_feed(&conn, &server.url("a"), "A", "local");
    }
    let client = ingestion::build_client(10);

    let first_db = db.clone();
    let first_client = client.clone();
    let first = tokio::spawn(async move { scheduler::refresh_all(&first_db, &first_client).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while server.hits("a") == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("首个全刷应已发起 HTTP 请求");
    first.abort();
    let _ = first.await;

    let second = tokio::time::timeout(Duration::from_secs(3), scheduler::refresh_all(&db, &client))
        .await
        .expect("调用方取消后 flight 仍须由后台任务完成并清理")
        .unwrap();
    assert_eq!(second, (1, 0));
    assert_eq!(
        server.hits("a"),
        1,
        "取消调用方不得触发第二轮 HTTP 或遗留无法加入的 flight"
    );
    server.stop().await;
}

#[tokio::test]
async fn shutdown_stops_queued_refresh_workers_before_they_send_requests() {
    let server = FeedServer::start(400).await;
    let (_guard, db) = setup("refresh_shutdown_queue").await;
    {
        let conn = db.lock().await;
        db::set_setting(&conn, "app_settings", r#"{"fetchConcurrency":1}"#).unwrap();
        for name in ["a", "b", "c"] {
            insert_feed(&conn, &server.url(name), name, "local");
        }
    }
    let client = ingestion::build_client(10);
    let first_db = db.clone();
    let first_client = client.clone();
    let first = tokio::spawn(async move { scheduler::refresh_all(&first_db, &first_client).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while ["a", "b", "c"]
            .iter()
            .map(|name| server.hits(name))
            .sum::<usize>()
            == 0
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("一个 worker 应已发请求，另两个还在等待并发许可");
    app_lib::sync::runtime::for_db(&db).stop();
    let result = tokio::time::timeout(Duration::from_secs(3), first)
        .await
        .expect("停止后刷新批必须有限收尾")
        .unwrap();
    assert_eq!(
        ["a", "b", "c"]
            .iter()
            .map(|name| server.hits(name))
            .sum::<usize>(),
        1,
        "停止信号后排队 worker 不得开始新的 HTTP 请求"
    );
    assert_eq!(result.unwrap_err().code, "syncCancelled");
    assert_eq!(
        scheduler::refresh_all(&db, &client).await.unwrap_err().code,
        "syncCancelled"
    );
    assert_eq!(
        scheduler::refresh_due_once(&db, &client)
            .await
            .unwrap_err()
            .code,
        "syncCancelled"
    );
    server.stop().await;
}

#[tokio::test]
async fn feed_query_failure_is_returned_instead_of_fake_empty_success() {
    let (_guard, db) = setup("refresh_query_failure").await;
    {
        let conn = db.lock().await;
        conn.execute_batch("ALTER TABLE feeds RENAME TO feeds_missing;")
            .unwrap();
    }
    let client = ingestion::build_client(10);
    let error = scheduler::refresh_all(&db, &client)
        .await
        .expect_err("feeds 查询失败必须上抛，不能伪装成 (0,0)");
    assert_eq!(error.code, "db");
    assert!(
        error.message.contains("feeds"),
        "错误应保留真实的查询失败现场: {error}"
    );
}

#[test]
fn stale_generation_cannot_overwrite_new_fetch_or_articles() {
    let (_guard, db) = setup_sync("refresh_generation");
    let conn = db.lock().unwrap();
    let feed_id = insert_feed(&conn, "http://127.0.0.1/generation.xml", "Auto", "local");

    let first = db::begin_feed_refresh(&conn, feed_id).unwrap();
    let second = db::begin_feed_refresh(&conn, feed_id).unwrap();
    assert_eq!(first.generation, 1);
    assert_eq!(second.generation, 2);

    let new_parsed = ingestion::ParsedFeed {
        title: Some("新抓取标题".into()),
        site_url: None,
        icon: None,
        articles: vec![direct_article("new", "新文章").into()],
    };
    let new_fetched = ingestion::Fetched::Body {
        bytes: Vec::new(),
        content_type: None,
        etag: Some("W/\"new\"".into()),
        last_modified: None,
    };
    assert_eq!(
        ingestion::apply_refresh_result(
            &conn,
            feed_id,
            &ingestion::RefreshWriteContext::new(
                "http://127.0.0.1/generation.xml",
                second.generation,
                None,
                None,
            ),
            &new_fetched,
            &new_parsed,
            false,
        )
        .unwrap(),
        ingestion::RefreshApplyOutcome::Applied(1)
    );

    let old_parsed = ingestion::ParsedFeed {
        title: Some("旧抓取标题".into()),
        site_url: None,
        icon: None,
        articles: vec![direct_article("old", "旧文章").into()],
    };
    let old_fetched = ingestion::Fetched::Body {
        bytes: Vec::new(),
        content_type: None,
        etag: Some("W/\"old\"".into()),
        last_modified: Some("Mon, 01 Jan 2024 00:00:00 GMT".into()),
    };
    assert_eq!(
        ingestion::apply_refresh_result(
            &conn,
            feed_id,
            &ingestion::RefreshWriteContext::new(
                "http://127.0.0.1/generation.xml",
                first.generation,
                None,
                None,
            ),
            &old_fetched,
            &old_parsed,
            false,
        )
        .unwrap(),
        ingestion::RefreshApplyOutcome::Superseded
    );
    assert!(!db::set_feed_fetch_state_if_generation(
        &conn,
        feed_id,
        first.generation,
        true,
        Some("旧失败"),
        Some("W/\"old\""),
        None,
    )
    .unwrap());

    let (etag, failed, error, title, fetched_title): (
        Option<String>,
        i64,
        Option<String>,
        String,
        String,
    ) = conn
        .query_row(
            "SELECT etag, fetch_failed, fetch_error, title, fetched_title
               FROM feeds WHERE id = ?1",
            [feed_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(etag.as_deref(), Some("W/\"new\""));
    assert_eq!(failed, 0);
    assert_eq!(error, None);
    assert_eq!(title, "新抓取标题");
    assert_eq!(fetched_title, "新抓取标题");
    let (new_exists, old_exists): (i64, i64) = conn
        .query_row(
            "SELECT
               EXISTS(SELECT 1 FROM articles WHERE feed_id = ?1 AND guid = 'new'),
               EXISTS(SELECT 1 FROM articles WHERE feed_id = ?1 AND guid = 'old')",
            [feed_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((new_exists, old_exists), (1, 0), "旧代文章不得落库");
}

#[test]
fn user_custom_title_survives_refresh_and_fetched_title_updates() {
    let (_guard, db) = setup_sync("refresh_custom_title");
    let conn = db.lock().unwrap();
    let feed_id = insert_feed(&conn, "http://127.0.0.1/title.xml", "自动标题", "local");
    db::update_feed_full(
        &conn,
        feed_id,
        Some("我的源名"),
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .unwrap();
    let attempt = db::begin_feed_refresh(&conn, feed_id).unwrap();
    let parsed = ingestion::ParsedFeed {
        title: Some("站点后来改的标题".into()),
        site_url: None,
        icon: None,
        articles: Vec::new(),
    };
    let fetched = ingestion::Fetched::Body {
        bytes: Vec::new(),
        content_type: None,
        etag: Some("W/\"t\"".into()),
        last_modified: None,
    };
    assert_eq!(
        ingestion::apply_refresh_result(
            &conn,
            feed_id,
            &ingestion::RefreshWriteContext::new(
                "http://127.0.0.1/title.xml",
                attempt.generation,
                None,
                None,
            ),
            &fetched,
            &parsed,
            false,
        )
        .unwrap(),
        ingestion::RefreshApplyOutcome::Applied(0)
    );
    let (title, custom, fetched_title): (String, Option<String>, String) = conn
        .query_row(
            "SELECT title, title_custom, fetched_title FROM feeds WHERE id = ?1",
            [feed_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(title, "我的源名");
    assert_eq!(custom.as_deref(), Some("我的源名"));
    assert_eq!(fetched_title, "站点后来改的标题");
}

#[test]
fn legacy_unknown_title_is_preserved_on_first_changed_refresh() {
    let (_guard, db) = setup_sync("refresh_legacy_title");
    let conn = db.lock().unwrap();
    let feed_id = insert_feed(
        &conn,
        "http://127.0.0.1/legacy-title.xml",
        "旧版里的名字",
        "local",
    );
    // v34 迁移后的旧行：fetched_title 回填当前 title，title_custom 保持 NULL。
    conn.execute(
        "UPDATE feeds SET fetched_title = title, title_custom = NULL WHERE id = ?1",
        [feed_id],
    )
    .unwrap();
    let attempt = db::begin_feed_refresh(&conn, feed_id).unwrap();
    let parsed = ingestion::ParsedFeed {
        title: Some("源站当前标题".into()),
        site_url: None,
        icon: None,
        articles: Vec::new(),
    };
    let fetched = ingestion::Fetched::Body {
        bytes: Vec::new(),
        content_type: None,
        etag: Some("W/\"legacy\"".into()),
        last_modified: None,
    };
    assert_eq!(
        ingestion::apply_refresh_result(
            &conn,
            feed_id,
            &ingestion::RefreshWriteContext::new(
                "http://127.0.0.1/legacy-title.xml",
                attempt.generation,
                None,
                None,
            ),
            &fetched,
            &parsed,
            false,
        )
        .unwrap(),
        ingestion::RefreshApplyOutcome::Applied(0)
    );
    let (title, custom, fetched_title): (String, Option<String>, String) = conn
        .query_row(
            "SELECT title, title_custom, fetched_title FROM feeds WHERE id = ?1",
            [feed_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(title, "旧版里的名字", "历史未知标题不得在首次刷新时被吞");
    assert_eq!(custom.as_deref(), Some("旧版里的名字"));
    assert_eq!(fetched_title, "源站当前标题");
}

#[test]
fn legacy_url_title_follows_fetched_title_on_first_refresh() {
    let (_guard, db) = setup_sync("refresh_legacy_url_title");
    let conn = db.lock().unwrap();
    let url = "http://127.0.0.1/legacy-title-is-url.xml";
    let feed_id = insert_feed(&conn, url, url, "local");
    conn.execute(
        "UPDATE feeds SET fetched_title = title, title_custom = NULL WHERE id = ?1",
        [feed_id],
    )
    .unwrap();
    let attempt = db::begin_feed_refresh(&conn, feed_id).unwrap();
    let parsed = ingestion::ParsedFeed {
        title: Some("源站真实标题".into()),
        site_url: None,
        icon: None,
        articles: Vec::new(),
    };
    let fetched = ingestion::Fetched::Body {
        bytes: Vec::new(),
        content_type: None,
        etag: Some("W/\"legacy-url\"".into()),
        last_modified: None,
    };
    assert_eq!(
        ingestion::apply_refresh_result(
            &conn,
            feed_id,
            &ingestion::RefreshWriteContext::new(url, attempt.generation, None, None),
            &fetched,
            &parsed,
            false,
        )
        .unwrap(),
        ingestion::RefreshApplyOutcome::Applied(0)
    );
    let (title, custom, fetched_title): (String, Option<String>, String) = conn
        .query_row(
            "SELECT title, title_custom, fetched_title FROM feeds WHERE id = ?1",
            [feed_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(title, "源站真实标题");
    assert_eq!(
        custom.as_deref(),
        Some(""),
        "URL 兜底旧标题不得被冻结为用户自定义"
    );
    assert_eq!(fetched_title, "源站真实标题");
}

fn setup_sync(name: &str) -> (common::TempDbGuard, StdMutex<rusqlite::Connection>) {
    let path = common::unique_db_path(name);
    let guard = common::TempDbGuard::new(path.clone());
    let conn = db::open(&path).unwrap();
    db::create_folder(&conn, "测试", "article").unwrap();
    (guard, StdMutex::new(conn))
}

#[tokio::test]
async fn zero_new_refresh_reports_body_media_and_error_recovery_but_not_unchanged_304() {
    let server = common::LocalFeedServer::start_with_feed(|port| {
        format!(
            r#"<?xml version="1.0"?><rss version="2.0"><channel><title>Current</title>
        <item><title>stable</title><guid>stable</guid><link>http://127.0.0.1:{port}/article</link>
        <description><![CDATA[<p>current body</p>]]></description>
        <enclosure url="http://127.0.0.1:{port}/audio.mp3" type="audio/mpeg" length="42"/>
        </item></channel></rss>"#
        )
    })
    .await
    .unwrap();
    let (_guard, database) = setup("refresh_zero_new_changes").await;
    let (fid, aid) = {
        let conn = database.lock().await;
        let fid = insert_feed(&conn, &server.url("/local_feed.xml"), "Current", "local");
        let mut old = direct_article("stable", "stable");
        old.url = Some(server.url("/article"));
        old.content_html = Some("<p>previous body</p>".into());
        old.body_text = "previous body".into();
        let aid = db::upsert_article_with_feed(&conn, fid, &old, false)
            .unwrap()
            .0;
        (fid, aid)
    };
    let client = ingestion::build_client(3);
    let refresh = ingestion::refresh_feed_staged(&database, &client, fid, false)
        .await
        .unwrap();
    assert!(
        matches!(
            refresh,
            ingestion::FeedRefreshOutcome::Applied {
                new_articles: 0,
                data_changed: true,
                ..
            }
        ),
        "same GUID body replacement is an observable change: {refresh:?}"
    );
    let unchanged = ingestion::refresh_feed_staged(&database, &client, fid, false)
        .await
        .unwrap();
    assert!(
        matches!(
            unchanged,
            ingestion::FeedRefreshOutcome::Applied {
                new_articles: 0,
                data_changed: false,
                ..
            }
        ),
        "healthy unchanged 304 must not emit false data changes: {unchanged:?}"
    );
    {
        let conn = database.lock().await;
        conn.execute(
            "UPDATE feeds SET fetch_failed=1,fetch_error='temporary' WHERE id=?1",
            [fid],
        )
        .unwrap();
    }
    let recovered = ingestion::refresh_feed_staged(&database, &client, fid, false)
        .await
        .unwrap();
    assert!(
        matches!(
            recovered,
            ingestion::FeedRefreshOutcome::Applied {
                new_articles: 0,
                data_changed: true,
                ..
            }
        ),
        "304 recovery clears visible failure even without new content: {recovered:?}"
    );
    {
        let conn = database.lock().await;
        conn.execute("UPDATE articles SET enclosure_url='https://old.invalid/previous.mp3',enclosure_mime='audio/ogg' WHERE id=?1", [aid]).unwrap();
        db::set_feed_fetch_state(&conn, fid, false, None, None, None).unwrap();
    }
    let media = ingestion::refresh_feed_staged(&database, &client, fid, false)
        .await
        .unwrap();
    assert!(
        matches!(
            media,
            ingestion::FeedRefreshOutcome::Applied {
                new_articles: 0,
                data_changed: true,
                ..
            }
        ),
        "media-only metadata change must reach scheduler event routing: {media:?}"
    );
    let row = db::get_article(&*database.lock().await, aid)
        .unwrap()
        .unwrap();
    assert_eq!(row.enclosure_mime.as_deref(), Some("audio/mpeg"));
    assert_eq!(row.content_html.as_deref(), Some("<p>current body</p>"));
    server.stop().await;
}
