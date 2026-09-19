# workflow-kit 当前项目入口

从项目根目录运行：

```text
python .workflow-kit/scripts/project_workflow.py resume --root .
```

先报告 integration.connected、目标目录、当前阶段和任务路径；未接入或校验失败时先排障，不能静默转回旧流程。

当前主会话是总控；Worker 身份只来自明确的任务执行包。接入文件不代表业务改造已经获批。

- [当前状态](.workflow-kit/tasks/PROJECT_STATE.md)
- [新流程](.workflow-kit/WORKFLOW.md)
- [问答](.workflow-kit/docs/workflow/INTAKE.md)

## 原项目资料与冲突处理

以下资料的产品、数据、安全和兼容约束继续核对；旧的角色、任务源、启动顺序不能静默覆盖用户当前选择。

onboard 前在 legacy_review 记录已读来源、保留约束、旧任务处置和流程冲突结论。新目标不重置旧任务预算，也不借旧批准自动开始新的工作。


WorkBuddy 或其他宿主若没有自动读取项目入口，在新会话首条消息明确要求读取本文件并运行 resume；不要假定某个厂商会自动加载所有 Markdown。
