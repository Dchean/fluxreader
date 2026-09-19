<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-065 · 阅读与翻译一致性三修复：锚定复位阅读视图标志、卡片译文按 HTML 渲染、流式译文消毒时序（REQ-102/N7/N8/N11）

**状态**：cancelled

**目标**：AUDIT-20260919-v2.md 的 N7/N8/N11（REQ-102，阅读与翻译路径的一致性与安全修复）：

【N7 · anchorToArticle 不复位阅读视图标志】src/store/slices/bootstrap.ts 的 anchorToArticle 只写 activeArticleId/openedReadIds，对照 selectArticle（reader.ts）缺 isShowingTranslatedProse/isRawRenderMode/showFulltext 三个复位——文章 A 开着「译文模式/全文视图」时经搜索或命令面板打开文章 B，Reader 按旧标志渲染 B（译文通常为空 → 正文整块空白）。修复：anchorToArticle 的 set 补齐三个复位（与 selectArticle 同口径）。

【N8 · 卡片译文按纯文本渲染 HTML】src/components/Timeline.tsx 的 SocialCard（约 452 行）与 NotifCard（约 739 行）译文块用 {item.translatedContent} 纯文本插值，而后端契约与 Reader 路径均按 HTML 处理（commands/ai.rs 落库前 ammonia 消毒、Reader dangerouslySetInnerHTML）——同一译文两种口径，卡片显示 <p>…</p> 字面标签基本不可读。修复：与 Reader 同口径按 HTML 渲染。

【N11 · 流式译文未消毒进 DOM + 回读失败常驻】译文流式期间 delta 是模型原始输出（未消毒），Reader/卡片直接渲染进 DOM；流结束的消毒回读（api.getArticle 取 DB 消毒版覆盖）无 .catch——回读失败时半截未消毒译文永久留在渲染路径 + unhandled rejection。修复引入 rawTranslatedIds 标记（store 新状态，语义=「该 id 的 translatedContent 当前是未消毒的流式产物」）：流启动时置位；消毒回读成功替换内容后清除；回读失败清除 translatedContent（丢弃未消毒半截）+ toast + 保持清除；流错误路径不清（半截未消毒内容保留供重试语义，按纯文本渲染）。渲染契约：rawTranslatedIds[id] 存在 → 译文按纯文本渲染（转义，模型输出的 HTML 标签以字面呈现）；不存在 → 按 HTML 渲染（Reader 与卡片四处分支统一）。DB 加载的译文（articleRowToEntry / getArticle 直读）后端落库前已消毒，无标记 → 正常 HTML 渲染。onDone 的回读补 .catch（toast「译文回读失败」+ 丢弃未消毒半截）。翻译中提示（翻译中…）随在途标记自然延长到回读完成，属如实状态呈现。

**依赖**：TASK-064
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src/store/types.ts, src/store/slices/ai.ts, src/store/slices/bootstrap.ts, src/components/Reader.tsx, src/components/Timeline.tsx, tools/frontend-regression.mjs

## 验收标准

