//! OPT-007（F09/F10/F11）：字段版本持久守卫与 GR 可靠 merge 的**屏障时序**端到端测试。
//!
//! 与单元测试的分工：单元测试锁「版本守卫的判定表」；本文件用真实 HTTP mock
//! （`mock_greader` 的响应在途屏障：响应体算定后、写回前挂起）驱动任务卡
//! 精确反例的**网络时间线**——测试在响应在途期间对真实 SQLite 库执行生产写入
//! 入口（含「推送成功 → prune 清队」的确认模拟），触发顺序完全可复现，不用
//! sleep 碰运气。
//!
//! 覆盖任务卡负例：
//! ① 请求发出 → localstar → 推送确认清队 → 旧远端 unstar 快照返回：star 保留
//!    （只看当前 pending 的实现会取消 star——本用例判别该形态）；
//! ② 只 read 字段本地改动：远端 star 无冲突照常收敛；反向（本地 star 改动、
//!    远端 read 收敛）由同一轮第二个条目锁定——字段彼此独立；
//! ③ 新条目入库后立即本地变更：同一轮后续分页/对账也保护（版本快照在入库时
//!    记录）；
//! ④ GR 匹配映射构建失败不得空映射假成功（游标不推进、修复后重拉合并）；
//!    批合并失败后 maps 按 DB 重建，重拉不跳过失败条目。
//!
//! R2（主控 2026-10-10 反例，已并入）：
//! ⑤ reading-list id 列举在途期间**直连插入同 feed 同 URL 行**并标读/收藏、
//!    模拟推送确认清队 → 该行不在 pull 起点快照映射里，upsert 命中既有行
//!    （inserted=false）：状态不得被旧远端快照整值覆写（修前 P2：状态由
//!    (1,1,1,1) 回滚为 (0,0,1,1)）；
//! ⑥ 既有行单字段冲突（读）不得压住另一字段（收藏）的远端收敛——upsert
//!    既有行分支也必须逐字段过版本守卫；
//! ⑦ 既有非零版本本轮未改仍可接收远端：快照 == 当前版本 且无 pending 时，
//!    版本非零不得被误当成「有本地意图」而拒绝（防过度保守），远端合并也不
//!    递增版本。
//!
//! R3（主控 2026-10-10 二轮 P2，F11 实际变化标记）：
//! ⑧ 同 ⑤ 的并发直连行场景：双字段受守卫保护（状态不丢）但**标题/正文被
//!    upsert 真实改写、remote_id 完成绑定** → entries_data_changed /
//!    has_data_change 必须为 true（修前 title 已变而所有计数为 0 → 不发事件）；
//! ⑨ 旧已绑定行（own）的**实际回填**同理：空正文被远端补进 → 标记必须为 true，
//!    状态字段不受牵连；
//! ⑩ 真正新行：新插入必须置标记（且旧计数兼容地 +1）；
//! ⑪ 同值 replay（changed_at 前移但字段全同）：守卫通过 → 旧计数照常累计，
//!    但**不得**置标记、不得发事件（零实际变化）。
//!
//! R4（主控 R3 初审复核，GR 逐语句提交的标记时序）：
//! ⑫ 已绑定条目远端 read/star 同时变更：read 写先提交、star 写被真 DB 触发器
//!    ABORT——已提交的 read 变化必须**即时**留有标记（修前攒到函数尾置位，被
//!    star 的失败提前返回跳过）；DB read 已变/star 未变、errors 非空、时间
//!    游标不前移、has_data_change=true；
//! ⑬ 反向锁：同一注入机制下，无任何已提交变化的**纯失败**不得虚标。
//!
//! R5（主控定点复核：binding 置位的窄路径）：
//! ⑭ 绑定已提交后若后续**字段版本读取**失败，已提交的绑定变化仍必须有标记
//!    （guard 内失败早退不得跳过置位）。R5 把置位进一步上移到
//!    `article_by_remote_id` 之前——该单行 SELECT 在现有夹具下无法确定性注入
//!    失败（见用例注释与 RESULT「R5」段），本用例不声称区分新旧位置，R5 位置
//!    由代码行序与注释锁定。
//!
//! 运行：cargo test --test pull_field_versions_e2e

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
    common::TempDbGuard,
) {
    let server = MockGReader::start().await.expect("start mock server");
    let tmp = common::unique_db_path(&format!("fieldver_{name}"));
    let _ = std::fs::remove_file(&tmp);
    let guard = common::TempDbGuard::new(tmp.clone());
    let conn = db::open(&tmp).expect("open db");
    db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
    db::set_setting(&conn, "greader_username", "test").unwrap();
    db::set_setting(&conn, "greader_password", "test-token").unwrap();
    let http = app_lib::ingestion::build_client(10);
    (Arc::new(Mutex::new(conn)), http, server, guard)
}

async fn last_sync(db: &Arc<Mutex<rusqlite::Connection>>) -> i64 {
    let conn = db.lock().await;
    db::last_sync_ts(&conn).unwrap()
}

async fn article_id_by_url(db: &Arc<Mutex<rusqlite::Connection>>, url: &str) -> Option<i64> {
    let conn = db.lock().await;
    conn.query_row(
        "SELECT id FROM articles WHERE url = ?1 ORDER BY id LIMIT 1",
        [url],
        |r| r.get(0),
    )
    .ok()
}

async fn status_of(db: &Arc<Mutex<rusqlite::Connection>>, aid: i64) -> (bool, bool) {
    let conn = db.lock().await;
    conn.query_row(
        "SELECT is_read, is_starred FROM articles WHERE id = ?1",
        [aid],
        |r| Ok((r.get::<_, i64>(0)? != 0, r.get::<_, i64>(1)? != 0)),
    )
    .unwrap()
}

/// (is_read, is_starred, read_version, star_version)——R2 既有行断言用。
async fn status_versions_of(
    db: &Arc<Mutex<rusqlite::Connection>>,
    aid: i64,
) -> (i64, i64, i64, i64) {
    let conn = db.lock().await;
    conn.query_row(
        "SELECT is_read, is_starred, read_version, star_version FROM articles WHERE id = ?1",
        [aid],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )
    .unwrap()
}

/// 标题（R3 实际数据前后核对用）。
async fn title_of(db: &Arc<Mutex<rusqlite::Connection>>, aid: i64) -> String {
    let conn = db.lock().await;
    conn.query_row("SELECT title FROM articles WHERE id = ?1", [aid], |r| {
        r.get(0)
    })
    .unwrap()
}

/// 正文 HTML（R3 实际数据前后核对用；NULL 归一为 ""）。
async fn content_html_of(db: &Arc<Mutex<rusqlite::Connection>>, aid: i64) -> String {
    let conn = db.lock().await;
    conn.query_row(
        "SELECT COALESCE(content_html, '') FROM articles WHERE id = ?1",
        [aid],
        |r| r.get(0),
    )
    .unwrap()
}

