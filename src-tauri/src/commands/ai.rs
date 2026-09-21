//! commands 的 ai 领域子模块（TASK-044 从 commands.rs 按既有章节拆分，纯搬运）。

use crate::db;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
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
async fn load_ai_config(state: &State<'_, AppState>) -> AppResult<crate::ai::AiConfig> {
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
async fn load_prompts(state: &State<'_, AppState>) -> (String, String) {
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

    // P2-11（TASK-076，DEC-req104-p2-11-ai-validation-20260920）：空正文显式报错，
    // 与 ai_translate 的既有校验（"文章无正文可翻译"）对称。此前这里没有检查，
    // 空正文会照样发一次模型请求：白花 token，且 outcome.text 为空时不落缓存、
    // 用户只看到「摘要没出来」，无从判断是内容问题还是服务问题。
    if body.trim().is_empty() {
        return Err(AppError::not_found("文章无正文可摘要"));
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
