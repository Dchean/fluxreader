<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-086 · F2：单键快捷键让路浮层的判据抽纯函数并补前端断言

**状态**：ready

**目标**：闭合 AUDIT P3[F2] 遗留的**覆盖缺口**：上一轮（TASK-081）已按审计要求让 S/M/J/K 在任一浮层打开时让路，但该判据**内联在 `src/App.tsx` 的 keydown 闭包里**，无任何断言——「改了行为却无法断言」。处置：按项目既有先例（`timelineSentinel.ts` / `src/store/selectors.ts` 的 `podcastClickAction`）把判据抽成**导出纯函数**`shouldYieldToOverlay(overlayOpen, key, hasModifier)`（新建 `src/components/shortcutYield.ts`，独立成文件以避开 oxlint 的 react/only-export-components），由 App.tsx 的真实 keydown 分支消费，并**由前端回归直接断言**。行为零变化：现有让路语义（浮层打开 + 非 Ctrl/Meta/Alt + 键属 s/S/m/M/j/k）逐条保持。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src/App.tsx, src/components/shortcutYield.ts, tools/frontend-regression.mjs

## 验收标准

- ① 判据抽为 `src/components/shortcutYield.ts` 的导出纯函数，且 `App.tsx` 的 keydown **确实调用它**（不得只建函数不用）
- ② 前端回归新增断言覆盖四种情形：浮层打开+单键 → 让路；浮层关闭+单键 → 不让路；浮层打开+带 Ctrl/Meta/Alt → 不让路（属浮层自身操作）；非 S/M/J/K 键 → 不让路
- ③ 断言须有捕获力并以变异取证：把该函数改回「永不让路」（修前行为）或改变判定条件，必须有断言失败（记录实测的失败用例名）
- ④ 行为零变化：四门禁全绿——cargo test ≥206 且 0 failed、9 ignored 不增；lint 0/0（注意不得因在组件文件 export 非组件函数而引入新警告）；build exit 0；frontend ≥314/314 不回退
- ⑤ 除本卡新增的断言外，既有前端断言一行不动（`既有回归 26/26 通过（未改动一行）` 与其它新增断言计数不变）

## 测试适用性

- 既有行为：保持
- 原始基线：PASS；2026-09-21 基线（TASK-085 验证 RUN-92059f91，提交 d50fd60）：cargo test 206 passed / 0 failed / 9 ignored、lint 0 warnings / 0 errors、build exit 0、frontend 314/314。本任务 behavior=preserve：让路语义已经在位（TASK-081 交付），本卡只把**判据抽成可断言的纯函数**并补断言，不改变任何用户可见行为；故既有全部断言必须原样通过。
- 基线证据：.workflow-kit/tasks/runs/RUN-92059f91051f47888cf95db5be2acb76.json
- 需求决定：沿用既有行为，无新增业务取舍
- 保留：全部既有 206 条 Rust 断言与 314 条前端断言；纯抽取 + 补断言，无行为改动；既有断言（尤其 `既有回归 26/26 通过（未改动一行）`）是「零行为变化」的判据。；验证：cargo_test, frontend
- 补充：shortcutYield 纯函数的四种情形断言（让路/不让路×带修饰键/非目标键）；AUDIT P3[F2] 的遗留正是「修复无断言」这一覆盖缺口；新断言必须对该判据本身取证，并以变异确认可失败，而不是只断言周边状态。；验证：frontend

## 执行与恢复

- 首次开始：None
- 原截止时间：None
- 当前截止时间：None
- 时钟：未开始
- 已用修复轮：0
- 阻塞：无
- 下一步：执行 start/next 获取可继续的动作

## 最近检查点


## 原始证据

[唯一状态记录](../items/TASK-086.json)


卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
