# UI 契约：TASK-041 遗留收尾（REQ-006 措辞 + REQ-008 控件可达性与一致性）

冻结时间：2026-09-17（本地）。本文件在 `prepare` 时进入候选快照并成为受保护路径，
任务执行期间不得修改；如需变更范围，走 review 反馈或新任务。

## 0. 背景与范围边界

TASK-041 已验收，但其独立复核（`.workflow-kit/tasks/evidence/_AUDIT-TASK-041-042-independent.md`）
确认契约 `UI-CONTRACT-REQ-006-008.md` 有四项未真正完成。本任务只收尾这些已定位的遗留：
焦点可达性、措辞口径、按钮收敛、提示残留。**不做**重构、不加依赖、不改布局与配色、不改任何既有鼠标行为。

## 1. 焦点可达性（核心项）

### 现状（根因）

焦点规则（`src/styles/base.css` 的 `button/input/textarea/select/a[href]/[role="button"]/[tabindex]:focus-visible`）
只命中原生控件与带 `role`/`tabindex` 的元素。但下列**真实交互控件**是 `div`/`img` + `onClick`，
既无 `role` 也无 `tabIndex`，因此既不进 Tab 序列、也拿不到焦点环。

### 必须修复的清单（按文件；行号为冻结时示意，按类名/组件定位为准）

| # | 元素 | 位置 | 必需结果 |
|---|---|---|---|
| 1 | `.article-card` | `src/components/Timeline.tsx`（列表卡片） | 键盘可选中打开，焦点可见 |
| 2 | `.feed-leaf-item` | `src/components/Sidebar.tsx` | 键盘可切换订阅源，焦点可见 |
| 3 | `.feed-folder-title` | `src/components/Sidebar.tsx` | 键盘可展开/收起分类，焦点可见 |
| 4 | `.flux-dropdown-trigger` | `src/components/primitives.tsx` | 键盘可打开，焦点可见，`aria-expanded` 正确 |
| 5 | `.flux-dropdown-option` | `src/components/primitives.tsx` | 键盘可选择 |
| 6 | `.ctx-menu-item` | `src/components/ContextMenu.tsx` | 键盘可执行菜单项 |
| 7 | `.player-progress-track` | `src/components/PlayerBar.tsx`（迷你条） | 键盘可 seek |
| 8 | `.player-progress-track.player-full-track` | `src/components/PlayerBar.tsx`（全屏） | 键盘可 seek |
| 9 | `.podcast-card` | `src/components/Timeline.tsx` | 键盘可播放，焦点可见 |
| 10 | `.social-text` | `src/components/Timeline.tsx` | 展开/收起可由键盘触发（或在报告中说明为何不需要） |
| 11 | 画廊 `<img>` 与 `.gallery-no-image` | `src/components/Timeline.tsx` | 键盘可开灯箱 |
| 12 | `.lightbox-overlay` | `src/components/Overlays.tsx` | 键盘可关闭（Esc 已存在则说明即可） |

### 必须遵守的实现约束

1. **不得制造 Tab 陷阱或 Tab 爆炸**：列表卡片这类成组控件必须用 **roving tabindex**
   （组内只有一个可 Tab 进入，组内用方向键移动），不得让 Tab 逐个走过可见卡片。
   单列布局已有 J/K 键盘流（`src/App.tsx`），新增机制须与它一致、不冲突。
2. **激活等价**：可聚焦元素必须能由 Enter/Space 触发与鼠标点击**相同**的动作，
   不得出现「能聚焦但按键无反应」。
3. **语义正确**：`role` 必须与行为一致（按钮用 `button`、菜单项用 `menuitem`、
   进度用 `slider` 并带 `aria-valuenow/min/max`）。纯装饰容器不得加 `role`。
4. **不改既有鼠标行为**：点击结果、跳转目标、计数口径、状态流转一律不变。
5. 焦点环复用既有 accent 语言，不新增颜色或令牌。

## 2. 措辞统一（REQ-006 第 1 条「同一动作一套说法」）

### 2.1 侧栏同步状态指示器不得混用「刷新/同步」

`src/components/Sidebar.tsx` 的状态标签在同一三元表达式里同时出现「刷新中…」
与「同步失败 / 后端已同步」。必须收敛为**一套词**：涉及后端状态用「同步」，
涉及抓取动作才用「刷新」，同一指示器内不得混用。

### 2.2 单源刷新与全量刷新各自唯一

- 刷新**单个**订阅源：全应用统一为「刷新此源」。
- 刷新**全部**订阅源：全应用统一为「刷新全部订阅源」。
- 相关位置至少覆盖 `src/components/Sidebar.tsx`（按钮 title 与工具栏）、
  `src/components/ContextMenu.tsx`、`src/store.ts`（toast 文案）。

## 3. 控件收敛（REQ-008）

1. **纯图标按钮**：`src/components/SettingsModal.tsx` 中 2 处「编辑」按钮为纯图标
   （仅一个 `<Icons.edit/>`），当前走 `.toggle-action-btn.icon-btn { padding: 4px 7px }`。
   必须并入既有两档命中区令牌（`--icon-btn-hit-sm` / `--icon-btn-hit-md`），与同类图标按钮一致。
2. **最显眼主按钮**：侧栏「刷新全部订阅源」当前用独立类 `.sidebar-refresh-all`
   （自带 `background: var(--accent)`、禁用态 `opacity:.6`），未使用统一变体类，
   因此也不吃统一禁用态（`opacity:.45`）。必须改用统一变体类，禁用态与其它主按钮一致。
3. 不得因此改变按钮的可用性判断或点击行为；只改样式归属。

## 4. 提示残留（REQ-006）

`src/components/Timeline.tsx` 的社交与通知译文块在生成中显示 ` ⏳ 翻译中…`
（含 emoji 与前导空格）。TASK-041 报告声称已统一为不带 ⏳，实际未改。
必须清理为与其它生成中提示一致的形式（无 emoji、无多余前导空格、省略号用 `…`）。

## 5. 验证方式（逐条可核）

- 代码层面：`npm run lint`（0/0）+ `npm run build` + `npm run test:frontend`（既有 26 项不得减少或削弱）
- 契约核对：逐条给出「文件:行 → 原文 → 改后」映射表
- 实机核对：`npm run tauri dev` 后用**键盘**实际 Tab 遍历，记录焦点顺序与焦点环可见性
- **诚实声明**：静帧截图不能证明焦点可达性与键盘操作，报告必须区分
  「代码路径核对」与「实机键盘观察」；不得用截图冒充动态证据。
  若某条无法实机验证，须明确标注「未验证」并说明原因，不得默认判过。
