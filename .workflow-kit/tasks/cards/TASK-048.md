<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-048 · 前端拆分前置：补 store.ts 行为测试（为拆分建立回归网）

**状态**：done

**目标**：为 TASK-049（拆分 store.ts）建立可复跑的行为回归网。BRIEF 的重构评估已明确指出「前端 7.8k 行…无组件级行为测试，仅类型检查式回归」且「前端需先补行为测试，起步成本高于后端」，owner 于 2026-09-17 选择『先补 store 行为测试，再拆』（DEC-a2c6584f8d4442daa3a65eb0bcc7ddaf）。本任务在既有前端测试机制上（npm run test:frontend = tsc -p tsconfig.test.json 编译到 dist-test/ 后跑 tools/frontend-regression.mjs），为 src/store.ts 补齐**状态机行为**断言：覆盖 store 的公开 action 与派生状态的因果（选择/筛选/布局切换/已读收藏/分页/搜索/toast/播放器/设置合并/AI per-id 流式等）。hard requirement：既有 26 项断言**逐字保留且全部通过**，新断言只增不改；测试必须在无网络、无 Tauri 依赖下可复跑（沿用现有 loader 与 mock）。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：.workflow-kit/tasks/evidence/baseline-2026-09-17-task048.md, src/**, tools/**, package.json, tsconfig.test.json

## 验收标准

- 既有 26 项断言逐字保留（可用 git diff 证明原有 check 行未被改动或删除）且全部通过
- 新增 store 行为断言覆盖以下**每一个**领域，且每项都断言具体因果而非仅「不抛错」：(a) bootstrap/dataMode 与错误路径；(b) 订阅源与分类的选择（activeFeedFilter）及派生树计数；(c) 视图筛选（view/timeline）与可见条目集合；(d) 内容布局切换（article/social/image/podcast）与派生状态；(e) 文章已读/收藏切换及其对未读计数的影响；(f) markAllRead 的范围语义（含视图筛选口径）；(g) 分页 loadMoreArticles 的游标推进与失败路径（含是否吞错）；(h) 搜索开关与结果竞态守卫；(i) toast 的生成/消失；(j) 播放器状态（激活/播放/进度/seek 夹取）；(k) 设置合并（bootstrapSettings 的校验与回落）；(l) AI per-id 流式的写入与失败标记
- 新增断言总数 ≥25，且全部为确定性断言（不依赖真实计时/网络；需要异步等待时用既有模式）
- npm run test:frontend 退出码 0，输出同时给出「既有 26 项」与「新增」的通过计数
- npm run lint 0 warnings / 0 errors；npm run build 通过
- 测试可重复运行且结果稳定（连续跑两次结论一致）
- 测试过程中不写入用户数据库、不依赖 Tauri 运行时（纯 dist-test 产物 + mock）
- 若测试暴露 src/store.ts 的真实缺陷：**只在报告中逐条记录**（含复现与证据），不在本任务顺手修改实现

## 测试适用性

- 既有行为：保持
- 原始基线：PASS；2026-09-17 基线（本会话实跑）：npm run lint 0 warnings/0 errors；npm run build ✓；npm run test:frontend 26/26 通过、退出码 0；Rust 侧 120 passed/0 failed/23 ignored（本任务不改 Rust）。既有前端套件是纯 Zustand 状态机测试，grep focus|querySelector|document\.|getComputedStyle|tabIndex|role= 命中 0，即它只证明状态流转与 IPC 未被破坏——这正是本任务要扩充的能力边界。
- 基线证据：.workflow-kit/tasks/evidence/baseline-2026-09-17-task048.md
- 需求决定：DEC-a2c6584f8d4442daa3a65eb0bcc7ddaf
- 保留：tools/frontend-regression.mjs 中既有的 26 项断言；它们是 TASK-029~047 累积的行为契约保护；本任务只增不改，任何削弱都会让后续拆分失去回归网；验证：frontend
- 补充：store 行为断言（≥25 项，覆盖 12 个领域）；BRIEF 明确「前端需先补行为测试，起步成本高于后端」；拆分 store.ts 若无行为网，只能靠审查者看 diff；验证：frontend
- 保留：src/store.ts 的实现与行为；本任务是测量而非修改；发现缺陷只记录，避免把「加测试」与「改实现」混成一个不可审查的变更；验证：lint, build, frontend

## 执行与恢复

- 首次开始：2026-09-17T09:09:32.123696Z
- 原截止时间：2026-09-17T13:09:32.123696Z
- 当前截止时间：2026-09-17T13:09:32.123696Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 17 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-17T09:09:32.183044Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后调用 finish，再运行 verify
- 2026-09-17T09:26:43.603663Z：编码结果已记录，差异范围已核对；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-17T09:27:24.499939Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-17T09:35:18.635730Z：当前候选的测试与审查通过；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-048.json)

- [RUN-4ec681d64ecd4d499c363d4ced54a1f3](../runs/RUN-4ec681d64ecd4d499c363d4ced54a1f3.json)
- [RUN-93bba5769406494c869c1f0f81b41c22](../runs/RUN-93bba5769406494c869c1f0f81b41c22.json)
- [RUN-c4a1935bb56c489d92bcfb0a0d69e4fa](../runs/RUN-c4a1935bb56c489d92bcfb0a0d69e4fa.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
