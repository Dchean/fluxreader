# TASK-095 基线：CI cargo test 偶发失败的根因与实测（不可变证据快照）

本文件是 2026-09-28 立 TASK-095 时从 JOURNAL.md 摘录固化的根因证据快照（JOURNAL 是工具自写的可变记录，不能作为冻结基线引用；此处内容为逐字摘录 + 当日 CI 事实，此后不再修改）。

## 当日 CI 事实（2026-09-28）

- CI run #101（c088871）failure，失败 job = "Check & test Rust (src-tauri)"，失败 step = "Run tests"（cargo test）。
- 前一 run #100（f3d0208）success；f3d0208 → c088871 仅差一个 `.workflow-kit/notes` 日志提交，业务代码零差异。
- 同日本地 verify（RUN-fb141ccb）cargo test 失败形态：`tests/refresh_dedup_e2e.rs:64` panic "called `Result::unwrap()` on an `Err` value: AppError { code: \"migration\", ... SqliteFailure(... \"table folders already exists\") }"；同候选重跑（RUN-659123b4）全绿。

## 根因（摘自 JOURNAL 2026-09-22T09:26:05Z，逐字）

CI 随机失败根因已定位（此前只记为「flaky，未定位」）——不是时序问题，是临时库文件名碰撞。现象：20c6781 的 CI 里 cargo test 失败，失败用例 rename_folder_updates_name 与 update_feed_partial_and_empty_title_fallback 同在 tests/feed_edit_e2e.rs:18:31 panic（该行即 db::open(&tmp).unwrap()），同二进制 2 passed / 2 failed / 0.18s。机制：cargo test 把同一二进制的多个 #[test] 作为线程并发跑在一个进程内，std::process::id() 完全相同，唯一性只能来自时钟部分；而 Windows 时钟在密集调用下精度不足——实测 1000 次紧邻 as_nanos() 调用只产生 350 个不同值。临时库名撞车后两个测试互相 remove_file / 争用同一 SQLite 文件，db::open 失败并 panic。实测证据（rustc 编译的对照实验，2000 轮 × 4 线程模拟并发命名，每轮用 Barrier 强制同刻采样）：旧 subsec_nanos() 碰撞 59/2000 = 2.95%；现用 as_nanos() 碰撞 61/2000 = 3.05%——两者风险相当（先前「as_nanos 更安全」的判断被证伪），根因是时钟粒度而非纳秒字段的选择。改用「as_nanos + 进程内 AtomicU64 计数器」后碰撞 0/2000。影响范围：src-tauri/tests 下 19 个文件各自复制了同一套临时库命名（2 个用 subsec_nanos、17 个用 as_nanos），全部同样暴露；tests/fixtures 是唯一子目录，无共享测试 helper，Cargo.toml 无 dev-dependencies（无 tempfile/rand 可用）。修复应抽一处共享唯一后缀 helper（as_nanos + 计数器）供 19 个文件复用。

## 同文件登记的测试基建脆弱点（摘自 JOURNAL 2026-09-21T12:24:14Z）

mock 服务器线程里 `let mut stream = stream.unwrap();`（tests/refresh_dedup_e2e.rs:25）在连接出错时会 panic 掉服务器线程，导致后续请求失败。……属该测试基建的真实脆弱点，登记备查。

## 当年受阻记录（摘自 JOURNAL 2026-09-22T09:26:06Z）

用户已明确选择「立项修」（修 19 个测试文件的临时库命名），但当前 workflow-kit 引擎无法从 complete 阶段开启新任务……根因、实测碰撞率、影响范围与修复方案均已记录在案。

（2026-09-28 项目处于 delivery 阶段，障碍不再存在；owner 已以 DEC-route-remaining-20260928 批准本卡。）
