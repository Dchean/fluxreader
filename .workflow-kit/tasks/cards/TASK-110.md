<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-110 · 筛选视图真分页——废除 limit:100000 近似全集（二阶段②，审计块②/完成标准「旧文章可达」）

**状态**：ready

**目标**：废除筛选视图（收藏/未读/今天）的唯一近似全集路径 reloadFilteredEntries（bootstrap.ts:266-306）的 limit:100000，改为真分页，使旧文章在筛选视图可达（审计二阶段完成标准）。现况事实（探查在案）：reloadFilteredEntries 是全库唯一超大 limit 调用点（bootstrap.ts:271-279），articlesExhausted 恒 true（:302），切排序只本地重排全集（nav.ts:200-201）；Rust 端 list_articles 本就支持 limit/offset（commands/articles.rs:54-67 limit unwrap_or(500) 无上限；db/articles.rs:182-188 ORDER/LIMIT/OFFSET）；ARTICLES_PAGE_SIZE=500（bootstrap.ts:34）；假后端 queryRows 已支持分页与筛选（tools/frontend-regression.mjs:365-376）。修法要求：①筛选视图分页拉取：首批 PAGE_SIZE，articlesCursor/scopePageKey 口径复用（游标键已含 layout，TASK-094 R7），加载更多在筛选视图可用（复用或扩展 loadMoreArticles，含其三重竞态守卫的等价物；sentinel 三态 timelineSentinel 与自动续拉 timelineRefill 在筛选视图生效）；articlesExhausted 改为真实判定（next.length < PAGE_SIZE）。②稳定序与偏移：追加按 id 去重（同步插入使 offset 漂移时不得重复入列）；新条目插入已加载页中间的处理策略二选一并写明（追加去重保序 vs 触发重拉），选定的策略要有判别断言。③切排序对齐 all：筛选视图切排序改为重拉当前范围（放弃「全集在内存本地重排」的旧语义；已加载条目的水合正文由 merge 保留——TASK-106 机制），toggleTimelineSort（nav.ts:184-203）相应收口。④计数口径不动（feedCounts 权威对账为 TASK-107 已建立语义）；大库下不再一次拉全集，内存占用随页数线性可控。⑤回归断言 t110-* ≥6 条：分页首屏、加载更多、追加去重（偏移漂移场景）、exhausted 判定、切排序重拉（水合保留）、筛选过滤参数（only_unread/only_starred/only_today）逐一如旧。既有断言零弱化——注意既有断言若依赖「筛选视图拉全集」行为（如 articlesExhausted 恒 true、切排序不重拉），需按行为变化更新并附理由。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src, tools/frontend-regression.mjs

## 验收标准

- ① 筛选视图分页：首屏 PAGE_SIZE，加载更多可用，exhausted 真实判定，sentinel/refill 协同（t110-*）
- ② 追加按 id 去重：同步插入导致 offset 漂移时不出现重复条目（判别断言）
- ③ 切排序重拉与 all 一致，已加载水合正文保留（t110-*）
- ④ 三种筛选参数行为与修前一致（t110-* 逐项锁定）
- ⑤ 门禁全绿：frontend 全过、lint/build exit 0、cargo fmt 不回退；cargo test/clippy 由 CI 承担（DEC-local-cargo-gate-20261005）
- ⑥ 独立审查（全新子代理，未参与编码）PASS findings=0

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；dev@二阶段①后基线：frontend ≥561 全绿、CI rust job 全绿。本卡改变筛选视图取数行为（全集→分页），属审计确认的行为变化（owner 指示按审计推进）。
- 基线证据：.workflow-kit/docs/AUDIT-20261005-core-consistency.md
- 需求决定：DEC-refactor-roadmap-20261005
- 补充：t110-* 断言（分页/去重/exhausted/切排序/过滤参数）；行为变化需成对断言；验证：frontend
- 适配：依赖「筛选视图拉全集」旧行为的既有断言（articlesExhausted 恒真、切排序仅本地重排等）；旧行为正是本卡废除的缺陷手法，更新需附理由且不得弱化有效保护；验证：frontend
- 保留：其余既有 frontend/cargo fmt/lint/build 断言；不回退证据；cargo test/clippy 由 CI 承担（DEC-local-cargo-gate-20261005）；验证：cargo_fmt, lint, build, frontend

## 执行与恢复

- 首次开始：None
- 原截止时间：None
- 当前截止时间：None
- 时钟：未开始
- 已用修复轮：0
- 阻塞：无
- 下一步：执行 start/next 获取可继续的动作

## 最近检查点


## 原始证据

[唯一状态记录](../items/TASK-110.json)


卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
