<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-039 · REQ-004 播客页 toast 位置 + REQ-008 设置页控件一致性

**状态**：done

**目标**：修复 REQ-004 与 REQ-008。(1) toast 位置：全屏播放器（playerExpanded，覆盖层 z-index 260）展开时迷你播放条已隐藏，但 body.has-player 仍把 toast 层抬高 96px，导致 toast 悬浮遮挡全屏播放器中下部；新增 has-player-expanded 标记（App 随 playerExpanded 切换），展开时 toast 回到右下贴底（24px），并确认 toast 层浮于播放器之上而低于二次确认弹窗。(2) 控件一致性：设置→同步的协议选择是全应用唯一的原生 <select>，改为统一组件 FluxDropdown；同时清理两处残留空 className。

**依赖**：TASK-030, TASK-033, TASK-034
**参考方案**：见 ../RESEARCH.md
**界面约定**：.workflow-kit/docs/UI-CONTRACT-REQ-004-008.md
**界面检查**：仅迷你播放条可见时 toast 位于右下、不遮挡播放条（bottom 96px 行为保留）, 全屏播放器展开时 toast 回到右下贴底并浮于播放器覆盖层之上、操作按钮可点击, 无播放时 toast 右下贴底（不回退既有行为）, 设置→同步协议下拉与全应用 FluxDropdown 一致（展开方向/Esc/点击外部/深色主题）, 下拉选项为空时不崩溃且保持当前值显示
**修改范围**：src/**, tools/**

## 验收标准

- 全屏播放器展开时 toast 位于右下贴底、浮于播放器之上且按钮可点击（UI 契约 A2）
- 迷你播放条场景的 toast 避让行为不回退（UI 契约 A1/A3）
- 同步协议选择改用 FluxDropdown，行为/外观与其他下拉一致（UI 契约 B5–B8）
- 残留空 className 清理；lint 0 警告、前端回归不回归

## 测试适用性

- 既有行为：保持
- 原始基线：PASS；2026-09-15 基线：lint 0/0、前端回归 21/21（S-1/S-2/S-3/S-4）。本次为既有界面内的位置与控件一致性调整（不改版、不改业务行为），UI 契约 .workflow-kit/docs/UI-CONTRACT-REQ-004-008.md 列出待核对状态。
- 基线证据：.workflow-kit/tasks/evidence/baseline-2026-09-15.md
- 需求决定：沿用既有行为，无新增业务取舍
- 保留：现有 21 项前端逻辑回归；store 行为不变，须保持全绿；验证：frontend
- 保留：oxlint 0 警告门禁；静态检查不放松；验证：lint

## 执行与恢复

- 首次开始：2026-09-15T16:46:04.686519Z
- 原截止时间：2026-09-15T20:46:04.686519Z
- 当前截止时间：2026-09-15T20:46:04.686519Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 6 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-15T16:46:04.804954Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后调用 finish，再运行 verify
- 2026-09-15T16:52:59.398624Z：编码结果已记录，差异范围已核对；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-15T16:53:07.050960Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-15T18:05:00.613748Z：当前候选的测试与审查通过；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-039.json)

- [RUN-f21c5797f24e4915b0b106ce7a1991f3](../runs/RUN-f21c5797f24e4915b0b106ce7a1991f3.json)
- [RUN-a3a084ab05a047fe80877affc5033b20](../runs/RUN-a3a084ab05a047fe80877affc5033b20.json)
- [RUN-076fd0bcdb214497bec7add940513c16](../runs/RUN-076fd0bcdb214497bec7add940513c16.json)
- [RUN-8181268c92db4430b9d99405d8118919](../runs/RUN-8181268c92db4430b9d99405d8118919.json)
- [RUN-08bd54dd3de9467c9cfeca77d1be013f](../runs/RUN-08bd54dd3de9467c9cfeca77d1be013f.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
