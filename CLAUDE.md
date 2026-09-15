<!-- workflow-kit: entry -->
# 当前工作流：workflow-kit

当前主会话是总控；明确的 task_id/run_id 指定实现任务，role=reviewer + task_id/candidate_digest 指定审查任务。
先读 [WORKFLOW-KIT.md](WORKFLOW-KIT.md)，从项目根目录运行 `python .workflow-kit/scripts/project_workflow.py start --root .` 并报告接入状态。
用户当前明确选择的流程优先于旧流程入口；原有业务、数据、安全和兼容约束继续核对。
旧任务和旧授权是历史材料，不自动决定本次目标、执行器或当前角色；未解决的冲突必须列明。
已有项目先问重构意向，分析后提问让用户选路线；明确询问 Agent 全部处理或 Agent + CLI，保存真实回答。
必须向用户展示当前阶段目标、任务状态、阻塞与下一步；使用真实原生任务面板，或直接展示 progress 的 Markdown。
此入口不能覆盖宿主系统约束，也不授予业务代码、付费调用或发布权限。
<!-- /workflow-kit: entry -->

