<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-114 · 五布局状态与快捷键统一——NotifCard 补水合态、Enter 绑定与 J/K 全布局化（三阶段①，REQ-005/008）

**状态**：done

**目标**：五布局交互一致性第一卡（探查证实加载/空/哨兵/refill 已五布局统一，本卡只收口三处真实不一致）：①NotifCard 水合态缺失——NotifCard 走 useLazyHydrate 水合（Timeline.tsx:960 一带）但不订阅 hydrationErrors/hydratedIds，失败时静默回退 snippet（:962-965），而 SocialCard 有完整失败→内联重试（retryHydration）/空正文→「暂无正文」/加载中三态（:660-671）。修法：NotifCard 对齐 SocialCard 三态（复用其 JSX/样式形态，通知卡文案域内适配），失败不再静默。②Enter 绑定不一致——Article/Podcast/Gallery 有 Enter/Space（选中/play/灯箱，:503-513/:901-907/:816-837），Social/Notif 只有方向键（:623-629/:996-1001）。修法：Social/Notif 补 Enter/Space=选中（与 ArticleCard 同语义：onSelect(id)），tabIndex/role 形态对齐。③J/K 范围——App.tsx:272-285 的 J/K 显式 `if (s.activeContentLayout !== 'article') return;` 仅文章布局生效。修法：扩展到全部虚拟化布局（article/social/podcast/notification；image 布局非虚拟化无 moveCardFocus 基建，维持现状并在 ShortcutsTab 注明「画廊布局不支持 J/K」），选中切换/循环回绕/moveCardFocus 行为不变；shouldYieldToOverlay 让路规则复用不扩键。ShortcutsTab（:8-16）文案同步。UI 契约文档（三检查点）随卡建立。回归断言 t114-* ≥6 条：NotifCard 三态 SSR/源级断言（复用 renderToStaticMarkup 先例 :3948-3959 与源级扫描先例）、Enter 绑定源级断言（五卡对照）、J/K 布局门控行为断言（store 层模拟五布局 keydown 语义）、ShortcutsTab 一致性扫描。既有断言零弱化（若 Social/Notif「仅方向键」有既有断言锁定，更新附理由）。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：.workflow-kit/docs/UI-CONTRACT-TASK-114-LAYOUT-CONSISTENCY.md
**界面检查**：X1.notif-hydration-states, X2.enter-binding-uniform, X3.jk-all-virtualized
**修改范围**：src, tools/frontend-regression.mjs

## 验收标准

- ① NotifCard 三态与 SocialCard 同构（X1：失败重试/暂无正文/加载中；SSR 或源级断言锁定）
- ② Enter 绑定五卡统一（X2：Social/Notif=选中，Podcast=play，Gallery=灯箱，Article=选中）
- ③ J/K 全虚拟化布局生效、画廊注明不支持（X3：行为断言+ShortcutsTab 同步）
- ④ UI 契约文档三检查点落账（.workflow-kit/docs/UI-CONTRACT-TASK-114-LAYOUT-CONSISTENCY.md）
- ⑤ 门禁全绿：frontend（t114-* 新增）、lint/build/cargo_fmt 不回退；cargo test/clippy 由 CI 承担（DEC-local-cargo-gate-20261005）
- ⑥ 独立审查（全新子代理）PASS findings=0；ui_change=true 需 UI 取证（SSR 形态断言可作证据通道，真机截图非必需但需注明）

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；dev@二阶段后（main@82b2488 同源）：frontend 605/605、CI rust job 全绿（dev@5860dad）。本卡为交互一致性补齐（新键绑定/新状态 UI），属行为变化（owner 指示按审计三阶段推进）。
- 基线证据：.workflow-kit/docs/AUDIT-20261005-core-consistency.md
- 需求决定：DEC-refactor-roadmap-20261005
- 补充：t114-* 断言（NotifCard 三态/Enter 五卡对照/J-K 门控/ShortcutsTab 同步）；行为变化需成对断言；验证：frontend
- 适配：锁定 Social/Notif「仅方向键」等旧行为的既有断言（如有）；旧行为正是本卡收口的不一致点，更新附理由；验证：frontend
- 保留：既有 frontend/cargo fmt/lint/build 断言；不回退证据；cargo 由 CI 承担（DEC-local-cargo-gate-20261005）；验证：cargo_fmt, lint, build, frontend

