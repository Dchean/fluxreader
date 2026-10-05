<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-106 · 文章快照与正文水合生命周期统一——刷新不丢正文、水合终态完备（承接 TASK-103，REQ-001）

**状态**：verified

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
- ⑤ 门禁全绿不回退：frontend 全过（t103-* 新增）、lint/build exit 0、cargo fmt 本地不回退；cargo test/clippy 由 CI rust job 承担（DEC-local-cargo-gate-20261005，CI 绿后合并）
- ⑥ 独立审查（全新子代理，未参与编码）PASS findings=0

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；dev@30cd2bc（v0.16.2 后，与 main@07a0db9 源码同基线）：外部审计实测 frontend 527/527 通过；cargo 本机预检 cargo check 通过（审计环境缺 link.exe 不适用于本机）；lint/build 既有门禁绿。本卡修复 REQ-001 根因并补终态机，属行为修复。
- 基线证据：.workflow-kit/docs/AUDIT-20261005-core-consistency.md
- 需求决定：DEC-refactor-roadmap-20261005, DEC-local-cargo-gate-20261005
- 补充：t103-* 源级/行为断言（快照保留、重水合、终态机、乱序防护、在途去重）；REQ-001 根因修复需防回退；审计探针场景转断言；验证：frontend
- 保留：既有 frontend/cargo fmt/fmt clippy(仅CI)/lint/build 断言；本地不回退证据=cargo_fmt+lint+build+frontend；cargo test/clippy 由 CI rust job 承担（DEC-local-cargo-gate-20261005）；验证：cargo_fmt, lint, build, frontend

## 执行与恢复

- 首次开始：2026-10-05T08:14:21.325452Z
- 原截止时间：2026-10-05T12:14:21.325452Z
- 当前截止时间：2026-10-05T12:14:21.325452Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 0 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-10-05T08:14:21.999974Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-10-05T08:15:20.011280Z：编码结果已记录，差异范围已核对：src/components/Timeline.tsx, src/store/internals.ts, src/store/selectors.ts, src/store/slices/bootstrap.ts, src/store/slices/nav.ts, src/store/slices/reader.ts, tools/frontend-regression.mjs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-10-05T08:15:42.665886Z：Required gate failed: cargo_fmt；下一步：先核对已有文件及原始日志，再处理 test_failure；不要新建任务或重置预算
- 2026-10-05T08:20:27.314106Z：阻塞已处置（test_failure）：前次 cargo_fmt 失败根因是 .cargo/bin 符号链接被 runtime resolve 成 rustup.exe（已以实体副本修复并复验 cargo fmt --version 正常），非代码格式问题；cargo fmt --check 手动复跑 exit 0。恢复验证。；下一步：verify 当前候选
- 2026-10-05T08:20:56.080072Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-10-05T08:35:24.987244Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-106.json)

- [RUN-a15d4240af8041daa4d065e5565999e7](../runs/RUN-a15d4240af8041daa4d065e5565999e7.json)
- [RUN-2eb9a897b5984192a621cd37ed2ec5f6](../runs/RUN-2eb9a897b5984192a621cd37ed2ec5f6.json)
- [RUN-f12e010ce35b40fd905675657a21a936](../runs/RUN-f12e010ce35b40fd905675657a21a936.json)
- [RUN-463ebba3d975422ea8f797f66cae7424](../runs/RUN-463ebba3d975422ea8f797f66cae7424.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
