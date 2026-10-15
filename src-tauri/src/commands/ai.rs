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

/* ============================================================
OPT-013 B1：输入快照与缓存签名

命中判定 = 产物非空 **且** 签名等于当前输入的重算值。生成、提交、校验三处
共用同一份输入定义（`ai::ai_cache_signature`）：产物类型、模型、base_url、
有效 system 提示词、标题、正文修订 (content_id, revision)、输入正文哈希、
分段策略版本。api_key 绝不进入签名、日志或 IPC 元数据。
============================================================ */

/// AI 产物种类（签名域内的稳定标识；`ai_cache_check` 的两个字段与之对应）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AiKind {
    Summary,
    Translation,
}

impl AiKind {
    /// 签名输入里的产物类型标识（改动即让全部旧签名失效——刻意如此）。
    fn id(self) -> &'static str {
        match self {
            AiKind::Summary => "summary",
            AiKind::Translation => "translation",
        }
    }
}

/// 提示词默认文案（用户未自定义时）。设计文档承诺的默认行为。
const DEFAULT_SUMMARIZE_SYSTEM: &str = "你是一名资讯编辑。请用简洁的中文总结这篇文章，输出 3-5 个要点，每个要点一行，以 - 开头。不要重复文章标题。";
const DEFAULT_TRANSLATE_SYSTEM: &str = "你是一名专业译者。请把用户提供的 HTML 片段翻译成简体中文：保留所有 HTML 标签和属性原样不动，只翻译标签内的文本内容。直接输出翻译后的 HTML，不要任何解释或代码块包裹。";

/// 一次读出的 AI 配置快照：有效模型/endpoint + 两类有效提示词。
///
/// OPT-013 B1 定版 §1：`ai_config` 原始 JSON 与提示词必须在**同一把 DB 锁内
/// 一次读取**——此前 `load_ai_config` 与 `load_prompts` 分两次读，两窗口之间
/// 改配置会让「实际发送的模型」与「签名/提交时的模型」不是同一份。
struct AiConfigSnapshot {
    cfg: crate::ai::AiConfig,
    summary_system: String,
    translate_system: String,
}

impl AiConfigSnapshot {
    /// 从同一份 ai_config 原始 JSON 解析有效配置与提示词。
    ///
    /// 未配置（None）报 aiNotConfigured（读库失败由调用方在锁内先区分、原样上抛
    /// ——P3[2] 语义保留）。提示词缺失/非字符串回落内置默认；有效 system 提示词
    /// 是签名输入的一部分，与请求实际发送的 system 逐字一致。
    fn from_raw(raw: Option<&str>) -> AppResult<Self> {
        let Some(raw) = raw else {
            return Err(AppError::new("aiNotConfigured", "请先在设置中配置 AI 服务"));
        };
        let cfg = crate::ai::AiConfig::from_json(raw)?;
        // from_json 已成功 = 合法 JSON；此处再解析不会失败（失败按「无提示词」，
        // 与旧 load_prompts 的 fallback 行为一致，且不与有效配置判定冲突）。
        let v: serde_json::Value = serde_json::from_str(raw).unwrap_or(serde_json::Value::Null);
        let summary_system = v["summaryPrompt"]
            .as_str()
            .unwrap_or(DEFAULT_SUMMARIZE_SYSTEM)
            .to_string();
        let translate_system = v["translatePrompt"]
            .as_str()
            .unwrap_or(DEFAULT_TRANSLATE_SYSTEM)
            .to_string();
        Ok(Self {
            cfg,
            summary_system,
            translate_system,
        })
    }

    fn system_for(&self, kind: AiKind) -> &str {
        match kind {
            AiKind::Summary => &self.summary_system,
            AiKind::Translation => &self.translate_system,
        }
    }
}

