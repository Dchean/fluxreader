# 文档索引

本目录保存需要长期与产品同步维护的事实；决定理由归 `.agents/notes/`，未解决的问题集中在 `roadmap.md`。文档状态和历史通过数不代替当前行为证据。

| 文档 | 内容与维护时机 |
| --- | --- |
| [同步兼容矩阵](sync-compat-matrix.md) | 协议/服务端能力、客户端政策和验证边界分别列出；政策或协议行为变化时同步 |
| [性能基线与方法](performance.md) | 带日期、构建、语料和测量范围的结果，以及不能外推的限制 |
| [备份与恢复](backup.md) | 快照触发、保留策略、内容和应用外恢复步骤；生命周期变化时同步 |
| [发布流程](release.md) | 分支、CI、tag 附注与 Release 正文的关系；流程变化时同步 |
| [当前缺口与已知限制](roadmap.md) | 仍成立的问题、接受的取舍及未验收范围；问题修复或证据改变时更新 |

## 决定记录

`.agents/notes/` 只保留难以从代码和测试恢复、未来仍需重访的重要理由、约束与取舍。普通局部修复不必新增笔记；既有决定事实失实则更新归属篇。每篇按项目现有 lifecycle/class 路径保存，结构见 [AGENTS.md](../AGENTS.md)。

- [架构与跨模块契约](../.agents/notes/implemented/architecture/)
- [功能取舍](../.agents/notes/implemented/feature/)
- [缺陷原因与防重引入边界](../.agents/notes/implemented/bug-fix/)
- [简化理由](../.agents/notes/implemented/simplification/)
- [测试策略与证据边界](../.agents/notes/implemented/testing/)
- [流程选择](../.agents/notes/implemented/process/)

关键实现的 `Note:` 注释指向实际归属。`implemented` 表示所记决定落地，不表示整篇事实永久有效、全部测试通过或产品已发布。当前轮次交付状态只在[现有验收入口](../.agents/notes/implemented/process/2026-11-03-修复波次的分批、证伪审计与证据门槛.md)维护简短摘要；专题篇不复制临时进度。

## 历史材料

2026-11-06 整理将旧审计及优化任务卡清出当前工作树。原文完整保留于固定提交 `91ece562d79fd909642cd6d570d636e884b7387e`，不改写其中的历史结论、失败或当时授权：

- [2026-10-08 审计快照](https://github.com/Dchean/fluxreader/blob/91ece562d79fd909642cd6d570d636e884b7387e/docs/audit-2026-10-08.md)
- [优化执行记录与任务卡](https://github.com/Dchean/fluxreader/tree/91ece562d79fd909642cd6d570d636e884b7387e/docs/optimization)

本地也可用 `git show 91ece562:docs/optimization/OPT-010.md` 等命令读取原文件。仍有效的产品约束由当前文档与活跃 Note 承接；旧派发、模型选择、临时写集和当时状态不再作为执行指令。

更早的一次性探针与测量工具位于既有分支 `archive/tooling-and-reports`；它不包含上述新清出的材料。其中 `archive/reports/BASELINE.md` 保存 2026-09-16 历史重写前后的提交号对照。两处历史来源不能混称。

2026-11-05/06 只读审计原始报告、日志、截图和探针保留在本机忽略目录 `tmp/audit-2026-11-05/` 与 `tmp/audit-2026-11-06/`。它们不是每次克隆都会获得的附件；需复核对应本地证据时应保留整目录。长期事实在各主题文档中说明证据时点及上限，不把旧运行结果拼成当前全量通过。

已封印的 `.agents/notes/archived/` 是历史快照，正文与既有 manifest 条目冻结。新解释和后继关系写在活跃篇；旧快照中的过去时结论不指导当前实现。
