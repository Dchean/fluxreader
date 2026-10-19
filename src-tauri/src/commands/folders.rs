//! commands 的 folders 领域子模块（TASK-044 从 commands.rs 按既有章节拆分，纯搬运）。

use super::{read_dedup_flag, sync_configured};
use crate::db;
use crate::error::{AppError, AppResult};
use crate::ingestion;
use crate::state::AppState;
use tauri::State;

/* ============================================================
Folders / Feeds
============================================================ */

#[tauri::command]
pub async fn list_folders(state: State<'_, AppState>) -> AppResult<Vec<db::FolderRow>> {
    let conn = state.db.lock().await;
    db::list_folders(&conn)
}

#[tauri::command]
pub async fn create_folder(
    state: State<'_, AppState>,
    name: String,
    layout: String,
) -> AppResult<i64> {
    let conn = state.db.lock().await;
    db::create_folder(&conn, &name, &layout)
}

/// 分类改名（命令与测试共用的真实逻辑）：本地改名 + 旧 label 墓碑（A-4）+ 成员
/// 订阅的远端移动意图（OPT-009 阶段 B2），全部在同一事务内提交（全有全无）。
///
/// 远端表达（B2-3）：GReader 没有「分类级改名」的可移植端点——`rename-tag`/
/// `disable-tag` 是 TheOldReader 扩展，固定上游里只有 FreshRSS 实现，本仓固定的
/// Miniflux 侧没有可核验支持；用未验证端点会让「2xx 但未生效」冒充成功。故改名
/// 表达为**逐成员订阅**的 `a=` 新 label + `r=` 旧 label（与移动分类同一 wire：
/// 目标分类加入、旧 label 移除、其它标签不被触碰）。
/// 空分类没有成员可表达：本地改名 + 旧 label 墓碑防复活，**不宣称远端已改**
/// （没有可验证的远端表达，不伪造成功——如实列为本卡边界）。
pub fn record_folder_rename(conn: &rusqlite::Connection, id: i64, new_name: &str) -> AppResult<()> {
    let old = db::folder_name(conn, id)?;
    let tx = conn.unchecked_transaction()?;
    // 凭据读取失败 → Err（失败关闭）：整个改名（含墓碑与成员意图）不落库。
    let account = crate::sync::current_account(&tx)?;
    let account_key = account.as_ref().map(|c| c.key.clone()).unwrap_or_default();
    if let Some(old_name) = old.as_deref().filter(|o| *o != new_name) {
        db::add_folder_tombstone(&tx, &account_key, old_name)?;
        if let Some(ctx) = &account {
            for (feed_url, remote_id) in db::folder_members(&tx, id)? {
                match remote_id {
                    Some(rid) => {
                        db::enqueue_edit(
                            &tx,
                            ctx.generation,
                            &ctx.key,
                            &feed_url,
                            rid,
                            None,
                            Some(new_name),
                            Some(old_name),
                        )?;
                    }
                    // remote_created 窗口（本地尚无 remote_id）：并入 pending
                    // subscribe 的目标；挂载时按 `a=` 新 label + `r=` 旧 label 发送。
                    None => {
                        db::update_pending_subscribe_desired(
                            &tx,
                            &ctx.key,
                            &feed_url,
                            None,
                            Some(new_name),
                            Some(old_name),
                        )?;
                    }
                }
            }
        }
    }
    db::rename_folder(&tx, id, new_name)?;
    tx.commit()?;
    Ok(())
}

/// 删除分类（命令与测试共用的真实逻辑）：旧 label 墓碑 + 每个成员订阅的 feed
/// 墓碑与**逐源持久退订意图**（B2-2/B2-3，级联删除前取成员与远端目标快照），
/// 再删目录（级联删订阅与本地条目）——全部同一事务，中途失败整体回滚。
/// 远端表达同改名：没有分类级删除端点（`disable-tag` 未验证），退订逐源表达；
/// label 在最后一个成员移除后自然消失。
pub fn record_folder_delete(conn: &rusqlite::Connection, id: i64) -> AppResult<()> {
    let tx = conn.unchecked_transaction()?;
    let account = crate::sync::current_account(&tx)?;
    let account_key = account.as_ref().map(|c| c.key.clone()).unwrap_or_default();
    if let Some(label) = db::folder_name(&tx, id)? {
        db::add_folder_tombstone(&tx, &account_key, &label)?;
        for (url, remote_id) in db::folder_members(&tx, id)? {
            db::add_feed_tombstone(&tx, &url)?;
            if let Some(ctx) = &account {
                // 每个实际远端成员各自一条可恢复退订意图（remote_id 为空的成员
                // 由 enqueue_unsubscribe 按同账号既有 pending 行结转）；
                // 移除基准 = 被删目录名（成员远端的旧 label，R2-3 后继重加用）。
                db::enqueue_unsubscribe(
                    &tx,
                    ctx.generation,
                    &ctx.key,
                    &url,
                    remote_id,
                    Some(&label),
                )?;
            }
        }
    }
    db::delete_folder(&tx, id)?;
    tx.commit()?;
    Ok(())
}

#[tauri::command]
pub async fn rename_folder(state: State<'_, AppState>, id: i64, name: String) -> AppResult<()> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::new("validate", "分类名称不能为空"));
    }
    // 本地改名 + 旧 label 墓碑（A-4）：远端分类是 label 名，不做远端改写，
    // 但必须阻挡 pull 按旧 label 复活空目录。
    {
        let conn = state.db.lock().await;
        record_folder_rename(&conn, id, &name)?;
    }
    // OPT-009 B3：成员改写的远端意图已在事务内入队（分类级命令不立即推送，
    // 等待下一轮同步）——锁外广播订阅写回回执，UI 立刻看到「待办出现」。
    crate::sync::notify_subscription_outbox_changed(&state.db).await;
    Ok(())
}

