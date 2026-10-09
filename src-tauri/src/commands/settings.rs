//! commands 的 settings 领域子模块（TASK-044 从 commands.rs 按既有章节拆分，纯搬运）。

use super::read_dedup_flag;
use crate::db;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use tauri::State;

/* ============================================================
Settings
============================================================ */

#[tauri::command]
pub async fn get_setting(state: State<'_, AppState>, key: String) -> AppResult<Option<String>> {
    let conn = state.db.lock().await;
    db::get_setting(&conn, &key)
}

/// OPT-006：活动账号与同步进度键——通用 `set_setting` 不得改写。
/// 这些都只能经专用命令修改（sync_save / sync_disconnect 在**同一事务**里
/// 推进代际并做并发校验；config_sync 导入只写待确认建议）：
/// - 活动凭据四键：协议 / 地址 / 用户名 / 密码；
/// - 代际与两类游标、Fever 历史状态、端点解析缓存：绕过写入会让在途会话的
///   代际复核失真或让新账号继承旧进度；
/// - 待确认连接建议：由导入与专用保存/放弃命令管理。
// Note: 通用 IPC 不得绕过账号生命周期（写入侧收口） — 见 .agents/notes/implemented/architecture/2026-10-08-账号会话与配置应用边界.md
const PROTECTED_SETTING_KEYS: &[&str] = &[
    "sync_protocol",
    "greader_endpoint",
    "greader_username",
    "greader_password",
    "sync_generation",
    "sync_last_sync",
    "sync_last_entry_id",
    "fever_history_state",
    "endpoint_resolved",
    "pending_connection_config",
    "pending_connection_serial",
];

/// 通用设置写入的真实实现（命令与单测共用；单测无法构造 State）。
pub(crate) fn apply_setting(conn: &rusqlite::Connection, key: &str, value: &str) -> AppResult<()> {
    // OPT-006：活动连接/代际/进度键必须走专用保存流程（含代际推进与并发校验），
    // 通用写入是一条会绕过账号生命周期守卫的旁路——在此拒绝。
    if PROTECTED_SETTING_KEYS.contains(&key) {
        return Err(AppError::new(
            "protectedSetting",
            format!("设置键「{key}」受账号生命周期保护，请使用「同步」设置页的保存/断开流程"),
        ));
    }
    // smartDedup 关闭瞬间清去重墓碑：用户显式想让重复文章回来，
    // 之后的抓取按无去重语义正常入库（不清的话墓碑会继续拦截）
    if key == "app_settings" {
        let was_on = read_dedup_flag(conn);
        let now_on = serde_json::from_str::<serde_json::Value>(value)
            .ok()
            .and_then(|v| v.get("smartDedup").and_then(|b| b.as_bool()))
            .unwrap_or(false);
        if was_on && !now_on {
            let _ = db::clear_dedup_tombstones(conn);
        }
    }
    db::set_setting(conn, key, value)
}

#[tauri::command]
pub async fn set_setting(state: State<'_, AppState>, key: String, value: String) -> AppResult<()> {
    let conn = state.db.lock().await;
    apply_setting(&conn, &key, &value)
}

/* ============================================================
更新检查（OPT-014 / F16）
============================================================ */

/// GitHub API 根（生产固定）。webview 只经命令拿结果，不持有出网能力——
/// 生产 CSP 的 connect-src 保持不放宽，也不提供「前端传任意 URL」的代理。
/// Note: 固定目的地与链接受信化见
/// .agents/notes/implemented/architecture/2026-10-08-凭据失败关闭与受控更新检查.md
const GITHUB_API_BASE: &str = "https://api.github.com";

/// 官方仓库最新发布的固定路径。测试只在模块单测内换 base 主机，路径恒为它。
const RELEASES_LATEST_PATH: &str = "/repos/Dchean/fluxreader/releases/latest";

