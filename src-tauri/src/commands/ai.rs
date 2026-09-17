//! commands 的 ai 领域子模块（TASK-044 从 commands.rs 按既有章节拆分，纯搬运）。

use crate::db;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use tauri::State;

/* ============================================================
AI 引擎（OpenAI 兼容：官方 / DeepSeek / GLM / newapi 中转）
============================================================ */

/// 推给前端的流式事件（camelCase）。
#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase", tag = "type", content = "data")]
pub enum AiEvent {
    Delta(String),
    Done,
    Error(String),
}

/// 读 ai_config JSON；未配置时报 aiNotConfigured。
async fn load_ai_config(state: &State<'_, AppState>) -> AppResult<crate::ai::AiConfig> {
    let raw = {
        let conn = state.db.lock().await;
        db::get_setting(&conn, "ai_config").ok().flatten()
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
#[tauri::command]
pub async fn get_ai_config(state: State<'_, AppState>) -> AppResult<Option<String>> {
    let conn = state.db.lock().await;
    Ok(db::get_setting(&conn, "ai_config").ok().flatten())
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
async fn load_prompts(state: &State<'_, AppState>) -> (String, String) {
    let raw = {
        let conn = state.db.lock().await;
        db::get_setting(&conn, "ai_config").ok().flatten()
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
    let cfg = load_ai_config(&state).await?;

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

    let user = format!("标题：{title}\n\n正文：\n{body}");
    let (system, _) = load_prompts(&state).await;
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

    if outcome.completed && !outcome.text.trim().is_empty() {
        // 落库缓存（失败不影响返回）
        let conn = state.db.lock().await;
        let _ = db::set_article_ai_fields(&conn, article_id, Some(&outcome.text), None);
    }
    let _ = on_channel.send(AiEvent::Done);
    Ok(outcome.text)
}

/// 流式翻译：同 ai_summarize 结构，产物写 translated_content。
#[tauri::command]
pub async fn ai_translate(
    state: State<'_, AppState>,
    article_id: i64,
    on_channel: tauri::ipc::Channel<AiEvent>,
) -> AppResult<String> {
    let cfg = load_ai_config(&state).await?;

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
    let (_, system) = load_prompts(&state).await;
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

    if outcome.completed && !outcome.text.trim().is_empty() {
        // 翻译产物是 HTML 且直接 dangerouslySetInnerHTML 渲染——入库前
        // 过消毒器（模型输出不可信：可能带 <script> 或被提示注入）。
        // 流式 delta 已发给前端（流中预览，未消毒）；落库的是消毒版。
        // 前端在流结束后回读 get_article 拿消毒版覆盖流式预览（见 store.ts
        // toggleReaderTranslation 的 onDone 回读逻辑）。
        let safe = crate::sanitize::sanitize(&outcome.text, None);
        let conn = state.db.lock().await;
        let _ = db::set_article_ai_fields(&conn, article_id, None, Some(&safe));
    }
    let _ = on_channel.send(AiEvent::Done);
    Ok(outcome.text)
}

#[cfg(test)]
mod ai_event_tests {
    use super::AiEvent;

    /// 前端 api.ts 按 {type, data} 解析；序列化必须严格一致（camelCase tag）。
    #[test]
    fn ai_event_serialization_matches_frontend() {
        let delta = serde_json::to_value(AiEvent::Delta("你好".into())).unwrap();
        assert_eq!(delta["type"], "delta");
        assert_eq!(delta["data"], "你好");
        let done = serde_json::to_value(AiEvent::Done).unwrap();
        assert_eq!(done["type"], "done");
        let err = serde_json::to_value(AiEvent::Error("失败".into())).unwrap();
        assert_eq!(err["type"], "error");
        assert_eq!(err["data"], "失败");
    }
}
