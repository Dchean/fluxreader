//! 当前格式 URL 候选规范化、持久 GUID 身份与墓碑失败关闭回归。

mod common;
mod mock_greader;

use app_lib::db;
use app_lib::sync;
use mock_greader::MockGReader;
use rusqlite::OptionalExtension;
use std::sync::Arc;
use tokio::sync::Mutex;

fn setting(conn: &rusqlite::Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
        r.get(0)
    })
    .optional()
    .unwrap()
}

#[test]
fn candidate_url_normalization_preserves_true_guids_and_business_parameters() {
    let tmp = common::unique_db_path("url_identity_dedup");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).expect("open db");
    conn.execute_batch("INSERT INTO feeds (feed_url, title) VALUES ('https://x.example/f', 'F');")
        .unwrap();

    let article = |url: &str, guid: &str| db::NewArticle {
        guid: guid.into(),
        url: Some(url.into()),
        title: "t".into(),
        author: None,
        summary: None,
        content_html: None,
        body_text: "b".into(),
        image_url: None,
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: None,
        source: "direct".into(),
    };

    // 营销参数只影响候选 URL 键；不同真实 GUID 都必须持久化。
    let (_, new1) =
        db::upsert_article_with_feed(&conn, 1, &article("https://n.example/story", "g1"), true)
            .unwrap();
    assert!(new1);
    let (_, new2) = db::upsert_article_with_feed(
        &conn,
        1,
        &article("https://n.example/story?utm_source=x&fbclid=y", "g2"),
        true,
    )
    .unwrap();
    assert!(new2, "不同真实 GUID 不得因规范化 URL 相同而丢失");

    // 业务参数（论坛主题号）→ 不同文章，必须各自入库
    let (_, new3) = db::upsert_article_with_feed(
        &conn,
        1,
        &article("https://forum.example/viewtopic.php?t=123", "g3"),
        true,
    )
    .unwrap();
    assert!(new3);
    let (_, new4) = db::upsert_article_with_feed(
        &conn,
        1,
        &article("https://forum.example/viewtopic.php?t=456", "g4"),
        true,
    )
    .unwrap();
    assert!(new4, "不同主题号不得被去重吞并");

    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 4, "四个真实 GUID 全部持久化");
    let keys: Vec<String> = conn
        .prepare("SELECT url_norm FROM articles ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(keys[0], keys[1], "已知营销参数仍规范化为同一个候选键");
    assert_ne!(keys[2], keys[3], "业务参数不能丢失");
    drop(conn);
    std::fs::remove_file(&tmp).expect("清理临时库失败");
}

/* ============================================================
R1：旧算法退订墓碑跨算法升级（真实 feeds_phase 驱动，无真实网络）
============================================================ */

fn tombstone_list(conn: &rusqlite::Connection, key: &str) -> Vec<String> {
    setting(conn, key)
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// ④ R1 保守权衡：旧墓碑键丢失了参数原值（原始 URL 不可反推、不能猜恢复），
/// 同一旧键的全部候选订阅会被一同抑制——宁可保守多拦，也不猜 raw 放行。
/// 只有远端旧规范全集确实不含该键时才回收。
#[test]
fn legacy_tombstone_suppresses_all_same_old_key_candidates() {
    let tmp = common::unique_db_path("url_identity_legacy_scope");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).expect("open db");
    // 旧算法墓碑：旧算法剥掉了 t，原值已不可知
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value) \
         VALUES ('feed_tombstones_legacy_v1', '[\"http://127.0.0.1:1/feed.xml\"]')",
        [],
    )
    .unwrap();
    // 两个不同 t 的候选都命中同一旧键 → 都压制（无法区分哪个是原订阅）
    assert!(db::feed_url_tombstoned(&conn, "http://127.0.0.1:1/feed.xml?t=123").unwrap());
    assert!(db::feed_url_tombstoned(&conn, "http://127.0.0.1:1/feed.xml?t=456").unwrap());
    // 不同路径不受旧墓碑影响（匹配范围就旧键本身，不扩大到全站）
    assert!(!db::feed_url_tombstoned(&conn, "http://127.0.0.1:1/other.xml?t=123").unwrap());
    // 远端旧规范全集确实不含该键 → 回收
    db::prune_feed_tombstones(&conn, &["http://127.0.0.1:1/something-else.xml".into()]).unwrap();
    assert!(
        db::legacy_feed_tombstones(&conn).unwrap().is_empty(),
        "远端旧规范全集不含该键时才回收 legacy 墓碑"
    );
    drop(conn);
    std::fs::remove_file(&tmp).expect("清理临时库失败");
}

