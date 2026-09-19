# TASK-060 实施报告：补强 P1/P4 两个弱断言测试

- 运行：RUN-91767d08e28d4534830e31b4fe3a37ed
- 任务：TASK-060（批次 BATCH-eb0897a2645c467eaf92824397f1b8e8）
- 授权：owner 2026-09-19 指令「清理旧残留，验收并批准立项」（batch 授权已入账 BATCH-eb0897a2）
- 日期：2026-09-19

---

## 1. 缺陷（TASK-054 捕获性验证的遗留结论）

TASK-054 对 4 个代表用例做变异验证：**P2、P3 具备捕获力，P1、P4 不具备**——
对产品代码施加记载的变异后，测试仍然通过，即「缺陷复现 ⇒ 测试不失败」。

| 编号 | 测试 | 记载的变异 | 根因（本轮实测确认） |
| --- | --- | --- | --- |
| P1 | `sync_content_e2e.rs::pending_local_read_wins_over_stale_remote_in_upsert` | `entries.rs:224` pending 守卫删除/反转 | ① 旧场景（同 feed 同 URL）走 `merge_remote_status` 而非 upsert；② mock 的 edit-tag 会把远端条目翻成已读，pull 读回状态与本地一致——守卫删掉也照样通过 |
| P4 | `sync_phases_e2e.rs::full_reconcile_backfills_missing_local_entries` | `db/sync_map.rs:91` pending 查询去掉 `read/unread` 动作 | 测试完全没有 pending 数据：队列空、推送成功，`pending_ids` 为空集，过滤与否不可观察 |

## 2. 补强方案

**共同前提**：让本地变更在 pull 时**仍处于 pending**——注入 `edit-tag → 500`
（`mock_greader.rs` 新增 `fail_edit_tag` + `set_fail_edit_tag()`）。推送失败 ⇒ 队列保留 ⇒
`pending_ids` 命中守卫；同时远端保持陈旧未读（服务端没有应用我们的标记）。

**P1（`sync_content_e2e.rs`，重写场景）**：同 feed 同 URL 绑定 + edit-tag 500。
pull 走 `merge_remote_status`（`entries.rs:76` 的 pending 守卫，**实测确认本场景的真正保护路径**）。
断言三段式：① 同步报告含注入的失败；② 队列保留（pending 前提成立）；③ 本地已读存活。

**P4（`sync_phases_e2e.rs`，在既有回填测试尾部追加段）**：对已绑定文章标读入队 + edit-tag 500，
full 对账后断言队列保留、本地已读存活。守卫消费的正是 `db/sync_map.rs:91` 的共享
pending 查询——该查询的变异由本段捕获。

产品代码（`src-tauri/src/**`）**零改动**（`git diff src-tauri/src/` 为空）。

## 3. 变异取证（成对证据）

| 变异 | 施加对象 | 结果（实测原文） | 还原后 |
| --- | --- | --- | --- |
| M-P1a：删除 `merge_remote_status` 的 pending 守卫（`entries.rs:76`） | P1 测试 | `panicked at tests\sync_content_e2e.rs:187:5: pending 守卫必须生效：本地已读不得被陈旧的远端未读覆盖` / `FAILED. 0 passed; 1 failed` | 通过 |
| M-P4：pending 查询去掉 `read/unread`（`db/sync_map.rs:91` → `IN ('star','unstar')`） | P1 测试 | `panicked at tests\sync_content_e2e.rs:187:5` / `FAILED` | 通过 |
| M-P4（同上） | P4 测试 | `panicked at tests\sync_phases_e2e.rs:346:9` / `FAILED. 0 passed; 1 failed` | 通过 |
| M-P1b：删除 `upsert_remote_entry` 的 pending 守卫（`entries.rs:224`） | P1 测试 | **仍通过**——见 §4：该分支在当前调用图不可达 | 通过（候选无变异） |

还原核验：`git diff --stat src-tauri/src/` 为空（产品源码与 HEAD 逐字一致）；
全部四门禁在无变异候选上重跑通过。

## 4. 发现：`upsert_remote_entry` 的 pending 守卫分支（`entries.rs:224`）不可达

**这是本轮最重要的发现，超出测试本身，需 owner 知悉。**

用探针实测：`merge_pulled_entry` 的 aid 计算是
`url_to_id[normalize(url)]` → `mf_id_to_article[entry_numeric_id]`（**均无 feed 过滤**）；
而 `upsert_remote_entry` 的 `existing` 用**完全相同的两个查找**。因此：

- 能进入 `upsert_remote_entry` 的条目，`merge` 的 aid 必然为 `None`
  （两个查找都没命中）⇒ `existing` 也必然为 `None`；
- `existing = Some(aid)`（`entries.rs:210-227`，含 `:224` 守卫）**从当前调用图不可达**。

推论：**`:224` 的 pending 守卫是死代码**，它保护的场景实际上由
`merge_remote_status`（`:76`）守卫承担。这也解释了 TASK-054 的困惑——
测试按名字（`..._in_upsert`）与行号（`:224`）去补，怎么补都补不到点上，
因为那条分支根本不会执行。

**处置**：本轮为测试补强任务，**不改产品代码**——删除死分支属于产品改动，
超出授权边界（且删除前应有 owner 对该结论的确认）。建议后续由 owner 决定：
确认死分支后删除（减代码），或由执行者给出可达性论证。**本轮变异取证如实记录：
对 `:224` 施加变异测试仍通过（分支不可达），不把它伪造成「已捕获」。**

## 5. 门禁（终版候选，无变异）

| 门禁 | 基线 | 实测 |
| --- | --- | --- |
| cargo | 161 passed / 0 failed / 9 ignored | **161 passed / 0 failed / 9 ignored**（161 = 既有 160 + 本任务补强不改测试数；两测试为重写/追加段，非新增用例） |
| lint | 0 warnings / 0 errors | **0 warnings / 0 errors** |
| build | exit 0 | **exit 0** |
| frontend | 283/283 | **283/283** |

改动文件（3，全部测试侧）：`sync_content_e2e.rs`（P1 场景重写）、
`sync_phases_e2e.rs`（P4 追加段）、`mock_greader.rs`（`fail_edit_tag` 注入）。

## 6. 未完成 / 限制

- **`:224` 死分支未删**（§4，产品改动超出本任务边界，待 owner 决定）。
- 变异取证只覆盖 TASK-054 记载的两种变异及其组合；未穷举其它变异形态
  （如反转守卫条件与删除守卫等价，未单列）。
- `stale_remote_read_converges_via_full_reconcile`（同文件既有测试）与本补强场景
  方向相反（远端已读收敛本地未读），两者共存验证了「pending 时本地赢、
  无 pending 时按对账收敛」的双向语义，本轮未新增断言（既有测试已覆盖另一方向）。