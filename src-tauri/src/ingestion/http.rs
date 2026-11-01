//! HTTP 层：客户端构建与条件 GET（ETag / If-Modified-Since）。

use crate::error::{AppError, AppResult};
use reqwest::header::{CONTENT_TYPE, ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};
use reqwest::{Client, StatusCode};
use std::time::Duration;

/// 桌面 RSS 客户端身份标识（避免被站点风控误伤为爬虫脚本）
pub const USER_AGENT: &str = "FluxReader/0.1 (+https://github.com/fluxreader; RSS reader)";

/// 响应体大小上限：feed 是文本，16 MiB 已很宽裕，防恶意/异常响应耗尽内存。
/// favicon 探测与全文提取等旁路读取共用同一界（read_body_capped 入参）。
pub(crate) const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

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

/// 分块读取响应体，超过 `max_bytes` 即中止。两道防线各自独立：
/// Content-Length 预检挡住明示的超大响应；累计分支防 Content-Length
/// 谎报小值的流式响应。favicon 探测与全文提取等旁路读取共用。
pub(crate) async fn read_body_capped(
    mut resp: reqwest::Response,
    max_bytes: usize,
) -> AppResult<Vec<u8>> {
    if resp.content_length().is_some_and(|n| n > max_bytes as u64) {
        return Err(AppError::new(
            "responseTooLarge",
            "response body exceeds limit",
        ));
    }
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        if buf.len() + chunk.len() > max_bytes {
            return Err(AppError::new(
                "responseTooLarge",
                "response body exceeds limit",
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
    let bytes = read_body_capped(resp, MAX_BODY_BYTES).await?;
    Ok(Fetched::Body {
        bytes,
        content_type,
        etag,
        last_modified,
    })
}

#[cfg(test)]
mod read_body_capped_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// 单连接 loopback mock：响应头声明 `declared_len`，实际发送 `actual_len`
    /// 字节——两值独立可谎报，分别覆盖 read_body_capped 的预检与累计分支。
    fn spawn_lying_mock(declared_len: u64, actual_len: usize) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let Some(Ok(mut stream)) = listener.incoming().next() else {
                return;
            };
            let mut buf = [0u8; 4096];
            let mut req = Vec::new();
            loop {
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    return;
                }
                req.extend_from_slice(&buf[..n]);
                if req.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                 Content-Length: {declared_len}\r\nConnection: close\r\n\r\n"
            );
            let _ = stream.write_all(head.as_bytes());
            let chunk = vec![b'x'; 64 * 1024];
            let mut sent = 0usize;
            while sent < actual_len {
                let n = chunk.len().min(actual_len - sent);
                // 客户端超限中止后写入会失败——忽略，测试只关心读取侧结果
                if stream.write_all(&chunk[..n]).is_err() {
                    return;
                }
                sent += n;
            }
            let _ = stream.flush();
        });
        port
    }

    async fn fetch_capped(url: &str) -> AppResult<Vec<u8>> {
        let resp = build_client(10).get(url).send().await?;
        read_body_capped(resp, MAX_BODY_BYTES).await
    }

    #[tokio::test]
    async fn content_length_precheck_rejects_oversized() {
        // 声明 20 MiB、实发 1 KiB：预检分支直接拒绝，不必读完 body
        let port = spawn_lying_mock(20 * 1024 * 1024, 1024);
        let err = fetch_capped(&format!("http://127.0.0.1:{port}/x"))
            .await
            .expect_err("Content-Length 超限必须在预检分支拒绝");
        assert_eq!(err.code, "responseTooLarge");
    }

    #[tokio::test]
    async fn lying_small_content_length_is_caught_by_accumulation_branch() {
        // 谎报 10 字节、实发超限：预检放行，累计分支必须中止
        let port = spawn_lying_mock(10, MAX_BODY_BYTES + 1024);
        let err = fetch_capped(&format!("http://127.0.0.1:{port}/x"))
            .await
            .expect_err("谎报小 Content-Length 的超限流必须被累计分支中止");
        assert_eq!(err.code, "responseTooLarge");
    }

    #[tokio::test]
    async fn body_within_limit_passes_through() {
        let port = spawn_lying_mock(1024, 1024);
        let body = fetch_capped(&format!("http://127.0.0.1:{port}/x"))
            .await
            .expect("未超限响应必须完整读取");
        assert_eq!(body.len(), 1024);
        assert!(body.iter().all(|b| *b == b'x'));
    }
}
