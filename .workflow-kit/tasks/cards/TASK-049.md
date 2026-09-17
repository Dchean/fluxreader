<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-049 · 前端拆分一：store.ts 按领域拆为 Zustand slice（行为保持不变）

**状态**：done

**目标**：把 src/store.ts 拆小。现状：文件 1657 行（可信口径），其中 171–1650 行是**一个 `create<AppState>((set, get) => ({ ... }))` 内的巨型对象字面量**——因此本任务不是「把函数搬到别的文件」，而是按 **Zustand slice 模式**把状态与 action 切成领域模块，再由 store.ts 组合。目标结构：store/slices/ 下按领域分文件（建议 bootstrap / ui / feeds / articles / reader / player / settings 七个 slice，实现者可调整并说明理由），每个 slice 用 `StateCreator<AppState, [], [], XxxSlice>` 形态；store.ts 只保留组合、公共入口与既有再导出。**硬要求：行为逐项保持不变**——包括当前已知缺陷（D1–D5）的现状行为，它们属独立的缺陷修复任务，不得在本任务顺手改。

**依赖**：TASK-048
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：.workflow-kit/tasks/evidence/baseline-2026-09-17-task049.md, src/**, tools/frontend-regression.mjs

## 验收标准

- src/store.ts 拆为 store/ 下的领域 slice；**每个文件 ≤400 行**（行数用可信口径：LF 字节数 = Python splitlines() = .NET ReadAllLines()；不得用 PowerShell Measure-Object -Line）
- 对外导出面不变：`useAppStore` 仍从 src/store.ts 导出且其**公开类型（AppState 及其成员）不变**；`export * from './store/selectors'` 与类型再导出保持可用；所有既有 import 路径（组件 import { useAppStore } from '../store'）零改动
- 行为不变：npm run test:frontend **141/141 全部通过**（既有 26 + TASK-048 新增 115），且 tools/frontend-regression.mjs **未被改动**
- slice 间通过 get()/set() 的交叉调用仍成立；**不得出现同名 key 被后展开的 slice 覆盖**（需给出检测证据）
- 每个 slice 的 state 片段类型有独立命名（如 XxxSlice），AppState = 各 slice 的交集，且组合后类型可编译（tsc -b 通过）
- npm run lint 0 warnings / 0 errors；npm run build 通过
- 不得改变 D1–D5 的现状行为：测试 141 项通过即包含这一点；若为完成拆分必须改动某处行为，**停下来报告**而不是自行决定
- 提供「纯搬运」证据：对拆分前后做 store 的**行为等价核对**（可用既有 141 项 + 额外说明哪些 action 被放在哪个 slice），并列出 state 键与 action 名的**完整清单**证明无遗漏、无重复
- 文本文件必须 LF 行尾（.gitattributes 为 * text=auto eol=lf）；用脚本写文件时显式指定行尾

## 测试适用性

- 既有行为：保持
- 原始基线：PASS；2026-09-17 基线（TASK-048 之后）：npm run lint 0 warnings/0 errors；npm run build ✓；npm run test:frontend 141/141（既有 26 + 新增 115）退出码 0，连跑 4 次断言序列逐行一致。该套件为纯状态机测试（无 DOM/网络/Tauri），已被独立审查以 9 组变异测试验证有鉴别力（9/9 被杀死且失败项落在对应领域）——因此它是本次拆分**唯一可信的行为回归网**。Rust 侧 120 passed/0 failed/23 ignored（本任务不改 Rust）。
- 基线证据：.workflow-kit/tasks/evidence/baseline-2026-09-17-task049.md
- 需求决定：DEC-a2c6584f8d4442daa3a65eb0bcc7ddaf
- 保留：tools/frontend-regression.mjs 的 141 项断言（含既有 26 与 TASK-048 新增 115）；拆分必须证明行为未变；该套件是唯一的行为契约保护，且已经过变异测试验证其鉴别力。只许通过，不许改；验证：frontend
- 保留：src/store.ts 的对外导出面与 AppState 公开类型；组件与其他模块按既有路径 import；导出面变化会波及全仓，超出「拆小」的目的；验证：build, frontend
- 保留：D1–D5 的现状行为；本任务是结构重排不是修缺陷；把两者混在一起会让审查无法区分「搬错了」与「改对了」；验证：frontend

## 执行与恢复

- 首次开始：2026-09-17T09:37:03.336785Z
- 原截止时间：2026-09-17T13:37:03.336785Z
- 当前截止时间：2026-09-17T13:37:03.336785Z
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-17T10:01:20.176928Z：Required gate failed: build；下一步：先核对已有文件及原始日志，再处理 test_failure；不要新建任务或重置预算
- 2026-09-17T10:01:26.070706Z：Required gate failed: build；下一步：先核对已有文件及原始日志，再处理 test_failure；不要新建任务或重置预算
- 2026-09-17T10:01:31.999446Z：Required gate failed: build；下一步：先核对已有文件及原始日志，再处理 test_failure；不要新建任务或重置预算
- 2026-09-17T10:01:37.921653Z：Required gate failed: build；下一步：先核对已有文件及原始日志，再处理 test_failure；不要新建任务或重置预算
- 2026-09-17T10:01:43.800003Z：Required gate failed: build；下一步：先核对已有文件及原始日志，再处理 test_failure；不要新建任务或重置预算
- 2026-09-17T10:01:49.700853Z：Required gate failed: build；下一步：先核对已有文件及原始日志，再处理 test_failure；不要新建任务或重置预算
- 2026-09-17T10:30:03.080758Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-17T10:40:56.428682Z：当前候选的测试与审查通过；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-049.json)

- [RUN-f171c12a8b4d473f8a820ee3af2b2854](../runs/RUN-f171c12a8b4d473f8a820ee3af2b2854.json)
- [RUN-3a52c7f3295f4b17a6b367358ace8f54](../runs/RUN-3a52c7f3295f4b17a6b367358ace8f54.json)
- [RUN-ca4f154f15a24c09b76007d05d05295c](../runs/RUN-ca4f154f15a24c09b76007d05d05295c.json)
- [RUN-cdc48b7973a949f2bd4a2fcd37b10436](../runs/RUN-cdc48b7973a949f2bd4a2fcd37b10436.json)
- [RUN-14d97a98ac1041f1b6c115c11197bb46](../runs/RUN-14d97a98ac1041f1b6c115c11197bb46.json)
- [RUN-2e661bc6630e4644884173fb8c89cb2f](../runs/RUN-2e661bc6630e4644884173fb8c89cb2f.json)
- [RUN-e8c080056d39447795165143a4fc5836](../runs/RUN-e8c080056d39447795165143a4fc5836.json)
- [RUN-3109bd90df2b408b844736ad26612e4b](../runs/RUN-3109bd90df2b408b844736ad26612e4b.json)
- [RUN-9de75af90f76452d879c109e77e53c11](../runs/RUN-9de75af90f76452d879c109e77e53c11.json)
- [RUN-64219aaa03d141a48ae0e7ae8a7ae73c](../runs/RUN-64219aaa03d141a48ae0e7ae8a7ae73c.json)
- [RUN-6ff6952446b04f1cacb175095075dd31](../runs/RUN-6ff6952446b04f1cacb175095075dd31.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
