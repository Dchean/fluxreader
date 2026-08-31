//! 编辑源 / 分类改名 集成测试（db 层）。
//! 覆盖：update_feed 一次性更新标题/分类/布局/AI 开关、空标题回退、
//! rename_folder 落库。运行：cargo test --test feed_edit_e2e

use app_lib::db;
use rusqlite::Connection;

fn seed() -> (Connection, std::path::PathBuf) {
    let tmp = std::env::temp_dir().join(format!(
        "fluxreader_feededit_{}_{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();
    let f1 = db::create_folder(&conn, "技术", "article").unwrap();
    let f2 = db::create_folder(&conn, "生活", "social").unwrap();
    let feed = db::insert_feed(&conn, "https://a.com/rss", Some("https://a.com"), "源A", None, f1, "inherit", true, false).unwrap();
    let _ = (f2, feed);  // seed 局部值：测试各自用 list_feeds 重新取
    (conn, tmp)
}

fn get_feed(conn: &Connection, id: i64) -> db::FeedRow {
    db::list_feeds(conn).unwrap().into_iter().find(|f| f.id == id).unwrap()
}

#[test]
fn update_feed_all_fields() {
    let (conn, _tmp) = seed();
    let f2 = db::list_folders(&conn).unwrap().into_iter().find(|f| f.name == "生活").unwrap();
    let feed = db::list_feeds(&conn).unwrap()[0].clone();

    db::update_feed(&conn, feed.id, Some("新名字"), Some(f2.id), Some("podcast"), Some(false), Some(true)).unwrap();

    let row = get_feed(&conn, feed.id);
    assert_eq!(row.title, "新名字");
    assert_eq!(row.folder_id, f2.id);
    assert_eq!(row.layout, "podcast");
    assert!(!row.auto_summary);
    assert!(row.auto_translate);
}

#[test]
fn update_feed_partial_and_empty_title_fallback() {
    let (conn, _tmp) = seed();
    let feed = db::list_feeds(&conn).unwrap()[0].clone();
    let f1 = db::list_folders(&conn).unwrap().into_iter().find(|f| f.name == "技术").unwrap();

    // 只改布局：其余传 None 保持不变
    db::update_feed(&conn, feed.id, None, None, Some("gallery"), None, None).unwrap();
    let row = get_feed(&conn, feed.id);
    assert_eq!(row.layout, "gallery");
    assert_eq!(row.title, "源A");
    assert_eq!(row.folder_id, f1.id);
    assert!(row.auto_summary);

    // 空白标题 → 回退原名（不置空）
    db::update_feed(&conn, feed.id, Some("   "), None, None, None, None).unwrap();
    let row = get_feed(&conn, feed.id);
    assert_eq!(row.title, "源A");
}

#[test]
fn rename_folder_updates_name() {
    let (conn, _tmp) = seed();
    let f1 = db::list_folders(&conn).unwrap().into_iter().find(|f| f.name == "技术").unwrap();
    db::rename_folder(&conn, f1.id, "重命名后的技术").unwrap();
    let folders = db::list_folders(&conn).unwrap();
    assert!(folders.iter().any(|f| f.name == "重命名后的技术"));
    assert!(!folders.iter().any(|f| f.name == "技术"));
    // 源的归属不受改名影响
    let feed = db::list_feeds(&conn).unwrap()[0].clone();
    assert_eq!(feed.folder_id, f1.id);
}

#[test]
fn feed_counts_counts_by_feed() {
    let (conn, _tmp) = seed();
    let feed = db::list_feeds(&conn).unwrap()[0].clone();
    // feed_counts 是 GROUP BY 语义：只返回有文章的源（无文章的源不出行）
    assert!(!db::feed_counts(&conn).unwrap().iter().any(|c| c.feed_id == feed.id));
}