/// remote_id 绑定值（R3 绑定变化核对用）。
async fn remote_id_of(db: &Arc<Mutex<rusqlite::Connection>>, aid: i64) -> Option<i64> {
    let conn = db.lock().await;
    conn.query_row("SELECT remote_id FROM articles WHERE id = ?1", [aid], |r| {
        r.get(0)
    })
    .unwrap()
}

/// 造一篇直连来源文章（R2 在途直连并发场景的写入形态），返回 (aid, inserted)。
fn insert_direct_article(
    conn: &rusqlite::Connection,
    feed_id: i64,
    guid: &str,
    url: &str,
) -> (i64, bool) {
    let article = db::NewArticle {
        guid: guid.into(),
        url: Some(url.into()),
        title: "local direct article".into(),
        author: None,
        summary: None,
        content_html: Some("<p>local</p>".into()),
        body_text: "local".into(),
        image_url: None,
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: None,
        source: "direct".into(),
    };
    db::upsert_article_with_feed(conn, feed_id, &article, false).expect("直连入库")
}

/// 模拟「本地变更已推送成功、远端按操作确认」：把当前队列整批 prune 清空。
/// （版本不随清队回退——这正是后续旧快照必须被版本守卫挡住的原因。）
async fn simulate_push_confirmed(db: &Arc<Mutex<rusqlite::Connection>>) {
    let conn = db.lock().await;
    let queued = db::take_sync_queue(&conn).unwrap();
    let ids: Vec<i64> = queued.iter().map(|q| q.id).collect();
    db::prune_sync(&conn, &ids).unwrap();
    assert!(
        db::take_sync_queue(&conn).unwrap().is_empty(),
        "前置：模拟推送确认后队列必须已清空"
    );
}

/// 直接改写 mock 的服务端条目（仅测试夹具用）：按 changed_at 契约第 4 条
/// **显式**拨回旧时间——构造「远端早已存在的状态变更」（不在本轮增量窗口里，
/// 只能靠 reconcile 的权威集合看到）。
fn mutate_server_entry(
    server: &MockGReader,
    eid: i64,
    read: Option<bool>,
    starred: Option<bool>,
    changed_at: Option<i64>,
) {
    let mut entries = server.entries.lock().unwrap();
    let e = entries
        .iter_mut()
        .find(|e| e.id == eid)
        .expect("mock 条目存在");
    if let Some(v) = read {
        e.read = v;
    }
    if let Some(v) = starred {
        e.starred = v;
    }
    if let Some(v) = changed_at {
        e.changed_at = v;
    }
}

/// R4 故障注入：远端状态合并的 **star 写入**（`is_starred` 0→1 的值条件
/// UPDATE，即生产语句 `sync_mark_starred_if_unstarred_at_version`）触发 ABORT；
/// read 写入是另一条独立 UPDATE，不受影响。GR 逐语句提交（无页事务）且
/// read 先于 star 写（`merge_remote_status` 与 `reconcile_reader_state` 同序），
/// 用于验证「已提交的真实变化在后续同函数失败时仍有标记信号」。
/// **不是 mock report 常量**：注入点是真实 SQL 触发器，错误与标记都由生产
/// 路径自然产生。
fn install_star_update_failure(conn: &rusqlite::Connection) {
    conn.execute_batch(
        "CREATE TRIGGER r4_fail_star_on_rise BEFORE UPDATE ON articles
         WHEN NEW.is_starred = 1 AND OLD.is_starred = 0
         BEGIN SELECT RAISE(ABORT, 'injected star update failure'); END;",
    )
    .unwrap();
}

/// 负例①：请求发出 → localstar → 推送确认清队 → 旧远端 unstar 快照返回。
/// 旧快照的 starred 集合（空）在响应在途期间算定；本地收藏发生在该请求
/// 发出之后，随后模拟推送确认（队列清空）。只有「字段版本持久保留」能保护
/// 新 star——只看当前 pending 的实现会在此取消收藏。
#[tokio::test]
async fn stale_unstar_snapshot_cannot_rollback_pushed_local_star() {
    let (db, http, server, _guard) = setup("stale_unstar").await;
    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase binds feed/10");

    // 远端事实：A 未读未收藏，changed_at 旧（不在增量窗口）。
    let eid = server.add_entry_ret(10, "https://example.com/stale-a", "A", false, false);
    // 首轮 full 把 A 合并入库（此时本地未收藏）
    sync::states_phase(&db, &http, true)
        .await
        .expect("首轮 full 同步");
    let aid = article_id_by_url(&db, "https://example.com/stale-a")
        .await
        .expect("A 已入库");
    assert_eq!(
        status_of(&db, aid).await,
        (false, false),
        "初态：未读未收藏"
    );

    // 把 changed_at 拨回过去：下一轮增量列举不再包含它，状态只能在 reconcile 中见
    mutate_server_entry(&server, eid, None, None, Some(1_700_000_000));

    // 屏障：starred 权威集合的响应在途挂起
    let barrier = server.arm_stream_ids_barrier("com.google/starred");
    let db2 = db.clone();
    let http2 = http.clone();
    let task = tokio::spawn(async move { sync::states_phase(&db2, &http2, false).await });
    barrier.arrived.await.expect("屏障已停靠");

    // 响应在途期间：本地收藏 + 模拟推送成功清队（版本保留）
    {
        let conn = db.lock().await;
        db::set_starred_with_enqueue(&conn, aid, true).unwrap();
    }
    simulate_push_confirmed(&db).await;
    assert_eq!(status_of(&db, aid).await, (false, true), "本地收藏已落地");

    barrier.release.send(()).expect("放行响应");
    let report = task
        .await
        .expect("pull 任务不 panic")
        .expect("states phase ok");

    assert_eq!(
        status_of(&db, aid).await,
        (false, true),
        "旧远端 unstar 快照（响应在途期间算定、队列已清）不得回滚新 star——\
         判别：只看当前 pending 的实现会在此取消收藏；错误: {:?}",
        report.errors
    );
}

