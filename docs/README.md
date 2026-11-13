# 文档索引

本目录保存需要随产品长期维护的事实：当前架构、开发与门禁、备份恢复、发布流程、同步兼容矩阵、性能基线与仍开放的缺口。决定理由与取舍归 `.agents/notes/`；未解决的问题集中在 [roadmap.md](roadmap.md)。文档状态、历史通过数不代替当前行为证据。

| 文档 | 内容与维护时机 |
| --- | --- |
| [架构](architecture.md) | Rust 模块与依赖方向、数据库归属、本地同步状态边界、前端数据流、并发与写预留纪律；模块边界变化时同步 |
| [开发与门禁](development.md) | 本地环境、脚本与 CI job 对应、测试分层、证据纪律、常见故障；流程或工具变化时同步 |
| [同步兼容矩阵](sync-compat-matrix.md) | 当前 Miniflux/GReader/Fever/WebDAV 的能力差异、冲突政策与验证边界；协议或政策变化时同步 |
| [性能基线与方法](performance.md) | 判定线、测量纪律、当前 10k/50k 基线与仍适用的查询/封面/画廊预算 |
| [备份与恢复](backup.md) | 便携 v4、WebDAV 持续同步、本机故障快照与应用外恢复；生命周期变化时同步 |
| [发布流程](release.md) | 分支、CI、tag 附注与 Release 正文的关系；流程变化时同步 |
| [当前缺口与已知限制](roadmap.md) | 仍成立的开放缺口、接受的边界与未验收范围 |

## 决定记录

`.agents/notes/` 只保留仍约束当前代码的契约、流程与测试纪律；普通局部修复不新增笔记；缺陷原因与防重边界已并入上方架构主题篇，不再单列目录。每篇按 lifecycle/class 路径保存，结构见 [AGENTS.md](../AGENTS.md)。

- [架构与跨模块契约](../.agents/notes/implemented/architecture/)
- [功能取舍](../.agents/notes/implemented/feature/)
- [简化理由](../.agents/notes/implemented/simplification/)
- [测试策略与证据边界](../.agents/notes/implemented/testing/)
- [流程选择](../.agents/notes/implemented/process/)

## 历史材料

- 旧审计与优化任务卡在固定提交 `91ece562d79fd909642cd6d570d636e884b7387e`，一次性探针与测量工具在分支 `archive/tooling-and-reports`。
- 本机 `tmp/` 证据已清理，只保留 `rust-env/`、`refactor-2026-11-09/` 与 `ui-layout-evidence/`。
- 2026-11-13 文档整理删除的旧记录可从提交 `b5590e3` 恢复（清单见提交正文）。
