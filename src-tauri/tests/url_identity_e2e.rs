//! OPT-008A（审计 F12）：保守 URL 匹配 + 版本化存量重建的真实 open 回归。
//!
//! 场景：
//!   ① 模拟 v18 存量现场——旧算法把 /viewtopic.php?t=123 与 ?t=456 归到同一
//!      url_norm、旧布尔完成标记在、内容去重墓碑在、读/藏状态在、用户退订/
//!      目录墓碑在；生产 db::open 升级后：两个主题重新分开、原始 url 与状态
//!      原串保持、内容去重墓碑清空、用户墓碑保留、版本标记落下、索引不受影响；
//!   ② 重复 open 幂等：键与版本标记保持稳定；
//!   ③ 升级后的本地真实去重不退化：明确营销参数变体仍合并，业务参数不合并。
//!   ④ R1：旧算法时代的退订墓碑（带业务参数的订阅 URL 被旧算法剥参后的键）
//!      跨算法升级后仍须拦下复活（真实 feeds_phase），远端确实消失才回收；
//!      用户显式重新添加原 URL 时新旧命名空间一并解除。
//!
//! 运行：cargo test --test url_identity_e2e

mod common;
mod mock_greader;

use app_lib::db;
use app_lib::sync;
use mock_greader::MockGReader;
use rusqlite::OptionalExtension;
use std::sync::Arc;
use tokio::sync::Mutex;

/// 造「v18 存量现场」：用生产 open 建出最新 schema（v19 为纯数据迁移，v18 与
/// v19 的 schema 相同），再把迁移簿记回拨到 18 并植入旧算法产物，使下一次
/// open 走真实的「v18 → v19 + 版本化重算」升级路径。
fn seed_v18_legacy(path: &std::path::Path) {
    let conn = db::open(path).expect("open db");
    conn.execute_batch(
        r#"
        INSERT INTO feeds (feed_url, title) VALUES ('https://forum.example/rss', 'F');
        INSERT INTO articles (feed_id, guid, title, url, url_norm, is_read, is_starred) VALUES
          (1, 'g1', 'topic-123', 'https://forum.example/viewtopic.php?t=123',
           'http://forum.example/viewtopic.php', 1, 1),
          (1, 'g2', 'topic-456', 'https://forum.example/viewtopic.php?t=456',
           'http://forum.example/viewtopic.php', 0, 1);
        INSERT INTO deduped_urls (url, kept_aid)
          VALUES ('http://forum.example/viewtopic.php', 1);
        INSERT OR REPLACE INTO settings (key, value) VALUES
          ('url_norm_backfill_done', '1'),
          ('feed_tombstones', '["https://gone.example/rss"]'),
          ('folder_tombstones', '["旧目录"]');
        DELETE FROM settings WHERE key = 'url_norm_backfill_version';
        PRAGMA user_version = 18;
        "#,
    )
    .unwrap();
}

fn setting(conn: &rusqlite::Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
        r.get(0)
    })
    .optional()
    .unwrap()
}

