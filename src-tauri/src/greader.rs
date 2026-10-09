//! Google Reader API（兼容协议）客户端。
//!
//! 后端：Miniflux（新版原生支持 Google Reader 兼容层 + Fever API）。
//! 本模块当前**只实现 Google Reader 协议**（后端可换的基线）；Fever API 预留
//! 未实现——它是可选备胎，仅在 Google Reader 兼容层不可用时才需要补充
//! （`POST /fever/?api` + `api_key=md5(username:password)`）。
//! 认证（OPT-004 起，契约见 `.agents/notes/implemented/architecture/2026-10-08-Reader适配的鉴权与分类契约.md`）：
//! 两步 ClientLogin 换取 auth token；**所有** API 请求一致携带
//! `Authorization: GoogleLogin auth=<auth>`（GET 与 POST 都带——FreshRSS 靠它设用户上下文），
//! 表单参数 `T` 则用写操作专用 action token（`GET /reader/api/0/token` 换取并缓存；
//! Miniflux 返回 auth 本身、FreshRSS 返回另一字符串，两种都接受）。
//!
//! 权威规范来源：Miniflux 源码 `internal/googlereader/`（README.md + middleware.go），
//! 并经 curl 连真实 Miniflux（`https://sync.example.invalid/`）实证。要点：
//!   - Google Reader username/password 是 Miniflux「集成」页单独配置的凭据（非账号密码）。
//!   - `stream/items/ids` 返回十进制 id 字符串；`stream/items/contents` 返回长格式 item id。
//!   - 已读/收藏状态从 `categories` 数组里的 `read`/`starred` tag 判断。
//!   - `enclosure` 无 `duration` 字段（与 Miniflux `/v1/` API 不同）。
//!   - `ot`/`nt` 是 unix **秒**。

use crate::error::{AppError, AppResult};
use reqwest::Client;
use serde::Deserialize;

/* ============================================================
行类型（Google Reader JSON 的最小子集）
============================================================ */

/// ClientLogin 响应（`POST /accounts/ClientLogin?output=json`）
/// 注意：Miniflux 返回字段名是 `SID`/`LSID`/`Auth`（首字母大写），serde 默认大小写敏感。
#[derive(Debug, Deserialize)]
pub struct ClientLoginResponse {
    #[serde(default, rename = "SID")]
    pub sid: String,
    #[serde(default, rename = "LSID")]
    pub lsid: String,
    /// auth token（后续请求用它做认证）
    #[serde(default, rename = "Auth")]
    pub auth: String,
}

/// 解析 ClientLogin 响应并取出 auth token。
///
/// 两种响应形态都要支持（REQ-SYNC-001 要求同时面向 Miniflux 与 FreshRSS）：
/// Miniflux 等尊重 `output=json`，返回 `{"SID":…,"LSID":…,"Auth":…}`；
/// FreshRSS 忽略 `output=json`，返回经典文本行 `SID=…` / `LSID=…` / `Auth=…`，
/// 凭据错误时是 `Error=BadAuthentication`。
/// 先按 JSON 解析以保持 Miniflux 既有行为，失败再按行取 `Auth=`。
// Note: 双格式登录的理由——不同服务端实现返回的 ClientLogin 响应体格式不一致
// （经典 text/plain 键值 与 JSON 两种），只认一种会误判为「凭据错误」；
// 被否方案：按服务端类型硬编码分支（无法覆盖自建/中间层实现）。
pub fn parse_client_login(body: &str) -> AppResult<String> {
    if let Ok(parsed) = serde_json::from_str::<ClientLoginResponse>(body) {
        if !parsed.auth.is_empty() {
            return Ok(parsed.auth);
        }
    }
    let mut error_code: Option<String> = None;
    for line in body.lines() {
        let line = line.trim();
        if let Some(token) = line.strip_prefix("Auth=") {
            let token = token.trim();
            if !token.is_empty() {
                return Ok(token.to_string());
            }
        } else if let Some(code) = line.strip_prefix("Error=") {
            error_code = Some(code.trim().to_string());
        }
    }
    match error_code {
        Some(code) if !code.is_empty() => {
            Err(AppError::network(format!("ClientLogin 失败：{code}")))
        }
        _ => Err(AppError::network(
            "ClientLogin 响应既非 JSON 也未包含 Auth= 行",
        )),
    }
}

