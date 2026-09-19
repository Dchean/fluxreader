//! Fever API 客户端（v3，Miniflux 兼容）。
//!
//! 认证：`api_key = md5(username:password)`（十六进制小写），随 query 传。
//! Fever 的 username/password 与 Google Reader 集成凭据 **相同**（Miniflux 里
//! Fever 与 Google Reader 共用「集成」页配置）。
//!
//! 已用 curl 连真实 Miniflux（`https://sync.example.invalid/`）实证的要点：
//! - action 必须放在 URL query（`/fever/?api&feeds`），**不能**放 form body；
//!   `api_key` 与参数（`since_id`/`with_ids`/`id`/`as`/`mark`）放 query 或 form 均可。
//! - `items` 端点最多返回 **50 条**（升序），增量用 `since_id` 分页拉全。
//! - `unread_item_ids`/`saved_item_ids` 是权威**全量** id 集合（不受 50 条限制）。
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
use serde::Deserialize;
use std::collections::HashMap;

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
    #[serde(default)]
    items: Vec<FeverItem>,
    #[serde(default)]
    unread_item_ids: Option<String>,
    #[serde(default)]
    saved_item_ids: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FeverGroup {
    id: i64,
    #[serde(default)]
    title: String,
}

#[derive(Debug, Deserialize)]
struct FeverFeedsGroup {
    #[serde(default)]
    group_id: i64,
    #[serde(default)]
    feed_ids: String,
}

#[derive(Debug, Deserialize)]
struct FeverFeed {
    id: i64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    site_url: String,
}

#[derive(Debug, Deserialize)]
struct FeverItem {
    id: i64,
    #[serde(default)]
    feed_id: i64,
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

    /// API 入口（不含 query）。两种后端形态不同：
    /// - Miniflux：`{域名}/fever/?api`（协议规定的 `/fever/` 路径）；
    /// - FreshRSS：`{域名}/api/fever.php?api`（脚本路径，**不是** `/fever/` 形态）。
    ///
    /// 端点解析（TASK-059）会把 `base` 定成其中一种，这里据其形态拼出正确的入口。
    fn api_entry(&self) -> String {
        if self.base.ends_with(".php") {
            format!("{}?api", self.base)
        } else {
            format!("{}/fever/?api", self.base)
        }
    }

    /// POST 请求：action 拼 query + api_key 拼 query，返回信封并校验 `auth == 1`。
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
        url.push_str(&format!("&api_key={}", self.api_key));
        if !action.is_empty() {
            url.push_str(&format!("&{action}"));
        }
        for (k, v) in extra {
            url.push_str(&format!("&{k}={v}"));
        }
        let resp = self.http.post(&url).send().await?;
        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            return Ok((None, status));
        }
        let env: FeverEnvelope = resp.json().await?;
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
            return Err(AppError::new("auth", "Fever 认证失败（api_key 不正确）"));
        }
        Ok((Some(env), status))
    }

    /// 连通测试：认证明文。
    ///
    /// **端点自动适配（TASK-059）**：`new` 收到的 endpoint 可能是**纯域名**——
    /// 依次尝试 `{域名}`（Miniflux 的 `/fever/` 形态）与 `{域名}/api/fever.php`
    /// （FreshRSS 形态），以 **404 = 路径不存在**、**其它状态码 = 路径存在** 判定。
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
            group_title.insert(g.id, g.title.clone());
        }
        // feed_id → 第一个归属 group_id
        let mut feed_group: HashMap<i64, i64> = HashMap::new();
        for fg in &groups_env.feeds_groups {
            for fid in fg
                .feed_ids
                .split(',')
                .filter_map(|s| s.trim().parse::<i64>().ok())
            {
                feed_group.entry(fid).or_insert(fg.group_id);
            }
        }

        Ok(feeds_env
            .feeds
            .into_iter()
            .map(|f| {
                let categories = feed_group
                    .get(&f.id)
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
                    id: format!("feed/{}", f.id),
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
    pub async fn unread_item_ids(&self) -> AppResult<Vec<i64>> {
        let env = self.call("unread_item_ids", &[]).await?;
        Ok(parse_csv_ids(env.unread_item_ids.as_deref()))
    }

    /// 权威收藏条目 id 全量集合。
    pub async fn saved_item_ids(&self) -> AppResult<Vec<i64>> {
        let env = self.call("saved_item_ids", &[]).await?;
        Ok(parse_csv_ids(env.saved_item_ids.as_deref()))
    }

    /// 增量拉条目：`id > since_id`（Miniflux 限制 50 条升序，调用方需分页到拿完）。
    /// 注意 `since_id=0` 是无效值（返回默认最近 50 条），首次同步请用 [`Self::items_recent`]。
    pub async fn items_since(&self, since_id: i64) -> AppResult<Vec<ItemContent>> {
        let env = self
            .call("items", &[("since_id", since_id.to_string())])
            .await?;
        Ok(env
            .items
            .into_iter()
            .map(fever_item_to_item_content)
            .collect())
    }

    /// 拉最近条目（items 无参数，Miniflux 返回最近 50 条，未读优先）。
    /// 仅用于 Fever 首次同步的已读种子；未读/收藏由 `unread_item_ids`/
    /// `saved_item_ids` + `items_with_ids` 补齐。
    pub async fn items_recent(&self) -> AppResult<Vec<ItemContent>> {
        let env = self.call("items", &[]).await?;
        Ok(env
            .items
            .into_iter()
            .map(fever_item_to_item_content)
            .collect())
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
        Ok(env
            .items
            .into_iter()
            .map(fever_item_to_item_content)
            .collect())
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
            url.push_str(&format!("&api_key={}", self.api_key));
            let resp = self.http.post(&url).send().await?;
            if !resp.status().is_success() {
                return Err(AppError::network(format!(
                    "Fever mark {mark} {id} → {}",
                    resp.status()
                )));
            }
            let env: FeverEnvelope = resp.json().await?;
            if env.auth != 1 {
                return Err(AppError::new("auth", "Fever 认证失败"));
            }
        }
        Ok(())
    }
}

/* ============================================================
映射辅助
============================================================ */

fn parse_csv_ids(s: Option<&str>) -> Vec<i64> {
    s.unwrap_or("")
        .split(',')
        .filter_map(|x| x.trim().parse().ok())
        .collect()
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
        id: item.id.to_string(),
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
            stream_id: format!("feed/{}", item.feed_id),
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
    fn parse_csv_handles_empty_and_trailing() {
        assert!(parse_csv_ids(None).is_empty());
        assert_eq!(parse_csv_ids(Some("")), Vec::<i64>::new());
        assert_eq!(
            parse_csv_ids(Some("5699,5700,5711")),
            vec![5699, 5700, 5711]
        );
    }

    #[test]
    fn item_maps_read_and_saved_to_tags() {
        let item = FeverItem {
            id: 5705,
            feed_id: 21,
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
