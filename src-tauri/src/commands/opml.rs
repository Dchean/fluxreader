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
#[derive(Debug, serde::Serialize)]
pub struct OpmlImportReport {
    pub imported: usize,
    pub skipped: usize,
}

#[tauri::command]
pub async fn opml_import(
    state: State<'_, AppState>,
    content: String,
) -> AppResult<OpmlImportReport> {
    let report = {
        let conn = state.db.lock().await;
        opml_import_for_test(&conn, &content)?
    };
    // OPT-009 B3：导入若已连接，订阅写回意图与本地导入同事务入队——锁外广播
    // 订阅写回回执（未连接/未入队时 notify 内部静默或同快照）。
    crate::sync::notify_subscription_outbox_changed(&state.db).await;
    Ok(report)
}

/// doc(hidden) 集成测试入口：**与 `opml_import` 命令同一实现**（不持 Tauri
/// State，测试用本地文件库）——避免「测试绿但命令路径未接线」。
/// 不是 IPC，webview 不可达。
#[doc(hidden)]
pub fn opml_import_for_test(conn: &Connection, content: &str) -> AppResult<OpmlImportReport> {
    let feeds = crate::opml::parse(content)?;
    import_feeds(conn, &feeds)
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

    // P3-1（自检 2026-09-29）：整个导入单事务——建目录 → 插 feed → 清墓碑 →
    // 入队，任一条失败整体回滚（Err 路径 Transaction drop 自动回滚；unchecked_
    // transaction 用法对齐同库 mark_all_read_with_enqueue / persist_new_feed）。
    // 此前逐条自动提交：第 N 条失败会留下「部分源已导入」的半套状态，且报告
    // 以 Err 丢弃、已导入计数无处可查，用户重试会得到叠加结果。
    let tx = conn.unchecked_transaction()?;

    for f in feeds {
        // 已存在（URL 碰撞）→ 跳过。
        // P3[9]（REQ-104）：去重改用**规范化 URL**（feed_id_by_url_normalized，与
        // sync 侧 pull_feeds / add_feed 同口径）。此前只有 feed_exists_by_url 的精确匹配，
        // 而 feeds.feed_url 的 UNIQUE 也按原串，于是同一订阅只要饰词不同
        // （https/http、www.、尾斜杠、utm_* 等跟踪参数）就能被再次导入成第二个 feed
        // → 文章翻倍、已读/收藏状态分裂、未读数与远端对不齐。
        if db::feed_exists_by_url(&tx, &f.feed_url)?
            || db::feed_id_by_url_normalized(&tx, &f.feed_url)?.is_some()
        {
            report.skipped += 1;
            continue;
        }
        let name = f.folder.as_deref().unwrap_or("导入");
        let folder_id = match folder_ids.get(name) {
            Some(id) => *id,
            None => {
                let id = db::create_folder(&tx, name, "article")?;
                folder_ids.insert(name.to_string(), id);
                id
            }
        };
        db::insert_feed(
            &tx,
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
        db::remove_feed_tombstone(&tx, &f.feed_url)?;
        // OPT-009 阶段 B1-R3（复审 P2-2）：新增订阅的**类型化**写回意图与本地
        // 新增同一事务——quick_add 成功后的绑定/分类挂载失败会保留队项重试。
        // 此前写 legacy `sync_queue.add_feed`：push_feeds 在 quick_add 成功时立即
        // 把队项加入 done 并剪除，「源已创建」被当成「完整订阅意图已完成」，
        // 分类挂载失败后无持久载体可重试（与执行器也会双重推送）。
        // 目录目标在入队时解析成 label（执行器挂载用）；未连接 → 仅本地
        // （首连弹窗的 sync_local_feeds 再推）；凭据读取失败 → Err 整体回滚。
        // B2-5：非空导入标题进入 typed subscribe 的 desired_title（quick_add 后
        // 用真实 t= 写回远端）；空标题不产生标题目标（不伪造远端标题）。
        // Note: OPML 导入与 add_feed 同一类型化接线 — 见 .agents/notes/implemented/architecture/2026-10-14-订阅写回outbox执行器.md
        if let Some(ctx) = crate::sync::current_account(&tx)? {
            let folder_label = db::folder_name(&tx, folder_id)?;
            // B2-5：非空导入标题进入 typed subscribe 的 desired_title（quick_add 后
            // 用真实 t= 写回远端）。**标题等于 URL** = 该 outlin 没给可用标题
            // （opml::parse 的 URL 兜底），不是用户意图 → 不产生标题目标
            // （否则会把 URL 当标题推到远端）。
            let title = f.title.trim();
            let desired_title = (!title.is_empty() && title != f.feed_url).then_some(title);
            db::enqueue_subscribe(
                &tx,
                ctx.generation,
                &ctx.key,
                &f.feed_url,
                desired_title,
                folder_label.as_deref(),
            )?;
        }
        report.imported += 1;
    }
    tx.commit()?;
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

    /// P3-1（自检 2026-09-29）：导入中途失败必须**整体回滚**——第 1 条已导入的
    /// 源/目录/队列/事务内墓碑清除全部撤销（修前逐条自动提交会残留半套）。
    /// 生产代码无注入点，用测试侧 RAISE(ABORT) 触发器在第 2 条的入队处注入
    /// 真实失败（SQLITE_CONSTRAINT 走的就是生产 Err 路径）。
    /// OPT-009 阶段 B1-R3：入队目标从 legacy `sync_queue.add_feed` 换成类型化
    /// `subscription_outbox`（复审 P2-2），注入点随之迁移；并预置凭据使
    /// current_account 命中（未连接时本就不入队，注入会失效）。
    #[test]
    fn import_feeds_midway_failure_rolls_back_everything() {
        let conn = conn();
        db::set_setting(&conn, "greader_endpoint", "https://example.com").unwrap();
        db::set_setting(&conn, "greader_username", "u").unwrap();
        db::set_setting(&conn, "greader_password", "pw").unwrap();
        assert!(
            crate::sync::current_account(&conn).unwrap().is_some(),
            "前置：凭据必须可读（否则不入队，注入失效）"
        );
        conn.execute_batch(
            "CREATE TRIGGER test_fail_second_enqueue BEFORE INSERT ON subscription_outbox
             WHEN NEW.feed_url = 'https://boom.example/rss' BEGIN
                 SELECT RAISE(ABORT, 'simulated midway failure');
             END;",
        )
        .unwrap();
        db::add_feed_tombstone(&conn, "https://a.example/rss").unwrap();

        let err = import_feeds(
            &conn,
            &[
                feed("https://a.example/rss", "A", None),
                feed("https://boom.example/rss", "B", None),
            ],
        )
        .unwrap_err();
        assert!(
            err.message.contains("simulated midway failure"),
            "必须是注入的约束失败现场: {err}"
        );

        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM feeds"),
            0,
            "中途失败必须整体回滚：第 1 条已导入的源也不得残留（修前残留）"
        );
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM folders WHERE name = '导入'"),
            0,
            "导入目录必须随事务回滚"
        );
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM subscription_outbox"),
            0,
            "类型化订阅队项必须随事务回滚"
        );
        assert!(
            db::feed_tombstones(&conn)
                .unwrap()
                .contains(&"http://a.example/rss".to_string()),
            "事务内的墓碑清除必须被回滚（否则半套状态破坏删除语义）"
        );
    }

    /// OPT-009 阶段 B1-R3（复审 P2-2）：导入入队的是**类型化** subscribe 意图
    /// （带账号 key/代际与目录 label），不再写 legacy add_feed——
    /// 后者在 quick_add 成功后即被剪队，分类挂载失败无持久载体可重试。
    /// 判别力：仍写 legacy 时本用例的 outbox 计数为 0、legacy 计数为 1。
    #[test]
    fn import_feeds_enqueues_typed_subscribe_not_legacy_add_feed() {
        let conn = conn();
        db::set_setting(&conn, "greader_endpoint", "https://opml.example").unwrap();
        db::set_setting(&conn, "greader_username", "u").unwrap();
        db::set_setting(&conn, "greader_password", "pw").unwrap();
        let ctx = crate::sync::current_account(&conn).unwrap().unwrap();

        let report =
            import_feeds(&conn, &[feed("https://a.example/rss", "A", Some("技术"))]).unwrap();
        assert_eq!(report.imported, 1);
        let items = db::pending_outbox(&conn).unwrap();
        assert_eq!(items.len(), 1, "必须写入类型化订阅意图: {items:?}");
        assert_eq!(items[0].action, "subscribe");
        assert_eq!(items[0].account_key, ctx.key);
        assert_eq!(items[0].account_generation, ctx.generation);
        assert_eq!(
            items[0].desired_folder.as_deref(),
            Some("技术"),
            "导入目录必须成为远端目标"
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM sync_queue WHERE action = 'add_feed'"
            ),
            0,
            "不得再写 legacy add_feed（与执行器双重推送；quick_add 后即被剪掉）"
        );
    }
}
