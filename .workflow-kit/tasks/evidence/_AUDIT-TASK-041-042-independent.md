# 独立复核：TASK-041 / TASK-042 是否如实优化完毕

复核时间：2026-09-17（本地）。
复核者：总控会话（未参与 TASK-041/042 实现）。
复核方式：不依赖任务卡与自述报告的结论，直接对源码、git 历史、构建产物、候选哈希与门禁实跑做交叉核对。
说明：本文件是复核工作记录，尚未进入任何任务的证据链；结论需用户确认后才转化为任务。

## 0. 复核基线（我自己实跑，非引用报告）

| 项 | 结果 | 命令 |
|---|---|---|
| lint | 0 warnings / 0 errors（24 files / 116 rules） | `npm run lint` |
| build | 通过，`✓ built in 127ms`，产物 `dist/assets/index-Bt9W-GZ6.css` 59.90 kB | `npm run build` |
| 前端回归 | 26/26，退出码 0 | `npm run test:frontend` |
| 候选哈希复核 | 候选 `059880b6…` 的 **17/17 文件**与当前工作区逐一 sha256 一致 | 见下 |

候选哈希核对的意义：TASK-042 的门禁（01:16:45）与源码最后修改（01:11:20）、提交（01:33:08）时序正确，
且**当前代码仍等于被验证的候选**，故门禁与审查证据对当前代码依然有效——这一点报告未证明，由本次复核补上。

## A. 已核实「如实完成」的部分

1. **ASCII 省略号清零（契约硬性条款）**——`src/` 字符串字面量与 JSX 文本中 `...` 计数为 **0**；
   其余 94 处 `...` 全部是 JS 展开/剩余语法，非用户可见文案。
2. **感叹号清零**——JSX 文本节点 0 处；5 处 `!` 均为 JS 取反或模板表达式。
3. **布尔控件统一**——`type="checkbox"` 全仓仅剩 `primitives.tsx:148`（`SwitchTrack` 自身实现），
   `accentColor` 旧写法 0 处；`SwitchInline` 实际使用 11 处（Overlays 6 + SettingsModal 4 + 定义 1），
   4 处 `compact` 档均已传参。报告称「替换 10 处」与代码相符。
4. **图标按钮尺寸收敛**——`tokens.css:15-16` 两档令牌存在，`base.css:3403-3426` 三个类共用同一盒模型。
5. **`.list-entering` 死代码修复**——`Timeline.tsx:63-67` 真实接线，key 为
   `布局|视图筛选|订阅筛选|未读筛选|排序`，与报告声称的五个维度完全一致。
6. **下拉菜单「先挂载、下一帧置 open」**——`primitives.tsx:33-39` 用 rAF 补类名；
   并移除了会盖死 from 态的内联 `transform`（`primitives.tsx:56-61` 改用 `classList` 维护 `drop-up`）。
7. **播放条 / 全屏播放器双向过渡**——`PlayerBar.tsx:200` 与 `:302` 确为常驻挂载；
   `.podcast-bottom-bar` 的 `display:none`（`base.css:1365`）与 `.player-full-overlay`（`base.css:3586`）
   隐藏语义保留，`display` 已列入 transition + `allow-discrete` + `@starting-style` 三要素齐全。
8. **译文块与设置页分类分组（第二轮补齐）**——`base.css:3677-3720` 四块写法一致且三要素齐全；
   `.group-mgr-body` 隐藏态为 `display:none`，不含可聚焦控件，符合「不可聚焦」要求。
9. **`prefers-reduced-motion` 覆盖完整**——`base.css:3744-3766` 用 `*` 全局压平
   `transition-duration`/`animation-duration`，块外无 `!important` 逃逸；
   加载指示器显式豁免（保留旋转）有合理理由。
10. **未违反性能取舍**——`base.css:225-228` 的「不做 grid-template-columns 过渡」注释仍在，
    全文件无该属性的过渡声明。
11. **未发明新时长/缓动**——本轮新增过渡全部取自 `--transition-fast` 或既有
    `0.2s cubic-bezier(0.16,1,0.3,1)`；`bottom 0.25s ease`、`transform 0.18s ease` 等经 git 核对为**既有**代码。
