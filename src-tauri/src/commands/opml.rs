//! commands 的 opml 领域子模块（TASK-044 从 commands.rs 按既有章节拆分，纯搬运）。

use crate::db;
use crate::error::AppResult;
use crate::state::AppState;
use rusqlite::Connection;
use tauri::State;

/* ============================================================
OPML 导入导出
============================================================ */

/// 导入 OPML：按目录建 folder → 插入 feed（已存在的 URL 跳过）→ 入同步队列。
/// 返回 (新增源数, 跳过数)。
#[derive(serde::Serialize)]
pub struct OpmlImportReport {
    pub imported: usize,
    pub skipped: usize,
}

#[tauri::command]
pub async fn opml_import(
    state: State<'_, AppState>,
    content: String,
) -> AppResult<OpmlImportReport> {
    let feeds = crate::opml::parse(&content)?;
    let conn = state.db.lock().await;
    import_feeds(&conn, &feeds)
}

/// 导入循环本体（从 opml_import 抽出以便测试）。
/// 目录解析单一路径：根级（无文件夹）订阅统一落「导入」，与具名目录共用
/// folder_ids 缓存——TASK-062 修复 N1：此前 None 分支每条都无条件
/// create_folder，而 folders.name 无 UNIQUE，导入 N 条根级订阅会产生
/// N 个同名「导入」目录。
fn import_feeds(
    conn: &Connection,
    feeds: &[crate::opml::ImportedFeed],
) -> AppResult<OpmlImportReport> {
    let mut report = OpmlImportReport {
        imported: 0,
        skipped: 0,
    };

    // 目录名 → folder_id 缓存（一次导入内同名目录只建一次）
    let mut folder_ids: std::collections::HashMap<String, i64> = std::collections::HashMap::new();

    for f in feeds {
        // 已存在（URL 碰撞）→ 跳过。
        // P3[9]（REQ-104）：去重改用**规范化 URL**（feed_id_by_url_normalized，与
        // sync 侧 pull_feeds / add_feed 同口径）。此前只有 feed_exists_by_url 的精确匹配，
        // 而 feeds.feed_url 的 UNIQUE 也按原串，于是同一订阅只要饰词不同
        // （https/http、www.、尾斜杠、utm_* 等跟踪参数）就能被再次导入成第二个 feed
        // → 文章翻倍、已读/收藏状态分裂、未读数与远端对不齐。
        if db::feed_exists_by_url(conn, &f.feed_url)?
            || db::feed_id_by_url_normalized(conn, &f.feed_url)?.is_some()
        {
            report.skipped += 1;
            continue;
        }
        let name = f.folder.as_deref().unwrap_or("导入");
        let folder_id = match folder_ids.get(name) {
            Some(id) => *id,
            None => {
                let id = db::create_folder(conn, name, "article")?;
                folder_ids.insert(name.to_string(), id);
                id
            }
        };
        db::insert_feed(
            conn,
            &f.feed_url,
            None,
            &f.title,
            None,
            folder_id,
            "inherit",
            true,
            false,
        )?;
        // TASK-064 N4：同 add_feed——重新导入 = 用户改变主意的最强证据，清掉
        // 同 URL 的删除墓碑，否则 pull 永久跳过该源（不绑 remote_id）、其未推送
        // 状态 30 天后被 prune_stale_unbound 物理删除。
        db::remove_feed_tombstone(conn, &f.feed_url)?;
        // 新增订阅入同步队列（连接 Miniflux 后补推）。payload 必须是含 folder_id
        // 的 JSON——push_feeds 据此把订阅挂到远端对应分类；此前误传标题字符串，
        // serde_json 解析失败导致 payload 丢弃、源被推到远端默认分类（目录丢失）。
        let payload = serde_json::json!({ "folder_id": folder_id }).to_string();
        db::enqueue_sync(conn, None, Some(&f.feed_url), "add_feed", Some(&payload))?;
        report.imported += 1;
    }
    Ok(report)
}

/// 导出 OPML：全部源 + 目录名 → OPML 文档字符串。
#[tauri::command]
pub async fn opml_export(state: State<'_, AppState>) -> AppResult<String> {
    let rows = {
        let conn = state.db.lock().await;
        db::export_feeds_with_folders(&conn)?
    };
    crate::opml::build(&rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::MIGRATIONS;

    fn conn() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        conn
    }

    fn feed(url: &str, title: &str, folder: Option<&str>) -> crate::opml::ImportedFeed {
        crate::opml::ImportedFeed {
            feed_url: url.into(),
            title: title.into(),
            folder: folder.map(|s| s.into()),
        }
    }

    fn count(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    /// N1 缺陷复现：两条根级订阅只允许产生一个「导入」目录（修前为 2 个）
    #[test]
    fn root_level_feeds_share_one_import_folder() {
        let conn = conn();
        let report = import_feeds(
            &conn,
            &[
                feed("https://a.example/rss", "A", None),
                feed("https://b.example/rss", "B", None),
            ],
        )
        .unwrap();
        assert_eq!(report.imported, 2);
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM folders WHERE name = '导入'"),
            1
        );
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM feeds"), 2);
    }

    /// 同名目录在缓存内只建一次（Some 分支既有行为锚定）
    #[test]
    fn same_named_folders_are_created_once() {
        let conn = conn();
        let report = import_feeds(
            &conn,
            &[
                feed("https://a.example/rss", "A", Some("技术")),
                feed("https://b.example/rss", "B", Some("技术")),
            ],
        )
        .unwrap();
        assert_eq!(report.imported, 2);
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM folders WHERE name = '技术'"),
            1
        );
    }

    /// 重复 URL 跳过（既有行为锚定）
    #[test]
    fn duplicate_urls_are_skipped() {
        let conn = conn();
        let report = import_feeds(
            &conn,
            &[
                feed("https://a.example/rss", "A", None),
                feed("https://a.example/rss", "A copy", None),
            ],
        )
        .unwrap();
        assert_eq!(report.imported, 1);
        assert_eq!(report.skipped, 1);
    }

    /// N4：重新导入清墓碑——先删源留墓碑，再导入同 URL，墓碑必须消失
    /// （否则 pull 永久跳过该源，其未推送状态 30 天后被老化物理删除）。
    /// 注意墓碑按 normalize_url 存储（https 统一为 http）。
    #[test]
    fn reimporting_a_url_clears_its_tombstone() {
        let conn = conn();
        db::add_feed_tombstone(&conn, "https://a.example/rss").unwrap();
        assert!(db::feed_tombstones(&conn)
            .unwrap()
            .contains(&"http://a.example/rss".to_string()));
        let report = import_feeds(&conn, &[feed("https://a.example/rss", "A", None)]).unwrap();
        assert_eq!(report.imported, 1);
        assert!(
            !db::feed_tombstones(&conn)
                .unwrap()
                .contains(&"http://a.example/rss".to_string()),
            "重新导入后墓碑必须清除（N4：否则 pull 永久跳过该源）"
        );
    }
}
