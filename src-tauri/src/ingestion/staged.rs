//! 三段式刷新管线（并发安全版）：锁内读快照 → 锁外 HTTP+解析 → 锁内写回。

use super::favicon::{discover_favicon, FAVICON_TRIED};
use super::http::{conditional_get, Fetched};
use super::parse::{parse_feed, ParsedFeed};
use crate::db;
use crate::error::{AppError, AppResult};
use reqwest::Client;
use rusqlite::Connection;
use std::sync::Arc;

/* ============================================================
三段式刷新管线（并发安全版）
============================================================ */

/// 单源刷新的第一阶段：锁内读 feed 行（URL + 条件 GET 头）。
/// 读到的快照交给锁外的 `fetch_and_parse`，HTTP 期间不占数据库锁——
/// 这是并发抓取能真正并行（而非被 Mutex 串行化）的前提。
pub fn read_feed_for_refresh(
    conn: &Connection,
    feed_id: i64,
) -> AppResult<(String, Option<String>, Option<String>)> {
    conn.query_row(
        "SELECT feed_url, etag, last_modified FROM feeds WHERE id = ?1",
        rusqlite::params![feed_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .map_err(|_| AppError::not_found(format!("feed {feed_id} not found")))
}

/// 第二阶段：锁外条件 GET + 解析。HTTP 失败与解析失败统一为 Err，
/// 调用方据此走失败写回；304 与 200-带-body 的区分在第三阶段处理。
pub async fn fetch_and_parse(
    client: &Client,
    feed_url: &str,
    etag: Option<&str>,
    last_modified: Option<&str>,
) -> AppResult<(Fetched, ParsedFeed)> {
    let fetched = conditional_get(client, feed_url, etag, last_modified).await?;
    match &fetched {
        Fetched::NotModified => Ok((
            fetched,
            ParsedFeed {
                title: None,
                site_url: None,
                icon: None,
                articles: Vec::new(),
            },
        )),
        Fetched::Body { bytes, .. } => {
            let parsed = parse_feed(bytes, feed_url)?;
            Ok((fetched, parsed))
        }
    }
}

/// 第三阶段：锁内写回。成功清失败标记 + 更新元数据 + upsert 条目；
/// 失败标记 fetch_failed（指数退避由 db 层维护）。
/// 返回本次新增条目数。old_etag/old_last_modified：DB 里读到的旧条件头，
/// 304 分支写回时复用（304 响应不带新头，写 None 会断掉条件 GET 链）。
pub fn apply_refresh_result(
    conn: &Connection,
    feed_id: i64,
    fetched: &Fetched,
    parsed: &ParsedFeed,
    dedup: bool,
    old_etag: Option<&str>,
    old_last_modified: Option<&str>,
) -> AppResult<usize> {
    match fetched {
        Fetched::NotModified => {
            db::set_feed_fetch_state(conn, feed_id, false, None, old_etag, old_last_modified)?;
            Ok(0)
        }
        Fetched::Body {
            etag,
            last_modified,
            ..
        } => {
            db::set_feed_title_and_icon(
                conn,
                feed_id,
                parsed.title.as_deref(),
                parsed.icon.as_deref(),
                parsed.site_url.as_deref(),
            )?;
            db::set_feed_fetch_state(
                conn,
                feed_id,
                false,
                None,
                etag.as_deref(),
                last_modified.as_deref(),
            )?;
            let mut new_count = 0;
            for a in &parsed.articles {
                let (_, was_new) = db::upsert_article_with_feed(conn, feed_id, a, dedup)?;
                if was_new {
                    new_count += 1;
                }
            }
            Ok(new_count)
        }
    }
}

/// 三段式整合入口：读快照 →（锁外 HTTP+解析）→ 写回。
/// 语义与旧 `refresh_feed` 一致，但 HTTP 期间不持数据库锁，
/// 多任务并发时网络等待真正重叠（旧版被外层 Mutex 串行化）。
pub async fn refresh_feed_staged(
    db: &Arc<tokio::sync::Mutex<Connection>>,
    client: &Client,
    feed_id: i64,
    dedup: bool,
) -> AppResult<usize> {
    let (feed_url, etag, last_modified) = {
        let conn = db.lock().await;
        read_feed_for_refresh(&conn, feed_id)?
    };

    match fetch_and_parse(client, &feed_url, etag.as_deref(), last_modified.as_deref()).await {
        Ok((fetched, parsed)) => {
            /* favicon 后台发现：feed 未带 icon 且 DB 无缓存且本进程未尝试过 →
            spawn 独立任务（不占刷新信号量、不拖慢刷新关键路径——favicon 是
            锦上添花）。负缓存防无 favicon 的站点每轮重付探测超时。 */
            if parsed.icon.is_none() {
                let (existing_icon, already_tried): (Option<String>, bool) = {
                    let conn = db.lock().await;
                    let icon = conn
                        .query_row(
                            "SELECT favicon_url FROM feeds WHERE id = ?1",
                            rusqlite::params![feed_id],
                            |r| r.get(0),
                        )
                        .ok()
                        .flatten();
                    (icon, FAVICON_TRIED.lock().unwrap().contains(&feed_id))
                };
                if existing_icon.is_none() && !already_tried {
                    FAVICON_TRIED.lock().unwrap().insert(feed_id);
                    let site = parsed.site_url.clone().or_else(|| Some(feed_url.clone()));
                    let db = db.clone();
                    let client = client.clone();
                    /* tokio::spawn（非 tauri::async_runtime）：本函数在测试里
                    无 Tauri 运行时也能跑；调度器/命令均在 tokio 上下文调用 */
                    tokio::spawn(async move {
                        let discovered = match site.as_deref() {
                            Some(s) => discover_favicon(&client, s).await,
                            None => None,
                        };
                        if let Some(icon) = discovered {
                            let conn = db.lock().await;
                            let _ = conn.execute(
                                "UPDATE feeds SET favicon_url = ?1 WHERE id = ?2 AND (favicon_url IS NULL OR favicon_url = '')",
                                rusqlite::params![icon, feed_id],
                            );
                        }
                        /* 失败留在 FAVICON_TRIED（本进程不再重试）；
                        前端下次 reload 拿到新 favicon（如有） */
                    });
                }
            }
            let conn = db.lock().await;
            apply_refresh_result(
                &conn,
                feed_id,
                &fetched,
                &parsed,
                dedup,
                etag.as_deref(),
                last_modified.as_deref(),
            )
        }
        Err(e) => {
            let conn = db.lock().await;
            let _ = db::set_feed_fetch_state(
                &conn,
                feed_id,
                true,
                Some(&e.message),
                etag.as_deref(),
                last_modified.as_deref(),
            );
            Err(e)
        }
    }
}
