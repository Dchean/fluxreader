# Batch 1/2 未入任务改动 · 独立审查报告（2026-09-24）

审查者：新开的子代理上下文（未参与 Batch 1/2 实现），只读；变异在 tmp/review-batch12/ 副本中进行。
范围：src/components/Timeline.tsx、src/lib/api.ts、src/store/slices/{ai,bootstrap,nav,reader}.ts、tools/frontend-regression.mjs；
后端 ArticleListArgs（删 rename_all）、mark_all_read / list_unread_ids_scoped 的 layout 参数、commands_extraction_tests.rs、
tests/sync_content_e2e.rs、tests/sync_gap_repro_e2e.rs。TASK-090/091 部分已另行审查验收，不在本次范围。

## 1. 总体结论：PASS（无 P0/P1/P2；5 条 P3 待跟进）

8 条声明逐条核实成立；门禁复跑全绿；13 个变异中 9 个使对应断言变红。

## 2. 发现

- **F1 · P3 · src/store/slices/bootstrap.ts:172（配合 nav.ts:152-161）**：loadMore 竞态守卫只比 scopeKey 与 articlesLimit===offset，不含排序方向。旧排序发出的 loadMore(offset=500) 若晚于切换排序后的重拉结果到达，且重拉后游标恰为 500，会被放行，把旧排序第 2 页接到新排序列表后。探针 tmp/review-batch12/probe-sort-race.mjs 输出：`after stale loadMore: entries 600 unique 500 duplicates 100 missing 100 cursor 600 exhausted true`。真机上后端 db.lock FIFO 且重拉要 4 个 IPC，后发先至很难出现，故 P3。建议：发起时记 timelineSort 或游标代际，返回时不一致即丢弃。
- **F2 · P3 · src/store/slices/reader.ts:396-404**：失败回滚是「再翻一次当前值」而非「恢复点击前值」。连点两次、第一次失败第二次成功时，UI 与 DB 不一致。探针 probe-rollback.mjs：`ipc: set_read(true), set_read(false)` → `UI isRead = true | DB is_read = false | unread count = 0 | toasts = 标读保存失败：db busy`。建议：仅当当前值仍等于乐观值时才写回原值，否则只 toast。
- **F3 · P3 · 平行路径未收口**：① reader.ts:321-333 toggleCurrentReadStatus 仍先弹乐观「已标为已读」、失败不回滚，与卡片路径删除假成功 toast 的口径相反；② ai.ts:231-252 toggleReaderTranslation 两条失败路径不处理 rawTranslatedIds，未套用「无半截译文即清标记」规则（标记按 id 被卡片与阅读器共用）。建议阅读器与卡片路径共用同一 helper。
- **F4 · P3 · tools/frontend-regression.mjs 覆盖缺口与记录不符**：日志称新增 +7 条，实际新增 9 个 checkNew（817、1586、1626、1629、1640、2212、2220、2224、2227 行），总数 348 正确；「每条都做过修前/修后成对验证」不成立——1626、2212、2227 三条在修前代码下也通过。未覆盖：收藏失败回滚（M2c）、onError 路径清标记（M6）、viewEntriesCache.clear（M4b）、Timeline 删除假成功 toast（M7）。
- **F5 · P3 · src/store/slices/nav.ts:159-160**：toggleTimelineSort 中 `void get().reloadFromBackend()` 无 .catch，而 reloadFromBackend toast 后会重抛 ⇒ unhandled rejection；也未像 selectFeed 那样判断 dataMode!=='tauri'；筛选视图（limit 100000 全集、本地排序）下重拉多余。
- **附注（范围外）**：src-tauri/src/sync/greader_pull.rs:147-149 的增量游标取拉取结束时的墙钟，拉取期间服务端变更的条目下一轮增量会漏掉，要等全量对账补回。建议单独立项。

## 3. 五项核对

| 项 | 结论 | 分析 |
| --- | --- | --- |
| requirements | PASS | 声明 1/2/3/4/6/8 与实现一致；rename_all=camelCase 确会丢 7/9 字段，删后前端全 snake_case，ArticleListArgs 全仓仅 listArticles/articleIndex 两处调用；两处 layout 子查询逐字相同、绑定顺序正确、与 resolveFeedLayout 等价（INNER JOIN 排除 folder_id 为 NULL 的源，与前端 folderRowsToCategories 口径一致）；markAllRead 全仓仅 nav.ts:179 一处调用且带 layout；声明 5 有 F1 潜在竞态；「有意不做」项未与实现矛盾 |
| regression | PASS | 门禁全绿（见第 5 节） |
| failure_paths | PASS（附 P3） | 卡片回滚、锚定打开 catch、全部已读失败 toast 均在；F2/F3/F5 待跟进 |
| maintainability | PASS（附 P3） | layout 子查询两处复制，建议抽 helper 保证入队集合=标读集合；覆盖缺口见 F4 |
| performance | PASS | layout 子查询不相关，只物化一次；切排序多一次全量重拉（4 IPC）可接受，筛选视图下多余（F5） |

## 4. 变异（判别力）

| 变异 | 改动 | 变红 |
| --- | --- | --- |
| M1 | markAllRead 去 layout | (f) 817，347/348 |
| M2 | reader 去已读回滚 | (p2b) 1629，347/348 |
| M2b | 卡片路径恢复修前（静默 catch） | 1629 红；1626 不红 |
| M2c | 去收藏回滚 | 无（348/348） |
| M3 | 锚定打开 catch 静默 | (p2c) 1640，347/348 |
| M4 | toggleTimelineSort 恢复修前 | (s7) 2220、2224，346/348；2227 不红 |
| M4b | 去 viewEntriesCache.clear | 无 |
| M5 | .catch 路径不清标记 | (l2) 1586，347/348 |
| M6 | onError 路径不清标记 | 无 |
| M6b | onError 路径无条件清标记 | 既有 (l2)「流错误路径…标记保持」，347/348 |
| M7 | 恢复 SocialCard 两处 toast | 无 |
| M8（Rust） | 加回 rename_all="camelCase" | article_list_args_deserializes_snake_case FAILED（left: None, right: Some(10)） |
| M9（Rust） | mark_all_read 的 layout 子句失效 | mark_all_read_layout_isolation FAILED（left: 2, right: 1） |

还原证明：审查前后对工作区已修改/未跟踪文件（排除 tmp/）做 sha256 快照，17 个业务/测试文件逐字节一致（NON_WORKFLOW_SHA_IDENTICAL）；差异仅在 .workflow-kit/（总控写入的 JOURNAL/RESUME 与新增 TASK-092-baseline-images.md）。

## 5. 命令与摘要行（照抄）

- `cd src-tauri && cargo test`：exit 0；28 行 test result 求和 213 passed / 0 failed / 9 ignored；首行 `test result: ok. 110 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.12s`
- `npm run test:frontend`：exit 0；`=== 既有回归 26/26 通过 ===` / `=== 新增 store 行为断言 322/322 通过 🆕 ===` / `=== 前端逻辑回归合计 348/348 通过 ===`
- `npm run lint`：exit 0（oxlint 无告警）；`npm run build`：exit 0，`✓ built in 130ms`；`cargo fmt --check`：exit 0
- 变异：`python tmp/review-batch12/mutate.py`；Rust 副本 `cargo test --lib -- article_list_args_deserializes_snake_case mark_all_read_layout_isolation` → `test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 108 filtered out`，exit 101（预期）
