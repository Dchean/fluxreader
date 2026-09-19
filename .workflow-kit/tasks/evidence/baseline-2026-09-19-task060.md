# 基线 · TASK-060（2026-09-19，TASK-059 验收之后）

## 门禁基线（本基线在 TASK-059 终版候选 22f7b858… / 提交 8f870f3 上实测）

| 门禁 | 命令 | 结果 |
| --- | --- | --- |
| cargo_test | `cargo test`（src-tauri） | **161 passed / 0 failed / 9 ignored** |
| lint | `npm run lint` | exit 0，0 warnings / 0 errors |
| build | `npm run build` | exit 0 |
| frontend | `npm run test:frontend` | exit 0，**283/283** |

## 本任务的靶子（TASK-054 捕获性验证的遗留结论）

TASK-054 对 4 个代表用例做了「变异后测试是否失败」的捕获力验证，结论：**P2、P3 具备捕获力，P1、P4 不具备**。

| 编号 | 测试 | 记载的变异 | TASK-054 实测 |
| --- | --- | --- | --- |
| P1 | `sync_content_e2e.rs::pending_local_read_wins_over_stale_remote_in_upsert` | `entries.rs:224` pending 守卫（删除包装 / 反转条件） | **未捕获**（两种变异均仍通过，已强制重编译排除陈旧 rlib） |
| P4 | `sync_phases_e2e.rs::full_reconcile_backfills_missing_local_entries` | `db/sync_map.rs` pending 查询去掉 `read/unread` 动作 | **未捕获**（独立审查复现） |

### P1 未捕获的机理（TASK-054 报告 §3.1）

该测试断言「本地已读在同步后仍为已读」，但 mock 的 `edit-tag` 处理在 push 成功时**会真的把远端
条目翻成 `read=true`**，因此 pull 阶段读到的远端状态本身就是 `read`——断言无法区分
「本地待推已读保护生效」与「远端本来就已读」。补强方向：让远端状态**不受本地 push 影响**
（或显式固定远端状态），使变异后断言真正失败。

### P4 未捕获的机理

`full_reconcile_backfills_missing_local_entries` 走的对账补拉路径在变异（pending 查询去掉
动作过滤）后行为恰好不变——测试场景没有构造出「该过滤与否会产生不同结果」的数据形态。

## 本任务的成功标准（详见任务卡 acceptance）

1. 对 P1 施加上述变异 ⇒ 测试**必须失败**；还原 ⇒ 通过。成对证据留档。
2. 对 P4 施加上述变异 ⇒ 测试**必须失败**；还原 ⇒ 通过。成对证据留档。
3. 除这两个测试的补强外，既有断言一行不改；161/0/9 与 283/283 不回退。
4. 变异只用于取证，最终候选不得含任何变异。

## 与本基线的差异说明

- 本基线相对 `baseline-2026-09-18-task059.md` 的变化全部来自 TASK-059（Endpoint 自动适配）：
  测试数 141 → 161、前端 280 → 283；与本任务靶子（P1/P4 两个测试）无重叠。
- P1/P4 的变异点（`entries.rs`、`db/sync_map.rs`）在 TASK-059 中**逐字未动**，
  TASK-054 时代记录的未捕获机理在今天仍然成立。