/// ① v18 存量库经生产 open 升级：主题分开、原始数据/状态/索引/用户墓碑保全、
/// 内容去重墓碑清空、版本标记落下；② 重复 open 幂等。
#[test]
fn open_upgrade_rebuilds_url_norm_and_separates_topic_urls() {
    let tmp = common::unique_db_path("url_identity_upgrade");
    let _ = std::fs::remove_file(&tmp);
    seed_v18_legacy(&tmp);

    let conn = db::open(&tmp).expect("v18 存量库必须完成升级");

    // 版本推进与全新库一致（不硬编码版本号）
    let fresh_tmp = common::unique_db_path("url_identity_fresh");
    let _ = std::fs::remove_file(&fresh_tmp);
    let fresh = db::open(&fresh_tmp).expect("open fresh db");
    let v: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    let latest: i64 = fresh
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, latest, "存量库必须升到与全新库相同的最新版本");
    drop(fresh);
    let _ = std::fs::remove_file(&fresh_tmp);

    // 原始 url 原串保持；url_norm 按当前算法重算、两条主题分开；读/藏状态原样
    let rows: Vec<(String, String, String, i64, i64)> = {
        let mut stmt = conn
            .prepare("SELECT guid, url, url_norm, is_read, is_starred FROM articles ORDER BY guid")
            .unwrap();
        stmt.query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
    };
    assert_eq!(rows.len(), 2, "存量行零丢失");
    assert_eq!(
        rows[0].1, "https://forum.example/viewtopic.php?t=123",
        "原始 url 不得被改写"
    );
    assert_eq!(
        rows[1].1, "https://forum.example/viewtopic.php?t=456",
        "原始 url 不得被改写"
    );
    assert_eq!(
        rows[0].2,
        db::normalize_url(&rows[0].1),
        "url_norm 必须由原始 url 重算"
    );
    assert_eq!(rows[1].2, db::normalize_url(&rows[1].1));
    assert_ne!(
        rows[0].2, rows[1].2,
        "不同主题号升级后必须是不同匹配键（F12）"
    );
    assert_eq!((rows[0].3, rows[0].4), (1, 1), "读/藏状态原样");
    assert_eq!((rows[1].3, rows[1].4), (0, 1), "读/藏状态原样");

    // 旧布尔标记清除、版本标记写下；内容去重墓碑清空、用户删除墓碑保留
    assert_eq!(
        setting(&conn, "url_norm_backfill_done"),
        None,
        "旧完成标记不得残留"
    );
    assert!(
        setting(&conn, "url_norm_backfill_version").is_some(),
        "成功重算必须落下版本标记"
    );
    assert_eq!(
        setting(&conn, "feed_tombstones_legacy_v1").as_deref(),
        Some("[\"https://gone.example/rss\"]"),
        "旧算法退订墓碑必须迁入 legacy 命名空间（字节保留，不反推原始 URL）"
    );
    assert_eq!(
        setting(&conn, "feed_tombstones"),
        None,
        "原墓碑键清空：新删除按新算法键写入，两个命名空间不得混用"
    );
    assert_eq!(
        setting(&conn, "folder_tombstones").as_deref(),
        Some("[\"旧目录\"]"),
        "目录删除墓碑绝不删除"
    );
    let dedup_left: i64 = conn
        .query_row("SELECT COUNT(*) FROM deduped_urls", [], |r| r.get(0))
        .unwrap();
    assert_eq!(dedup_left, 0, "内容去重墓碑清空以允许重拉");

    // 原查索引不受迁移影响：url_norm 查询仍走 idx_articles_url_norm；分页/状态索引原样
    let plan: String = conn
        .query_row(
            "EXPLAIN QUERY PLAN SELECT id FROM articles WHERE url_norm = ?1",
            [rows[0].2.as_str()],
            |r| r.get(3),
        )
        .unwrap();
    assert!(
        plan.contains("idx_articles_url_norm"),
        "url_norm 索引必须在升级后仍是查询计划的一部分：{plan}"
    );
    for idx in ["idx_articles_published_id", "idx_articles_read_published"] {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
                [idx],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "索引 {idx} 必须原样保留");
    }

    // ② 升级幂等：重复 open 键与版本标记保持稳定
    drop(conn);
    let conn = db::open(&tmp).expect("重复 open 必须成功");
    let (n1, n2): (String, String) = conn
        .query_row(
            "SELECT (SELECT url_norm FROM articles WHERE guid='g1'), (SELECT url_norm FROM articles WHERE guid='g2')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        n1,
        db::normalize_url("https://forum.example/viewtopic.php?t=123")
    );
    assert_eq!(
        n2,
        db::normalize_url("https://forum.example/viewtopic.php?t=456")
    );
    assert!(setting(&conn, "url_norm_backfill_version").is_some());
    drop(conn);
    std::fs::remove_file(&tmp).expect("清理临时库失败");
}

