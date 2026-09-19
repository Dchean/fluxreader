<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-042 · 交互动画打磨（REQ-005）：补全过渡与 prefers-reduced-motion

**状态**：done

**目标**：按 UI 契约（.workflow-kit/docs/UI-CONTRACT-REQ-005.md）打磨交互过渡：把已声明却从未被应用的 .list-entering 真正接到列表内容上；下拉/右键菜单改为先挂载、下一帧再置 open；阅读器视图与文章切换、播放条与全屏播放器、折叠展开区块、覆盖层与加载更多改用可过渡的可见性方案；为本次新增或修改的全部过渡补 prefers-reduced-motion 保护。只改过渡与时长曲线、复用既有 token 与曲线，不改布局结构、信息层级、配色与行为。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：.workflow-kit/docs/UI-CONTRACT-REQ-005.md
**界面检查**：列表与视图切换：切换布局/筛选/订阅时列表内容淡入（含 .list-entering 死代码修复）, 下拉菜单与右键菜单：打开时的入场过渡（先挂载、下一帧置 open）, 阅读器：视图切换与文章之间的切换过渡, 播放：底部播放条与全屏播放器的出现/消失过渡, 折叠与展开：侧栏订阅树、设置页分组、通知/译文/AI 区块, 覆盖层：弹窗卡片入场、灯箱图片、设置页分区切换、加载更多状态, 无障碍：prefers-reduced-motion: reduce 下关闭或显著削弱上述全部过渡
**修改范围**：src/**, tools/frontend-regression.mjs

## 验收标准

- 每个改动点都能在代码中定位到具体选择器与时长/曲线，且取值来自既有动效词汇表（--transition-fast、0.2s 同曲线）
- .list-entering 真正被应用到元素上，不再是被声明却未使用的死代码
- 下拉与右键菜单入场为先挂载、下一帧置 open，而不是初始即带 open
- 以 display 切换的区块（AI/译文/通知等）改为可过渡的可见性方案，且不可见时仍不可聚焦、不被读屏读到
- prefers-reduced-motion: reduce 覆盖本次新增或修改的全部过渡
- 不改行为与布局：前端状态机回归 26/26 全绿
- lint 0/0、build 通过
- UI 证据齐备：主视图与设置页截图 + 逐条「选择器 → 原状态 → 改后时长/曲线 → 触发场景」报告，并区分代码核对与实机观察

## 测试适用性

- 既有行为：保持
- 原始基线：PASS；2026-09-16（CI 门禁修复后）基线：lint 0/0（23 文件）、build 通过、前端状态机回归 26/26、cargo fmt --check 干净、cargo clippy -D warnings 零告警、cargo test 全绿、CI 选定 mock e2e 全绿；CI run 35085964805 两 job 全步成功。本任务只改过渡与可见性方案，不改变行为契约；前端回归不检查动画细节，动画正确性依靠代码定位与实机观察证据。
- 基线证据：.workflow-kit/tasks/evidence/baseline-2026-09-16-task042.md
- 需求决定：DEC-batch4-polish-refactor-20260916
- 保留：既有前端状态机回归 26 项（S-1..S-5）；动画改动不得改变交互行为与状态流转，回归必须保持全绿；验证：frontend
- 保留：lint 与 TypeScript/Vite 构建门禁；样式与组件改动仍需类型正确、可构建；验证：lint, build

## 执行与恢复

- 首次开始：2026-09-16T10:58:57.780748Z
- 原截止时间：2026-09-16T14:58:57.780748Z
- 当前截止时间：2026-09-16T19:40:24.523381Z
- 时钟：按墙钟计：额度 450 分钟，写入阶段已用约 193 分钟
- 已用修复轮：2
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-16T16:43:50.508528Z：台账订正（owner 授权）：补全漏列的删除文件，闭合该 repair run；下一步：运行 verify；订正只改记录，代码与候选未变
- 2026-09-16T16:44:31.531839Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-16T17:05:54.070074Z：Review requires changes; inspect the findings；下一步：先核对已有文件及原始日志，再处理 review_failure；不要新建任务或重置预算
- 2026-09-16T17:07:48.675718Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后调用 finish，再运行 verify
- 2026-09-16T17:16:32.651987Z：编码结果已记录，差异范围已核对；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-16T17:16:48.943000Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-16T17:29:44.696760Z：台账订正（owner 授权）：审查证据引用了工具会改写的任务记录而自锁，恢复审查阶段；下一步：用不含可改写台账文件的报告重新记录审查
- 2026-09-16T17:32:40.035398Z：当前候选的测试与审查通过；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-042.json)

- [RUN-6f0e832a53404f5aba4395e9b5d4fd7c](../runs/RUN-6f0e832a53404f5aba4395e9b5d4fd7c.json)
- [RUN-144b9715e60a4c09a2316f513c554b27](../runs/RUN-144b9715e60a4c09a2316f513c554b27.json)
- [RUN-9c9414c0e7f94834b7221abca7acd88d](../runs/RUN-9c9414c0e7f94834b7221abca7acd88d.json)
- [RUN-f42870a50ab1460a91ed38a452125d93](../runs/RUN-f42870a50ab1460a91ed38a452125d93.json)
- [RUN-d96df0d47425428dba073a2de084409f](../runs/RUN-d96df0d47425428dba073a2de084409f.json)
- [RUN-a1989b4a32874dfcb4bd2ef82ca3161f](../runs/RUN-a1989b4a32874dfcb4bd2ef82ca3161f.json)
- [RUN-c765ce94d3b4418aa71e503db85c104b](../runs/RUN-c765ce94d3b4418aa71e503db85c104b.json)
- [RUN-4fbd80b68ea64d4f938e790783c159d3](../runs/RUN-4fbd80b68ea64d4f938e790783c159d3.json)
- [RUN-36c2bbeca0fd40ce853fe75faa4133d4](../runs/RUN-36c2bbeca0fd40ce853fe75faa4133d4.json)
- [RUN-509596721679456e9133448dbf6959a6](../runs/RUN-509596721679456e9133448dbf6959a6.json)
- [RUN-22754520e36a4bb0ba66ec40f88f422e](../runs/RUN-22754520e36a4bb0ba66ec40f88f422e.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
