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
/// 约束三类对象：单条完整行（复制/解析前检查）、无换行的残留、单个事件的
/// data 载荷总量——注释行同样受约束，text 预算替代不了它。
const MAX_SSE_BUFFER: usize = 8 * 1024 * 1024;

/// 累计输出文本预算：正常请求 max_tokens ≤ 4096，任何合规服务端都不可能产出
/// 256 KiB 文本（4096 token 按最宽 64 B/token 也只有 256 KiB 的一半）。
/// 达到即明确失败，防故障/恶意端点拖爆内存——不静默截断、不假装完成。
const MAX_OUTPUT_TEXT: usize = 256 * 1024;

/// 官方预设：preset 名 → (base_url, 默认模型)。
/// 自定义预设（newapi 等）直接存 base_url，走同一条 OpenAI 兼容路径。
///
/// 注意：OpenAI 的 gpt-4.1 系列（及 o1 推理模型）已弃用 `max_tokens`，改用
/// `max_completion_tokens`；而 DeepSeek/GLM 仍只认 `max_tokens`。本项目统一
/// 用 `max_tokens`（stream_chat 请求体），故 OpenAI 预设默认模型须选仍支持
/// `max_tokens` 的 gpt-4o-mini（而非 gpt-4.1-mini），否则默认配置即报错。
pub const PRESETS: &[(&str, &str, &str)] = &[
    ("deepseek", "https://api.deepseek.com", "deepseek-chat"),
    ("openai", "https://api.openai.com/v1", "gpt-4o-mini"),
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
        // TASK-076（P2-11，DEC-req104-p2-11-ai-validation-20260920）：未知 preset 不再
        // 静默回落 deepseek-chat / api.deepseek.com。此前用户选了一个不存在的预设时，
        // 请求会被打到其**并未选择**的厂商（DeepSeek）上，既可能计费错对象，也无任何
        // 提示——属于「配置写错但看起来正常」。现在只有两种合法形态：
        //   ① preset 命中 PRESETS 之一 → 用该预设的默认模型/地址；
        //   ② 自定义（自定义预设存 base_url，preset 可为空或自定义名）→ 必须显式提供
        //      非空 model 与 baseUrl。
        // 两者都不满足 → 报错让用户看见，而不是替他选一家厂商。
        let preset_entry = PRESETS.iter().find(|(p, _, _)| *p == preset);
        let explicit_model = v["model"]
            .as_str()
            .map(|m| m.trim())
            .filter(|m| !m.is_empty());
        let explicit_base = v["baseUrl"]
            .as_str()
            .map(|u| u.trim().trim_end_matches('/'))
            .filter(|u| !u.is_empty());

        let model = match (preset_entry, explicit_model) {
            (Some((_, _, m)), None) => (*m).to_string(),
            (_, Some(m)) => m.to_string(),
            (None, None) => {
                return Err(AppError::new(
                    "aiNotConfigured",
                    format!(
                        "无法识别的 AI 预设「{preset}」：请选择受支持的预设，或自定义填写模型与 API 地址"
                    ),
                ))
            }
        };
        let base_url = match (preset_entry, explicit_base) {
            (_, Some(u)) => u.to_string(),
            (Some((_, u, _)), None) => (*u).to_string(),
            (None, None) => {
                return Err(AppError::new(
                    "aiNotConfigured",
                    format!(
                        "无法识别的 AI 预设「{preset}」：请选择受支持的预设，或自定义填写模型与 API 地址"
                    ),
                ))
            }
        };
        Ok(AiConfig {
            api_key,
            model,
            base_url,
        })
    }

    /// 已解析的模型名（key 不外泄）。
    pub fn model(&self) -> &str {
        &self.model
    }
}

/* ---------------- OPT-013 B1/B2：AI 产物缓存签名与分段策略 ---------------- */

/// 分段策略版本：签名输入的一部分。
/// - `v1-flat`（B1）：整篇一次性发送，无分段。
/// - `v2-html-blocks-8192`（B2）：按 HTML 节点边界、单段预算
///   [`TRANSLATE_SEGMENT_MAX_BYTES`] 的确定性分段（见 `crate::ai_segments`）。
///
/// 策略（算法/预算/边界规则/摘要超预算语义）任何变化都必须递增本常量：
/// 它进签名输入，旧签名与旧片段因此不再被当成同一策略的缓存/可恢复片段。
pub const SEGMENT_POLICY_VERSION: &str = "v2-html-blocks-8192";

/// 翻译分段的单段预算（UTF-8 字节）。取 8192：约 2.7k 汉字 / 8k ASCII 字符，
/// 按最宽估算其译文输出仍小于 [`TRANSLATE_MAX_TOKENS`]（4096 token）的模型
/// 输出上限；超长单节点按同一预算递归拆分（有界拆分下限见 ai_segments 头注）。
pub const TRANSLATE_SEGMENT_MAX_BYTES: usize = 8192;

/// 摘要输入预算（UTF-8 字节）：超过即**显式失败**（错误码 `aiTooLong`）——
/// 不静默截掉后半篇、也不假称完整。策略取舍见 Note「OPT-013 B2 追加」：
/// 模板摘要（多段摘要再合并）会引入二次模型调用与合并口径，成本/不确定性
/// 高于收益，本卡选择如实失败并把重试入口留给用户。
///
/// 该上限是**本地防御性预算**，不是模型上下文保证：更小的模型窗口由服务端
/// 错误如实回传（原样透传，不翻译成"内容太长"之类的猜测）；64 KiB 已远超
/// common 8k–32k 上下文模型的单次输入容量，只拦真正病态的输入。
pub const SUMMARY_INPUT_MAX_BYTES: usize = 64 * 1024;

