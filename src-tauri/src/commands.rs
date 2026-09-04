//! Tauri IPC 命令面：前端 store 经 invoke 调用这里。
//! 每个命令短小：拿锁 → db:: 类型化函数 → 返回 Serialize 行类型。

use crate::db::{self, ArticleQuery};
use crate::error::{AppError, AppResult};
use crate::ingestion;
use crate::miniflux::MinifluxClient;
use crate::state::AppState;
use rusqlite::OptionalExtension;
use serde::Deserialize;
use std::sync::atomic::{AtomicBool, Ordering};
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
   即时状态推送调度（防抖合批）
   set_read/set_starred/mark_all_read 入队后调 schedule_state_push：
   - AtomicBool 防重入：已有一个推送任务在飞时只标记"再来一轮"
   - 800ms 防抖：快速滚动批量标读只发一次 PUT
   - 失败静默（队列保留）→ 下次变更或下轮同步自动重推
   ============================================================ */

static STATE_PUSH_FLYING: AtomicBool = AtomicBool::new(false);
static STATE_PUSH_PENDING: AtomicBool = AtomicBool::new(false);
const STATE_PUSH_DEBOUNCE_MS: u64 = 800;

pub(crate) fn schedule_state_push(state: &AppState) {
    STATE_PUSH_PENDING.store(true, Ordering::SeqCst);
    if STATE_PUSH_FLYING.swap(true, Ordering::SeqCst) {
        return; // 已有任务在飞：它收尾时会看到 PENDING 再跑一轮
    }
    let db = state.db.clone();
    let http = state.http.clone();
    tauri::async_runtime::spawn(async move {
        // Drop 守卫：循环任何出口（含 push_states_now 内部未来可能出现的
        // panic 展开）都复位 FLYING——否则一次 panic 后推送永久静默
        struct FlyingGuard;
        impl Drop for FlyingGuard {
            fn drop(&mut self) {
                STATE_PUSH_FLYING.store(false, Ordering::SeqCst);
            }
        }
        let _guard = FlyingGuard;
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(STATE_PUSH_DEBOUNCE_MS)).await;
            if !STATE_PUSH_PENDING.swap(false, Ordering::SeqCst) {
                break;
            }
            crate::sync::push_states_now(&db, &http).await;
            // push 期间又入队 → 继续循环；否则退出并放行下一个调度
            if !STATE_PUSH_PENDING.load(Ordering::SeqCst) {
                break;
            }
        }
    });
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
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::new("validate", "分类名称不能为空"));
    }
    // 锁内：改名落库 + 收集远端映射；锁外：同步改名到 Miniflux（best-effort）
    let remote = {
        let conn = state.db.lock().await;
        let mf_id: Option<i64> = conn
            .query_row("SELECT miniflux_id FROM folders WHERE id = ?1", [id], |r| r.get(0))
            .ok()
            .flatten();
        let creds = if mf_id.is_some() && sync_configured(&conn) {
            crate::sync::read_credentials(&conn)
        } else {
            None
        };
        db::rename_folder(&conn, id, &name)?;
        mf_id.zip(creds)
    };
    if let Some((mf_id, (endpoint, token))) = remote {
        let client = MinifluxClient::new(&endpoint, &token, state.http.clone());
        client
            .rename_category(mf_id, &name)
            .await
            .map_err(|e| AppError::network(format!("同步分类改名到 Miniflux 失败: {e}")))?;
    }
    Ok(())
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

