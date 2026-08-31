//! Tauri IPC 命令面：前端 store 经 invoke 调用这里。
//! 每个命令短小：拿锁 → db:: 类型化函数 → 返回 Serialize 行类型。

use crate::db::{self, ArticleQuery};
use crate::error::{AppError, AppResult};
use crate::ingestion;
use crate::state::AppState;
use rusqlite::OptionalExtension;
use serde::Deserialize;
use tauri::State;

/// 读 app_settings JSON 里的 smartDedup 开关（默认关：保持既有抓取行为）。
pub(crate) fn read_dedup_flag(conn: &rusqlite::Connection) -> bool {
    db::get_setting(conn, "app_settings")
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("smartDedup").and_then(|f| f.as_bool()))
        .unwrap_or(false)
}

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

#[tauri::command]
pub async fn rename_folder(state: State<'_, AppState>, id: i64, name: String) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::rename_folder(&conn, id, &name)
}

#[tauri::command]
pub async fn delete_folder(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::delete_folder(&conn, id)
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

#[tauri::command]
pub async fn set_folder_ai_flags(
    state: State<'_, AppState>,
    id: i64,
    summary: bool,
    translate: bool,
) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::set_folder_ai_flags(&conn, id, summary, translate)
}

#[tauri::command]
pub async fn list_feeds(state: State<'_, AppState>) -> AppResult<Vec<db::FeedRow>> {
    let conn = state.db.lock().await;
    db::list_feeds(&conn)
}

/// 添加订阅源：先直连抓一次验证 URL 是有效 feed，成功才入库（不依赖 Miniflux）
#[tauri::command]
pub async fn add_feed(
    state: State<'_, AppState>,
    feed_url: String,
    title: Option<String>,
    folder_id: i64,
    layout: String,
    auto_summary: bool,
    auto_translate: bool,
) -> AppResult<db::FeedRow> {
    // 1. 抓取验证（直连，第一优先级）
    let fetched = ingestion::conditional_get(&state.http, &feed_url, None, None).await?;
    let (bytes, etag, last_modified) = match fetched {
        ingestion::Fetched::NotModified => {
            return Err(AppError::new("parse", "unexpected 304 on first fetch"))
        }
        ingestion::Fetched::Body { bytes, etag, last_modified, .. } => (bytes, etag, last_modified),
    };
    let parsed = ingestion::parse_feed(&bytes, &feed_url)?;

    // 2. 入库（feed 元数据 + 全量条目，source='direct'）
    let conn = state.db.lock().await;
    if db::find_feed_by_url(&conn, &feed_url)?.is_some() {
        return Err(AppError::new("duplicate", "该订阅地址已存在"));
    }
    let final_title = title.filter(|t| !t.trim().is_empty())
        .or(parsed.title.clone())
        .unwrap_or_else(|| feed_url.clone());
    let feed_id = db::insert_feed(
        &conn,
        &feed_url,
        parsed.site_url.as_deref(),
        &final_title,
        parsed.icon.as_deref(),
        folder_id,
        &layout,
        auto_summary,
        auto_translate,
    )?;
    db::set_feed_fetch_state(&conn, feed_id, false, None, etag.as_deref(), last_modified.as_deref())?;
    let dedup = read_dedup_flag(&conn);
    for a in &parsed.articles {
        db::upsert_article_with_feed(&conn, feed_id, a, dedup)?;
    }
    // 连接了 Miniflux → 入队推送新订阅
    if sync_configured(&conn) {
        let payload = serde_json::json!({ "folder_id": folder_id }).to_string();
        db::enqueue_sync(&conn, None, Some(&feed_url), "add_feed", Some(&payload))?;
    }
    let row = db::list_feeds(&conn)?.into_iter().find(|f| f.id == feed_id)
        .ok_or_else(|| AppError::internal("feed row vanished after insert"))?;
    Ok(row)
}

