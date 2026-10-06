<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-113 · TASK-112 延续：修复新增代码的 clippy 警告并经 CI 复验（行为零变化保持）

**状态**：done

**目标**：延续任务（continuation_of=TASK-112 候选 7205fab91198e15b42120da9d5df19361783e3bb40a133ccdd3f41081d6d4a18）：dev@d167135 的 CI rust job clippy（--all-targets -- -D warnings）失败（fmt --check 过、cargo test 被跳过、frontend job 绿）。本地无 MSVC 链接器无法运行 clippy（DEC-local-cargo-gate-20261005 工具盲区）。修法要求：①静态排查 TASK-112 新增/修改的 Rust 代码（src-tauri/src/sync/conflict_policy.rs 新建、greader_pull.rs/fever_pull.rs 修改与新增测试、push.rs 注释、db/articles.rs 注释）中触发 clippy 默认 lint 的模式（重点：dead_code 未用测试助手/未用导入、needless return/borrow、冗余 match/if、格式化参数、单臂枚举相关建议、assert_eq 字面量、unused_mut 等），逐一修复；②修复仅限使 clippy 干净与等价重写，不得改变行为（本卡与 TASK-112 同为行为零变化约束）、不得删除有效测试断言；③修复后经推送 CI 复验（cargo test + clippy 全绿，含 TASK-112 新增 9 条测试真正执行通过）——CI 是本卡 cargo 门禁的执行器；④无法本地验证 clippy 属已知限制，coder 尽最大静态核对并在汇报列明全部改动点供审查。

**依赖**：TASK-112
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/src, docs

## 验收标准

- ① clippy 触发点修复：静态排查列明的全部改动点可解释；行为零变化保持（diff 等价重写）
- ② CI rust job 全绿：cargo fmt/clippy/test（含 TASK-112 新增 9 条测试）通过——推送后以 CI 结论为准
- ③ TASK-112 既有测试与其余断言零改动零弱化
- ④ 独立审查（全新子代理，未参与编码）PASS findings=0（聚焦 clippy 修复的等价性与测试完整性）
- ⑤ 本地门禁：cargo fmt、lint、build、frontend 不回退

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；dev@d167135（TASK-112 候选）：本地四门禁绿、独立审查 PASS，但 CI rust job clippy -D warnings 失败（本卡存在的直接原因）。本卡修复 clippy 触发点并经 CI 复验全绿。
- 基线证据：.workflow-kit/docs/AUDIT-20261005-core-consistency.md
- 需求决定：DEC-refactor-roadmap-20261005
- 适配：TASK-112 新增测试/代码中触发 clippy 的等价重写；等价重写不弱化保护；CI 全绿为验收；验证：cargo_fmt, lint, build, frontend
- 保留：TASK-112 既有断言与本地四门禁；不回退证据；cargo 证据由 CI 复验承担（DEC-local-cargo-gate-20261005）；验证：cargo_fmt, lint, build, frontend

## 执行与恢复

- 首次开始：2026-10-06T04:18:39.157194Z
- 原截止时间：2026-10-06T08:18:39.157194Z
- 当前截止时间：2026-10-06T08:18:39.157194Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 30 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-10-06T04:18:40.084219Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-10-06T04:48:42.034156Z：编码结果已记录，差异范围已核对：src-tauri/src/sync/fever_pull.rs, src-tauri/src/sync/greader_pull.rs, src-tauri/src/sync/mod.rs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-10-06T04:49:12.717868Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-10-06T05:09:48.104093Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-113.json)

- [RUN-db542dab68694d4084e075d631e9152e](../runs/RUN-db542dab68694d4084e075d631e9152e.json)
- [RUN-294c3cdc945240eeacd6ec65bbcd4cad](../runs/RUN-294c3cdc945240eeacd6ec65bbcd4cad.json)
- [RUN-53910b7822454abd9bd893437aa3f7bc](../runs/RUN-53910b7822454abd9bd893437aa3f7bc.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
