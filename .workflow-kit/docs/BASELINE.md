# 测试基线

## 本次检查范围

- 快照：git HEAD `8966ece`（fix(greader): accept both JSON and classic-text ClientLogin responses, TASK-028）+ 未提交修改仅涉及工作流文件（旧系统删除、新 .workflow-kit/、README/AGENTS/CLAUDE 入口调整），**业务源码与 HEAD 一致**。
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

- 源码基准：git HEAD 8966ece；工作区未提交改动仅为工作流文件，业务代码可随时 `git stash`/对照恢复。
- snapshot 仅提供哈希核对，不作为源码备份。

## 与旧基线的关系

旧 docs/BASELINE.md（git 历史）记录"前端 8/8、Rust 95 项通过；格式检查失败"。本次实测 Rust 108 项通过且 fmt 通过——旧记录基于更早源码基准，仅作历史参照，不作为本基线。