/// 当前输入下的期望签名（生成/提交/校验三处同源）。
fn expected_signature(
    cfg: &crate::ai::AiConfig,
    system: &str,
    kind: AiKind,
    title: &str,
    token: &db::AiCommitToken,
    body: &str,
) -> String {
    crate::ai::ai_cache_signature(&crate::ai::AiCacheInputs {
        kind: kind.id(),
        model: &cfg.model,
        base_url: &cfg.base_url,
        system,
        title,
        content_id: token.content_id,
        content_revision: token.content_revision,
        input_body: body,
    })
}

/// 缓存命中判定：产物非空 **且** 存盘签名等于当前输入重算值。
/// 旧无签名（存量行）或签名不匹配（模型/提示词/正文修订/标题变化）一律未命中
/// ——产物保留可显示，但不得短路；成功生成即覆盖（产物与签名同条 UPDATE）。
fn signature_verified(
    cached: &Option<String>,
    stored_sig: &Option<String>,
    expected: &str,
) -> bool {
    let has_product = cached.as_deref().is_some_and(|s| !s.trim().is_empty());
    has_product && stored_sig.as_deref() == Some(expected)
}

/// 生成/提交共用的输入快照（同一把 DB 锁内一次性读取配置 + 文章输入 + 提交前提）。
struct AiInputSnapshot {
    cfg: AiConfigSnapshot,
    system: String,
    title: String,
    /// 输入正文（summary = body_text；translation = content_html）
    body: String,
    cached: Option<String>,
    cached_sig: Option<String>,
    token: db::AiCommitToken,
    /// 生成时算出的期望签名——提交时**原样落库**（与生成同一快照，不做二次读取）。
    signature: String,
}

impl AiInputSnapshot {
    /// 已验证缓存（命中则返回产物文本）。
    fn verified_cache(&self) -> Option<&str> {
        signature_verified(&self.cached, &self.cached_sig, &self.signature)
            .then(|| self.cached.as_deref().unwrap_or_default())
            .filter(|s| !s.trim().is_empty())
    }
}

/// 单把锁内读配置 + 提示词 + 文章输入 + 提交前提，并计算期望签名。
async fn load_ai_snapshot(
    state: &AppState,
    article_id: i64,
    kind: AiKind,
) -> AppResult<AiInputSnapshot> {
    let conn = state.db.lock().await;
    let raw = match db::get_setting(&conn, "ai_config") {
        Ok(value) => value,
        Err(err) => {
            log::warn!("ai: 读取 ai_config 失败: {err}");
            return Err(err);
        }
    };
    let cfg = AiConfigSnapshot::from_raw(raw.as_deref())?;
    let not_found = || AppError::not_found(format!("article {article_id}"));
    let (title, body, cached, cached_sig) = match kind {
        AiKind::Summary => {
            let (title, body, cached, sig) =
                db::get_article_for_summary(&conn, article_id)?.ok_or_else(not_found)?;
            (title, body, cached, sig)
        }
        AiKind::Translation => {
            let (title, body, cached, sig) =
                db::get_article_for_translation(&conn, article_id)?.ok_or_else(not_found)?;
            (title, body, cached, sig)
        }
    };
    let token = db::article_ai_commit_token(&conn, article_id)?.ok_or_else(not_found)?;
    let system = cfg.system_for(kind).to_string();
    let signature = expected_signature(&cfg.cfg, &system, kind, &title, &token, &body);
    Ok(AiInputSnapshot {
        cfg,
        system,
        title,
        body,
        cached,
        cached_sig,
        token,
        signature,
    })
}

/// AI 缓存校验响应（前端命中判定用；camelCase 与前端类型逐字对应）。
#[derive(serde::Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiCacheCheck {
    pub summary_verified: bool,
    pub translation_verified: bool,
}

