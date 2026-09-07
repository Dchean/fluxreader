//! Google Reader API（兼容协议）客户端。
//!
//! 后端：Miniflux（新版原生支持 Google Reader 兼容层 + Fever API）。
//! 本模块当前**只实现 Google Reader 协议**（后端可换的基线）；Fever API 预留
//! 未实现——它是可选备胎，仅在 Google Reader 兼容层不可用时才需要补充
//! （`POST /fever/?api` + `api_key=md5(username:password)`）。
//! 认证：两步 ClientLogin 换取 auth token，后续请求用 `Authorization: GoogleLogin auth=<token>`（GET）
//! 或表单参数 `T=<token>`（POST）。
//!
//! 权威规范来源：Miniflux 源码 `internal/googlereader/`（README.md + middleware.go），
//! 并经 curl 连真实 Miniflux（`https://rss.chean.top/`）实证。要点：
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

#[derive(Clone)]
pub struct GReaderClient {
    /// 后端根 URL（去尾部斜杠），如 `https://rss.chean.top`
    base: String,
    /// ClientLogin 换取的 auth token（`username/hmac`）
    token: String,
    http: Client,
}

impl GReaderClient {
    /// 用已知的 auth token 构建（不重新 ClientLogin）。
    pub fn new(endpoint: &str, token: &str, http: Client) -> Self {
        let base = endpoint.trim_end_matches('/').to_string();
        Self { base, token: token.to_string(), http }
    }

    /// 两步认证：先 ClientLogin 换 token，再构建客户端。
    /// `username`/`password` 是 Google Reader 集成凭据（非 Miniflux 账号密码）。
    pub async fn login(endpoint: &str, username: &str, password: &str, http: Client) -> AppResult<Self> {
        let base = endpoint.trim_end_matches('/').to_string();
        let resp = http
            .post(format!("{base}/accounts/ClientLogin"))
            .form(&[("Email", username), ("Passwd", password), ("output", "json")])
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(AppError::network(format!(
                "ClientLogin → {}",
                resp.status()
            )));
        }
        let body: ClientLoginResponse = resp.json().await?;
        if body.auth.is_empty() {
            return Err(AppError::network("ClientLogin 响应缺少 Auth token"));
        }
        Ok(Self { base, token: body.auth, http })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
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

