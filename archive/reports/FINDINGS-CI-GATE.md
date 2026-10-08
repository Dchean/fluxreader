# 发现：CI 自 2026-09-16 08:24Z 起连续失败（fmt 掩盖了 clippy）

日期：2026-09-16。范围：CI 门禁 + src-tauri 源码与测试的格式/lint，无行为改动。

## 现象

自 2026-09-16T08:24:41Z 的推送起，`main` 连续 5 次推送的 CI 全部失败，且都停在同一步骤：

| 运行 | 提交 | 失败步骤 |
| --- | --- | --- |
| 35073619248 | feat(ui): TASK-041 文案统一精简与控件一致性收尾 | Format (cargo fmt --check) |
| 35077361239 | chore(workflow): accept TASK-041 | 同上 |
| 35081628894 | chore(workflow): accept TASK-041 | 同上 |
| 35081728779 | chore(workflow): 更新 TASK-041 验收记录的 merge-ref | 同上 |
| 35083831321 | chore(workflow): 修正 TASK-035..040 合并记录 | 同上 |

`Format` 是 rust job 的第 5 步，排在 `clippy`、`cargo test` 与 mock e2e 之前，因此后面的步骤一次都没执行过。

## 根因

两个互相独立的问题，第二个被第一个掩盖。

### 1) rustfmt 行宽超限（4 个文件）

TASK-041 期间的文本替换把若干测试常量换成了更长的占位符字符串，使原本恰好等于 `max_width = 100`（允许）的几行变为超限：

- `src-tauri/src/fever.rs`：`FeverClient::new(...)` 单行调用；
- `src-tauri/tests/fever_live_e2e.rs`、`fever_sync_live_e2e.rs`、`greader_live_e2e.rs`：`let password = std::env::var(...)...;` 单行。

### 2) clippy 两处失败（被 fmt 掩盖）

- `src-tauri/src/commands.rs:340`：`record_feed_edit` 返回 `AppResult<Option<(i64, Option<String>, Option<String>)>>`，触发 `clippy::type_complexity`。
- `src-tauri/tests/sync_gap_repro_e2e.rs:332`：`server.status_updates.lock().unwrap()` 的 `MutexGuard` 跨 `db.lock().await` 持有，触发 `clippy::await_holding_lock`。

两处都出自 TASK-029..040 的同步缺口修复批次。该批次的本地验证只覆盖 `cargo test` 与 mock e2e——`docs/BASELINE.md` 明确记载 `cargo clippy --all-targets -- -D warnings` 本地未运行、交由 CI 覆盖——而 CI 覆盖 clippy 的前提是 fmt 先通过。fmt 恰好在此前失败，于是 clippy 从未真正执行，缺陷一路穿过 TASK-041 的验收保留至今。

## 修复

- fmt：按 rustfmt 重排上述 4 个文件（纯换行，无语义变化）。
- clippy：`record_feed_edit` 的返回元组抽为 `pub type FeedEditPush = (i64, Option<String>, Option<String>)`；`sync_gap_repro_e2e` 把状态断言收进 `{ … }` 作用域，使 `MutexGuard` 在 `await` 之前释放（断言语义不变，调用方因类型别名而无需改动）。

## 验证

本地按 CI rust job 的顺序复跑（Windows，rustc 1.98.0 / rustfmt 1.9.0-stable）：

| 检查 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | 0（无 diff） |
| `cargo clippy --all-targets -- -D warnings` | 0（全目标零告警） |
| `cargo test` | 0（含改动的 `offline_read_change_pushed_after_connect ... ok`） |
| CI 选定的 6 个 mock e2e（`--ignored`） | 0 |

## 门禁缺口与遗留

- **本地门禁缺 clippy**：本批次"本地跑 test、CI 跑 clippy"的分工在 fmt 失败时整体失效——中间门禁挡住后置门禁，等于没有门禁。建议把 `cargo fmt --all -- --check` 与 `cargo clippy --all-targets -- -D warnings` 写进任务本地门禁清单，不再依赖 CI 兜底。
- **Release 门禁的 tag 场景**（未决）：`release.yml` 要求"tagged commit 上的 ci.yml 全绿"，但 `ci.yml` 只在 `push: branches: [main]` 与 PR 上触发。当 tag 指向非 main tip 的提交时，该 SHA 上永远不会出现 CI 运行，门禁会空等到 45 分钟超时后报错——本次强制重推 `v0.12.0` / `v0.13.0` 触发的运行 35081633560 即此情形（另一枚 tag 的提交未被重写、存在旧的成功 CI 运行，故同批次的 35081633281 正常通过）。可选修法：给 `ci.yml` 增加 `push: tags: ["v*"]` 触发，或让门禁在找不到运行时通过 `workflow_dispatch` 补发一次 CI。待 owner 决定后实施。