/// 添加订阅源：先直连抓一次验证 URL 是有效 feed，成功才入库（不依赖 Miniflux）。
/// `folder_id = None`（UI 未选分类，如全新安装无任何分类时）→ 自动落到
/// 「未分类」文件夹（不存在则创建）——首次使用添加源不再报错。
#[tauri::command]
pub async fn add_feed(
    state: State<'_, AppState>,
    feed_url: String,
    title: Option<String>,
    folder_id: Option<i64>,
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
    // 未选分类 → 「未分类」文件夹（无则建）。创建失败必须上抛——
    // 兜底到 id=1 会在 folder 1 不存在时触发外键违约，文章静默丢失。
    let folder_id = match folder_id {
        Some(fid) => fid,
        None => {
            let existing: Option<i64> = conn
                .query_row(
                    "SELECT id FROM folders WHERE name = '未分类' ORDER BY id LIMIT 1",
                    [],
                    |r| r.get(0),
                )
                .ok()
                .flatten();
            match existing {
                Some(fid) => fid,
                None => db::create_folder(&conn, "未分类", "article")?,
            }
        }
    };
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
    // 删除语义（SUB-4/SYN-1）：本地删除 ≠ 强删远端，避免误删服务端数据。
    // 不建 remove_feed 队项（该动作从未被 sync.rs 消费，只会累积僵尸队列）；
    // 若历史版本残留了 remove_feed 僵尸项，由 sync 阶段统一清理（见 sync.rs）。
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

/// 编辑源：标题/所属分类/布局/AI 开关一次性更新。
/// 连接 Miniflux 时改名与移动分类尽量同步到远端（best-effort，失败不阻塞本地落库结果）。
#[tauri::command]
pub async fn update_feed(
    state: State<'_, AppState>,
    id: i64,
    title: Option<String>,
    folder_id: Option<i64>,
    layout: Option<String>,
    auto_summary: Option<bool>,
    auto_translate: Option<bool>,
) -> AppResult<()> {
    let title = title.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());

    // 三段式锁纪律：锁内校验+读映射+落库，锁外执行 Miniflux 网络 IO——
    // 远端 30s 超时期间其他 DB 命令不被冻结（与 sync.rs/refresh_feed_staged 一致）
    let (mf_feed_id, mf_new_cat, creds) = {
        let conn = state.db.lock().await;
        // 目标分类必须存在（防 UI 传错 id 把源挂飞）
        if let Some(fid) = folder_id {
            let exists: bool = conn
                .query_row("SELECT COUNT(*) FROM folders WHERE id = ?1", [fid], |r| r.get::<_, i64>(0))
                .map(|n| n > 0)?;
            if !exists {
                return Err(AppError::new("validate", "目标分类不存在"));
            }
        }
        let configured = sync_configured(&conn);
        let mf_feed_id: Option<i64> = conn
            .query_row("SELECT miniflux_id FROM feeds WHERE id = ?1", [id], |r| r.get(0))
            .ok()
            .flatten();
        let creds = if configured && mf_feed_id.is_some() {
            crate::sync::read_credentials(&conn)
        } else {
            None
        };
        let mf_new_cat: Option<i64> = match (configured, folder_id) {
            (true, Some(fid)) => conn
                .query_row("SELECT miniflux_id FROM folders WHERE id = ?1", [fid], |r| r.get(0))
                .ok()
                .flatten(),
            _ => None,
        };
        db::update_feed(&conn, id, title.as_deref(), folder_id, layout.as_deref(), auto_summary, auto_translate)?;
        (mf_feed_id, mf_new_cat, creds)
    };

    // 锁外：远端改名 / 移动分类（best-effort，失败不阻塞本地结果）
    if let (Some(mf_id), Some((endpoint, token))) = (mf_feed_id, creds) {
        let client = MinifluxClient::new(&endpoint, &token, state.http.clone());
        if let Some(t) = title.as_deref() {
            let cat = mf_new_cat.unwrap_or(1);
            let _ = client.update_feed_title(mf_id, t, cat).await;
        }
        if let Some(cat) = mf_new_cat {
            let _ = client.move_feed_category(mf_id, cat).await;
        }
    }
    Ok(())
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
    {
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
        } else {
            return Ok(());
        }
    }
    // 锁外调度即时推送（防抖合批，~1s 内到达服务端）
    schedule_state_push(&state);
    Ok(())
}