12. **`useEnteringClass` 防冒泡缺陷已修**——`useEnteringClass.ts:26` 校验 `event.target !== el`。

## B. 确认为「未完成 / 与契约不符」的部分

### B-1 焦点可见性覆盖不到自定义可点击元素（契约第 5 条，实质缺口）

契约要求「所有可交互控件（按钮、输入框、文本域、图标按钮、开关）都有可见的 focus-visible 指示」。
焦点规则（`base.css:3380-3389`）只命中 `button / input / textarea / select / a[href] / [role=button] / [tabindex]`。

但主界面大量核心交互是 `div`/`img` + `onClick`，**既无 `role` 也无 `tabIndex`**，
因此既不进入 Tab 序列，也命中不到任何焦点规则。经多行 JSX 解析并逐个人工区分
「真实交互控件」与「仅阻止冒泡的容器」后，**确认为真实控件且缺 role/tabIndex 的共 14 处**：

| # | 元素 | 位置 | 用户后果 |
|---|---|---|---|
| 1 | `.article-card` | `Timeline.tsx:256` | 打开文章的主入口（列表/画廊/社交/通知布局下键盘不可达） |
| 2 | `.feed-leaf-item` | `Sidebar.tsx:210` | 切换订阅源 |
| 3 | `.feed-folder-title` | `Sidebar.tsx:172` | 展开/收起分类 |
| 4 | `.flux-dropdown-trigger` | `primitives.tsx:95` | 全应用所有自定义下拉的触发器 |
| 5 | `.flux-dropdown-option` | `primitives.tsx:103` | 下拉选项 |
| 6 | `.ctx-menu-item` | `ContextMenu.tsx:92` | 右键菜单项 |
| 7 | `.player-progress-track` | `PlayerBar.tsx:227` | 迷你播放条 seek |
| 8 | `.player-progress-track.player-full-track` | `PlayerBar.tsx:313` | 全屏播放器 seek |
| 9 | `.podcast-card` | `Timeline.tsx:515` | 播客卡片播放 |
| 10 | `.social-text` | `Timeline.tsx:348` | 社交正文展开/收起 |
| 11 | `.gallery-no-image` | `Timeline.tsx:482` | 画廊无图占位点击开图 |
| 12 | 画廊 `<img>` | `Timeline.tsx:480` | 点图开灯箱 |
| 13 | `.sidebar-search-pill` 之外的侧栏检索入口 | `Sidebar.tsx:91` | 已带 role/tabIndex，**合规**（列此仅作对照，不计入缺口） |
| 14 | `.lightbox-overlay` | `Overlays.tsx:314` | 点遮罩关闭灯箱（次要，有 Esc 兜底） |

以下 8 处经核实**只是事件容器**（仅 `stopPropagation` 或承载子控件），不计入缺口：
`ContextMenu.tsx:90`、`primitives.tsx:238/239/265/271`、`SettingsModal.tsx:80/484`、
`PlayerBar.tsx:303`、`Reader.tsx:283`（正文区代理点击 `<a>`，由子元素语义承担）。

其中第 4、5、6 项（下拉触发器与选项、右键菜单项）影响面最大：
设置页所有 FluxDropdown 与所有右键菜单都只能鼠标操作。契约要求「所有可交互控件都有可见焦点」，
这 12 处真实控件既不进 Tab 序列也无焦点环（第 13 项合规、第 14 项次要），且报告未披露。

报告对此的表述是验收标准原文「**所有**可交互控件具备可见的 focus-visible 指示」，
但报告同时承认「按钮、开关、文本域的焦点环本轮未获实机目视确认」——
即**既没有实机证据，覆盖范围本身也不完整**；报告只披露了「合成 Tab 未驱动 WebView2 焦点遍历」
这一测试手段受限，未披露上述控件根本不在覆盖范围内。

### B-2 「演示模式」提示前缀未统一（契约第 3 条，残留）