/// 受信发布页根：html_url 缺失时回落到它；非受信值一律拒绝（见下）。
const TRUSTED_RELEASES_URL: &str = "https://github.com/Dchean/fluxreader/releases";

/// 更新检查结果（IPC 返回）：version 已去可选 v/V 前缀，url 已受信化。
#[derive(Debug, Clone, serde::Serialize)]
pub struct UpdateCheckOutcome {
    pub version: String,
    pub url: String,
}

/// 校验一个**已提供**的 html_url 是否属于受信本仓库 releases 域：
/// 仅接受 https + github.com 主机 + 本仓库 releases 路径；其余（畸形/非 https/
/// 其他域/其他仓库/伪装前缀/协议注入）返回 None。
/// GitHub owner/repo 大小写不敏感，路径按小写比较；段边界必须整段或接 `/`，
/// 防 `fluxreader-evil` 之类的前缀伪装。
fn trusted_release_url(raw: &str) -> Option<String> {
    let u = url::Url::parse(raw).ok()?;
    if u.scheme() != "https" {
        return None;
    }
    if u.host_str().map(|h| h.eq_ignore_ascii_case("github.com")) != Some(true) {
        return None;
    }
    let path = u.path().to_ascii_lowercase();
    if path != "/dchean/fluxreader/releases" && !path.starts_with("/dchean/fluxreader/releases/") {
        return None;
    }
    Some(raw.to_string())
}

/// 更新检查专用 HTTP client（OPT-014 R1）：
/// - **禁止跟随重定向**——目的地必须固定在 GitHub 首跳，绝不能把固定 URL 的
///   请求被 30x 带去任意主机/降级 http（3xx 一律按失败处理）；
/// - 显式超时（含连接超时），慢响应有界失败；
/// - 应用身份 UA。**不复用抓取 client**（后者按设计允许跨域跳转）。
fn update_check_client(timeout: std::time::Duration) -> AppResult<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .connect_timeout(std::time::Duration::from_secs(10))
        .user_agent(format!("FluxReader/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| AppError::internal(format!("更新检查 HTTP client 构建失败：{e}")))
}

/// 更新检查内部实现（与命令分离；只在模块单测里换 base 主机与超时，
/// 无任何 pub 测试入口）。
///
/// 响应契约（R1 P4）：
/// - `html_url` **缺失** → 回落官方 releases 页（可容忍）；
/// - `html_url` **已提供但非 string / 非法 / 非受信** → Err（不得悄悄换成官方页
///   还返回 Ok——同版本时前端会误报「已是最新」）。
async fn check_for_updates_at_base(
    api_base: &str,
    timeout: std::time::Duration,
) -> AppResult<UpdateCheckOutcome> {
    let client = update_check_client(timeout)?;
    let url = format!("{api_base}{RELEASES_LATEST_PATH}");
    let resp = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        let hint = match status.as_u16() {
            300..=399 => "重定向被拒绝（更新检查目的地必须固定）",
            404 => "未找到发布，可能尚未发布正式版本",
            429 => "请求被限流，请稍后重试",
            s if s >= 500 => "GitHub 服务暂时不可用",
            _ => "请求被拒绝",
        };
        return Err(AppError::network(format!(
            "GitHub 返回 HTTP {}：{hint}",
            status.as_u16()
        )));
    }
    let v: serde_json::Value = resp.json().await?;
    let tag = v
        .get("tag_name")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .trim();
    // 发布标签常见 `v0.18.0` / `V0.18.0` 形态：去掉可选单个 v/V 前缀后交给
    // 前端 compareVersions 比较；空标签（含裸 "v"）视为不可用响应。
    let version = tag
        .strip_prefix('v')
        .or_else(|| tag.strip_prefix('V'))
        .unwrap_or(tag);
    if version.is_empty() {
        return Err(AppError::network(
            "更新响应缺少可用版本号（tag_name 缺失或为空）",
        ));
    }
    let url = match v.get("html_url") {
        None => TRUSTED_RELEASES_URL.to_string(),
        Some(serde_json::Value::String(raw)) => trusted_release_url(raw).ok_or_else(|| {
            // 不回显外部提供的地址原文（避免把任意串带进 UI 错误文案）
            AppError::network("更新响应的 html_url 不属于受信本仓库 releases 域")
        })?,
        Some(_) => {
            return Err(AppError::network(
                "更新响应的 html_url 类型非法（应为字符串）",
            ));
        }
    };
    Ok(UpdateCheckOutcome {
        version: version.to_string(),
        url,
    })
}

