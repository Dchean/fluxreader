# 文档索引

`docs/` 只放需要长期与代码同步维护的事实。判断一段内容该不该写在这里，先看它属于哪一类：

| 类别 | 文件 | 什么时候更新 | 不该放什么 |
| --- | --- | --- | --- |
| 长期事实 | [performance.md](performance.md) | 代码、数据或测量方法变化使结论失效时同批更新；每条结论必须带构建、语料、日期与测量范围 | 一次性审计结论、任务进度、未经验证的收益 |
| 长期事实 | [sync-compat-matrix.md](sync-compat-matrix.md) | 协议行为、客户端策略或测试锁定变化时同批更新（政策点、本文、锁定测试三处一致） | 把「客户端未实现」写成「协议不支持」；把未实机验收的版本写成已验证 |
| 长期事实 | [backup.md](backup.md) | 快照触发、保留策略、备份内容或恢复边界变化时同批更新 | 把应用内恢复写成已实现；把备份等同于跨用户可移植 |
| 长期事实 | [release.md](release.md) | 分支流向、CI/Release 工作流、tag 附注结构或版本策略变化时更新 | 发布当次的进度播报、临时排障记录 |
| 现状与限制 | [roadmap.md](roadmap.md) | 缺口修复后删除；新限制经代码或审计确认后加入 | 已收口事项、修复步骤、逐条证据 |
| 历史材料（待决策） | [audit-2026-10-08.md](audit-2026-10-08.md)、[optimization/](optimization/) | 只读留存，不再追加长期事实 | 当前验收结论、需要持续维护的协议或性能事实 |

## 长期事实

- **[performance.md](performance.md)**：大库性能基线、测量方法与数据安全纪律。判定线、受测条件、失效条件与未测边界都在文内；引用数字时必须连同构建、语料与范围一起引用。
- **[sync-compat-matrix.md](sync-compat-matrix.md)**：Google Reader / Fever 的协议 × 服务端兼容矩阵，以及冲突政策、增量游标和已知实现差异。用户可见对照表；政策的代码单点在 `src-tauri/src/sync/conflict_policy.rs`。
- **[backup.md](backup.md)**：迁移/镜像/手动快照、保留策略、备份安全边界与退出应用后的手动恢复步骤。
- **[release.md](release.md)**：发布序列、tag 附注结构、存量版本说明与提交信息约定。

## 现状与限制

- **[roadmap.md](roadmap.md)**：当前仍成立的缺口、已知限制与已评估不修的边界。它不是任务队列，也不展开修复方案。

## 历史材料（待决策）

- **[audit-2026-10-08.md](audit-2026-10-08.md)**：2026-10-08 的独立审计快照，按用户当时要求留在 `docs/`。它记录的是当时的发现与建议，不是当前修复验收。
- **[optimization/](optimization/)**：2026-10 优化重构的任务卡与逐卡证据。属于执行记录，不是长期事实；是否归档到 `archive/tooling-and-reports` 或删除待决策。

历史材料不删除、不移动，直到有明确决策。一次性探针、测量工具与更早的调研报告原文在存档分支 `archive/tooling-and-reports`；其中 `archive/reports/BASELINE.md` 的提交号对照表是解读 2026-09-16 历史重写前后旧提交号的唯一线索。

旧卡与 Note 的 Verification 保留各自验收时点的测试数量、构建和环境，不能拼接成当前工作树的一次全量通过。2026-10-31 的 Rust 不可链接探针仍是当时事实；当前本机隔离环境与 CI 分工以 [AGENTS.md](../AGENTS.md) 为准。

## 决定与取舍写在哪

`.agents/notes/` 记录「为什么这样做、当时放弃了什么」，一篇一个决定；`docs/` 只记录需要长期与代码同步维护的事实，`roadmap.md` 只记录还没做的事与已知限制。路径即分类：`{lifecycle}/{class}/yyyy-mm-dd-topic.md`，lifecycle 取 `proposed` / `implemented` / `rejected` / `archived`，class 取 `feature` / `bug-fix` / `simplification` / `architecture` / `process` / `testing`。

- 找架构与结构决策 → [`architecture/`](../.agents/notes/implemented/architecture/)
- 找「为什么放弃方案 B」 → 各篇的 `## Alternatives considered` 节
- 找测试与门禁口径 → [`testing/`](../.agents/notes/implemented/testing/)
- 找门禁、发布、协作流程 → [`process/`](../.agents/notes/implemented/process/)

关键实现点采用 `// Note: … 见 .agents/notes/…` 反向注释，从代码可以走回决定；流程类记录未必有单一代码入口，不把软锚点报告中的“无代码入口”直接当作失效决定。活跃 Note 的状态跟随路径，归档按工具契约保留 `Status: implemented` 并紧接 `Archived:` 日期、由 manifest 封印。文档约定见 [AGENTS.md](../AGENTS.md)。