/// ③ 升级后的本地真实去重：明确营销参数变体仍合并；业务参数不合并。
#[test]
fn dedup_after_upgrade_keeps_true_dedup_and_business_params_apart() {
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

    // 明确营销参数变体 → 同 URL 去重拦截
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
    assert!(!new2, "utm/fbclid 变体必须仍被去重");

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
    assert_eq!(n, 3, "3 篇入库、1 篇被真去重拦截");
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

/// 旧算法时代的删除现场：本地直连订阅被真实删除路径（record_feed_deletion）
/// 删掉后，把墓碑改写为旧算法键（旧算法会剥掉业务参数 t）并把迁移簿记回拨
/// v18，重新 open 走真实 v19 迁移。返回 (路径, 连接, HTTP 客户端, mock,
/// 订阅 URL, 旧算法键)。
async fn seed_deleted_feed_legacy_era(
    name: &str,
) -> (
    std::path::PathBuf,
    Arc<Mutex<rusqlite::Connection>>,
    reqwest::Client,
    Arc<MockGReader>,
    String,
    String,
) {
    let server = MockGReader::start().await.expect("start mock server");
    let tmp = common::unique_db_path(name);
    let _ = std::fs::remove_file(&tmp);

    // 远端订阅 URL 带业务参数 t：旧算法剥掉它写墓碑，新算法保留它
    let sub_url = format!("{}/local_feed.xml?t=123", server.url());
    let legacy_key = format!("http://127.0.0.1:{}/local_feed.xml", server.port);
    *server.subscriptions.lock().unwrap() = vec![mock_greader::MockSubscription {
        id: "feed/10".into(),
        title: "Deleted Feed".into(),
        url: sub_url.clone(),
        html_url: None,
        categories: vec![("Default".into(), "folder".into())],
    }];

    let conn = db::open(&tmp).expect("open db");
    db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
    db::set_setting(&conn, "greader_username", "test").unwrap();
    db::set_setting(&conn, "greader_password", "test-token").unwrap();
    db::create_folder(&conn, "F", "article").unwrap();
    db::insert_feed(
        &conn,
        &sub_url,
        None,
        "Deleted Feed",
        None,
        1,
        "inherit",
        true,
        false,
    )
    .unwrap();
    let http = app_lib::ingestion::build_client(10);
    let dbm = Arc::new(Mutex::new(conn));
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase (bind)");

    {
        let conn = dbm.lock().await;
        let fid: i64 = conn
            .query_row(
                "SELECT id FROM feeds WHERE feed_url = ?1",
                [&sub_url],
                |r| r.get(0),
            )
            .unwrap();
        let target =
            app_lib::commands::record_feed_deletion(&conn, fid).expect("在库订阅删除必须成功");
        assert!(
            target.is_some(),
            "已绑定远端：删除应返回待退订目标（测试不实际退订，远端保持列出）"
        );
        // 改写为旧算法时代现场：墓碑剥参键 + 旧标记 + 版本回拨
        let legacy_json = serde_json::to_string(&vec![legacy_key.clone()]).unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES ('feed_tombstones', ?1)",
            [legacy_json],
        )
        .unwrap();
        conn.execute_batch(
            "DELETE FROM settings WHERE key = 'feed_tombstones_legacy_v1';
             INSERT OR REPLACE INTO settings (key, value) VALUES ('url_norm_backfill_done', '1');
             PRAGMA user_version = 18;",
        )
        .unwrap();
    }
    drop(dbm);

    let conn = db::open(&tmp).expect("v18→v19 升级");
    (
        tmp,
        Arc::new(Mutex::new(conn)),
        http,
        server,
        sub_url,
        legacy_key,
    )
}

