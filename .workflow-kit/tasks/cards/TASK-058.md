<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-058 · 同步失败对用户可见：前端消费 SyncReport.errors

**状态**：done

**目标**：TASK-056 已让后端把 pull 失败写入 `report.errors`，但**前端没有任何代码读取该字段**，导致**同步实际失败时界面仍提示「后端同步完成」**——用户看到成功、数据却没进来。已实证：TASK-056 的端到端测试用测试库触发器注入真实插入失败，修复后的应用返回 `errors:["拉取订阅 … 建本地失败: [db] e2e injected insert failure", …]`，界面**依然**弹「后端同步完成」。根因：`src-tauri/src/sync/phases.rs` 在 errors 非空时仍返回 `Ok(report)`，故 `SyncTab.tsx` 的 `.catch()` 对该路径永不触发；而两处 `syncPhase` 调用点（`SyncTab.tsx` 的保存后后台链、`store/slices/sync.ts` 的 `triggerManualSync`）都**丢弃了返回的 report**。本任务让前端消费该字段，把「同步完成但有 N 项失败」真实告知用户。

**依赖**：TASK-057
**参考方案**：见 ../RESEARCH.md
**界面约定**：.workflow-kit/docs/UI-CONTRACT-REQ-058.md
**界面检查**：同步过程中后端返回非空 errors 时，用户能看到明示「有 N 项失败」的提示（不再是「后端同步完成」）, 提示与既有 toast 风格一致（沿用 showToast，不新造控件/弹窗）, 成功路径（errors 为空）文案与顺序完全不变：仍提示「后端同步完成」, 同时存在「直连抓取失败」与「同步 errors」时两类信息都在，不互相覆盖, 深色与浅色两套主题下提示完整可读、不截断、不溢出, 错误明细可查看：提示中或其后能获知具体失败原因（非仅计数）
**修改范围**：.workflow-kit/docs/UI-CONTRACT-REQ-058.md, .workflow-kit/tasks/evidence/baseline-2026-09-18-task058.md, .workflow-kit/tasks/evidence/TASK-058-*, src/**, tools/**, tsconfig.test.json

## 验收标准

- **建立可复跑的失败场景**：新增测试确定性构造「后端返回非空 errors」的 `SyncReport`，断言前端**确实**产生失败提示（而非成功提示）。该断言须**修前失败、修后通过**，并给出修前失败证据
- **成功路径逐字不变**：errors 为空时，既有文案（`已拉取订阅源，正在同步文章状态…`、`后端同步完成`、`订阅同步完成，正在同步文章状态…`）与调用顺序**一字不改**；须以断言或 diff 证据证明
- **两处调用点都覆盖**：`SyncTab.tsx`（保存并同步的后台链）与 `store/slices/sync.ts`（`triggerManualSync`）**都要**消费 report.errors；只改一处不算完成
- **错误明细可获知**：提示须让用户能看到**具体失败原因**（可含计数 + 首条原因，或可展开），不得只给一个孤立数字
- **不覆盖既有失败信息**：`triggerManualSync` 末端已有「N 个源直连失败」提示，两类失败信息须共存（不互相吞掉）
- **UI 证据（ui_change=true）**：提供**深色与浅色两套主题**下的实机截图，覆盖 `ui_checks` 全部条目；须为运行中应用的截图（非设计稿），并附交互报告说明验证方式与操作路径
- **端到端验证（本任务特有，须做）**：用真实运行的应用 + 本地协议服务端，注入真实插入失败，实测**界面出现失败提示**；给出实际 toast 文本。若无法达成，须停下来报告而不是改断言
- 既有前端回归不回归：`npm run test:frontend` 通过（基线 **255/255**，可增不可减）
- 既有 Rust 测试不得回退：`cargo test` 通过（基线 **141 passed / 0 failed / 9 ignored**）
- 四门禁全绿：`cargo test`、`npm run lint`、`npm run build`、`npm run test:frontend`
- **`src-tauri/**` 零改动**（给出 `git diff --stat -- src-tauri` 为空的证据）；不引入新依赖（`package.json` 零改动）
- 文本文件必须 LF 行尾；台账改动须在 begin 之前完成；行数以 `splitlines()` 口径报告（不得用 `Measure-Object -Line`）
- **用户真实数据库不得写入**：`%APPDATA%\com.fluxreader.app` 只读使用；端到端测试如需改数据，须先备份并在结束后逐字节还原（给出哈希一致的证据）

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；2026-09-18 基线（TASK-057 之后）：`npm run lint` exit 0（0 warnings/0 errors）、`npm run build` exit 0、`npm run test:frontend` exit 0 且 **255/255**（既有 26 + 新增 229）、`cargo test` **141 passed / 0 failed / 9 ignored**。**本任务有意改变行为**：同步在「完成但存在失败项」时对用户的呈现，由『提示成功』改为『提示成功 + 明示 N 项失败及原因』——这是**用户可见文案与可观测性契约变化**，已由 DEC-sync-errors-visibility-20260918 覆盖。**基线盲区须如实记录**：前端 229 条新增断言中**没有一条**针对 `SyncReport.errors` 的消费（全仓 `src/**` 与 `tools/**` 无任何代码读取 `.errors`），因此该路径此前完全无覆盖；本任务必须补上，否则同类失败仍会对用户不可见。
- 基线证据：.workflow-kit/tasks/evidence/baseline-2026-09-18-task058.md
- 需求决定：DEC-sync-errors-visibility-20260918
- 补充：『后端返回非空 errors ⇒ 前端产生失败提示』的可复跑断言（含修前失败证据）；该路径此前完全无覆盖（无任何代码读取 errors）。须有断言锁定新契约，防止回退成『静默报成功』。；验证：frontend
- 补充：『errors 为空 ⇒ 既有成功文案逐字不变』的断言；本次改动触及同步提示链，必须锁定成功路径未被改坏——否则修复失败可见性的同时会破坏既有已验收交互。；验证：frontend
- 适配：`src/components/settings/SyncTab.tsx` 与 `src/store/slices/sync.ts` 中两处 `syncPhase` 调用点的 report 消费；这两个调用点当前丢弃 report（`.then(async () => …)` 不接收参数 / `feedsReport` 仅用于判真）。需改为读取 errors 并提示；若既有断言引用了旧提示文案，须相应适配而非删除。；验证：frontend
- 保留：其余前端断言（含 TASK-057 的 14 条 Endpoint 文案/提示断言、TASK-052 分页口径、TASK-051 store 行为）；本任务只改同步失败提示的呈现，不得波及已验收的文案、分页与 store 行为；它们是『改动未外溢』的直接证据。；验证：frontend
- 保留：全部 Rust 测试（141 passed / 9 ignored，含 TASK-055/056 的墓碑与零 folder 新库回归网）；本任务按边界不改 Rust（`src-tauri/**` 零改动）；Rust 侧全绿即证明同步引擎语义未被触碰。；验证：cargo_test

## 执行与恢复

- 首次开始：2026-09-18T12:05:05.524260Z
- 原截止时间：2026-09-18T16:05:05.524260Z
- 当前截止时间：2026-09-18T16:05:05.524260Z
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-18T12:05:05.632949Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后调用 finish，再运行 verify
- 2026-09-18T12:16:07.916993Z：编码结果已记录，差异范围已核对；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-18T12:16:29.368054Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-18T12:31:07.073931Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-18T12:54:44.572502Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-18T13:08:41.262163Z：当前候选的测试与审查通过；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-058.json)

- [RUN-5fe04a5e590e47e6a11cd7f0e8d30ff3](../runs/RUN-5fe04a5e590e47e6a11cd7f0e8d30ff3.json)
- [RUN-0cdedd59c05544b8b2dd7a3508448af8](../runs/RUN-0cdedd59c05544b8b2dd7a3508448af8.json)
- [RUN-9e2e3d0f136b4f5eb7b33f82f9d29861](../runs/RUN-9e2e3d0f136b4f5eb7b33f82f9d29861.json)
- [RUN-1d654a01e98c499382916771f20acec8](../runs/RUN-1d654a01e98c499382916771f20acec8.json)
- [RUN-99e553929986422baffb9c416e758752](../runs/RUN-99e553929986422baffb9c416e758752.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