- diff 仅含 allowed_paths 内 6 文件：bootstrap.ts 补三个复位；ai.ts 新增 rawTranslatedIds 状态并按生命周期维护（流启动置位、回读成功清除、回读失败清内容+toast+清除、流错误保持）；types.ts AppState 扩展该键；Reader/Timeline 四处译文渲染分支（raw→纯文本、否则 dangerouslySetInnerHTML）
- N7/N11 行为断言 + N8 渲染分支静态断言全部通过；变异取证成对：① 还原 anchorToArticle（删除三复位行）→ N7 断言失败；② 删除 rawTranslatedIds 维护（回读后不清除）→ N11 生命周期断言失败；③ Timeline 译文块还原纯文本插值 → N8 静态断言失败；还原后全过
- 既有 286 条断言一行不动且全部通过（S-4 翻译流时序在 onDone 重构后保持）
- 四门禁全绿：cargo test 171/0/9 不回退、lint 0/0、build exit 0、frontend 通过数 ≥286 且存量不回退
- 不引入新依赖；package.json/Cargo.toml/Cargo.lock 零改动
- 文本文件 LF 行尾；台账改动须在 begin 之前完成
- 用户真实数据库不得写入（沿用项目约束）

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；2026-09-19 基线（TASK-064 终版候选验证 RUN-177672cf，提交 6e36fdb）：cargo test 171 passed / 0 failed / 9 ignored、npm run lint exit 0（0 warnings/0 errors）、npm run build exit 0、npm run test:frontend exit 0 且 286/286。本任务 behavior=change 的范围如实声明：① anchorToArticle 复位三个阅读视图标志（搜索/锚定路径与列表点开路径对齐，缺陷修复）；② 卡片译文从纯文本插值改为 HTML 渲染（消毒后内容渲染口径与 Reader 统一，修前显示字面标签）；③ 流式期间与回读失败时译文按纯文本渲染（新安全契约：未消毒内容不进 HTML 渲染路径）；④ 新增 rawTranslatedIds 状态键（store 形状扩展，纯标记无持久化）。既有 286 条断言预期全部保持：S-4 翻译流测试的时序在 onDone 重构后仍成立（回读替换语义不变，仅补充标记维护与 catch）。
- 基线证据：.workflow-kit/tasks/runs/RUN-177672cf4733428eb607b1645ca11e9d.json
- 需求决定：DEC-992f64fd15714ca2a614fe66d5a144ec
- 补充：N7：anchorToArticle 复位断言——预置 isShowingTranslatedProse/showFulltext/isRawRenderMode 为 true，锚定打开另一篇文章后三者必须为 false（复用 harness 既有锚定测试的 article_index/list_articles mock 模式）；搜索/锚定路径的复位此前零覆盖；验证：frontend
- 补充：N11：rawTranslatedIds 生命周期断言——translateEntry 流式期间标记为真；消毒回读成功后标记清除且内容为 DB 消毒版；回读被拒时 toast 出现、未消毒半截被丢弃、标记清除；流错误路径标记保持（半截内容按纯文本渲染语义）；消毒时序契约此前零覆盖；S-4 既有流式 mock（heldAi/aiTr）提供确定性驱动；验证：frontend
- 补充：N8+Reader 渲染分支静态断言（沿用 harness 既有 CSS/源码正则烟测风格）：Timeline.tsx 的 social-translated-block 与 notif-translated-block、Reader.tsx 的译文渲染必须包含 rawTranslatedIds 分支与 dangerouslySetInnerHTML（修前为纯文本插值 → 断言失败）；组件渲染分支无 DOM harness，源码形态断言是既有测试风格的等价替代；配合 N11 行为断言覆盖契约；验证：frontend
- 保留：其余全部既有断言（含 S-4 翻译流、(s) 系列、286 条存量）逐字不动；除新增断言块外契约不变；验证：cargo_test, lint, build, frontend

## 执行与恢复

- 首次开始：2026-09-19T21:55:06.590959Z
- 原截止时间：2026-09-20T01:55:06.590959Z
- 当前截止时间：2026-09-20T01:55:06.590959Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 18 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：如需同一目标，准备新的任务并引用本任务作为历史

## 最近检查点

- 2026-09-19T21:55:06.749019Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-19T22:13:44.774440Z：编码结果已记录，差异范围已核对：src/components/Reader.tsx, src/components/Timeline.tsx, src/store/slices/ai.ts, src/store/slices/bootstrap.ts, src/store/types.ts, tools/frontend-regression.mjs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-19T22:14:05.477894Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-19T22:43:47.812167Z：任务已取消：review_failure 的修复已落地但审查绑定无法回录（工作区已前移、冻结候选重建未果）；取消任务身份，成果并入新任务收口，历史与证据完整保留；下一步：如需同一目标，准备新的任务并引用本任务作为历史

## 原始证据

[唯一状态记录](../items/TASK-065.json)

- [RUN-398597123b1443bc864d67a037145a02](../runs/RUN-398597123b1443bc864d67a037145a02.json)
- [RUN-5f81dedb4b814346bdb906b9e9cbe421](../runs/RUN-5f81dedb4b814346bdb906b9e9cbe421.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
