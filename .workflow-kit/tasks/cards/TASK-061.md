<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-061 · 删除 upsert_remote_entry 不可达的 existing 守卫分支（sync/entries.rs 死代码清理）

**状态**：verified

**目标**：TASK-060 实证 upsert_remote_entry 的 existing 守卫分支（src-tauri/src/sync/entries.rs，if let Some(aid) = existing 块，含其中 :224 附近的 pending 守卫）从当前调用图不可达：merge_pulled_entry 的 aid 计算与 upsert_remote_entry 的 existing 计算使用完全相同的两个查找（mf_id_to_article[entry_numeric_id] 与 url_to_id[normalize(url)]，均无 feed 过滤），只有两个查找都未命中才会进入 upsert_remote_entry，故 existing 恒为 None；该分支保护的场景实际由 merge_remote_status（entries.rs:76 的 pending 守卫）承担。owner 已确认该死代码结论成立并批准立项删除（验收问答 2026-09-19，批次 BATCH-e4114a0450f2403c9fe42b2561eed78a）。本任务删除该不可达分支：existing 计算一并删除，else 主体提升为函数体（else 仍使用 maps，函数签名不变），更新函数文档注释并补一句调用图不变式注释（merge_pulled_entry 仅在两个查找都未命中时才调用本函数），防止未来改动重新引入重复绑定路径时不自知。产品行为零变化：TASK-060 补强后的 P1/P4 测试保护真实守卫路径，若不可达论证有误，删除将改变状态合并行为并被既有测试捕获。四门禁全绿，既有测试一条不改。

**依赖**：TASK-060
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/src/sync/entries.rs

## 验收标准

- 删除范围精确：diff 仅含 src-tauri/src/sync/entries.rs；删除 upsert_remote_entry 的 existing 计算（约 184-196 行）与不可达的 if let Some(aid) = existing 分支（约 210-227 行，含其中 pending 守卫），else 主体提升为函数体；maps 参数仍被 else 主体使用，函数签名与调用点不变；函数文档注释同步更新（原文描述的「URL 兜底合并需同源校验」语义已随死分支删除）并补一句调用图不变式注释（merge_pulled_entry 仅在两个查找都未命中时才调用本函数，TASK-060 §4 论证）
- 产品行为零变化：cargo test 161 passed / 0 failed / 9 ignored 不回退（通过数可增不可减，ignored 不得增加）；前端 283/283 不回退
- 四门禁全绿：cargo test、npm run lint、npm run build、npm run test:frontend
- 不引入新依赖；Cargo.toml/Cargo.lock 与 package.json 零改动
- 文本文件 LF 行尾；台账改动须在 begin 之前完成
- 用户真实数据库不得写入（与 TASK-059/060 相同约束）
- 实施报告须附删除前后 diff 与调用图论证核对（引用 TASK-060 报告 §4 并独立复核两个查找的一致性）；若任何测试失败或发现与不可达论证不符的迹象，立即停止并上报，不得修改测试或产品语义来强行通过

## 测试适用性

- 既有行为：保持
- 原始基线：PASS；2026-09-19 基线（TASK-060 终版候选验证 RUN-e6040878c41e4d729e86380954385c7d，提交 4c67a07；其后 HEAD 6fd382e 仅含工作流台账与脚本升级改动，产品代码 src-tauri/src 逐字一致）：cargo test 161 passed / 0 failed / 9 ignored、npm run lint exit 0（0 warnings/0 errors）、npm run build exit 0、npm run test:frontend exit 0 且 283/283。本任务为行为保持（behavior=preserve）：删除的 existing 分支经 TASK-060 探针实测从当前调用图不可达（对 :224 施加 TASK-054 记载变异测试仍通过，机理：merge 与 upsert 使用相同两个查找，能进入 upsert 的条目 existing 必为 None，见 TASK-060-report.md §4）。TASK-060 补强后的 P1/P4 测试（保护 merge_remote_status 的 pending 守卫与 db/sync_map.rs:91 的 pending 查询）构成删除安全网：若不可达论证有误、分支实际可达，删除将改变状态合并行为并被测试捕获。基线盲区如实记录：本任务不新增测试覆盖，不可达性依据是调用图论证加既有测试不回退，非新增专项断言。
- 基线证据：.workflow-kit/tasks/runs/RUN-e6040878c41e4d729e86380954385c7d.json
- 需求决定：沿用既有行为，无新增业务取舍
- 保留：全部既有测试（Rust 161 条，含 TASK-060 补强后的 P1/P4；前端 283 条断言）逐字不动；纯产品死代码删除，测试回归网是删除安全性的主要证据；弱化或修改测试都会使安全网失效；验证：cargo_test, lint, build, frontend

## 执行与恢复

- 首次开始：2026-09-19T07:53:22.291212Z
- 原截止时间：2026-09-19T11:53:22.291212Z
- 当前截止时间：2026-09-19T11:53:22.291212Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 11 分钟
- 已用修复轮：1
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-19T07:53:22.496000Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-19T08:03:45.028600Z：编码结果已记录，差异范围已核对：src-tauri/src/sync/entries.rs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-19T08:07:52.679053Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-19T08:16:54.565548Z：Review requires changes; inspect the findings；下一步：先核对已有文件及原始日志，再处理 review_failure；不要新建任务或重置预算
- 2026-09-19T08:17:56.031320Z：开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-19T08:18:38.493916Z：编码结果已记录，差异范围已核对：src-tauri/src/sync/entries.rs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-19T08:22:43.003415Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-19T08:26:52.373945Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-061.json)

- [RUN-b39a86a12204432f8380f48a175b906e](../runs/RUN-b39a86a12204432f8380f48a175b906e.json)
- [RUN-0b6ec07d02634da6a4d99a57243dd20f](../runs/RUN-0b6ec07d02634da6a4d99a57243dd20f.json)
- [RUN-8858f86c80d945e5929695fd25bbc2b8](../runs/RUN-8858f86c80d945e5929695fd25bbc2b8.json)
- [RUN-16d5c0b2e9b644c39daef65a06997581](../runs/RUN-16d5c0b2e9b644c39daef65a06997581.json)
- [RUN-52a9b197256c41c0bf409052cd17f8dd](../runs/RUN-52a9b197256c41c0bf409052cd17f8dd.json)
- [RUN-400ca1a601a34ece96d11463b76bf4b0](../runs/RUN-400ca1a601a34ece96d11463b76bf4b0.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