#[tauri::command]
pub async fn delete_feed(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    let conn = state.db.lock().await;
    // 删除前取 URL 入同步队列（连接 Miniflux 后补推 remove_feed，服务端同步删）
    let url: Option<String> = conn
        .query_row(
            "SELECT feed_url FROM feeds WHERE id = ?1",
            rusqlite::params![id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(url) = url {
        db::enqueue_sync(&conn, None, Some(&url), "remove_feed", None)?;
    }
    db::delete_feed(&conn, id)
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

#[tauri::command]
pub async fn move_feed(
    state: State<'_, AppState>,
    id: i64,
    folder_id: i64,
) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::move_feed(&conn, id, folder_id)
}

/* ============================================================
   Articles
   ============================================================ */

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArticleListArgs {
    pub feed_id: Option<i64>,
    pub folder_id: Option<i64>,
    pub only_unread: Option<bool>,
    pub only_starred: Option<bool>,
    pub only_today: Option<bool>,
    pub newest_first: Option<bool>,
    pub limit: Option<i64>,
}

#[tauri::command]
pub async fn list_articles(
    state: State<'_, AppState>,
    args: ArticleListArgs,
) -> AppResult<Vec<db::ArticleListItem>> {
    let conn = state.db.lock().await;
    db::list_articles(
        &conn,
        &ArticleQuery {
            feed_id: args.feed_id,
            folder_id: args.folder_id,
            only_unread: args.only_unread.unwrap_or(false),
            only_starred: args.only_starred.unwrap_or(false),
            only_today: args.only_today.unwrap_or(false),
            newest_first: args.newest_first.unwrap_or(true),
            limit: args.limit.unwrap_or(500),
        },
    )
}

#[tauri::command]
pub async fn get_article(
    state: State<'_, AppState>,
    id: i64,
) -> AppResult<Option<db::ArticleRow>> {
    let conn = state.db.lock().await;
    db::get_article(&conn, id)
}

/// 全文搜索（FTS5）：标题/正文/作者/AI 摘要/翻译
#[tauri::command]
pub async fn search_articles(
    state: State<'_, AppState>,
    query: String,
    limit: Option<i64>,
) -> AppResult<Vec<db::ArticleListItem>> {
    let conn = state.db.lock().await;
    db::search_articles(&conn, &query, limit.unwrap_or(50))
}

#[tauri::command]
pub async fn set_read(state: State<'_, AppState>, id: i64, read: bool) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::set_read(&conn, id, read)?;
    // 连接了 Miniflux 才入队；未连接时纯本地生效（连接后首 Pull 全量对齐）
    if sync_configured(&conn) {
        db::enqueue_sync(
            &conn,
            Some(id),
            None,
            if read { "read" } else { "unread" },
            None,
        )?;
    }
    Ok(())
}

#[tauri::command]
pub async fn set_starred(state: State<'_, AppState>, id: i64, starred: bool) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::set_starred(&conn, id, starred)?;
    if sync_configured(&conn) {
        db::enqueue_sync(
            &conn,
            Some(id),
            None,
            if starred { "star" } else { "unstar" },
            None,
        )?;
    }
    Ok(())
}

fn sync_configured(conn: &rusqlite::Connection) -> bool {
    crate::sync::read_credentials(conn).is_some()
}

#[tauri::command]
pub async fn mark_all_read(
    state: State<'_, AppState>,
    feed_id: Option<i64>,
    folder_id: Option<i64>,
) -> AppResult<usize> {
    let conn = state.db.lock().await;
    let n = db::mark_all_read(&conn, feed_id, folder_id)?;
    if sync_configured(&conn) {
        // 逐条入队（量级可控：个人订阅日常几十条）
        let mut sql = String::from("SELECT id FROM articles WHERE is_read = 0");
        if let Some(fid) = feed_id {
            sql.push_str(&format!(" AND feed_id = {fid}"));
        }
        if let Some(f) = folder_id {
            sql.push_str(&format!(" AND feed_id IN (SELECT id FROM feeds WHERE folder_id = {f})"));
        }
        let ids: Vec<i64> = {
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        for id in ids {
            db::enqueue_sync(&conn, Some(id), None, "read", None)?;
        }
    }
    Ok(n)
}

#[tauri::command]
pub async fn feed_counts(state: State<'_, AppState>) -> AppResult<Vec<db::FeedCounts>> {
    let conn = state.db.lock().await;
    db::feed_counts(&conn)
}

/* ============================================================
   刷新（直连优先）
   ============================================================ */

/// 刷新单个订阅源（直连）
#[tauri::command]
pub async fn refresh_feed(state: State<'_, AppState>, feed_id: i64) -> AppResult<usize> {
    let client = state.http.clone();
    // HTTP 在锁外执行（网络 IO 不占数据库写锁）
    let mut conn = state.db.lock().await;
    let dedup = read_dedup_flag(&conn);
    ingestion::refresh_feed(&mut conn, &client, feed_id, dedup).await
}

/// 刷新全部订阅源（直连，顺序执行避免并发风控）
#[tauri::command]
pub async fn refresh_all_feeds(state: State<'_, AppState>) -> AppResult<RefreshSummary> {
    let client = state.http.clone();
    let (feed_ids, dedup): (Vec<i64>, bool) = {
        let conn = state.db.lock().await;
        let ids = db::list_feeds(&conn)?.into_iter().map(|f| f.id).collect();
        (ids, read_dedup_flag(&conn))
    };

    let mut summary = RefreshSummary::default();
    for id in feed_ids {
        let mut conn = state.db.lock().await;
        match ingestion::refresh_feed(&mut conn, &client, id, dedup).await {
            Ok(n) => summary.new_articles += n,
            Err(_e) => summary.failed_feeds += 1,
        }
    }
    Ok(summary)
}

#[derive(serde::Serialize, Default)]
pub struct RefreshSummary {
    pub new_articles: usize,
    pub failed_feeds: usize,
}

/* ============================================================
   Settings
   ============================================================ */

#[tauri::command]
pub async fn get_setting(state: State<'_, AppState>, key: String) -> AppResult<Option<String>> {
    let conn = state.db.lock().await;
    db::get_setting(&conn, &key)
}

#[tauri::command]
pub async fn set_setting(state: State<'_, AppState>, key: String, value: String) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::set_setting(&conn, &key, &value)
}

/* ============================================================
   全文提取（Readability）
   ============================================================ */

/// 全文提取：拉文章网页 → Readability 抽正文 → 覆盖该条目 content_html
/// （「默认打开方式=自动全文」：RSS 摘要型源打开时自动触发）。
#[tauri::command]
pub async fn extract_fulltext(state: State<'_, AppState>, article_id: i64) -> AppResult<String> {
    let url: Option<String> = {
        let conn = state.db.lock().await;
        conn.query_row(
            "SELECT url FROM articles WHERE id = ?1",
            rusqlite::params![article_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten()
    };
    let Some(url) = url.filter(|u| !u.trim().is_empty()) else {
        return Err(AppError::not_found("该条目没有原文网页地址"));
    };

    // 拉网页（复用抓取 client；30s 超时）
    let resp = state
        .http
        .get(&url)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(AppError::network(format!("网页拉取失败：HTTP {}", resp.status())));
    }
    let html = resp.text().await?;

    // Readability 不是 Send → spawn_blocking 里跑
    let base = url.clone();
    let html2 = html.clone();
    let extracted = tokio::task::spawn_blocking(move || crate::extraction::extract_article(&html, &base))
        .await
        .map_err(|e| AppError::internal(format!("blocking task: {e}")))??;

    // 头图兜底（正文没封面时）
    let base2 = url.clone();
    let image = tokio::task::spawn_blocking(move || crate::extraction::lead_image(&html2, &base2))
        .await
        .map_err(|e| AppError::internal(format!("blocking task: {e}")))?
        .unwrap_or_default();

    // 落库覆盖正文（全文 > RSS 摘要）+ 置提取标志（按钮/设置状态共用）
    {
        let conn = state.db.lock().await;
        conn.execute(
            "UPDATE articles SET content_html = ?1, fulltext_extracted = 1 WHERE id = ?2",
            rusqlite::params![extracted, article_id],
        )?;
        if !image.is_empty() {
            conn.execute(
                "UPDATE articles SET image_url = COALESCE(image_url, ?1) WHERE id = ?2",
                rusqlite::params![image, article_id],
            )?;
        }
    }
    Ok(extracted)
}

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
    let mut report = OpmlImportReport { imported: 0, skipped: 0 };
    let conn = state.db.lock().await;

    // 目录名 → folder_id 缓存（一次导入内同名目录只建一次）
    let mut folder_ids: std::collections::HashMap<String, i64> = std::collections::HashMap::new();

    for f in &feeds {
        // 已存在（URL 碰撞）→ 跳过
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM feeds WHERE feed_url = ?1)",
                rusqlite::params![f.feed_url],
                |r| r.get(0),
            )
            .unwrap_or(false);
        if exists {
            report.skipped += 1;
            continue;
        }
        let folder_id = match f.folder.as_deref() {
            Some(name) => match folder_ids.get(name) {
                Some(id) => *id,
                None => {
                    let id = db::create_folder(&conn, name, "article")?;
                    folder_ids.insert(name.to_string(), id);
                    id
                }
            },
            None => db::create_folder(&conn, "导入", "article")?,
        };
        db::insert_feed(&conn, &f.feed_url, None, &f.title, None, folder_id, "inherit", true, false)?;
        // 新增订阅入同步队列（连接 Miniflux 后补推）
        db::enqueue_sync(&conn, None, Some(&f.feed_url), "add_feed", Some(&f.title))?;
        report.imported += 1;
    }
    Ok(report)
}

/// 导出 OPML：全部源 + 目录名 → OPML 文档字符串。
#[tauri::command]
pub async fn opml_export(state: State<'_, AppState>) -> AppResult<String> {
    let rows: Vec<(String, String, Option<String>)> = {
        let conn = state.db.lock().await;
        let mut stmt = conn.prepare(
            "SELECT f.title, f.feed_url, fo.name
             FROM feeds f LEFT JOIN folders fo ON f.folder_id = fo.id
             ORDER BY fo.name, f.title",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    crate::opml::build(&rows)
}

/* ============================================================
   Miniflux 同步
   ============================================================ */

/// 保存 Endpoint/Token 并测试连接。成功返回欢迎信息。
#[tauri::command]
pub async fn sync_connect(
    state: State<'_, AppState>,
    endpoint: String,
    token: String,
) -> AppResult<String> {
    // 先写凭据再测试（test_connection 读 settings）；锁不跨 await
    {
        let conn = state.db.lock().await;
        db::set_setting(&conn, "miniflux_endpoint", &endpoint)?;
        db::set_setting(&conn, "miniflux_token", &token)?;
    }
    let msg = crate::sync::test_connection(&endpoint, &token, &state.http).await?;
    // 连接成功即做一次全量同步（订阅合并）。失败时回滚凭据：
    // 否则设置页显示"已连接"但首次合并实际没跑，用户无从得知。
    {
        let mut conn = state.db.lock().await;
        if let Err(e) = crate::sync::sync_now(&mut conn, &state.http).await {
            db::set_setting(&conn, "miniflux_endpoint", "")?;
            db::set_setting(&conn, "miniflux_token", "")?;
            return Err(AppError::new(
                "syncFailed",
                &format!("连接成功但首次同步失败：{e}"),
            ));
        }
    }
    Ok(msg)
}

/// 断开连接（清凭据，本地数据不动）
#[tauri::command]
pub async fn sync_disconnect(state: State<'_, AppState>) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::set_setting(&conn, "miniflux_endpoint", "")?;
    db::set_setting(&conn, "miniflux_token", "")?;
    db::set_setting(&conn, "miniflux_last_sync", "0")?;
    Ok(())
}

/// 手动全量同步（侧栏刷新按钮 / 同步中心）
#[tauri::command]
pub async fn sync_now(state: State<'_, AppState>) -> AppResult<crate::sync::SyncReport> {
    let client = state.http.clone();
    let mut conn = state.db.lock().await;
    crate::sync::sync_now(&mut conn, &client).await
}

/// 同步配置状态（设置页显示用）
#[derive(serde::Serialize)]
pub struct SyncStatusInfo {
    pub connected: bool,
    pub endpoint: Option<String>,
    pub last_sync: i64,
}

#[tauri::command]
pub async fn sync_status(state: State<'_, AppState>) -> AppResult<SyncStatusInfo> {
    let conn = state.db.lock().await;
    let endpoint = db::get_setting(&conn, "miniflux_endpoint").ok().flatten()
        .filter(|e| !e.trim().is_empty());
    Ok(SyncStatusInfo {
        connected: endpoint.is_some(),
        endpoint,
        last_sync: db::last_sync_ts(&conn).unwrap_or(0),
    })
}

/* ============================================================
   AI 引擎（OpenAI 兼容：官方 / DeepSeek / GLM / newapi 中转）
   ============================================================ */

/// 推给前端的流式事件（camelCase）。
#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase", tag = "type", content = "data")]
pub enum AiEvent {
    Delta(String),
    Done,
    Error(String),
}

/// 读 ai_config JSON；未配置时报 aiNotConfigured。
async fn load_ai_config(state: &State<'_, AppState>) -> AppResult<crate::ai::AiConfig> {
    let raw = {
        let conn = state.db.lock().await;
        db::get_setting(&conn, "ai_config").ok().flatten()
    };
    match raw {
        Some(json) => crate::ai::AiConfig::from_json(&json),
        None => Err(AppError::new("aiNotConfigured", "请先在设置中配置 AI 服务")),
    }
}

/// 保存 AI 配置（前端 AI tab 表单）。value 为整份 JSON。
#[tauri::command]
pub async fn save_ai_config(state: State<'_, AppState>, value: String) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::set_setting(&conn, "ai_config", &value)
}

/// 读 AI 配置（前端启动时恢复表单）。
#[tauri::command]
pub async fn get_ai_config(state: State<'_, AppState>) -> AppResult<Option<String>> {
    let conn = state.db.lock().await;
    Ok(db::get_setting(&conn, "ai_config").ok().flatten())
}

/// 连通性测试 + 拉模型列表（官方与 newapi 都支持 /models）。
#[tauri::command]
pub async fn ai_list_models(
    state: State<'_, AppState>,
    base_url: String,
    api_key: String,
) -> AppResult<Vec<String>> {
    let cfg = crate::ai::AiConfig {
        api_key: api_key.trim().to_string(),
        model: String::new(),
        base_url: base_url.trim().trim_end_matches('/').to_string(),
    };
    crate::ai::list_models(&state.http, &cfg).await
}

/// 提示词默认文案（用户未自定义时）。设计文档承诺的默认行为。
const DEFAULT_SUMMARIZE_SYSTEM: &str = "你是一名资讯编辑。请用简洁的中文总结这篇文章，输出 3-5 个要点，每个要点一行，以 - 开头。不要重复文章标题。";
const DEFAULT_TRANSLATE_SYSTEM: &str = "你是一名专业译者。请把用户提供的 HTML 片段翻译成简体中文：保留所有 HTML 标签和属性原样不动，只翻译标签内的文本内容。直接输出翻译后的 HTML，不要任何解释或代码块包裹。";

/// 读 ai_config JSON 里用户自定义的提示词；未配置用默认。
async fn load_prompts(state: &State<'_, AppState>) -> (String, String) {
    let raw = {
        let conn = state.db.lock().await;
        db::get_setting(&conn, "ai_config").ok().flatten()
    };
    match raw.as_deref().and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok()) {
        Some(v) => {
            let summary = v["summaryPrompt"].as_str().unwrap_or(DEFAULT_SUMMARIZE_SYSTEM).to_string();
            let translate = v["translatePrompt"].as_str().unwrap_or(DEFAULT_TRANSLATE_SYSTEM).to_string();
            (summary, translate)
        }
        None => (DEFAULT_SUMMARIZE_SYSTEM.to_string(), DEFAULT_TRANSLATE_SYSTEM.to_string()),
    }
}

/// 流式摘要：读文章 → 已有缓存直接返回 → 否则调 AI 流式生成 → 落库。
/// on_channel 前端增量渲染；返回最终全文（前端也用于缓存判断）。
#[tauri::command]
pub async fn ai_summarize(
    state: State<'_, AppState>,
    article_id: i64,
    on_channel: tauri::ipc::Channel<AiEvent>,
) -> AppResult<String> {
    let cfg = load_ai_config(&state).await?;

    // 取文章内容（锁内快照，锁外跑网络）
    let (title, body, cached) = {
        let conn = state.db.lock().await;
        let row = conn
            .query_row(
                "SELECT title, COALESCE(body_text, ''), ai_summary FROM articles WHERE id = ?1",
                rusqlite::params![article_id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| AppError::not_found(format!("article {article_id}")))?;
        row
    };

    // 缓存命中：直接推给前端，不重算
    if let Some(summary) = cached.filter(|s| !s.trim().is_empty()) {
        let _ = on_channel.send(AiEvent::Delta(summary.clone()));
        let _ = on_channel.send(AiEvent::Done);
        return Ok(summary);
    }

    let user = format!("标题：{title}\n\n正文：\n{body}");
    let (system, _) = load_prompts(&state).await;
    let mut sink = |delta: &str| {
        let _ = on_channel.send(AiEvent::Delta(delta.to_string()));
        true // 前端关闭面板的场景由 Channel 自身丢弃处理
    };
    let outcome = crate::ai::stream_chat(
        &state.http,
        &cfg,
        &system,
        &user,
        &mut sink,
        crate::ai::SUMMARY_MAX_TOKENS,
    )
    .await?;

    if outcome.completed && !outcome.text.trim().is_empty() {
        // 落库缓存（失败不影响返回）
        let conn = state.db.lock().await;
        let _ = db::set_article_ai_fields(&conn, article_id, Some(&outcome.text), None);
    }
    let _ = on_channel.send(AiEvent::Done);
    Ok(outcome.text)
}

/// 流式翻译：同 ai_summarize 结构，产物写 translated_content。
#[tauri::command]
pub async fn ai_translate(
    state: State<'_, AppState>,
    article_id: i64,
    on_channel: tauri::ipc::Channel<AiEvent>,
) -> AppResult<String> {
    let cfg = load_ai_config(&state).await?;

    let (title, html, cached) = {
        let conn = state.db.lock().await;
        let row = conn
            .query_row(
                "SELECT title, COALESCE(content_html, ''), translated_content FROM articles WHERE id = ?1",
                rusqlite::params![article_id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| AppError::not_found(format!("article {article_id}")));
        match row {
            Ok(r) => r,
            Err(e) => return Err(e),
        }
    };

    if let Some(translated) = cached.filter(|s| !s.trim().is_empty()) {
        let _ = on_channel.send(AiEvent::Delta(translated.clone()));
        let _ = on_channel.send(AiEvent::Done);
        return Ok(translated);
    }

    if html.trim().is_empty() {
        return Err(AppError::not_found("文章无正文可翻译"));
    }

    let user = format!("标题：{title}\n\nHTML：\n{html}");
    let (_, system) = load_prompts(&state).await;
    let mut sink = |delta: &str| {
        let _ = on_channel.send(AiEvent::Delta(delta.to_string()));
        true
    };
    let outcome = crate::ai::stream_chat(
        &state.http,
        &cfg,
        &system,
        &user,
        &mut sink,
        crate::ai::TRANSLATE_MAX_TOKENS,
    )
    .await?;

    if outcome.completed && !outcome.text.trim().is_empty() {
        let conn = state.db.lock().await;
        let _ = db::set_article_ai_fields(&conn, article_id, None, Some(&outcome.text));
    }
    let _ = on_channel.send(AiEvent::Done);
    Ok(outcome.text)
}

#[cfg(test)]
mod ai_event_tests {
    use super::AiEvent;

    /// 前端 api.ts 按 {type, data} 解析；序列化必须严格一致（camelCase tag）。
    #[test]
    fn ai_event_serialization_matches_frontend() {
        let delta = serde_json::to_value(AiEvent::Delta("你好".into())).unwrap();
        assert_eq!(delta["type"], "delta");
        assert_eq!(delta["data"], "你好");
        let done = serde_json::to_value(AiEvent::Done).unwrap();
        assert_eq!(done["type"], "done");
        let err = serde_json::to_value(AiEvent::Error("失败".into())).unwrap();
        assert_eq!(err["type"], "error");
        assert_eq!(err["data"], "失败");
    }
}
