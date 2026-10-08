<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-122 · 正文/AI 实体缓存分离与显式失效——清理缓存不再复活旧产物（审计 P2-3）

**状态**：done

**目标**：落地审计目标结构的 bodyById 块（tmp/audit-20261007/REVIEW.md 第 3 节 [P2] + 架构目标节，探针 P3/P4 场景）。根因：①清理 AI 缓存真实路径把 DB ai_summary/translated_content 置 NULL 再 reload，列表行空摘要被转 ''，mergeSnapshotEntries 的 `a.aiSummary || prev.aiSummary` 把旧值复活（UI 与 DB 分离；探针实测 old summary/old translation）；②snippet 更新后正文仍陈旧（entryNeedsHydration=false 不重取）；③'' 同时承担「无」与「已清空」两种语义。设计要求（Zustand 内实现，禁第二套框架）：①新增独立正文实体缓存（模块级 bodyById：Map<id, BodyEntry>，BodyEntry={content,rawContent,translatedContent,aiSummary,fulltextExtracted,contentRevision,state}，state 为显式判别态：loading|ready|cleared|missing|failed——收敛 TASK-103 的分散态）；②显式失效：设置页清理 AI 缓存（CacheCleanupSection 调用链）后对受影响 id bump contentRevision 并置 cleared 态（不再依赖 merge 继承）；③**merge 不再继承正文/AI/译文重字段**（快照合并退化为视图行字段；isRead/isStarred 继承与版本守卫语义不变——TASK-107/118 机制保持）；④卡片/阅读器从 bodyById 读正文（选择器单点），懒水合判定改从 bodyById 状态判定（无记录→未请求；loading 在途；cleared→「已清空」呈现；missing→「文章不存在」；failed→重试）；⑤正文内存预算（LRU 或条数上限，TASK-111 纪律）与淘汰策略（淘汰后回未请求态可重取）；⑥视图行保留的 snippet/url 等轻字段照旧（快照合并保留这些）。⑦兼容硬约束：t103/t104/t108/t109/t110/t111/t117/t118 既有断言语义不回退（快照继承相关断言按新架构更新，逐条附理由——本卡是审计点名「文章实体与视图分离尚未完成」的核心落地，允许大改但保护意图必须保留：刷新不丢已加载正文（从 bodyById 来）、水合死区不复现、清理失效语义正确）；Rust 零改动。⑧回归 t122-* ≥6 条：清理 AI 缓存→UI 空非旧值（探针 P3 本体）；snippet 更新→正文重取（探针 P4）；'' vs cleared 语义区分；正文预算淘汰→可重取；快照替换后 bodyById 稳定（正文不丢）；水合死区回归（审计探针场景）仍被锁。代码注释：bodyById 的状态机与失效规则单点文档化。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src, tools/frontend-regression.mjs

## 验收标准

- ① 清理 AI 缓存→UI 显示空/已清空而非旧值（审计探针 P3 本体转回归，t122-*）
- ② snippet 更新→正文重取（探针 P4 本体）；正文/AI/译文 ''与 cleared 语义区分
- ③ bodyById 状态机显式化（loading/ready/cleared/missing/failed 判别联合，无字符串态共用）
- ④ 正文内存预算与淘汰→可重取；快照替换不丢已加载正文（从 bodyById）
- ⑤ 水合死区回归仍锁定；t103-t118 既有断言零弱化（架构更新逐条附理由）
- ⑥ 门禁全绿：frontend（t122-* 新增）、lint/build/cargo_fmt 不回退；cargo 由 CI 承担（DEC-local-cargo-gate-20261005）
- ⑦ 独立审查（全新子代理）PASS findings=0

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；dev@main 合并后（P1 批次 117-121 全部验收）：frontend 701/701、main@b497288 CI 全绿。本卡落地审计架构目标 bodyById 块，属高风险架构变更（owner 指示按审计收口顺序推进）。
- 基线证据：.workflow-kit/docs/DEC-gate-adjust-20261007.md
- 需求决定：DEC-gate-adjust-20261007, DEC-refactor-roadmap-20261005
- 补充：t122-* 回归（清理失效/重取/语义区分/预算淘汰/死区回归）；架构变更需成对断言（DEC-gate-adjust ①）；验证：frontend
- 适配：快照正文继承相关的 t103 既有断言族；继承机制被 bodyById 取代，保护意图（刷新不丢正文/死区不复发）必须保留并转由新机制断言承载，逐条附理由；验证：frontend
- 保留：其余既有 frontend/cargo fmt/lint/build 断言；不回退证据；验证：cargo_fmt, lint, build, frontend

## 执行与恢复

- 首次开始：2026-10-07T09:12:23.217220Z
- 原截止时间：2026-10-07T13:12:23.217220Z
- 当前截止时间：2026-10-08T00:59:47.739600Z
- 时钟：按墙钟计：额度 300 分钟，写入阶段已用约 109 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-10-07T23:02:56.393716Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-10-07T23:59:02.326434Z：Review requires changes; inspect the findings；下一步：先核对已有文件及原始日志，再处理 review_failure；不要新建任务或重置预算
- 2026-10-08T00:01:04.461597Z：依据新决定追加预算；原始时钟与失败记录保留；下一步：先核对已有成果，再按原任务范围继续
- 2026-10-08T00:01:13.511447Z：阻塞已处置（review_failure）：P0：selectArticleBody 每调用经 bodyViewFrom 返回新对象，Reader/SocialCard/NotificationCard 三处未包 useShallow → useSyncExternalStore 无限重渲染 #185。修复 = 三处订阅包 useShallow（一行级）。；下一步：begin 重新实现
- 2026-10-08T00:02:33.291896Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-10-08T00:09:25.865974Z：编码结果已记录，差异范围已核对：src/components/Reader.tsx, src/components/Timeline.tsx, tools/frontend-regression.mjs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-10-08T00:10:11.200178Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-10-08T00:20:25.470135Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-122.json)

- [RUN-5e9729be46194e2c9a535b4c75a4794d](../runs/RUN-5e9729be46194e2c9a535b4c75a4794d.json)
- [RUN-184370835e824075b7093110edb90c7f](../runs/RUN-184370835e824075b7093110edb90c7f.json)
- [RUN-0512d0a3a3fe422296d4f89f097661d3](../runs/RUN-0512d0a3a3fe422296d4f89f097661d3.json)
- [RUN-4e9d315ed3dc4cbe86024e9d6fc6e2b4](../runs/RUN-4e9d315ed3dc4cbe86024e9d6fc6e2b4.json)
- [RUN-788a9af874e14b358c82a9dfc8f62123](../runs/RUN-788a9af874e14b358c82a9dfc8f62123.json)
- [RUN-5834c5a91f0a4bdbbd6af4801f864c98](../runs/RUN-5834c5a91f0a4bdbbd6af4801f864c98.json)
- [RUN-e4a1095b1ef244739fe319c2128dbcf4](../runs/RUN-e4a1095b1ef244739fe319c2128dbcf4.json)
- [RUN-4f92ab4690554440a77baf5412ffdaa5](../runs/RUN-4f92ab4690554440a77baf5412ffdaa5.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
