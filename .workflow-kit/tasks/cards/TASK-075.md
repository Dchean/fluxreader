<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-075 · 同步语义行为变更收口：配置同步删除语义与计数口径（P2-12）+ 远端退订同步删本地（P3-11）（REQ-104）

**状态**：verified

**目标**：本任务收口被取消的 TASK-074 的同一成果（历史：TASK-074 已完成全部实现——四项门禁实测通过、新增 6 条测试、两条关键行为各有变异取证——但 finish 的范围检查无法通过：owner 批准的 allowed_paths 订正（增加 src/lib/api.ts 与 src/components/settings/ConfigSyncSection.tsx）落盘于该任务 begin 之后，任务基线为订正前的快照，diff 恒把 .workflow-kit/tasks/items/TASK-074.json 判为 protected，unblock 循环无法收敛，故按 TASK-068→TASK-069 先例取消并以本任务收口；本任务相对 begin 快照为零新增改动，成果经候选快照绑定）。内容按 owner 2026-09-20 两项设计边界裁决实施行为变更。A【P2-12 配置同步删除语义，DEC-req104-p2-12-config-delete-20260920】① merge_app_settings 由只 upsert 改为「远端缺失的白名单键本地同步删除」，并新增 is_local_only_setting 单一判定收敛 autoStart/closePromptShown（上传过滤与下载合并共用，避免漂移）；远端 app_settings 非对象时不做删除（防一次坏 payload 清空本地设置）；② 计数口径修正：apply_payload 返回值由 (imported, skipped) 改为 ApplyOutcome{imported, updated, skipped}——此前 skipped 在「已存在并被更新」分支自增、语义实为已更新数却被前端展示为「跳过」；现在先读现值判断有无变化，有变化计 updated、完全一致才计 skipped；前端 api.ts 新增 ConfigSyncApplyResult 类型、ConfigSyncSection.tsx 文案改为如实分别展示「新增/更新/跳过（内容一致）」。B【P3-11 远端退订同步删本地，DEC-req104-p3-11-remote-unsub-20260920】pull_feeds 新增删除段，同时满足三条才删：origin='remote' 且 remote_id IS NOT NULL、规范化 URL 不在本轮远端订阅列表、sync_queue 无该 URL 的未推送变更；刻意不写 feed_tombstone（墓碑语义是「用户本地删除、不许复活」，此处是跟随远端事实删除，服务端重新订阅应能正常建回）；SyncReport 新增 removed_feeds。四项门禁须保持：cargo test 通过数 ≥183 且 0 failed、9 ignored 不增；lint 0/0；build exit 0；frontend 通过数 ≥303。

**依赖**：TASK-070
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/src/config_sync.rs, src-tauri/src/sync/mod.rs, src-tauri/src/sync/subscriptions.rs, src-tauri/src/commands/sync.rs, src-tauri/src/sync/phases.rs, src-tauri/src/db/feeds.rs, src-tauri/tests/config_sync_e2e.rs, src-tauri/tests/sync_e2e.rs, src-tauri/tests/mock_greader.rs, src-tauri/tests/account_lifecycle_e2e.rs, src/lib/api.ts, src/components/settings/ConfigSyncSection.tsx, tools/frontend-regression.mjs

## 验收标准

