//! OPT-008B 第一阶段（条目身份与实际消费者）：跨源同 URL 独立条目身份回归。
//!
//! 目标语义（OPT-008B 卡片第一阶段）：
//! - 条目是「订阅源 / 远端 ID」的独立实体：两个源推送同一 URL、远端 entry id
//!   不同时，本地必须各有持久条目（不因 URL 相同被吞掉/借位绑定）；
//! - 精确协议 entry ID 优先；URL 兜底只在同一订阅源内选行；
//! - 读广播（同文副本跨源传播）保留：本地标读 → 推送广播到同文副本的 remote_id；
//!   远端任一副本已读 → 拉取合并广播到同文其它条目；
//! - unread / star / unstar 只作用于当前条目（收藏隔离）；
//! - 重复 pull 幂等；删除/断开一个源不污染另一个同 URL 条目。
//!
//! 本文件先于实现编写（反例驱动）：实现前同 URL 跨源 entry 只有一行（或落在
//! CSV 记账里），断言必红；实现后两协议都必须各留两行并各自收敛。

mod common;
mod mock_greader;

use app_lib::db;
use app_lib::sync;
use mock_greader::MockGReader;
use std::sync::Arc;
use tokio::sync::Mutex;

const URL_SHARED: &str = "http://127.0.0.1:8765/post/shared";

async fn setup(
    name: &str,
) -> (
    Arc<Mutex<rusqlite::Connection>>,
    reqwest::Client,
    Arc<MockGReader>,
) {
    let server = MockGReader::start().await.expect("start mock server");
    let tmp = common::unique_db_path(&format!("identity_{name}"));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).expect("open db");
    db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
    db::set_setting(&conn, "greader_username", "test").unwrap();
    db::set_setting(&conn, "greader_password", "test-token").unwrap();
    let http = app_lib::ingestion::build_client(10);
    (Arc::new(Mutex::new(conn)), http, server)
}

/// 两个本地源：feed A（远端 feed 10）、feed B（远端 feed 11），各自归属一个分类。
/// 返回 (conn, feedA, feedB, folderA, folderB)。
fn seed_two_bound_feeds(conn: &rusqlite::Connection) -> (i64, i64, i64, i64) {
    let folder_a = db::create_folder(conn, "分类A", "article").unwrap();
    let folder_b = db::create_folder(conn, "分类B", "article").unwrap();
    let feed_a = db::insert_feed(
        conn,
        "http://127.0.0.1:8765/local_feed.xml",
        None,
        "源A",
        None,
        folder_a,
        "inherit",
        false,
        false,
    )
    .unwrap();
    let feed_b = db::insert_feed(
        conn,
        "http://example.com/remote-only.xml",
        None,
        "源B",
        None,
        folder_b,
        "inherit",
        false,
        false,
    )
    .unwrap();
    db::set_feed_remote_id(conn, feed_a, 10).unwrap();
    db::set_feed_remote_id(conn, feed_b, 11).unwrap();
    (feed_a, feed_b, folder_a, folder_b)
}

