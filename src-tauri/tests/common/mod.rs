//! 集成测试共享 helper（TASK-096 测试基建收口）。
//!
//! 背景（基线证据原文在存档分支 archive/tooling-and-reports）：CI cargo test
//! 偶发失败（CI #101、2026-09-28 本地 verify 均复现）——cargo test 把同一
//! 测试二进制的多个 #[test] 作为线程并发跑在同一进程内，此前各测试文件
//! 自拼的临时库名唯一性只靠时钟纳秒，而 Windows 时钟在密集调用下精度不足
//! （实测 1000 次紧邻 `as_nanos()` 仅产生 350 个不同值），库名碰撞后两个
//! 测试互相 remove_file / 争用同一 SQLite 文件，`db::open` 建库期报
//! "table folders already exists"。实测碰撞率：subsec_nanos 2.95%、
//! as_nanos 3.05%（同等危险）；as_nanos + 进程内 AtomicU64 计数器 = 0/2000。
//!
//! 粒度说明：每个集成测试二进制会各自编译一份本模块——AtomicU64 是
//! 进程内计数，恰好是正确粒度（碰撞只发生在同进程线程间；跨二进制
//! std::process::id() 不同，天然不撞）。
//!
//! `unique_db_path` 仅用 std 实现；OPT-016A 的自托管 feed 服务（LocalFeedServer）
//! 复用项目已有的 tokio 依赖（Cargo.toml 无 dev-dependencies，仍不新增任何依赖）。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// 进程内递增序号：同进程内每次调用必然不同，是唯一性的主保证；
/// pid 区分不同测试二进制，时钟纳秒只作可读性前缀。
static UNIQUE_SEQ: AtomicU64 = AtomicU64::new(0);

/// 返回进程内唯一的临时 SQLite 库路径：`fluxreader_{base}_{pid}_{nanos}_{seq}.db`。
///
/// 保持既有 `fluxreader_<base>_…db` 命名风格；不触碰文件系统，
/// 既有调用方的 remove_file 语义原样保留（由调用方决定）。
#[allow(dead_code)] // 跨 test target 共享，未用到的 target 会报 dead_code，显式豁免（同 mock_greader 惯例）
pub fn unique_db_path(base: &str) -> PathBuf {
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let seq = UNIQUE_SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("fluxreader_{base}_{pid}_{nanos}_{seq}.db"))
}

/* ============================================================
OPT-016A：自托管 feed HTTP 服务 + 临时库清理守卫
============================================================ */

/// 临时库文件守卫：正常返回与 panic 展开（Drop）都会删除本测试创建的临时
/// SQLite 库及其 WAL 旁路文件。使用约定：只包裹 `unique_db_path` 产出的路径；
/// 声明**先于** Connection，保证 Drop 时连接已关闭（Windows 上仍打开的文件
/// 无法删除）。
#[allow(dead_code)] // 跨 test target 共享，未用到的 target 会报 dead_code（同 unique_db_path 惯例）
pub struct TempDbGuard {
    path: PathBuf,
}

#[allow(dead_code)]
impl TempDbGuard {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for TempDbGuard {
    fn drop(&mut self) {
        let base = self.path.display().to_string();
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(format!("{base}-wal"));
        let _ = std::fs::remove_file(format!("{base}-shm"));
    }
}

/// 固定的确定性 feed 内容（端口注入；与历史 fixtures/local_feed.xml 同构：
/// 标题、两条条目、相对链接与相对图片，用于验证解析/消毒的相对 URL 解析）。
/// 额外带 channel `<image>`：解析后 icon 非空，刷新管线不再 spawn favicon
/// 后台探测任务——自托管测试结束时不残留并发请求与连接。
#[allow(dead_code)] // 跨 test target 共享，未用到的 target 会报 dead_code
pub fn local_feed_xml(port: u16) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
<channel>
  <title>Local Test Feed</title>
  <link>http://127.0.0.1:{port}/</link>
  <description>self-hosted deterministic fixture</description>
  <image>
    <url>http://127.0.0.1:{port}/icon.png</url>
    <title>Local Test Feed</title>
    <link>http://127.0.0.1:{port}/</link>
  </image>
  <item>
    <title>Direct fetch entry (newest)</title>
    <link>/post/1</link>
    <guid isPermaLink="false">fixture-001</guid>
    <pubDate>Mon, 24 Aug 2026 11:00:00 GMT</pubDate>
    <description><![CDATA[<p>Newest entry: relative link and relative image must resolve against the feed base URL.</p><img src="/img/a.png" />]]></description>
  </item>
  <item>
    <title>Older entry</title>
    <link>/post/2</link>
    <guid isPermaLink="false">fixture-002</guid>
    <pubDate>Mon, 24 Aug 2026 10:00:00 GMT</pubDate>
    <description><![CDATA[<p>Older entry for newest-first ordering assertion.</p>]]></description>
  </item>
</channel>
</rss>
"#
    )
}

