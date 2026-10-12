use super::*;

fn new_article(url: &str, guid: &str) -> NewArticle {
    NewArticle {
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
    }
}

/// OPT-008B 第一阶段迁移：智能去重不再在入库丢身份——跨源同 URL 各自独立入库；
/// 同源同 URL（不同 guid）仍命中同一行（更新）；开关值不再影响入库结果。
#[test]
fn smart_dedup_no_longer_drops_cross_feed_identity() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    let f1 = create_folder(&conn, "A", "article").unwrap();
    let f2 = create_folder(&conn, "B", "article").unwrap();
    let feed1 = insert_feed(
        &conn,
        "https://x.example/f1",
        None,
        "f1",
        None,
        f1,
        "inherit",
        true,
        false,
    )
    .unwrap();
    let feed2 = insert_feed(
        &conn,
        "https://x.example/f2",
        None,
        "f2",
        None,
        f2,
        "inherit",
        true,
        false,
    )
    .unwrap();

    // feed1 首个入库
    let (id1, new1) = upsert_article_with_feed(
        &conn,
        feed1,
        &new_article("https://n.example/a", "g1"),
        true,
    )
    .unwrap();
    assert!(new1);

    // feed2 推来同 URL（不同 guid）→ 独立入库（旧语义「被 dedup 拦截」已迁移：
    // 智能去重改为显示层聚合，条目身份保留）
    let (id2, new2) = upsert_article_with_feed(
        &conn,
        feed2,
        &new_article("https://n.example/a", "g2"),
        true,
    )
    .unwrap();
    assert!(new2, "跨源同 URL 必须独立入库（身份保留）");
    assert_ne!(id1, id2, "两条不同持久条目");

    // feed2 不同 URL → 正常入库
    let (_, new3) = upsert_article_with_feed(
        &conn,
        feed2,
        &new_article("https://n.example/b", "g3"),
        true,
    )
    .unwrap();
    assert!(new3);

    // 同源同 URL 重放（不同 guid）→ 命中同一行（源内稳定身份）
    let (same1, new4) = upsert_article_with_feed(
        &conn,
        feed1,
        &new_article("https://n.example/a", "g4"),
        false,
    )
    .unwrap();
    assert_eq!(same1, id1, "同源同 URL 是同一行");
    assert!(!new4, "同源重放不新增行");

    // 开关值不再改变入库结果：同源同 URL（即使新 guid）仍是源内同一行——
    // 「同一订阅源内 URL 稳定」是身份规则，与智能去重开关无关。
    let (again2, new5) = upsert_article_with_feed(
        &conn,
        feed2,
        &new_article("https://n.example/a", "g5"),
        false,
    )
    .unwrap();
    assert_eq!(again2, id2, "同源同 URL 始终命中同一行（稳定身份）");
    assert!(!new5);

    let _ = (f1, f2);
}
/// OPT-008A（F12）保守规则经真实入库路径：明确归属的营销参数变体仍被去重
/// 拦截（真去重不退化）；论坛主题 t、通用 s、章节/分组 ID 等业务参数不再互相
/// 吞并；X 状态页例外只对确切域名生效。
#[test]
fn conservative_url_rules_keep_true_dedup_and_separate_business_params() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    let f = create_folder(&conn, "F", "article").unwrap();
    let feed = insert_feed(
        &conn,
        "https://x.example/f",
        None,
        "f",
        None,
        f,
        "inherit",
        true,
        false,
    )
    .unwrap();
    let up = |url: &str, guid: &str| {
        upsert_article_with_feed(&conn, feed, &new_article(url, guid), true).unwrap()
    };

    // 明确归属的营销参数变体 → 同一篇，后续被拦
    let (_, n1) = up("https://n.example/post", "g1");
    assert!(n1);
    let (_, n2) = up("https://n.example/post?utm_source=rss&fbclid=abc", "g2");
    assert!(!n2, "utm/fbclid 变体必须仍被去重");
    let (_, n3) = up("https://n.example/post?mc_cid=1", "g3");
    assert!(!n3, "mc_cid 变体必须仍被去重");

    // 论坛主题号（F12 核心反例）：t=123 与 t=456 必须分别入库
    let (_, n4) = up("https://forum.example/viewtopic.php?t=123", "g4");
    assert!(n4);
    let (_, n5) = up("https://forum.example/viewtopic.php?t=456", "g5");
    assert!(n5, "不同主题号不得被去重吞并");
    let (_, n6) = up("https://forum.example/viewtopic.php?s=foo", "g6");
    assert!(n6);
    let (_, n7) = up("https://forum.example/viewtopic.php?s=bar", "g7");
    assert!(n7, "普通站点上通用 s 参数是业务参数，不得吞并");
    // 章节/分组业务 ID
    let (_, n8) = up("https://novel.example/read?web_chapter_id=1", "g8");
    assert!(n8);
    let (_, n9) = up("https://novel.example/read?web_chapter_id=2", "g9");
    assert!(n9, "不同章节 ID 不得被去重吞并");
    let (_, n10) = up("https://novel.example/read?group_id=1", "g10");
    assert!(n10);
    let (_, n11) = up("https://novel.example/read?group_id=2", "g11");
    assert!(n11, "不同 group_id 不得被去重吞并");

    // X 状态页：允许的跟踪变体仍是同一篇
    let (_, n12) = up("https://x.com/u/status/123", "g12");
    assert!(n12);
    let (_, n13) = up("https://x.com/u/status/123?s=20&t=abc", "g13");
    assert!(!n13, "X 状态页的 t/s 变体必须仍被去重");

    // 伪域名不具备 X 状态页例外
    let (_, n14) = up("https://x.com.evil/u/status/123", "g14");
    assert!(n14);
    let (_, n15) = up("https://x.com.evil/u/status/123?s=20", "g15");
    assert!(n15, "伪域名不得套 X 状态页例外");
}