/// 某源下 URL 命中的 (article_id, remote_id)（同源同 URL 重放不得出现两行）。
fn row_of_feed(conn: &rusqlite::Connection, feed_id: i64, url: &str) -> (i64, Option<i64>) {
    conn.query_row(
        "SELECT id, remote_id FROM articles WHERE feed_id = ?1 AND url_norm = ?2",
        rusqlite::params![feed_id, db::normalize_url(url)],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .unwrap()
}

fn state_of(conn: &rusqlite::Connection, id: i64) -> (bool, bool) {
    conn.query_row(
        "SELECT is_read, is_starred FROM articles WHERE id = ?1",
        [id],
        |r| Ok((r.get::<_, i64>(0)? != 0, r.get::<_, i64>(1)? != 0)),
    )
    .unwrap()
}

/// ① Google Reader：两源同 URL、不同 entry id —— 必须各自独立入库；
/// 收藏隔离；本地标读广播到副本 remote_id；unstar 只命中自己的 remote_id；
/// 重复 pull 幂等且不换绑。
#[tokio::test]
async fn greader_cross_feed_same_url_keeps_independent_entries() {
    let (dbm, http, server) = setup("gr_cross").await;
    let (feed_a, feed_b, ..) = {
        let conn = dbm.lock().await;
        seed_two_bound_feeds(&conn)
    };

    // 服务端两条 entry：同 URL、不同 feed、不同 (远端分配) entry id
    let eid_a = server.add_entry_ret(10, URL_SHARED, "copy-10", false, false);
    let eid_b = server.add_entry_ret(11, URL_SHARED, "copy-11", false, false);
    assert_ne!(eid_a, eid_b, "夹具前提：两个远端 entry id 不同");

    sync::states_phase(&dbm, &http, true)
        .await
        .expect("full states phase");

    {
        let conn = dbm.lock().await;
        let total: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM articles WHERE url_norm = ?1",
                [db::normalize_url(URL_SHARED)],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            total, 2,
            "跨源同 URL + 不同 remote id 必须实际独立入库（修前被吞成 1 行 / CSV 记账）"
        );

        let (aid_a, bound_a) = row_of_feed(&conn, feed_a, URL_SHARED);
        let (aid_b, bound_b) = row_of_feed(&conn, feed_b, URL_SHARED);
        assert_eq!(bound_a, Some(eid_a), "源A 条目绑定自己的 remote id");
        assert_eq!(bound_b, Some(eid_b), "源B 条目绑定自己的 remote id");
        assert_ne!(aid_a, aid_b, "两行是不同的持久条目");

        // 不再需要 CSV 记账（两源都已绑定 → 各自成行）
        let dups: (String, String) = conn
            .query_row(
                "SELECT (SELECT remote_dup_ids FROM articles WHERE id = ?1),
                        (SELECT remote_dup_ids FROM articles WHERE id = ?2)",
                [aid_a, aid_b],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            dups,
            (String::new(), String::new()),
            "两源都已绑定后不得再走 CSV 记账"
        );
    }

    // ---- 收藏隔离：远端只收藏 B，对账不得碰 A ----
    {
        let mut es = server.entries.lock().unwrap();
        if let Some(e) = es.iter_mut().find(|e| e.id == eid_b) {
            e.starred = true;
            e.changed_at = chrono::Utc::now().timestamp();
        }
    }
    sync::states_phase(&dbm, &http, true)
        .await
        .expect("sync after star");
    {
        let conn = dbm.lock().await;
        let (aid_a, _) = row_of_feed(&conn, feed_a, URL_SHARED);
        let (aid_b, _) = row_of_feed(&conn, feed_b, URL_SHARED);
        assert!(state_of(&conn, aid_b).1, "远端收藏必须落到自己的条目");
        assert!(
            !state_of(&conn, aid_a).1,
            "收藏只当前条目：另一源的同文条目不得被连带收藏"
        );
    }

    // ---- 推送精确目标 + 读广播（同文副本）----
    {
        let conn = dbm.lock().await;
        let (aid_a, _) = row_of_feed(&conn, feed_a, URL_SHARED);
        let (aid_b, _) = row_of_feed(&conn, feed_b, URL_SHARED);
        // 取消 B 的收藏（unstar 只应命中 B 自己的 remote_id）
        db::set_starred(&conn, aid_b, false).unwrap();
        db::enqueue_sync(&conn, Some(aid_b), None, "unstar", None).unwrap();
        // 标读 A（read 应广播到同文副本 B 的 remote_id）
        db::set_read(&conn, aid_a, true).unwrap();
        db::enqueue_sync(&conn, Some(aid_a), None, "read", None).unwrap();
    }
    sync::push_states_now(&dbm, &http).await;
    {
        let updates = mock_greader::status_updates_map(&server);
        assert_eq!(
            updates.get(&eid_a).map(String::as_str),
            Some("read"),
            "标读必须推送自己的 remote_id"
        );
        assert_eq!(
            updates.get(&eid_b).map(String::as_str),
            Some("read"),
            "读广播必须覆盖同文副本的 remote_id（双端：手机侧副本也已读）"
        );
        let toggles = server.bookmark_toggles.lock().unwrap().clone();
        assert!(
            toggles.contains(&eid_b) && !toggles.contains(&eid_a),
            "unstar 只命中当前条目的 remote_id（收藏不广播）: {toggles:?}"
        );
    }

    // ---- 重复 pull 幂等：行数不变、绑定不变、状态保持 ----
    sync::states_phase(&dbm, &http, true)
        .await
        .expect("repeat full pull");
    {
        let conn = dbm.lock().await;
        let total: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM articles WHERE url_norm = ?1",
                [db::normalize_url(URL_SHARED)],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(total, 2, "重复 pull 不得复制条目");
        let (aid_a, bound_a) = row_of_feed(&conn, feed_a, URL_SHARED);
        let (aid_b, bound_b) = row_of_feed(&conn, feed_b, URL_SHARED);
        assert_eq!(bound_a, Some(eid_a), "重复 pull 不得换绑");
        assert_eq!(bound_b, Some(eid_b), "重复 pull 不得换绑");
        assert!(state_of(&conn, aid_a).0, "已读状态在幂等重拉后保持");
        assert!(state_of(&conn, aid_b).0, "推送广播后的副本保持已读");
    }

    // ---- 删除一个源不污染另一个同 URL 条目 ----
    {
        let conn = dbm.lock().await;
        db::delete_feed(&conn, feed_b).unwrap();
        let total: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM articles WHERE url_norm = ?1",
                [db::normalize_url(URL_SHARED)],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(total, 1, "删除源B 只能删自己的条目");
        let (aid_a, bound_a) = row_of_feed(&conn, feed_a, URL_SHARED);
        assert_eq!(bound_a, Some(eid_a), "源A 条目绑定与内容不受删除源B 影响");
        assert!(state_of(&conn, aid_a).0, "源A 读状态保留");
    }
}

