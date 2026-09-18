<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-054 · 恢复被失效 #[ignore] 理由掩盖的 14 个测试（先补网）

**状态**：done

**目标**：14 个测试标注 `#[ignore = "spins a local mock server"]`，即『该测试需要起一个本地 mock server，故默认不跑』。**该理由已被实证证伪**：同目录 `sync_gap_repro_e2e.rs` 使用**同一个 `mod mock_greader`**，其 13 个测试默认全部运行（本机 `cargo test` 基线 120 passed）。被忽略的 14 个测试实跑（`--ignored`）**全部通过、0 失败、合计约 0.4 秒**，其中包含真实保护（`full_reconcile_backfills_missing_local_entries`、`instant_push_only_pushes_and_drains_queue`、`stale_remote_read_converges_via_full_reconcile`、`pending_local_read_wins_over_stale_remote_in_upsert`、`light_sync_converges_stale_remote_read_via_unread_ids` 等）。本任务**去掉这 14 个失效的 `#[ignore]`**，让既存保护回到默认门禁，并把 baseline 的 ignored 计数由 23 更正为实际应有值。这是**恢复既存保护**，不是新增需求，也不是降低标准。**9 个理由成立的 `#[ignore]`（环境依赖型）保持不动**，仅在文档登记。计数口径经机械核对：14（失效）+ 9（环境依赖）= 23（当前 ignored 总数），闭合。

**依赖**：TASK-053
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：.workflow-kit/tasks/evidence/baseline-2026-09-18-task054.md, .workflow-kit/docs/FINDINGS-IGNORED-TESTS.md, src-tauri/tests/**

## 验收标准

- **14 个失效 `#[ignore]` 全部去除**，逐个列出（文件 + 行 + 测试名）并给出删除前后对照；不得遗漏、不得多删
- **9 个成立的 `#[ignore]` 逐个留档并保持原样**：列出文件、测试名、原理由，并说明其环境依赖为何真实存在（需真实账号/网络、或需 127.0.0.1:8765 外部服务）
- **默认门禁实跑**：`cargo test`（不带 `--ignored`）现在应覆盖这 14 个测试并**全部通过**；报告给出 passed/failed/ignored 的实际数值，且 `ignored` 必须**下降**（原 23）
- **证明捕获性（不得只展示转正后 PASS）**：对这 14 个测试中至少 3 个代表性用例，用**故障注入或断言反转**证明转正后确实会在回归时失败（录下修前失败输出再还原），杜绝『转正了一个永远不会失败的测试』
- **零断言改动**：给出 diff 证明只删除了 `#[ignore ...]` 属性行；测试函数体、断言、fixture 一律未动（可用 `git diff` 的逐行核对或等价机械核对）
- `src/` 生产代码零改动（`git diff --stat` 证明）；不引入新依赖；`Cargo.toml`/`Cargo.lock` 零改动
- 新增 `.workflow-kit/docs/FINDINGS-IGNORED-TESTS.md`：登记两类 `#[ignore]` 的完整清单与判定依据，写明『同类测试一半跑一半不跑』的成因与纠正，供后来者核对
- 其它门禁不得回退：`npm run lint`、`npm run build`、`npm run test:frontend` 全绿（前端 241/241）
- 文本文件必须 LF 行尾；行数以 `splitlines()` 口径报告（不得用 `Measure-Object -Line`）；台账改动须在 begin 之前完成
- 若任一被转正测试实际失败：**停止并报告**，不得改断言、不得重新加回 `#[ignore]`

## 测试适用性

- 既有行为：保持
- 原始基线：PASS；2026-09-18 基线（TASK-053 之后）：npm run lint exit 0（0 warnings/0 errors）、npm run build exit 0、npm run test:frontend exit 0 且 241/241、cargo test 122 passed / 0 failed / 23 ignored。**本任务不改变任何被验证行为**：它只让 14 个已存在、已通过、但被失效理由挡在默认门禁之外的测试重新运行。因此基线语义是『既有保护不削弱且覆盖面变宽』，`ignored` 由 23 降至 9 是**覆盖面扩大**，不是标准放宽（14 失效 + 9 环境依赖 = 23，经机械核对闭合）。判定依据与实测输出见 .workflow-kit/docs/FINDINGS-IGNORED-TESTS.md。
- 基线证据：.workflow-kit/tasks/evidence/baseline-2026-09-18-task054.md
- 需求决定：DEC-tombstone-and-ignored-tests-20260918
- 适配：14 个被失效理由忽略的 Rust e2e 测试（sync_phases_e2e / sync_content_e2e / sync_e2e / account_lifecycle_e2e / ai_e2e）；`spins a local mock server` 理由已被同一 mock 的 sync_gap_repro_e2e 默认运行所证伪；这 14 个测试本应受默认 cargo_test 保护。转正即恢复既存保护。；验证：cargo_test
- 保留：9 个环境依赖型 #[ignore] 测试（fever_live_e2e / fever_sync_live_e2e / greader_live_e2e / ingestion_e2e / scheduler_e2e）；其理由成立（需真实 Miniflux 账号+网络，或需 127.0.0.1:8765 外部服务），本轮明确保持忽略；仅登记不改运行行为。；验证：cargo_test
- 保留：sync_gap_repro_e2e.rs 全部 13 个默认运行测试（含 TASK-032/035/052/053 转正的同步保护）；这些是 REQ-002/003 的直接回归网，本任务不得改动其断言；它们是本任务『覆盖面变宽但不放松』的对照基准。；验证：cargo_test
- 保留：前端 241 项断言；本任务不改前端；该套件证明前端契约未被波及。；验证：frontend

## 执行与恢复

- 首次开始：2026-09-18T02:24:52.762881Z
- 原截止时间：2026-09-18T06:24:52.762881Z
- 当前截止时间：2026-09-18T06:24:52.762881Z
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-18T02:24:52.858747Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后调用 finish，再运行 verify
- 2026-09-18T02:36:25.255383Z：Worker result must match the complete worker-result contract；下一步：先核对已有文件及原始日志，再处理 protocol；不要新建任务或重置预算
- 2026-09-18T02:49:56.212111Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-18T03:12:35.518298Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-18T03:24:36.587320Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-18T03:24:54.225106Z：当前候选的测试与审查通过；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-054.json)

- [RUN-e86a7063796948cb978f914c9a87e633](../runs/RUN-e86a7063796948cb978f914c9a87e633.json)
- [RUN-cb8d3741da14429cb8b0782012c95c74](../runs/RUN-cb8d3741da14429cb8b0782012c95c74.json)
- [RUN-53b3dfa4d0cb4900a04c0e05f16e4a56](../runs/RUN-53b3dfa4d0cb4900a04c0e05f16e4a56.json)
- [RUN-2e20089a9bc8406f8cacfee05b1a24db](../runs/RUN-2e20089a9bc8406f8cacfee05b1a24db.json)
- [RUN-3104a608120d487dba1a48735927f8f7](../runs/RUN-3104a608120d487dba1a48735927f8f7.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
