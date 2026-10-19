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
    /// P3-11（TASK-074）：本轮因远端已退订而从本地删除的源数。
    pub removed_feeds: usize,
    pub errors: Vec<String>,
    /// R3（F11）：本轮是否产生**实际条目数据变化**——新插入、既有行可见字段
    /// （正文/元数据/封面/附件）真实改变、远端绑定真实变化、读藏值实际改变、
    /// 对账值条件写命中。只信「前后数据比对 / 值条件写命中行数 / 真实绑定变化」，
    /// 绝不拿处理次数、守卫通过、同值 UPDATE 的 changes() 冒充变化。
    /// `pulled_entries` 是旧口径「处理过/已接受」计数（含守卫通过但值未变的
    /// 条目），不参与事件判定；`merged_states` 在判定中兼容保留（states 阶段
    /// 来源是对账值条件写命中行数=真实变化；订阅阶段该计数的性质与可达性见
    /// `has_data_change` 注释）。
    ///
    /// `serde(skip)`：这是内部判定信号，不是 IPC 面——对外 JSON 形状零变化，
    /// 前端无需感知（选型说明：bool 足够表达「有无实际变化」；未用富结构是因为
    /// 消费点只有事件判定，多出的字段会成为无人消费的 API 面）。
    #[serde(skip)]
    pub entries_data_changed: bool,
    /// 本轮条目页遇到尚无本地 feed 绑定的远端 feed。内部信号：states 阶段可
    /// 在只读订阅发现后重试一次；不属于对外 report schema。
    #[serde(skip)]
    pub binding_incomplete: bool,
}

impl SyncReport {
    /// 本轮是否产生了**用户可见的数据变化**（事件层单点判定，OPT-007 / F11；
    /// R3 立口径，R4 按主控 rg 核对修正 `merged_states` 的归属）。
    ///
    /// 判定只数「实际变更」：
    /// - `entries_data_changed`：条目层实际变化（新插入 / 正文·绑定 / 真实读藏
    ///   变化 / 纯读藏对账），由各生产合并点用前后比对、值条件写命中行数、
    ///   真实绑定变化**即时**接线（R4：每个成功变更点立即合并——GR 逐语句提交
    ///   无页事务，后续步骤失败不得让已提交的变化失去信号）；
    /// - `merged_states`：来源为 states 阶段对账值条件写命中行数
    ///   （`greader_pull.rs` / `fever_pull.rs` 的 reconcile——真实变化）。
    ///   订阅发现只产生真实的 `pulled_feeds`/`removed_feeds`；既有源匹配不再
    ///   伪造计数，绑定写错误进入 report 而不是被吞掉；
    /// - `pulled_feeds` / `removed_feeds`：订阅层增删。
    ///
    /// 明确**不**包含：`pulled_entries`（旧口径计数——守卫通过但值未变的条目
    /// 也计数，不能冒充语义变化）、`pushed_states` / `pushed_feeds`（本地变更
    /// 推送成功——UI 早已按本地意图显示）、`errors`（失败不是数据变化，由错误
    /// 呈现通道承担）。全零 = 空变化，**不 emit**（不制造无谓 reload）；
    /// `false` 返回是这一契约的单点实现，供 scheduler 与测试共用。
    pub fn has_data_change(&self) -> bool {
        self.entries_data_changed
            || self.merged_states > 0
            || self.pulled_feeds > 0
            || self.removed_feeds > 0
    }
}

#[cfg(test)]
mod report_change_tests {
    use super::*;

