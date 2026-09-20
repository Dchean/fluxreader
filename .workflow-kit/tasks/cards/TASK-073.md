<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-073 · ingestion.rs 拆分为 ingestion/ 领域模块（REQ-105）

**状态**：ready

**目标**：把 ingestion.rs（766 行，最后一个未拆旧单体）拆分为 ingestion/ 领域模块，沿用 db.rs / sync.rs / commands.rs 的既有试点配方：先补断言 → 纯搬运 → 四门禁不回归。目标结构（按审计建议的领域切分）：conditional_get（条件 GET + read_capped + build_client）、parse_feed（parse_feed/resolve_url/clamp_publish_date/map_entry/mime_from_url 等纯解析）、staged 刷新（refresh_feed_staged/read_feed_for_refresh/apply_refresh_result 等三段式）、favicon 发现（discover_favicon/extract_icon_link/rel_is_icon/extract_html_attr）。同时按 TASK-070 的死代码结论处置旧版 refresh_feed（若 TASK-070 已删则此处无需处理；若保留则随搬运标注）。硬约束：crate::ingestion 的公开路径保持不变（调用点不因拆分而失败，全部走 pub use 重导出），行为零变化——除机械搬运与模块声明外不改任何逻辑。

**依赖**：TASK-069
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/src/ingestion.rs, src-tauri/src/ingestion/mod.rs, src-tauri/src/ingestion/conditional_get.rs, src-tauri/src/ingestion/parse_feed.rs, src-tauri/src/ingestion/staged.rs, src-tauri/src/ingestion/favicon.rs, src-tauri/src/lib.rs

## 验收标准

- ① 拆分后无超过 800 行的生产单体（对照审计口径核对全项目 src-tauri/src 与 src）
- ② 行为零变化：四门禁全绿，cargo test 通过数 ≥177 且 0 failed、9 ignored 不增；lint 0/0；build exit 0；frontend 通过数 ≥303
- ③ 既有断言一行不动（纯搬运；若确需调整仅限模块路径/导入入口，不得弱化契约），测试文件须逐字保持
- ④ crate::ingestion 的公开路径不变（build_client/conditional_get/parse_feed/refresh_feed_staged/apply_refresh_result/read_feed_for_refresh 等调用点无需改动即可编译）
- ⑤ 拆分不改变并发/锁边界：staged 三段式的 HTTP 锁外与写库持锁位置逐字一致
- ⑥ 搬运可为机械 diff 核对：新增模块文件内容来自原文件对应区段，不夹带语义改动（建议在 worker-result 说明各区段来源行号）
- ⑦ 不引入新依赖；package.json/Cargo.toml/Cargo.lock 零改动；文本文件 LF；用户真实数据库不得写入

## 测试适用性

- 既有行为：保持
- 原始基线：PASS；2026-09-20 基线（TASK-069 终版候选验证 RUN-5380e8e4，提交 452e0e2）：cargo test 177 passed / 0 failed / 9 ignored、lint 0/0、build exit 0、frontend 303/303。本任务 behavior=preserve：纯搬运拆分，不改任何产品行为，故不改动任何既有断言。既有覆盖中与 ingestion 直接相关的是 ingestion_e2e.rs、staged_refresh_e2e.rs、refresh_dedup_e2e.rs、endpoint_autodetect_e2e.rs 等 14 个引用了 ingestion 的测试文件——它们经 crate::ingestion 公开路径调用，拆分后路径不变则无需修改；若因模块化导致路径变化，按 adapt 调整测试的导入入口（只改入口，不动断言语义）。这些测试正是本次拆分的行为保护，必须继续全部执行且逐字保留断言。
- 基线证据：.workflow-kit/tasks/runs/RUN-5380e8e45603487db6019495fdb61fb7.json
- 需求决定：DEC-992f64fd15714ca2a614fe66d5a144ec
- 保留：全部既有断言与测试（177 条 Rust + 303 条前端）逐字不动；与 ingestion 相关的 14 个测试文件继续经 crate::ingestion 路径执行；behavior=preserve 的纯搬运重构，既有断言即本次的行为保护，不得调整；验证：cargo_test, lint, build, frontend
- 适配：若模块化导致测试或调用点的导入入口变化：仅调整 use/路径引用，不动断言内容与语义；行为不变，测试依赖的是模块路径这一实现细节；按 GATES 对 adapt 的定义只改入口；验证：cargo_test

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

[唯一状态记录](../items/TASK-073.json)


卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
