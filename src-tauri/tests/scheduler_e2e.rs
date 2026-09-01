//! 调度器无头集成测试：到期判定 → 并发抓取 → 退避 → 设置实时生效。
//! 用本地 HTTP feed server（127.0.0.1:8765）+ 临时数据库，不依赖外网和 UI。
//!
//! 由于调度器核心循环绑定了 AppHandle（事件 emit），可测部分拆为两层：
//! 1. `db::feeds_due_for_refresh` 到期/退避判定（纯 SQL，直接断言）
//! 2. `refresh_feed_staged` 三段式管线（锁外 HTTP + 状态写回）
//!    ——并发重叠已有专门测试（staged_refresh_e2e），此处只测调度语义
//!
//! 运行：先 python -m http.server 8765 --bind 127.0.0.1（serve fixtures 目录）
//! 然后 cargo test --test scheduler_e2e -- --ignored --nocapture

use app_lib::db;
use app_lib::ingestion;
use std::sync::Arc;
use tokio::sync::Mutex;

const FEED_URL: &str = "http://127.0.0.1:8765/local_feed.xml";

/// 搭测试环境：临时 DB + 两个源（一个指向本地 server，一个指向死地址）
async fn setup() -> (Arc<Mutex<rusqlite::Connection>>, reqwest::Client, std::path::PathBuf) {
    let tmp = std::env::temp_dir().join("fluxreader_scheduler_test.db");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).expect("open db");
    let client = ingestion::build_client(30);
    (Arc::new(Mutex::new(conn)), client, tmp)
}

/// 刷新到期源：直接走三段式管线（生产同款，Semaphore 并发由调度器持有）
async fn refresh_due(
    db: &Arc<Mutex<rusqlite::Connection>>,
    client: &reqwest::Client,
    interval_min: i64,
) -> (usize, usize) {
    let due: Vec<i64> = {
        let conn = db.lock().await;
        db::feeds_due_for_refresh(&conn, interval_min, true).unwrap()
    };
    if due.is_empty() {
        return (0, 0);
    }
    let mut n = 0;
    let mut f = 0;
    for id in due {
        match ingestion::refresh_feed_staged(db, client, id, false).await {
            Ok(count) => n += count,
            Err(_) => f += 1,
        }
    }
    (n, f)
}

