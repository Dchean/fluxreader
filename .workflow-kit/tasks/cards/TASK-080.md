<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-080 · 修复 prepare 静默回退缺口（工程侧守卫 + 让既有回归测试可跑，不改工作流引擎）

**状态**：done

**目标**：修复「按项目模板把 allowed_paths 写在 spec 的嵌套 scope.allowed_paths 时被静默忽略、回退成 snapshot_paths」这一真实缺口——**不修改工作流引擎**（.workflow-kit/scripts/** 与 templates/** 属托管、且用户 2026-09-21 明确要求不动工作流），改为在项目自有范围内交付等效且更稳的防护。背景与根因（已用 git 定案）：① 缺口文档 TOOL-GAP-prepare-allowed-paths.md 记载其建议第 1 项『已实施（TASK-046, 2026-09-17）』——commit a267b1e 曾新增纯函数 resolve_allowed_paths 并让 prepare 调用它；② 但 commit 6fd382e『升级 workflow-kit 至 2026-09-18.3（rebind --upgrade-tools）』把该修复**整段删除**并还原为旧写法 copy.deepcopy(specification.get('allowed_paths', paths))，证据为 git show 6fd382e 中 -def resolve_allowed_paths 与 -task['scope']['allowed_paths'] = resolve_allowed_paths(...) / +...get('allowed_paths', paths)；③ 项目自有的回归测试 .workflow-kit/scripts/tests/test_allowed_paths.py 未被一并回退，故其由通过变为**静默失败**（实测 0/7 通过、exit 1、全部 AttributeError: no attribute 'resolve_allowed_paths'）。当前严重性已核实并如实下调：升级后的 matches() 已支持目录前缀（matches('src-tauri/src/ai.rs', ['src-tauri/src']) == True），故文档当年『目录名匹配不到具体文件 → 全部误判越界』的死锁前提**已不成立**；但**静默篡改仍存在**——仅写嵌套 scope.allowed_paths 时（模拟 line 509）记录里落成 ['src','tools']（即 snapshot_paths），spec 声明的范围被无声丢弃。本任务交付：(A) 项目自有的 prepare 前置校验脚本（放 tools/，纯 Python、零依赖、可独立运行），在任何 prepare 之前校验 spec 并**拒绝**以下形态并给出可操作报错：allowed_paths 只存在于嵌套 scope.allowed_paths（会被静默忽略）、顶层与嵌套同时给出且不一致、缺失/空/非字符串列表、snapshot_paths 缺失或非法；通过时以退出码 0 输出规范化结果（顶层写法），并在存在嵌套写法时明确提示改为顶层。(B) 让防护不依赖那个已损坏的引擎侧测试：.workflow-kit/scripts/tests/test_allowed_paths.py 位于模板默认 protected_paths 覆盖的 .workflow-kit/scripts/** 内，**任何任务都不得修改**（这正是 TASK-046 当初必须走总控级基础设施提交的原因）。在不改引擎、也不越界修改受保护文件的前提下，改为在工程侧 tools/ 内交付**自带完整可复跑自测**的守卫（tools/task-spec-guard-test.py），覆盖原 7 个用例的全部意图（仅顶层/仅嵌套/一致/冲突/都无/scope 非 dict/snapshot_paths 缺失），并在缺口文档中如实说明引擎侧测试当前处于损坏状态、其覆盖由工程侧自测承接。(C) 订正 TOOL-GAP-prepare-allowed-paths.md 的失实记载：把『第 1 项——已实施』订正为『曾实施（a267b1e）→ 被 6fd382e 升级覆盖 → 当前引擎未实施；改由工程侧 TASK-080 守卫兜底』，并新增一条升级注意事项：rebind --upgrade-tools 会覆盖项目本地对引擎的补丁，凡依赖本地引擎补丁的防护都必须改由工程侧或记录级手段承载。(D) 把该前置校验接入项目文档/约定（在缺口文档与 TOOL-GAP 同类文档中写明用法），使后续任务按模板嵌套写法提交 spec 时会被**在 prepare 之前**拦下，而不是等到 finish 才以越界或范围错配爆出。

**依赖**：TASK-079
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：tools/task-spec-guard.py, tools/task-spec-guard-test.py, .workflow-kit/docs/TOOL-GAP-prepare-allowed-paths.md

## 验收标准

- ① 交付项目侧前置校验脚本 tools/task-spec-guard.py：对「allowed_paths 仅在嵌套 scope.allowed_paths」的 spec 以非零退出码拒绝并给出明确修复指引；对「顶层与嵌套不一致」同样拒绝；对合法顶层写法退出码 0 并打印规范化结果
- ② 该脚本自带可复跑自测 tools/task-spec-guard-test.py，覆盖至少 7 例：仅顶层通过、仅嵌套拒绝、两处一致通过、两处冲突拒绝、都未给拒绝并提示、snapshot_paths 缺失拒绝、非字符串元素拒绝；全部通过时退出码 0
- ③ 不改任何受保护文件：git diff 对 .workflow-kit/scripts/**（含那个已损坏的 test_allowed_paths.py）与 .workflow-kit/tasks/templates/** 为空；受保护文件的既有损坏状态只做如实记录，不在本任务修复
- ④ .workflow-kit/docs/TOOL-GAP-prepare-allowed-paths.md 的『第 1 项——已实施』订正为如实状态（曾实施 a267b1e→被 6fd382e 升级覆盖→当前引擎未实施、改由工程侧 TASK-080 守卫兜底），并如实记载引擎侧测试 test_allowed_paths.py 当前 0/7 通过且位于受保护区不得修改、其覆盖由 tools/task-spec-guard-test.py 承接；新增 rebind --upgrade-tools 会覆盖本地引擎补丁的注意事项
- ⑤ 不引入新依赖（仅 Python 标准库）；文本文件 LF；用户真实数据库不得写入
- ⑥ 四门禁全绿：cargo test 通过数 ≥193 且 0 failed、9 ignored 不增；lint 0/0；build exit 0；frontend 303/303（本任务不触碰产品代码，四门禁应与基线同值）

## 测试适用性

- 既有行为：保持
- 原始基线：PASS；2026-09-21 基线（TASK-079 终版候选验证 RUN-88d6970e，提交 7a12beb）：cargo test 193 passed / 0 failed / 9 ignored、npm run lint exit 0（0 warnings/0 errors）、npm run build exit 0、npm run test:frontend exit 0 且 303/303。本任务不触碰业务产品代码与既有测试，故四门禁应保持同一基线值（作为『未误伤产品』的回归保护）。本任务新增的工程侧守卫属于新的可复跑自测，不改动任何既有断言语义；对 .workflow-kit/scripts/tests/test_allowed_paths.py 的改动是修复其静默失败（该文件当前 0/7 通过、exit 1，属真实损坏而非有效基线）。
- 基线证据：.workflow-kit/tasks/runs/RUN-88d6970ed646409595b4d8694f579944.json
- 需求决定：沿用既有行为，无新增业务取舍
- 保留：全部既有业务断言（193 条 Rust + 303 条前端）与产品代码逐字不动，作为本任务未误伤产品的回归保护；本任务只新增工程侧守卫与文档订正，不改任何产品行为；验证：cargo_test, lint, build, frontend
- 补充：新增工程侧守卫的独立自测 tools/task-spec-guard-test.py（7 例，承接原 test_allowed_paths.py 的全部覆盖意图）；.workflow-kit/scripts/tests/test_allowed_paths.py 位于受保护的 .workflow-kit/scripts/** 内、任何任务不得修改，且其依赖的引擎函数已被升级覆盖而 0/7 失败；在不改引擎的约束下，其覆盖意图须由工程侧自测承接；验证：cargo_test

## 执行与恢复

- 首次开始：2026-09-21T07:42:31.007779Z
- 原截止时间：2026-09-21T11:42:31.007779Z
- 当前截止时间：2026-09-21T11:42:31.007779Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 3 分钟
- 已用修复轮：1
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-21T07:45:42.055355Z：编码结果已记录，差异范围已核对：.workflow-kit/docs/TOOL-GAP-prepare-allowed-paths.md, tools/task-spec-guard-test.py, tools/task-spec-guard.py；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-21T07:46:07.816092Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-21T07:50:54.109035Z：Review requires changes; inspect the findings；下一步：先核对已有文件及原始日志，再处理 review_failure；不要新建任务或重置预算
- 2026-09-21T07:51:39.039514Z：开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-21T07:52:23.564561Z：编码结果已记录，差异范围已核对：.workflow-kit/docs/TOOL-GAP-prepare-allowed-paths.md, tools/task-spec-guard-test.py, tools/task-spec-guard.py；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-21T07:52:32.134641Z：Required gate failed: cargo_test；下一步：先核对已有文件及原始日志，再处理 test_failure；不要新建任务或重置预算
- 2026-09-21T08:03:09.852828Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-21T08:21:55.897213Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-080.json)

- [RUN-ee681e49647c4833b10cfe3239cd8de4](../runs/RUN-ee681e49647c4833b10cfe3239cd8de4.json)
- [RUN-8479219029eb449393b7cf185ae06e6b](../runs/RUN-8479219029eb449393b7cf185ae06e6b.json)
- [RUN-084877e4af824f0096a606e833801f00](../runs/RUN-084877e4af824f0096a606e833801f00.json)
- [RUN-a1d26701359646dcbd19582092c4dee7](../runs/RUN-a1d26701359646dcbd19582092c4dee7.json)
- [RUN-d541d5cd22ad4161bbe6f32672ae4493](../runs/RUN-d541d5cd22ad4161bbe6f32672ae4493.json)
- [RUN-891bfe42b74e4a4791c41dcfeb22a4c1](../runs/RUN-891bfe42b74e4a4791c41dcfeb22a4c1.json)
- [RUN-44a34fbb9b954b2eaaffcb6139a17f83](../runs/RUN-44a34fbb9b954b2eaaffcb6139a17f83.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
