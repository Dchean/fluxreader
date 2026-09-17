# TASK-043 交互与核对报告（TASK-041 遗留收尾）

采集时间：2026-09-17。范围：`src/**`（`tools/frontend-regression.mjs` 未改动）。

## 0. 证据边界（先声明）

- **代码路径核对**：第 2 节逐条给出「文件:行 → 原文 → 改后」，可对源码逐项复核。
- **门禁实跑**：第 4 节，本会话真实执行。
- **实机键盘观察**：**本轮未做**。理由与影响见第 5 节「未覆盖项」——
  不把静态代码核对冒充为「键盘可用的实机证明」。这是本报告最重要的诚实声明。

## 1. 本轮修复的遗留（对应独立复核 B-1/B-2/B-5/B-6/B-7/B-8 与 E-3）

来源：`.workflow-kit/tasks/evidence/_AUDIT-TASK-041-042-independent.md`
（其 B-1 已在实施时被更正，见该文件内的更正注记）。

## 2. 逐条改动（文件:行 → 原文 → 改后）

### 2.1 焦点可达性（B-1）

根因：焦点规则只命中 `button/input/textarea/select/a[href]/[role=button]/[tabindex]`，
而下列交互是 `div`/`img` + `onClick`，既无 `role` 也无 `tabIndex`，键盘既进不去也没有焦点环。

| 文件:行 | 原文 | 改后 |
| --- | --- | --- |
| `src/components/primitives.tsx:95` | `<div className="flux-dropdown-trigger" onClick={() => setOpen((v) => !v)}>` | 补 `role="combobox"`、`tabIndex={0}`、`aria-haspopup`、`aria-expanded`、`aria-label`、`onKeyDown`（方向键/Enter/Space 开合与提交） |
| `src/components/primitives.tsx:103` | `<div className={...flux-dropdown-option...} onClick={...}>` | 补 `role="option"`、`aria-selected`、`onMouseEnter` 同步高亮；外层菜单补 `role="listbox"` |
| `src/components/ContextMenu.tsx:90` | `<div className="ctx-menu" ...>` | 补 `role="menu"` |
| `src/components/ContextMenu.tsx:92` | `<div className={...ctx-menu-item...} onClick={...}>` | 补 `role="menuitem"`、`tabIndex`（禁用项 -1）、`aria-disabled`、`onKeyDown`（Enter/Space 执行同一动作） |
| `src/components/Timeline.tsx`（ArticleCard） | `<div className="article-card ..." onClick={() => onSelect(art.id)} ...>` | 补 `role="button"`、roving `tabIndex`、`data-card-index`、`onKeyDown`（Enter/Space 打开；方向键在卡片间移动） |
| `src/components/Timeline.tsx`（PodcastCard） | `<div className="podcast-card ..." onClick={() => playPodcastEpisode(...)}>` | 同上（激活＝播放，与点击同源） |
| `src/components/Timeline.tsx`（GalleryCard，有图/无图两支） | `<img ... onClick={openImage} />`、`<div className="gallery-no-image" onClick={openImage}>无图</div>` | 补 `role="button"`、roving `tabIndex`、`onKeyDown`（Enter/Space 开灯箱；方向键移动） |
| `src/components/PlayerBar.tsx:313` | `<div className="player-progress-track player-full-track" onClick={(e) => seekByRatio(...)}>` | 补 `role="slider"`、`tabIndex={0}`、`aria-label`、`aria-valuenow/min/max`、`onKeyDown`（←/→ ±5 秒，Home 回起点） |

**为何用 roving tabindex 而不是逐个 `tabIndex={0}`**：契约 §1.1 明确禁止 Tab 爆炸。
列表卡片与画廊图片是成组控件，故组内只保留一个可 Tab 进入（`tabIndex=0`），
组内用方向键移动、并同步 `scrollToIndex` + 聚焦（虚拟化下目标可能尚未渲染）。
既有 J/K 键盘流（`src/App.tsx`）未改动，与新机制同语义、不冲突。

**健壮性处理**：`tabbableIndex` 对 `items.length` 做夹取。若不夹取，
筛选/切换订阅后 `focusIndex` 越界会导致**没有任何卡片 `tabIndex=0`**，键盘就再也进不了列表
（即"修完反而更糟"）。该夹取保证列表中始终有一张卡可进入。

### 2.2 措辞统一（B-5）

| 文件:行 | 原文 | 改后 |
| --- | --- | --- |
| `src/components/Sidebar.tsx:71` | `? '刷新中…'`（syncStatus==='syncing'） | `? '同步中…'` |
| `src/components/Sidebar.tsx:73` | `? '刷新中…'`（backgroundSyncing） | `? '同步中…'` |
| `src/store.ts:1449` | `showToast('正在刷新该订阅源…')` | `showToast('正在刷新此源…')` |

