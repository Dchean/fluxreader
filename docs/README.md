# 文档索引与阅读路由

本目录保存需随产品长期维护的事实。**不要通读**：先按任务在下表找 1–3 篇，需要时再顺着"代码锚点"进代码。决定理由与取舍归 `.agents/notes/`；开放缺口集中在 [当前缺口](roadmap.md)；维护规则见 [维护与文档纪律](maintenance.md)。

## 任务 → 必读

| 我要做的事 | 先读 | 需要时再看 |
| --- | --- | --- |
| 改同步协议、冲突政策、读藏语义 | [同步兼容矩阵](sync-compat-matrix.md) | [架构](architecture.md) 的同步边界、[契约单点](contracts.md) |
| 改 schema、迁移、查询或分页 | [架构](architecture.md) 的数据库归属 | [性能基线](performance.md) 的查询预算、[契约单点](contracts.md) |
| 改前端状态、交互或组件 | [架构](architecture.md) 的前端结构与数据流 | [开发与门禁](development.md) 的测试分层、`src/store/internals.ts` |
| 改 AI 生成、缓存或验签 | [契约单点](contracts.md) 的"正文与 AI 产物身份" | [当前缺口](roadmap.md) 的 AI 段、`ai.rs` / `commands/ai.rs` |
| 改备份、便携导出、WebDAV 本地同步 | [备份与恢复](backup.md) | [同步兼容矩阵](sync-compat-matrix.md) 的 WebDAV 段、[契约单点](contracts.md) |
| 改门禁、测试或工具链 | [开发与门禁](development.md) | [维护与文档纪律](maintenance.md) 第 8 节 |
| 做发布 | [发布流程](release.md) | [开发与门禁](development.md) 的 CI 对应表 |
| 定位"这个功能在哪、改了会影响谁" | [契约单点](contracts.md) | [架构](architecture.md) 的模块与依赖方向 |
| 查已知缺口或什么没验证 | [当前缺口](roadmap.md) | 对应主题篇的"验证边界" |
| 改前端组件、类名或令牌 | [前端结构台账](frontend.md) | [架构](architecture.md) 的前端结构与数据流、[契约单点](contracts.md) |
| 查历史决定为什么这么做 | [.agents/notes/](../.agents/notes/) | 该 Note 的 Alternatives 与 Consequences |
| 接手审计或修复基线 | [审计记录](audits.md) | 该轮的审计报告与证据位置 |

## 文档清单

| 文档 | 内容 | 代码锚点 | 触发更新（改到就同批更） | 最近核对 |
| --- | --- | --- | --- | --- |
| [架构](architecture.md) | 模块与依赖方向、数据库归属、同步与本地同步边界、前端数据流、并发与写预留 | `lib.rs`、`commands/`、`db/`、`sync/`、`local_sync/`、`src/store/` | 模块边界、依赖方向、锁序或数据归属变化 | 2026-11-14（基线 d4a460d） |
| [契约单点](contracts.md) | 每类事实的唯一出处、消费者、同步面与锁定方式 | 各行的"唯一出处"列 | 新增或搬迁契约、改变消费者或同步面 | 2026-11-14 |
| [维护与文档纪律](maintenance.md) | 证据标准、审计与证伪、变更流程、冲突处置、写作纪律、门禁 | `tools/docs-check.mjs` | 流程或门禁变化 | 2026-11-14 |
| [开发与门禁](development.md) | 本地环境、脚本与 CI job 对应、测试分层、证据纪律、常见故障 | `package.json`、`.github/workflows/ci.yml`、`tmp/rust-env/` | 脚本、CI、测试分层或环境变化 | 2026-11-14 |
| [同步兼容矩阵](sync-compat-matrix.md) | GR/Fever/WebDAV 的能力差异、冲突政策、真实服务端证据与边界 | `conflict_policy.rs`、`greader.rs`、`fever.rs`、`sync/`、`local_sync/` | 协议实现、冲突政策或服务端证据变化 | 2026-11-07（真实服务端实测） |
| [性能基线](performance.md) | 判定线、测量纪律、10k/50k 基线与查询/封面/画廊/图片代理预算 | `scheduler.rs`、`db/articles.rs`、`commands/settings.rs`、`src/lib/imageProxy.ts` | 预算、索引、容量口径或方法与基线变化 | 2026-11-13 |
| [备份与恢复](backup.md) | 便携 v4、WebDAV 持续同步、本机故障快照与恢复边界 | `local_sync/backup.rs`、`db/backup.rs` | 备份格式、恢复语义或生命周期变化 | 未标 |
| [发布流程](release.md) | 分支、CI、tag 附注与 Release 正文的关系 | `.github/workflows/release.yml`、`tools/release/` | 发布步骤、tag 或资产策略变化 | 未标 |
| [当前缺口](roadmap.md) | 仍成立的开放缺口、接受的边界与未验收范围 | 各条目的主题篇 | 缺口关闭、新增或范围变化 | 2026-11-14 |
| [审计记录](audits.md) | 各轮审计与证伪的结论摘要、基线、证据位置与复跑入口 | `tmp/` 证据 | 新一轮审计或结论状态变化 | 2026-11-14 |
| [前端结构台账](frontend.md) | 组件清单、类名账目、令牌使用与动态拼接契约、控件层现状 | `src/components/`、`src/store/`、`src/styles/`、`src/lib/syncPill.ts` | 增删组件、类名体系或令牌变化 | 2026-11-15（基线 77c1964） |

## 决定记录

`.agents/notes/` 只保留仍约束当前代码的契约、流程与测试纪律；普通局部修复不新增笔记；缺陷原因与防重边界已并入架构主题篇。每篇按 lifecycle/class 路径保存，结构见 [AGENTS.md](../AGENTS.md)。

- [架构与跨模块契约](../.agents/notes/implemented/architecture/)
- [功能取舍](../.agents/notes/implemented/feature/)
- [简化理由](../.agents/notes/implemented/simplification/)
- [测试策略与证据边界](../.agents/notes/implemented/testing/)
- [流程选择](../.agents/notes/implemented/process/)

## 一次性材料

审计报告、探针脚本、完整日志与调研输出属一次性材料：放忽略的 `tmp/` 或存档分支 `archive/tooling-and-reports`。**本页不再枚举 `tmp/` 内容**——那是随任务变化的临时区，枚举必然过期。需要长期保留的只有结论摘要，登记在 [审计记录](audits.md)。

## 历史材料

- 旧审计与优化任务卡在固定提交 `91ece562d79fd909642cd6d570d636e884b7387e`，一次性探针与测量工具在分支 `archive/tooling-and-reports`。
- 2026-11-13 文档整理删除的旧记录可从提交 `b5590e3` 恢复（清单见提交正文）。
- 历史材料的定位与提交号对照以 Git 为准，不在本页维护快照。
