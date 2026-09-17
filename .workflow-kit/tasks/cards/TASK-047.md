<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-047 · 可访问性修复：关闭态浮层不可聚焦（inert），消除 Tab 进入不可见控件

**状态**：done

**目标**：修复复核报告 B-3：关闭态浮层内的控件仍在 Tab 序列中。根因已定位——src/components/primitives.tsx 的 ModalOverlay（:298-318）**无条件渲染** children，只用 `className={`modal-overlay ${open ? 'open' : ''}`}` 切换类名；而 src/styles/base.css:1962 的 .modal-overlay 关闭态仅 `opacity: 0; pointer-events: none`（不改可聚焦性），故关闭时内部约 20 个控件仍可被 Tab 命中——用户会 Tab 进一个看不见的对话框。同一模式还用于 Overlays.tsx:314 的灯箱。修法：利用 React 19 对 `inert` 属性的支持，在浮层未打开时加 `inert`，使其整棵子树不可聚焦、不可交互且从无障碍树中移除；并在关闭时把焦点从浮层内移出（避免焦点留在 inert 子树内）。灯箱、命令面板、右键菜单、小弹窗按同一原则处理。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：.workflow-kit/docs/UI-CONTRACT-REQ-047.md
**界面检查**：默认（全部浮层关闭）状态下，Tab 遍历不得停留在任何不可见控件上, 设置弹窗打开时，其内部控件可正常 Tab 进入并可见焦点环, 设置弹窗关闭后，焦点若原在弹窗内应回到触发元素或 body，且弹窗内控件不再可 Tab, 命令面板（搜索）打开/关闭时同上：关闭后其控件不可 Tab, 灯箱打开时 Esc 可关闭，关闭后其控件不可 Tab, 右键菜单关闭后其菜单项不可 Tab, 小弹窗（新建分类 / 添加源 / 编辑源 / 重命名分类 / 确认对话框）关闭后其控件不可 Tab, Tab 顺序仍为：窗口控件 → 侧栏搜索 → 视图 → 布局 → 工具栏 → 内容区，不因本次改动新增停靠点
**修改范围**：.workflow-kit/docs/UI-CONTRACT-REQ-047.md, .workflow-kit/tasks/evidence/baseline-2026-09-17-task047.md, src/**, tools/frontend-regression.mjs

## 验收标准

- 所有关闭态浮层的子树带 inert（或等效的 aria-hidden + tabindex 管理），实测不可 Tab 进入
- 浮层打开时其控件可正常 Tab 进入，`:focus-visible` 焦点环可见（沿用既有 accent 语言，不新增颜色）
- 关闭浮层后焦点不停留在 inert 子树内（回到触发元素或 body）
- 默认全关状态下，主视图 Tab 停靠点数量与 TASK-043 记录的 16 个一致（不因本次改动增减可见控件）
- 不改既有鼠标行为：点击结果、Esc 关闭链、开合状态流转一律不变
- 前端回归 26/26 通过且 tools/frontend-regression.mjs 未被改动
- lint 0 warnings / 0 errors、build 通过
- 提供实机证据：关闭态 Tab 不进入浮层控件、打开态可进入且焦点环可见、关闭后焦点位置正确

## 测试适用性

- 既有行为：保持
- 原始基线：PASS；2026-09-17 基线（TASK-044/045 后）：前端 lint 0/0、build 通过、前端逻辑回归 26/26（纯 Zustand 状态机测试，退出码 0）；Rust 侧 120 passed / 0 failed / 23 ignored（本任务不改 Rust）。该套件不覆盖焦点/可聚焦性（grep focus|querySelector|getComputedStyle|tabIndex|role= 命中 0），故本任务的正确性依靠 UI 证据与人工核对，回归套件只证明状态流转与 IPC 未被破坏。
- 基线证据：.workflow-kit/tasks/evidence/baseline-2026-09-17-task047.md
- 需求决定：沿用既有行为，无新增业务取舍
- 保留：tools/frontend-regression.mjs 的 26 项断言；本次改动不涉及状态机；既有断言必须原样通过，不得削弱或跳过；验证：frontend
- 保留：既有浮层开合与 Esc 关闭链（App.tsx:199-211）；inert 只影响可聚焦性与交互，不得改变 Esc 逐层关闭的既有行为；验证：frontend, lint

## 执行与恢复

- 首次开始：2026-09-17T08:17:24.693223Z
- 原截止时间：2026-09-17T12:17:24.693223Z
- 当前截止时间：2026-09-17T12:17:24.693223Z
- 已用修复轮：1
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-17T08:17:24.754983Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后调用 finish，再运行 verify
- 2026-09-17T08:27:30.969387Z：编码结果已记录，差异范围已核对；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-17T08:27:45.219375Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-17T08:39:26.683548Z：Review requires changes; inspect the findings；下一步：先核对已有文件及原始日志，再处理 review_failure；不要新建任务或重置预算
- 2026-09-17T08:39:27.656864Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后调用 finish，再运行 verify
- 2026-09-17T08:44:20.867732Z：编码结果已记录，差异范围已核对；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-17T08:44:59.705500Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-17T08:52:23.976329Z：当前候选的测试与审查通过；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-047.json)

- [RUN-5bd0750260a349ee88748800987b7d25](../runs/RUN-5bd0750260a349ee88748800987b7d25.json)
- [RUN-21b2f218b6cb40088dc8692da7f18704](../runs/RUN-21b2f218b6cb40088dc8692da7f18704.json)
- [RUN-09eb35b7655744e3a1c7fcfdff72570c](../runs/RUN-09eb35b7655744e3a1c7fcfdff72570c.json)
- [RUN-bb470e5e13dd4b6fa5717ba80b4349bc](../runs/RUN-bb470e5e13dd4b6fa5717ba80b4349bc.json)
- [RUN-4683feec76e44f74a8ae9ebd34cc6dfc](../runs/RUN-4683feec76e44f74a8ae9ebd34cc6dfc.json)
- [RUN-5aef5bbd84344a8c962c1b031792593c](../runs/RUN-5aef5bbd84344a8c962c1b031792593c.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