/// 负例①的「最早 HTTP 窗口」变体（任务卡「版号快照在HTTP前抓」的直接判别）：
/// 本地变更是发生在本次 pull 的**最早 HTTP（reading-list id 列举）在途期间**，
/// 而不是后续权威集合在途期间。旧实现把版本快照放在 id 列举之后构建——该窗口
/// 内的本地写入会被晚抓的快照洗成「起点版本」（其后哪怕推送成功、队列已清也
/// 无从拦截），权威集合携带的旧 unstar 快照即可取消用户刚确认的收藏。修后快照
/// 先于任何 HTTP，旧快照按版本不符被守卫拒绝。
/// 判别力：把快照构建挪回 id 列举之后的实现，本用例在最终断言处红。
#[tokio::test]
async fn snapshot_before_any_http_protects_write_during_id_listing() {
    let (db, http, server, _guard) = setup("snapshot_http_order").await;
    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase binds feed/10");

    // 远端事实：A 未读、已收藏（首轮 full 后本地 star=true，版本 0）。
    let eid = server.add_entry_ret(10, "https://example.com/order-a", "A", false, true);
    sync::states_phase(&db, &http, true)
        .await
        .expect("首轮 full 同步");
    let aid = article_id_by_url(&db, "https://example.com/order-a")
        .await
        .expect("A 已入库");
    assert_eq!(status_of(&db, aid).await, (false, true), "初态：远端已收藏");

    // 远端取消收藏，changed_at 拨回旧时间：不在下一轮增量窗口里，状态只能经
    // reconcile 的 starred 权威集合（不含 A）看到——「旧远端 unstar 快照」构造。
    mutate_server_entry(&server, eid, None, Some(false), Some(1_700_000_000));

    // 屏障：**reading-list id 列举**（本轮 pull 的最早 HTTP）响应在途挂起。
    let barrier = server.arm_stream_ids_barrier("reading-list");
    let db2 = db.clone();
    let http2 = http.clone();
    let task = tokio::spawn(async move { sync::states_phase(&db2, &http2, false).await });
    barrier.arrived.await.expect("屏障已停靠");

    // 响应在途期间：本地再次确认收藏（star_version 0→1）+ 模拟推送成功清队。
    {
        let conn = db.lock().await;
        db::set_starred_with_enqueue(&conn, aid, true).unwrap();
    }
    simulate_push_confirmed(&db).await;
    assert_eq!(status_of(&db, aid).await, (false, true), "本地收藏已确认");

    barrier.release.send(()).expect("放行响应");
    let report = task
        .await
        .expect("pull 任务不 panic")
        .expect("states phase ok");

    assert_eq!(
        status_of(&db, aid).await,
        (false, true),
        "id 列举在途期间的本地收藏（版本已变、队列已清）不得被旧 unstar 快照回滚——\
         判别：快照晚于 id 列举抓取的实现会在此取消收藏；错误: {:?}",
        report.errors
    );
}

/// 负例②：字段彼此独立。
/// 同一轮里两个条目：
/// - A：响应在途期间本地标读（pending_read 在，队列未清）——远端 star 命中
///   必须照常收藏（读取字段的 pending 不得连坐 star）；
/// - B：响应在途期间本地收藏（pending_star 在）——远端 read 命中必须照常
///   标读（star 的 pending 不得连坐 read）。
/// 旧的 aid 级 pending 并集实现会把两个条目整行跳过，两条断言都红。
#[tokio::test]
async fn local_change_in_one_field_does_not_block_the_other_field() {
    let (db, http, server, _guard) = setup("field_independent").await;
    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase binds feed/10");

    let eid_a = server.add_entry_ret(10, "https://example.com/ind-a", "A", false, false);
    let eid_b = server.add_entry_ret(10, "https://example.com/ind-b", "B", false, false);
    sync::states_phase(&db, &http, true)
        .await
        .expect("首轮 full 同步");
    let aid_a = article_id_by_url(&db, "https://example.com/ind-a")
        .await
        .expect("A 已入库");
    let aid_b = article_id_by_url(&db, "https://example.com/ind-b")
        .await
        .expect("B 已入库");

    // 远端「早已存在」的状态变更（changed_at 拨回，仅供 reconcile 权威集合可见）：
    // A 被远端收藏；B 被远端标读。
    mutate_server_entry(&server, eid_a, None, Some(true), Some(1_700_000_000));
    mutate_server_entry(&server, eid_b, Some(true), None, Some(1_700_000_000));

    let barrier = server.arm_stream_ids_barrier("com.google/starred");
    let db2 = db.clone();
    let http2 = http.clone();
    let task = tokio::spawn(async move { sync::states_phase(&db2, &http2, false).await });
    barrier.arrived.await.expect("屏障已停靠");

    // 期间：A 本地标读（pending_read）；B 本地收藏（pending_star）
    {
        let conn = db.lock().await;
        db::set_read_with_enqueue(&conn, aid_a, true).unwrap();
        db::set_starred_with_enqueue(&conn, aid_b, true).unwrap();
    }
    barrier.release.send(()).expect("放行响应");
    let report = task
        .await
        .expect("pull 任务不 panic")
        .expect("states phase ok");

    let (a_read, a_starred) = status_of(&db, aid_a).await;
    assert!(
        a_starred,
        "A 的读字段有本地意图，但 star 字段无冲突：远端收藏必须照常收敛\
         （aid 级 pending 并集会整行跳过）；错误: {:?}",
        report.errors
    );
    assert!(a_read, "A 本地标读保留（读字段被自身 pending 守卫保护）");
    let (b_read, b_starred) = status_of(&db, aid_b).await;
    assert!(
        b_read,
        "B 的收藏字段有本地意图，但 read 字段无冲突：远端已读必须照常收敛\
         （star 的 pending 不得连坐 read）；错误: {:?}",
        report.errors
    );
    assert!(
        b_starred,
        "B 本地收藏保留（star 字段被自身 pending 守卫保护）"
    );
}

