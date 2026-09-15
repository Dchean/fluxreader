# TASK-039 UI 审查证据（独立上下文审查者产出）

审查对象：candidate f8c270d023b1137a46ea226fe86648ced0fa4ec5ca909da6684f27424f44ffa6
契约：`.workflow-kit/docs/UI-CONTRACT-REQ-004-008.md`
审查者：独立 Explore 上下文（未参与实现）；方式：diff 逐条核对 + 复跑门禁。运行时目视为待办（见末节）。

## 逐状态核对（对照任务卡 ui_checks）

1. **仅迷你播放条可见时 toast 位于右下、不遮挡播放条（bottom 96px 行为保留）**
   通过（代码级）。`body.has-player` 由 `player.isActive` 切换（App.tsx:53-56），CSS `body.has-player .toast-layer { bottom: 96px }`（base.css:2968）未被改动；迷你栏在 `player.isActive` 时恒渲染（PlayerBar.tsx:190 仅在非 active 时 return null），避让语义与改动前一致。

2. **全屏播放器展开时 toast 回到右下贴底并浮于播放器覆盖层之上、操作按钮可点击**
   通过（代码级）。App.tsx:60-63 在 `playerExpanded && playerActive` 时切换 `body.has-player-expanded`；CSS `body.has-player-expanded .toast-layer { bottom: 24px }`（base.css:2974）与 `has-player` 规则同特异性（0,2,1）且位于其后，展开态必胜回 24px。层级：toast-layer z-index 500（base.css:2962）> `.player-full-overlay` 260（base.css:3215），< `.confirm-overlay` 3000（base.css:2603）。`.toast-pill.with-action { pointer-events: auto }`（base.css:3013）保证「重试」按钮在覆盖层之上可点击。
   三态组合完整性：expanded&&active→24px；!expanded&&active→96px；expanded&&!active→两 class 均不置→基础 24px（无播放器语义正确）。

3. **无播放时 toast 右下贴底（不回退既有行为）**
   通过（代码级）。基础规则 `.toast-layer { bottom: 24px }`（base.css:2957）未改动；无播放时两个 body 类都不置。

4. **设置→同步协议下拉与全应用 FluxDropdown 一致（展开方向/Esc/点击外部/深色主题）**
   通过（代码级）。SettingsModal.tsx:1001-1009 由原生 `<select>` 改为 `FluxDropdown`，value/onChange/两个选项一一对应，`v === 'fever' ? 'fever' : 'greader'` 钳制保留。菜单 `createPortal` 到 body + `position: fixed` + `zIndex: 2000`（primitives.tsx:40-44,82-103），逃逸 `.settings-modal { overflow: hidden }`（base.css:1995）；`updatePosition` 的 `openUp` 分支负责视口底部向上弹；监听 window scroll（捕获相位）跟随设置弹窗内容区滚动重定位。Esc：primitives.tsx:55-59 在 document 捕获相位 `stopPropagation`，早于 App.tsx:245 的 window 冒泡监听 → 只关下拉，不连带关闭设置弹窗；点击外部由 document 捕获 click 实现。

5. **下拉选项为空时不崩溃且保持当前值显示**
   通过（代码级）。options 为内联字面量恒 2 项（该状态不可达）；`current?.label` 可选链保护（primitives.tsx:74,79），空数组不抛错。

## 附带核对

- 全仓原生控件现状：`<select>` 0 处（仅注释命中）；11 处原生 `<input type="checkbox">` 均为已样式化的多选语义控件（`.mgr-checkbox-label` / `.mini-dialog-checkbox` / `.switch-control`），非本次范围。
- 范围：diff 仅触及 App.tsx、SettingsModal.tsx、base.css，未越界。
- 复跑：`npm run lint` 0 警告 0 错误；`npm run test:frontend` 21/21 通过。

## 非阻塞观察（留档）

- FluxDropdown 触发器是无 tabIndex/role 的 div：相对被移除的原生 select 属键盘/读屏可访问性回退（与全应用其他 12 处下拉一致）。建议作为通用增强项（tabIndex + role=combobox + 方向键）列入后续批次。
- 宽度 220 与同页 240 的 `setting-input` 存在 20px 装饰性参差，需实机目视确认可接受度。
- 契约文中「设置弹窗 150/250」的 250 为笔误（实际仅 150）。

## 待运行时目视（用户验收或 GUI 运行确认）

- A1/A2/A3：迷你栏、全屏展开、无播放三种场景的 toast 实机位置与遮挡
- A4：展开/收起切换过程中 0.25s 过渡的平滑度
- B5：深色主题下下拉展开/选中高亮外观，及 220 宽度观感
- B6：把协议行滚到视口底部附近，确认菜单向上弹出且不被裁切
- B7：Esc 与点击弹窗外的手感、可见焦点样式
