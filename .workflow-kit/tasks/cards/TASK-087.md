<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-087 · REQ-105 验收①按「生产代码段」明确化 + 修 refresh_dedup_e2e mock 服务器线程 panic

**状态**：ready

**目标**：两件收尾事项。① **按 owner 2026-09-21 的裁决**（见 DEC-req105-production-segment-basis-20260921）把 BRIEF REQ-105 的验收① 由「拆分后无超过 800 行的生产单体」明确为「拆分后无超过 800 行的**生产代码段**（以该文件的 `#[cfg(test)]` 为界）」，使其与 REQ-105 描述所述目标（原 `ingestion.rs` 766 行生产单体）口径一致——否则任何给既有文件补单测的任务都会「违反」该验收，而补测试是受鼓励的。TASK-082 的审计节已记录实测依据（`db/articles.rs` 生产段 747 行、同文件单测 247 行）。② 修一处**实测发现的**测试基建脆弱点：`src-tauri/tests/refresh_dedup_e2e.rs:25` 的 mock 服务器线程用 `stream.unwrap()`，连接出错时会 panic 掉整个服务器线程，导致同测试内后续请求全部失败（症状会伪装成「抓取失败」而非「测试基建故障」）。改为忽略单个连接错误并继续 accept。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：.workflow-kit/tasks/BRIEF.json, src-tauri/tests/refresh_dedup_e2e.rs

## 验收标准

- ① BRIEF REQ-105 验收① 明确为按「生产代码段（以 #[cfg(test)] 为界）」衡量，措辞不再与「给既有文件补单测」冲突；其余两条验收逐字不变
- ② `refresh_dedup_e2e.rs` 的 mock 服务器不再因单个连接出错而 panic 掉服务器线程（改后仍能正常服务后续请求）
- ③ 该测试的两个既有断言与语义一行不动；测试名不变
- ④ 四门禁全绿且不回退：cargo test ≥206 且 0 failed、9 ignored 不增；lint 0/0；build exit 0；frontend ≥322/322
- ⑤ 变异取证（对②）：把 mock 服务器改回「首个连接出错即 panic/退出」的形态，必须有可观测的失败（记录实测结论）；若该路径在当前环境下不可稳定触发，则如实记录「未能构造出可复现的失败」，不得声称已取证

## 测试适用性

- 既有行为：保持
- 原始基线：PASS；2026-09-21 基线（TASK-086 验证 RUN-110241c0，提交 2d50c2b）：cargo test 206 passed / 0 failed / 9 ignored、lint 0 warnings / 0 errors、build exit 0、frontend 322/322。本任务 behavior=preserve：① 只改需求文档措辞（验收口径明确化，无代码行为变化）；② 只把测试基建的 unwrap 改为容错（不触碰被测行为与断言）。故既有全部断言必须原样通过。
- 基线证据：.workflow-kit/tasks/runs/RUN-110241c05b914c77af21d9eed6dc012c.json
- 需求决定：沿用既有行为，无新增业务取舍
- 保留：全部既有 206 条 Rust 断言（含 refresh_dedup_e2e 的 2 条）与 322 条前端断言；本卡不改被测行为：BRIEF 措辞变更不影响运行期；mock 服务器容错化只影响「服务器线程是否存活」，不改 feed 内容与断言语义。既有断言即零行为变化的判据。；验证：cargo_test, frontend

## 执行与恢复

- 首次开始：None
- 原截止时间：None
- 当前截止时间：None
- 时钟：未开始
- 已用修复轮：0
- 阻塞：无
- 下一步：执行 start/next 获取可继续的动作

## 最近检查点


## 原始证据

[唯一状态记录](../items/TASK-087.json)


卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
