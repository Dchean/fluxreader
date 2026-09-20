<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-070 · 死代码/空壳集群删除：零生产调用代码清理与 SyncReport 字段收口（REQ-104）

**状态**：verified

**目标**：删除经 grep 验证零生产调用的死代码/空壳集群，确保无未落实的宣称能力（PI-无空壳）。逐项：① db/feeds.rs 的 Miniflux 兜底三查询 feeds_fetch_failed(:245)/feeds_origin_remote(:257)/feeds_fetch_failed_bound(:269) —— 生产仅被 db.rs:34-35 的 pub use 导出、无任何调用点，且审计判定『Miniflux 兜底路径未实现』（P2-9）：实际兜底由 reading-list pull（GReader）/未读+收藏（Fever）隐式覆盖；② ingestion.rs 旧版 refresh_feed(:330-407，非 staged，持锁跑 HTTP) —— 生产无调用点（命令层 commands/articles.rs:202 已改调 refresh_feed_staged），仅注释提及；③ greader.rs 的 GReaderClient::mark_all_read(:501)/subscribe(:511) —— 客户端级方法零调用点（命令层 mark_all_read 走 db 路径，订阅走 quick_add/edit_subscription）；④ db/sync_map.rs 被 SyncMatchMaps 取代的逐条查询（article_matches_remote_feed/article_id_by_url/article_has_pending_sync/set_folder_remote_id/feed_by_remote_id 等，仅测试引用）—— 按审计建议『统一删除或 #[cfg(test)] 下沉』处置；⑤ sync/mod.rs:29 SyncReport.fallback_entries 恒 0（全库无自增点，仅 phases.rs:90 与 commands/sync.rs:269 互相赋值）—— 删除字段及其赋值点、前端 SyncReport 类型字段与断言，并修订 sync/mod.rs 模块头注释里已不存在的『兜底』宣称；⑥ config_sync.rs:24 STATE_FILE_NAME 预留常量零引用；⑦ lib/api.ts:503 api.syncNow 前端零调用（P3-2 死接口）；⑧ AiEvent::Error 死变体（ai.rs:18 声明，生产从不构造，仅 ai.rs:212 测试构造；前端 'error' 分支因此不可达）—— 按 REQ-104『删或接通』选择删除变体与其测试，并同步清理不可达分支。非目标：不改任何仍被生产调用的函数行为；不动 merge_remote_status 一带同步合并语义（书面不变式，审计明确不建议动）；不拆除 db/sync_map.rs 中仍被 SyncMatchMaps 使用的函数；不改协议客户端与状态库路线。

**依赖**：TASK-069
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/src/db/feeds.rs, src-tauri/src/db.rs, src-tauri/src/db/sync_map.rs, src-tauri/src/ingestion.rs, src-tauri/src/greader.rs, src-tauri/src/sync/mod.rs, src-tauri/src/sync/phases.rs, src-tauri/src/commands/sync.rs, src-tauri/src/commands/ai.rs, src-tauri/src/config_sync.rs, src/lib/api.ts, src-tauri/tests/dedup_sync_e2e.rs, src-tauri/tests/sync_content_e2e.rs, src-tauri/tests/account_lifecycle_e2e.rs, src-tauri/tests/sync_e2e.rs, src-tauri/tests/mock_greader.rs, src/lib/types.ts, tools/frontend-regression.mjs

## 验收标准

