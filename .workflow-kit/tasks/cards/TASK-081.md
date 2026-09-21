<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-081 · REQ-104 P3 卫生项逐条处置（含不修的理由），闭合验收第三条

**状态**：done

**目标**：闭合 REQ-104 的第三项验收标准「P3 项每条有处置结论（修/不修+理由）」——该项此前只交付了删除面（TASK-070/078）与设计裁决面（TASK-075），P3 卫生清单本身尚未逐条处置留痕。来源：.workflow-kit/docs/AUDIT-20260919-v2.md 的「### P3 卫生类」共 13 条后端 + 7 条前端 + 死代码集群（死代码已由 TASK-070/078 完成，本任务不重复）。**已用只读核查确认各项当前真实状态**（审计报告的行号已漂移，故逐项按代码实测重新定位）：后端 [1] 绑定/清理写失败被吞——实测 sync/subscriptions.rs:119,139、sync/entries.rs:63,92,132、sync/phases.rs:55,74、ingestion/staged.rs:153,176 仍是 `let _ =`（未 warn）；[2] 读失败当默认值——commands/ai.rs:26,45,71 仍是 `.ok().flatten()`（未 warn）；[3] sync_save 留空密码复用（需按「留空只复用 password」核对当前实现）；[4] purge_remote_data:391 仍 `DELETE FROM folders WHERE id NOT IN (SELECT ... FROM feeds ...)`（删所有无成员目录，含用户自建空目录）；[5] cleanup_cache:405 仍 `datetime('now','-N days','localtime')`（时区混用可多删最近 N 小时）；[6] apply_refresh_result（现 ingestion/staged.rs:61+）部分失败口径；[7] sync_local_feeds（commands/sync.rs:151+）读队列失败降级→重复入队；[8] search_articles 的 LIMIT 插值；[9] add_feed/OPML 去重仍走精确 URL（commands/folders.rs:173 `find_feed_by_url`，未 normalize_url）；[10] gist_id 落库失败→孤儿 Gist；[12] init/运行期 unwrap；[13] 锁内 sanitize（审计自身标注「记录备查，量级可接受」）。前端：[F1] selectors.ts:11-18 文件头注释块**实测确实整段重复两次**（11-13 与 16-18 完全相同）；[F2] App.tsx:229-249 的 S/M/J/K 快捷键未让路浮层（Space 已有让路逻辑，见 :217-227）；[F3] 播放中同集再点应播放/暂停切换而非重头开播；[F4] reader.ts markEntriesReadBulk 逐条 IPC；[F5] Timeline.tsx lastStartIndexRef 在 filterKey 后未重置；[F6] bootstrap.ts layoutNeedsBody 恒 false（注释已对齐，仅形式残留）；[F7] mock 模式标读不一致。**实施要求**：逐条给出「修」或「不修＋理由」的结论，并写入 AUDIT 文档或等价台账，使验收可核；对判定为「修」的项实施修复并补断言，对判定为「不修」的项说明理由（如审计自身已标『量级可接受』的 [13]、或属产品决策的项），**不得以『工作量大』为由跳过**。优先处置数据正确性类（[4] 误删用户目录、[5] 时区多删、[9] 去重不一致）与低风险高确定性项（[F1] 重复注释、[1][2] 吞错 warn 化）。

**依赖**：TASK-080
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/src/sync/subscriptions.rs, src-tauri/src/sync/entries.rs, src-tauri/src/sync/phases.rs, src-tauri/src/ingestion/staged.rs, src-tauri/src/commands/ai.rs, src-tauri/src/commands/sync.rs, src-tauri/src/commands/folders.rs, src-tauri/src/commands/opml.rs, src-tauri/src/db/articles.rs, src-tauri/src/config_sync.rs, src-tauri/src/lib.rs, src/store/selectors.ts, src/App.tsx, src/store/slices/reader.ts, src/components/Timeline.tsx, src/store/slices/bootstrap.ts, .workflow-kit/docs/AUDIT-20260919-v2.md, tools/frontend-regression.mjs

## 验收标准