说明：该指示器描述的是与**后端同步**的状态（同三元里还有「同步失败/**后端已同步**」），
此前却在忙碌态说「刷新中」，一个状态机混用两套词。现统一为「同步」口径；
「刷新此源 / 刷新全部订阅源」保留给**抓取动作**，两者语义不同、不再交叉。

**关于仍存在的 2 处「刷新中…」（对独立审查 finding 5 的明确处置）**：

| 位置 | 所属路径 | 处置 |
| --- | --- | --- |
| `src/components/Sidebar.tsx:304` | 「刷新全部订阅源」主按钮的**忙碌态** | **保留**。该按钮触发的就是抓取动作（`triggerManualSync`），其正常态文案即「刷新全部订阅源」，忙碌态说「刷新中…」与该动作同属一套词，不构成混用 |
| `src/store.ts:1195` | **mock/演示分支**内的假进度提示 | **保留**。已核实该行位于 `triggerManualSync` 的 `dataMode !== 'tauri'` 分支（第 1192 行有 `return`，其后即 mock 路径），是浏览器预览用的模拟提示，不在真实 Tauri 刷新链路上 |

即：上游复核 B-5 把 `store.ts:1195` 列为「混用实例」的判断**不成立**——
它属于演示分支，与后端同步状态指示器不在同一语义域。
真实链路（`dataMode === 'tauri'`）的措辞已统一为「已刷新…」系列（`:1180-1184`），无「刷新中…」。

### 2.3 示范模式口径（B-2）

| 文件:行 | 原文 | 改后 |
| --- | --- | --- |
| `src/components/SettingsModal.tsx:976` | `title="浏览器开发模式"` | `title="演示模式"` |
| `src/components/SettingsModal.tsx:977` | `<span className="about-arch-tag">Mock 模式</span>` | `<span className="about-arch-tag">不支持同步</span>` |

理由：同一屏的 toast 早已统一为「演示模式不支持…」前缀，此处却写「浏览器开发模式 / Mock 模式」，
属契约明确禁止的口径分叉。

### 2.4 控件收敛（B-6 / B-7）

| 文件:行 | 原文 | 改后 |
| --- | --- | --- |
| `src/styles/base.css:839` | `.toggle-action-btn.icon-btn { padding: 4px 7px; }` | 该规则并入 §2「图标按钮命中区收敛」的共享盒模型（`padding:0` + `inline-flex` 居中），并补 `width/height: var(--icon-btn-hit-sm)`；原散落规则删除（**第一次没收干净，见 §7.6**） |
| `src/components/Sidebar.tsx:297` | `className={\`sidebar-refresh-all ${isBusy ? 'sync-refresh-all-busy' : ''}\`}` | `className="sidebar-refresh-all btn-primary"`（`sync-refresh-all-busy` 全仓无任何样式定义，是死类名） |
| `src/styles/base.css:671-695` | `.sidebar-refresh-all` 自带 `background: var(--accent)`、`color:#fff`、`:disabled { opacity:.6 }` | 移除自带底色/前景色与独立禁用态，改由统一变体类 `.btn-primary`（含 `:disabled { opacity:.45 }`）提供；本类只保留整行布局与字号差异 |

影响：2 处纯图标「编辑」按钮（`SettingsModal.tsx` 的分类/订阅编辑）此前盒模型与同类图标按钮不一致；
侧栏最显眼的主按钮此前不吃统一禁用态（`.6` vs 其它主按钮的 `.45`）。
两处均只改样式归属，**未改任何可用性判断或点击行为**（`disabled={isBusy}` 原样保留）。

### 2.5 提示残留（B-8）

| 文件:行 | 原文 | 改后 |
| --- | --- | --- |
| `src/components/Timeline.tsx`（社交译文块） | `{translatingCard ? <span> ⏳ 翻译中…</span> : null}` | `{translatingCard ? <span>翻译中…</span> : null}` |
| `src/components/Timeline.tsx`（通知译文块） | 同上 | 同上 |

TASK-041 报告曾声称「两处同类提示统一为不带 ⏳、去掉多余前导空格」，实际并未改。
本轮清理后全仓 `⏳` 计数为 0。**过程留痕**：第一次用 `replace_all` 只改掉 2 处中的 1 处
（两处缩进不同，模式未匹配全），复核时才发现并补齐——说明此类清理必须全仓复查而非依赖单次替换。

### 2.6 台账陈述更正（E-3）

`TASK-041.json` 的 `test_review.baseline.summary` 曾写「前端回归**不使用任何用户可见中文串做断言**」。
实测为假：`tools/frontend-regression.mjs:119` 断言 `text.includes('全文提取失败')`、`:120` 断言
`action?.label === '重试'`。已在 `TASK-043.json` 的基线中写明该套件**确实断言中文串**，
并据此确认本轮改动的三个串（`刷新该订阅源`、`⏳ 翻译中…`、`浏览器开发模式`）**均不在**被断言集合内，
故未触发回归。TASK-041 台账原文保持不动（历史记录不篡改），更正以本任务记录为准。

## 3. 未改动的部分（避免越界）

- 未改 `tools/frontend-regression.mjs`（无 `src/` 之外的改动）。
- 未改任何点击行为、跳转目标、计数口径、状态流转；`data-*`、`onClick` 逻辑与 `stopPropagation` 全部原样。
- 未引入依赖、未改布局结构、配色或动画（REQ-005 属 TASK-042，已冻结）。
- 未处理「关闭态弹窗仍可被 Tab 聚焦」（复核 B-3）：那是**既有**结构问题、非 TASK-041 遗留，
  且涉及 `inert`/`aria-hidden` 的交互语义变更，超出本任务范围，留待独立任务。

## 4. 门禁结果（本会话实跑）

| 检查 | 命令 | 结果 |
| --- | --- | --- |
| lint | `npm run lint` | `Found 0 warnings and 0 errors.`（24 files / 116 rules） |
| build | `npm run build` | 通过（`tsc -b && vite build`，`✓ built in 134ms`） |
| 前端回归 | `npm run test:frontend` | `=== 前端逻辑回归 26/26 通过 ===`，**退出码 0** |

说明：终端里 `npm run ... | Select-Object` 的管道会掩盖真实退出码，
故门禁一律**重定向到文件后读 `$LASTEXITCODE`** 判定，不以屏幕文字为准。

## 5. 实机证据（2026-09-17 本轮实做）

设备：Windows，双屏（屏幕 A = `\\.\DISPLAY2` 主屏 X0–2560；屏幕 B = `\\.\DISPLAY1` X2560–5120，
均 2560×1440、96 DPI）。按用户指示**在屏幕 B 调试**，窗口固定在 X=2700 起，未占用用户正在使用的屏幕 A。

运行方式：`npm run dev`（vite :5173）+ `src-tauri/target/debug/app.exe`（dev 构建，devUrl 指向 5173），
即**当前候选代码的真实渲染**，不是旧构建产物。

### 5.0 证据文件清单（13 张现存；其中 13 号因无效已删除、14 号为修复后补拍）

| 文件 | 内容 |
| --- | --- |
| `TASK-043-01-main.png` | 主视图（侧栏 / 工具栏 / 空态 / 阅读区空态） |
| `TASK-043-02-command-palette.png` | 命令面板（真实 Ctrl+K 打开；输入框可见焦点环） |
| `TASK-043-03-settings.png` | 设置中心「通用」页 |
| `TASK-043-04-settings-general.png` | 设置中心「通用」页（另一时点） |
| `TASK-043-05-focus-search-entry.png` | **焦点环**：侧栏搜索入口（`[role=button]`）。**注**：该元素的 `role`/`tabIndex` 是**既有**的（HEAD `Sidebar.tsx:91-92`），本轮未改；此图仅作为「`[role=button]` 选择器在运行时确实命中并渲染焦点环」的例证 |
| `TASK-043-06-focus-view-button.png` | **焦点环**：视图项按钮（「收藏」） |
| `TASK-043-07-focus-primary-button.png` | **焦点环**：侧栏主按钮（本轮改用 `.btn-primary`） |
| `TASK-043-08-dropdown-keyboard-open.png` | **键盘打开下拉**：`role=listbox`、4 选项、1 个键盘高亮 |
| `TASK-043-09-dropdown-after-enter.png` | 下拉 Enter 提交后 |
| `TASK-043-10-settings-focus-ring.png` | **焦点环**：设置页输入框（Tab 到达） |
| `TASK-043-11-focus-icon-button.png` | **焦点环**：侧栏订阅源工具栏图标按钮（`.icon-sub-btn`） |
| `TASK-043-12-list-roving.png` | **列表卡片 roving**：方向键移动焦点至下一张卡，被聚焦卡片可见焦点环（内存 mock 数据） |
| ~~`TASK-043-13-gallery-roving.png`~~ | **已删除**：独立审查逐像素证实该图**并无焦点环**（当时外描边被 `.gallery-card` 的 `overflow:hidden` 裁掉，只剩底边一条线）→ 属无效证据，撞契约 §5「不得用截图冒充动态证据」 |
| `TASK-043-14-gallery-focus-inset.png` | **画廊焦点环（修复后）**：改用内描边后，逐边像素判定**四边均有 accent 描边**（本会话在 DPR 1.71、焦点图片 rect ≈ [279,90,222,295] 下测得四边各 300+ 命中；独立审查在其自身测量条件下另行测得 上600/下630/左823/右823——两者绝对计数因截图几何不同而不同，**结论一致：四边均可见**） |

其中 05/06/07/11 已用 3× 最近邻放大逐张目视复核，焦点环（蓝色 `outline` 描边）清晰可见。

### 5.1 取证手段（两次返工后的最终方案）

| 手段 | 结论 |
| --- | --- |
| `CopyFromScreen`（按窗口矩形截屏） | **不可用**：窗口被遮挡时只拍到上层窗口像素。首次取证 10 张图内容其实是**另一款软件的界面**（Rhino 图层面板），已全部删除 |
| `PrintWindow(hwnd, …, PW_RENDERFULLCONTENT)` | 可用：按窗口句柄取自身内容，不受遮挡影响 |
| **CDP（`--remote-debugging-port=9222`）** | **最终采用**：`Input.dispatchKeyEvent` 把按键直接送进渲染器，`Page.captureScreenshot` 取真实渲染帧，`Runtime.evaluate` 读 `document.activeElement` 与计算样式。**不经过操作系统输入层，因此不占用用户键盘/鼠标、不抢窗口焦点** |

CDP 连接确认：`/json/version` 返回 `Browser: Edg/153.0.4234.32`，
`/json/list` 返回 `type=page title='FluxReader' url=http://localhost:5173/`——证明被测对象是应用自身页面。

### 5.2 实机验证结果：TAB 焦点遍历**成立**（本轮关键结论）

通过 CDP 注入**真实 Tab 键**（`rawKeyDown`/`keyUp`，`windowsVirtualKeyCode=9`）遍历主视图，
每次读取 `document.activeElement` 与其计算样式：

```
tab 1 BUTTON            ti=0  fv=true outline=solid 1.75439px :: 最小化到托盘
tab 2 BODY              ti=-1 fv=false                        <- 非控件，正常
tab 3 BUTTON            ti=0  fv=true outline=solid 1.75439px :: —
tab 4 BUTTON            ti=0  fv=true outline=solid 1.75439px :: □
tab 5 BUTTON            ti=0  fv=true outline=solid 1.75439px :: ✕
tab 6 DIV[role=button]  ti=0  fv=true outline=solid 1.75439px :: 全局搜索…Ctrl K
tab 7 BUTTON            ti=0  fv=true outline=solid 1.75439px :: 全部0
tab 8 BUTTON            ti=0  fv=true outline=solid 1.75439px :: 今天0
tab 9 BUTTON            ti=0  fv=true outline=solid 1.75439px :: 未读0
tab10 BUTTON            ti=0  fv=true outline=solid 1.75439px :: 收藏0
tab11 BUTTON            ti=0  fv=true outline=solid 1.75439px :: 文章
tab12 BUTTON            ti=0  fv=true outline=solid 1.75439px :: 社交
tab13 BUTTON            ti=0  fv=true outline=solid 1.75439px :: 画廊
tab14 BUTTON            ti=0  fv=true outline=solid 1.75439px :: 播客
tab15 BUTTON            ti=0  fv=true outline=solid 1.75439px :: 通知
tab16 BUTTON            ti=0  fv=true outline=solid 1.75439px :: icon-sub-btn
```

判定（对照契约 §1）：

| 契约要求 | 实机结果 |
| --- | --- |
| 控件进入 Tab 序列 | **成立（但范围有限）**：16 个停靠点**互不相同**（去重 16/16），顺序为窗口控件 → 搜索入口 → 视图四项 → 内容布局五项 → 工具栏，与契约「侧栏 → 工具栏」一致。**更正**：原写「未跳入不可见区域」判定过强——报告 §5.7 自己测得「关闭态设置弹窗内仍有 20 个可 Tab 控件（`inert=false`）」，即**确实存在跳入不可见区域**（§5.7 与 `TASK-043-08` 截图即为该现象）。该问题是**既有结构缺陷**、非本轮引入，但它使本行的适用范围仅限于「主视图可见控件之间的顺序」 |
| 可见焦点环 | **成立**：除 `BODY` 外**每个停靠点 `:focus-visible=true` 且 `outline: solid 1.75px`**（≈2.92 物理像素 @DPR 1.71） |
| 非原生控件可被命中 | **成立**：`tab 6` 是 `DIV[role="button"] ti=0`。**更正**：该元素是侧栏搜索入口 `.sidebar-search-pill`，其 `role`/`tabIndex`/`onKeyDown` 在 HEAD（`Sidebar.tsx:91-92`）**早已存在**，本轮 diff 未触及它——它在此处只能作为「`[role=button]` 选择器在运行时确实命中」的例证，**不能**当作本轮修复的成果。本轮真正新增 role/tabIndex 的是列表/播客/画廊卡片、下拉触发器与选项、右键菜单项、全屏进度条 |
| 无 Tab 爆炸 | **成立**：`tabIndex=-1` 命中次数 = 0；列表卡片总数 0（库为空），故 roving 的组内约束未能在实机演示（见 5.4） |

截图（`Page.captureScreenshot` 直取渲染帧）：
`TASK-043-05-focus-search-entry.png`、`TASK-043-06-focus-view-button.png`、
`TASK-043-07-focus-primary-button.png`、`TASK-043-10-settings-focus-ring.png`；
其中搜索入口与视图项经 3× 放大复核，**蓝色焦点环清晰可见**。

### 5.3 FluxDropdown 键盘操作**成立**（本轮新增功能）

```
focus 到 .flux-dropdown-trigger -> role=combobox, tabIndex=0, aria-expanded=false
ArrowDown 之后  -> menuOpen=true, role=listbox, 选项 4 个, 键盘高亮 1 个, aria-expanded=true
再 ArrowDown    -> 高亮项随之下移
Enter           -> 菜单关闭, 取值由「未读」变为「全部」   （键盘改变取值: true）
```

即：**触发器可聚焦、可开合、方向键移动高亮、Enter 提交且真的改变取值**，与鼠标操作等价。
截图：`TASK-043-08-dropdown-keyboard-open.png`（键盘打开 + 高亮）、`TASK-043-09-dropdown-after-enter.png`。

### 5.4 列表卡片 roving tabindex：**已实机验证**（用内存 mock 数据）

本机用户库为空（`feeds/folders/articles/sync_queue` 实测均 0 行），主视图本来无卡片可测。
为把「roving 只有一个可 Tab」这条真正验证掉，采用**只改内存、不写库**的办法：
经 Vite dev 模块图 `import('/src/mockData.ts')` 取内置 mock 分类与条目，
连同按同规则重建的 `feedIndex` 一起 `useAppStore.setState({...})` 注入内存态。

**未触碰用户数据库**：注入后再次只读校验，`feeds/folders/articles/sync_queue` **仍为 0 行**。

实测结果（CDP 读取实时 DOM）：

```
注入: { ok:true, feedIndex:9, entries:11 }
卡片: { total:3, tabbable:1, roles:["button"] }        <- 3 张卡，恰有 1 张 tabIndex=0
方向键: ArrowDown -> data-card-index=0 -> 1 -> 2       <- 方向键在卡片间移动焦点
        ArrowUp   -> data-card-index=1
roving 不变式: 卡片 3 张，tabIndex=0 的有 1 张（应恰为 1）  <- 成立
Enter 激活: activeArticleId null -> art-2（改变=true）    <- 与鼠标点击等价
画廊布局: { total:3, tabbable:1, roles:["button"] }       <- 画廊同样恰为 1
```

即契约 §1.1「组内只有一个可 Tab 进入、组内用方向键移动」与 §1.2「Enter 触发与鼠标相同的动作」
**均在实机成立**；截图 `TASK-043-12-list-roving.png`（列表：被聚焦卡片带焦点环）为证。

**画廊布局的补充更正**：原引用的 `TASK-043-13-gallery-roving.png` 经独立审查逐像素证实
**不含焦点环**（当时外描边被 `.gallery-card` 的 `overflow:hidden` 裁掉），故该图已删除、
对应的「画廊焦点环可见」结论**当时不成立**。修复（改用内描边）后补拍
`TASK-043-14-gallery-focus-inset.png`，并经逐边像素判定确认**四边均有 accent 描边**
（本会话测得各边 300+ 命中；独立审查在其自身条件下测得 上600/下630/左823/右823——
绝对计数随截图几何而异，**四边均可见的结论一致**）——画廊焦点环至此才真正可见。
详见 §7.7。

### 5.5 实机**未**覆盖到的（不冒充通过）

1. **右键菜单**（`.ctx-menu`）：需要先有内容再右键，「上下文菜单」在空库下无目标元素可弹
   （实测 `[data-ctx]` 无匹配）；该项仅有代码依据（`role=menu` / `menuitem` / `tabIndex` / Enter 处理）。
2. **灯箱**与**播客卡片**：同样受限于数据（无附件/无播客源），仅有代码依据。
3. **虚拟列表滚动后**的 roving 正确性未验证：mock 数据仅 3 条，不足以触发虚拟化的滚动复用路径。
4. **浅色主题**下的焦点环未核对（本轮均为深色主题）。
5. `prefers-reduced-motion` 未受影响（本轮未改过渡），未重复验证。

### 5.6 一个必须写明的技术细节：`:focus-visible` 与「脚本聚焦」

核验中发现：用脚本 `el.focus()` 聚焦时，`:focus-visible` **不一定**为 true
（实测侧栏主按钮出现 `fv=false / outline:none`），这是浏览器对「是否为键盘意图」的启发式判定，
**不是本轮缺陷**——同一元素在真实 Tab 遍历下 `fv=true` 且 outline 正常（见 5.2）。
因此本轮证据以**真实 Tab 遍历**为准，不以脚本 `.focus()` 的结果判定可达性。

### 5.7 附：复核报告 B-3 的运行时确认

CDP 直接读取「已关闭的弹窗」状态：

```json
{"present":true,"controls":20,"tabbableWhenClosed":20,"inert":false,
 "ariaHidden":null,"opacity":"0","pointerEvents":"none","visible":"visible"}
```

即关闭态弹窗内仍有 **20 个可 Tab 控件**，未被 `inert`/`aria-hidden` 排除
（仅靠 `opacity:0` + `pointer-events:none` 隐藏）。
**这是既有结构问题，非本轮引入**，也不在本任务范围内，已单列建议另立任务处理。
本轮实机中也确实观察到该现象：设置弹窗关闭时其内部下拉仍能被键盘打开并在右下角渲染
（`TASK-043-08` 截图即为此现象），从侧面印证了 B-3 的真实性。

## 6. 未覆盖项（不冒充通过）

1. **右键菜单、灯箱、播客卡片**的焦点行为未实机验证（需内容/附件才能触发，见 5.5）；
   契约 §1 中这些条目的结论为**代码核对**。
2. roving tabindex 在**虚拟列表滚动/筛选变化后**的正确性未实机验证（mock 仅 3 条，未触发虚拟化复用路径）。
3. **浅色主题**下的焦点环外观未核对。
4. `prefers-reduced-motion` 未受影响（本轮未改过渡），未重复验证。
## 7. 实施过程中的自查与返工（留痕）

### 7.1 我自己的上游复核报告有 3 处误报，已更正

TASK-041 的独立复核报告 B-1 最初用正则匹配 JSX 开标签，而属性里的箭头函数
`onClick={() => …}` 含 `>`，标签在 `>` 处被截断，于是把**已合规**的元素误判为缺失。
改为括号/引号感知的解析后确认 3 处误报：`Sidebar` 的 `.feed-folder-title`、`.feed-leaf-item`
与 `PlayerBar` 的迷你 `.player-progress-track`（三者早已带 `role`+`tabIndex`+`onKeyDown`）。
真实缺口由 12 处修正为 9 处。报告内已留更正注记。
教训：**JSX 结构核对不能用朴素行/标签正则**。

### 7.2 `⏳` 清理第一次没做干净

第一次用 `replace_all` 只改掉 2 处中的 1 处（两处缩进不同，模式未全匹配），
复核时全仓 grep 才发现并补齐。说明此类清理必须全仓复查，不能依赖单次替换的"成功"回执。

### 7.3 verify 之后才发现并修掉一个边界缺陷（已重新验证）

`FluxDropdown` 的键盘逻辑在没有做空列表守卫时，`options.length === 0` 会让
`(i + 1) % 0` 得到 `NaN`。已补 `if (options.length === 0) return;`。
由于该修改发生在首次 `verify` **之后**，原候选 `a66b7081…` 已失效——
按流程**重新完整验证**，当前候选为 `35715bf1…`（三门禁重新实跑全 PASS）。
留此记录是为了说明「改完必须重验」，旧的验证结论不适用于新代码。

### 7.4 画廊布局的焦点移动已核实无副作用

画廊布局不启用虚拟化（`enabled: activeContentLayout !== 'image'`），
`moveCardFocus` 仍会调用 `rowVirtualizer.scrollToIndex`。
已核实 `@tanstack/virtual-core` 的 `scrollToIndex` 在拿不到 offsetInfo 时**直接 return**，
不会滚动或抛错；画廊是全量渲染，目标节点已在 DOM 中，`requestAnimationFrame` 后的
`focus()` 正常生效。故该调用对画廊是安全 no-op，不需要分支。

### 7.5 自查发现「可聚焦但不可操作」，已修并再次重验

在收尾自查中，把本轮新增的所有 `role` 与它们的键盘处理逐条对照（契约 §1.2 要求
「可聚焦的元素必须能由 Enter/Space 触发与鼠标相同的动作」），发现**一处真实缺口**：

`.player-progress-track`（**迷你播放条**进度条，`PlayerBar.tsx`）此前只有
`role="slider"` + `tabIndex={0}` + `aria-value*`，**却没有 `onKeyDown`**——
即键盘 Tab 到它之后按任何键都无反应。而契约 §1 第 7 行明确写着该元素要求「键盘可 seek」。

对照之下，全屏播放器的同类进度条（第 8 行）本轮已补方向键 seek，迷你条被漏掉了。

**修复**：给迷你条补 `aria-label="播放进度"` 与 `onKeyDown`——方向键 ±5 秒（与全屏条同源，
都走 `seekPlayer`）、`Home` 回起点；并在 `durationSec <= 0`（无音频时长）时直接返回，
避免对空播放器做无意义 seek。

**再次重验**：该修复发生在 `verify` **之后**，故原候选失效，按流程**重新完整验证**：
门禁三门重跑全绿，当前候选 `33986d3c…`（`src/components/PlayerBar.tsx` 的候选内哈希已核对为
本次修复后的内容）。

**教训**：新增 `role`/`tabIndex` 时，必须同时核对**该元素是否有对应的键盘动作**——
"能聚焦" 与 "能操作" 是两件事，只做前者会把契约第 1.2 条变成新的空壳。

### 7.6 旧 CSS 规则第一次没删干净（报告曾失准，已修并再次重验）

§2.4 那张表里我写过「原散落规则删除」，但收尾自查时发现
`src/styles/base.css` **仍保留**着旧规则：

```css
.toggle-action-btn.icon-btn {
  padding: 4px 7px;      /* 旧值 */
}
```

**为什么之前没暴露**：它位于文件前部（约 837 行），而新规则在文件末尾（约 3403 行），
两者特异性相同（0,2,0），**同特异性下后者胜出**，所以*渲染结果*一直是正确的
（实测 `padding` 生效为 `0`、`width/height` 取自 `--icon-btn-hit-sm`）——
门禁与截图都发现不了，只有**逐个选择器比对源码**才能发现。

**性质**：这是**维护性缺陷 + 报告失准**（说删了其实没删），属于我最该避免的那类问题，
因此按自查要求改正而不是放过：

- **代码**：删除该旧规则，原处留一行注释指向文件末尾的统一小节，说明为何不再在此定义；
- **报告**：§2.4 该行补注「第一次没收干净，见 §7.6」，如实标注曾失准。

**再次重验**：修改发生在 `verify` 之后 → 按流程重新完整验证，门禁三门重跑全绿，
当前候选 `504e7b91…`（`src/styles/base.css` 候选内哈希已核对为本次修复后内容）。

**教训**：
1. 「新规则覆盖旧规则」不等于「旧规则已清理」——CSS 层叠会掩盖残留；
2. 报告里的「已删除/已清理」这类断言，必须**用 grep 验证目标字符串确实为 0 次出现**，
   不能凭「我写了新代码」推定旧代码已消失；
3. 这与 §7.2 的 `⏳` 是同一类失误（两处只改到一处），说明**清理类改动必须全仓复查**。

### 7.7 独立审查判 FAIL：一处真缺陷 + 两处报告失准（已全部处置）

第一轮独立审查（新上下文，未参与实现）判 **FAIL**，5 条 finding。逐条复核结论：

| # | 审查结论 | 我的复核 | 处置 |
| --- | --- | --- | --- |
| 1 | **画廊 `<img>` 焦点环实际不可见**（`.gallery-card` 的 `overflow:hidden` + 图片铺满内容盒，外描边 2px+1px offset 需 3px 空间 → 上/左/右三边被裁，仅剩底边） | **成立，是真缺陷**。实测卡片四边余量仅 1px | **已修**：给 `.gallery-card img:focus-visible` 与 `.gallery-card .gallery-no-image:focus-visible` 改用**内描边**（`outline-offset: -2px`），环带落在裁剪框内。修复后逐边像素实测四边均有描边（上 367 / 下 371 / 左 493 / 右 491 命中） |
| 2 | **`TASK-043-13-gallery-roving.png` 里根本没有焦点环**（逐像素证实 accent 为 0） | **成立**。该图是 finding 1 的直接后果——环被裁掉，所以图里确实看不出来 | **已删除该文件**，改用修复后补拍的 `TASK-043-14-gallery-focus-inset.png` |
| 3 | §5.2 把侧栏搜索入口说成「本轮补 role 的」，实际 HEAD 早已具备、diff 未触及 | **成立**。`git cat-file -p HEAD:src/components/Sidebar.tsx` 第 91-92 行确有 `role`/`tabIndex`/`onKeyDown` | **已更正** §5.2 该行表述 |
| 4 | §5.2 判「未跳入不可见区域」与 §5.7 自测的「关闭态弹窗 20 个可 Tab 控件」矛盾 | **成立**，判定过强 | **已更正**：改为「范围有限」并点明确实存在跳入不可见区域的既有缺陷 |
| 5 | `刷新中…` 仍留在 `Sidebar.tsx:304` 与 `store.ts:1195`，需明确是否豁免 | 部分成立：`Sidebar.tsx:304` 是抓取按钮忙碌态（同属一套词）；`store.ts:1195` 经核实位于 **mock 分支**（1192 行 `return` 之后） | **已明确豁免并写明理由**（见 §2.2 表）；上游把 `store.ts:1195` 当混用实例的判断不成立 |

**审查者同时确认成立的部分**（我不再重复自证）：候选 16/16 哈希一致；三门禁其独立复跑全绿；
13 张 PNG 无逐字节重复；`⏳`=0；8 个文件；用户库 0 行未被写；契约 §1 十二项中 11 项成立；
§7 四处自曝（含 §7.6「说删了其实没删」）**全部属实**。

**教训**：
1. **「加得上焦点」≠「看得见焦点」**——外描边依赖父容器给出 ≥3px 余量；
   凡可聚焦元素位于 `overflow:hidden` 且无 padding 的容器内，必须改用内描边或给容器留白。
   本轮只在 `.gallery-card` 命中此坑（已全仓排查其余裁剪容器，无同类问题）。
2. 用截图当证据前，必须**逐像素确认该证据真的包含目标特征**——
   我此前只核了「文件互不相同」，却没有核「图里到底有没有环」，
   这正是 TASK-041 教训的更深一层。
3. 归因到「本轮改动」前，必须先 `git cat-file -p HEAD:<file>` 核对改前状态，
   不能凭印象把既有能力算作本轮成果。

### 7.8 第 2 轮独立审查：本轮**新引入**的 Space 双重动作（已修并实测）

第 2 轮独立审查（新上下文）同样判 **FAIL**，但性质变了——这次是**本轮改动引入的真实交互回归**：

**缺陷**：本轮为卡片/下拉/菜单等元素补 `role`+`tabIndex` 时，键盘处理只调了
`e.preventDefault()`，**没有 `stopPropagation()`**。而 `App.tsx` 的全局快捷键监听在 `window` 上
（`:245`），其 Space 分支（`:215`）位于 `if (inInput) return;`（`:212`）**之后**，
且**不检查 `defaultPrevented`**。后果：播放器激活时，对这些**新可聚焦**元素按一次 Space，
会**同时**执行控件自身动作并切换播放/暂停。

**为何是本轮引入**：这些元素此前**不可聚焦**，Space 根本落不到它们身上；
「全局 Space 响应」是既有行为，但「新聚焦元素叠加自身动作」是本轮才出现的组合。

**修复（外科式，只动一处）**：在 `App.tsx` 的 **Space 分支内**加 `if (e.defaultPrevented) return;`。

```js
if (e.key === ' ' && s.player.isActive) {
  if (e.defaultPrevented) return;   // 已被更具体的控件消费 → 让路
  e.preventDefault();
  s.togglePlayerPlay();
  return;
}
```

**为什么不在 onKey 顶部统一 `if (e.defaultPrevented) return;`**：
我最初就是这么写的，但审查同样的方式会让 `ConfirmDialog` 的 Esc
（`primitives.tsx:271` 也调 `preventDefault`）不再走全局 Esc 链，改变既有语义。
故收敛到 Space 分支——全局分支里只有 `' '`、`'s'`、`'m'`、`'j'`、`'k'` 可能与控件自身激活冲突，
而用 Space 激活控件是本轮新增的唯一新组合。

**实测验证**（CDP 直连真实 WebView2，注入内存 mock，包裹 `togglePlayerPlay` 计数）：

```
A) 焦点在 article-card 按 Space → activeArticleId null→art-1；togglePlayerPlay = 0   ✓（期望 0）
B) 对照：焦点在 body    按 Space → togglePlayerPlay = 1                              ✓（期望 ≥1，全局快捷键未失效）
C) 对照：Enter 不受影响：togglePlayerPlay = 0，卡片动作正常                            ✓
D) 下拉触发器按 Space → aria-expanded false→true；togglePlayerPlay = 0                ✓
=> 修复成立（双重动作消失，且 Space 播放/暂停的既有承诺保留）
```

**本轮同时改正的另两处（第 2 轮 findings 2/3）**：
- §5.0 证据表仍写「本轮新增 role/tabIndex」描述侧栏搜索入口，与 §5.2 的更正自相矛盾 → 已改为「既有，本轮未改，仅作选择器命中例证」；
- §5.0/§5.4 里我给出的四边像素计数**在审查者的测量条件下无法复现**（绝对计数随截图几何而异）
  → 已改为「本会话测得各边 300+ / 审查者测得 上600下630左823右823，结论一致：四边均可见」，
  不再宣称一组唯一数字。

**教训**：新增可聚焦元素时，除了「激活等价」，还必须核对**与既有全局键盘监听的冲突**——
全局监听若在后且不看 `defaultPrevented`，同一按键会被消费两次。
这类回归只有在「播放器激活 + 焦点落在新元素」的组合下才出现，静态看代码很容易漏。

