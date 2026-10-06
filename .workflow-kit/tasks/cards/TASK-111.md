<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-111 · 查询缓存实体预算 + 后台刷新保位（二阶段③，完成标准「快速切换不串数据」的容量与位置面）

**状态**：verified

**目标**：①缓存实体预算（审计：「单纯限制为 8 个视图并不能限制每个视图的大小」）：viewEntriesCache 现为 LRUMap 上限 8 键（internals.ts:50-80，TASK-100 P3-7 引入），单键内 entries 数组无上界——TASK-110 分页化后单键仍随加载增长。修法：增加单键实体预算（如 VIEW_ENTRIES_CACHE_ENTRY_BUDGET=1000 常量，注释载明审计依据），超限从尾部截断（缓存语义=首屏快照，尾部截断安全；命中恢复后 articlesExhausted/续拉衔接必须正确——截断处数 ≤ 已加载总数，恢复后续拉从游标继续，不得因截断丢 exhausted 判据）；syncCurrentViewCache（internals.ts:226-229）写入时执行同一预算。entryMutationVersion Map 会话内增长维持现状（注释已说明，不纳入本卡）。②后台刷新保位（审计：「后台刷新保留当前阅读位置，避免靠整体替换列表刷新所有内容」）：现况探查——feeds-updated 事件（scheduler.rs:176-179/244-248 → App.tsx:86-98）触发 reloadFromBackend 整体替换 entries；filterKey（Timeline.tsx:93-100）不含 entries 故滚动不归零，但 newest_first 下新文章插入头部会使索引后移，虚拟列表视觉跳动；activeArticleId 定位 effect 仅在变化时触发（Timeline.tsx:218-227），目标不在新快照时跳过（:221），无锚定恢复。修法：Timeline 在滚动时以节流方式记录「顶条锚」（可见首条目 id，存 store 或模块级状态），reloadFromBackend 完成后若锚 id 仍在（或其 feed 内同文/同 id 存活）新快照中，滚动到其新索引（程序性滚动抑制复用 Timeline.tsx:222-227 既有机制），锚 id 不在时维持现状回落（不强制顶部）；阅读中文章（activeArticleId）保持既有不变语义。③回归断言 t111-* ≥5 条：预算截断触发与恢复正确性、截断后续拉衔接（exhausted 不误判）、后台刷新后顶条锚定（新文章插入场景）、锚丢失回落、锚不影响用户主动滚动。既有断言零弱化。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src, tools/frontend-regression.mjs

## 验收标准

- ① 单键实体预算生效：超限尾部截断，恢复后续拉衔接正确、exhausted 不误判（t111-*）
- ② 后台刷新保位：新文章插入头部时顶条锚定回滚视觉跳动，锚丢失回落现状（t111-* 判别断言）
- ③ 主动滚动/键盘导航行为不受锚机制干扰（t111-*）
- ④ 门禁全绿：frontend 全过、lint/build exit 0、cargo fmt 不回退；cargo test/clippy 由 CI 承担（DEC-local-cargo-gate-20261005）
- ⑤ 独立审查（全新子代理，未参与编码）PASS findings=0

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；dev@二阶段②后基线：frontend 全绿、CI 全绿。本卡为缓存容量治理与刷新位置保持，用户可见行为改善（减少跳动）。
- 基线证据：.workflow-kit/docs/AUDIT-20261005-core-consistency.md
- 需求决定：DEC-refactor-roadmap-20261005
- 补充：t111-* 断言（预算/截断/保位/回落）；新机制需防回退；验证：frontend
- 保留：既有 frontend/cargo fmt/lint/build 断言；不回退证据；cargo test/clippy 由 CI 承担（DEC-local-cargo-gate-20261005）；验证：cargo_fmt, lint, build, frontend

## 执行与恢复

- 首次开始：2026-10-06T01:41:31.068111Z
- 原截止时间：2026-10-06T05:41:31.068111Z
- 当前截止时间：2026-10-06T05:41:31.068111Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 41 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-10-06T02:13:39.164488Z：编码结果已记录，差异范围已核对：src/App.tsx, src/components/Timeline.tsx, src/components/timelineAnchor.ts, src/store/internals.ts, src/store/slices/bootstrap.ts, src/store/slices/feeds.ts, src/store/slices/nav.ts, src/store/slices/sync.ts, src/store/types.ts, tools/frontend-regression.mjs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-10-06T02:14:09.530798Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-10-06T02:36:16.323296Z：Review requires changes; inspect the findings；下一步：先核对已有文件及原始日志，再处理 review_failure；不要新建任务或重置预算
- 2026-10-06T02:36:22.484326Z：阻塞已处置（review_failure）：审查 FAIL：产品代码本体核实正确，2 条 Medium findings 均为回归网判别力缺陷（t111-4 selectView 不 bump 断言空洞=自比较恒真；主动切范围回落断言因列表为空而以错误原因通过，filterKey 失配分支无判别力）。进修复轮 R1：按审查建议修两处断言并变异自证，产品代码不动。；下一步：begin 重新实现
- 2026-10-06T02:37:04.194914Z：开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-10-06T02:46:19.460019Z：编码结果已记录，差异范围已核对：src/App.tsx, src/components/Timeline.tsx, src/components/timelineAnchor.ts, src/store/internals.ts, src/store/slices/bootstrap.ts, src/store/slices/feeds.ts, src/store/slices/nav.ts, src/store/slices/sync.ts, src/store/types.ts, tools/frontend-regression.mjs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-10-06T02:46:50.108881Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-10-06T03:01:12.114518Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-111.json)

- [RUN-9bffe3094ee847199572940d82d4d7ec](../runs/RUN-9bffe3094ee847199572940d82d4d7ec.json)
- [RUN-6d3ff49f2ad846a88b60694528e916c4](../runs/RUN-6d3ff49f2ad846a88b60694528e916c4.json)
- [RUN-a4cc2cc6b9c547938cc4eadb84de6c09](../runs/RUN-a4cc2cc6b9c547938cc4eadb84de6c09.json)
- [RUN-f91149c0a86e43b8bd181f72bfea4c56](../runs/RUN-f91149c0a86e43b8bd181f72bfea4c56.json)
- [RUN-cd3f59f29ef145d582a841f4bd13e409](../runs/RUN-cd3f59f29ef145d582a841f4bd13e409.json)
- [RUN-446697ec40e7480684d04446ddef2e3b](../runs/RUN-446697ec40e7480684d04446ddef2e3b.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
