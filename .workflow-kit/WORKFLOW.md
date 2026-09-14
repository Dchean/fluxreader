# 项目工作入口

先运行下面的 start，核对 integration.connected、实际入口、状态文件和当前任务；仅阅读本文件不证明已经接入。代码权限以当前 POLICY 与任务为准，接入文件不创造业务授权。

先读 [自动状态总览](tasks/PROJECT_STATE.md)，再核对 [真实项目状态](tasks/PROJECT.json)、[用户决定](tasks/DECISIONS.json) 和当前任务。

~~~text
python .workflow-kit/scripts/project_workflow.py start --root .
~~~

首次按 [问答流程](docs/workflow/INTAKE.md) 补充未知的业务信息，沿用已确认答案和授权。Agent 保存需求、参考记录与任务卡，用户无需编辑 JSON。

[工具说明](docs/workflow/TOOLING.md) 列出实际命令。断线后先检查文件、日志和进程，再从原任务恢复；需要更多时间时追加明确授权的预算，保留原始时钟和历史。

PROJECT_STATE、任务卡和看板是自动视图；机器任务记录是进度事实。技术选择沿用已有 ADR/Notes，普通任务不要求新增整套文档。

原有业务、数据、安全和兼容约束继续核对；流程入口、角色和任务源按用户当前选择及 legacy_review 明确处理。不要因旧文档把当前总控当成 Worker，也不要自动恢复不属于本次目标的旧任务。

有界面时按 [前端流程](docs/workflow/FRONTEND.md) 展示可点击预览并确认；按 [恢复规则](docs/workflow/RECOVERY.md) 对可安全重复的任务有限自动接续。界面、网络或角色职责都不靠额外手写一套状态维持。
