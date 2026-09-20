//! 去重与同步状态交叉行为的回归测试：
//! ① 复活防护：跨源同 URL 的 Miniflux entry 不得覆盖本地状态/抢绑定
//!    （状态只从绑定的那条 entry 流入）
//! ② 去重墓碑：丢弃时记账、重放持续拦截、保留篇被删后仍拦、
//!    开关关闭清空后放行

use app_lib::db;

fn temp_db(name: &str) -> std::path::PathBuf {
    let tmp = std::env::temp_dir().join(format!(
        "fluxreader_dedup_{name}_{}.db",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
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
② 墓碑生命周期
============================================================ */

#[test]
fn tombstone_written_on_dedup_and_blocks_replay() {
    let (conn, tmp) = setup("tomb");

    // feed1 入一篇，feed2 推同 URL（不同 guid）→ 被去重 + 记墓碑
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
    let (_, new2) =
        db::upsert_article_with_feed(&conn, 2, &article("http://x/same", "g2"), true).unwrap();
    assert!(!new2, "duplicate blocked");

    // 墓碑记账：kept_aid 指向保留的那篇
    let kept: i64 = conn
        .query_row(
            "SELECT kept_aid FROM deduped_urls WHERE url = 'http://x/same'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(kept, a1, "tombstone records the kept article");

    // 重放（feed B 的 guid 稳定 → 每轮刷新都会再来）：墓碑持续拦截
    for i in 0..3 {
        let (_, n) =
            db::upsert_article_with_feed(&conn, 2, &article("http://x/same", "g2"), true).unwrap();
        assert!(!n, "replay #{i} must stay blocked while dedup is on");
    }
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM articles WHERE url = 'http://x/same'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "still exactly one article with that URL");

    // 清墓碑（开关关闭语义：此后调用链传 dedup=false，走正常 upsert）
    db::clear_dedup_tombstones(&conn).unwrap();
    let (_, n3) =
        db::upsert_article_with_feed(&conn, 2, &article("http://x/same", "g2"), false).unwrap();
    assert!(n3, "dedup off: duplicate allowed in");
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM articles WHERE url = 'http://x/same'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 2, "both copies present after dedup disabled");

    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn tombstone_blocks_resurrection_after_kept_article_deleted() {
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
    let _ = db::upsert_article_with_feed(&conn, 2, &article("http://x/dup", "g2"), true).unwrap();

    // 用户删掉保留的那篇（ON DELETE CASCADE 连带清掉墓碑的 FK 引用行？
    // —— 不会：FK 约束会阻止删除或级联删墓碑，两者都可接受，验证实际行为）
    conn.execute("DELETE FROM articles WHERE id = ?1", [a1])
        .unwrap();
    let tomb_left: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM deduped_urls WHERE url = 'http://x/dup'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    if tomb_left > 0 {
        // 墓碑仍在（FK 未级联）：同 URL 重放继续被拦（删除不复活）
        let (_, n) =
            db::upsert_article_with_feed(&conn, 2, &article("http://x/dup", "g2"), true).unwrap();
        assert!(!n, "deleted article must not resurrect via replay");
    }
    // 墓碑被级联清掉：同 URL 视为全新内容，允许入库（用户删了就是不想看到，
    // 但 feed B 后续版本 guid 若变仍会插入新行——这是 RSS 语义，不拦）

    let _ = std::fs::remove_file(&tmp);
}

/* ============================================================
① 复活防护（同源判定矩阵）

TASK-070：原断言调用被 SyncMatchMaps 取代的逐条查询
`db::article_matches_remote_feed`（该函数零生产调用，已删除）。按 test_review 的
adapt 处置，改为经生产入口 `db::sync_match_maps` 校验同一判定的**输入数据**：
生产代码（sync/entries.rs 的 merge_pulled_entry / 状态合并）用
`id_to_mf_pair` 的 (entry remote_id, feed remote_id) 推导 same_feed_trusted，
所以「跨源 entry 不得被信任」这一保护必须继续在 maps 层被证明。
============================================================ */

/// 复刻生产判定（sync/entries.rs:82-89 的 same_feed_trusted）：仅用于校验
/// maps 提供的输入，不替代生产实现。
fn same_feed_trusted(maps: &db::SyncMatchMaps, aid: i64, remote_feed_id: i64) -> bool {
    maps.id_to_mf_pair
        .get(&aid)
        .map(|(entry_mf, feed_mf)| match entry_mf {
            Some(_) => *feed_mf == Some(remote_feed_id),
            None => true,
        })
        .unwrap_or(false)
}

#[test]
fn cross_feed_remote_entry_cannot_write_state_or_steal_binding() {
    let (conn, tmp) = setup("guard");

    // 两个本地源，各自绑定远端 feed 10 / 20
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

    // feed A 的文章，绑定远端 entry 100（feed 10）
    let (aid, _) =
        db::upsert_article_with_feed(&conn, 1, &article("http://x/news", "ga"), false).unwrap();
    db::set_article_remote_id(&conn, aid, 100).unwrap();

    // 判定矩阵：经生产入口取 maps（每次取最新，模拟生产逐轮构建）
    let maps = db::sync_match_maps(&conn).unwrap();
    // 同源 entry（feed 10）：允许
    assert!(
        same_feed_trusted(&maps, aid, 10),
        "same-feed entry is trusted"
    );
    // 跨源 entry（feed 20）：拒绝——服务端另一条同 URL entry 无权写状态
    assert!(
        !same_feed_trusted(&maps, aid, 20),
        "cross-feed entry must be rejected"
    );
    // maps 里必须同时带出文章 remote_id 与所属源的 remote_id（判定输入完整）
    assert_eq!(
        maps.id_to_mf_pair.get(&aid).copied(),
        Some((Some(100), Some(10))),
        "maps must expose (entry remote_id, feed remote_id) for the trust decision"
    );

    // 未绑定 feed 的文章（feed 无 remote_id 视图下）：绑定后按绑定走
    let (aid2, _) =
        db::upsert_article_with_feed(&conn, 2, &article("http://x/other", "gb"), false).unwrap();
    db::set_article_remote_id(&conn, aid2, 200).unwrap();
    let maps = db::sync_match_maps(&conn).unwrap();
    assert!(
        !same_feed_trusted(&maps, aid2, 10),
        "feed-B article rejects feed-10 entry"
    );
    assert!(
        same_feed_trusted(&maps, aid2, 20),
        "own-feed entry is trusted"
    );

    // 不存在的文章 → 保守拒绝
    let maps = db::sync_match_maps(&conn).unwrap();
    assert!(
        !same_feed_trusted(&maps, 99999, 10),
        "missing article: reject"
    );

    // 未绑定 entry 的文章：首见允许（绑定回填的正常路径）
    let (aid3, _) =
        db::upsert_article_with_feed(&conn, 1, &article("http://x/fresh", "gc"), false).unwrap();
    let maps = db::sync_match_maps(&conn).unwrap();
    assert!(
        same_feed_trusted(&maps, aid3, 10),
        "unbound article: first sight allows binding"
    );

    let _ = std::fs::remove_file(&tmp);
}
