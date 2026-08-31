//! AI 引擎：OpenAI 兼容协议的流式对话（摘要/翻译）。
//! 兼容官方 OpenAI/DeepSeek/GLM 与任意 newapi 类中转（协议相同）。
//!
//! UI-free：token 增量通过 `FnMut(&str) -> bool` sink 推送（返回 false 提前
//! 停止），Tauri 命令层把 sink 包装成 ipc::Channel 推给前端。

use crate::error::{AppError, AppResult};
use reqwest::{Client, Response};
use serde_json::{json, Value};
use std::time::Duration;

/// token 增量接收器；返回 false = 消费方已关闭，提前终止流。
pub type DeltaSink<'a> = dyn FnMut(&str) -> bool + Send + 'a;

/// AI 请求独立超时（共享 client 的 30s 抓取超时会截断长生成）。
const AI_REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

/// 摘要输出上限。
pub const SUMMARY_MAX_TOKENS: u32 = 1024;
/// 翻译输出上限（译文长度跟随原文）。
pub const TRANSLATE_MAX_TOKENS: u32 = 4096;

/// SSE 行缓冲上限：恶意/非 SSE 端点防内存膨胀（8 MiB 远超正常帧）。
const MAX_SSE_BUFFER: usize = 8 * 1024 * 1024;

/// 官方预设：preset 名 → (base_url, 默认模型)。
/// 自定义预设（newapi 等）直接存 base_url，走同一条 OpenAI 兼容路径。
pub const PRESETS: &[(&str, &str, &str)] = &[
    ("deepseek", "https://api.deepseek.com", "deepseek-chat"),
    ("openai", "https://api.openai.com/v1", "gpt-4.1-mini"),
    ("glm", "https://open.bigmodel.cn/api/paas/v4", "glm-4-flash"),
];

/// 从 settings 表的 ai_config JSON 解析出的有效配置。
pub struct AiConfig {
    pub api_key: String,
    pub model: String,
    /// API 根（无尾斜杠），请求时拼 /chat/completions。
    pub base_url: String,
}

impl AiConfig {
    /// 从原始 JSON 构建：key/model/base_url 全 trim；空 key 报 aiNotConfigured。
    pub fn from_json(raw: &str) -> AppResult<Self> {
        let v: Value = serde_json::from_str(raw)
            .map_err(|_| AppError::new("aiNotConfigured", "AI 配置格式无效"))?;
        let api_key = v["apiKey"]
            .as_str()
            .map(|k| k.trim())
            .filter(|k| !k.is_empty())
            .ok_or_else(|| AppError::new("aiNotConfigured", "未配置 API Key"))?
            .to_string();
        let preset = v["preset"].as_str().unwrap_or("").trim();
        let model = v["model"]
            .as_str()
            .map(|m| m.trim())
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| {
                PRESETS
                    .iter()
                    .find(|(p, _, _)| *p == preset)
                    .map(|(_, _, m)| *m)
                    .unwrap_or("deepseek-chat")
            })
            .to_string();
        let base_url = v["baseUrl"]
            .as_str()
            .map(|u| u.trim().trim_end_matches('/'))
            .filter(|u| !u.is_empty())
            .map(String::from)
            .unwrap_or_else(|| {
                PRESETS
                    .iter()
                    .find(|(p, _, _)| *p == preset)
                    .map(|(_, u, _)| u.to_string())
                    .unwrap_or_else(|| "https://api.deepseek.com".to_string())
            });
        Ok(AiConfig { api_key, model, base_url })
    }

    /// 已解析的模型名（key 不外泄）。
    pub fn model(&self) -> &str {
        &self.model
    }
}

/// 流式对话结果。
pub struct ChatOutcome {
    pub text: String,
    /// false = 消费方中断（不落库残缺文本）。
    pub completed: bool,
}

/// 单轮流式对话：POST {base}/chat/completions（SSE），逐 token 推给 sink。
pub async fn stream_chat(
    client: &Client,
    cfg: &AiConfig,
    system: &str,
    user: &str,
    sink: &mut DeltaSink<'_>,
    max_tokens: u32,
) -> AppResult<ChatOutcome> {
    let body = json!({
        "model": cfg.model,
        "max_tokens": max_tokens,
        "stream": true,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ],
    });
    let resp = client
        .post(format!("{}/chat/completions", cfg.base_url))
        .bearer_auth(&cfg.api_key)
        .timeout(AI_REQUEST_TIMEOUT)
        .json(&body)
        .send()
        .await?;
    consume_sse(resp, sink).await
}

/// 拉模型列表（连通性测试 + 模型下拉）。官方与 newapi 都支持 /models。
pub async fn list_models(client: &Client, cfg: &AiConfig) -> AppResult<Vec<String>> {
    let resp = client
        .get(format!("{}/models", cfg.base_url))
        .bearer_auth(&cfg.api_key)
        .timeout(Duration::from_secs(20))
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let detail = resp.text().await.unwrap_or_default();
        return Err(AppError::new(
            "aiConnectivity",
            format!("HTTP {status}: {detail}"),
        ));
    }
    let v: Value = resp.json().await?;
    let mut models: Vec<String> = v["data"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m["id"].as_str())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    models.sort();
    Ok(models)
}

