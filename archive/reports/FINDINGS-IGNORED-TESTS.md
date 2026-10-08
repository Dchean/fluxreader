# 被 `#[ignore]` 掩盖的测试（TASK-054 发现与纠正）

- 发现于：2026-09-18（TASK-053 之后，复核「之前的重构是否落实」时顺带核出）
- 处理任务：TASK-054（RUN-e86a7063796948cb978f914c9a87e633）
- 授权：DEC-tombstone-and-ignored-tests-20260918

## 一句话结论

仓库共有 **23 个 `#[ignore]`**。其中 **14 个的理由（`spins a local mock server`）已被实证证伪**，
它们**本应受默认 `cargo test` 保护**却被静默跳过；另 **9 个理由成立**（真实外部依赖），保持忽略。

## 一、失效理由：为什么「因为要起 mock 所以不跑」站不住

14 个测试标注：

```rust
#[ignore = "spins a local mock server"]
```

但同目录 `sync_gap_repro_e2e.rs` 使用**同一个** `mod mock_greader`
（`MockGReader::start()` 绑定 `127.0.0.1:0` 随机端口），其 13 个测试**默认全部运行**，
且是基线 `cargo test` 122 passed 的组成部分。

**同一份 mock、同一个机制，一半跑一半不跑，而理由说「因为要起 mock 所以不跑」——理由与实际不符。**

`MockGReader` 绑定随机端口（`:0`），不占用固定端口、不依赖外部服务，**默认运行完全可行**。

## 二、机械计数（按属性行，不采信文档注释字样）

```text
stale (spins a local mock server) attribute count : 14
env-dependent attribute count                     : 9
TOTAL                                             : 23
```

`14 + 9 = 23`，与 `cargo test` 报出的 `ignored: 23` **精确闭合**。

> 口径提醒：`sync_gap_repro_e2e.rs` 头部**文档注释**里出现的 `#[ignore]` 字样是约定说明文字，不是属性，
> 不计入统计。曾有一次按文档字样粗数得出「8」的口径错误，已用 `l.strip().startswith("#[ignore")` 机械核对纠正。

## 三、14 个失效项（TASK-054 已去除）

| 文件 | 数量 | 测试 |
| --- | --- | --- |
| `sync_phases_e2e.rs` | 5 | `instant_push_only_pushes_and_drains_queue`、`instant_push_read_broadcasts_dup_entries`、`feeds_and_states_phases_run_independently`、`stale_remote_read_converges_via_full_reconcile`、`full_reconcile_backfills_missing_local_entries` |
| `sync_content_e2e.rs` | 6 | `miniflux_origin_feed_pulls_new_entries_in_light_sync`、`pending_local_read_wins_over_stale_remote_in_upsert`、`miniflux_enclosure_is_not_used_as_cover`、`miniflux_existing_entry_backfills_cover`、`miniflux_origin_pulls_historical_entries_regardless_of_published_at`、`light_sync_converges_stale_remote_read_via_unread_ids` |
| `sync_e2e.rs` | 1 | `miniflux_sync_end_to_end` |
| `account_lifecycle_e2e.rs` | 1 | `reconnect_other_account_no_mixing` |
| `ai_e2e.rs` | 1 | `ai_summarize_translate_and_cache_pipeline` |

去除后默认门禁：**136 passed / 0 failed / 9 ignored**（136 = 122 + 14；9 = 23 − 14）。

## 四、9 个理由成立的项（保持忽略，仅登记）

| 文件 | 数量 | 原理由 | 为何成立 |
| --- | --- | --- | --- |
| `fever_live_e2e.rs` | 3 | 需真实 Miniflux 测试账号 + 网络 | 打真实外部后端，无 mock |
| `fever_sync_live_e2e.rs` | 1 | 需真实 Miniflux 测试账号 + 网络 | 同上 |
| `greader_live_e2e.rs` | 3 | 需真实 Miniflux 测试账号 + 网络 | 同上 |
| `ingestion_e2e.rs` | 1 | requires local feed server on 127.0.0.1:8765 | 需外部固定端口服务，非进程内 mock |
| `scheduler_e2e.rs` | 1 | requires local feed server on 127.0.0.1:8765 | 同上 |

这些测试**确实**需要外部环境，保持 `#[ignore]` 是正确处置。若将来要纳入 CI，需 `ci=true` 授权与门禁脚本改动。

## 五、重要的后续发现：转正 ≠ 有保护

TASK-054 的捕获性验证（变异测试）发现：**执行覆盖恢复，不等于断言强度足够**。

| 测试 | 对应生产代码变异 | 是否失败 |
| --- | --- | --- |
| `miniflux_enclosure_is_not_used_as_cover` | `image_url` 改用 `enc_url` | **是（捕获）** |
| `miniflux_existing_entry_backfills_cover` | 删除 `entries.rs:143` 的 `backfill_entry_content` 调用 | **是（捕获）** |
| `pending_local_read_wins_over_stale_remote_in_upsert` | pending 守卫：删除包装 / 反转条件 | 否 |
| `full_reconcile_backfills_missing_local_entries` | `db/sync_map.rs` pending 查询去掉 `read/unread` | 否 |

**即 4 个代表用例中 2 个具备捕获力，2 个不具备。**

**根因举例（P1）**：mock 的 `edit-tag` 在 push 成功时会把远端翻成 `read=true`（`mock_greader.rs:494-495`），
于是 pull 读到的远端状态本就是 `read`——**pending 守卫在不在，结论都一样**，断言恒真。

**处置**：本任务授权为「只删 `#[ignore]`、不改断言」，故**未改断言**，而是如实上报并建议另立任务补强
（仅 P1 与 P4 需要补强）。

### 5.1 订正记录：一次被独立审查拦下的假阴性

本节初版把 `miniflux_existing_entry_backfills_cover` 也记为「未捕获」，**这是错的**。

- **根因**：我的探针脚本用硬编码缩进匹配源码行（搜索 8 空格），而实际调用在 `entries.rs:143` 是 **12 空格**；
  脚本的 `count != 1 → SKIP` 守卫**静默跳过**了该例，却仍把结果写成「未捕获」。
- **拦截**：独立审查（TASK-054 第 1 轮）按我报告里写的变异点自行复现，得到**硬失败**：
  `left: None, right: Some("https://img.example.com/backfill.jpg")`，判定 FAIL。
- **我随后自行复现确认**：修正缩进后变异 → exit 101 失败；还原 → exit 0 通过。
- **附带的第二个方法坑**：cargo 的 mtime 指纹可能继续使用**陈旧的 `app_lib` rlib**，
  还原后不重编译时的结果不可信。变异测试必须**确认出现 `Compiling app`** 才采信（独立审查同样独立踩到并提醒）。

**教训**：负面结论与正面结论**需要同等强度的复现证据**；探针必须**区分「未执行」与「执行后未捕获」**，
跳过要显式报错，不能与负面结果混同。本会话已第四次因「自建口径与权威事实不一致」出错，此为其中之一。
