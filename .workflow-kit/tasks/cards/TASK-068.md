<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-068 · 结构性硬化：pull 游标失败守卫、app_settings 读取收口、行类型 fixture 防漂移、仓库卫生（REQ-103）

**状态**：cancelled

**目标**：AUDIT-20260919-v2.md 架构评估的结构性硬化四项（REQ-103）：

【N-硬1 · pull 游标失败守卫（最有价值单点）】greader_pull.rs：item_contents 分块失败仅 continue，随后 set_last_sync_ts(now) 无条件执行——失败块的条目本轮丢失且游标已推进，下一轮增量从新游标起步，这些条目只能等全量同步补回（「偶发漏文章」的结构性温床）。修复：块失败计数，chunk_failures>0 时不推进游标（保持旧 last_sync_ts，下一轮自动重拉同一窗口，合并幂等）并 log::warn。fever_pull.rs 对称处理（since_id 游标同理：分块/单页失败时不推进 since_id）。

【N-硬2 · app_settings 读取收口】同一段 get_setting("app_settings") → from_str → v.get(field) 模板在 lib.rs（read_close_to_tray/read_close_prompt_shown 等）、commands/mod.rs（read_dedup_flag）、scheduler.rs（read_sync_mode_conn/auto_sync_backend）复制 6+ 处，默认值处理各异（新增设置项易漏镜像）。修复：db/settings.rs 增加类型化助手 app_settings_bool(conn, key, default) 与 app_settings_str(conn, key, default)（get_setting 失败/JSON 解析失败/字段缺失一律返回 default），上述调用点全部收口；不改变任何既有默认值语义（closeToTray=true、closePromptShown=false、smartDedup=false、syncMode=direct 等）。

【N-硬3 · 行类型契约防漂移】lib/api.ts 手写 Rust Serialize 结构镜像（FeedRow/ArticleListItemRow 等），Rust 侧改字段 TS 侧只在运行时发现。修复：新增 Rust 测试把规范的 FeedRow 与 ArticleListItemRow serde_json 序列化结果与检入的 tests/fixtures/row_fixture.json 逐字节比对（漂移即测试失败）；前端回归新增断言：读同一 fixture，经 articleRowToEntry/feedRowToItem 映射后逐字段核对（两侧共用同一 fixture，任一侧漂移都会被捕获）。不引入 codegen 依赖。

【N-硬4 · 仓库卫生】删除根目录垃圾文件（run*.log、teeth.log、probe.log、baseline-test.log、build.log、gate-exits.log、lint*.log 及字面名为 ''' + $db + ''' 的脚本事故产物等，仅删 .log/事故文件）；.gitignore 补全（*.log、tmp/、dist-test/、target/ 等生成物）。_rev501-bak/ 备份目录不做删除（出处待 owner 确认），仅加入 .gitignore。

**依赖**：TASK-067
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/src/sync/greader_pull.rs, src-tauri/src/sync/fever_pull.rs, src-tauri/src/db/settings.rs, src-tauri/src/lib.rs, src-tauri/src/commands/mod.rs, src-tauri/src/scheduler.rs, src-tauri/tests/mock_greader.rs, src-tauri/tests/pull_cursor_e2e.rs, src-tauri/tests/fixtures/row_fixture.json, src-tauri/tests/row_fixture_e2e.rs, tools/frontend-regression.mjs, .gitignore

## 验收标准

- diff 仅含 allowed_paths 内文件；N-硬1 两处 pull 在 chunk_failures>0 时不推进游标且 report.errors 非空；N-硬2 六处调用点收口且默认值逐点保持；N-硬3 双侧共用 fixture；N-硬4 垃圾文件删除 + .gitignore 补全
- 变异可检出：① 还原 greader_pull 为无条件 set_last_sync_ts → pull_cursor_e2e 的守卫断言失败；② fixture 改动一个字段值 → Rust 比对测试失败；③ TS 适配器删一个字段映射 → (r) 断言失败（模拟漂移）；还原后全过
- 四门禁全绿：cargo test 通过数 ≥171+新增 且 0 failed、9 ignored 不增；lint 0/0；build exit 0；frontend 通过数 ≥299 且存量不回退
- 不引入新依赖；package.json/Cargo.toml/Cargo.lock 零改动；文本文件 LF 行尾；台账改动须在 begin 之前完成；用户真实数据库不得写入

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；2026-09-19 基线（TASK-067 终版候选验证 RUN-73a92538，提交 dd65408）：cargo test 171 passed / 0 failed / 9 ignored、npm run lint exit 0（0 warnings/0 errors）、npm run build exit 0、npm run test:frontend exit 0 且 299/299。本任务 behavior=change 的范围如实声明：① pull 分块失败时游标不再推进（缺陷行为=失败也推进）——下一轮会重拉同一窗口，合并幂等所以无重复副作用；② app_settings 读取收口为类型化助手——默认值语义逐点保持（纯搬运重构）；③ 新增 fixture 防漂移测试（纯新增覆盖）；④ 仓库垃圾文件删除（非代码行为）。既有测试预期全部保持。
- 基线证据：.workflow-kit/tasks/runs/RUN-73a92538633d4f40a218a7dcb5097c76.json
- 需求决定：DEC-992f64fd15714ca2a614fe66d5a144ec
- 补充：pull_cursor_e2e.rs：mock_greader 新增 fail_item_contents 注入（镜像 fail_edit_tag 模式）——注入后 pull 完成时 last_sync_ts 保持旧值且 report.errors 非空；不注入时游标正常推进、条目合并（双向锚定）；游标推进守卫此前零覆盖；mock 注入使全链路确定性验证；验证：cargo_test
- 补充：db/settings.rs 文件内测试：app_settings_bool/str 的缺省、解析、坏 JSON 三形态；row_fixture_e2e.rs：serde_json 序列化与 fixture 逐字节比对；收口助手与 fixture 防漂移均需直接覆盖；验证：cargo_test
- 补充：frontend-regression 新增 (r) 断言：读 tests/fixtures/row_fixture.json 经 articleRowToEntry/feedRowToItem 映射后逐字段核对；TS 侧字段漂移此前无保护；验证：frontend
- 保留：其余全部既有断言与测试（171+299 存量）逐字不动；除新增外契约不变；验证：cargo_test, lint, build, frontend

