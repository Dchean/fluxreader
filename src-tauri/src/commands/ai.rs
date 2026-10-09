//! commands 的 ai 领域子模块（TASK-044 从 commands.rs 按既有章节拆分，纯搬运）。

use crate::db;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use rusqlite::Connection;
use tauri::State;

/* ============================================================
AI 引擎（OpenAI 兼容：官方 / DeepSeek / GLM / newapi 中转）
============================================================ */

/// 推给前端的流式事件（camelCase）。
/// TASK-070（REQ-104）：原 `Error(String)` 变体在生产从不构造（失败改由
/// invoke rejection + 前端 .catch 传达），属未落实的宣称能力，已删除。
#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase", tag = "type", content = "data")]
pub enum AiEvent {
    Delta(String),
    Done,
}

/// 读 ai_config JSON；未配置时报 aiNotConfigured。
///
/// P3[2]（REQ-104）：此前用 `.ok().flatten()` 把**读库失败**与「没配置过」压成同一个
/// None，于是数据库读错误会被报成「请先在设置中配置 AI 服务」——用户按提示去填配置，
/// 却怎么都修不好，属误导性错误。现在把两者区分开：读失败原样上抛（可诊断），
/// 只有确实没配置时才提示去配置。
async fn load_ai_config(state: &AppState) -> AppResult<crate::ai::AiConfig> {
    let raw = {
        let conn = state.db.lock().await;
        match db::get_setting(&conn, "ai_config") {
            Ok(value) => value,
            Err(err) => {
                log::warn!("ai: 读取 ai_config 失败: {err}");
                return Err(err);
            }
        }
    };
    match raw {
        Some(json) => crate::ai::AiConfig::from_json(&json),
        None => Err(AppError::new("aiNotConfigured", "请先在设置中配置 AI 服务")),
    }
}

/// 保存 AI 配置（前端 AI tab 表单）。value 为整份 JSON。
#[tauri::command]
pub async fn save_ai_config(state: State<'_, AppState>, value: String) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::set_setting(&conn, "ai_config", &value)
}

/// 读 AI 配置（前端启动时恢复表单）。
///
/// P3[2]：读失败不再静默降级成「无配置」——那会让设置页显示成空表单，用户以为
/// 配置丢了而重新填写。读失败上报并留 warn。
#[tauri::command]
pub async fn get_ai_config(state: State<'_, AppState>) -> AppResult<Option<String>> {
    let conn = state.db.lock().await;
    match db::get_setting(&conn, "ai_config") {
        Ok(value) => Ok(value),
        Err(err) => {
            log::warn!("ai: 读取 ai_config 失败: {err}");
            Err(err)
        }
    }
}

/// 连通性测试 + 拉模型列表（官方与 newapi 都支持 /models）。
#[tauri::command]
pub async fn ai_list_models(
    state: State<'_, AppState>,
    base_url: String,
    api_key: String,
) -> AppResult<Vec<String>> {
    let cfg = crate::ai::AiConfig {
        api_key: api_key.trim().to_string(),
        model: String::new(),
        base_url: base_url.trim().trim_end_matches('/').to_string(),
    };
    crate::ai::list_models(&state.http, &cfg).await
}

/// 提示词默认文案（用户未自定义时）。设计文档承诺的默认行为。
const DEFAULT_SUMMARIZE_SYSTEM: &str = "你是一名资讯编辑。请用简洁的中文总结这篇文章，输出 3-5 个要点，每个要点一行，以 - 开头。不要重复文章标题。";
const DEFAULT_TRANSLATE_SYSTEM: &str = "你是一名专业译者。请把用户提供的 HTML 片段翻译成简体中文：保留所有 HTML 标签和属性原样不动，只翻译标签内的文本内容。直接输出翻译后的 HTML，不要任何解释或代码块包裹。";

/// 读 ai_config JSON 里用户自定义的提示词；未配置用默认。
///
/// P3[2]：提示词缺失时回落内置默认是**设计行为**（用户没自定义就该用默认），
/// 但「读库失败」不该与「没配置」混为一谈——前者会静默用默认提示词覆盖用户的
/// 自定义感受。故读失败时留 warn 以便诊断，行为仍回落默认（不阻塞摘要/翻译）。
async fn load_prompts(state: &AppState) -> (String, String) {
    let raw = {
        let conn = state.db.lock().await;
        match db::get_setting(&conn, "ai_config") {
            Ok(value) => value,
            Err(err) => {
                log::warn!("ai: 读取 ai_config 失败，本次使用内置默认提示词: {err}");
                None
            }
        }
    };
    match raw
        .as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
    {
        Some(v) => {
            let summary = v["summaryPrompt"]
                .as_str()
                .unwrap_or(DEFAULT_SUMMARIZE_SYSTEM)
                .to_string();
            let translate = v["translatePrompt"]
                .as_str()
                .unwrap_or(DEFAULT_TRANSLATE_SYSTEM)
                .to_string();
            (summary, translate)
        }
        None => (
            DEFAULT_SUMMARIZE_SYSTEM.to_string(),
            DEFAULT_TRANSLATE_SYSTEM.to_string(),
        ),
    }
}

