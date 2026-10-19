//! Fever API 客户端（v3，Miniflux 兼容）。
//!
//! 认证：`api_key = md5(username:password)`（十六进制小写），随 **POST form body**
//! 传（application/x-www-form-urlencoded；TASK-101——FreshRSS 的 fever.php 只读
//! `$_POST['api_key']`，api_key 拼 query 会被 FreshRSS 当作空）。公式不变。
//! Fever 的 username/password 与 Google Reader 集成凭据 **相同**（Miniflux 里
//! Fever 与 Google Reader 共用「集成」页配置；FreshRSS 用个人设置里的「API 密码」）。
//!
//! 已用 curl 连真实 Miniflux（`https://sync.example.invalid/`）实证的要点：
//! - action 必须放在 URL query（`/fever/?api`），**不能**放 form body；
//!   其余参数（`since_id`/`with_ids`/`id`/`as`/`mark`）放 query 或 form 均可。
//! - `api_key`：Miniflux（`r.FormValue`）query 与 form body 都收，**FreshRSS 只收
//!   form body**（`p/api/fever.php:172` 只读 `$_POST['api_key']`）——故统一走
//!   form body（TASK-101），其余参数留在 query（两类后端的 `$_REQUEST`/FormValue
//!   都收）。顺带消除 api_key 进服务器访问日志的泄露面。
//! - `items` 端点最多返回 **50 条**/页；`since_id` 向更新方向分页增量，
//!   `max_id` 向**更旧**方向分页（`id < max_id`）。两家**固定实现**的 max_id
//!   页均为 `ORDER BY id DESC`（FreshRSS `p/api/fever.php` 的 `findEntries`；
//!   Miniflux `internal/fever/handler.go` 约 227-267 行），即返回紧邻 max_id
//!   的最近 50 条；客户端游标取**页内最小 id**，与页内顺序无关（升序页只在
//!   测试夹具中作泛化鲁棒性对照，**不是**任何固定实现的事实）。首/全量同步
//!   循环 `items_before` 直到返回空数组，即取尽服务端保留历史（TASK-125 审计
//!   P2-8；「历史是否仍被服务端保留另当别论」）；分页失败 / 页预算到达 /
//!   游标不前进都保留 checkpoint，下一次同步（含自动 light）自动续取。
//! - **资源界限**：响应体读取有 16 MiB 上限（`MAX_RESPONSE_BYTES`），超限
//!   显式协议错误；历史回溯在 `sync/fever_pull.rs` 逐页落库并释放正文，
//!   不累计全历史。
//! - **id 类型（审计 F03）**：FreshRSS 的 Fever 端把 64 位 id 作为 PHP
//!   numeric-string 序列化（`json_encode` 输出 JSON **字符串**，如
//!   `"1791440000000000"`），Miniflux 输出 JSON **数字**；`id`/`feed_id`/
//!   `group_id` 两种形态都接受，一律按**十进制**解析——绝不按十六进制猜，
//!   也不经 f64 中转。非法/溢出/负值显式报错（`FeverId`）。
//! - `unread_item_ids`/`saved_item_ids` 是权威**全量** id 集合（不受 50 条限制）；
//!   响应缺该字段是协议错误，**不得当空集合**（空集合会触发双向对账清状态）。
//! - `mark=item` 只接受**单个** id（逗号分隔无效），推送需逐个条目调用。
//! - 不支持添加订阅（Fever 协议无写订阅端点）——`quick_add` 由上层降级处理。
//!
//! 结构映射：Fever 的 item id 即 Miniflux entry id（十进制），与 Google Reader
//! `stream/items/ids` 返回的十进制 id 同源，`remote_id` 语义通用。

use crate::error::{AppError, AppResult};
use crate::greader::{
    tags, CategoryRef, ContentBlock, EnclosureRef, ItemContent, LinkRef, OriginRef, Subscription,
    TagRef,
};
use md5::{Digest, Md5};
use reqwest::Client;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer};
use std::collections::HashMap;
use std::fmt;

