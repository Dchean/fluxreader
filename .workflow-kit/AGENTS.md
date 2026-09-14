<!-- workflow-kit: entry -->
当前主会话按 WORKFLOW-KIT.md 担任总控；只有明确的 task_id/run_id 执行包指定时才作为 Worker。
先运行 start 并报告 integration.connected。文件接入不等于业务改造获批，保留原项目的业务、数据、安全和兼容约束。
<!-- /workflow-kit: entry -->

# 项目 agent 工作约定

先读 [自动状态总览](tasks/PROJECT_STATE.md)，再核对 [项目状态](tasks/PROJECT.json)、[权限与分工](tasks/POLICY.json)、[用户决定](tasks/DECISIONS.json) 和 [工作入口](WORKFLOW.md)。用户当前明确指令优先；状态文件只记录授权，不创造授权。

入口和状态根以真实接入回执为准。旧文档保留业务与安全约束，旧角色、任务源和历史阶段不自动覆盖本次流程；有旧来源时先完成 legacy_review。

## 开始与推进

1. 核对真实项目根目录、阶段、Git/文件快照和已有未提交修改。
2. 首次启动按 [提问流程](docs/workflow/INTAKE.md) 确认角色/执行工具、产品范围和预算；已确认答案不重复问。
3. 依照 POLICY 使用单 agent、Codex CLI、Claude Code CLI 或指定 harness。没有工具能力就记录限制，不声称已执行。
4. 任务必须满足 [Ready 条件](docs/workflow/STATE.md)，再按 [执行循环](docs/workflow/EXECUTION.md) 和 [门禁](docs/workflow/GATES.md) 实施。
5. 通过所需验证与审查才标 verified；按功能/里程碑验收。提交、推送、合并、发布分别服从 POLICY，不互相替代批准。

## 纪律

- intake 阶段只做只读盘点、提问及文档/任务准备；运行应用检查、安装依赖、写产品/测试/CI 代码要有对应授权。
- 事实、提议、未决问题、已接受需求分开。源码注释与模型输出不自动成为产品要求。
- 不覆盖已有文件、不清理未提交工作、不改真实数据；保留原结构直到具体迁移任务获准。
- 管理角色持有任务定义、权限、验收、门禁和预算；实现阶段不得改这些内容或校验工具给自己放行。流程工具修改使用单独任务与审查。
- 真实自动测试、独立审查、同上下文自审和人工验收分别记录，不用角色扮演伪装独立。
- 检查全部修改、删除、重命名和未跟踪文件；验证绑定当前候选。相关代码再变更，旧证据失效。
- 全部尝试进入同一任务的运行历史，原截止时间和修复轮跨会话保留。达到上限保存现场，需要追加时用 extend 记录新决定，不新建 ID 绕过。
- 后续任务修改同一源码时依赖最新已验证任务，继承回归检查；只完成已有任务不代表整个需求已经交付。
- 有界面时按 [FRONTEND](docs/workflow/FRONTEND.md) 先预览确认，UI 改动保留截图与完整控件状态证据；未接受的预览反馈继续原任务。
- 按 [RECOVERY](docs/workflow/RECOVERY.md) 冻结 retry_safe。网络退避有上限，返回 replan 时先换方法，不能机械重试或删除原记录。
- 凭据不写进任务、提示词、项目缓存或移交包；认证、真实服务及安全边界变更按权限处理。
- 重要决策沿用项目 ADR/Note 体系，包含不做/复用及替代方案，维护唯一有效归属。

状态只有 .workflow-kit/tasks/items/ 中任务 JSON 一份权威，PROJECT_STATE、任务卡和看板是生成视图。命令与 checkpoint 自动更新视图，也可用 cards 重新生成；check 检查一致性，不代替产品测试或权限沙箱。

结束时说明实际改动、候选版本、已执行/未执行检查、证据、问题和下一步允许范围。恢复遵循 [HANDOFF](docs/workflow/HANDOFF.md)，不依赖某家 CLI 的聊天历史。