/// 流式摘要：读文章 → 已有缓存直接返回 → 否则调 AI 流式生成 → 落库。
/// on_channel 前端增量渲染；返回最终全文（前端也用于缓存判断）。
#[tauri::command]
pub async fn ai_summarize(
    state: State<'_, AppState>,
    article_id: i64,
    on_channel: tauri::ipc::Channel<AiEvent>,
) -> AppResult<String> {
    ai_summarize_inner(&state, article_id, &on_channel).await
}

/// ai_summarize 的实现体：与 State/命令宏解耦，供模块测试直接注入 AppState
/// 与 Channel（验证「缓存提交失败不发 Done 假成功」）。
async fn ai_summarize_inner(
    state: &AppState,
    article_id: i64,
    on_channel: &tauri::ipc::Channel<AiEvent>,
) -> AppResult<String> {
    let cfg = load_ai_config(state).await?;

    // 取文章内容（锁内快照，锁外跑网络）
    let (title, body, cached) = {
        let conn = state.db.lock().await;
        db::get_article_for_summary(&conn, article_id)?
            .ok_or_else(|| AppError::not_found(format!("article {article_id}")))?
    };

    // 缓存命中：直接推给前端，不重算
    if let Some(summary) = cached.filter(|s| !s.trim().is_empty()) {
        let _ = on_channel.send(AiEvent::Delta(summary.clone()));
        let _ = on_channel.send(AiEvent::Done);
        return Ok(summary);
    }

    // P2-11（TASK-076，DEC-req104-p2-11-ai-validation-20260920）：空正文显式报错，
    // 与 ai_translate 的既有校验（"文章无正文可翻译"）对称。此前这里没有检查，
    // 空正文会照样发一次模型请求：白花 token，且 outcome.text 为空时不落缓存、
    // 用户只看到「摘要没出来」，无从判断是内容问题还是服务问题。
    if body.trim().is_empty() {
        return Err(AppError::not_found("文章无正文可摘要"));
    }

    let user = format!("标题：{title}\n\n正文：\n{body}");
    let (system, _) = load_prompts(state).await;
    let mut sink = |delta: &str| {
        // 前端关闭面板（Channel 被 drop）时 send 返回 Err → 返回 false 让
        // stream_chat 提前终止，不浪费 token、不落残缺产物（A-5）。
        on_channel.send(AiEvent::Delta(delta.to_string())).is_ok()
    };
    let outcome = crate::ai::stream_chat(
        &state.http,
        &cfg,
        &system,
        &user,
        &mut sink,
        crate::ai::SUMMARY_MAX_TOKENS,
    )
    .await?;

    if outcome.completed {
        if !outcome.text.trim().is_empty() {
            // OPT-013A：保存失败必须 Err——不得吞错后发送 Done 假成功（此前
            // `let _ =` 即此病）。前端经 invoke rejection 显示失败并可重试；
            // 缓存未写，重试不会被缓存短路。
            commit_summary_cache(&state.db, article_id, &outcome.text).await?;
        }
        let _ = on_channel.send(AiEvent::Done);
    }
    // completed=false（消费方取消）：不发 Done、不写缓存，返回已渲染文本。
    Ok(outcome.text)
}

/// 完整摘要落缓存。保存失败必须原样上抛——调用方不得吞错后发送 Done 假成功。
/// （决定与备选见 .agents/notes/implemented/architecture/2026-10-08-AI完成信号与缓存提交.md）
async fn commit_summary_cache(
    db_lock: &tokio::sync::Mutex<Connection>,
    article_id: i64,
    text: &str,
) -> AppResult<()> {
    let conn = db_lock.lock().await;
    db::set_article_ai_fields(&conn, article_id, Some(text), None)
}

