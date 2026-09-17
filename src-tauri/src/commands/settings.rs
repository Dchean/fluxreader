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
    {
        let conn = state.db.lock().await;
        let original = db::get_article_content_html(&conn, article_id)?;
        let orig_text_len = crate::sanitize::html_to_text(&original).trim().len();
        let extracted_text_len = crate::sanitize::html_to_text(&extracted).trim().len();
        // 提取结果显著更短（不足原文 80%）→ 判定退化，保留原文
        if extracted_text_len > 0 && extracted_text_len * 5 < orig_text_len * 4 {
            return Ok(original);
        }
        db::update_article_fulltext(&conn, article_id, &extracted, true)?;
        if !image.is_empty() {
            db::update_article_image_if_empty(&conn, article_id, &image)?;
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