/// ①b Google Reader：两源同 URL 的**未绑定直连行**已存在时，full pull 必须把
/// 每条 entry 绑到**自己源的那一行**——不得让 URL 兜底永远只命中最小 id 行、
/// 另一行的 entry 被记成副本（旧行为：第二条 entry 进 CSV，行永不绑定）。
#[tokio::test]
async fn greader_full_pull_binds_each_same_url_row_to_its_own_entry() {
    let (dbm, http, server) = setup("gr_bind").await;
    let (feed_a, feed_b, ..) = {
        let conn = dbm.lock().await;
        let (feed_a, feed_b, ..) = seed_two_bound_feeds(&conn);
        // 直连风格两行：同 URL、各自源、均未绑定（模拟先直连后接同步）
        let article = |guid: &str| db::NewArticle {
            guid: guid.into(),
            url: Some(URL_SHARED.into()),
            title: "direct copy".into(),
            author: None,
            summary: None,
            content_html: Some("<p>c</p>".into()),
            body_text: "c".into(),
            image_url: None,
            enclosure_url: None,
            enclosure_mime: None,
            duration_sec: None,
            published_at: Some(chrono::Utc::now().to_rfc3339()),
            source: "direct".into(),
        };
        db::upsert_article_with_feed(&conn, feed_a, &article("g-da"), false).unwrap();
        db::upsert_article_with_feed(&conn, feed_b, &article("g-db"), false).unwrap();
        (feed_a, feed_b, 0i64, 0i64)
    };

    let eid_a = server.add_entry_ret(10, URL_SHARED, "copy-10", false, false);
    let eid_b = server.add_entry_ret(11, URL_SHARED, "copy-11", false, false);
    sync::states_phase(&dbm, &http, true)
        .await
        .expect("full states phase");

    let conn = dbm.lock().await;
    let (_, bound_a) = row_of_feed(&conn, feed_a, URL_SHARED);
    let (_, bound_b) = row_of_feed(&conn, feed_b, URL_SHARED);
    assert_eq!(
        bound_a,
        Some(eid_a),
        "源A 行必须绑定自己的 entry（不得被另一源占位）"
    );
    assert_eq!(
        bound_b,
        Some(eid_b),
        "源B 行必须绑定自己的 entry（旧行为：被 URL 兜底吞成副本、永不绑定）"
    );
}

/// ①c Google Reader：任一源副本远端已读时，pull 合并必须把同文兄弟条目
/// 一并标读（read-anywhere-wins 广播）；远端未读只作用于自己的条目
/// （unread 不广播给兄弟）。
#[tokio::test]
async fn greader_pull_read_broadcasts_to_sibling_entries_only_for_read() {
    let (dbm, http, server) = setup("gr_broadcast").await;
    let (feed_a, feed_b, ..) = {
        let conn = dbm.lock().await;
        seed_two_bound_feeds(&conn)
    };
    // A（feed10，先处理）远端未读；B（feed11）远端已读 —— B 合并时 A 行已存在，
    // 已读广播必须落在兄弟行上（旧模型下第二条 entry 被 URL 兜底吞掉，此断言红；
    // 没有广播时 A 会保持未读）。
    let eid_a = server.add_entry_ret(10, URL_SHARED, "copy-10", false, false);
    let eid_b = server.add_entry_ret(11, URL_SHARED, "copy-11", true, false);

    sync::states_phase(&dbm, &http, true)
        .await
        .expect("full states phase");

    {
        let conn = dbm.lock().await;
        let (aid_a, _) = row_of_feed(&conn, feed_a, URL_SHARED);
        let (aid_b, _) = row_of_feed(&conn, feed_b, URL_SHARED);
        assert!(state_of(&conn, aid_b).0, "远端已读的副本落地已读");
        assert!(
            state_of(&conn, aid_a).0,
            "同文兄弟必须被已读广播（读到哪算读）"
        );
    }

    // 反向：远端 B 拨回未读、A 保持已读 → **读优先**（政策单点，读到哪算读）：
    // A 的已读广播覆盖 B 的 own 远端未读；unread 永不广播（A 不得被翻回未读）。
    {
        let mut es = server.entries.lock().unwrap();
        if let Some(e) = es.iter_mut().find(|e| e.id == eid_b) {
            e.read = false;
            e.changed_at = chrono::Utc::now().timestamp();
        }
        if let Some(e) = es.iter_mut().find(|e| e.id == eid_a) {
            e.read = true;
            e.changed_at = chrono::Utc::now().timestamp();
        }
    }
    sync::states_phase(&dbm, &http, true)
        .await
        .expect("full states phase 2");
    {
        let conn = dbm.lock().await;
        let (aid_a, _) = row_of_feed(&conn, feed_a, URL_SHARED);
        let (aid_b, _) = row_of_feed(&conn, feed_b, URL_SHARED);
        assert!(
            state_of(&conn, aid_a).0,
            "unread 不得广播：已读兄弟不得被 B 的未读翻回"
        );
        assert!(
            state_of(&conn, aid_b).0,
            "读优先：A 的已读广播覆盖 B 的 own 远端未读（读到哪算读）"
        );
    }
}

