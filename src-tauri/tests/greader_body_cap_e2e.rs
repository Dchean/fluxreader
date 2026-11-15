//! P1-2：GR 响应读取点的 16 MiB 硬界（超界单测两形态：chunked + 压缩膨胀）。
//!
//! GR 四个读取点已收口到 `ingestion::http::read_body_capped / read_text_capped`；
//! GR 侧读体失败语义 = **network** 分类 + 中文文案（诊断白名单
//! `diagnostics::error_class` 只认现行码）。本文件用自备裸 TCP mock 分别发送
//! chunked（无 Content-Length）与 brotli 压缩膨胀两种超限响应，验证 GR 路径
//! （decode_json_response 的 bytes 收体点与 ClientLogin 的 text 收体点）在
//! 读取中被硬界拦住。brotli 载荷与 base64 解码取自 `fever_compat_e2e.rs`
//! （node zlib 预生成，同一份真值）。

use app_lib::greader::GReaderClient;
use app_lib::ingestion::build_client;
use std::io::{Read, Write};
use std::net::TcpListener;

/// 17 MiB 重复字节的 brotli 压缩流（node zlib 预生成，40 字节）。
/// 与 `fever_compat_e2e.rs::BROTLI_17MIB_B64` 同源。
const BROTLI_17MIB_B64: &str = "y///P/gl8OKxQCD3/o///3/wSwDEYRGA7v1f/f8/fgmAOCwA0L1/AA==";

/// 极简 base64 解码（测试用；忽略 `=` 与空白）。载荷由 node zlib 预生成。
fn base64_decode(input: &str) -> Vec<u8> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::new();
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for &c in input.as_bytes() {
        let Some(v) = val(c) else { continue };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xFF) as u8);
        }
    }
    out
}

enum MockKind {
    /// `Transfer-Encoding: chunked`，无 Content-Length，总载荷 17 MiB。
    Chunked17MiB,
    /// `Content-Encoding: br`，线上 40 字节，解压后 17 MiB。
    Brotli17MiB,
}

/// 裸 TCP mock：对每个连接读掉请求头后发送同一份原始响应。测试客户端每个
/// 用例只发起 1 个请求，多留几个连接名额避免竞态挂死；客户端超限中止后
/// 写入失败即退出（测试只关心读取侧结果）。
fn spawn_raw_mock(kind: MockKind) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for _ in 0..4 {
            let Ok((mut stream, _)) = listener.accept() else {
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
            match kind {
                MockKind::Chunked17MiB => {
                    let head = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                                Transfer-Encoding: chunked\r\n\r\n";
                    if stream.write_all(head.as_bytes()).is_err() {
                        return;
                    }
                    let chunk = vec![b'x'; 64 * 1024];
                    let total = 17 * 1024 * 1024;
                    let mut sent = 0usize;
                    while sent < total {
                        let n = chunk.len().min(total - sent);
                        // chunked 编码：十六进制块大小行 + 数据 + CRLF
                        if stream.write_all(format!("{n:X}\r\n").as_bytes()).is_err() {
                            return;
                        }
                        if stream.write_all(&chunk[..n]).is_err() {
                            return;
                        }
                        if stream.write_all(b"\r\n").is_err() {
                            return;
                        }
                        sent += n;
                    }
                    let _ = stream.write_all(b"0\r\n\r\n");
                }
                MockKind::Brotli17MiB => {
                    let body = base64_decode(BROTLI_17MIB_B64);
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                         Content-Encoding: br\r\nContent-Length: {}\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(&body);
                }
            }
            let _ = stream.flush();
        }
    });
    port
}

/// decode_json_response 点（bytes 收体）：chunked 无 CL 的 17 MiB 响应必须
/// 在读取中被 16 MiB 硬界拦下，错误保持 network 分类并点明 16 MiB。
#[tokio::test]
async fn subscription_list_chunked_oversize_is_capped_as_network() {
    let port = spawn_raw_mock(MockKind::Chunked17MiB);
    let client = GReaderClient::new(
        &format!("http://127.0.0.1:{port}"),
        "token",
        build_client(10),
    );
    let err = client
        .subscriptions()
        .await
        .expect_err("chunked 超限响应必须失败");
    assert_eq!(err.code, "network", "GR 读体失败保持 network 分类：{err}");
    assert!(
        err.message.contains("16 MiB"),
        "文案须点明 16 MiB 上限：{err}"
    );
}

/// decode_json_response 点：线上 40 字节 brotli 流解压出 17 MiB——上限必须
/// 按**解压后**的 chunk 累计生效（压缩膨胀形态）。
#[tokio::test]
async fn subscription_list_brotli_inflated_oversize_is_capped_as_network() {
    let body = base64_decode(BROTLI_17MIB_B64);
    assert!(
        body.len() < 1024,
        "线上载荷应为压缩后的小字节：{}",
        body.len()
    );
    let port = spawn_raw_mock(MockKind::Brotli17MiB);
    let client = GReaderClient::new(
        &format!("http://127.0.0.1:{port}"),
        "token",
        build_client(10),
    );
    let err = client
        .subscriptions()
        .await
        .expect_err("解压后超 16 MiB 的响应必须失败");
    assert_eq!(err.code, "network", "GR 读体失败保持 network 分类：{err}");
    assert!(
        err.message.contains("16 MiB"),
        "文案须点明 16 MiB 上限：{err}"
    );
}

/// ClientLogin 文本收体点（read_text_capped）：chunked 超限同样被拦，
/// 错误保持 network 分类并点明 16 MiB。
#[tokio::test]
async fn client_login_chunked_oversize_is_capped_as_network() {
    let port = spawn_raw_mock(MockKind::Chunked17MiB);
    let err = GReaderClient::login_resolved(
        &format!("http://127.0.0.1:{port}"),
        "user",
        "pass",
        build_client(10),
    )
    .await
    .expect_err("登录响应超限必须失败");
    assert_eq!(err.code, "network", "GR 读体失败保持 network 分类：{err}");
    assert!(
        err.message.contains("16 MiB"),
        "文案须点明 16 MiB 上限：{err}"
    );
}
