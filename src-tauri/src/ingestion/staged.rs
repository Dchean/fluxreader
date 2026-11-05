//! 三段式刷新管线（并发安全版）：锁内读快照 → 锁外 HTTP+解析 → 锁内写回。

use super::favicon::{discover_favicon, FAVICON_TRIED};
use super::http::{conditional_get, Fetched};
use super::parse::{parse_feed, ParsedFeed};
use crate::db;
use crate::error::AppResult;
use reqwest::Client;
use rusqlite::Connection;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::{LazyLock, Mutex as StdMutex};

/* ============================================================
三段式刷新管线（并发安全版）
============================================================ */

/// 进程内同源在途集合。键必须同时包含数据库身份与规范化 feed URL：集成测试会在
/// 同一进程里打开多个临时库，若只按 URL 去重会跨库误跳过；生产只有一个库，db 的
/// Arc 指针在所有入口克隆间保持不变。不能用 rowid——SQLite 删除最大 id 后会复用
/// 该 id，新源不能被旧源的未完成请求误跳过。同 URL 第二次请求返回
/// `SkippedInFlight`（new_articles=0），由第一轮完成写库与事件；这比排队后重复抓取
/// 更符合「refresh 是不可叠加动作」。
// Note: 同源在途去重与 generation CAS — 见 .agents/notes/implemented/architecture/2026-11-04-刷新编排与源名分域.md
static IN_FLIGHT_FEEDS: LazyLock<StdMutex<HashSet<(usize, String)>>> =
    LazyLock::new(|| StdMutex::new(HashSet::new()));

struct InFlightFeed {
    key: (usize, String),
}

impl Drop for InFlightFeed {
    fn drop(&mut self) {
        IN_FLIGHT_FEEDS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.key);
    }
}

fn claim_feed_refresh(
    db: &Arc<tokio::sync::Mutex<Connection>>,
    feed_url: &str,
) -> Option<InFlightFeed> {
    let key = (Arc::as_ptr(db) as usize, crate::db::normalize_url(feed_url));
    let mut in_flight = IN_FLIGHT_FEEDS.lock().unwrap_or_else(|e| e.into_inner());
    if !in_flight.insert(key.clone()) {
        return None;
    }
    Some(InFlightFeed { key })
}

/// 刷新写回结果：`Superseded` 表示本轮 generation 已被更新的一轮取代，
/// 所有状态/标题/文章写入都被丢弃。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshApplyOutcome {
    Applied(usize),
    Superseded,
}

/// 一次抓取动作的最终归属。调度器只能把 `Applied` 记入跨批次 footprint：
/// 在途跳过、代际被更新轮次取代、以及 HTTP/写回失败都不能算“刚抓成功”。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedRefreshOutcome {
    Applied {
        new_articles: usize,
        feed_url: String,
    },
    SkippedInFlight,
    Superseded,
}

impl FeedRefreshOutcome {
    pub fn new_articles(&self) -> usize {
        match self {
            Self::Applied { new_articles, .. } => *new_articles,
            Self::SkippedInFlight | Self::Superseded => 0,
        }
    }
}

/// 本轮写回需要的源身份、代际与旧条件头。304 分支没有新头，必须复用本轮开始时
/// 读到的旧值；把这些字段成组传递，避免 feed_url / generation / 条件头在调用点错位。
#[derive(Debug, Clone, Copy)]
pub struct RefreshWriteContext<'a> {
    pub feed_url: &'a str,
    pub generation: i64,
    pub old_etag: Option<&'a str>,
    pub old_last_modified: Option<&'a str>,
}

impl<'a> RefreshWriteContext<'a> {
    pub fn new(
        feed_url: &'a str,
        generation: i64,
        old_etag: Option<&'a str>,
        old_last_modified: Option<&'a str>,
    ) -> Self {
        Self {
            feed_url,
            generation,
            old_etag,
            old_last_modified,
        }
    }
}

