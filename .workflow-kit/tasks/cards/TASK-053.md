<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-053 · P1-5 离线期间的「全部已读」不入队：改为无论是否已配置都入队待推

**状态**：done

**目标**：修复 P1-5 的残留：`mark_all_read`（以及同源路径）**仍以 `sync_configured` 作为入队条件**，导致离线期间的「全部已读」不写入待推队列；连接后首次全量对账会按远端状态把本地已读**翻回未读**——用户看到的「已读」会自己变回去。owner 已明确裁决『另立任务修（需授权改协议行为）』，故本任务**获授权改变同步协议行为**（仅限本缺陷所必需）。参照同一批次已完成的 A-5 修复先例（`set_read`/`set_starred` 已改为『无论是否 configured 都入队，推送段在未配置时静默跳过』），本任务应把 `mark_all_read` 等剩余路径对齐到同一语义。

**依赖**：TASK-052
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：.workflow-kit/tasks/evidence/baseline-2026-09-17-task053.md, src-tauri/src/**, src-tauri/tests/**

## 验收标准

- **协议语义变更须显式论证**：在报告中写清「改了什么语义」「为什么必要」「对既有后端/其它客户端的兼容影响」，并说明为何**不**影响推送顺序与冲突解决
- `mark_all_read` 的入队条件不再依赖 `sync_configured`：无论是否已配置远端，离线期间的「全部已读」都写入待推队列；推送段在未配置时静默跳过（与 A-5 先例一致）
- **逐一排查同源残留**：全仓搜索 `sync_configured` 的入队前置判断，列出所有仍在用它作为「是否入队」条件的路径，并判断每个路径应否对齐；未对齐的须逐条说明理由
- **可复跑测试**：新增或转正一个离线复现测试（测试名与意图写明），断言「未配置/离线时 mark_all_read 入队成功，且连接后推送段确实把该动作推给后端」；该测试必须**修前失败、修后通过**（给出修前失败的证据）
- 既有 Rust 测试不得回退：`cargo test` 通过（基线 120 passed / 0 failed / 23 ignored；通过数可增不可减，`#[ignore]` 数不得增加除非有说明）
- 不引入新依赖；`Cargo.toml` / `Cargo.lock` 零改动；不改前端（本缺陷在后端）
- 文本文件必须 LF 行尾；台账改动须在 begin 之前完成

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；2026-09-17 基线（TASK-052 之后）：npm run lint exit 0（0 warnings/0 errors）、npm run build exit 0、npm run test:frontend exit 0 且 241/241（既有 26 + 新增 215）、cargo test 120 passed / 0 failed / 23 ignored。**本任务有意改变行为**（同步协议入队语义），故基线不是「保持不变」，而是「既有后端保护不削弱 + 语义变更显式论证 + 每处变化有对应测试」。**更正（2026-09-17，review_failure 修复轮后）**：我原先在此声称「Rust 侧既有同步测试是本任务的直接回归网」——**该陈述不实，已由独立审查实证 + 修复轮独立复现推翻**：既有 offline_read_change_pushed_after_connect 只覆盖 set_read，**根本不覆盖 mark_all_read**（其内部用 return 门控逐字复刻该测试、在回退后的代码上跑得 exit 0 / 1 passed）。正确表述：**本缺陷在既有回归网中原本未受保护**，保护是由本任务**新增**的 offline_mark_all_read_queued_and_pushed_after_connect 才建立的。修复轮另扩展出覆盖矩阵（逐条回退跑完整 cargo test）：set_read ✅ 有保护；**set_starred ❌ 空白**（审查者未报的新发现，已记录未修）；mark_all_read ⚠️ 原本空白。教训：把「有测试」当成「有保护」——未逐条核对覆盖关系。详见 .workflow-kit/tasks/evidence/TASK-053-repair-round-report.md。
- 基线证据：.workflow-kit/tasks/evidence/baseline-2026-09-17-task053.md
- 需求决定：DEC-c22f890a49524130a32d3d8254d742cb
- 补充：离线 mark_all_read 入队与连接后推送的复现测试；该缺陷的修复是行为变更，必须有可复跑证据；并参照 A-5 的先例把复现测试转正为必过；验证：cargo_test
- 保留：其余全部既有 Rust 测试（含 A-5/C-1 转正的同步测试）；本任务只改「是否入队」这一条件，不得借机改动推送顺序、冲突解决或对账口径；既有同步测试正是这些语义的保护；验证：cargo_test
- 保留：前端 241 项断言；本任务不改前端；该套件证明前端契约未被波及；验证：frontend

## 执行与恢复

- 首次开始：2026-09-17T23:33:36.716995Z
- 原截止时间：2026-09-18T03:33:36.716995Z
- 当前截止时间：2026-09-18T03:33:36.716995Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 45 分钟
- 已用修复轮：2
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-18T00:17:27.728154Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-18T00:26:01.758877Z：Review requires changes; inspect the findings；下一步：先核对已有文件及原始日志，再处理 review_failure；不要新建任务或重置预算
- 2026-09-18T00:26:10.405500Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后调用 finish，再运行 verify
- 2026-09-18T00:34:02.285516Z：Worker changed_files does not match the observed project diff；下一步：先核对已有文件及原始日志，再处理 protocol；不要新建任务或重置预算
- 2026-09-18T00:43:07.651372Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后调用 finish，再运行 verify
- 2026-09-18T00:43:30.528551Z：编码结果已记录，差异范围已核对；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-18T00:43:52.499522Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-18T00:49:26.352497Z：当前候选的测试与审查通过；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-053.json)

- [RUN-7808dbff015c4659ae6b496181636ecd](../runs/RUN-7808dbff015c4659ae6b496181636ecd.json)
- [RUN-5d5e382638e84c259904235c78a778ae](../runs/RUN-5d5e382638e84c259904235c78a778ae.json)
- [RUN-c265e3f09bbc425cbf569b6c25121ef5](../runs/RUN-c265e3f09bbc425cbf569b6c25121ef5.json)
- [RUN-29f39e45a77f4976843ffc6f68735ad1](../runs/RUN-29f39e45a77f4976843ffc6f68735ad1.json)
- [RUN-5d40a928c96340e199c7e902120f89e1](../runs/RUN-5d40a928c96340e199c7e902120f89e1.json)
- [RUN-9b3dbc2ec07e4306a0f07b362ff0a90a](../runs/RUN-9b3dbc2ec07e4306a0f07b362ff0a90a.json)
- [RUN-5b202743d8b44abc86134ada2c4bd827](../runs/RUN-5b202743d8b44abc86134ada2c4bd827.json)
- [RUN-e4f0b7b7ff7d4f89b6688ba1eb849210](../runs/RUN-e4f0b7b7ff7d4f89b6688ba1eb849210.json)
- [RUN-d804e57c6d1c4370a711f658abba62c9](../runs/RUN-d804e57c6d1c4370a711f658abba62c9.json)
- [RUN-183beae08354498983b7b0c4dd35dc34](../runs/RUN-183beae08354498983b7b0c4dd35dc34.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
