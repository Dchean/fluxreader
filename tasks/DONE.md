# 已完成视图

状态权威为 [items](items/)，当前视图随 JSON 同步维护；尚无自动生成器。

**以下任务 status=verified，但均未合并、未发布。** verified 只表示管理 agent 已独立执行验证计划并通过；合并与发布仍需用户分别确认（DEC-002 / DEC-005）。

| 任务 | 完成内容 | 关键证据 |
| --- | --- | --- |
| [TASK-001](items/TASK-001.json) | 治理启动包与首轮考古 | 见 [M0 报告](runs/M0-20260912.json) |
| [TASK-004](items/TASK-004.json) | 清理 `tools/frontend-regression.mjs` 三个未使用绑定 | lint 警告 9→6；`test:frontend` 8/8；`build` 通过；`check(` 调用数不变 |
| [TASK-005](items/TASK-005.json) | 将前端逻辑回归接入 CI | 新增 3 行步骤；无新 action / 无权限变更 / 无 continue-on-error；托管 CI **NOT_RUN** |
| [TASK-006](items/TASK-006.json) | 统一 38 个 Rust 文件的格式 | `cargo fmt --check` 由 FAIL 转 **0**；格式化复现 39/39 逐字节一致（R-05 更正后的运作证据）；测试计数与基线一致 |
| [TASK-007](items/TASK-007.json) | ci.yml 补 Note 反向追溯注释 | 哈希证明仅 1 行注释变更；lint 6 警告不变；`test:frontend` 8/8 |
| [TASK-008](items/TASK-008.json) | 修复契约文件 $schema 键 | CLI 以契约原文通过 `--json-schema`（真实探测）；约束逐项不变；运行期去键副本作废 |
| [TASK-009](items/TASK-009.json) | 脱敏 .cache 含令牌文件 | 4 文件原位脱敏；复扫 0 残留；前后哈希对应入册 |

## 仍需跟踪的已知问题

- **托管 CI 已对 BATCH-001 候选通过**（2026-09-13，PR #2 双检查 success，[证据](runs/A2-HOSTED-CI-20260913.json)）；ISSUE-010 的"从未运行"状态解除。CI 不覆盖真实桌面 UI / 真实服务，也不构成合并批准。
- **`docs/contracts/worker-result.schema.json` 的 `$schema` 键已修复**（TASK-008，CLI 实测接受）；运行期副本绕过作废。
- **lint 仍有 6 条警告**：技能副本 3 条（不在任何任务范围）+ Reader/Overlays hooks 3 条（未立任务）。
- **真实桌面 UI E2E 与真实服务兼容性仍未验证**（ISSUE-003 / ISSUE-007）。
- **OPT-004 的非敏感配置同步边界尚未实现**（ISSUE-012 范围已定，实现未做）。
