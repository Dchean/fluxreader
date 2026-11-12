//! App-owned image requests: bounded admission and cancellable HTTP lifetime, no DB lock.
use crate::error::{AppError, AppResult};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

pub const MAX_ACTIVE_IMAGES: usize = 4;
#[derive(Default)]
pub struct ImageRequests {
    active: Mutex<HashMap<String, watch::Sender<bool>>>,
}
pub struct ImageRequest {
    owner: Arc<ImageRequests>,
    key: String,
    cancelled: watch::Receiver<bool>,
}
impl ImageRequests {
    pub fn begin(self: &Arc<Self>, window: &str, request_id: &str) -> AppResult<ImageRequest> {
        if request_id.is_empty() || request_id.len() > 128 {
            return Err(AppError::new("badImageRequest", "无效图片请求"));
        }
        let key = format!("{window}\n{request_id}");
        let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        if active.contains_key(&key) || active.len() >= MAX_ACTIVE_IMAGES {
            return Err(AppError::new("imageBusy", "图片请求正在排队"));
        }
        let (tx, rx) = watch::channel(false);
        active.insert(key.clone(), tx);
        Ok(ImageRequest {
            owner: self.clone(),
            key,
            cancelled: rx,
        })
    }
    pub fn cancel(&self, window: &str, request_id: &str) {
        let key = format!("{window}\n{request_id}");
        if let Some(tx) = self
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
        {
            tx.send_replace(true);
        }
    }
}
impl ImageRequest {
    pub async fn run<T>(
        mut self,
        operation: impl std::future::Future<Output = AppResult<T>>,
    ) -> AppResult<T> {
        tokio::select! {
            biased;
            _ = self.cancelled() => Err(AppError::new("imageCancelled", "图片请求已取消")),
            result = tokio::time::timeout(std::time::Duration::from_secs(30), operation) => {
                result.unwrap_or_else(|_| Err(AppError::new("network", "图片加载超时，可重试")))
            }
        }
    }
    pub async fn cancelled(&mut self) {
        loop {
            if *self.cancelled.borrow_and_update() {
                return;
            }
            if self.cancelled.changed().await.is_err() {
                return;
            }
        }
    }
}
impl Drop for ImageRequest {
    fn drop(&mut self) {
        self.owner
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    use std::time::{Duration, Instant};

    #[tokio::test]
    async fn bounded_admission_early_cancel_and_old_id_do_not_affect_new_request() {
        let requests = Arc::new(ImageRequests::default());
        let holds = (0..4)
            .map(|i| requests.begin("main", &i.to_string()).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            requests.begin("main", "overflow").err().unwrap().code,
            "imageBusy"
        );
        drop(holds);
        let early = requests.begin("main", "early").unwrap();
        requests.cancel("other-window", "early");
        requests.cancel("main", "early");
        let polled = AtomicBool::new(false);
        let error = early
            .run(async {
                polled.store(true, Ordering::Relaxed);
                Ok(())
            })
            .await
            .unwrap_err();
        assert_eq!(error.code, "imageCancelled");
        assert!(
            !polled.load(Ordering::Relaxed),
            "cancel before send must not start the HTTP future"
        );
        let fresh = requests.begin("main", "fresh").unwrap();
        requests.cancel("main", "early");
        assert_eq!(fresh.run(async { Ok(42) }).await.unwrap(), 42);
        assert!(requests.active.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn cancelling_during_body_read_drops_response_and_returns_admission() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let (release_tx, release_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(e) => panic!("accept: {e}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = Vec::new();
            let mut buf = [0; 1024];
            while !request.windows(4).any(|s| s == b"\r\n\r\n") {
                let n = stream.read(&mut buf).unwrap();
                assert!(n > 0);
                request.extend_from_slice(&buf[..n]);
            }
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nConnection: close\r\n\r\nbody",
                )
                .unwrap();
            stream.flush().unwrap();
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            stream.set_nonblocking(true).unwrap();
            let chunk = [0_u8; 64 * 1024];
            let mut sent = 0;
            let deadline = Instant::now() + Duration::from_secs(3);
            while sent < 32 * 1024 * 1024 && Instant::now() < deadline {
                match stream.write(&chunk) {
                    Ok(n) => {
                        if n == 0 {
                            break;
                        }
                        sent += n;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(_) => break,
                }
            }
            sent
        });
        let requests = Arc::new(ImageRequests::default());
        let registration = requests.begin("main", "reading").unwrap();
        let (body_started, reached_body) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            registration
                .run(async move {
                    let mut response = crate::ingestion::build_client(5)
                        .get(format!("http://{address}/image"))
                        .send()
                        .await?;
                    assert!(response.chunk().await?.is_some());
                    body_started.send(()).unwrap();
                    crate::ingestion::read_body_capped(response, 25 * 1024 * 1024).await
                })
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), reached_body)
            .await
            .unwrap()
            .unwrap();
        requests.cancel("main", "reading");
        let result = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.unwrap_err().code, "imageCancelled");
        release_tx.send(()).unwrap();
        assert!(
            server.join().unwrap() < 32 * 1024 * 1024,
            "server cannot send whole body after cancellation"
        );
        assert!(requests.active.lock().unwrap().is_empty());
    }
}
