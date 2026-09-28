<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-097 · 同步增量窗口收口：greader 游标改取拉取起点 + mock_greader changed_at 语义全面核对（REQ-002）

**状态**：verified

**目标**：收口两条登记在案的同步增量窗口缺陷（JOURNAL 2026-09-24T05:06:31Z 与 2026-09-23T07:57Z，均建议合并为一卡评估）。① greader_pull.rs 游标语义：src-tauri/src/sync/greader_pull.rs:149 在本轮拉取全部成功后 set_last_sync_ts(Utc::now())（拉取结束墙钟）。id 列举发生在拉取过程内的某个时刻，凡 changed_at 落在「id 列举之后 ~ 拉取结束之前」的服务端变更不在本轮结果里，又因 changed_at < 游标被排除在下一轮增量之外，只能等全量对账补回——拉取耗时越长漏得越多。修法（保持既有基础设施）：在 id 列举开始前取本轮游标候选（pull 起点墙钟，或取本轮实际观察到的最大 changed_at，二者择一并注释论证），成功推进游标时写该值而非结束时刻；明确与既有「failures>0 不推进游标、下一轮重拉同一窗口」语义的组合正确性（起点游标 + 幂等合并 ⇒ 无漏无重）。注意核对增量过滤的比较方向（>= 还是 >）与游标回退/首次同步（last_sync_ts 为空取 0）的边界。② mock_greader.rs changed_at 语义全面核对：09-23 已修一处（sync_content_e2e 的 miniflux_existing_entry_backfills_cover：mock 改正文时未同步前移 changed_at，与真实 Miniflux「重新抓取会更新 crawl/change 时间」语义不符，导致与增量窗口赛跑的假 flaky）。任务：通读 mock_greader.rs 与所有引用它的测试，枚举「mock 修改了条目内容/状态但未更新 changed_at」的全部同类点，逐个对照真实 GReader/Miniflux 服务端语义订正 mock（真实服务端改内容必更新 changed_at），并在 mock 顶部注释写明语义契约；每处订正附说明（为什么该用例的真实服务端行为会更新 changed_at，或该用例确实不涉及变更故无需改）。不改产品同步逻辑的其他部分；Fever/pull_cursor 等既有游标测试语义不动（除受①影响的断言外）。

**依赖**：TASK-096
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/src/**, src-tauri/tests/**

## 验收标准

- ① 游标推进值不再取拉取结束墙钟：拉取成功时写入的游标 ≤ id 列举时刻；有测试锁定「拉取期间服务端变更的条目在下一轮增量被拉回」（修前失败：结束墙钟游标下该条目被跳过；修后通过）
- ② failures>0 时不推进游标的既有语义保持并有断言；首次同步（游标 0）与边界比较方向经注释论证无漏
- ③ mock_greader changed_at 语义核对全覆盖：所有「改内容/状态不改 changed_at」的同类点已订正或在注释中论证无需订正；mock 顶部有语义契约注释；既有断言无一削弱
- ④ 门禁全绿且不回退：cargo test 通过数 ≥ TASK-096 验证后的 218、0 failed、9 ignored 不增；fmt/clippy/lint/build exit 0；frontend ≥412 全部通过

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：FAIL；门禁全绿（cargo 218/0/9），但游标取拉取结束墙钟的漏拉窗口真实存在（JOURNAL 09-24 登记待办）；mock_greader 已发现一处语义不符并修（09-23），同类点未全面核对。
- 基线证据：.workflow-kit/tasks/evidence/TASK-095-baseline-flaky.md
- 需求决定：DEC-route-remaining-20260928
- 保留：既有同步/游标测试（pull_cursor_e2e 等）；游标语义修正后既有行为兼容；验证：cargo_test
- 补充：Rust：拉取期间变更的条目下一轮被拉回的成对测试 + mock changed_at 订正；增量窗口正确性需要修前复现；验证：cargo_test
- 保留：fmt/clippy/lint/build/frontend；不回退证据；验证：cargo_fmt, cargo_clippy, lint, build, frontend

## 执行与恢复

- 首次开始：2026-09-28T11:10:54.519594Z
- 原截止时间：2026-09-28T15:10:54.519594Z
- 当前截止时间：2026-09-28T15:10:54.519594Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 37 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-28T11:10:54.715166Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-28T11:47:55.936352Z：编码结果已记录，差异范围已核对：src-tauri/src/sync/greader_pull.rs, src-tauri/src/sync/mod.rs, src-tauri/tests/dual_client_e2e.rs, src-tauri/tests/mock_greader.rs, src-tauri/tests/pull_window_e2e.rs, src-tauri/tests/sync_content_e2e.rs, src-tauri/tests/sync_e2e.rs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-28T11:48:59.923199Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-28T12:01:42.125876Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-097.json)

- [RUN-6e3fd167cb89415aa92d9e58fd9caa76](../runs/RUN-6e3fd167cb89415aa92d9e58fd9caa76.json)
- [RUN-89d6b3893b924afd9ee3122eb55e3c0a](../runs/RUN-89d6b3893b924afd9ee3122eb55e3c0a.json)
- [RUN-97317332f6524f75a6fcfaa286b4161e](../runs/RUN-97317332f6524f75a6fcfaa286b4161e.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