/* ============================================================
Fever 数值 id（审计 F03）

FreshRSS 的 Fever 端把 64 位 id 作为 PHP numeric-string 序列化
（`json_encode` 输出 JSON 字符串，如 `"1791440000000000"`）；Miniflux 输出
JSON 数字；greader 侧的 `stream/items/ids` 同样返回十进制字符串。
两种形态都按**十进制**解析——**绝不按十六进制猜**（16 位纯数字若按十六进制
解释会静默得到另一个 id，remote_id 与状态全部对不上），也不经 f64 中转
（大整数会失真）。非法（含 `0x` 前缀/空串/非数字）、负数、超出 i64 一律
显式报错，不静默过滤。
============================================================ */

// Note: 两种 JSON 形态（数字/十进制字符串）都按十进制、绝不猜十六进制 — 见 .agents/notes/implemented/architecture/2026-10-08-Fever身份与历史回溯.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct FeverId(i64);

impl FeverId {
    fn get(self) -> i64 {
        self.0
    }
}

/// 十进制字符串 → id：必须全部是 ASCII 数字（拒绝 `0x`、`-`、空串、空白内嵌）。
fn parse_decimal_id(raw: &str) -> Result<i64, String> {
    let t = raw.trim();
    if t.is_empty() {
        return Err("Fever ID 字符串为空".to_string());
    }
    if !t.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!(
            "Fever ID 不是十进制数字字符串：{raw:?}（不得按十六进制解释）"
        ));
    }
    t.parse::<i64>()
        .map_err(|_| format!("Fever ID 超出 i64 范围：{raw:?}"))
}

impl<'de> Deserialize<'de> for FeverId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct IdVisitor;

        impl<'de> Visitor<'de> for IdVisitor {
            type Value = FeverId;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("Fever ID（非负十进制整数或十进制数字字符串）")
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<FeverId, E> {
                if v < 0 {
                    return Err(E::custom(format!("Fever ID 不得为负值：{v}")));
                }
                Ok(FeverId(v))
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<FeverId, E> {
                i64::try_from(v)
                    .map(FeverId)
                    .map_err(|_| E::custom(format!("Fever ID 超出 i64 范围：{v}")))
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<FeverId, E> {
                parse_decimal_id(v).map(FeverId).map_err(E::custom)
            }
        }

        deserializer.deserialize_any(IdVisitor)
    }
}

/* ============================================================
Fever 原始响应（JSON）
============================================================ */