#[tauri::command]
pub async fn delete_folder(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    {
        let conn = state.db.lock().await;
        record_folder_delete(&conn, id)?;
    }
    // OPT-009 B3：逐源退订意图已在事务内入队——锁外广播订阅写回回执
    // （同样不立即推送，等待下一轮同步执行器）。
    crate::sync::notify_subscription_outbox_changed(&state.db).await;
    Ok(())
}

#[tauri::command]
pub async fn update_folder_layout(
    state: State<'_, AppState>,
    id: i64,
    layout: String,
) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::update_folder_layout(&conn, id, &layout)
}

#[tauri::command]
pub async fn set_folder_collapsed(
    state: State<'_, AppState>,
    id: i64,
    collapsed: bool,
) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::set_folder_collapsed(&conn, id, collapsed)
}

/// 分类 AI 默认开关写入。OPT-011：参数改为可选（单字段 patch）——新 caller 只传
/// 被切换的字段（summary 或 translate），两字段独立落库，乱序到达不互相覆盖；
/// 旧 full caller 继续传两个 bool，语义不变（整对写）。
#[tauri::command]
pub async fn set_folder_ai_flags(
    state: State<'_, AppState>,
    id: i64,
    summary: Option<bool>,
    translate: Option<bool>,
) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::set_folder_ai_flags_partial(&conn, id, summary, translate)
}

#[tauri::command]
pub async fn list_feeds(state: State<'_, AppState>) -> AppResult<Vec<db::FeedRow>> {
    let conn = state.db.lock().await;
    db::list_feeds(&conn)
}

/// 添加订阅源：先直连抓一次验证 URL 是有效 feed，成功才入库（不依赖 Miniflux）。
/// `folder_id = None`（UI 未选分类，如全新安装无任何分类时）→ 自动落到
/// 「未分类」文件夹（不存在则创建）——首次使用添加源不再报错。
/// 参数较多是 IPC 契约（前端 invoke 逐字段传），加 allow 避免 clippy 误报。
///
/// OPT-011：新增 `summary_mode` / `translate_mode` 三态可选参数（新 UI 默认
/// 'inherit'）；旧 caller 仍可只传 bool——合并规则单点在 resolve_ai_modes。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn add_feed(
    state: State<'_, AppState>,
    feed_url: String,
    title: Option<String>,
    folder_id: Option<i64>,
    layout: String,
    auto_summary: Option<bool>,
    auto_translate: Option<bool>,
    summary_mode: Option<String>,
    translate_mode: Option<String>,
    sync_to_backend: bool,
) -> AppResult<db::FeedRow> {
    // OPT-011：mode 校验先于任何副作用（含出网抓取）——非法值直接拒绝，
    // 不悄悄回默认。新 caller 传 mode 优先；旧 caller 只传 bool 时映射显式
    // on/off；两者皆缺 = 全新源默认 inherit。
    let (summary_mode, translate_mode) = resolve_ai_modes(
        summary_mode.as_deref(),
        translate_mode.as_deref(),
        auto_summary,
        auto_translate,
    )?;
    // 1. 抓取验证（直连，第一优先级）
    let fetched = ingestion::conditional_get(&state.http, &feed_url, None, None).await?;
    let (bytes, etag, last_modified) = match fetched {
        ingestion::Fetched::NotModified => {
            return Err(AppError::new("parse", "unexpected 304 on first fetch"))
        }
        ingestion::Fetched::Body {
            bytes,
            etag,
            last_modified,
            ..
        } => (bytes, etag, last_modified),
    };
    let parsed = ingestion::parse_feed(&bytes, &feed_url)?;

    // 2. 入库（feed 元数据 + 全量条目，source='direct'）
    let row = {
        let conn = state.db.lock().await;
        persist_new_feed(
            &conn,
            &feed_url,
            &parsed,
            title.as_deref(),
            etag.as_deref(),
            last_modified.as_deref(),
            folder_id,
            &layout,
            summary_mode,
            translate_mode,
            sync_to_backend,
        )?
    };
    // OPT-009 B3：勾选同步且已连接时，类型化 subscribe 意图与本地入库同事务
    // 入队——锁外广播订阅写回回执（未入队/未配置时 notify 内部静默或同快照）。
    if sync_to_backend {
        crate::sync::notify_subscription_outbox_changed(&state.db).await;
    }
    Ok(row)
}

/// OPT-011：AI 开关输入的合并规则（单点）——mode 优先，旧 bool 回退为显式
/// on/off，两者皆缺取 inherit（「新增源默认继承」）。非法 mode 串 Err。
fn resolve_ai_modes(
    summary_mode: Option<&str>,
    translate_mode: Option<&str>,
    auto_summary: Option<bool>,
    auto_translate: Option<bool>,
) -> AppResult<(db::FeedAiMode, db::FeedAiMode)> {
    let summary = match summary_mode {
        Some(raw) => db::FeedAiMode::parse(raw)?,
        None => auto_summary
            .map(db::FeedAiMode::from_legacy_bool)
            .unwrap_or(db::FeedAiMode::Inherit),
    };
    let translate = match translate_mode {
        Some(raw) => db::FeedAiMode::parse(raw)?,
        None => auto_translate
            .map(db::FeedAiMode::from_legacy_bool)
            .unwrap_or(db::FeedAiMode::Inherit),
    };
    Ok((summary, translate))
}

