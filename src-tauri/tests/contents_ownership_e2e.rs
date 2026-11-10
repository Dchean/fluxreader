//! OPT-008B 第二阶段：contents 单一正文所有权的端到端回归（当前格式隔离库）。
//!
//! 覆盖卡末尾必测：等价共享 / 不同内容不共享 / 空内容不聚合 / 单引用修订分叉 /
//! 删除一个源 vs 删除最后引用 / 正文后半变更（snippet 相同但修订/内容变化）/
//! 搜索索引随共享内容更新（每个引用条目可搜、旧词消失；v29 起为 trigram 候选索引 + LIKE 验证）/ 异步提交守卫
//! （过期前提拒绝落库、清理代次拒绝复活）/ 同格式重开与内容ID不复用。
//! 使用隔离临时库；不访问网络/用户库。

use app_lib::db;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::PathBuf;

mod common;

struct TempDb {
    path: PathBuf,
}

impl TempDb {
    fn new(name: &str) -> Self {
        let path = common::unique_db_path(name);
        let _ = std::fs::remove_file(&path);
        Self { path }
    }
    fn open(&self) -> Connection {
        db::open(&self.path).expect("open isolated db")
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(self.path.with_extension("db-wal"));
        let _ = std::fs::remove_file(self.path.with_extension("db-shm"));
    }
}

fn seed_feed(conn: &Connection, folder: i64, url: &str, title: &str) -> i64 {
    db::insert_feed(
        conn, url, None, title, None, folder, "inherit", false, false,
    )
    .unwrap()
}

fn new_article(guid: &str, url: &str, title: &str, html: &str) -> db::NewArticle {
    // 夹具：纯文本由 HTML 粗略剥标签得出（生产由抓取层 html_to_text 保证同源；
    // 测试只需 body_text 与 html 一致以便 搜索/snippet 断言）。
    let body = html.replace("<p>", "").replace("</p>", "");
    db::NewArticle {
        guid: guid.into(),
        url: Some(url.into()),
        title: title.into(),
        author: None,
        summary: None,
        content_html: Some(html.into()),
        body_text: body,
        image_url: None,
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: Some("2026-01-01T00:00:00+00:00".into()),
        source: "direct".into(),
    }
}

fn content_id_of(conn: &Connection, aid: i64) -> Option<i64> {
    conn.query_row(
        "SELECT content_id FROM articles WHERE id = ?1",
        [aid],
        |r| r.get(0),
    )
    .unwrap()
}

fn contents_count(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM contents", [], |r| r.get(0))
        .unwrap()
}

/// 生产搜索入口（OPT-017B 起 = trigram 候选索引 + LIKE 验证）在该词下的命中
/// id，排序后返回——与排序契约解耦（排序等值在 db/articles.rs 的等值矩阵锁定）。
fn search_ids(conn: &Connection, term: &str) -> Vec<i64> {
    let mut ids: Vec<i64> = db::search_articles(conn, term, 50)
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    ids.sort_unstable();
    ids
}

/* ============================================================
一、候选与等价：URL 只是候选；内容等价才共享；不同内容确定分叉
============================================================ */

