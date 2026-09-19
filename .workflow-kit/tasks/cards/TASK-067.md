<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-067 · 交互落库与错误可见性收尾：列宽拖拽松手持久化 + 异步失败可见（REQ-102/N9/N10）

**状态**：verified

**目标**：AUDIT-20260919-v2.md 的 N9/N10（REQ-102，REQ-102 前端最后两项）：

【N9 · 列宽拖拽每像素落库】src/App.tsx 的 startDragListWidth 在 onMove（每像素）调用 updateSettings（整包 JSON.stringify + set_setting IPC + 全 App 重渲染），与注释「松手持久化」相反——一次拖动几十上百次 IPC、SQLite 写放大、拖动手感发涩。修复：拖动期间只更新本地拖拽态（useState 的 dragWidth，渲染用 effectiveListWidth = dragWidth ?? settings.listWidth），mouseup 时一次性 updateSettings 落库并清除拖拽态。

【N10 · 一批 async action 无 catch：IPC 失败 → unhandled rejection + 本地与后端静默脱节】逐点修复（口径与既有 loadMoreArticles/refreshOneFeed/updateCatLayout 的「失败 toast」一致）：
① bootstrap.ts reloadFromBackend：Promise.all 无 catch——后台刷新事件/范围切换路径失败完全不可见。修复：内部 try/catch → toast「刷新失败：…」后 rethrow（bootstrapFromBackend 的 bootstrapError 路径保持）；
② bootstrap.ts 的 api.syncStatus().then（顺带刷新连接态）补 .catch(() => {})（纯提示性刷新，失败无需打扰）；
③ bootstrap.ts reloadFilteredEntries：try/catch → toast「筛选列表加载失败：…」；
④ bootstrap.ts anchorToArticle：try/catch → toast「打开文章失败：…」；
⑤ nav.ts markCurrentViewAllRead 的 void api.markAllRead：补 .catch → toast「全部已读未能保存：…」（修前本地已全标读、计数已扣、无任何提示，重启后全部回退未读）；
⑥ reader.ts selectArticle 的 void api.setRead：补 .catch → toast「标读失败：…」；
⑦ reader.ts toggleCurrentRead 的 setRead 与 toggleCurrentStar 的 setStarred：补 .catch → toast；
⑧ reader.ts markEntriesReadBulk 的逐条 for 循环 void api.setRead：改为 Promise.allSettled → 任一失败单条 toast「部分文章标读失败」（避免批量失败时 toast 洪峰）；
⑨ feeds.ts 六个 AI/折叠开关（toggleCatSummary/toggleCatTranslate/toggleFeedSummary/toggleFeedTranslate/toggleFolderCollapse/toggleAllFolders 的落库调用）：补 .catch → toast「…未能保存，重启后可能回退」（镜像同文件 updateCatLayout 既有口径）；
⑩ 设置页三处挂载期状态拉取（SyncTab 的 syncStatus、ConfigSyncSection 的 configSyncStatus、AiTab 的 getAiConfig）：补 .catch(() => {})（挂载期默认态兜底，非用户动作，不弹 toast；消除 unhandled rejection）。

App.tsx 的 feeds-updated 监听调用 reloadFromBackend 无需单独处理（①的内部 catch 已覆盖）。

**依赖**：TASK-066
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src/App.tsx, src/store/slices/bootstrap.ts, src/store/slices/nav.ts, src/store/slices/reader.ts, src/store/slices/feeds.ts, src/components/settings/SyncTab.tsx, src/components/settings/ConfigSyncSection.tsx, src/components/settings/AiTab.tsx, tools/frontend-regression.mjs

## 验收标准

- diff 仅含 allowed_paths 内 9 文件；N9 拖拽本地态+松手落库；N10 各点按声明口径补 catch/toast（reloadFromBackend rethrow 保持 bootstrapError 路径；markEntriesReadBulk 用 allSettled 单条 toast 防洪峰；挂载期拉取静默兜底）
- 变异可检出：(p) 失败可见性断言与 N9 源码断言在对应缺陷还原后必须失败（如还原 onMove 内 updateSettings → N9 断言失败）
- 四门禁全绿：cargo test 171/0/9 不回退、lint 0/0、build exit 0、frontend 通过数 ≥295 且存量不回退
- 不引入新依赖；package.json/Cargo.toml/Cargo.lock 零改动；文本文件 LF 行尾；台账改动须在 begin 之前完成；用户真实数据库不得写入

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；2026-09-19 基线（TASK-066 终版候选验证 RUN-e7b42163，提交 f2cd034）：cargo test 171 passed / 0 failed / 9 ignored、npm run lint exit 0（0 warnings/0 errors）、npm run build exit 0、npm run test:frontend exit 0 且 295/295。本任务 behavior=change 的范围如实声明：① IPC 失败从「静默/unhandled rejection」变为「用户可见 toast」（N10 各点）——失败提示是新增行为；② 列宽拖拽从「每像素落库」变为「松手一次性持久化」（N9，交互时序变化，最终持久化语义不变）；③ 挂载期拉取失败从 unhandled rejection 变为 console 兜底。既有断言中与失败路径相关的 (a) bootstrap 失败测试在 toast 新增后仍应通过（其断言不含 toast 排除）。
- 基线证据：.workflow-kit/tasks/runs/RUN-e7b42163aaa14d45b754261992f5214d.json
- 需求决定：DEC-992f64fd15714ca2a614fe66d5a144ec
- 补充：(p) 失败可见性行为断言（harness 加通用 rejectCmds 注入旋钮）：① mark_all_read 拒绝 → markCurrentViewAllRead 后出现「全部已读未能保存」toast；② set_read 拒绝 → selectArticle 后出现「标读失败」toast；③ failReload → reloadFromBackend 后出现「刷新失败」toast 且既有 (a) bootstrapError 断言不回退；静默失败此前零覆盖；失败注入是 harness 既有能力（failReload/reject 模式）；验证：frontend
- 补充：(p) N9 源码形态断言（沿用 (n8) 风格）：App.tsx 的 onMove 回调内不得出现 updateSettings（拖动中不落库），onUp 内必须出现（松手持久化）——修前 onMove 内有 updateSettings → 断言失败；拖拽交互无 DOM harness，源码形态断言是既有风格的等价替代；验证：frontend
- 保留：其余全部既有断言（295 条存量）逐字不动；除新增断言块外契约不变；验证：cargo_test, lint, build, frontend

## 执行与恢复

- 首次开始：2026-09-19T23:05:00.182876Z
- 原截止时间：2026-09-20T03:05:00.182876Z
- 当前截止时间：2026-09-20T03:05:00.182876Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 12 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-19T23:05:00.345489Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-19T23:17:49.535651Z：编码结果已记录，差异范围已核对：src/App.tsx, src/components/settings/AiTab.tsx, src/components/settings/ConfigSyncSection.tsx, src/components/settings/SyncTab.tsx, src/store/slices/bootstrap.ts, src/store/slices/feeds.ts, src/store/slices/nav.ts, src/store/slices/reader.ts, tools/frontend-regression.mjs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-19T23:18:10.127053Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-19T23:37:22.313130Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-067.json)

- [RUN-4d2026072d8a436e89e7bfd557c0fc63](../runs/RUN-4d2026072d8a436e89e7bfd557c0fc63.json)
- [RUN-73a92538633d4f40a218a7dcb5097c76](../runs/RUN-73a92538633d4f40a218a7dcb5097c76.json)
- [RUN-e54ccca37ce84aaea60189a339104193](../runs/RUN-e54ccca37ce84aaea60189a339104193.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