/// add_feed 抓取验证后的入库段（TASK-064 抽出以便脱离 Tauri State 测试）。
/// 查重 → 标题兜底 → 「未分类」兜底 → 插入 → 清同 URL 删除墓碑（N4）→
/// 写抓取状态 → 建文章 → 按需入队推送。
/// OPT-011：AI 开关以三态 mode 传入（调用方经 resolve_ai_modes 归一）。
#[allow(clippy::too_many_arguments)]
fn persist_new_feed(
    conn: &rusqlite::Connection,
    feed_url: &str,
    parsed: &ingestion::ParsedFeed,
    title: Option<&str>,
    etag: Option<&str>,
    last_modified: Option<&str>,
    folder_id: Option<i64>,
    layout: &str,
    summary_mode: db::FeedAiMode,
    translate_mode: db::FeedAiMode,
    sync_to_backend: bool,
) -> AppResult<db::FeedRow> {
    // P3[9]（REQ-104）：改用**规范化 URL** 去重（与 sync 侧 pull_feeds 同一判据）。
    // 此前用 find_feed_by_url 精确匹配，而 feeds.feed_url 的 UNIQUE 也是按原串，
    // 于是同一订阅只要饰词不同（https/http、www.、尾斜杠、utm_* 等跟踪参数）就能
    // 被重复添加 → 文章翻倍、已读/收藏状态分裂、未读数与远端对不齐。
    if db::feed_id_by_url_normalized(conn, feed_url)?.is_some() {
        return Err(AppError::new("duplicate", "该订阅地址已存在"));
    }
    let final_title = title
        .filter(|t| !t.trim().is_empty())
        .map(String::from)
        .or_else(|| parsed.title.clone())
        .unwrap_or_else(|| feed_url.to_string());
    // 未选分类 → 「未分类」文件夹（无则建）。创建失败必须上抛——
    // 兜底到 id=1 会在 folder 1 不存在时触发外键违约，文章静默丢失。
    // P1-1（自检 2026-09-29）：显式传入的分类 id 也必须真实存在（对齐
    // update_feed → record_feed_edit 的既有校验形态）——此前 Some(0)（前端
    // 无分类时把空 catId 换算成 0）或陈旧分类 id 会直通 insert_feed，在
    // foreign_keys=ON 下裸报「FOREIGN KEY constraint failed」；现在报可读的
    // folder_not_found。None 路径保持「未分类」兜底不变。
    let folder_id = match folder_id {
        Some(fid) => {
            if !db::folder_exists(conn, fid)? {
                return Err(AppError::new("folder_not_found", "目标分类不存在"));
            }
            fid
        }
        None => db::ensure_uncategorized_folder(conn)?,
    };
    // P3-1（自检 2026-09-29）：入库段全程单事务（unchecked_transaction 用法对齐
    // 同库 mark_all_read_with_enqueue，db/articles.rs）——insert_feed → 清墓碑 →
    // 写抓取状态 → 逐条 upsert 文章 → 按需入队，任一步失败整体回滚（Err 路径
    // Transaction drop 自动回滚）。此前逐条自动提交：文章 upsert 中途失败会留下
    // 「feed 已存在而文章残缺」的半套状态，且重试被「该订阅地址已存在」判重
    // 拦住，用户必须先删再加。查重/分类校验是纯读，留在事务外不影响原子性。
    let tx = conn.unchecked_transaction()?;
    let feed_id = db::insert_feed_origin_modes(
        &tx,
        feed_url,
        parsed.site_url.as_deref(),
        &final_title,
        parsed.icon.as_deref(),
        folder_id,
        layout,
        summary_mode,
        translate_mode,
        "local",
    )?;
    // TASK-064 N4：重新添加 = 用户改变主意的最强证据——清掉同 URL 的删除
    // 墓碑。此前墓碑只在 pull 的「远端不再列出」分支清除，重新添加的源被永久
    // 压制：pull 跳过绑定、其未推送状态 30 天后被 prune_stale_unbound 物理删除。
    db::remove_feed_tombstone(&tx, feed_url)?;
    db::set_feed_fetch_state(&tx, feed_id, false, None, etag, last_modified)?;
    let dedup = read_dedup_flag(&tx);
    for a in &parsed.articles {
        db::upsert_article_with_feed(&tx, feed_id, a, dedup)?;
    }
    // 勾选「同步到后端」且已连接 → 入队类型化订阅 outbox（feeds 阶段推远端）。
    // OPT-014 R1：凭据读取失败 → Err（不得静默当未连接跳过入队）。
    // OPT-009 阶段 B：与本地入库同一事务、携带账号 key/代际；远端确认按
    // (id, generation) 精确出队（由 sync 的订阅执行器处理）。
    if sync_to_backend {
        if let Some(ctx) = crate::sync::current_account(&tx)? {
            let folder_label = db::folder_name(&tx, folder_id)?;
            // B2-5：用户/来源给的标题作为**目标**随队项落库（挂载时用真实 t=
            // 写回）；URL 兜底标题不是用户意图（会把 URL 当标题推到远端）→ 不推。
            let desired_title = (final_title != feed_url).then_some(final_title.as_str());
            db::enqueue_subscribe(
                &tx,
                ctx.generation,
                &ctx.key,
                feed_url,
                desired_title,
                folder_label.as_deref(),
            )?;
        }
    }
    tx.commit()?;
    let row = db::list_feeds(conn)?
        .into_iter()
        .find(|f| f.id == feed_id)
        .ok_or_else(|| AppError::internal("feed row vanished after insert"))?;
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn test_conn() -> rusqlite::Connection {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        db::MIGRATIONS.to_latest(&mut conn).unwrap();
        conn
    }

    fn minimal_parsed() -> ingestion::ParsedFeed {
        ingestion::parse_feed(
            br#"<?xml version="1.0"?><rss version="2.0"><channel><title>T</title>
<item><title>a</title><link>https://e.example/1</link><guid>g1</guid></item>
</channel></rss>"#,
            "https://f.example/rss",
        )
        .unwrap()
    }

    /// N4：重新添加清墓碑——先删源留墓碑，再 persist 同 URL，墓碑必须消失
    /// （否则 pull 永久跳过该源，其未推送状态 30 天后被老化物理删除）。
    #[test]
    fn republishing_a_url_clears_its_tombstone() {
        let conn = test_conn();
        db::add_feed_tombstone(&conn, "https://f.example/rss").unwrap();
        assert!(db::feed_tombstones(&conn)
            .unwrap()
            .contains(&"http://f.example/rss".to_string()));

        let parsed = minimal_parsed();
        let row = persist_new_feed(
            &conn,
            "https://f.example/rss",
            &parsed,
            None,
            None,
            None,
            None,
            "inherit",
            db::FeedAiMode::On,
            db::FeedAiMode::Off,
            false,
        )
        .unwrap();
        assert_eq!(row.feed_url, "https://f.example/rss");
        assert!(
            !db::feed_tombstones(&conn)
                .unwrap()
                .contains(&"http://f.example/rss".to_string()),
            "重新添加后墓碑必须清除（N4：否则 pull 永久跳过该源）"
        );
    }

    /// P1-1（自检 2026-09-29）：folder_id=Some(不存在的 id) 必须报可读的
    /// 「目标分类不存在」（folder_not_found），而不是直通 insert_feed 在
    /// foreign_keys=ON 下裸抛「FOREIGN KEY constraint failed」（修前形态：
    /// Err{code:"db", message 含 FOREIGN KEY}——前端 toast 只能显示这条裸错误）。
    /// 修前此用例红：err.code == "db"；修后绿：err.code == "folder_not_found"。
    #[test]
    fn add_feed_with_missing_folder_id_is_rejected() {
        let conn = test_conn();
        let parsed = minimal_parsed();
        let err = persist_new_feed(
            &conn,
            "https://f.example/rss",
            &parsed,
            None,
            None,
            None,
            Some(999), // folders.id 从 1 起：0（前端无分类时传 0）与 999 均不存在
            "inherit",
            db::FeedAiMode::On,
            db::FeedAiMode::Off,
            false,
        )
        .unwrap_err();
        assert_eq!(
            err.code, "folder_not_found",
            "必须报 folder_not_found（修前是 db/FOREIGN KEY 裸错）: {err}"
        );
        assert!(
            !err.message.contains("FOREIGN KEY"),
            "不得把外键违约裸暴露给前端: {err}"
        );
        // 不得留下半套数据：feed 未插入
        let feeds: i64 = conn
            .query_row("SELECT COUNT(*) FROM feeds", [], |r| r.get(0))
            .unwrap();
        assert_eq!(feeds, 0, "校验失败时不得插入 feed");
    }

    /// P1-1 回归锚：校验不得误伤两条既有正常路径——
    /// ① None → 「未分类」兜底（与 republishing_a_url_clears_its_tombstone /
    ///    duplicate_url_is_rejected 的既有 None 路径同源，这里显式断言目录名）；
    /// ② Some(存在的分类 id) → 正常挂到该分类。
    #[test]
    fn add_feed_folder_validation_keeps_valid_paths() {
        let conn = test_conn();
        let parsed = minimal_parsed();
        // ① None：未分类兜底
        let row = persist_new_feed(
            &conn,
            "https://a.example/rss",
            &parsed,
            None,
            None,
            None,
            None,
            "inherit",
            db::FeedAiMode::Off,
            db::FeedAiMode::Off,
            false,
        )
        .unwrap();
        let folder = db::list_folders(&conn)
            .unwrap()
            .into_iter()
            .find(|f| f.id == row.folder_id)
            .expect("兜底目录必须存在");
        assert_eq!(folder.name, "未分类", "None 必须落「未分类」兜底");
        // ② Some(存在)：正常入库
        let fid = db::create_folder(&conn, "技术", "article").unwrap();
        let row2 = persist_new_feed(
            &conn,
            "https://b.example/rss",
            &parsed,
            None,
            None,
            None,
            Some(fid),
            "inherit",
            db::FeedAiMode::Off,
            db::FeedAiMode::Off,
            false,
        )
        .unwrap();
        assert_eq!(row2.folder_id, fid, "存在的分类 id 必须原样采用");
    }

    /// 重复 URL 仍被拒绝（既有行为锚定，抽取不改变查重）
    #[test]
    fn duplicate_url_is_rejected() {
        let conn = test_conn();
        let parsed = minimal_parsed();
        persist_new_feed(
            &conn,
            "https://f.example/rss",
            &parsed,
            None,
            None,
            None,
            None,
            "inherit",
            db::FeedAiMode::On,
            db::FeedAiMode::Off,
            false,
        )
        .unwrap();
        let err = persist_new_feed(
            &conn,
            "https://f.example/rss",
            &parsed,
            None,
            None,
            None,
            None,
            "inherit",
            db::FeedAiMode::On,
            db::FeedAiMode::Off,
            false,
        )
        .unwrap_err();
        assert_eq!(err.code, "duplicate");
    }

    /// P3[9]（REQ-104）：**带饰词**的同一订阅也必须判重。
    ///
    /// 修前用精确匹配，`https://f.example/rss?utm_source=x` 或 `http://www.f.example/rss/`
    /// 都会被当作新源插入 → 同一订阅出现两个 feed（文章翻倍、状态分裂）。
    #[test]
    fn dressed_up_duplicate_url_is_rejected() {
        let conn = test_conn();
        let parsed = minimal_parsed();
        persist_new_feed(
            &conn,
            "https://f.example/rss",
            &parsed,
            None,
            None,
            None,
            None,
            "inherit",
            db::FeedAiMode::On,
            db::FeedAiMode::Off,
            false,
        )
        .unwrap();

        for variant in [
            "https://f.example/rss?utm_source=newsletter", // 跟踪参数
            "http://f.example/rss",                        // 协议差异（https→http）
            "https://www.f.example/rss",                   // www. 前缀
            "https://f.example/rss/",                      // 尾斜杠
        ] {
            let err = persist_new_feed(
                &conn,
                variant,
                &parsed,
                None,
                None,
                None,
                None,
                "inherit",
                db::FeedAiMode::On,
                db::FeedAiMode::Off,
                false,
            )
            .unwrap_err();
            assert_eq!(err.code, "duplicate", "变体 {variant} 必须被判重");
        }
    }

    /// P3-1（自检 2026-09-29）：入库段中途失败必须**整体回滚**——feed 不残留、
    /// 已 upsert 的文章不残留、事务内的墓碑清除被撤销。生产代码无注入点，用
    /// 测试侧 RAISE(ABORT) 触发器在第 2 篇文章 upsert 处注入真实失败
    /// （SQLITE_CONSTRAINT 走的就是生产 Err 路径；修前逐条自动提交会残留半套）。
    #[test]
    fn persist_new_feed_midway_failure_rolls_back_everything() {
        let conn = test_conn();
        conn.execute_batch(
            "CREATE TRIGGER test_fail_second_article BEFORE INSERT ON articles
             WHEN NEW.guid = 'boom' BEGIN
                 SELECT RAISE(ABORT, 'simulated midway failure');
             END;",
        )
        .unwrap();
        db::add_feed_tombstone(&conn, "https://f.example/rss").unwrap();

        let mut parsed = minimal_parsed();
        parsed.articles.push(db::NewArticle {
            guid: "boom".into(),
            url: Some("https://e.example/boom".into()),
            title: "boom".into(),
            author: None,
            summary: None,
            content_html: None,
            body_text: String::new(),
            image_url: None,
            enclosure_url: None,
            enclosure_mime: None,
            duration_sec: None,
            published_at: Some("2025-01-01T00:00:00+00:00".into()),
            source: "direct".into(),
        });

        let err = persist_new_feed(
            &conn,
            "https://f.example/rss",
            &parsed,
            None,
            None,
            None,
            None,
            "inherit",
            db::FeedAiMode::On,
            db::FeedAiMode::Off,
            false,
        )
        .unwrap_err();
        assert!(
            err.message.contains("simulated midway failure"),
            "必须是注入的约束失败现场: {err}"
        );

        let feeds: i64 = conn
            .query_row("SELECT COUNT(*) FROM feeds", [], |r| r.get(0))
            .unwrap();
        assert_eq!(feeds, 0, "中途失败必须整体回滚：feed 不得残留（修前残留）");
        let articles: i64 = conn
            .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            articles, 0,
            "中途失败必须整体回滚：先于失败点 upsert 的文章也不得残留"
        );
        assert!(
            db::feed_tombstones(&conn)
                .unwrap()
                .contains(&"http://f.example/rss".to_string()),
            "事务内的墓碑清除必须被回滚（否则半套状态破坏删除语义）"
        );
        let queued: i64 = conn
            .query_row("SELECT COUNT(*) FROM sync_queue", [], |r| r.get(0))
            .unwrap();
        assert_eq!(queued, 0, "add_feed 队项不得残留");
    }

    /* ---------- OPT-014 R2：record_* 的可失败检查必须先于本地副作用 ---------- */

    /// 凭据读取器置为不可用：坏密文（读时 Err，不是「未配置」）。
    fn corrupt_sync_credentials(conn: &rusqlite::Connection) {
        db::set_setting(conn, "greader_endpoint", "https://example.com").unwrap();
        db::set_setting(conn, "greader_username", "u").unwrap();
        db::set_setting(conn, "greader_password", "pw").unwrap();
        conn.execute(
            "UPDATE settings SET value = 'dpapi:@@corrupt@@' WHERE key = 'greader_password'",
            [],
        )
        .unwrap();
        assert!(
            crate::sync::read_credentials(conn).is_err(),
            "前置：坏密文必须让凭据读取失败（不是 Ok(None)）"
        );
    }

    /// 一个已绑定远端的本地订阅 + 一篇文章。
    fn seed_feed_with_article(conn: &rusqlite::Connection) -> (i64, i64) {
        let folder = db::create_folder(conn, "分类", "article").unwrap();
        let feed = db::insert_feed(
            conn,
            "https://r2.example/feed.xml",
            None,
            "旧标题",
            None,
            folder,
            "inherit",
            false,
            false,
        )
        .unwrap();
        let a = db::NewArticle {
            guid: "r2-a1".into(),
            url: Some("https://r2.example/a1".into()),
            title: "A".into(),
            author: None,
            summary: None,
            content_html: Some("<p>a</p>".into()),
            body_text: "a".into(),
            image_url: None,
            enclosure_url: None,
            enclosure_mime: None,
            duration_sec: None,
            published_at: Some(chrono::Utc::now().to_rfc3339()),
            source: "direct".into(),
        };
        let (aid, _) = db::upsert_article_with_feed(conn, feed, &a, false).unwrap();
        db::set_feed_remote_id(conn, feed, 10).unwrap();
        (feed, aid)
    }

    /// R2 P2：坏密文时删除必须整单失败且**先于任何本地副作用**——订阅/文章/
    /// 墓碑原样（修前：墓碑已落、订阅已删，才发现凭据读取失败）。
    #[test]
    fn record_feed_deletion_fails_before_local_side_effects_on_corrupt_credentials() {
        let conn = test_conn();
        let (feed, aid) = seed_feed_with_article(&conn);
        corrupt_sync_credentials(&conn);

        let err = record_feed_deletion(&conn, feed).expect_err("坏密文必须让删除整体失败");
        assert!(
            err.code == "credentialCorrupt" || err.code == "credentialDecrypt",
            "应为凭据解密错误：{err}"
        );

        let feed_left: i64 = conn
            .query_row("SELECT COUNT(*) FROM feeds WHERE id = ?1", [feed], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(feed_left, 1, "删除失败后订阅必须原样");
        let art_left: i64 = conn
            .query_row("SELECT COUNT(*) FROM articles WHERE id = ?1", [aid], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(art_left, 1, "删除失败后文章必须原样");
        assert_eq!(
            db::feed_tombstones(&conn).unwrap(),
            Vec::<String>::new(),
            "删除失败后不得留下墓碑"
        );
    }

    /// R2 P2：坏密文时编辑必须整单失败且字段原样（修前：update_feed 已生效）。
    #[test]
    fn record_feed_edit_fails_before_local_side_effects_on_corrupt_credentials() {
        let conn = test_conn();
        let (feed, _aid) = seed_feed_with_article(&conn);
        let other = db::create_folder(&conn, "另一个分类", "social").unwrap();
        corrupt_sync_credentials(&conn);

        let err = record_feed_edit(
            &conn,
            feed,
            Some("新标题"),
            Some(other),
            Some("social"),
            Some(true),
            Some(true),
            None,
            None,
        )
        .expect_err("坏密文必须让编辑整体失败");
        assert!(
            err.code == "credentialCorrupt" || err.code == "credentialDecrypt",
            "应为凭据解密错误：{err}"
        );

        let row = db::list_feeds(&conn)
            .unwrap()
            .into_iter()
            .find(|f| f.id == feed)
            .expect("订阅应仍在");
        assert_eq!(row.title, "旧标题", "编辑失败后标题必须原样");
        assert_eq!(row.layout, "inherit", "编辑失败后布局必须原样");
        assert!(!row.auto_summary, "编辑失败后 auto_summary 必须原样");
        assert!(!row.auto_translate, "编辑失败后 auto_translate 必须原样");
        assert_ne!(row.folder_id, other, "编辑失败后分类不得被移动");
    }

    /* ---------- OPT-011：AI 开关输入的合并规则与三态落库 ---------- */

    /// resolve_ai_modes：new caller 的 mode 优先；旧 caller 只传 bool 时映射
    /// 显式 on/off；两者皆缺 = 全新源默认 inherit；非法 mode 串拒绝。
    #[test]
    fn resolve_ai_modes_prefers_mode_then_legacy_bool_then_inherit() {
        assert_eq!(
            resolve_ai_modes(Some("on"), Some("inherit"), Some(false), Some(false)).unwrap(),
            (db::FeedAiMode::On, db::FeedAiMode::Inherit),
            "mode 是 new caller 的真值（压过同帧的旧 bool）"
        );
        assert_eq!(
            resolve_ai_modes(None, None, Some(true), Some(false)).unwrap(),
            (db::FeedAiMode::On, db::FeedAiMode::Off),
            "旧 caller 只传 bool → 显式 on/off（不引入 inherit）"
        );
        assert_eq!(
            resolve_ai_modes(None, None, None, None).unwrap(),
            (db::FeedAiMode::Inherit, db::FeedAiMode::Inherit),
            "两者皆缺 = 新增源默认继承（默认分类仍 false，先不烧 AI）"
        );
        let err = resolve_ai_modes(Some("maybe"), None, None, None).unwrap_err();
        assert_eq!(err.code, "validate", "非法 mode 必须拒绝且不悄悄回默认");
    }

    /// 新 UI 路径：persist_new_feed 收 inherit → 三态列 inherit、旧 bool 镜像 0；
    /// 旧 caller 路径（On/Off）语义不变。
    #[test]
    fn persist_new_feed_keeps_inherit_default_and_explicit_modes() {
        let conn = test_conn();
        let parsed = minimal_parsed();
        let row = persist_new_feed(
            &conn,
            "https://f.example/rss",
            &parsed,
            None,
            None,
            None,
            None,
            "inherit",
            db::FeedAiMode::Inherit,
            db::FeedAiMode::Inherit,
            false,
        )
        .unwrap();
        assert_eq!(
            (row.summary_mode.as_str(), row.translate_mode.as_str()),
            ("inherit", "inherit"),
            "新 UI 默认 inherit 必须原样落库（真值不是 bool 回显）"
        );
        assert!(
            !row.auto_summary && !row.auto_translate,
            "bool 兼容列镜像 0"
        );
        let inherit_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM feeds WHERE summary_mode = 'inherit'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(inherit_count, 1, "库内真值列确实写了 inherit");
    }

    /// record_feed_edit 的新真值入口：只改一个 mode、其余字段不动；
    /// 显式 off 能压过分类开启（写库层只存选择，解析在前端 resolver——见 Note）。
    #[test]
    fn record_feed_edit_writes_explicit_modes_without_touching_other_fields() {
        let conn = test_conn();
        let (feed, _aid) = seed_feed_with_article(&conn);
        let push = record_feed_edit(
            &conn,
            feed,
            None,
            None,
            None,
            None,
            None,
            Some(db::FeedAiMode::Off),
            Some(db::FeedAiMode::Inherit),
        )
        .unwrap();
        assert!(push.is_none(), "未配置同步时无远端推送目标");
        let row = db::list_feeds(&conn)
            .unwrap()
            .into_iter()
            .find(|f| f.id == feed)
            .unwrap();
        assert_eq!(
            (row.summary_mode.as_str(), row.translate_mode.as_str()),
            ("off", "inherit"),
            "显式关闭与继承态分别原样落库"
        );
        assert_eq!(row.title, "旧标题", "未给的字段不被连带改动");
        // 非法 mode 不可能经此入口进入：parse 在命令层、单测这里直接给类型化枚举
    }
}

/// 删除订阅的本地记录 + 删除墓碑（命令与测试共用的真实逻辑）。
/// 返回 Some((remote_id, feed_url, generation))：该订阅已绑定远端且同步已配置，
/// 调用方应即时尝试一次推送（**真正的持久意图在下面的事务里**）。
///
/// `generation` 是读取 remote_id 同一持锁内捕获的账号代际（OPT-006）——退货
/// 路径复核用；typed 队项自带账号 key/代际，执行器按 (key, generation) 屏障发送。
///
/// OPT-014 R2：可失败的凭据读取前移到**任何本地副作用之前**并复用结果——
/// 坏密文时整单失败，不留下「订阅已删 + 墓碑已落」却报错的半套状态。
///
/// OPT-009 阶段 B2：删除墓碑、本地删除与 **typed unsubscribe 入队**同一事务
/// （全有全无）——离线/重启后执行器按队列重试，不再依赖一次性 best-effort；
/// 队项只认当前账号 key，换号后不会把旧账号的 remote_id 发给新账号；
/// subscription_outbox 与 feeds 无外键，本地订阅消亡不带走退订意图。
///
/// Note: .agents/notes/implemented/architecture/2026-10-08-凭据失败关闭与受控更新检查.md
pub fn record_feed_deletion(
    conn: &rusqlite::Connection,
    id: i64,
) -> AppResult<Option<(i64, String, i64)>> {
    let sync_ok = sync_configured(conn)?;
    let generation = db::sync_generation(conn)?;
    let (feed_url, remote_id) = db::feed_remote_info(conn, id)?;
    // B2-R2/R2-3：删除时快照「远端当前 label」的移除基准（本地当前分类名）——
    // 退订未确认前同一 URL 被快速重加时，后继 subscribe 行继承它做 r= 移除基准，
    // 否则远端会残留这个旧 label（挂载只发 a= 是追加分类）。
    let folder_label = db::feed_folder_label(conn, id)?;
    let tx = conn.unchecked_transaction()?;
    // A-1：墓碑先落，退订失败也不得让 pull 把已删订阅拉回来
    db::add_feed_tombstone(&tx, &feed_url)?;
    // B2-2：typed 退订意图与墓碑/本地删除同生共死；凭据读取失败 → Err 整体回滚
    // （失败关闭：不留下半套状态）。未连接 → 仅本地（无账号 key 可入队）。
    if let Some(ctx) = crate::sync::current_account(&tx)? {
        db::enqueue_unsubscribe(
            &tx,
            ctx.generation,
            &ctx.key,
            &feed_url,
            remote_id,
            folder_label.as_deref(),
        )?;
    }
    db::delete_feed(&tx, id)?;
    tx.commit()?;
    Ok(if sync_ok {
        remote_id.map(|rid| (rid, feed_url, generation))
    } else {
        None
    })
}

#[tauri::command]
pub async fn delete_feed(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    // 删除语义（SUB-4/SYN-1）：本地删除 ≠ 强删远端。
    // 不建 remove_feed 队项（该动作从未被 sync.rs 消费，只会累积僵尸队列）。
    let unsubscribe = {
        let conn = state.db.lock().await;
        record_feed_deletion(&conn, id)?
    };
    // 锁外即时尝试一次订阅写回执行器（与 update_feed 同一 UX 形态）：退订意图
    // 已在上面的事务里持久化，这次尝试失败只是留给下一轮同步重试。
    // 注意：请求成功仅意味着后端接受（2xx），**不代表远端已删除**——墓碑仍只由
    // pull 在「远端列表确认已不含该 URL」时收敛清除（TASK-055）。
    // OPT-006：执行器的代际屏障内建（会话构建 + 每次回写前复核），换号后
    // 不会把旧账号的 remote_id 发给新账号。
    if unsubscribe.is_some() {
        crate::sync::try_push_subscription_outbox(&state.db, &state.http).await;
    }
    Ok(())
}

#[tauri::command]
pub async fn update_feed_layout(
    state: State<'_, AppState>,
    id: i64,
    layout: String,
) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::update_feed_layout(&conn, id, &layout)
}

