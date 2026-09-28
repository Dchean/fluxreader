<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-099 · REQ-108 SQL 性能与迁移加固：M-5 列表索引失效、M-7 sync_queue 索引、M-9 全部已读往返、M-14 迁移回填事务性

**状态**：done

**目标**：实施 REQ-108（审计 route-M 四项，主控已用 EXPLAIN QUERY PLAN 独立复核过定性；DEC-task099-req108-20260928 解除暂缓）。逐项修法方向（实现时按现场核实调整，但每项必须有成对证据）：① M-5：src-tauri/src/db/articles.rs 列表查询以 COALESCE(published_at, fetched_at) 排序使 idx_articles_published 对 8 个查询变体全失效（SCAN + USE TEMP B-TREE FOR ORDER BY，TASK-094 的 EXPLAIN 报告再次确认）。修法候选：迁移把 published_at 为空的存量行用 fetched_at 回填非空（配合应用层写入兜底），使排序退化为纯 ORDER BY published_at 走索引；或等价的重构（union/两段查询），择优并附 EXPLAIN 前后对比。注意与「只填空」语义和 TASK-090 的 COALESCE(NULLIF(image_url,''),?) 口径区分（本项只动 published_at，不动 image_url）。② M-7：sync_queue 零索引（src-tauri/src/db/ 或 migrations 中建表处）——新迁移加覆盖查询所需索引（先 EXPLAIN 确认实际查询形态再定列），附前后对比。③ M-9：一次全部已读 = 1 SELECT + 1 UPDATE + 2N 次往返且全程持锁（mark_all_read 链路）——改为集合化操作（单条 UPDATE + 受影响的 feed 计数批量重算），减少往返与持锁时长；行为语义（含按范围+布局过滤，TASK-094 后的口径）逐字不变，往返计数前后对比入证据。④ M-14：migrations.rs v6→v7 的后置回填在事务外且以 user_version 当完成标记，半途中断永不重试——把回填纳入与版本推进同事务（或新增 settings 完成标记并在迁移事务内落标），并对「已停在 v7 但回填未完成」的存量库做幂等补跑（可重入、有测试）；补迁移中断复现测试（在回填中途模拟失败，断言下次启动会补完）。迁移一律遵循项目既有迁移框架（rusqlite_migrate，见 refresh_dedup 报错信息中的既有形态），新版本号顺延，不改动既有 v1..v7 已发布语义（M-14 的补跑是对存量库的幂等修复，不改 v7 定义本身则无需 bump，若必须 bump 说明理由）。前端 UI 与文案零变化。

