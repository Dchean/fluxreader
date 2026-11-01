//! W3-1 / W3-2：同步会话缓存与有界并发拉取的端到端锁定。
//!
//! 会话缓存（按账号身份 + 代际，TTL 10 分钟）场景：
//! ① 命中缓存零网络——连续同步只有第一次 ClientLogin，会话功能等价；
//! ② 401/认证类失败回落——缓存会话被服务端吊销（ClientLogin 仍可通过）
//!    时，剔除缓存、重认证并重试一次，且**只**重试一次；回落重建的会话
//!    重新入缓存，服务端恢复后零认证直接复用；
//! ③ 代际失配 = 缓存 miss——账号提交推进代际后不复用旧会话。
//!
//! 有界并发（批内 ≤ 3、按原块顺序合并）场景：
//! ④ 多块正文并发拉取的失败计数与游标守卫不变量——>100 条分多块，全部
//!    分块失败时 chunk_failures 如实计数、游标保留；服务端恢复后同一窗口
//!    重拉并全量合并（合并幂等，无重复条目）。
//!
//! 运行：cargo test --test session_cache_e2e

mod common;
mod mock_greader;

use app_lib::db;
use app_lib::sync;
use mock_greader::MockGReader;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::sync::Mutex;

/// 单个测试进程内串行化：会话缓存与 PUSH_LOCK/PULL_LOCK 是进程级静态，
/// 且各用例要在起点清空缓存——串行避免并发用例互相污染认证计数。
async fn serialize() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    LOCK.lock().await
}

async fn setup(
    name: &str,
) -> (
    Arc<Mutex<rusqlite::Connection>>,
    reqwest::Client,
    Arc<MockGReader>,
    PathBuf,
) {
    // 启用会话缓存并清空残留（仅本测试二进制注册；其他测试二进制不注册，
    // sync 引擎行为与无缓存时代逐字一致）。
    sync::init_session_cache();
    sync::clear_session_cache_for_tests();
    let server = MockGReader::start().await.expect("start mock server");
    let tmp = common::unique_db_path(&format!("session_cache_{name}"));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).expect("open db");
    db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
    db::set_setting(&conn, "greader_username", "test").unwrap();
    db::set_setting(&conn, "greader_password", "test-token").unwrap();
    let http = app_lib::ingestion::build_client(10);
    (Arc::new(Mutex::new(conn)), http, server, tmp)
}

/// ① 命中缓存零网络：首次同步认证一次，后续同步（light / full）零 ClientLogin，
/// 且缓存会话功能等价（条目照常拉取入库）。
#[tokio::test]
async fn second_sync_reuses_cached_session_without_relogin() {
    let _serial = serialize().await;
    let (db, http, server, tmp) = setup("hit").await;
    server.add_entry(10, "https://example.com/cache-hit-1", "H1", "unread", false);

    sync::feeds_phase(&db, &http).await.expect("feeds phase");
    assert_eq!(
        server.login_request_count(),
        1,
        "首次同步必须恰好认证一次：{:?}",
        server.login_request_count()
    );

    let report = sync::states_phase(&db, &http, false)
        .await
        .expect("states light");
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(
        server.login_request_count(),
        1,
        "第二次同步必须复用缓存会话（零认证请求）"
    );

    server.clear_request_log();
    let full = sync::states_phase(&db, &http, true)
        .await
        .expect("states full");
    assert!(full.errors.is_empty(), "{:?}", full.errors);
    assert_eq!(
        server.login_request_count(),
        0,
        "全量同步同样必须复用缓存会话"
    );
    {
        let conn = db.lock().await;
        let bound: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM articles WHERE remote_id IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(bound >= 1, "缓存会话必须照常拉取条目（bound={bound}）");
    }
    let _ = std::fs::remove_file(&tmp);
}

