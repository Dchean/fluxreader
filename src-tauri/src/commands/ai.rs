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
    /// OPT-013 B2：翻译分段计划（前端「继续 / 整篇重新生成」最小提示的数据源）。
    pub translation_segments: TranslationSegmentPlan,
}

/// 翻译分段计划：当前输入下的总段数、当前签名下已完成的段数、因签名变化
/// （模型/提示词/正文修订/分段策略）而**作废**的旧片段数。
#[derive(serde::Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TranslationSegmentPlan {
    pub total: usize,
    pub completed: usize,
    pub discarded: usize,
}

/// 计算翻译分段计划（同一把锁内；`translation_signature` 与命中判定同一份）。
fn translation_segment_plan(
    conn: &Connection,
    article_id: i64,
    html: &str,
    translation_signature: &str,
) -> AppResult<TranslationSegmentPlan> {
    // 空正文（无输入）→ 0 段；其余情况精确分段（与生成路径同一函数，段数一致）
    let total = if html.trim().is_empty() {
        0
    } else {
        crate::ai_segments::split_html_segments(html, crate::ai::TRANSLATE_SEGMENT_MAX_BYTES).len()
    };
    let (completed, discarded) = db::ai_segment_stats(
        conn,
        article_id,
        db::AI_PRODUCT_TRANSLATION,
        translation_signature,
    )?;
    Ok(TranslationSegmentPlan {
        total,
        completed: completed.min(total),
        discarded,
    })
}

/// AI 缓存校验（OPT-013 B1）：产物非空且签名匹配才算命中。
///
/// 前端无法自行计算签名（输入含模型/endpoint/提示词/正文哈希，只有后端在
/// 同一份配置快照下能算），因此**短路判定在动作时刻取后端权威结论**：对应项
/// 为 true 才允许展示既有产物作为已验证缓存；false（未生成/无签名/已过期）时
/// 前端走重新生成——旧产物保留显示（未标记版本），不冒充可复用缓存。
///
/// OPT-013 B2：同一响应携带翻译分段计划——`discarded > 0` 表示存在因输入
/// 变化作废的旧片段（前端必须如实告知）；但**不**一律等于整篇重新生成：
/// `0 < completed < total` 时复用当前签名已完成段、从第 `completed + 1` 段
/// 继续（只重发未完成段；`completed == total` 时零重发、直接提交——文案的
/// 精确收口归 B3；R2 复审口径收紧）。
#[tauri::command]
pub async fn ai_cache_check(
    state: State<'_, AppState>,
    article_id: i64,
) -> AppResult<AiCacheCheck> {
    ai_cache_check_inner(&state, article_id).await
}

/// OPT-013 B3：按「条目 × 产物」清除 AI 缓存（真实消费者「清除」入口的后端接线）。
///
/// 删除该条目该产物在**所有签名**下的持久化片段（后台恢复记录）与整篇产物/
/// 签名，并推进清除代次（在途迟到提交被拒）。清除后该条目回到「无产物/未生成」
/// ——不可能再显示 partial，也不可能被恢复复用（零恢复）。实现体拆出供模块
/// 测试直接注入 AppState（与 ai_*_inner 同口径）。
#[tauri::command]
pub async fn ai_clear_cache(
    state: State<'_, AppState>,
    article_id: i64,
    product: String,
) -> AppResult<()> {
    ai_clear_cache_inner(&state, article_id, &product).await
}

