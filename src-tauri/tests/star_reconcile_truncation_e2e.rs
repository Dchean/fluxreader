//! P2-1（自检 2026-09-29）：starred 权威集合分页中断不得静默当「拿全」。
//!
//! 缺陷形态（修前）：`fetch_stream_ids` 对「有 continuation 但解析失败」与
//! 「有 continuation 但本页无可用 id」静默 break，把**截断**的 starred 权威集合
//! 交给 `reconcile_reader_state` —— 排在截断点之后的已收藏条目被误判为
//! 「远端已取消收藏」，本地星标静默丢失（无队列记录、无报错、不可恢复）。
//!
//! 修后：与主列举循环 TASK-069-F1 同口径——报错中止，调用方走既有 C-1 守卫
//! （「状态对账跳过：远端状态集合拉取失败」），本轮不合并远端状态；解除注入后
//! 对账正常完成、星标无损。
//!
//! 运行：cargo test --test star_reconcile_truncation_e2e

mod common;
mod mock_greader;

use app_lib::db;
use app_lib::sync;
use mock_greader::MockGReader;
use std::sync::Arc;
use tokio::sync::Mutex;

async fn setup(
    name: &str,
) -> (
    Arc<Mutex<rusqlite::Connection>>,
    reqwest::Client,
    Arc<MockGReader>,
    std::path::PathBuf,
) {
    let server = MockGReader::start().await.expect("start mock server");
    let tmp = common::unique_db_path(&format!("star_{name}"));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).expect("open db");
    db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
    db::set_setting(&conn, "greader_username", "test").unwrap();
    db::set_setting(&conn, "greader_password", "test-token").unwrap();
    let http = app_lib::ingestion::build_client(10);
    (Arc::new(Mutex::new(conn)), http, server, tmp)
}

async fn count_starred(db: &Arc<Mutex<rusqlite::Connection>>) -> i64 {
    let conn = db.lock().await;
    conn.query_row(
        "SELECT COUNT(*) FROM articles WHERE is_starred = 1",
        [],
        |r| r.get(0),
    )
    .unwrap()
}

/// 修前红：损坏的 continuation 使客户端只拿到 1/2 条 starred id，reconcile 把
/// 未列出的 S2 判为「远端已取消收藏」→ 本地星标 2 → 1（断言 starred_after==2 失败）。
/// 修后绿：fetch_stream_ids 报错中止 → C-1 守卫跳过本轮对账 → 星标保持 2，
/// 且 report.errors 带上「状态对账跳过 + continuation」；解除注入后对账正常、
/// 星标仍为 2。
#[tokio::test]
async fn starred_reconcile_aborts_on_corrupt_continuation() {
    let (db, http, server, tmp) = setup("corrupt").await;
    // 绑定 mock 的 feed/10 到本地源，远端条目才有合并目标（同 pull_cursor_e2e 做法）
    sync::feeds_phase(&db, &http).await.expect("feeds phase");

    // 远端事实：两条收藏条目
    server.add_entry(10, "https://example.com/star-1", "S1", "unread", true);
    server.add_entry(10, "https://example.com/star-2", "S2", "unread", true);

    // 首轮同步：两条都合并进本地并按权威集合标星（正常分页路径不回归）
    let report = sync::states_phase(&db, &http, false)
        .await
        .expect("states phase ok");
    assert!(
        report.errors.is_empty(),
        "首轮不应有错误: {:?}",
        report.errors
    );
    assert_eq!(
        count_starred(&db).await,
        2,
        "首轮两条收藏条目都应在本地标星"
    );

    // 注入：starred 流首页只回 1 条 + 非数字 continuation（模拟「还有更多页但损坏」）
    server.set_corrupt_starred_continuation(true);
    let report2 = sync::states_phase(&db, &http, false)
        .await
        .expect("states phase ok");
    assert!(
        report2
            .errors
            .iter()
            .any(|e| e.contains("状态对账跳过") && e.contains("continuation")),
        "损坏的权威集合必须中止本轮对账并上报（C-1 守卫口径）: {:?}",
        report2.errors
    );
    assert_eq!(
        count_starred(&db).await,
        2,
        "对账中止后星标不得被截断集合回滚（修前：S2 被静默取消收藏）"
    );

    // 解除注入：恢复后对账正常完成，星标无损
    server.set_corrupt_starred_continuation(false);
    let report3 = sync::states_phase(&db, &http, false)
        .await
        .expect("states phase ok");
    assert!(
        report3.errors.is_empty(),
        "恢复后不应有错误: {:?}",
        report3.errors
    );
    assert_eq!(count_starred(&db).await, 2, "恢复轮星标无损");

    let _ = std::fs::remove_file(&tmp);
}
