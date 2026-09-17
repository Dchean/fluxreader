# TASK-045 基线（2026-09-17）：sync.rs 拆分前

采集于 TASK-044（commands.rs 拆分）完成之后的工作区。

## 拆分前规模（可信口径）

行数用三法交叉验证：LF 字节数、Python `splitlines()`、.NET `ReadAllLines()`，三者一致。

| 文件 | 行数 |
| --- | --- |
| `src-tauri/src/sync.rs` | 1352 |
| `src-tauri/src/ingestion.rs` | 766 |
| `src-tauri/src/config_sync.rs` | 579 |

> **口径警告（TASK-044 教训）**：不要用 PowerShell `Get-Content … | Measure-Object -Line` 计行数，
> 它**会少算**（对 327 行的 `folders.rs` 报 281，对 1369 行的原 `commands.rs` 报 1157）。
> 这是 TASK-044 报告中一处真实缺陷的根因，已记录于该任务报告 §6.3。

已完成拆分的参照：
- `db/` 8 个领域子模块（TASK-023 试点）
- `commands/` 7 个领域子模块（TASK-044）：mod.rs 90、folders.rs 327、articles.rs 209、
  settings.rs 212、opml.rs 81、sync.rs 296、ai.rs 216

## Rust 测试基线

```
cargo test --manifest-path src-tauri/Cargo.toml
=> 120 passed / 0 failed / 23 ignored
```

**注意**：用管道读 `cargo test` 退出码可能被掩盖（曾见 `exit code: 1` 而实际全绿）。
门禁一律**重定向到文件后读退出码**，并以日志里的 `test result:` 行交叉核对。

## sync.rs 的既有章节边界（拆分依据）

| 起始行 | 章节 | 行数 |
| --- | --- | --- |
| 37 | 凭据 | 126 |
| 163 | ① Push：本地状态变更 → 后端（只推不拉） | 180 |
| 343 | ② Pull：远端 → 本地（订阅关系 + 状态 + 条目） | 883 |
| 1226 | 总入口 | 126 |

`SyncReport` 定义在文件头（37 行之前）。

## 与 TASK-044 的关键区别（本任务的真实风险）

`commands.rs` 的拆分能做到**零可见性改动**（跨章节引用的项恰好都是 `pub`，
其余共享项提升到 `mod.rs` 即可）。

`sync.rs` **做不到**：总入口章节（`feeds_phase` / `states_phase` / `sync_now` /
`sync_light` / `test_connection`）要用到 Push 与 Pull 两侧的**私有**项：

| 总入口用到的项 | 来自 | 原可见性 |
| --- | --- | --- |
| `PUSH_LOCK`、`age_stale_queue`、`exec_push`、`plan_push` | ① Push | private |
| `pull_entries`、`pull_feeds`、`push_feeds` | ② Pull | private |
| `build_client`、`subscriptions` | 凭据 | private |

**因此本任务必然包含最小必要的可见性放大（private → pub(super)）**，
这与 TASK-044 的「纯搬运」不同。任务卡 acceptance 明确要求**逐项列出**放大清单，
不得冒充零改动——这是本任务最容易被含糊过去的地方，也是审查重点。
