# TASK-054 实施报告：恢复被失效 `#[ignore]` 理由掩盖的 14 个测试

- 运行：RUN-e86a7063796948cb978f914c9a87e633
- 任务：TASK-054（批次 BATCH-201a8e98478f40e88c6f5a8dc54990b6）
- 授权：DEC-tombstone-and-ignored-tests-20260918（owner 选择「两项都做，先补网后修缺陷」）
- 日期：2026-09-18

---

## 1. 交付内容

删除 14 行失效的 `#[ignore = "spins a local mock server"]` 属性，使既存测试回到默认门禁。

`git diff --numstat`：

```text
0	1	src-tauri/tests/account_lifecycle_e2e.rs
0	1	src-tauri/tests/ai_e2e.rs
0	6	src-tauri/tests/sync_content_e2e.rs
0	1	src-tauri/tests/sync_e2e.rs
0	5	src-tauri/tests/sync_phases_e2e.rs
```

**全部为 0 增 1 删（合计 0 增 / 14 删）**，即只删属性行，未改任何测试体、断言或 fixture。

去除清单（文件 · 测试名）：

| 文件 | 测试名 |
| --- | --- |
| `sync_phases_e2e.rs` | `instant_push_only_pushes_and_drains_queue` |
| `sync_phases_e2e.rs` | `instant_push_read_broadcasts_dup_entries` |
| `sync_phases_e2e.rs` | `feeds_and_states_phases_run_independently` |
| `sync_phases_e2e.rs` | `stale_remote_read_converges_via_full_reconcile` |
| `sync_phases_e2e.rs` | `full_reconcile_backfills_missing_local_entries` |
| `sync_content_e2e.rs` | `miniflux_origin_feed_pulls_new_entries_in_light_sync` |
| `sync_content_e2e.rs` | `pending_local_read_wins_over_stale_remote_in_upsert` |
| `sync_content_e2e.rs` | `miniflux_enclosure_is_not_used_as_cover` |
| `sync_content_e2e.rs` | `miniflux_existing_entry_backfills_cover` |
| `sync_content_e2e.rs` | `miniflux_origin_pulls_historical_entries_regardless_of_published_at` |
| `sync_content_e2e.rs` | `light_sync_converges_stale_remote_read_via_unread_ids` |
| `sync_e2e.rs` | `miniflux_sync_end_to_end` |
| `account_lifecycle_e2e.rs` | `reconnect_other_account_no_mixing` |
| `ai_e2e.rs` | `ai_summarize_translate_and_cache_pipeline` |

9 个环境依赖型 `#[ignore]` **保持原样**：`fever_live_e2e.rs`(3)、`fever_sync_live_e2e.rs`(1)、`greader_live_e2e.rs`(3)、
`ingestion_e2e.rs`(1)、`scheduler_e2e.rs`(1)。清单与判定见 `.workflow-kit/docs/FINDINGS-IGNORED-TESTS.md`。

## 2. 门禁结果

| 门禁 | 命令 | 结果 |
| --- | --- | --- |
| cargo_test | `cargo test`（src-tauri） | **136 passed / 0 failed / 9 ignored** |
| lint | `npm run lint` | exit 0，0 warnings / 0 errors |
| build | `npm run build` | exit 0 |
| frontend | `npm run test:frontend` | exit 0，241/241 |

**计数闭合**：136 = 122（基线）+ 14（转正）；9 = 23（基线）− 14（转正）。两侧精确对上。

`src/` 生产代码**零改动**（`git diff --stat -- src-tauri/src` 为空），`Cargo.toml`/`Cargo.lock` 零改动。

## 3. 捕获性验证（验收项要求「不得只展示转正后 PASS」）

对代表性用例做**变异测试**：临时改坏生产代码 → 确认测试失败 → 逐字节还原（每例都验证 `sha256` 还原一致）。
变异**全部已还原**，`src/` 最终零改动。

> **本节已订正（见 §3.2）**：初版把 P3 记为「未捕获」是**错的**，根因是我的探针脚本写错了缩进
> （搜索 8 空格缩进，而实际调用在 `entries.rs:143` 是 12 空格），`count != 1` 的守卫**静默跳过**了该例，
> 却仍把结果写成「未捕获」。独立审查复现出该测试**确实失败**，我随后自行复现确认。下表为订正后的结论。

| # | 目标测试 | 变异点 | 结果 |
| --- | --- | --- | --- |
| P2 | `miniflux_enclosure_is_not_used_as_cover` | `entries.rs`：`image_url: first_image(&content_html)` → `image_url: enc_url.clone()` | **捕获**（exit 101，断言「封面必须是正文图，不是 enclosure URL」失败） |
| P3 | `miniflux_existing_entry_backfills_cover` | `entries.rs:143` 删除 `backfill_entry_content(conn, aid, e);` | **捕获**（exit 101，`left: None, right: Some("https://img.example.com/backfill.jpg")`） |
| P1 | `pending_local_read_wins_over_stale_remote_in_upsert` | `entries.rs:224` pending 守卫（删除包装 / 反转条件） | **未捕获**（两种变异均仍通过，且已强制重编译排除陈旧 rlib） |
| P4 | `full_reconcile_backfills_missing_local_entries` | `db/sync_map.rs` pending 查询去掉 `read/unread` 动作 | **未捕获**（独立审查复现） |

**订正后的结论：4 个代表用例中 2 个（P2、P3）具备捕获力，2 个（P1、P4）不具备。**

