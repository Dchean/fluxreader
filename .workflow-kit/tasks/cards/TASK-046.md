<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-046 · 流程工具修复：prepare 读取 scope.allowed_paths 并对静默回退报错（消除二次命中的死锁缺口）

**状态**：cancelled

**目标**：修复已两次导致任务被 scope 门禁卡死（且 CLI 无任何恢复入口）的流程工具缺口。缺口记录于 .workflow-kit/docs/TOOL-GAP-prepare-allowed-paths.md：workflow_runtime.py:435 只从 spec 的**顶层**读 allowed_paths（specification.get('allowed_paths', paths)），而项目自带模板 tasks/templates/TASK.json 把该字段放在 **scope.allowed_paths**（嵌套）；Agent 按模板形状书写时会被**静默忽略**并回退为目录式 snapshot_paths，因 matches() 是纯 fnmatch（目录名不匹配具体文件），导致 finish 把全部改动文件误判越界。首次命中 TASK-043，第二次命中 TASK-044（均由 owner 授权做记录级订正解锁）。本任务实施缺口文档「建议的工具修复」第 1 项（优先项）：让 prepare 同时接受顶层与 scope.allowed_paths（嵌套优先，因为它更具体），并在「spec 里存在 scope.allowed_paths 却与生效值不一致」时**直接报错退出**而不是静默回退。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：.workflow-kit/scripts/**, .workflow-kit/docs/TOOL-GAP-prepare-allowed-paths.md, .workflow-kit/tasks/evidence/baseline-2026-09-17-task046.md, .workflow-kit/tasks/evidence/TASK-046-toolfix-report.md

## 验收标准

- prepare 在 spec 提供 scope.allowed_paths（顶层未提供）时，采用该值而非静默回退到 snapshot_paths
- 顶层 allowed_paths 与 scope.allowed_paths **同时存在且不一致**时，prepare 报错退出并说明冲突，不静默择一
- 两者都不存在时，保持现有行为（回退到 snapshot_paths），不破坏既有 spec 的兼容性
- 回归验证：用三份真实 spec 复跑 prepare 到临时任务目录，证明(a)仅顶层、(b)仅嵌套、(c)两者一致 三种写法都得到正确的 scope.allowed_paths；(d)两者冲突时报错
- 回归验证：既有已 prepare 的任务不受影响（对当前 16 个任务跑 check 应全绿）
- 缺口文档更新：把「建议的工具修复」第 1 项标记为已实施，并写明实施方式与验证证据
- 不改动 matches()、不放宽门禁、不改任何业务代码与既有任务记录

## 测试适用性

- 既有行为：保持
- 原始基线：PASS；2026-09-17 基线：project_workflow.py check 全绿（16 任务 / 0 errors / 0 warnings）。本任务只改 prepare 的入参读取与冲突检测，不触碰 matches()/finish/verify/review 的判定逻辑，也不改任何业务代码；判据是 (1) check 仍全绿、(2) 新增的四类 spec 写法回归自测全部通过、(3) 既有任务记录零改动（台账文件 sha256 不变）。
- 基线证据：.workflow-kit/tasks/evidence/baseline-2026-09-17-task046.md
- 需求决定：DEC-toolfix-extend-deadlock-20260915
- 保留：workflow_runtime.matches() 与 finish 的越界判定逻辑；缺口文档第 2 项建议（让目录名匹配其下文件）会放宽越界判定语义；本任务刻意不采纳，只修入参读取，保持门禁强度不变；验证：check
- 保留：既有 16 个任务的台账记录与 allowed_paths；本次只修工具行为，不为任何历史任务改写授权；历史订正已由 owner 单独授权；验证：check
- 补充：.workflow-kit/scripts/tests/test_allowed_paths.py（新增回归自测）；该缺口已两次静默致阻塞，必须留下可复跑的回归测试，覆盖四种 spec 写法（仅顶层/仅嵌套/一致/冲突）；验证：selftest

## 执行与恢复

- 首次开始：2026-09-17T07:31:55.879636Z
- 原截止时间：2026-09-17T11:31:55.879636Z
- 当前截止时间：2026-09-17T11:31:55.879636Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 6 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：先核对已有文件及原始日志，再处理 scope；不要新建任务或重置预算

## 最近检查点

- 2026-09-17T07:31:55.945239Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后调用 finish，再运行 verify
- 2026-09-17T07:38:39.441210Z：Out-of-scope changes: .workflow-kit/binding.json, .workflow-kit/scripts/tests/test_allowed_paths.py, .workflow-kit/scripts/workflow_runtime.py；下一步：先核对已有文件及原始日志，再处理 scope；不要新建任务或重置预算

## 原始证据

[唯一状态记录](../items/TASK-046.json)

- [RUN-3df9bad45a3946ea8f1e3da51effe38f](../runs/RUN-3df9bad45a3946ea8f1e3da51effe38f.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
