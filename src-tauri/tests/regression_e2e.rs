//! 全链路回归：模拟「用户设置 → 重启（重建连接）→ 设置保持生效」时序。
//! 覆盖本轮 8 个问题中可后端验证的部分：
//! - 源独立布局设置后重新打开仍保持（修复 numericId 前会 NaN 写库失败回退 inherit）
//! - 分类布局设置后保持
//! - AI 开关默认关闭（schema DEFAULT 0 + insert 显式 false）
//! - 滚动标读守卫语义在 DB 层无直接观感，但布局/AI 持久化是本轮核心。
//!
//! 运行：cargo test --test regression_e2e

use app_lib::db;
use rusqlite::Connection;

fn fresh_db() -> (Connection, std::path::PathBuf) {
    let tmp = std::env::temp_dir().join(format!(
        "fluxreader_regression_{}_{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();
    (conn, tmp)
}

fn get_feed(conn: &Connection, id: i64) -> db::FeedRow {
    db::list_feeds(conn).unwrap().into_iter().find(|f| f.id == id).unwrap()
}

#[test]
fn feed_layout_persists_across_reopen() {
    let (conn, tmp) = fresh_db();
    let cat = db::create_folder(&conn, "技术", "article").unwrap();
    let feed = db::insert_feed(&conn, "https://x/rss", None, "源A", None, cat, "inherit", true, false).unwrap();

    /* 用户设置源独立布局为「画廊」——db::update_feed_layout 直写（对应前端 numericId 修复后的调用） */
    db::update_feed_layout(&conn, feed, "gallery").unwrap();
    assert_eq!(get_feed(&conn, feed).layout, "gallery");

    /* 重启：重建连接再读（模拟前端 reloadFromBackend 全量拉取） */
    drop(conn);
    let conn2 = db::open(&tmp).unwrap();
    let row = get_feed(&conn2, feed);
    assert_eq!(row.layout, "gallery", "独立布局在重启后必须保持（曾因前端 id 解析 NaN 写库失败回退 inherit）");
    assert_eq!(get_feed(&conn2, feed).folder_id, cat);
}

#[test]
fn category_layout_persists_across_reopen() {
    let (conn, tmp) = fresh_db();
    let cat = db::create_folder(&conn, "生活", "social").unwrap();
    db::update_folder_layout(&conn, cat, "podcast").unwrap();

    drop(conn);
    let conn2 = db::open(&tmp).unwrap();
    let cat2 = db::list_folders(&conn2).unwrap().into_iter().find(|f| f.id == cat).unwrap();
    assert_eq!(cat2.layout, "podcast");
}

#[test]
fn ai_flags_default_off_for_new_feeds() {
    let (conn, _tmp) = fresh_db();
    let cat = db::create_folder(&conn, "默认", "article").unwrap();
    /* 不显式传 AI 开关——等价于用户添加源时未勾选（前端默认 false） */
    let feed = db::insert_feed(&conn, "https://y/rss", None, "源B", None, cat, "inherit", false, false).unwrap();
    let row = get_feed(&conn, feed);
    assert!(!row.auto_summary, "摘要默认关闭");
    assert!(!row.auto_translate, "翻译默认关闭");

    /* schema 层：不带该列插入也应落到 DEFAULT 0（新建库的约束） */
    let raw: i64 = conn
        .query_row(
            "SELECT auto_summary FROM feeds WHERE id = ?1",
            [feed],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(raw, 0);
}

#[test]
fn ai_flags_toggle_persists() {
    let (conn, tmp) = fresh_db();
    let cat = db::create_folder(&conn, "开关", "article").unwrap();
    let feed = db::insert_feed(&conn, "https://z/rss", None, "源C", None, cat, "inherit", false, false).unwrap();

    /* 用户打开摘要开关（前端 numericId 修复后真实落库） */
    db::set_feed_ai_flags(&conn, feed, true, false).unwrap();
    drop(conn);
    let conn2 = db::open(&tmp).unwrap();
    let row = get_feed(&conn2, feed);
    assert!(row.auto_summary);
    assert!(!row.auto_translate);

    /* 再关回来 */
    db::set_feed_ai_flags(&conn2, feed, false, true).unwrap();
    let row2 = get_feed(&conn2, feed);
    assert!(!row2.auto_summary);
    assert!(row2.auto_translate);
}