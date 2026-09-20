<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-069 · 结构性硬化收口：pull 游标失败守卫、app_settings 读取收口、行类型 fixture 防漂移、仓库卫生（REQ-103）

**状态**：verified

**目标**：本任务收口被取消的 TASK-068 的同一成果（历史：TASK-068 已实施 REQ-103 四项硬化并通过四门禁与三项变异取证，但 finish 的范围检查无法通过——db.rs 导出行（owner 已批准 DEC-ec075000）与事故文件删除（规格 N-硬4）在冻结 allowed_paths 外，任务基线为首次 begin 快照、unblock 循环无法收敛，故按流程取消并以本任务收口；本任务相对 begin 快照为零新增改动，成果经候选快照绑定）。四项内容：【N-硬1 游标守卫】greader_pull.rs 分块失败计数、chunk_failures>0 时不推进 last_sync_ts 并 warn（修前失败块条目只能等全量补回的「偶发漏文章」温床）；fever_pull.rs 对称（三处失败计数、时间戳游标仅无失败时推进；last_sync_entry_id 只计已合并条目本就安全）。【N-硬2 收口】db/settings.rs 新增 app_settings_bool/str 类型化助手（失败/坏 JSON/缺失/类型不符一律返回 default）+ 4 条文件内测试；db.rs 一行 pub use 导出（owner 批准 DEC-ec075000）；调用点收口：lib.rs read_close_to_tray/read_close_prompt_shown、commands/mod.rs read_dedup_flag、scheduler.rs read_sync_mode_conn/autoSync 布尔（refreshInterval 数值留待后续，raw 绑定保留）——默认值语义逐点保持。【N-硬3 fixture 防漂移】tests/fixtures/row_fixture.json 检入规范 FeedRow+ArticleListItem 序列化形态；row_fixture_e2e.rs Rust 侧逐字段比对；frontend-regression (r) 断言读同一 fixture 经 articleRowToEntry/feedRowToItem 映射逐字段核对——双侧共用 fixture，任一侧漂移都被捕获。【N-硬4 卫生】删除根目录 14 个垃圾文件（13 个 *.log + 字面名 ''' + $db + ''' 的脚本事故产物）；.gitignore 补 *.log/tmp/（_rev501-bak/ 出处待 owner 确认仅加入忽略规则）。r1 遗留：TASK-068 取消前未及独立审查，本任务的独立审查按同等标准覆盖全部四项内容。

**依赖**：TASK-067
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/src/sync/greader_pull.rs, src-tauri/src/sync/fever_pull.rs, src-tauri/src/db/settings.rs, src-tauri/src/db.rs, src-tauri/src/lib.rs, src-tauri/src/commands/mod.rs, src-tauri/src/scheduler.rs, src-tauri/tests/mock_greader.rs, src-tauri/tests/pull_cursor_e2e.rs, src-tauri/tests/fixtures/row_fixture.json, src-tauri/tests/row_fixture_e2e.rs, tools/frontend-regression.mjs, .gitignore

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

- 首次开始：2026-09-20T00:51:13.565241Z
- 原截止时间：2026-09-20T04:51:13.565241Z
- 当前截止时间：2026-09-20T04:51:13.565241Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 19 分钟
- 已用修复轮：1
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-20T01:21:34.486015Z：开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-20T01:23:14.584079Z：编码结果已记录，差异范围已核对：无文件变化；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-20T01:27:10.067923Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-20T02:09:42.232823Z：Review requires changes; inspect the findings；下一步：先核对已有文件及原始日志，再处理 review_failure；不要新建任务或重置预算
- 2026-09-20T02:11:53.043821Z：开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-20T02:28:14.526558Z：编码结果已记录，差异范围已核对：.gitignore, src-tauri/src/scheduler.rs, src-tauri/src/sync/fever_pull.rs, src-tauri/src/sync/greader_pull.rs, src-tauri/tests/fixtures/row_fixture.json, src-tauri/tests/mock_greader.rs, src-tauri/tests/pull_cursor_e2e.rs, src-tauri/tests/row_fixture_e2e.rs, tools/frontend-regression.mjs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-20T02:28:41.949386Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-20T02:58:05.041050Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-069.json)

- [RUN-aded091805ce4138bc7fb34d754ac472](../runs/RUN-aded091805ce4138bc7fb34d754ac472.json)
- [RUN-93cf2b5286504fd4bb74ab024badc39a](../runs/RUN-93cf2b5286504fd4bb74ab024badc39a.json)
- [RUN-ade2f7ef40e548b78691b35ee639d2d6](../runs/RUN-ade2f7ef40e548b78691b35ee639d2d6.json)
- [RUN-152b43e855c246849a5227eb31c2b633](../runs/RUN-152b43e855c246849a5227eb31c2b633.json)
- [RUN-37386261afe04e13ad226cb5387a0d24](../runs/RUN-37386261afe04e13ad226cb5387a0d24.json)
- [RUN-64e391b489054ed1943f4f09bd0a42da](../runs/RUN-64e391b489054ed1943f4f09bd0a42da.json)
- [RUN-5380e8e45603487db6019495fdb61fb7](../runs/RUN-5380e8e45603487db6019495fdb61fb7.json)
- [RUN-d1c63c7328034b139f57bf48c2b7310b](../runs/RUN-d1c63c7328034b139f57bf48c2b7310b.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
