//! Miniflux REST API 客户端（v1）。
//! 认证：X-Auth-Token 头。全部走 state.http 共享连接池。
//! API 形状对齐 miniflux 2.x：/v1/me 验证、/v1/feeds、/v1/categories、
//! /v1/entries、PUT 状态/收藏。

use crate::error::{AppError, AppResult};
use reqwest::Client;
use serde::{Deserialize, Serialize};

/* ============================================================
   行类型（Miniflux JSON 的最小子集）
   ============================================================ */

#[derive(Debug, Deserialize)]
pub struct Me {
    pub id: i64,
    pub username: String,
}

#[derive(Debug, Deserialize)]
pub struct Category {
    pub id: i64,
    pub title: String,
}

#[derive(Debug, Deserialize)]
pub struct Feed {
    pub id: i64,
    pub feed_url: String,
    pub site_url: Option<String>,
    pub title: String,
    #[serde(default)]
    pub category: Option<CategoryRef>,
    /// favicon 引用（真实字段是 icon 对象 {feed_id, icon_id}，无 icon_url 字符串；
    /// icon 为 null 时表示无 favicon）
    #[serde(default)]
    pub icon: Option<FeedIcon>,
}

#[derive(Debug, Deserialize)]
pub struct FeedIcon {
    pub feed_id: i64,
    pub icon_id: i64,
    #[serde(default)]
    pub external_icon_id: String,
}

#[derive(Debug, Deserialize)]
pub struct CategoryRef {
    pub id: i64,
    pub title: String,
}

#[derive(Debug, Deserialize)]
pub struct Entry {
    pub id: i64,
    pub feed_id: i64,
    pub url: Option<String>,
    pub title: String,
    pub author: Option<String>,
    pub content: String,
    pub published_at: String,
    pub changed_at: String,
    pub status: String,           // "unread" | "read"
    #[serde(default)]
    pub starred: bool,
    /// 播客音频/视频附件（Miniflux API enclosures 数组）
    #[serde(default)]
    pub enclosures: Vec<Enclosure>,
}

/// 条目附件：播客音频/视频/封面图
#[derive(Debug, Clone, Deserialize)]
pub struct Enclosure {
    pub url: String,
    #[serde(default)]
    pub mime_type: String,
    /// 时长（秒）——Miniflux 在 itunes:duration 可解析时提供
    #[serde(default)]
    pub duration: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct EntryListResponse {
    pub total: i64,
    pub entries: Vec<Entry>,
}

/* ============================================================
   客户端
   ============================================================ */

pub struct MinifluxClient {
    base: String,
    token: String,
    http: Client,
}

impl MinifluxClient {
    pub fn new(endpoint: &str, token: &str, http: Client) -> Self {
        // 去尾部斜杠，后续路径拼接统一 "/v1/..."
        let base = endpoint.trim_end_matches('/').to_string();
        let token = token.to_string();
        Self { base, token, http }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    async fn get_json<T: for<'de> Deserialize<'de>>(&self, path: &str) -> AppResult<T> {
        let resp = self
            .http
            .get(self.url(path))
            .header("X-Auth-Token", &self.token)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(AppError::network(format!(
                "GET {path} → {}",
                resp.status()
            )));
        }
        Ok(resp.json().await?)
    }

    async fn send_expect_ok(&self, method: reqwest::Method, path: &str, body: Option<String>) -> AppResult<()> {
        let mut req = self
            .http
            .request(method, self.url(path))
            .header("X-Auth-Token", &self.token);
        if let Some(b) = body {
            req = req.header("Content-Type", "application/json").body(b);
        }
        let resp = req.send().await?;
        if !resp.status().is_success() {
            return Err(AppError::network(format!(
                "{path} → {}",
                resp.status()
            )));
        }
        Ok(())
    }

    /* ---------- 连接与只读 ---------- */

    /// 测试连接：GET /v1/me
    pub async fn me(&self) -> AppResult<Me> {
        self.get_json("/v1/me").await
    }

    pub async fn categories(&self) -> AppResult<Vec<Category>> {
        self.get_json("/v1/categories").await
    }

