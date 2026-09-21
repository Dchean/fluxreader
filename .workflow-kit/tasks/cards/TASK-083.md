<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-083 · F4：markEntriesReadBulk 逐条 IPC 改为一次批量命令

**状态**：cancelled

**目标**：消除 AUDIT P3[F4] 登记的遗留：`src/store/slices/reader.ts` 的 `markEntriesReadBulk` 目前对每个未读 id 各发一次 `invoke('set_read')`（`reader.ts:365` 的 `Promise.allSettled(unread.map((id) => api.setRead(...)))`），「全部已读」或滚动标读传入几百个 id 时会产生几百次 IPC 往返。改为**一次**批量调用：① Rust 侧在 `src-tauri/src/commands/articles.rs` 新增 `set_read_bulk(ids, read)` 命令，复用既有的 `record_read_state`（每 id 的本地写入 + 入队语义完全不变），整批在**同一次持锁**内完成，锁外只调用一次 `schedule_state_push`；② 前端 `src/lib/api.ts` 新增 `setReadBulk`；③ `reader.ts` 的 `markEntriesReadBulk` 改调它，失败提示语义保持（整批失败给一次 toast，不再逐条）。行为契约不变：本地已读状态、`sync_queue` 入队口径、离线补推语义均与逐条路径一致；唯一变化是 IPC 次数与「部分失败」的粒度（整批原子提交，失败即整批不写）。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/src/commands/articles.rs, src-tauri/src/lib.rs, src/lib/api.ts, src/store/slices/reader.ts, tools/frontend-regression.mjs

## 验收标准

- ① 新增 `set_read_bulk` 命令并在 lib.rs 注册；传入 N 个 id 时**只发一次 IPC**（以前端断言/桩证明调用次数为 1，而非 N）
- ② 语义等价：对同一组 id，批量路径与既有逐条路径产生的本地状态（is_read）与 sync_queue 入队项集合**完全一致**（Rust 单测断言入队项数量与 action 名）
- ③ 批量命令在同一次持锁内完成整批写入，锁外只调用一次 schedule_state_push；ids 为空时安全返回不报错
- ④ 前端 `markEntriesReadBulk` 失败时仍给出用户可见提示（整批一次），且不产生 unhandled rejection（沿用 allSettled 外的等价保护）
- ⑤ 四门禁全绿且不回退：cargo test 通过数 ≥202 且 0 failed、9 ignored 不增；lint 0/0；build exit 0；frontend ≥313/313
- ⑥ 既有断言一行不动；新增断言须有捕获力（说明「只发一次 IPC」如何被证明）

## 测试适用性

- 既有行为：保持
- 原始基线：PASS；2026-09-21 基线（TASK-082 验证 RUN-082fcac4，提交 4df55cb）：cargo test 202 passed / 0 failed / 9 ignored、lint 0 warnings / 0 errors、build exit 0、frontend 313/313。本任务 behavior=preserve：批量标读是对既有逐条标读的**等价重写**（同样的 is_read 结果、同样的 sync_queue 入队项、同样的离线补推），只减少 IPC 次数；故既有全部断言必须原样通过，既有契约不得改变。用户可见行为不变（标读结果与失败提示语义一致），无需 owner 裁决。
- 基线证据：.workflow-kit/tasks/runs/RUN-082fcac4ab9e47669883eeac24241e47.json
- 需求决定：沿用既有行为，无新增业务取舍
- 保留：全部既有 202 条 Rust 断言与 313 条前端断言；批量命令复用既有 record_read_state/db::set_read，逐条路径的既有测试（set_read 相关、mark_all_read 视图过滤、同步入队口径）必须原样通过，作为「等价重写」的判据。；验证：cargo_test, frontend
- 补充：Rust：set_read_bulk 的等价性断言（N 个 id → N 条 sync_queue read 项；空 ids 不报错）与前端：markEntriesReadBulk 只发一次 IPC 的断言；本任务的核心主张是「一次 IPC 且语义等价」，必须对该主张本身取证，而不是只断言周边。前端用桩替换 api.setReadBulk 计数调用次数（必须是 1，且参数含全部未读 id），Rust 用单测断言入队项集合与逐条路径一致。；验证：cargo_test, frontend

## 执行与恢复

- 首次开始：None
- 原截止时间：None
- 当前截止时间：None
- 时钟：未开始
- 已用修复轮：0
- 阻塞：无
- 下一步：如需同一目标，准备新的任务并引用本任务作为历史

## 最近检查点

- 2026-09-21T11:06:09.858345Z：任务已取消：prepare 时 spec 的 allowed_paths 误写在嵌套 scope 下（工具只读顶层，已静默回退为 snapshot_paths，范围偏窄且不可信）；本次工具自带的 task-spec-guard 已捕获该问题。按纪律取消并以订正后的顶层 allowed_paths 重新立项，避免带着错误范围进入 begin（TASK-074/076 同类处置）。；下一步：如需同一目标，准备新的任务并引用本任务作为历史

## 原始证据

[唯一状态记录](../items/TASK-083.json)


卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