/// ④ R2：墓碑存储损坏（畸形 JSON / BLOB 类型值，新与 legacy 命名空间各一组）
/// 意味着墓碑状态**未知**，而不是「没有墓碑」——真实 feeds_phase 必须记录
/// report 错误并停止订阅导入（0 次导入），且损坏的 setting 原值（含类型）保持
/// 不动，修复后可重试。判别力：把解析错吞成空表、或对读错误 unwrap_or(false)/
/// 仅 warn 继续导入时，订阅被复活，断言必红。
#[tokio::test]
async fn corrupted_tombstone_state_stops_import_and_preserves_value() {
    for (i, (key, kind)) in [
        ("feed_tombstones", "text"),
        ("feed_tombstones", "blob"),
        ("feed_tombstones_legacy_v1", "text"),
        ("feed_tombstones_legacy_v1", "blob"),
    ]
    .iter()
    .enumerate()
    {
        let server = MockGReader::start().await.expect("start mock server");
        let tmp = common::unique_db_path(&format!("url_identity_corrupt_{i}"));
        let _ = std::fs::remove_file(&tmp);
        let sub_url = format!("{}/local_feed.xml?t=123", server.url());
        *server.subscriptions.lock().unwrap() = vec![mock_greader::MockSubscription {
            id: "feed/10".into(),
            title: "Ghost Feed".into(),
            url: sub_url.clone(),
            html_url: None,
            categories: vec![("Default".into(), "folder".into())],
        }];

        let conn = db::open(&tmp).expect("open db");
        db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
        db::set_setting(&conn, "greader_username", "test").unwrap();
        db::set_setting(&conn, "greader_password", "test-token").unwrap();
        // 损坏值：畸形 JSON（TEXT）或合法 JSON 字节但 BLOB 类型（String 读取本身会失败）
        let original: rusqlite::types::Value = match *kind {
            "text" => rusqlite::types::Value::Text("{not json".into()),
            _ => rusqlite::types::Value::Blob(b"[]".to_vec()),
        };
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
            rusqlite::params![key, original.clone()],
        )
        .unwrap();
        let dbm = Arc::new(Mutex::new(conn));
        let http = app_lib::ingestion::build_client(10);

        let report = sync::feeds_phase(&dbm, &http)
            .await
            .expect("墓碑未知不是硬失败：必须返回报告并停止导入");
        assert!(
            report.errors.iter().any(|e| e.contains("墓碑")),
            "{key}/{kind}: 墓碑未知必须记入 report"
        );
        let conn = dbm.lock().await;
        let imported: i64 = conn
            .query_row("SELECT COUNT(*) FROM feeds", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            imported, 0,
            "{key}/{kind}: 墓碑未知时不得导入任何订阅（不复活）"
        );
        let after: rusqlite::types::Value = conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            after, original,
            "{key}/{kind}: 损坏的 setting 原值（含类型）必须原样保留"
        );
        drop(conn);
        drop(dbm);
        let _ = std::fs::remove_file(&tmp);
    }
}

/// ④ R2：显式 re-add（persist_new_feed 的真实解除调用）覆盖「新旧命名空间同时
/// 有记录」：两处对应键都清除、无关墓碑保留；解除动作并入调用方事务，后续步骤
/// 失败回滚时两侧墓碑原样（可重试补上）。
#[test]
fn readd_clears_both_nonempty_namespaces_and_rolls_back_retryable() {
    let tmp = common::unique_db_path("url_identity_readd_rollback");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).expect("open db");
    let sub_url = "http://127.0.0.1:1/local_feed.xml?t=123".to_string();
    let new_key = db::normalize_url(&sub_url);
    let legacy_key = "http://127.0.0.1:1/local_feed.xml".to_string();
    let seed = |key: &str, list: &[&str]| {
        let json = serde_json::to_string(list).unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
            [key, json.as_str()],
        )
        .unwrap();
    };
    seed(
        "feed_tombstones",
        &[new_key.as_str(), "http://keep.example/current"],
    );
    seed(
        "feed_tombstones_legacy_v1",
        &[legacy_key.as_str(), "http://keep.example/legacy"],
    );

    // 模拟 persist_new_feed 的调用形态：解除写并入调用方事务
    {
        let tx = conn.unchecked_transaction().unwrap();
        db::remove_feed_tombstone(&tx, &sub_url).unwrap();
        assert!(
            !tombstone_list(&conn, "feed_tombstones")
                .iter()
                .any(|k| k == &new_key),
            "事务内当前命名空间对应键已移除"
        );
        assert!(
            !tombstone_list(&conn, "feed_tombstones_legacy_v1")
                .iter()
                .any(|k| k == &legacy_key),
            "事务内 legacy 命名空间对应键已移除"
        );
        tx.rollback().unwrap();
    }
    // 失败回滚可补：两侧墓碑（含被解除的两个键）原样
    assert!(
        tombstone_list(&conn, "feed_tombstones")
            .iter()
            .any(|k| k == &new_key),
        "回滚后当前命名空间墓碑必须原样"
    );
    assert!(
        tombstone_list(&conn, "feed_tombstones_legacy_v1")
            .iter()
            .any(|k| k == &legacy_key),
        "回滚后 legacy 命名空间墓碑必须原样"
    );

    // 重试：只清对应键，无关墓碑保留
    db::remove_feed_tombstone(&conn, &sub_url).unwrap();
    let current = tombstone_list(&conn, "feed_tombstones");
    let legacy = tombstone_list(&conn, "feed_tombstones_legacy_v1");
    assert!(!current.iter().any(|k| k == &new_key));
    assert!(!legacy.iter().any(|k| k == &legacy_key));
    assert!(
        current.iter().any(|k| k == "http://keep.example/current"),
        "无关当前命名空间墓碑保留"
    );
    assert!(
        legacy.iter().any(|k| k == "http://keep.example/legacy"),
        "无关 legacy 墓碑保留"
    );
    drop(conn);
    std::fs::remove_file(&tmp).expect("清理临时库失败");
}