- ① P3 清单（后端 13 条 + 前端 7 条）**每条**都有明确处置结论：修（含改动与断言）或不修（含理由），并记录在案使验收可逐条核对
- ② 数据正确性类项优先处置并有成对证据：[4] purge_remote_data 不再误删用户自建空目录、[5] cleanup_cache 消除时区混用、[9] add_feed/OPML 去重口径与 normalize_url 一致
- ③ 吞错类（[1][2]）改为 log::warn! 并可被断言或可复核；[F1] selectors.ts 重复注释块清除
- ④ 前端 [F2] S/M/J/K 让路浮层（与既有 Space 让路逻辑一致）或其「不修」理由成立
- ⑤ 四门禁全绿：cargo test 通过数 ≥193 且 0 failed、9 ignored 不增；lint 0/0；build exit 0；frontend 通过数 ≥303
- ⑥ 不引入新依赖；package.json/Cargo.toml/Cargo.lock 零改动；文本文件 LF；用户真实数据库不得写入
- ⑦ 台账改动（AUDIT 处置结论）须在 begin 之前完成或由本任务在同一候选内交付，避免相对基线越界（TASK-074/076 教训）

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；2026-09-21 基线（TASK-080 终版候选验证 RUN-891bfe42，提交 d697afc）：cargo test 193 passed / 0 failed / 9 ignored、npm run lint exit 0（0 warnings/0 errors）、npm run build exit 0、npm run test:frontend exit 0 且 303/303。本任务 behavior=change：多数 P3 项属缺陷修复（[4] 误删用户目录、[5] 时区多删最近 N 小时、[9] 去重口径不一致、[1][2] 吞错改 warn 化），会改变运行时行为与可观察日志；依据 owner 在 REQ-104 中已裁决「P3 项逐条处置（修/不修+理由）」（见 BRIEF REQ-104 描述与验收第三条），该裁决 scope 覆盖 REQ-104，故替换/新增断言有据。既有断言除必要适配外逐字保留。
- 基线证据：.workflow-kit/tasks/runs/RUN-891bfe42b74e4a4791c41dcfeb22a4c1.json
- 需求决定：DEC-req104-p3-hygiene-disposition-20260921
- 保留：全部既有 193 条 Rust + 303 条前端断言逐字保留（除因行为修正而必须适配者，且适配须在 worker-result 中逐条列明理由）；本次为缺陷修复与卫生处置，不重构既有语义；BRIEF 明确要求既有质量底线不回退（通过数可增不可减）；验证：cargo_test, lint, build, frontend
- 补充：为判定「修」的项补成对证据（修前可复现/修后通过），至少覆盖 [4] 用户自建空目录不被误删、[5] 时区不再多删、[9] 归一化去重一致；数据正确性类改动必须有修前/修后对照，否则无法证明修复成立；验证：cargo_test

## 执行与恢复

- 首次开始：2026-09-21T08:35:15.235054Z
- 原截止时间：2026-09-21T12:35:15.235054Z
- 当前截止时间：2026-09-21T12:35:15.235054Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 52 分钟
- 已用修复轮：3
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-21T10:02:17.886433Z：编码结果已记录，差异范围已核对：.workflow-kit/docs/AUDIT-20260919-v2.md, src/components/Timeline.tsx, src/store/selectors.ts, tools/frontend-regression.mjs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-21T10:02:44.171600Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-21T10:11:24.369636Z：Review requires changes; inspect the findings；下一步：先核对已有文件及原始日志，再处理 review_failure；不要新建任务或重置预算
- 2026-09-21T10:13:35.228042Z：开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-21T10:15:58.112220Z：编码结果已记录，差异范围已核对：.workflow-kit/docs/AUDIT-20260919-v2.md, tools/frontend-regression.mjs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-21T10:17:10.190441Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-21T10:28:56.643483Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-21T10:34:12.071683Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-081.json)

- [RUN-7605e3fa5b694ae6a60399446899ed56](../runs/RUN-7605e3fa5b694ae6a60399446899ed56.json)
- [RUN-d4d5322673164bb798c606917265255a](../runs/RUN-d4d5322673164bb798c606917265255a.json)
- [RUN-d2c519c2227340dbbb516d2c0cf467df](../runs/RUN-d2c519c2227340dbbb516d2c0cf467df.json)
- [RUN-f84c991aecf84dc290ece265e3a3a963](../runs/RUN-f84c991aecf84dc290ece265e3a3a963.json)
- [RUN-4ead30c08c0d4b7fa09c1ceeb93c88ea](../runs/RUN-4ead30c08c0d4b7fa09c1ceeb93c88ea.json)
- [RUN-a4b42e0e98c942cc87129ddb9a0808e6](../runs/RUN-a4b42e0e98c942cc87129ddb9a0808e6.json)
- [RUN-1be4fce72b17495283bf7d7c6a18cdcf](../runs/RUN-1be4fce72b17495283bf7d7c6a18cdcf.json)
- [RUN-0de5935451f44ccba0c284347bb49c69](../runs/RUN-0de5935451f44ccba0c284347bb49c69.json)
- [RUN-3fdc51e9a6e64fc69aa4ffb9bf820ca3](../runs/RUN-3fdc51e9a6e64fc69aa4ffb9bf820ca3.json)
- [RUN-39becef96b16447d95203a33d412cf17](../runs/RUN-39becef96b16447d95203a33d412cf17.json)
- [RUN-37efbed38af647729ceff855242701bd](../runs/RUN-37efbed38af647729ceff855242701bd.json)
- [RUN-9417a6a4d14c4974a897e808254acbe1](../runs/RUN-9417a6a4d14c4974a897e808254acbe1.json)
- [RUN-d13cc7b3e2b546f98d5282a39a65575f](../runs/RUN-d13cc7b3e2b546f98d5282a39a65575f.json)
- [RUN-d8ee5c8ac2374b308f60c97e9f80a446](../runs/RUN-d8ee5c8ac2374b308f60c97e9f80a446.json)
- [RUN-a4f5b89423c74de99e331efa14466caa](../runs/RUN-a4f5b89423c74de99e331efa14466caa.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