/// 缓存签名的输入清单（字段集合 = 定版 OPT-013 §1 的签名输入；**api_key 不在
/// 其中**——签名可公开，凭据绝不进入）。
///
/// Note: 命中判定 / 同锁快照 / 提交与代次守卫 — 见
/// .agents/notes/implemented/architecture/2026-10-08-AI完成信号与缓存提交.md
/// 末节「OPT-013 B1 追加（2026-10-15）」。
#[derive(Debug, Clone, Copy)]
pub struct AiCacheInputs<'a> {
    /// 产物类型标识（"summary" / "translation"；改动即让全部旧签名失效）。
    pub kind: &'a str,
    pub model: &'a str,
    pub base_url: &'a str,
    /// 有效 system 提示词（与请求实际发送的 system 逐字一致）。
    pub system: &'a str,
    pub title: &'a str,
    pub content_id: Option<i64>,
    pub content_revision: i64,
    /// 输入正文（summary = body_text；translation = content_html）。
    pub input_body: &'a str,
}

/// AI 产物缓存签名（SHA-256，小写十六进制）。
///
/// 输入（定版 OPT-013 §1）：`sha256(产物类型 ‖ 模型 ‖ base_url ‖ 有效 system
/// 提示词 ‖ 标题 ‖ 正文修订(content_id,revision) ‖ 输入正文哈希 ‖ 分段策略版本)`。
///
/// 编码：各字段以「标签 + 小端 u64 长度前缀 + UTF-8 字节」依次喂入哈希流，
/// 避免朴素拼接的边界歧义（两串不同输入因分隔符吸收重合出同一签名）；正文
/// 先单独取 sha256 再作为 `body_sha256` 字段喂入（即规范里的「输入正文哈希」）。
///
/// 语义：签名 = 「这份产物是在什么输入（模型/endpoint/提示词/标题/修订/正文/
/// 分段策略）下生成的」。缓存命中要求产物非空**且**存盘签名等于当前输入重算值；
/// 任何一项变化（换模型、换提示词、正文修订推进、标题更新）都让旧产物降级为
/// 「未标记/过期版本」，可显示、可被重新生成覆盖，但不得短路。
pub fn ai_cache_signature(inputs: &AiCacheInputs<'_>) -> String {
    use sha2::{Digest, Sha256};

    fn feed(h: &mut Sha256, label: &str, value: &str) {
        h.update(label.as_bytes());
        h.update((value.len() as u64).to_le_bytes());
        h.update(value.as_bytes());
    }

    let mut h = Sha256::new();
    h.update(b"fluxreader-ai-cache-sig-v1");
    feed(&mut h, "kind", inputs.kind);
    feed(&mut h, "model", inputs.model);
    feed(&mut h, "base_url", inputs.base_url);
    feed(&mut h, "system", inputs.system);
    feed(&mut h, "title", inputs.title);
    feed(
        &mut h,
        "content_id",
        &inputs
            .content_id
            .map_or_else(|| "-".to_string(), |v| v.to_string()),
    );
    feed(&mut h, "revision", &inputs.content_revision.to_string());
    let body_sha256 = {
        let mut b = Sha256::new();
        b.update(inputs.input_body.as_bytes());
        format!("{:x}", b.finalize())
    };
    feed(&mut h, "body_sha256", &body_sha256);
    feed(&mut h, "segment_policy", SEGMENT_POLICY_VERSION);
    format!("{:x}", h.finalize())
}

/// 流式对话结果。
pub struct ChatOutcome {
    /// 完整拼好的文本（消费方取消时含已推送的部分）。
    pub text: String,
    /// true = 模型显式 finish_reason="stop" 且收尾为 [DONE]（权威终态，立即
    /// 返回）或干净 EOF；false = 消费方中断（取消，不落库残缺文本）。
    pub completed: bool,
}

/// 单轮流式对话：POST {base}/chat/completions（SSE），逐 token 推给 sink。
///
/// 完成语义（OPT-013A，决定与备选见 .agents/notes/implemented/architecture/
/// 2026-10-08-AI完成信号与缓存提交.md）：HTTP EOF 不是完成信号——只有模型显式
/// finish_reason="stop" 且流正常收尾才 completed=true；收尾形态为随后到达的
/// [DONE]（权威终态：立即返回、释放响应、不再读取网络）或干净 EOF。
/// length→aiTruncated、content_filter→aiFiltered、tool_calls/未知/无 finish 的
/// EOF/DONE 先于 stop→aiIncomplete。帧解析按 SSE 规范（流首可选 BOM、空行
/// 事件分隔、多行 data 合并、CRLF），行/事件/正文各有显式预算（见 SseParser）。
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

/// 单行/单步处理结果。
enum Step {
    Continue,
    /// 消费方关闭（sink 返回 false）：取消，无错误、不缓存。
    ChannelClosed,
    /// stop 之后收到 [DONE]：权威终态——立即完成并停止读取后续网络数据。
    Complete,
}

/// 流首可选 BOM（EF BB BF）：只可能出现在流最前且可能跨 TCP chunk。
///
/// 前缀不足 3 字节且仍与 BOM 前缀相同时保持“未判定”（继续等待后续 chunk）；
/// 一旦可判定就置 `at_start=false`（无论是否真的剥掉了 BOM）——BOM 只在流首
/// 支持，流中不再剥除。
fn strip_leading_bom(at_start: &mut bool, buf: &mut Vec<u8>) {
    if !*at_start {
        return;
    }
    const BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];
    if buf.len() >= 3 {
        if buf[..3] == BOM {
            buf.drain(..3);
        }
        *at_start = false;
    } else if !buf.iter().zip(BOM.iter()).all(|(b, p)| b == p) {
        // 前几字节已偏离 BOM 前缀：正常内容，无需再等
        *at_start = false;
    }
}

