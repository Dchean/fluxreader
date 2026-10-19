//! OPT-013A：AI 流式完成信号与错误分型的严格 loopback 集成测试。
//!
//! 本地一次性 TcpListener 夹具逐段发送真实 HTTP/SSE 字节（规范事件分隔
//! `data:` 行 + 空行、跨 TCP chunk 的 UTF-8 切分/BOM、CRLF、无尾换行帧），验证：
//!   - 只有 finish_reason="stop" 且收尾为 [DONE]（权威终态，立即返回不等待
//!     socket 关闭）或干净 EOF 才是 completed=true；
//!   - length/content_filter/tool_calls/未知 finish/无 finish 的干净 EOF、
//!     DONE 先于 stop 都是显式错误；
//!   - 重复/冲突 finish、finish 后增量被拒绝；DONE 之后的数据不再消费；
//!   - 流首可选 BOM（可跨 chunk）只剥一次，不吞首 delta；多行 data 合并解析；
//!   - 非 JSON data、坏 UTF-8、单行/残留/单事件超预算、超大注释行都显式失败；
//!   - sink 关闭（前端取消）→ Ok(completed=false)，不是错误也不算完成。
//!
//! 不访问任何真实外部服务；运行：`cargo test --test ai_stream_completion_e2e`。

use app_lib::ai::{self, AiConfig, ChatOutcome};
use app_lib::error::{AppError, AppResult};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

/// 启动一次性 loopback HTTP 服务器：依次写出 `parts`，每段之间小睡以制造真实
/// TCP chunk 切分；写完关闭连接（干净 EOF）。仅处理第一个连接。
fn start_sse_server(parts: Vec<Vec<u8>>) -> u16 {
    start_sse_server_hold(parts, Duration::ZERO)
}

/// 同 [`start_sse_server`]，但写完 `parts` 后保持连接打开 `hold` 时长再关闭——
/// 用于验证「DONE 后不等待 socket close 立即返回」的 keep-alive 场景。
fn start_sse_server_hold(parts: Vec<Vec<u8>>, hold: Duration) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        // 只处理第一个连接（每个测试发一次请求）；连接失败则直接结束
        let Some(Ok(mut stream)) = listener.incoming().next() else {
            return;
        };
        // 读走请求（读到请求头结束即可，正文内容不关心）
        let mut req = Vec::new();
        let mut buf = [0u8; 4096];
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
        for part in &parts {
            if stream.write_all(part).is_err() {
                return; // 客户端已断开（例如行缓冲超限提前失败）
            }
            let _ = stream.flush();
            thread::sleep(Duration::from_millis(15));
        }
        thread::sleep(hold);
        // 自然结束 → 关闭连接（干净 EOF）
    });
    port
}

const HTTP_HEAD: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";

fn http_parts(body: &str) -> Vec<Vec<u8>> {
    vec![HTTP_HEAD.to_vec(), body.as_bytes().to_vec()]
}

fn cfg_for(port: u16) -> AiConfig {
    AiConfig {
        api_key: "sk-test".into(),
        model: "deepseek-chat".into(),
        base_url: format!("http://127.0.0.1:{port}"),
    }
}

/// 跑一次 stream_chat，返回结果与收到的增量。
async fn run_stream(parts: Vec<Vec<u8>>) -> (AppResult<ChatOutcome>, Vec<String>) {
    let port = start_sse_server(parts);
    let client = app_lib::ingestion::build_client(30);
    let mut deltas: Vec<String> = Vec::new();
    let mut sink = |d: &str| {
        deltas.push(d.to_string());
        true
    };
    let outcome = ai::stream_chat(
        &client,
        &cfg_for(port),
        "sys",
        "user",
        &mut sink,
        ai::SUMMARY_MAX_TOKENS,
    )
    .await;
    (outcome, deltas)
}

/// 断言 Err 且错误码精确匹配。
fn expect_err(res: AppResult<ChatOutcome>, code: &str) -> AppError {
    match res {
        Ok(outcome) => panic!(
            "预期 Err({code})，实际 Ok(completed={}, text={:?})",
            outcome.completed, outcome.text
        ),
        Err(err) => {
            assert_eq!(err.code, code, "错误码不匹配：{err}");
            err
        }
    }
}

