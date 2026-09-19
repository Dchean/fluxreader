<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-060 · 补强 TASK-054 遗留的两个弱断言测试（P1/P4：缺陷复现必须导致测试失败）

**状态**：ready

**目标**：TASK-054 的捕获性验证发现 4 个代表用例中 2 个不具备捕获力：P1（sync_content_e2e.rs::pending_local_read_wins_over_stale_remote_in_upsert，变异 entries.rs:224 的 pending 守卫后仍通过——根因是 mock 的 edit-tag 在 push 成功时把远端条目翻成 read，pull 读回的状态与本地一致，断言无法区分）与 P4（sync_phases_e2e.rs::full_reconcile_backfills_missing_local_entries，变异 db/sync_map.rs 的 pending 查询去掉 read/unread 动作过滤后仍通过，独立审查复现）。本任务补强这两个测试，使「缺陷复现 ⇒ 测试失败」成立，并以变异验证留证：对每个测试施加其文档记载的变异，测试必须失败；还原变异后必须通过。不改任何产品代码语义，不动推送顺序/对账口径/入队条件/墓碑语义。

**依赖**：TASK-059
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/tests/sync_content_e2e.rs, src-tauri/tests/sync_phases_e2e.rs, src-tauri/tests/mock_greader.rs, tools/**

## 验收标准

- **P1 捕获力实证**：对 entries.rs:224 的 pending 守卫施加 TASK-054 记载的变异（删除包装 / 反转条件），`pending_local_read_wins_over_stale_remote_in_upsert` 必须失败；还原后必须通过。给出修前失败/修后通过的成对证据
- **P4 捕获力实证**：对 db/sync_map.rs 的 pending 查询去掉 read/unread 动作过滤，`full_reconcile_backfills_missing_local_entries` 必须失败；还原后必须通过。给出成对证据
- **无弱化**：除这两个测试的补强外，既有断言一行不改、不删除、不改名；既有 161 passed / 0 failed / 9 ignored 不回退（通过数可增不可减，ignored 不得增加）；前端 283/283 不回退
- **变异验证必须在冻结候选上重跑**：变异只用于取证，最终候选必须不含任何变异（给出还原后的 diff 证据与最终四门禁全绿记录）
- 四门禁全绿：cargo test、npm run lint、npm run build、npm run test:frontend
- 不引入新依赖；Cargo.toml/Cargo.lock 与 package.json 零改动
- 文本文件 LF 行尾；台账改动须在 begin 之前完成
- 用户真实数据库不得写入（与 TASK-059 相同约束）

## 测试适用性

- 既有行为：保持
- 原始基线：PASS；2026-09-19 基线（TASK-059 验收之后，提交 8f870f3）：`cargo test` **161 passed / 0 failed / 9 ignored**、`npm run lint` exit 0（0 warnings/0 errors）、`npm run build` exit 0、`npm run test:frontend` exit 0 且 **283/283**。**本任务不改任何行为**（behavior=preserve）：只补强两个既有测试的断言强度，使 TASK-054 记载的变异（P1：entries.rs:224 pending 守卫；P4：db/sync_map.rs pending 查询去动作过滤）能被捕获。**基线盲区须如实记录**：这两个测试今天仍然不具备捕获力（TASK-054 实测变异后仍通过），即「测试存在 ≠ 有保护」；P1 的根因是 mock 的 edit-tag 会把远端条目翻成已读，pull 读回状态与本地一致，断言无法区分。变异点在 TASK-059 中逐字未动，机理今天仍成立。
- 基线证据：.workflow-kit/tasks/evidence/baseline-2026-09-19-task060.md
- 需求决定：沿用既有行为，无新增业务取舍
- 适配：P1：sync_content_e2e.rs::pending_local_read_wins_over_stale_remote_in_upsert —— 补强断言，使 entries.rs:224 pending 守卫的变异（删除包装/反转条件）被捕获；TASK-054 捕获性验证实测该变异后测试仍通过（根因：mock 的 edit-tag 会把远端条目翻成已读，pull 读回状态与本地一致，断言无法区分）。「缺陷复现 ⇒ 测试失败」必须成立；验证：cargo_test
- 适配：P4：sync_phases_e2e.rs::full_reconcile_backfills_missing_local_entries —— 补强断言，使 db/sync_map.rs pending 查询去掉 read/unread 动作过滤的变异被捕获；TASK-054 实测未捕获且独立审查复现。测试场景未构造出「该过滤与否会产生不同结果」的数据形态，需补上；验证：cargo_test
- 保留：其余全部既有测试（含既有 161 passed 的 Rust 回归网与前端 283 条断言，逐字不动）；本任务只补强上述两个指定测试，不得弱化、删除或改名任何既有保护；验证：cargo_test, lint, build, frontend

## 执行与恢复

- 首次开始：None
- 原截止时间：None
- 当前截止时间：None
- 已用修复轮：0
- 阻塞：无
- 下一步：执行 start/next 获取可继续的动作

## 最近检查点


## 原始证据

[唯一状态记录](../items/TASK-060.json)


卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
