# 文档索引

三类地方，各有各的读者：

| 想找什么 | 去哪 |
| --- | --- |
| 某个决定**为什么**这样做、当时放弃了什么 | [`.agents/notes/`](../.agents/notes/) |
| 稳定的对照表、测量数据、发布流程 | `docs/`（就是这里） |
| 还没做的事、已知限制、明确不修的边界 | [roadmap.md](roadmap.md) |

`docs/` 与 `.agents/notes/` 的分工：Note 记录「为什么」，一篇一个决定，随代码同批更新；本目录放的是需要长期与代码同步维护的事实——协议矩阵、性能基线、发布流程。

## 本目录

| 文件 | 内容 |
| --- | --- |
| [sync-compat-matrix.md](sync-compat-matrix.md) | 同步协议 × 服务端兼容矩阵（Google Reader / Fever）。用户可见对照表；政策的代码单点在 `src-tauri/src/sync/conflict_policy.rs`，改政策要同时改三处 |
| [performance.md](performance.md) | 大库性能基线与测量方法。判定线、受测条件下的实测数字、测量纪律与数据安全纪律 |
| [release.md](release.md) | 发布流程、tag 附注结构、存量版本说明 |
| [roadmap.md](roadmap.md) | 未做事项与已知限制 |

## `.agents/notes/` 怎么读

路径即分类：`{lifecycle}/{class}/yyyy-mm-dd-topic.md`。lifecycle 是 `proposed` / `implemented` / `rejected` / `archived`，class 是 `feature` / `bug-fix` / `simplification` / `architecture` / `process` / `testing`。

- 找架构与结构决策 → [`architecture/`](../.agents/notes/implemented/architecture/)
- 找「为什么放弃方案 B」 → 各篇的 `## Alternatives considered` 节；被审慎否掉的提案在 `rejected/`（用到时才有这个目录）
- 找测试与门禁口径 → [`testing/`](../.agents/notes/implemented/testing/)
- 找门禁、发布、协作流程 → [`process/`](../.agents/notes/implemented/process/)

每篇 Note 的关键实现点在源码入口留了一行 `// Note: … 见 .agents/notes/…` 反向注释，从代码可以走回决定。

## 历史材料

一次性探针、测量工具与整理前的全部调研报告原文在存档分支 `archive/tooling-and-reports`（该分支只有这些文件，不含项目代码）。其中 `archive/reports/BASELINE.md` 的提交号对照表是解读 2026-09-16 历史重写前后旧提交号的唯一线索。
