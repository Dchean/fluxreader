//! Shared service backoff for the two RSS protocols. A rejected request never sleeps
//! while holding a database lock and does not discard its durable local intent.
use crate::error::{AppError, AppResult};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

static COOLDOWNS: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();

fn retry_after_seconds(value: Option<&str>, now: chrono::DateTime<chrono::Utc>) -> u64 {
    value
        .and_then(|value| {
            value.trim().parse().ok().or_else(|| {
                chrono::DateTime::parse_from_rfc2822(value)
                    .ok()
                    .map(|date| {
                        (date.with_timezone(&chrono::Utc) - now)
                            .num_seconds()
                            .max(0) as u64
                    })
            })
        })
        .unwrap_or(60)
        .max(1)
}

pub(crate) async fn send(builder: reqwest::RequestBuilder) -> AppResult<reqwest::Response> {
    // reqwest 0.12.28 RequestBuilder::build_split returns the owning Client and Request.
    let (client, request) = builder.build_split();
    let request = request?;
    let origin = request.url().origin().ascii_serialization();
    {
        let now = Instant::now();
        let mut cooldowns = COOLDOWNS
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        cooldowns.retain(|_, until| *until > now);
        if let Some(until) = cooldowns.get(&origin) {
            return Err(AppError::new(
                "rateLimited",
                format!(
                    "服务端要求稍后重试（约 {} 秒）",
                    until.duration_since(now).as_secs() + 1
                ),
            ));
        }
    }
    let response = client.execute(request).await?;
    let status = response.status().as_u16();
    if status == 429
        || (status == 503
            && response
                .headers()
                .contains_key(reqwest::header::RETRY_AFTER))
    {
        let seconds = retry_after_seconds(
            response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok()),
            chrono::Utc::now(),
        );
        let until = Instant::now()
            .checked_add(Duration::from_secs(seconds))
            .ok_or_else(|| AppError::new("protocol", "服务端 Retry-After 超出支持范围"))?;
        let mut cooldowns = COOLDOWNS
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        cooldowns
            .entry(origin)
            .and_modify(|old| *old = (*old).max(until))
            .or_insert(until);
        return Err(AppError::new(
            "rateLimited",
            format!("服务端暂时限制请求，将在 {seconds} 秒后重试"),
        ));
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retry_after_supports_seconds_dates_and_safe_fallback() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-09T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert_eq!(retry_after_seconds(Some("120"), now), 120);
        assert_eq!(
            retry_after_seconds(Some("Mon, 09 Nov 2026 00:03:00 GMT"), now),
            180
        );
        assert_eq!(retry_after_seconds(Some("invalid"), now), 60);
    }

    #[tokio::test]
    async fn retry_after_blocks_new_requests_until_the_server_deadline() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/state", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicUsize::new(0));
        let server_hits = hits.clone();
        let server = tokio::spawn(async move {
            for response in [
                "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                    let mut chunk = [0; 2048];
                    let read = socket.read(&mut chunk).await.unwrap();
                    assert!(read > 0, "request closed before complete headers");
                    request.extend_from_slice(&chunk[..read]);
                    assert!(request.len() <= 16 * 1024, "request header budget");
                }
                server_hits.fetch_add(1, Ordering::SeqCst);
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let client = reqwest::Client::new();
        let first = send(client.get(&url)).await.unwrap_err();
        assert_eq!(first.code, "rateLimited");
        for _ in 0..3 {
            assert_eq!(
                send(client.get(&url)).await.unwrap_err().code,
                "rateLimited"
            );
        }
        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "new clicks must not bypass the origin cooldown"
        );
        tokio::time::sleep(Duration::from_millis(1100)).await;
        assert_eq!(send(client.get(&url)).await.unwrap().status(), 200);
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }
}