/// IPC 命令：检查更新。无 URL 入参——目的地恒为官方仓库 releases/latest；
/// 不存在任何可注入 client/URL 的测试后门（注入面只在模块单测内）。
#[tauri::command]
pub async fn check_for_updates() -> AppResult<UpdateCheckOutcome> {
    check_for_updates_at_base(GITHUB_API_BASE, std::time::Duration::from_secs(10)).await
}

/* ============================================================
设置写入保护（OPT-006 / F05）

通用 set_setting 是一条绕过账号生命周期的旁路：任何前端/其它模块都能直接
改写活动凭据、代际与游标。账号身份变化必须走 sync_save / sync_disconnect
的专用事务（含清理、代际推进与并发复核），配置导入只写待确认建议。
本段单测锁定「受保护键一律拒绝、普通键照常写入」。
============================================================ */
#[cfg(test)]
mod protected_setting_tests {
    use super::*;

    fn conn() -> rusqlite::Connection {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::MIGRATIONS.to_latest(&mut conn).unwrap();
        conn
    }

    /// 判别力：保护清单被移除任一项时对应用例必红（写入成功 = 旁路存在）。
    #[test]
    fn protected_keys_are_rejected_with_dedicated_flow_hint() {
        let conn = conn();
        for key in [
            "sync_protocol",
            "greader_endpoint",
            "greader_username",
            "greader_password",
            "sync_generation",
            "sync_last_sync",
            "sync_last_entry_id",
            "fever_history_state",
            "endpoint_resolved",
            "pending_connection_config",
        ] {
            let err = apply_setting(&conn, key, "tampered").expect_err("受保护键必须拒绝");
            assert_eq!(err.code, "protectedSetting", "{key} 应返回明确错误码");
            assert!(
                err.message.contains("保存") || err.message.contains("断开"),
                "{key} 错误应指向专用流程：{err}"
            );
            let stored = db::get_setting(&conn, key).unwrap();
            assert!(stored.is_none(), "{key} 被拒绝后不得留下任何写入");
        }
    }

    /// 对照：普通键（app_settings / 播放进度等）不受影响。
    #[test]
    fn ordinary_keys_still_write() {
        let conn = conn();
        apply_setting(&conn, "app_settings", r#"{"themeMode":"dark"}"#).unwrap();
        let saved = db::get_setting(&conn, "app_settings").unwrap().unwrap();
        assert!(saved.contains("dark"));
    }
}

/* ============================================================
完整 SQL 全文提取（Readability）
============================================================ */

/// 全文提取的结果（TASK-076 / P2-10 后半，DEC-req104-p2-10b-fulltext-degraded-20260920）。
///
/// 此前只返回一个 String，「提取成功」与「因防退化保留了原文」在返回值上**无法区分**，
/// 前端只能靠「返回内容 == 当前正文」的字符串比对来猜（reader.ts 就是这么做的）——
/// 一旦正文恰好相同就误判，用户也无从知道到底发生了什么。现在改为结构化结果：
///   `html`     落库/展示用的正文（成功时是提取结果，降级时是保留的原文）
///   `degraded` 是否降级（true = 本次没有采用提取结果）
///   `reason`   降级原因；成功时为 None
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractOutcome {
    pub html: String,
    pub degraded: bool,
    pub reason: Option<String>,
}

impl ExtractOutcome {
    fn extracted(html: String) -> Self {
        Self {
            html,
            degraded: false,
            reason: None,
        }
    }

