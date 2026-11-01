//! 2026-11-01 未读差分（light 轮 read 对账改形态）的等价性端到端锁定。
//!
//! 机制：light 轮不再全量拉 read 集合，改拉 reading-list + xt=read 的当前未读
//! 全集（fetch_stream_ids 的 P2-1 纪律保证 Ok ⟺ continuation 耗尽，完整性守卫
//! 是结构性的），与本地「已绑定未读」行差分出「远端新变已读」，经
//! conflict_policy 单向落地。本文件锁定四个等价性用例：
//! ① 远端新标已读 → 本地落已读（条目 published 早于游标使增量窗口不可达，
//!    差分是唯一 read 通道）；
//! ② 本地已读幂等 0 行（已读行不进差分域）；
//! ③ 远端已删除的未读条目不误标（候选缺席 curr_unread 但 contents 存在性
//!    校验确认「远端已不再跟踪」，不落地差分）；
//! ④ 未读集合请求失败 → 本轮整轮跳过差分（fail-closed），不误标；恢复后收敛。
//! ⑤ 探测阶段 contents 网络型失败 → 聚合诊断进 report（不静默）、候选不落地
//!    （不误标）；恢复后收敛。
//!
//! 运行：cargo test --test unread_diff_e2e

mod common;
mod mock_greader;

use app_lib::db;
use app_lib::sync;
use mock_greader::MockGReader;
use std::sync::Arc;
use tokio::sync::Mutex;

const T0: i64 = 1_700_000_000;
/// 用例硬超时：正常夹具的全部链路（认证 + 列举 + 正文 + 差分）是毫秒级回环
/// 请求，单个请求另有 build_client(10) 的 10s 总超时兜底；120s 只可能在
/// 「某段等待永远不满足」时触发。挂起形态在此转为带诊断的红色失败，
/// CI 的 Run tests 步骤不再可能被本套件永久占住。
const TEST_HARD_LIMIT: std::time::Duration = std::time::Duration::from_secs(120);

/// 用例体统一经硬超时驱动（挂起诊断锚点：超时消息携带用例名）。
async fn run_bounded(name: &str, case: impl std::future::Future<Output = ()>) {
    if tokio::time::timeout(TEST_HARD_LIMIT, case).await.is_err() {
        panic!(
            "unread_diff_e2e 用例 {name} 硬超时（{TEST_HARD_LIMIT:?}）：存在未在时限内收敛的等待"
        );
    }
}

async fn setup(
    name: &str,
) -> (
    Arc<Mutex<rusqlite::Connection>>,
    reqwest::Client,
    Arc<MockGReader>,
    std::path::PathBuf,
) {
    let server = MockGReader::start().await.expect("start mock server");
    let tmp = common::unique_db_path(&format!("unread_diff_{name}"));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).expect("open db");
    db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
    db::set_setting(&conn, "greader_username", "test").unwrap();
    db::set_setting(&conn, "greader_password", "test-token").unwrap();
    let http = app_lib::ingestion::build_client(10);
    (Arc::new(Mutex::new(conn)), http, server, tmp)
}

/// 夹具：feeds_phase 绑定 mock feed 10 → 本地源；远端加一条 published 早于
/// 游标的条目（增量窗口不可达）；本地直连入库同 URL 行并绑定 entry id，
/// 本地读状态由调用方指定。游标种子为 T0。返回 (本地行 id, 远端 entry id)。
async fn seed_bound_row(
    db: &Arc<Mutex<rusqlite::Connection>>,
    server: &MockGReader,
    http: &reqwest::Client,
    url: &str,
    remote_read: bool,
    local_read: bool,
) -> (i64, i64) {
    sync::feeds_phase(db, http).await.expect("feeds phase");
    let eid =
        server.add_entry_with_published(10, url, "Diff fixture", remote_read, false, T0 - 100);
    let aid = {
        let conn = db.lock().await;
        let feed: i64 = conn
            .query_row("SELECT id FROM feeds WHERE remote_id = 10", [], |r| {
                r.get(0)
            })
            .unwrap();
        let a = db::NewArticle {
            guid: format!("diff-{url}"),
            url: Some(url.into()),
            title: "Diff fixture".into(),
            author: None,
            summary: None,
            content_html: None,
            body_text: "b".into(),
            image_url: None,
            enclosure_url: None,
            enclosure_mime: None,
            duration_sec: None,
            published_at: Some("2026-01-01T00:00:00+00:00".into()),
            source: "direct".into(),
        };
        let (aid, _) = db::upsert_article_with_feed(&conn, feed, &a, false).unwrap();
        db::set_article_remote_id(&conn, aid, eid).unwrap();
        if local_read {
            db::set_read(&conn, aid, true).unwrap();
        }
        db::set_last_sync_ts(&conn, T0).unwrap();
        aid
    };
    (aid, eid)
}

async fn is_read(db: &Arc<Mutex<rusqlite::Connection>>, aid: i64) -> bool {
    let conn = db.lock().await;
    conn.query_row("SELECT is_read FROM articles WHERE id = ?1", [aid], |r| {
        r.get::<_, i64>(0).map(|v| v != 0)
    })
    .unwrap()
}

/// ① 差分主路径：远端已读（窗口外旧变更）→ 本地未读行落已读；恰好一次
/// 状态写入；游标照常推进。
#[tokio::test]
async fn diff_marks_locally_unread_row_read_when_remote_read() {
    run_bounded("mark_read", case_mark_read()).await;
}