#[tauri::command]
pub async fn set_starred(state: State<'_, AppState>, id: i64, starred: bool) -> AppResult<()> {
    {
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
        } else {
            return Ok(());
        }
    }
    schedule_state_push(&state);
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
    let n = {
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
            drop(conn);
            schedule_state_push(&state);
            return Ok(n);
        }
        n
    };
    Ok(n)
}

#[tauri::command]
pub async fn feed_counts(state: State<'_, AppState>) -> AppResult<Vec<db::FeedCounts>> {
    let conn = state.db.lock().await;
    db::feed_counts(&conn)
}

/* ============================================================
   刷新（直连抓取）
   ============================================================ */

/// 刷新单个订阅源（直连）。三段式：锁内取条件头 → 锁外 HTTP+解析 → 锁内落库。
/// 与并发管线共用 refresh_feed_staged，网络 IO 不占数据库写锁。
#[tauri::command]
pub async fn refresh_feed(state: State<'_, AppState>, feed_id: i64) -> AppResult<usize> {
    let db = state.db.clone();
    let client = state.http.clone();
    let dedup = {
        let conn = db.lock().await;
        read_dedup_flag(&conn)
    };
    ingestion::refresh_feed_staged(&db, &client, feed_id, dedup).await
}

/// 刷新全部订阅源（直连，并发上限 = 设置 fetchConcurrency，默认 4）。
/// 复用调度器的三段式管线：HTTP 锁外并行，写库短暂持锁。
#[tauri::command]
pub async fn refresh_all_feeds(state: State<'_, AppState>) -> AppResult<RefreshSummary> {
    let db = state.db.clone();
    let http = state.http.clone();
    let (n, f) = crate::scheduler::refresh_all(&db, &http).await;
    Ok(RefreshSummary { new_articles: n, failed_feeds: f })
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
    // smartDedup 关闭瞬间清去重墓碑：用户显式想让重复文章回来，
    // 之后的抓取按无去重语义正常入库（不清的话墓碑会继续拦截）
    if key == "app_settings" {
        let was_on = read_dedup_flag(&conn);
        let now_on = serde_json::from_str::<serde_json::Value>(&value)
            .ok()
            .and_then(|v| v.get("smartDedup").and_then(|b| b.as_bool()))
            .unwrap_or(false);
        if was_on && !now_on {
            let _ = db::clear_dedup_tombstones(&conn);
        }
    }
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

    // 落库覆盖正文（全文 > RSS 摘要）+ 置提取标志（按钮/设置状态共用）。
    // 智能全文防退化：提取结果剥标签后若比原 RSS 正文还短，说明原内容已是
    // 全文或提取失败——保留原内容、不置提取标志（避免把好正文换成更短的）。
    {
        let conn = state.db.lock().await;
        let original: String = conn
            .query_row(
                "SELECT COALESCE(content_html, '') FROM articles WHERE id = ?1",
                rusqlite::params![article_id],
                |r| r.get(0),
            )
            .unwrap_or_default();
        let orig_text_len = crate::sanitize::html_to_text(&original).trim().len();
        let extracted_text_len = crate::sanitize::html_to_text(&extracted).trim().len();
        // 提取结果显著更短（不足原文 80%）→ 判定退化，保留原文
        if extracted_text_len > 0 && extracted_text_len * 5 < orig_text_len * 4 {
            return Ok(original);
        }
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
   图片代理（防盗链兼容）——参考 Papr 方案
   ============================================================ */

/// 浏览器 UA（部分图床除 Referer 外还检查 UA）。
const IMAGE_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
    AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36";

/// 图片字节上限（防恶意/误配 URL 撑爆内存）。
const MAX_IMAGE_BYTES: u64 = 25 * 1024 * 1024;

/// Referer 候选链：防盗链双向——黑名单式（sinaimg.cn 拒外来 Referer、只认裸请求）
/// 与白名单式（少数派 cdnfile.sspai.com 拒裸请求、要求 sspai.com Referer）无法用
/// 单一值同时满足，故依次尝试：无 Referer → 图片自身 origin → 文章原文 URL。
fn referer_candidates(image_url: &str, page_url: Option<&str>) -> Vec<Option<String>> {
    let mut out = vec![None];
    if let Ok(u) = url::Url::parse(image_url) {
        let origin = u.origin().ascii_serialization();
        if origin != "null" {
            out.push(Some(format!("{origin}/")));
        }
    }
    if let Some(p) = page_url {
        if (p.starts_with("http://") || p.starts_with("https://")) && url::Url::parse(p).is_ok() {
            let candidate = Some(p.to_string());
            if !out.contains(&candidate) {
                out.push(candidate);
            }
        }
    }
    out
}

/// 后端抓图：走 Referer 候选链直到某值被图床接受，返回图片字节。
/// 用于 webview 自身加载失败（防盗链）时的重试——webview 的 Referer 无法按
/// 域名变化，Rust 端可控制。传输错误（DNS/超时）直接中止（换 Referer 无济于事），
/// 仅 HTTP 状态错误才继续下一候选。
#[tauri::command]
pub async fn fetch_image(
    state: State<'_, AppState>,
    url: String,
    page_url: Option<String>,
) -> AppResult<Vec<u8>> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(AppError::new("badImageUrl", "仅支持 http/https 图片"));
    }
    let http = &state.http;
    let mut last_err = AppError::new("imageFetch", "图片抓取失败");
    for referer in referer_candidates(&url, page_url.as_deref()) {
        let mut req = http.get(&url).header("User-Agent", IMAGE_UA);
        if let Some(r) = &referer {
            req = req.header("Referer", r.as_str());
        }
        match req.send().await {
            Err(e) => return Err(e.into()), // 传输错误：换 Referer 无济于事
            Ok(resp) => match resp.error_for_status() {
                Err(e) => last_err = AppError::new("imageFetch", format!("HTTP 错误: {e}")),
                Ok(resp) => {
                    if resp.content_length().is_some_and(|n| n > MAX_IMAGE_BYTES) {
                        return Err(AppError::new("imageTooLarge", "图片过大"));
                    }
                    let bytes = resp.bytes().await?;
                    if bytes.len() as u64 > MAX_IMAGE_BYTES {
                        return Err(AppError::new("imageTooLarge", "图片过大"));
                    }
                    return Ok(bytes.to_vec());
                }
            },
        }
    }
    Err(last_err)
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

/// 测试连接（轻量）：纯 GET /v1/me，不落库、不做任何同步。
/// 用于填表时快速验证连通性。
#[tauri::command]
pub async fn sync_test(
    state: State<'_, AppState>,
    endpoint: String,
    token: String,
) -> AppResult<String> {
    let (msg, _) = crate::sync::test_connection(&endpoint, &token, &state.http).await?;
    Ok(msg)
}

/// 保存凭据：先轻量测试（失败不保存），通过后立即落库返回。
/// 首连的重活（拉订阅、同步状态）由前端随后台阶段执行，不阻塞这里。
/// Token 留空且已连接 → 复用已存 Token（仅改 Endpoint 的场景）。
/// 换账号检测：已连接其他账号（endpoint 或 token 不同）时先清理旧账号
/// 数据（订阅/绑定/队列），避免两份订阅列表混杂。
#[tauri::command]
pub async fn sync_save(
    state: State<'_, AppState>,
    endpoint: String,
    token: String,
) -> AppResult<String> {
    // 留空 Token 且已连接 → 复用旧 Token（改地址不动密钥）
    let (endpoint, token) = {
        let conn = state.db.lock().await;
        let old = crate::sync::read_credentials(&conn);
        match (&old, token.trim().is_empty()) {
            (Some((_old_ep, old_tk)), true) => (endpoint.trim().to_string(), old_tk.clone()),
            (None, true) => {
                return Err(AppError::new("validate", "请填写 API Token"));
            }
            _ => (endpoint.trim().to_string(), token.trim().to_string()),
        }
    };
    // 换账号检测（锁内读旧凭据）；保存前凭据为空 = 首连
    let (account_changed, old_was_empty) = {
        let conn = state.db.lock().await;
        let old = crate::sync::read_credentials(&conn);
        match old {
            Some((old_ep, old_tk)) => {
                let changed =
                    old_ep.trim_end_matches('/') != endpoint.trim_end_matches('/') || old_tk != token;
                (changed, false)
            }
            None => (false, true),
        }
    };
    // 测试新凭据（失败不保存不动现状）；账户名随凭据落库（设置页动态显示）
    let (msg, account) = crate::sync::test_connection(&endpoint, &token, &state.http).await?;
    {
        let mut conn = state.db.lock().await;
        if account_changed {
            let (feeds, _) = db::purge_miniflux_data(&mut conn)?;
            log::info!("sync: 账号切换，清理旧账号数据：{feeds} 个订阅");
        }
        // 首连判定（保存前凭据为空 = 第一次连接）：供前端决定是否弹
        // 「同步本地订阅到 Miniflux」（本地有未绑源时）
        let unbound_local: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM feeds WHERE origin = 'local' AND miniflux_id IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let first_connect = old_was_empty && unbound_local > 0;
        db::set_setting(&conn, "miniflux_endpoint", &endpoint)?;
        db::set_setting(&conn, "miniflux_token", &token)?;
        db::set_setting(&conn, "miniflux_account", &account)?;
        // 新连接：清增量游标，让首同步从全量开始（对账旧状态差异）
        db::set_setting(&conn, "miniflux_last_sync", "0")?;
        if first_connect {
            return Ok(serde_json::json!({
                "message": msg,
                "firstConnect": true,
                "unboundLocalFeeds": unbound_local,
            })
            .to_string());
        }
    }
    Ok(serde_json::json!({ "message": msg, "firstConnect": false, "unboundLocalFeeds": 0 }).to_string())
}

/// 分步同步：which="feeds"（订阅层，秒级）| "states"（状态+条目层，慢）。
/// states 全量对账只在 full=true（手动/首连）时做。
#[tauri::command]
pub async fn sync_phase(
    state: State<'_, AppState>,
    which: String,
    full: Option<bool>,
) -> AppResult<crate::sync::SyncReport> {
    match which.as_str() {
        "feeds" => crate::sync::feeds_phase(&state.db, &state.http).await,
        "states" => crate::sync::states_phase(&state.db, &state.http, full.unwrap_or(false)).await,
        _ => Err(AppError::new("validate", "which 必须是 feeds 或 states")),
    }
}

/// 把本地直连订阅（origin='local' 且未绑定 miniflux_id）推送到服务端：
/// 入队 add_feed（带分类映射 payload）→ 立即跑 feeds 阶段（推送+碰撞绑定）。
/// 幂等：已绑定的源不入队；服务端已存在同 URL（409）回查绑定，不构成错误。
/// 返回 (待推数, 推送摘要)——首连弹窗与手动按钮共用此入口。
#[tauri::command]
pub async fn sync_local_feeds(state: State<'_, AppState>) -> AppResult<String> {
    // 锁内：找未绑定的本地源并入队（分类 id 随 payload，push 时映射远端分类）；
    // 查重：队列里已有同 URL 的 add_feed 项则跳过（弹窗确认 + 手动按钮连点
    // 不会堆积重复队列——失败项保留是重试语义，重复入队才是堆积）
    let queued = {
        let conn = state.db.lock().await;
        if !sync_configured(&conn) {
            return Err(AppError::new("notConnected", "未连接 Miniflux"));
        }
        let mut stmt = conn
            .prepare(
                "SELECT f.id, f.feed_url, f.folder_id FROM feeds f
                 WHERE f.origin = 'local' AND f.miniflux_id IS NULL",
            )?;
        let rows: Vec<(i64, String, Option<i64>)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        let pending_urls: std::collections::HashSet<String> = db::take_sync_queue(&conn)
            .unwrap_or_default()
            .into_iter()
            .filter(|i| i.action == "add_feed")
            .filter_map(|i| i.feed_url)
            .collect();
        let mut n = 0usize;
        for (_id, url, folder) in rows {
            if pending_urls.contains(&url) {
                continue; // 已在队列（上次失败待重试）
            }
            let payload = serde_json::json!({ "folder_id": folder }).to_string();
            db::enqueue_sync(&conn, None, Some(&url), "add_feed", Some(&payload))?;
            n += 1;
        }
        n
    };
    if queued == 0 {
        // 没有新入队，但可能仍有待推队列项（上次失败的）——检查后再决定
        let has_pending = {
            let conn = state.db.lock().await;
            db::take_sync_queue(&conn)
                .map(|q| q.iter().any(|i| i.action == "add_feed"))
                .unwrap_or(false)
        };
        if !has_pending {
            return Ok("没有需要同步的本地订阅（全部已绑定或已推送）".into());
        }
    }
    // feeds 阶段：push（新入队的 + 队列残留的）+ pull（碰撞绑定 + 远端新订阅）
    let report = crate::sync::feeds_phase(&state.db, &state.http).await?;
    if report.errors.is_empty() {
        Ok(format!("已同步 {queued} 个本地订阅到 Miniflux（推送 {}）", report.pushed_feeds))
    } else {
        Ok(format!(
            "已同步 {queued} 个本地订阅，其中 {} 个失败（下次同步自动重试）：{}",
            report.errors.len(),
            report.errors.join("；")
        ))
    }
}

/// 断开连接：清凭据 + 清理服务端来源数据（订阅/条目/绑定/队列）。
/// 用户直连订阅（origin='local'）保留——断开只清服务端数据的产品语义。
#[tauri::command]
pub async fn sync_disconnect(state: State<'_, AppState>) -> AppResult<String> {
    let (feeds, articles) = {
        let mut conn = state.db.lock().await;
        let r = db::purge_miniflux_data(&mut conn)?;
        db::set_setting(&conn, "miniflux_endpoint", "")?;
        db::set_setting(&conn, "miniflux_token", "")?;
        db::set_setting(&conn, "miniflux_account", "")?;
        db::set_setting(&conn, "miniflux_last_sync", "0")?;
        r
    };
    Ok(format!("已断开并清理：移除 {feeds} 个服务端订阅（{articles} 处绑定），本地直连订阅保留"))
}

/// 缓存清理：删除指定天数前的文章（收藏/待同步项保留）或仅清 AI 缓存。
/// scope='articles' | 'ai'。返回 (删文章数, 清 AI 字段数)。
#[tauri::command]
pub async fn cache_cleanup(
    state: State<'_, AppState>,
    days: i64,
    scope: String,
) -> AppResult<String> {
    if !(1..=3650).contains(&days) {
        return Err(AppError::new("validate", "天数需在 1–3650 之间"));
    }
    if scope != "articles" && scope != "ai" {
        return Err(AppError::new("validate", "scope 必须是 articles 或 ai"));
    }
    let (deleted, ai_cleared) = {
        let mut conn = state.db.lock().await;
        db::cleanup_cache(&mut conn, days, &scope)?
    };
    Ok(if scope == "articles" {
        format!("已清理 {days} 天前的文章 {deleted} 篇（收藏文章已保留）")
    } else {
        format!("已清理 {days} 天前文章的 AI 摘要与翻译缓存 {ai_cleared} 篇")
    })
}

/// 手动全量同步（侧栏刷新按钮 / 同步中心）。staged：锁只在 DB 读写时持有。
#[tauri::command]
pub async fn sync_now(state: State<'_, AppState>) -> AppResult<crate::sync::SyncReport> {
    // feeds 阶段 → states 阶段（full 对账），两阶段各自内部管理锁
    let mut report = crate::sync::feeds_phase(&state.db, &state.http).await?;
    let states = crate::sync::states_phase(&state.db, &state.http, true).await?;
    // 合并报告（错误聚合，方便前端展示）
    report.pushed_states = states.pushed_states;
    report.pulled_entries = states.pulled_entries;
    report.fallback_entries = states.fallback_entries;
    report.errors.extend(states.errors);
    Ok(report)
}

/// 同步配置状态（设置页显示用）。account = 连接时记录的服务端用户名。
#[derive(serde::Serialize)]
pub struct SyncStatusInfo {
    pub connected: bool,
    pub endpoint: Option<String>,
    pub account: Option<String>,
    pub last_sync: i64,
}

#[tauri::command]
pub async fn sync_status(state: State<'_, AppState>) -> AppResult<SyncStatusInfo> {
    let conn = state.db.lock().await;
    let endpoint = db::get_setting(&conn, "miniflux_endpoint").ok().flatten()
        .filter(|e| !e.trim().is_empty());
    let account = db::get_setting(&conn, "miniflux_account").ok().flatten()
        .filter(|a| !a.trim().is_empty());
    Ok(SyncStatusInfo {
        connected: endpoint.is_some(),
        endpoint,
        account,
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
        // 前端关闭面板（Channel 被 drop）时 send 返回 Err → 返回 false 让
        // stream_chat 提前终止，不浪费 token、不落残缺产物（A-5）。
        on_channel.send(AiEvent::Delta(delta.to_string())).is_ok()
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
        // 前端关闭面板（Channel 被 drop）时 send 返回 Err → 返回 false 让
        // stream_chat 提前终止，不浪费 token、不落残缺产物（A-5）。
        on_channel.send(AiEvent::Delta(delta.to_string())).is_ok()
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
        // 翻译产物是 HTML 且直接 dangerouslySetInnerHTML 渲染——入库前
        // 过消毒器（模型输出不可信：可能带 <script> 或被提示注入）。
        // 流式 delta 已发给前端（流中预览，未消毒）；落库的是消毒版。
        // 前端在流结束后回读 get_article 拿消毒版覆盖流式预览（见 store.ts
        // toggleReaderTranslation 的 onDone 回读逻辑）。
        let safe = crate::sanitize::sanitize(&outcome.text, None);
        let conn = state.db.lock().await;
        let _ = db::set_article_ai_fields(&conn, article_id, None, Some(&safe));
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

#[cfg(test)]
mod image_proxy_tests {
    use super::referer_candidates;

    /// Referer 候选链：无 Referer → 图床 origin → 文章 URL（白名单式防盗链靠最后一项）。
    #[test]
    fn referer_candidates_tries_none_origin_then_page() {
        let got = referer_candidates(
            "https://cdnfile.sspai.com/a.jpg",
            Some("https://sspai.com/post/123"),
        );
        assert_eq!(
            got,
            vec![
                None,
                Some("https://cdnfile.sspai.com/".to_string()),
                Some("https://sspai.com/post/123".to_string()),
            ],
            "候选链顺序必须是 无→origin→文章URL"
        );
    }

    #[test]
    fn referer_candidates_without_page_url() {
        let got = referer_candidates("https://wx1.sinaimg.cn/large/a.jpg", None);
        assert_eq!(got, vec![None, Some("https://wx1.sinaimg.cn/".to_string())]);
    }

    #[test]
    fn referer_candidates_skips_non_http_page_url() {
        let got = referer_candidates("https://ex.com/a.png", Some("mailto:editor@ex.com"));
        assert_eq!(got, vec![None, Some("https://ex.com/".to_string())]);
    }

    #[test]
    fn referer_candidates_dedupes_page_equal_to_origin() {
        let got = referer_candidates("https://ex.com/a.png", Some("https://ex.com/"));
        assert_eq!(got, vec![None, Some("https://ex.com/".to_string())]);
    }
}