### 3.1 仍然成立的**负面结果**（P1/P4）

P1 与 P4 **未捕获**，我没有把「转正后通过」当作保护已恢复的证据，因此追查了原因。

**P1（本地待推已读保护）的机理**：该测试断言「本地已读在同步后仍为已读」。
但 mock 的 `edit-tag` 处理在 push 成功时**会真的把远端条目翻成 `read=true`**
（`mock_greader.rs:494-495`）。因此 pull 阶段读到的远端状态本身就是 `read`——
**无论 pending 守卫在不在，最终结果都是「本地已读」**，断言恒真。
我把守卫删除包装、以及反转条件后该测试**仍然通过**（均在 `Compiling app` 出现后复跑），
证实它当前**并未真正施加约束**。

**P4 同类**：变异后仍通过，说明对应断言在现有 fixture 下不构成约束。

### 3.2 本次的方法教训（作者失误，如实记录）

> **审查历史**：第 1 轮独立审查判定 **FAIL**，理由即本节所述的 P3 假阴性
> （完整报告见 `.workflow-kit/tasks/evidence/TASK-054-review-r1.json`，针对候选
> `90a9c96400bad77bbc57826139d515a651b692d8df7e847954341a273f457446`）。
> 订正本节内容后（`FINDINGS-IGNORED-TESTS.md` 属本任务快照根，订正使其候选摘要变化），
> 已按工作流**重新运行 verify** 产生新候选 `f023057068b77c313c4dc680ab77a736269d7594e3427606976fab9356ed0058`，
> 并提交第 2 轮独立审查。**交付物本身未被任何订正触及**（`git diff --numstat -- src-tauri/tests`
> 始终为 0 增 / 14 删，`src-tauri/src` 始终零改动）——变化的只有报告与文档的**表述准确性**。

| 失误 | 后果 | 教训 |
| --- | --- | --- |
| 探针脚本用**硬编码缩进**匹配源码行，且 `count != 1` 时**静默 SKIP** | P3 实际未执行却被写成「未捕获」，给出一条**假阴性结论**并传播到 3 份产物 | 探针必须**区分「未执行」与「执行后未捕获」**；跳过要显式报错，不能与负面结果混同 |
| 变异/还原后**未强制重编译** | cargo 的 mtime 指纹可能继续用**陈旧的 `app_lib` rlib**，让还原后的结果不可信 | 变异测试必须**确认出现 `Compiling app`** 才采信结果（独立审查同样独立踩到并提醒了这一点） |
| 把「未捕获」当作**已验证事实**写入结论 | 下游据此建议「另立任务补强 `miniflux_existing_entry_backfills_cover`」，而该测试**本就具备捕获力**，无需补强 | 负面结论同样需要**同等强度的复现证据**，不能因为「反正是坏消息」就降低核对标准 |

**这是我本会话第四次因「自建口径/副本与权威事实不一致」而产出错误结论**（前三次：`changed_files` 声明错误、
门禁笔误 `npm build`、worker-result 漏必填字段）。本次因独立审查拦下，特此在报告中留痕。
**独立审查在此处的价值得到了直接验证**：它没有采信我的表述，而是按我写明的变异点自行复现，从而推翻了结论。

**结论（诚实表述）**：这 14 个测试**此前从未在默认门禁中运行**，其**通过是真实的**（它们确实执行了真实代码路径、
无 panic、退出码 0），但**其中至少 4 个的断言强度不足以在对应缺陷复现时失败**。
因此本任务的交付应准确表述为：

> **恢复 14 个测试的执行覆盖**（已确证：执行、通过、计数闭合），
> **其中 P2、P3 已确证具备捕获力**；**P1、P4 的断言强度不足，已实证证伪**，
> 属**需要另行补强的既有测试弱点**，不在本任务「只删 `#[ignore]`、不改断言」的授权范围内。

我没有就此放宽任何标准：本任务授权明确要求「若去 ignore 后失败，停下来报告，而不是改断言去迁就」。
此处是**通过但未构成有效保护**的情形——同样如实上报，**不改断言**（授权不允许），
并建议**另立任务**补强 P1/P4 这两个测试的断言（使其在对应缺陷复现时确实失败）。

## 4. 遵守的边界

- 未修改任何测试断言、测试体或 fixture（`0 增 / 14 删` 为证）。
- 未修改 `src/` 生产代码；变异测试的改动已全部还原并校验哈希。
- 未引入新依赖；未改工作流脚本 / `binding.json`。
- 未动 9 个环境依赖型 `#[ignore]`。
- 文本文件全部 LF（实测 `src-tauri/tests/**.rs` 中 CRLF 文件数 = 0）。
- 行数/计数按机械口径报告（Python 属性行统计），未使用 `Measure-Object -Line`。

## 5. 遗留与建议

1. **建议另立任务**：补强 **P1（`pending_local_read_wins_over_stale_remote_in_upsert`）** 与
   **P4（`full_reconcile_backfills_missing_local_entries`）** 的断言强度，使「缺陷复现 ⇒ 测试失败」成立。
   这是一项**独立发现**，由本任务的捕获性验证产出。
   （初版曾把 P3 也列入此建议，经订正后 P3 已确证具备捕获力，**无需补强**。）
2. `subscriptions.rs:48` 墓碑缺陷为**同批次下一任务**（先补网、后修缺陷），本任务不含。
3. 环境依赖型 `#[ignore]` 若将来要纳入 CI，需 `ci=true` 授权与门禁脚本改动，本轮未做。
