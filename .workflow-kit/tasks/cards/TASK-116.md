<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-116 · 同步四态展示——队列状态列、统计命令与主窗口/设置页呈现（三阶段③，REQ-002/003）

**状态**：done

**目标**：同步四态展示（审计优化点「同步界面区分本地已保存/等待同步/远端已确认/部分失败」；探查事实：sync_queue 无状态列 migrations.rs:94-101、失败项无 per-item 标记仅聚合 errors、无生产命令暴露队列、主窗口指示为侧栏 pill 四分支且失败态会被 syncing 覆盖 Sidebar.tsx:88-100）。修法要求：①Rust——新迁移加列：sync_queue 增 attempts INTEGER NOT NULL DEFAULT 0 与 last_error TEXT（沿用既有迁移模式，user_version 递增；旧库兼容）；push.rs 推送失败项 UPDATE attempts=attempts+1, last_error=…，成功 prune 照旧（远端确认=出队）；新增命令 sync_queue_stats() -> { waiting: usize, failed: usize }（waiting=队列行数按 action 分组可选，failed=attempts>0 的行数与最新 last_error 摘要），commands/sync.rs 注册+lib.rs invoke_handler；顺手修正 greader_pull.rs:310 一带 seed_bound 的 doc 笔误（写「返回二元组」实际返回 aid——延续任务审查裁定的登记项，同文件领域顺手修）。②前端——api.syncQueueStats()；侧栏 pill 扩展：waiting>0 时文案带「等待同步 N 条」（优先级修正：error > syncing > waiting > connected，避免既有「失败被 syncing 覆盖」），失败>0 时 tooltip/文案带「部分失败」；SyncTab 四态摘要卡（X2，数据来自 stats + 既有 syncStatus.last_sync/SyncReport 口径）；「本地已保存」语义= articles 状态已事务化落库（TASK-108 交付），在摘要卡说明文案体现（不新增每卡状态徽标）。③测试——Rust：迁移测试（新列存在+旧库升级）、push 失败标记/成功 prune 后 failed 归零、stats 命令返回（cargo test 由 CI 执行）；前端 t116-* ≥5 条（api 形态、pill 优先级纯函数、SyncTab 摘要渲染源级/SSR、无队列不劣化 X3）。既有断言零弱化（侧栏 pill 四分支既有断言如有冲突，更新附理由）。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：.workflow-kit/docs/UI-CONTRACT-TASK-116-SYNC-FOUR-STATES.md
**界面检查**：X1.sidebar-waiting-count, X2.settings-four-states, X3.no-fake-states
**修改范围**：src-tauri/src, src, tools/frontend-regression.mjs

## 验收标准

- ① 迁移落地：sync_queue 增 attempts/last_error，旧库升级兼容（cargo 测试，CI 执行）
- ② push 失败标记/成功 prune 语义正确，stats 命令返回准确（cargo 测试，CI 执行）
- ③ 侧栏 pill 优先级修正与等待计数（X1）
- ④ SyncTab 四态摘要卡（X2/X3，如实口径）
- ⑤ 门禁全绿：frontend（t116-* 新增）、lint/build/cargo_fmt 不回退；cargo test/clippy 由 CI 承担且必须绿（DEC-local-cargo-gate-20261005）
- ⑥ 独立审查（全新子代理）PASS findings=0；ui_change=true 需 UI 取证说明

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；dev@TASK-115 后基线：frontend 全绿、CI rust job 全绿。本卡为新增能力（队列状态暴露+UI 呈现），属行为变化（owner 指示按审计三阶段推进）。
- 基线证据：.workflow-kit/docs/AUDIT-20261005-core-consistency.md
- 需求决定：DEC-refactor-roadmap-20261005
- 补充：Rust 迁移/push 标记/stats 测试（CI 执行）+ 前端 t116-* 断言；新能力需成对断言；验证：cargo_fmt, frontend
- 保留：既有 frontend/cargo fmt/lint/build 断言；不回退证据；cargo 由 CI 承担（DEC-local-cargo-gate-20261005）；验证：cargo_fmt, lint, build, frontend

## 执行与恢复

- 首次开始：2026-10-06T13:39:32.160643Z
- 原截止时间：2026-10-06T17:39:32.160643Z
- 当前截止时间：2026-10-06T17:39:32.160643Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 35 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-10-06T13:39:33.073406Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-10-06T14:15:23.051851Z：编码结果已记录，差异范围已核对：src-tauri/src/commands/sync.rs, src-tauri/src/db.rs, src-tauri/src/db/migrations.rs, src-tauri/src/db/sync_queue.rs, src-tauri/src/lib.rs, src-tauri/src/sync/greader_pull.rs, src-tauri/src/sync/phases.rs, src-tauri/src/sync/push.rs, src/components/Sidebar.tsx, src/components/settings/SyncTab.tsx, src/lib/api.ts, src/lib/syncPill.ts, src/store/slices/bootstrap.ts, src/store/slices/sync.ts, src/store/types.ts, tools/frontend-regression.mjs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-10-06T14:15:59.879232Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-10-06T14:38:37.480095Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-116.json)

- [RUN-1a3b64049652406a84d9c07fb925f872](../runs/RUN-1a3b64049652406a84d9c07fb925f872.json)
- [RUN-53b1b55fb36e4e6986f2b0934e1efd24](../runs/RUN-53b1b55fb36e4e6986f2b0934e1efd24.json)
- [RUN-57027f4be2354841ae440b52eb542265](../runs/RUN-57027f4be2354841ae440b52eb542265.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
