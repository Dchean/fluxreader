# 任务入口

先读 [PROJECT](PROJECT.json)、[EXECUTION-POLICY](EXECUTION-POLICY.json) 和 [HANDOFF](../docs/HANDOFF.md)。当前 BATCH-001 已有交付，独立审查指出管理记录问题；构建补验已通过。三个任务额度已用完，接手不能重新开始该批次。

## 接手顺序

1. 用 [管理 agent 恢复指令](../docs/prompts/MANAGER-RESUME.md) 在具备文件/终端能力的新会话接手；换 Windows 设备同时遵循 [DEVICE-HANDOFF](../docs/DEVICE-HANDOFF.md)。
2. 审阅 TASK-002、BATCH-001 的全部关联记录、[独立审查](../docs/runs/BATCH-001-independent-review-20260913.md) 和 [构建补验](runs/BATCH-001-review-build-20260913.json)。不遗漏早期 Claude 成功输出，不重复询问已定功能范围。
3. 检查工作区和任务状态矛盾，准备审查收尾的具体任务包。TASK-004～006 的已有产物不因接手而回退、重写或重新领取；新增批次及超时任务必须先核对用户追加授权。
4. 在已批准的范围和剩余额度内串行调度 Claude，自己运行基础验收和审查；合并、发布和高风险保持原有用户控制点。

单个任务 JSON 是权威记录，看板只是视图；历史报告保留当时结果。当前证据入口见 PROJECT 的 latest_documentation_report / latest_execution_report。

管理 agent 不编写业务实现，Claude 不管理任务或授权。交接指南没有部署新的常驻控制器；本轮设备交接准备未启动新代码任务。