## 执行与恢复

- 首次开始：2026-09-20T00:19:48.810096Z
- 原截止时间：2026-09-20T04:19:48.810096Z
- 当前截止时间：2026-09-20T04:19:48.810096Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 22 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：如需同一目标，准备新的任务并引用本任务作为历史

## 最近检查点

- 2026-09-20T00:19:48.971628Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-20T00:41:47.493957Z：Out-of-scope changes: .workflow-kit/tasks/items/TASK-068.json, ''' + $db + ''' (allowed: src-tauri/src/sync/greader_pull.rs, src-tauri/src/sync/fever_pull.rs, src-tauri/src/db/settings.rs, src-tauri/src/lib.rs, src-tauri/src/commands/mod.rs, src-tauri/src/scheduler.rs, src-tauri/tests/mock_greader.rs, src-tauri/tests/pull_cursor_e2e.rs, src-tauri/tests/fixtures/row_fixture.json, src-tauri/tests/row_fixture_e2e.rs, tools/frontend-regression.mjs, .gitignore, src-tauri/src/db.rs)；下一步：核对 diff --run 列出的越界文件，撤销或用 unblock --note 说明归属后再 begin；不要新建任务或重置预算
- 2026-09-20T00:42:11.585530Z：阻塞已处置（scope）：两处范围外标记均为已授权改动：① items/TASK-068.json 的修改是 ledger_correction 本身（owner 批准 allowed_paths 加 db.rs，DEC-ec07500051dc4fe982de4ba9934816b5）；② 删除被误提交的脚本事故文件 ''' + $db + ''' 是任务规格 N-硬4 仓库卫生的明确内容（AUDIT-20260919-v2.md 架构评估第 5 项），owner 已批准路线。两项无需撤销；下一步：begin 重新实现
- 2026-09-20T00:43:33.738908Z：开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-20T00:43:48.703541Z：Out-of-scope changes: ''' + $db + ''', src-tauri/src/db.rs (allowed: src-tauri/src/sync/greader_pull.rs, src-tauri/src/sync/fever_pull.rs, src-tauri/src/db/settings.rs, src-tauri/src/lib.rs, src-tauri/src/commands/mod.rs, src-tauri/src/scheduler.rs, src-tauri/tests/mock_greader.rs, src-tauri/tests/pull_cursor_e2e.rs, src-tauri/tests/fixtures/row_fixture.json, src-tauri/tests/row_fixture_e2e.rs, tools/frontend-regression.mjs, .gitignore)；下一步：核对 diff --run 列出的越界文件，撤销或用 unblock --note 说明归属后再 begin；不要新建任务或重置预算
- 2026-09-20T00:44:02.418572Z：阻塞已处置（scope）：两处范围外均为已授权改动：① src-tauri/src/db.rs 的 pub use 导出行——owner 批准的台账订正（因 recompute 需已有审查、冻结摘要无法重算，改为 finish 时 unblock 豁免落地，与 TASK-051 重算先例等效）；② 删除被误提交的脚本事故文件为任务规格 N-硬4 的明确内容。两项无需撤销；下一步：begin 重新实现
- 2026-09-20T00:51:01.440962Z：任务已取消：范围豁免机制与冻结基线冲突（db.rs 导出与事故文件删除为已授权改动但无法表达进冻结 allowed_paths）；取消并重新收口；下一步：如需同一目标，准备新的任务并引用本任务作为历史

## 原始证据

[唯一状态记录](../items/TASK-068.json)

- [RUN-7b2b068ef30240b89d2f9bf35f36d7a6](../runs/RUN-7b2b068ef30240b89d2f9bf35f36d7a6.json)
- [RUN-32258f091f1749feb8ff76a9b78e92c3](../runs/RUN-32258f091f1749feb8ff76a9b78e92c3.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
