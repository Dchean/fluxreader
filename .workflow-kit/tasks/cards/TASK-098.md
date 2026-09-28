<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-098 · 前端 reload 异常处理收口：selectView/selectFeed 补 .catch（REQ-102）

**状态**：done

**目标**：收口 TASK-093 独立审查披露的范围外观察项（tmp/review-093.json summary）：src/store/slices/nav.ts 的 selectView/selectFeed 路径存在 `void get().reloadFromBackend()` 无 .catch——切视图/切源时后端调用失败（IPC 异常、后端错误）会产生 unhandled rejection 且无用户可见反馈，列表静默停留旧数据。F5 已在 TASK-093 修掉 toggleTimelineSort 的同款（.catch 失败提示由 reloadFromBackend 自身给出），本卡把剩余同款调用点全部收口：① grep src/ 全仓枚举所有 `void reloadFromBackend`（及等价的未处理 promise reload 调用）调用点，逐点补 .catch（失败可见性语义与 F5 同口径：吞掉的是 reloadFromBackend toast 之后的重抛，不新增第二套提示文案）；② 断言：对每个收口点加「后端拒绝时不产生 unhandledRejection」断言（可复用 TASK-093 的 (p3-f5) 模式与 unhandled 捕获装置），成对验证（临时去掉 .catch → 红 → 还原 → 绿，日志存 tmp/task-098/）；③ 既有断言无一削弱。

**依赖**：TASK-097
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src/**, tools/frontend-regression.mjs

## 验收标准

- ① src/ 下不再存在无 .catch 的 `void reloadFromBackend`（及等价未处理 reload promise）调用点；grep 清单与处理方式写进 summary
- ② 每个收口点有「后端拒绝时无 unhandledRejection」断言，成对验证红绿日志在案；失败提示语义与 F5 同口径（无第二套文案）
- ③ 门禁全绿且不回退：cargo test ≥219 / 0 failed / 9 ignored 不增；fmt/clippy/lint/build exit 0；frontend ≥ TASK-097 验证后的数目且全部通过

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；门禁全绿（cargo 219/0/9、frontend 412/412），但 selectView/selectFeed 的 void reloadFromBackend 无 .catch 为 TASK-093 审查披露的既有暴露（unhandled rejection、静默失败），当时不在授权范围未修。
- 基线证据：.workflow-kit/tasks/evidence/RUN-bb3cea5349834fc7a33650bb0c3ea9f4-review.json
- 需求决定：DEC-route-remaining-20260928
- 保留：前端既有断言；收口不改变成功路径行为；验证：frontend
- 补充：tools/frontend-regression.mjs：各收口点后端拒绝无 unhandledRejection 断言；静默失败需要成对断言；验证：frontend
- 保留：src-tauri 既有测试、fmt、clippy、lint、build；本卡不改 Rust，作不回退证据；验证：cargo_test, cargo_fmt, cargo_clippy, lint, build

## 执行与恢复

- 首次开始：2026-09-28T12:03:07.971870Z
- 原截止时间：2026-09-28T16:03:07.971870Z
- 当前截止时间：2026-09-28T16:03:07.971870Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 16 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-28T12:03:08.156613Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-28T12:19:46.658045Z：编码结果已记录，差异范围已核对：src/App.tsx, src/store/slices/nav.ts, tools/frontend-regression.mjs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-28T12:20:11.301949Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-28T12:29:24.847660Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-098.json)

- [RUN-44086bc99b8641a8b5d782d29844ee60](../runs/RUN-44086bc99b8641a8b5d782d29844ee60.json)
- [RUN-f274bed777c9457d8f1a778a2637f0e5](../runs/RUN-f274bed777c9457d8f1a778a2637f0e5.json)
- [RUN-31169407c9d046078c8839b846a8be66](../runs/RUN-31169407c9d046078c8839b846a8be66.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