/// 基础绿灯：delta + finish stop + [DONE]（CRLF、规范空行分隔、[DONE] 尾帧无换行）。
#[tokio::test]
async fn stop_with_done_is_complete_crlf_and_tail_without_newline() {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"你\"}}],\"error\":null}\r\n",
        "\r\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"好，世界\"}}]}\r\n",
        "\r\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\r\n",
        "\r\n",
        "data: [DONE]" // 尾帧无换行也必须处理（EOF 派发）
    );
    let (res, deltas) = run_stream(http_parts(body)).await;
    let outcome = res.expect("finish stop + [DONE] 必须完成");
    assert!(outcome.completed, "stop 且收到 [DONE] 才算自然完整");
    assert_eq!(outcome.text, "你好，世界");
    assert_eq!(deltas, vec!["你", "好，世界"], "增量与全文一致");
}

/// 跨 TCP chunk 切开的 UTF-8 多字节序列必须重组成完整行、不得损坏字符。
#[tokio::test]
async fn utf8_split_across_tcp_chunks_is_reassembled() {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"前\"}}]}\n",
        "\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"世界\"}}]}\n",
        "\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n",
        "\n",
        "data: [DONE]\n"
    );
    let bytes = body.as_bytes().to_vec();
    // 在「世」的 UTF-8 序列中间切开，强制跨 chunk 重组
    let cut = body.find('世').unwrap() + 1;
    let parts = vec![
        HTTP_HEAD.to_vec(),
        bytes[..cut].to_vec(),
        bytes[cut..].to_vec(),
    ];
    let (res, _) = run_stream(parts).await;
    let outcome = res.expect("跨 chunk UTF-8 不得损坏");
    assert!(outcome.completed);
    assert_eq!(outcome.text, "前世界");
}

/// P2-1：流首合法 UTF-8 BOM 必须被剥除——首 delta 不能被吞，且流仍正常完成。
#[tokio::test]
async fn leading_bom_is_stripped_before_first_event() {
    let mut body: Vec<u8> = vec![0xEF, 0xBB, 0xBF];
    body.extend_from_slice(
        concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"第一段\"}}]}\r\n\r\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\r\n\r\n",
            "data: [DONE]\r\n\r\n"
        )
        .as_bytes(),
    );
    let (res, deltas) = run_stream(vec![HTTP_HEAD.to_vec(), body]).await;
    let outcome = res.expect("BOM 不得导致失败");
    assert!(outcome.completed);
    assert_eq!(outcome.text, "第一段", "BOM 不得吞掉首 delta");
    assert_eq!(deltas, vec!["第一段"]);
}

/// P2-1：BOM 三个字节跨 TCP chunk——凑齐后剥一次，首 delta 不丢。
#[tokio::test]
async fn leading_bom_split_across_chunks_is_stripped() {
    let tail = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"首字\"}}]}\r\n\r\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\r\n\r\n",
        "data: [DONE]\r\n\r\n"
    );
    let mut rest: Vec<u8> = vec![0xBB, 0xBF];
    rest.extend_from_slice(tail.as_bytes());
    let parts = vec![HTTP_HEAD.to_vec(), vec![0xEF], rest];
    let (res, _) = run_stream(parts).await;
    let outcome = res.expect("跨 chunk BOM 不得损坏流");
    assert!(outcome.completed);
    assert_eq!(outcome.text, "首字");
}

/// 规范事件：同一事件的多条 data: 行以 \n 连接为一个 JSON 载荷再解析，
/// 不能逐行独立 parse 丢事件语义。
#[tokio::test]
async fn multiline_data_event_is_joined_per_sse_spec() {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":\r\n",
        "data: {\"content\":\"多行事件\"}}]}\r\n",
        "\r\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\r\n\r\n",
        "data: [DONE]\r\n\r\n"
    );
    let (res, _) = run_stream(http_parts(body)).await;
    let outcome = res.expect("多行 data 事件应合并解析");
    assert!(outcome.completed);
    assert_eq!(outcome.text, "多行事件");
}