/// 完整译文落缓存（先过消毒器：模型输出不可信）；保存失败必须原样上抛。
async fn commit_translation_cache(
    db_lock: &tokio::sync::Mutex<Connection>,
    article_id: i64,
    text: &str,
) -> AppResult<()> {
    // 翻译产物是 HTML 且直接 dangerouslySetInnerHTML 渲染——入库前
    // 过消毒器（模型输出不可信：可能带 <script> 或被提示注入）。
    // 流式 delta 已发给前端（流中预览，未消毒）；落库的是消毒版。
    // 前端在流结束后回读 get_article 拿消毒版覆盖流式预览（见 store.ts
    // toggleReaderTranslation 的 onDone 回读逻辑）。
    let safe = crate::sanitize::sanitize(text, None);
    let conn = db_lock.lock().await;
    db::set_article_ai_fields(&conn, article_id, None, Some(&safe))
}

/// 流式翻译：同 ai_summarize 结构，产物写 translated_content。
#[tauri::command]
pub async fn ai_translate(
    state: State<'_, AppState>,
    article_id: i64,
    on_channel: tauri::ipc::Channel<AiEvent>,
) -> AppResult<String> {
    ai_translate_inner(&state, article_id, &on_channel).await
}

/// ai_translate 的实现体（与 State/命令宏解耦，供模块测试注入）。
async fn ai_translate_inner(
    state: &AppState,
    article_id: i64,
    on_channel: &tauri::ipc::Channel<AiEvent>,
) -> AppResult<String> {
    let cfg = load_ai_config(state).await?;

    let (title, html, cached) = {
        let conn = state.db.lock().await;
        db::get_article_for_translation(&conn, article_id)?
            .ok_or_else(|| AppError::not_found(format!("article {article_id}")))?
    };

    if let Some(translated) = cached.filter(|s| !s.trim().is_empty()) {
        let _ = on_channel.send(AiEvent::Delta(translated.clone()));
        let _ = on_channel.send(AiEvent::Done);
        return Ok(translated);
    }

    if html.trim().is_empty() {
        return Err(AppError::not_found("文章无正文可翻译"));
    }

    let user = format!("标题：{title}\n\nHTML：\n{html}");
    let (_, system) = load_prompts(state).await;
    let mut sink = |delta: &str| {
        // 前端关闭面板（Channel 被 drop）时 send 返回 Err → 返回 false 让
        // stream_chat 提前终止，不浪费 token、不落残缺产物（A-5）。
        on_channel.send(AiEvent::Delta(delta.to_string())).is_ok()
    };
    let outcome = crate::ai::stream_chat(
        &state.http,
        &cfg,
        &system,
        &user,
        &mut sink,
        crate::ai::TRANSLATE_MAX_TOKENS,
    )
    .await?;

    if outcome.completed {
        if !outcome.text.trim().is_empty() {
            // OPT-013A：同 ai_summarize——保存失败必须 Err，不得 Done 假成功。
            commit_translation_cache(&state.db, article_id, &outcome.text).await?;
        }
        let _ = on_channel.send(AiEvent::Done);
    }
    // completed=false（消费方取消）：不发 Done、不写缓存。
    Ok(outcome.text)
}

#[cfg(test)]
mod ai_event_tests {
    use super::AiEvent;

    /// 前端 api.ts 按 {type, data} 解析；序列化必须严格一致（camelCase tag）。
    /// TASK-070：原断言还覆盖 `AiEvent::Error`——该变体在生产从不构造
    /// （失败经 invoke rejection + 前端 .catch 传达），属空壳，已随 REQ-104 删除。
    #[test]
    fn ai_event_serialization_matches_frontend() {
        let delta = serde_json::to_value(AiEvent::Delta("你好".into())).unwrap();
        assert_eq!(delta["type"], "delta");
        assert_eq!(delta["data"], "你好");
        let done = serde_json::to_value(AiEvent::Done).unwrap();
        assert_eq!(done["type"], "done");
    }
}

