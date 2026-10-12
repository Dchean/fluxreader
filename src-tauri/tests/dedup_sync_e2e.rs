//! 去重与条目身份的回归测试（OPT-008B 第一阶段迁移版）。
//!
//! 迁移映射（旧期望 → 新语义；旧期望是「入库丢身份」模型的产物）：
//! - 旧①「跨源同 URL 被 dedup 拦截、写 deduped_urls 墓碑、重放持续拦截」
//!   → 新：跨源同 URL **必须各自入库**（条目是订阅源/远端 ID 的独立实体）；
//!   智能去重不再在入库丢弃身份，改为查询呈现层的聚合策略（见 db/articles.rs
//!   的 dedup 谓词与 dedup_tests.rs 的展示集合一致性断言）。deduped_urls 表与
//!   clear_dedup_tombstones 保留为**有限兼容**（008A 迁移已清空旧墓碑；开关
//!   关闭路径仍调用清理，表结构/清理函数不得删除），但入库不再读写它。
//! - 旧②「保留篇被删后墓碑仍拦重放」→ 新：入库不再有墓碑闸门；RSS 重放按
//!   常规语义处理（guid 命中更新 / 新 guid 插入），另一源的独立条目不受影响。
//! - 旧③「maps 的 id_to_mf_pair 推导 same_feed_trusted 拒绝跨源」→ 新：
//!   条目身份在选行层收口（精确 remote_id 直配 + 同 feed URL 兜底），
//!   `feed_url_to_id` 按 (feed, url_norm) 键隔离各源；跨源 entry 在结构上
//!   命不中另一源的行。真实 pull 路径的跨源隔离断言在
//!   tests/entry_identity_e2e.rs（真实隔离库 + 两协议）。

use app_lib::db;

mod common;

fn temp_db(name: &str) -> std::path::PathBuf {
    let tmp = common::unique_db_path(&format!("dedup_{name}"));
    let _ = std::fs::remove_file(&tmp);
    tmp
}

fn article(url: &str, guid: &str) -> db::NewArticle {
    db::NewArticle {
        guid: guid.into(),
        url: Some(url.into()),
        title: format!("T-{guid}"),
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
    }
}

fn setup(name: &str) -> (rusqlite::Connection, std::path::PathBuf) {
    let tmp = temp_db(name);
    let conn = db::open(&tmp).expect("open db");
    db::create_folder(&conn, "F", "article").unwrap();
    (conn, tmp)
}

/* ============================================================
① 跨源同 URL：身份保留（旧「吞条目」语义的迁移版）
============================================================ */

/// dedup 开关开着也不得丢身份：feed1 / feed2 同 URL、不同 guid → 两行都在；
/// 同源同 URL 重放仍是更新（不新增行）；旧内容墓碑（deduped_urls）不再拦新行。
#[test]
fn cross_feed_same_url_keeps_both_rows_and_legacy_tombstone_is_inert() {
    let (conn, tmp) = setup("identity");

    db::insert_feed(
        &conn,
        "http://x/1.xml",
        None,
        "F1",
        None,
        1,
        "inherit",
        false,
        false,
    )
    .unwrap();
    db::insert_feed(
        &conn,
        "http://x/2.xml",
        None,
        "F2",
        None,
        1,
        "inherit",
        false,
        false,
    )
    .unwrap();

    let (a1, new1) =
        db::upsert_article_with_feed(&conn, 1, &article("http://x/same", "g1"), true).unwrap();
    assert!(new1, "first article ingested");
    let (a2, new2) =
        db::upsert_article_with_feed(&conn, 2, &article("http://x/same", "g2"), true).unwrap();
    assert!(
        new2,
        "跨源同 URL 必须独立入库（旧模型下被 dedup 拦截——该期望已按新语义迁移）"
    );
    assert_ne!(a1, a2, "两条是不同持久条目");

    // 同源同 URL 重放（guid 稳定）→ 仍命中自己的行（更新，不新增）
    let (again1, new3) =
        db::upsert_article_with_feed(&conn, 1, &article("http://x/same", "g1"), true).unwrap();
    assert_eq!(again1, a1);
    assert!(!new3, "同源同 URL 重放不得新增行");

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM articles WHERE url = 'http://x/same'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 2, "两源各一条；无第三行");

    // 旧内容墓碑（历史遗留）：不再作为入库闸门
    conn.execute(
        "INSERT OR REPLACE INTO deduped_urls (url, kept_aid) VALUES ('http://x/same/legacy', ?1)",
        [a1],
    )
    .unwrap();
    db::insert_feed(
        &conn,
        "http://x/3.xml",
        None,
        "F3",
        None,
        1,
        "inherit",
        false,
        false,
    )
    .unwrap();
    let (_, new4) =
        db::upsert_article_with_feed(&conn, 3, &article("http://x/same/legacy", "g3"), true)
            .unwrap();
    assert!(
        new4,
        "旧墓碑（deduped_urls）不得再拦截新条目入库（有限兼容不再是身份闸门）"
    );

    // 开关关闭路径的清理仍可用且幂等（保留的有限兼容入口）
    db::clear_dedup_tombstones(&conn).unwrap();
    db::clear_dedup_tombstones(&conn).unwrap();

    let _ = std::fs::remove_file(&tmp);
}

