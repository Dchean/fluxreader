# 进行中与待审阅视图

当前 0 个任务在跑（`max_concurrent_workers=1` 未违反）。

## 优化重构流程已完成（DEC-013，2026-09-13）

BATCH-003～006 全部合并至 main（CI 门禁通过）：
- fad4f35 BATCH-003：hooks lint 修复、OPT-004 字段清单、冒烟记录
- 443fb86 BATCH-004：OPT-004 白名单同步实现、CI fmt 门禁、测试隔离
- a0ce278 BATCH-005：M2 处置方案 + SQL 边界试点、注释清理、release 加固
- edf5cde BATCH-006：文档对齐、终验、发布就绪

终验：Rust 96/0/23、前端 8/8、lint 3 警告（技能副本）、clippy/fmt 0、NSIS+MSI 双安装包、桌面冒烟两次通过。

## 待用户决定

1. **发布**（打 tag；release.yml 已有 CI 全绿守卫）——需单独确认。
2. **后续整理程序**（TASK-021～024：store 拆 slice、db/sync 全量收敛、sync 分层、commands 拆组）——M2 处置方案已列，是否继续。
3. **真实服务验证**（ISSUE-006/007）——需真实账号决策。

证据：[BATCH-006 终验报告](runs/BATCH-006-final-20260913.json)。
