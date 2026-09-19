//! Endpoint 解析：把「用户填的域名」自动适配到实际的 API 根地址（TASK-059）。
//!
//! ## 为什么需要
//!
//! 用户只需要填**域名**（如 `https://demo.freshrss.org`），不需要知道后端把 API
//! 放在哪个子路径。但两种自建后端的布局不同：
//!
//! | 后端 | GReader API 所在 | Fever API 所在 |
//! | --- | --- | --- |
//! | **Miniflux** | 站点**根**（`{域名}/accounts/ClientLogin`） | `{域名}/fever/` |
//! | **FreshRSS** | **子路径** `{域名}/api/greader.php` | `{域名}/api/fever.php` |
//!
//! 此前代码把用户输入**原样**当作 API 根，于是填域名时：
//! `POST {域名}/accounts/ClientLogin` → **404**（实测），FreshRSS 用户必然连不上。
//! 改动前 owner 曾被告知该行为「不改探测逻辑、只改文案」，导致界面反过来教用户
//! 去填完整后缀——方向是错的（DEC-endpoint-autodetect-20260918 已纠正）。
//!
//! ## 判定依据（实测，2026-09-18）
//!
//! ```text
//! POST https://demo.freshrss.org/accounts/ClientLogin                  → 404（无此端点）
//! POST https://demo.freshrss.org/api/greader.php/accounts/ClientLogin   → 401（端点存在，凭据被拒）
//! GET  https://demo.freshrss.org/api/fever.php?api                      → 200 {"api_version":4,"auth":0}
//! GET  https://demo.freshrss.org/fever/?api                             → 404
//! ```
//!
//! 即 **404 = 路径不存在（可继续试下一个候选）**，
//! **其它状态码 = 路径存在（必须立刻停下）**。
//!
//! ## 必须守住的边界
//!
//! **凭据错误绝不能被当成「路径不对」而继续尝试**——否则用户把密码填错时，
//! 会收到误导性的「找不到 API 端点」，比改动前更糟。因此：
//!
//! - `404` → 候选不匹配，试下一个；
//! - **任何其它响应**（含 401/403/400/5xx）→ 该候选**就是**正确路径，立即返回它，
//!   由调用方按真实状态码报错（凭据问题报凭据问题）。
//!
//! ## 探测有界
//!
//! 每个协议的候选表是**固定且有限**的常量，不含任何无限重试。

use rusqlite::Connection;

/// GReader 的候选 API 根（按尝试顺序）。
///
/// **顺序有意如此**：先试用户原样输入——这样「已填完整路径」的老用法**首个候选即命中**，
/// 既不改变既有行为，也不产生多余请求（向后兼容）。
pub fn greader_candidates(endpoint: &str) -> Vec<String> {
    let base = endpoint.trim().trim_end_matches('/').to_string();
    let mut out = vec![base.clone()];
    // 仅当用户没有自己带上子路径时，才补充 FreshRSS 形态。
    // （用户已填 `/api/greader.php` 时再拼一次会得到无意义的 `.../api/greader.php/api/greader.php`）
    if !base.ends_with("/api/greader.php") {
        out.push(format!("{base}/api/greader.php"));
    }
    out
}

/// Fever 的候选 API 根（按尝试顺序）。
///
/// Fever 客户端的请求路径是 `{base}/fever/?api`（协议规定 `action` 拼 query），
/// 故这里的「根」指的是 **`/fever/` 之前**那一段：
/// - Miniflux：`{域名}/fever/?api` → 根 = `{域名}`；
/// - FreshRSS：`{域名}/api/fever.php?api` → 它不是 `{base}/fever/` 形态，
///   需单独作为**完整端点**处理（见 `fever_endpoint_candidates`）。
pub fn fever_candidates(endpoint: &str) -> Vec<String> {
    let base = endpoint.trim().trim_end_matches('/').to_string();
    let mut out = vec![base.clone()];
    if !base.ends_with("/api/fever.php") {
        out.push(format!("{base}/api/fever.php"));
    }
    out
}

