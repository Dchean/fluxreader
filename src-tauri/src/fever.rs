//! Fever API 客户端（v3，Miniflux 兼容）。
//!
//! 认证：`api_key = md5(username:password)`（十六进制小写），随 query 传。
//! Fever 的 username/password 与 Google Reader 集成凭据 **相同**（Miniflux 里
//! Fever 与 Google Reader 共用「集成」页配置）。
//!
//! 已用 curl 连真实 Miniflux（`https://rss.chean.top/`）实证的要点：
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
    CategoryRef, ContentBlock, EnclosureRef, ItemContent, LinkRef, OriginRef, Subscription, TagRef,
    tags,
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

/// 反序列化「数字或数字字符串」→ i64。
/// FreshRSS 的 entry id 是 16 位微秒时间戳，PHP 以字符串返回（`getItems()` 里
/// `'id' => $entry->id()`，entry id 本身是 numeric-string）；Miniflux 返回 JSON 数字
/// （Go int64）。两处都要能解析。
fn flex_i64<'de, D>(de: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    let v = serde_json::Value::deserialize(de)?;
    match v {
        serde_json::Value::Number(n) => n
            .as_i64()
            .ok_or_else(|| D::Error::custom("id 不是有效 i64")),
        serde_json::Value::String(s) => s
            .parse::<i64>()
            .map_err(|_| D::Error::custom(format!("id 字符串不是有效数字：{s}"))),
        _ => Err(D::Error::custom("id 必须是数字或数字字符串")),
    }
}

#[derive(Debug, Deserialize)]
struct FeverItem {
    #[serde(deserialize_with = "flex_i64")]
    id: i64,
    #[serde(default, deserialize_with = "flex_i64")]
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
    /// `endpoint` 是已按 ServerKind 推导的 Fever API base（Miniflux `{root}/fever/`、
    /// FreshRSS `{root}/api/fever.php`）。保留尾斜杠——Miniflux 的 Fever 路由挂载在
    /// `/fever/`（尾斜杠是路径一部分），去掉会导致 `/fever?api` 404。
    pub fn new(endpoint: &str, username: &str, password: &str, http: Client) -> Self {
        // Fever 规范：api_key = md5("username:password") 十六进制小写
        let digest = Md5::digest(format!("{username}:{password}").as_bytes());
        let api_key = format!("{digest:x}");
        Self {
            base: endpoint.to_string(),
            api_key,
            http,
        }
    }

    /// POST 请求：action + 带值参数（since_id/with_ids/max_id）拼 query，
    /// `api_key` 放 body。这是 Miniflux 与 FreshRSS 的交集（逐行核对服务端源码）：
    ///
    /// - Miniflux `handleItems` 用 `QueryStringParam`/`HasQueryParam` **只读 query**
    ///   （`since_id`/`with_ids`/`max_id` 放 body 会被静默忽略，退化为最近 50 条）；
    /// - FreshRSS `fever.php` 用 `$_REQUEST`（query/body 均可）读参数，`$_POST['api_key']`
    ///   **只读 body**。故参数进 query、api_key 进 body 双方都能过。
    ///
    /// 返回信封并校验 `auth == 1`。
    /// `base` 是已按 ServerKind 推导的 Fever 端点（Miniflux `{root}/fever/`、
    /// FreshRSS `{root}/api/fever.php`）。
    /// `action` 形如 `feeds` / `groups` / `items` / `unread_item_ids`（无值参数）。
    /// `extra` 是 `since_id=5900` / `with_ids=1,2` 这类带值参数（进 query）。
    async fn call(&self, action: &str, extra: &[(&str, String)]) -> AppResult<FeverEnvelope> {
        // URL：`{base}?api` + action + 带值参数全在 query。
        let mut url = format!("{}?api", self.base);
        if !action.is_empty() {
            url.push_str(&format!("&{action}"));
        }
        for (k, v) in extra {
            url.push_str(&format!("&{k}={v}"));
        }
        // body：仅 api_key（FreshRSS 只读 `$_POST['api_key']`）。
        let resp = self
            .http
            .post(&url)
            .form(&[("api_key", self.api_key.clone())])
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(AppError::network(format!(
                "Fever {action} → {}",
                resp.status()
            )));
        }
        let env: FeverEnvelope = resp.json().await?;
        // Fever 规范基线是 v3；Miniflux 返回 3，FreshRSS 返回 4（其内部 API_LEVEL）。
        // 只拒绝明确不兼容的 < 3，>= 3 均接受（向后兼容）。
        if env.api_version < 3 {
            return Err(AppError::new(
                "protocol",
                format!("不支持的 Fever API 版本 {}", env.api_version),
            ));
        }
        if env.auth != 1 {
            return Err(AppError::new("auth", "Fever 认证失败（api_key 不正确）"));
        }
        Ok(env)
    }

    /// 连通测试：`/fever/?api` 认证明文。
    pub async fn verify(&self) -> AppResult<()> {
        self.call("", &[]).await.map(|_| ())
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
        Ok(env.items.into_iter().map(fever_item_to_item_content).collect())
    }

    /// 拉最近条目（items 无参数，Miniflux 返回最近 50 条，未读优先）。
    /// 仅用于 Fever 首次同步的已读种子；未读/收藏由 `unread_item_ids`/
    /// `saved_item_ids` + `items_with_ids` 补齐。
    pub async fn items_recent(&self) -> AppResult<Vec<ItemContent>> {
        let env = self.call("items", &[]).await?;
        Ok(env.items.into_iter().map(fever_item_to_item_content).collect())
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
        Ok(env.items.into_iter().map(fever_item_to_item_content).collect())
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
            // URL：`{base}?api&mark=item&as={mark}&id={id}`（action/flag 在 query）；
            // api_key 放 body（FreshRSS 只读 $_POST['api_key']）。
            let url = format!("{}?api&mark=item&as={mark}&id={id}", self.base);
            let resp = self
                .http
                .post(&url)
                .form(&[("api_key", self.api_key.clone())])
                .send()
                .await?;
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
        // 与 `python3 -c "hashlib.md5(b'test:testtest').hexdigest()"` 一致
        let c = FeverClient::new("https://rss.chean.top", "test", "testtest", Client::new());
        assert_eq!(c.api_key, "c328c2c122f5415f68ab71a042936fe5");
    }

    #[test]
    fn parse_csv_handles_empty_and_trailing() {
        assert!(parse_csv_ids(None).is_empty());
        assert_eq!(parse_csv_ids(Some("")), Vec::<i64>::new());
        assert_eq!(parse_csv_ids(Some("5699,5700,5711")), vec![5699, 5700, 5711]);
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
        assert!(crate::greader::has_tag(&ic.categories, "/com.google/starred"));
        assert_eq!(
            ic.origin.as_ref().map(|o| o.stream_id.as_str()),
            Some("feed/21")
        );
        assert_eq!(ic.published, 1788000000);
    }
}