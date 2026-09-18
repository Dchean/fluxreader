<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-055 · 修 subscriptions.rs 墓碑误清除：已删订阅被 pull 复活（P1）

**状态**：verified

**目标**：`sync/subscriptions.rs:46-49` 在 `unsubscribe_remote` **返回成功时清除删除墓碑**，但『成功』的判据是 `greader.rs:462 post_form_text` 的 `resp.status().is_success()`——**只要 HTTP 2xx 即视为『远端已确认退订』**。GReader 的 `subscription/edit` 端点在 token 失效、权限不足或 `s=feed/<id>` 目标不存在等情况下**可能返回 2xx + 错误体**。此时墓碑被清除，而远端仍列出该订阅，下次 `feeds_phase` 的 pull 分支（`:204-207` 只用墓碑挡复活）便**把已删除的订阅重新建回本地**——用户现象：『删掉的订阅自己回来了』。本任务把这个误判的清除点在**源头**去掉，只保留 `:201-202` 那条**有证据支撑**的清除条件（远端订阅列表确认已不含该 URL 时才清墓碑）。

**依赖**：TASK-054
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：.workflow-kit/tasks/evidence/baseline-2026-09-18-task055.md, src-tauri/src/**, src-tauri/tests/**

## 验收标准

- **复现测试先证缺陷成立**：新增一个测试，注入『退订请求返回 2xx 但远端保留该订阅』（扩展现有 mock：给 `mock_greader.rs` 加一个故障注入开关，仿照既有 `fail_stream_ids` 的写法），断言修复前**已删订阅被复活**（该断言在修复前必须失败/或写成修复后必须通过且给出修复前失败的证据）。报告中给出修复前的实际失败输出
- **修复后测试转正必过**：同一测试在修复后必须通过，且必须**证明其捕获性**（把修复回退后测试确实失败——用变异测试给出证据）
- **墓碑清除语义收口到唯一有证据的判据**：`unsubscribe_remote` 不再清除墓碑；墓碑只由 `:201-202`『远端订阅列表已不含该 URL』清除。报告中说明该改动对 A-1 既有测试 `deleted_feed_stays_deleted_and_unsubscribes` 的影响并给出其通过证据（该测试第 ③ 段本就断言『远端不再列出后同步不复活』，应仍然成立）
- **核对目录墓碑是否有同类误判**：阅读 `:172-178` 与相关命令路径，在报告中明确回答『目录墓碑的清除判据是否同样可能把 2xx 当远端确认』，给出代码依据；若存在同类问题，**停下报告**而不是顺手扩大改动范围
- **逐一排查其它『把 2xx/Ok 当远端确认』的清墓碑点**：全仓搜索 `remove_feed_tombstone` / `remove_folder_tombstone` 的调用点，逐个说明判据是否可靠
- 既有 Rust 测试不得回退：`cargo test` 通过（基线 **136 passed / 0 failed / 9 ignored**，即 TASK-054 之后的值；通过数可增不可减，ignored 不得增加）
- 四门禁全绿：`cargo test`、`npm run lint`、`npm run build`、`npm run test:frontend`（前端 241/241）
- 不引入新依赖；`Cargo.toml`/`Cargo.lock` 零改动；不改前端
- **不改同步协议语义**：本次只是收紧『远端确认』的判据，不改任何对外请求的内容与顺序
- 文本文件必须 LF 行尾；台账改动须在 begin 之前完成
- 行数以 `splitlines()` 口径报告（不得用 `Measure-Object -Line`）

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；2026-09-18 基线（TASK-054 之后）：npm run lint exit 0（0 warnings/0 errors）、npm run build exit 0、npm run test:frontend exit 0 且 241/241、cargo test **136 passed / 0 failed / 9 ignored**（TASK-054 已把 14 个失效 #[ignore] 转正；14 失效 + 9 环境依赖 = 23 与改动前 ignored 总数闭合）。**本任务有意改变行为**：`unsubscribe_remote` 不再在收到 2xx 时清除删除墓碑——『远端确认』的判据由『HTTP 2xx』收紧为『远端订阅列表实际不再包含该 URL』。这是**行为变更**（部分原本会清墓碑的情形今后不再清），已由 DEC-tombstone-and-ignored-tests-20260918 覆盖。A-1 的既有测试 `deleted_feed_stays_deleted_and_unsubscribes`（`sync_gap_repro_e2e.rs`）是本缺陷的直接回归网，其第 ③ 段断言『远端不再列出后不复活』在本次改动后应仍然成立。
- 基线证据：.workflow-kit/tasks/evidence/baseline-2026-09-18-task055.md
- 需求决定：DEC-tombstone-and-ignored-tests-20260918
- 补充：『退订 2xx 但远端保留 ⇒ 不得清墓碑、不得复活』的复现测试（含 mock 故障注入开关）；该缺陷此前无任何测试覆盖：既有 deleted_feed_stays_deleted_and_unsubscribes 只覆盖『远端确实移除』的正常路径，而缺陷恰在异常路径上。必须有可复跑且经变异证明具备捕获力的证据。；验证：cargo_test
- 适配：`unsubscribe_remote` 的返回值语义与调用方注释（commands/folders.rs:215 及 subscriptions.rs:31-32 的文档注释）；不再清墓碑后，返回值『远端是否确认』仅表示请求是否被接受，不再等价于『远端已删除』。注释必须与新语义一致，否则后来者会重蹈此误判。；验证：cargo_test
- 保留：A-1 既有回归网：`deleted_feed_stays_deleted_and_unsubscribes`（含墓碑写入、pull 不复活、退订动作送达、远端移除后不复活四段）；这些断言保护 A-1 的核心契约，本任务只收紧清墓碑判据，不得削弱其中任何一段；它们是『改动未破坏既有防复活保证』的直接证据。；验证：cargo_test
- 保留：TASK-054 转正的 14 个测试及其余全部 Rust 测试（合计 136 passed）；本任务改动 subscriptions.rs 的墓碑判据，须证明同步链路其余语义（push 顺序、对账口径、状态保护）未被波及。；验证：cargo_test
- 保留：前端 241 项断言；本任务不改前端；该套件证明前端契约未被波及。；验证：frontend

## 执行与恢复

- 首次开始：2026-09-18T03:25:40.644657Z
- 原截止时间：2026-09-18T07:25:40.644657Z
- 当前截止时间：2026-09-18T07:25:40.644657Z
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-18T03:25:40.751041Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后调用 finish，再运行 verify
- 2026-09-18T03:35:05.471779Z：Worker changed_files does not match the observed project diff；下一步：先核对已有文件及原始日志，再处理 protocol；不要新建任务或重置预算
- 2026-09-18T05:19:54.372744Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-18T05:32:16.469400Z：当前候选的测试与审查通过；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-055.json)

- [RUN-b7427f9111624a43b095cf8059657ff0](../runs/RUN-b7427f9111624a43b095cf8059657ff0.json)
- [RUN-576384bc45f04335be92267425a4555e](../runs/RUN-576384bc45f04335be92267425a4555e.json)
- [RUN-785b0d2f53124c0994ca98efdd598bac](../runs/RUN-785b0d2f53124c0994ca98efdd598bac.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