    pub async fn feeds(&self) -> AppResult<Vec<Feed>> {
        // 真实 Miniflux GET /v1/feeds 返回裸数组 [...]（handler 直接 response.JSON(w, r, feeds)）
        self.get_json("/v1/feeds").await
    }

    /// 拉条目：after/changed_after 为 unix **秒**（Miniflux API 文档如此；
    /// 传毫秒会被当作未来时间 → 永远返回空，增量同步静默失效）。
    /// order=id asc + offset 翻页保证稳定。
    pub async fn entries(&self, feed_id: Option<i64>, after_epoch_s: i64, changed: bool) -> AppResult<Vec<Entry>> {
        let mut path = format!(
            "/v1/entries?limit=100&order=id&direction=asc&{}={after_epoch_s}",
            if changed { "changed_after" } else { "after" }
        );
        if let Some(fid) = feed_id {
            path.push_str(&format!("&feed_id={fid}"));
        }
        let mut all = Vec::new();
        loop {
            let r: EntryListResponse = self.get_json(&path).await?;
            let got = r.entries.len();
            all.extend(r.entries);
            if got < 100 || r.total <= all.len() as i64 {
                break;
            }
            // offset 翻页（order=id asc 保证稳定）
            path = format!(
                "/v1/entries?limit=100&order=id&direction=asc&offset={}&{}={after_epoch_s}",
                all.len(),
                if changed { "changed_after" } else { "after" }
            );
            if let Some(fid) = feed_id {
                path.push_str(&format!("&feed_id={fid}"));
            }
        }
        Ok(all)
    }

    /// 拉指定状态的 entry id 列表（轻量对账用）。
    /// GET /v1/entries/ids?status=unread（或 read）→ {"total": N, "entry_ids": [...]}。
    /// 单次最多 10000 个 id（Miniflux MaxEntryIDsLimit），超量需 offset 翻页。
    /// 用于「未读/已读状态精确对齐」——比 changed_after 增量可靠，不受游标漂移影响。
    pub async fn entry_ids(&self, status: &str) -> AppResult<Vec<i64>> {
        let mut all: Vec<i64> = Vec::new();
        loop {
            let path = format!("/v1/entries/ids?status={status}&limit=10000&offset={}", all.len());
            #[derive(Deserialize)]
            struct IdsResp {
                #[serde(default)]
                entry_ids: Vec<i64>,
            }
            let r: IdsResp = self.get_json(&path).await?;
            let got = r.entry_ids.len();
            all.extend(r.entry_ids);
            if got < 10000 {
                break;
            }
        }
        Ok(all)
    }

    /* ---------- 写操作 ---------- */

    /// 批量更新条目状态：PUT /v1/entries
    pub async fn update_entries_status(&self, entry_ids: &[i64], status: &str) -> AppResult<()> {
        let body = serde_json::json!({ "entry_ids": entry_ids, "status": status }).to_string();
        self.send_expect_ok(reqwest::Method::PUT, "/v1/entries", Some(body))
            .await
    }

    /// 收藏/取消收藏：PUT /v1/entries/{id}/bookmark
    pub async fn toggle_bookmark(&self, entry_id: i64) -> AppResult<()> {
        self.send_expect_ok(
            reqwest::Method::PUT,
            &format!("/v1/entries/{entry_id}/bookmark"),
            None,
        )
        .await
    }