/// SSE 事件框架 + 完成语义状态机。
///
/// 框架按 WHATWG SSE 规范：行以 `\n` 分隔，行结束符（LF 及其前 CR）由调用方
/// 剥除，值不携带任何 EOL；只有**真正空行**（去行结束符后长度 0）派发当前
/// 事件——纯空白行是未知字段、忽略（否则多行 data JSON 会被提前截断）；同一
/// 事件的多条 `data:` 行以 `\n` 连接为一个载荷（不逐行独立 parse 丢事件语义；
/// join 插入的 LF 计入载荷预算，空 data 行也计入）；`data:` 值剥一个前导空格，
/// 无冒号的 `data` 行 = 空值；注释（`:` 开头）与其他字段行忽略；流首可选 BOM
/// 由调用方在字节层剥除（可跨 chunk）。
///
/// 完成语义（OPT-013A，决定与备选见
/// .agents/notes/implemented/architecture/2026-10-08-AI完成信号与缓存提交.md）：
/// 次序契约为 stop 先、DONE 后——DONE 是权威终态，收到即完成并立即返回，其后
/// 网络数据（含同一 chunk 尾随字节）不再消费；DONE 先于 stop 按 aiIncomplete
/// 处理。未收到 DONE 时，只有 stop + 干净 EOF 才算完成——HTTP EOF 本身不是
/// 完成信号。心跳例外：空行、空 `data:` 行、`choices:[]` 的 usage 帧与注释行
/// 均不影响完成语义。
///
/// 兼容旧供应方：单行 JSON 单帧可以在事件空行或 EOF 时派发。规范在 EOF 会
/// 丢弃未以空行收尾的未完成事件，这里刻意偏离以免尾帧无空行时丢掉 finish
/// 帧（Note 已声明该取舍）。
struct SseParser {
    /// 已累计的文本（发给 sink 的增量总和）。
    full: String,
    /// 当前事件已累积的 data: 载荷（多行以 \n 连接后统一 parse）。
    data_lines: Vec<String>,
    /// 当前事件的 data 总字节（无空行分隔的多行洪泛防护，与行缓冲同口径）。
    data_bytes: usize,
    /// 已见的 finish_reason；本状态机只接受 "stop" 存活到流结束。
    finish_reason: Option<String>,
}

impl SseParser {
    fn new() -> Self {
        Self {
            full: String::new(),
            data_lines: Vec::new(),
            data_bytes: 0,
            finish_reason: None,
        }
    }

    /// 处理一条完整行（调用方已剥 `\n` 与行尾 `\r`）。
    ///
    /// 返回 Err = 协议级失败或模型非正常完成：不完整文本绝不能因 EOF 被误报为完成。
    fn handle_line(&mut self, line: &str, sink: &mut DeltaSink<'_>) -> AppResult<Step> {
        if line.is_empty() {
            // 真正的空行（去行结束符后长度为 0）= 事件分隔：派发当前事件。
            // 纯空白行不是分隔符（按规范是未知字段行），否则多行 data JSON 会被
            // 夹在中间的空格行提前截断。
            return self.dispatch_event(sink);
        }
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("data:") {
            // 规范：值剥一个前导空格（只一个，其余属于内容）
            let value = rest.strip_prefix(' ').unwrap_or(rest);
            self.push_data_line(value)?;
        } else if trimmed == "data" {
            // 规范：无冒号的字段行 = 空值 data 行
            self.push_data_line("")?;
        }
        // 其余（注释 : 开头 / event: / id: / retry:）不影响完成语义
        Ok(Step::Continue)
    }

    /// 追加一条 data 行（值已剥行结束符与一个前导空格）。
    ///
    /// 实际载荷预算 = 各值字节 + join 时插入的 LF（首行不插；空 data 行也计入），
    /// 防无空行分隔的多行洪泛；恰好 8 MiB 合法，超 1 字节显式失败。
    fn push_data_line(&mut self, value: &str) -> AppResult<()> {
        let join_lf = usize::from(!self.data_lines.is_empty());
        let add = value.len().saturating_add(join_lf);
        if self.data_bytes.saturating_add(add) > MAX_SSE_BUFFER {
            return Err(AppError::new(
                "aiStream",
                "SSE 单个事件的 data 载荷超过行缓冲上限（8 MiB），响应不是正常的 SSE 流",
            ));
        }
        self.data_bytes += add;
        self.data_lines.push(value.to_string());
        Ok(())
    }

