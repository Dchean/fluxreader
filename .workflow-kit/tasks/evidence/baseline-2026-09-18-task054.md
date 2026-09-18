# 基线 · TASK-054（2026-09-18，TASK-053 之后）

## 门禁基线

| 门禁 | 命令 | 结果 |
| --- | --- | --- |
| lint | `npm run lint` | exit 0，0 warnings / 0 errors |
| build | `npm run build` | exit 0 |
| frontend | `npm run test:frontend` | exit 0，241/241 |
| cargo_test | `cargo test`（src-tauri） | 122 passed / 0 failed / **23 ignored** |

工作区：`git status --porcelain` 为空；`check` 全绿（tasks 25 / runs 145 / batches 8，0 errors / 0 warnings）。

## 本任务的核心事实：14 个 `#[ignore]` 的理由已被证伪

### 被证伪的理由

14 个测试标注：

```rust
#[ignore = "spins a local mock server"]
```

该理由声称「这些测试要起一个本地 mock server，所以默认不跑」。

**但这是站不住的**：同目录 `sync_gap_repro_e2e.rs` 使用**同一个** `mod mock_greader`（`MockGReader::start()` 绑定 `127.0.0.1:0` 随机端口），
其 13 个测试**默认全部运行**，是当前 `cargo test` 122 passed 的组成部分。

即：**同一份 mock、同一个机制，一半跑、一半不跑，而理由说"因为要起 mock 所以不跑"。**

### 实测：这 14 个测试全部通过

2026-09-18 实跑（`cargo test --test <file> -- --ignored`）：

| 文件 | 测试数 | 结果 | 耗时 |
| --- | --- | --- | --- |
| `sync_phases_e2e.rs` | 5 | 5 passed / 0 failed | 0.09s |
| `sync_content_e2e.rs` | 6 | 6 passed / 0 failed | 0.09s |
| `sync_e2e.rs` | 1 | 1 passed / 0 failed | 0.08s |
| `account_lifecycle_e2e.rs` | 1 | 1 passed / 0 failed | 0.03s |
| `ai_e2e.rs` | 1 | 1 passed / 0 failed | 0.02s |
| **合计** | **14** | **14 passed / 0 failed** | **~0.31s** |

被掩盖的真实保护举例：

- `full_reconcile_backfills_missing_local_entries`（全量对账回填本地缺失条目）
- `instant_push_only_pushes_and_drains_queue`（即时推送只推不拉、队列清空）
- `instant_push_read_broadcasts_dup_entries`（已读广播到重复条目）
- `stale_remote_read_converges_via_full_reconcile`（陈旧远端已读经全量对账收敛）
- `feeds_and_states_phases_run_independently`（两阶段独立可跑）
- `pending_local_read_wins_over_stale_remote_in_upsert`（本地待推已读胜过陈旧远端）
- `light_sync_converges_stale_remote_read_via_unread_ids`（light 路径收敛陈旧已读）
- `miniflux_origin_feed_pulls_new_entries_in_light_sync`
- `miniflux_enclosure_is_not_used_as_cover` / `miniflux_existing_entry_backfills_cover`
- `miniflux_origin_pulls_historical_entries_regardless_of_published_at`
- `miniflux_sync_end_to_end`
- `reconnect_other_account_no_mixing`（切换账号不串数据）
- `ai_summarize_translate_and_cache_pipeline`

## 9 个理由成立的 `#[ignore]`（保持不动）

| 文件 | 测试数 | 原理由 | 为何成立 |
| --- | --- | --- | --- |
| `fever_live_e2e.rs` | 3 | 需真实 Miniflux 测试账号 + 网络 | 打真实外部后端，无 mock |
| `fever_sync_live_e2e.rs` | 1 | 需真实 Miniflux 测试账号 + 网络 | 同上 |
| `greader_live_e2e.rs` | 3 | 需真实 Miniflux 测试账号 + 网络 | 同上 |
| `ingestion_e2e.rs` | 1 | requires local feed server on 127.0.0.1:8765 | 需外部固定端口服务，非本进程内 mock |
| `scheduler_e2e.rs` | 1 | requires local feed server on 127.0.0.1:8765 | 同上 |
| **合计** | **9** | | |

## 计数闭合并经机械核对

按**属性行**（`l.strip().startswith("#[ignore")`）机械统计，不采信文档注释中的字样：

```text
stale (spins a local mock server) attribute count : 14
env-dependent attribute count                     : 9
TOTAL                                             : 23
```

`14 + 9 = 23`，与 `cargo test` 报出的 `ignored: 23` **精确闭合**。

> 口径说明：`sync_gap_repro_e2e.rs` 头部文档注释里出现的 `#[ignore]` 字样是**约定说明文字**，不是属性，
> 不构成被忽略的测试——它在本统计中为零，正确。

## 判定结论

- 14 个 `#[ignore]` **理由失效**，转正后 `ignored` 应由 23 降至 **9**；这是**覆盖面扩大**，不是验收标准放宽。
- 本任务**不改变任何被验证行为**，也不修改任何断言；只删除失效属性行。
- 若转正后出现失败，那是真发现，必须停止并报告。
