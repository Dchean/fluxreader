//! TASK-068：pull 分块失败时的游标守卫端到端测试。
//! 缺陷形态（修前）：item_contents 分块失败仅 continue，随后 set_last_sync_ts
//! 无条件推进——失败块的条目本轮丢失且游标已前移，下一轮增量从新游标起步，
//! 这些条目只能等全量同步补回（「偶发漏文章」的结构性温床）。
//! 修后：chunk_failures > 0 时保持旧游标，下一轮重拉同一窗口（合并幂等）。
//! 运行：cargo test --test pull_cursor_e2e

mod mock_greader;

use app_lib::db;
use app_lib::sync;
use mock_greader::MockGReader;
use std::sync::Arc;
use tokio::sync::Mutex;

async fn setup(name: &str) -> (Arc<Mutex<rusqlite::Connection>>, reqwest::Client, Arc<MockGReader>, std::path::PathBuf) {
    let server = MockGReader::start().await.expect("start mock server");
    let tmp = std::env::temp_dir().join(format!(
        "fluxreader_cursor_{}_{}_{}.db",
        name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).expect("open db");
    db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
    db::set_setting(&conn, "greader_username", "test").unwrap();
    db::set_setting(&conn, "greader_password", "test-token").unwrap();
    let http = app_lib::ingestion::build_client(10);
    (Arc::new(Mutex::new(conn)), http, server, tmp)
}

/// 分块失败：report.errors 必须记录失败，且游标保持旧值不推进（守卫核心断言）。
#[tokio::test]
async fn pull_chunk_failure_keeps_last_sync_ts() {
    let (db, http, server, tmp) = setup("guard").await;
    server.add_entry(10, "https://example.com/a", "A", "unread", false);

    // 种子旧游标 T0
    let t0: i64 = 1_700_000_000;
    {
        let conn = db.lock().await;
        db::set_last_sync_ts(&conn, t0).unwrap();
    }

    server.set_fail_item_contents(true);
    let report = sync::states_phase(&db, &http, false).await.expect("states_phase ok");
    assert!(
        report.errors.iter().any(|e| e.contains("拉取条目正文失败")),
        "分块失败必须进入 report.errors"
    );
    {
        let conn = db.lock().await;
        assert_eq!(
            db::last_sync_ts(&conn).unwrap(),
            t0,
            "分块失败时游标不得推进（TASK-068 守卫；修前会推进到 now）"
        );
    }

    // 解除注入：下一轮重拉同一窗口，条目合并、游标正常推进
    server.set_fail_item_contents(false);
    let report2 = sync::states_phase(&db, &http, false).await.expect("states_phase ok");
    assert!(report2.errors.is_empty(), "恢复后不应再有错误");
    {
        let conn = db.lock().await;
        assert!(
            db::last_sync_ts(&conn).unwrap() > t0,
            "恢复后游标正常推进"
        );
    }

    let _ = std::fs::remove_file(&tmp);
}

/// 双向锚定：无失败时游标照常推进（守卫不误伤正常路径）。
#[tokio::test]
async fn pull_success_advances_last_sync_ts() {
    let (db, http, server, tmp) = setup("advance").await;
    server.add_entry(10, "https://example.com/b", "B", "unread", false);

    let t0: i64 = 1_700_000_000;
    {
        let conn = db.lock().await;
        db::set_last_sync_ts(&conn, t0).unwrap();
    }

    let report = sync::states_phase(&db, &http, false).await.expect("states_phase ok");
    assert!(report.errors.is_empty(), "正常路径不应有错误");
    {
        let conn = db.lock().await;
        assert!(db::last_sync_ts(&conn).unwrap() > t0, "成功后游标推进");
    }

    let _ = std::fs::remove_file(&tmp);
}

/// TASK-069 审查 F1：id 列举失败同样不得推进游标。
/// 修前形态：id 列举失败只 push errors + break ⇒ all_item_ids 为空 ⇒ 下游 chunks(100)
/// 一次都不执行 ⇒ chunk_failures 仍为 0 ⇒ 游标照常推进到 now，该窗口被静默跳过
/// （正是 N-硬1 要关闭的「偶发漏文章」形态；探针实测：连续 4 轮增量 errors=0 而
/// 新文章 present 仍为 0，只有 full=true 才补回）。
#[tokio::test]
async fn pull_id_listing_failure_keeps_last_sync_ts() {
    let (db, http, server, tmp) = setup("idfail").await;
    // 先建立本地源绑定（feed 10 ↔ 远端 10），否则条目无处可并——同 sync_content_e2e 的
    // 既有做法：feeds_phase 会把 mock 的订阅拉成本地源并写 remote_id。
    sync::feeds_phase(&db, &http).await.expect("feeds phase");
    server.add_entry(10, "https://example.com/c", "C", "unread", false);

    let t0: i64 = 1_700_000_000;
    {
        let conn = db.lock().await;
        db::set_last_sync_ts(&conn, t0).unwrap();
    }

    // 仅 reading-list 主列举失败：read/starred 对账仍成功（隔离被验证的路径）
    server.set_fail_reading_list_ids(true);
    let report = sync::states_phase(&db, &http, false).await.expect("states_phase ok");
    assert!(
        report.errors.iter().any(|e| e.contains("拉取条目 id 失败")),
        "id 列举失败必须进入 report.errors"
    );
    {
        let conn = db.lock().await;
        assert_eq!(
            db::last_sync_ts(&conn).unwrap(),
            t0,
            "id 列举失败时游标不得推进（F1 守卫；修前会推进到 now 并跳过该窗口）"
        );
    }
    // 修前形态下这些条目本轮就丢了，且因游标已前移，后续增量永远看不到它们
    let before: i64 = {
        let conn = db.lock().await;
        conn.query_row(
            "SELECT COUNT(*) FROM articles WHERE url = ?1",
            rusqlite::params!["https://example.com/c"],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(before, 0, "失败轮本就没有拿到条目正文");

    // 解除注入：下一轮重拉同一窗口，条目合并、游标正常推进（守卫不永久卡死）
    server.set_fail_reading_list_ids(false);
    let report2 = sync::states_phase(&db, &http, false).await.expect("states_phase ok");
    assert!(report2.errors.is_empty(), "恢复后不应再有错误");
    {
        let conn = db.lock().await;
        assert!(db::last_sync_ts(&conn).unwrap() > t0, "恢复后游标正常推进");
    }
    // 该窗口的条目确实补回（证明守卫达到「下一轮重拉同一窗口」的目的，且不重复）
    let after: i64 = {
        let conn = db.lock().await;
        conn.query_row(
            "SELECT COUNT(*) FROM articles WHERE url = ?1",
            rusqlite::params!["https://example.com/c"],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(after, 1, "恢复轮必须把失败窗口的条目补回（且不重复）");

    let _ = std::fs::remove_file(&tmp);
}
