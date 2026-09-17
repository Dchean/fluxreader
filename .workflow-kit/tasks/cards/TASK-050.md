<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-050 · 前端拆分二：SettingsModal.tsx 按既有函数边界拆为设置页子模块（纯移动）

**状态**：done

**目标**：把 src/components/SettingsModal.tsx（1513 行）拆小。可行性评估的关键事实：该文件**已经天然分解为 14 个顶层函数**（每个标签页/区块一个，最大 SyncTab 293 行），因此本任务是**纯移动**而非重写——就是把既有函数搬到 src/components/settings/ 下的子模块，模块级共享常量与接口搬到该子目录的共享模块。目标：src/components/SettingsModal.tsx 保留 `export function SettingsModal()`（外壳，App.tsx:7 按 './components/SettingsModal' 导入、:317 渲染，**导入路径必须不变**），其余单元移入 src/components/settings/。硬要求：**JSX、逻辑、文案、类名、布局一律逐字不变**，仅调整文件归属与 import。

**依赖**：TASK-049
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：.workflow-kit/tasks/evidence/baseline-2026-09-17-task050.md, src/**, tools/frontend-regression.mjs

## 验收标准

- src/components/SettingsModal.tsx 只保留**外壳** `export function SettingsModal()`；其余 13 个单元移入 src/components/settings/ 下的子模块；**每个文件 ≤400 行**（可信口径：LF 字节数 = Python splitlines() = .NET ReadAllLines()；不得用 PowerShell Measure-Object -Line）
- **导出面与导入路径不变**：App.tsx 仍 `import { SettingsModal } from './components/SettingsModal'` 且渲染点不变；`git diff HEAD -- src/App.tsx` 应为空
- **纯移动可机械核验**：提供脚本证据，证明每个被移动单元的函数体（去空白规范化后）与拆分前**逐字等价**——不一致处必须为 0；若因 import 调整而有整行差异，须逐类列出且全部可解释（只许 import/声明/导出修饰，不得有业务表达式增删）
- npm run lint 0 warnings / 0 errors；npm run build 通过（`tsc -b` 会类型检查 JSX）
- npm run test:frontend **141/141** 通过且 tools/frontend-regression.mjs 未被改动（证明未波及 store 契约）
- **CDP 实机冒烟证据**（真实 Tauri WebView2，不得注入系统级键鼠、不得抢占用户焦点）：打开设置弹窗，逐个切换全部 8 个标签页（通用/外观/阅读/订阅/AI服务/同步/快捷键/关于），每个标签页都确认 (a) 渲染出预期内容、(b) **无 console 错误**；并至少验证 2 处交互接线仍有效（如切换一个开关、改一个下拉值）
- 不得引入新依赖：package.json 与 lock 文件不得改动（git diff 应为空）
- 文本文件必须 LF 行尾（.gitattributes 为 * text=auto eol=lf）；用脚本写文件时显式指定行尾

## 测试适用性

- 既有行为：保持
- 原始基线：PASS；2026-09-17 基线（TASK-049 之后）：npm run lint exit 0（0 warnings/0 errors）；npm run build exit 0；npm run test:frontend exit 0 且 141/141（既有 26 + TASK-048 新增 115），tools/frontend-regression.mjs 未改动。**关键限制**：该套件是纯 Zustand 状态机测试，tsconfig.test.json 的 include 只含 store/mockData/types/lib 而**不含任何 .tsx**，因此它对本任务的 JSX 与渲染时序**不提供任何证据**；devDependencies 里也没有 jsdom/RTL 等组件测试基建。故本任务的行为证据由「函数体逐字等价证明 + lint/build 的类型检查 + CDP 实机冒烟」三者共同构成，并如实披露组件级自动断言的长期缺口仍未补。
- 基线证据：.workflow-kit/tasks/evidence/baseline-2026-09-17-task050.md
- 需求决定：DEC-a2c6584f8d4442daa3a65eb0bcc7ddaf
- 保留：src/components/SettingsModal.tsx 的导出面与 App.tsx 的导入路径；App.tsx 是唯一调用方；导入路径变化会波及全仓且无收益；验证：build
- 保留：tools/frontend-regression.mjs 的 141 项断言；证明本次拆分未波及 store 契约；只许通过不许改；验证：frontend

## 执行与恢复

- 首次开始：2026-09-17T15:33:43.335890Z
- 原截止时间：2026-09-17T19:33:43.335890Z
- 当前截止时间：2026-09-17T19:33:43.335890Z
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-17T15:33:43.429565Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后调用 finish，再运行 verify
- 2026-09-17T16:17:56.231675Z：编码结果已记录，差异范围已核对；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-17T16:18:23.551921Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-17T16:32:26.036267Z：当前候选的测试与审查通过；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-050.json)

- [RUN-8daffa34fe3b49bf87314ead66606b98](../runs/RUN-8daffa34fe3b49bf87314ead66606b98.json)
- [RUN-3c350415fa5241879c634b0d48b99132](../runs/RUN-3c350415fa5241879c634b0d48b99132.json)
- [RUN-9226e9e12b2447538bdc6030b2f90063](../runs/RUN-9226e9e12b2447538bdc6030b2f90063.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