- A① 远端白名单字段在远端消失后本地同步删除（有断言证明删除），本地专属字段 autoStart/closePromptShown 不被远端删除；远端 app_settings 畸形时不清空本地
- A② apply_payload 计数口径修正为三口径分离（imported/updated/skipped），有断言区分「已更新」与「已跳过」，前端类型与文案如实对应
- B① 远端退订的已绑定且 origin='remote' 的订阅在本地删除，且下一轮 pull 不复活
- B② 本地直连订阅（origin='local'）与未绑定订阅在远端退订场景下不被删除（防误删断言）
- B③ 有未推送队项（pending）的源不被远端快照删除（失败路径断言，且用例内先证明队项确实还在）
- ① 两条关键行为各有修前可复现/修后通过的成对证据（变异实测：删掉实现即失败、还原即通过）
- ② 四门禁全绿：cargo test 通过数 ≥183 且 0 failed、9 ignored 不增；lint 0/0；build exit 0；frontend 通过数 ≥303
- ③ 不引入新依赖；package.json/Cargo.toml/Cargo.lock 零改动；文本文件 LF；用户真实数据库不得写入
- ④ 台账改动（任务记录/决定）须在 begin 之前完成，避免相对基线越界

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；2026-09-20 基线（TASK-070 终版候选验证 RUN-44d4ee12，提交 48e2763）：cargo test 177 passed / 0 failed / 9 ignored、npm run lint exit 0（0 warnings/0 errors）、npm run build exit 0、npm run test:frontend exit 0 且 303/303。本任务 behavior=change 且属业务契约变更，依据 owner 2026-09-20 三项裁决：① 配置同步由『只 upsert』改为『远端消失则本地删除』；② 应用结果计数由『skipped=已存在被更新数』改为 imported/updated/skipped 三口径分离（并同步前端类型与文案）；③ 远端退订由『本地永不删』改为『同步删本地（限 origin=remote 且已绑定，且无未推送变更）』。这些都与审计记录的旧行为描述冲突（P2-12『无删除语义』、P3-11『本地永不删，疑有意』），属用户已确认的新行为与旧行为冲突，故按 replace 语义处理：替换断言须引用对应 owner 决定。既有测试中与旧行为绑定的部分改造为断言新行为（config_sync_e2e 的 imported/skipped 断言改为三口径、sync_e2e 的既有断言逐字保持），其余全部保持。注：本任务收口 TASK-074 的成果，其实现与取证已在被取消的 TASK-074 中完成并留在工作区；本任务 begin 快照包含全部成果，故相对基线为零新增改动。
- 基线证据：.workflow-kit/tasks/runs/RUN-44d4ee12298e4fc6a9a862cbc2c1f93c.json
- 需求决定：DEC-req104-p2-12-config-delete-20260920, DEC-req104-p3-11-remote-unsub-20260920, DEC-req104-t074-scope-frontend-contract-20260920
- 替换：配置同步的『远端缺失即保留本地』旧断言：改为『远端白名单字段消失后本地同步删除，本地专属字段保留，畸形 payload 不清空』；owner 裁决 DEC-req104-p2-12-config-delete-20260920 选择实施删除语义，与旧断言冲突；验证：cargo_test
- 替换：config_sync_e2e 的 imported/skipped 二元断言：改为 imported/updated/skipped 三口径（并核对前端 ConfigSyncApplyResult 与展示文案）；owner 裁决 DEC-req104-p2-12-config-delete-20260920 与范围订正 DEC-req104-t074-scope-frontend-contract-20260920 要求口径如实分离并贯通前端；验证：cargo_test, frontend
- 替换：远端退订后『本地订阅保留』旧断言：改为『已绑定且 origin=remote 的本地订阅被删除；本地直连/未绑定/pending 保留』；owner 裁决 DEC-req104-p3-11-remote-unsub-20260920 选择实施本地删除，与旧断言冲突；验证：cargo_test
- 补充：失败路径与防误删覆盖（4 条）：origin='local' 不被误删、未绑定源不被删、有未推送队项的源受保护（含队项存在性的前置断言）、删除后下一轮不复活；以及远端 app_settings 畸形不清空本地；高风险的异常路径必须有覆盖，且这些是本次行为变更最可能造成数据丢失的面；验证：cargo_test
- 保留：其余全部既有断言与测试（177 条 Rust + 303 条前端）逐字不动，含 TASK-035 的删除墓碑防复活既有断言；除上述契约变更外，同步语义其余部分保持；验证：cargo_test, lint, build, frontend

## 执行与恢复

- 首次开始：2026-09-20T08:03:53.638550Z
- 原截止时间：2026-09-20T12:03:53.638550Z
- 当前截止时间：2026-09-20T12:03:53.638550Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 0 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-20T08:03:54.062119Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-20T08:04:17.571144Z：编码结果已记录，差异范围已核对：无文件变化；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-20T08:04:44.547981Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-20T08:41:13.427164Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-20T09:07:43.631282Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-075.json)

- [RUN-0c142dcb55c04ffdb3924bc3b825799f](../runs/RUN-0c142dcb55c04ffdb3924bc3b825799f.json)
- [RUN-02dbb418948e438984a2d4ef058cce9d](../runs/RUN-02dbb418948e438984a2d4ef058cce9d.json)
- [RUN-4386e97bc9044e44b27bc91863c4c3a8](../runs/RUN-4386e97bc9044e44b27bc91863c4c3a8.json)
- [RUN-8af02e46a67f46398e47074b77196a07](../runs/RUN-8af02e46a67f46398e47074b77196a07.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