/// 由状态码判定该候选是否「路径存在」。
///
/// 唯一的「路径不存在」信号是 **404**；其余一律视为该路径存在
/// （含 401/403 凭据被拒、400 参数问题、5xx 服务端错误）——
/// 这样凭据错误才会被如实报成凭据错误，而不会退化成「找不到 API」。
pub fn path_exists(status: u16) -> bool {
    status != 404
}

/// 把 endpoint 归一成候选表用的形态（去空白、去尾斜杠）。
fn normalize(endpoint: &str) -> String {
    endpoint.trim().trim_end_matches('/').to_string()
}

/* ============================================================
解析结果缓存
============================================================ */

/// 缓存键（settings 表；非敏感值，不加密）。
const CACHE_KEY: &str = "endpoint_resolved";

/// 缓存的解析结果。
///
/// **存下「来源输入」是有意的**：命中条件是「协议 + 用户输入都与缓存一致」，
/// 于是用户改了地址（或切了协议）时缓存**自动失效**，不需要额外的失效钩子——
/// 也就不会出现「改了地址却仍按旧地址同步」这种最难排查的状态。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct CacheEntry {
    protocol: String,
    input: String,
    base: String,
}

/// 读缓存：命中返回**已解析**的 API 根。协议或输入不一致 → 视为未命中。
pub fn cached_base(conn: &Connection, protocol: &str, input: &str) -> Option<String> {
    let raw = crate::db::get_setting(conn, CACHE_KEY).ok().flatten()?;
    let entry: CacheEntry = serde_json::from_str(&raw).ok()?;
    if entry.protocol == protocol && entry.input == normalize(input) {
        Some(entry.base)
    } else {
        None
    }
}

/// 记下解析结果。解析成功一次即可，后续同步不再重复探测。
pub fn remember_base(
    conn: &Connection,
    protocol: &str,
    input: &str,
    base: &str,
) -> crate::error::AppResult<()> {
    let entry = CacheEntry {
        protocol: protocol.to_string(),
        input: normalize(input),
        base: normalize(base),
    };
    crate::db::set_setting(conn, CACHE_KEY, &serde_json::to_string(&entry)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greader_bare_domain_yields_root_then_freshrss_subpath() {
        assert_eq!(
            greader_candidates("https://demo.freshrss.org"),
            vec![
                "https://demo.freshrss.org".to_string(),
                "https://demo.freshrss.org/api/greader.php".to_string(),
            ]
        );
    }

    #[test]
    fn greader_full_path_keeps_single_candidate() {
        // 向后兼容：已填完整路径时不追加无意义候选
        assert_eq!(
            greader_candidates("https://demo.freshrss.org/api/greader.php"),
            vec!["https://demo.freshrss.org/api/greader.php".to_string()]
        );
    }

    #[test]
    fn trailing_slash_and_whitespace_normalized() {
        assert_eq!(
            greader_candidates("  https://x.example/  "),
            vec![
                "https://x.example".to_string(),
                "https://x.example/api/greader.php".to_string()
            ]
        );
    }

    #[test]
    fn fever_bare_domain_yields_root_then_freshrss_endpoint() {
        assert_eq!(
            fever_candidates("https://demo.freshrss.org"),
            vec![
                "https://demo.freshrss.org".to_string(),
                "https://demo.freshrss.org/api/fever.php".to_string(),
            ]
        );
    }

    #[test]
    fn fever_full_path_keeps_single_candidate() {
        assert_eq!(
            fever_candidates("https://demo.freshrss.org/api/fever.php"),
            vec!["https://demo.freshrss.org/api/fever.php".to_string()]
        );
    }

    #[test]
    fn only_404_means_path_missing() {
        // 关键边界：凭据类状态码绝不能被当成「路径不对」
        assert!(!path_exists(404));
        for s in [200, 400, 401, 403, 405, 410, 500, 502, 503] {
            assert!(path_exists(s), "status {s} 应视为路径存在");
        }
    }
}