/// finish_reason=length → Err aiTruncated；已推送增量保留（前端可见部分文本）。
#[tokio::test]
async fn length_finish_is_ai_truncated() {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"被截断的正文\"}}]}\r\n\r\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\r\n\r\n",
        "data: [DONE]\r\n\r\n"
    );
    let (res, deltas) = run_stream(http_parts(body)).await;
    let err = expect_err(res, "aiTruncated");
    assert!(err.message.contains("长度"), "{err}");
    assert_eq!(deltas, vec!["被截断的正文"], "截断前已渲染的增量不得撤回");
}

/// finish_reason=content_filter → Err aiFiltered。
#[tokio::test]
async fn content_filter_finish_is_ai_filtered() {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"部分内容\"}}]}\r\n\r\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"content_filter\"}]}\r\n\r\n"
    );
    let (res, deltas) = run_stream(http_parts(body)).await;
    let err = expect_err(res, "aiFiltered");
    assert!(err.message.contains("过滤"), "{err}");
    assert_eq!(deltas, vec!["部分内容"]);
}

/// 干净 EOF（HTTP 正常关闭）但没有 finish_reason → aiIncomplete，不是完成。
#[tokio::test]
async fn clean_eof_without_finish_is_incomplete() {
    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"半截\"}}]}\r\n\r\n";
    let (res, deltas) = run_stream(http_parts(body)).await;
    let err = expect_err(res, "aiIncomplete");
    assert!(err.message.contains("finish_reason"), "{err}");
    assert_eq!(deltas, vec!["半截"]);
}

/// 只有 [DONE] 没有 finish_reason → 不能称自然完整。
#[tokio::test]
async fn done_without_finish_reason_is_incomplete() {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"只有正文\"}}]}\r\n\r\n",
        "data: [DONE]\r\n\r\n"
    );
    let (res, _) = run_stream(http_parts(body)).await;
    expect_err(res, "aiIncomplete");
}

/// 次序契约：stop 必须先于 DONE；DONE 先到立即 aiIncomplete，其后的数据不再读取。
#[tokio::test]
async fn done_before_stop_is_incomplete() {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\r\n\r\n",
        "data: [DONE]\r\n\r\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\r\n\r\n"
    );
    let (res, _) = run_stream(http_parts(body)).await;
    expect_err(res, "aiIncomplete");
}

/// finish_reason=stop 但无 [DONE]（尾帧无换行、干净 EOF）→ 正常关闭，算完成。
#[tokio::test]
async fn stop_with_clean_eof_is_complete() {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"正常收尾\"}}]}\r\n\r\n",
        // 尾帧无换行：状态机须与 [DONE] 路径一致（兼容旧供应方）
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}"
    );
    let (res, _) = run_stream(http_parts(body)).await;
    let outcome = res.expect("stop + 干净 EOF 必须完成");
    assert!(outcome.completed);
    assert_eq!(outcome.text, "正常收尾");
}

/// P2-3：stop 后收到 [DONE] 即权威终态——立即返回并释放 response，不等待
/// socket close。keep-alive 服务器保持连接 5s；用 2s 测试级超时判定及时返回
///（不用 sleep 计时）。
#[tokio::test]
async fn done_returns_without_waiting_for_socket_close() {
    let port = start_sse_server_hold(
        http_parts(concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"终态\"}}]}\r\n\r\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\r\n\r\n",
            "data: [DONE]\r\n\r\n"
        )),
        Duration::from_secs(5),
    );
    let client = app_lib::ingestion::build_client(30);
    let mut sink = |_: &str| true;
    let outcome = tokio::time::timeout(
        Duration::from_secs(2),
        ai::stream_chat(
            &client,
            &cfg_for(port),
            "sys",
            "user",
            &mut sink,
            ai::SUMMARY_MAX_TOKENS,
        ),
    )
    .await
    .expect("[DONE] 后必须立即返回，不得等待 socket 关闭")
    .expect("流应完成");
    assert!(outcome.completed);
    assert_eq!(outcome.text, "终态");
}