    /// POST 请求（表单 `T=<token>` 认证）。
    async fn post_form<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        form: &[(&str, String)],
    ) -> AppResult<T> {
        let mut params: Vec<(&str, String)> = vec![("T", self.token.clone())];
        params.extend_from_slice(form);
        let resp = self.http.post(self.url(path)).form(&params).send().await?;
        if !resp.status().is_success() {
            return Err(AppError::network(format!("POST {path} → {}", resp.status())));
        }
        Ok(resp.json().await?)
    }

    /// POST 请求返回纯文本（edit-tag 等返回 `OK`）。
    async fn post_form_text(&self, path: &str, form: &[(&str, String)]) -> AppResult<()> {
        let mut params: Vec<(&str, String)> = vec![("T", self.token.clone())];
        params.extend_from_slice(form);
        let resp = self.http.post(self.url(path)).form(&params).send().await?;
        if !resp.status().is_success() {
            return Err(AppError::network(format!("POST {path} → {}", resp.status())));
        }
        Ok(())
    }

    /* ---------- 连接与只读 ---------- */

    /// 拉订阅列表（含分类归属）。等价于 Miniflux `/v1/feeds` + `/v1/categories`。
    pub async fn subscriptions(&self) -> AppResult<Vec<Subscription>> {
        let r: SubscriptionListResponse =
            self.get_json("/reader/api/0/subscription/list?output=json").await?;
        Ok(r.subscriptions)
    }

    /// 拉标签列表（starred + 用户分类 label/folder）。
    pub async fn tags(&self) -> AppResult<Vec<TagRef>> {
        let r: TagListResponse =
            self.get_json("/reader/api/0/tag/list?output=json").await?;
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
        let r: ItemContentsResponse =
            self.post_form("/reader/api/0/stream/items/contents?output=json", &form).await?;
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

    /// 全部已读（某 stream 在 ts 之前）。
    pub async fn mark_all_read(&self, stream: &str, ts: Option<i64>) -> AppResult<()> {
        let mut form: Vec<(&str, String)> = vec![("s", stream.to_string())];
        if let Some(v) = ts {
            form.push(("ts", v.to_string()));
        }
        self.post_form_text("/reader/api/0/mark-all-as-read", &form).await
    }

    /// 订阅（`ac=subscribe`，`s=feed/<绝对URL>`）。返回新 feed 的数字 id（若有）。
    pub async fn subscribe(&self, feed_url: &str) -> AppResult<()> {
        let form: Vec<(&str, String)> = vec![
            ("ac", "subscribe".to_string()),
            ("s", format!("feed/{feed_url}")),
        ];
        self.post_form_text("/reader/api/0/subscription/edit", &form).await
    }

    /// 快速订阅（自动发现 feed，等价旧 `/v1/feeds` 创建 + 幂等）。
    pub async fn quick_add(&self, feed_url: &str) -> AppResult<QuickAddResponse> {
        let form: Vec<(&str, String)> = vec![("quickadd", feed_url.to_string())];
        self.post_form("/reader/api/0/subscription/quickadd", &form).await
    }

    /// 退订（`ac=unsubscribe`，`s=feed/<数字id>`）。
    pub async fn unsubscribe(&self, feed_numeric_id: i64) -> AppResult<()> {
        let form: Vec<(&str, String)> = vec![
            ("ac", "unsubscribe".to_string()),
            ("s", format!("feed/{feed_numeric_id}")),
        ];
        self.post_form_text("/reader/api/0/subscription/edit", &form).await
    }

    /// 编辑订阅（`ac=edit`，`s=feed/<数字id>`，可改标题 `t` 或移分类 `a`）。
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
        if let Some(a) = dest_label {
            form.push(("a", a.to_string()));
        }
        self.post_form_text("/reader/api/0/subscription/edit", &form).await
    }
}

/* ============================================================
   辅助：解析工具
   ============================================================ */

/// 从 `feed/42` 流 id 提取数字 id。
pub fn parse_feed_numeric_id(stream_id: &str) -> Option<i64> {
    stream_id.strip_prefix("feed/")?.parse().ok()
}

/// 从长格式 item id（`tag:google.com,2005:reader/item/0000000000001675`）提取十进制 id。
/// 长格式 id 的尾部是 16 位十六进制；也可直接是十进制（`12345`）。
pub fn parse_item_id(id: &str) -> Option<i64> {
    if let Some(hex) = id.rsplit('/').next() {
        // 16 位十六进制 → 十进制
        if hex.len() == 16 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
            if let Ok(v) = i64::from_str_radix(hex, 16) {
                return Some(v);
            }
        }
        // 纯十进制
        if let Ok(v) = hex.parse::<i64>() {
            return Some(v);
        }
    }
    id.parse::<i64>().ok()
}

/// 判断 item 的 categories 是否含某 tag（read/starred 状态判断）。
pub fn has_tag(categories: &[String], tag_suffix: &str) -> bool {
    categories.iter().any(|c| c.ends_with(tag_suffix))
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

    #[test]
    fn parse_feed_numeric_id_works() {
        assert_eq!(parse_feed_numeric_id("feed/42"), Some(42));
        assert_eq!(parse_feed_numeric_id("feed/0"), Some(0));
        assert_eq!(parse_feed_numeric_id("user/-/state/com.google/read"), None);
        assert_eq!(parse_feed_numeric_id("feed/abc"), None);
    }

    #[test]
    fn parse_item_id_handles_both_formats() {
        // 长格式：tag:google.com,2005:reader/item/0000000000001675 → 十进制 5749
        assert_eq!(parse_item_id("tag:google.com,2005:reader/item/0000000000001675"), Some(5749));
        // 十进制
        assert_eq!(parse_item_id("5749"), Some(5749));
        // 16 位十六进制（无前缀）
        assert_eq!(parse_item_id("0000000000001675"), Some(5749));
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