#[derive(Debug, Deserialize)]
struct FeverEnvelope {
    #[serde(default)]
    api_version: i64,
    #[serde(default)]
    auth: i64,
    #[serde(default)]
    groups: Vec<FeverGroup>,
    #[serde(default)]
    feeds_groups: Vec<FeverFeedsGroup>,
    #[serde(default)]
    feeds: Vec<FeverFeed>,
    /// `items` 方法必须带 `items` 字段——**缺失 ≠ 空数组**（缺失是协议错误，
    /// 不得静默当作「没有条目」耗尽历史）。非 items 方法（认证探针等）响应
    /// 不含该字段，故为 Option。
    #[serde(default)]
    items: Option<Vec<FeverItem>>,
    /// 同 `items`：权威集合方法响应缺该字段是协议错误，不得当空集合
    /// （空集合会触发双向对账把本地状态清掉）。
    #[serde(default)]
    unread_item_ids: Option<String>,
    #[serde(default)]
    saved_item_ids: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FeverGroup {
    id: FeverId,
    #[serde(default)]
    title: String,
}

#[derive(Debug, Deserialize)]
struct FeverFeedsGroup {
    group_id: FeverId,
    #[serde(default)]
    feed_ids: String,
}

#[derive(Debug, Deserialize)]
struct FeverFeed {
    id: FeverId,
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    site_url: String,
}

#[derive(Debug, Deserialize)]
struct FeverItem {
    id: FeverId,
    #[serde(default)]
    feed_id: FeverId,
    #[serde(default)]
    title: String,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    html: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    is_saved: i64,
    #[serde(default)]
    is_read: i64,
    #[serde(default)]
    created_on_time: i64,
}

/* ============================================================
客户端
============================================================ */

/// Fever 响应体读取上限：16 MiB。单页 50 条的正常响应远小于此；超过即视为
/// 协议/实现异常，显式失败而不是把整段读入内存再解析。
///
/// 实现说明（R2）：`Content-Length` 预检（快路径）+ `Response::chunk()` 逐块
/// 累计的**读取中硬界**；chunk 上自动解压透明生效，覆盖 chunked（无长度）与
/// 压缩膨胀两种形态。不新增 reqwest feature/依赖。
const MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;

/// 读取并解析 Fever 信封（带 16 MiB 上限）。
///
/// R2：按 `Response::chunk()` **逐块累计**（reqwest 的 chunk 无需 `stream`
/// feature；解压也在其上透明生效）——不是「先 `bytes()` 读全再量长度」。
/// Content-Length 只是提前拒绝的快路径；无长度（chunked）/压缩（解压后膨胀）
/// 的响应同样在读取过程中被硬界拦住。
async fn read_envelope(mut resp: reqwest::Response, action: &str) -> AppResult<FeverEnvelope> {
    let label = if action.is_empty() { "探针" } else { action };
    if let Some(len) = resp.content_length() {
        if len > MAX_RESPONSE_BYTES {
            return Err(AppError::new(
                "protocol",
                format!("Fever {label} 响应超过 16 MiB 上限（Content-Length={len}）"),
            ));
        }
    }
    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        if body.len() as u64 + chunk.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(AppError::new(
                "protocol",
                format!(
                    "Fever {label} 响应超过 16 MiB 上限（读取中累计超过 {} 字节）",
                    MAX_RESPONSE_BYTES
                ),
            ));
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body)
        .map_err(|e| AppError::new("protocol", format!("Fever {label} 响应解析失败：{e}")))
}

/// TASK-101：认证失败统一口径（`call_probe` 与 `mark_items` 两处共用）。
/// FreshRSS 的 Fever 与 GReader 都使用个人设置里的「API 密码」——提示用户
/// 别拿登录密码试 Fever。
const AUTH_FAILED_MSG: &str =
    "Fever 认证失败（api_key 不正确；FreshRSS 请使用个人设置里的「API 密码」）";

#[derive(Clone)]
pub struct FeverClient {
    base: String,
    api_key: String,
    http: Client,
}

impl FeverClient {
    pub fn new(endpoint: &str, username: &str, password: &str, http: Client) -> Self {
        // Fever 规范：api_key = md5("username:password") 十六进制小写
        let digest = Md5::digest(format!("{username}:{password}").as_bytes());
        let api_key = format!("{digest:x}");
        Self {
            base: endpoint.trim_end_matches('/').to_string(),
            api_key,
            http,
        }
    }

    /// API 入口（不含 query）。各后端形态不同：
    /// - Miniflux：`{域名}/fever/?api`（协议规定的 `/fever/` 路径）；
    /// - FreshRSS：`{域名}/api/fever.php?api`（脚本路径，**不是** `/fever/` 形态；
    ///   新版布局移到 `{域名}/p/api/fever.php`，TASK-101）。
    ///
    /// 端点解析（TASK-059/101）会把 `base` 定成其中一种，这里据其形态拼出正确的入口。
    fn api_entry(&self) -> String {
        if self.base.ends_with(".php") {
            format!("{}?api", self.base)
        } else {
            format!("{}/fever/?api", self.base)
        }
    }

    /// POST 请求：action 与其余参数拼 query、api_key 走 POST form body，
    /// 返回信封并校验 `auth == 1`。
    /// `action` 形如 `feeds` / `groups` / `items` / `unread_item_ids`（无值参数）。
    /// `extra` 是 `since_id=5900` / `with_ids=1,2` 这类带值参数。
    async fn call(&self, action: &str, extra: &[(&str, String)]) -> AppResult<FeverEnvelope> {
        let (env, status) = self.call_probe(action, extra).await?;
        match env {
            Some(env) => Ok(env),
            None => Err(AppError::network(format!("Fever {action} → {status}"))),
        }
    }

