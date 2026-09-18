# TASK-056 遗留：`SyncReport.errors` 在前端无人消费（第 1 轮审查 FINDING 2）

- 发现：TASK-056 第 1 轮独立审查
- 状态：**未修复，如实登记**（按 TASK-056 `non_goals` 的『若发现前端仍需改，停下报告』）
- 日期：2026-09-18

## 问题

TASK-056 让 pull 建订阅/建条目的失败写入 `report.errors`（后端已生效），
**但前端没有任何代码读取 `SyncReport.errors`**，因此：

- 对**非外键**的插入失败（例如磁盘满、SQLITE_BUSY、约束变化等），
  用户界面**仍会提示「已拉取订阅源」「后端同步完成」**；
- 即 spec 声称的可观测契约变化（「失败的同步不再对外表现为成功」）
  **只交付了后端一半**，端到端对用户仍不可见。

## 证据（本轮实测）

穷举 `src/`、`tools/`、`src-tauri/src/` 中所有 `.errors` 引用：

| 位置 | 用途 | 是否面向用户 |
| --- | --- | --- |
| `src-tauri/src/commands/sync.rs:193,201,202` | `sync_local_feeds` 把 errors 拼进返回**字符串** | 是（但仅该命令） |
| `src-tauri/src/scheduler.rs:238` | 只 `log` errors.len() | 否（仅日志） |
| `src-tauri/src/sync/*` | 各处 `push` 进 errors | 否（仅是写入） |
| `src/lib/api.ts:96-104` | **只声明类型** `SyncReport` | 否 |
| `src/components/settings/SyncTab.tsx:87-91` | `api.syncPhase('feeds')` 的返回值**被丢弃** | 否 |
| `src/store/slices/sync.ts:75-80` | 同上，`feedsReport` 仅用于判空 | 否 |

关键点：`sync_phase` 命令在 `errors` 非空时**仍返回 `Ok(report)`**
（`phases.rs` 的 `feeds_phase` / `states_phase` 均为 `Ok(report)`），
因此 `SyncTab.tsx` 的 `.catch()` 对该路径**永不触发**——
「既有 catch 会显示」这一说法**不成立**。

## 为何本任务未修

TASK-056 的 `non_goals` 明确：

> 改前端（后端已有 report.errors 通道，前端 SyncTab 既有 catch 会显示；**若发现前端仍需改，停下报告**）

审查证实「前端仍需改」，故按该条**停下报告**，不在本任务内扩大改动范围。

**同时如实记录作者失误**：我在报告 §7 中写下了「前端既有 catch 会显示」这一**未经核对**的断言，
审查者以穷举 grep 与调用链分析推翻。这与本会话此前几次失误同源——
**把「我以为的调用关系」当成「已核对的事实」写进结论**。

## 建议的修复方向（留给决策）

TASK-057 已经拥有 `SyncTab.tsx` 的改动范围，是承载该修复的自然位置。可选：

1. `SyncTab.tsx` / `store/slices/sync.ts` 在得到 report 后检查 `errors.length > 0`，
   以 toast 明示「已同步，其中 N 项失败」——与 `sync_local_feeds` 现有的字符串拼接口径一致；
2. 或在 `sync_phase` 命令层把非空 errors 转成一个结构化字段/警告，
   交由前端统一呈现（改动面更大，需另立决策）。

**注意**：无论选哪种，都属**前端行为变更**，需要 owner 授权与 UI 证据（若触及界面文案）。
本文件仅登记问题与方向，不代表已获授权。
