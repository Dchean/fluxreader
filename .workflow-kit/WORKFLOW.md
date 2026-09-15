# 项目工作入口

先运行下面的 start，核对 integration.connected、实际入口、状态文件和当前任务；仅阅读本文件不证明已经接入。代码权限以当前 POLICY 与任务为准，接入文件不创造业务授权。

先读 [自动状态总览](tasks/PROJECT_STATE.md)，再核对 [真实项目状态](tasks/PROJECT.json)、[用户决定](tasks/DECISIONS.json) 和当前任务。

~~~text
python .workflow-kit/scripts/project_workflow.py start --root .
~~~

首次按 [问答流程](docs/workflow/INTAKE.md) 使用“提问＋补充”，沿用已确认答案。已有项目先问重构意向，分析后让用户选路线；必须明确询问 Agent 全部处理还是 Agent + CLI。用 intake 分轮保存真实回答，start/next 补缺项，onboard 校验后再执行。用户无需编辑 JSON。

明确收集已有 Bug、新功能、其他补充与质量目标。Agent 包办也可使用独立审查上下文，不等于只能自审。按 [展示规则](docs/workflow/PRESENTATION.md) 直接呈现阶段目标和任务状态；有原生面板时同步，否则在对话显示 progress，不只保存文件。

以可维护、稳定和性能为目标，不以实现工作量少作选择。完整目标分步验证，防空转预算不用于压低验收标准；有必要的复杂度须以收益与证据说明。

[工具说明](docs/workflow/TOOLING.md) 列出实际命令。断线后先检查文件、日志和进程，再从原任务恢复；需要更多时间时追加明确授权的预算，保留原始时钟和历史。

PROJECT_STATE、任务卡和看板是自动视图；机器任务记录是进度事实。技术选择沿用已有 ADR/Notes，普通任务不要求新增整套文档。

原有业务、数据、安全和兼容约束继续核对；流程入口、角色和任务源按用户当前选择及 legacy_review 明确处理。不要因旧文档把当前总控当成 Worker，也不要自动恢复不属于本次目标的旧任务。

有界面时按 [前端流程](docs/workflow/FRONTEND.md) 展示可点击预览并确认；按 [恢复规则](docs/workflow/RECOVERY.md) 对可安全重复的任务有限自动接续。界面、网络或角色职责都不靠额外手写一套状态维持。