/// 搜索（LIKE 子串）：中文子串、多词 AND、通配符转义。
/// 修复背景：unicode61 FTS 把整段中文当一个 token，搜「科技」匹配不到
/// 「科技公司新闻」——改为子串匹配后语义对任意语言正确。
#[test]
fn search_finds_chinese_substring_and_multi_term_and() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    let f = create_folder(&conn, "F", "article").unwrap();
    let feed = insert_feed(
        &conn,
        "https://x.example/f",
        None,
        "f",
        None,
        f,
        "inherit",
        true,
        false,
    )
    .unwrap();

    let art = |url: &str, guid: &str, title: &str, body: &str| {
        let mut a = new_article(url, guid);
        a.title = title.into();
        a.body_text = body.into();
        let _ = upsert_article_with_feed(&conn, feed, &a, false).unwrap();
    };
    art(
        "https://n.example/1",
        "g1",
        "科技公司新闻",
        "今天发布了新产品",
    );
    art(
        "https://n.example/2",
        "g2",
        "无关标题",
        "正文提到了科技公司",
    );
    art("https://n.example/3", "g3", "另一个", "完全没有相关内容");

    // 中文子串：标题或正文含「科技」都命中（FTS 时代这条是失败的）
    let r1 = search_articles(&conn, "科技", 50).unwrap();
    assert_eq!(
        r1.len(),
        2,
        "chinese substring must match both: {:?}",
        r1.iter().map(|a| &a.title).collect::<Vec<_>>()
    );

    // 多词 AND：两个词都命中才返回
    let r2 = search_articles(&conn, "科技 产品", 50).unwrap();
    assert_eq!(
        r2.len(),
        1,
        "AND semantics: {:?}",
        r2.iter().map(|a| &a.title).collect::<Vec<_>>()
    );
    assert_eq!(r2[0].title, "科技公司新闻");

    // 无命中
    let r3 = search_articles(&conn, "不存在的词", 50).unwrap();
    assert!(r3.is_empty());

    // LIKE 通配符按字面转义：存 % 和 _ 的标题不被 % 通配误命中
    art("https://n.example/4", "g4", "100%_安全", "特殊字符");
    let r4 = search_articles(&conn, "100%", 50).unwrap();
    assert_eq!(r4.len(), 1, "literal %% must match: {}", r4.len());
    assert_eq!(r4[0].title, "100%_安全");
    // 单下划线词不当作通配符命中任意单字符
    let r5 = search_articles(&conn, "100X_安全", 50).unwrap();
    assert!(r5.is_empty(), "underscore must be literal, not wildcard");

    // 含 FTS 特殊字符的词安全
    art(
        "https://n.example/5",
        "g5",
        "node.js 指南",
        "C++ 与 Rust 对比",
    );
    let r6 = search_articles(&conn, "node.js", 50).unwrap();
    assert_eq!(r6.len(), 1);
    let r7 = search_articles(&conn, "C++", 50).unwrap();
    assert_eq!(r7.len(), 1);
}