toast 已统一为「演示模式不支持…」前缀（`store.ts:511/663/766/1446`、`SettingsModal.tsx:409/418/715/940`），
但设置页同一语义处仍是另一套措辞且报告未披露：

- `SettingsModal.tsx:976`：`title="浏览器开发模式"`（应为「演示模式」口径）
- `SettingsModal.tsx:977`：`<span className="about-arch-tag">Mock 模式</span>`

同一屏里 toast 说「演示模式」、卡片标题说「浏览器开发模式 / Mock 模式」，属契约明确禁止的口径分叉。

### B-3 关闭态弹窗仍可被键盘聚焦 / 读屏读到（契约要求的「不可聚焦」未满足）

`.modal-overlay` 关闭态只做 `opacity:0` + `pointer-events:none`（`base.css:1974-1975`），
**没有 `inert`、也没有 `aria-hidden`**；全仓仅 `Overlays.tsx:238` 用过 `aria-hidden`（且无关）。
弹窗是常驻挂载（`App.tsx:312-315`），因此关闭后其内部输入框（如
`Overlays.tsx:273` 的 `.search-modal-input`）仍在 DOM 与 Tab 序列中。

需要如实标注的两点：
- 该结构在 TASK-041 **之前**就已如此（`git show 01a176a^:src/App.tsx` 亦为常驻挂载），**不是本轮新引入的回归**；
- 但 TASK-042 契约把「不可见时仍不可聚焦、不被读屏读到」写成对**本轮改动对象**的要求，
  而本轮把 `.group-mgr-body` 改成常驻（该处用 `display:none`，合规），
  却未处理同样常驻、且隐瞒态不是 `display:none` 的弹窗遮罩——属覆盖不完整，应在报告「未覆盖项」中披露，实际未披露。

### B-4 B-1 的部分缓解：存在 J/K 键盘流，但仅限 article 布局

`App.tsx:229-243` 实现了 J/K 切换卡片，但 `App.tsx:231` 有硬门禁
`if (s.activeContentLayout !== 'article') return;`。
因此：

- 在 article 布局下，键盘可选中文章 → B-1 对该布局的严重性下降（但仍无**可见焦点指示**，
  用户无法看出当前焦点在哪；契约要的是「可见的 focus-visible 指示」）；
- 在列表 / 画廊 / 社交 / 通知 / 播客布局下，`div.article-card` 等既不可 Tab 也无 J/K 兜底 → 键盘完全不可操作。

附带发现（非本轮范围，属既有问题）：设置页快捷键表（`SettingsModal.tsx:1411`）把 J/K 的作用域写成
「时间流」，与实际「仅 article 布局」不符；同表也未列出 `App.tsx:215` 已实现的 Space 播放/暂停
（该缺口 FINDINGS-REQ-007.md 的 P3 备注已记录）。

### B-5 「同一动作一套说法」未达成（契约第 1 条，明确不成立）

契约第 1 条要求「同一动作在全应用只用一套说法：标为已读/未读、查看原文、刷新、显示全文」。
「标为已读/未读」「查看原文」「显示全文」三组已统一，但**「刷新」这组混用两套词**：

| 位置 | 文案 | 问题 |
|---|---|---|
| `Sidebar.tsx:71`、`:73` | 忙碌态「刷新中…」 | 同一状态机内 |
| `Sidebar.tsx:75` | `syncStatus === 'error'` → 「同步失败」 | 与上行同属一个指示器 |
| `Sidebar.tsx:77` | 「后端已同步」 | 同上 |
| `store.ts:1195` → `:1166` → `:1198` | 一次操作三次换词：「刷新中…」→「订阅同步完成，正在同步文章状态…」→「已刷新」 | 同一次 `triggerManualSync` |
| `SettingsModal.tsx:902/923/927` | 对同一后端同步全程用「同步」 | 与 store 层口径不一致 |
| `Sidebar.tsx:255`、`ContextMenu.tsx:156` | 「刷新此源」 | 按钮/菜单 |
| `store.ts:1449` | 「正在刷新该订阅源…」 | toast 措辞不同 |

即 `Sidebar.tsx:69-78` 一个三元表达式里同时出现「刷新中…」与「同步失败/后端已同步」。