    fn degraded(html: String, reason: impl Into<String>) -> Self {
        Self {
            html,
            degraded: true,
            reason: Some(reason.into()),
        }
    }
}

/// 全文提取：拉文章网页 → Readability 抽正文 → 覆盖该条目 content_html
/// （「默认打开方式=自动全文」：RSS 摘要型源打开时自动触发）。
#[tauri::command]
pub async fn extract_fulltext(
    state: State<'_, AppState>,
    article_id: i64,
) -> AppResult<ExtractOutcome> {
    let url: Option<String> = {
        let conn = state.db.lock().await;
        db::get_article_url(&conn, article_id)?
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
        return Err(AppError::network(format!(
            "网页拉取失败：HTTP {}",
            resp.status()
        )));
    }
    let html = resp.text().await?;

    // Readability 不是 Send → spawn_blocking 里跑
    let base = url.clone();
    let html2 = html.clone();
    let extracted =
        tokio::task::spawn_blocking(move || crate::extraction::extract_article(&html, &base))
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
    //
    // TASK-076：两条「没有采用提取结果」的路径都返回 degraded=true 并带上原因，
    // 不再让调用方自己去猜（此前提取结果为空时同样落到下面的补丁分支，
    // 与「防退化」混在一起、无法区分）。
    {
        let conn = state.db.lock().await;
        let original = db::get_article_content_html(&conn, article_id)?;
        // 判定抽到 extraction::degradation_reason（纯函数、可单测），此处只负责
        // 按判定结果落库或如实返回降级信息。
        if let Some(reason) = crate::extraction::degradation_reason(&original, &extracted) {
            return Ok(ExtractOutcome::degraded(original, reason));
        }
        db::update_article_fulltext(&conn, article_id, &extracted, true)?;
        if !image.is_empty() {
            db::update_article_image_if_empty(&conn, article_id, &image)?;
        }
    }
    Ok(ExtractOutcome::extracted(extracted))
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

/* OPT-014 / F16：更新检查链接受信化与受控 HTTP。
R1 收窄：本地 HTTP mock 测试收进模块单测（原 tests/update_check_e2e.rs 删除）——
`check_for_updates_at_base` 不再有任何 pub 测试入口，webview/外部 Rust 均不可达；
测试仍全部真实执行（本地回环 HTTP）。 */
#[cfg(test)]
mod update_check_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// 受信地址原样返回：本仓库 releases/tag 页与 releases 根页；owner/repo 大小写不敏感。
    #[test]
    fn accepts_only_trusted_repo_release_urls() {
        for ok in [
            "https://github.com/Dchean/fluxreader/releases/tag/v0.18.0",
            "https://github.com/Dchean/fluxreader/releases",
            "https://github.com/dchean/fluxreader/releases/tag/v0.18.0",
        ] {
            assert_eq!(
                trusted_release_url(ok).as_deref(),
                Some(ok),
                "受信地址应原样返回"
            );
        }
    }

    /// 非受信地址一律判非法：畸形/非 https/其他域/其他仓库/前缀伪装/协议注入。
    #[test]
    fn rejects_untrusted_release_urls() {
        for bad in [
            "",
            "not a url",
            "http://github.com/Dchean/fluxreader/releases/tag/v1",
            "https://evil.example.com/updates",
            "https://github.com.evil.example/Dchean/fluxreader/releases",
            "https://github.com/Other/repo/releases/tag/v1",
            "https://github.com/Dchean/fluxreader-evil/releases/tag/v1",
            "https://github.com/Dchean/fluxreader/releases-evil",
            "javascript:alert(1)",
        ] {
            assert!(
                trusted_release_url(bad).is_none(),
                "非受信地址必须拒绝：{bad}"
            );
        }
    }

    struct Mock {
        base: String,
        last_request: Arc<Mutex<String>>,
    }

    /// 本地 mock：对任意请求固定返回 (status, body)，并记录最后一个请求原文。
    fn spawn_mock(status: u16, body: &str) -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let last_request: Arc<Mutex<String>> = Default::default();
        let sink = last_request.clone();
        let body = body.to_string();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let sink = sink.clone();
                let body = body.clone();
                std::thread::spawn(move || {
                    let mut stream = stream;
                    let mut buf = [0u8; 8192];
                    let Ok(n) = stream.read(&mut buf) else { return };
                    *sink.lock().unwrap() = String::from_utf8_lossy(&buf[..n]).to_string();
                    let reason = match status {
                        200 => "OK",
                        404 => "Not Found",
                        429 => "Too Many Requests",
                        _ => "Internal Server Error",
                    };
                    let resp = format!(
                        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.flush();
                });
            }
        });
        Mock {
            base: format!("http://127.0.0.1:{port}"),
            last_request,
        }
    }

    /// 计数 listener：对任何请求 +1（若被误访问，测试可明确诊断），并回 200。
    fn spawn_counting_mock() -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                counter.fetch_add(1, Ordering::SeqCst);
                let mut stream = stream;
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        (format!("http://127.0.0.1:{port}"), hits)
    }

    /// 302 入口 listener：把 Location 指向 `target`（若被跟随，target 会计数）。
    fn spawn_redirect_to(target_port: u16) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let resp = format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{target_port}/evil\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    fn port_of(base: &str) -> u16 {
        base.rsplit(':').next().unwrap().parse().unwrap()
    }

    /// 正例：协议路径/请求头/解析全链路。客户端不带默认 UA——UA 必须来自专用 client。
    #[tokio::test]
    async fn release_response_parsed_with_app_identity_headers_on_fixed_path() {
        let mock = spawn_mock(
            200,
            r#"{"tag_name":"v0.18.0","html_url":"https://github.com/Dchean/fluxreader/releases/tag/v0.18.0"}"#,
        );
        let out = check_for_updates_at_base(&mock.base, Duration::from_secs(5))
            .await
            .expect("有效响应应解析成功");
        assert_eq!(out.version, "0.18.0", "tag_name 应去掉 v 前缀");
        assert_eq!(
            out.url, "https://github.com/Dchean/fluxreader/releases/tag/v0.18.0",
            "受信 html_url 应原样返回"
        );

        let req = mock.last_request.lock().unwrap().clone();
        assert!(
            req.starts_with("GET /repos/Dchean/fluxreader/releases/latest "),
            "目的地路径必须固定：{req}"
        );
        let lower = req.to_ascii_lowercase();
        assert!(
            lower.contains("user-agent: fluxreader/"),
            "必须带应用身份 UA：{req}"
        );
        assert!(
            lower.contains("accept: application/vnd.github"),
            "需要 GitHub JSON Accept 头：{req}"
        );
    }

    /// 缺失 html_url 可容忍：回落官方 releases 页；无 v 前缀 tag 原样保留。
    #[tokio::test]
    async fn missing_html_url_falls_back_to_official_page() {
        let mock = spawn_mock(200, r#"{"tag_name":"0.19.0"}"#);
        let out = check_for_updates_at_base(&mock.base, Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(out.version, "0.19.0");
        assert_eq!(out.url, TRUSTED_RELEASES_URL);
    }

    /// R1 P4：**已提供**但非受信/非法/非 string 的 html_url 必须 Err——
    /// 修前回落官方页并返回 Ok，同版本时前端会误报「已是最新」。
    #[tokio::test]
    async fn provided_untrusted_html_url_is_error_not_silent_fallback() {
        for body in [
            r#"{"tag_name":"v9.9.9","html_url":"https://evil.example.com/updates"}"#,
            r#"{"tag_name":"v9.9.9","html_url":"http://github.com/Dchean/fluxreader/releases/tag/v9"}"#,
            r#"{"tag_name":"v9.9.9","html_url":"https://github.com/Other/repo/releases/tag/v9"}"#,
            r#"{"tag_name":"v9.9.9","html_url":"https://github.com/Dchean/fluxreader-evil/releases"}"#,
            r#"{"tag_name":"v9.9.9","html_url":""}"#,
            r#"{"tag_name":"v9.9.9","html_url":"javascript:alert(1)"}"#,
            r#"{"tag_name":"v9.9.9","html_url":42}"#,
            r#"{"tag_name":"v9.9.9","html_url":{"nested":true}}"#,
        ] {
            let mock = spawn_mock(200, body);
            let err = check_for_updates_at_base(&mock.base, Duration::from_secs(5))
                .await
                .expect_err("已提供但非法的 html_url 必须 Err");
            assert!(
                err.message.contains("html_url"),
                "错误应指向 html_url：{err}"
            );
        }
    }

    /// 404/429/5xx：必须 Err（错误码可见），不得静默当作「已是最新」。
    #[tokio::test]
    async fn http_error_statuses_fail_instead_of_reporting_latest() {
        for status in [404u16, 429, 500, 503] {
            let mock = spawn_mock(status, "{}");
            let err = check_for_updates_at_base(&mock.base, Duration::from_secs(5))
                .await
                .expect_err("HTTP 错误不得作为检查结果");
            assert!(
                err.message.contains(&status.to_string()),
                "错误信息应含状态码 {status}：{err}"
            );
        }
    }

    /// 无 tag / 空 tag / 裸 v / 非对象 body：必须 Err（不能拿空版本去比较）。
    #[tokio::test]
    async fn missing_or_empty_tag_is_error() {
        for body in [
            "{}",
            r#"{"tag_name":""}"#,
            r#"{"tag_name":"  "}"#,
            r#"{"tag_name":"v"}"#,
            "[]",
            "\"just-a-string\"",
        ] {
            let mock = spawn_mock(200, body);
            let err = check_for_updates_at_base(&mock.base, Duration::from_secs(5))
                .await
                .expect_err("缺少可用版本号必须 Err");
            assert!(
                err.message.contains("tag_name"),
                "错误应说明缺少 tag_name：{err}"
            );
        }
    }

    /// R1 P3：302 必须失败，且重定向目标**零请求**——更新检查专用 client 禁止跟随
    /// 重定向（修前共享抓取 client 会跟到任意主机）。
    #[tokio::test]
    async fn redirect_is_rejected_and_target_receives_zero_requests() {
        let (target_base, hits) = spawn_counting_mock();
        let entry = spawn_redirect_to(port_of(&target_base));
        let err = check_for_updates_at_base(&entry, Duration::from_secs(5))
            .await
            .expect_err("302 必须判失败");
        assert!(err.message.contains("302"), "错误应含状态码 302：{err}");
        assert_eq!(
            hits.load(Ordering::SeqCst),
            0,
            "禁止跟随重定向：目标主机不得收到任何请求"
        );
    }

    /// R1 P3：慢响应必须被 timeout 有界掐断（专用 client 显式超时）。
    #[tokio::test]
    async fn slow_response_times_out_bounded() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        // 接受连接但永不响应：请求应挂在超时而非无限等待
        std::thread::spawn(move || {
            let mut held = Vec::new();
            for stream in listener.incoming().flatten() {
                held.push(stream);
            }
        });
        let start = Instant::now();
        let err = check_for_updates_at_base(&base, Duration::from_millis(250))
            .await
            .expect_err("慢响应必须超时失败");
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "超时必须是有界的（实际 {:?}）",
            start.elapsed()
        );
        assert_eq!(err.code, "network", "超时应是网络类错误：{err}");
    }
}
