<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-087 · REQ-105 验收①按「生产代码段」明确化 + 修 refresh_dedup_e2e mock 服务器线程 panic

**状态**：cancelled

**目标**：两件收尾事项。① **按 owner 2026-09-21 的裁决**（DEC-req105-production-segment-basis-20260921）把 REQ-105 验收① 的口径**明确记录在案**：按「生产代码段」（以该文件的 `#[cfg(test)]` 为界）衡量，依据是该需求的目标对象本就是原 `ingestion.rs` 这一 766 行**生产**单体，而按全文件行数衡量会让任何「给既有文件补单测」的任务违反该验收。**不改动需求定义文件本身**——`.workflow-kit/tasks/**` 属 protected_paths，引擎禁止实现类任务改写需求定义，故以 decision 记录为判定依据（口径的最终采信留待 owner 在需要时通过 intake 层调整）。② 修一处**实测发现的**测试基建脆弱点：`refresh_dedup_e2e.rs` 的 mock 服务器线程原有两个会杀死整个服务器线程的出口（`stream.unwrap()` panic；`n == 0 { return }` 在正常断开时退出整个 accept 循环），会让后续请求全部失败、症状伪装成「抓取失败」。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：.workflow-kit/tasks/BRIEF.json, src-tauri/tests/refresh_dedup_e2e.rs

## 验收标准

- ① REQ-105 验收① 的口径（按「生产代码段」= 以该文件 `#[cfg(test)]` 为界）经 owner 裁决 DEC-req105-production-segment-basis-20260921 明确记录在案，且**不改动需求定义文件本身**——`.workflow-kit/tasks/**` 属本任务的 protected_paths，引擎据此禁止实现类任务改写需求，该口径以 decision 记录作为判定依据（见 dispositions 的范围订正说明）
- ② `refresh_dedup_e2e.rs` 的 mock 服务器不再因单个连接出错而 panic 掉服务器线程（改后仍能正常服务后续请求）
- ③ 该测试的两个既有断言与语义一行不动；测试名不变
- ④ 四门禁全绿且不回退：cargo test ≥206 且 0 failed、9 ignored 不增；lint 0/0；build exit 0；frontend ≥322/322
- ⑤ 变异取证（对②）：若该路径在当前环境下不可稳定触发，须如实记录「未能构造出可复现的失败」，不得声称已取证（本次实测结论：把 `n == 0` 改回旧的 `return` 后该测试仍 2 passed，即当前用例无法观测差异——已如实记录）

## 测试适用性

- 既有行为：保持
- 原始基线：PASS；2026-09-21 基线（TASK-086 验证 RUN-110241c0，提交 2d50c2b）：cargo test 206 passed / 0 failed / 9 ignored、lint 0 warnings / 0 errors、build exit 0、frontend 322/322。本任务 behavior=preserve：① 只改需求文档措辞（验收口径明确化，无代码行为变化）；② 只把测试基建的 unwrap 改为容错（不触碰被测行为与断言）。故既有全部断言必须原样通过。
- 基线证据：.workflow-kit/tasks/runs/RUN-110241c05b914c77af21d9eed6dc012c.json
- 需求决定：沿用既有行为，无新增业务取舍
- 保留：全部既有 206 条 Rust 断言（含 refresh_dedup_e2e 的 2 条）与 322 条前端断言；本卡不改被测行为：BRIEF 措辞变更不影响运行期；mock 服务器容错化只影响「服务器线程是否存活」，不改 feed 内容与断言语义。既有断言即零行为变化的判据。；验证：cargo_test, frontend

## 执行与恢复

- 首次开始：2026-09-22T00:56:59.028148Z
- 原截止时间：2026-09-22T04:56:59.028148Z
- 当前截止时间：2026-09-22T04:56:59.028148Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 3 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：如需同一目标，准备新的任务并引用本任务作为历史

## 最近检查点

- 2026-09-22T00:56:59.315254Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-22T00:59:40.751791Z：Out-of-scope changes: .workflow-kit/tasks/BRIEF.json (allowed: .workflow-kit/tasks/BRIEF.json, src-tauri/tests/refresh_dedup_e2e.rs)；下一步：核对 diff --run 列出的越界文件，撤销或用 unblock --note 说明归属后再 begin；不要新建任务或重置预算
- 2026-09-22T01:02:51.087952Z：阻塞已处置（scope）：范围订正说明已写入任务 dispositions；check 通过。；下一步：begin 重新实现
- 2026-09-22T01:03:10.201894Z：开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-22T01:03:35.427346Z：Out-of-scope changes: .workflow-kit/tasks/items/TASK-087.json (allowed: src-tauri/tests/refresh_dedup_e2e.rs)；下一步：核对 diff --run 列出的越界文件，撤销或用 unblock --note 说明归属后再 begin；不要新建任务或重置预算
- 2026-09-22T01:04:29.342247Z：任务已取消：任务记录在实现中经多次合法修订（撤回 BRIEF.json 的 allowed_paths、改写验收与 objective 以反映引擎对需求定义的保护），导致 repair 运行继承首次 begin 的旧基线、判 .workflow-kit/tasks/items/TASK-087.json 越界且无法收敛（TASK-074/076/084 同类）。按既有先例取消本卡并以新卡收口：BRIEF 措辞的口径结论已由 owner 裁决 DEC-req105-production-segment-basis-20260921 承载（需求定义文件本身按引擎治理保持不动），本卡余下的测试基建修复在工作树中，由新卡取快照承载。；下一步：如需同一目标，准备新的任务并引用本任务作为历史

## 原始证据

[唯一状态记录](../items/TASK-087.json)

- [RUN-44e68063e95b4929a847ec1287505523](../runs/RUN-44e68063e95b4929a847ec1287505523.json)
- [RUN-b547f9a6e9e84570af77483dee137c25](../runs/RUN-b547f9a6e9e84570af77483dee137c25.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
