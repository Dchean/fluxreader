# 项目文档

先读 [项目工作入口](../WORKFLOW.md)、[自动状态总览](../tasks/PROJECT_STATE.md) 和 [产品定义](PRODUCT.md)。机器记录由 Agent 维护，空模板不代表需求已确认或测试已通过。

| 内容 | 入口 |
| --- | --- |
| 目标、范围、验收、待定项 | [PRODUCT](PRODUCT.md) |
| 当前状态与下一步 | [PROJECT_STATE](../tasks/PROJECT_STATE.md) |
| 技术取舍 | [ADR](ADR/README.md)，或项目已有 Notes/ADR |
| 用户决定 | [DECISIONS](../tasks/DECISIONS.json) |
| 执行和恢复命令 | [TOOLING](workflow/TOOLING.md) |
| 问答与参考检索 | [INTAKE](workflow/INTAKE.md)、[RESEARCH](workflow/RESEARCH.md) |
| 重构价值与试点 | [REFACTOR](workflow/REFACTOR.md)，先允许保持现状或仅补保护 |
| 有界面的预览与验收 | [FRONTEND](workflow/FRONTEND.md)，项目 UI 约定按需建立 |
| 有限自动接续 | [RECOVERY](workflow/RECOVERY.md) |
| 本项目调查与发现 | [BASELINE](BASELINE.md)、[FINDINGS-REQ-007](FINDINGS-REQ-007.md)、[FINDINGS-SYNC-GAP](FINDINGS-SYNC-GAP.md)、[FINDINGS-CI-GATE](FINDINGS-CI-GATE.md) |
| 冻结的界面契约 | [REQ-004/008](UI-CONTRACT-REQ-004-008.md)、[REQ-005](UI-CONTRACT-REQ-005.md)、[REQ-006/008](UI-CONTRACT-REQ-006-008.md) |
| 已知工具缺陷与修复 | [protocol 无恢复入口](TOOL-GAP-protocol-recovery.md)、[审查证据自锁](TOOL-GAP-review-evidence-self-lock.md)、[extend 死锁修复](TOOL-FIX-extend-deadlock.md)；缺陷仍在工具中存活，实施前先读 |

重构项目在 BASELINE.md 记录已有行为与检查结果。架构、接口、数据、测试计划等可先写在现有文档中，内容确实增多后再拆分，并在这里增加链接。Memory 和 Lessons 按需要使用，避免重复维护。
