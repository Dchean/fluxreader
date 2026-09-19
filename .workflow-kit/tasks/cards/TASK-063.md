<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-063 · 接线 selectFeed 范围切换拉取：缓存命中同步恢复 + 未命中重拉，修复跨范围污染与重复卡片（REQ-101/N2）

**状态**：done

**目标**：AUDIT-20260919-v2.md 新发现 N2（REQ-101，前端排查确认，P1）：nav.ts 的 selectFeed 只写 per-scope 游标镜像，注释宣称「由调用方随后拉取」，但 Sidebar.tsx 与 Overlays.tsx 的全部调用点均未接线任何 reload——点击订阅源后 entries 停留旧范围快照：① 目标源不在快照时空列表补拉 effect（Timeline.tsx 空列表分支）触发 loadMoreArticles，把该范围第 1 页**追加**进旧快照（全链路无按 id 去重）→ 切回「全部」同文双卡、虚拟滚动 duplicate key；② 目标源在快照中少量条目且不可滚动 → 该源自己的第 1 页永远拉不到；③ 可滚动场景 offset=0 补拉与旧快照交集重复。关键证据：回归测试 (s4) 在 selectFeed 后**手工调用 reloadFromBackend() 才通过**——测试模拟了 UI 中不存在的一步。修复（与 selectView 的缓存优先契约同构）：tauri 模式下 selectFeed 写完游标镜像后，查 viewEntriesCache（layout|view|新 scopeKey）——命中则同步恢复该范围快照（entries + applyArticlesCursor 收口游标 + 清 hydratedIds/hydrationErrors）并后台刷新；未命中直接后台重拉（view!=='all' 走 reloadFilteredEntries(view)，否则 reloadFromBackend()，两者都在发起时读取刚写入的 activeFeedFilter 且自带代际/竞态守卫）；mock 模式保持纯游标镜像不变。附带修复同类隐患：selectView 缓存命中恢复同样不清水合状态——水合守卫（reader.ts ensureArticleContent 的 art.content || hydratedIds[id] 短路）会对缓存快照（无正文）误判已水合，恢复到后台刷新落地之间社交/通知卡片正文空白且不会重水合；两处恢复统一补 hydratedIds:{}, hydrationErrors:{}（后台刷新本就会整体替换并清空，补齐消除空窗）。契约变更如实声明：范围切换的恢复语义从「保留旧 limit 数值但 entries 从不对齐」（缺陷行为）改为「恢复该范围快照并刷新（'all' 视图游标=快照长度，可继续翻页；筛选视图标记已到底）」——(s4) 末段「切回源A 恢复它自己的游标（1000）」随之更新为新契约。不改 loadMoreArticles 的追加逻辑与 Timeline 的补拉 effect（瞬态重复由重拉整体替换自愈，窗口亚秒级且 id 相同渲染无感）。

**依赖**：TASK-062
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src/store/slices/nav.ts, tools/frontend-regression.mjs

## 验收标准

