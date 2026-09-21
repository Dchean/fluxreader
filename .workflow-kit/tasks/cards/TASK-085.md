<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-085 · F4：markEntriesReadBulk 逐条 IPC 改为一次批量命令（收口 TASK-084）

**状态**：done

**目标**：TASK-084 的实现成果收口（该卡因任务记录合法修订与旧基线不收敛而取消，见其 dispositions）。内容不变：把 AUDIT P3[F4] 的逐条 IPC 收敛为一次批量命令 —— ① Rust `commands/articles.rs` 新增 `set_read_bulk(ids, read)` 并注册于 lib.rs；批量循环抽为 `apply_read_bulk`，命令与测试共用同一段真实代码，避免「把循环抄进测试」造成的伪证据；② `src/lib/api.ts` 新增 `setReadBulk`；③ `reader.ts` 的 `markEntriesReadBulk` 改单次调用。本轮同时订正审查 FINDING TASK-084-F1/F2/F3/F4：测试驱动真实函数（两种变异实测可致失败）、(L1) 断言钉精确 id 集合而非长度、契约变更由 owner 裁决 DEC-t084-replace-per-item-ipc-assertions-20260921 授权、清理实现期遗留脚本。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/src/commands/articles.rs, src-tauri/src/lib.rs, src/lib/api.ts, src/store/slices/reader.ts, tools/frontend-regression.mjs

## 验收标准

- ① `set_read_bulk` 已注册；批量路径经 `apply_read_bulk` 复用 `record_read_state`，前端 `markEntriesReadBulk` 只发**一次** IPC（以桩计数证明为 1 且 set_read 为 0）
- ② 语义等价有**真实**证据：测试调用命令实际使用的 `apply_read_bulk`，且对批量路径做变异（绕过 record_read_state / 跳过某 id 入队）测试必须失败（实测已满足）
- ③ (L1) 批量断言须钉**精确 id 集合**（101,103,104,105,201,301），不得只钉载荷长度
- ④ 契约变更（改写两条编码旧逐条 IPC 的前端断言）由 owner 裁决 DEC-t084-replace-per-item-ipc-assertions-20260921 授权，且除该两条外既有断言一字不动
- ⑤ 四门禁全绿且不回退：cargo test ≥206 且 0 failed、9 ignored 不增；lint 0/0；build exit 0；frontend ≥314/314
- ⑥ 无实现期遗留的临时/探测脚本残留在受保护证据目录

## 测试适用性

- 既有行为：保持
- 原始基线：PASS；2026-09-21 基线（TASK-082 验证 RUN-082fcac4，提交 4df55cb）：cargo test 202 passed / 0 failed / 9 ignored、lint 0 warnings / 0 errors、build exit 0、frontend 313/313。本任务 behavior=preserve：批量标读是对既有逐条标读的**等价重写**（同样的 is_read 结果、同样的 sync_queue 入队项、同样的离线补推），只减少 IPC 次数；故既有全部断言必须原样通过，既有契约不得改变。用户可见行为不变（标读结果与失败提示语义一致），无需 owner 裁决。
- 基线证据：.workflow-kit/tasks/runs/RUN-082fcac4ab9e47669883eeac24241e47.json
- 需求决定：DEC-t084-replace-per-item-ipc-assertions-20260921
- 保留：除 DEC-t084-replace-per-item-ipc-assertions-20260921 批准改写的 2 条外，全部既有 Rust 与前端断言；批量命令复用既有 record_read_state，逐条路径的既有测试必须原样通过，作为「等价重写」的判据。两条编码旧逐条 IPC 契约的前端断言（(f) 与 (L1)）与验收①「只发一次 IPC」逻辑上不可兼得，经 owner 裁决后改写计数口径，且强度不低于原断言。；验证：cargo_test, frontend
- 补充：Rust：apply_read_bulk 的等价性/逐 id 入队/空 ids/read=false 四条断言；前端：一次性 IPC + 精确 id 集合断言；核心主张是「一次 IPC 且语义等价」，必须对该主张本身取证。审查 FINDING TASK-084-F1 揭穿上一版证据是装饰性的（测试从不调用真实函数），现改为驱动真实函数并以变异实测确认可失败。；验证：cargo_test, frontend

## 执行与恢复

- 首次开始：2026-09-21T11:51:01.467571Z
- 原截止时间：2026-09-21T15:51:01.467571Z
- 当前截止时间：2026-09-21T15:51:01.467571Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 0 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-21T11:51:01.759544Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-21T11:51:37.586543Z：编码结果已记录，差异范围已核对：无文件变化；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-21T11:52:13.560039Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-21T12:03:48.269828Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-085.json)

- [RUN-51e74dad3b1e4df88a1c4e42206d4b1a](../runs/RUN-51e74dad3b1e4df88a1c4e42206d4b1a.json)
- [RUN-92059f91051f47888cf95db5be2acb76](../runs/RUN-92059f91051f47888cf95db5be2acb76.json)
- [RUN-54a5a3a171af43bc84ccff4beec537f4](../runs/RUN-54a5a3a171af43bc84ccff4beec537f4.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