/// AI 缓存校验（OPT-013 B1）：产物非空且签名匹配才算命中。
///
/// 前端无法自行计算签名（输入含模型/endpoint/提示词/正文哈希，只有后端在
/// 同一份配置快照下能算），因此**短路判定在动作时刻取后端权威结论**：对应项
/// 为 true 才允许展示既有产物作为已验证缓存；false（未生成/无签名/已过期）时
/// 前端走重新生成——旧产物保留显示（未标记版本），不冒充可复用缓存。
#[tauri::command]
pub async fn ai_cache_check(
    state: State<'_, AppState>,
    article_id: i64,
) -> AppResult<AiCacheCheck> {
    ai_cache_check_inner(&state, article_id).await
}

async fn ai_cache_check_inner(state: &AppState, article_id: i64) -> AppResult<AiCacheCheck> {
    let conn = state.db.lock().await;
    let raw = db::get_setting(&conn, "ai_config")?;
    let cfg = AiConfigSnapshot::from_raw(raw.as_deref())?;
    let not_found = || AppError::not_found(format!("article {article_id}"));
    let (title, body_text, summary, summary_sig) =
        db::get_article_for_summary(&conn, article_id)?.ok_or_else(not_found)?;
    let (_title, html, translated, translated_sig) =
        db::get_article_for_translation(&conn, article_id)?.ok_or_else(not_found)?;
    let token = db::article_ai_commit_token(&conn, article_id)?.ok_or_else(not_found)?;
    let summary_verified = signature_verified(
        &summary,
        &summary_sig,
        &expected_signature(
            &cfg.cfg,
            cfg.system_for(AiKind::Summary),
            AiKind::Summary,
            &title,
            &token,
            &body_text,
        ),
    );
    let translation_verified = signature_verified(
        &translated,
        &translated_sig,
        &expected_signature(
            &cfg.cfg,
            cfg.system_for(AiKind::Translation),
            AiKind::Translation,
            &title,
            &token,
            &html,
        ),
    );
    Ok(AiCacheCheck {
        summary_verified,
        translation_verified,
    })
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

/// 流式摘要：读文章 → 已有**已验证**缓存直接返回 → 否则调 AI 流式生成 → 落库。
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
    let snap = load_ai_snapshot(state, article_id, AiKind::Summary).await?;

    // 缓存命中：产物非空且签名匹配当前输入（模型/endpoint/提示词/标题/修订/
    // 正文/分段策略）→ 直接推给前端，不重算。无签名/签名不匹配（存量产物、
    // 换过模型或提示词、正文已更新）不短路：保留旧产物显示，走生成覆盖。
    if let Some(summary) = snap.verified_cache() {
        let _ = on_channel.send(AiEvent::Delta(summary.to_string()));
        let _ = on_channel.send(AiEvent::Done);
        return Ok(summary.to_string());
    }

    // P2-11（TASK-076，DEC-req104-p2-11-ai-validation-20260920）：空正文显式报错，
    // 与 ai_translate 的既有校验（"文章无正文可翻译"）对称。此前这里没有检查，
    // 空正文会照样发一次模型请求：白花 token，且 outcome.text 为空时不落缓存、
    // 用户只看到「摘要没出来」，无从判断是内容问题还是服务问题。
    if snap.body.trim().is_empty() {
        return Err(AppError::not_found("文章无正文可摘要"));
    }

    let user = format!("标题：{}\n\n正文：\n{}", snap.title, snap.body);
    let mut sink = |delta: &str| {
        // 前端关闭面板（Channel 被 drop）时 send 返回 Err → 返回 false 让
        // stream_chat 提前终止，不浪费 token、不落残缺产物（A-5）。
        on_channel.send(AiEvent::Delta(delta.to_string())).is_ok()
    };
    let outcome = crate::ai::stream_chat(
        &state.http,
        &snap.cfg.cfg,
        &snap.system,
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
            // OPT-008B 第二阶段：带前提校验——期间内容被替换/条目被清理 → 拒绝
            // 落旧产物（如实 Err，不写 Done）。
            // OPT-013 B1：签名与产物同一条 UPDATE；签名取生成时快照（不二次读配置）。
            commit_ai_product(
                &state.db,
                article_id,
                &snap.token,
                AiKind::Summary,
                &snap.signature,
                &outcome.text,
            )
            .await?;
        }
        let _ = on_channel.send(AiEvent::Done);
    }
    // completed=false（消费方取消）：不发 Done、不写缓存，返回已渲染文本。
    Ok(outcome.text)
}

