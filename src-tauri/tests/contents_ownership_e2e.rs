//! OPT-008B 第二阶段：contents 单一正文所有权的端到端回归（真实迁移库）。
//!
//! 覆盖卡末尾必测：等价共享 / 不同内容不共享 / 空内容不聚合 / 单引用修订分叉 /
//! 删除一个源 vs 删除最后引用 / 正文后半变更（snippet 相同但修订/内容变化）/
//! FTS 派生索引随共享内容更新（每个引用条目可搜、旧词消失）/ 异步提交守卫
//! （过期前提拒绝落库、清理代次拒绝复活）/ 旧库（v21 形状）迁移无损与回滚重试。
//! 使用隔离临时库；不访问网络/用户库。

use app_lib::db;
use rusqlite::{Connection, OptionalExtension};
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
    fn path(&self) -> &std::path::Path {
        &self.path
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
    // 测试只需 body_text 与 html 一致以便 FTS/snippet 断言）。
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

fn fts_rowids(conn: &Connection, term: &str) -> Vec<i64> {
    let mut stmt = conn
        .prepare("SELECT rowid FROM articles_fts WHERE articles_fts MATCH ?1 ORDER BY rowid")
        .unwrap();
    stmt.query_map([term], |r| r.get(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
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
    // guid 不同但同 URL：URL 兜底命中同一行 → 再次替换 → 旧行回收，仍只一行
    let (id3, _) = db::upsert_article_with_feed(
        &conn,
        a,
        &new_article("g-other", "https://a.example/post/x", "甲", "<p>版本三</p>"),
        false,
    )
    .unwrap();
    assert_eq!(id3, id1, "同 feed 同规范化 URL 兜底命中既有条目");
    assert_ne!(
        content_id_of(&conn, id3),
        Some(cid_first),
        "再次不同字节 → 又一行新内容"
    );
    assert_eq!(contents_count(&conn), 1, "旧行持续回收");
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
        fts_rowids(&conn, "提取全文"),
        vec![id1],
        "提取正文仅条目 1 可搜"
    );
    assert_eq!(
        fts_rowids(&conn, "RSS"),
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

    // 正常路径：前提相符 → 写入成功
    let token = db::article_ai_commit_token(&conn, aid).unwrap().unwrap();
    assert!(db::set_article_ai_fields_checked(&conn, aid, &token, Some("摘要 v1"), None).unwrap());

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
        !db::set_article_ai_fields_checked(&conn, aid, &stale, Some("过期摘要"), None).unwrap(),
        "内容已替换：旧前提的迟到 AI 提交必须拒绝"
    );
    let summary: Option<String> = conn
        .query_row(
            "SELECT ai_summary FROM articles WHERE id = ?1",
            [aid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(summary.as_deref(), Some("摘要 v1"), "拒绝不覆盖既有产物");

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
        !db::set_article_ai_fields_checked(&conn2, aid, &pre_clean_token, Some("复活摘要"), None)
            .unwrap(),
        "清理后的迟到提交必须被拒（不得复活已清产物）"
    );
    let summary2: Option<String> = conn2
        .query_row(
            "SELECT ai_summary FROM articles WHERE id = ?1",
            [aid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(summary2, None, "清理保持：无 AI 产物");

    // 条目被删除后：迟到提交同样拒绝（不复活已删条目）
    conn2
        .execute("DELETE FROM articles WHERE id = ?1", [aid])
        .unwrap();
    assert!(
        !db::set_article_ai_fields_checked(&conn2, aid, &pre_clean_token, Some("复活摘要2"), None)
            .unwrap(),
        "已删除条目的迟到 AI 提交必须拒绝"
    );
}

/* ============================================================
三、引用生命周期：删源/删最后引用、FTS 同步
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
        fts_rowids(&conn, "共享"),
        vec![id1, id2],
        "共享正文两个条目都可搜"
    );

    // 删源 A：级联删条目 1；共享内容仍被条目 2 引用 → 保活；FTS 行 1 消失、行 2 保留
    db::delete_feed(&conn, a).unwrap();
    assert!(db::get_article(&conn, id1).unwrap().is_none());
    assert_eq!(
        content_id_of(&conn, id2),
        Some(shared),
        "共享内容不被误删（仍有引用）"
    );
    assert_eq!(
        fts_rowids(&conn, "共享"),
        vec![id2],
        "删除源 A 后其条目不可搜、条目 2 仍可搜"
    );

    // 删最后引用：内容与 FTS 行一并回收
    db::delete_feed(&conn, b).unwrap();
    assert_eq!(contents_count(&conn), 0, "最后引用消失才回收 contents");
    assert!(
        fts_rowids(&conn, "共享").is_empty(),
        "无引用条目：FTS 行一并移除"
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
        fts_rowids(&conn, "共享清理").is_empty(),
        "FTS 派生行一并移除"
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
        fts_rowids(&conn, "alpha").is_empty(),
        "旧词对两个条目都消失"
    );
    assert_eq!(
        fts_rowids(&conn, "beta"),
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
四、v21 → v22 迁移：无损 + 失败整体回滚可重试
============================================================ */

/// 造「v21 存量现场」：用生产 open 建最新库，物理回拨 v22（contents 迁出 +
/// FTS 派生表 + content_id/ai_epoch 列）与 v23（subscription_outbox 表 + 两索引）
/// 并把 user_version 拨回 21；随后以
/// **v21 时代 SQL** 直插旧行（旧正文列 + AI 产物 + 读藏/版本/远端绑定 + 队列），
/// 使下一次 open 走真实 v22/v23 升级路径。
fn seed_v21_legacy(path: &std::path::Path) -> (i64, i64) {
    let conn = db::open(path).expect("open latest");
    let f = db::create_folder(&conn, "F", "article").unwrap();
    let feed = seed_feed(&conn, f, "https://legacy.example/rss", "旧源");
    conn.execute_batch(
        r#"
        DROP TRIGGER IF EXISTS trg_articles_fts_ai;
        DROP TRIGGER IF EXISTS trg_articles_fts_ad;
        DROP TRIGGER IF EXISTS trg_articles_fts_au;
        DROP TRIGGER IF EXISTS trg_contents_fts_au;
        DROP TABLE IF EXISTS articles_fts;
        DROP INDEX IF EXISTS idx_articles_content;
        ALTER TABLE articles DROP COLUMN content_id;
        ALTER TABLE articles DROP COLUMN ai_epoch;
        ALTER TABLE articles ADD COLUMN content_html TEXT;
        ALTER TABLE articles ADD COLUMN body_text TEXT NOT NULL DEFAULT '';
        ALTER TABLE articles ADD COLUMN fulltext_extracted INTEGER NOT NULL DEFAULT 0;
        DROP TABLE IF EXISTS contents;
        -- OPT-009 v23 物理回拨：类型化订阅 outbox（表 + 两索引）在 v21 现场不得
        -- 残留，否则重放 v23 时 CREATE TABLE 报 already exists（表删除即带走索引，
        -- DROP INDEX 为显式清点）。此后新增建表迁移须在此追加对应回拨。
        DROP INDEX IF EXISTS idx_subscription_outbox_url;
        DROP INDEX IF EXISTS idx_subscription_outbox_pending;
        DROP TABLE IF EXISTS subscription_outbox;
        PRAGMA user_version = 21;
        "#,
    )
    .unwrap();
    // v21 时代现场：旧正文列 + AI + 状态/版本 + 远端绑定直插。
    conn.execute(
        "INSERT INTO articles (feed_id, guid, url, url_norm, title, content_html, body_text,
             ai_summary, translated_content, source, published_at, is_read, is_starred,
             remote_id, read_version, star_version)
         VALUES (?1, 'lg1', 'https://legacy.example/a', 'https://legacy.example/a', '旧标题',
             '<p>旧正文</p>', '旧正文', '旧摘要', '<p>旧译文</p>', 'direct', '2026-01-01T00:00:00+00:00',
             1, 1, 77, 3, 2)",
        [feed],
    )
    .unwrap();
    let aid = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO sync_queue (article_id, action) VALUES (?1, 'star')",
        [aid],
    )
    .unwrap();
    drop(conn);
    (aid, feed)
}

#[test]
fn v21_upgrade_is_lossless_and_idempotent() {
    let tmp = TempDb::new("contents_migration");
    let (aid, _feed) = seed_v21_legacy(tmp.path());

    let conn = tmp.open();
    // 分组读取（避免 9 元组越过 clippy::type_complexity 阈值）
    let (read, star, rv, sv): (i64, i64, i64, i64) = conn
        .query_row(
            "SELECT is_read, is_starred, read_version, star_version FROM articles WHERE id = ?1",
            [aid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    let (remote, title, guid): (Option<i64>, String, String) = conn
        .query_row(
            "SELECT remote_id, title, guid FROM articles WHERE id = ?1",
            [aid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    let (summary, translated): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT ai_summary, translated_content FROM articles WHERE id = ?1",
            [aid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((read, star), (1, 1), "读藏状态保 ID 无损");
    assert_eq!(remote, Some(77), "远端绑定保持");
    assert_eq!(
        summary.as_deref(),
        Some("旧摘要"),
        "旧 AI 摘要保留（迁移不丢）"
    );
    assert_eq!(translated.as_deref(), Some("<p>旧译文</p>"), "旧译文保留");
    assert_eq!((rv, sv), (3, 2), "读藏版本保持");
    assert_eq!(
        (title.as_str(), guid.as_str()),
        ("旧标题", "lg1"),
        "身份字段保持"
    );
    let qn: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sync_queue WHERE article_id = ?1",
            [aid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(qn, 1, "队列不变");
    // 正文迁入 contents 且旧列消失
    let (html, body): (String, String) = conn
        .query_row(
            "SELECT c.content_html, c.body_text FROM articles a JOIN contents c ON c.id = a.content_id WHERE a.id = ?1",
            [aid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((html.as_str(), body.as_str()), ("<p>旧正文</p>", "旧正文"));
    let old_cols: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('articles') WHERE name IN ('content_html','body_text','fulltext_extracted')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(old_cols, 0, "旧正文列必须消除（不保两套真值）");
    // FTS 派生索引已重建：旧内容可搜
    assert_eq!(fts_rowids(&conn, "旧正文"), vec![aid]);
    // 幂等重开
    drop(conn);
    let conn2 = tmp.open();
    let n: i64 = conn2
        .query_row("SELECT COUNT(*) FROM contents", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1, "重复 open 幂等：不重复迁移/不复制内容行");
}

#[test]
fn v21_upgrade_failure_rolls_back_and_retries() {
    // 在 v22 迁移路径上制造失败：预建一张与 contents 同名的**残留**表会以
    // 「table contents already exists」使 v22 SQL 段失败——验证迁移事务整体回滚
    // （user_version 不推进、旧形状与数据原样），清障后重开成功。
    let tmp = TempDb::new("contents_migration_fail");
    let (aid, _feed) = seed_v21_legacy(tmp.path());
    {
        let conn = Connection::open(tmp.path()).unwrap();
        conn.execute_batch("CREATE TABLE contents (id INTEGER PRIMARY KEY);")
            .unwrap();
    }
    let err = db::open(tmp.path()).expect_err("迁移必须失败（残留表冲突）");
    let msg = format!("{err}");
    assert!(msg.contains("contents"), "失败原因可见：{msg}");
    {
        let conn = Connection::open(tmp.path()).unwrap();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 21, "失败迁移不得推进版本（整体回滚）");
        // 旧形状仍在（v21 列回归原位），数据无损
        let html: String = conn
            .query_row(
                "SELECT COALESCE(content_html, '') FROM articles WHERE id = ?1",
                [aid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(html, "<p>旧正文</p>", "回滚后旧正文列数据原样");
        let cid_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('articles') WHERE name = 'content_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cid_exists, 0, "v22 增列随事务回滚");
        conn.execute_batch("DROP TABLE contents;").unwrap();
    }
    // 清障后重开：正常升级
    let conn = tmp.open();
    let v: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert!(v >= 22, "重试后升级完成（user_version={v}）");
    let body: String = conn
        .query_row(
            "SELECT c.body_text FROM articles a JOIN contents c ON c.id = a.content_id WHERE a.id = ?1",
            [aid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(body, "旧正文");
}

#[test]
fn v21_upgrade_midway_failure_rolls_back_and_retries() {
    // **中途**失败注入：旧正文列被一个既有索引引用时 DROP COLUMN 必须失败——
    // 该失败发生在回填（contents 行已建、articles.content_id 已写）之后，用于
    // 验证「回填出的中间态随事务整体回滚」而不只是首个 SQL 段失败。
    let tmp = TempDb::new("contents_migration_midfail");
    let (aid, _feed) = seed_v21_legacy(tmp.path());
    {
        let conn = Connection::open(tmp.path()).unwrap();
        conn.execute_batch("CREATE INDEX legacy_content_probe ON articles(content_html);")
            .unwrap();
    }
    let err = db::open(tmp.path()).expect_err("迁移必须在中途失败（旧列被索引引用）");
    let msg = format!("{err}");
    assert!(
        msg.contains("content_html") || msg.to_lowercase().contains("index"),
        "失败原因应指向旧正文列/索引：{msg}"
    );
    {
        let conn = Connection::open(tmp.path()).unwrap();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 21, "中途失败不得推进版本（整体回滚）");
        let contents_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='contents'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            contents_exists, 0,
            "回填出的 contents 表随事务回滚（无半迁移残留）"
        );
        let new_cols: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('articles') WHERE name IN ('content_id','ai_epoch')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(new_cols, 0, "v22 增列随事务回滚");
        let html: String = conn
            .query_row(
                "SELECT COALESCE(content_html, '') FROM articles WHERE id = ?1",
                [aid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(html, "<p>旧正文</p>", "回滚后旧正文列数据原样");
        conn.execute_batch("DROP INDEX legacy_content_probe;")
            .unwrap();
    }
    // 清障后重开：正常升级，正文迁入 contents
    let conn = tmp.open();
    let v: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert!(v >= 22, "重试后升级完成（user_version={v}）");
    let body: String = conn
        .query_row(
            "SELECT c.body_text FROM articles a JOIN contents c ON c.id = a.content_id WHERE a.id = ?1",
            [aid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(body, "旧正文");
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
        fts_rowids(&conn, "RSS"),
        vec![aid],
        "旧引用 FTS 行保持（回滚无损）"
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
    assert_eq!(contents_count(&conn), 1, "旧行无引用即回收");
    assert_eq!(
        fts_rowids(&conn, "RSS"),
        Vec::<i64>::new(),
        "旧词消失（重指后每个引用条目索引更新）"
    );
    // 后续 AI 输入以新版本为准：summary/translation 的正文读取经 contents JOIN
    // ——重指后同一入口读到提取正文（而非旧 RSS 摘要）。
    let (title, body, _cached) = db::get_article_for_summary(&conn, aid).unwrap().unwrap();
    assert_eq!(title, "甲");
    assert!(
        body.contains("提取全文"),
        "AI 输入读取提交后的新版本正文：{body}"
    );
}

/// R3（故障原子性）：RSS 刷新重指失败 → 新 RSS 行整体回滚（无孤儿）、条目保持
/// 旧指向、FTS 不出现新词；清障后重试成功并回收旧行。
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
    assert_eq!(fts_rowids(&conn, "alpha"), vec![aid], "旧词仍可搜");
    assert!(
        fts_rowids(&conn, "beta").is_empty(),
        "失败的新内容不得进 FTS"
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
    assert_eq!(fts_rowids(&conn, "beta"), vec![aid], "新词可搜");
    assert!(fts_rowids(&conn, "alpha").is_empty(), "旧词消失");
}

/// R3（故障原子性）：新条目 INSERT 后 content_id 回填失败 → 整个插入（条目行 +
/// 内容行 + FTS 行）随 SAVEPOINT 回滚，库中无半插入条目；清障后重试成功。
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
    assert!(fts_rowids(&conn, "新正文").is_empty(), "FTS 无半插入残留");
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
    assert_eq!(fts_rowids(&conn, "新正文"), vec![aid], "重试后 FTS 可搜");
}

/// R3（id 稳定性）：`contents.id` 为 AUTOINCREMENT——删除表内最大内容行后，
/// 新内容不得复用同一 id（否则前端 (content_id, revision) 失效判据会在删除+
/// 重建窗口读到同值对）。覆盖新建库与 v21 迁移库两条路径。
#[test]
fn contents_ids_are_not_reused_after_max_delete_and_after_migration() {
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

    // v21 迁移路径：升级完成后同样保证（迁移回填也经 AUTOINCREMENT 表）
    let tmp2 = TempDb::new("contents_autoincrement_migrated");
    let (aid, _feed) = seed_v21_legacy(tmp2.path());
    let conn = tmp2.open();
    let migrated_cid = content_id_of(&conn, aid).unwrap();
    conn.execute("DELETE FROM articles WHERE id = ?1", [aid])
        .unwrap();
    assert_eq!(contents_count(&conn), 0, "旧行随最后引用回收");
    let f = db::create_folder(&conn, "F2", "article").unwrap();
    let a2 = seed_feed(&conn, f, "https://after.example/rss", "迁移后");
    let (aid2, _) = db::upsert_article_with_feed(
        &conn,
        a2,
        &new_article(
            "gm",
            "https://after.example/1",
            "迁移后条目",
            "<p>迁移后内容</p>",
        ),
        false,
    )
    .unwrap();
    let cid_after = content_id_of(&conn, aid2).unwrap();
    assert!(
        cid_after > migrated_cid,
        "迁移库同一保证：新内容 id 不得复用已删除的最大 id（{cid_after} vs {migrated_cid}）"
    );
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
        fts_rowids(&conn, "分类共享"),
        vec![id2],
        "级联删除的条目 FTS 行消失、存留条目仍可搜"
    );

    db::delete_folder(&conn, f2).unwrap();
    assert_eq!(contents_count(&conn), 0, "最后引用随分类级联消失才回收");
    assert!(fts_rowids(&conn, "分类共享").is_empty());
}

/// R3：账号断开（`purge_remote_data`）删除服务端来源订阅的条目，但共享内容仍
/// 被本地直连条目引用时保活；删最后引用才回收（含 FTS）。
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
        fts_rowids(&conn, "账号共享"),
        vec![id_local],
        "存留条目 FTS 仍可搜"
    );

    db::delete_feed(&conn, local).unwrap();
    assert_eq!(contents_count(&conn), 0, "最后引用消失才回收 contents");
    assert!(fts_rowids(&conn, "账号共享").is_empty(), "FTS 行一并移除");
}
