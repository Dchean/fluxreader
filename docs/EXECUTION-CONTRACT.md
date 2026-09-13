# 跨 agent 执行契约 v1

本规范用于人工或自动总控向 Codex、Claude Code 等执行器交付任务。当前可由管理 agent 按 [HANDOFF](HANDOFF.md) 用终端工具调度 Claude CLI。尚未实现独立调度器、CLI 适配器程序、自动看板或权限拦截程序；不要把文档当成这些能力已经部署。

## 事实源

- 项目阶段和授权记录：tasks/PROJECT.json；日常委托权限与预算：tasks/EXECUTION-POLICY.json。
- 单个任务定义和状态：tasks/items/TASK-NNN.json。
- 产品依据：PRODUCT / FEATURES；实现现状：ARCHITECTURE / DATA-MODEL / API。
- 检查入口与结果：TEST-PLAN / BASELINE；问题：ISSUES。
- 看板：BACKLOG / IN_PROGRESS / DONE，仅是派生视图。
- 运行证据：tasks/runs/ 摘要及本地/CI 原始产物引用；执行器聊天只作辅助。

文件中的 approval 只能记录真实用户决定；编辑字段不能创造授权。用户新指令优先，冲突必须显式同步。

## 任务包

总控提供：任务 JSON、必读文档的确切版本、源码基准、依赖产物、已知问题、允许范围、适用门禁和预期报告。使用仓库相对路径，不绑定开发者电脑路径。

[任务模板](../tasks/TEMPLATE.json) 是未就绪模板，不能原样领取。结构字段：

| 字段 | 含义 |
| --- | --- |
| schema_version / id / kind / milestone / status | 稳定标识、任务类型、所属里程碑、当前状态 |
| title / objective / non_goals | 目标与明确不做的内容 |
| requirement_refs / required_reading | 需求依据及当前任务必读上下文 |
| source_baseline / dependencies | 源码 SHA 与依赖；有额外工作区改动时另记录快照 |
| authorization / blockers | 当前阶段授权、用户决定引用、阻塞原因 |
| scope | allowed_paths、protected_paths、generated_artifacts、network_and_data |
| steps / acceptance_criteria | 执行步骤与可核实完成标准 |
| verification | 允许的具体命令或门禁引用、结果要求 |
| stop_conditions / budget | 需求/环境/越界等停止规则，以及明确的自动化预算 |
| outputs / evidence | 应交付文件与实际证据引用 |

Ready 之前不得保留 TODO 占位、未知范围或未满足的必需授权。测试/文档路径必须写入允许范围，不能只允许 src 后再默认扩展到全仓。

新任务增加 risk、executor、policy_ref、note_requirement 与必要的 mechanical_actions 字段。source_baseline 是冻结参照；管理 agent 另记录本次执行的真实提交/快照，不把旧基准当作重置当前工作区的指令。首批 draft 任务由管理 agent 完成证据审查、Note 和运行基准后转为 ready，不需要用户逐项批准低风险任务。

## 输出约定

执行器返回简短摘要和结构化报告，至少包含：

```json
{
  "schema_version": 1,
  "task_id": "TASK-NNN",
  "run_id": "unique-run-id",
  "executor": {"provider": "record-actual-provider", "version": "record-actual-version"},
  "source_commit": "record-actual-commit",
  "workspace_snapshot": "record-if-not-a-clean-commit",
  "outcome": "ready_for_verification",
  "changed_files": [],
  "verification_reports": [],
  "unresolved_items": [],
  "proposed_next_step": "independent verification"
}
```

outcome 只表达执行器提交结果或阻塞，不允许用 approved/merged/released 代替用户决定。提供 JSON 或满足格式要求不代表内容正确；总控仍需核对差异、命令结果和适用版本。

## 验证、审查和状态权限

编码执行器不能自批需求、架构例外、门禁降级或范围扩展。总控通过实际文件差异检查路径，包括新增、删除、重命名及未跟踪文件。

验证工具在明确基准上独立执行，保存真实退出码/日志；不以编码 agent 的“全部通过”作为证据。产品代码、测试有效性和控制器代码的审查使用分开的上下文。门禁规则改动单独审查。

文档自检只证明链接、记录、引用和状态一致性；不替代应用测试或代码审查。当前 Claude 代码执行器的输入与结果格式另见 CLAUDE-WORKER，管理 agent 不直接采信其通过声明。

## 执行器适配与移交

未来适配器负责启动、输出收集、超时、取消和报告归一化。任务规范不依赖某一 CLI 的 session ID、隐式上下文或厂商专用目录。

Codex 非交互运行与结构化输出可参考 [官方文档](https://learn.chatgpt.com/docs/non-interactive-mode)。本机 Claude Code 2.1.270 已有实际代码委派和独立验证记录，调用方法见 CLAUDE-WORKER，已发现的管理记录问题见 BATCH-001 独立审查；没有独立适配器程序。

移交时保留：当前任务/状态、源码与补丁快照、变更清单、已执行命令和原始结果、未解决问题、下一步允许的动作。新执行器首先重新核对状态和差异，不盲目重做已完成操作。

具体换设备流程见 [DEVICE-HANDOFF](DEVICE-HANDOFF.md)。使用现有任务身份、完整运行历史与原始截止时间；交接记录只是这些权威文件的快照索引。新机的环境验证单独记录，不将旧机 PASS 改写为新机已经运行，也不因目录或设备变化而扩大权限。

## 自动化运行前的条件

先通过一个人工调度的完整试点，再实现最小程序。程序至少负责单任务领取、持久化状态、幂等恢复、有限重试、证据绑定和审批等待。

初版串行运行一个 Claude 执行器，使用现有任务 JSON。用户已确定 3 轮修复、90 分钟/任务、3 任务/批次，并明确不设费用上限；financial_cap_enabled=false 与未决定预算不同。其他未定义权限不能由执行器自行补全。