/// 订阅（`subscription/list` 的 subscriptions[] 元素）
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subscription {
    /// feed 流 id，如 `feed/42`（数字 id）
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub categories: Vec<CategoryRef>,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub icon_url: Option<String>,
}

/// 分类（subscription 的 categories[] 元素 / tag/list 的 folder）
#[derive(Debug, Deserialize)]
pub struct CategoryRef {
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub r#type: Option<String>,
}

/// 标签列表（tag/list 响应）
#[derive(Debug, Deserialize)]
pub struct TagListResponse {
    #[serde(default)]
    pub tags: Vec<TagRef>,
}

#[derive(Debug, Deserialize)]
pub struct TagRef {
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub r#type: Option<String>,
}

/// 订阅列表（subscription/list 响应）
#[derive(Debug, Deserialize)]
pub struct SubscriptionListResponse {
    #[serde(default)]
    pub subscriptions: Vec<Subscription>,
}

/// 条目 id 列表（stream/items/ids 响应）
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemIdsResponse {
    #[serde(default)]
    pub item_refs: Vec<ItemRef>,
    /// 续读游标：数字 offset，JSON 字符串
    #[serde(default)]
    pub continuation: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ItemRef {
    pub id: String,
}

/// 条目内容列表（stream/items/contents 响应）
#[derive(Debug, Deserialize)]
pub struct ItemContentsResponse {
    #[serde(default)]
    pub items: Vec<ItemContent>,
}

#[derive(Debug, Deserialize)]
pub struct ItemContent {
    /// 长格式 id：`tag:google.com,2005:reader/item/...`
    pub id: String,
    /// 状态 tag 列表：含 `read`/`starred`/`reading-list`/分类 label
    #[serde(default)]
    pub categories: Vec<String>,
    pub title: String,
    #[serde(default)]
    pub author: Option<String>,
    /// unix 秒
    #[serde(default)]
    pub published: i64,
    #[serde(default)]
    pub updated: i64,
    #[serde(default)]
    pub alternate: Vec<LinkRef>,
    #[serde(default)]
    pub summary: Option<ContentBlock>,
    #[serde(default)]
    pub content: Option<ContentBlock>,
    #[serde(default)]
    pub origin: Option<OriginRef>,
    #[serde(default)]
    pub enclosure: Vec<EnclosureRef>,
}

#[derive(Debug, Deserialize)]
pub struct LinkRef {
    #[serde(default)]
    pub href: String,
    #[serde(default)]
    pub r#type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ContentBlock {
    #[serde(default)]
    pub content: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OriginRef {
    /// `feed/42`（数字 id）
    #[serde(default)]
    pub stream_id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub html_url: String,
}

#[derive(Debug, Deserialize)]
pub struct EnclosureRef {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub r#type: Option<String>,
}

/// quickadd 响应
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickAddResponse {
    #[serde(default)]
    pub num_results: i64,
    #[serde(default)]
    pub stream_id: Option<String>,
    #[serde(default)]
    pub stream_name: Option<String>,
}

/* ============================================================
状态 tag 常量
============================================================ */

pub mod tags {
    pub const READ: &str = "user/-/state/com.google/read";
    pub const STARRED: &str = "user/-/state/com.google/starred";
    pub const KEPT_UNREAD: &str = "user/-/state/com.google/kept-unread";
    pub const READING_LIST: &str = "user/-/state/com.google/reading-list";
}

/* ============================================================
客户端
============================================================ */

#[derive(Clone, Debug)]
pub struct GReaderClient {
    /// 后端根 URL（去尾部斜杠），如 `https://sync.example.invalid`
    base: String,
    /// ClientLogin 换取的 auth token（`username/hmac`），用于 Authorization 头
    token: String,
    /// 写操作专用 action token（`GET /token` 换取，惰性 + 单次缓存）。
    /// 失败不缓存：错误返回后下次调用会重试（OnceCell 的 get_or_try_init 语义）。
    action_token: tokio::sync::OnceCell<String>,
    http: Client,
}

impl GReaderClient {
    /// 用已知的 auth token 构建（不重新 ClientLogin）。
    /// 写请求所需的 action token 在首次 POST 时惰性获取并缓存。
    pub fn new(endpoint: &str, token: &str, http: Client) -> Self {
        let base = endpoint.trim_end_matches('/').to_string();
        Self {
            base,
            token: token.to_string(),
            action_token: tokio::sync::OnceCell::new(),
            http,
        }
    }

    /// 两步认证：先 ClientLogin 换 token，再构建客户端。
    /// `username`/`password` 是 Google Reader 集成凭据（非 Miniflux 账号密码）。
    ///
    /// **端点自动适配（TASK-059）**：`endpoint` 可以是**纯域名**——依次尝试
    /// `{域名}` 与 `{域名}/api/greader.php`（FreshRSS 形态），
    /// 以 **404 = 路径不存在**、**其它状态码 = 路径存在** 判定。
    /// 凭据错误（401/403/400）会**立即停止探测**并报凭据原因，
    /// 绝不会被误报成「找不到 API」（见 `endpoint_resolve` 模块文档）。
    pub async fn login(
        endpoint: &str,
        username: &str,
        password: &str,
        http: Client,
    ) -> AppResult<Self> {
        let candidates = crate::endpoint_resolve::greader_candidates(endpoint);
        let mut tried: Vec<String> = Vec::new();

        for base in candidates {
            tried.push(base.clone());
            match Self::login_at(&base, username, password, &http).await? {
                Some(client) => return Ok(client),
                // 404：该候选下没有 API，继续试下一个
                None => continue,
            }
        }

        Err(AppError::network(format!(
            "在该地址下找不到 GReader API（HTTP 404，已尝试：{}）。请确认域名是否正确",
            tried.join("、")
        )))
    }

    /// 用**已解析**的 API 根直接登录，**不做任何探测**。
    ///
    /// `base` 来自上次解析结果（`endpoint_resolve::cached_base`）——同一个 endpoint
    /// 被反复登录时（每次同步的 `build_client`）省掉探测请求：唯一候选即命中。
    pub async fn login_resolved(
        base: &str,
        username: &str,
        password: &str,
        http: Client,
    ) -> AppResult<Self> {
        let base = base.trim().trim_end_matches('/').to_string();
        match Self::login_at(&base, username, password, &http).await? {
            Some(client) => Ok(client),
            None => Err(AppError::network(format!(
                "在该地址下找不到 GReader API（HTTP 404，已尝试：{base}）。请确认域名是否正确"
            ))),
        }
    }

    /// 在**单个**候选地址上做 ClientLogin。
    ///
    /// `Ok(Some)` = 成功；`Ok(None)` = **404，该路径下不存在 API**（调用方可继续试下一个）；
    /// `Err` = 路径存在但请求失败——**凭据类问题必须走这里**，绝不能被上层当成「路径不对」。
    async fn login_at(
        base: &str,
        username: &str,
        password: &str,
        http: &Client,
    ) -> AppResult<Option<Self>> {
        let resp = http
            .post(format!("{base}/accounts/ClientLogin"))
            .form(&[
                ("Email", username),
                ("Passwd", password),
                ("output", "json"),
            ])
            .send()
            .await?;
        let status = resp.status().as_u16();

        if !crate::endpoint_resolve::path_exists(status) {
            return Ok(None);
        }
        if !resp.status().is_success() {
            // 路径存在但请求被拒（多为凭据问题）——立即停下，如实报错，
            // 不得继续尝试其它候选而掩盖真实原因。
            return Err(AppError::network(format!(
                "ClientLogin → {}（已定位 API：{base}）",
                resp.status()
            )));
        }
        // 双格式：先 JSON（Miniflux），失败回退经典文本 Auth= 行（FreshRSS）。
        let body = resp.text().await?;
        let token = parse_client_login(&body)?;
        Ok(Some(Self {
            base: base.to_string(),
            token,
            action_token: tokio::sync::OnceCell::new(),
            http: http.clone(),
        }))
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    /// 实际使用的 API 根（TASK-059 端点自动适配的结果）。
    ///
    /// 供测试与诊断使用：用户填的是纯域名时，这里会显示**解析后**的真实地址
    /// （如 `https://demo.freshrss.org/api/greader.php`）。
    pub fn resolved_base(&self) -> &str {
        &self.base
    }

    /// GET 请求（带 `Authorization: GoogleLogin auth=<token>`）。
    async fn get_json<T: for<'de> Deserialize<'de>>(&self, path: &str) -> AppResult<T> {
        let resp = self
            .http
            .get(self.url(path))
            .header("Authorization", format!("GoogleLogin auth={}", self.token))
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(AppError::network(format!("GET {path} → {}", resp.status())));
        }
        Ok(resp.json().await?)
    }

    /// 写操作专用 action token（`GET /reader/api/0/token`），同一客户端只取一次。
    ///
    /// 职责分离（OPT-004）：Authorization 头始终是登录 auth（服务端据此设用户上下文），
    /// POST 表单的 `T` 是 action token——Miniflux 的 /token 返回 auth 本身，
    /// FreshRSS 返回另一字符串（`str_pad(sha1(...), 57, 'Z')`），两种都接受。
    ///
    /// 回退边界：**只有 /token 明确 404**（该实现没有此端点）才退回用登录 auth 当 T；
    /// 401/403/5xx、空响应体、网络失败一律如实报错——不回退、不改猜 URL、不吞认证错误。
    /// 依据：已固定的两服务端上游源码都有 /token；404 之外的失败说明认证/服务异常，
    /// 回退会把真实故障伪装成另一种请求形态，掩盖问题。
    // Note: T 与 auth 的职责分离、404-only 回退 — 见 .agents/notes/implemented/architecture/2026-10-08-Reader适配的鉴权与分类契约.md
    async fn action_token(&self) -> AppResult<&str> {
        self.action_token
            .get_or_try_init(|| async { self.fetch_action_token().await })
            .await
            .map(String::as_str)
    }

    async fn fetch_action_token(&self) -> AppResult<String> {
        let resp = self
            .http
            .get(self.url("/reader/api/0/token"))
            .header("Authorization", format!("GoogleLogin auth={}", self.token))
            .send()
            .await
            .map_err(|e| AppError::network(format!("获取 action token 失败：网络错误（{e}）")))?;
        let status = resp.status().as_u16();
        if status == 404 {
            // 兼容无 /token 的实现：明确 404 才回退到登录 auth（见方法注释的边界说明）。
            if self.token.is_empty() {
                return Err(AppError::network(
                    "获取 action token 失败：/token 不存在且登录 auth 为空",
                ));
            }
            return Ok(self.token.clone());
        }
        if !resp.status().is_success() {
            return Err(AppError::network(format!(
                "获取 action token 失败：GET /reader/api/0/token → {status}"
            )));
        }
        // 不把响应体裁剪进错误/日志：token 与凭据同属敏感值。
        let body = resp.text().await.map_err(|e| {
            AppError::network(format!("获取 action token 失败：读取响应失败（{e}）"))
        })?;
        let token = body.trim();
        if token.is_empty() {
            return Err(AppError::network(
                "获取 action token 失败：服务端返回空 token",
            ));
        }
        Ok(token.to_string())
    }

    /// 发 POST：Authorization 头带登录 auth，表单 `T` 用 action token（缓存，多次 POST 只取一次）。
    /// 表单编码交给 reqwest `.form()`（URL 转义由它负责，不手拼）。
    async fn post_with_action_token(
        &self,
        path: &str,
        form: &[(&str, String)],
    ) -> AppResult<reqwest::Response> {
        let token = self.action_token().await?.to_string();
        let mut params: Vec<(&str, String)> = vec![("T", token)];
        params.extend_from_slice(form);
        let resp = self
            .http
            .post(self.url(path))
            .header("Authorization", format!("GoogleLogin auth={}", self.token))
            .form(&params)
            .send()
            .await
            .map_err(|e| AppError::network(format!("POST {path} 网络失败（{e}）")))?;
        if !resp.status().is_success() {
            return Err(AppError::network(format!(
                "POST {path} → {}",
                resp.status()
            )));
        }
        Ok(resp)
    }

    /// POST 请求（JSON 响应）。
    async fn post_form<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        form: &[(&str, String)],
    ) -> AppResult<T> {
        let resp = self.post_with_action_token(path, form).await?;
        Ok(resp.json().await?)
    }

    /// POST 请求返回纯文本（edit-tag / subscription/edit 成功时返回 `OK`）。
    ///
    /// **严格校验成功正文**（OPT-004 R1 P2-1）：只验 HTTP 状态不够——真实生态里
    /// 「2xx + 错误体」（如 `FAIL`）表示写操作未生效，若当成功处理，调用方会按
    /// 成功 prune 队列，用户意图静默丢失且永不重试。要求 trim 后等于 `OK`
    /// （允许周围空白）；200+FAIL、200+空体、其他正文一律报协议错误。
    /// 错误信息**不回显响应体**（实现细节/敏感片段不进错误与日志）。
    async fn post_form_text(&self, path: &str, form: &[(&str, String)]) -> AppResult<()> {
        let resp = self.post_with_action_token(path, form).await?;
        let body = resp
            .text()
            .await
            .map_err(|e| AppError::network(format!("POST {path} 成功但读取响应体失败（{e}）")))?;
        if body.trim() != "OK" {
            return Err(AppError::network(format!(
                "POST {path} → 200 但响应体不是 OK（协议错误，写操作未生效）"
            )));
        }
        Ok(())
    }

    /* ---------- 连接与只读 ---------- */

    /// 拉订阅列表（含分类归属）。等价于 Miniflux `/v1/feeds` + `/v1/categories`。
    pub async fn subscriptions(&self) -> AppResult<Vec<Subscription>> {
        let r: SubscriptionListResponse = self
            .get_json("/reader/api/0/subscription/list?output=json")
            .await?;
        Ok(r.subscriptions)
    }

    /// 拉标签列表（starred + 用户分类 label/folder）。
    pub async fn tags(&self) -> AppResult<Vec<TagRef>> {
        let r: TagListResponse = self.get_json("/reader/api/0/tag/list?output=json").await?;
        Ok(r.tags)
    }

    /// 拉条目 id（增量）。`stream` 如 `user/-/state/com.google/reading-list` 或 `feed/42`。
    /// `ot`=仅此时间戳后（unix 秒），`nt`=之前，`n`=最大条数，`c`=续读 offset。
    pub async fn item_ids(
        &self,
        stream: &str,
        ot: Option<i64>,
        nt: Option<i64>,
        n: Option<u32>,
        c: Option<u64>,
    ) -> AppResult<ItemIdsResponse> {
        let mut path = format!("/reader/api/0/stream/items/ids?output=json&s={stream}");
        if let Some(v) = ot {
            path.push_str(&format!("&ot={v}"));
        }
        if let Some(v) = nt {
            path.push_str(&format!("&nt={v}"));
        }
        if let Some(v) = n {
            path.push_str(&format!("&n={v}"));
        }
        if let Some(v) = c {
            path.push_str(&format!("&c={v}"));
        }
        self.get_json(&path).await
    }

    /// 拉条目正文（按十进制 id，可重复）。返回 items 含状态/正文/enclosure。
    pub async fn item_contents(&self, ids: &[i64]) -> AppResult<Vec<ItemContent>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let form: Vec<(&str, String)> = ids.iter().map(|id| ("i", id.to_string())).collect();
        let r: ItemContentsResponse = self
            .post_form("/reader/api/0/stream/items/contents?output=json", &form)
            .await?;
        Ok(r.items)
    }

    /* ---------- 写操作 ---------- */

    /// 标读/未读、收藏/取消收藏。
    /// `add`/`remove` 是 tag 列表（如 `user/-/state/com.google/read`）。
    pub async fn edit_tag(&self, item_ids: &[i64], add: &[&str], remove: &[&str]) -> AppResult<()> {
        let mut form: Vec<(&str, String)> = Vec::new();
        for id in item_ids {
            form.push(("i", id.to_string()));
        }
        for tag in add {
            form.push(("a", tag.to_string()));
        }
        for tag in remove {
            form.push(("r", tag.to_string()));
        }
        // 注意：`a`/`r` 必须来自 body（PostForm），不能放 query。用 .form() 默认发 body。
        self.post_form_text("/reader/api/0/edit-tag", &form).await
    }

    /// 标已读。
    pub async fn mark_read(&self, item_ids: &[i64]) -> AppResult<()> {
        self.edit_tag(item_ids, &[tags::READ], &[]).await
    }

    /// 标未读。
    pub async fn mark_unread(&self, item_ids: &[i64]) -> AppResult<()> {
        self.edit_tag(item_ids, &[], &[tags::READ]).await
    }

    /// 收藏。
    pub async fn mark_starred(&self, item_ids: &[i64]) -> AppResult<()> {
        self.edit_tag(item_ids, &[tags::STARRED], &[]).await
    }

    /// 取消收藏。
    pub async fn mark_unstarred(&self, item_ids: &[i64]) -> AppResult<()> {
        self.edit_tag(item_ids, &[], &[tags::STARRED]).await
    }

    /// 快速订阅（自动发现 feed，等价旧 `/v1/feeds` 创建 + 幂等）。
    pub async fn quick_add(&self, feed_url: &str) -> AppResult<QuickAddResponse> {
        let form: Vec<(&str, String)> = vec![("quickadd", feed_url.to_string())];
        self.post_form("/reader/api/0/subscription/quickadd", &form)
            .await
    }

    /// 退订（`ac=unsubscribe`，`s=feed/<数字id>`）。
    pub async fn unsubscribe(&self, feed_numeric_id: i64) -> AppResult<()> {
        let form: Vec<(&str, String)> = vec![
            ("ac", "unsubscribe".to_string()),
            ("s", format!("feed/{feed_numeric_id}")),
        ];
        self.post_form_text("/reader/api/0/subscription/edit", &form)
            .await
    }

    /// 编辑订阅（`ac=edit`，`s=feed/<数字id>`，可改标题 `t` 或移分类 `a`）。
    ///
    /// `dest_label` 是**用户目录名**（裸名，如 `技术/阅读`）；wire 上的 `a` 必须是
    /// 完整 label stream id `user/-/label/<名>`（FreshRSS `subscriptionEdit` 按
    /// `user/-/label/` 或 `user/<user>/label/` 前缀解析；Miniflux 同形态）。
    /// 这里**无条件前置一次**——目录名本身以 `user/-/label/` 开头时也不能按字面
    /// 判定「已格式化」而少前置（用户的目录名是数据，不是标记）。
    // Note: `a` 的完整 stream id 形态 — 见 .agents/notes/implemented/architecture/2026-10-08-Reader适配的鉴权与分类契约.md
    pub async fn edit_subscription(
        &self,
        feed_numeric_id: i64,
        title: Option<&str>,
        dest_label: Option<&str>,
    ) -> AppResult<()> {
        let mut form: Vec<(&str, String)> = vec![
            ("ac", "edit".to_string()),
            ("s", format!("feed/{feed_numeric_id}")),
        ];
        if let Some(t) = title {
            form.push(("t", t.to_string()));
        }
        if let Some(name) = dest_label {
            form.push(("a", label_stream_id(name)));
        }
        self.post_form_text("/reader/api/0/subscription/edit", &form)
            .await
    }
}

