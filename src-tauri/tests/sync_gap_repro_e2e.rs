//! REQ-002/003 双向同步的缺口回归与修复验证（TASK-031 起，随修复批次转正）。
//!
//! 约定：修复完成的场景去掉 #[ignore] 并断言期望行为（默认 `cargo test` 即覆盖）；
//! 尚未修复的场景保留 #[ignore] 与原因标注（复现旧缺陷，修复后转正）。
//! 缺口定位与修复设计见 .workflow-kit/docs/FINDINGS-SYNC-GAP.md。
//!
//! 运行：cargo test --test sync_gap_repro_e2e（默认集含全部已转正场景）

mod mock_greader;

use app_lib::db;
use app_lib::sync;
use mock_greader::{subscription_edit_actions, MockGReader};
use std::sync::Arc;
use tokio::sync::Mutex;

async fn setup(
    name: &str,
) -> (
    Arc<Mutex<rusqlite::Connection>>,
    reqwest::Client,
    Arc<MockGReader>,
) {
    let server = MockGReader::start().await.expect("start mock server");
    let tmp = std::env::temp_dir().join(format!(
        "fluxreader_gap_{}_{}_{}.db",
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
    (Arc::new(Mutex::new(conn)), http, server)
}

/// 本地造一篇直连文章（URL 与 mock feed 10 的 entry 对齐，供绑定回填）。
async fn seed_local_article(
    db: &Arc<Mutex<rusqlite::Connection>>,
    server: &MockGReader,
    url: &str,
    read: bool,
) -> i64 {
    // 远端造同 URL entry（绑定回填的匹配目标——mock entries 默认为空）
    server.add_entry(10, url, "Remote Entry", "unread", false);
    let conn = db.lock().await;
    let folder_id = db::create_folder(&conn, "测试分类", "article").unwrap();
    let feed_id = db::insert_feed(
        &conn,
        "http://127.0.0.1:8765/local_feed.xml", // 与 mock feed 10 同 URL
        None,
        "Local Direct Feed",
        None,
        folder_id,
        "inherit",
        true,
        false,
    )
    .unwrap();
    let a = db::NewArticle {
        guid: format!("guid-{url}"),
        url: Some(url.into()),
        title: "Local Article".into(),
        author: None,
        summary: None,
        content_html: Some("<p>local</p>".into()),
        body_text: "local".into(),
        image_url: None,
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: Some(chrono::Utc::now().to_rfc3339()),
        source: "direct".into(),
    };
    let aid = db::upsert_article_with_feed(&conn, feed_id, &a, false)
        .unwrap()
        .0;
    if read {
        db::set_read(&conn, aid, true).unwrap();
    }
    aid
}

/// 本地造一个直连订阅 + 若干未读文章（同一 feed 多篇：seed_local_article 每次
/// 新建 feed，会在 `feeds.feed_url` 唯一约束上冲突，故多篇场景用这个）。
/// 每篇对应远端同 URL entry（feed 10），供连接后的绑定回填。
async fn seed_local_articles_in_one_feed(
    db: &Arc<Mutex<rusqlite::Connection>>,
    server: &MockGReader,
    urls: &[&str],
) -> Vec<i64> {
    let mut ids = Vec::new();
    for url in urls {
        server.add_entry(10, url, "Remote Entry", "unread", false);
    }
    let conn = db.lock().await;
    let folder_id = db::create_folder(&conn, "测试分类", "article").unwrap();
    let feed_id = db::insert_feed(
        &conn,
        "http://127.0.0.1:8765/local_feed.xml", // 与 mock feed 10 同 URL
        None,
        "Local Direct Feed",
        None,
        folder_id,
        "inherit",
        true,
        false,
    )
    .unwrap();
    for url in urls {
        let a = db::NewArticle {
            guid: format!("guid-{url}"),
            url: Some((*url).into()),
            title: format!("Local Article {url}"),
            author: None,
            summary: None,
            content_html: Some("<p>local</p>".into()),
            body_text: "local".into(),
            image_url: None,
            enclosure_url: None,
            enclosure_mime: None,
            duration_sec: None,
            published_at: Some(chrono::Utc::now().to_rfc3339()),
            source: "direct".into(),
        };
        ids.push(
            db::upsert_article_with_feed(&conn, feed_id, &a, false)
                .unwrap()
                .0,
        );
    }
    ids
}

/// A-1 修复后的期望行为（原复现测试转正）：删除已绑定远端的订阅会 best-effort
/// 退订（GReader），并写入删除墓碑——下次 pull 不得按远端订阅列表复活已删订阅。
#[tokio::test]
async fn deleted_feed_stays_deleted_and_unsubscribes() {
    let (db, http, server) = setup("gap_delete").await;
    let feed_id = {
        let conn = db.lock().await;
        let folder_id = db::create_folder(&conn, "测试分类", "article").unwrap();
        db::insert_feed(
            &conn,
            "http://127.0.0.1:8765/local_feed.xml", // 与 mock feed 10 同 URL（远端订阅存在）
            None,
            "Local Direct Feed",
            None,
            folder_id,
            "inherit",
            true,
            false,
        )
        .unwrap()
    };
    // 绑定远端：feeds_phase 按 URL 匹配写入 remote_id=10
    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase (bind)");
    {
        let conn = db.lock().await;
        let bound: Option<i64> = conn
            .query_row(
                "SELECT remote_id FROM feeds WHERE id = ?1",
                [feed_id],
                |r| r.get(0),
            )
            .ok()
            .flatten();
        assert_eq!(bound, Some(10), "前置条件：订阅已绑定远端 feed 10");
    }

    // 删除（命令层真实逻辑）：写墓碑 + 删除本地 + 返回待退订的远端 id
    let unsubscribe = {
        let conn = db.lock().await;
        app_lib::commands::record_feed_deletion(&conn, feed_id).expect("record deletion")
    };
    assert!(
        matches!(unsubscribe, Some((10, _))),
        "已绑定远端且同步已配置时应返回待退订目标"
    );

    // ① 删除后立即同步：远端订阅列表仍含 feed/10（列表滞后），墓碑必须阻止复活
    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase right after delete");
    let revived = {
        let conn = db.lock().await;
        conn.query_row(
            "SELECT COUNT(*) FROM feeds WHERE feed_url = 'http://127.0.0.1:8765/local_feed.xml'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    };
    assert_eq!(revived, 0, "已删除订阅不得被 pull 复活（墓碑生效）");

    // ② 退订远端（best-effort，GReader）。
    // 注意（TASK-055 语义变更）：返回值 true 只表示**请求被后端接受（2xx）**，
    // **不再等价于「远端已删除」**，因此此处**不清墓碑**——墓碑改由后续 pull
    // 在「远端列表确认已不含该 URL」时收敛清除（见 ③ 之后的断言）。
    // 修复前此处按 2xx 清墓碑，导致「2xx 但远端未生效」时已删订阅复活。
    let (remote_id, feed_url) = unsubscribe.expect("unsubscribe target");
    assert!(
        sync::unsubscribe_remote(&db, &http, remote_id, &feed_url).await,
        "退订请求应被后端接受（mock 回 200）"
    );
    assert!(
        subscription_edit_actions(&server)
            .iter()
            .any(|(ac, s)| ac == "unsubscribe" && s.contains("feed/10")),
        "远端应收到 ac=unsubscribe"
    );
    {
        let conn = db.lock().await;
        assert!(
            !db::feed_tombstones(&conn).unwrap().is_empty(),
            "仅收到 2xx 不足以确认远端已删除，墓碑必须保留（TASK-055）"
        );
    }

    // ③ 再次同步：此时 mock 已按真实行为把该订阅从远端列表移除，
    //    pull 得以确认「远端已不含」→ 墓碑才被收敛清除，且不复活
    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase after unsubscribe");
    let existed = {
        let conn = db.lock().await;
        conn.query_row(
            "SELECT COUNT(*) FROM feeds WHERE feed_url = 'http://127.0.0.1:8765/local_feed.xml'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    };
    assert_eq!(existed, 0, "退订后同步不得复活（远端已不再列出该订阅）");
    {
        let conn = db.lock().await;
        assert!(
            db::feed_tombstones(&conn).unwrap().is_empty(),
            "远端列表确认已不含该 URL 后，墓碑应由 pull 收敛清除"
        );
    }
}

/// TASK-055：退订请求返回 **2xx 但远端实际未删除**时，不得清除删除墓碑，
/// 更不得让已删订阅被下次 pull 复活。
///
/// 缺陷形态（修复前）：`sync::unsubscribe_remote` 以 `post_form_text` 的
/// `resp.status().is_success()` 作为「远端已确认退订」，据此 `remove_feed_tombstone`；
/// 而真实 GReader 在 token 失效/权限不足/`s=feed/<id>` 不存在时可能回 2xx + 错误体。
/// 墓碑被清后，`pull_feeds` 的防复活唯一防线消失 → 远端仍列出该订阅 → 复活。
#[tokio::test]
async fn unsubscribe_2xx_without_removal_keeps_tombstone_and_no_revive() {
    let (db, http, server) = setup("gap_unsub_2xx").await;
    let feed_id = {
        let conn = db.lock().await;
        let folder_id = db::create_folder(&conn, "测试分类", "article").unwrap();
        db::insert_feed(
            &conn,
            "http://127.0.0.1:8765/local_feed.xml", // 与 mock feed 10 同 URL（远端订阅存在）
            None,
            "Local Direct Feed",
            None,
            folder_id,
            "inherit",
            true,
            false,
        )
        .unwrap()
    };
    // 绑定远端：feeds_phase 按 URL 匹配写入 remote_id=10
    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase (bind)");

    // 删除（命令层真实逻辑）：写墓碑 + 删除本地 + 返回待退订的远端 id
    let (remote_id, feed_url) = {
        let conn = db.lock().await;
        app_lib::commands::record_feed_deletion(&conn, feed_id)
            .expect("record deletion")
            .expect("已绑定远端且已配置：应返回待退订目标")
    };

    // 故障注入：退订回 200，但服务端**保留**该订阅（模拟 2xx 但未生效）
    server
        .unsubscribe_returns_2xx_without_removing
        .store(true, std::sync::atomic::Ordering::SeqCst);

    let accepted = sync::unsubscribe_remote(&db, &http, remote_id, &feed_url).await;
    assert!(accepted, "请求被后端接受（2xx），返回值应为 true");

    // ① 墓碑必须仍在：请求成功 ≠ 远端已删除，不能据此清墓碑
    {
        let conn = db.lock().await;
        let tombstones = db::feed_tombstones(&conn).unwrap();
        assert!(
            tombstones.iter().any(|u| u.contains("local_feed.xml")),
            "2xx 但未确认远端已删除时，删除墓碑必须保留（TASK-055）"
        );
    }

    // ② 再次同步：远端仍列出该订阅，但其墓碑在 → 不得复活
    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase after 2xx-without-removal");
    {
        let conn = db.lock().await;
        let revived = conn
            .query_row(
                "SELECT COUNT(*) FROM feeds WHERE feed_url = 'http://127.0.0.1:8765/local_feed.xml'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(revived, 0, "2xx 但远端未删除时不得复活已删订阅（TASK-055）");
    }

    // ③ 远端最终确认删除后：墓碑才应由 pull 收敛清除（唯一有证据的清除条件）
    server
        .subscriptions
        .lock()
        .unwrap()
        .retain(|s| s.id != format!("feed/{remote_id}"));
    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase after remote removal");
    {
        let conn = db.lock().await;
        assert!(
            db::feed_tombstones(&conn).unwrap().is_empty(),
            "远端列表确认已不含该 URL 后，墓碑应由 pull 收敛清除"
        );
        let revived = conn
            .query_row(
                "SELECT COUNT(*) FROM feeds WHERE feed_url = 'http://127.0.0.1:8765/local_feed.xml'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(revived, 0, "远端确认删除后仍不得复活");
    }
}

/// TASK-056：**零 folder 的新库**（用户首次连接的真实初始状态）拉取远端订阅时，
/// 必须真正把订阅建进本地，且失败时必须可见。
///
/// 缺陷形态（修复前）：pull 建本地 feed 的目录兜底是
/// `get_first_folder_id(&conn).ok().flatten()).unwrap_or(1)`——folders 为空时给出
/// **不存在的目录 id 1**，而 `feeds.folder_id REFERENCES folders(id)` 且连接开启了
/// `PRAGMA foreign_keys=ON`，插入抛 `FOREIGN KEY constraint failed`；
/// 随后 `if let Ok(fid) = inserted` **没有 else**，错误被静默吞掉、`report.errors` 保持为空，
/// 前端却照常提示「已拉取订阅源」「后端同步完成」——用户看到成功、实际零订阅。
///
/// 这是既有测试网的整体盲区：其它 e2e 一律先 `create_folder` 再插 feed，
/// `folders` 从不为空，该兜底分支从未被执行。
#[tokio::test]
async fn fresh_empty_db_pull_creates_remote_subscriptions() {
    let (db, http, server) = setup("gap_empty_db").await;

    // 远端：一条【无分类】订阅 + 一条【有分类】订阅。
    // 无分类那条才会走 `remote_folder_label = None` → 目录兜底分支（缺陷所在）；
    // 有分类那条覆盖「远端分类可映射」的正常路径，防止只修一半。
    // 关键：必须同时清空 mock 的 folders（tag/list），否则 pull 会先建出远端分类目录，
    // 本地 folders 便不再为空，兜底分支不会被触发（这正是其它测试漏掉该缺陷的原因）。
    server.folders.lock().unwrap().clear();
    *server.subscriptions.lock().unwrap() = vec![
        mock_greader::MockSubscription {
            id: "feed/10".into(),
            title: "Uncategorized Remote".into(),
            url: "http://127.0.0.1:8765/uncategorized.xml".into(),
            html_url: None,
            categories: vec![], // ← 无分类：触发目录兜底
        },
        mock_greader::MockSubscription {
            id: "feed/11".into(),
            title: "Categorized Remote".into(),
            url: "http://127.0.0.1:8765/categorized.xml".into(),
            html_url: None,
            categories: vec![("Remote Cat".into(), "folder".into())],
        },
    ];

    // 前置条件：本地**没有任何 folder**（真实新库状态）
    {
        let conn = db.lock().await;
        let folders: i64 = conn
            .query_row("SELECT COUNT(*) FROM folders", [], |r| r.get(0))
            .unwrap();
        assert_eq!(folders, 0, "前置条件：新库应零 folder");
        let feeds: i64 = conn
            .query_row("SELECT COUNT(*) FROM feeds", [], |r| r.get(0))
            .unwrap();
        assert_eq!(feeds, 0, "前置条件：新库应零订阅");
    }

    let report = sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase on fresh db");

    let conn = db.lock().await;
    let local_feeds: i64 = conn
        .query_row("SELECT COUNT(*) FROM feeds", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        local_feeds, 2,
        "零 folder 的新库必须把远端订阅（含无分类的）真正建立到本地（TASK-056）；\
         report.errors={:?}",
        report.errors
    );

    // 建立出来的订阅必须挂在**真实存在**的目录上（外键完整性）
    let orphan: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM feeds f
             WHERE NOT EXISTS (SELECT 1 FROM folders d WHERE d.id = f.folder_id)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(orphan, 0, "订阅不得挂在不存在目录上");
}

/// TASK-056（修复轮 1）：**锁定「失败必须可见」契约本身**。
///
/// 第 1 轮独立审查指出：`pull_feed_failure_is_reported_not_swallowed` 的断言是
/// 析取式 `local_feeds > 0 || !report.errors.is_empty()`，一旦目录兜底修好，
/// 左式恒真 → **把 `Err` 分支改回静默吞错后全套测试仍全绿**（审查者 Probe C 实证：
/// 回退错误上报后 cargo test 仍 139 passed / 0 failed），即该契约其实没有任何守卫。
///
/// 本测试用**确定性制造一次真实插入失败**来锁定它：在测试库上给 `feeds` 加一条
/// `BEFORE INSERT` 触发器，遇到目标 URL 时 `RAISE(ABORT)`。
/// 这是在测试库注入失败的标准做法——**不改生产代码、不依赖任何巧合**，
/// 且与真实缺陷走同一条代码路径（无分类订阅 → 目录兜底 → 插入 → Err 分支）。
///
/// 断言为**强断言**（非析取式）：`report.errors` 必须非空，且必须包含失败的订阅 URL。
/// 只要把 `Err` 分支改回 `if let Ok(...)` 静默形式，本测试立即失败。
#[tokio::test]
async fn pull_feed_insert_failure_must_be_reported() {
    let (db, http, server) = setup("gap_pull_force_fail").await;

    // 远端：一条无分类订阅（走目录兜底分支，与真实缺陷同路径）
    server.folders.lock().unwrap().clear();
    let failing_url = "http://127.0.0.1:8765/will_fail.xml";
    *server.subscriptions.lock().unwrap() = vec![mock_greader::MockSubscription {
        id: "feed/10".into(),
        title: "Will Fail".into(),
        url: failing_url.into(),
        html_url: None,
        categories: vec![],
    }];

    // 在测试库上注入确定性失败：插该 URL 时 RAISE(ABORT)
    {
        let conn = db.lock().await;
        conn.execute_batch(&format!(
            "CREATE TRIGGER t056_fail_insert BEFORE INSERT ON feeds
             WHEN NEW.feed_url = '{failing_url}'
             BEGIN SELECT RAISE(ABORT, 't056 injected insert failure'); END;"
        ))
        .expect("install fault-injection trigger");
    }

    let report = sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase should still return Ok（失败经 report.errors 上报，不上抛）");

    // ① 强断言：错误必须被上报（静默吞错则此处失败）
    assert!(
        !report.errors.is_empty(),
        "注入的插入失败必须写入 report.errors（不得静默吞掉，TASK-056）；\
         report.errors={:?}",
        report.errors
    );
    // ② 强断言：错误信息必须包含出错的订阅 URL，可定位
    assert!(
        report.errors.iter().any(|e| e.contains(failing_url)),
        "错误信息应包含失败的订阅 URL 以便定位；report.errors={:?}",
        report.errors
    );
    // ③ 该订阅确实没被建出来（确认失败真的发生了，而非触发器未生效）
    {
        let conn = db.lock().await;
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM feeds WHERE feed_url = ?1",
                [failing_url],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0, "触发器应阻止该订阅插入（前置条件校验）");
    }
}

/// TASK-056（修复轮 1）：**锁定条目路径的「失败必须可见」契约**。
///
/// 与 `pull_feed_insert_failure_must_be_reported` 同理，但针对 `entries.rs` 的
/// pull 条目插入分支——第 1 轮审查的 Probe D 证实：把该处 `Err` 上报回退为
/// `if let Ok(...)` 静默形式后，全套测试仍 139 passed / 0 failed，即该契约同样无守卫。
///
/// 手段：给 `articles` 加一条 `BEFORE INSERT` 触发器，遇到目标 guid 时 `RAISE(ABORT)`。
/// 订阅与文章都预先造好，确保走到「远端条目 → 建本地文章」这条路径。
#[tokio::test]
async fn pull_entry_insert_failure_must_be_reported() {
    let (db, http, server) = setup("gap_entry_force_fail").await;

    // 本地：一个已绑定远端的 feed（文章要挂到它下面）
    let feed_id = {
        let conn = db.lock().await;
        let folder_id = db::create_folder(&conn, "条目测试", "article").unwrap();
        let fid = db::insert_feed(
            &conn,
            "http://127.0.0.1:8765/local_feed.xml",
            None,
            "Local",
            None,
            folder_id,
            "inherit",
            true,
            false,
        )
        .unwrap();
        db::set_feed_remote_id(&conn, fid, 10).unwrap();
        fid
    };
    let _ = feed_id;

    // 远端：feed/10 上一条条目（本地没有 → 走 upsert 新建分支）
    let entry_url = "http://127.0.0.1:8765/post/will_fail";
    let entry_id = server.add_entry_ret(10, entry_url, "Will Fail", false, false);

    // 注入确定性失败：插该 guid 的文章时 RAISE(ABORT)
    let guid = format!("remote-{entry_id}");
    {
        let conn = db.lock().await;
        conn.execute_batch(&format!(
            "CREATE TRIGGER t056_fail_entry BEFORE INSERT ON articles
             WHEN NEW.guid = '{guid}'
             BEGIN SELECT RAISE(ABORT, 't056 injected entry insert failure'); END;"
        ))
        .expect("install entries fault trigger");
    }

    let report = sync::states_phase(&db, &http, true)
        .await
        .expect("states phase should still return Ok（失败经 report.errors 上报）");

    // 强断言：条目插入失败必须被上报，且含可定位的 URL
    assert!(
        !report.errors.is_empty(),
        "注入的条目插入失败必须写入 report.errors（不得静默吞掉，TASK-056）；\
         report.errors={:?}",
        report.errors
    );
    assert!(
        report.errors.iter().any(|e| e.contains(entry_url)),
        "条目错误信息应包含失败的条目 URL 以便定位；report.errors={:?}",
        report.errors
    );
}

/// TASK-056：pull 建订阅**不得静默失败**——要么真的建出来，要么必须给出原因。
///
/// **本测试的定位（经第 1 轮独立审查订正）**：它的断言是**析取式**
/// `local_feeds > 0 || !report.errors.is_empty()`。目录兜底修好后左式恒真，
/// 因此它**不能**单独守住「失败必须可见」这条契约（审查者 Probe C 实证：
/// 回退错误上报后本测试仍通过）。该契约现由
/// `pull_feed_insert_failure_must_be_reported`（注入确定性插入失败 + 强断言）锁定。
///
/// 本测试保留的价值：作为**缺陷复现锁**——把目录兜底回退为 `unwrap_or(1)` 后，
/// 本地订阅为 0 且 errors 为空，两者同时为假 → 失败。即它防的是「再次建不出」，
/// 而非「再次静默」。
///
/// 必须清空 mock 的 folders（tag/list）与给订阅**无分类**，否则 pull 会先按远端分类
/// 建出目录、本地 folders 不再为空，便走不到目录兜底分支（这正是其它测试漏掉该缺陷的原因）。
#[tokio::test]
async fn pull_feed_failure_is_reported_not_swallowed() {
    let (db, http, server) = setup("gap_pull_visible").await;
    // 清空远端分类 + 只留一条无分类订阅 → 必然走目录兜底分支
    server.folders.lock().unwrap().clear();
    *server.subscriptions.lock().unwrap() = vec![mock_greader::MockSubscription {
        id: "feed/10".into(),
        title: "Remote".into(),
        url: "http://127.0.0.1:8765/local_feed.xml".into(),
        html_url: None,
        categories: vec![],
    }];

    let report = sync::feeds_phase(&db, &http).await.expect("feeds phase");

    let conn = db.lock().await;
    let local_feeds: i64 = conn
        .query_row("SELECT COUNT(*) FROM feeds", [], |r| r.get(0))
        .unwrap();

    // 契约：要么真的建出来了；要么没建出来但必须给出原因（不得静默）
    assert!(
        local_feeds > 0 || !report.errors.is_empty(),
        "零订阅且零错误 = 静默失败，禁止（TASK-056）；\
         local_feeds={local_feeds} report.errors={:?}",
        report.errors
    );
}

/// A-2 修复后的期望行为（原复现测试转正）：订阅改名/移动目录会 best-effort
/// 推送远端（GReader ac=edit，t=新标题 / a=目标分类）。
#[tokio::test]
async fn feed_rename_and_move_push_edit_subscription() {
    let (db, http, server) = setup("gap_rename").await;
    let (feed_id, target_folder) = {
        let conn = db.lock().await;
        let folder_id = db::create_folder(&conn, "测试分类", "article").unwrap();
        let target = db::create_folder(&conn, "目标分类", "article").unwrap();
        let feed_id = db::insert_feed(
            &conn,
            "http://127.0.0.1:8765/local_feed.xml", // 与 mock feed 10 同 URL
            None,
            "Old Title",
            None,
            folder_id,
            "inherit",
            true,
            false,
        )
        .unwrap();
        (feed_id, target)
    };
    // 绑定远端（remote_id=10）
    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase (bind)");

    // ① 改名：命令层真实逻辑返回待推送目标（remote_id + 新标题）
    let push = {
        let conn = db.lock().await;
        app_lib::commands::record_feed_edit(
            &conn,
            feed_id,
            Some("Brand New Title"),
            None,
            None,
            None,
            None,
        )
        .expect("record edit")
    };
    assert!(
        matches!(&push, Some((10, Some(t), None)) if t == "Brand New Title"),
        "已绑定远端且已配置时应返回 (10, 新标题, None)，实际 {push:?}"
    );
    let (rid, title, label) = push.unwrap();
    assert!(
        sync::edit_remote_subscription(&db, &http, rid, title.as_deref(), label.as_deref()).await,
        "改名推送应成功"
    );
    let form = mock_greader::last_subscription_edit_form(&server);
    let has = |k: &str, v: &str| form.iter().any(|(fk, fv)| fk == k && fv == v);
    assert!(
        has("ac", "edit") && has("s", "feed/10"),
        "远端应收到 ac=edit 且 s=feed/10，实际 {form:?}"
    );
    assert!(
        has("t", "Brand New Title"),
        "远端应收到新标题 t=Brand New Title，实际 {form:?}"
    );

    // ② 移动目录：a=目标分类名
    let push2 = {
        let conn = db.lock().await;
        app_lib::commands::record_feed_edit(
            &conn,
            feed_id,
            None,
            Some(target_folder),
            None,
            None,
            None,
        )
        .expect("record edit (move)")
    };
    assert!(
        matches!(&push2, Some((10, None, Some(l))) if l == "目标分类"),
        "移动目录应返回 (10, None, 目标分类名)，实际 {push2:?}"
    );
    let (rid2, title2, label2) = push2.unwrap();
    assert!(
        sync::edit_remote_subscription(&db, &http, rid2, title2.as_deref(), label2.as_deref())
            .await,
        "移动目录推送应成功"
    );
    let form2 = mock_greader::last_subscription_edit_form(&server);
    assert!(
        form2.iter().any(|(k, v)| k == "a" && v == "目标分类"),
        "远端应收到 a=目标分类，实际 {form2:?}"
    );
}

/// A-5 修复后的期望行为（原复现测试转正）：离线（未配置凭据）期间的已读变更
/// 现在也会入队（commands::record_read_state），连接后 states_phase 推送段补推。
#[tokio::test]
async fn offline_read_change_pushed_after_connect() {
    // "离线"阶段：未配置任何凭据
    let server = MockGReader::start().await.expect("start mock server");
    let tmp = std::env::temp_dir().join(format!(
        "fluxreader_gap_offline_{}_{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&tmp);
    let db = Arc::new(Mutex::new(db::open(&tmp).expect("open db")));
    let http = app_lib::ingestion::build_client(10);

    // 离线阶段：走命令层真实逻辑（record_read_state = set_read + 入队）
    let aid = seed_local_article(&db, &server, "http://127.0.0.1:8765/post/1", false).await;
    {
        let conn = db.lock().await;
        app_lib::commands::record_read_state(&conn, aid, true).expect("record read");
    }
    let queued: i64 = {
        let conn = db.lock().await;
        conn.query_row("SELECT COUNT(*) FROM sync_queue", [], |r| r.get(0))
            .unwrap()
    };
    assert!(queued >= 1, "离线变更也应写入同步队列（A-5）");

    // "连接"：配置凭据后做全量同步（绑定回填）——首推时条目可能尚未绑定，
    // 队列保留；第二次 states_phase 在绑定完成后补推
    {
        let conn = db.lock().await;
        db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
        db::set_setting(&conn, "greader_username", "test").unwrap();
        db::set_setting(&conn, "greader_password", "test-token").unwrap();
    }
    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase (bind feed)");
    sync::states_phase(&db, &http, true)
        .await
        .expect("states phase (bind article)");

    // 连接后的补推：推送段消费队列（即时推送路径与 states_phase 推送段共用 plan_push）
    sync::push_states_now(&db, &http).await;

    // 修复期望：离线变更被补推到远端
    {
        let updates = server.status_updates.lock().unwrap();
        assert!(
            updates.iter().any(|(_eid, st)| *st == "read"),
            "修复期望：离线已读变更在连接后被补推（收到 edit-tag read）"
        );
    }
    let remains: i64 = {
        let conn = db.lock().await;
        conn.query_row("SELECT COUNT(*) FROM sync_queue", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(remains, 0, "补推完成后队列应清空");
}

/// A-5 同源残留修复（TASK-053 / P1-5）：离线（未配置凭据）期间的「全部已读」
/// 也必须入队——mark_all_read 此前仍以 sync_configured 作为入队前置条件，
/// 离线标读永不补推，且连接后首次全量对账按远端状态把本地已读翻回未读
/// （用户现象：「刚标的已读自己变回去了」）。
///
/// 断言两段（与 offline_read_change_pushed_after_connect 同口径，只是入口换成
/// mark_all_read 的范围标读路径）：
///   ① 未配置时 commands::apply_mark_all_read 仍写入 sync_queue（入队不受配置影响）；
///   ② 连接后 states_phase 推送段确实把该动作推给后端（远端收到 edit-tag read），
///      且推送完成后队列清空。
#[tokio::test]
async fn offline_mark_all_read_queued_and_pushed_after_connect() {
    // "离线"阶段：未配置任何凭据
    let server = MockGReader::start().await.expect("start mock server");
    let tmp = std::env::temp_dir().join(format!(
        "fluxreader_gap_offline_all_{}_{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&tmp);
    let db = Arc::new(Mutex::new(db::open(&tmp).expect("open db")));
    let http = app_lib::ingestion::build_client(10);

    // 离线阶段：造两篇未读本地文章（URL 与 mock feed 10 的远端 entry 对齐，
    // 供连接后的绑定回填）
    let aids = seed_local_articles_in_one_feed(
        &db,
        &server,
        &[
            "http://127.0.0.1:8765/post/all-1",
            "http://127.0.0.1:8765/post/all-2",
        ],
    )
    .await;
    let (a1, a2) = (aids[0], aids[1]);

    // 命令层真实逻辑（mark_all_read 的范围标读 + 入队）；此处显式前置断言
    // "未配置"以证明测试确实覆盖离线分支
    let (n, scoped) = {
        let conn = db.lock().await;
        assert!(
            sync::read_credentials(&conn).is_none(),
            "前置条件：离线阶段必须未配置同步凭据"
        );
        let ids = db::list_unread_ids_scoped(&conn, None, None, false, None).unwrap();
        let n = app_lib::commands::apply_mark_all_read(&conn, None, None, false, None)
            .expect("mark_all_read (offline)");
        (n, ids)
    };
    assert_eq!(n, 2, "离线「全部已读」应标读两篇未读文章");
    assert_eq!(scoped.len(), 2, "前置条件：入队集合应含两篇未读文章");

    // ① 入队成功：未配置也必须写入待推队列（修复点）
    let queued: Vec<(Option<i64>, String)> = {
        let conn = db.lock().await;
        let mut stmt = conn
            .prepare("SELECT article_id, action FROM sync_queue ORDER BY id")
            .unwrap();
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        rows
    };
    assert_eq!(
        queued.len(),
        2,
        "修复期望：未配置时「全部已读」也入队（A-5），实际 {queued:?}"
    );
    for id in [a1, a2] {
        assert!(
            queued
                .iter()
                .any(|(aid, act)| *aid == Some(id) && act == "read"),
            "修复期望：文章 {id} 应有 read 待推项，实际 {queued:?}"
        );
    }

    // "连接"：配置凭据后做全量同步（feeds 绑定订阅 → states 绑定条目并补推）
    {
        let conn = db.lock().await;
        db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
        db::set_setting(&conn, "greader_username", "test").unwrap();
        db::set_setting(&conn, "greader_password", "test-token").unwrap();
    }
    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase (bind feed)");
    sync::states_phase(&db, &http, true)
        .await
        .expect("states phase (bind articles)");

    // ② 连接后推送段确实把该动作推给后端（远端收到 edit-tag read）
    {
        let updates = server.status_updates.lock().unwrap();
        assert!(
            updates.iter().any(|(_eid, st)| st == "read"),
            "修复期望：离线「全部已读」在连接后被推送到后端（收到 edit-tag read），实际 {updates:?}"
        );
    }

    let remains: i64 = {
        let conn = db.lock().await;
        conn.query_row("SELECT COUNT(*) FROM sync_queue", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(remains, 0, "补推完成后队列应清空");
}

/// A-5 同源测试补测（TASK-053 repair 轮 2）：离线（未配置凭据）期间的**收藏**
/// 变更（commands::record_star_state = set_starred 的真实代码路径）也必须入队，
/// 连接后 states_phase 推送段补推。既有两处 record_star_state 调用
/// （reconcile_skipped_when_state_fetch_fails / fresh_queue_items_survive_aging）
/// 都在已配置态，离线入队分支无测试保护——本测试补上该分支。
///
/// 与 offline_read_change_pushed_after_connect 同形（同样的"离线 → 连接 → 补推"
/// 两段结构），只是把 read 换成 star（Google Reader 的收藏语义：add/remove
/// `com.google/starred` 标签，非 toggle）。
///
/// 断言两段：
///   ① 未配置时 record_star_state 仍写入 sync_queue（action=star）；
///   ② 连接后 states_phase 补推段确实把该动作推给后端（mock 收到 edit-tag 的
///      starred 标签变更），且推送完成后队列清空。
#[tokio::test]
async fn offline_star_change_pushed_after_connect() {
    // "离线"阶段：未配置任何凭据
    let server = MockGReader::start().await.expect("start mock server");
    let tmp = std::env::temp_dir().join(format!(
        "fluxreader_gap_offline_star_{}_{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&tmp);
    let db = Arc::new(Mutex::new(db::open(&tmp).expect("open db")));
    let http = app_lib::ingestion::build_client(10);

    // 离线阶段：造一篇未读未收藏的本地文章（URL 与 mock feed 10 的远端 entry
    // 对齐，供连接后的绑定回填）
    let aid = seed_local_article(&db, &server, "http://127.0.0.1:8765/post/star-1", false).await;

    // 命令层真实逻辑（set_starred → record_star_state = 落库 + 入队）；此处显式
    // 前置断言"未配置"以证明测试确实覆盖离线分支
    {
        let conn = db.lock().await;
        assert!(
            sync::read_credentials(&conn).is_none(),
            "前置条件：离线阶段必须未配置同步凭据"
        );
        app_lib::commands::record_star_state(&conn, aid, true).expect("record star");
    }

    // ① 入队成功：未配置也必须写入待推队列（A-5）
    let queued: Vec<(Option<i64>, String)> = {
        let conn = db.lock().await;
        let mut stmt = conn
            .prepare("SELECT article_id, action FROM sync_queue ORDER BY id")
            .unwrap();
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        rows
    };
    assert_eq!(
        queued,
        vec![(Some(aid), "star".to_string())],
        "修复期望：未配置时收藏变更也入队（A-5），实际 {queued:?}"
    );
    // 本地状态同批落库（命令层真实路径的落库半边）
    {
        let conn = db.lock().await;
        let starred: i64 = conn
            .query_row(
                "SELECT is_starred FROM articles WHERE id = ?1",
                [aid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(starred, 1, "离线收藏应落在本地");
    }

    // "连接"：配置凭据后做全量同步（feeds 绑定订阅 → states 绑定条目并补推）
    {
        let conn = db.lock().await;
        db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
        db::set_setting(&conn, "greader_username", "test").unwrap();
        db::set_setting(&conn, "greader_password", "test-token").unwrap();
    }
    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase (bind feed)");
    sync::states_phase(&db, &http, true)
        .await
        .expect("states phase (bind article)");

    // ② 连接后推送段确实把该动作推给后端：mock 的 edit-tag 处理器只在收到
    //    com.google/starred 标签变更时记录 bookmark_toggles（star 与 unstar 都记）
    {
        let toggles = server.bookmark_toggles.lock().unwrap();
        assert!(
            !toggles.is_empty(),
            "修复期望：离线收藏变更在连接后被推送到后端（收到 edit-tag starred），实际 {toggles:?}"
        );
    }
    let remote_starred: i64 = {
        let toggles = server.bookmark_toggles.lock().unwrap();
        let entries = server.entries.lock().unwrap();
        let eid = toggles[0];
        entries
            .iter()
            .find(|e| e.id == eid)
            .map(|e| i64::from(e.starred))
            .unwrap_or(-1)
    };
    assert_eq!(
        remote_starred, 1,
        "修复期望：后端 entry 应被标为已收藏（edit-tag a=starred）"
    );

    // ③ 补推完成后队列清空
    let remains: i64 = {
        let conn = db.lock().await;
        conn.query_row("SELECT COUNT(*) FROM sync_queue", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(remains, 0, "补推完成后队列应清空");
}

/// C-1 修复后的期望行为：GReader 权威状态集合拉取失败时跳过对账，
/// 绝不把"拉取失败"当"空集合"清空本地收藏。
#[tokio::test]
async fn reconcile_skipped_when_state_fetch_fails() {
    let (db, http, server) = setup("gap_c1").await;
    let (aid, _rid) = {
        let conn = db.lock().await;
        let folder_id = db::create_folder(&conn, "测试分类", "article").unwrap();
        let feed_id = db::insert_feed(
            &conn,
            "http://127.0.0.1:8765/local_feed.xml",
            None,
            "Local Direct Feed",
            None,
            folder_id,
            "inherit",
            true,
            false,
        )
        .unwrap();
        let a = db::NewArticle {
            guid: "guid-c1".into(),
            url: Some("http://127.0.0.1:8765/post/1".into()),
            title: "Local Article".into(),
            author: None,
            summary: None,
            content_html: Some("<p>local</p>".into()),
            body_text: "local".into(),
            image_url: None,
            enclosure_url: None,
            enclosure_mime: None,
            duration_sec: None,
            published_at: Some(chrono::Utc::now().to_rfc3339()),
            source: "direct".into(),
        };
        let aid = db::upsert_article_with_feed(&conn, feed_id, &a, false)
            .unwrap()
            .0;
        (aid, ())
    };

    // 首次全量同步：绑定 feed 与文章
    sync::feeds_phase(&db, &http).await.expect("feeds phase");
    sync::states_phase(&db, &http, true)
        .await
        .expect("states phase 1");

    // 本地收藏（走命令层真实逻辑，入队）
    {
        let conn = db.lock().await;
        app_lib::commands::record_star_state(&conn, aid, true).expect("record star");
    }
    // 推送收藏成功后，注入状态集合拉取故障
    sync::states_phase(&db, &http, true)
        .await
        .expect("states phase 2 (push star)");
    server
        .fail_stream_ids
        .store(true, std::sync::atomic::Ordering::SeqCst);

    // 轻量同步触发对账：read/starred 集合拉取失败 → 必须跳过对账
    let report = sync::states_phase(&db, &http, false)
        .await
        .expect("states phase 3");
    assert!(
        report.errors.iter().any(|e| e.contains("状态对账跳过")),
        "修复期望：对账因集合拉取失败被跳过并记录 errors"
    );

    // 本地收藏保持：绝不被"空集合对账"静默取消
    let conn = db.lock().await;
    let starred: i64 = conn
        .query_row(
            "SELECT is_starred FROM articles WHERE id = ?1",
            [aid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(starred, 1, "修复期望：拉取失败时本地收藏不被清空");
}

/// A-2 边界：未绑定远端或未配置同步时，编辑只落本地、不推送、不报错。
#[tokio::test]
async fn feed_edit_without_backend_stays_local() {
    // 不配置任何凭据 = 未连接后端
    let tmp = std::env::temp_dir().join(format!(
        "fluxreader_gap_localedit_{}_{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&tmp);
    let db = Arc::new(Mutex::new(db::open(&tmp).expect("open db")));
    let feed_id = {
        let conn = db.lock().await;
        let folder_id = db::create_folder(&conn, "本地分类", "article").unwrap();
        db::insert_feed(
            &conn,
            "http://127.0.0.1:8765/local_feed.xml",
            None,
            "Old Title",
            None,
            folder_id,
            "inherit",
            true,
            false,
        )
        .unwrap()
    };

    let push = {
        let conn = db.lock().await;
        app_lib::commands::record_feed_edit(
            &conn,
            feed_id,
            Some("Locally Renamed"),
            None,
            None,
            None,
            None,
        )
        .expect("record edit (local only)")
    };
    assert!(
        push.is_none(),
        "未配置同步时不应产生待推送目标，实际 {push:?}"
    );

    let conn = db.lock().await;
    let title: String = conn
        .query_row("SELECT title FROM feeds WHERE id = ?1", [feed_id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        title, "Locally Renamed",
        "本地改名必须生效（不受推送状态影响）"
    );
}

/// A-3：add_feed 队列携带的目标分类在推送后补挂到远端（quick_add 只有 URL，
/// 若不补 edit_subscription(a=label)，OPML/add_feed 选的目录在远端会落默认分类）。
#[tokio::test]
async fn add_feed_pushes_folder_membership() {
    let (db, http, server) = setup("gap_a3").await;
    let url = "http://example.com/new-feed.xml";
    let _folder_id = {
        let conn = db.lock().await;
        let fid = db::create_folder(&conn, "目标分类", "article").unwrap();
        // 本地先落订阅行（推送绑定时按 URL 匹配）
        db::insert_feed(
            &conn, url, None, "New Feed", None, fid, "inherit", true, false,
        )
        .unwrap();
        // 命令层等价入队：payload 携带目标分类 id
        db::enqueue_sync(
            &conn,
            None,
            Some(url),
            "add_feed",
            Some(&serde_json::json!({ "folder_id": fid }).to_string()),
        )
        .unwrap();
        fid
    };

    sync::feeds_phase(&db, &http).await.expect("feeds phase");

    // quick_add 收到订阅
    assert!(
        server
            .subscribed_urls
            .lock()
            .unwrap()
            .iter()
            .any(|u| u == url),
        "远端应收到 quick_add 订阅"
    );
    // 补挂分类：最后一次 subscription/edit 应为 a=目标分类（远端 feed 101）
    let form = mock_greader::last_subscription_edit_form(&server);
    let get = |k: &str| form.iter().find(|(fk, _)| fk == k).map(|(_, v)| v.clone());
    assert_eq!(
        get("ac").as_deref(),
        Some("edit"),
        "应补发 ac=edit，实际 {form:?}"
    );
    assert_eq!(
        get("a").as_deref(),
        Some("目标分类"),
        "应携带 a=目标分类（A-3），实际 {form:?}"
    );
    assert_eq!(
        get("s").as_deref(),
        Some("feed/101"),
        "应指向 quick_add 返回的远端 id"
    );

    // 本地绑定远端 id（quick_add 返回 feed/101）
    let conn = db.lock().await;
    let bound: Option<i64> = conn
        .query_row(
            "SELECT remote_id FROM feeds WHERE feed_url = ?1",
            [url],
            |r| r.get(0),
        )
        .ok();
    assert_eq!(bound, Some(101), "本地订阅应绑定 quick_add 返回的远端 id");
}

/// A-4：分类改名后，远端旧 label 不再被 pull 复活成重复空目录。
#[tokio::test]
async fn folder_rename_does_not_revive_old_label() {
    let (db, http, _server) = setup("gap_a4r").await;
    // 远端 tag/list 默认含 "Default"；本地同名分类改名后应留下墓碑
    let folder_id = {
        let conn = db.lock().await;
        db::create_folder(&conn, "Default", "article").unwrap()
    };
    sync::feeds_phase(&db, &http).await.expect("feeds phase 1");

    {
        let conn = db.lock().await;
        app_lib::commands::record_folder_rename(&conn, folder_id, "重命名分类").expect("rename");
        assert!(
            db::folder_tombstones(&conn)
                .unwrap()
                .iter()
                .any(|l| l == "Default"),
            "改名应写入旧 label 墓碑"
        );
    }

    sync::feeds_phase(&db, &http).await.expect("feeds phase 2");

    let conn = db.lock().await;
    let revived: Option<i64> = conn
        .query_row("SELECT id FROM folders WHERE name = 'Default'", [], |r| {
            r.get(0)
        })
        .ok();
    let renamed: Option<i64> = conn
        .query_row(
            "SELECT id FROM folders WHERE name = '重命名分类'",
            [],
            |r| r.get(0),
        )
        .ok();
    assert!(
        revived.is_none(),
        "旧 label 不应被 pull 复活为空目录（A-4）"
    );
    assert!(renamed.is_some(), "新名称保留");
}

/// A-4：删除分类后，目录与其内订阅均不复活（目录墓碑 + 订阅补墓碑）。
#[tokio::test]
async fn folder_delete_does_not_revive_folder_or_feeds() {
    let (db, http, _server) = setup("gap_a4d").await;
    let feed_url = "http://127.0.0.1:8765/local_feed.xml"; // 远端订阅列表含该 URL
    let folder_id = {
        let conn = db.lock().await;
        let fid = db::create_folder(&conn, "Default", "article").unwrap();
        db::insert_feed(
            &conn,
            feed_url,
            None,
            "Feed In Default",
            None,
            fid,
            "inherit",
            true,
            false,
        )
        .unwrap();
        fid
    };
    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase (bind)");

    {
        let conn = db.lock().await;
        app_lib::commands::record_folder_delete(&conn, folder_id).expect("delete folder");
        let t = db::folder_tombstones(&conn).unwrap();
        let ft = db::feed_tombstones(&conn).unwrap();
        assert!(t.iter().any(|l| l == "Default"), "删除应写入目录墓碑");
        assert!(
            ft.iter().any(|u| u.contains("local_feed.xml")),
            "应为其内订阅补墓碑"
        );
    }

    sync::feeds_phase(&db, &http).await.expect("feeds phase 2");

    let conn = db.lock().await;
    let folder: Option<i64> = conn
        .query_row("SELECT id FROM folders WHERE name = 'Default'", [], |r| {
            r.get(0)
        })
        .ok();
    let feed: Option<i64> = conn
        .query_row(
            "SELECT id FROM feeds WHERE feed_url = ?1",
            [feed_url],
            |r| r.get(0),
        )
        .ok();
    assert!(folder.is_none(), "删除的目录不应复活（A-4）");
    assert!(feed.is_none(), "目录内订阅不应随目录复活（A-4 补墓碑）");
}

/// A-4：分类墓碑在远端不再列出该 label 后被清除（避免墓碑永久堆积）。
#[tokio::test]
async fn folder_tombstone_cleared_when_remote_drops_label() {
    let (db, http, server) = setup("gap_a4c").await;
    sync::feeds_phase(&db, &http).await.expect("feeds phase 1");

    {
        let conn = db.lock().await;
        // 手工写入一个远端 tag/list 已不含的 label 墓碑（模拟远端已删除该分类）
        db::add_folder_tombstone(&conn, "已消失分类").unwrap();
        assert!(
            db::folder_tombstones(&conn)
                .unwrap()
                .iter()
                .any(|l| l == "已消失分类"),
            "前置：墓碑已写入"
        );
    }

    // 远端 tag/list 不含该 label（mock 默认 folders 无此项）→ pull 应清墓碑
    sync::feeds_phase(&db, &http).await.expect("feeds phase 2");

    let conn = db.lock().await;
    assert!(
        !db::folder_tombstones(&conn)
            .unwrap()
            .iter()
            .any(|l| l == "已消失分类"),
        "远端已不含的 label 墓碑应被清除"
    );
    // 仍未消失的标签（远端含）其墓碑保留：补一个正例对照
    db::add_folder_tombstone(&conn, "Default").unwrap();
    drop(conn);
    sync::feeds_phase(&db, &http).await.expect("feeds phase 3");
    let conn = db.lock().await;
    assert!(
        db::folder_tombstones(&conn)
            .unwrap()
            .iter()
            .any(|l| l == "Default"),
        "远端仍列出的 label 墓碑应保留（继续阻挡复活）"
    );
    let _ = server;
}

/// A-8：超过保留期且仍无法绑定远端的状态队列项被老化清理并记录。
#[tokio::test]
async fn stale_unbound_queue_items_are_pruned() {
    let (db, http, _server) = setup("gap_a8").await;
    // 本地直连文章：URL 在远端不存在对应 entry → 永不绑定 remote_id
    let aid = {
        let conn = db.lock().await;
        let folder_id = db::create_folder(&conn, "本地分类", "article").unwrap();
        let feed_id = db::insert_feed(
            &conn,
            "http://example.com/never-in-remote.xml",
            None,
            "Local Only Feed",
            None,
            folder_id,
            "inherit",
            true,
            false,
        )
        .unwrap();
        let a = db::NewArticle {
            guid: "guid-a8".into(),
            url: Some("http://example.com/never-in-remote-post".into()),
            title: "Local Only Article".into(),
            author: None,
            summary: None,
            content_html: Some("<p>x</p>".into()),
            body_text: "x".into(),
            image_url: None,
            enclosure_url: None,
            enclosure_mime: None,
            duration_sec: None,
            published_at: Some(chrono::Utc::now().to_rfc3339()),
            source: "direct".into(),
        };
        db::upsert_article_with_feed(&conn, feed_id, &a, false)
            .unwrap()
            .0
    };
    {
        let conn = db.lock().await;
        app_lib::commands::record_read_state(&conn, aid, true).expect("enqueue read");
        // 回填入队时间为 40 天前（超过 30 天保留期）
        let old = (chrono::Utc::now() - chrono::Duration::days(40))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        conn.execute("UPDATE sync_queue SET created_at = ?1", [old])
            .unwrap();
    }

    let report = sync::states_phase(&db, &http, false)
        .await
        .expect("states phase");

    let left: i64 = {
        let conn = db.lock().await;
        conn.query_row("SELECT COUNT(*) FROM sync_queue", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(left, 0, "陈旧且无法绑定的队列项应被老化清理");
    assert!(
        report.errors.iter().any(|e| e.contains("队列老化")),
        "应记录老化清理动作，实际 {:?}",
        report.errors
    );
}

/// A-8 对照：保留期内的队列项不受老化影响（避免误清正在等绑定的变更）。
#[tokio::test]
async fn fresh_queue_items_survive_aging() {
    let (db, http, _server) = setup("gap_a8b").await;
    let aid = {
        let conn = db.lock().await;
        let folder_id = db::create_folder(&conn, "本地分类", "article").unwrap();
        let feed_id = db::insert_feed(
            &conn,
            "http://example.com/never-in-remote.xml",
            None,
            "Local Only Feed",
            None,
            folder_id,
            "inherit",
            true,
            false,
        )
        .unwrap();
        let a = db::NewArticle {
            guid: "guid-a8b".into(),
            url: Some("http://example.com/never-in-remote-post".into()),
            title: "Local Only Article".into(),
            author: None,
            summary: None,
            content_html: Some("<p>x</p>".into()),
            body_text: "x".into(),
            image_url: None,
            enclosure_url: None,
            enclosure_mime: None,
            duration_sec: None,
            published_at: Some(chrono::Utc::now().to_rfc3339()),
            source: "direct".into(),
        };
        db::upsert_article_with_feed(&conn, feed_id, &a, false)
            .unwrap()
            .0
    };
    {
        let conn = db.lock().await;
        app_lib::commands::record_star_state(&conn, aid, true).expect("enqueue star");
    }

    let _ = sync::states_phase(&db, &http, false)
        .await
        .expect("states phase");

    let left: i64 = {
        let conn = db.lock().await;
        conn.query_row("SELECT COUNT(*) FROM sync_queue", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(left, 1, "保留期内的队列项应保留（等待绑定后补推）");
}
