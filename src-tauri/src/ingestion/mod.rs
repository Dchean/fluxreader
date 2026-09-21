//! 直连抓取管线（实施方案 §4.2 第一优先级）：
//! 条件 GET（ETag/If-Modified-Since）→ feed-rs 解析 → HTML 消毒 → upsert（source='direct'）。
//!
//! 失败时标记 feeds.fetch_failed=1 并按指数退避推迟重试（见 db/feeds.rs 的
//! set_feed_fetch_state 与 feeds_due_for_refresh）。
//! TASK-070：此处原写「供 Miniflux 兜底路径查询」——该兜底并不存在，且按该列
//! 过滤的查询（feeds_fetch_failed / feeds_fetch_failed_bound）已作为死代码删除。
//!
//! TASK-079（REQ-105）：本模块由原 680 行的单体按领域拆分为子模块（纯搬运，行为零变化）。
//! 子模块经 `pub use` 重导出，`crate::ingestion::<item>` 的公开路径逐字不变，
//! 故 47 处既有调用点无需任何改动。

mod favicon;
mod http;
mod parse;
mod staged;

pub use http::{build_client, conditional_get, Fetched, USER_AGENT};
pub use parse::{parse_feed, ParsedFeed};
pub use staged::{
    apply_refresh_result, fetch_and_parse, read_feed_for_refresh, refresh_feed_staged,
};