**依赖**：TASK-098
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/src/**, src-tauri/tests/**

## 验收标准

- ① M-5：修后列表查询 EXPLAIN QUERY PLAN 不再有 SCAN + USE TEMP B-TREE FOR ORDER BY（改走索引）；至少覆盖「全部/按 feed/按分类」三个代表变体；EXPLAIN 前后对比全文入 evidence
- ② M-7：sync_queue 查询 EXPLAIN 前后对比（修前 SCAN、修后走新索引）；索引迁移有 up 测试
- ③ M-9：全部已读的往返计数前后对比（目标从 2N+2 量级降到常数级）；行为语义不变的等价性测试（与修前同一批数据产生相同的 is_read 结果与 feed 计数）
- ④ M-14：迁移中断复现测试（回填中途失败 → 重启补完且幂等）；v7 已完成库零重复工作
- ⑤ 成对证据归档 .workflow-kit/tasks/evidence/TASK-099-*：每项修前红/修后绿或前后对比；真实库规模（≥3000 行 articles 夹具）下的耗时对比
- ⑥ 门禁全绿且不回退：cargo test ≥ TASK-098 验证后的 219、0 failed、9 ignored 不增（新增用例除外）；fmt/clippy/lint/build exit 0；frontend ≥ TASK-098 验证后的数目且全部通过

## 测试适用性

- 既有行为：保持
- 原始基线：PASS；门禁全绿（cargo 219/0/9、frontend 419/419），REQ-108 四项为性能/健壮性加固：行为语义保持不变（is_read 结果与 feed 计数等价、查询结果集相同），变化仅在执行计划、往返数与迁移可重入性。
- 基线证据：.workflow-kit/tasks/evidence/TASK-094-explain-query-plan.md
- 需求决定：DEC-task099-req108-20260928
- 保留：既有 cargo 测试（列表/全部已读/迁移/同步队列行为）；语义保持，修后必须全绿；验证：cargo_test
- 补充：Rust：EXPLAIN 计划断言（或前后对比脚本+输出归档）、往返计数断言、迁移中断复现测试；REQ-108 验收要求成对证据；验证：cargo_test
- 保留：fmt/clippy/lint/build/frontend；不回退证据；验证：cargo_fmt, cargo_clippy, lint, build, frontend

## 执行与恢复

- 首次开始：2026-09-28T12:31:41.417952Z
- 原截止时间：2026-09-28T16:31:41.417952Z
- 当前截止时间：2026-09-28T21:15:23.036912Z
- 时钟：按墙钟计：额度 480 分钟，写入阶段已用约 282 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-28T13:32:37.391899Z：2026-09-28 13:28Z 主控核对配额耗尽前子代理的半成品：仅 src-tauri/src/db/migrations.rs 有改动（+298/-17）——v14 迁移（published_at 存量兜底回填 + idx_articles_feed_published + idx_sync_queue_article/created）、M-14 的 ensure_url_norm_backfill（回填与 settings 标记同事务、cfg(test) 中断注入点、open() 改按标记补跑）、以及 3 个已完成单测（中断复现/存量库补跑/零重复工作）；第 4 个测试 v14_adds_indexes_and_backfills_missing_published_at 写到一半被打断（:551 unterminated string，cargo check E0765）。M-5 查询侧（articles.rs COALESCE 排序）、M-7 查询形态核对、M-9 集合化、EXPLAIN/往返计数证据、门禁与 worker-result 全部未做。本文件非变异态（无 m0-backup，改动自洽可续）。；下一步：新开实现子代理接手：先修复 migrations.rs 语法收尾并跑通单测，再完成 M-5/M-7/M-9、成对证据与门禁，写 worker-result；主控随后 diff --run、归档证据、finish、verify、新子代理独立审查
- 2026-09-28T17:13:54.942385Z：Invalid worker status/summary；下一步：按报错列出的漏报/多报文件修正 worker-result，再 unblock 后 begin；不要新建任务或重置预算
- 2026-09-28T17:15:40.574420Z：依据新决定追加预算；原始时钟与失败记录保留；下一步：先核对已有成果，再按原任务范围继续
- 2026-09-28T17:15:52.314485Z：阻塞已处置（protocol）：protocol 阻塞（Invalid worker status/summary）：worker-result 首版两处不合契约——status 用了 ok（应为 ready_for_verification）、validation_requests 用了对象数组（应为字符串数组）。已按 validate_worker_result 契约重写文件（9 键不变，内容为真实门禁数字）；diff 已核对 outside/protected 为空。预算已 extend 240min（DEC-d62afaed512c416cac7617f43d51adb4）。；下一步：begin 重新实现
- 2026-09-28T17:16:24.156395Z：开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-28T17:16:55.891837Z：编码结果已记录，差异范围已核对：src-tauri/src/commands/articles.rs, src-tauri/src/db.rs, src-tauri/src/db/articles.rs, src-tauri/src/db/auth_probe.rs, src-tauri/src/db/migrations.rs, src-tauri/src/db/sync_map.rs, src-tauri/tests/migration_test.rs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-28T17:18:54.133885Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-28T17:34:58.994707Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-099.json)

- [RUN-870073e646464480be80c09d01f854bf](../runs/RUN-870073e646464480be80c09d01f854bf.json)
- [RUN-51527a3d51f142f7afa23465e704744d](../runs/RUN-51527a3d51f142f7afa23465e704744d.json)
- [RUN-fc3b9da176274a80bf2b0271260d78c3](../runs/RUN-fc3b9da176274a80bf2b0271260d78c3.json)
- [RUN-ede5c9655af74d2a81303f4ce0063667](../runs/RUN-ede5c9655af74d2a81303f4ce0063667.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