/// 自托管 feed HTTP 服务：只绑定 127.0.0.1 且端口由内核分配（`:0`），仅回应
/// 本测试进程的请求——不依赖、不占用固定端口（如 8765），也不借用用户现有服务。
/// 行为：
/// - `GET /local_feed.xml` → 200 + `start_with_feed` 渲染的固定正文 +
///   固定 `ETag`/`Last-Modified`；
/// - 携带匹配 `If-None-Match`/`If-Modified-Since` 的复请求 → 304（条件 GET 真实路径）；
/// - 其它路径/方法 → 404，不触任何外部资源。
///
/// 有界性：每条连接读请求头上限 16 KiB、整体 5s 超时；响应 `Connection: close`
/// 写完即关连接。`stop()` 显式停 accept 循环并等待退出；panic 展开路径由 Drop
/// 兜底中止，避免任务悬挂。
#[allow(dead_code)] // 跨 test target 共享，未用到的 target 会报 dead_code
pub struct LocalFeedServer {
    port: u16,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    join: Option<tokio::task::JoinHandle<()>>,
}

#[allow(dead_code)]
impl LocalFeedServer {
    /// 绑定 `127.0.0.1:0`，用实际端口渲染固定 feed 内容（渲染在 spawn 前完成）。
    pub async fn start_with_feed(render_feed: impl FnOnce(u16) -> String) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let body: Arc<str> = Arc::from(render_feed(port));
        let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
        let join = tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    _ = &mut stop_rx => break,
                    r = listener.accept() => r,
                };
                let Ok((stream, _)) = accepted else { continue };
                let body = body.clone();
                tokio::spawn(async move {
                    // 每连接 5s 上限：半开/慢连接只拖累自己，不挂死测试
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(5),
                        serve_feed_conn(stream, body),
                    )
                    .await;
                });
            }
        });
        Ok(Self {
            port,
            stop: Some(stop_tx),
            join: Some(join),
        })
    }

    /// 服务地址（含实际端口），`path` 需以 `/` 开头。
    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    /// 显式关闭：停 accept 循环并等待其退出（各连接随响应完成关闭）。
    pub async fn stop(mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        if let Some(join) = self.join.take() {
            let _ = join.await;
        }
    }
}

impl Drop for LocalFeedServer {
    fn drop(&mut self) {
        // panic 展开路径兜底：不等 await，直接中止 accept 循环，避免后台任务悬挂
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        if let Some(join) = self.join.take() {
            join.abort();
        }
    }
}

/// 固定验证器：正文不变，验证器字符串也不变。
const FEED_ETAG: &str = "\"fluxreader-local-feed-v1\"";
const FEED_LAST_MODIFIED: &str = "Mon, 24 Aug 2026 11:00:00 GMT";
const FEED_PATH: &str = "/local_feed.xml";

/// 单连接处理：只读请求头（GET 无 body），按条件响应规则回 200/304/404。
async fn serve_feed_conn(mut stream: TcpStream, body: Arc<str>) -> std::io::Result<()> {
    let mut buf: Vec<u8> = Vec::new();
    let head_end = loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos;
        }
        if buf.len() > 16 * 1024 {
            // 请求头超限：有界拒绝，不继续读
            return feed_respond(&mut stream, "431 Request Header Fields Too Large", &[], b"")
                .await;
        }
        let mut tmp = [0u8; 2048];
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&tmp[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or_default();
    let mut request = request_line.split_whitespace();
    let method = request.next().unwrap_or_default();
    let path = request
        .next()
        .unwrap_or_default()
        .split('?')
        .next()
        .unwrap_or_default();

    if method != "GET" || path != FEED_PATH {
        return feed_respond(&mut stream, "404 Not Found", &[], b"not found").await;
    }

    let mut not_modified = false;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim();
            if name == "if-none-match" && value == FEED_ETAG {
                not_modified = true;
            }
            if name == "if-modified-since" && value == FEED_LAST_MODIFIED {
                not_modified = true;
            }
        }
    }
    let validators = [("ETag", FEED_ETAG), ("Last-Modified", FEED_LAST_MODIFIED)];
    if not_modified {
        return feed_respond(&mut stream, "304 Not Modified", &validators, b"").await;
    }
    let mut headers = vec![("Content-Type", "application/rss+xml; charset=utf-8")];
    headers.extend_from_slice(&validators);
    feed_respond(&mut stream, "200 OK", &headers, body.as_bytes()).await
}

async fn feed_respond(
    stream: &mut TcpStream,
    status: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> std::io::Result<()> {
    let mut resp = format!(
        "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (name, value) in headers {
        resp.push_str(&format!("{name}: {value}\r\n"));
    }
    resp.push_str("\r\n");
    stream.write_all(resp.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await
}