/// rowid 会被 SQLite 复用，generation 在新行上也可能从同一数值重新开始。
/// 因此写回必须同时确认当前行仍是本轮抓取时的那个 URL。
fn feed_url_matches(conn: &Connection, feed_id: i64, expected: &str) -> AppResult<bool> {
    Ok(db::feed_url_by_id(conn, feed_id)?.as_deref() == Some(expected))
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
/// Body 分支的源元数据/条目/条件头/失败状态在**单一短事务**内提交，失败全量回滚。
/// 返回本次新增条目数。old_etag/old_last_modified：DB 里读到的旧条件头，
/// 304 分支写回时复用（304 响应不带新头，写 None 会断掉条件 GET 链）。
pub fn apply_refresh_result(
    conn: &Connection,
    feed_id: i64,
    write: &RefreshWriteContext<'_>,
    fetched: &Fetched,
    parsed: &ParsedFeed,
    dedup: bool,
) -> AppResult<RefreshApplyOutcome> {
    match fetched {
        Fetched::NotModified => {
            if !feed_url_matches(conn, feed_id, write.feed_url)? {
                return Ok(RefreshApplyOutcome::Superseded);
            }
            let current = db::set_feed_fetch_state_if_generation(
                conn,
                feed_id,
                write.generation,
                false,
                None,
                write.old_etag,
                write.old_last_modified,
            )?;
            Ok(if current {
                RefreshApplyOutcome::Applied(0)
            } else {
                RefreshApplyOutcome::Superseded
            })
        }
        Fetched::Body {
            etag,
            last_modified,
            ..
        } => {
            // Note: Body 写回的源元数据/条目/条件头/失败状态必须同一短事务提交（HTTP 已锁外完成） — 见 .agents/notes/implemented/architecture/2026-09-19-后端模块拆分与并发纪律.md
            // OPT-003：三段式只保证 HTTP 在锁外；写回内部此前逐语句自动提交，
            // 中途任一失败（条目插入/触发器/磁盘错误）会留下「标题与条件头已换新、
            // 条目只进一半」的半提交状态。包进单一事务后，数据与状态同生共死；
            // 失败注入回归见 tests/staged_refresh_e2e.rs（BEFORE INSERT 触发器反例）。
            let tx = conn.unchecked_transaction()?;
            // CAS 必须先行：旧轮次连标题/文章都不允许进入写路径。条件更新命中后，
            // 同一事务持写锁到 commit，本轮 generation 不可能再被替换。
            if !feed_url_matches(&tx, feed_id, write.feed_url)? {
                return Ok(RefreshApplyOutcome::Superseded);
            }
            if !db::set_feed_fetch_state_if_generation(
                &tx,
                feed_id,
                write.generation,
                false,
                None,
                etag.as_deref(),
                last_modified.as_deref(),
            )? {
                return Ok(RefreshApplyOutcome::Superseded);
            }
            db::set_feed_title_and_icon(
                &tx,
                feed_id,
                parsed.title.as_deref(),
                parsed.icon.as_deref(),
                parsed.site_url.as_deref(),
            )?;
            let mut new_count = 0;
            for a in &parsed.articles {
                let (_, was_new) = db::upsert_article_with_feed_source(
                    &tx,
                    feed_id,
                    &a.article,
                    dedup,
                    a.published_at_clamped,
                )?;
                if was_new {
                    new_count += 1;
                }
            }
            tx.commit()?;
            Ok(RefreshApplyOutcome::Applied(new_count))
        }
    }
}

/// 三段式整合入口：读快照 →（锁外 HTTP+解析）→ 写回。
/// 语义与旧 `refresh_feed` 一致，但 HTTP 期间不持数据库锁，
/// 多任务并发时网络等待真正重叠（旧版被外层 Mutex 串行化）。
/// 返回值区分真正写回、在途跳过与代际取代；调度器只有 `Applied` 可记
/// 跨批次 footprint。
pub async fn refresh_feed_staged(
    db: &Arc<tokio::sync::Mutex<Connection>>,
    client: &Client,
    feed_id: i64,
    dedup: bool,
) -> AppResult<FeedRefreshOutcome> {
    let (attempt, _in_flight) = {
        let conn = db.lock().await;
        let feed_url = db::feed_url_by_id(&conn, feed_id)?.ok_or_else(|| {
            crate::error::AppError::not_found(format!("feed {feed_id} not found"))
        })?;
        let Some(in_flight) = claim_feed_refresh(db, &feed_url) else {
            log::debug!("ingestion: feed {feed_id} 已有刷新在途，跳过重复请求");
            return Ok(FeedRefreshOutcome::SkippedInFlight);
        };
        (db::begin_feed_refresh(&conn, feed_id)?, in_flight)
    };

    match fetch_and_parse(
        client,
        &attempt.feed_url,
        attempt.etag.as_deref(),
        attempt.last_modified.as_deref(),
    )
    .await
    {
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
                    // P3（2026-09-22 复查）：与 lib.rs 的既有处置一致——`lock().unwrap()`
                    // 在互斥量中毒时会 panic。锁内只是 HashSet<i64>（「试过 favicon 的源」），
                    // 中毒不影响其可用性，故用 into_inner 容忍中毒而非 panic。
                    (
                        icon,
                        FAVICON_TRIED
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .contains(&feed_id),
                    )
                };
                if existing_icon.is_none() && !already_tried {
                    FAVICON_TRIED
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(feed_id);
                    let site = parsed
                        .site_url
                        .clone()
                        .or_else(|| Some(attempt.feed_url.clone()));
                    let write_feed_url = attempt.feed_url.clone();
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
                            // P3[1]：favicon 落库失败此前静默。favicon 是锦上添花，
                            // 失败不影响抓取，但写不进去会让用户看到「图标一直不出现」
                            // 却无任何线索。改为 warn。
                            if let Err(err) = conn.execute(
                                "UPDATE feeds SET favicon_url = ?1
                                  WHERE id = ?2 AND feed_url = ?3
                                    AND (favicon_url IS NULL OR favicon_url = '')",
                                rusqlite::params![icon, feed_id, write_feed_url],
                            ) {
                                log::warn!("ingestion: favicon 落库失败（feed={feed_id}）: {err}");
                            }
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
                &RefreshWriteContext::new(
                    &attempt.feed_url,
                    attempt.generation,
                    attempt.etag.as_deref(),
                    attempt.last_modified.as_deref(),
                ),
                &fetched,
                &parsed,
                dedup,
            )
            .map(|outcome| match outcome {
                RefreshApplyOutcome::Applied(new_articles) => FeedRefreshOutcome::Applied {
                    new_articles,
                    feed_url: attempt.feed_url.clone(),
                },
                RefreshApplyOutcome::Superseded => {
                    log::debug!("ingestion: feed {feed_id} 刷新结果已被更新轮次取代，丢弃");
                    FeedRefreshOutcome::Superseded
                }
            })
        }
        Err(e) => {
            let conn = db.lock().await;
            if !feed_url_matches(&conn, feed_id, &attempt.feed_url)? {
                log::debug!("ingestion: feed {feed_id} 已被删除或替换，失败结果丢弃");
                return Ok(FeedRefreshOutcome::Superseded);
            }
            // P3[1]（REQ-104）：失败标记写回此前被 `let _ =` 静默吞掉，后果比一般
            // 吞错更重——fetch_failed/fail_count/next_retry_at 写不进去，
            // **指数退避就不会生效**，失败源会被每轮调度反复重抓（放大对站点的压力、
            // 且前端错误标志不亮）。此处必须显式记录，让现场可诊断。
            match db::set_feed_fetch_state_if_generation(
                &conn,
                feed_id,
                attempt.generation,
                true,
                Some(&e.message),
                attempt.etag.as_deref(),
                attempt.last_modified.as_deref(),
            ) {
                Ok(true) => Err(e),
                Ok(false) => {
                    log::debug!("ingestion: feed {feed_id} 的失败结果已被更新轮次取代，丢弃");
                    Ok(FeedRefreshOutcome::Superseded)
                }
                Err(err) => {
                    log::warn!(
                        "ingestion: 写回抓取失败标记失败（feed={feed_id}）——退避将不生效: {err}"
                    );
                    Err(e)
                }
            }
        }
    }
}