#[test]
fn equivalent_content_shares_one_row_across_sources() {
    let tmp = TempDb::new("contents_share");
    let conn = tmp.open();
    let f = db::create_folder(&conn, "F", "article").unwrap();
    let a = seed_feed(&conn, f, "https://a.example/rss", "A");
    let b = seed_feed(&conn, f, "https://b.example/rss", "B");

    // 跨源同文（相同来源类型 + 相同字节）= 等价 → 共享一行。
    let (id1, _) = db::upsert_article_with_feed(
        &conn,
        a,
        &new_article("g1", "https://a.example/post/1", "甲", "<p>共享正文</p>"),
        false,
    )
    .unwrap();
    let (id2, _) = db::upsert_article_with_feed(
        &conn,
        b,
        &new_article(
            "g2",
            "https://b.example/post/1",
            "乙（转载）",
            "<p>共享正文</p>",
        ),
        false,
    )
    .unwrap();
    assert_eq!(
        content_id_of(&conn, id1),
        content_id_of(&conn, id2),
        "等价内容共享同一 contents 行"
    );
    assert_eq!(contents_count(&conn), 1, "只应有一行 contents");
    // 标题保持各自条目所有权（不因正文同一被覆盖）
    let t1: String = conn
        .query_row("SELECT title FROM articles WHERE id = ?1", [id1], |r| {
            r.get(0)
        })
        .unwrap();
    let t2: String = conn
        .query_row("SELECT title FROM articles WHERE id = ?1", [id2], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!((t1.as_str(), t2.as_str()), ("甲", "乙（转载）"));
}

#[test]
fn same_candidate_url_different_html_forks() {
    let tmp = TempDb::new("contents_fork_diff");
    let conn = tmp.open();
    let f = db::create_folder(&conn, "F", "article").unwrap();
    let a = seed_feed(&conn, f, "https://a.example/rss", "A");

    let (id1, _) = db::upsert_article_with_feed(
        &conn,
        a,
        &new_article("g1", "https://a.example/post/x", "甲", "<p>版本一</p>"),
        false,
    )
    .unwrap();
    let cid_original = content_id_of(&conn, id1).unwrap();
    // 同 feed 同 URL：guid 相同 → 既有行刷新；guid 不同 → URL 兜底命中同一行刷新。
    // 两种情况内容都被替换为不同字节 → 必须产生**新的** contents 行（不误并与旧行）。
    let id2 = db::upsert_article_with_feed_tracked(
        &conn,
        a,
        &new_article("g1", "https://a.example/post/x", "甲", "<p>版本二不同</p>"),
        false,
    )
    .unwrap();
    assert_eq!(id1, id2.id, "同 guid 重放命中同一条目行");
    assert!(id2.content_changed, "不同字节必须计为内容变化");
    let cid_first = content_id_of(&conn, id1).unwrap();
    assert_ne!(
        cid_first, cid_original,
        "不同字节 = 新 contents 行（分叉，不是原地改旧行）"
    );
    let html: String = conn
        .query_row(
            "SELECT content_html FROM contents WHERE id = ?1",
            [cid_first],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        html, "<p>版本二不同</p>",
        "条目指向新版本内容（不同字节 = 新 contents 行，不误并）"
    );
    assert_eq!(
        contents_count(&conn),
        1,
        "同条目替换后旧行无引用即回收（最后引用才回收，不留孤儿）"
    );
    // 不同真实 GUID 保留各自实体，即使 URL 相同；不同正文也不得合组。
    let (id3, _) = db::upsert_article_with_feed(
        &conn,
        a,
        &new_article("g-other", "https://a.example/post/x", "甲", "<p>版本三</p>"),
        false,
    )
    .unwrap();
    assert_ne!(id3, id1, "不同真实 GUID 不得借 URL 合并身份");
    assert_ne!(
        content_id_of(&conn, id3),
        Some(cid_first),
        "再次不同字节 → 又一行新内容"
    );
    assert_eq!(contents_count(&conn), 2, "两份不同 RSS 正文各有持久引用");
}

#[test]
fn same_html_different_plain_text_forks() {
    let tmp = TempDb::new("contents_body_fork");
    let conn = tmp.open();
    let f = db::create_folder(&conn, "F", "article").unwrap();
    let a = seed_feed(&conn, f, "https://a.example/rss", "A");
    let b = seed_feed(&conn, f, "https://b.example/rss", "B");

    // 同候选 URL、同 HTML 字节、同来源类型，**纯文本不同** → 身份必须分叉
    // （纯文本进等价判定：只比 HTML 会把两份不同内容误并）。
    let mut one = new_article("g1", "https://shared.example/p", "一", "<p>相同 HTML</p>");
    one.body_text = "纯文本甲".into();
    let mut two = new_article("g2", "https://shared.example/p", "二", "<p>相同 HTML</p>");
    two.body_text = "纯文本乙".into();
    let (id1, _) = db::upsert_article_with_feed(&conn, a, &one, false).unwrap();
    let (id2, _) = db::upsert_article_with_feed(&conn, b, &two, false).unwrap();
    assert_ne!(
        content_id_of(&conn, id1),
        content_id_of(&conn, id2),
        "纯文本不同必须自成分叉（不共享）"
    );
    assert_eq!(contents_count(&conn), 2, "两份内容各自一行");
}

#[test]
fn empty_content_never_aggregates() {
    let tmp = TempDb::new("contents_empty");
    let conn = tmp.open();
    let f = db::create_folder(&conn, "F", "article").unwrap();
    let a = seed_feed(&conn, f, "https://a.example/rss", "A");

    let mut empty1 = new_article("e1", "https://a.example/e/1", "空一", "");
    empty1.content_html = None;
    let mut empty2 = new_article("e2", "https://a.example/e/2", "空二", "");
    empty2.content_html = None;
    let (id1, _) = db::upsert_article_with_feed(&conn, a, &empty1, false).unwrap();
    let (id2, _) = db::upsert_article_with_feed(&conn, a, &empty2, false).unwrap();
    assert_ne!(
        content_id_of(&conn, id1),
        content_id_of(&conn, id2),
        "空内容条目各自私有，不聚合"
    );
    assert_eq!(contents_count(&conn), 2);
}

/* ============================================================
二、修订与分叉：全文提取（单引用）与 RSS 保护
============================================================ */

#[test]
fn extraction_forks_from_shared_rss_and_protects_against_rss_refresh() {
    let tmp = TempDb::new("contents_extract");
    let conn = tmp.open();
    let f = db::create_folder(&conn, "F", "article").unwrap();
    let a = seed_feed(&conn, f, "https://a.example/rss", "A");
    let b = seed_feed(&conn, f, "https://b.example/rss", "B");

    let (id1, _) = db::upsert_article_with_feed(
        &conn,
        a,
        &new_article("g1", "https://a.example/p", "甲", "<p>RSS</p>"),
        false,
    )
    .unwrap();
    let (id2, _) = db::upsert_article_with_feed(
        &conn,
        b,
        &new_article("g2", "https://b.example/p", "乙", "<p>RSS</p>"),
        false,
    )
    .unwrap();
    let shared = content_id_of(&conn, id1).unwrap();
    assert_eq!(Some(shared), content_id_of(&conn, id2));

    // 条目 1 做全文提取（Readability 变体）→ 单引用分叉：新行 + 重指，条目 2 不受影响
    db::update_article_fulltext(&conn, id1, "<p>提取全文（更完整）</p>", true).unwrap();
    let cid1 = content_id_of(&conn, id1).unwrap();
    assert_ne!(cid1, shared, "提取必须分叉到新内容行");
    assert_eq!(
        content_id_of(&conn, id2),
        Some(shared),
        "兄弟条目仍指向原共享行"
    );
    let (flag1, body1): (i64, String) = conn
        .query_row(
            "SELECT fulltext_extracted, body_text FROM contents WHERE id = ?1",
            [cid1],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(flag1, 1);
    assert_eq!(body1, "提取全文（更完整）", "提取行纯文本与 HTML 同源");

    // 条目 1 再经 RSS 刷新（同 URL、更短摘要）→ 保护：正文不被覆盖、条目不重指
    let (id_again, up) = db::upsert_article_with_feed(
        &conn,
        a,
        &new_article("g1", "https://a.example/p", "甲", "<p>短 RSS 摘要</p>"),
        false,
    )
    .unwrap();
    assert_eq!(id_again, id1);
    let _ = up;
    assert_eq!(
        content_id_of(&conn, id1),
        Some(cid1),
        "提取正文不得被 RSS 刷新替换"
    );
    let html1: String = conn
        .query_row(
            "SELECT content_html FROM contents WHERE id = ?1",
            [cid1],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(html1, "<p>提取全文（更完整）</p>");

    // 两个条目各自可搜到（提取正文新词只在条目 1 可搜；共享 RSS 正文只剩条目 2）
    assert_eq!(
        search_ids(&conn, "提取全文"),
        vec![id1],
        "提取正文仅条目 1 可搜"
    );
    assert_eq!(
        search_ids(&conn, "RSS"),
        vec![id2],
        "分叉后共享 RSS 正文仅兄弟条目（条目 2）可搜"
    );
}

#[test]
fn late_extraction_commit_is_rejected_after_content_replaced() {
    let tmp = TempDb::new("contents_late_commit");
    let conn = tmp.open();
    let f = db::create_folder(&conn, "F", "article").unwrap();
    let a = seed_feed(&conn, f, "https://a.example/rss", "A");
    let (aid, _) = db::upsert_article_with_feed(
        &conn,
        a,
        &new_article("g1", "https://a.example/p", "甲", "<p>RSS v1</p>"),
        false,
    )
    .unwrap();

    // 捕获发起前提（模拟网络前快照），随后内容被替换（新 RSS 字节 → 重指新行）
    let (token, original) = db::article_content_commit_token(&conn, aid)
        .unwrap()
        .unwrap();
    assert_eq!(original, "<p>RSS v1</p>");
    db::upsert_article_with_feed(
        &conn,
        a,
        &new_article("g1", "https://a.example/p", "甲", "<p>RSS v2 正文更长</p>"),
        false,
    )
    .unwrap();

    // 迟到的提取提交：前提（修订已变）不符 → 拒绝，不写分叉、不覆盖新内容
    let committed =
        db::replace_article_content_checked(&conn, aid, &token, "<p>迟到的提取结果</p>").unwrap();
    assert!(committed.is_none(), "过期前提的迟到提取必须被拒绝");
    let html: String = conn
        .query_row(
            "SELECT c.content_html FROM articles a LEFT JOIN contents c ON c.id = a.content_id WHERE a.id = ?1",
            [aid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(html, "<p>RSS v2 正文更长</p>", "拒绝后正文保持最新版本");

    // 条目被删除：迟到提取同样拒绝（不复活已删除条目），最后引用随删除回收
    conn.execute("DELETE FROM articles WHERE id = ?1", [aid])
        .unwrap();
    assert_eq!(contents_count(&conn), 0, "条目删除后最后引用即回收");
    assert!(
        db::replace_article_content_checked(&conn, aid, &token, "<p>删除后的迟到提取</p>")
            .unwrap()
            .is_none(),
        "已删除条目的迟到提取必须被拒绝"
    );
}

#[test]
fn ai_commit_guard_rejects_after_content_change_and_cleanup() {
    let tmp = TempDb::new("contents_ai_guard");
    let conn = tmp.open();
    let f = db::create_folder(&conn, "F", "article").unwrap();
    let a = seed_feed(&conn, f, "https://a.example/rss", "A");
    let (aid, _) = db::upsert_article_with_feed(
        &conn,
        a,
        &new_article("g1", "https://a.example/p", "甲", "<p>正文一</p>"),
        false,
    )
    .unwrap();

    // 正常路径：前提相符 → 写入成功（OPT-013 B1：产物与签名成对提交）
    let token = db::article_ai_commit_token(&conn, aid).unwrap().unwrap();
    let ok = db::set_article_ai_fields_checked(
        &conn,
        aid,
        &token,
        db::AiProduct::Summary {
            text: "摘要 v1",
            signature: "sig-v1",
        },
    )
    .unwrap();
    assert!(ok, "前提相符提交成功");
    let (prod, sig): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT ai_summary, ai_summary_sig FROM articles WHERE id = ?1",
            [aid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(prod.as_deref(), Some("摘要 v1"));
    assert_eq!(
        sig.as_deref(),
        Some("sig-v1"),
        "签名与产物同一条 UPDATE 落库"
    );

    // 内容被替换后再提交旧前提 → 拒绝（不得写已替换内容）
    let stale = token.clone();
    db::upsert_article_with_feed(
        &conn,
        a,
        &new_article("g1", "https://a.example/p", "甲", "<p>正文二（更新）</p>"),
        false,
    )
    .unwrap();
    assert!(
        !db::set_article_ai_fields_checked(
            &conn,
            aid,
            &stale,
            db::AiProduct::Summary {
                text: "过期摘要",
                signature: "sig-stale",
            },
        )
        .unwrap(),
        "内容已替换：旧前提的迟到 AI 提交必须拒绝"
    );
    let (summary, summary_sig): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT ai_summary, ai_summary_sig FROM articles WHERE id = ?1",
            [aid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(summary.as_deref(), Some("摘要 v1"), "拒绝不覆盖既有产物");
    assert_eq!(summary_sig.as_deref(), Some("sig-v1"), "拒绝不得半写签名");

    // 清理（ai scope）→ ai_epoch 推进：清前捕获的前提提交被拒（不复活已清产物）
    conn.execute(
        "UPDATE articles SET published_at = datetime('now', '-40 days') WHERE id = ?1",
        [aid],
    )
    .unwrap();
    let mut conn2 = conn;
    let pre_clean_token = db::article_ai_commit_token(&conn2, aid).unwrap().unwrap();
    let (_deleted, cleared) = db::cleanup_cache(&mut conn2, 30, "ai").unwrap();
    assert_eq!(cleared, 1, "按年龄清理命中该条目 AI");
    assert!(
        !db::set_article_ai_fields_checked(
            &conn2,
            aid,
            &pre_clean_token,
            db::AiProduct::Summary {
                text: "复活摘要",
                signature: "sig-revive",
            },
        )
        .unwrap(),
        "清理后的迟到提交必须被拒（不得复活已清产物）"
    );
    let (summary2, summary_sig2): (Option<String>, Option<String>) = conn2
        .query_row(
            "SELECT ai_summary, ai_summary_sig FROM articles WHERE id = ?1",
            [aid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(summary2, None, "清理保持：无 AI 产物");
    assert_eq!(
        summary_sig2, None,
        "清理同时使签名失效（不得只清产物留旧签名）"
    );

    // 条目被删除后：迟到提交同样拒绝（不复活已删条目）
    conn2
        .execute("DELETE FROM articles WHERE id = ?1", [aid])
        .unwrap();
    assert!(
        !db::set_article_ai_fields_checked(
            &conn2,
            aid,
            &pre_clean_token,
            db::AiProduct::Summary {
                text: "复活摘要2",
                signature: "sig-revive2",
            },
        )
        .unwrap(),
        "已删除条目的迟到 AI 提交必须拒绝"
    );
}

/* ============================================================
二·五、identity 不变量（OPT-008C）：修复后身份必须与字节一致
============================================================ */

/// 造「历史异常提取行」：readability + fulltext_extracted=1、HTML 正常但
/// body_text 与 HTML 不同源（旧版只写 HTML+flag 的形态），并把条目指过去。
/// 身份使用与内容无关的私有串，模拟修复前「identity 不随 body 变」的行。
fn install_legacy_readability_row(
    conn: &Connection,
    article_id: i64,
    html: &str,
    wrong_body: &str,
    articles_to_point: &[i64],
) -> i64 {
    conn.execute(
        "INSERT INTO contents
            (identity, url_norm, content_html, body_text, extraction_source, fulltext_extracted)
         VALUES (?1, NULL, ?2, ?3, 'readability', 1)",
        params![format!("h|legacy-mismatch-{article_id}"), html, wrong_body],
    )
    .unwrap();
    let cid = conn.last_insert_rowid();
    for aid in articles_to_point {
        conn.execute(
            "UPDATE articles SET content_id = ?1 WHERE id = ?2",
            params![cid, aid],
        )
        .unwrap();
    }
    cid
}

/// OPT-008C 裁决 a（重算 identity + 同等价合并）：修复受保护提取行的 body
/// 后，新身份命中已有等价行——全部引用条目重指目标行、目标 revision 递增、
/// 原行删除；不留下「身份与字节不一致」的孤儿。
#[test]
fn repair_recomputes_identity_and_merges_equivalent_references() {
    let tmp = TempDb::new("contents_identity_merge");
    let conn = tmp.open();
    let f = db::create_folder(&conn, "F", "article").unwrap();
    let feed = seed_feed(&conn, f, "https://a.example/rss", "A");

    // 等价目标行：合法的 readability 提取行（同 source + 同 HTML + 同纯文本）。
    let (control_aid, _) = db::upsert_article_with_feed(
        &conn,
        feed,
        &new_article("ctrl", "https://a.example/ctrl", "控制", "<p>控制 RSS</p>"),
        false,
    )
    .unwrap();
    let (token, _) = db::article_content_commit_token(&conn, control_aid)
        .unwrap()
        .unwrap();
    let (target_id, target_rev_before) =
        db::replace_article_content_checked(&conn, control_aid, &token, "<p>提取正文</p>")
            .unwrap()
            .unwrap();

    // 两条条目引用同一历史错配行（合并必须重指全部引用，不只当前条目）。
    let (aid1, _) = db::upsert_article_with_feed(
        &conn,
        feed,
        &new_article("g1", "https://a.example/p1", "甲", "<p>甲 RSS</p>"),
        false,
    )
    .unwrap();
    let (aid2, _) = db::upsert_article_with_feed(
        &conn,
        feed,
        &new_article("g2", "https://a.example/p2", "乙", "<p>乙 RSS</p>"),
        false,
    )
    .unwrap();
    let legacy_id = install_legacy_readability_row(
        &conn,
        aid1,
        "<p>提取正文</p>",
        "旧 RSS 错配正文",
        &[aid1, aid2],
    );
    assert_ne!(
        legacy_id, target_id,
        "夹具：错配行与等价目标行必须是两行（否则无合并可测）"
    );

    // RSS 刷新命中受保护行 → 历史错配修复。
    db::upsert_article_with_feed(
        &conn,
        feed,
        &new_article("g1", "https://a.example/p1", "甲", "<p>短 RSS 摘要</p>"),
        false,
    )
    .unwrap();

    assert_eq!(
        content_id_of(&conn, aid1),
        Some(target_id),
        "当前条目必须重指等价目标行"
    );
    assert_eq!(
        content_id_of(&conn, aid2),
        Some(target_id),
        "共享同一错配行的其他引用也必须重指（不得指向已删除行）"
    );
    let legacy_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM contents WHERE id = ?1",
            [legacy_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(legacy_exists, 0, "合并后原错配行必须删除");
    let target_rev_after: i64 = conn
        .query_row(
            "SELECT revision FROM contents WHERE id = ?1",
            [target_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        target_rev_after > target_rev_before,
        "合并必须递增目标行 revision（所有引用感知内容版本变化）：{target_rev_before} -> {target_rev_after}"
    );
    let equivalent_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM contents
             WHERE extraction_source = 'readability' AND content_html = '<p>提取正文</p>'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(equivalent_rows, 1, "同等价内容只允许一行");
}

/// OPT-008C 反例：修复后 identity 必须与字节一致——后续以同字节调用
/// `ensure_content`（经真实提取提交入口）必须复用修复行，而不是新建一行。
/// 修前：body 被就地改写但 identity 未重算 → 新内容算出的身份查不到旧行，
/// 产生第二条同字节 readability 行（欠共享）。
#[test]
fn ensure_content_after_repair_reuses_fixed_row() {
    let tmp = TempDb::new("contents_identity_reuse");
    let conn = tmp.open();
    let f = db::create_folder(&conn, "F", "article").unwrap();
    let feed = seed_feed(&conn, f, "https://a.example/rss", "A");

    let (aid, _) = db::upsert_article_with_feed(
        &conn,
        feed,
        &new_article("g1", "https://a.example/p1", "甲", "<p>甲 RSS</p>"),
        false,
    )
    .unwrap();
    let legacy_id =
        install_legacy_readability_row(&conn, aid, "<p>修复后正文</p>", "旧 RSS 错配", &[aid]);

    // 修复
    db::upsert_article_with_feed(
        &conn,
        feed,
        &new_article("g1", "https://a.example/p1", "甲", "<p>短 RSS</p>"),
        false,
    )
    .unwrap();
    let fixed_id = content_id_of(&conn, aid).unwrap();
    assert_eq!(
        fixed_id, legacy_id,
        "无等价行时修复就地更新原行（不得新建内容行）"
    );
    let (fixed_body, fixed_rev): (String, i64) = conn
        .query_row(
            "SELECT body_text, revision FROM contents WHERE id = ?1",
            [fixed_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        fixed_body,
        app_lib::sanitize::html_to_text("<p>修复后正文</p>"),
        "body 必须按保留 HTML 同源修复"
    );

    // 后续同字节内容：ensure_content 必须命中修复行的身份。
    // （修前此断言先红：身份仍与旧 body 绑定 → 新建第二行。）
    let (aid2, _) = db::upsert_article_with_feed(
        &conn,
        feed,
        &new_article("g2", "https://a.example/p2", "乙", "<p>乙 RSS</p>"),
        false,
    )
    .unwrap();
    let (token, _) = db::article_content_commit_token(&conn, aid2)
        .unwrap()
        .unwrap();
    let (reused_id, reused_rev) =
        db::replace_article_content_checked(&conn, aid2, &token, "<p>修复后正文</p>")
            .unwrap()
            .unwrap();
    assert_eq!(
        reused_id, fixed_id,
        "修复后同字节 ensure_content 必须复用修复行（修前：新建第二行）"
    );
    assert_eq!(reused_rev, fixed_rev, "复用不改 revision（同一内容版本）");
    let fixed_identity: String = conn
        .query_row(
            "SELECT identity FROM contents WHERE id = ?1",
            [fixed_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_ne!(
        fixed_identity,
        format!("h|legacy-mismatch-{aid}"),
        "修复必须重算 identity（修前：identity 保持与旧字节绑定的值）"
    );
    let same_bytes_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM contents
             WHERE extraction_source = 'readability' AND content_html = '<p>修复后正文</p>'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(same_bytes_rows, 1, "同字节 readability 行只能有一条");
}

/// 造「旧实现修复态」：body 已等于 `html_to_text(html)`（旧版修过 body），但
/// identity 仍是旧 body 的规范哈希（`h|...`、无 `|c` 盐）——旧版只改 body
/// 不改 identity 的存量行。返回内容行 id。
fn install_old_repaired_row(
    conn: &Connection,
    article_id: i64,
    html: &str,
    old_body: &str,
    articles_to_point: &[i64],
) -> i64 {
    let body = app_lib::sanitize::html_to_text(html);
    // 旧 body 的规范身份（哈希段用条目 id 占位保证唯一；长度取旧 body，
    // 足以与修复后的 expected 身份区分）。
    let stale_identity = format!("h|{:032x}|{}|{}", article_id, html.len(), old_body.len());
    conn.execute(
        "INSERT INTO contents
            (identity, url_norm, content_html, body_text, extraction_source, fulltext_extracted)
         VALUES (?1, NULL, ?2, ?3, 'readability', 1)",
        params![stale_identity, html, body],
    )
    .unwrap();
    let cid = conn.last_insert_rowid();
    for aid in articles_to_point {
        conn.execute(
            "UPDATE articles SET content_id = ?1 WHERE id = ?2",
            params![cid, aid],
        )
        .unwrap();
    }
    cid
}

/// OPT-008C 返工 P2：旧实现已修过 body、identity 仍指向旧 body 的存量行，
/// 再次进入修复路径必须收敛 identity（不改 body、不 bump revision）；随后
/// 同字节 `ensure_content` 复用该行。
///
/// 修前（早退只看 body）此用例红于 identity 未收敛；再以同字节提取提交会
/// 新建第二行（欠共享）。
#[test]
fn stale_identity_after_old_repair_converges_and_reuses_row() {
    let tmp = TempDb::new("contents_stale_identity_reuse");
    let conn = tmp.open();
    let f = db::create_folder(&conn, "F", "article").unwrap();
    let feed = seed_feed(&conn, f, "https://a.example/rss", "A");

    let (aid, _) = db::upsert_article_with_feed(
        &conn,
        feed,
        &new_article("g1", "https://a.example/p1", "甲", "<p>甲 RSS</p>"),
        false,
    )
    .unwrap();
    let stale_id =
        install_old_repaired_row(&conn, aid, "<p>旧修复态正文</p>", "旧 RSS 错配正文", &[aid]);
    let (identity_before, body_before, rev_before): (String, String, i64) = conn
        .query_row(
            "SELECT identity, body_text, revision FROM contents WHERE id = ?1",
            [stale_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        body_before,
        app_lib::sanitize::html_to_text("<p>旧修复态正文</p>"),
        "夹具：body 必须是旧实现已修好的同源文本"
    );

    // 再次触达修复路径（受保护行的 RSS 刷新）。
    db::upsert_article_with_feed(
        &conn,
        feed,
        &new_article("g1", "https://a.example/p1", "甲", "<p>短 RSS</p>"),
        false,
    )
    .unwrap();

    let fixed_id = content_id_of(&conn, aid).unwrap();
    assert_eq!(fixed_id, stale_id, "无等价行：就地收敛，不新建/不重指");
    let (identity_after, body_after, rev_after): (String, String, i64) = conn
        .query_row(
            "SELECT identity, body_text, revision FROM contents WHERE id = ?1",
            [stale_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_ne!(
        identity_after, identity_before,
        "旧实现修复态的 identity 必须在再次触达时收敛到新字节"
    );
    assert_eq!(body_after, body_before, "身份收敛不得改 body");
    assert_eq!(
        rev_after, rev_before,
        "字节未变：身份收敛不得 bump revision"
    );

    // 同字节 ensure_content 必须命中收敛后的身份。
    let (aid2, _) = db::upsert_article_with_feed(
        &conn,
        feed,
        &new_article("g2", "https://a.example/p2", "乙", "<p>乙 RSS</p>"),
        false,
    )
    .unwrap();
    let (token, _) = db::article_content_commit_token(&conn, aid2)
        .unwrap()
        .unwrap();
    let (reused_id, reused_rev) =
        db::replace_article_content_checked(&conn, aid2, &token, "<p>旧修复态正文</p>")
            .unwrap()
            .unwrap();
    assert_eq!(
        reused_id, stale_id,
        "收敛后同字节 ensure_content 必须复用该行（修前：新建第二行）"
    );
    assert_eq!(reused_rev, rev_after, "复用不改 revision");
}

/// OPT-008C 返工 P2（合并支）：存量旧修复态行的 expected 身份已存在等价行时，
/// 必须合并——全部引用重指目标、目标 revision+1、原行删除。
///
/// 修前（早退只看 body）此用例红于引用仍指向陈旧行。
#[test]
fn stale_identity_merges_into_equivalent_row() {
    let tmp = TempDb::new("contents_stale_identity_merge");
    let conn = tmp.open();
    let f = db::create_folder(&conn, "F", "article").unwrap();
    let feed = seed_feed(&conn, f, "https://a.example/rss", "A");

    // 等价目标行：合法 readability 行（同 source + 同 HTML + 同 body）。
    let (control_aid, _) = db::upsert_article_with_feed(
        &conn,
        feed,
        &new_article("ctrl", "https://a.example/ctrl", "控制", "<p>控制 RSS</p>"),
        false,
    )
    .unwrap();
    let (token, _) = db::article_content_commit_token(&conn, control_aid)
        .unwrap()
        .unwrap();
    let (target_id, target_rev_before) =
        db::replace_article_content_checked(&conn, control_aid, &token, "<p>存量正文</p>")
            .unwrap()
            .unwrap();

    let (aid1, _) = db::upsert_article_with_feed(
        &conn,
        feed,
        &new_article("g1", "https://a.example/p1", "甲", "<p>甲 RSS</p>"),
        false,
    )
    .unwrap();
    let (aid2, _) = db::upsert_article_with_feed(
        &conn,
        feed,
        &new_article("g2", "https://a.example/p2", "乙", "<p>乙 RSS</p>"),
        false,
    )
    .unwrap();
    let stale_id = install_old_repaired_row(
        &conn,
        aid1,
        "<p>存量正文</p>",
        "旧 RSS 错配正文",
        &[aid1, aid2],
    );
    assert_ne!(stale_id, target_id, "夹具：陈旧行与等价目标必须是两行");

    // 触发第一条目的修复路径 → 应合并（全部引用重指）。
    db::upsert_article_with_feed(
        &conn,
        feed,
        &new_article("g1", "https://a.example/p1", "甲", "<p>短 RSS</p>"),
        false,
    )
    .unwrap();

    assert_eq!(
        content_id_of(&conn, aid1),
        Some(target_id),
        "陈旧行必须合并到等价目标"
    );
    assert_eq!(
        content_id_of(&conn, aid2),
        Some(target_id),
        "共享陈旧行的其他引用也必须重指"
    );
    let stale_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM contents WHERE id = ?1",
            [stale_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stale_exists, 0, "合并后陈旧行必须删除");
    let target_rev_after: i64 = conn
        .query_row(
            "SELECT revision FROM contents WHERE id = ?1",
            [target_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        target_rev_after > target_rev_before,
        "合并必须递增目标 revision：{target_rev_before} -> {target_rev_after}"
    );
    let equivalent_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM contents
             WHERE extraction_source = 'readability' AND content_html = '<p>存量正文</p>'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(equivalent_rows, 1, "同等价内容只允许一行");
}

/// OPT-008C tidy P3-1：非规范身份（私有 `e|<article_id>` / 加盐
/// `h|...|c<n>`）即使 body 已同源，也不得被存量身份收敛误改——
/// identity、body_text、revision、content_id 全部保持不变。
#[test]
fn non_canonical_identity_rows_are_left_untouched() {
    let tmp = TempDb::new("contents_noncanonical_identity");
    let conn = tmp.open();
    let f = db::create_folder(&conn, "F", "article").unwrap();
    let feed = seed_feed(&conn, f, "https://a.example/rss", "A");

    let (private_aid, _) = db::upsert_article_with_feed(
        &conn,
        feed,
        &new_article(
            "g-private",
            "https://a.example/private",
            "私有",
            "<p>私有 RSS</p>",
        ),
        false,
    )
    .unwrap();
    let (salted_aid, _) = db::upsert_article_with_feed(
        &conn,
        feed,
        &new_article(
            "g-salted",
            "https://a.example/salted",
            "加盐",
            "<p>加盐 RSS</p>",
        ),
        false,
    )
    .unwrap();

    // 私有身份行：e|<article_id>，body 已与 HTML 同源。
    let private_html = "<p>私有身份正文</p>";
    let private_body = app_lib::sanitize::html_to_text(private_html);
    let private_identity = format!("e|{private_aid}");
    conn.execute(
        "INSERT INTO contents
            (identity, url_norm, content_html, body_text, extraction_source, fulltext_extracted)
         VALUES (?1, NULL, ?2, ?3, 'readability', 1)",
        params![private_identity, private_html, private_body],
    )
    .unwrap();
    let private_cid = conn.last_insert_rowid();
    conn.execute(
        "UPDATE articles SET content_id = ?1 WHERE id = ?2",
        params![private_cid, private_aid],
    )
    .unwrap();

    // 加盐身份行：h|<hash>|<len>|<len>|c<n>，body 同样已同源。
    let salted_html = "<p>加盐身份正文</p>";
    let salted_body = app_lib::sanitize::html_to_text(salted_html);
    let salted_identity = format!(
        "h|{:032x}|{}|{}|c7",
        salted_aid,
        salted_html.len(),
        salted_body.len()
    );
    conn.execute(
        "INSERT INTO contents
            (identity, url_norm, content_html, body_text, extraction_source, fulltext_extracted)
         VALUES (?1, NULL, ?2, ?3, 'readability', 1)",
        params![salted_identity, salted_html, salted_body],
    )
    .unwrap();
    let salted_cid = conn.last_insert_rowid();
    conn.execute(
        "UPDATE articles SET content_id = ?1 WHERE id = ?2",
        params![salted_cid, salted_aid],
    )
    .unwrap();

    let snapshot = |cid: i64| -> (String, String, i64) {
        conn.query_row(
            "SELECT identity, body_text, revision FROM contents WHERE id = ?1",
            [cid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
    };
    let before_private = snapshot(private_cid);
    let before_salted = snapshot(salted_cid);

    // 通过真实保护路径触发修复（RSS 刷新命中受保护提取行）。
    db::upsert_article_with_feed(
        &conn,
        feed,
        &new_article(
            "g-private",
            "https://a.example/private",
            "私有",
            "<p>短 RSS 一</p>",
        ),
        false,
    )
    .unwrap();
    db::upsert_article_with_feed(
        &conn,
        feed,
        &new_article(
            "g-salted",
            "https://a.example/salted",
            "加盐",
            "<p>短 RSS 二</p>",
        ),
        false,
    )
    .unwrap();

    assert_eq!(
        content_id_of(&conn, private_aid),
        Some(private_cid),
        "私有身份行不得被重指"
    );
    assert_eq!(
        content_id_of(&conn, salted_aid),
        Some(salted_cid),
        "加盐身份行不得被重指"
    );
    assert_eq!(
        snapshot(private_cid),
        before_private,
        "私有身份行的 identity/body/revision 必须全部不变"
    );
    assert_eq!(
        snapshot(salted_cid),
        before_salted,
        "加盐身份行的 identity/body/revision 必须全部不变"
    );
}

/* ============================================================
三、引用生命周期：删源/删最后引用、搜索索引同步
============================================================ */

#[test]
fn deleting_one_source_keeps_shared_content_until_last_reference() {
    let tmp = TempDb::new("contents_lifecycle");
    let conn = tmp.open();
    let f = db::create_folder(&conn, "F", "article").unwrap();
    let a = seed_feed(&conn, f, "https://a.example/rss", "A");
    let b = seed_feed(&conn, f, "https://b.example/rss", "B");

    let (id1, _) = db::upsert_article_with_feed(
        &conn,
        a,
        &new_article("g1", "https://a.example/p", "甲", "<p>共享</p>"),
        false,
    )
    .unwrap();
    let (id2, _) = db::upsert_article_with_feed(
        &conn,
        b,
        &new_article("g2", "https://b.example/p", "乙", "<p>共享</p>"),
        false,
    )
    .unwrap();
    let shared = content_id_of(&conn, id1).unwrap();
    assert_eq!(
        search_ids(&conn, "共享"),
        vec![id1, id2],
        "共享正文两个条目都可搜"
    );

    // 删源 A：级联删条目 1；共享内容仍被条目 2 引用 → 保活；命中 1 消失、2 保留
    db::delete_feed(&conn, a).unwrap();
    assert!(db::get_article(&conn, id1).unwrap().is_none());
    assert_eq!(
        content_id_of(&conn, id2),
        Some(shared),
        "共享内容不被误删（仍有引用）"
    );
    assert_eq!(
        search_ids(&conn, "共享"),
        vec![id2],
        "删除源 A 后其条目不可搜、条目 2 仍可搜"
    );

    // 删最后引用：内容与搜索命中一并回收
    db::delete_feed(&conn, b).unwrap();
    assert_eq!(contents_count(&conn), 0, "最后引用消失才回收 contents");
    assert!(
        search_ids(&conn, "共享").is_empty(),
        "无引用条目：搜索命中一并移除"
    );
}

#[test]
fn cache_cleanup_recycles_contents_at_last_reference() {
    let tmp = TempDb::new("contents_cleanup_gc");
    let conn = tmp.open();
    let f = db::create_folder(&conn, "F", "article").unwrap();
    let a = seed_feed(&conn, f, "https://a.example/rss", "A");
    let b = seed_feed(&conn, f, "https://b.example/rss", "B");

    let (id1, _) = db::upsert_article_with_feed(
        &conn,
        a,
        &new_article("g1", "https://a.example/p", "甲", "<p>共享清理</p>"),
        false,
    )
    .unwrap();
    let (id2, _) = db::upsert_article_with_feed(
        &conn,
        b,
        &new_article("g2", "https://b.example/p", "乙", "<p>共享清理</p>"),
        false,
    )
    .unwrap();
    assert_eq!(content_id_of(&conn, id1), content_id_of(&conn, id2));
    // 两条目均进清理窗口（按年龄、已读、非收藏）：命中同一 contents 行
    conn.execute(
        "UPDATE articles SET is_read = 1, published_at = datetime('now', '-40 days') WHERE id IN (?1, ?2)",
        [id1, id2],
    )
    .unwrap();
    let mut conn = conn;
    let (deleted, cleared) = db::cleanup_cache(&mut conn, 30, "articles").unwrap();
    assert_eq!(
        (deleted, cleared),
        (2, 0),
        "按年龄清理删两条目、不动 AI 字段"
    );
    assert_eq!(
        contents_count(&conn),
        0,
        "缓存清理的最后引用消失才回收 contents"
    );
    assert!(
        search_ids(&conn, "共享清理").is_empty(),
        "搜索命中一并消失（候选索引随条目删除同步清理）"
    );
}

#[test]
fn ai_cleanup_splits_shared_content_siblings_by_age() {
    let tmp = TempDb::new("contents_ai_cleanup_split");
    let conn = tmp.open();
    let f = db::create_folder(&conn, "F", "article").unwrap();
    let a = seed_feed(&conn, f, "https://a.example/rss", "A");
    let b = seed_feed(&conn, f, "https://b.example/rss", "B");

    // 共享同一 contents 行，但条目年龄不同（40 天 vs 3 天）。
    let (old_aid, _) = db::upsert_article_with_feed(
        &conn,
        a,
        &new_article("g-old", "https://a.example/p", "旧", "<p>共享 AI 正文</p>"),
        false,
    )
    .unwrap();
    let (young_aid, _) = db::upsert_article_with_feed(
        &conn,
        b,
        &new_article(
            "g-young",
            "https://b.example/p",
            "新",
            "<p>共享 AI 正文</p>",
        ),
        false,
    )
    .unwrap();
    let shared = content_id_of(&conn, old_aid).unwrap();
    assert_eq!(
        content_id_of(&conn, young_aid),
        Some(shared),
        "夹具：两条目必须共享同一内容行"
    );
    conn.execute(
        "UPDATE articles SET published_at = datetime('now', '-40 days'),
             ai_summary = '旧摘要', translated_content = '<p>旧译文</p>' WHERE id = ?1",
        [old_aid],
    )
    .unwrap();
    conn.execute(
        "UPDATE articles SET published_at = datetime('now', '-3 days'),
             ai_summary = '年轻摘要', translated_content = '<p>年轻译文</p>' WHERE id = ?1",
        [young_aid],
    )
    .unwrap();
    let (html_before, body_before): (String, String) = conn
        .query_row(
            "SELECT content_html, body_text FROM contents WHERE id = ?1",
            [shared],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();

    let mut conn = conn;
    let (deleted, ai_cleared) = db::cleanup_cache(&mut conn, 30, "ai").unwrap();
    assert_eq!(deleted, 0, "scope=ai 不删除条目");
    assert_eq!(
        ai_cleared, 1,
        "只清理到期旧条目的 AI（年轻兄弟不在清理对象内）"
    );

    // 旧条目：AI/签名清空、epoch 推进；正文与共享行保留。
    let old_row: (Option<String>, Option<String>, i64) = conn
        .query_row(
            "SELECT ai_summary, translated_content, ai_epoch FROM articles WHERE id = ?1",
            [old_aid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(old_row, (None, None, 1), "旧条目 AI 清空且 epoch 推进");
    let old_sigs: (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT ai_summary_sig, translated_sig FROM articles WHERE id = ?1",
            [old_aid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(old_sigs, (None, None), "清理同时使签名失效");

    // 年轻条目：AI 原样保留（不误清兄弟）。
    let young_row: (Option<String>, Option<String>, i64) = conn
        .query_row(
            "SELECT ai_summary, translated_content, ai_epoch FROM articles WHERE id = ?1",
            [young_aid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        young_row,
        (
            Some("年轻摘要".to_string()),
            Some("<p>年轻译文</p>".to_string()),
            0
        ),
        "年轻兄弟的 AI 与 epoch 保持"
    );

    // 共享正文保留、引用不变。
    let (cnt, html_after, body_after): (i64, String, String) = conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM contents WHERE id = ?1),
                    (SELECT content_html FROM contents WHERE id = ?1),
                    (SELECT body_text FROM contents WHERE id = ?1)",
            [shared],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!((cnt, html_after, body_after), (1, html_before, body_before));
    assert_eq!(content_id_of(&conn, old_aid), Some(shared));
    assert_eq!(content_id_of(&conn, young_aid), Some(shared));
}

/* ============================================================
三·五、五布局读藏隔离（OPT-008C）：共享正文不传播读藏
============================================================ */

/// OPT-008C 必测 8：五种 layout 的 `is_read`/`is_starred` 各自独立可解释；
/// 布局过滤不吞掉其他布局条目；共享正文修订不改变未变化兄弟的读藏状态。
#[test]
fn five_layouts_keep_read_and_star_isolated_under_shared_content() {
    let tmp = TempDb::new("contents_five_layouts");
    let conn = tmp.open();
    let layouts = ["article", "social", "image", "podcast", "notification"];
    let mut ids = Vec::new();
    for layout in layouts {
        let folder = db::create_folder(&conn, layout, layout).unwrap();
        let feed = seed_feed(
            &conn,
            folder,
            &format!("https://{layout}.example/rss"),
            layout,
        );
        let (aid, _) = db::upsert_article_with_feed(
            &conn,
            feed,
            &new_article(
                &format!("g-{layout}"),
                &format!("https://{layout}.example/p"),
                layout,
                "<p>五布局共享正文</p>",
            ),
            false,
        )
        .unwrap();
        ids.push(aid);
    }
    let shared = content_id_of(&conn, ids[0]).unwrap();
    for id in &ids {
        assert_eq!(
            content_id_of(&conn, *id),
            Some(shared),
            "夹具：五布局条目必须共享同一内容行"
        );
    }
    assert_eq!(contents_count(&conn), 1);

    // 读藏矩阵：四种组合都在场，且逐条目独立。
    let want = [
        (true, false),
        (false, true),
        (true, true),
        (false, false),
        (true, false),
    ];
    for (id, (read, star)) in ids.iter().zip(want) {
        db::set_read(&conn, *id, read).unwrap();
        db::set_starred(&conn, *id, star).unwrap();
    }
    for (idx, (id, (read, star))) in ids.iter().zip(want).enumerate() {
        let row = db::get_article(&conn, *id).unwrap().unwrap();
        assert_eq!(
            (row.is_read, row.is_starred),
            (read, star),
            "布局 {} 的读藏必须独立可解释",
            layouts[idx]
        );
    }

    // 布局过滤：每个 layout 恰好看到自己的条目；五次查询合起来覆盖全部五条，
    // 不吞掉任何其他布局条目。
    let list_ids = |layout: &str| -> Vec<i64> {
        let q = db::ArticleQuery {
            feed_id: None,
            folder_id: None,
            only_unread: false,
            only_starred: false,
            only_today: false,
            newest_first: true,
            limit: 100,
            offset: 0,
            with_content: false,
            layout: Some(layout.to_string()),
            last_published: None,
            last_id: None,
        };
        let mut out: Vec<i64> = db::list_articles(&conn, &q)
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        out.sort_unstable();
        out
    };
    let mut seen = Vec::new();
    for (idx, layout) in layouts.iter().enumerate() {
        let got = list_ids(layout);
        assert_eq!(
            got,
            vec![ids[idx]],
            "layout={layout} 过滤必须只返回本布局条目（不吞其他布局）"
        );
        seen.extend(got);
    }
    seen.sort_unstable();
    assert_eq!(seen, ids, "五布局查询合计必须覆盖全部五条（无条目被吞）");

    // 共享正文修订：内容版本变化，但五个条目的读藏状态不得被传播/重置。
    conn.execute(
        "UPDATE contents SET content_html = content_html || '<p>共享尾部更新</p>',
             body_text = body_text || ' 共享尾部更新', revision = revision + 1 WHERE id = ?1",
        [shared],
    )
    .unwrap();
    for (idx, (id, (read, star))) in ids.iter().zip(want).enumerate() {
        let row = db::get_article(&conn, *id).unwrap().unwrap();
        assert_eq!(
            (row.is_read, row.is_starred),
            (read, star),
            "共享正文修订不得改变布局 {} 的读藏（未变化兄弟保持原状态）",
            layouts[idx]
        );
        assert!(row.content_revision >= 2, "修订应传到每个引用条目");
    }
    assert_eq!(contents_count(&conn), 1, "共享正文修订不得分叉内容行");

    // 布局过滤写路径同样隔离：social 仍为未读，podcast 也未读——按 social
    // 标读只翻转 social 一条。
    assert_eq!(
        db::mark_all_read(&conn, None, None, false, None, Some("social")).unwrap(),
        1
    );
    let social_row = db::get_article(&conn, ids[1]).unwrap().unwrap();
    assert!(
        social_row.is_read && social_row.is_starred,
        "social 标读不影响其收藏"
    );
    let podcast_row = db::get_article(&conn, ids[3]).unwrap().unwrap();
    assert!(
        !podcast_row.is_read && !podcast_row.is_starred,
        "其他布局（podcast）的读藏不得被 social 标读波及"
    );
}

#[test]
fn shared_content_update_reaches_every_referencing_entry_fts() {
    let tmp = TempDb::new("contents_shared_fts");
    let conn = tmp.open();
    let f = db::create_folder(&conn, "F", "article").unwrap();
    let a = seed_feed(&conn, f, "https://a.example/rss", "A");
    let b = seed_feed(&conn, f, "https://b.example/rss", "B");
    let (id1, _) = db::upsert_article_with_feed(
        &conn,
        a,
        &new_article("g1", "https://a.example/p", "甲", "<p>旧词 alpha</p>"),
        false,
    )
    .unwrap();
    let (id2, _) = db::upsert_article_with_feed(
        &conn,
        b,
        &new_article("g2", "https://b.example/p", "乙", "<p>旧词 alpha</p>"),
        false,
    )
    .unwrap();
    let shared = content_id_of(&conn, id1).unwrap();

    // 直接替换共享内容（模拟全量重拉/回填写入同一行）+ 修订推进
    conn.execute(
        "UPDATE contents SET content_html = '<p>新词 beta</p>', body_text = '新词 beta', revision = revision + 1 WHERE id = ?1",
        [shared],
    )
    .unwrap();
    assert!(
        search_ids(&conn, "alpha").is_empty(),
        "旧词对两个条目都消失"
    );
    assert_eq!(
        search_ids(&conn, "beta"),
        vec![id1, id2],
        "新词对每个引用条目都可搜（不只代表）"
    );

    // 修订暴露给前端：JOIN 得到的 revision 已递增
    let rev: i64 = conn
        .query_row(
            "SELECT c.revision FROM articles a JOIN contents c ON c.id = a.content_id WHERE a.id = ?1",
            [id1],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        rev >= 2,
        "内容替换推进修订（前端以 (content_id, revision) 失效）"
    );
}

/* ============================================================
五、LIKE 搜索经 contents：共享内容每个源可达（snippet 未变但正文变化）
============================================================ */

#[test]
fn like_search_reads_contents_and_covers_all_sharers() {
    let tmp = TempDb::new("contents_search");
    let conn = tmp.open();
    let f = db::create_folder(&conn, "F", "article").unwrap();
    let a = seed_feed(&conn, f, "https://a.example/rss", "A");
    let b = seed_feed(&conn, f, "https://b.example/rss", "B");
    let (id1, _) = db::upsert_article_with_feed(
        &conn,
        a,
        &new_article("g1", "https://a.example/p", "甲", "<p>正文里没有关键词</p>"),
        false,
    )
    .unwrap();
    let (id2, _) = db::upsert_article_with_feed(
        &conn,
        b,
        &new_article("g2", "https://b.example/p", "乙", "<p>正文里没有关键词</p>"),
        false,
    )
    .unwrap();
    let shared = content_id_of(&conn, id1).unwrap();

    // 正文后半变更（snippet/summary 不变）：共享行替换后，两个条目都能按新词搜到
    conn.execute(
        "UPDATE contents SET content_html = content_html || '<p>新增尾部标记 deltamarker</p>',
             body_text = '正文里没有关键词 新增尾部标记 deltamarker', revision = revision + 1 WHERE id = ?1",
        [shared],
    )
    .unwrap();
    let hits = db::search_articles(&conn, "deltamarker", 50).unwrap();
    let ids: Vec<i64> = hits.iter().map(|h| h.id).collect();
    assert!(
        ids.contains(&id1) && ids.contains(&id2),
        "共享正文新词对每个源可达：{ids:?}"
    );
    // snippet 仍是 summary/正文前缀（未变的部分）——但命中来自 contents JOIN
    let row = hits.iter().find(|h| h.id == id1).unwrap();
    assert_eq!(
        row.content_revision, 2,
        "搜索结果携带最新修订（前端失效依据）"
    );
}

/// 兜底：确认旧列在 v22 后不存在（防回归：任何遗留 SELECT 会在此表结构上失败）。
#[test]
fn articles_table_has_no_legacy_content_columns() {
    let tmp = TempDb::new("contents_schema");
    let conn = tmp.open();
    let legacy: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('articles')
             WHERE name IN ('content_html','body_text','fulltext_extracted')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(legacy, 0, "旧正文列不得保留（单一正文所有权）");
    let has_contents: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='contents'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(has_contents, 1);
    let _ = conn
        .query_row(
            "SELECT content_id, ai_epoch FROM articles LIMIT 1",
            [],
            |_r| Ok::<_, rusqlite::Error>(()),
        )
        .optional()
        .unwrap();
}

/* ============================================================
六、R3 故障原子性与 id 稳定性：重指/插入失败不留孤儿、contents.id 不复用
============================================================ */

/// R3（故障原子性）：全文提取重指失败 → 新提取行随 SAVEPOINT 整体回滚（无
/// 孤儿），条目/旧引用/FTS 保持原样；清障后同一前提可重试成功，且返回值携带
/// 与 DB 一致的实际内容身份/修订（wire 契约）。
#[test]
fn extract_commit_repoint_failure_is_atomic_and_returns_identity() {
    let tmp = TempDb::new("contents_extract_atomic");
    let conn = tmp.open();
    let f = db::create_folder(&conn, "F", "article").unwrap();
    let a = seed_feed(&conn, f, "https://a.example/rss", "A");
    let (aid, _) = db::upsert_article_with_feed(
        &conn,
        a,
        &new_article("g1", "https://a.example/p", "甲", "<p>RSS v1 摘要</p>"),
        false,
    )
    .unwrap();
    let cid_old = content_id_of(&conn, aid).unwrap();
    let (token, _original) = db::article_content_commit_token(&conn, aid)
        .unwrap()
        .unwrap();

    // 注入：条目重指 UPDATE 必失败（ensure_content 已插入新提取行之后）
    conn.execute_batch(
        "CREATE TRIGGER fail_extract_repoint BEFORE UPDATE OF content_id ON articles
         WHEN NEW.content_id IS NOT NULL AND NEW.content_id != OLD.content_id
         BEGIN SELECT RAISE(ABORT, 'injected extract repoint failure'); END;",
    )
    .unwrap();
    let err = db::replace_article_content_checked(
        &conn,
        aid,
        &token,
        "<p>提取全文（更完整的一大段正文，用于区分于原摘要且不含旧词）</p>",
    )
    .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("injected"), "SQL 失败原因可见：{msg}");
    assert_eq!(
        contents_count(&conn),
        1,
        "失败时新提取行随事务回滚（否则留下无引用的孤儿行）"
    );
    assert_eq!(content_id_of(&conn, aid), Some(cid_old), "条目仍指向旧内容");
    assert_eq!(
        search_ids(&conn, "RSS"),
        vec![aid],
        "旧引用搜索命中保持（回滚无损）"
    );
    conn.execute_batch("DROP TRIGGER fail_extract_repoint;")
        .unwrap();

    // 清障重试：前提未被改动（内容未变）→ 成功；返回身份与 DB 提交结果逐值一致
    let committed = db::replace_article_content_checked(
        &conn,
        aid,
        &token,
        "<p>提取全文（更完整的一大段正文，用于区分于原摘要且不含旧词）</p>",
    )
    .unwrap()
    .expect("清障后同一前提应提交成功");
    let (db_cid, db_rev): (i64, i64) = conn
        .query_row(
            "SELECT c.id, c.revision FROM articles a JOIN contents c ON c.id = a.content_id WHERE a.id = ?1",
            [aid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(committed, (db_cid, db_rev), "返回身份 = DB 提交结果");
    assert_ne!(db_cid, cid_old, "提取变体分叉新行（来源进身份）");
    assert_eq!(
        contents_count(&conn),
        2,
        "提取正文与 RSS 分组正文各有持久引用"
    );
    assert_eq!(
        search_ids(&conn, "RSS"),
        Vec::<i64>::new(),
        "旧词消失（重指后每个引用条目索引更新）"
    );
    // 后续 AI 输入以新版本为准：summary/translation 的正文读取经 contents JOIN
    // ——重指后同一入口读到提取正文（而非旧 RSS 摘要）。
    let (title, body, _cached, _sig) = db::get_article_for_summary(&conn, aid).unwrap().unwrap();
    assert_eq!(title, "甲");
    assert!(
        body.contains("提取全文"),
        "AI 输入读取提交后的新版本正文：{body}"
    );
}

/// R3（故障原子性）：RSS 刷新重指失败 → 新 RSS 行整体回滚（无孤儿）、条目保持
/// 旧指向、搜索索引不出现新词；清障后重试成功并回收旧行。
#[test]
fn refresh_repoint_failure_leaves_no_orphan_and_is_retryable() {
    let tmp = TempDb::new("contents_refresh_atomic");
    let conn = tmp.open();
    let f = db::create_folder(&conn, "F", "article").unwrap();
    let a = seed_feed(&conn, f, "https://a.example/rss", "A");
    let (aid, _) = db::upsert_article_with_feed(
        &conn,
        a,
        &new_article("g1", "https://a.example/p", "甲", "<p>版本一 alpha</p>"),
        false,
    )
    .unwrap();
    let cid_old = content_id_of(&conn, aid).unwrap();

    conn.execute_batch(
        "CREATE TRIGGER fail_refresh_repoint BEFORE UPDATE OF content_id ON articles
         WHEN NEW.content_id IS NOT NULL AND NEW.content_id != OLD.content_id
         BEGIN SELECT RAISE(ABORT, 'injected refresh repoint failure'); END;",
    )
    .unwrap();
    let err = db::upsert_article_with_feed(
        &conn,
        a,
        &new_article("g1", "https://a.example/p", "甲", "<p>版本二 beta</p>"),
        false,
    )
    .unwrap_err();
    assert!(format!("{err}").contains("injected"));
    assert_eq!(contents_count(&conn), 1, "新行随失败回滚（无孤儿）");
    assert_eq!(content_id_of(&conn, aid), Some(cid_old), "条目仍指旧行");
    assert_eq!(search_ids(&conn, "alpha"), vec![aid], "旧词仍可搜");
    assert!(
        search_ids(&conn, "beta").is_empty(),
        "失败的新内容不得进搜索索引"
    );
    conn.execute_batch("DROP TRIGGER fail_refresh_repoint;")
        .unwrap();

    let (id2, _) = db::upsert_article_with_feed(
        &conn,
        a,
        &new_article("g1", "https://a.example/p", "甲", "<p>版本二 beta</p>"),
        false,
    )
    .unwrap();
    assert_eq!(id2, aid, "重试命中同一条目");
    assert_ne!(content_id_of(&conn, aid), Some(cid_old), "重试成功重指新行");
    assert_eq!(contents_count(&conn), 1, "旧行被回收（最后引用）");
    assert_eq!(search_ids(&conn, "beta"), vec![aid], "新词可搜");
    assert!(search_ids(&conn, "alpha").is_empty(), "旧词消失");
}

/// R3（故障原子性）：新条目 INSERT 后 content_id 回填失败 → 整个插入（条目行 +
/// 内容行 + 搜索命中）随 SAVEPOINT 回滚，库中无半插入条目；清障后重试成功。
#[test]
fn insert_new_article_repoint_failure_is_atomic_and_retryable() {
    let tmp = TempDb::new("contents_insert_atomic");
    let conn = tmp.open();
    let f = db::create_folder(&conn, "F", "article").unwrap();
    let a = seed_feed(&conn, f, "https://a.example/rss", "A");

    conn.execute_batch(
        "CREATE TRIGGER fail_insert_repoint BEFORE UPDATE OF content_id ON articles
         WHEN NEW.content_id IS NOT NULL
         BEGIN SELECT RAISE(ABORT, 'injected insert repoint failure'); END;",
    )
    .unwrap();
    let err = db::upsert_article_with_feed(
        &conn,
        a,
        &new_article("g-new", "https://a.example/new", "新条目", "<p>新正文</p>"),
        false,
    )
    .unwrap_err();
    assert!(format!("{err}").contains("injected"));
    let articles: i64 = conn
        .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        articles, 0,
        "插入随失败整体回滚（不留 content_id NULL 的条目）"
    );
    assert_eq!(contents_count(&conn), 0, "私有内容行一并回滚");
    assert!(
        search_ids(&conn, "新正文").is_empty(),
        "搜索索引无半插入残留"
    );
    conn.execute_batch("DROP TRIGGER fail_insert_repoint;")
        .unwrap();

    let (aid, inserted) = db::upsert_article_with_feed(
        &conn,
        a,
        &new_article("g-new", "https://a.example/new", "新条目", "<p>新正文</p>"),
        false,
    )
    .unwrap();
    assert!(inserted, "重试成功插入");
    assert!(content_id_of(&conn, aid).is_some(), "content_id 已回填");
    assert_eq!(contents_count(&conn), 1);
    assert_eq!(search_ids(&conn, "新正文"), vec![aid], "重试后搜索可命中");
}

/// R3（id 稳定性）：`contents.id` 为 AUTOINCREMENT——删除表内最大内容行后，
/// 新内容不得复用同一 id（否则前端 (content_id, revision) 失效判据会在删除+
/// 重建窗口读到同值对）。覆盖新建库与同格式重开两条路径。
#[test]
fn contents_ids_are_not_reused_after_max_delete_and_reopen() {
    let tmp = TempDb::new("contents_autoincrement");
    {
        let conn = tmp.open();
        let sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='contents'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            sql.contains("AUTOINCREMENT"),
            "v22 contents.id 必须 AUTOINCREMENT（防 rowid 复用）：{sql}"
        );
        let f = db::create_folder(&conn, "F", "article").unwrap();
        let a = seed_feed(&conn, f, "https://a.example/rss", "A");
        let (id1, _) = db::upsert_article_with_feed(
            &conn,
            a,
            &new_article("g1", "https://a.example/1", "甲", "<p>甲内容</p>"),
            false,
        )
        .unwrap();
        let (id2, _) = db::upsert_article_with_feed(
            &conn,
            a,
            &new_article("g2", "https://a.example/2", "乙", "<p>乙内容</p>"),
            false,
        )
        .unwrap();
        let cid2 = content_id_of(&conn, id2).unwrap();
        // 删最大内容行的最后引用（条目删除触发回收）
        conn.execute("DELETE FROM articles WHERE id = ?1", [id2])
            .unwrap();
        let gone: i64 = conn
            .query_row("SELECT COUNT(*) FROM contents WHERE id = ?1", [cid2], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(gone, 0, "前置：最大内容行已回收");
        let (id3, _) = db::upsert_article_with_feed(
            &conn,
            a,
            &new_article("g3", "https://a.example/3", "丙", "<p>丙内容</p>"),
            false,
        )
        .unwrap();
        let cid3 = content_id_of(&conn, id3).unwrap();
        assert_ne!(cid3, cid2, "删除最大 id 后新内容不得复用该 id");
        assert!(cid3 > cid2, "新 id 严格大于已提交过的最大值");
        let _ = id1;
    }

    // 关闭重开同一当前格式，序号仍不得退回并复用已提交的内容身份。
    let conn = tmp.open();
    let previous_max: i64 = conn
        .query_row("SELECT MAX(id) FROM contents", [], |r| r.get(0))
        .unwrap();
    conn.execute("DELETE FROM articles", []).unwrap();
    assert_eq!(contents_count(&conn), 0);
    let feed: i64 = conn
        .query_row("SELECT id FROM feeds ORDER BY id LIMIT 1", [], |r| r.get(0))
        .unwrap();
    let (aid, _) = db::upsert_article_with_feed(
        &conn,
        feed,
        &new_article(
            "reopened",
            "https://a.example/reopened",
            "重开后",
            "<p>重开后的正文</p>",
        ),
        false,
    )
    .unwrap();
    assert!(content_id_of(&conn, aid).unwrap() > previous_max);
}

/* ============================================================
七、R3 删除路径：分类级联 / 账号断开（purge_remote_data）的共享引用生命周期
============================================================ */

/// R3：`delete_folder` 级联（folders → feeds → articles → 触发器回收）下，
/// 共享内容“删一个分类的引用仍保活、删最后引用才回收”。
#[test]
fn delete_folder_cascade_recycles_shared_contents_at_last_reference() {
    let tmp = TempDb::new("contents_folder_cascade");
    let conn = tmp.open();
    let f1 = db::create_folder(&conn, "F1", "article").unwrap();
    let f2 = db::create_folder(&conn, "F2", "article").unwrap();
    let a = seed_feed(&conn, f1, "https://a.example/rss", "A");
    let b = seed_feed(&conn, f2, "https://b.example/rss", "B");
    let (id1, _) = db::upsert_article_with_feed(
        &conn,
        a,
        &new_article("g1", "https://a.example/p", "甲", "<p>分类共享</p>"),
        false,
    )
    .unwrap();
    let (id2, _) = db::upsert_article_with_feed(
        &conn,
        b,
        &new_article("g2", "https://b.example/p", "乙", "<p>分类共享</p>"),
        false,
    )
    .unwrap();
    let shared = content_id_of(&conn, id1).unwrap();
    assert_eq!(content_id_of(&conn, id2), Some(shared));

    db::delete_folder(&conn, f1).unwrap();
    assert!(
        db::get_article(&conn, id1).unwrap().is_none(),
        "分类级联删源/条目"
    );
    assert_eq!(
        content_id_of(&conn, id2),
        Some(shared),
        "仍被另一分类条目引用：共享内容保活"
    );
    assert_eq!(
        search_ids(&conn, "分类共享"),
        vec![id2],
        "级联删除的条目命中消失、存留条目仍可搜"
    );

    db::delete_folder(&conn, f2).unwrap();
    assert_eq!(contents_count(&conn), 0, "最后引用随分类级联消失才回收");
    assert!(search_ids(&conn, "分类共享").is_empty());
}

/// R3：账号断开（`purge_remote_data`）删除服务端来源订阅的条目，但共享内容仍
/// 被本地直连条目引用时保活；删最后引用才回收（含搜索索引）。
#[test]
fn purge_remote_data_keeps_shared_content_until_last_reference() {
    let tmp = TempDb::new("contents_purge_remote");
    let mut conn = tmp.open();
    let f = db::create_folder(&conn, "F", "article").unwrap();
    // 服务端来源源（账号断开时清除）与本地直连源共享同一内容行
    let remote = db::insert_feed_origin(
        &conn,
        "https://remote.example/rss",
        None,
        "远端源",
        None,
        f,
        "inherit",
        false,
        false,
        "remote",
    )
    .unwrap();
    let local = seed_feed(&conn, f, "https://local.example/rss", "本地源");
    let (id_remote, _) = db::upsert_article_with_feed(
        &conn,
        remote,
        &new_article(
            "gr",
            "https://remote.example/p",
            "远端条目",
            "<p>账号共享</p>",
        ),
        false,
    )
    .unwrap();
    let (id_local, _) = db::upsert_article_with_feed(
        &conn,
        local,
        &new_article(
            "gl",
            "https://local.example/p",
            "本地条目",
            "<p>账号共享</p>",
        ),
        false,
    )
    .unwrap();
    let shared = content_id_of(&conn, id_remote).unwrap();
    assert_eq!(content_id_of(&conn, id_local), Some(shared));

    let (feeds_deleted, _articles_updated) = db::purge_remote_data(&mut conn).unwrap();
    assert!(feeds_deleted >= 1, "服务端来源订阅被清除");
    assert!(
        db::get_article(&conn, id_remote).unwrap().is_none(),
        "账号断开删除服务端来源条目"
    );
    assert_eq!(
        content_id_of(&conn, id_local),
        Some(shared),
        "本地条目仍引用：共享内容保活（不随账号断开误删）"
    );
    assert_eq!(
        search_ids(&conn, "账号共享"),
        vec![id_local],
        "存留条目仍可搜"
    );

    db::delete_feed(&conn, local).unwrap();
    assert_eq!(contents_count(&conn), 0, "最后引用消失才回收 contents");
    assert!(search_ids(&conn, "账号共享").is_empty(), "命中一并移除");
}