/// 负例③：新条目入库后立即本地变更，同轮后续（第二正文块 + 权威集合对账）
/// 也受保护——版本快照在该条目入库时记录，本地写入把版本推过它。
#[tokio::test]
async fn local_change_after_new_insert_survives_rest_of_round() {
    let (db, http, server, _guard) = setup("insert_then_change").await;
    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase binds feed/10");

    // 150 条：reading-list 一页列举（<1000），正文分两块（100 + 50）。
    // 目标条目落在第一块。
    let target_url = "https://example.com/many/0";
    for i in 0..150 {
        server.add_entry_ret(
            10,
            &format!("https://example.com/many/{i}"),
            &format!("M{i}"),
            false,
            false,
        );
    }

    // 屏障：第二块正文的响应在途挂起（第一块已合并完成）
    let barrier = server.arm_contents_barrier(2);
    let db2 = db.clone();
    let http2 = http.clone();
    let task = tokio::spawn(async move { sync::states_phase(&db2, &http2, false).await });
    barrier.arrived.await.expect("屏障已停靠");

    // 目标条目已在第一块入库；立即本地收藏 + 模拟推送确认清队
    let aid = article_id_by_url(&db, target_url)
        .await
        .expect("目标条目已随第一块入库");
    {
        let conn = db.lock().await;
        db::set_starred_with_enqueue(&conn, aid, true).unwrap();
    }
    simulate_push_confirmed(&db).await;

    barrier.release.send(()).expect("放行响应");
    let report = task
        .await
        .expect("pull 任务不 panic")
        .expect("states phase ok");

    assert!(
        report.errors.is_empty(),
        "本轮应无错误（分块与对账全部完成）: {:?}",
        report.errors
    );
    assert!(
        status_of(&db, aid).await.1,
        "新入库后立即本地收藏：同轮后续（第二块 + 对账空 starred 集合）\
         不得回滚——版本快照在入库时记录，本地写入必须使其失效"
    );
    // 恢复完整窗口的旁证：150 条全部入库
    let count: i64 = {
        let conn = db.lock().await;
        conn.query_row(
            "SELECT COUNT(*) FROM articles WHERE url LIKE 'https://example.com/many/%'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(count, 150, "本轮两块正文都必须落库");
}

/// 负例④a：GR 匹配映射构建失败**不得空映射假成功**。
/// 注入真实 DB 列类型错误（url_norm 写 BLOB → sync_match_maps 读取失败）：
/// - 错误必须可见（report.errors 带「同步匹配映射构建失败」）；
/// - 游标不得推进（修前空映射会「合法跳过」全部合并且照常推进 —— 窗口被
///   静默跳过）；
/// - 修复列后重拉同一窗口：条目合并、游标推进（不永久卡死、不跳过）。
#[tokio::test]
async fn gr_maps_build_failure_keeps_cursor_and_repull_merges() {
    let (db, http, server, _guard) = setup("maps_fail").await;
    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase binds feed/10");
    server.add_entry_ret(10, "https://example.com/maps-e", "E", false, false);

    let t0: i64 = 1_700_000_000;
    {
        let conn = db.lock().await;
        db::set_last_sync_ts(&conn, t0).unwrap();
        // 真实列类型错误：url_norm 写 BLOB（sync_match_maps 以 String 读取）
        let feed_id: i64 = conn
            .query_row("SELECT id FROM feeds LIMIT 1", [], |r| r.get(0))
            .unwrap();
        conn.execute(
            "INSERT INTO articles (feed_id, guid, title, url, url_norm)
             VALUES (?1, 'blob-row', 't', 'https://example.com/blob', X'00FF')",
            [feed_id],
        )
        .unwrap();
    }

    let report = sync::states_phase(&db, &http, false)
        .await
        .expect("states phase ok（错误经 report 呈现）");
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("同步匹配映射构建失败")),
        "映射构建失败必须可见: {:?}",
        report.errors
    );
    assert_eq!(
        last_sync(&db).await,
        t0,
        "映射构建失败时游标不得推进（修前空映射假成功会推进并跳过整个窗口）"
    );
    assert!(
        article_id_by_url(&db, "https://example.com/maps-e")
            .await
            .is_none(),
        "失败轮不得谎称已合并"
    );

    // 修复列（真实值），重拉同一窗口
    {
        let conn = db.lock().await;
        conn.execute(
            "UPDATE articles SET url_norm = 'https://example.com/blob' WHERE guid = 'blob-row'",
            [],
        )
        .unwrap();
    }
    let report2 = sync::states_phase(&db, &http, false)
        .await
        .expect("states phase ok");
    assert!(
        !report2.errors.iter().any(|e| e.contains("映射")),
        "修复后不应再有映射错误: {:?}",
        report2.errors
    );
    assert!(
        last_sync(&db).await > t0,
        "修复后重拉成功、游标正常推进（守卫不永久卡死）"
    );
    let (read, starred) = {
        let aid = article_id_by_url(&db, "https://example.com/maps-e")
            .await
            .expect("重拉必须把失败窗口的条目合并回来");
        status_of(&db, aid).await
    };
    assert_eq!((read, starred), (false, false), "条目状态按远端落地");
}