/// 编辑源：标题/所属分类/布局/AI 开关一次性更新。
/// 连接 Miniflux 时改名与移动分类尽量同步到远端（best-effort，失败不阻塞本地落库结果）。
///
/// OPT-011：新增三态 `summary_mode` / `translate_mode` 可选参数（新 UI 真值）；
/// 旧 caller 仍可只传 bool（映射显式 on/off，见 db::update_feed_full）。
/// 非法 mode 在任何写入前拒绝。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn update_feed(
    state: State<'_, AppState>,
    id: i64,
    title: Option<String>,
    folder_id: Option<i64>,
    layout: Option<String>,
    auto_summary: Option<bool>,
    auto_translate: Option<bool>,
    summary_mode: Option<String>,
    translate_mode: Option<String>,
) -> AppResult<()> {
    let title = title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    // mode 校验先于持锁/任何写入：非法输入不产生半套状态
    let summary_mode = summary_mode
        .as_deref()
        .map(db::FeedAiMode::parse)
        .transpose()?;
    let translate_mode = translate_mode
        .as_deref()
        .map(db::FeedAiMode::parse)
        .transpose()?;

    let push = {
        let conn = state.db.lock().await;
        record_feed_edit(
            &conn,
            id,
            title.as_deref(),
            folder_id,
            layout.as_deref(),
            auto_summary,
            auto_translate,
            summary_mode,
            translate_mode,
        )?
    };
    // 锁外 best-effort 推送远端（GReader ac=edit）：OPT-009 阶段 B 起，
    // 编辑已在 record_feed_edit 的同一事务内入队订阅 outbox，此处立即尝试一次
    // 执行器推送（失败静默——队项保留，下一轮同步自动重试；不再出现
    // 「本地已改、网络失败后无持久意图」的丢失窗口）。
    // OPT-006：执行器的代际屏障内建（会话构建 + 每次回写前复核），换号后
    // 不会把旧账号的 remote_id 发给新账号。
    if push.is_some() {
        crate::sync::try_push_subscription_outbox(&state.db, &state.http).await;
    }
    Ok(())
}