### B-6 图标按钮尺寸未完全收敛（契约第 8 条，部分不成立）

契约要求「图标按钮尺寸与内边距收敛到一组取值」。两档令牌确实落地，但仍有同类纯图标按钮在名单外：

- `SettingsModal.tsx:522-528`、`:596-602`：`className="toggle-action-btn icon-btn"`，内容**仅一个 `<Icons.edit/>`**，
  却走 `base.css:839` 的 `.toggle-action-btn.icon-btn { padding: 4px 7px }`，
  既非 `padding:0` 也不用 `--icon-btn-hit-sm/md`。
- `Sidebar.tsx:283-289` 的 `.sync-refresh-btn`（`base.css:733`，`padding:4px`）同样未被收敛。

### B-7 主按钮未统一到一个类（契约第 9 条，部分不成立）

契约要求「同类主按钮样式统一到一个类」。最显眼的实心主按钮未统一：

- `Sidebar.tsx:296` 的「刷新全部订阅源」用 `.sidebar-refresh-all`
  （`base.css:671-695`：自带 `background: var(--accent)`、禁用态 `opacity: .6`），
  **未**使用 `.btn-primary`，因此也不吃本轮新增的统一禁用态 `opacity: .45`（`base.css:3482-3485`）。
- `base.css:2989` 的 `.toast-action-btn` 是另一套实心按钮（`background: var(--accent)`、`padding:4px 12px`），
  同样不参与统一禁用规则。

### B-8 报告 F2 声明与代码不符（报告自身遗漏）

TASK-041 报告「修复轮记录」第 306 行称：
「两处同类提示统一为**不带 ⏳**、统一用 `…`，并去掉多余前导空格」。

实测 `Timeline.tsx:386` 与 `:629` **仍是** `{translatingCard ? <span> ⏳ 翻译中…</span> : null}`
——emoji 与前导空格都在，报告未披露这处遗漏。

## C. 报告声称与实际的偏差（诚实性问题）

1. 「所有可交互控件具备可见焦点」——见 B-1，覆盖不全且无实机证据（报告已在别处承认无实机证据，
   但验收标准仍以「所有」表述并据此判过）。
2. 「演示模式提示统一前缀」——见 B-2，设置页残留未披露。
3. 「同一动作一套说法」——见 B-5，「刷新/同步」两套词混用，契约第 1 条不成立。
4. 「图标按钮收敛 / 主按钮统一」——见 B-6、B-7，均只做到部分。
5. 报告「修复轮记录」称已去除 `⏳` 与前导空格——见 B-8，`Timeline.tsx:386/629` 仍在。
6. 报告承认「静帧无法证明动态效果、未做录屏或逐帧观察」，
   而 REQ-005 的验收标准是「关键交互动画流畅自然，**可观察验证**」。
   即：**代码级证据充分，用户可感层面的验收证据缺失**——这是 TASK-042 最实质的验收缺口。

## D. 证据链时效性（本次新增核对）

- TASK-041 报告引用的构建产物 `dist/assets/index-C5bTmR8B.css` 已不存在
  （本次复核重新构建，产物改为 `index-Bt9W-GZ6.css`），
  故「产物级验证」这条证据**已无法复核**；报告中的该项结论只能按当时记录采信。
- TASK-042 的门禁日志（01:16:45）早于提交（01:33:08）但晚于源码最后修改（01:11:20），
  且候选哈希 17/17 与当前工作区一致 → 该门禁证据**仍然有效**。

## E. 结构性缺陷（本次复核发现，比「做没做」更值得处理）

### E-1 实现者自己撰写了本次任务的验收契约（治理问题）

两份契约都是在**各自的实现提交里被创建的**，而两份任务卡又把该契约列为受保护路径：

| 契约文件 | 创建提交 | 任务卡 `ui_contract_ref` | 是否在 `protected_paths` | `allowed_paths` |
|---|---|---|---|---|
| `UI-CONTRACT-REQ-005.md` | `fb46404`（TASK-042 实现提交） | TASK-042 | 是 | 仅 `src/**`、`tools/frontend-regression.mjs` |
| `UI-CONTRACT-REQ-006-008.md` | `01a176a`（TASK-041 实现提交） | TASK-041 | 是 | 同上 |