/// 搜索命中 AI 摘要/翻译（SRH-2）：正文/标题都不含关键词、仅 AI 产物含时也要命中。
#[test]
fn search_finds_ai_summary_and_translation() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    let f = create_folder(&conn, "F", "article").unwrap();
    let feed = insert_feed(
        &conn,
        "https://x.example/f",
        None,
        "f",
        None,
        f,
        "inherit",
        true,
        false,
    )
    .unwrap();

    let mut a = new_article("https://n.example/1", "g1");
    a.title = "普通标题".into();
    a.body_text = "正文不含关键词".into();
    let (id, _) = upsert_article_with_feed(&conn, feed, &a, false).unwrap();

    // 仅摘要含关键词
    set_article_ai_fields(&conn, id, Some("这篇讲的是量子计算的突破"), None).unwrap();
    let r1 = search_articles(&conn, "量子计算", 50).unwrap();
    assert_eq!(r1.len(), 1, "ai_summary 命中");

    // 仅译文含关键词
    set_article_ai_fields(&conn, id, None, Some("译文里提到了深度学习模型")).unwrap();
    let r2 = search_articles(&conn, "深度学习", 50).unwrap();
    assert_eq!(r2.len(), 1, "translated_content 命中");

    // 无关词不命中
    let r3 = search_articles(&conn, "不存在的词", 50).unwrap();
    assert!(r3.is_empty());
}

/// feed_id_by_url_normalized：Miniflux 返回的 feed_url 与本地直连添加时
/// 有协议/www./尾斜杠/跟踪参数差异时，必须规范化为同一 feed（否则同一
/// 订阅出现两个本地 feed → 文章翻倍、状态分裂、数量对不齐）。
#[test]
fn feed_url_normalized_matching_collides_differently_decorated_urls() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    let f = create_folder(&conn, "F", "article").unwrap();
    // 本地直连添加：https + www + 尾斜杠 + 跟踪参数
    let fid = insert_feed(
        &conn,
        "https://www.example.com/feed/?utm_source=x",
        None,
        "f",
        None,
        f,
        "inherit",
        false,
        false,
    )
    .unwrap();

    // Miniflux 返回：http + 无 www + 无尾斜杠 + 无跟踪参数 → 必须匹配到同一 feed
    let matched = feed_id_by_url_normalized(&conn, "http://example.com/feed").unwrap();
    assert_eq!(
        matched,
        Some(fid),
        "规范化后不同饰的 feed_url 必须匹配同一本地 feed"
    );

    // 精确匹配（旧函数）对这种情况会漏判——保持旧函数不变，仅新函数规范化
    let exact = feed_id_by_url(&conn, "http://example.com/feed").unwrap();
    assert!(
        exact.is_none(),
        "精确匹配对规范化差异应返回 None（这正是修复前漏判的根因）"
    );
}

