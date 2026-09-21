<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-078 · REQ-104 收尾清理：删除孤儿 commands::sync_now + 修订陈旧 Miniflux 兜底注释

**状态**：ready

**目标**：清理 TASK-070 移交的三条 REQ-104 卫生项（均不在当时 allowed_paths 内，留待后续任务清理）：① 删除孤儿 IP​C 命令 commands::sync_now——该函数在前端 api.ts 中已无调用方（TASK-070 删除了 api.syncNow），但 lib.rs 仍注册着 commands::sync_now 作为 invoke handler。该命令目前仅为一行转调 sync::sync_now，与前端实际调用的 sync_phase 功能重叠（后者由 api.syncPhase 调用）。删除该注册行的同时可删除 commands/sync.rs 中的包裹函数。② 修订三处「Miniflux 兜底」陈旧注释——src/types.ts:44、src-tauri/tests/staged_refresh_e2e.rs:195、src-tauri/tests/sync_phases_e2e.rs:246，这些描述在当前代码中已不准确（该兜底路径已在 0ba940f 协议切换时移除以使 Miniflux 时代不可达代码失效；当前有且仅有直连抓取一种路径，无兜底逻辑）；将其更新为描述当前的降级语义（set_feed_fetch_state 写 fetch_failed 供前端显示错误标志，仅驱动指数退避重试，不再有「兜底拉取」）。本任务为纯删除 + 注释修订，不引入新行为、不新增依赖。

**依赖**：TASK-077
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/src/commands/sync.rs, src-tauri/src/lib.rs, src/lib/api.ts, src/types.ts, src-tauri/tests/staged_refresh_e2e.rs, src-tauri/tests/sync_phases_e2e.rs

## 验收标准

- ① commands::sync_now 及其 lib.rs 注册行均不在（git grep 验证无 product 调用方依赖于该 invoke name）
- ② 三处 Miniflux 兜底注释修正为当前 fetch_failed 语义（仅驱动前端错误标志 + 指数退避重试）
- ③ cargo test 通过数 ≥193 且 0 failed、9 ignored 不增；lint 0/0；build exit 0；frontend 303/303
- ④ 不引入新依赖；package.json/Cargo.toml/Cargo.lock 零改动；文本文件 LF；用户真实数据库不得写入

## 测试适用性

- 既有行为：保持
- 原始基线：PASS；2026-09-21 基线（TASK-077 终版候选验证 RUN-f5c07ebc，提交 b1fe783）：cargo test 193 passed / 0 failed / 9 ignored、npm run lint exit 0（0 warnings/0 errors）、npm run build exit 0、npm run test:frontend exit 0 且 303/303。本任务为纯删除+注释修订，不涉及行为变更。所有既有断言逐字保持。
- 基线证据：.workflow-kit/tasks/runs/RUN-f5c07ebc890b4519a2f79d843e8d2bcd.json
- 需求决定：沿用既有行为，无新增业务取舍
- 保留：全部 193 条 Rust + 303 条前端断言逐字保持；sync::sync_now 核心函数与其测试调用点不动。三处「Miniflux 兜底」注释的文字修订不影响任何断言或行为（仅为让注释与当前 fetch_failed 语义一致）；本任务只删孤儿 IPC 壳与修订注释文案，不涉及行为变更，全部断言原样保留；验证：cargo_test, lint, build, frontend

## 执行与恢复

- 首次开始：None
- 原截止时间：None
- 当前截止时间：None
- 时钟：未开始
- 已用修复轮：0
- 阻塞：无
- 下一步：执行 start/next 获取可继续的动作

## 最近检查点


## 原始证据

[唯一状态记录](../items/TASK-078.json)


卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