/// 负例④b：GR 批合并失败后 maps 按 DB 重建、游标保留——重拉不跳过失败条目，
/// 已成功条目不重复。
/// 注入手法：articles 的 BEFORE INSERT 触发器让第二条目的新入库失败（纯 SQL
/// 注入，不打桩）；失败块中止后：错误可见、游标不推进、已合并条目保留；
/// 解除注入重拉：失败条目补回且不重复。
#[tokio::test]
async fn gr_chunk_merge_failure_rebuilds_maps_repull_merges_failed_entry() {
    let (db, http, server, _guard) = setup("merge_fail").await;
    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase binds feed/10");
    server.add_entry_ret(10, "https://example.com/fb-1", "F1", false, false);
    let eid2 = server.add_entry_ret(10, "https://example.com/fb-2", "F2", false, false);

    let t0: i64 = 1_700_000_000;
    {
        let conn = db.lock().await;
        db::set_last_sync_ts(&conn, t0).unwrap();
        // 第二条目（guid 形如 remote-<eid>）的新入库触发失败——块内中止点
        conn.execute_batch(&format!(
            "CREATE TRIGGER inject_fail_second BEFORE INSERT ON articles
             WHEN NEW.guid = 'remote-{eid2}'
             BEGIN SELECT RAISE(ABORT, 'injected merge failure'); END"
        ))
        .unwrap();
    }

    let report = sync::states_phase(&db, &http, false)
        .await
        .expect("states phase ok（块失败经 report 呈现）");
    assert!(
        report.errors.iter().any(|e| e.contains("合并条目")),
        "批合并失败必须可见: {:?}",
        report.errors
    );
    assert_eq!(
        last_sync(&db).await,
        t0,
        "块失败时游标不得推进（失败窗口下轮重拉）"
    );
    assert!(
        article_id_by_url(&db, "https://example.com/fb-1")
            .await
            .is_some(),
        "失败块中先于失败点已合并的条目保留（GR 无页事务，逐语句提交）"
    );
    assert!(
        article_id_by_url(&db, "https://example.com/fb-2")
            .await
            .is_none(),
        "失败点条目本轮未落库"
    );

    // 解除注入，重拉同一窗口：失败条目补回、已成功条目零重复
    {
        let conn = db.lock().await;
        conn.execute_batch("DROP TRIGGER inject_fail_second")
            .unwrap();
    }
    let report2 = sync::states_phase(&db, &http, false)
        .await
        .expect("states phase ok");
    assert!(
        report2.errors.is_empty(),
        "解除注入后不应再有错误: {:?}",
        report2.errors
    );
    assert!(last_sync(&db).await > t0, "重拉成功、游标推进");
    let count1: i64 = {
        let conn = db.lock().await;
        conn.query_row(
            "SELECT COUNT(*) FROM articles WHERE url = 'https://example.com/fb-1'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(count1, 1, "已成功条目不重复（重拉幂等）");
    assert!(
        article_id_by_url(&db, "https://example.com/fb-2")
            .await
            .is_some(),
        "失败条目必须被重拉合并回来（maps 不污染 → 不跳过）"
    );
}

/* ============================================================
R2（主控 2026-10-10 反例 + 防过度保守反向锁，见文件头 ⑤⑥⑦）
============================================================ */

/// ⑤ 主控确定性反例并入：reading-list id 列举（pull 最早 HTTP）在途期间，直连
/// 抓取插入**同 feed 同 URL** 的行并本地标读/收藏，随后模拟推送确认清队。该行
/// 不在 pull 起点快照映射里：正文合并时 URL / remote_id 两个查找都未命中快照 →
/// upsert 命中既有行（inserted=false）。修前实现丢弃 inserted 标志、无条件整值
/// `sync_set_article_status`，成功 pull 后状态由 (1,1,1,1) 回滚为 (0,0,1,1)。
/// 修后既有行逐字段过版本/pending 守卫：两字段都被「快照 0 != 当前 1」拒绝，
/// 状态与版本原样保留。
/// 判别力：恢复「丢弃 inserted + 整值写状态」的实现，本用例最终断言红。
#[tokio::test]
async fn stale_snapshot_upsert_of_concurrent_direct_row_preserves_local_intent() {
    let (db, http, server, _guard) = setup("upsert_existing").await;
    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase binds feed/10");

    // 远端快照：该条目未读未收藏（旧快照随正文响应返回）。
    let url = "https://example.com/upsert-existing";
    server.add_entry_ret(10, url, "remote snapshot", false, false);

    // 屏障：reading-list id 列举在途——快照映射已建好，直连行尚未存在。
    let barrier = server.arm_stream_ids_barrier("reading-list");
    let db2 = db.clone();
    let http2 = http.clone();
    let task = tokio::spawn(async move { sync::states_phase(&db2, &http2, true).await });
    barrier.arrived.await.expect("屏障已停靠");

    // 在途期间：直连入库同 feed 同 URL 行 + 标读 + 收藏 + 模拟推送确认清队。
    let aid = {
        let conn = db.lock().await;
        let local_feed = db::sync_match_maps(&conn).unwrap().feed_mf_to_id[&10];
        let (aid, inserted) =
            insert_direct_article(&conn, local_feed, "direct-guid-upsert-existing", url);
        assert!(
            inserted,
            "前置：该行此刻必须是新插入（不在 pull 起点快照里）"
        );
        db::set_read_with_enqueue(&conn, aid, true).unwrap();
        db::set_starred_with_enqueue(&conn, aid, true).unwrap();
        aid
    };
    simulate_push_confirmed(&db).await;
    assert_eq!(
        status_versions_of(&db, aid).await,
        (1, 1, 1, 1),
        "前置：本地标读/收藏已落地、推送已确认（队列已清）"
    );

    barrier.release.send(()).expect("放行响应");
    let report = task
        .await
        .expect("pull 任务不 panic")
        .expect("states phase ok");

    assert!(
        report.errors.is_empty(),
        "本轮必须是成功 pull（旧快照正常返回、无错误）: {:?}",
        report.errors
    );
    assert_eq!(
        status_versions_of(&db, aid).await,
        (1, 1, 1, 1),
        "既有行不得被旧远端 unread/unstar 快照整值覆写：状态与版本原样保留——\
         判别：丢弃 upsert inserted 标志 + 无条件 sync_set_article_status 的实现\
         会在此把状态回滚为 (0,0,1,1)"
    );
}

/// ⑥ 既有行单字段冲突不压住另一字段：同 ⑤ 的在途直连行，本地只标读
/// （read_version 0→1，推送已确认），远端快照 read=false、star=true。
/// upsert 既有行分支必须逐字段守卫：read 被版本守卫拒绝（本地标读保留）；
/// star 无冲突且版本未变（0==0）照常收敛为远端 true——不能因读字段冲突整条
/// 跳过，也不能整值覆写（修前整值写会把 read 翻回 false）。
/// 判别力：把既有行分支改成「任一字段冲突即整条跳过」或恢复整值写，本用例红。
#[tokio::test]
async fn existing_row_single_field_conflict_does_not_block_other_field() {
    let (db, http, server, _guard) = setup("upsert_field_indep").await;
    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase binds feed/10");

    let url = "https://example.com/upsert-field-indep";
    server.add_entry_ret(10, url, "remote snapshot", false, true);

    let barrier = server.arm_stream_ids_barrier("reading-list");
    let db2 = db.clone();
    let http2 = http.clone();
    let task = tokio::spawn(async move { sync::states_phase(&db2, &http2, true).await });
    barrier.arrived.await.expect("屏障已停靠");

    let aid = {
        let conn = db.lock().await;
        let local_feed = db::sync_match_maps(&conn).unwrap().feed_mf_to_id[&10];
        let (aid, inserted) =
            insert_direct_article(&conn, local_feed, "direct-guid-upsert-field-indep", url);
        assert!(
            inserted,
            "前置：该行此刻必须是新插入（不在 pull 起点快照里）"
        );
        db::set_read_with_enqueue(&conn, aid, true).unwrap();
        aid
    };
    simulate_push_confirmed(&db).await;
    assert_eq!(
        status_versions_of(&db, aid).await,
        (1, 0, 1, 0),
        "前置：仅读字段有本地写入（版本 1）、推送已确认"
    );

    barrier.release.send(()).expect("放行响应");
    let report = task
        .await
        .expect("pull 任务不 panic")
        .expect("states phase ok");

    assert!(
        report.errors.is_empty(),
        "本轮应无错误: {:?}",
        report.errors
    );
    assert_eq!(
        status_versions_of(&db, aid).await,
        (1, 1, 1, 0),
        "读字段冲突（版本 0→1）只保护读；star 字段无冲突必须照常收敛为远端 true\
         （远端合并不递增 star_version；整条跳过或整值覆写的实现会在此红）"
    );
}

/// ⑦ 既有非零版本本轮未改仍可接收远端（防「守卫过度保守」的反向锁）。
/// 首轮 full 同步合并条目后，本地对读字段做过一次「碰过并确认」的写入
/// （值未变、read_version=1、推送已确认清队）。次轮 full 的 pull 起点快照
/// read_version=1 == 当前 1 且无 pending：远端 read=true 必须被正常接受——
/// 若守卫把「版本非零」当成本地意图，或实现把快照重新定义为 0 / 现读版本，
/// 本用例红；远端合并不递增版本（read_version 保持 1）。
/// 判别力：把既有行分支改成「版本非零一律拒绝」或快照覆盖为 0，最终断言红。
#[tokio::test]
async fn nonzero_version_untouched_this_round_still_accepts_remote() {
    let (db, http, server, _guard) = setup("nonzero_accept").await;
    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase binds feed/10");

    let url = "https://example.com/nonzero-accept";
    let eid = server.add_entry_ret(10, url, "N", false, false);
    sync::states_phase(&db, &http, true)
        .await
        .expect("首轮 full 同步");
    let aid = article_id_by_url(&db, url).await.expect("条目已入库");

    // 本地对读字段的「碰过并确认」写入（值未变，版本 0→1）+ 模拟推送确认清队。
    {
        let conn = db.lock().await;
        db::set_read_with_enqueue(&conn, aid, false).unwrap();
    }
    simulate_push_confirmed(&db).await;
    assert_eq!(
        status_versions_of(&db, aid).await,
        (0, 0, 1, 0),
        "前置：read_version=1、无 pending"
    );

    // 远端随后在别处被标读：本轮 pull 携带 read=true。
    mutate_server_entry(&server, eid, Some(true), None, None);

    let report = sync::states_phase(&db, &http, true)
        .await
        .expect("次轮 full 同步 ok");
    assert!(
        report.errors.is_empty(),
        "本轮应无错误: {:?}",
        report.errors
    );
    assert_eq!(
        status_versions_of(&db, aid).await,
        (1, 0, 1, 0),
        "快照 1 == 当前 1 且无 pending：版本非零不得拒绝远端 read=true（防过度保守）；\
         远端合并不递增版本（read_version 保持 1）"
    );
}

/* ============================================================
R3（主控 2026-10-10 二轮 P2：F11 实际变化标记，见文件头 ⑧⑨⑩⑪）
============================================================ */

/// ⑧ 主控二轮反例并入（真实 sync_light + mock 屏障）：reading-list 在途期间
/// 直连插入同 feed 同 URL 行并标读/收藏、推送确认清队；放行后正文合并 upsert
/// 命中既有行——R2 已锁状态/版本保持 (1,1,1,1)，但**标题被真实改写、remote_id
/// 完成绑定**。修前：pulled_entries/merged_states 全 0 → has_data_change=false
/// （内容变了却不发数据事件）；修后：entries_data_changed 必须为 true，且该
/// 场景旧计数保持 0（计数与真实变化判定分离）。
/// 判别力：不接标记（恢复计数驱动）→ 最终两个断言红；数据断言（title/绑定/
/// 状态）保证不是「只查 flag 常量」。
#[tokio::test]
async fn concurrent_direct_row_content_change_still_reports_data_change() {
    let (db, http, server, _guard) = setup("r3_concurrent_content").await;
    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase binds feed/10");

    let url = "https://example.com/r3-concurrent-content";
    let eid = server.add_entry_ret(10, url, "remote snapshot", false, false);

    let barrier = server.arm_stream_ids_barrier("reading-list");
    let db2 = db.clone();
    let http2 = http.clone();
    let task = tokio::spawn(async move { sync::sync_light(&db2, &http2).await });
    barrier.arrived.await.expect("屏障已停靠");

    // 在途期间：直连入库同 feed 同 URL 行 + 标读 + 收藏 + 模拟推送确认清队。
    let aid = {
        let conn = db.lock().await;
        let local_feed = db::sync_match_maps(&conn).unwrap().feed_mf_to_id[&10];
        let (aid, inserted) =
            insert_direct_article(&conn, local_feed, "direct-guid-r3-content", url);
        assert!(inserted, "前置：该行此刻必须是新插入（不在 pull 快照里）");
        db::set_read_with_enqueue(&conn, aid, true).unwrap();
        db::set_starred_with_enqueue(&conn, aid, true).unwrap();
        aid
    };
    simulate_push_confirmed(&db).await;
    assert_eq!(
        status_versions_of(&db, aid).await,
        (1, 1, 1, 1),
        "前置：本地标读/收藏已落地、推送已确认（队列已清）"
    );
    assert_eq!(
        title_of(&db, aid).await,
        "local direct article",
        "前置：本地标题"
    );
    assert_eq!(remote_id_of(&db, aid).await, None, "前置：直连行未绑定");

    barrier.release.send(()).expect("放行响应");
    let report = task
        .await
        .expect("pull 任务不 panic")
        .expect("states phase ok");

    assert!(
        report.errors.is_empty(),
        "本轮必须是成功 pull（旧快照正常返回、无错误）: {:?}",
        report.errors
    );
    // 实际数据核对（先证明确实变了/确实被保护）
    assert_eq!(
        status_versions_of(&db, aid).await,
        (1, 1, 1, 1),
        "R2 保护保持：双字段状态与版本不得被旧快照回滚"
    );
    assert_eq!(
        title_of(&db, aid).await,
        "remote snapshot",
        "upsert 确实改写了可见内容（标题）"
    );
    assert_eq!(
        remote_id_of(&db, aid).await,
        Some(eid),
        "合并完成后 remote_id 已绑定（绑定变化）"
    );
    // 标记与旧计数的分离
    assert_eq!(
        (report.pulled_entries, report.merged_states),
        (0, 0),
        "该场景守卫全部拒绝状态写：旧计数保持 0（统计兼容，不代表无变化）"
    );
    assert!(
        report.entries_data_changed,
        "正文/绑定真实改变必须置数据变化标记（修前此处为 false）"
    );
    assert!(
        report.has_data_change(),
        "内容真实变化必须发数据变化事件（R3 P2 反例本体）"
    );
}

/// ⑨ 旧已绑定行（own）的实际回填同理：首轮全量合并空正文条目，本地标读/收藏
/// 并推送确认（版本 1,1）；次轮远端补上正文且 read/star=true——状态字段写全部
/// 是值条件 0 行（无变化），但 backfill **真实把空正文补进** → 标记必须为 true。
/// 修前：backfill 不回传变化、pulled 计数（此处 1）不经真实判定 → 事件判定
/// 只可能依赖旧计数；本用例锁定「own 行回填」也被标记覆盖，且现场正文确实被补。
#[tokio::test]
async fn own_bound_row_backfill_still_reports_data_change() {
    let (db, http, server, _guard) = setup("r3_own_backfill").await;
    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase binds feed/10");

    let url = "https://example.com/r3-own-backfill";
    let eid = server.add_entry_full(10, url, "E", false, false, "", Vec::new());
    sync::states_phase(&db, &http, true)
        .await
        .expect("首轮 full 同步");
    let aid = article_id_by_url(&db, url).await.expect("条目已入库");
    assert_eq!(
        content_html_of(&db, aid).await,
        "",
        "前置：首轮合并后正文为空"
    );

    // 本地标读 + 收藏并模拟推送确认（版本 1,1）——状态字段不再是可回写状态。
    {
        let conn = db.lock().await;
        db::set_read_with_enqueue(&conn, aid, true).unwrap();
        db::set_starred_with_enqueue(&conn, aid, true).unwrap();
    }
    simulate_push_confirmed(&db).await;
    assert_eq!(
        status_versions_of(&db, aid).await,
        (1, 1, 1, 1),
        "前置：本地标读/收藏已确认"
    );

    // 远端后补正文（changed_at 前移到增量窗口），read/star 与本地一致。
    let now = chrono::Utc::now().timestamp();
    {
        let mut entries = server.entries.lock().unwrap();
        let e = entries
            .iter_mut()
            .find(|e| e.id == eid)
            .expect("mock 条目存在");
        e.content = "<p>late content</p>".into();
        e.read = true;
        e.starred = true;
        e.changed_at = now;
    }

    let report = sync::states_phase(&db, &http, false)
        .await
        .expect("次轮增量同步 ok");
    assert!(
        report.errors.is_empty(),
        "本轮应无错误: {:?}",
        report.errors
    );
    // 实际数据核对：正文确实被补、状态保持不变。
    assert_eq!(
        content_html_of(&db, aid).await,
        "<p>late content</p>",
        "own 行空正文必须被真实回填"
    );
    assert_eq!(
        status_versions_of(&db, aid).await,
        (1, 1, 1, 1),
        "状态字段值条件写 0 行：读/藏与版本原样"
    );
    assert!(
        report.entries_data_changed,
        "实际回填必须置数据变化标记（修前漏标——旧已绑定行 backfill 同根）"
    );
    assert!(report.has_data_change(), "回填是用户可见变化，必须发事件");
}

/// ⑩ 真正新行：新插入必须置数据变化标记（并且旧计数兼容地 +1）。
#[tokio::test]
async fn new_row_pull_sets_data_change() {
    let (db, http, server, _guard) = setup("r3_new_row").await;
    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase binds feed/10");

    let url = "https://example.com/r3-new-row";
    server.add_entry_ret(10, url, "N", true, true);

    let report = sync::states_phase(&db, &http, false)
        .await
        .expect("增量同步 ok");
    assert!(
        report.errors.is_empty(),
        "本轮应无错误: {:?}",
        report.errors
    );
    let aid = article_id_by_url(&db, url).await.expect("新条目已入库");
    assert_eq!(
        status_of(&db, aid).await,
        (true, true),
        "远端 read/star=true 必须落到新行"
    );
    assert!(report.pulled_entries >= 1, "旧计数兼容：新行仍计 pulled");
    assert!(report.entries_data_changed, "新插入必须置数据变化标记");
    assert!(report.has_data_change(), "新文章必须发数据事件");
}

/// ⑪ 同值 replay：only 前移 changed_at（字段全同）让条目留在增量窗口——
/// 守卫通过、旧计数照常累计（pulled_entries ≥ 1），但**实际数据零变化**：
/// 不得置标记、不得发事件。锁定「计数 ≠ 语义变化」的分离（R3 核心口径）。
#[tokio::test]
async fn same_value_replay_counts_no_data_change() {
    let (db, http, server, _guard) = setup("r3_same_value").await;
    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase binds feed/10");

    let url = "https://example.com/r3-same-value";
    let eid = server.add_entry_ret(10, url, "S", true, true);
    let first = sync::states_phase(&db, &http, false)
        .await
        .expect("首轮增量 ok");
    assert!(first.entries_data_changed, "前置：首轮新行确有变化");
    let aid = article_id_by_url(&db, url).await.expect("条目已入库");
    let before = (
        title_of(&db, aid).await,
        status_versions_of(&db, aid).await,
        content_html_of(&db, aid).await,
    );

    // 仅前移 changed_at（模拟服务端 touch，不改任何字段值）→ 仍进增量窗口。
    let now = chrono::Utc::now().timestamp();
    mutate_server_entry(&server, eid, None, None, Some(now));

    let report = sync::states_phase(&db, &http, false)
        .await
        .expect("次轮增量 ok");
    assert!(
        report.errors.is_empty(),
        "本轮应无错误: {:?}",
        report.errors
    );
    let after = (
        title_of(&db, aid).await,
        status_versions_of(&db, aid).await,
        content_html_of(&db, aid).await,
    );
    assert_eq!(before, after, "同值 replay 后现场数据必须逐项不变");
    assert!(
        report.pulled_entries >= 1,
        "守卫通过仍按旧口径计入 pulled（统计兼容）"
    );
    assert!(
        !report.entries_data_changed,
        "字段全同：不得置数据变化标记（同值 UPDATE 的 changes() 不算变化）"
    );
    assert!(
        !report.has_data_change(),
        "零实际变化不得发数据事件（不制造无谓 reload）"
    );
}

/* ============================================================
R4（主控 R3 初审复核：GR 逐语句提交下的标记时序，见文件头 ⑫⑬）
============================================================ */

/// ⑫ 主控/初审确认的反例：GR 逐语句提交、无页事务——已绑定条目远端 read/star
/// 同时变更（changed_at 前移使其走正文合并路径），read 写先提交、star 写被真
/// DB 触发器 ABORT。修前 `merge_remote_status` 把标记攒到函数尾：star 的 `?`
/// 提前返回导致**已提交的 read 变化没有信号**（has_data_change=false）。修后
/// 每个成功变更点即时置位。
/// 注入机制声明：触发器挂在生产语句 `sync_mark_starred_if_unstarred_at_version`
/// 命中的 `is_starred 0→1` UPDATE 上——不是 mock report 常量，错误/标记都由
/// 生产路径自然产生。
/// 判别力：把标记改回「函数尾累计」的实现，本用例红在 has_data_change 断言。
#[tokio::test]
async fn gr_committed_read_change_still_marks_after_star_write_failure() {
    let (db, http, server, _guard) = setup("r4_star_fail_read_committed").await;
    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase binds feed/10");

    let url = "https://example.com/r4-read-committed";
    let eid = server.add_entry_ret(10, url, "E", false, false);
    sync::states_phase(&db, &http, true)
        .await
        .expect("首轮 full 同步");
    let aid = article_id_by_url(&db, url).await.expect("条目已入库");
    assert_eq!(
        status_versions_of(&db, aid).await,
        (0, 0, 0, 0),
        "前置：未读未收藏、版本 0"
    );
    let t0 = last_sync(&db).await;

    // 远端 read/star 同时变 true（changed_at 前移 → 本轮增量窗口内，走正文合并）。
    mutate_server_entry(
        &server,
        eid,
        Some(true),
        Some(true),
        Some(chrono::Utc::now().timestamp()),
    );
    // 注入：star 的值条件写 ABORT（read 写不受影响）。
    {
        let conn = db.lock().await;
        install_star_update_failure(&conn);
    }

    let report = sync::states_phase(&db, &http, false)
        .await
        .expect("增量同步 ok（错误经 report 呈现）");
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("injected star update failure")),
        "star 写失败必须可见: {:?}",
        report.errors
    );
    // 已提交/未提交的现场逐项核对
    assert_eq!(
        status_versions_of(&db, aid).await,
        (1, 0, 0, 0),
        "read 写已提交（is_read=1）、star 写失败未落地（is_starred=0）；远端合并不递增版本"
    );
    assert_eq!(
        last_sync(&db).await,
        t0,
        "分块失败时时间游标不得前移（窗口下轮重拉）"
    );
    assert!(
        report.entries_data_changed,
        "已提交的 read 变化必须立即留有标记（修前攒尾置位被 star 失败跳过 → false）"
    );
    assert!(report.has_data_change(), "实际变化已提交：必须发数据事件");
}