- diff 仅含 src/store/slices/nav.ts 与 tools/frontend-regression.mjs：selectFeed 在 tauri 模式下接线（缓存命中同步恢复 entries + applyArticlesCursor 收口 + 清 hydratedIds/hydrationErrors + 后台刷新；未命中后台重拉，view!=='all' 走 reloadFilteredEntries）；mock 模式保持纯游标镜像；selectView 缓存命中补清水合状态
- 缺陷复现⇒测试失败成对证据：把 selectFeed 的接线还原为纯游标镜像后，新增的「自动重拉/同步恢复」断言必须失败；还原后必须通过
- (s4) 契约更新如实：假步骤移除、末段按新契约断言；(b) 块与全部其他既有断言一行不动
- 四门禁全绿：cargo test 164/0/9 不回退（ignored 不增）、lint 0 warnings/0 errors、build exit 0、frontend 通过数 ≥283 且既有存量不回退（ignored/存量断言不得删除弱化）
- 不引入新依赖；package.json/Cargo.toml/Cargo.lock 零改动
- 文本文件 LF 行尾；台账改动须在 begin 之前完成
- 用户真实数据库不得写入（沿用项目约束）

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；2026-09-19 基线（TASK-062 终版候选验证 RUN-0259efca，提交 62a3b73）：cargo test 164 passed / 0 failed / 9 ignored、npm run lint exit 0（0 warnings/0 errors）、npm run build exit 0、npm run test:frontend exit 0 且 283/283。本任务 behavior=change 的范围如实声明：① selectFeed 从「只写游标镜像、entries 不对齐」（缺陷行为）改为「缓存命中同步恢复 + 后台刷新 / 未命中后台重拉」——范围切换的 entries 对齐性是新契约；② 范围切换的分页恢复语义相应变化：'all' 视图切回已加载过的范围，游标从旧 limit 数值（如 1000）变为快照长度（500，可继续翻页），与 reloadFromBackend（feeds-updated 事件已存在的重置语义）一致，筛选视图标记已到底；③ selectView/selectFeed 缓存恢复清水合状态（修隐患，无可见行为回归）。既有 283 条断言中仅 (s4) 末段钉住缺陷契约（「恢复它自己的游标（1000）」）需按新契约更新，其余全部不动。
- 基线证据：.workflow-kit/tasks/runs/RUN-0259efca5b644b909fcf022fefeaeb73.json
- 需求决定：DEC-992f64fd15714ca2a614fe66d5a144ec
- 适配：(s4) per-scope 游标测试：移除 selectFeed 后手工调用 reloadFromBackend() 的假步骤（改为 await 等待 selectFeed 自身触发的重拉落地）；末段「切回源A 恢复它自己的游标（1000）」更新为新契约断言（缓存快照恢复：entries 全属源A 第 1 页、游标=快照长度 500、源B 游标 1000 不被污染）；(s4) 原本钉住的是缺陷契约（entries 不对齐下的 limit 保留）；selectFeed 接线后该手工步骤与末段断言描述的行为被本任务契约取代；验证：frontend
- 补充：新增断言块：(i) selectFeed 缓存命中零延迟恢复——selectFeed('11') 后同步断言 entries 立即为源B 快照（不 await）、游标=快照长度、hydratedIds 已清空；(ii) selectFeed 缓存未命中自动重拉——selectFeed 后 await 落地，entries 全属新范围、该范围游标写入；(iii) selectView 缓存命中恢复后 hydratedIds 已清空（水合状态一致）；缺陷此前无覆盖（(s4) 的假步骤掩盖了 UI 未接线的事实）；新契约的行为需要正反断言锚定；验证：frontend
- 保留：其余全部既有断言（含 (a)(b)(c)(s1)(s2) 等既有块与 283 条存量）逐字不动；除声明适配的 (s4) 外，其他契约不变；既有回归网是改动安全网；验证：cargo_test, lint, build, frontend

## 执行与恢复

- 首次开始：2026-09-19T14:32:12.891198Z
- 原截止时间：2026-09-19T18:32:12.891198Z
- 当前截止时间：2026-09-19T18:32:12.891198Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 16 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-19T14:32:13.049260Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-19T14:48:58.575574Z：编码结果已记录，差异范围已核对：src/store/slices/nav.ts, tools/frontend-regression.mjs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-19T14:49:18.750470Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-19T15:05:13.354849Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-063.json)

- [RUN-ab44d829b4fd42c68c4c935a5f8e60d5](../runs/RUN-ab44d829b4fd42c68c4c935a5f8e60d5.json)
- [RUN-1a3b0ef069ce4696b3c3bbd75de5a3b8](../runs/RUN-1a3b0ef069ce4696b3c3bbd75de5a3b8.json)
- [RUN-9329a573e6fa449dae0899b1531846a0](../runs/RUN-9329a573e6fa449dae0899b1531846a0.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