/// DONE 是权威终态：其后（含同一 chunk 尾随）的数据一律不再消费。
/// 尾随数据刻意放入「若被读取必然致命」的 length finish 与坏 JSON。
#[tokio::test]
async fn done_is_terminal_and_trailing_data_is_not_consumed() {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"正文\"}}]}\r\n\r\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\r\n\r\n",
        "data: [DONE]\r\n\r\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\r\n\r\n",
        "data: {oops not json}\r\n\r\n"
    );
    let (res, deltas) = run_stream(http_parts(body)).await;
    let outcome = res.expect("DONE 后应立即完成，尾随数据不得影响结果");
    assert!(outcome.completed);
    assert_eq!(outcome.text, "正文");
    assert_eq!(deltas, vec!["正文"], "尾随数据不得进入增量");
}

/// choices 为空的 usage 统计帧（含 finish 之后的 usage 帧）不误报、不打断完成。
#[tokio::test]
async fn empty_choices_usage_frames_are_benign() {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"正文\"}}]}\r\n\r\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\r\n\r\n",
        "data: \r\n\r\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\r\n\r\n",
        "data: {\"choices\":[],\"usage\":{\"completion_tokens\":2}}\r\n\r\n",
        "data: [DONE]\r\n\r\n"
    );
    let (res, _) = run_stream(http_parts(body)).await;
    let outcome = res.expect("usage 帧不得误报");
    assert!(outcome.completed);
    assert_eq!(outcome.text, "正文");
}

/// "error":null 忽略；error 对象是明确错误。
#[tokio::test]
async fn null_error_is_ignored_and_object_error_fails() {
    let ok_body = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}],\"error\":null}\r\n\r\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\r\n\r\n",
        "data: [DONE]\r\n\r\n"
    );
    let (res, _) = run_stream(http_parts(ok_body)).await;
    assert!(res.expect("error:null 不应失败").completed);

    let err_body = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"先来的正文\"}}]}\r\n\r\n",
        "data: {\"error\":{\"message\":\"rate limited\"}}\r\n\r\n"
    );
    let (res, _) = run_stream(http_parts(err_body)).await;
    let err = expect_err(res, "aiStream");
    assert!(err.message.contains("rate limited"), "{err}");
}

/// 非 JSON 的 data 行必须显式失败，不得静默丢弃后假完成。
#[tokio::test]
async fn non_json_data_line_is_error_not_silent() {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"好\"}}]}\r\n\r\n",
        "data: {oops not json}\r\n\r\n",
        "data: [DONE]\r\n\r\n"
    );
    let (res, _) = run_stream(http_parts(body)).await;
    let err = expect_err(res, "aiStream");
    assert!(err.message.contains("JSON"), "{err}");
}

/// 坏 UTF-8 字节必须显式失败，不得 lossy 替换后当成功。
#[tokio::test]
async fn invalid_utf8_bytes_are_rejected_not_lossy() {
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(b"data: {\"choices\":[{\"delta\":{\"content\":\"");
    body.extend_from_slice(&[0xFF, 0xFE]);
    body.extend_from_slice(b"\"}}]}\r\n\r\n");
    body.extend_from_slice(b"data: [DONE]\r\n\r\n");
    let (res, _) = run_stream(vec![HTTP_HEAD.to_vec(), body]).await;
    let err = expect_err(res, "aiStream");
    assert!(err.message.contains("UTF-8"), "{err}");
}

/// finish_reason=tool_calls → aiIncomplete（非文本完成原因）。
#[tokio::test]
async fn tool_calls_finish_is_incomplete() {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"id\":\"call_1\"}]}}]}\r\n\r\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\r\n\r\n",
        "data: [DONE]\r\n\r\n"
    );
    let (res, _) = run_stream(http_parts(body)).await;
    let err = expect_err(res, "aiIncomplete");
    assert!(err.message.contains("tool_calls"), "{err}");
}

/// 未知 finish_reason → aiIncomplete（不能当 stop）。
#[tokio::test]
async fn unknown_finish_reason_is_incomplete() {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"made_up\"}]}\r\n\r\n",
        "data: [DONE]\r\n\r\n"
    );
    let (res, _) = run_stream(http_parts(body)).await;
    let err = expect_err(res, "aiIncomplete");
    assert!(err.message.contains("made_up"), "{err}");
}