#[tokio::test]
#[ignore = "requires local feed server on 127.0.0.1:8765"]
async fn scheduler_due_backoff_and_interval_pipeline() {
    let (db, client, tmp) = setup().await;

    // ---------- 场景：1 个好源 + 1 个坏源 ----------
    {
        let conn = db.lock().await;
        db::create_folder(&conn, "技术", "article").unwrap();
    }
    let good_id = {
        let conn = db.lock().await;
        db::insert_feed(
            &conn, FEED_URL, None, "Local Test Feed", None, 1, "inherit", true, false,
        ).unwrap()
    };
    let bad_id = {
        let conn = db.lock().await;
        db::insert_feed(
            &conn, "http://127.0.0.1:1/dead.xml", None, "Dead Feed", None, 1, "inherit", true, false,
        ).unwrap()
    };

    // ---------- 1. 首轮：两个源都到期，好源抓到 2 条，坏源失败 ----------
    let (new_articles, failed_feeds) = refresh_due(&db, &client, 30).await;
    println!("first pass: new={new_articles} failed={failed_feeds}");
    assert_eq!(new_articles, 2, "good feed should ingest 2 articles");
    assert_eq!(failed_feeds, 1, "dead feed should count as failed");

    {
        let conn = db.lock().await;
        // 好源：成功清零
        let (ff, fc, nra): (i64, i64, Option<String>) = conn.query_row(
            "SELECT fetch_failed, fail_count, next_retry_at FROM feeds WHERE id = ?1",
            [good_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
        assert_eq!((ff, fc), (0, 0), "good feed must reset failure state");
        assert!(nra.is_none());
        // 坏源：fail_count=1，5 分钟后重试
        let (ff, fc, nra): (i64, i64, Option<String>) = conn.query_row(
            "SELECT fetch_failed, fail_count, next_retry_at FROM feeds WHERE id = ?1",
            [bad_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
        assert_eq!((ff, fc), (1, 1), "dead feed records first failure");
        assert!(nra.is_some(), "dead feed gets a retry window");
        // 到期判定：坏源在退避窗口内 → 不再 due（interval=0 下好源必然 due，跳过它）
        let due: Vec<i64> = db::feeds_due_for_refresh(&conn, 0, true).unwrap();
        assert!(!due.contains(&bad_id), "backoff must exclude failed feed");
    }

    // ---------- 2. 第二轮：全部被排除（好源刚抓过、坏源退避中）----------
    let (n, f) = refresh_due(&db, &client, 30).await;
    assert_eq!((n, f), (0, 0), "second immediate pass must be a no-op");

    // ---------- 3. 坏源退避过期后 → 回到 due 集合 ----------
    {
        let conn = db.lock().await;
        conn.execute(
            "UPDATE feeds SET next_retry_at = datetime('now', '-1 minute') WHERE id = ?1",
            [bad_id]).unwrap();
    }
    let due: Vec<i64> = {
        let conn = db.lock().await;
        db::feeds_due_for_refresh(&conn, 0, true).unwrap()
    };
    assert!(due.contains(&bad_id), "expired backoff returns feed to due set");

    // ---------- 4. 好源间隔未到 → 不 due；间隔设 0 → due ----------
    {
        let conn = db.lock().await;
        let due = db::feeds_due_for_refresh(&conn, 30, true).unwrap();
        assert!(!due.contains(&good_id), "within interval → not due");
        let due = db::feeds_due_for_refresh(&conn, 0, true).unwrap();
        assert!(due.contains(&good_id), "interval=0 → always due");
    }

    // ---------- 5. autoRefresh=false 语义（调度循环里读设置决定，这里验证读取函数可用）----------
    {
        let conn = db.lock().await;
        db::set_setting(&conn, "app_settings", r#"{"autoRefresh":false,"refreshInterval":45}"#).unwrap();
    }
    let raw = {
        let conn = db.lock().await;
        db::get_setting(&conn, "app_settings").unwrap().unwrap()
    };
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(v["autoRefresh"], false, "settings round-trip for scheduler");

    let _ = std::fs::remove_file(&tmp);
    println!("ALL SCHEDULER ASSERTIONS PASSED");
}

/* ============================================================
   同步模式（syncMode）过滤语义：hybrid 跳过 Miniflux 源，direct 全含，
   手动全量入口（feeds_all_ids(true)）不受模式影响。
   ============================================================ */

fn seed_mode_feeds(conn: &rusqlite::Connection) -> (i64, i64) {
    let folder = db::create_folder(conn, "模式", "article").unwrap();
    let direct = db::insert_feed_origin(
        conn, "http://127.0.0.1:1/direct.xml", None, "Direct", None, folder, "inherit", true, false, "local",
    ).unwrap();
    let mf = db::insert_feed_origin(
        conn, "http://127.0.0.1:1/remote.xml", None, "Remote", None, folder, "inherit", true, false, "miniflux",
    ).unwrap();
    (direct, mf)
}

#[test]
fn sync_mode_hybrid_skips_miniflux_feeds_in_due_query() {
    let tmp = std::env::temp_dir().join(format!("fluxreader_mode_{}.db", line!()));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();
    let (direct, mf) = seed_mode_feeds(&conn);

    // hybrid（include_miniflux=false）：Miniflux 源被跳过
    let due = db::feeds_due_for_refresh(&conn, 0, false).unwrap();
    assert!(due.contains(&direct), "direct feed still refreshed in hybrid mode");
    assert!(!due.contains(&mf), "miniflux feed must be skipped in hybrid mode");

    // direct（include_miniflux=true）：全部包含（旧行为）
    let due_all = db::feeds_due_for_refresh(&conn, 0, true).unwrap();
    assert!(due_all.contains(&direct) && due_all.contains(&mf), "direct mode refreshes all feeds");

    // 未设置 syncMode 时 scheduler 默认 direct（read_sync_mode_conn 是私有函数，
    // 通过行为验证：due 查询 include=true 的调用在 scheduler 内部由该默认驱动）
}

#[test]
fn sync_mode_manual_refresh_always_includes_miniflux_feeds() {
    let tmp = std::env::temp_dir().join(format!("fluxreader_mode_{}.db", line!()));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();
    let (direct, mf) = seed_mode_feeds(&conn);

    // 手动/托盘全量入口：无论模式，始终全部源（模式只影响后台定时）
    let all = db::feeds_all_ids(&conn, true).unwrap();
    assert!(all.contains(&direct) && all.contains(&mf), "manual refresh includes all feeds");
}

#[test]
fn sync_mode_default_is_direct_when_unset() {
    let tmp = std::env::temp_dir().join(format!("fluxreader_mode_{}.db", line!()));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();
    // 未写 app_settings → 读到默认 "direct"（scheduler 的判定依赖此默认）
    // 复刻 scheduler 私有函数的读取逻辑：
    let mode = crate::db::get_setting(&conn, "app_settings")
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("syncMode").and_then(|m| m.as_str()).map(String::from))
        .unwrap_or_else(|| "direct".into());
    assert_eq!(mode, "direct", "unset syncMode must default to direct (legacy behavior)");

    // 写入 hybrid → 读回 hybrid
    db::set_setting(&conn, "app_settings", r#"{"syncMode":"hybrid"}"#).unwrap();
    let mode2 = crate::db::get_setting(&conn, "app_settings")
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("syncMode").and_then(|m| m.as_str()).map(String::from))
        .unwrap_or_else(|| "direct".into());
    assert_eq!(mode2, "hybrid");
}