/* ============================================================
辅助：解析工具
============================================================ */

/// 从 `feed/42` 流 id 提取数字 id。
pub fn parse_feed_numeric_id(stream_id: &str) -> Option<i64> {
    stream_id.strip_prefix("feed/")?.parse().ok()
}

/// 从 item id 提取十进制 id——按**协议形状**解析，不按长度猜进制（审计 F03）：
///
/// - 规范长格式 `tag:google.com,2005:reader/item/<hex>`：尾部按**十六进制**解析
///   （Google Reader item tag 规范；纯数字与含 a-f 都合法）。
/// - 无前缀的**纯十进制数字串**（FreshRSS greader 的 `stream/items/ids` 返回
///   64 位十进制、Fever 条目 id 是十进制字面值）：按**十进制**解析。
///   **绝不按十六进制猜**——16 位纯数字（如 `"1791440000000000"`）若按十六进制
///   解释会静默落成另一个数字，remote_id 与状态全线错位。
/// - 其余形态（无前缀且含非数字字符）返回 None：不猜、不靠 `len == 16` 试探。
// Note: item id 按协议形状（前缀 hex / 无前缀十进制）解析，不按长度猜 — 见 .agents/notes/implemented/architecture/2026-10-08-Fever身份与历史回溯.md
pub fn parse_item_id(id: &str) -> Option<i64> {
    const ITEM_TAG_PREFIX: &str = "tag:google.com,2005:reader/item/";
    if let Some(hex) = id.strip_prefix(ITEM_TAG_PREFIX) {
        if !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return i64::from_str_radix(hex, 16).ok();
        }
        return None;
    }
    if !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) {
        return id.parse::<i64>().ok();
    }
    None
}