/// ② 401 回落：缓存会话被吊销 → 剔除缓存重认证（恰好一次）→ 重试一次且只
/// 重试一次（重试仍 401 则按 fail-closed 计失败、游标保留）；服务端恢复后
/// 复用回落重建的会话，零认证恢复正常拉取并推进游标。
#[tokio::test]
async fn auth_failure_on_cached_session_reauths_and_retries_once() {
    let _serial = serialize().await;
    let (db, http, server, tmp) = setup("auth401").await;
    sync::feeds_phase(&db, &http).await.expect("feeds phase");
    server.add_entry(10, "https://example.com/revoke-1", "R1", "unread", false);
    let t0: i64 = 1_700_000_000;
    {
        let conn = db.lock().await;
        db::set_last_sync_ts(&conn, t0).unwrap();
    }

    // 吊销会话 token（ClientLogin 不受影响——服务端仍接受凭据）。
    server.api_unauthorized.store(true, Ordering::SeqCst);
    let report = sync::states_phase(&db, &http, false)
        .await
        .expect("states_phase 返回 Ok（失败经 report.errors 上报）");
    assert_eq!(
        server.login_request_count(),
        2,
        "认证类失败必须触发恰好一次重认证（1 次建连 + 1 次回落）：{:?}",
        server.login_request_count()
    );
    assert!(
        report.errors.iter().any(|e| e.contains("→ 401")),
        "认证类失败必须进入 report.errors：{:?}",
        report.errors
    );
    {
        let conn = db.lock().await;
        assert_eq!(
            db::last_sync_ts(&conn).unwrap(),
            t0,
            "重试仍失败时游标必须保留（fail-closed）"
        );
    }

    // 服务端恢复：回落重建的会话已在缓存——零认证、正常拉取、游标推进。
    server.api_unauthorized.store(false, Ordering::SeqCst);
    server.clear_request_log();
    let recovered = sync::states_phase(&db, &http, false)
        .await
        .expect("recovered states");
    assert!(recovered.errors.is_empty(), "{:?}", recovered.errors);
    assert_eq!(
        server.login_request_count(),
        0,
        "恢复轮必须复用回落重建的会话"
    );
    {
        let conn = db.lock().await;
        assert!(db::last_sync_ts(&conn).unwrap() > t0, "恢复轮必须推进游标");
    }
    let _ = std::fs::remove_file(&tmp);
}

/// ③ 代际失配 = 缓存 miss：账号提交推进代际（同账号改密码的等价形态）后，
/// 旧会话不得复用——下一轮重新认证。
#[tokio::test]
async fn generation_bump_invalidates_cached_session() {
    let _serial = serialize().await;
    let (db, http, server, tmp) = setup("generation").await;
    sync::feeds_phase(&db, &http).await.expect("feeds phase");
    assert_eq!(server.login_request_count(), 1);

    {
        let conn = db.lock().await;
        db::bump_sync_generation(&conn).unwrap();
    }
    server.clear_request_log();
    let report = sync::states_phase(&db, &http, false).await.expect("states");
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(
        server.login_request_count(),
        1,
        "代际失配必须使缓存失效并重新认证"
    );
    let _ = std::fs::remove_file(&tmp);
}

/// ④ 多块并发拉取的失败不变量：>100 条分多块，全部块失败 → chunk_failures
/// 如实计数（每块一条）、游标保留；恢复后同一窗口重拉并全量合并、无重复。
#[tokio::test]
async fn bounded_concurrent_chunk_failures_keep_cursor_then_recover() {
    let _serial = serialize().await;
    let (db, http, server, tmp) = setup("batch").await;
    sync::feeds_phase(&db, &http).await.expect("feeds phase");
    let total: usize = 250; // 3 块（100/100/50），覆盖两轮并发批
    for i in 0..total {
        server.add_entry(
            10,
            &format!("https://example.com/batch-{i}"),
            &format!("B{i}"),
            "unread",
            false,
        );
    }
    let t0: i64 = 1_700_000_000;
    {
        let conn = db.lock().await;
        db::set_last_sync_ts(&conn, t0).unwrap();
    }

    server.set_fail_item_contents(true);
    let failed = sync::states_phase(&db, &http, false).await.expect("states");
    let chunk_failures = failed
        .errors
        .iter()
        .filter(|e| e.contains("拉取条目正文失败"))
        .count();
    assert_eq!(
        chunk_failures, 3,
        "每块失败必须各计一次（并发批次不改变失败计数口径）：{:?}",
        failed.errors
    );
    {
        let conn = db.lock().await;
        assert_eq!(
            db::last_sync_ts(&conn).unwrap(),
            t0,
            "任一块失败游标必须保留"
        );
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "全部块失败时不得有正文落库");
    }

    server.set_fail_item_contents(false);
    let recovered = sync::states_phase(&db, &http, false).await.expect("states");
    assert!(recovered.errors.is_empty(), "{:?}", recovered.errors);
    assert_eq!(
        recovered.pulled_entries, total,
        "恢复轮必须重拉同一窗口并合并全部条目"
    );
    {
        let conn = db.lock().await;
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, total as i64, "合并幂等，无重复条目");
        assert!(db::last_sync_ts(&conn).unwrap() > t0, "成功轮推进游标");
    }
    let _ = std::fs::remove_file(&tmp);
}