/// 远端订阅编辑的推送目标：远端 id、新标题（None 表示不改标题）、目标分类名（None 表示未移动分类）、
/// 捕获代际（OPT-006：发送前复核，换号后不向新账号发送旧 remote_id）。
pub type FeedEditPush = (i64, Option<String>, Option<String>, i64);

/// 更新订阅并返回需推送远端的编辑目标（命令与测试共用的真实逻辑，A-2）。
/// 返回 Some((remote_id, 新标题, 目标分类名, generation))：该订阅已绑定远端且同步已配置。
/// generation 与 remote_id 同一次持锁内捕获（OPT-006）。
///
/// OPT-011：AI 开关支持三态 mode（新真值）；旧 bool 参数保留为兼容输入。
/// 合并规则与写入收口在 db::update_feed_full（单条 SQL，bool 镜像不分叉）。
///
/// OPT-014 R2：可失败的凭据读取前移到 `db::update_feed` 之前并复用结果——
/// 坏密文时整单失败，不留下「字段已改」却报错的半套状态。
///
/// OPT-009 阶段 B：
/// - 本地修改与订阅 outbox 入队**同一事务**（全有全无）；入队携带账号 key/代际，
///   由生产执行器按 (id, generation) 精确确认；
/// - 只发目标分类 `a=` 不是移动——入队同时携带编辑前的旧 label（wire 发 `r=`）；
/// - 本地尚无 remote_id（quick_add 已建、绑定未完成的 remote_created 窗口）时
///   并入 pending subscribe 行的目标字段，绑定义务不因编辑丢失；
/// - 目标信息（remote_id / 旧分类 label）在**写前**读取：读失败在本地修改之前
///   暴露，不会出现「本地已改却报错称未改」的谎言窗口（返回结构的语义化在 B2）。
///
/// Note: .agents/notes/implemented/architecture/2026-10-08-凭据失败关闭与受控更新检查.md
#[allow(clippy::too_many_arguments)]
pub fn record_feed_edit(
    conn: &rusqlite::Connection,
    id: i64,
    title: Option<&str>,
    folder_id: Option<i64>,
    layout: Option<&str>,
    auto_summary: Option<bool>,
    auto_translate: Option<bool>,
    summary_mode: Option<db::FeedAiMode>,
    translate_mode: Option<db::FeedAiMode>,
) -> AppResult<Option<FeedEditPush>> {
    // 账号身份 + 代际与凭据同源快照；Err（坏密文）在任何本地副作用之前上抛。
    let account = crate::sync::current_account(conn)?;
    // 目标分类必须存在（防 UI 传错 id 把源挂飞）
    if let Some(fid) = folder_id {
        if !db::folder_exists(conn, fid)? {
            return Err(AppError::new("validate", "目标分类不存在"));
        }
    }
    // 空标题 = 不改名（与 update_feed_full 的回退语义一致）
    let new_title = title.map(str::trim).filter(|t| !t.is_empty());
    // 写前预读推送目标：read 失败 → 整体 Err，且**尚未**触碰本地字段。
    let (feed_url, remote_id) = db::feed_remote_info(conn, id)?;
    let new_folder_label = match folder_id {
        Some(fid) => db::folder_name(conn, fid)?,
        None => None,
    };
    let prev_folder_label = if folder_id.is_some() {
        db::feed_folder_label(conn, id)?
    } else {
        None
    };
    // 移动语义：目标 label 与当前 label 相同不算移动（不发 a=/r=）
    let moved =
        folder_id.is_some() && new_folder_label.is_some() && new_folder_label != prev_folder_label;
    let desired_folder = if moved {
        new_folder_label.as_deref()
    } else {
        None
    };
    let old_label = if moved {
        prev_folder_label.as_deref()
    } else {
        None
    };
    let changed = new_title.is_some() || moved;

    let tx = conn.unchecked_transaction()?;
    db::update_feed_full(
        &tx,
        id,
        new_title,
        folder_id,
        layout,
        auto_summary,
        auto_translate,
        summary_mode,
        translate_mode,
    )?;
    if changed {
        if let Some(ctx) = &account {
            match remote_id {
                Some(rid) => {
                    db::enqueue_edit(
                        &tx,
                        ctx.generation,
                        &ctx.key,
                        &feed_url,
                        rid,
                        new_title,
                        desired_folder,
                        old_label,
                    )?;
                }
                None => {
                    // remote_created 窗口：编辑并入 pending subscribe 的目标字段
                    // （本地已入库的目标以订阅行的 desired_* 为准，绑定后补推）。
                    // B2-3：移动同时携带旧 label（远端挂载若已按旧 label 发过，
                    // 需要 r= 移除；未发过时是无害 no-op）。
                    db::update_pending_subscribe_desired(
                        &tx,
                        &ctx.key,
                        &feed_url,
                        new_title,
                        desired_folder,
                        old_label,
                    )?;
                }
            }
        }
    }
    tx.commit()?;

    if !changed {
        return Ok(None);
    }
    match (account, remote_id) {
        (Some(ctx), Some(rid)) => Ok(Some((
            rid,
            new_title.map(str::to_string),
            new_folder_label,
            ctx.generation,
        ))),
        // 未配置同步 / 未绑定远端（含 remote_created 窗口）：本地已接受，无直接推送目标
        _ => Ok(None),
    }
}