    /// 新增订阅：POST /v1/feeds {feed_url, category_id}。
    /// 幂等：服务端已存在同 URL 订阅时，Miniflux 返回 **400**（非 409）——
    /// 响应体 `{"error_message": "This feed already exists"}`（validator 的
    /// error.feed_already_exists 经 JSONBadRequest 返回）。此时回查 /v1/feeds
    /// 按 URL 找到既有 feed_id 返回，同一 URL 重复推送不构成错误。
    pub async fn create_feed(&self, feed_url: &str, category_id: i64) -> AppResult<i64> {
        #[derive(Serialize)]
        struct Body<'a> {
            feed_url: &'a str,
            category_id: i64,
        }
        let body = serde_json::to_string(&Body { feed_url, category_id })?;
        let resp = self
            .http
            .post(self.url("/v1/feeds"))
            .header("X-Auth-Token", &self.token)
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await?;
        let status = resp.status();
        if status.is_success() {
            #[derive(Deserialize)]
            struct Created {
                feed_id: i64,
            }
            let created: Created = resp.json().await?;
            return Ok(created.feed_id);
        }
        // Miniflux 对重复 feed 返回 400（validator.ValidateFeedCreation 的
        // error.feed_already_exists 经 JSONBadRequest 返回）。真实服务端也可能
        // 返回 409（旧版本/反代），两种都兼容：只要能确认「已存在」，就回查绑定。
        if status.as_u16() == 400 || status.as_u16() == 409 {
            let is_conflict = resp
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|v| v.get("error_message").and_then(|m| m.as_str()).map(String::from))
                .map(|msg| msg.to_ascii_lowercase().contains("already exists"))
                .unwrap_or(false)
                || status.as_u16() == 409;
            if is_conflict {
                // 兜底：列表回查，按 URL 找到既有 feed_id
                let feeds = self.feeds().await?;
                return feeds
                    .iter()
                    .find(|f| f.feed_url == feed_url)
                    .map(|f| f.id)
                    .ok_or_else(|| AppError::network("订阅已存在但列表中找不到该 URL"));
            }
        }
        Err(AppError::network(format!("POST /v1/feeds → {status}")))
    }

    /// 删除订阅：DELETE /v1/feeds/{id}
    pub async fn delete_feed(&self, feed_id: i64) -> AppResult<()> {
        self.send_expect_ok(reqwest::Method::DELETE, &format!("/v1/feeds/{feed_id}"), None)
            .await
    }

    /// 更新订阅标题：PUT /v1/feeds/{id} {title}（Miniflux 要求带 category_id）
    pub async fn update_feed_title(&self, feed_id: i64, title: &str, category_id: i64) -> AppResult<()> {
        #[derive(Serialize)]
        struct Body<'a> {
            title: &'a str,
            category_id: i64,
        }
        let body = serde_json::to_string(&Body { title, category_id })?;
        self.send_expect_ok(reqwest::Method::PUT, &format!("/v1/feeds/{feed_id}"), Some(body))
            .await
    }

    /// 移动订阅到分类：PUT /v1/feeds/{id} {category_id}。
    /// 注意：Miniflux 没有独立的 /v1/feeds/{id}/category 端点——移动分类与改名
    /// 共用 PUT /v1/feeds/{id}（Update Feed，body 可含 title 与 category_id）。
    pub async fn move_feed_category(&self, feed_id: i64, category_id: i64) -> AppResult<()> {
        #[derive(Serialize)]
        struct Body {
            category_id: i64,
        }
        let body = serde_json::to_string(&Body { category_id })?;
        self.send_expect_ok(
            reqwest::Method::PUT,
            &format!("/v1/feeds/{feed_id}"),
            Some(body),
        )
        .await
    }

    /// 改名分类：PUT /v1/categories/{id} {title}
    pub async fn rename_category(&self, category_id: i64, title: &str) -> AppResult<()> {
        #[derive(Serialize)]
        struct Body<'a> {
            title: &'a str,
        }
        let body = serde_json::to_string(&Body { title })?;
        self.send_expect_ok(
            reqwest::Method::PUT,
            &format!("/v1/categories/{category_id}"),
            Some(body),
        )
        .await
    }

    /// 新建分类：POST /v1/categories {title}
    pub async fn create_category(&self, title: &str) -> AppResult<i64> {
        #[derive(Serialize)]
        struct Body<'a> {
            title: &'a str,
        }
        let resp = self
            .http
            .post(self.url("/v1/categories"))
            .header("X-Auth-Token", &self.token)
            .header("Content-Type", "application/json")
            .body(serde_json::to_string(&Body { title })?)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(AppError::network(format!(
                "POST /v1/categories → {}",
                resp.status()
            )));
        }
        let cat: Category = resp.json().await?;
        Ok(cat.id)
    }
}