后果：`ui_checks` 与「验收要点」由**被审查方自己定义**，
而审查者被要求「逐条核对契约」——存在循环性：漏做某项时，
补写报告或改契约比改代码更容易，第一轮审查判 FAIL 的两条 finding 正是这个模式。
这与 `.workflow-kit/AGENTS.md`「实现阶段不得改验收/门禁」的要求冲突。
未发现被实际利用（两轮审查确实抓出问题并判过 FAIL），但机制上不可依赖。

**建议**：把 `UI-CONTRACT-*.md` 移出实现任务的 `allowed_paths`，
改由定义阶段（prepare 之前）单独提交，使契约对实现阶段不可写。

### E-2 TASK-041 的两张「焦点证据」截图是同一张图（证据无效）

经 sha256 实测，TASK-041 的 6 张截图中有两对**字节完全相同**：

| 文件 | sha256 前 16 位 | 字节 |
|---|---|---|
| `TASK-041-01-main.png` | `FF158D2BB01936A0` | 111715 |
| `TASK-041-04-focus-visible.png` | `FF158D2BB01936A0` | 111715 |
| `TASK-041-05-add-feed-dialog.png` | `30E7349A920C9325` | 204725 |
| `TASK-041-06-focus-ring-switch.png` | `30E7349A920C9325` | 204725 |

即文件名声称的「focus-visible 焦点环」截图，内容与主视图/对话框截图逐字节一致——
**焦点证据为零**。报告正文对此是诚实的（承认合成 Tab 未驱动 WebView2 焦点遍历、未目视到焦点环），
但文件命名「04-focus-visible」「06-focus-ring-switch」本身构成误导，
且契约明确要求「不得用截图冒充动态证据」。

### E-3 TASK-041 台账中一条关于测试能力的陈述与代码不符

`TASK-041.json` 的 `test_review.baseline.summary` 写：
「前端回归**不使用任何用户可见中文串做断言**，故文案改动不影响既有断言」。

实测为假：`tools/frontend-regression.mjs:119` 断言
`toasts.some((t) => t.text.includes('全文提取失败'))`，
`:120` 断言 `t.action?.label === '重试'`——两处都是用户可见中文串。

缓解（本次核实）：`git show 01a176a -- src/store.ts` 显示 TASK-041 **没有改动**这两个串
（现仍在 `store.ts:418`、`:605`），故未造成实际回归。
但「26/26 全绿 ⇒ 文案未被破坏」的推理链比台账声称的**更弱**，台账该句应更正。

### E-4 两次任务的候选快照都漏了被改动的文件（同类缺口）

- TASK-042：`src/hooks/useEnteringClass.ts` → 改为 `src/components/useEnteringClass.ts`。
  该路径**从未进入 git 历史**（`git log --all --name-status -- "src/hooks/*"` 为空），
  当时是未跟踪文件；三个引用方均已是 `./useEnteringClass`，无残留引用。
  台账订正 #1 把它描述为「被删除的」文件，在 git 语境下措辞失实，**净效果是路径迁移**。
- TASK-041：报告自述改了 `src/mockData.ts`（省略号修复），
  但该文件**不在** TASK-041 的 `snapshot_paths` 覆盖内，且从未订正、未被审查点出。

### E-5 TASK-042 的验收记录尚未入库

`DEC-f7376cf8…-acceptance.json` 是五份 acceptance 中**唯一未跟踪（untracked）**的，
即 TASK-042 的「已验收」目前只存在于本地工作区；
`TASK-042.json` 的 `status: verified→done`、`acceptance_ref`、`merge_ref` 三处也仍是**未提交改动**。

## F. 边界（本次复核未能覆盖）