    /// OPT-007（F11）/ R3+R4+R5 判别力：
    /// - 条目层实际变化（标记）与订阅层增删必须算数据变化；
    /// - `merged_states` 判定兼容保留：来源是 states 阶段对账值条件写命中行数
    ///   （真实变化）；订阅发现不伪造该计数——见 `has_data_change` 注释；
    /// - `pulled_entries` 是「处理过/已接受」计数（守卫通过但值未变也计数）——
    ///   单独为真**不算**（R3 反例：内容真实改变但计数为 0 时不得漏报）；
    /// - 仅推送成功 / 仅错误不算数据变化；空 report 必须 false。
    #[test]
    fn has_data_change_counts_only_actual_changes() {
        assert!(!SyncReport::default().has_data_change(), "空变化不得发事件");

        // 条目层实际变化（新插入/正文·绑定/真实读藏变化/对账）
        let entries_changed = SyncReport {
            entries_data_changed: true,
            ..Default::default()
        };
        assert!(
            entries_changed.has_data_change(),
            "实际条目变化必须发数据变化"
        );

        // 订阅层增删
        assert!(SyncReport {
            pulled_feeds: 1,
            ..Default::default()
        }
        .has_data_change());
        assert!(SyncReport {
            removed_feeds: 1,
            ..Default::default()
        }
        .has_data_change());

        // merged_states：唯一生产消费路径上为 states 阶段对账值条件写命中行数
        // （真实变化）；订阅发现只计真实增删，既有源匹配不再伪造计数——
        // 兼容判定为变化（R5 按主控复核限定口径，见 has_data_change 注释）。
        let merged_only = SyncReport {
            merged_states: 3,
            ..Default::default()
        };
        assert!(
            merged_only.has_data_change(),
            "merged_states 在消费路径（states）上为真实变化计数，兼容保留在判定中"
        );

        // pulled_entries：「处理过/已接受」计数——守卫通过但值未变也计数，
        // 单独为真不得冒充语义变化
        let pulled_only = SyncReport {
            pulled_entries: 7,
            ..Default::default()
        };
        assert!(
            !pulled_only.has_data_change(),
            "pulled_entries 是处理计数，不得冒充语义变化"
        );

        // 仅推送成功 / 仅错误：不算数据变化（不 emit）
        let pushed_only = SyncReport {
            pushed_states: 3,
            pushed_feeds: 2,
            ..Default::default()
        };
        assert!(
            !pushed_only.has_data_change(),
            "本地意图推送成功不改变可见数据（UI 已是本地态），不得触发 reload"
        );
        let errors_only = SyncReport {
            errors: vec!["拉取条目正文失败: timeout".into()],
            ..Default::default()
        };
        assert!(!errors_only.has_data_change(), "错误不是数据变化");
    }
}

/* ============================================================
子模块声明与重导出

`crate::sync::<fn>` 的对外路径必须逐字不变（lib.rs / commands / scheduler 等
调用点零改动），故对**含 pub 项**的子模块做 `pub use` 重导出。

entries / greader_pull / fever_pull 三个子模块**不含任何 pub 项**——
它们只提供 sync 内部使用的实现细节，全部为 `pub(super)`；
对它们做 `pub use` 会被 rustc 判为「glob 未重导出任何 pub 项」并告警，
故改用模块内私有 glob，使其项仍可经 `use super::*` 被兄弟模块使用。

唯一例外（TASK-097）：`greader_pull::set_greader_pull_clock_override` 是
`#[doc(hidden)] pub` 的可测性小口子（集成测试注入游标墙钟，构造确定性
时间线），需经下方显式 `pub use` 暴露给 tests/；不影响任何对外行为。
============================================================ */
mod conflict_policy;
mod credentials;
mod entries;
mod fever_pull;
mod greader_pull;
mod phases;
mod push;
mod session;
mod subscriptions;

pub use credentials::*;
pub use phases::*;
pub use push::*;
pub use subscriptions::*;

/// TASK-097：唯一从「无 pub 项」子模块显式重导出的项（见上方例外说明）。
pub use greader_pull::set_greader_pull_clock_override;

/* 无 pub 项的内部实现模块：私有重导出，仅供 sync 内部（含兄弟子模块）使用。
TASK-113：conflict_policy 例外——不参与裸名 glob 重导出。其消费点
（greader_pull/fever_pull）一律走 `conflict_policy::` 限定路径，glob 引入的
裸名无任何消费者，rustc 1.98 起按 unused_imports 告警（clippy -D warnings
下致命）；兄弟子模块经 `use super::*` 仍能取得模块名本身，限定路径可解析。 */
use entries::*;
use fever_pull::*;
use greader_pull::*;
use session::*;
