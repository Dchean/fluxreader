//! HTTP 层：客户端构建与条件 GET（ETag / If-Modified-Since）。

use crate::error::{AppError, AppResult};
use reqwest::header::{CONTENT_TYPE, ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};
use reqwest::{Client, StatusCode};
use std::time::Duration;

/// 桌面 RSS 客户端身份标识（避免被站点风控误伤为爬虫脚本）
pub const USER_AGENT: &str = "FluxReader/0.1 (+https://github.com/fluxreader; RSS reader)";

/// 响应体大小上限：feed 是文本，16 MiB 已很宽裕，防恶意/异常响应耗尽内存。
/// favicon 探测与全文提取等旁路读取共用同一界（read_body_capped 入参）。
pub const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

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
/// Content-Length 预检挡住明示的超大响应；累计分支防**无 Content-Length
/// 定界（chunked / EOF）的超量流式响应**——hyper 客户端按 CL 定界，
/// 谎报小 CL 只会让 body 提前完成（截断），不会绕过累计上限。
/// favicon 探测与全文提取等旁路读取共用。
pub async fn read_body_capped(mut resp: reqwest::Response, max_bytes: usize) -> AppResult<Vec<u8>> {
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

/// 文本消费方复用同一收体预算和 charset 解码，不能先 text()/json() 后再检查。
pub async fn read_text_capped(resp: reqwest::Response, max_bytes: usize) -> AppResult<String> {
    let content_type = resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = read_body_capped(resp, max_bytes).await?;
    Ok(decode_body_bytes(&bytes, content_type.as_deref()))
}

/// 错误详情只需要有界前缀。读到 8 KiB 后最多再看一块以判定截断，随后释放
/// 响应；不为一条错误通知收集整页/无限流，截断以明确标记呈现。
pub async fn read_error_detail(mut resp: reqwest::Response) -> AppResult<String> {
    const MAX_ERROR_BYTES: usize = 8 * 1024;
    let content_type = resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let mut bytes = Vec::new();
    let mut truncated = false;
    while let Some(chunk) = resp.chunk().await? {
        let remaining = MAX_ERROR_BYTES - bytes.len();
        bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        if chunk.len() > remaining {
            truncated = true;
            break;
        }
    }
    let mut text = decode_body_bytes(&bytes, content_type.as_deref());
    if truncated {
        text.push_str("…（响应详情已截断）");
    }
    Ok(text)
}

/// 按 Content-Type 的 charset 参数解码响应体，语义与 `reqwest::text()`
/// 一致：无 charset 头或未知标签按 UTF-8，BOM 优先于标签，无效字节
/// lossy 替换。read_body_capped 收的是原始字节，文本类调用方必须经
/// 此解码——GBK/GB2312 站点按 UTF-8 硬解会把乱码写进正文。
pub fn decode_body_bytes(bytes: &[u8], content_type: Option<&str>) -> String {
    let charset = content_type.and_then(|ct| {
        ct.split(';').map(str::trim).find_map(|param| {
            let lower = param.to_ascii_lowercase();
            lower
                .starts_with("charset=")
                .then(|| param["charset=".len()..].trim().trim_matches('"'))
        })
    });
    let encoding = charset
        .and_then(|c| encoding_rs::Encoding::for_label(c.as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    let (text, _had_errors, _) = encoding.decode(bytes);
    text.into_owned()
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

    /// 单连接 loopback mock：`content_length` 为 Some 时声明该 CL（预检分支
    /// 依据），None 时不发 CL、用 `Connection: close` + EOF 定界——hyper 客户端
    /// 按 CL 定界，谎报小 CL 只会让 body 提前完成，累计分支只能由无 CL 的
    /// 超量流触达。实发 `actual_len` 字节后关连接。
    fn spawn_streaming_mock(content_length: Option<u64>, actual_len: usize) -> u16 {
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
            let cl_header = content_length
                .map(|cl| format!("Content-Length: {cl}\r\n"))
                .unwrap_or_default();
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                 {cl_header}Connection: close\r\n\r\n"
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
        // 诚实声明 20 MiB：预检分支依据响应头直接拒绝，不读 body
        let port = spawn_streaming_mock(Some(20 * 1024 * 1024), 1024);
        let err = fetch_capped(&format!("http://127.0.0.1:{port}/x"))
            .await
            .expect_err("Content-Length 超限必须在预检分支拒绝");
        assert_eq!(err.code, "responseTooLarge");
    }

    #[tokio::test]
    async fn oversize_stream_without_content_length_is_caught_by_accumulation_branch() {
        // 无 CL 定界（EOF）、实发超限：预检无从下手，累计分支必须中止
        let port = spawn_streaming_mock(None, MAX_BODY_BYTES + 1024);
        let err = fetch_capped(&format!("http://127.0.0.1:{port}/x"))
            .await
            .expect_err("无 Content-Length 定界的超量流必须被累计分支中止");
        assert_eq!(err.code, "responseTooLarge");
    }

    #[tokio::test]
    async fn body_within_limit_passes_through() {
        let port = spawn_streaming_mock(None, 1024);
        let body = fetch_capped(&format!("http://127.0.0.1:{port}/x"))
            .await
            .expect("未超限响应必须完整读取");
        assert_eq!(body.len(), 1024);
        assert!(body.iter().all(|b| *b == b'x'));
    }

    #[test]
    fn decode_body_bytes_follows_charset_header() {
        // GBK 的「中文」（中=D6D0 文=CEC4）：charset 标签必须生效
        let gbk = [0xD6u8, 0xD0, 0xCE, 0xC4];
        assert_eq!(
            decode_body_bytes(&gbk, Some("text/html; charset=GBK")),
            "中文",
            "charset=GBK 必须生效（原 text() 的解码语义）"
        );
        // 无 charset 头默认 UTF-8；注意两点：GBK 字节对 D6D0/CEC4 恰是
        // 合法的两字节 UTF-8 序列（解码为错误但有效的字符），而 FF/FE
        // 是 UTF-16LE BOM 会被 decode() 的 BOM 嗅探按 UTF-16LE 解码——
        // lossy 断言须用 FF/FF：永不合法且非 BOM 前缀 → 两个 U+FFFD
        assert_eq!(decode_body_bytes(&[0xFFu8, 0xFF], None), "\u{FFFD}\u{FFFD}");
        assert_eq!(
            decode_body_bytes("中文".as_bytes(), Some("text/plain")),
            "中文"
        );
        // 未知标签按 UTF-8
        assert_eq!(
            decode_body_bytes("ok".as_bytes(), Some("text/plain; charset=x-unknown")),
            "ok"
        );
    }
}