    /// 派发当前事件（空行或 EOF 触发）。规范：多条 data: 以 \n 连接为一个载荷。
    fn dispatch_event(&mut self, sink: &mut DeltaSink<'_>) -> AppResult<Step> {
        if self.data_lines.is_empty() {
            return Ok(Step::Continue); // 空行心跳/事件分隔，无负载
        }
        let payload = self.data_lines.join("\n");
        self.data_lines.clear();
        self.data_bytes = 0;
        let payload = payload.trim();
        if payload.is_empty() {
            return Ok(Step::Continue); // 空 data 心跳（data: / data: 空格）
        }

        // [DONE]：次序契约 stop 先、DONE 后；DONE 即权威终态，立即完成返回。
        if payload == "[DONE]" {
            if self.finish_reason.as_deref() == Some("stop") {
                return Ok(Step::Complete);
            }
            return Err(AppError::new(
                "aiIncomplete",
                "收到 [DONE] 但此前未见 finish_reason=stop，不能视为完整",
            ));
        }

        let value: Value = serde_json::from_str(payload).map_err(|e| {
            AppError::new(
                "aiStream",
                format!("SSE data 不是合法 JSON（不静默丢弃）: {e}"),
            )
        })?;

        // 兼容中转会在成功帧里带 "error": null；非 null 的 error 一律显式失败。
        if let Some(err) = value.get("error").filter(|v| !v.is_null()) {
            let msg = err["message"]
                .as_str()
                .filter(|m| !m.is_empty())
                .map(String::from)
                .unwrap_or_else(|| err.to_string());
            return Err(AppError::new("aiStream", format!("AI 流式错误: {msg}")));
        }

        let Some(choices) = value.get("choices").and_then(|c| c.as_array()) else {
            return Ok(Step::Continue); // 非 chat 帧（如仅含 id 的保活）不影响完成语义
        };
        let Some(choice) = choices.first() else {
            return Ok(Step::Continue); // usage 统计帧：choices 为空，不误报
        };

        // 文本增量。同一事件内 content 与 finish_reason 并存时先收文本后记 finish
        //（部分中转会把最后一个字符与 finish 放一起）；finish 之后的增量拒绝越界拼接。
        if let Some(text) = choice["delta"]["content"].as_str() {
            if !text.is_empty() {
                if self.finish_reason.is_some() {
                    return Err(AppError::new(
                        "aiStream",
                        "finish_reason 之后仍收到文本增量，拒绝越界拼接",
                    ));
                }
                if self.full.len().saturating_add(text.len()) > MAX_OUTPUT_TEXT {
                    return Err(AppError::new(
                        "aiStream",
                        format!("AI 输出超过本地预算上限（{MAX_OUTPUT_TEXT} 字节）"),
                    ));
                }
                self.full.push_str(text);
                if !sink(text) {
                    return Ok(Step::ChannelClosed);
                }
            }
        }

        // finish_reason：null/缺省 = 尚未完成；非 null 必须一次性给出合法值。
        if let Some(reason) = choice.get("finish_reason").filter(|v| !v.is_null()) {
            if self.finish_reason.is_some() {
                return Err(AppError::new("aiStream", "SSE 流出现重复的 finish_reason"));
            }
            let reason = reason.as_str().unwrap_or("");
            match reason {
                "stop" => self.finish_reason = Some("stop".to_string()),
                "length" => {
                    return Err(AppError::new(
                        "aiTruncated",
                        "模型输出达到长度上限被截断（finish_reason=length）",
                    ))
                }
                "content_filter" => {
                    return Err(AppError::new(
                        "aiFiltered",
                        "模型输出被内容过滤中断（finish_reason=content_filter）",
                    ))
                }
                "" => {
                    return Err(AppError::new(
                        "aiIncomplete",
                        "finish_reason 不是字符串，无法视为正常完成",
                    ))
                }
                other => {
                    return Err(AppError::new(
                        "aiIncomplete",
                        format!("模型以非文本完成原因结束（finish_reason={other}）"),
                    ))
                }
            }
        }
        Ok(Step::Continue)
    }

    /// EOF：先派发可能未以空行收尾的最后一帧（兼容承诺），再判定终态。
    fn finish_stream(mut self, sink: &mut DeltaSink<'_>) -> AppResult<ChatOutcome> {
        match self.dispatch_event(sink)? {
            Step::Complete => return Ok(self.into_complete()),
            Step::ChannelClosed => return Ok(self.into_cancelled()),
            Step::Continue => {}
        }
        if self.finish_reason.as_deref() == Some("stop") {
            Ok(self.into_complete())
        } else {
            // EOF 不是完成信号：无 finish_reason 的流按不完整处理。
            Err(AppError::new(
                "aiIncomplete",
                "流在未收到 finish_reason 的情况下结束（HTTP EOF 不是完成信号）",
            ))
        }
    }

    /// DONE 权威终态：完成，且调用方停止读取并释放响应。
    fn into_complete(self) -> ChatOutcome {
        ChatOutcome {
            text: self.full,
            completed: true,
        }
    }

    /// 消费方取消：无错误，不缓存。
    fn into_cancelled(self) -> ChatOutcome {
        ChatOutcome {
            text: self.full,
            completed: false,
        }
    }
}

/// 从缓冲行字节剥掉行结束符：一个 LF，以及紧邻其前的一个 CR（CRLF）。
/// EOF 残余若没有 LF，则剥尾部单独的一个 CR（CR-only 端点）。返回值即「真正的
/// 字段行内容」——空行判定与 data 值都不携带任何 EOL 字节（join 只插入 LF）。
fn strip_line_ending(bytes: &[u8]) -> &[u8] {
    let mut end = bytes.len();
    if end > 0 && bytes[end - 1] == b'\n' {
        end -= 1;
    }
    if end > 0 && bytes[end - 1] == b'\r' {
        end -= 1;
    }
    &bytes[..end]
}