async fn case_mark_read() {
    let (db, http, server, tmp) = setup("mark_read").await;
    let (aid, _eid) = seed_bound_row(
        &db,
        &server,
        &http,
        "http://127.0.0.1:8765/diff/a",
        true,
        false,
    )
    .await;
    assert!(!is_read(&db, aid).await, "前置：本地未读");

    let report = sync::states_phase(&db, &http, false).await.expect("light");
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(
        is_read(&db, aid).await,
        "远端新变已读必须经未读差分落地本地已读"
    );
    assert_eq!(report.merged_states, 1, "差分恰好一次状态写入");
    {
        let conn = db.lock().await;
        assert!(
            db::last_sync_ts(&conn).unwrap() > T0,
            "差分成功轮游标照常推进"
        );
    }
    let _ = std::fs::remove_file(&tmp);
}

/// ② 本地已读幂等：已读行不进差分域，整轮 0 行状态写入。
#[tokio::test]
async fn diff_is_idempotent_for_locally_read_rows() {
    run_bounded("idempotent", case_idempotent()).await;
}

async fn case_idempotent() {
    let (db, http, server, tmp) = setup("idempotent").await;
    let (aid, _eid) = seed_bound_row(
        &db,
        &server,
        &http,
        "http://127.0.0.1:8765/diff/b",
        true,
        true,
    )
    .await;
    assert!(is_read(&db, aid).await, "前置：本地已读");

    let report = sync::states_phase(&db, &http, false).await.expect("light");
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(is_read(&db, aid).await, "本地已读保持");
    assert_eq!(report.merged_states, 0, "已读行不进差分域：0 行写入");
    let _ = std::fs::remove_file(&tmp);
}

/// ③ 远端已删除的未读条目不误标：候选缺席 curr_unread，但 contents 校验
/// 确认远端已不再跟踪该 id → 不落地差分（本地未读守恒、无错误）。
#[tokio::test]
async fn diff_does_not_mark_deleted_unread_entries() {
    run_bounded("deleted", case_deleted()).await;
}

async fn case_deleted() {
    let (db, http, server, tmp) = setup("deleted").await;
    let (aid, eid) = seed_bound_row(
        &db,
        &server,
        &http,
        "http://127.0.0.1:8765/diff/c",
        false,
        false,
    )
    .await;

    // 首轮：条目仍在远端未读集合里，两端一致（无候选），本地未读守恒。
    let r1 = sync::states_phase(&db, &http, false)
        .await
        .expect("light 1");
    assert!(r1.errors.is_empty(), "{:?}", r1.errors);
    assert!(!is_read(&db, aid).await, "两端一致时不得有差分写入");

    // 模拟远端删除条目（从所有流消失）；本地行保持绑定与未读。
    server.entries.lock().unwrap().retain(|e| e.id != eid);

    let r2 = sync::states_phase(&db, &http, false)
        .await
        .expect("light 2");
    assert!(
        r2.errors.is_empty(),
        "删除不产生错误（校验未确认按跳过处理）: {:?}",
        r2.errors
    );
    assert!(
        !is_read(&db, aid).await,
        "远端已删除的未读条目不得被差分误标已读"
    );
    let _ = std::fs::remove_file(&tmp);
}

/// ④ 完整性守卫（请求失败形态）：未读集合列举失败 → 本轮整轮跳过差分
/// （fail-closed），不落地任何差分结果；解除注入后下一轮收敛。
#[tokio::test]
async fn diff_skips_entirely_when_unread_collection_fails() {
    run_bounded("incomplete", case_incomplete()).await;
}

async fn case_incomplete() {
    let (db, http, server, tmp) = setup("incomplete").await;
    let (aid, _eid) = seed_bound_row(
        &db,
        &server,
        &http,
        "http://127.0.0.1:8765/diff/d",
        true,
        false,
    )
    .await;

    server.set_fail_unread_stream_ids(true);
    let r1 = sync::states_phase(&db, &http, false)
        .await
        .expect("light 1");
    assert!(
        r1.errors
            .iter()
            .any(|e| e.contains("unread 权威集合") && e.contains("跳过")),
        "未读集合失败必须可见并声明跳过差分: {:?}",
        r1.errors
    );
    assert!(!is_read(&db, aid).await, "未读集合不完整时不得落地差分结果");

    server.set_fail_unread_stream_ids(false);
    let r2 = sync::states_phase(&db, &http, false)
        .await
        .expect("light 2");
    assert!(r2.errors.is_empty(), "{:?}", r2.errors);
    assert!(is_read(&db, aid).await, "恢复后差分收敛");
    let _ = std::fs::remove_file(&tmp);
}

/// ⑤ 探测网络故障可见性：contents 网络型失败（500）→ 聚合诊断必须进
/// report.errors（protocol 缺条形态才静默），候选不落地（不误标）；解除注入
/// 后下一轮差分收敛。
#[tokio::test]
async fn diff_reports_network_failures_during_existence_probe() {
    run_bounded("probe_network", case_probe_network()).await;
}

async fn case_probe_network() {
    let (db, http, server, tmp) = setup("probe_network").await;
    let (aid, _eid) = seed_bound_row(
        &db,
        &server,
        &http,
        "http://127.0.0.1:8765/diff/e",
        true,
        false,
    )
    .await;

    server.set_fail_item_contents(true);
    let r1 = sync::states_phase(&db, &http, false)
        .await
        .expect("light 1");
    assert!(
        r1.errors
            .iter()
            .any(|e| e.contains("存在性校验") && e.contains("未确认")),
        "contents 网络型失败必须聚合进 report.errors: {:?}",
        r1.errors
    );
    assert!(
        !is_read(&db, aid).await,
        "存在性校验未确认不得落地差分（不误标）"
    );

    server.set_fail_item_contents(false);
    let r2 = sync::states_phase(&db, &http, false)
        .await
        .expect("light 2");
    assert!(r2.errors.is_empty(), "{:?}", r2.errors);
    assert!(is_read(&db, aid).await, "恢复后差分收敛");
    let _ = std::fs::remove_file(&tmp);
}