/// ② 断开（purge_remote_data）：远端来源源的条目整体删除，本地直连源的同 URL
/// 条目必须原样保留（正文/读藏/行 id 不动）——「删除/解绑不污染其他条目」。
#[test]
fn purge_remote_source_keeps_local_entry_of_same_url() {
    let tmp = common::unique_db_path("identity_purge");
    let _ = std::fs::remove_file(&tmp);
    let mut conn = db::open(&tmp).expect("open db");
    let folder = db::create_folder(&conn, "F", "article").unwrap();
    let remote_feed = db::insert_feed_origin(
        &conn,
        "http://r.example/feed.xml",
        None,
        "远端源",
        None,
        folder,
        "inherit",
        false,
        false,
        "remote",
    )
    .unwrap();
    let local_feed = db::insert_feed(
        &conn,
        "http://l.example/feed.xml",
        None,
        "本地源",
        None,
        folder,
        "inherit",
        false,
        false,
    )
    .unwrap();

    let article = |guid: &str| db::NewArticle {
        guid: guid.into(),
        url: Some(URL_SHARED.into()),
        title: "t".into(),
        author: None,
        summary: None,
        content_html: Some("<p>c</p>".into()),
        body_text: "c".into(),
        image_url: None,
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: Some(chrono::Utc::now().to_rfc3339()),
        source: "direct".into(),
    };
    let (remote_aid, _) =
        db::upsert_article_with_feed(&conn, remote_feed, &article("g-remote"), false).unwrap();
    let (local_aid, _) =
        db::upsert_article_with_feed(&conn, local_feed, &article("g-local"), false).unwrap();
    db::set_article_remote_id(&conn, remote_aid, 777).unwrap();
    db::set_read(&conn, local_aid, true).unwrap();
    db::set_starred(&conn, local_aid, true).unwrap();

    db::purge_remote_data(&mut conn).expect("断开清理");

    let left: Vec<(i64, Option<i64>, bool, bool)> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, remote_id, is_read, is_starred FROM articles WHERE url_norm = ?1 ORDER BY id",
            )
            .unwrap();
        stmt.query_map([db::normalize_url(URL_SHARED)], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get::<_, i64>(2)? != 0,
                r.get::<_, i64>(3)? != 0,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
    };
    assert_eq!(left.len(), 1, "只应剩本地源的条目");
    assert_eq!(left[0].0, local_aid, "本地条目行 id 不变（未重建/未覆盖）");
    assert_eq!(left[0].1, None, "断开后本地条目的远端绑定清除");
    assert!(left[0].2 && left[0].3, "本地条目的读/藏状态在断开后保留");
    let _ = std::fs::remove_file(&tmp);
}