    /// 同 `call`，但**把 HTTP 状态码交回调用方**——端点解析需要区分
    /// 「404 路径不存在」与「路径正确但凭据错」，不能靠解析错误字符串来判断。
    ///
    /// 返回 `Ok((Some(env), status))` 表示成功；`Ok((None, status))` 表示
    /// HTTP 层失败（调用方据 `status` 判定路径是否存在）；`Err` 表示传输层错误。
    async fn call_probe(
        &self,
        action: &str,
        extra: &[(&str, String)],
    ) -> AppResult<(Option<FeverEnvelope>, u16)> {
        let mut url = self.api_entry();
        if !action.is_empty() {
            url.push_str(&format!("&{action}"));
        }
        for (k, v) in extra {
            url.push_str(&format!("&{k}={v}"));
        }
        // TASK-101：api_key 必须走 POST form body（application/x-www-form-urlencoded）。
        // FreshRSS 的 p/api/fever.php:172 只读 `$_POST['api_key']`、完全不读 query——
        // api_key 拼 query 时 FreshRSS 收到的恒为空 → auth 恒 0（「api_key 不正确」）。
        // Miniflux 用 r.FormValue 取值，query 与 form body 都收，形状兼容不回退。
        // action/mark/as/id/with_ids/since_id 等留在 query（两类后端均走 $_REQUEST/
        // FormValue）；api_key 不再出现在 URL，顺带消除进服务器访问日志的泄露面。
        let resp = self
            .http
            .post(&url)
            .form(&[("api_key", self.api_key.as_str())])
            .send()
            .await?;
        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            return Ok((None, status));
        }
        let env: FeverEnvelope = read_envelope(resp, action).await?;
        // TASK-059（owner 授权放宽）：原为 `!= 3` 即拒绝，但 FreshRSS 的 Fever 实测返回
        // `{"api_version":4,"auth":0}`——它用 4 表示自身实现版本，而**信封结构与 v3 一致**
        // （`api_version` + `auth` 两字段语义不变）。故改为**兼容 3 及以上**。
        // **不放松 `auth` 校验**（见下）：认证失败仍必须报错。
        if env.api_version < 3 {
            return Err(AppError::new(
                "protocol",
                format!("不支持的 Fever API 版本 {}", env.api_version),
            ));
        }
        if env.auth != 1 {
            return Err(AppError::new("auth", AUTH_FAILED_MSG));
        }
        Ok((Some(env), status))
    }

    /// 连通测试：认证明文。
    ///
    /// **端点自动适配（TASK-059）**：`new` 收到的 endpoint 可能是**纯域名**——
    /// 依次尝试 `{域名}`（Miniflux 的 `/fever/` 形态）、`{域名}/api/fever.php`
    /// （FreshRSS 形态）与 `{域名}/p/api/fever.php`（FreshRSS 新版布局，TASK-101），
    /// 以 **404 = 路径不存在**、**其它状态码 = 路径存在** 判定。
    ///
    /// **凭据错误必须立即停下**：非 404 的失败（含 `auth != 1`）都说明**路径已找对**，
    /// 继续试下一个候选只会掩盖真实原因（把密码错报成「找不到 API」）。
    ///
    /// 需要「解析出来的地址」时用 [`FeverClient::resolve`]——同步侧就是这么做的。
    pub async fn verify(&self) -> AppResult<()> {
        self.resolve().await.map(|_| ())
    }

    /// **端点解析并采用结果**：探测候选地址，返回 **base 已确定为 API 根**的客户端。
    ///
    /// `verify` 只回答「能不能连」，而同步真正需要的是「该用哪个地址连」——探测成功后
    /// 必须把地址**带出去**（只 `map(|_| ())` 会把解析结果丢掉，同步侧仍拿纯域名拼
    /// `{域名}/fever/?api`，在 FreshRSS 上依旧 404）。
    pub async fn resolve(&self) -> AppResult<Self> {
        let candidates = crate::endpoint_resolve::fever_candidates(&self.base);
        let mut last_status: Option<u16> = None;

        for base in &candidates {
            let candidate = self.at_resolved(base);
            let (env, status) = candidate.call_probe("", &[]).await?;
            if env.is_some() {
                return Ok(candidate);
            }
            last_status = Some(status);
            if !crate::endpoint_resolve::path_exists(status) {
                continue; // 404：该候选下没有 Fever API，试下一个
            }
            // 路径存在但请求被拒（多为凭据问题）——立即停下如实报错
            return Err(AppError::network(format!(
                "Fever 认证被拒 → {status}（已定位 API：{base}）"
            )));
        }

        Err(AppError::network(match last_status {
            Some(_) => format!(
                "在该地址下找不到 Fever API（HTTP 404，已尝试：{}）。请确认域名是否正确",
                candidates.join("、")
            ),
            None => "没有可用的 API 地址".to_string(),
        }))
    }

    /// 生成 base 已被替换为**已解析** API 根的客户端（不再探测）。
    ///
    /// 供同步侧消费缓存：同一个 endpoint 被反复 `build_client` 时直接命中，
    /// 不重复发探测请求。
    pub fn at_resolved(&self, base: &str) -> Self {
        Self {
            base: base.trim().trim_end_matches('/').to_string(),
            api_key: self.api_key.clone(),
            http: self.http.clone(),
        }
    }

    /// 实际使用的 API 根（TASK-059 端点自动适配的结果）。
    pub fn resolved_base(&self) -> &str {
        &self.base
    }

    /// 拉分组（分类）→ 统一 `TagRef`（folder 类型，label = group title）。
    pub async fn tags(&self) -> AppResult<Vec<TagRef>> {
        let env = self.call("groups", &[]).await?;
        Ok(env
            .groups
            .into_iter()
            .map(|g| TagRef {
                id: format!("user/-/label/{}", g.title),
                label: Some(g.title),
                r#type: Some("folder".into()),
            })
            .collect())
    }

    /// 拉订阅列表（含分类归属，供 pull_feeds 的「分类挂到 folder」逻辑复用）。
    /// 需要 groups + feeds_groups 把 feed → group → title 串起来。
    pub async fn subscriptions(&self) -> AppResult<Vec<Subscription>> {
        let feeds_env = self.call("feeds", &[]).await?;
        let groups_env = self.call("groups", &[]).await?;

        let mut group_title: HashMap<i64, String> = HashMap::new();
        for g in &groups_env.groups {
            group_title.insert(g.id.get(), g.title.clone());
        }
        // feed_id → 第一个归属 group_id（feed_ids 是 CSV；非法项显式报错，
        // 不得 filter_map 静默跳过——那会把归属错误当成「无分类」悄悄落地）。
        let mut feed_group: HashMap<i64, i64> = HashMap::new();
        for fg in &groups_env.feeds_groups {
            for fid in parse_csv_ids(&fg.feed_ids, "feeds_groups.feed_ids")? {
                feed_group.entry(fid).or_insert(fg.group_id.get());
            }
        }

        Ok(feeds_env
            .feeds
            .into_iter()
            .map(|f| {
                let categories = feed_group
                    .get(&f.id.get())
                    .and_then(|gid| group_title.get(gid))
                    .map(|t| CategoryRef {
                        id: format!("user/-/label/{t}"),
                        label: Some(t.clone()),
                        r#type: Some("folder".into()),
                    })
                    .into_iter()
                    .collect();
                let html_url = if f.site_url.is_empty() {
                    None
                } else {
                    Some(f.site_url.clone())
                };
                Subscription {
                    id: format!("feed/{}", f.id.get()),
                    title: f.title,
                    categories,
                    url: f.url,
                    html_url,
                    icon_url: None,
                }
            })
            .collect())
    }

    /// 权威未读条目 id 全量集合。
    ///
    /// 响应**缺 `unread_item_ids` 字段是协议错误**（auth-only 响应只对认证
    /// 方法合法）：绝不能当空集合——空集合会在双向对账里被解释成「远端全部
    /// 未读消失 = 全变已读」，静默清掉本地状态。
    pub async fn unread_item_ids(&self) -> AppResult<Vec<i64>> {
        let env = self.call("unread_item_ids", &[]).await?;
        let raw = env.unread_item_ids.ok_or_else(|| {
            AppError::new(
                "protocol",
                "Fever unread_item_ids 响应缺少 unread_item_ids 字段（缺失不得当空集合）",
            )
        })?;
        parse_csv_ids(&raw, "unread_item_ids")
    }

    /// 权威收藏条目 id 全量集合（缺字段同样是协议错误，见 `unread_item_ids`）。
    pub async fn saved_item_ids(&self) -> AppResult<Vec<i64>> {
        let env = self.call("saved_item_ids", &[]).await?;
        let raw = env.saved_item_ids.ok_or_else(|| {
            AppError::new(
                "protocol",
                "Fever saved_item_ids 响应缺少 saved_item_ids 字段（缺失不得当空集合）",
            )
        })?;
        parse_csv_ids(&raw, "saved_item_ids")
    }

    /// 增量拉条目：`id > since_id`（服务端限制 50 条/页，调用方按页内最大 id
    /// 递增游标分页到拿完）。注意 `since_id=0` 是无效值（服务端返回默认最近
    /// 50 条），首次同步走 [`Self::items_before`] 的完整历史回溯。
    pub async fn items_since(&self, since_id: i64) -> AppResult<Vec<ItemContent>> {
        let env = self
            .call("items", &[("since_id", since_id.to_string())])
            .await?;
        parse_items(env.items)
    }

    /// 历史回溯：`id < max_id` 的最近一页（`max_id` 不超过 50 条/页）。
    ///
    /// 语义（协议/两家固定实现一致，见模块头）：「向更旧翻页」——调用方以
    /// **页内最小 id** 作为下一页游标（与页内顺序无关：FreshRSS DESC、
    /// Miniflux 升序），重复请求直到返回空数组即取尽服务端保留历史。
    pub async fn items_before(&self, max_id: i64) -> AppResult<Vec<ItemContent>> {
        let env = self
            .call("items", &[("max_id", max_id.to_string())])
            .await?;
        parse_items(env.items)
    }

    /// 拉最近条目（items 无参数，服务端返回最近 50 条）。
    ///
    /// 首/全量同步现走 [`Self::items_before`] 循环取尽历史（TASK-125 / OPT-005），
    /// 本方法保留为显式「只看最近窗口」接口。
    pub async fn items_recent(&self) -> AppResult<Vec<ItemContent>> {
        let env = self.call("items", &[]).await?;
        parse_items(env.items)
    }

    /// 按 id 精确拉条目正文。
    pub async fn items_with_ids(&self, ids: &[i64]) -> AppResult<Vec<ItemContent>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let csv = ids
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let env = self.call("items", &[("with_ids", csv)]).await?;
        parse_items(env.items)
    }

    /* ---------- 状态写入（mark=item，单个 id 逐个调用） ---------- */

    pub async fn mark_read(&self, ids: &[i64]) -> AppResult<()> {
        self.mark_items(ids, "read").await
    }
    pub async fn mark_unread(&self, ids: &[i64]) -> AppResult<()> {
        self.mark_items(ids, "unread").await
    }
    pub async fn mark_starred(&self, ids: &[i64]) -> AppResult<()> {
        self.mark_items(ids, "saved").await
    }
    pub async fn mark_unstarred(&self, ids: &[i64]) -> AppResult<()> {
        self.mark_items(ids, "unsaved").await
    }

    async fn mark_items(&self, ids: &[i64], mark: &str) -> AppResult<()> {
        for id in ids {
            // 必须走 `api_entry()`：FreshRSS 的端点是 `/api/fever.php`，
            // 若在此写死 `{base}/fever/?api` 会拼成 `…/api/fever.php/fever/?api` → 404，
            // 于是「Fever + FreshRSS」拉得到、推不出去（TASK-059 审查发现）。
            let mut url = self.api_entry();
            url.push_str(&format!("&mark=item&as={mark}&id={id}"));
            // TASK-101：api_key 与 call_probe 一致走 POST form body（FreshRSS 的
            // fever.php 只读 `$_POST['api_key']`——此前 api_key 拼 query、POST 空
            // body，FreshRSS 上「拉得到、推不出去」的认证侧根因）；mark/as/id 留
            // query（FreshRSS 走 $_REQUEST，Miniflux 实证 query 可用，不动）。
            let resp = self
                .http
                .post(&url)
                .form(&[("api_key", self.api_key.as_str())])
                .send()
                .await?;
            if !resp.status().is_success() {
                return Err(AppError::network(format!(
                    "Fever mark {mark} {id} → {}",
                    resp.status()
                )));
            }
            let env: FeverEnvelope = read_envelope(resp, "mark").await?;
            if env.auth != 1 {
                return Err(AppError::new("auth", AUTH_FAILED_MSG));
            }
        }
        Ok(())
    }
}