- ① 删除项经 grep 全库验证零生产调用（含 #[cfg(test)] 之外的引用），删除后 cargo 无 dead_code/unused 告警新增
- ② 四门禁全绿：cargo test 通过数 ≥177 且 0 failed、9 ignored 不增；lint 0/0；build exit 0；frontend 通过数 ≥303
- ③ fallback_entries 与 api.syncNow 与 STATE_FILE_NAME 在 src/ 与 src-tauri/src/ 全库零残留（grep 为空）
- ④ AiEvent::Error 变体及其不可达前端分支一并清理，或在任务内说明改为接通并给出证据（二选一，如实记录）
- ⑤ sync/mod.rs 模块头注释的『兜底』宣称与实现一致（不再宣称不存在的 Miniflux 兜底）
- ⑥ 不引入新依赖；package.json/Cargo.toml/Cargo.lock 零改动；文本文件 LF；用户真实数据库不得写入
- ⑦ 被退役测试覆盖的能力若仍有效，须有等价替代覆盖；只退役确实以死代码为对象的断言

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；2026-09-20 基线（TASK-069 终版候选验证 RUN-5380e8e4，提交 452e0e2）：cargo test 177 passed / 0 failed / 9 ignored、npm run lint exit 0（0 warnings/0 errors）、npm run build exit 0、npm run test:frontend exit 0 且 303/303。本任务 behavior=change 的范围如实声明：删除的是零生产调用代码与其专属测试（覆盖对象消失），不改变任何仍被生产调用的行为。具体退役项：dedup_sync_e2e.rs 对 article_matches_remote_feed 的 7 处断言、sync_content_e2e.rs 对 article_id_by_url 的 2 处、account_lifecycle_e2e.rs 对 set_folder_remote_id 的 1 处、sync_e2e.rs 对 fallback_entries 的 2 处、ai.rs 文件内对 AiEvent::Error 的 1 处、tools/frontend-regression.mjs 对 fallback_entries 的 1 处。上述断言的对象均随本次删除消失；其中若某断言实际覆盖的仍是有效业务能力（例如跨源同文去重语义），须改测仍存活的生产入口而不是直接删除——退役与补充的取舍在实现时逐条判定并留证。其余全部既有断言（177+303 存量）逐字不动。
- 基线证据：.workflow-kit/tasks/runs/RUN-5380e8e45603487db6019495fdb61fb7.json
- 需求决定：DEC-992f64fd15714ca2a614fe66d5a144ec
- 退役：以死代码为唯一对象的测试断言：sync_e2e.rs 的 fallback_entries 断言、ai.rs 文件内 AiEvent::Error 序列化断言、frontend-regression.mjs 的 fallback_entries 断言；REQ-104 已确认删除这些零生产调用的空壳（fallback_entries 恒 0、AiEvent::Error 从不构造），其覆盖对象随删除消失，无业务对象可测；验证：对应需求已退役
- 适配：db/sync_map.rs 被 SyncMatchMaps 取代的逐条查询：其测试引用改为经 sync_match_maps 或对应生产入口验证同一行为；无法等价表达时按 retire 处置并记录；行为不变，仅测试引用了被取代的内部实现细节；按 GATES 优先改测入口而不是弱化契约；验证：cargo_test
- 补充：若某退役断言实际覆盖仍有效的业务能力（如跨源同文去重、读状态推送覆盖面），补测仍存活的生产入口；删除死代码不等于允许丢失仍有效的行为保护；验证：cargo_test
- 保留：其余全部既有断言与测试（177 条 Rust + 303 条前端）逐字不动；除死代码专属断言外契约不变；验证：cargo_test, lint, build, frontend

## 执行与恢复

- 首次开始：2026-09-20T03:38:00.554027Z
- 原截止时间：2026-09-20T07:38:00.554027Z
- 当前截止时间：2026-09-20T07:38:00.554027Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 50 分钟
- 已用修复轮：3
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-20T05:23:43.549532Z：开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-20T05:40:59.603440Z：编码结果已记录，差异范围已核对：src-tauri/src/ingestion.rs, src-tauri/src/sync/mod.rs, src-tauri/tests/mock_greader.rs, src-tauri/tests/sync_e2e.rs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-20T05:41:26.764684Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-20T06:19:36.796405Z：Review requires changes; inspect the findings；下一步：先核对已有文件及原始日志，再处理 review_failure；不要新建任务或重置预算
- 2026-09-20T06:19:54.925788Z：开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-20T06:25:55.138056Z：编码结果已记录，差异范围已核对：src-tauri/src/sync/mod.rs, src-tauri/tests/sync_e2e.rs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-20T06:26:24.908087Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-20T07:25:24.241972Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-070.json)

- [RUN-d81de29f2076460298a9f0731a6f871c](../runs/RUN-d81de29f2076460298a9f0731a6f871c.json)
- [RUN-49ba05fcc71e4ddca7298fc20cdb9a4c](../runs/RUN-49ba05fcc71e4ddca7298fc20cdb9a4c.json)
- [RUN-30ac731a5fab463799dafea3e5ddf8f1](../runs/RUN-30ac731a5fab463799dafea3e5ddf8f1.json)
- [RUN-d70e8abae2fc46988793a1041bda0c91](../runs/RUN-d70e8abae2fc46988793a1041bda0c91.json)
- [RUN-dafb37cd657a4921a6ae2eec959ad29f](../runs/RUN-dafb37cd657a4921a6ae2eec959ad29f.json)
- [RUN-15cf7e8442784a1a928d4adabfb3f4f8](../runs/RUN-15cf7e8442784a1a928d4adabfb3f4f8.json)
- [RUN-b0ee99bb02ad4420b1f485168107ecb1](../runs/RUN-b0ee99bb02ad4420b1f485168107ecb1.json)
- [RUN-ec17efa7238a4073a5cbc0bbd3267037](../runs/RUN-ec17efa7238a4073a5cbc0bbd3267037.json)
- [RUN-d9db4b82befc4db09752598c3a8a0f17](../runs/RUN-d9db4b82befc4db09752598c3a8a0f17.json)
- [RUN-23f7391a2bdf4490af6ea0cb4d606455](../runs/RUN-23f7391a2bdf4490af6ea0cb4d606455.json)
- [RUN-44d4ee12298e4fc6a9a862cbc2c1f93c](../runs/RUN-44d4ee12298e4fc6a9a862cbc2c1f93c.json)
- [RUN-c261b07ac58a4c9c8d3eb2d84af1892a](../runs/RUN-c261b07ac58a4c9c8d3eb2d84af1892a.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