/// ③ 同轮冲突的顺序确定性（政策单点）：组内 A 远端已读、B 自己的绑定远端未读。
/// 政策 = **读优先（读到哪算读）**：同轮内任何已读信号（own 或兄弟广播）落定后，
/// own unread 不再翻转该行——两种处理顺序最终同为「A 已读、B 已读」，不再随
/// 插入顺序漂移。对照：无任何已读信号的独立条目（不与已读副本同组）远端 unread
/// 照常作用（unread 只认绑定同源 entry 的边界仍立）。
#[tokio::test]
async fn same_round_read_broadcast_and_explicit_unread_is_order_independent() {
    // 排列一：A（read）先处理，B（unread）后处理；排列二：B 先、A 后。
    for (name, read_first) in [("read_first", true), ("unread_first", false)] {
        let (dbm, http, server) = setup(&format!("order_{name}")).await;
        let (feed_a, feed_b, ..) = {
            let conn = dbm.lock().await;
            seed_two_bound_feeds(&conn)
        };
        let (eid_a, eid_b) = if read_first {
            let a = server.add_entry_ret(10, URL_SHARED, "copy-10", true, false);
            let b = server.add_entry_ret(11, URL_SHARED, "copy-11", false, false);
            (a, b)
        } else {
            let b = server.add_entry_ret(11, URL_SHARED, "copy-11", false, false);
            let a = server.add_entry_ret(10, URL_SHARED, "copy-10", true, false);
            (a, b)
        };
        assert!(eid_a > 0 && eid_b > 0);
        // 对照组：独立条目（唯一 URL，无同组已读兄弟）远端未读 → own unread 照常作用
        let solo =
            server.add_entry_ret(10, "http://127.0.0.1:8765/post/solo", "solo", false, false);

        sync::states_phase(&dbm, &http, true)
            .await
            .expect("full states phase");
        {
            let conn = dbm.lock().await;
            let (aid_a, bound_a) = row_of_feed(&conn, feed_a, URL_SHARED);
            let (aid_b, bound_b) = row_of_feed(&conn, feed_b, URL_SHARED);
            assert_eq!(bound_a, Some(eid_a));
            assert_eq!(bound_b, Some(eid_b));
            assert!(state_of(&conn, aid_a).0, "[{name}] A 远端已读 → 本地已读");
            assert!(
                state_of(&conn, aid_b).0,
                "[{name}] 读优先：B 的远端未读不得翻转同轮已读广播（顺序无关，两排列同结果）"
            );
            let solo_read: i64 = conn
                .query_row(
                    "SELECT is_read FROM articles WHERE remote_id = ?1",
                    [solo],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(
                solo_read, 0,
                "[{name}] 无同组已读信号时，own 远端 unread 照常作用（unread 只认绑定条目）"
            );
        }
    }
}

/// ③b 字段版本/pending 优先级在两种排列下同样成立：B 有未推送的本地读意图时，
/// 同一轮里 A 的已读广播与 B 自己的远端未读**都不得**改动 B（pending 优先）。
#[tokio::test]
async fn same_round_order_conflict_keeps_pending_local_intent() {
    let (dbm, http, server) = setup("order_pending").await;
    let (feed_a, feed_b, ..) = {
        let conn = dbm.lock().await;
        seed_two_bound_feeds(&conn)
    };
    let _eid_a = server.add_entry_ret(10, URL_SHARED, "copy-10", true, false);
    let eid_b = server.add_entry_ret(11, URL_SHARED, "copy-11", false, false);
    {
        let conn = dbm.lock().await;
        // 预置 B 行（直连、未绑定）并本地标读 + 入队（pending）
        let (aid_b, _) = db::upsert_article_with_feed(
            &conn,
            feed_b,
            &db::NewArticle {
                guid: "g-pending-b".into(),
                url: Some(URL_SHARED.into()),
                title: "t".into(),
                author: None,
                summary: None,
                content_html: None,
                body_text: "b".into(),
                image_url: None,
                enclosure_url: None,
                enclosure_mime: None,
                duration_sec: None,
                published_at: Some(chrono::Utc::now().to_rfc3339()),
                source: "direct".into(),
            },
            false,
        )
        .unwrap();
        db::set_read_with_enqueue(&conn, aid_b, true).unwrap();
        let bound: Option<i64> = conn
            .query_row(
                "SELECT remote_id FROM articles WHERE id = ?1",
                [aid_b],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bound, None, "前置：行保持未绑定（由本轮远端补绑）");
    }
    let _ = feed_a;
    sync::states_phase(&dbm, &http, true)
        .await
        .expect("full states phase");
    {
        let conn = dbm.lock().await;
        let (aid_b, bound_b) = row_of_feed(&conn, feed_b, URL_SHARED);
        assert_eq!(bound_b, Some(eid_b), "绑定回填不受 pending 影响");
        assert!(
            state_of(&conn, aid_b).0,
            "pending 优先：远端未读与兄弟已读广播都不得翻回"
        );
    }
    // pending 经同步推送段发出（states 阶段含 push）：服务端收到 read
    let updates = mock_greader::status_updates_map(&server);
    assert_eq!(
        updates.get(&eid_b).map(String::as_str),
        Some("read"),
        "本地读意图经推送段回传（绑定后队列被确认）"
    );
}

/// ④ 同 feed 同 URL、不同协议 entry id：精确协议 ID 必须保有自己身份——
/// 两条 entry 各自独立成行、各自绑定、收藏独立；不得再以 CSV 记账吞掉第二条。
/// 契约区分：「直连 guid 变化但 URL 稳定」只在**未绑定**行上按 URL 复用；
/// 已绑定另一精确 entry 的行不与新 entry 合并。
#[tokio::test]
async fn same_feed_distinct_remote_ids_keep_own_identity() {
    let (dbm, http, server) = setup("same_feed_eids").await;
    let feed = {
        let conn = dbm.lock().await;
        let folder = db::create_folder(&conn, "F", "article").unwrap();
        let feed = db::insert_feed(
            &conn,
            "http://127.0.0.1:8765/local_feed.xml",
            None,
            "F",
            None,
            folder,
            "inherit",
            false,
            false,
        )
        .unwrap();
        db::set_feed_remote_id(&conn, feed, 10).unwrap();
        feed
    };
    let eid1 = server.add_entry_ret(10, URL_SHARED, "entry-1", false, false);
    let eid2 = server.add_entry_ret(10, URL_SHARED, "entry-2", false, false);
    assert_ne!(eid1, eid2);

    sync::states_phase(&dbm, &http, true)
        .await
        .expect("full states phase");

    {
        let conn = dbm.lock().await;
        let rows: Vec<(i64, Option<i64>, String)> = conn
            .prepare(
                "SELECT id, remote_id, remote_dup_ids FROM articles
                 WHERE feed_id = ?1 AND url_norm = ?2 ORDER BY id",
            )
            .unwrap()
            .query_map(
                rusqlite::params![feed, db::normalize_url(URL_SHARED)],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            rows.len(),
            2,
            "同 feed 两个精确 entry 必须各自成行（旧行为：CSV 吞掉第二条）"
        );
        assert_eq!(rows[0].1, Some(eid1));
        assert_eq!(rows[1].1, Some(eid2));
        assert_eq!(rows[0].2, "", "不再以 CSV 记账充当新条目实体");
        assert_eq!(rows[1].2, "");
    }

    // 收藏隔离：远端只收藏 entry-2
    {
        let mut es = server.entries.lock().unwrap();
        if let Some(e) = es.iter_mut().find(|e| e.id == eid2) {
            e.starred = true;
            e.changed_at = chrono::Utc::now().timestamp();
        }
    }
    sync::states_phase(&dbm, &http, true)
        .await
        .expect("sync after star");
    {
        let conn = dbm.lock().await;
        let rows: Vec<(i64, i64)> = conn
            .prepare(
                "SELECT remote_id, is_starred FROM articles
                 WHERE feed_id = ?1 AND url_norm = ?2 ORDER BY remote_id",
            )
            .unwrap()
            .query_map(
                rusqlite::params![feed, db::normalize_url(URL_SHARED)],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            rows,
            vec![(eid1, 0), (eid2, 1)],
            "每个精确 entry 的收藏独立：entry-1 未收藏、entry-2 已收藏"
        );
    }
}

/// ⑤ 精确 entry 命中后的信任边界（回归与依据）：
/// - 行所属 feed 的 remote_id 未知（None）→ 精确 entry ID 仍是身份锚：准予合并
///   （否则解绑/迁移后的行会被永久搁置）；身份由 remote_id 承担、不依赖 feed 绑定；
/// - 行所属 feed 已知且与 entry 的 feed 冲突 → 拒绝写入该行（脏绑定不得被跨源
///   entry 改写），entry 落到自己源（或独立行）。
#[tokio::test]
async fn precise_remote_id_trust_boundary_for_unknown_and_conflicting_feed() {
    use app_lib::sync;
    let (dbm, http, server) = setup("trust").await;
    let shared = "http://127.0.0.1:8765/post/trust";

    // —— 场景一：行所属 feed 未绑定（remote_id 未知）→ 精确 eid 命中即信任 ——
    let unbound_row = {
        let conn = dbm.lock().await;
        let folder = db::create_folder(&conn, "U", "article").unwrap();
        let feed = db::insert_feed(
            &conn,
            "http://127.0.0.1:8765/local_feed.xml",
            None,
            "U",
            None,
            folder,
            "inherit",
            false,
            false,
        )
        .unwrap();
        let (aid, _) = db::upsert_article_with_feed(
            &conn,
            feed,
            &db::NewArticle {
                guid: "g-unbound".into(),
                url: Some(shared.into()),
                title: "t".into(),
                author: None,
                summary: None,
                content_html: None,
                body_text: "b".into(),
                image_url: None,
                enclosure_url: None,
                enclosure_mime: None,
                duration_sec: None,
                published_at: Some(chrono::Utc::now().to_rfc3339()),
                source: "direct".into(),
            },
            false,
        )
        .unwrap();
        // feed 保持未绑定（remote_id = NULL）
        aid
    };
    let eid = server.add_entry_ret(10, shared, "trust", false, true); // 远端已收藏
    {
        let conn = dbm.lock().await;
        db::set_article_remote_id(&conn, unbound_row, eid).unwrap();
    }
    sync::states_phase(&dbm, &http, true)
        .await
        .expect("full states phase");
    {
        let conn = dbm.lock().await;
        assert!(
            state_of(&conn, unbound_row).1,
            "feed 绑定未知时，精确 eid 命中仍按身份合并（远端收藏落地）"
        );
    }

    // —— 场景二：行所属 feed 已知（20）且与 entry 的 feed（10）冲突 → 拒绝写该行 ——
    let (conflict_row, other_row) = {
        let conn = dbm.lock().await;
        let folder = db::create_folder(&conn, "C", "article").unwrap();
        let feed20 = db::insert_feed(
            &conn,
            "https://conflict.invalid/f20",
            None,
            "F20",
            None,
            folder,
            "inherit",
            false,
            false,
        )
        .unwrap();
        db::set_feed_remote_id(&conn, feed20, 20).unwrap();
        let (aid, _) = db::upsert_article_with_feed(
            &conn,
            feed20,
            &db::NewArticle {
                guid: "g-conflict".into(),
                url: Some(format!("{shared}?v=2")),
                title: "t".into(),
                author: None,
                summary: None,
                content_html: None,
                body_text: "b".into(),
                image_url: None,
                enclosure_url: None,
                enclosure_mime: None,
                duration_sec: None,
                published_at: Some(chrono::Utc::now().to_rfc3339()),
                source: "direct".into(),
            },
            false,
        )
        .unwrap();
        (aid, feed20)
    };
    // entry 来自 feed 10，eid 恰好是冲突行上写的远端 id（历史脏绑定形态）
    let eid_conflict = server.add_entry_ret(10, &format!("{shared}?v=2"), "conflict", false, true);
    {
        let conn = dbm.lock().await;
        db::set_article_remote_id(&conn, conflict_row, eid_conflict).unwrap();
    }
    // 本地直连 feed 绑定远端 10，供 entry 落位
    {
        let conn = dbm.lock().await;
        let folder = db::create_folder(&conn, "C2", "article").unwrap();
        let feed10 = db::insert_feed(
            &conn,
            "https://conflict.invalid/f10",
            None,
            "F10",
            None,
            folder,
            "inherit",
            false,
            false,
        )
        .unwrap();
        db::set_feed_remote_id(&conn, feed10, 10).unwrap();
    }
    let _ = other_row;
    let report = sync::states_phase(&dbm, &http, true)
        .await
        .expect("sync after conflict binding");
    assert!(
        report.errors.is_empty(),
        "冲突绑定按「干净拒绝」处理：不得让整页失败（不得撞 UNIQUE(remote_id)）：{:?}",
        report.errors
    );
    {
        let conn = dbm.lock().await;
        let conflict_star: i64 = conn
            .query_row(
                "SELECT is_starred FROM articles WHERE id = ?1",
                [conflict_row],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            conflict_star, 0,
            "feed 冲突的脏绑定行不得被跨源 entry 改写（信任复核干净拒绝）"
        );
        // remote_id 唯一：冲突下不为该 eid 另建绑定行（entry 的本轮状态被跳过，
        // 留待远端重拉/人工修复；不猜归属、不抢绑定、不拖垮整页）
        let rebound: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM articles WHERE remote_id = ?1 AND feed_id IN
                   (SELECT id FROM feeds WHERE remote_id = 10)",
                [eid_conflict],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            rebound, 0,
            "不得重复绑定同一 remote_id（UNIQUE 约束下的正确拒绝）"
        );
    }
}