/// OPT-013A 命令层提交语义测试（真实 loopback mock + DB 触发器故障注入）。
///
/// 关键性质：缓存保存失败必须让命令返回 Err，且**不得**再发送 Done——
/// 此前实现用 `let _ =` 吞掉保存错误后照发 Done，前端看到「成功」而缓存未写，
/// 属假成功。测试经真实 Channel 观测事件流（不是只看 ChatOutcome 是否存在）。
/// 决定与备选见 .agents/notes/implemented/architecture/2026-10-08-AI完成信号与缓存提交.md
#[cfg(test)]
mod ai_commit_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex as StdMutex};
    use tauri::ipc::{Channel, InvokeResponseBody};

    /// 记录事件类型的测试 Channel（事件按前端契约 {type, data} 序列化）。
    fn recording_channel(events: Arc<StdMutex<Vec<String>>>) -> Channel<AiEvent> {
        Channel::new(move |body: InvokeResponseBody| {
            let v: serde_json::Value = body
                .deserialize()
                .unwrap_or_else(|_| serde_json::json!({"type": "unparseable"}));
            events
                .lock()
                .unwrap()
                .push(v["type"].as_str().unwrap_or("?").to_string());
            Ok(())
        })
    }

    /// 一次性 loopback SSE 服务器（与 tests/ai_stream_completion_e2e.rs 同构）。
    fn start_mock_sse(body: &'static str) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let Some(Ok(mut stream)) = listener.incoming().next() else {
                return;
            };
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
            let head =
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
            if stream.write_all(head).is_err() {
                return;
            }
            let _ = stream.write_all(body.as_bytes());
        });
        port
    }

    /// 计数版 mock：任何入站连接都会让计数 +1（缓存命中测试要求为 0）。
    /// 若实现短路失效而真的发起请求，会得到 500 → Err，测试双重失败。
    fn start_counting_sse(counter: Arc<StdMutex<usize>>) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let Some(Ok(mut stream)) = listener.incoming().next() else {
                return;
            };
            *counter.lock().unwrap() += 1;
            let _ = stream
                .write_all(b"HTTP/1.1 500 Internal Server Error\r\nConnection: close\r\n\r\n");
        });
        port
    }

    /// 建内存库（settings + articles 最小 schema）并构造 AppState；
    /// `extra_sql` 用于注入触发器等故障。返回共享的事件类型表。
    fn setup(port: u16, extra_sql: &str) -> (AppState, Arc<StdMutex<Vec<String>>>) {
        let conn = Connection::open_in_memory().unwrap();
        let cfg = format!(
            r#"{{"preset":"custom","baseUrl":"http://127.0.0.1:{port}","apiKey":"sk-test","model":"m1"}}"#
        );
        conn.execute_batch(&format!(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE articles (id INTEGER PRIMARY KEY, title TEXT NOT NULL, body_text TEXT,
                 content_html TEXT, ai_summary TEXT, translated_content TEXT);
             INSERT INTO articles (id, title, body_text, content_html)
                 VALUES (1, '测试文章', '这是正文内容。', '<p>Hello World</p>');
             INSERT INTO settings (key, value) VALUES ('ai_config', '{cfg}');
             {extra_sql}"
        ))
        .unwrap();
        let state = AppState::new(
            conn,
            crate::ingestion::build_client(30),
            crate::media::MediaHandle::inactive(),
        );
        (state, Arc::new(StdMutex::new(Vec::new())))
    }

    fn query_summary(conn: &Connection) -> Option<String> {
        conn.query_row("SELECT ai_summary FROM articles WHERE id = 1", [], |r| {
            r.get(0)
        })
        .unwrap()
    }

    const COMPLETE_SSE: &str = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"要点一\"}}]}\r\n\r\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\r\n\r\n",
        "data: [DONE]\r\n\r\n"
    );

    /// 基线：完整流 + 无注入故障 → 落缓存、发 Done（保证测试夹具本身有效）。
    #[tokio::test]
    async fn complete_summary_caches_and_sends_done() {
        let port = start_mock_sse(COMPLETE_SSE);
        let (state, events) = setup(port, "");
        let channel = recording_channel(events.clone());
        let text = ai_summarize_inner(&state, 1, &channel).await.unwrap();
        assert_eq!(text, "要点一");
        assert_eq!(
            events.lock().unwrap().clone(),
            vec!["delta", "done"],
            "完整路径先增量后 Done"
        );
        let conn = state.db.lock().await;
        assert_eq!(query_summary(&conn).as_deref(), Some("要点一"));
    }

    /// 核心反例：缓存保存失败 → Err，且不得发送 Done（不假成功），缓存未写。
    #[tokio::test]
    async fn summary_cache_failure_is_err_without_done() {
        let port = start_mock_sse(COMPLETE_SSE);
        let (state, events) = setup(
            port,
            "CREATE TRIGGER fail_summary BEFORE UPDATE OF ai_summary ON articles
             BEGIN SELECT RAISE(ABORT, 'injected cache failure'); END;",
        );
        let channel = recording_channel(events.clone());
        let err = ai_summarize_inner(&state, 1, &channel)
            .await
            .expect_err("保存失败必须 Err");
        assert_eq!(err.code, "db", "{err}");
        let types = events.lock().unwrap().clone();
        assert_eq!(types, vec!["delta"], "保存失败不得发送 Done: {types:?}");
        let conn = state.db.lock().await;
        assert_eq!(query_summary(&conn), None, "失败不得半写缓存");
    }

    /// 不完整流（无 finish 的干净 EOF）→ Err aiIncomplete，不发 Done、不写缓存。
    #[tokio::test]
    async fn incomplete_stream_is_err_without_cache_or_done() {
        let port =
            start_mock_sse("data: {\"choices\":[{\"delta\":{\"content\":\"半截\"}}]}\r\n\r\n");
        let (state, events) = setup(port, "");
        let channel = recording_channel(events.clone());
        let err = ai_summarize_inner(&state, 1, &channel)
            .await
            .expect_err("不完整流必须 Err");
        assert_eq!(err.code, "aiIncomplete", "{err}");
        let types = events.lock().unwrap().clone();
        assert_eq!(types, vec!["delta"], "不完整不得发送 Done: {types:?}");
        let conn = state.db.lock().await;
        assert_eq!(query_summary(&conn), None);
    }

    /// 翻译路径同样：保存失败 → Err，不发 Done，消毒后产物未落库。
    #[tokio::test]
    async fn translation_cache_failure_is_err_without_done() {
        let port = start_mock_sse(concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"<p>你好</p>\"}}]}\r\n\r\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\r\n\r\n",
            "data: [DONE]\r\n\r\n"
        ));
        let (state, events) = setup(
            port,
            "CREATE TRIGGER fail_translation BEFORE UPDATE OF translated_content ON articles
             BEGIN SELECT RAISE(ABORT, 'injected cache failure'); END;",
        );
        let channel = recording_channel(events.clone());
        let err = ai_translate_inner(&state, 1, &channel)
            .await
            .expect_err("保存失败必须 Err");
        assert_eq!(err.code, "db", "{err}");
        let types = events.lock().unwrap().clone();
        assert_eq!(types, vec!["delta"], "保存失败不得发送 Done: {types:?}");
        let conn = state.db.lock().await;
        let translated: Option<String> = conn
            .query_row(
                "SELECT translated_content FROM articles WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(translated, None);
    }

    /// 缓存命中完整已有结果：delta(缓存)+Done，且完全不发起网络请求。
    #[tokio::test]
    async fn cache_hit_sends_delta_and_done_without_network() {
        let counter = Arc::new(StdMutex::new(0usize));
        let port = start_counting_sse(counter.clone());
        let (state, events) = setup(
            port,
            "UPDATE articles SET ai_summary = '已缓存的摘要' WHERE id = 1;",
        );
        let channel = recording_channel(events.clone());
        let text = ai_summarize_inner(&state, 1, &channel).await.unwrap();
        assert_eq!(text, "已缓存的摘要");
        assert_eq!(
            events.lock().unwrap().clone(),
            vec!["delta", "done"],
            "缓存命中路径：delta + Done"
        );
        assert_eq!(*counter.lock().unwrap(), 0, "缓存命中不得发起网络请求");
    }

    /// 消费方取消（completed=false）→ 命令层不再发送 Done（不是依赖通道已关闭），
    /// 不写缓存，返回已渲染文本且不算错误。
    #[tokio::test]
    async fn cancel_does_not_send_done_and_writes_no_cache() {
        let port = start_mock_sse(COMPLETE_SSE);
        let (state, events) = setup(port, "");
        let channel: Channel<AiEvent> = Channel::new({
            let events = events.clone();
            move |body: InvokeResponseBody| {
                let v: serde_json::Value = body
                    .deserialize()
                    .unwrap_or_else(|_| serde_json::json!({"type": "unparseable"}));
                let ty = v["type"].as_str().unwrap_or("?").to_string();
                events.lock().unwrap().push(ty.clone());
                if ty == "delta" {
                    // 模拟 Channel 已关闭：send 返回 Err → sink false → 取消
                    return Err(tauri::Error::Io(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "channel closed",
                    )));
                }
                Ok(())
            }
        });
        let text = ai_summarize_inner(&state, 1, &channel)
            .await
            .expect("取消不是错误");
        assert_eq!(text, "要点一", "已渲染文本保留在返回值里");
        let types = events.lock().unwrap().clone();
        assert_eq!(types, vec!["delta"], "取消后不得发送 Done: {types:?}");
        let conn = state.db.lock().await;
        assert_eq!(query_summary(&conn), None, "取消不得写缓存");
    }
}
