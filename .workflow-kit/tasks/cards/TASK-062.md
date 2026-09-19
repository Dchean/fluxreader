<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-062 · 修复 OPML 导入每条根级订阅重复新建「导入」文件夹（REQ-101/N1）

**状态**：verified

**目标**：AUDIT-20260919-v2.md 新发现 N1（REQ-101，三路体检后端排查确认）：src-tauri/src/commands/opml.rs 的 opml_import 在 f.folder==None 分支每条都无条件 db::create_folder("导入")，folders.name 无 UNIQUE 约束且该分支不查不写 folder_ids 缓存（对照 Some 分支有缓存），导致导入含 N 条根级（无文件夹包裹）outline 的 OPML 后侧栏出现 N 个同名「导入」目录；且与 add_feed 路径的「未分类」兜底约定不一致（本任务不改兜底命名，只修重复创建）。修复方案：把目录解析统一为单一路径——folder 名取 f.folder.as_deref().unwrap_or("导入")，Some/None 共用同一 folder_ids 缓存（get→or_insert create_folder）；为使缺陷可测，把导入循环从 async command 中抽为纯函数 import_feeds(conn: &Connection, feeds: &[crate::opml::ImportedFeed]) -> AppResult<OpmlImportReport>（除修复点外逐字纯搬运），opml_import 持锁后调用。并在该文件补 #[cfg(test)] 测试（沿用 db/commands_extraction_tests.rs 的 open_in_memory + MIGRATIONS.to_latest 模式）：① 两条根级订阅导入后名为「导入」的目录数==1、feeds 表 2 条、report.imported==2；② 两条订阅带同名目录 → 该目录数==1（锚定缓存既有行为）；③ 重复 URL 第二条跳过（report.skipped==1，锚定既有行为）。不改其他文件，不动 src/opml.rs 的解析与导出。

**依赖**：TASK-061
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/src/commands/opml.rs

## 验收标准

- diff 仅含 src-tauri/src/commands/opml.rs：folder 名统一 f.folder.as_deref().unwrap_or("导入")，Some/None 共用 folder_ids 缓存；导入循环抽为 import_feeds 纯函数（除修复点外纯搬运）；opml_import 行为除修复点外不变
- 新增 ≥3 条测试并全部通过：根级订阅单目录（缺陷复现⇒测试失败：把修复还原为无条件 create_folder 后测试 ① 必须失败）、同名目录缓存、重复 URL 跳过；cargo 通过数 ≥164（161+新增）、0 failed、9 ignored 不增
- 四门禁全绿：cargo test、npm run lint、npm run build、npm run test:frontend
- 不引入新依赖；Cargo.toml/Cargo.lock 与 package.json 零改动
- 文本文件 LF 行尾；台账改动须在 begin 之前完成
- 用户真实数据库不得写入（沿用项目约束）

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；2026-09-19 基线（TASK-061 终版候选验证 RUN-52a9b197，提交 5a4de83；其后 HEAD 1075b23/151cfaf 仅含 .workflow-kit 台账与文档，产品代码 src-tauri/src 与 src 逐字一致）：cargo test 161 passed / 0 failed / 9 ignored、npm run lint exit 0（0 warnings/0 errors）、npm run build exit 0、npm run test:frontend 283/283。本任务 behavior=change 仅限一处：folder==None 分支从「每条无条件新建目录」改为「共用 folder_ids 缓存」（缺陷修复，非契约变更）；导入循环抽取为纯函数是行为保持的纯搬运。基线盲区如实记录：commands/opml.rs 此前无任何测试，重复目录缺陷在既有 161 条测试下不可见（无覆盖即无保护）——本任务以新增 3 条文件内测试补上该盲区。
- 基线证据：.workflow-kit/tasks/runs/RUN-52a9b197256c41c0bf409052cd17f8dd.json
- 需求决定：DEC-992f64fd15714ca2a614fe66d5a144ec
- 补充：commands/opml.rs 文件内 #[cfg(test)]：① 两条根级订阅 → 「导入」目录数==1 且 feeds 2 条且 report.imported==2；② 同名目录两条订阅 → 目录数==1；③ 重复 URL → report.skipped==1；缺陷此前零覆盖（无覆盖即无保护）；测试沿用 db/commands_extraction_tests.rs 的 open_in_memory + MIGRATIONS.to_latest 模式；验证：cargo_test
- 保留：其余全部既有测试（Rust 161 条含 src/opml.rs 的 parse/build 测试、前端 283 条断言）逐字不动；除修复点外行为保持；既有回归网是改动安全网；验证：cargo_test, lint, build, frontend

## 执行与恢复

- 首次开始：2026-09-19T13:53:56.239908Z
- 原截止时间：2026-09-19T17:53:56.239908Z
- 当前截止时间：2026-09-19T17:53:56.239908Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 11 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-19T13:53:56.407482Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-19T14:05:00.061728Z：编码结果已记录，差异范围已核对：src-tauri/src/commands/opml.rs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-19T14:05:20.652922Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-19T14:16:21.927830Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-062.json)

- [RUN-bcb00d0579a94b0abc1f82fd1fc22cb4](../runs/RUN-bcb00d0579a94b0abc1f82fd1fc22cb4.json)
- [RUN-0259efca5b644b909fcf022fefeaeb73](../runs/RUN-0259efca5b644b909fcf022fefeaeb73.json)
- [RUN-58414b3933bf4615b6fedbd4c48d2ee3](../runs/RUN-58414b3933bf4615b6fedbd4c48d2ee3.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