/* ============================================================
映射辅助
============================================================ */

/// `items` 方法响应 → 条目列表：缺 `items` 字段是协议错误（缺失 ≠ 空数组）。
fn parse_items(items: Option<Vec<FeverItem>>) -> AppResult<Vec<ItemContent>> {
    let items = items.ok_or_else(|| {
        AppError::new(
            "protocol",
            "Fever items 响应缺少 items 字段（缺失不得当空数组）",
        )
    })?;
    Ok(items.into_iter().map(fever_item_to_item_content).collect())
}

/// 逗号分隔的十进制 id 集合（`unread_item_ids` / `saved_item_ids` /
/// `feeds_groups.feed_ids`）。
///
/// 空串 = 空集合（合法）；其余任何非法项（非十进制、负数、溢出、空项）都
/// **显式报错**——绝不 `filter_map(..ok())` 静默过滤：那会把协议错误伪造成
/// 「更小的集合」，权威对账据此回写会把本地状态清错。
fn parse_csv_ids(raw: &str, field: &str) -> AppResult<Vec<i64>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for token in trimmed.split(',') {
        let t = token.trim();
        if t.is_empty() {
            return Err(AppError::new(
                "protocol",
                format!("Fever {field} 含空项：{raw:?}"),
            ));
        }
        let id = parse_decimal_id(t)
            .map_err(|e| AppError::new("protocol", format!("Fever {field} 非法：{e}")))?;
        out.push(id);
    }
    Ok(out)
}