/// 删除一条后，另一源的独立条目原样；被删源的重放按常规 RSS 语义重新插入
/// （不再有「墓碑拦重放」——那是旧吞身份模型的删除保护，已随新语义迁移）。
#[test]
fn deleting_one_copy_does_not_affect_the_other() {
    let (conn, tmp) = setup("del");

    db::insert_feed(
        &conn,
        "http://x/1.xml",
        None,
        "F1",
        None,
        1,
        "inherit",
        false,
        false,
    )
    .unwrap();
    db::insert_feed(
        &conn,
        "http://x/2.xml",
        None,
        "F2",
        None,
        1,
        "inherit",
        false,
        false,
    )
    .unwrap();
    let (a1, _) =
        db::upsert_article_with_feed(&conn, 1, &article("http://x/dup", "g1"), true).unwrap();
    let (a2, _) =
        db::upsert_article_with_feed(&conn, 2, &article("http://x/dup", "g2"), true).unwrap();
    db::set_read(&conn, a2, true).unwrap();

    conn.execute("DELETE FROM articles WHERE id = ?1", [a1])
        .unwrap();

    // 另一源的条目与状态原样
    let (kept_read, kept_url): (i64, String) = conn
        .query_row(
            "SELECT is_read, url FROM articles WHERE id = ?1",
            [a2],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(kept_read, 1, "删除同 URL 的兄弟条目不影响本条的读状态");
    assert_eq!(kept_url, "http://x/dup");

    // feed1 重放：重新入库（常规 RSS 语义；无墓碑闸门）
    let (replayed, replayed_new) =
        db::upsert_article_with_feed(&conn, 1, &article("http://x/dup", "g1"), true).unwrap();
    assert!(replayed_new, "删除后重放按常规语义重新插入");
    assert_ne!(replayed, a1, "重新插入是新行 id");
    let (still_read,): (i64,) = conn
        .query_row("SELECT is_read FROM articles WHERE id = ?1", [a2], |r| {
            Ok((r.get(0)?,))
        })
        .unwrap();
    assert_eq!(still_read, 1, "重放不得触碰另一源的行");

    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
② 选行映射结构：跨源 entry 在结构上命不中另一源的行
============================================================ */

/// 两个本地源各自绑定远端 feed 10 / 20，各有一条同 URL 的行（未绑定）：
/// - `feed_url_to_id` 以 (feed id, url_norm) 为键——每个源只能选到自己的行；
/// - `mf_id_to_article` 在绑定后各自成键；
/// - `id_to_feed_mf` 给出每行的 feed remote id（精确匹配后的信任复核输入）。
#[test]
fn maps_select_rows_per_feed_not_globally() {
    let (conn, tmp) = setup("maps");

    db::insert_feed(
        &conn,
        "http://x/a.xml",
        None,
        "FA",
        None,
        1,
        "inherit",
        false,
        false,
    )
    .unwrap();
    db::insert_feed(
        &conn,
        "http://x/b.xml",
        None,
        "FB",
        None,
        1,
        "inherit",
        false,
        false,
    )
    .unwrap();
    db::set_feed_remote_id(&conn, 1, 10).unwrap();
    db::set_feed_remote_id(&conn, 2, 20).unwrap();

    let (aid_a, _) =
        db::upsert_article_with_feed(&conn, 1, &article("http://x/news", "ga"), false).unwrap();
    let (aid_b, _) =
        db::upsert_article_with_feed(&conn, 2, &article("http://x/news", "gb"), false).unwrap();

    let norm = db::normalize_url("http://x/news");
    let maps = db::sync_match_maps(&conn).unwrap();
    assert_eq!(
        maps.feed_url_to_id.get(&(1, norm.clone())).copied(),
        Some(aid_a),
        "feed A 的 URL 兜底只能选到自己源的行"
    );
    assert_eq!(
        maps.feed_url_to_id.get(&(2, norm.clone())).copied(),
        Some(aid_b),
        "feed B 的 URL 兜底只能选到自己源的行（旧全局映射会选最小 id 行）"
    );
    assert_eq!(
        maps.id_to_feed_mf.get(&aid_a).copied().flatten(),
        Some(10),
        "每行暴露自己的 feed remote id"
    );
    assert_eq!(maps.id_to_feed_mf.get(&aid_b).copied().flatten(), Some(20));
    // 同文行集（已读广播选择集）：两行都要在
    let mut ids = maps.url_norm_to_ids.get(&norm).cloned().unwrap_or_default();
    ids.sort_unstable();
    assert_eq!(ids, vec![aid_a, aid_b], "同文行集包含跨源两行");

    // 绑定后：mf_id_to_article 各自成键
    db::set_article_remote_id(&conn, aid_a, 100).unwrap();
    db::set_article_remote_id(&conn, aid_b, 200).unwrap();
    let maps = db::sync_match_maps(&conn).unwrap();
    assert_eq!(maps.mf_id_to_article.get(&100).copied(), Some(aid_a));
    assert_eq!(maps.mf_id_to_article.get(&200).copied(), Some(aid_b));
    assert!(
        !maps.mf_id_to_article.values().any(|v| *v == 999_999),
        "无虚构映射"
    );

    let _ = std::fs::remove_file(&tmp);
}

/// 已读广播目标集（推送侧消费）：同文兄弟的 remote_id + 旧 CSV 记账的
/// 有限兼容，去重且排除自身。
#[test]
fn read_broadcast_targets_cover_siblings_and_legacy_csv() {
    let (conn, tmp) = setup("broadcast_targets");

    db::insert_feed(
        &conn,
        "http://x/1.xml",
        None,
        "F1",
        None,
        1,
        "inherit",
        false,
        false,
    )
    .unwrap();
    db::insert_feed(
        &conn,
        "http://x/2.xml",
        None,
        "F2",
        None,
        1,
        "inherit",
        false,
        false,
    )
    .unwrap();
    let (a1, _) =
        db::upsert_article_with_feed(&conn, 1, &article("http://x/b", "g1"), false).unwrap();
    let (a2, _) =
        db::upsert_article_with_feed(&conn, 2, &article("http://x/b", "g2"), false).unwrap();
    db::set_article_remote_id(&conn, a1, 100).unwrap();
    db::set_article_remote_id(&conn, a2, 200).unwrap();
    // 旧 CSV 记账：远端 feed 未绑定时的历史副本（有限兼容）
    db::add_article_dup_entry(&conn, a1, 300).unwrap();
    db::add_article_dup_entry(&conn, a1, 200).unwrap(); // 与兄弟重复 → 去重

    let targets = db::read_broadcast_targets(&conn, a1).unwrap();
    assert_eq!(
        targets,
        vec![200, 300],
        "目标集 = 同文兄弟 remote_id + CSV 记账（去重、升序、不含自身 100）"
    );

    // 不相关 URL 的行不进广播集
    let (a3, _) =
        db::upsert_article_with_feed(&conn, 1, &article("http://x/other", "g3"), false).unwrap();
    db::set_article_remote_id(&conn, a3, 400).unwrap();
    let targets = db::read_broadcast_targets(&conn, a1).unwrap();
    assert_eq!(targets, vec![200, 300], "不同 URL 的行不参与广播");

    let _ = std::fs::remove_file(&tmp);
}
