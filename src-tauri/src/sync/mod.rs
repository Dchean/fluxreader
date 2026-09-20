//! 同步引擎（Google Reader 兼容协议，后端 Miniflux）：
//!
//! ① Push：sync_queue 里的本地变更推到后端
//! ② Pull：拉远端订阅/分类/条目状态变化，URL 碰撞合并
//! ③ 条目补齐：origin='remote' 的源由 reading-list 全量拉取（GReader）或
//!    未读+收藏集合补齐（Fever）隐式覆盖，因此本地无需为其直连抓取。
//!    TASK-070：本条原先宣称「直连失败的源从 Miniflux 拉条目」——该路径在当前
//!    实现中已不存在。它**曾经存在**：Miniflux 协议时期 pull 里有 failed_feeds
//!    循环，直接调 feeds_fetch_failed / feeds_fetch_failed_bound；0ba940f
//!    （协议切换 Google Reader）删掉了该循环，此后 feeds.fetch_failed 在 Rust 侧
//!    没有读取方，这两条查询也随本任务作为死代码删除。该列的现存用途是经
//!    FeedRow（commands/folders.rs 的 list_feeds）下发前端做失败标记
//!    （api.ts 的 fetchFailed → Sidebar 的 feed-error-dot）；抓取退避由
//!    fail_count / next_retry_at 承担（db/feeds.rs 的 set_feed_fetch_state 写入、
//!    feeds_due_for_refresh 按 next_retry_at 过滤），与该列无关。
//! 本地未连接期间添加的源，首次 Pull 时按 URL 碰撞检测：
//!   远端无 → 推送创建；远端有 → 合并（remote id 绑定本地 feed）
//!
//! 锁纪律：与 refresh_feed_staged 相同的三段式——锁内读写 SQLite，
//! HTTP 全部在锁外执行，同步进行时其他 DB 命令不被冻结。
//!
//! 阶段划分（前端分步同步 + 后台自动同步复用）：
//!   feeds 阶段  = push_feeds + pull_feeds（订阅层，秒级）
//!   states 阶段 = push_queue + pull_entries（状态+条目层，慢）
//! sync_now = 两个阶段串联（全量路径，含绑定回填+全量状态对账）。

//! TASK-045：按既有章节边界拆为领域子模块；子模块经 `pub use` 重导出，
//! 使 `crate::sync::<fn>` 路径对 lib.rs / commands / scheduler 等调用点保持不变。

use serde::Serialize;

#[derive(Debug, Default, Serialize)]
pub struct SyncReport {
    pub pushed_states: usize,
    pub pushed_feeds: usize,
    pub pulled_feeds: usize,
    pub pulled_entries: usize,
    pub merged_states: usize,
    pub errors: Vec<String>,
}

/* ============================================================
子模块声明与重导出

`crate::sync::<fn>` 的对外路径必须逐字不变（lib.rs / commands / scheduler 等
调用点零改动），故对**含 pub 项**的子模块做 `pub use` 重导出。

entries / greader_pull / fever_pull 三个子模块**不含任何 pub 项**——
它们只提供 sync 内部使用的实现细节，全部为 `pub(super)`；
对它们做 `pub use` 会被 rustc 判为「glob 未重导出任何 pub 项」并告警，
故改用模块内私有 glob，使其项仍可经 `use super::*` 被兄弟模块使用。
============================================================ */
mod credentials;
mod entries;
mod fever_pull;
mod greader_pull;
mod phases;
mod push;
mod subscriptions;

pub use credentials::*;
pub use phases::*;
pub use push::*;
pub use subscriptions::*;

/* 无 pub 项的内部实现模块：私有重导出，仅供 sync 内部（含兄弟子模块）使用 */
use entries::*;
use fever_pull::*;
use greader_pull::*;