/// ⑬ 反向锁：**没有先成功变更的纯失败不得虚标**。同一注入机制下，条目远端只
/// 发生 star 变更（read 保持未读——生产 read 分支写的是「unread」值条件
/// UPDATE，行值未变、命中 0 行）：star 写 ABORT、无任何已提交变化 → 标记与
/// has_data_change 必须 false，DB 现场不动，游标同样不前移。
#[tokio::test]
async fn gr_pure_star_failure_without_committed_change_does_not_mark() {
    let (db, http, server, _guard) = setup("r4_pure_star_fail").await;
    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase binds feed/10");

    let url = "https://example.com/r4-pure-star-fail";
    let eid = server.add_entry_ret(10, url, "G", false, false);
    sync::states_phase(&db, &http, true)
        .await
        .expect("首轮 full 同步");
    let aid = article_id_by_url(&db, url).await.expect("条目已入库");
    let t0 = last_sync(&db).await;

    // 远端只改 star（read 不变；changed_at 前移 → 走正文合并路径）。
    mutate_server_entry(
        &server,
        eid,
        None,
        Some(true),
        Some(chrono::Utc::now().timestamp()),
    );
    {
        let conn = db.lock().await;
        install_star_update_failure(&conn);
    }

    let report = sync::states_phase(&db, &http, false)
        .await
        .expect("增量同步 ok（错误经 report 呈现）");
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("injected star update failure")),
        "star 写失败必须可见: {:?}",
        report.errors
    );
    assert_eq!(
        status_versions_of(&db, aid).await,
        (0, 0, 0, 0),
        "无已提交变化：read 分支的「unread」值条件写命中 0 行、star 写失败未落地"
    );
    assert_eq!(last_sync(&db).await, t0, "分块失败时时间游标不得前移");
    assert!(
        !report.entries_data_changed,
        "纯失败不得置标记（失败本身不是数据变化）"
    );
    assert!(!report.has_data_change(), "零实际变化不得发事件");
}