/// Fever item → 统一 `ItemContent`（复用 greader 的状态 tag 语义）。
/// `is_read`/`is_saved` 映射为 categories 里的 read/starred tag，这样
/// sync.rs 里 `greader::has_tag(...)` 的合并/广播逻辑两种协议通用。
fn fever_item_to_item_content(item: FeverItem) -> ItemContent {
    let mut categories = vec![tags::READING_LIST.to_string()];
    if item.is_read == 1 {
        categories.push(tags::READ.to_string());
    }
    if item.is_saved == 1 {
        categories.push(tags::STARRED.to_string());
    }
    ItemContent {
        // 十进制字面值：与 FeverId 的整数定义一致（mark/with_ids/对账共用同一
        // 定义；下游 greader::parse_item_id 对无前缀纯数字同样按十进制解析）。
        id: item.id.get().to_string(),
        categories,
        title: item.title,
        author: item.author,
        published: item.created_on_time,
        updated: item.created_on_time,
        alternate: vec![LinkRef {
            href: item.url.clone().unwrap_or_default(),
            r#type: Some("text/html".into()),
        }],
        summary: None,
        content: Some(ContentBlock {
            content: item.html.unwrap_or_default(),
        }),
        origin: Some(OriginRef {
            stream_id: format!("feed/{}", item.feed_id.get()),
            title: String::new(),
            html_url: String::new(),
        }),
        enclosure: Vec::<EnclosureRef>::new(),
    }
}

