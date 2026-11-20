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
| 启用三层编排或交接总控 | [三层编排](../AGENTS.md#三层编排) | [审计与证伪](maintenance.md#2-审计与证伪) 的登记分工 |
| 接手审计或修复基线 | [审计记录](audits.md) | 该轮的审计报告与证据位置 |

## 文档清单

| 文档 | 内容 | 代码锚点 | 触发更新（改到就同批更） | 最近核对 |
| --- | --- | --- | --- | --- |
| [架构](architecture.md) | 模块与依赖方向、数据库归属、同步与本地同步边界、前端数据流、并发与写预留 | `lib.rs`、`commands/`、`db/`、`sync/`、`local_sync/`、`src/store/` | 模块边界、依赖方向、锁序或数据归属变化 | 2026-11-19（基线 30cfd65） |
| [契约单点](contracts.md) | 每类事实的唯一出处、消费者、同步面与锁定方式 | 各行的"唯一出处"列 | 新增或搬迁契约、改变消费者或同步面 | 2026-11-19（基线 30cfd65） |
| [维护与文档纪律](maintenance.md) | 证据标准、审计与证伪、变更流程、冲突处置、写作纪律、门禁 | `tools/docs-check.mjs` | 流程或门禁变化 | 2026-11-20（基线 8fe8a5e；审计登记分工） |
| [开发与门禁](development.md) | 本地环境、脚本与 CI job 对应、测试分层、证据纪律、常见故障 | `package.json`、`.github/workflows/ci.yml`、`tmp/rust-env/` | 脚本、CI、测试分层或环境变化 | 2026-11-19（基线 30cfd65） |
| [同步兼容矩阵](sync-compat-matrix.md) | GR/Fever/WebDAV 的能力差异、冲突政策、真实服务端证据与边界 | `sync/conflict_policy.rs`、`greader.rs`、`fever.rs`、`sync/`、`local_sync/` | 协议实现、冲突政策或服务端证据变化 | 2026-11-19（基线 30cfd65；真实服务端实测 2026-11-07 见本篇） |
| [性能基线](performance.md) | 判定线、测量纪律、10k/50k 基线与查询/封面/画廊/图片代理预算 | `scheduler.rs`、`db/articles.rs`、`commands/settings.rs`、`src/lib/imageProxy.ts` | 预算、索引、容量口径或方法与基线变化 | 2026-11-19（基线 30cfd65） |
| [备份与恢复](backup.md) | 便携 v4、WebDAV 持续同步、本机故障快照与恢复边界 | `local_sync/backup.rs`、`db/backup.rs` | 备份格式、恢复语义或生命周期变化 | 2026-11-19（基线 30cfd65） |
| [发布流程](release.md) | 分支、CI、tag 附注与 Release 正文的关系 | `.github/workflows/release.yml`、`tools/release/` | 发布步骤、tag 或资产策略变化 | 2026-11-19（基线 30cfd65） |
| [当前缺口](roadmap.md) | 仍成立的开放缺口、接受的边界与未验收范围 | 各条目的主题篇 | 缺口关闭、新增或范围变化 | 2026-11-19（基线 30cfd65） |
| [审计记录](audits.md) | 各轮审计与证伪的结论摘要、基线、证据位置与复跑入口 | `tmp/` 证据 | 新一轮审计或结论状态变化 | 2026-11-20（基线 2ab27db；三层编排规则独审登记） |
| [前端结构台账](frontend.md) | 组件清单、类名账目、令牌使用与动态拼接契约、控件层现状 | `src/components/`、`src/store/`、`src/styles/`、`src/lib/syncPill.ts` | 增删组件、类名体系或令牌变化 | 2026-11-20（基线 d40c4f1；社交/通知卡局部译文状态行；类计数口径见本篇） |

本页（索引）最近核对：2026-11-20（基线 2ab27db；三层编排路由与审计登记核对）；各篇「最近核对」的填写口径见 [维护与文档纪律](maintenance.md) 第 7 节。

## 决定记录

`.agents/notes/` 只保留仍约束当前代码的契约、流程与测试纪律；普通局部修复不新增笔记；缺陷原因与防重边界已并入架构主题篇。每篇按 lifecycle/class 路径保存，结构见 [AGENTS.md](../AGENTS.md)。

- [架构与跨模块契约](../.agents/notes/implemented/architecture/)
- [功能取舍](../.agents/notes/implemented/feature/)
- [简化理由](../.agents/notes/implemented/simplification/)
- [测试策略与证据边界](../.agents/notes/implemented/testing/)
- [流程选择](../.agents/notes/implemented/process/)

按主题查高频活跃篇（其余按上面的分类目录进）：

| 主题 | 决定记录 |
| --- | --- |
| 同步冲突政策与写回状态 | [同步冲突政策单点](../.agents/notes/implemented/architecture/2026-10-07-同步冲突政策单点.md)、[同步写回与状态一致性](../.agents/notes/implemented/architecture/2026-11-13-同步写回与状态一致性.md) |
| 同步队列生命周期与本地意图 | [同步队列生命周期与本地意图](../.agents/notes/implemented/architecture/2026-11-03-同步队列生命周期与本地意图.md) |
| 订阅写回 outbox 与能力矩阵 | [订阅写回 outbox 执行器](../.agents/notes/implemented/architecture/2026-10-14-订阅写回outbox执行器.md)、[订阅写回能力矩阵与持久回执](../.agents/notes/implemented/architecture/2026-10-15-订阅写回能力矩阵与持久回执.md) |
| 查询与分页 | [查询与分页契约](../.agents/notes/implemented/architecture/2026-10-05-查询与分页契约.md) |
| 乐观写入 | [乐观写入的版本守卫](../.agents/notes/implemented/architecture/2026-10-05-乐观写入的版本守卫.md) |
| 正文与 AI 产物缓存、验签 | [正文与 AI 产物的实体缓存](../.agents/notes/implemented/architecture/2026-10-06-正文与-AI-产物的实体缓存.md)、[AI 完成信号与缓存提交](../.agents/notes/implemented/architecture/2026-10-08-AI完成信号与缓存提交.md) |
| 可渲染正文安全边界 | [可渲染正文安全边界](../.agents/notes/implemented/architecture/2026-10-08-可渲染正文安全边界.md) |
| 凭据与账号边界 | [凭据失败关闭与受控更新检查](../.agents/notes/implemented/architecture/2026-10-08-凭据失败关闭与受控更新检查.md)、[账号会话与配置应用边界](../.agents/notes/implemented/architecture/2026-10-08-账号会话与配置应用边界.md) |
| WebDAV 便携恢复与本地同步 | [纯本地资料库同步与便携恢复](../.agents/notes/implemented/architecture/2026-11-09-纯本地资料库同步与便携恢复.md)、[本地同步与便携恢复界面边界](../.agents/notes/implemented/architecture/2026-11-09-本地同步与便携恢复界面边界.md) |
| 数据库快照与恢复 | [数据库快照与恢复边界](../.agents/notes/implemented/architecture/2026-11-05-数据库快照与恢复边界.md) |
| 前端布局、焦点与交互契约 | [全布局键盘焦点与响应式阅读列](../.agents/notes/implemented/architecture/2026-11-04-全布局键盘焦点与响应式阅读列.md)、[前端交互契约](../.agents/notes/implemented/feature/2026-11-13-前端交互契约.md) |
| 测试门禁与证据纪律 | [测试门禁与断言纪律](../.agents/notes/implemented/testing/2026-09-16-测试门禁与断言纪律.md)、[验证方法与复核纪律](../.agents/notes/implemented/testing/2026-09-22-验证方法与复核纪律.md)、[修复波次的分批、证伪审计与证据门槛](../.agents/notes/implemented/process/2026-11-03-修复波次的分批、证伪审计与证据门槛.md) |
| 维护与文档纪律 | [维护与文档纪律](../.agents/notes/implemented/process/2026-11-14-维护与文档纪律.md) |

## 一次性材料

审计报告、探针脚本、完整日志与调研输出属一次性材料：放忽略的 `tmp/` 或存档分支 `archive/tooling-and-reports`。**本页不再枚举 `tmp/` 内容**——那是随任务变化的临时区，枚举必然过期。需要长期保留的只有结论摘要，登记在 [审计记录](audits.md)。

## 历史材料

- 旧审计与优化任务卡在固定提交 `91ece562d79fd909642cd6d570d636e884b7387e`，一次性探针与测量工具在分支 `archive/tooling-and-reports`。
- 2026-11-13 文档整理删除的旧记录可从提交 `b5590e3` 恢复（清单见提交正文）。
- 历史材料的定位与提交号对照以 Git 为准，不在本页维护快照。
