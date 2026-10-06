# UI 契约：TASK-116 同步四态展示

- 依据：AUDIT-20261005-core-consistency.md 优化点 + 探查事实（2026-10-06 dev）
- 四态口径（X3 的如实原则）：「本地已保存」= 状态变更已事务化落库（TASK-108）；
  「等待同步」= sync_queue 中待推行数；「部分失败」= attempts>0 的行（最近错误摘要）；
  「远端已确认」= 队列已 prune（**不可累计溯源**，以「上次推送/上次同步」时间口径呈现，不虚构总数）。

## X1.sidebar-waiting-count

侧栏同步 pill 优先级修正与扩展（Sidebar.tsx:88-100 现状四分支）：

| 优先级 | 条件 | 文案 |
| --- | --- | --- |
| 1 | syncStatus==='error' | 同步失败（不再被 syncing 覆盖：错误态在手动同步进行中仍显示，修复现有覆盖缺陷） |
| 2 | 同步中（手动/后台） | 同步中… |
| 3 | 等待同步 N>0 | 等待同步 N 条 |
| 4 | 其余 | 既有：后端已同步 / 本地模式 · 直连抓取 |

部分失败 N>0 时 pill 追加「· 部分失败」段（同一 pill 内，≤48 字）。点击行为不变（进设置同步页）。

## X2.settings-four-states

SyncTab 新增「同步状态」摘要卡（数据：syncQueueStats + syncStatus）：

- 等待同步 N 条；部分失败 N（最新错误 ≤1 行摘要）；上次推送确认/上次同步时间（如实口径）。
- 「本地已保存」说明句：状态变更已保存，连接后自动补推（≤48 字）。
- 文案纪律沿用 TASK-102 口径：复述性/花絮不写。

## X3.no-fake-states

- 无队列且无失败：维持「后端已同步」既有语义，不新增噪音。
- 不显示「已确认累计 N 条」（prune 不可溯源）。


## 实施记录（TASK-116，2026-10-06，主控落账）

1. **X1 补记**：`failed>0` 的「· 部分失败」后缀对**全部四个优先级分支**生效（含「同步失败 · 部分失败」两事实并存如实呈现）；优先级修正为 error > syncing > waiting > connected（修复修前「失败被 syncing 覆盖」缺陷）。
2. **X2 补记**：「本地已保存」说明句落在摘要卡 desc 收尾而非独立 hint 块（受 TASK-101/102「SyncTab 恰一块 mini-dialog-hint」既有断言约束）；文案真值表收口单点为 `src/lib/syncPill.ts` 的 `syncPillLabel/syncStateSummary` 两个纯函数。
3. **X3 补记**：`last_error` 口径按「id 最大」近似「最近」（入队互斥合并删旧插新 ⇒ id 单调递增），无独立 last_failed_at 时间戳，取舍已入 SQL 注释。
4. **取证边界**：SSR（zustand v5 getInitialState）只作烟测；浏览器演示模式 SyncTab 短路，摘要卡与等待/失败分支由纯函数真值表+源级断言+CI Rust 测试（t116-r0..r3）锁定；pill 刷新时机=挂载+手动同步完成后 reload（即时推送与后台同步期间为契约约定内时滞）。
