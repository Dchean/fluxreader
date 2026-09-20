<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-074 · 同步语义行为变更：配置同步删除语义（P2-12）+ 远端退订同步删本地（P3-11）（REQ-104）

**状态**：running

**目标**：按 owner 2026-09-20 两项设计边界裁决实施行为变更（两项都改数据同步语义，故合并为一个高风险任务并绑定裁决）。A【P2-12 配置同步删除语义，DEC-req104-p2-12-config-delete-20260920】现状：config_sync.rs 的 merge_app_settings 只做 upsert，远端删除的配置项在本地不删除；且 skipped 计数实际是『已更新』口径。实施：① 远端白名单字段在远端消失时本地同步删除（删除该键或回落默认值，语义在实现时择一并留证）；② 修正 skipped 计数口径为真实跳过数；③ 保持 autoStart/closePromptShown 等本地专属字段不被远端删除（现白名单已排除，须保持并有断言）。B【P3-11 远端退订同步删本地，DEC-req104-p3-11-remote-unsub-20260920】现状：pull_feeds 无删除分支，远端退订后本地订阅永不删除。实施：④ 远端权威集合中消失的『已绑定』订阅在本地删除（只处理曾绑定且远端已消失的源）；⑤ 必须保留本地直连订阅（origin='local'）与未绑定订阅；⑥ 与 TASK-035 的删除墓碑防复活机制协同——不得被下一轮 pull 建回，也不得误删本地新订阅；⑦ 明确与 pending 未推送队列的交互：本地刚改名/移动尚未推送时不得被远端快照删除。两项都属行为变更且有数据丢失面，须有成对证据（修前复现/修后通过）、失败路径测试与独立审查。

**依赖**：TASK-070
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/src/config_sync.rs, src-tauri/src/sync/subscriptions.rs, src-tauri/src/sync/mod.rs, src-tauri/src/sync/phases.rs, src-tauri/src/db/feeds.rs, src-tauri/src/commands/sync.rs, src-tauri/tests/config_sync_e2e.rs, src-tauri/tests/account_lifecycle_e2e.rs, src-tauri/tests/mock_greader.rs, src-tauri/tests/sync_e2e.rs, tools/frontend-regression.mjs

## 验收标准

- A① 远端白名单字段在远端消失后本地同步删除（修前保留、修后有断言证明删除），本地专属字段 autoStart/closePromptShown 不被远端删除
- A② skipped 计数口径修正为真实跳过数，并有断言区分『已更新』与『已跳过』
- B① 远端退订的已绑定订阅在本地删除（修前保留、修后有断言），且下一轮 pull 不复活（墓碑协同生效）
- B② 本地直连订阅（origin='local'）与未绑定订阅在远端退订场景下不被删除（防误删断言）
- B③ 本地有未推送变更（pending）的订阅不被远端快照删除（失败路径断言）
- ③ 四门禁全绿：cargo test 通过数 ≥177 且 0 failed、9 ignored 不增；lint 0/0；build exit 0；frontend 通过数 ≥303
- ④ 每项都有修前可复现/修后通过的成对证据（失败路径优先用 mock 注入构造）
- ⑤ 不引入新依赖；package.json/Cargo.toml/Cargo.lock 零改动；文本文件 LF；用户真实数据库不得写入

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；2026-09-20 基线（TASK-070 终版候选验证 RUN-44d4ee12，提交 48e2763）：cargo test 177 passed / 0 failed / 9 ignored、npm run lint exit 0（0 warnings/0 errors）、npm run build exit 0、npm run test:frontend exit 0 且 303/303。本任务 behavior=change 且属业务契约变更，依据 owner 2026-09-20 两项裁决：① 配置同步由『只 upsert』改为『远端消失则本地删除』；② 远端退订由『本地永不删』改为『同步删本地』。这两项都推翻了审计记录的现状描述（P2-12『无删除语义』、P3-11『本地永不删，疑有意』），属用户已确认的新行为与旧行为冲突，故按 replace 语义处理：替换断言须引用对应 owner 决定。既有测试中与旧行为绑定的部分须改造为断言新行为，其余全部保持。
- 基线证据：.workflow-kit/tasks/runs/RUN-44d4ee12298e4fc6a9a862cbc2c1f93c.json
- 需求决定：DEC-req104-p2-12-config-delete-20260920, DEC-req104-p3-11-remote-unsub-20260920
- 替换：配置同步的『远端缺失即保留本地』旧断言：改为『远端白名单字段消失后本地同步删除，本地专属字段 autoStart/closePromptShown 保留』；owner 裁决 DEC-req104-p2-12-config-delete-20260920 选择实施删除语义，与旧断言冲突；验证：cargo_test
- 替换：远端退订后『本地订阅保留』旧断言：改为『已绑定且远端已消失的本地订阅被删除；本地直连/未绑定/pending 保留』；owner 裁决 DEC-req104-p3-11-remote-unsub-20260920 选择实施本地删除，与旧断言冲突；验证：cargo_test
- 补充：失败路径与防误删覆盖：本地专属字段不被远端删除、本地直连订阅不被误删、pending 未推送订阅不被删除、删除后下一轮 pull 不复活（墓碑协同）；高风险的异常路径必须有覆盖，且这些是本次行为变更最可能造成数据丢失的面；验证：cargo_test
- 保留：其余全部既有断言与测试（177 条 Rust + 303 条前端）逐字不动，含 TASK-035 的删除墓碑防复活既有断言；除上述两处契约变更外，同步语义其余部分保持；验证：cargo_test, lint, build, frontend

## 执行与恢复

- 首次开始：2026-09-20T07:31:48.013644Z
- 原截止时间：2026-09-20T11:31:48.013644Z
- 当前截止时间：2026-09-20T11:31:48.013644Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 2 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify

## 最近检查点

- 2026-09-20T07:31:48.395364Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify

## 原始证据

[唯一状态记录](../items/TASK-074.json)

- [RUN-93cca1ab6b134cd9904f973bbc508d48](../runs/RUN-93cca1ab6b134cd9904f973bbc508d48.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
