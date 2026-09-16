<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-038 · 同步队列卫生：老化清理（A-8）+ 吞错日志（C-2）

**状态**：verified

**目标**：修复同步队列两个卫生缺口。(1) A-8：无远端绑定的队列项（远端无对应条目/源）永久滞留——plan_push 跳过后保留、sync_map pending_ids 长期保护、sync_queue 无 TTL；改为队列项写入时带 created_at，push/pull 规划时清理超过保留期（30 天）且仍无法绑定的项，并在清出时记 report.errors 供诊断。(2) C-2：take_sync_queue 的 DB 错误被 unwrap_or_default 当空队列静默吞掉（push_feeds、sync_local_feeds 三处）；改为记录 log::warn 并把错误并入 SyncReport.errors，避免订阅推送静默延迟无痕。新增回归测试覆盖老化清理。

**依赖**：TASK-037
**参考方案**：REF-001
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/src/**, src-tauri/tests/**

## 验收标准

- 队列表新增 created_at（迁移幂等，旧库兼容）
- 超过保留期且仍无绑定/无法绑定的队列项被清理并记 errors（新测试断言）
- 有效的待推项不受老化影响（保留期内正常推送，新测试断言）
- take_sync_queue 失败不再静默（错误进日志与 report）
- fmt 与 cargo test 全绿，既有同步契约不回归

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；2026-09-15 基线：cargo test 117/0。行为变更仅限：无法绑定的陈旧队列项会被老化清理（记 errors）；队列读取失败不再静默。均为 FINDINGS-SYNC-GAP.md A-8/C-2（REQ-002/003 范围），不影响正常推送路径。
- 基线证据：.workflow-kit/tasks/evidence/baseline-2026-09-15.md
- 需求决定：DEC-defect-batch2-20260915
- 保留：既有同步契约与队列推送测试；不得回归；验证：rust-test
- 补充：队列老化清理与保留期保护的回归测试；A-8 此前无覆盖；验证：rust-test

## 执行与恢复

- 首次开始：2026-09-15T16:06:22.839607Z
- 原截止时间：2026-09-15T20:06:22.839607Z
- 当前截止时间：2026-09-15T20:06:22.839607Z
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-15T16:06:22.910774Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后调用 finish，再运行 verify
- 2026-09-15T16:18:38.155248Z：编码结果已记录，差异范围已核对；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-15T16:18:45.134553Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-15T16:37:09.766066Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-15T16:37:29.281443Z：当前候选的测试与审查通过；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-038.json)

- [RUN-1297f47fb8b9490b9fe9febbe0b808ce](../runs/RUN-1297f47fb8b9490b9fe9febbe0b808ce.json)
- [RUN-c372106d1e4d49d992bfbe622c6cf233](../runs/RUN-c372106d1e4d49d992bfbe622c6cf233.json)
- [RUN-98cd5ef2c33943f6a4b54d9668b8ed62](../runs/RUN-98cd5ef2c33943f6a4b54d9668b8ed62.json)
- [RUN-986d757c1fac4eee923c1f6ba9852d78](../runs/RUN-986d757c1fac4eee923c1f6ba9852d78.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