/// 判断 item 的 categories 是否含某 tag（read/starred 状态判断）。
pub fn has_tag(categories: &[String], tag_suffix: &str) -> bool {
    categories.iter().any(|c| c.ends_with(tag_suffix))
}

/// 从分类 id 解析 label 后缀：`user/<user>/label/<name>` → `<name>`。
///
/// 只去掉 `user/` 与第一个 `/label/` 前缀；后缀原样保留——分类名是用户可见标签
/// 本身，中文与斜杠（如 `技术/阅读`）必须完整带回，不能按 `/` 再切分。
/// 非 label 前缀（state tag 等）返回 None。
pub fn category_label_from_id(id: &str) -> Option<&str> {
    let rest = id.strip_prefix("user/")?;
    let (_, name) = rest.split_once("/label/")?;
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// 用户目录名 → 完整 label stream id（`user/-/label/<名>`）。
///
/// 用于 subscription/edit 的 `a` 参数（见 `edit_subscription`）。**无条件前置**：
/// 目录名是用户数据，即使字面上以 `user/-/label/` 开头也只是名字的一部分，
/// 不得按字面判定「已格式化」。
fn label_stream_id(name: &str) -> String {
    format!("user/-/label/{name}")
}

/// 分类元素规范化：把 `subscription/list` 的 categories[] 与 `tag/list` 的 tag
/// 统一归一成「分类名」，供 sync 建目录/归属使用。
///
/// 两种服务端形态都覆盖（固定上游源码，见 OPT-004 的 Note）：
/// - Miniflux：`{id, label, type:"folder"}` —— type 标 folder，label 直接用；
/// - FreshRSS：tag 只有 `{id:"user/-/label/名", type:"folder"}`（无 label），
///   subscription 的 category 只有 `{id:"user/-/label/名", label:"名"}`（无 type）。
///
/// 规则：type == "folder"，或 **type 缺失但 id 是 label 前缀** → 分类（label 优先，
/// 缺 label 从 id 后缀解析）；其余（state tag、用户 tag）→ None。
/// **普通 state tag 绝不能建目录**——`user/-/state/com.google/...` 与
/// `user/-/state/org.freshrss/main` 都会在这里被挡掉。
// Note: 分类识别规则与 FreshRSS/Miniflux 固定上游源码逐条对应 — 见 .agents/notes/implemented/architecture/2026-10-08-Reader适配的鉴权与分类契约.md
pub fn category_name(id: &str, label: Option<&str>, type_: Option<&str>) -> Option<String> {
    let from_id = category_label_from_id(id);
    let is_category = type_ == Some("folder") || (type_.is_none() && from_id.is_some());
    if !is_category {
        return None;
    }
    label
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .or_else(|| from_id.map(str::to_string))
}

/* ============================================================
集成测试入口（供 tests/ 复用）
============================================================ */

#[doc(hidden)]
pub fn client_login_url(base: &str) -> String {
    format!("{}/accounts/ClientLogin", base.trim_end_matches('/'))
}

/* ============================================================
单元测试（纯解析逻辑，无网络）
============================================================ */

#[cfg(test)]
mod tests {
    use super::*;

    /// Miniflux 尊重 output=json：返回 JSON 形态，字段名首字母大写。
    #[test]
    fn parse_client_login_reads_json_body() {
        let body = r#"{"SID":"sid-value","LSID":"lsid-value","Auth":"user/abc123"}"#;
        assert_eq!(parse_client_login(body).unwrap(), "user/abc123");
    }

    /// FreshRSS 忽略 output=json：返回经典文本行形态。
    #[test]
    fn parse_client_login_reads_classic_text_body() {
        let body = "SID=sid-value\nLSID=lsid-value\nAuth=user/xyz789\n";
        assert_eq!(parse_client_login(body).unwrap(), "user/xyz789");
    }

    /// 文本形态的 Error= 要带出失败原因；既非 JSON 也无 Auth= 时给出明确错误。
    #[test]
    fn parse_client_login_rejects_error_and_garbage() {
        let err = parse_client_login("Error=BadAuthentication\n").unwrap_err();
        assert!(
            err.to_string().contains("BadAuthentication"),
            "应带出 Error 码：{err}"
        );

        let err = parse_client_login("{\"SID\":\"only-sid\"}").unwrap_err();
        assert!(err.to_string().contains("Auth="), "应说明缺少 Auth=：{err}");
    }

    #[test]
    fn parse_feed_numeric_id_works() {
        assert_eq!(parse_feed_numeric_id("feed/42"), Some(42));
        assert_eq!(parse_feed_numeric_id("feed/0"), Some(0));
        assert_eq!(parse_feed_numeric_id("user/-/state/com.google/read"), None);
        assert_eq!(parse_feed_numeric_id("feed/abc"), None);
    }

    #[test]
    fn parse_item_id_handles_protocol_shapes() {
        // 规范长格式（前缀形状）→ 尾部按十六进制：纯数字与含 a-f 都合法。
        assert_eq!(
            parse_item_id("tag:google.com,2005:reader/item/0000000000001675"),
            Some(5749)
        );
        assert_eq!(
            parse_item_id("tag:google.com,2005:reader/item/00000000000016ab"),
            Some(5803)
        );
        // 无前缀纯十进制（含 FreshRSS greader 的 64 位十进制长 id）：按十进制，
        // **不得**按 len==16 猜十六进制（否则 0x0000000000001675=5749 会顶替 1675）。
        assert_eq!(parse_item_id("5749"), Some(5749));
        assert_eq!(parse_item_id("0000000000001675"), Some(1675));
        assert_eq!(parse_item_id("1791440000000000"), Some(1791440000000000));
        // 其余形态不猜（无协议形状可依）。
        assert_eq!(parse_item_id("00000000000016ab"), None);
        assert_eq!(parse_item_id("feed/42"), None);
        assert_eq!(parse_item_id(""), None);
    }

    /// Miniflux 形态：type=folder + label 直接用。
    #[test]
    fn category_name_miniflux_folder_uses_label() {
        assert_eq!(
            category_name("user/-/label/科技", Some("科技"), Some("folder")),
            Some("科技".to_string())
        );
        // label 为空串视为缺失，回退 id 后缀
        assert_eq!(
            category_name("user/-/label/科技", Some(""), Some("folder")),
            Some("科技".to_string())
        );
    }

    /// FreshRSS 形态一：folder tag 只有 id+type（无 label），从 id 后缀取，
    /// 中文与斜杠原样保留。
    #[test]
    fn category_name_freshrss_folder_tag_parses_id_suffix() {
        assert_eq!(
            category_name("user/-/label/技术/阅读", None, Some("folder")),
            Some("技术/阅读".to_string())
        );
        // 带用户名的前缀同样适用
        assert_eq!(
            category_name("user/alice/label/News", None, Some("folder")),
            Some("News".to_string())
        );
    }

    /// FreshRSS 形态二：subscription category 无 type，id 为 label 前缀 → 分类；
    /// label 优先于 id 后缀。
    #[test]
    fn category_name_freshrss_subscription_category_without_type() {
        assert_eq!(
            category_name("user/-/label/科技", Some("科技"), None),
            Some("科技".to_string())
        );
        assert_eq!(
            category_name("user/alice/label/名 称", Some("名 称"), None),
            Some("名 称".to_string())
        );
    }

    /// state tag 与普通 tag 绝不归一成目录；label 前缀但 type=tag 也不建目录
    /// （Inoreader 用户 tag 是标签不是分类）。
    #[test]
    fn category_name_rejects_state_and_user_tags() {
        for id in [
            "user/-/state/com.google/read",
            "user/-/state/com.google/starred",
            "user/-/state/com.google/reading-list",
            "user/-/state/org.freshrss/main",
        ] {
            assert_eq!(
                category_name(id, None, None),
                None,
                "state tag 不得建目录: {id}"
            );
        }
        assert_eq!(
            category_name("user/-/label/tag-only", Some("tag-only"), Some("tag")),
            None,
            "type=tag 的用户标签不是分类"
        );
        // label 前缀但后缀为空 → 不成名，拒绝
        assert_eq!(category_name("user/-/label/", Some(""), None), None);
    }

    /// 目录名 → wire stream id：无条件前置一次；字面量名字不得被误判为已格式化。
    #[test]
    fn label_stream_id_always_prefixes_once() {
        assert_eq!(label_stream_id("技术/阅读"), "user/-/label/技术/阅读");
        assert_eq!(
            label_stream_id("user/-/label/伪装"),
            "user/-/label/user/-/label/伪装"
        );
    }

    #[test]
    fn category_label_from_id_edge_cases() {
        assert_eq!(category_label_from_id("user/-/label/a/b/c"), Some("a/b/c"));
        assert_eq!(category_label_from_id("feed/42"), None);
        assert_eq!(category_label_from_id("user/1/state/com.google/read"), None);
        assert_eq!(category_label_from_id("user/-/label/"), None);
    }

    #[test]
    fn has_tag_matches_suffix() {
        let cats = vec![
            "user/2/state/com.google/reading-list".to_string(),
            "user/2/label/图片".to_string(),
            "user/2/state/com.google/read".to_string(),
        ];
        assert!(has_tag(&cats, "/com.google/read"));
        assert!(!has_tag(&cats, "/com.google/starred"));
    }
}
