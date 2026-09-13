# 文档入口

本批文档已更新到 2026-09-13 的独立审查与设备交接准备状态。源码基准：6550a223d3e5fb4cae66b07af31fc35c5f1fd6a9。前端 8/8、Rust 95 项、Clippy、Rustfmt 及构建复验通过；Note 校验和管理记录有待收尾，真实桌面和服务验收未完成。新接手方先读 [HANDOFF](HANDOFF.md)；换 Windows 设备按 [DEVICE-HANDOFF](DEVICE-HANDOFF.md) 操作。

## 五分钟接手

1. 阅读 [项目状态](../tasks/PROJECT.json)，确认允许的阶段。
2. 阅读 [PRODUCT](PRODUCT.md) 与 [FEATURES](FEATURES.md) 的已确认范围，再读 [执行权限](../tasks/EXECUTION-POLICY.json)。
3. 阅读 [PROCESS](PROCESS.md)、[EXECUTION-CONTRACT](EXECUTION-CONTRACT.md)。
4. 找到 [任务入口](../tasks/README.md) 中的当前任务，按引用阅读所需架构和测试资料。
5. 只有任务满足进入条件且用户已授权相关阶段，才执行其命令。

## 文档职责

| 文档 | 负责回答 | 性质 |
| --- | --- | --- |
| [PRODUCT](PRODUCT.md) | 用户目标、授权边界、未决产品问题 | 已确认决定 + 明示的待定项 |
| [FEATURES](FEATURES.md) | 核心能力与附加功能保留清单 | 需求映射与源码盘点 |
| [USER-FLOWS](USER-FLOWS.md) | 当前关键流程及待核实行为 | 静态追踪，非 UI 验收结论 |
| [ARCHITECTURE](ARCHITECTURE.md) | 当前模块和调用关系 | 现状，不是目标设计 |
| [DATA-MODEL](DATA-MODEL.md) | SQLite 实体与存储边界 | 源码事实，未操作实际数据库 |
| [API](API.md) | IPC 与外部协议边界 | 当前接口索引，非完整规范 |
| [BASELINE](BASELINE.md) | 源码、环境、实际结果和未运行范围 | 可追溯的测量记录 |
| [ISSUES](ISSUES.md) | 问题、证据、影响、后续处理 | 区分确认事实与待验证风险 |
| [TEST-PLAN](TEST-PLAN.md) | 测量命令与后续验证方向 | 已执行基线方案 + 后续保护计划 |
| [HANDOFF](HANDOFF.md) | 管理 agent 如何接手并自主推进 | 角色、风险和运行约定 |
| [DEVICE-HANDOFF](DEVICE-HANDOFF.md) | 换设备时复制什么、如何保存现场和恢复 | Windows 交接步骤，不增加权限或预算 |
| [MANAGER-RESUME](prompts/MANAGER-RESUME.md) | 新 agent 的接手指令 | 恢复已有项目，不重新初始化 |
| [CLAUDE-WORKER](CLAUDE-WORKER.md) | Claude 的输入、参数与结果处理 | CLI 调用约定，新机能力需重新核对 |
| [PROCESS](PROCESS.md) | 阶段、状态、角色、进入退出条件 | 按用户边界制定的工作流程 |
| [EXECUTION-CONTRACT](EXECUTION-CONTRACT.md) | 跨 agent 任务包与交付格式 | 执行约定，尚无自动执行程序 |
| [RELEASE-ROLLBACK](RELEASE-ROLLBACK.md) | 合并、发布、回滚证据与权限 | 后续阶段的约束 |
| [ADR](ADR/README.md) | 重要决策的理由及生命周期 | 决策记录规则 |

当前继续工作的证据入口：[独立审查](runs/BATCH-001-independent-review-20260913.md)、[构建补验](../tasks/runs/BATCH-001-review-build-20260913.json)。BATCH-001 已用满三个任务；接手先准备审查收尾，实际新批次执行仍需用户追加授权。

## 事实和决策

- 用户已确认：可作为需求与授权依据。
- 源码已核实：仅说明代码中存在该实现，不表示运行正确。
- 待运行验证：必须通过测试或实际界面验证才能确认。
- 提议 / 待用户决定：不能直接转换为实施任务。

目标架构在测试基线与模块处置方案完成后单独形成；本批文档没有批准更换技术栈或整体重写。功能保留范围已按 DEC-008 / DEC-009 更新为核心与全部 OPT 项必须保留，其中 OPT-004 仅同步订阅源与客户端设置；代码修改按当前风险授权与具体任务推进，不用历史阶段文字覆盖新的用户授权。

文档引用源码以路径和符号为主；历史证据绑定 BASELINE 中的提交。后续改动同时更新相关文档，避免把新实现与旧证据混在一起。
