<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-096 · e2e 临时库唯一命名收口：共享 helper 消除 CI flaky 根因 + mock 线程健壮性（REQ-102 测试基建）

**状态**：done

**目标**：修 CI cargo test 偶发失败的根因（CI run #101 c088711 失败、#100 f3d0208 通过，两提交仅差日志文件；失败步 Run tests；本地 verify 同形态复现过）。根因已定位并实测（JOURNAL 2026-09-22T09:26:05Z）：cargo test 同一二进制的多个 #[test] 并发跑在同一进程，std::process::id() 相同，临时库名唯一性只靠时钟纳秒；Windows 时钟密集调用下精度不足（实测 1000 次相邻 as_nanos() 仅 350 个不同值），库名碰撞后两个测试互 remove_file/争用同一 SQLite 文件，db::open 报 "table folders already exists"。实测碰撞率：subsec_nanos 59/2000=2.95%、as_nanos 61/2000=3.05%（同等危险）；as_nanos + 进程内 AtomicU64 计数器 = 0/2000。修复：① 在 src-tauri/tests 下建一处共享 helper（tests/common/mod.rs，pub fn 返回进程内唯一的临时库路径：pid + as_nanos + AtomicU64 递增，保持既有 fluxreader_<base>_…db 命名风格与 remove_file 语义由调用方决定）；② 全部自带临时库命名的 e2e 测试文件改为调用该 helper（grep 核清现有个数，09-22 时为 19 个，此后可能新增），逐文件替换，不得改变各文件既有用例逻辑；③ 新增唯一性压力测试（多线程并发取名 N 轮断言无重复，锁住 helper 的不变量）；④ 顺带收口同属测试基建、09-21 登记备查的脆弱点：tests/refresh_dedup_e2e.rs:25 mock 服务器线程 stream.unwrap() 在连接出错时 panic 掉服务器线程——改为记录并继续/安全退出（不改变用例断言）。产品行为零变化（本卡只动 tests/）。

**依赖**：TASK-094
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/tests/**

## 验收标准

- ① 临时库唯一命名逻辑在 tests 下只有一处定义（共享 helper）；grep 确认测试文件中不再有 pid+时钟自拼库名（subsec_nanos/as_nanos 直拼清零）；helper 采用 pid + as_nanos + AtomicU64 计数器
- ② 唯一性压力测试入 Cargo 门禁：多线程并发取名（如 4 线程 × 2000 轮，Barrier 同刻采样）断言两两不同，修前实现（无计数器）下该测试确定性失败、修后通过
- ③ 既有全部 e2e 用例逻辑不变：仅库名生成方式替换；refresh_dedup_e2e 的 mock 服务器线程不再因连接错误 panic（连接错误被记录，不再传播 unwind）
- ④ 门禁全绿且不回退：cargo test 通过数 ≥ TASK-094 验证后的数目（新增用例除外）、0 failed、9 ignored 不增；fmt/clippy/lint/build exit 0；frontend ≥412 全部通过

## 测试适用性

- 既有行为：保持
- 原始基线：PASS；门禁与 CI 平时全绿（CI #99/#100 success），但 cargo test 在 Windows 下有实测 ~3% 概率的临时库命名碰撞（09-22 定位：时钟粒度），表现为建库期 "table folders already exists" panic；CI #101 与 2026-09-28 本地 verify 各撞中一次。09-21 还登记了 refresh_dedup_e2e mock 服务器线程 stream.unwrap() 脆弱点。
- 基线证据：.workflow-kit/tasks/evidence/TASK-095-baseline-flaky.md
- 需求决定：沿用既有行为，无新增业务取舍
- 保留：全部既有 e2e 用例与断言（仅库名生成方式替换）；测试基建修复不改变被测行为；验证：cargo_test
- 补充：tests/：共享唯一命名 helper + 多线程唯一性压力测试 + mock 服务器线程健壮性；flaky 根因需要结构性修复与不变量锁；验证：cargo_test
- 保留：fmt/clippy/lint/build/frontend；不回退证据；验证：cargo_fmt, cargo_clippy, lint, build, frontend

## 执行与恢复

- 首次开始：2026-09-28T10:36:13.300490Z
- 原截止时间：2026-09-28T14:36:13.300490Z
- 当前截止时间：2026-09-28T14:36:13.300490Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 20 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-28T10:36:13.498575Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-28T10:56:56.925153Z：编码结果已记录，差异范围已核对：src-tauri/tests/account_lifecycle_e2e.rs, src-tauri/tests/ai_e2e.rs, src-tauri/tests/common/mod.rs, src-tauri/tests/config_sync_e2e.rs, src-tauri/tests/cover_backfill_e2e.rs, src-tauri/tests/dedup_sync_e2e.rs, src-tauri/tests/dual_client_e2e.rs, src-tauri/tests/endpoint_autodetect_e2e.rs, src-tauri/tests/feed_edit_e2e.rs, src-tauri/tests/fever_sync_live_e2e.rs, src-tauri/tests/ingestion_e2e.rs, src-tauri/tests/migration_test.rs, src-tauri/tests/pull_cursor_e2e.rs, src-tauri/tests/refresh_dedup_e2e.rs, src-tauri/tests/regression_e2e.rs, src-tauri/tests/scheduler_e2e.rs, src-tauri/tests/staged_refresh_e2e.rs, src-tauri/tests/sync_content_e2e.rs, src-tauri/tests/sync_e2e.rs, src-tauri/tests/sync_gap_repro_e2e.rs, src-tauri/tests/sync_phases_e2e.rs, src-tauri/tests/unique_db_path_test.rs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-28T10:57:31.160901Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-28T11:08:43.428973Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-096.json)

- [RUN-f7ab6551d20a4e90b6b073153ceccb2b](../runs/RUN-f7ab6551d20a4e90b6b073153ceccb2b.json)
- [RUN-ccfdd25e7cb04cba8ae611ff62a82442](../runs/RUN-ccfdd25e7cb04cba8ae611ff62a82442.json)
- [RUN-e996d4e9298343baa02b73f4a5f76db7](../runs/RUN-e996d4e9298343baa02b73f4a5f76db7.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