/// ⑭ 绑定标记与守卫读取失败的时序（R5）：绑定已提交后，若后续**字段版本读取**
/// （`field_accepts_remote → article_field_versions`）失败，已提交的绑定变化仍
/// 必须有标记。
///
/// 注入（无生产开关）：直连未绑定行（URL 与远端同）走 own 合并路径完成绑定；
/// `AFTER UPDATE OF remote_id` 触发器在该绑定 UPDATE 提交后把本行 read_version
/// 写成 BLOB → 随后生产路径的版本读取 `r.get::<i64>` 必然 Err。断言：绑定已
/// 落库（remote_id=eid）、read/star 未写、errors 非空、时间游标不前移、标记与
/// has_data_change=true。
///
/// 判别力与边界（不虚构外推）：标记若回到「函数尾累计」（R3 形态）本用例红
/// （guard 内失败早退，尾部置位被跳过）。R5 把置位从 R4 位置（guard 之前）
/// 再上移到 `article_by_remote_id` 之前——该单行 SELECT 在当前夹具下**无法
/// 确定性注入失败**（rusqlite query_row 对多行取首行、无值条件可破坏；确定性
/// 失败需 DB 级损坏或 SQLite authorizer，而 rusqlite `hooks` feature 本项目
/// 未启用且不可改 Cargo/加生产开关），故本用例不声称区分这两个位置；R5 位置
/// 由代码行序与注释锁定（见 RESULT「R5」段）。
#[tokio::test]
async fn gr_binding_change_marks_before_later_read_failure() {
    let (db, http, server, _guard) = setup("r5_binding_mark").await;
    sync::feeds_phase(&db, &http)
        .await
        .expect("feeds phase binds feed/10");

    // 直连行（未绑定，URL 与远端条目相同）——pull 快照 URL 命中，走 own 合并路径。
    let url = "https://example.com/r5-binding-mark";
    let aid = {
        let conn = db.lock().await;
        let local_feed = db::sync_match_maps(&conn).unwrap().feed_mf_to_id[&10];
        let (aid, inserted) = insert_direct_article(&conn, local_feed, "direct-guid-r5-bind", url);
        assert!(inserted, "前置：直连行新插入且未绑定");
        aid
    };
    let eid = server.add_entry_ret(10, url, "R5", false, false);

    // 注入：绑定 UPDATE 提交后把 read_version 写坏 → 后续版本读取必然 Err。
    {
        let conn = db.lock().await;
        db::set_last_sync_ts(&conn, 1_700_000_000).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER r5_corrupt_after_bind AFTER UPDATE OF remote_id ON articles
             WHEN NEW.remote_id IS NOT NULL
             BEGIN
                 UPDATE articles SET read_version = X'00' WHERE id = NEW.id;
             END;",
        )
        .unwrap();
    }
    let t0 = last_sync(&db).await;

    let report = sync::states_phase(&db, &http, false)
        .await
        .expect("同步 ok（错误经 report 呈现）");
    assert!(
        report.errors.iter().any(|e| e.contains("合并条目")),
        "绑定后的版本读取失败必须可见: {:?}",
        report.errors
    );
    // 已提交/未提交的现场逐项核对（read_version 已被注入写坏，不查它）。
    {
        let conn = db.lock().await;
        let (remote_id, is_read, is_starred): (Option<i64>, i64, i64) = conn
            .query_row(
                "SELECT remote_id, is_read, is_starred FROM articles WHERE id = ?1",
                [aid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(remote_id, Some(eid), "绑定 UPDATE 已提交（真实变化已落库）");
        assert_eq!(
            (is_read, is_starred),
            (0, 0),
            "守卫版本读取失败：状态字段未被写"
        );
    }
    assert_eq!(last_sync(&db).await, t0, "分块失败时时间游标不得前移");
    assert!(
        report.entries_data_changed,
        "已提交的绑定变化必须立即留有标记（旧位置在 guard 读之后，失败即跳过）"
    );
    assert!(report.has_data_change(), "已提交的绑定变化必须发数据事件");
}
