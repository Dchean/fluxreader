# TASK-045 实施报告：sync.rs 领域拆分

**性质**：模块拆分。**与 TASK-044 的关键区别**：本任务包含**不可省略的可见性放大**
（`private → pub(super)`），因为总入口与各实现模块之间存在真实的内部依赖。
按任务卡 acceptance 要求，放大清单在 §3 **逐项列出**，不冒充零改动。

**结论**：`cargo test` 120 passed / 0 failed / 23 ignored，与拆分前基线**逐项一致**。

## 1. 拆分结果

`src-tauri/src/sync.rs`（1352 行，已删除）→ `src-tauri/src/sync/`（8 文件）：

| 文件 | 行数 | 内容 |
| --- | --- | --- |
| `mod.rs` | 64 | 模块文档、`SyncReport`、子模块声明与重导出 |
| `credentials.rs` | 138 | 凭据：`read_credentials` / `Backend` + impl / `build_client` |
| `push.rs` | 193 | ① Push：本地状态变更 → 后端 |
| `subscriptions.rs` | 264 | 订阅关系：`push_feeds` / `pull_feeds` / `edit_remote_subscription` / `unsubscribe_remote` |
| `entries.rs` | 289 | 条目公共层：`item_*` / `merge_*` / `upsert_remote_entry` / `pull_entries` / `backfill_entry_content` |
| `greader_pull.rs` | 178 | GReader 条目拉取 |
| `fever_pull.rs` | 197 | Fever 条目拉取 |
| `phases.rs` | 133 | 总入口：`feeds_phase` / `states_phase` / `sync_now` / `sync_light` / `test_connection` |

最大单文件 **289 行**（验收 ≤400）。行数用可信口径（LF 字节数 = `splitlines()` = `ReadAllLines()`）。

> 拆分前 Pull 章节 883 行**超过 400**，故按职责细分为 `subscriptions` / `entries` /
> `greader_pull` / `fever_pull` 四块，而非照搬原章节边界。

## 2. 路径不变性

`mod.rs` 对**含 pub 项**的 4 个模块做 `pub use` 重导出
（`credentials` / `push` / `subscriptions` / `phases`），
使 `crate::sync::<fn>` 对既有调用点（`lib.rs` / `commands/*` / `scheduler` / `ingestion`）逐字不变。

`entries` / `greader_pull` / `fever_pull` **不含任何 pub 项**（只提供内部实现细节），
对它们做 `pub use` 会被 rustc 判为「glob 未重导出任何 pub 项」并告警，
故改用**模块内私有 glob**，其项仍可经 `use super::*` 被兄弟模块使用。

## 3. 可见性放大清单（19 项，逐项声明）

放大**不是随意的**：先做跨文件引用分析，只对**确实被别的文件引用**的项加 `pub(super)`。

| 文件 | 放大的项 | 谁在用 |
| --- | --- | --- |
| `credentials.rs` | `build_client` | push / subscriptions / phases |
| | `Backend` 的 8 个方法：`subscriptions` / `edit_subscription` / `tags` / `mark_read` / `mark_unread` / `mark_starred` / `mark_unstarred` / `quick_add` | push / subscriptions / phases |
| `push.rs` | `PUSH_LOCK` / `plan_push` / `exec_push` / `age_stale_queue` | phases |
| | `PushPlan` / `PushStatus` 及其字段 | phases 访问 `plan.status` / `plan.stars` |
| `subscriptions.rs` | `push_feeds` / `pull_feeds` | phases / entries |
| `entries.rs` | `item_numeric_id` / `merge_pulled_entry` / `pull_entries` | greader_pull / fever_pull / phases |
| `greader_pull.rs` | `pull_entries_greader` | entries |
| `fever_pull.rs` | `pull_entries_fever` | entries |

**未放大的**：`item_feed_id` / `item_url` / `item_content_html` / `item_published_at` /
`merge_remote_status` / `upsert_remote_entry` / `strip_html_text` / `backfill_entry_content` /
`fetch_stream_ids` / `reconcile_reader_state` / `reconcile_fever_state` / `collect_fever_items` 等
仍保持 `private`（只在各自模块内使用）。

> 可见性放大属**语义可见范围**变化，不是行为变化：`pub(super)` 只在 `sync` 模块内可见，
> 不对外暴露新 API，`crate::sync::` 的公开面与拆分前一致。

## 4. 「行为未变」的证明

写脚本把 `git show HEAD:src-tauri/src/sync.rs` 与新 `sync/*.rs` 按 `fn` 切分，
剥离注释、归一化空白后比对**函数体**与**签名**：

```
原函数数 40 / 新函数数 40
『函数体』逐字不一致的函数: 无 ✓
『签名』归一化后不一致    : build_client, exec_push, push_feeds, pull_feeds  ← 见下
```

对那 4 个签名做**字符级定位**，确认差异**仅为 rustfmt 补的尾随逗号**：

```
build_client  首处不同 @ 68: 原='...reqwest::Client)->Option<Backend>'
                             新='...reqwest::Client,)->Option<Backend>'
              仅差尾随逗号? True
```

**成因**：加 `pub(super) ` 后签名超出行宽上限，`cargo fmt` 把参数折行并补尾随逗号。
Rust 中函数参数列表的尾随逗号**无语义**。四例全部如此，函数体零差异。

## 5. 门禁结果（本会话实跑，重定向后读退出码）

| 门禁 | 结果 |
| --- | --- |
| `cargo build` | exit 0（唯一 warning 为既有的 linker 提示，TASK-044 时同样存在） |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy --all-targets -- -D warnings` | exit 0，Finished |
| `cargo test` | exit 0，**120 passed / 0 failed / 23 ignored**（基线同为 120/0/23） |

日志存档：`TASK-045-cargo-test.log`、`TASK-045-cargo-clippy.log`。

## 6. 实施过程中的返工（留痕）

1. **`PushPlan` 字段可见性**：首轮编译报 2 处 `E0616`（`phases.rs` 访问 `plan.status` / `plan.stars`）。
   原实现只放大了类型本身，未放大**字段**——而 Rust 的字段可见性独立于类型可见性。
   补 `pub(super)` 到 `status` / `stars` 后通过。
2. **`use super::*` 未使用告警**：`credentials.rs` 是基础模块、不引用兄弟项，
   生成器却统一加了该行 → 1 处 `unused import`（clippy `-D warnings` 会拒绝）。已移除。
3. **3 处 glob 重导出告警**：`entries` / `greader_pull` / `fever_pull` 无 pub 项，
   `pub use` 触发「glob 未重导出任何 pub 项」→ 改为模块内私有 glob（见 §2）。

三次都由编译器/工具抓出。**前两次是我对 Rust 可见性规则的疏忽**：
类型可见 ≠ 字段可见；基础模块不需要兄弟导入。

## 7. 未做的部分（避免越界）

- 未拆 `ingestion.rs`（766 行）与 `config_sync.rs`（579 行）——独立后续任务。
- 未改任何函数逻辑、协议行为、错误文案、日志。
- 未动 `commands/`（TASK-044 产物）、`db/`、前端。