/// 重复 finish_reason（同值重复或冲突值）都拒绝。
#[tokio::test]
async fn duplicate_or_conflicting_finish_is_rejected() {
    let dup = concat!(
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\r\n\r\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\r\n\r\n"
    );
    let (res, _) = run_stream(http_parts(dup)).await;
    let err = expect_err(res, "aiStream");
    assert!(err.message.contains("重复"), "{err}");

    let conflict = concat!(
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\r\n\r\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\r\n\r\n"
    );
    let (res, _) = run_stream(http_parts(conflict)).await;
    let err = expect_err(res, "aiStream");
    assert!(err.message.contains("重复"), "{err}");
}

/// finish_reason 之后的文本增量拒绝越界拼接（DONE 之前的语义事件）。
#[tokio::test]
async fn content_after_finish_is_rejected() {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"已收正文\"}}]}\r\n\r\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\r\n\r\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"不该拼接\"}}]}\r\n\r\n"
    );
    let (res, deltas) = run_stream(http_parts(body)).await;
    let err = expect_err(res, "aiStream");
    assert!(err.message.contains("拼接"), "{err}");
    assert_eq!(deltas, vec!["已收正文"]);
}

/// 消费方关闭 sink（前端取消）→ Ok(completed=false)，不缓存、不算错误。
#[tokio::test]
async fn sink_close_cancels_without_error() {
    let port = start_sse_server(http_parts(concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"第一\"}}]}\r\n\r\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"第二\"}}]}\r\n\r\n",
        "data: [DONE]\r\n\r\n"
    )));
    let client = app_lib::ingestion::build_client(30);
    let mut seen = 0;
    let mut sink = |d: &str| {
        seen += 1;
        d == "第一" // 第二段返回 false：消费方已关闭
    };
    let outcome = ai::stream_chat(
        &client,
        &cfg_for(port),
        "sys",
        "user",
        &mut sink,
        ai::SUMMARY_MAX_TOKENS,
    )
    .await
    .expect("消费方取消不是错误");
    assert!(
        !outcome.completed,
        "sink 关闭 → completed=false（取消，不缓存）"
    );
    assert_eq!(outcome.text, "第一第二", "已推送的增量保留在 outcome 里");
    assert_eq!(seen, 2, "第二段推送后立即停止，不再继续读流");
}

/// 累计文本超过有限预算 → 显式失败（不静默截断、不假装完成）。
#[tokio::test]
async fn output_text_over_budget_fails_explicitly() {
    let chunk = "a".repeat(64 * 1024);
    let frame =
        format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{chunk}\"}}}}]}}\r\n\r\n");
    let mut body = String::new();
    for _ in 0..5 {
        body.push_str(&frame); // 5 × 64 KiB = 320 KiB > 256 KiB 预算
    }
    body.push_str("data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\r\n\r\n");
    let (res, _) = run_stream(http_parts(&body)).await;
    let err = expect_err(res, "aiStream");
    assert!(err.message.contains("预算"), "{err}");
}

/// 单行（无换行）超过行缓冲上限 → 显式失败（防非 SSE/恶意端点拖爆内存）。
#[tokio::test]
async fn oversized_sse_line_over_line_budget_fails() {
    let huge = "a".repeat(9 * 1024 * 1024);
    let body = format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{huge}\"}}}}]}}");
    let (res, _) = run_stream(vec![HTTP_HEAD.to_vec(), body.into_bytes()]).await;
    let err = expect_err(res, "aiStream");
    assert!(err.message.contains("上限"), "{err}");
}

/// P2-2：**完整**（带换行）的超大注释行也必须在复制/解析前被行缓冲预算拒绝；
/// text 预算只管正文，替代不了这里的框架预算。
#[tokio::test]
async fn oversized_comment_line_over_line_budget_fails() {
    let huge = format!(":{}", "a".repeat(9 * 1024 * 1024));
    let body = format!("{huge}\r\ndata: [DONE]\r\n\r\n");
    let (res, _) = run_stream(http_parts(&body)).await;
    let err = expect_err(res, "aiStream");
    assert!(err.message.contains("上限"), "{err}");
}

/// R2-P2-1：纯空白的“未知字段”行不是事件分隔符——标准 SSE 只有去掉行结束符
/// 后长度为 0 的空行才派发。多行 data JSON 中间插入空格行不得提前派发半截 JSON。
#[tokio::test]
async fn space_only_line_is_not_an_event_delimiter() {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":\r\n",
        "   \r\n",
        "data: {\"content\":\"空格行\"}}]}\r\n",
        "\r\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\r\n\r\n",
        "data: [DONE]\r\n\r\n"
    );
    let (res, _) = run_stream(http_parts(body)).await;
    let outcome = res.expect("空格行不得被当作事件分隔符");
    assert!(outcome.completed);
    assert_eq!(outcome.text, "空格行");
}