#[tauri::command]
pub async fn set_feed_ai_flags(
    state: State<'_, AppState>,
    id: i64,
    summary: bool,
    translate: bool,
) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::set_feed_ai_flags(&conn, id, summary, translate)
}

/// OPT-011：设置源级 AI 开关三态（新真值入口；设置页三态控件等使用）。
/// 只动给出的字段（None = 不改）；只写本地（AI 开关是本地配置，不推远端，
/// 与旧 set_feed_ai_flags 的边界一致）。非法 mode 拒绝，不悄悄回默认。
#[tauri::command]
pub async fn set_feed_ai_modes(
    state: State<'_, AppState>,
    id: i64,
    summary_mode: Option<String>,
    translate_mode: Option<String>,
) -> AppResult<()> {
    let summary_mode = summary_mode
        .as_deref()
        .map(db::FeedAiMode::parse)
        .transpose()?;
    let translate_mode = translate_mode
        .as_deref()
        .map(db::FeedAiMode::parse)
        .transpose()?;
    if summary_mode.is_none() && translate_mode.is_none() {
        return Err(AppError::new(
            "validate",
            "至少给出一个要更新的 AI 开关模式",
        ));
    }
    let conn = state.db.lock().await;
    db::set_feed_ai_modes(&conn, id, summary_mode, translate_mode)
}