- 未做实机录屏或逐帧观察，无法判断动画「观感是否流畅自然」（REQ-005 的用户可感验收项）。
- 未在 WebView2 上实测 `allow-discrete` / `@starting-style` 的实际表现与旧内核降级行为。
- 未逐一复跑 TASK-041 的全部截图场景（本机数据库为空，主视图为空态）。
- TASK-041 报告承认未覆盖：四种布局卡片本体、阅读器正文区与 AI 区块、右键菜单、灯箱、toast、
  分类管理紧凑档开关、浅色主题 —— 这些仍未获实机覆盖。
- `src/styles/base.css` 中文注释中约 115 个 U+FFFD 替换字符为**既有**损坏。
  经精确复核（Python 逐版本计数）：`01a176a^` = **118**、`01a176a` = **115**、HEAD = **115**、工作区 = **115**，
  与 TASK-041 报告「HEAD 为 118，本轮移除 3 个」的声明**完全一致**，此项报告属实。
  （注：用 PowerShell `Get-Content -Raw` 读该文件会得到 0，是编码伪影，不可作为判据。）
- 未在 WebView2 上实测退化路径；未核 `fb46404` 之前工作区的中间状态（`src/hooks/` 从未入 git）。

## G. 「26/26 全绿」这条证据的有效边界（两个任务都在用）

`tools/frontend-regression.mjs` 是**无浏览器**的 Zustand 状态机测试
（`frontend-regression.mjs:1-6`：伪造 `window.__TAURI_INTERNALS__`，用 `__INVOKE__` mock 后端命令，
断言的是 store 行为与 IPC 调用参数，例如 S-5 断言 `mark_all_read` 是否带 `starredOnly`）。

因此该套件：

- **能**证明：状态流转与 IPC 调用口径未被破坏（回归保护）。
- **不能**证明：任何 DOM 渲染结果、任何 CSS 过渡或动画
  （对 `animation|transition|opacity|classList|focus|entering|prefers` 的 grep 为 0 命中，
  且套件只 import `dist-test/store.js`，不加载 CSS）。
- **并非**「不断言中文串」：`:119`、`:120` 确实断言了中文（见 E-3）。

推论：TASK-041 的「文案与控件一致性」与 TASK-042 的「动画过渡」的**正确性证据，
实际上完全落在自述报告与静态代码核对上**，26/26 对本轮改动内容不构成验收证据。
TASK-042 的任务卡与报告在这点上表述准确（明说回归不检查动画细节）；
TASK-041 的台账表述有误（见 E-3）。

## H. 最终判断

- **「报告说做了、实际没做」不成立。** 两次优化的代码改动真实存在、门禁可复现、
  TASK-042 的候选摘要 `059880b6…` 与当前 HEAD 的 17 个文件逐一哈希一致
  （我已独立重算），故其门禁与审查证据至今有效。
- **但 TASK-041 不能判定为完整达成契约。** 按 `UI-CONTRACT-REQ-006-008.md` 逐条判定：

  | 契约条款 | 判定 | 依据 |
  |---|---|---|
  | 1 同一动作一套说法 | **不成立** | B-5（刷新/同步混用） |
  | 2 省略号一律 `…` | 成立 | 字符串字面量与 JSX 文本 0 处 ASCII 省略号 |
  | 3 冒号一律 `：` | 成立 | 用户可见文案无残留（`0:00` 属时间格式） |
  | 4 全仓 0 感叹号 | 成立 | JSX 文本 0 处 |
  | 5 所有可交互控件可见焦点 | **不成立** | B-1（12 处真实控件无 role/tabIndex，规则命中不到） |
  | 6 布尔控件统一 | 成立 | 仅剩 `SwitchTrack` 自身实现 |
  | 7 图标按钮尺寸收敛 | **部分成立** | B-6 |
  | 8 同类主按钮统一到一个类 | **部分成立** | B-7 |

- **TASK-042 的代码实现基本达成契约**（A 节 12 项经核对成立），
  但其验收标准里的「可观察验证」没有任何证据支撑，且 E-2 类问题（截图空证据）同样存在于 TASK-041。
- 综合：**需要的不是重做，而是收尾**——补齐 B-1/B-5/B-6/B-7/B-8 五处遗留，
  更正 E-3 台账陈述，并把 E-1 的契约定权问题从流程上修掉。