/// ai_clear_cache 的实现体（product：`"summary"` | `"translation"`，其余值显式
/// validate 失败；条目不存在 → not_found——不清不静默成功）。
async fn ai_clear_cache_inner(state: &AppState, article_id: i64, product: &str) -> AppResult<()> {
    let mut conn = state.db.lock().await;
    let cleared = db::clear_article_ai_product(&mut conn, article_id, product)?;
    if !cleared {
        return Err(AppError::not_found(format!("article {article_id}")));
    }
    Ok(())
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
    let translation_signature = expected_signature(
        &cfg.cfg,
        cfg.system_for(AiKind::Translation),
        AiKind::Translation,
        &title,
        &token,
        &html,
    );
    let translation_verified =
        signature_verified(&translated, &translated_sig, &translation_signature);
    let translation_segments =
        translation_segment_plan(&conn, article_id, &html, &translation_signature)?;
    Ok(AiCacheCheck {
        summary_verified,
        translation_verified,
        translation_segments,
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

    // OPT-013 B2：摘要长输入策略（定版 §3）——超预算显式失败，不静默截掉
    // 后半篇、不假称完整；策略与上限依据见 ai::SUMMARY_INPUT_MAX_BYTES 注释
    // 与 Note「OPT-013 B2 追加」。分段摘要 + 合并（二次模型调用）不在本卡范围。
    if snap.body.len() > crate::ai::SUMMARY_INPUT_MAX_BYTES {
        return Err(AppError::new(
            "aiTooLong",
            format!(
                "文章正文 {} 字节，超过单次摘要预算 {} 字节：未生成摘要（不截断、不假称完整）",
                snap.body.len(),
                crate::ai::SUMMARY_INPUT_MAX_BYTES
            ),
        ));
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

/// 翻译的分段用户消息（第 i/N 段）。单段（未分段）时与 B1 同形（一次请求）；
/// 发送文本是分段器输出——规范序列化输入下逐字等于原文，非规范输入先经
/// html5ever 规范化（R1 复审 P3-3，见 ai_segments 模块头注）；缓存签名仍绑定
/// 原始正文，缓存身份不受规范化影响。
fn segment_user_message(title: &str, segment: &str, index: usize, total: usize) -> String {
    if total <= 1 {
        format!("标题：{title}\n\nHTML：\n{segment}")
    } else {
        format!(
            "标题：{title}\n\nHTML（第 {}/{} 段，请只翻译本段并保持标签与结构）：\n{segment}",
            index + 1,
            total
        )
    }
}

/// ai_translate 的实现体（与 State/命令宏解耦，供模块测试注入）。
///
/// OPT-013 B2（定版 §3）：超长正文按 HTML 边界分段（`ai_segments`，确定性），
/// **逐段生成、逐段持久化**（绑定同一缓存签名）——任一时刻崩溃/失败后重启，
/// 只重发未完成段；失败段重试按序号覆盖，不追加重复文本。只有全部片段完成才
/// 拼出整篇并经 `set_article_ai_fields_checked` 提交（部分态绝不冒充完整缓存）。
/// 每段仍走 013A 的 `stream_chat`：完成语义、length/content_filter/取消分型、
/// 预算与流式显示全部不变（delta 直接转发给前端）。
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

    let segments =
        crate::ai_segments::split_html_segments(&snap.body, crate::ai::TRANSLATE_SEGMENT_MAX_BYTES);
    let total = segments.len();
    // 同一签名下已完成的片段（恢复依据）：只重发缺失段，已完成段直接复用。
    let completed: std::collections::HashMap<i64, String> = {
        let conn = state.db.lock().await;
        db::list_ai_segments(
            &conn,
            article_id,
            db::AI_PRODUCT_TRANSLATION,
            &snap.signature,
        )?
        .into_iter()
        .collect()
    };

    let mut parts: Vec<String> = Vec::with_capacity(total);
    for (index, segment) in segments.iter().enumerate() {
        let key = index as i64;
        if let Some(done) = completed.get(&key) {
            parts.push(done.clone());
            continue;
        }
        let user = segment_user_message(&snap.title, segment, index, total);
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
        if !outcome.completed {
            // 消费方取消（013A）：不发 Done、不写缓存；本段不落片段（已完成
            // 段保留，重试从本段继续）。返回已渲染文本。
            return Ok(outcome.text);
        }
        let safe = crate::sanitize::sanitize(&outcome.text, None);
        if safe.trim().is_empty() {
            if total > 1 {
                // 多段路径的空段产物 = 整篇不完整：显式失败（不静默跳过、不假称
                // 完整，也不写入空段）；已完成段保留，重试从本段继续。
                return Err(AppError::new(
                    "aiIncomplete",
                    format!(
                        "模型对第 {}/{} 段返回空内容：未形成完整译文（不写缓存，可重试）",
                        index + 1,
                        total
                    ),
                ));
            }
            // 未分段（单段）路径保持 013A/B1 语义：空文本不落缓存、照发 Done，
            // 也不写入空产物/空段。
            let _ = on_channel.send(AiEvent::Done);
            return Ok(outcome.text);
        }
        {
            let conn = state.db.lock().await;
            // OPT-013 B3：片段写入带清除代次前提——生成在途时用户清除（ai_epoch
            // 前进）→ 拒绝写入并立即中止（迟到的片段不得在清除之后复活恢复记录，
            // 否则「清除后零恢复」被破坏、UI 可能重新显示 partial）。
            let written = db::upsert_ai_segment(
                &conn,
                article_id,
                db::AI_PRODUCT_TRANSLATION,
                &snap.signature,
                key,
                &safe,
                snap.token.ai_epoch,
            )?;
            if !written {
                return Err(AppError::new(
                    "aiStale",
                    format!(
                        "内容已在生成期间被清除，已丢弃过期译文（第 {}/{} 段后中止，可重试）",
                        index + 1,
                        total
                    ),
                ));
            }
        }
        parts.push(safe);
    }
    debug_assert_eq!(parts.len(), total, "全部片段就位后才允许拼整篇");
    let full = parts.concat();

    // OPT-013A：同 ai_summarize——保存失败必须 Err，不得 Done 假成功。
    // OPT-008B 第二阶段：带前提校验（迟到译文不写已替换/已清理内容）。
    // 翻译产物是 HTML 且直接 dangerouslySetInnerHTML 渲染——入库前过消毒器
    // （模型输出不可信：可能带 <script> 或被提示注入；片段落库已逐段消毒，
    // 整篇再消毒一次保持既有契约）。流式 delta 已发给前端（未消毒预览）；
    // 前端在流结束后回读 get_article 拿消毒版覆盖（见 slices/ai.ts 的回读）。
    let safe_full = crate::sanitize::sanitize(&full, None);
    // R1 复审 P2-1：退化输入守卫——`body.trim()` 非空但分段解析未产出任何内容
    // 节点（如 "<html></html>" 这类容器标签被解析器吸收）时 `total == 0`，空拼串
    // 会以有效签名提交空产物并在后面发 Done（假完整旁路；单段/多段路径各自的
    // 空产物守卫都覆盖不到它）。「无有效内容」必须显式失败：不写缓存、不发 Done。
    // `safe_full` 空守卫为同族防御（全部分段的产物消毒后为空时同样不得提交）。
    if total == 0 || safe_full.trim().is_empty() {
        return Err(AppError::new(
            "aiIncomplete",
            "正文没有可翻译的有效内容（分段后为空）：未形成译文（不写缓存，可重试）",
        ));
    }
    commit_ai_product(
        &state.db,
        article_id,
        &snap.token,
        AiKind::Translation,
        &snap.signature,
        &safe_full,
    )
    .await?;
    // 整篇产物已成立：清掉其他签名的旧片段（输入已变，旧进度不再有恢复价值；
    // 当前签名的片段保留——下次清理/再来生成时按需复用或随清除一起删除）。
    // R1 复审 P3-1：这是提交成功后的 best-effort 维护——失败只记日志，不得把
    // 已成事实的成功（产物+签名已落库、Done 待发）倒挂成 Err（修前 `?` 会在
    // 清理故障时返回 Err 且不发 Done，而产物其实已提交，前端显示失败、下次命中
    // 缓存自愈，前后端状态互相矛盾）。残留旧片段不影响正确性：读段只按同一签名。
    {
        let conn = state.db.lock().await;
        if let Err(err) = db::delete_stale_ai_segments(
            &conn,
            article_id,
            db::AI_PRODUCT_TRANSLATION,
            &snap.signature,
        ) {
            log::warn!("ai: 提交成功后清理旧签名片段失败（忽略，不影响本笔成功）: {err}");
        }
    }
    let _ = on_channel.send(AiEvent::Done);
    Ok(full)
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
             CREATE TABLE ai_segments (article_id INTEGER NOT NULL, product TEXT NOT NULL,
                 signature TEXT NOT NULL, segment_index INTEGER NOT NULL, content TEXT NOT NULL,
                 created_at TEXT NOT NULL DEFAULT (datetime('now')),
                 PRIMARY KEY (article_id, product, signature, segment_index));
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

    /// OPT-013 B3：清除命令（ai_clear_cache）在生成在途时也必须拒绝迟到写入——
    /// 清除推进 ai_epoch，迟到的片段写入与整篇提交都进不去（aiStale、无产物/
    /// 签名/片段、不发 Done）。判别力：若片段写入不带代次前提，迟到的完成段会
    /// 在清除**之后**复活（segments 非空 → 重启可恢复），「清除后零恢复」被破坏。
    #[tokio::test]
    async fn clear_during_generation_rejects_late_segment_and_commit() {
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
            // 模型在途：用户执行清除（真实命令路径）
            ai_clear_cache_inner(&state, 1, "translation")
                .await
                .expect("清除命令必须成功");
            release_tx.send(()).unwrap();
        };
        let (res, _) = tokio::join!(ai_translate_inner(&state, 1, &channel), mutate);
        let err = res.expect_err("清除后的迟到写入必须拒绝");
        assert_eq!(err.code, "aiStale", "{err}");
        let conn = state.db.lock().await;
        let translated: Option<String> = conn
            .query_row(
                "SELECT translated_content FROM articles WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(translated, None, "拒绝后不得写产物");
        let sig: Option<String> = conn
            .query_row(
                "SELECT translated_sig FROM articles WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sig, None, "拒绝后不得半写签名");
        let segs: i64 = conn
            .query_row("SELECT COUNT(*) FROM ai_segments", [], |r| r.get(0))
            .unwrap();
        drop(conn);
        assert_eq!(segs, 0, "迟到片段不得在清除之后复活（零恢复）");
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

    /// OPT-013 B1/B2：缓存校验（前端命中判定的后端权威）——仅「产物非空且签名
    /// 匹配」为 true；无产物/无签名/签名不匹配（含换模型）都为 false。
    /// B2 追加：同一响应携带翻译分段计划（total/completed/discarded）。
    #[tokio::test]
    async fn ai_cache_check_reports_verified_only_on_signature_match() {
        // 端口 0：本命令只读库，不发起任何网络请求
        let (state, _events) = setup(0, "");
        let none = ai_cache_check_inner(&state, 1).await.unwrap();
        assert!(
            !none.summary_verified && !none.translation_verified,
            "无产物 = 未命中"
        );
        assert_eq!(
            none.translation_segments,
            TranslationSegmentPlan {
                total: 1,
                completed: 0,
                discarded: 0
            },
            "短文单段：计划 total=1、无已完成/作废片段"
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
        let hit = ai_cache_check_inner(&state, 1).await.unwrap();
        assert!(
            hit.summary_verified && hit.translation_verified,
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
        let stale = ai_cache_check_inner(&state, 1).await.unwrap();
        assert!(
            !stale.summary_verified && !stale.translation_verified,
            "模型变化后旧签名全部失效"
        );
        assert_eq!(
            stale.translation_segments,
            TranslationSegmentPlan {
                total: 1,
                completed: 0,
                discarded: 0
            },
            "换模型只影响片段签名的匹配（本例无片段，计划不变）"
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

/// OPT-013 B2 命令层测试：长文分段发送、片段持久化与恢复（真实 loopback mock）。
///
/// 证据口径：记录型 SSE mock 逐请求记录**实际发送的请求体**（含分段正文），
/// 断言发送的分段序列、恢复时的「只重发未完成段」与拼接无重复；「重启」用
/// 文件库 + 新连接 + 新 AppState 真实模拟（片段必须来自 DB，不能靠内存）。
/// 决定与备选见 .agents/notes/implemented/architecture/2026-10-08-AI完成信号与缓存提交.md
/// （OPT-013 B2 追加）。
#[cfg(test)]
mod ai_segment_tests {
    use super::*;
    use crate::ai_segments::unbalanced_tags;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex as StdMutex};
    use tauri::ipc::{Channel, InvokeResponseBody};

    /// 逐请求的脚本化回应（超出脚本一律默认成功文本）。
    #[derive(Clone)]
    enum Reply {
        /// 完整流（stop + [DONE]），内容 = 给定文本
        Text(String),
        /// finish_reason=length → aiTruncated
        Length,
        /// finish_reason=content_filter → aiFiltered
        Filtered,
        /// HTTP 500 → aiStream
        Http500,
    }

    impl Reply {
        fn default_ok() -> Self {
            Reply::Text("<p>默认译文</p>".into())
        }

        fn sse_body(&self) -> String {
            match self {
                Reply::Text(text) => {
                    let escaped = serde_json::to_string(text).unwrap();
                    format!(
                        "data: {{\"choices\":[{{\"delta\":{{\"content\":{escaped}}}}}]}}\r\n\r\n\
                         data: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"stop\"}}]}}\r\n\r\n\
                         data: [DONE]\r\n\r\n"
                    )
                }
                Reply::Length => concat!(
                    "data: {\"choices\":[{\"delta\":{\"content\":\"半截\"}}]}\r\n\r\n",
                    "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\r\n\r\n"
                )
                .to_string(),
                Reply::Filtered => concat!(
                    "data: {\"choices\":[{\"delta\":{\"content\":\"半截\"}}]}\r\n\r\n",
                    "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"content_filter\"}]}\r\n\r\n"
                )
                .to_string(),
                Reply::Http500 => String::new(),
            }
        }
    }

    /// 记录型多连接 SSE mock：逐连接**完整读取**请求（按 Content-Length 读 body）
    /// 并记录，然后按请求序号从 `script` 取回应。返回 (端口, 请求文本表)。
    fn start_recording_sse(script: Vec<Reply>) -> (u16, Arc<StdMutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(StdMutex::new(Vec::new()));
        let requests_for_thread = requests.clone();
        std::thread::spawn(move || {
            let max_connections = script.len() + 8;
            for (idx, stream) in listener.incoming().take(max_connections).enumerate() {
                let Ok(mut stream) = stream else { return };
                let mut req = Vec::new();
                let mut buf = [0u8; 4096];
                let header_end = loop {
                    let n = stream.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    req.extend_from_slice(&buf[..n]);
                    if let Some(pos) = req.windows(4).position(|w| w == b"\r\n\r\n") {
                        break pos + 4;
                    }
                };
                let head = String::from_utf8_lossy(&req[..header_end]).to_string();
                let content_length = head
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.eq_ignore_ascii_case("content-length")
                            .then(|| v.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                while req.len() < header_end + content_length {
                    let n = stream.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    req.extend_from_slice(&buf[..n]);
                }
                requests_for_thread
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&req).to_string());
                let reply = script.get(idx).cloned().unwrap_or_else(Reply::default_ok);
                let resp = match reply {
                    Reply::Http500 => {
                        "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_string()
                    }
                    other => {
                        let body = other.sse_body();
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                    }
                };
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        (port, requests)
    }

    /// 从记录的请求文本取 JSON body。
    fn request_body(req: &str) -> serde_json::Value {
        let body = req
            .split_once("\r\n\r\n")
            .map(|(_, b)| b)
            .unwrap_or_default();
        serde_json::from_str(body).unwrap_or_else(|e| panic!("请求体不是合法 JSON: {e}\n{body}"))
    }

    /// 从请求里取出实际发送的 HTML 段（剥离标题/段头外框）。
    fn sent_segment(req: &str) -> String {
        let user = request_body(req)["messages"][1]["content"]
            .as_str()
            .unwrap_or_else(|| panic!("请求缺少 user 消息"))
            .to_string();
        if let Some((_, seg)) = user.split_once("）：\n") {
            return seg.to_string();
        }
        user.split_once("HTML：\n")
            .map(|(_, seg)| seg.to_string())
            .unwrap_or_else(|| panic!("user 消息不含 HTML 段：{user}"))
    }

    /// 记录事件类型的测试 Channel（与 B1 测试同口径）。
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

    /// 临时文件库（迁移建库；进程重启语义 = 重新 open 同一路径）。
    struct TempAiDb {
        path: PathBuf,
    }

    impl TempAiDb {
        fn new(tag: &str) -> Self {
            static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!(
                "fluxreader-opt013b2-{tag}-{}-{seq}.db",
                std::process::id()
            ));
            let _ = std::fs::remove_file(&path);
            Self { path }
        }
    }

    impl Drop for TempAiDb {
        fn drop(&mut self) {
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{suffix}", self.path.display()));
            }
        }
    }

    /// 迁移建库 + 最小条目夹具（真实 schema，含 v28 片段表与 FK 级联）。
    fn setup_migrated(path: &Path, port: u16, html: &str, body_text: &str) -> AppState {
        let conn = db::open(path).expect("迁移建库");
        seed_fixture(&conn, port, html, body_text);
        AppState::new(
            conn,
            crate::ingestion::build_client(30),
            crate::media::MediaHandle::inactive(),
        )
    }

    /// 「重启」：同一文件库的新连接 + 新 AppState（片段只能来自 DB）。
    /// 不改 `ai_config`——签名含 base_url/模型，重启本身不改变配置（否则
    /// 测的就不是「恢复」而是「签名变化」）。
    fn reopen_migrated(path: &Path) -> AppState {
        let conn = db::open(path).expect("重新打开库");
        AppState::new(
            conn,
            crate::ingestion::build_client(30),
            crate::media::MediaHandle::inactive(),
        )
    }

    fn update_ai_config(conn: &Connection, port: u16) {
        let cfg = format!(
            r#"{{"preset":"custom","baseUrl":"http://127.0.0.1:{port}","apiKey":"sk-test","model":"m1"}}"#
        );
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES ('ai_config', ?1)",
            rusqlite::params![cfg],
        )
        .unwrap();
    }

    fn seed_fixture(conn: &Connection, port: u16, html: &str, body_text: &str) {
        conn.execute(
            "INSERT INTO folders (id, name, layout) VALUES (1, 'F', 'article')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO feeds (id, folder_id, feed_url, title) VALUES (1, 1, 'https://a.example/rss', 'A')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO contents (id, identity, content_html, body_text) VALUES (1, 'h|seg', ?1, ?2)",
            rusqlite::params![html, body_text],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO articles (id, feed_id, guid, title, content_id) VALUES (1, 1, 'g1', '测试文章', 1)",
            [],
        )
        .unwrap();
        update_ai_config(conn, port);
    }

    /// 确定性长正文：n 个段落，每段带唯一标记（用于断言发送顺序与拼接）。
    fn long_html(paragraphs: usize) -> String {
        (0..paragraphs)
            .map(|i| format!("<p>段落{i}标记{}内容</p>", "字".repeat(180)))
            .collect()
    }

    async fn translated_of(state: &AppState) -> Option<String> {
        let conn = state.db.lock().await;
        conn.query_row(
            "SELECT translated_content FROM articles WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    async fn translated_sig_of(state: &AppState) -> Option<String> {
        let conn = state.db.lock().await;
        conn.query_row(
            "SELECT translated_sig FROM articles WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    /// 片段快照（签名, 序号, 内容），按签名 + 序号排序。
    async fn segments_snapshot(state: &AppState) -> Vec<(String, i64, String)> {
        let conn = state.db.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT signature, segment_index, content FROM ai_segments
                 WHERE article_id = 1 ORDER BY signature, segment_index",
            )
            .unwrap();
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .unwrap();
        rows.collect::<Result<Vec<_>, _>>().unwrap()
    }

    async fn clear_full_product(state: &AppState) {
        let conn = state.db.lock().await;
        conn.execute(
            "UPDATE articles SET translated_content = NULL, translated_sig = NULL WHERE id = 1",
            [],
        )
        .unwrap();
        conn.execute("DELETE FROM ai_segments", []).unwrap();
    }

    fn expected_segments(html: &str) -> Vec<String> {
        crate::ai_segments::split_html_segments(html, crate::ai::TRANSLATE_SEGMENT_MAX_BYTES)
    }

    /// 反例 1（命令层，真实 wire）：超长 HTML 分段后，实际发送的多段输入 =
    /// 分段器输出（顺序稳定），每段都是标签配对的合法 HTML 片段；整篇产物 =
    /// 各段回应按序拼接。
    #[tokio::test]
    async fn long_article_is_sent_in_ordered_balanced_segments() {
        let html = long_html(40);
        let expected = expected_segments(&html);
        assert!(
            expected.len() >= 3,
            "夹具必须产生多段（实际 {}）",
            expected.len()
        );
        let script = (0..expected.len())
            .map(|i| Reply::Text(format!("<p>译{i}</p>")))
            .collect::<Vec<_>>();
        let (port, requests) = start_recording_sse(script);
        let tmp = TempAiDb::new("seg-order");
        let state = setup_migrated(&tmp.path, port, &html, "正文");
        let events = Arc::new(StdMutex::new(Vec::new()));
        let channel = recording_channel(events.clone());

        let text = ai_translate_inner(&state, 1, &channel).await.unwrap();

        let reqs = requests.lock().unwrap().clone();
        assert_eq!(reqs.len(), expected.len(), "每段一次请求，无多余请求");
        let sent: Vec<String> = reqs.iter().map(|r| sent_segment(r)).collect();
        assert_eq!(
            sent, expected,
            "实际发送的分段内容与顺序必须与分段器逐字一致"
        );
        for (i, seg) in sent.iter().enumerate() {
            assert!(
                unbalanced_tags(seg).is_empty(),
                "第 {i} 段必须是合法 HTML 片段（标签配对）：{seg}"
            );
            assert!(!seg.is_empty());
        }
        let expect_product: String = (0..expected.len())
            .map(|i| format!("<p>译{i}</p>"))
            .collect();
        assert_eq!(text, expect_product, "返回值 = 各段按序拼接");
        assert_eq!(
            translated_of(&state).await.as_deref(),
            Some(expect_product.as_str()),
            "整篇产物 = 各段按序拼接（消毒后）"
        );
        assert_eq!(
            events.lock().unwrap().clone(),
            {
                let mut v = vec!["delta"; expected.len()];
                v.push("done");
                v
            },
            "每段 delta 实时转发 + 全部完成才 Done"
        );

        // 顺序稳定：清掉产物与片段后重跑（同一 mock/同一配置），发送序列逐字相同
        clear_full_product(&state).await;
        let before = requests.lock().unwrap().len();
        let state2 = reopen_migrated(&tmp.path);
        ai_translate_inner(&state2, 1, &channel).await.unwrap();
        let reqs2 = requests.lock().unwrap().clone();
        let sent2: Vec<String> = reqs2[before..].iter().map(|r| sent_segment(r)).collect();
        assert_eq!(sent2, sent, "同一输入的发送分段与顺序必须稳定");
    }

    /// 反例 2 + 4（命令层，真实重启）：第二段失败 → 进程重启（同一文件库新连接）
    /// → 只重发未完成段；已完成段复用不重发；拼接无重复文本；全部完成前
    /// `translated_content`/`translated_sig` 必须为 NULL（部分态不冒充完整）。
    #[tokio::test]
    async fn failed_segment_resumes_after_restart_without_duplicates() {
        let html = long_html(40);
        let expected = expected_segments(&html);
        assert!(expected.len() >= 3);
        let script = vec![
            Reply::Text("<p>首次第一段</p>".into()),
            Reply::Http500,
            // —— 重启后的重发段（第 2、3 段；同一 mock，脚本续接）——
            Reply::Text("<p>重发第二段</p>".into()),
            Reply::Text("<p>重发第三段</p>".into()),
        ];
        let (port, requests) = start_recording_sse(script);
        let tmp = TempAiDb::new("seg-resume");
        let state = setup_migrated(&tmp.path, port, &html, "正文");
        let events1 = Arc::new(StdMutex::new(Vec::new()));

        let err = ai_translate_inner(&state, 1, &recording_channel(events1.clone()))
            .await
            .expect_err("第二段失败必须如实 Err");
        assert_eq!(err.code, "aiStream", "{err}");
        assert_eq!(requests.lock().unwrap().len(), 2, "第一次共发出 2 段请求");
        // 反例 4：部分态不写整篇产物（产物与签名都不得出现）
        assert_eq!(translated_of(&state).await, None, "未完成不得写整篇产物");
        assert_eq!(translated_sig_of(&state).await, None, "未完成不得写签名");
        let rows = segments_snapshot(&state).await;
        assert_eq!(rows.len(), 1, "只有完成段落库（第 1 段）");
        assert_eq!((rows[0].1, rows[0].2.as_str()), (0, "<p>首次第一段</p>"));
        let types1 = events1.lock().unwrap().clone();
        assert_eq!(
            types1,
            vec!["delta"],
            "HTTP 失败在流开始前分型：不得有第二段 delta、不得有 Done"
        );
        drop(state); // 「进程结束」：内存状态全部丢弃

        // 重启（新连接 + 新 AppState，同配置；片段只能来自 DB）：只重发未完成段
        let state2 = reopen_migrated(&tmp.path);
        let events2 = Arc::new(StdMutex::new(Vec::new()));
        let text = ai_translate_inner(&state2, 1, &recording_channel(events2.clone()))
            .await
            .unwrap();
        let reqs2 = requests.lock().unwrap().clone();
        assert_eq!(
            reqs2.len(),
            2 + (expected.len() - 1),
            "重启后只重发未完成段（已完成段不重发）"
        );
        let sent2: Vec<String> = reqs2[2..].iter().map(|r| sent_segment(r)).collect();
        assert_eq!(sent2, expected[1..].to_vec(), "从第 2 段继续，顺序一致");
        let expect_product = format!(
            "<p>首次第一段</p>{}{}",
            "<p>重发第二段</p>", "<p>重发第三段</p>"
        );
        assert_eq!(text, expect_product, "拼接正确且无重复文本");
        assert_eq!(
            translated_of(&state2).await.as_deref(),
            Some(expect_product.as_str())
        );
        assert_eq!(
            events2.lock().unwrap().clone(),
            vec!["delta", "delta", "done"],
            "复用段不重放，仅重发段有 delta，全部完成才 Done"
        );
        assert_eq!(segments_snapshot(&state2).await.len(), expected.len());
    }

    /// 反例 3（命令层）：模型/提示词/正文修订变化 → 旧片段不复用；整篇重新
    /// 生成并清掉旧签名片段；计划查询如实报告作废段数（前端提示的数据源）。
    #[tokio::test]
    async fn changed_model_does_not_reuse_segments() {
        let html = long_html(40);
        let expected = expected_segments(&html);
        let script = vec![Reply::Text("<p>旧模型第一段</p>".into()), Reply::Http500];
        let (port, requests) = start_recording_sse(script);
        let tmp = TempAiDb::new("seg-sig");
        let state = setup_migrated(&tmp.path, port, &html, "正文");
        ai_translate_inner(
            &state,
            1,
            &recording_channel(Arc::new(StdMutex::new(Vec::new()))),
        )
        .await
        .expect_err("第二段失败");
        assert_eq!(
            segments_snapshot(&state).await.len(),
            1,
            "旧签名下 1 段进度"
        );

        // 换模型（仅模型变化；签名随之变化）
        {
            let conn = state.db.lock().await;
            conn.execute(
                "UPDATE settings SET value = ?1 WHERE key = 'ai_config'",
                rusqlite::params![format!(
                    r#"{{"preset":"custom","baseUrl":"http://127.0.0.1:{port}","apiKey":"sk-test","model":"m2"}}"#
                )],
            )
            .unwrap();
        }
        // 计划查询（换模型后）：旧进度作废、当前签名完成 0
        let plan = ai_cache_check_inner(&state, 1)
            .await
            .unwrap()
            .translation_segments;
        assert_eq!(
            plan,
            TranslationSegmentPlan {
                total: expected.len(),
                completed: 0,
                discarded: 1
            },
            "计划必须如实报告：总段数、当前签名完成数、作废段数"
        );

        // 换模型后重跑：必须整篇重发（旧片段不复用），脚本续接给出新段文本
        let text = ai_translate_inner(
            &state,
            1,
            &recording_channel(Arc::new(StdMutex::new(Vec::new()))),
        )
        .await
        .unwrap();
        let reqs = requests.lock().unwrap().clone();
        let sent: Vec<String> = reqs[2..].iter().map(|r| sent_segment(r)).collect();
        assert_eq!(sent.len(), expected.len(), "签名变化后退化为整篇重新生成");
        assert_eq!(sent, expected, "全部段都要重发（旧片段不复用）");
        assert!(
            !text.contains("旧模型第一段"),
            "旧签名片段不得混入新产物：{text}"
        );
        let rows = segments_snapshot(&state).await;
        assert_eq!(rows.len(), expected.len());
        assert!(
            rows.iter().all(|r| r.0 == rows[0].0),
            "成功后只保留当前签名的片段（旧签名进度清理）：{rows:?}"
        );
    }

    /// 反例 5a（命令层）：分段中某段 length → 013A 语义（aiTruncated、不写整篇、
    /// 可重试）；重试只补失败段，拼接无重复文本。
    #[tokio::test]
    async fn length_failure_mid_segments_is_retryable_without_duplicates() {
        let html = long_html(40);
        let expected = expected_segments(&html);
        let script = vec![
            Reply::Text("<p>第一段成品</p>".into()),
            Reply::Length,
            // —— 重试阶段（同一 mock 续接）——
            Reply::Text("<p>第二段重试</p>".into()),
            Reply::Text("<p>第三段重试</p>".into()),
        ];
        let (port, requests) = start_recording_sse(script);
        let tmp = TempAiDb::new("seg-length");
        let state = setup_migrated(&tmp.path, port, &html, "正文");
        let err = ai_translate_inner(
            &state,
            1,
            &recording_channel(Arc::new(StdMutex::new(Vec::new()))),
        )
        .await
        .expect_err("length 必须失败");
        assert_eq!(err.code, "aiTruncated", "{err}");
        assert_eq!(requests.lock().unwrap().len(), 2, "第三段不得被发送");
        assert_eq!(translated_of(&state).await, None, "不写整篇产物");
        assert_eq!(segments_snapshot(&state).await.len(), 1, "只有完成段落库");

        let state2 = reopen_migrated(&tmp.path);
        let text = ai_translate_inner(
            &state2,
            1,
            &recording_channel(Arc::new(StdMutex::new(Vec::new()))),
        )
        .await
        .unwrap();
        let reqs = requests.lock().unwrap().clone();
        assert_eq!(reqs.len(), 2 + (expected.len() - 1), "重试只补未完成段");
        let expect_product = "<p>第一段成品</p><p>第二段重试</p><p>第三段重试</p>".to_string();
        assert_eq!(text, expect_product);
        assert_eq!(
            translated_of(&state2).await.as_deref(),
            Some(expect_product.as_str())
        );
        for marker in ["第一段成品", "第二段重试", "第三段重试"] {
            assert_eq!(
                expect_product.matches(marker).count(),
                1,
                "标记 {marker} 不得重复追加"
            );
        }
    }

    /// 反例 5b（命令层）：分段中某段 content_filter → aiFiltered；失败段不落库
    /// （前一已完成段保留）。
    #[tokio::test]
    async fn content_filter_failure_keeps_previous_segments_only() {
        let html = long_html(40);
        let expected = expected_segments(&html);
        let script = vec![Reply::Filtered];
        let (port, requests) = start_recording_sse(script);
        let tmp = TempAiDb::new("seg-filter");
        let state = setup_migrated(&tmp.path, port, &html, "正文");
        let err = ai_translate_inner(
            &state,
            1,
            &recording_channel(Arc::new(StdMutex::new(Vec::new()))),
        )
        .await
        .expect_err("content_filter 必须失败");
        assert_eq!(err.code, "aiFiltered", "{err}");
        assert_eq!(requests.lock().unwrap().len(), 1);
        assert!(expected.len() > 1);
        assert_eq!(translated_of(&state).await, None, "不写整篇产物");
        assert!(segments_snapshot(&state).await.is_empty(), "失败段不落库");
    }

    /// 反例 5c（命令层）：分段中取消（消费方关闭）→ 013A 语义：不发送 Done、
    /// 不写整篇产物、取消段不落库，已完成段保留。
    #[tokio::test]
    async fn cancel_mid_segment_keeps_013a_no_done_no_cache() {
        let html = long_html(40);
        let expected = expected_segments(&html);
        assert!(expected.len() >= 2);
        let script = vec![
            Reply::Text("<p>第一段成品</p>".into()),
            Reply::Text("<p>第二段半截</p>".into()),
        ];
        let (port, requests) = start_recording_sse(script);
        let tmp = TempAiDb::new("seg-cancel");
        let state = setup_migrated(&tmp.path, port, &html, "正文");
        let events = Arc::new(StdMutex::new(Vec::new()));
        // 第二段的第一个 delta 起通道关闭（模拟前端关闭面板）
        let channel: Channel<AiEvent> = Channel::new({
            let events = events.clone();
            move |body: InvokeResponseBody| {
                let v: serde_json::Value = body
                    .deserialize()
                    .unwrap_or_else(|_| serde_json::json!({"type": "unparseable"}));
                let ty = v["type"].as_str().unwrap_or("?").to_string();
                events.lock().unwrap().push(ty.clone());
                let seen_deltas = events
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|t| *t == "delta")
                    .count();
                if seen_deltas >= 2 {
                    return Err(tauri::Error::Io(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "channel closed",
                    )));
                }
                Ok(())
            }
        });

        let text = ai_translate_inner(&state, 1, &channel)
            .await
            .expect("取消不是错误");
        assert!(!text.is_empty(), "已渲染文本保留在返回值里");
        assert_eq!(requests.lock().unwrap().len(), 2, "取消后不得继续后续段");
        let types = events.lock().unwrap().clone();
        assert_eq!(
            types,
            vec!["delta", "delta"],
            "取消后不得发送 Done: {types:?}"
        );
        assert_eq!(translated_of(&state).await, None, "取消不得写整篇产物");
        let rows = segments_snapshot(&state).await;
        assert_eq!(rows.len(), 1, "取消段不落库（只有已完成段）");
        assert_eq!(rows[0].2, "<p>第一段成品</p>");
    }

    /// B2 附加：摘要超预算 → 显式失败（aiTooLong），不发请求、不写缓存、
    /// 不发 Done、不静默截断。
    #[tokio::test]
    async fn oversized_summary_input_fails_explicitly_without_network() {
        let (port, requests) = start_recording_sse(vec![]);
        let body = "字".repeat(crate::ai::SUMMARY_INPUT_MAX_BYTES + 1);
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
             CREATE TABLE ai_segments (article_id INTEGER NOT NULL, product TEXT NOT NULL,
                 signature TEXT NOT NULL, segment_index INTEGER NOT NULL, content TEXT NOT NULL,
                 created_at TEXT NOT NULL DEFAULT (datetime('now')),
                 PRIMARY KEY (article_id, product, signature, segment_index));
             INSERT INTO contents (id, identity, content_html, body_text) VALUES (1, 'h|big', '<p>x</p>', '小');
             INSERT INTO articles (id, title, content_id) VALUES (1, '超长文章', 1);
             INSERT INTO settings (key, value) VALUES ('ai_config', '{cfg}');"
        ))
        .unwrap();
        conn.execute(
            "UPDATE contents SET body_text = ?1 WHERE id = 1",
            rusqlite::params![body],
        )
        .unwrap();
        let state = AppState::new(
            conn,
            crate::ingestion::build_client(30),
            crate::media::MediaHandle::inactive(),
        );
        let events = Arc::new(StdMutex::new(Vec::new()));
        let err = ai_summarize_inner(&state, 1, &recording_channel(events.clone()))
            .await
            .expect_err("超预算必须显式失败");
        assert_eq!(err.code, "aiTooLong", "{err}");
        assert!(err.message.contains("预算"), "{err}");
        assert!(
            requests.lock().unwrap().is_empty(),
            "超预算必须在本地失败，不发起任何网络请求"
        );
        assert!(events.lock().unwrap().is_empty(), "失败不得发送 delta/Done");
        let conn = state.db.lock().await;
        let summary: Option<String> = conn
            .query_row("SELECT ai_summary FROM articles WHERE id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(summary, None, "超预算不得写缓存（不截断、不假称完整）");
    }

    /// R1 复审 P2-1（命令层，真实 wire）：退化输入——`body.trim()` 非空但分段
    /// 解析未产出任何内容节点（`<html></html>` 的容器标签被解析器吸收）——
    /// 必须显式 `aiIncomplete`：不得走「空拼串 → 以有效签名提交空产物 → Done」
    /// 的假完整旁路（单段/多段路径各自的空产物守卫都覆盖不到 `total == 0`）。
    #[tokio::test]
    async fn degenerate_input_without_content_nodes_fails_without_done() {
        let (port, requests) = start_recording_sse(vec![]);
        let tmp = TempAiDb::new("seg-no-content");
        // content_html 解析后无任何内容节点；body_text 非空以便勾过「无正文」守卫。
        let state = setup_migrated(&tmp.path, port, "<html></html>", "正文");
        let events = Arc::new(StdMutex::new(Vec::new()));

        let err = ai_translate_inner(&state, 1, &recording_channel(events.clone()))
            .await
            .expect_err("无有效内容必须显式失败，不得空产物 + Done");
        assert_eq!(err.code, "aiIncomplete", "{err}");
        assert!(
            requests.lock().unwrap().is_empty(),
            "退化输入必须在本地失败，不发起任何网络请求"
        );
        assert!(events.lock().unwrap().is_empty(), "失败不得发送 delta/Done");
        assert_eq!(translated_of(&state).await, None, "不得写空整篇产物");
        assert_eq!(translated_sig_of(&state).await, None, "不得写有效签名");
        assert!(segments_snapshot(&state).await.is_empty(), "不得写片段");
    }

    /// R1 复审 P3-1（命令层，DB 触发器注入）：整篇产物成功提交后「清理其他
    /// 签名片段」失败时必须 best-effort 吞错——命令返回 Ok、发送 Done、产物与
    /// 签名已落库（「Done ⇔ 完整产物已提交」不被维护动作破坏）。修前清理带
    /// `?`：DB 故障让命令返回 Err 且不发 Done，而产物其实已提交——前端显示失败、
    /// 下次命中缓存自愈，前后端状态互相矛盾。
    #[tokio::test]
    async fn stale_segment_cleanup_failure_does_not_flip_success() {
        let script = vec![Reply::Text("<p>新译文</p>".into())];
        let (port, requests) = start_recording_sse(script);
        let tmp = TempAiDb::new("seg-cleanup");
        let state = setup_migrated(&tmp.path, port, "<p>甲</p>", "正文");
        {
            let conn = state.db.lock().await;
            // 旧签名残留片段 + 让清理 DELETE 必然失败（触发器注入）。
            db::upsert_ai_segment(
                &conn,
                1,
                db::AI_PRODUCT_TRANSLATION,
                "old-sig",
                0,
                "旧翻译",
                0,
            )
            .unwrap();
            conn.execute_batch(
                "CREATE TRIGGER block_seg_delete BEFORE DELETE ON ai_segments
                 BEGIN SELECT RAISE(ABORT, 'injected cleanup failure'); END;",
            )
            .unwrap();
        }
        let events = Arc::new(StdMutex::new(Vec::new()));

        let text = ai_translate_inner(&state, 1, &recording_channel(events.clone()))
            .await
            .expect("清理失败不得把已成事实的成功倒挂成 Err");
        assert_eq!(text, "<p>新译文</p>");
        assert_eq!(requests.lock().unwrap().len(), 1, "单段一次请求");
        assert_eq!(
            translated_of(&state).await.as_deref(),
            Some("<p>新译文</p>"),
            "产物已提交"
        );
        assert!(
            translated_sig_of(&state).await.is_some(),
            "签名与产物成对落库"
        );
        assert_eq!(
            events.lock().unwrap().clone(),
            vec!["delta", "done"],
            "成功必须发送 Done（清理故障不得吞掉完成信号）"
        );
        let rows = segments_snapshot(&state).await;
        assert_eq!(
            rows.len(),
            2,
            "清理失败被吞：旧签名片段残留（不影响正确性——读段只按同一签名）: {rows:?}"
        );
    }

    /* ============================================================
    OPT-013 B3：真实消费者三入口的命令层证据
    ============================================================ */

    /// B3 反例 3（命令层，真实 wire）：同进程「继续」——第二段失败后再次触发
    /// 生成，只重发缺失段（已完成段不重发、拼接无重复）；失败后的状态源
    /// （`ai_cache_check` 的 completed/total）如实报告 partial 进度。
    #[tokio::test]
    async fn continue_after_failure_resends_only_missing_segments() {
        let html = long_html(40);
        let expected = expected_segments(&html);
        assert!(expected.len() >= 3);
        let script = vec![Reply::Text("<p>首段</p>".into()), Reply::Http500];
        let (port, requests) = start_recording_sse(script);
        let tmp = TempAiDb::new("b3-continue");
        let state = setup_migrated(&tmp.path, port, &html, "正文");

        let err = ai_translate_inner(
            &state,
            1,
            &recording_channel(Arc::new(StdMutex::new(Vec::new()))),
        )
        .await
        .expect_err("第二段失败必须如实 Err");
        assert_eq!(err.code, "aiStream", "{err}");
        assert_eq!(requests.lock().unwrap().len(), 2, "第一次共发出 2 段请求");

        // 「继续」入口的状态源：partial = 0 < completed < total；整篇不得冒充完整
        let plan = ai_cache_check_inner(&state, 1)
            .await
            .unwrap()
            .translation_segments;
        assert_eq!(
            plan,
            TranslationSegmentPlan {
                total: expected.len(),
                completed: 1,
                discarded: 0
            },
            "失败后计划必须如实报告已完成 1 段（前端「继续」入口的数据源）"
        );
        assert_eq!(translated_of(&state).await, None, "部分态绝不写整篇产物");
        assert_eq!(translated_sig_of(&state).await, None, "部分态不得写签名");

        // 触发「继续」（同一命令、同一进程）：只重发缺失段
        let text = ai_translate_inner(
            &state,
            1,
            &recording_channel(Arc::new(StdMutex::new(Vec::new()))),
        )
        .await
        .unwrap();
        let reqs = requests.lock().unwrap().clone();
        assert_eq!(
            reqs.len(),
            2 + (expected.len() - 1),
            "继续只重发缺失段（已完成段不重发）"
        );
        let sent: Vec<String> = reqs[2..].iter().map(|r| sent_segment(r)).collect();
        assert_eq!(sent, expected[1..].to_vec(), "从第 2 段继续，顺序一致");
        let expect_product = format!(
            "<p>首段</p>{}",
            "<p>默认译文</p>".repeat(expected.len() - 1)
        );
        assert_eq!(text, expect_product, "拼接正确且无重复文本");
        assert_eq!(
            text.matches("<p>首段</p>").count(),
            1,
            "已完成段不得重复追加"
        );
        assert_eq!(
            translated_of(&state).await.as_deref(),
            Some(expect_product.as_str()),
            "全部段就位后才形成整篇产物"
        );
    }

    /// B3 反例 2（命令层，真实重启）：重启（同一文件库新连接 + 新 AppState）后
    /// `ai_cache_check` 报告的已完成段数不减少——partial 仍为 partial，且整篇
    /// 产物仍为空（刷新重启不得把 partial 当完整缓存）。
    #[tokio::test]
    async fn restart_keeps_completed_segment_count_for_partial() {
        let html = long_html(40);
        let expected = expected_segments(&html);
        assert!(expected.len() >= 3);
        let script = vec![Reply::Text("<p>首段</p>".into()), Reply::Http500];
        let (port, _requests) = start_recording_sse(script);
        let tmp = TempAiDb::new("b3-restart-plan");
        let state = setup_migrated(&tmp.path, port, &html, "正文");
        ai_translate_inner(
            &state,
            1,
            &recording_channel(Arc::new(StdMutex::new(Vec::new()))),
        )
        .await
        .expect_err("第二段失败");
        let before = ai_cache_check_inner(&state, 1)
            .await
            .unwrap()
            .translation_segments;
        assert_eq!((before.total, before.completed), (expected.len(), 1));
        drop(state); // 「进程结束」

        let state2 = reopen_migrated(&tmp.path);
        let after = ai_cache_check_inner(&state2, 1)
            .await
            .unwrap()
            .translation_segments;
        assert_eq!(
            after, before,
            "重启后分段进度不减少（completed 不归零、total 不变）"
        );
        assert_eq!(
            translated_of(&state2).await,
            None,
            "重启后 partial 仍不是完整产物（不得被当完整缓存）"
        );
        assert_eq!(translated_sig_of(&state2).await, None);
    }

    /// B3 反例 4（命令层）：清除入口——清除后该条目该产物在**所有签名**下的
    /// 片段为空、整篇产物与签名为空、清除代次推进；随后再生成必须零恢复
    /// （全部分段重发，旧片段绝不混入新产物）。
    #[tokio::test]
    async fn clear_removes_all_segments_and_continue_recovers_nothing() {
        let html = long_html(40);
        let expected = expected_segments(&html);
        assert!(expected.len() >= 3);
        let script = vec![Reply::Text("<p>第一段</p>".into()), Reply::Http500];
        let (port, requests) = start_recording_sse(script);
        let tmp = TempAiDb::new("b3-clear");
        let state = setup_migrated(&tmp.path, port, &html, "正文");
        ai_translate_inner(
            &state,
            1,
            &recording_channel(Arc::new(StdMutex::new(Vec::new()))),
        )
        .await
        .expect_err("第二段失败");
        {
            let conn = state.db.lock().await;
            // 另一签名下的旧片段（清除必须跨签名生效）
            db::upsert_ai_segment(
                &conn,
                1,
                db::AI_PRODUCT_TRANSLATION,
                "old-sig",
                0,
                "旧签名片段",
                0,
            )
            .unwrap();
        }
        assert_eq!(
            segments_snapshot(&state).await.len(),
            2,
            "清除前：当前签名 1 段 + 旧签名 1 段"
        );
        let epoch_before = {
            let conn = state.db.lock().await;
            db::article_ai_commit_token(&conn, 1)
                .unwrap()
                .unwrap()
                .ai_epoch
        };

        ai_clear_cache_inner(&state, 1, "translation")
            .await
            .unwrap();

        assert!(
            segments_snapshot(&state).await.is_empty(),
            "清除必须删除全部签名下的片段（后台恢复记录）"
        );
        assert_eq!(translated_of(&state).await, None, "整篇产物为空");
        assert_eq!(translated_sig_of(&state).await, None, "签名与产物同灭");
        assert_eq!(
            {
                let conn = state.db.lock().await;
                db::article_ai_commit_token(&conn, 1)
                    .unwrap()
                    .unwrap()
                    .ai_epoch
            },
            epoch_before + 1,
            "清除推进清除代次（在途迟到提交被拒）"
        );
        let plan = ai_cache_check_inner(&state, 1)
            .await
            .unwrap()
            .translation_segments;
        assert_eq!(
            plan,
            TranslationSegmentPlan {
                total: expected.len(),
                completed: 0,
                discarded: 0
            },
            "清除后回到「无产物/未生成」——不得再报 partial（completed/discarded 全 0）"
        );

        // 再「继续」：零恢复——全部分段重发
        let before = requests.lock().unwrap().len();
        let text = ai_translate_inner(
            &state,
            1,
            &recording_channel(Arc::new(StdMutex::new(Vec::new()))),
        )
        .await
        .unwrap();
        let reqs = requests.lock().unwrap().clone();
        assert_eq!(
            reqs.len() - before,
            expected.len(),
            "清除后再生成必须全部分段重发（不得复用任何旧片段）"
        );
        let sent: Vec<String> = reqs[before..].iter().map(|r| sent_segment(r)).collect();
        assert_eq!(sent, expected, "全部分段（含曾完成的第 1 段）都重发");
        assert!(
            !text.contains("第一段") && !text.contains("旧签名片段"),
            "旧片段不得混入新产物：{text}"
        );
        assert_eq!(
            translated_of(&state).await.as_deref(),
            Some(text.as_str()),
            "重新生成完成后形成整篇产物"
        );
    }

    /// B3：清除命令的参数/存在性边界——非法 product 显式 validate 失败、
    /// 条目不存在 not_found（不得静默成功成「已清除」假象）。
    #[tokio::test]
    async fn clear_rejects_invalid_product_and_missing_article() {
        let tmp = TempAiDb::new("b3-clear-validate");
        let state = setup_migrated(&tmp.path, 0, "<p>甲</p>", "正文");
        let err = ai_clear_cache_inner(&state, 1, "bogus")
            .await
            .expect_err("非法 product 必须显式失败");
        assert_eq!(err.code, "validate", "{err}");
        let err = ai_clear_cache_inner(&state, 999, "translation")
            .await
            .expect_err("条目不存在必须 not_found");
        assert_eq!(err.code, "notFound", "{err}");
    }
}
