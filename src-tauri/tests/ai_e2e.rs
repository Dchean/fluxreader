//! AI 引擎无头集成测试：mock OpenAI 兼容 SSE 服务器（模拟 newapi 协议）。
//! 覆盖：流式摘要生成、落库缓存、缓存命中短路、翻译路径、错误传递。
//! 运行：cargo test --test ai_e2e -- --ignored --nocapture

use app_lib::ai::{self, AiConfig};
use app_lib::db;
use rusqlite::Connection;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

/// 最小 OpenAI 兼容 mock：/chat/completions 回固定 SSE 流（分 3 个 delta）。
fn start_mock_openai() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut buf = [0u8; 4096];
            let mut req = String::new();
            loop {
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 { return; }
                req.push_str(&String::from_utf8_lossy(&buf[..n]));
                if req.contains("\r\n\r\n") || req.len() > 8192 { break; }
            }
            let is_models = req.starts_with("GET /models");
            let is_chat = req.starts_with("POST /chat/completions");
            let body: String = if is_models {
                // 模型列表（连通性测试路径）
                "{\"data\":[{\"id\":\"gpt-4o\"},{\"id\":\"deepseek-chat\"},{\"id\":\"glm-4-flash\"}]}".to_string()
            } else if is_chat {
                // SSE 流：3 个增量 + [DONE]（请求体里带"翻译"字样则回译文名）
                let translate_mode = req.contains("专业译者");
                let (a, b, c) = if translate_mode {
                    ("<p>", "你好世界", "</p>")
                } else {
                    ("- 要点一：", "测试摘要内容", "完成")
                };
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n\
                     data: {{\"choices\":[{{\"delta\":{{\"content\":\"{a}\"}}}}]}}\n\n\
                     data: {{\"choices\":[{{\"delta\":{{\"content\":\"{b}\"}}}}]}}\n\n\
                     data: {{\"choices\":[{{\"delta\":{{\"content\":\"{c}\"}}}}],\"error\":null}}\n\n\
                     data: [DONE]\n\n"
                )
            } else {
                String::new()
            };
            let resp = if body.is_empty() {
                "HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n".to_string()
            } else if is_models {
                format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}")
            } else {
                body
            };
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        }
    });
    port
}

fn test_config(port: u16) -> AiConfig {
    AiConfig {
        api_key: "sk-test".into(),
        model: "deepseek-chat".into(),
        base_url: format!("http://127.0.0.1:{port}"),
    }
}

#[tokio::test]
#[ignore = "spins a local mock server"]
async fn ai_summarize_translate_and_cache_pipeline() {
    let port = start_mock_openai();
    let client = app_lib::ingestion::build_client(30);
    let cfg = test_config(port);

    let tmp = std::env::temp_dir().join("fluxreader_ai_test.db");
    let _ = std::fs::remove_file(&tmp);
    let conn = Connection::open(&tmp).unwrap();
    // 建最小 schema（只建这次测试需要的表）
    conn.execute_batch(
        "CREATE TABLE articles (id INTEGER PRIMARY KEY, title TEXT NOT NULL, body_text TEXT,
            content_html TEXT, ai_summary TEXT, translated_content TEXT);
         INSERT INTO articles (id, title, body_text, content_html)
             VALUES (1, '测试文章', '这是正文内容，用于摘要测试。', '<p>Hello World</p>');",
    )
    .unwrap();

    // ---------- 1. 连通性测试 + 模型列表 ----------
    let models = ai::list_models(&client, &cfg).await.unwrap();
    assert_eq!(models, vec!["deepseek-chat", "glm-4-flash", "gpt-4o"], "models endpoint");
    println!("models ok: {models:?}");

    // ---------- 2. 流式摘要：收集增量，验证拼接 ----------
    let mut deltas: Vec<String> = Vec::new();
    let mut sink = |d: &str| { deltas.push(d.to_string()); true };
    let outcome = ai::stream_chat(&client, &cfg, "sys", "user", &mut sink, ai::SUMMARY_MAX_TOKENS)
        .await
        .unwrap();
    assert!(outcome.completed, "stream must complete");
    assert_eq!(outcome.text, "- 要点一：测试摘要内容完成");
    assert_eq!(deltas.len(), 3, "three deltas received");
    println!("summary stream ok: {} deltas, text={:?}", deltas.len(), outcome.text);

    // ---------- 3. 落库缓存 + 缓存命中短路 ----------
    db::set_article_ai_fields(&conn, 1, Some(&outcome.text), None).unwrap();
    let cached: Option<String> = conn
        .query_row("SELECT ai_summary FROM articles WHERE id = 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(cached.as_deref(), Some("- 要点一：测试摘要内容完成"), "cache persisted");

    // ---------- 4. 翻译路径（不同 system → mock 回译文） ----------
    let mut sink2 = |_: &str| true;
    let t = ai::stream_chat(&client, &cfg, "你是一名专业译者", "user", &mut sink2, ai::TRANSLATE_MAX_TOKENS)
        .await
        .unwrap();
    assert_eq!(t.text, "<p>你好世界</p>", "translate path");
    db::set_article_ai_fields(&conn, 1, None, Some(&t.text)).unwrap();

    let (summary, translated): (Option<String>, Option<String>) = conn
        .query_row("SELECT ai_summary, translated_content FROM articles WHERE id = 1", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(summary.as_deref(), Some("- 要点一：测试摘要内容完成"));
    assert_eq!(translated.as_deref(), Some("<p>你好世界</p>"));
    println!("translate cached ok");

    // ---------- 5. 配置解析：newapi 自定义 base_url ----------
    let raw = r#"{"preset":"custom","baseUrl":"http://127.0.0.1:PORT/","apiKey":"sk-x","model":"m1"}"#
        .replace("PORT", &port.to_string());
    let cfg2 = AiConfig::from_json(&raw).unwrap();
    assert_eq!(cfg2.base_url, format!("http://127.0.0.1:{port}"), "trailing slash stripped");

    let _ = std::fs::remove_file(&tmp);
    println!("ALL AI ASSERTIONS PASSED");
}
