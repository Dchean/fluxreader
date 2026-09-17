# TASK-053 基线（2026-09-17）：离线「全部已读」入队语义修复前

采集于 TASK-052 验收之后（提交 `71406d2`），工作区干净。

## 授权（本任务的关键前提）

owner 2026-09-17 就 TASK-051 报告列出的未修项逐项裁决：**P1-5 选择『另立任务修（需授权改协议行为）』**
（`DEC-c22f890a49524130a32d3d8254d742cb`，scope `["TASK-052","TASK-053","REQ-007"]`）。

因此本任务是本项目中**少数获授权改变同步协议行为**的任务之一。
该授权**仅限本缺陷所必需**：改「是否入队」，**不动**推送顺序、冲突解决策略与对账口径。

## 问题（P1-5 残留）

`mark_all_read`（及可能的同源路径）**仍以 `sync_configured` 作为入队前置条件**。

后果：**离线期间的「全部已读」不写入待推队列** → 连接后首次全量对账按远端状态把本地已读
**翻回未读**。用户看到的现象是「刚标的已读自己变回去了」。

## 已有的同类先例（本任务应与之对齐）

TASK-032 已修复 A-5：`set_read` / `set_starred` 改为

> **无论是否 `configured` 都入队，推送段在未配置时静默跳过**

并把 offline 复现测试**转正为必过**。本任务应把 `mark_all_read` 等剩余路径对齐到**同一语义**，
避免出现「有的状态变更离线会入队、有的不会」的不一致。

## 相关代码位置（自己核对，不要只信这里）

- `src-tauri/src/sync/`（TASK-045 拆分后的领域子模块）：`push.rs`、`entries.rs`、`phases.rs`、`mod.rs`
- `src-tauri/src/config_sync.rs`、`src-tauri/src/ingestion.rs`：其它可能触及同步的路径
- 搜索关键词：`sync_configured`、`enqueue`、`sync_queue`、`mark_all_read`
- 既有同步测试：`src-tauri/tests/`（含 A-5/C-1 的 offline 复现测试）

## 基线结果（本会话实跑）

```
npm run lint          => 0 warnings and 0 errors
npm run build         => ✓ built（tsc -b + vite）
npm run test:frontend => 241/241（既有 26 + 新增 215），退出码 0
cargo test            => 120 passed / 0 failed / 23 ignored
```

## 必须保持不变的既有语义（TASK-032/035/036/037/038 已建立）

TASK-032~038 建立起一整套同步语义，本任务**只许动「是否入队」这一个条件**：

| 任务 | 已建立的语义 |
| --- | --- |
| TASK-032 | C-1：集合拉取失败即跳过该轮对账（不得把错误当空集合）；A-5：状态变更一律入队 |
| TASK-035 | 删除订阅写墓碑 + best-effort 远端退订 |
| TASK-036 | 改名/移动目录推送 `edit_subscription` |
| TASK-037 | push 挂分类；分类改名/删除写墓碑防复活 |
| TASK-038 | 队列老化清理（30 天）；`take_sync_queue` 吞错改为记 `report.errors` |

这些是既有 Rust 测试的保护面，**任何一条被改坏都应在 `cargo test` 上暴露**。

## 口径与行尾纪律

- 行数用可信口径（LF / `splitlines()` / `ReadAllLines()`）；**不用** PowerShell `Measure-Object -Line`。
- 文本必须 **LF**；用 Python / PowerShell 写文件须显式保证行尾。
- **台账改动必须在 `begin` 之前完成**（`items/*.json` 属 `protected_paths`，begin 之后再改会被判越界，
  而 `scope` 失败**没有任何官方恢复路径**）。
- `npm` 门禁的 `args` 必须写成 `["run", "<script>"]`——我在 TASK-049 与 TASK-051 **两次**漏写 `run`
  导致门禁执行 `npm <script>`；`cargo` 门禁形如 `["test"]`（cargo 的子命令不带 `run`）。立卡后请复查。