/// article_index 与 list_articles 位置对齐：某篇文章的绝对位置 = list_articles
/// 用该 offset 拉取时的第一条。验证双向分页锚定（搜索/深层打开文章）的正确性。
#[test]
fn article_index_positions_align_with_list() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    let f = create_folder(&conn, "F", "article").unwrap();
    let feed = insert_feed(
        &conn,
        "https://x.example/f",
        None,
        "f",
        None,
        f,
        "inherit",
        true,
        false,
    )
    .unwrap();

    // 插入 5 篇，published_at 递增（最新在最前，newest_first=true）
    let mut ids = Vec::new();
    for i in 0..5 {
        let mut a = new_article(&format!("https://n.example/{i}"), &format!("g{i}"));
        a.published_at = Some(format!("2026-01-0{}T00:00:00Z", i + 1));
        let (id, _) = upsert_article_with_feed(&conn, feed, &a, false).unwrap();
        ids.push(id);
    }

    let q = ArticleQuery {
        feed_id: Some(feed),
        folder_id: None,
        only_unread: false,
        only_starred: false,
        only_today: false,
        newest_first: true,
        limit: 500,
        offset: 0,
        with_content: false,
        layout: None,
        last_published: None,
        last_id: None,
    };

    // 全量列表顺序：最新（i=4）在最前
    let all = list_articles(&conn, &q).unwrap();
    assert_eq!(all.len(), 5);
    // 位置 0 = i=4（最新），位置 4 = i=0（最旧）
    assert_eq!(all[0].id, ids[4], "newest first: ids[4] at pos 0");
    assert_eq!(all[4].id, ids[0], "newest first: ids[0] at pos 4");

    // 每篇的 article_index 应与它在列表中的位置一致
    for (pos, row) in all.iter().enumerate() {
        let idx = article_index(&conn, &q, row.id).unwrap();
        assert_eq!(
            idx,
            Some(pos as i64),
            "article {} should be at pos {}",
            row.id,
            pos
        );
    }

    // 从某个 offset 拉取，第一条应是 article_index 等于该 offset 的文章
    let target = all[2].id; // 位置 2 的文章
    let idx = article_index(&conn, &q, target).unwrap().unwrap();
    assert_eq!(idx, 2);
    let page = list_articles(
        &conn,
        &ArticleQuery {
            offset: idx,
            ..q.clone()
        },
    )
    .unwrap();
    assert_eq!(page[0].id, target, "offset={} 的第一条应是目标文章", idx);
}

/* ============================================================
OPT-008B 第一阶段：smartDedup 显示集合的一致性（真实 SQL）
============================================================ */

