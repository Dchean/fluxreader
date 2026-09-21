//! HTTP 层：客户端构建与条件 GET（ETag / If-Modified-Since）。

use crate::error::{AppError, AppResult};
use reqwest::header::{CONTENT_TYPE, ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};
use reqwest::{Client, StatusCode};
use std::time::Duration;

/// 桌面 RSS 客户端身份标识（避免被站点风控误伤为爬虫脚本）
pub const USER_AGENT: &str = "FluxReader/0.1 (+https://github.com/fluxreader; RSS reader)";

/// 响应体大小上限：feed 是文本，16 MiB 已很宽裕，防恶意/异常响应耗尽内存
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/* ============================================================
HTTP
============================================================ */

pub fn build_client(timeout_secs: u64) -> Client {
    // 直连源站需要能走用户代理（国内网络访问境外 feed 常见需求）。
    // reqwest 默认读取 HTTP_PROXY/HTTPS_PROXY/ALL_PROXY 环境变量。
    Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(timeout_secs.clamp(5, 300)))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .expect("failed to build reqwest client")
}

/// 条件 GET 结果
pub enum Fetched {
    NotModified,
    Body {
        bytes: Vec<u8>,
        content_type: Option<String>,
        etag: Option<String>,
        last_modified: Option<String>,
    },
}

/// 分块读取响应体，超过上限即中止（防 Content-Length 撒谎的流式响应）
async fn read_capped(mut resp: reqwest::Response) -> AppResult<Vec<u8>> {
    if resp
        .content_length()
        .is_some_and(|n| n > MAX_BODY_BYTES as u64)
    {
        return Err(AppError::new(
            "responseTooLarge",
            "feed body exceeds 16 MiB",
        ));
    }
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        if buf.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(AppError::new(
                "responseTooLarge",
                "feed body exceeds 16 MiB",
            ));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

/// 条件 GET：有 ETag/Last-Modified 时携带，304 直接返回未变更
pub async fn conditional_get(
    client: &Client,
    url: &str,
    etag: Option<&str>,
    last_modified: Option<&str>,
) -> AppResult<Fetched> {
    let mut req = client.get(url);
    if let Some(e) = etag {
        req = req.header(IF_NONE_MATCH, e);
    }
    if let Some(lm) = last_modified {
        req = req.header(IF_MODIFIED_SINCE, lm);
    }
    let resp = req.send().await?;
    if resp.status() == StatusCode::NOT_MODIFIED {
        return Ok(Fetched::NotModified);
    }
    let resp = resp.error_for_status()?;
    let header = |name: reqwest::header::HeaderName| {
        resp.headers()
            .get(&name)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
    };
    let etag = header(ETAG);
    let last_modified = header(LAST_MODIFIED);
    let content_type = header(CONTENT_TYPE);
    let bytes = read_capped(resp).await?;
    Ok(Fetched::Body {
        bytes,
        content_type,
        etag,
        last_modified,
    })
}
