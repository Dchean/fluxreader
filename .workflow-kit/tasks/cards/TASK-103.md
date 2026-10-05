<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-103 · 文章快照与正文水合生命周期统一——刷新不丢正文、水合终态完备（REQ-001）

**状态**：cancelled

**目标**：修复 REQ-001（社交布局正文一直加载）的根因并补齐水合终态机，依据外部审计探针复现的链路（AUDIT-20261005-core-consistency.md）：①src/store/slices/bootstrap.ts reloadFromBackend 快照替换 entries 时新行不含正文且清空 hydratedIds，而虚拟列表按文章 id 保持组件身份、useLazyHydrate（src/components/Timeline.tsx 一带）effect 依赖仅 [id]，同 id 不再触发水合请求，卡片停留「加载正文…」且实际无请求在途。修法要求：快照替换时按 id 保留既有条目的正文字段（content/rawContent/translatedContent/aiSummary/fulltextExtracted/hydrated），新行自带正文（with_content 场景）时以新行为准；hydratedIds 不再无条件清空；hydrated 保留与新行合并逻辑收口到单一函数（避免 bootstrap/bootstrapFromBackend/其他快照路径各自为政）。②水合触发修正：useLazyHydrate 不再只依赖 [id]——卡片挂载期间观察 store 的「无正文 && 未水合 && 无终态 && 无在途」状态，条件重新成立时重新入队（或 reloadFromBackend 完成后对未水合条目统一重新入队，coder 二选一并断言锁定）。③终态机完备（reader.ts hydrateArticleContent / enqueueHydration / retryHydration）：成功含空正文 → hydrated=true 终态「无正文」（不再显示加载占位、不无限重试）；请求 rows 中缺失的 id → 「文章不存在」终态（hydrationErrors 明确错误或从 entries 清理，不得留加载占位）；请求失败 → hydrationErrors 保留内联重试；空 ids/空 rows 不再静默 return 留占位；乱序/过期响应防护（reload 已有 reloadGeneration 手法，水合补同类保护），旧响应不得覆盖新状态。④既有 enqueueHydration 在途去重语义保留。回归断言：tools/frontend-regression.mjs 新增 t103-* ≥5 条（快照替换保留正文、同 id 刷新后重新水合、空正文/缺行/失败终态、乱序防护、在途去重），与既有断言冲突项同步更新。coder 开工前先实证 with_content 在各布局的实际取值（layoutNeedsBody），据实修正注释与逻辑，不以猜测为准。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src, tools/frontend-regression.mjs

## 验收标准

- ① 快照替换保留水合：构造已水合 entries + reloadFromBackend 快照替换，同 id 条目 content/hydrated 保留、hydratedIds 不清空；新行自带正文时以新行为准（t103-snapshot-preserves-hydration）
- ② 同 id 刷新后重新水合：审计探针场景（同 ID 刷新后 content=''）必须重新发起水合或直接恢复正文，不再出现「显示加载中但无请求」死区（t103-stale-card-rehydrates）
- ③ 终态机：空正文→「无正文」终态、缺行→「文章不存在」终态、失败→hydrationErrors+内联重试、空 ids/空 rows 不留占位（t103-hydration-terminals）
- ④ 乱序/过期防护与在途去重：旧响应不覆盖新状态，同 id 在途不重复 IPC（t103-race-and-dedup）
- ⑤ 门禁全绿不回退：frontend 全过（t103-* 新增）、lint/build exit 0、cargo test/fmt/clippy 不回退
- ⑥ 独立审查（全新子代理，未参与编码）PASS findings=0

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；dev@30cd2bc（v0.16.2 后，与 main@07a0db9 源码同基线）：外部审计实测 frontend 527/527 通过；cargo 本机预检 cargo check 通过（审计环境缺 link.exe 不适用于本机）；lint/build 既有门禁绿。本卡修复 REQ-001 根因并补终态机，属行为修复。
- 基线证据：.workflow-kit/docs/AUDIT-20261005-core-consistency.md
- 需求决定：DEC-refactor-roadmap-20261005
- 补充：t103-* 源级/行为断言（快照保留、重水合、终态机、乱序防护、在途去重）；REQ-001 根因修复需防回退；审计探针场景转断言；验证：frontend
- 保留：既有 frontend/cargo/fmt/clippy/lint/build 断言；不回退证据（本地）；cargo_test/clippy 因本机无 MSVC 链接器由 CI rust job 承担（DEC-local-cargo-gate-20261005，dev 推送即跑，CI 绿后合并）；验证：cargo_fmt, lint, build, frontend

## 执行与恢复

- 首次开始：2026-10-05T07:08:17.408657Z
- 原截止时间：2026-10-05T11:08:17.408657Z
- 当前截止时间：2026-10-05T11:08:17.408657Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 42 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：如需同一目标，准备新的任务并引用本任务作为历史

## 最近检查点

- 2026-10-05T07:08:18.223664Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-10-05T07:50:25.101115Z：Worker requests manager action; inspect the result；下一步：处理执行者提出的请求，再 unblock 后 begin；不要新建任务或重置预算
- 2026-10-05T07:51:19.848486Z：阻塞已处置（action_required）：首次 finish 因 worker-result unresolved_items 非空被判 action_required；主控已核对：unresolved 两项均为非阻塞备注（会话内空正文终态语义、NotifCard 视觉不变），已并入 summary 清空列表重交。代码改动范围经 diff 核对全部在 allowed_paths 内。；下一步：begin 重新实现
- 2026-10-05T07:52:17.960979Z：开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-10-05T07:53:01.812230Z：编码结果已记录，差异范围已核对：src/components/Timeline.tsx, src/store/internals.ts, src/store/selectors.ts, src/store/slices/bootstrap.ts, src/store/slices/nav.ts, src/store/slices/reader.ts, tools/frontend-regression.mjs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-10-05T07:53:17.043912Z：Required gate failed: cargo_test；下一步：先核对已有文件及原始日志，再处理 test_failure；不要新建任务或重置预算
- 2026-10-05T08:08:11.352287Z：阻塞已处置（test_failure）：前次 verify 失败根因是环境性的：本机无 MSVC 链接器导致 cargo_test 必败（GNU link.exe 遮蔽+无 VS Build Tools，探针证据在案）。主控已将 cargo_test/cargo_clippy 改为可选门禁并记录决定；cargo 权威证据由 CI rust job 承担。非代码问题，恢复验证。；下一步：verify 当前候选
- 2026-10-05T08:10:38.643526Z：任务已取消：主控立项时误将 cargo_test/cargo_clippy 设为必需门禁，与本机无 MSVC 链接器的环境事实冲突（DEC-local-cargo-gate-20261005）；prepare 后改门禁被 verify 的防 rehash 设计正确拒绝。按工具设计取消原卡，以修正门禁（cargo_test/clippy 可选+理由，cargo 权威证据=CI rust job）重建同规格任务；代码改动已 stash，将在新卡 begin 前恢复以保证输入快照→候选差异完整。编码结果经 frontend 543/543、lint、build 自检通过。；下一步：如需同一目标，准备新的任务并引用本任务作为历史

## 原始证据

[唯一状态记录](../items/TASK-103.json)

- [RUN-cebca80c3fa14897aa7effcad1540d61](../runs/RUN-cebca80c3fa14897aa7effcad1540d61.json)
- [RUN-2e4b96f5e37147599352e10b01941b9f](../runs/RUN-2e4b96f5e37147599352e10b01941b9f.json)
- [RUN-5fa718eff0df4a14804ef314346ed909](../runs/RUN-5fa718eff0df4a14804ef314346ed909.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