/* ============================================================
OPT-008B R2（主控清单②）：pull 建同 feed 同 URL 不同 remoteID 两行后，
**真实 direct refresh（scheduler::refresh_all 生产入口 + 真实 RSS HTTP）**
不得吞掉任一身份/收藏。
- 既有覆盖：`same_feed_distinct_remote_ids_keep_own_identity` 锁定 pull 建两行；
- 本用例补「随后直连刷新」：RSS 条目与两行同 URL、guid 不同 → 只按同 feed
  URL 稳定复用命中最小 id 行做内容刷新（保留 source/读藏/绑定），另一精确
  entry 行不被改写；刷新后仍两行、绑定与收藏逐行不变，不新增第三行。
============================================================ */

/// 最小 RSS 服务（真实 HTTP）：任意 GET 回同一份 RSS。返回端口。
fn start_identity_rss_server(body: String) -> u16 {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0u8; 4096];
            let mut req = String::new();
            loop {
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break;
                }
                req.push_str(&String::from_utf8_lossy(&buf[..n]));
                if req.contains("\r\n\r\n") || req.len() > 8192 {
                    break;
                }
            }
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/rss+xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    port
}

/// pull 两行 → direct refresh：身份/收藏不吞的生产入口回归。
#[tokio::test]
async fn same_feed_distinct_remote_ids_survive_real_direct_refresh() {
    let (dbm, http, server) = setup("same_feed_refresh").await;
    let rss = format!(
        r#"<?xml version="1.0"?><rss version="2.0"><channel><title>F</title>
<item><title>直连刷新标题</title><link>{URL_SHARED}</link><guid>rss-post-1</guid></item>
</channel></rss>"#
    );
    let rss_port = start_identity_rss_server(rss);
    let feed = {
        let conn = dbm.lock().await;
        let folder = db::create_folder(&conn, "F", "article").unwrap();
        let feed = db::insert_feed(
            &conn,
            &format!("http://127.0.0.1:{rss_port}/feed.xml"),
            None,
            "F",
            None,
            folder,
            "inherit",
            false,
            false,
        )
        .unwrap();
        db::set_feed_remote_id(&conn, feed, 10).unwrap();
        feed
    };
    let eid1 = server.add_entry_ret(10, URL_SHARED, "entry-1", false, false);
    let eid2 = server.add_entry_ret(10, URL_SHARED, "entry-2", false, false);

    // pull 建立两行（精确 entry 各自成行、各自绑定）。
    sync::states_phase(&dbm, &http, true)
        .await
        .expect("full states phase");
    // 远端只收藏 entry-2（收藏隔离的初态）。
    {
        let mut es = server.entries.lock().unwrap();
        if let Some(e) = es.iter_mut().find(|e| e.id == eid2) {
            e.starred = true;
            e.changed_at = chrono::Utc::now().timestamp();
        }
    }
    sync::states_phase(&dbm, &http, true)
        .await
        .expect("sync after star");

    let rows_before: Vec<(i64, Option<i64>, i64, String)> = {
        let conn = dbm.lock().await;
        let rows = conn
            .prepare(
                "SELECT id, remote_id, is_starred, title FROM articles
                 WHERE feed_id = ?1 AND url_norm = ?2 ORDER BY id",
            )
            .unwrap()
            .query_map(
                rusqlite::params![feed, db::normalize_url(URL_SHARED)],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        rows
    };
    assert_eq!(rows_before.len(), 2, "pull 必须建立两行：{rows_before:?}");
    assert_eq!(
        (rows_before[0].1, rows_before[1].1),
        (Some(eid1), Some(eid2)),
        "两行各自绑定自己的精确 entry"
    );
    assert_eq!(
        (rows_before[0].2, rows_before[1].2),
        (0, 1),
        "收藏隔离：entry-1 未收藏、entry-2 已收藏"
    );

    // 真实 direct refresh（生产入口）：RSS 与两行同 URL、guid 不同。
    let (new_articles, failed) = app_lib::scheduler::refresh_all(&dbm, &http).await;
    assert_eq!(failed, 0, "direct refresh 不得失败");
    assert_eq!(new_articles, 0, "同 feed 同 URL 已有行 → 不新增第三行");

    let conn = dbm.lock().await;
    let rows_after: Vec<(i64, Option<i64>, i64, String, String)> = conn
        .prepare(
            "SELECT id, remote_id, is_starred, title, remote_dup_ids FROM articles
             WHERE feed_id = ?1 AND url_norm = ?2 ORDER BY id",
        )
        .unwrap()
        .query_map(
            rusqlite::params![feed, db::normalize_url(URL_SHARED)],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        rows_after.len(),
        2,
        "direct refresh 后仍必须两行（不得吞并/复制）：{rows_after:?}"
    );
    assert_eq!(
        (rows_after[0].1, rows_after[1].1),
        (Some(eid1), Some(eid2)),
        "刷新不得换绑/清空精确 entry 绑定"
    );
    assert_eq!(
        (rows_after[0].2, rows_after[1].2),
        (0, 1),
        "刷新不得吞掉任一行的收藏"
    );
    assert_eq!(
        rows_after.iter().filter(|r| r.3 == "直连刷新标题").count(),
        1,
        "刷新必须真实发生（URL 稳定复用命中一行做内容更新）：{rows_after:?}"
    );
    assert_eq!(
        (rows_after[0].4.as_str(), rows_after[1].4.as_str()),
        ("", ""),
        "不得退回 CSV 记账充当条目实体"
    );
}