/// 完整 AI 产物落缓存（带提交前提 + 缓存签名）：任一前提不符返回 Err（前端
/// 如实显示失败并可重试；绝不 Done 假成功——契约见
/// .agents/notes/implemented/architecture/2026-10-08-AI完成信号与缓存提交.md）。
/// 签名与产物在**同一条 UPDATE** 内写入（db::set_article_ai_fields_checked），
/// 不允许出现「新产物 + 旧签名」或「新签名 + 旧产物」。
async fn commit_ai_product(
    db_lock: &tokio::sync::Mutex<Connection>,
    article_id: i64,
    expected: &db::AiCommitToken,
    kind: AiKind,
    signature: &str,
    text: &str,
) -> AppResult<()> {
    let product = match kind {
        AiKind::Summary => db::AiProduct::Summary { text, signature },
        AiKind::Translation => db::AiProduct::Translation { text, signature },
    };
    let conn = db_lock.lock().await;
    let committed = db::set_article_ai_fields_checked(&conn, article_id, expected, product)?;
    if !committed {
        let what = match kind {
            AiKind::Summary => "摘要",
            AiKind::Translation => "译文",
        };
        return Err(AppError::new(
            "aiStale",
            format!("内容已在生成期间更新，已丢弃过期{what}（可重试）"),
        ));
    }
    Ok(())
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
    let snap = load_ai_snapshot(state, article_id, AiKind::Translation).await?;

    // 缓存命中（同 ai_summarize：产物非空且签名匹配）。
    if let Some(translated) = snap.verified_cache() {
        let _ = on_channel.send(AiEvent::Delta(translated.to_string()));
        let _ = on_channel.send(AiEvent::Done);
        return Ok(translated.to_string());
    }

    if snap.body.trim().is_empty() {
        return Err(AppError::not_found("文章无正文可翻译"));
    }

    let user = format!("标题：{}\n\nHTML：\n{}", snap.title, snap.body);
    let mut sink = |delta: &str| {
        // 前端关闭面板（Channel 被 drop）时 send 返回 Err → 返回 false 让
        // stream_chat 提前终止，不浪费 token、不落残缺产物（A-5）。
        on_channel.send(AiEvent::Delta(delta.to_string())).is_ok()
    };
    let outcome = crate::ai::stream_chat(
        &state.http,
        &snap.cfg.cfg,
        &snap.system,
        &user,
        &mut sink,
        crate::ai::TRANSLATE_MAX_TOKENS,
    )
    .await?;

    if outcome.completed {
        if !outcome.text.trim().is_empty() {
            // OPT-013A：同 ai_summarize——保存失败必须 Err，不得 Done 假成功。
            // OPT-008B 第二阶段：带前提校验（迟到译文不写已替换/已清理内容）。
            // 翻译产物是 HTML 且直接 dangerouslySetInnerHTML 渲染——入库前
            // 过消毒器（模型输出不可信：可能带 <script> 或被提示注入）。
            // 流式 delta 已发给前端（流中预览，未消毒）；落库的是消毒版。
            // 前端在流结束后回读 get_article 拿消毒版覆盖流式预览（见 store.ts
            // toggleReaderTranslation 的 onDone 回读逻辑）。
            let safe = crate::sanitize::sanitize(&outcome.text, None);
            commit_ai_product(
                &state.db,
                article_id,
                &snap.token,
                AiKind::Translation,
                &snap.signature,
                &safe,
            )
            .await?;
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

    /// 阻塞版 SSE mock（OPT-008B 第二阶段）：响应头发完后经 `received` 通知测试，
    /// 等待 `release` 才发送 body——制造可控的「模型在途窗口」（不靠 sleep 猜时序）。
    fn start_blocking_sse(
        body: &'static str,
        received: tokio::sync::mpsc::UnboundedSender<()>,
        release: std::sync::mpsc::Receiver<()>,
    ) -> u16 {
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
            let _ = received.send(());
            if release.recv().is_err() {
                return;
            }
            let _ = stream.write_all(body.as_bytes());
        });
        port
    }

    /// 建内存库（settings + articles/contents 最小 schema——
    /// OPT-008B 第二阶段：正文真值源 contents，条目经 content_id 引用）并构造
    /// AppState；`extra_sql` 用于注入触发器等故障。返回共享的事件类型表。
    fn setup(port: u16, extra_sql: &str) -> (AppState, Arc<StdMutex<Vec<String>>>) {
        let conn = Connection::open_in_memory().unwrap();
        let cfg = format!(
            r#"{{"preset":"custom","baseUrl":"http://127.0.0.1:{port}","apiKey":"sk-test","model":"m1"}}"#
        );
        conn.execute_batch(&format!(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE contents (id INTEGER PRIMARY KEY, identity TEXT NOT NULL UNIQUE,
                 content_html TEXT, body_text TEXT NOT NULL DEFAULT '', extraction_source TEXT NOT NULL DEFAULT 'rss',
                 fulltext_extracted INTEGER NOT NULL DEFAULT 0, revision INTEGER NOT NULL DEFAULT 1);
             CREATE TABLE articles (id INTEGER PRIMARY KEY, title TEXT NOT NULL, content_id INTEGER,
                 ai_summary TEXT, translated_content TEXT, ai_summary_sig TEXT, translated_sig TEXT,
                 ai_epoch INTEGER NOT NULL DEFAULT 0);
             INSERT INTO contents (id, identity, content_html, body_text)
                 VALUES (1, 'h|test', '<p>Hello World</p>', '这是正文内容。');
             INSERT INTO articles (id, title, content_id) VALUES (1, '测试文章', 1);
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

    fn query_summary_sig(conn: &Connection) -> Option<String> {
        conn.query_row(
            "SELECT ai_summary_sig FROM articles WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    /// 夹具当前输入下的期望签名（与生产同一定义；测试用来伪造/预置缓存）。
    fn fixture_sig(kind: AiKind, port: u16, model: &str, system: &str, body: &str) -> String {
        crate::ai::ai_cache_signature(&crate::ai::AiCacheInputs {
            kind: kind.id(),
            model,
            base_url: &format!("http://127.0.0.1:{port}"),
            system,
            title: "测试文章",
            content_id: Some(1),
            content_revision: 1,
            input_body: body,
        })
    }

    fn fixture_summary_sig(port: u16) -> String {
        fixture_sig(
            AiKind::Summary,
            port,
            "m1",
            DEFAULT_SUMMARIZE_SYSTEM,
            "这是正文内容。",
        )
    }

    fn fixture_translation_sig(port: u16) -> String {
        fixture_sig(
            AiKind::Translation,
            port,
            "m1",
            DEFAULT_TRANSLATE_SYSTEM,
            "<p>Hello World</p>",
        )
    }

    /// 计数版成功 SSE mock：记录入站连接数并回完整流——「未短路 → 真的发起
    /// 模型请求」的行为证据（缓存命中用例要求计数为 0）。
    fn start_counting_ok_sse(body: &'static str, counter: Arc<StdMutex<usize>>) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let Some(Ok(mut stream)) = listener.incoming().next() else {
                return;
            };
            *counter.lock().unwrap() += 1;
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

    /// 缓存命中（OPT-013 B1 语义）：产物非空**且签名匹配当前输入**——
    /// delta(缓存)+Done，且完全不发起网络请求。
    #[tokio::test]
    async fn cache_hit_requires_matching_signature_and_avoids_network() {
        let counter = Arc::new(StdMutex::new(0usize));
        let port = start_counting_ok_sse(COMPLETE_SSE, counter.clone());
        let sig = fixture_summary_sig(port);
        let (state, events) = setup(
            port,
            &format!(
                "UPDATE articles SET ai_summary = '已缓存的摘要', ai_summary_sig = '{sig}' WHERE id = 1;"
            ),
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

    /// OPT-013 B1 反例 1（后端）：**旧签名**（此前模型/提示词下生成的产物）不得
    /// 短路——命中要求签名等于当前输入重算值。判别力：若仍按「非空即缓存」
    /// 短路，计数为 0 且产物保持「旧模型的摘要」，断言即红。
    #[tokio::test]
    async fn stale_signature_product_is_not_cache_and_regenerates() {
        let counter = Arc::new(StdMutex::new(0usize));
        let port = start_counting_ok_sse(COMPLETE_SSE, counter.clone());
        // 旧产物：当时模型 m0、旧提示词「旧提示词」生成的签名（与当前配置不符）
        let old_sig = fixture_sig(AiKind::Summary, port, "m0", "旧提示词", "这是正文内容。");
        let (state, events) = setup(
            port,
            &format!(
                "UPDATE articles SET ai_summary = '旧模型的摘要', ai_summary_sig = '{old_sig}' WHERE id = 1;"
            ),
        );
        let channel = recording_channel(events.clone());
        let text = ai_summarize_inner(&state, 1, &channel).await.unwrap();
        assert_eq!(text, "要点一", "旧签名不短路：真的重新生成");
        assert_eq!(
            *counter.lock().unwrap(),
            1,
            "旧模型/旧提示词签名不匹配 → 必须发起模型请求"
        );
        let conn = state.db.lock().await;
        assert_eq!(
            query_summary(&conn).as_deref(),
            Some("要点一"),
            "新产物覆盖旧产物"
        );
        assert_eq!(
            query_summary_sig(&conn).as_deref(),
            Some(fixture_summary_sig(port).as_str()),
            "产物与当前签名同时更新"
        );
    }

    /// OPT-013 B1 反例 2（后端）：旧**无签名**产物保留可见、不短路，重新生成后
    /// 签名与产物同时更新（同一条 UPDATE）。
    #[tokio::test]
    async fn unsigned_legacy_product_regenerates_and_gains_signature() {
        let counter = Arc::new(StdMutex::new(0usize));
        let port = start_counting_ok_sse(COMPLETE_SSE, counter.clone());
        let (state, events) = setup(
            port,
            "UPDATE articles SET ai_summary = '存量无签名摘要' WHERE id = 1;",
        );
        {
            // 前置（反例 2 的「可见」）：无签名产物仍在行里（数据保留、可显示）
            let conn = state.db.lock().await;
            assert_eq!(query_summary(&conn).as_deref(), Some("存量无签名摘要"));
            assert_eq!(query_summary_sig(&conn), None, "存量行无签名");
        }
        let channel = recording_channel(events.clone());
        let text = ai_summarize_inner(&state, 1, &channel).await.unwrap();
        assert_eq!(text, "要点一", "无签名不短路：重新生成");
        assert_eq!(*counter.lock().unwrap(), 1);
        let conn = state.db.lock().await;
        assert_eq!(query_summary(&conn).as_deref(), Some("要点一"));
        assert_eq!(
            query_summary_sig(&conn).as_deref(),
            Some(fixture_summary_sig(port).as_str()),
            "重新生成后签名与产物同时更新"
        );
    }

    /// OPT-013 B1 反例 4：生成期间 `ai_epoch` 前进（清缓存）→ 提交被拒、无 Done、
    /// 产物与签名都不得写入（同一条 UPDATE 整体失败）。
    #[tokio::test]
    async fn ai_epoch_advance_during_generation_rejects_commit() {
        let (received_tx, mut received_rx) = tokio::sync::mpsc::unbounded_channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let port = start_blocking_sse(COMPLETE_SSE, received_tx, release_rx);
        let (state, events) = setup(port, "");
        let channel = recording_channel(events.clone());

        let mutate = async {
            tokio::time::timeout(std::time::Duration::from_secs(5), received_rx.recv())
                .await
                .expect("mock 必须收到模型请求")
                .expect("received 通道未关闭");
            // 模型在途：模拟 cache_cleanup(scope='ai')——清产物 + 推进清除代次
            let conn = tokio::time::timeout(std::time::Duration::from_secs(2), state.db.lock())
                .await
                .expect("网络期间不得持有 DB 锁");
            conn.execute(
                "UPDATE articles SET ai_summary = NULL, translated_content = NULL,
                        ai_summary_sig = NULL, translated_sig = NULL, ai_epoch = ai_epoch + 1
                 WHERE id = 1",
                [],
            )
            .unwrap();
            drop(conn);
            release_tx.send(()).unwrap();
        };
        let (res, _) = tokio::join!(ai_summarize_inner(&state, 1, &channel), mutate);
        let err = res.expect_err("清缓存后的迟到提交必须拒绝");
        assert_eq!(err.code, "aiStale", "{err}");
        let conn = state.db.lock().await;
        assert_eq!(query_summary(&conn), None, "拒绝后不得写产物");
        assert_eq!(query_summary_sig(&conn), None, "拒绝后不得半写签名");
        drop(conn);
        let types = events.lock().unwrap().clone();
        assert_eq!(types, vec!["delta"], "拒绝后不得发送 Done: {types:?}");
    }

    /// OPT-013 B1：生成期间**标题**变化（签名输入之一）→ 提交被拒——
    /// 同一条 UPDATE 的 WHERE 复核标题前提；否则会落「新产物 + 旧标题的签名」，
    /// 之后按当前标题重算签名必然不匹配（产物明明刚生成却是「过期版本」）。
    #[tokio::test]
    async fn title_change_during_generation_rejects_commit() {
        let (received_tx, mut received_rx) = tokio::sync::mpsc::unbounded_channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let port = start_blocking_sse(COMPLETE_SSE, received_tx, release_rx);
        let (state, events) = setup(port, "");
        let channel = recording_channel(events.clone());

        let mutate = async {
            tokio::time::timeout(std::time::Duration::from_secs(5), received_rx.recv())
                .await
                .expect("mock 必须收到模型请求")
                .expect("received 通道未关闭");
            let conn = tokio::time::timeout(std::time::Duration::from_secs(2), state.db.lock())
                .await
                .expect("网络期间不得持有 DB 锁");
            conn.execute(
                "UPDATE articles SET title = '改名后的标题' WHERE id = 1",
                [],
            )
            .unwrap();
            drop(conn);
            release_tx.send(()).unwrap();
        };
        let (res, _) = tokio::join!(ai_summarize_inner(&state, 1, &channel), mutate);
        let err = res.expect_err("标题变化后的迟到提交必须拒绝");
        assert_eq!(err.code, "aiStale", "{err}");
        let conn = state.db.lock().await;
        assert_eq!(query_summary(&conn), None, "拒绝后不得写产物");
        assert_eq!(query_summary_sig(&conn), None, "拒绝后不得半写签名");
        drop(conn);
        let types = events.lock().unwrap().clone();
        assert_eq!(types, vec!["delta"], "拒绝后不得发送 Done: {types:?}");
    }

    /// OPT-013 B1：缓存校验（前端命中判定的后端权威）——仅「产物非空且签名
    /// 匹配」为 true；无产物/无签名/签名不匹配（含换模型）都为 false。
    #[tokio::test]
    async fn ai_cache_check_reports_verified_only_on_signature_match() {
        // 端口 0：本命令只读库，不发起任何网络请求
        let (state, _events) = setup(0, "");
        assert_eq!(
            ai_cache_check_inner(&state, 1).await.unwrap(),
            AiCacheCheck {
                summary_verified: false,
                translation_verified: false
            },
            "无产物 = 未命中"
        );
        {
            let conn = state.db.lock().await;
            conn.execute(
                "UPDATE articles SET ai_summary = '摘要', ai_summary_sig = 'sig-x' WHERE id = 1",
                [],
            )
            .unwrap();
        }
        assert!(
            !ai_cache_check_inner(&state, 1)
                .await
                .unwrap()
                .summary_verified,
            "签名不匹配（伪造/旧签名）= 未命中"
        );
        {
            let conn = state.db.lock().await;
            let sig = fixture_summary_sig(0);
            let tr_sig = fixture_translation_sig(0);
            conn.execute(
                "UPDATE articles SET ai_summary_sig = ?1, translated_content = '<p>译文</p>', translated_sig = ?2 WHERE id = 1",
                rusqlite::params![sig, tr_sig],
            )
            .unwrap();
        }
        assert_eq!(
            ai_cache_check_inner(&state, 1).await.unwrap(),
            AiCacheCheck {
                summary_verified: true,
                translation_verified: true
            },
            "签名匹配 = 命中（两类产物各自独立判定）"
        );
        // 换模型：同一存盘签名不再匹配当前输入 → 两类都未命中
        {
            let conn = state.db.lock().await;
            conn.execute(
                "UPDATE settings SET value = '{\"preset\":\"custom\",\"baseUrl\":\"http://127.0.0.1:0\",\"apiKey\":\"sk-test\",\"model\":\"m2\"}' WHERE key = 'ai_config'",
                [],
            )
            .unwrap();
        }
        assert_eq!(
            ai_cache_check_inner(&state, 1).await.unwrap(),
            AiCacheCheck {
                summary_verified: false,
                translation_verified: false
            },
            "模型变化后旧签名全部失效"
        );
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

    /// OPT-008B 第二阶段：模型在途窗口内 (a) DB 锁必须空闲（不持锁跑网络），
    /// (b) 期间内容修订被推进 → 迟到产物提交被拒（aiStale，不写缓存、不发 Done）。
    /// 判别力：若窗口内持锁，`state.db.lock()` 超时即红；若守卫失效，提交成功 →
    /// 无 Err 且缓存被写，断言即红。
    #[tokio::test]
    async fn stale_product_after_content_change_is_rejected_and_lock_free_during_network() {
        let (received_tx, mut received_rx) = tokio::sync::mpsc::unbounded_channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let port = start_blocking_sse(COMPLETE_SSE, received_tx, release_rx);
        let (state, events) = setup(port, "");
        let channel = recording_channel(events.clone());

        let mutate = async {
            tokio::time::timeout(std::time::Duration::from_secs(5), received_rx.recv())
                .await
                .expect("mock 必须收到模型请求")
                .expect("received 通道未关闭");
            // 模型在途：DB 锁必须仍可获取（获取超时即证明被网络路径持有）
            let conn = tokio::time::timeout(std::time::Duration::from_secs(2), state.db.lock())
                .await
                .expect("网络期间不得持有 DB 锁");
            conn.execute(
                "UPDATE contents SET revision = revision + 1 WHERE id = 1",
                [],
            )
            .unwrap();
            drop(conn);
            release_tx.send(()).unwrap();
        };
        let (res, _) = tokio::join!(ai_summarize_inner(&state, 1, &channel), mutate);
        let err = res.expect_err("内容修订变化后的迟到提交必须拒绝");
        assert_eq!(err.code, "aiStale", "{err}");
        let conn = state.db.lock().await;
        assert_eq!(query_summary(&conn).as_deref(), None, "拒绝后不得写缓存");
        drop(conn);
        let types = events.lock().unwrap().clone();
        assert_eq!(types, vec!["delta"], "拒绝后不得发送 Done: {types:?}");
    }
}
