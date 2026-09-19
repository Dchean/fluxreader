<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-066 · 阅读与翻译一致性三修复收口：锚定复位、卡片译文 HTML 渲染、流式译文消毒时序（REQ-102/N7/N8/N11）

**状态**：verified

**目标**：本任务收口被取消的 TASK-065 的同一成果（历史：TASK-065 实施了 AUDIT-20260919-v2.md N7/N8/N11 三项修复并通过四门禁，但独立审查 r1 发现门禁缺陷——(n8) NotifCard 源码断言因切片标记在 SocialCard 内先出现而恒失败、(n8)/(n11) 断言块误置于汇总计算之后永远无法影响退出码、if 行缩进误改；修复已全部落地（NotifCard 切片改在起始位置之后搜索终点、断言块移到汇总计算之前、缩进恢复，重验 295/295 且 M3 变异经退出码 1 检出），因工具按工作区实时候选校验审查绑定、r1 FAIL 报告无法回录，TASK-065 按流程取消，本任务以当前工作区（已含全部修复）走完整的 finish/verify/独立复审/验收闭环绑定候选）。三项修复内容：【N7】bootstrap.ts anchorToArticle 补齐与 selectArticle 同口径的三个复位（isShowingTranslatedProse/isRawRenderMode/showFulltext）——修前搜索/命令面板打开新文章残留旧标志，Reader 按译文模式渲染空译文导致正文整块空白。【N8】Timeline.tsx SocialCard/NotifCard 译文块 rawTranslated 分支渲染：未消毒流式产物按纯文本，消毒后与 Reader 同口径 dangerouslySetInnerHTML——修前消毒译文显示字面标签。【N11】新增 rawTranslatedIds 状态（types.ts + ai slice）：流启动置位、消毒回读成功覆盖后清除、回读失败丢弃未消毒半截 + translateErrors + toast、流错误路径保持（半截内容按纯文本渲染，重试语义不变）；toggleReaderTranslation 缓存命中补 raw 守卫；两处 onDone 的消毒回读补 .catch（修前无 catch：半截未消毒译文永久留在渲染路径 + unhandled rejection）。渲染契约：rawTranslatedIds[id] 存在 → 纯文本；不存在 → HTML（DB 加载的译文后端已消毒）。translatingIds/translating 维持既有时序，消毒时序由 rawTranslatedIds 承担。r1 审查发现低优先级缺口（done→回读窗口内的重入可在毫秒级窗口内让新流未消毒 delta 短暂走 HTML 路径且自恢复）已如实记录于日志，作为已知限制不在本任务处理。

**依赖**：TASK-064
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src/store/types.ts, src/store/slices/ai.ts, src/store/slices/bootstrap.ts, src/components/Reader.tsx, src/components/Timeline.tsx, tools/frontend-regression.mjs

## 验收标准

- diff 仅含 allowed_paths 内 6 文件；三项修复与 rawTranslatedIds 生命周期与声明一致；渲染契约四处分支统一（raw→纯文本、否则 dangerouslySetInnerHTML）
- 前端回归 295/295 通过（286 存量一行不动 + (n7)/(l2)/(n8) 新断言计入门禁）；四门禁全绿：cargo test 171/0/9 不回退、lint 0/0、build exit 0
- 变异可检出：(n8) 卡片还原纯文本插值 → 门禁退出码非 0（TASK-065 教训闭环，已验证）
- 不引入新依赖；package.json/Cargo.toml/Cargo.lock 零改动；文本文件 LF 行尾；台账改动须在 begin 之前完成；用户真实数据库不得写入

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；2026-09-19 基线（TASK-064 终版候选验证 RUN-177672cf，提交 6e36fdb）：cargo test 171 passed / 0 failed / 9 ignored、npm run lint exit 0（0 warnings/0 errors）、npm run build exit 0、npm run test:frontend exit 0 且 286/286。本任务 behavior=change 的范围如实声明：① anchorToArticle 复位三个阅读视图标志；② 卡片译文从纯文本插值改为 HTML 渲染（消毒后内容与 Reader 统一口径）；③ 流式期间与回读失败时译文按纯文本渲染（未消毒内容不进 HTML 渲染路径）；④ 新增 rawTranslatedIds 状态键（纯标记，无持久化）；⑤ 前端回归新增 (n7)/(l2)/(n8) 断言块且计入门禁（TASK-065 教训：断言必须在汇总计算之前）。既有 286 条存量断言一行不动。
- 基线证据：.workflow-kit/tasks/runs/RUN-177672cf4733428eb607b1645ca11e9d.json
- 需求决定：DEC-992f64fd15714ca2a614fe66d5a144ec
- 补充：(n7) 锚定复位断言、(l2) rawTranslatedIds 生命周期三段断言（流中置位/回读落地清除/回读失败丢弃+toast/流错误保持）、(n8) 卡片与 Reader 渲染分支源码形态断言（按块切片，NotifCard 终点标记在起点之后搜索）——全部位于汇总计算之前，可影响门禁退出码；TASK-065 r1 审查的门禁缺陷已修复；断言计入门禁后变异可检出；验证：frontend
- 保留：其余全部既有断言（286 条存量）逐字不动；除新增断言块外契约不变；验证：cargo_test, lint, build, frontend

## 执行与恢复

- 首次开始：2026-09-19T22:44:42.633190Z
- 原截止时间：2026-09-20T02:44:42.633190Z
- 当前截止时间：2026-09-20T02:44:42.633190Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 0 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-19T22:44:42.798828Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-19T22:45:21.441565Z：Worker changed_files does not match the observed diff; declared but unchanged: src/components/Reader.tsx, src/components/Timeline.tsx, src/store/slices/ai.ts, src/store/slices/bootstrap.ts, src/store/types.ts, tools/frontend-regression.mjs; declare either the task's cumulative changes [] or this run's changes []；下一步：按报错列出的漏报/多报文件修正 worker-result，再 unblock 后 begin；不要新建任务或重置预算
- 2026-09-19T22:45:58.052516Z：阻塞已处置（protocol）：核对 RUN-e0b922917a024c5fbf281030e90f14a2 的 diff 回执：观察改动为空（begin 快照已含 TASK-065 全部成果与 r1 整改），worker-result 已改为零改动声明并如实说明成果来源；下一步：begin 重新实现
- 2026-09-19T22:46:01.946146Z：开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-19T22:46:13.183978Z：编码结果已记录，差异范围已核对：无文件变化；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-19T22:46:33.723845Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-19T22:51:39.466370Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-066.json)

- [RUN-e0b922917a024c5fbf281030e90f14a2](../runs/RUN-e0b922917a024c5fbf281030e90f14a2.json)
- [RUN-f0223f527be84ed68d12ed6bf44cea2d](../runs/RUN-f0223f527be84ed68d12ed6bf44cea2d.json)
- [RUN-e7b42163aaa14d45b754261992f5214d](../runs/RUN-e7b42163aaa14d45b754261992f5214d.json)
- [RUN-5b0f409b5aca4afc8448e86c04432531](../runs/RUN-5b0f409b5aca4afc8448e86c04432531.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