/// 驱动 SSE 响应流：按规范切帧（流首 BOM / 空行事件 / 多行 data / CRLF）+
/// 严格完成语义。每条完整行在复制/解析前受行缓冲预算约束，未结束残留同样
/// 受限——超限的注释行也会被拒绝（text 预算只管正文，替代不了框架预算）。
async fn consume_sse(resp: Response, sink: &mut DeltaSink<'_>) -> AppResult<ChatOutcome> {
    let mut resp = if resp.status().is_success() {
        resp
    } else {
        let status = resp.status();
        let detail = resp.text().await.unwrap_or_default();
        return Err(AppError::new(
            "aiStream",
            format!("AI API {status}: {detail}"),
        ));
    };

    let mut buf: Vec<u8> = Vec::new();
    let mut parser = SseParser::new();
    let mut at_start = true; // 流首 BOM 判定（可跨 chunk）

    while let Some(chunk) = resp.chunk().await? {
        buf.extend_from_slice(&chunk);
        strip_leading_bom(&mut at_start, &mut buf);
        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            // 完整行先过预算再复制/解析（8 MiB-ε 残行 + 下一 chunk 补 newline 不能漏）
            if pos + 1 > MAX_SSE_BUFFER {
                return Err(AppError::new(
                    "aiStream",
                    "SSE 单行超过行缓冲上限（8 MiB），响应不是正常的 SSE 流",
                ));
            }
            let raw: Vec<u8> = buf.drain(..=pos).collect();
            // 严格 UTF-8：跨 TCP chunk 的多字节序列在完整行内重组；
            // 真出现非法字节就让请求失败，而不是 lossy 替换后当成功。
            // 行结束符（LF 与其前 CR）在进状态机前剥除，值不再携带 EOL。
            let line = std::str::from_utf8(strip_line_ending(&raw))
                .map_err(|_| AppError::new("aiStream", "SSE 流包含非法 UTF-8，无法作为文本处理"))?;
            match parser.handle_line(line, sink)? {
                Step::Continue => {}
                Step::ChannelClosed => return Ok(parser.into_cancelled()),
                // stop 后的 [DONE] 是权威终态：立即返回并释放响应，
                // 不再读取/等待后续网络数据（同一 chunk 的尾随字节一并丢弃）。
                Step::Complete => return Ok(parser.into_complete()),
            }
        }
        if buf.len() > MAX_SSE_BUFFER {
            return Err(AppError::new(
                "aiStream",
                "SSE 行缓冲超过上限（未结束行），响应不是正常的 SSE 流",
            ));
        }
    }
    // 尾帧可能无换行/无结尾空行（兼容旧供应方）：按同一状态机处理残余，
    // 未完成事件在 EOF 派发（规范本应丢弃；见 Note 的兼容声明）。
    if !buf.is_empty() {
        let line = std::str::from_utf8(strip_line_ending(&buf))
            .map_err(|_| AppError::new("aiStream", "SSE 流包含非法 UTF-8，无法作为文本处理"))?;
        match parser.handle_line(line, sink)? {
            Step::Continue => {}
            Step::ChannelClosed => return Ok(parser.into_cancelled()),
            Step::Complete => return Ok(parser.into_complete()),
        }
    }
    parser.finish_stream(sink)
}

#[cfg(test)]
mod tests {
    use super::*;

    /* ---- SSE 状态机单测（规范帧 + 严格完成语义；真实 loopback 端到端见
    tests/ai_stream_completion_e2e.rs，两边互为对照）---- */

    /// 按 SSE 规范喂入若干行（空字符串 = 事件分隔/派发），返回最终结果。
    fn run_lines(lines: &[&str]) -> AppResult<ChatOutcome> {
        let mut parser = SseParser::new();
        let mut sink = |_: &str| true;
        for line in lines {
            match parser.handle_line(line, &mut sink) {
                Ok(Step::Continue) => {}
                Ok(Step::Complete) => return Ok(parser.into_complete()),
                Ok(Step::ChannelClosed) => panic!("sink 不应关闭"),
                Err(e) => return Err(e),
            }
        }
        parser.finish_stream(&mut sink)
    }

    /// 期望成功完成。
    fn feed_ok(lines: &[&str]) -> ChatOutcome {
        match run_lines(lines) {
            Ok(outcome) => outcome,
            Err(e) => panic!("不应 Err: {e}"),
        }
    }

