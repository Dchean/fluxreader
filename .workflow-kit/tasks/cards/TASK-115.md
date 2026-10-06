<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-115 · 返回位置统一规则——切换返回滚动恢复与阅读器焦点归还（三阶段②，REQ-005）

**状态**：done

**目标**：返回位置统一规则（审计三阶段完成标准之一；探查事实：切布局/视图/范围/排序均 filterKey 归零丢位置 Timeline.tsx:94-101，缓存恢复只恢复 entries 不恢复滚动；阅读器关闭 clearReaderSelection 只清字段无焦点归还 reader.ts:302-303+Timeline.tsx:223-232 提前返回；TASK-111 后台刷新保位是唯一回位场景）。修法要求：①切换返回恢复——Timeline 已节流记录顶条锚（recordTopAnchor :275-277 一带，含 filterKey）；把「离开时的锚 id」按 filterKey 存档（模块级 Map 或 store 自选，注意容量上限与清理时机——复用 TASK-111 预算纪律），在 selectLayout/selectView/selectFeed 的缓存恢复路径（entries 恢复后）按存档锚恢复滚动（锚 id 在恢复列表中→scrollToIndex align:start；不在→归零回落）；后台 reload 落地后由既有 activeArticleId/锚机制校准，不双重滚动（与 TASK-111 positionRestoreNonce 的协同要显式：切换返回的恢复先于/独立于刷新保位，两者不叠加）。②阅读器焦点归还——clearReaderSelection 前记录原 activeArticleId 的卡片 index（或由 Timeline 消费一个关闭信号），关闭后 moveCardFocus 到原卡片（原卡片不在可见范围→scrollToIndex 定位；列表已切换→归零回落）。③统一规则文档化：UI 契约文档三检查点 + 代码单点注释（timelineAnchor 或新模块头注）列明全场景规则。画廊布局非虚拟化：切换返回恢复与阅读器焦点归还均回落现状（与 TASK-111 同口径注明）。回归断言 t115-* ≥6 条：存档-恢复（切走再切回滚到原锚）、锚丢失归零回落、切排序重拉后不恢复（filterKey 已变，重拉是新语境——写明该裁定）、阅读器关闭焦点归还原卡、关闭后列表已切回落、与 TASK-111 刷新保位不叠加。既有断言零弱化（t111-* 20 条必须原样通过）。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：.workflow-kit/docs/UI-CONTRACT-TASK-115-RETURN-POSITION.md
**界面检查**：X1.switch-return-scroll, X2.reader-close-focus, X3.rule-documented
**修改范围**：src, tools/frontend-regression.mjs

## 验收标准

- ① 切换返回滚动恢复生效（X1：缓存命中路径立即恢复；锚丢失归零回落）
- ② 阅读器关闭焦点归还原卡（X2）
- ③ 统一规则文档化（X3：契约文档+代码单点注释；切排序=新语境不恢复的裁定写明）
- ④ 与 TASK-111 刷新保位机制不叠加（行为断言）
- ⑤ 门禁全绿：frontend（t115-* 新增、t111-* 原样）、lint/build/cargo_fmt 不回退；cargo test/clippy 由 CI 承担（DEC-local-cargo-gate-20261005）
- ⑥ 独立审查（全新子代理）PASS findings=0；ui_change=true 需 UI 取证说明

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；dev@TASK-114 后基线：frontend ≥611 全绿、CI 全绿。本卡新增返回位置恢复（行为增强），属行为变化（owner 指示按审计三阶段推进）。
- 基线证据：.workflow-kit/docs/AUDIT-20261005-core-consistency.md
- 需求决定：DEC-refactor-roadmap-20261005
- 补充：t115-* 断言（存档恢复/回落/焦点归还/不叠加）；新机制需防回退；验证：frontend
- 保留：既有 frontend（t103-t114 全部）/cargo fmt/lint/build 断言；不回退证据；cargo 由 CI 承担（DEC-local-cargo-gate-20261005）；验证：cargo_fmt, lint, build, frontend

## 执行与恢复

- 首次开始：2026-10-06T12:02:27.245265Z
- 原截止时间：2026-10-06T16:02:27.245265Z
- 当前截止时间：2026-10-06T16:02:27.245265Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 34 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-10-06T12:33:35.418391Z：编码结果已记录，差异范围已核对：src/components/Timeline.tsx, src/components/timelineAnchor.ts, src/store/slices/nav.ts, src/store/slices/reader.ts, src/store/types.ts, tools/frontend-regression.mjs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-10-06T12:34:13.016486Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-10-06T13:04:19.691224Z：Review requires changes; inspect the findings；下一步：先核对已有文件及原始日志，再处理 review_failure；不要新建任务或重置预算
- 2026-10-06T13:04:28.448322Z：阻塞已处置（review_failure）：R0 审查 FAIL（1 低危）：types.ts/reader.ts 两处注释引用不存在的 noteReaderFocusReturn 机制（失实注释，X3 载体即注释，判 FAIL 合理）。R1：注释改为与实现同口径（Timeline ref 记账 + readerCloseNonce 信号），重新 verify+复审。时序/不叠加/变异判别全部核实成立。；下一步：begin 重新实现
- 2026-10-06T13:05:28.080876Z：开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-10-06T13:08:20.397867Z：编码结果已记录，差异范围已核对：src/components/Timeline.tsx, src/components/timelineAnchor.ts, src/store/slices/nav.ts, src/store/slices/reader.ts, src/store/types.ts, tools/frontend-regression.mjs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-10-06T13:09:02.468426Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-10-06T13:34:29.978477Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-115.json)

- [RUN-b9405b007154440d97e517faa3936153](../runs/RUN-b9405b007154440d97e517faa3936153.json)
- [RUN-afa3352925cc4920b8bc881a19267c12](../runs/RUN-afa3352925cc4920b8bc881a19267c12.json)
- [RUN-747beb5a61a14ebdbca56858dcd286a9](../runs/RUN-747beb5a61a14ebdbca56858dcd286a9.json)
- [RUN-4923ef0af3394d1d9b430d7a4cd3f79c](../runs/RUN-4923ef0af3394d1d9b430d7a4cd3f79c.json)
- [RUN-d9858a974d1c40ef92cd213b6b95dbfd](../runs/RUN-d9858a974d1c40ef92cd213b6b95dbfd.json)
- [RUN-ae9b2be69a2d417f94169ffb9c9d2b1a](../runs/RUN-ae9b2be69a2d417f94169ffb9c9d2b1a.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