fn set_smart_dedup(conn: &Connection, on: bool) {
    set_setting(conn, "app_settings", &format!(r#"{{"smartDedup":{on}}}"#)).unwrap();
}

fn dq(feed: Option<i64>, folder: Option<i64>, unread: bool, layout: Option<&str>) -> ArticleQuery {
    ArticleQuery {
        feed_id: feed,
        folder_id: folder,
        only_unread: unread,
        only_starred: false,
        only_today: false,
        newest_first: true,
        limit: 500,
        offset: 0,
        with_content: false,
        layout: layout.map(str::to_string),
        last_published: None,
        last_id: None,
    }
}

fn ids_of(conn: &Connection, q: &ArticleQuery) -> Vec<i64> {
    list_articles(conn, q)
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect()
}

fn row_at(conn: &Connection, id: i64) -> (Option<String>, i64, i64) {
    conn.query_row(
        "SELECT url_norm, is_read, is_starred FROM articles WHERE id = ?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .unwrap()
}

/// 夹具：分类 A（article），f1/f2 继承，f3 源级覆盖 social。
/// 行（按插入序即 id 序）：
///   X1(f1,U1,已读) X2(f2,U1,未读) Y1(f1,U2,未读) Z1(f2,U3,未读) S1(f3,U1,未读)
struct DisplayFixture {
    conn: Connection,
    f1: i64,
    f2: i64,
    f3: i64,
    x1: i64,
    x2: i64,
    y1: i64,
    z1: i64,
    s1: i64,
}

fn display_fixture() -> DisplayFixture {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    let fa = create_folder(&conn, "A", "article").unwrap();
    let f1 = insert_feed(
        &conn,
        "https://d.example/f1",
        None,
        "f1",
        None,
        fa,
        "inherit",
        false,
        false,
    )
    .unwrap();
    let f2 = insert_feed(
        &conn,
        "https://d.example/f2",
        None,
        "f2",
        None,
        fa,
        "inherit",
        false,
        false,
    )
    .unwrap();
    let f3 = insert_feed(
        &conn,
        "https://d.example/f3",
        None,
        "f3",
        None,
        fa,
        "social",
        false,
        false,
    )
    .unwrap();
    let mk = |url: &str, guid: &str, title: &str, published: &str| {
        let mut a = new_article(url, guid);
        a.title = title.into();
        a.published_at = Some(published.into());
        a
    };
    let (x1, _) = upsert_article_with_feed(
        &conn,
        f1,
        &mk(
            "https://n.example/u1",
            "x1",
            "同一篇",
            "2026-01-01T00:01:00Z",
        ),
        false,
    )
    .unwrap();
    let (x2, _) = upsert_article_with_feed(
        &conn,
        f2,
        &mk(
            "https://n.example/u1",
            "x2",
            "同一篇",
            "2026-01-01T00:02:00Z",
        ),
        false,
    )
    .unwrap();
    let (y1, _) = upsert_article_with_feed(
        &conn,
        f1,
        &mk(
            "https://n.example/u2",
            "y1",
            "第二篇",
            "2026-01-01T00:03:00Z",
        ),
        false,
    )
    .unwrap();
    let (z1, _) = upsert_article_with_feed(
        &conn,
        f2,
        &mk(
            "https://n.example/u3",
            "z1",
            "第三篇",
            "2026-01-01T00:04:00Z",
        ),
        false,
    )
    .unwrap();
    let (s1, _) = upsert_article_with_feed(
        &conn,
        f3,
        &mk(
            "https://n.example/u1",
            "s1",
            "同一篇",
            "2026-01-01T00:05:00Z",
        ),
        false,
    )
    .unwrap();
    set_read(&conn, x1, true).unwrap();
    DisplayFixture {
        conn,
        f1,
        f2,
        f3,
        x1,
        x2,
        y1,
        z1,
        s1,
    }
}

/// 显示集合按 scope 形成：全部（当前布局）跨源折叠；单源/单布局各自可达，
/// 未被吞。开关关闭时全部可见（逐行等价旧行为）。
#[test]
fn display_dedup_is_per_scope_and_source_reachable() {
    let fx = display_fixture();
    let conn = &fx.conn;
    set_smart_dedup(conn, true);

    // 全部 × article 布局：U1{U1@f1 最小}=代表 X1；U2、U3 各一行 → 3
    let all_article = ids_of(conn, &dq(None, None, false, Some("article")));
    assert_eq!(all_article.len(), 3, "跨源同 URL 在全部视图折叠为 1");
    assert!(all_article.contains(&fx.x1) && !all_article.contains(&fx.x2));

    // 单源 f2：自己的 U1 副本（X2）必须可达（旧全局去重会吞掉它）
    let f2_article = ids_of(conn, &dq(Some(fx.f2), None, false, Some("article")));
    assert_eq!(f2_article.len(), 2);
    assert!(f2_article.contains(&fx.x2), "单源视图不得被其它源吞条目");

    // social 布局：f3 的副本独立可达
    let social = ids_of(conn, &dq(None, None, false, Some("social")));
    assert_eq!(social, vec![fx.s1], "不同布局的自身条目不被吞");

    // 未读视图：代表在**未读集合**里选（X1 已读 → 代表是 X2）——不漏项
    let unread = ids_of(conn, &dq(None, None, true, Some("article")));
    assert_eq!(unread.len(), 3, "同组仍有未读副本时未读视图不缺项");
    assert!(unread.contains(&fx.x2) && !unread.contains(&fx.x1));

    // 关闭开关：全部可见（跨源两行都出）
    set_smart_dedup(conn, false);
    let all_off = ids_of(conn, &dq(None, None, false, Some("article")));
    assert_eq!(all_off.len(), 4, "关闭后逐行可见（旧行为）");
    assert!(all_off.contains(&fx.x2));
    let _ = (fx.f1, fx.f3, fx.y1, fx.z1);
}

/// 计数与列表同一口径：feed_counts（源 scope）、scope_counts（全部/分类/源）、
/// markAll/keyset 全部与对应 scope 的列表逐行一致。
#[test]
fn display_dedup_counts_and_mark_all_share_one_set() {
    let fx = display_fixture();
    let conn = &fx.conn;
    set_smart_dedup(conn, true);

    // 计数 = 各 scope 的列表行数
    let counts = feed_counts(conn).unwrap();
    let by_feed = |fid: i64| counts.iter().find(|c| c.feed_id == fid).unwrap().total;
    assert_eq!(
        by_feed(fx.f1),
        ids_of(conn, &dq(Some(fx.f1), None, false, None)).len() as i64
    );
    assert_eq!(
        by_feed(fx.f2),
        ids_of(conn, &dq(Some(fx.f2), None, false, None)).len() as i64
    );
    assert_eq!(
        by_feed(fx.f3),
        ids_of(conn, &dq(Some(fx.f3), None, false, None)).len() as i64
    );

    let sc = scope_counts(conn, Some("article")).unwrap();
    let all_len = ids_of(conn, &dq(None, None, false, Some("article"))).len() as i64;
    assert_eq!(
        sc.all.total, all_len,
        "全部计数 == 全部列表行数（同 scope 同谓词）"
    );
    let folder_len = ids_of(conn, &dq(None, Some(1), false, Some("article"))).len() as i64;
    assert_eq!(
        sc.folders.iter().find(|c| c.scope_id == 1).unwrap().total,
        folder_len,
        "分类计数 == 分类列表行数"
    );
    for c in &sc.feeds {
        let l = ids_of(conn, &dq(Some(c.scope_id), None, false, Some("article"))).len() as i64;
        assert_eq!(c.total, l, "源 {} 计数 == 源列表行数", c.scope_id);
    }

    // markAll 的范围 = **读目标集合**（范围未读 ∪ 同组闭包；R1：显示去重不参与
    // 操作集合——X1 已读不在内；U1 组含 social 布局的 S1，闭包按 read 广播政策
    // 跨 scope 标读+入队，不依赖远端收敛）。
    let unread_before =
        list_unread_ids_scoped(conn, None, None, false, None, Some("article")).unwrap();
    assert_eq!(unread_before.len(), 3);
    assert!(unread_before.contains(&fx.x2) && !unread_before.contains(&fx.s1));

    // keyset 翻页走完的集合 == 一次取全（开关开启时也不跳页/不漏项；在标读前）
    let q = dq(None, None, true, Some("article"));
    let ids_full = ids_of(conn, &q);
    let mut cursor = q.clone();
    cursor.limit = 1;
    let mut walked = Vec::new();
    loop {
        let rows = list_articles(conn, &cursor).unwrap();
        let n = rows.len() as i64;
        walked.extend(rows.iter().map(|r| r.id));
        if n < 1 {
            break;
        }
        let last = rows.last().unwrap();
        cursor.last_published = last.published_at.clone();
        cursor.last_id = Some(last.id);
    }
    assert_eq!(walked, ids_full, "keyset 逐页与整取集合一致");

    let marked =
        mark_all_read_with_enqueue(conn, None, None, false, None, Some("article")).unwrap();
    assert_eq!(
        marked, 4,
        "标读数量 = 目标集合内真实改变行数（X2/Y1/Z1 范围未读 + S1 同组跨 scope 广播）"
    );
    assert_eq!(row_at(conn, fx.x2).1, 1, "范围未读副本被标读");
    assert_eq!(row_at(conn, fx.y1).1, 1);
    assert_eq!(row_at(conn, fx.z1).1, 1);
    assert_eq!(
        row_at(conn, fx.s1).1,
        1,
        "同组跨 scope 兄弟按 read 广播政策一并标读（不依赖远端收敛）"
    );
    // 入队集合 = 标读集合（键集一致）
    let queued = {
        let mut stmt = conn
            .prepare("SELECT article_id FROM sync_queue WHERE action = 'read' ORDER BY article_id")
            .unwrap();
        let rows = stmt
            .query_map([], |r| r.get::<_, i64>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        rows
    };
    let mut want = vec![fx.x2, fx.y1, fx.z1, fx.s1];
    want.sort_unstable();
    assert_eq!(queued, want, "入队集合 == 标读集合 == 读目标集合");
    assert!(
        list_unread_ids_scoped(conn, None, None, false, None, Some("article"))
            .unwrap()
            .is_empty(),
        "markAll 后该 scope 未读为空（不需要账号/后续 pull）"
    );

    // article_index 与显示集合对齐：代表有位置，被折叠的副本无位置（同 scope）
    let all_article = dq(None, None, false, Some("article"));
    let pos_y1 = article_index(conn, &all_article, fx.y1).unwrap();
    assert_eq!(pos_y1, Some(1), "显示集合内位置 == 列表位置");
    assert_eq!(
        article_index(conn, &all_article, fx.x2).unwrap(),
        None,
        "被折叠副本在全部 scope 无位置（锚定不会指向不可见行）"
    );
    let f2_scope = dq(Some(fx.f2), None, false, Some("article"));
    assert_eq!(
        article_index(conn, &f2_scope, fx.x2).unwrap(),
        Some(1),
        "单源 scope 内自己的条目有位置"
    );

    // 搜索与显示集合同口径（全局 scope）：开 1 条、关 3 条（含 social 副本）
    let on = search_articles(conn, "同一篇", 50).unwrap();
    assert_eq!(on.len(), 1, "搜索折叠到全局代表");
    set_smart_dedup(conn, false);
    let off = search_articles(conn, "同一篇", 50).unwrap();
    assert_eq!(off.len(), 3, "关闭后搜索逐行可见");
    // 计数回退到原始行数
    let sc_off = scope_counts(conn, Some("article")).unwrap();
    assert_eq!(sc_off.all.total, 4, "关闭后全部计数回到原始行数");
}

/// OPT-008B R1：搜索去重必须在**关键词匹配集合内**选代表——同 URL 组里
/// 较小 id 的副本不含唯一词、较大 id 的副本含词时，后者不得被前者隐藏；
/// 两行都命中时按匹配集合最小 id 折叠；命中的行必须可在当前列表 scope 锚定
/// （可真实打开），不能只折叠不定位。
#[test]
fn display_dedup_search_representative_within_match_set() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    let folder = create_folder(&conn, "S", "article").unwrap();
    let f1 = insert_feed(
        &conn,
        "https://s.example/f1",
        None,
        "f1",
        None,
        folder,
        "inherit",
        false,
        false,
    )
    .unwrap();
    let f2 = insert_feed(
        &conn,
        "https://s.example/f2",
        None,
        "f2",
        None,
        folder,
        "inherit",
        false,
        false,
    )
    .unwrap();
    let mk = |url: &str, guid: &str, title: &str, published: &str| {
        let mut a = new_article(url, guid);
        a.title = title.into();
        a.published_at = Some(published.into());
        a
    };
    // A 较小 id：不含 unique 词；B 较大 id：含 unique 词；两者都含 shared 词
    let (a, _) = upsert_article_with_feed(
        &conn,
        f1,
        &mk(
            "https://s.example/u",
            "a",
            "shared-word alpha",
            "2026-01-01T00:01:00Z",
        ),
        false,
    )
    .unwrap();
    let (b, _) = upsert_article_with_feed(
        &conn,
        f2,
        &mk(
            "https://s.example/u",
            "b",
            "shared-word uniquekeyword",
            "2026-01-01T00:02:00Z",
        ),
        false,
    )
    .unwrap();
    set_smart_dedup(&conn, true);

    // 唯一词只在 B：B 不得被 A 隐藏（修前：全局选代表 = A → 漏匹配）
    let r = search_articles(&conn, "uniquekeyword", 50).unwrap();
    assert_eq!(r.len(), 1, "命中的副本必须被搜到");
    assert_eq!(r[0].id, b, "返回的是含词副本 B（不是其它行的代表）");

    // 共同词都命中：在匹配集合内折叠到最小 id = A
    let r = search_articles(&conn, "shared-word", 50).unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].id, a, "都命中时按匹配集合最小 id 折叠");

    // 命中行可真实打开/定位：A 在当前列表 scope（全部 × article）有位置
    let q = dq(None, None, false, Some("article"));
    assert_eq!(
        article_index(&conn, &q, a).unwrap(),
        Some(0),
        "搜索代表在列表 scope 可锚定（打开路径可用）"
    );

    // 关闭开关：逐行可见，B 也可被安全打开
    set_smart_dedup(&conn, false);
    let r = search_articles(&conn, "uniquekeyword", 50).unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].id, b);
    assert!(
        article_index(&conn, &q, b).unwrap().is_some(),
        "关闭开关后 B 在列表中可见且可锚定"
    );
}