/* ============================================================
单元测试
============================================================ */

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_key_is_md5_of_username_colon_password() {
        // 与 `python3 -c "hashlib.md5(b'REDACTED_USER:REDACTED_PASSWORD').hexdigest()"` 一致
        let c = FeverClient::new(
            "https://sync.example.invalid",
            "REDACTED_USER",
            "REDACTED_PASSWORD",
            Client::new(),
        );
        assert_eq!(c.api_key, "6ac0be0ba0aa8e8a4972b225d7cea926");
    }

    #[test]
    fn parse_csv_ids_accepts_empty_and_decimal_tokens() {
        assert_eq!(parse_csv_ids("", "x").unwrap(), Vec::<i64>::new());
        assert_eq!(
            parse_csv_ids("5699,5700,5711", "x").unwrap(),
            vec![5699, 5700, 5711]
        );
        // 周围空白容忍（服务端一般不产生，但容忍不影响语义）
        assert_eq!(parse_csv_ids(" 1 , 2 ", "x").unwrap(), vec![1, 2]);
    }

    /// 非法项必须显式报错，绝不静默过滤成「更小的集合」。
    #[test]
    fn parse_csv_ids_rejects_invalid_instead_of_filtering() {
        for bad in [
            "1,abc",
            "0x1F",
            "1,,2",
            "1,-2",
            "99999999999999999999",
            ",1",
            "1,",
        ] {
            assert!(
                parse_csv_ids(bad, "unread_item_ids").is_err(),
                "{bad:?} 应报错而不是被过滤"
            );
        }
    }

    /// F03：JSON 数字与十进制数字字符串都接受；不经 f64 中转失真。
    #[test]
    fn fever_id_accepts_number_and_decimal_string() {
        #[derive(Deserialize)]
        struct Probe {
            id: FeverId,
        }
        // 契约给出的长 id（FreshRSS numeric-string 形态）。
        let s: Probe = serde_json::from_str(r#"{"id":"1791440000000000"}"#).unwrap();
        let n: Probe = serde_json::from_str(r#"{"id":1791440000000000}"#).unwrap();
        assert_eq!(s.id.get(), 1791440000000000);
        assert_eq!(n.id.get(), 1791440000000000);
        // 2^53 + 1：f64 无法精确表示，必须走整数解析（证明不经 f64 中转）。
        let big: Probe = serde_json::from_str(r#"{"id":9007199254740993}"#).unwrap();
        assert_eq!(big.id.get(), 9007199254740993);
    }

    /// 非法/负值/溢出/浮点一律显式报错，不静默。
    #[test]
    fn fever_id_rejects_invalid_negative_overflow_and_float() {
        // 本测试只断言反序列化失败，字段不会被读取。
        #[derive(Deserialize)]
        struct Probe {
            #[allow(dead_code)]
            id: FeverId,
        }
        for bad in [
            r#"{"id":"0x1F"}"#,
            r#"{"id":"-5"}"#,
            r#"{"id":-5}"#,
            r#"{"id":""}"#,
            r#"{"id":"99999999999999999999"}"#,
            r#"{"id":99999999999999999999}"#,
            r#"{"id":1.5}"#,
        ] {
            assert!(
                serde_json::from_str::<Probe>(bad).is_err(),
                "{bad} 应显式报错"
            );
        }
    }

    #[test]
    fn item_maps_read_and_saved_to_tags() {
        let item = FeverItem {
            id: FeverId(5705),
            feed_id: FeverId(21),
            title: "t".into(),
            author: Some("a".into()),
            html: Some("<p>x</p>".into()),
            url: Some("https://x".into()),
            is_saved: 1,
            is_read: 1,
            created_on_time: 1788000000,
        };
        let ic = fever_item_to_item_content(item);
        assert_eq!(ic.id, "5705");
        assert!(crate::greader::has_tag(&ic.categories, "/com.google/read"));
        assert!(crate::greader::has_tag(
            &ic.categories,
            "/com.google/starred"
        ));
        assert_eq!(
            ic.origin.as_ref().map(|o| o.stream_id.as_str()),
            Some("feed/21")
        );
        assert_eq!(ic.published, 1788000000);
    }
}
