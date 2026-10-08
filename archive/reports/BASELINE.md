# 测试基线

## 本次检查范围

- 快照：git HEAD `f9122d5`（fix(greader): accept both JSON and classic-text ClientLogin responses, TASK-028；2026-09-16 历史重写后的等价提交，原记录 8966ece）+ 未提交修改仅涉及工作流文件（旧系统删除、新 .workflow-kit/、README/AGENTS/CLAUDE 入口调整），**业务源码与 HEAD 一致**。
- 环境：Windows 10.0.26100 x64；Python 3.13.14；Node（CI 用 22，本机以 npm 实际运行为准）；Rust stable（本机 cargo）。
- 已观察事实：Rust 冷编译后全部测试目标可运行；ignored 测试分为"本地 mock 服务"与"真实服务器"两类，本基线只跑默认集与 CI 选定的 mock 子集。

## 原始结果（2026-09-15，UTC 04:31 前）

| 检查 | 命令 | 状态 | 记录 |
| --- | --- | --- | --- |
| 前端 lint | `npm run lint`（oxlint） | PASS：0 警告 0 错误，23 文件 | 本机 |
| 前端逻辑回归 | `npm run test:frontend` | PASS：8/8 | 本机 |
| Rust 默认测试集 | `cargo test --manifest-path src-tauri/Cargo.toml` | PASS：108 通过 0 失败，21 个测试目标 | 本机，日志 /tmp/cargo-test-baseline.log（会话临时） |
| Rust 格式门禁 | `cargo fmt --all -- --check` | PASS | 本机 |
| Mock e2e（CI 选定子集，本地 mock 服务） | `cargo test … --test account_lifecycle_e2e --test ai_e2e --test dual_client_e2e --test sync_content_e2e --test sync_e2e --test sync_phases_e2e -- --ignored` | PASS：14 通过 0 失败 | 本机 |

## 已知缺口（NOT_RUN / SKIPPED）

- `npm run build`（tsc -b + vite build）：本基线未运行（CI push/PR 时覆盖）。
- `cargo clippy --all-targets -- -D warnings`：本基线未运行（CI 覆盖；首次冷编译后如需可补跑）。
- 真实服务器 e2e（fever_live_e2e、greader_live_e2e 等）：未运行——旧约束"不使用真实账号跑同步测试"已被用户 2026-09-15 决定取代（允许连接用户已有同步后端验证），但需在具体任务内单独约定读写边界后执行，仍不作为统一门禁。
- CI 中其余未选定的 --ignored mock 测试：未运行。
- 桌面 UI 人工验证：未运行（`npm run tauri dev` 启动观察留待 UI 相关任务）。

## 恢复入口

- 源码基准：git HEAD f9122d5（原 8966ece，见文末提交号对照）；工作区未提交改动仅为工作流文件，业务代码可随时 `git stash`/对照恢复。
- snapshot 仅提供哈希核对，不作为源码备份。

## 与旧基线的关系

旧 docs/BASELINE.md（git 历史）记录"前端 8/8、Rust 95 项通过；格式检查失败"。本次实测 Rust 108 项通过且 fmt 通过——旧记录基于更早源码基准，仅作历史参照，不作为本基线。

## 提交号对照（2026-09-16 全量历史重写）

2026-09-16 对仓库历史做了一次全量重写，凡是树内容被改动的提交都换了新提交号；重写工具未留下映射文件，因此**旧提交号在本仓库已无法解析**。下表按提交信息逐条核对得出，供旧记录中的引用回溯：

| 旧提交号（已失效） | 当前等价提交 | 提交信息 |
| --- | --- | --- |
| 8966ece | f9122d5 | fix(greader): accept both JSON and classic-text ClientLogin responses (TASK-028) |
| 26a6a51 | 4156b57 | chore(workflow): attach workflow-kit and prepare refactor pilot TASK-023 |
| fdcf9a2 | 63de5a0 | refactor(db): split db.rs into domain submodules, public paths unchanged (TASK-023) |
| 259f00d | 925a0b1 | chore(workflow): accept TASK-023 - db.rs modularization pilot complete |
| 40d005d | d2473b8 | merge: workflow-kit governed defect fixes and refactor batch (TASK-029..040) |
| 919a73e | 61563e6 | chore(workflow): accept TASK-035..040; record re-verification, reviews and tool fix |
| 18faeb7 | 748e42b | fix(frontend+db): per-id summary state, anchor-mark-read, view-scoped mark-all-read, search race guard (F4/F7/F8/F20) |
| ebb4653（仅按提交信息匹配，未逐字验证） | 1105e43 | fix(sync): age stale unbound queue items and stop swallowing queue read errors (A-8, C-2) |

下列历史材料仍保留写入时的旧提交号，**不做追溯改写**：`.workflow-kit/tasks/evidence/` 下已冻结的证据文件（其内容与审查摘要绑定，改动会触发"审查后证据已变更"门禁）、`tasks/BRIEF.json` 与 `tasks/DECISIONS.json` 中已由 owner 确认的简报内容、以及各任务卡片与其实测记录。需要这些引用时请用上表换算。