/// R2-P2-2：无冒号 `data` 行 = 空值 data（规范），参与多行事件合并（其 join LF
/// 也是载荷字节）。
#[tokio::test]
async fn bare_data_line_is_empty_value_in_multiline_event() {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":\r\n",
        "data\r\n",
        "data: {\"content\":\"空值行\"}}]}\r\n",
        "\r\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\r\n\r\n",
        "data: [DONE]\r\n\r\n"
    );
    let (res, _) = run_stream(http_parts(body)).await;
    let outcome = res.expect("无冒号 data 行应为空值 data 而非协议错");
    assert!(outcome.completed);
    assert_eq!(outcome.text, "空值行");
}

/// R2-P2-2 边界：CRLF 多行事件的实际 data 载荷（值字节 + join LF，不含 EOL）
/// 恰好 8 MiB 时合法——由正文预算（256 KiB）接手拦截；真载荷 +1 字节才报
/// 行缓冲上限。旧实现的 EOL 残留会把恰好 8 MiB 误算超限。
#[tokio::test]
async fn crlf_exact_event_payload_budget_boundary() {
    const LIMIT: usize = 8 * 1024 * 1024;
    let p1 = "{\"choices\":";
    let p2_head = "[{\"delta\":{\"content\":\"";
    let p2_tail = "\"}}]}";
    // 联接载荷 = p1 + join LF + p2_head + a*n + p2_tail = 恰好 LIMIT
    let n = LIMIT - p1.len() - 1 - p2_head.len() - p2_tail.len();
    let exact = format!(
        "data: {p1}\r\ndata: {p2_head}{a}{p2_tail}\r\n\r\n",
        a = "a".repeat(n)
    );
    let (res, _) = run_stream(http_parts(&exact)).await;
    let err = expect_err(res, "aiStream");
    assert!(
        err.message.contains("本地预算上限"),
        "恰好 8 MiB 的 data 载荷必须通过事件预算、由正文预算拦截: {err}"
    );

    // +1 字节：真载荷越界 → 行缓冲上限拒绝
    let over = format!(
        "data: {p1}\r\ndata: {p2_head}{a}{p2_tail}\r\n\r\n",
        a = "a".repeat(n + 1)
    );
    let (res, _) = run_stream(http_parts(&over)).await;
    let err = expect_err(res, "aiStream");
    assert!(err.message.contains("行缓冲上限"), "{err}");
}

/// R2-P2-2 边界：单行预算按缓冲中的物理字节（含 CRLF）计——恰好 8 MiB 的注释
/// 行合法（其后的流仍能正常完成），+1 字节显式失败。
#[tokio::test]
async fn crlf_comment_line_exact_line_budget_boundary() {
    const LIMIT: usize = 8 * 1024 * 1024;
    // 注释行 = ":" + (LIMIT-3) + CRLF = 恰好 LIMIT 字节
    let ok_comment = format!(":{}", "a".repeat(LIMIT - 3));
    let ok_body = format!(
        "{ok_comment}\r\n\
         data: {{\"choices\":[{{\"delta\":{{\"content\":\"边界\"}}}}]}}\r\n\r\n\
         data: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"stop\"}}]}}\r\n\r\n\
         data: [DONE]\r\n\r\n"
    );
    let (res, _) = run_stream(http_parts(&ok_body)).await;
    let outcome = res.expect("恰好 8 MiB 的单行必须合法");
    assert!(outcome.completed);
    assert_eq!(outcome.text, "边界");

    // +1 字节 → 行缓冲上限
    let bad_comment = format!(":{}", "a".repeat(LIMIT - 2));
    let bad_body = format!("{bad_comment}\r\ndata: [DONE]\r\n\r\n");
    let (res, _) = run_stream(http_parts(&bad_body)).await;
    let err = expect_err(res, "aiStream");
    assert!(err.message.contains("上限"), "{err}");
}