/// ④ R1 P2-1：删除发生在旧算法时代（墓碑键已剥业务参数）的订阅，跨算法升级后
/// 远端仍列出时不得复活（legacy 命名空间兼容匹配），且远端旧规范全集确实不含
/// 时才回收墓碑。判别力：缺 v19 墓碑迁移 / pull 只用新键匹配 / stale 清理按
/// 新键全集判定时，已删订阅被复活且旧墓碑被误清，断言必红。
#[tokio::test]
async fn cross_upgrade_legacy_feed_tombstone_blocks_resurrect_and_recycles() {
    let (tmp, dbm, http, server, sub_url, legacy_key) =
        seed_deleted_feed_legacy_era("url_identity_legacy_tomb").await;

    // 升级后远端仍列出（列表滞后）：不得复活；legacy 墓碑必须保持
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase after upgrade");
    {
        let conn = dbm.lock().await;
        let revived: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM feeds WHERE feed_url = ?1",
                [&sub_url],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            revived, 0,
            "跨算法升级后已退订订阅不得被 pull 复活（R1 P2-1）"
        );
        assert!(
            tombstone_list(&conn, "feed_tombstones_legacy_v1")
                .iter()
                .any(|k| k == &legacy_key),
            "远端仍列出（旧规范全集含该键）时 legacy 墓碑必须保持"
        );
        assert!(
            tombstone_list(&conn, "feed_tombstones").is_empty(),
            "当前命名空间不得混入旧算法键（旧键只存在于 legacy 命名空间）"
        );
    }

    // 远端真实消失 → 墓碑才回收（只有远端旧规范全集确实不存在时清）
    server.remove_remote_subscription("feed/10");
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase after remote removal");
    {
        let conn = dbm.lock().await;
        assert!(
            tombstone_list(&conn, "feed_tombstones_legacy_v1").is_empty(),
            "远端旧规范全集确实不含该键后，legacy 墓碑应回收"
        );
        let revived: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM feeds WHERE feed_url = ?1",
                [&sub_url],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(revived, 0, "远端确认消失后仍不得复活");
    }
    drop(dbm);
    std::fs::remove_file(&tmp).expect("清理临时库失败");
}

/// ④ R1：用户显式重新添加原 URL（与 commands::persist_new_feed 同一解除调用点）
/// 必须同时解除旧/新命名空间墓碑，之后 pull 才允许重建该订阅——其他路径
/// （退订 2xx / 远端列表滞后）不得主动删用户意图。
#[tokio::test]
async fn explicit_readd_clears_both_tombstone_namespaces() {
    let (tmp, dbm, http, server, sub_url, legacy_key) =
        seed_deleted_feed_legacy_era("url_identity_readd").await;

    {
        let conn = dbm.lock().await;
        assert!(
            tombstone_list(&conn, "feed_tombstones_legacy_v1")
                .iter()
                .any(|k| k == &legacy_key),
            "前置：升级后 legacy 墓碑在"
        );
        db::remove_feed_tombstone(&conn, &sub_url).expect("显式重新添加必须解除墓碑");
        assert!(
            tombstone_list(&conn, "feed_tombstones_legacy_v1").is_empty(),
            "用户显式重新添加原 URL 时旧命名空间墓碑必须解除"
        );
        assert!(
            tombstone_list(&conn, "feed_tombstones").is_empty(),
            "当前命名空间墓碑同步解除"
        );
    }

    // 墓碑已解除 → 远端仍列出，pull 正常重建（不再被压制）
    assert!(
        server
            .remote_subscription_ids()
            .iter()
            .any(|id| id == "feed/10"),
        "前置：远端仍列出该订阅（重建必须来自 pull 对账）"
    );
    sync::feeds_phase(&dbm, &http)
        .await
        .expect("feeds phase after readd");
    {
        let conn = dbm.lock().await;
        let revived: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM feeds WHERE feed_url = ?1",
                [&sub_url],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(revived, 1, "显式重新添加后 pull 应能正常重建该订阅");
    }
    drop(dbm);
    std::fs::remove_file(&tmp).expect("清理临时库失败");
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
