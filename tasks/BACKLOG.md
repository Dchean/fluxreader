# 待办视图

BATCH-001 已跑满 3 个任务上限并全部 verified，**等待用户审阅**。本批不自动追加任务。

## 待用户批准的事项

### A-1 合并 BATCH-001 的变更

三项已完成、已验证、**未合并**：

| 任务 | 变更文件 |
| --- | --- |
| [TASK-004](items/TASK-004.json) | `tools/frontend-regression.mjs` |
| [TASK-005](items/TASK-005.json) | `.github/workflows/ci.yml` |
| [TASK-006](items/TASK-006.json) | `src-tauri/**` 38 个 `.rs` 文件 |

合并前建议先决定是否一并处理下列存量未提交文件：`README.md`（非本批产生）、`AGENTS.md`、`CLAUDE.md`、`docs/**`、`tasks/**`、`.agents/**`。

### A-2 托管 CI 证据

TASK-005 的 CI 变更只在本机验证过。托管 CI 从未运行（NOT_RUN），环境存在差异（本地 Windows/Node 24 vs CI Ubuntu/Node 22，ISSUE-010）。合并前需在具备仓库授权的环境取得真实运行证据。

### A-3 仓库契约文件的兼容性缺陷

`docs/contracts/worker-result.schema.json` 的 `$schema` 键会被本机 Claude Code 2.1.270 拒绝。本批以运行期副本绕过，受保护文件未修改。建议单独立任务修复。

## 后续候选任务（均未授权，需用户追加预算）

| 候选 | 依据 | 风险 |
| --- | --- | --- |
| 修复 `worker-result.schema.json` 的 `$schema` 键 | A-3 | 低，但触及受保护契约 |
| 逐条处理 lint 剩余 6 条警告 | ISSUE-014 | 中（Reader/Overlays hooks 警告需逐个判断，不能机械改依赖） |
| 在 CI 中新增 `cargo fmt --check` 门禁 | TASK-006 已完成格式化 | 低—中（属增加检查而非降级） |
| OPT-004 非敏感配置同步的字段清单与实现 | DEC-009 / ISSUE-012 | 中—高（涉及凭据边界，需先写 proposed Note） |
| 真实桌面 UI E2E 与真实服务兼容性验证 | ISSUE-003 / ISSUE-007 | 高（需要驱动能力与真实服务账号决定） |

## 预算状态

`max_tasks_per_batch=3` 已用尽。已消耗修复轮 0/3（三次均为首次通过，无修复）。**追加任务需用户明确授权**，管理 agent 不自行开新批次或改批次 ID 继续跑。
