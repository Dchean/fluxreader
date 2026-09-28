//! TASK-097：增量窗口收口——「拉取进行中服务端变更的条目，下一轮增量必须拉回」。
//!
//! 缺陷形态（修前）：greader pull 成功后写「拉取结束墙钟」游标；id 列举发生在
//! 拉取过程内，changed_at 落在「id 列举之后 ~ 拉取结束之前」的服务端变更既不在
//! 本轮结果里，又因 changed_at < 结束游标被排除在下一轮增量外——只能等全量对账
//! 补回（拉取越慢漏得越多）。修后：游标候选在 id 列举**开始前**取（拉取起点
//! 墙钟），成功时写起点（无漏无重论证见 src-tauri/src/sync/greader_pull.rs）。
//!
//! 确定性设计（绝不与真实时钟赛跑，无 sleep）：经
//! `sync::set_greader_pull_clock_override` 把 greader pull 的游标墙钟注入为固定
//! 过去值 V（2023）；「拉取中变更」的条目 B 在 mock 的首轮 reading-list id 列举
//! 响应快照之后插入，changed_at = V+100。于是：
//! - 修后：第一轮游标 = V ≤ changed_at(B) ⇒ 第二轮增量（ot=V）必列出 B 并合并；
//! - 修前（变异：游标写回结束真实墙钟）：游标 = 真实 now（2026）> changed_at(B)
//!   ⇒ B 被跳过，断言确定性失败（changed_at 是固定过去常量，真实 now 恒大于它）。
//!
//! 变异红绿证据：tmp/task-097/（red.log = 修前失败，green.log = 修后通过）。
//! 运行：cargo test --test pull_window_e2e

mod common;
mod mock_greader;

use app_lib::db;
use app_lib::sync;
use mock_greader::MockGReader;
use std::sync::Arc;
use tokio::sync::Mutex;

#[tokio::test]
async fn entry_changed_during_pull_is_pulled_by_next_increment() {
    const T0: i64 = 1_700_000_000; // 种子的旧游标（2023-11）
    const V: i64 = T0 + 500; // 注入的「拉取起点」墙钟 = 修后第一轮写入的游标
    const T_B: i64 = V + 100; // 拉取中变更条目的 changed_at：T0 < V < T_B << 真实 now

    let server = MockGReader::start().await.expect("start mock server");
    let tmp = common::unique_db_path("pull_window");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).expect("open db");
    db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
    db::set_setting(&conn, "greader_username", "test").unwrap();
    db::set_setting(&conn, "greader_password", "test-token").unwrap();
    let http = app_lib::ingestion::build_client(10);
    let db = Arc::new(Mutex::new(conn));

    // 建立源绑定（mock feed 10 ↔ 本地源），拉到的条目才有合并目标
    // （同 pull_cursor_e2e::pull_id_listing_failure_keeps_last_sync_ts 的做法）
    sync::feeds_phase(&db, &http).await.expect("feeds phase");

    // 第一轮增量窗口内的既有条目（changed_at = T0+10，在 T0 与 V 之间：
    // 修后第二轮 ot=V 不会再列出它，用于同时锁定「无重复」）
    server.add_entry_with_times(
        10,
        "https://example.com/win/a",
        "A",
        false,
        false,
        T0 + 10,
        T0 + 10,
    );

    // 「拉取进行中变更」的条目：注册为首轮 reading-list id 列举响应快照之后插入，
    // changed_at = T_B 落在受控起点游标 V 与修前结束墙钟游标（真实 now）之间
    server.arm_inject_entry_after_first_reading_list_ids(
        10,
        "https://example.com/win/b",
        "B",
        false,
        false,
        T_B,
    );

    {
        let conn = db.lock().await;
        db::set_last_sync_ts(&conn, T0).unwrap();
    }

    // 注入游标墙钟（= 受控拉取起点），跑第一轮增量拉取
    sync::set_greader_pull_clock_override(Some(V));
    let r1 = sync::states_phase(&db, &http, false)
        .await
        .expect("round 1");
    assert!(r1.errors.is_empty(), "第一轮不应有错误：{:?}", r1.errors);
    {
        let conn = db.lock().await;
        // 注入发生在首轮列举快照之后：B 不允许出现在第一轮结果里
        let b_after_r1: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM articles WHERE url = ?1",
                rusqlite::params!["https://example.com/win/b"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(b_after_r1, 0, "首轮列举快照之后注入的条目不应在第一轮入库");
    }

    // 第二轮增量：修后 ot=V ≤ changed_at(B) ⇒ B 被拉回；
    // 修前 ot=真实 now > T_B ⇒ B 被跳过（本测试的确定性失败点）
    let r2 = sync::states_phase(&db, &http, false)
        .await
        .expect("round 2");
    assert!(r2.errors.is_empty(), "第二轮不应有错误：{:?}", r2.errors);

    sync::set_greader_pull_clock_override(None);

    {
        let conn = db.lock().await;
        let (a, b): (i64, i64) = conn
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM articles WHERE url = 'https://example.com/win/a'),
                    (SELECT COUNT(*) FROM articles WHERE url = 'https://example.com/win/b')",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            b, 1,
            "拉取进行中服务端变更的条目必须在下一轮增量被拉回（TASK-097 核心断言）"
        );
        assert_eq!(a, 1, "幂等合并：起点游标之前的既有条目不得重复入库");
        // 锁定 ①：成功推进写入的是「id 列举开始前的起点候选」V，而非结束墙钟
        assert_eq!(
            db::last_sync_ts(&conn).unwrap(),
            V,
            "游标必须等于 id 列举开始前取的起点候选（修前实现会写成结束墙钟 now）"
        );
    }

    let _ = std::fs::remove_file(&tmp);
}