    /// 期望显式失败（返回首个错误）。
    fn feed_err(lines: &[&str]) -> AppError {
        match run_lines(lines) {
            Ok(outcome) => panic!("应产生 Err，实际 completed={}", outcome.completed),
            Err(e) => e,
        }
    }

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
        assert_eq!(cfg.model, "gpt-4o-mini");
    }

    #[test]
    fn config_missing_key_rejected() {
        assert!(AiConfig::from_json(r#"{"preset":"openai"}"#).is_err());
    }

    /// TASK-076（P2-11）：未知 preset 必须显式报错，**不得**静默回落到
    /// deepseek-chat / https://api.deepseek.com。
    ///
    /// 修前行为：`{"preset":"azure","apiKey":"sk-x"}` 会得到
    /// model=deepseek-chat、base_url=https://api.deepseek.com —— 用户选了不存在的
    /// 预设，请求却被悄悄打到 DeepSeek（可能计费错对象），且没有任何提示。
    #[test]
    fn unknown_preset_is_rejected_instead_of_silently_falling_back() {
        let msg = match AiConfig::from_json(r#"{"preset":"azure","apiKey":"sk-x"}"#) {
            Ok(_) => panic!("未知 preset 必须报错，而不是静默回落"),
            Err(e) => format!("{e}"),
        };
        assert!(
            msg.contains("azure"),
            "报错应指明无法识别的 preset 名，实际: {msg}"
        );
    }

    /// 未知 preset 但同时显式给了 model + baseUrl → 视为自定义配置，正常可用。
    #[test]
    fn unknown_preset_with_explicit_model_and_base_url_is_custom() {
        let cfg = AiConfig::from_json(
            r#"{"preset":"azure","apiKey":"sk-x","model":"gpt-4o","baseUrl":"https://az.example/v1"}"#,
        )
        .unwrap();
        assert_eq!(cfg.model, "gpt-4o");
        assert_eq!(cfg.base_url, "https://az.example/v1");
    }

    /// 空 preset（历史配置可能没有该字段）+ 显式 model/baseUrl → 自定义，仍可用。
    #[test]
    fn empty_preset_with_explicit_config_is_custom() {
        let cfg = AiConfig::from_json(
            r#"{"apiKey":"sk-x","model":"m1","baseUrl":"https://c.example/v1/"}"#,
        )
        .unwrap();
        assert_eq!(cfg.model, "m1");
        assert_eq!(cfg.base_url, "https://c.example/v1");
    }

    /// 未知 preset 且缺 baseUrl（只有 model）→ 仍须报错（不能替用户选厂商地址）。
    #[test]
    fn unknown_preset_with_only_model_is_rejected() {
        assert!(
            AiConfig::from_json(r#"{"preset":"azure","apiKey":"sk-x","model":"gpt-4o"}"#).is_err()
        );
    }

    /// P2-1：BOM 只在流首支持；跨 chunk 凑齐后剥一次，且等待期间不改动字节。
    #[test]
    fn bom_stripped_only_at_stream_start() {
        let mut at_start = true;
        let mut buf = vec![0xEF, 0xBB];
        strip_leading_bom(&mut at_start, &mut buf);
        assert!(at_start, "BOM 前缀未凑齐时应继续等待");
        assert_eq!(buf, vec![0xEF, 0xBB], "等待期间不得改动字节");

        buf.extend_from_slice(&[0xBF, b'd']);
        strip_leading_bom(&mut at_start, &mut buf);
        assert!(!at_start);
        assert_eq!(buf, b"d", "凑齐后剥除 BOM");

        // 非流首不再剥（即使字节恰好以 BOM 开头）
        let mut later = vec![0xEF, 0xBB, 0xBF, b'x'];
        strip_leading_bom(&mut at_start, &mut later);
        assert_eq!(later, vec![0xEF, 0xBB, 0xBF, b'x'], "BOM 只在流首支持");
    }

    /// 首字节已偏离 BOM 前缀时立即判定，不拖延正常内容。
    #[test]
    fn non_bom_prefix_decides_immediately() {
        let mut at_start = true;
        let mut buf = b"da".to_vec();
        strip_leading_bom(&mut at_start, &mut buf);
        assert!(!at_start, "首字节非 BOM 前缀即判定");
        assert_eq!(buf, b"da");
    }

    #[test]
    fn sse_line_extracts_delta_and_ignores_null_error() {
        let outcome = feed_ok(&[
            r#"data: {"choices":[{"delta":{"content":"你好"}}],"error":null}"#,
            "",
            r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            "",
            "data: [DONE]",
            "",
        ]);
        assert_eq!(outcome.text, "你好");
    }

    #[test]
    fn sse_line_object_error_is_error() {
        let err = feed_err(&[r#"data: {"error":{"message":"rate limited"}}"#, ""]);
        assert_eq!(err.code, "aiStream");
        assert!(err.message.contains("rate limited"), "{err}");
    }

    #[test]
    fn sse_line_sink_close_stops() {
        let mut parser = SseParser::new();
        let mut sink = |_: &str| false; // 模拟前端关闭
        let line = r#"data: {"choices":[{"delta":{"content":"x"}}]}"#;
        assert!(matches!(
            parser.handle_line(line, &mut sink),
            Ok(Step::Continue)
        ));
        match parser.handle_line("", &mut sink).unwrap() {
            Step::ChannelClosed => {}
            _ => panic!("expected channel closed"),
        }
        assert_eq!(parser.into_cancelled().text, "x", "已推送的增量保留");
    }

    /// 规范事件：同一事件的多条 data: 行以 \n 连接为一个 JSON 载荷再解析。
    #[test]
    fn sse_multiline_data_lines_join() {
        let outcome = feed_ok(&[
            r#"data: {"choices":[{"delta":"#,
            r#"data: {"content":"多行事件"}}]}"#,
            "",
            r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            "",
            "data: [DONE]",
            "",
        ]);
        assert!(outcome.completed);
        assert_eq!(outcome.text, "多行事件");
    }

    /// stop + [DONE] 或 stop + 干净 EOF 都算完成；只有 [DONE] 没 reason 不算；
    /// DONE 先于 stop 违反次序契约 → aiIncomplete。
    #[test]
    fn sse_completion_requires_stop_and_normal_end() {
        let delta = r#"data: {"choices":[{"delta":{"content":"正文"}}]}"#;
        let stop = r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#;

        let with_done = feed_ok(&[delta, "", stop, "", "data: [DONE]", ""]);
        assert!(with_done.completed);
        assert_eq!(with_done.text, "正文");

        let clean_eof = feed_ok(&[delta, "", stop]);
        assert!(clean_eof.completed, "stop + 干净 EOF 属正常关闭");

        let err = feed_err(&[delta, "", "data: [DONE]", ""]);
        assert_eq!(err.code, "aiIncomplete", "[DONE] 无 reason 不能称自然完整");

        let err = feed_err(&[delta, "", "data: [DONE]", "", stop, ""]);
        assert_eq!(err.code, "aiIncomplete", "DONE 先于 stop 立即 aiIncomplete");

        let err = feed_err(&[delta, ""]);
        assert_eq!(err.code, "aiIncomplete", "无 finish 的 EOF 不是完成信号");
    }

    #[test]
    fn sse_length_and_content_filter_map_to_typed_errors() {
        let err = feed_err(&[
            r#"data: {"choices":[{"delta":{},"finish_reason":"length"}]}"#,
            "",
        ]);
        assert_eq!(err.code, "aiTruncated");
        let err = feed_err(&[
            r#"data: {"choices":[{"delta":{},"finish_reason":"content_filter"}]}"#,
            "",
        ]);
        assert_eq!(err.code, "aiFiltered");
    }

    #[test]
    fn sse_tool_calls_and_unknown_reason_are_incomplete() {
        let err = feed_err(&[
            r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
            "",
        ]);
        assert_eq!(err.code, "aiIncomplete");
        let err = feed_err(&[
            r#"data: {"choices":[{"delta":{},"finish_reason":"made_up"}]}"#,
            "",
        ]);
        assert_eq!(err.code, "aiIncomplete");
        assert!(err.message.contains("made_up"), "{err}");
    }

    /// 重复/冲突 finish、finish 后增量都拒绝，且不拼接越界文本。
    #[test]
    fn sse_out_of_band_data_is_rejected() {
        let stop = r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#;
        let err = feed_err(&[stop, "", stop, ""]);
        assert_eq!(err.code, "aiStream", "重复 finish");

        let err = feed_err(&[
            stop,
            "",
            r#"data: {"choices":[{"delta":{},"finish_reason":"length"}]}"#,
            "",
        ]);
        assert_eq!(err.code, "aiStream", "冲突 finish");

        let err = feed_err(&[
            stop,
            "",
            r#"data: {"choices":[{"delta":{"content":"尾巴"}}]}"#,
            "",
        ]);
        assert_eq!(err.code, "aiStream", "finish 后增量");
        assert!(err.message.contains("拼接"), "{err}");
    }

    #[test]
    fn sse_non_json_data_and_empty_choices() {
        let err = feed_err(&["data: {oops}", ""]);
        assert_eq!(err.code, "aiStream", "非 JSON 不得静默丢弃");

        // usage 帧（choices 空）、无冒号/空值的 data 心跳不影响完成语义
        let stop = r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#;
        let outcome = feed_ok(&[
            r#"data: {"choices":[],"usage":{"completion_tokens":1}}"#,
            "",
            "data:",
            "",
            "data: ",
            "",
            stop,
            "",
            "data: [DONE]",
            "",
        ]);
        assert!(outcome.completed);
    }

    /// 累计文本预算：达到上限后的下一个字符必须显式失败，不静默截断。
    #[test]
    fn sse_text_over_budget_is_explicit_error() {
        let mut parser = SseParser::new();
        let mut sink = |_: &str| true;
        let big = "a".repeat(MAX_OUTPUT_TEXT);
        let line = format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{big}\"}}}}]}}");
        parser.handle_line(&line, &mut sink).unwrap();
        parser.handle_line("", &mut sink).unwrap(); // 第一段恰好等于预算，允许
        parser.handle_line(&line, &mut sink).unwrap();
        let err = match parser.handle_line("", &mut sink) {
            Ok(_) => panic!("超预算必须失败"),
            Err(e) => e,
        };
        assert_eq!(err.code, "aiStream");
        assert!(err.message.contains("预算"), "{err}");
    }

    /// 无空行分隔的多行 data 洪泛：单事件载荷总字节同样受行缓冲上限约束。
    #[test]
    fn sse_event_payload_over_budget_is_explicit_error() {
        let mut parser = SseParser::new();
        let mut sink = |_: &str| true;
        let line = format!("data: {}", "a".repeat(1024));
        let mut last = Ok(Step::Continue);
        for _ in 0..(MAX_SSE_BUFFER / 1024 + 1) {
            last = parser.handle_line(&line, &mut sink);
            if last.is_err() {
                break;
            }
        }
        let err = match last {
            Ok(_) => panic!("单事件 data 载荷超限必须失败"),
            Err(e) => e,
        };
        assert_eq!(err.code, "aiStream");
    }

    /// R2-P2-1：只有真正空行（去行结束符后长度为 0）派发事件；纯空白行是
    /// 未知字段，必须忽略——否则多行 data JSON 中间的空格行会提前派发半截 JSON。
    #[test]
    fn blank_line_must_be_truly_empty_to_dispatch() {
        let outcome = feed_ok(&[
            r#"data: {"choices":[{"delta":"#,
            "   ",
            r#"data: {"content":"空格行"}}]}"#,
            "",
            r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            "",
            "data: [DONE]",
            "",
        ]);
        assert!(outcome.completed);
        assert_eq!(outcome.text, "空格行");
    }

    /// R2-P2-2：实际 data 预算 = 各 data 行值字节 + join 插入的 LF（空/无冒号
    /// data 行也计入）；恰好 8 MiB 合法，+1 字节显式失败。
    #[test]
    fn sse_event_payload_budget_counts_join_lf() {
        let mut parser = SseParser::new();
        let mut sink = |_: &str| true;
        let mib = 1024 * 1024;
        // 7 × 1 MiB 值：7 MiB + 6 个 join LF
        for _ in 0..7 {
            let line = format!("data: {}", "a".repeat(mib));
            assert!(matches!(
                parser.handle_line(&line, &mut sink),
                Ok(Step::Continue)
            ));
        }
        // 第 8 行值 1 MiB - 8：合计 8 MiB - 1
        let line = format!("data: {}", "a".repeat(mib - 8));
        assert!(matches!(
            parser.handle_line(&line, &mut sink),
            Ok(Step::Continue)
        ));
        // 无冒号 data 行（空值）：+1 个 join LF → 恰好 8 MiB，仍合法
        assert!(matches!(
            parser.handle_line("data", &mut sink),
            Ok(Step::Continue)
        ));
        // 再来一个空 data 行：8 MiB + 1 → 显式失败（若空 data 行被忽略则不会触发）
        let err = match parser.handle_line("data:", &mut sink) {
            Ok(_) => panic!("真载荷超 8 MiB 必须失败"),
            Err(e) => e,
        };
        assert_eq!(err.code, "aiStream");
        assert!(err.message.contains("行缓冲上限"), "{err}");
    }

    /// OPT-013A：完成语义修复不得靠调大 max_tokens 掩盖——输出上限保持不变。
    #[test]
    fn output_token_budgets_stay_1024_and_4096() {
        assert_eq!(SUMMARY_MAX_TOKENS, 1024);
        assert_eq!(TRANSLATE_MAX_TOKENS, 4096);
    }

    /// OPT-013 B2：分段/输入预算常量与策略版本是**签名输入与持久化片段的共同
    /// 契约**——改动即让旧签名/旧片段失效，必须显式递增版本号（本测试锁定
    /// 现值，防"改了预算忘了改版本"）。
    #[test]
    fn segment_policy_constants_are_versioned() {
        assert_eq!(SEGMENT_POLICY_VERSION, "v2-html-blocks-8192");
        assert_eq!(TRANSLATE_SEGMENT_MAX_BYTES, 8192);
        assert_eq!(SUMMARY_INPUT_MAX_BYTES, 64 * 1024);
        // 分段预算必须显著小于分段策略命名所描述的量级（防手误改坏版本语义）
        assert!(
            SEGMENT_POLICY_VERSION.contains(&TRANSLATE_SEGMENT_MAX_BYTES.to_string()),
            "策略版本号必须携带当前分段预算（{SEGMENT_POLICY_VERSION} vs {TRANSLATE_SEGMENT_MAX_BYTES}）"
        );
    }

    /* ---- OPT-013 B1：缓存签名 ---- */

    /// 签名用例的输入清单（Default = 基准输入；变体用 ..Default::default() 只改待测字段）。
    /// 字段取 String：测试要表达「运行时配置解析出的值」也能参与签名。
    struct SigCase {
        kind: String,
        model: String,
        base_url: String,
        system: String,
        title: String,
        content_id: Option<i64>,
        content_revision: i64,
        input_body: String,
    }

    impl Default for SigCase {
        fn default() -> Self {
            Self {
                kind: "summary".into(),
                model: "m1".into(),
                base_url: "https://api.example/v1".into(),
                system: "sys".into(),
                title: "标题".into(),
                content_id: Some(7),
                content_revision: 3,
                input_body: "正文".into(),
            }
        }
    }

    fn sig(case: &SigCase) -> String {
        ai_cache_signature(&AiCacheInputs {
            kind: &case.kind,
            model: &case.model,
            base_url: &case.base_url,
            system: &case.system,
            title: &case.title,
            content_id: case.content_id,
            content_revision: case.content_revision,
            input_body: &case.input_body,
        })
    }

    /// 固定输入 → 稳定十六进制 sha256（跨进程/跨调用确定）；不同字段任一变化
    /// 必产出不同签名——签名的判别力是「缓存不冒充」的全部依据。
    #[test]
    fn ai_cache_signature_is_deterministic_and_field_sensitive() {
        let base = sig(&SigCase::default());
        assert_eq!(base.len(), 64, "sha256 十六进制长度");
        assert!(base
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_eq!(base, sig(&SigCase::default()), "同输入同签名");
        // 逐字段敏感：任一输入变化都必须改变签名（否则旧产物会被当成当前缓存）
        let variants = [
            SigCase {
                kind: "translation".into(),
                ..Default::default()
            },
            SigCase {
                model: "m2".into(),
                ..Default::default()
            },
            SigCase {
                base_url: "https://api.other/v1".into(),
                ..Default::default()
            },
            SigCase {
                system: "sys2".into(),
                ..Default::default()
            },
            SigCase {
                title: "标题2".into(),
                ..Default::default()
            },
            SigCase {
                content_id: Some(8),
                ..Default::default()
            },
            SigCase {
                content_revision: 4,
                ..Default::default()
            },
            SigCase {
                input_body: "正文2".into(),
                ..Default::default()
            },
            SigCase {
                content_id: None,
                ..Default::default()
            },
        ];
        for case in variants {
            assert_ne!(sig(&case), base, "任一签名输入变化都必须改变签名");
        }
    }

    /// 长度前缀编码：字段边界不可被内容吸收（"ab"+"c" 与 "a"+"bc" 不得同签名）。
    #[test]
    fn ai_cache_signature_has_no_delimiter_ambiguity() {
        let a = sig(&SigCase {
            title: "ab".into(),
            input_body: "c".into(),
            ..Default::default()
        });
        let b = sig(&SigCase {
            title: "a".into(),
            input_body: "bc".into(),
            ..Default::default()
        });
        assert_ne!(a, b, "标题/正文拼接歧义不得碰撞");
    }

    /// 签名输入不含 api_key（输入清单即保证；此测试锁定未来不得偷偷加回）。
    #[test]
    fn ai_cache_signature_api_key_is_not_an_input() {
        // 同一组合配置，用户仅更换 api_key——签名必须不变（换 key 不使缓存失效，
        // 也不得把凭据卷进可公开的签名）。
        let cfg_a = AiConfig::from_json(
            r#"{"preset":"custom","baseUrl":"https://x/v1","apiKey":"sk-a","model":"m1"}"#,
        )
        .unwrap();
        let cfg_b = AiConfig::from_json(
            r#"{"preset":"custom","baseUrl":"https://x/v1","apiKey":"sk-b","model":"m1"}"#,
        )
        .unwrap();
        let case_a = SigCase {
            model: cfg_a.model.clone(),
            base_url: cfg_a.base_url.clone(),
            ..Default::default()
        };
        let case_b = SigCase {
            model: cfg_b.model.clone(),
            base_url: cfg_b.base_url.clone(),
            ..Default::default()
        };
        assert_eq!(sig(&case_a), sig(&case_b), "api_key 不进签名");
    }
}