/* ---------------- SSE 解析 ---------------- */

/// 单行处理结果。
enum LineOutcome {
    Continue,
    ChannelClosed,
}

/// 处理一行 SSE：取 data: 载荷，识别错误，推送增量。
fn handle_sse_line(line: &str, full: &mut String, sink: &mut DeltaSink<'_>) -> AppResult<LineOutcome> {
    let Some(data) = line.trim().strip_prefix("data:") else {
        return Ok(LineOutcome::Continue);
    };
    let data = data.trim();
    if data.is_empty() || data == "[DONE]" {
        return Ok(LineOutcome::Continue);
    }
    let Ok(value) = serde_json::from_str::<Value>(data) else {
        return Ok(LineOutcome::Continue);
    };
    // 兼容服务器会在成功 chunk 里带 "error": null —— 只有非 null 对象才是错误
    if let Some(err) = value.get("error").filter(|e| e.is_object()) {
        let msg = err["message"]
            .as_str()
            .filter(|m| !m.is_empty())
            .unwrap_or("stream error");
        return Err(AppError::new("aiStream", format!("AI 流式错误: {msg}")));
    }
    if let Some(text) = value["choices"][0]["delta"]["content"].as_str() {
        full.push_str(text);
        if !sink(text) {
            return Ok(LineOutcome::ChannelClosed);
        }
    }
    Ok(LineOutcome::Continue)
}

/// 驱动 SSE 响应流，逐 token 抽取。
async fn consume_sse(resp: Response, sink: &mut DeltaSink<'_>) -> AppResult<ChatOutcome> {
    let mut resp = if resp.status().is_success() {
        resp
    } else {
        let status = resp.status();
        let detail = resp.text().await.unwrap_or_default();
        return Err(AppError::new("aiStream", format!("AI API {status}: {detail}")));
    };

    let mut buf: Vec<u8> = Vec::new();
    let mut full = String::new();

    while let Some(chunk) = resp.chunk().await? {
        buf.extend_from_slice(&chunk);
        if buf.len() > MAX_SSE_BUFFER {
            return Err(AppError::new("aiStream", "响应不是 SSE 流"));
        }
        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let raw: Vec<u8> = buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&raw);
            match handle_sse_line(&line, &mut full, &mut *sink)? {
                LineOutcome::Continue => {}
                LineOutcome::ChannelClosed => {
                    return Ok(ChatOutcome { text: full, completed: false });
                }
            }
        }
    }
    // 尾帧可能无换行（自定义端点行为）：流结束时补处理一次
    if !buf.is_empty() {
        let line = String::from_utf8_lossy(&buf);
        match handle_sse_line(&line, &mut full, &mut *sink)? {
            LineOutcome::Continue => {}
            LineOutcome::ChannelClosed => {
                return Ok(ChatOutcome { text: full, completed: false });
            }
        }
    }
    Ok(ChatOutcome { text: full, completed: true })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_from_json_full() {
        let cfg = AiConfig::from_json(
            r#"{"preset":"custom","baseUrl":"https://newapi.example.com/v1/","apiKey":" sk-1 ","model":" gpt-4o "}"#,
        )
        .unwrap();
        assert_eq!(cfg.api_key, "sk-1");
        assert_eq!(cfg.model, "gpt-4o");
        assert_eq!(cfg.base_url, "https://newapi.example.com/v1");
    }

    #[test]
    fn config_defaults_from_preset() {
        // 只给 preset+key：base_url/model 取预设默认
        let cfg = AiConfig::from_json(r#"{"preset":"openai","apiKey":"sk-2"}"#).unwrap();
        assert_eq!(cfg.base_url, "https://api.openai.com/v1");
        assert_eq!(cfg.model, "gpt-4.1-mini");
    }

    #[test]
    fn config_missing_key_rejected() {
        assert!(AiConfig::from_json(r#"{"preset":"openai"}"#).is_err());
    }

    #[test]
    fn sse_line_extracts_delta_and_ignores_null_error() {
        let mut full = String::new();
        let mut sink = |_: &str| true;
        let line = r#"data: {"choices":[{"delta":{"content":"你好"}}],"error":null}"#;
        match handle_sse_line(line, &mut full, &mut sink).unwrap() {
            LineOutcome::Continue => {}
            _ => panic!("expected continue"),
        }
        assert_eq!(full, "你好");
    }

    #[test]
    fn sse_line_object_error_is_error() {
        let mut full = String::new();
        let mut sink = |_: &str| true;
        let line = r#"data: {"error":{"message":"rate limited"}}"#;
        assert!(handle_sse_line(line, &mut full, &mut sink).is_err());
    }

    #[test]
    fn sse_line_sink_close_stops() {
        let mut full = String::new();
        let mut sink = |_: &str| false; // 模拟前端关闭
        let line = r#"data: {"choices":[{"delta":{"content":"x"}}]}"#;
        match handle_sse_line(line, &mut full, &mut sink).unwrap() {
            LineOutcome::ChannelClosed => {}
            _ => panic!("expected channel closed"),
        }
    }
}