## 执行与恢复

- 首次开始：2026-10-06T09:21:21.079559Z
- 原截止时间：2026-10-06T13:21:21.079559Z
- 当前截止时间：2026-10-06T13:21:21.079559Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 55 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-10-06T10:55:42.346844Z：编码结果已记录，差异范围已核对：src/components/Timeline.tsx, tools/frontend-regression.mjs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-10-06T10:56:13.448460Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-10-06T11:20:33.588848Z：Review requires changes; inspect the findings；下一步：先核对已有文件及原始日志，再处理 review_failure；不要新建任务或重置预算
- 2026-10-06T11:20:42.212774Z：阻塞已处置（review_failure）：R1 复审 FAIL（1 低危残留）：展开按钮门控 {isLong && !hydrationError} 在组合态（错误滞留+fullText 到达）下 clamp snippet 却无展开入口（修前与同态 SocialCard 均可展开）。R2 一行修：门控改 {isLong && (!hydrationError || !!fullText)} + 注释修正 + x1g 同步。修复轮 2/4。；下一步：begin 重新实现
- 2026-10-06T11:21:39.817311Z：开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-10-06T11:25:58.732262Z：编码结果已记录，差异范围已核对：src/components/Timeline.tsx, tools/frontend-regression.mjs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-10-06T11:26:33.053069Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-10-06T11:57:02.007052Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-114.json)

- [RUN-b6ade06caf2448969617ab6dccf1f3d3](../runs/RUN-b6ade06caf2448969617ab6dccf1f3d3.json)
- [RUN-730237dfc22742c99b1bcc1ce6e53770](../runs/RUN-730237dfc22742c99b1bcc1ce6e53770.json)
- [RUN-047746af8dc54cdb9a50fa06ea9a338d](../runs/RUN-047746af8dc54cdb9a50fa06ea9a338d.json)
- [RUN-b287f3fc3c2e46ada62ec2ff196ef958](../runs/RUN-b287f3fc3c2e46ada62ec2ff196ef958.json)
- [RUN-ac62aa3f118d440194c71a2ebde87107](../runs/RUN-ac62aa3f118d440194c71a2ebde87107.json)
- [RUN-43c232645aa346338e6a63221cac32c2](../runs/RUN-43c232645aa346338e6a63221cac32c2.json)
- [RUN-160011a0c9d941a2815b4ca89458f040](../runs/RUN-160011a0c9d941a2815b4ca89458f040.json)
- [RUN-3615bd26477e436abe9d54e2da88ea16](../runs/RUN-3615bd26477e436abe9d54e2da88ea16.json)
- [RUN-ce4c23844b214c1ab6f726ff11ce85b3](../runs/RUN-ce4c23844b214c1ab6f726ff11ce85b3.json)
- [RUN-83a6895a363e4b44a812835be7d0c59e](../runs/RUN-83a6895a363e4b44a812835be7d0c59e.json)
- [RUN-7581ba793a7c4a10b31b3e560fb57b5f](../runs/RUN-7581ba793a7c4a10b31b3e560fb57b5f.json)
- [RUN-2730518f88984fb0bf240f2f006e824e](../runs/RUN-2730518f88984fb0bf240f2f006e824e.json)
- [RUN-21ee986be8bd458bb7dce8519b7dca81](../runs/RUN-21ee986be8bd458bb7dce8519b7dca81.json)
- [RUN-3354f36bf6e5436292b1155b9cec7315](../runs/RUN-3354f36bf6e5436292b1155b9cec7315.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
