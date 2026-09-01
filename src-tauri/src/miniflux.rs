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
    pub icon_url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CategoryRef {
    pub id: i64,
    pub title: String,
}

#[derive(Debug, Deserialize)]
pub struct FeedListResponse {
    pub feeds: Vec<Feed>,
    pub total: i64,
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
        // 标准格式 {"feeds": [...], "total": N}；部分版本/反代返回裸数组，两种都兼容
        match self.get_json::<FeedListResponse>("/v1/feeds").await {
            Ok(r) => Ok(r.feeds),
            Err(_) => Ok(self.get_json::<Vec<Feed>>("/v1/feeds").await?),
        }
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
    /// 幂等：服务端已存在（409）时回查 /v1/feeds 按 URL 找到既有 feed_id
    /// 返回（真实 Miniflux 409 响应体里带 feed_id，优先取响应体，
    /// 兜底走列表回查）——同一 URL 重复推送不构成错误，视为"已同步"。
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
        if status.as_u16() == 409 {
            // 已存在：先试响应体里的 feed_id（真实服务端行为）
            #[derive(Deserialize, Default)]
            struct Conflict {
                #[serde(default)]
                feed_id: Option<i64>,
            }
            if let Ok(c) = resp.json::<Conflict>().await {
                if let Some(id) = c.feed_id {
                    return Ok(id);
                }
            }
            // 兜底：列表回查
            let feeds = self.feeds().await?;
            return feeds
                .iter()
                .find(|f| f.feed_url == feed_url)
                .map(|f| f.id)
                .ok_or_else(|| AppError::network("409 但列表中找不到该订阅"));
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

    /// 移动订阅到分类：PUT /v1/feeds/{id}/category {category_id}
    pub async fn move_feed_category(&self, feed_id: i64, category_id: i64) -> AppResult<()> {
        #[derive(Serialize)]
        struct Body {
            category_id: i64,
        }
        let body = serde_json::to_string(&Body { category_id })?;
        self.send_expect_ok(
            reqwest::Method::PUT,
            &format!("/v1/feeds/{feed_id}/category"),
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
