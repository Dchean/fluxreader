# TASK-041 交互与截图证据（实机运行）

采集时间：2026-09-16 本地 12:42–12:50（UTC 04:42–04:50）。
环境：Windows 11，`npm run tauri dev` 启动**真实桌面应用**（Tauri 2 + WebView2，debug 构建），
computer-use 键盘交互 + 窗口位图截取（取 DWM 可见边界 `DWMWA_EXTENDED_FRAME_BOUNDS`；
屏幕 2560×1440、系统缩放约 150%，故应用窗口物理像素约 2082×1341）。

采集期间未新增或删除任何订阅/文章/分类，未提交任何对话框（对话框均以 Esc 关闭），
对正式数据库仅做只读检查，未写入。

## 采集到的证据文件

| 文件 | 内容 |
| --- | --- |
| `TASK-041-01-main.png` | 主视图：侧栏（品牌、全局搜索、视图四项计数、内容布局五项、订阅源工具栏三枚图标按钮、底部「本地模式 · 直连抓取」+ 刷新图标按钮 + 实心「刷新全部订阅源」+ 设置中心）、时间线工具栏（文章 / 未读 / 最新 / 全部已读）、列表空态、阅读区空态 |
| `TASK-041-02-settings.png` | 设置中心 → 通用：八个分区导航、分区标题与说明、统一开关（自动刷新 / 打开文章时标为已读 / 滚动到底部时标为已读 / 滚动出列表区域时标为已读）、滑杆 + 数值标签（刷新间隔 30 分钟、并发抓取数 4 路）、底部版本号 |
| `TASK-041-03-command-palette.png` | Ctrl+K 命令面板覆盖层：操作列表（刷新全部订阅源 / 将当前列表全部标为已读 / 切换 未读·全部 筛选 / 切换深色·浅色 模式 / 添加订阅源… / 新建分类… / 打开设置… / AI 服务设置…）与底部按键提示。**输入框可见 accent 焦点环** |
| `TASK-041-05-add-feed-dialog.png` | 添加订阅源对话框：归属分类与内容布局两个 FluxDropdown、两个文本框、说明文字、**自动摘要 / 自动翻译两枚统一开关**、次要按钮「取消」与实心主按钮「添加订阅」 |
| `TASK-041-06-focus-ring-switch.png` | 同上对话框，合成 Tab×3 之后（用于焦点核对，见下文诚实声明） |
| `TASK-041-04-focus-visible.png` | 主视图，合成 Tab×3 之后（同上） |

## 逐条核对（对照任务卡 ui_checks）

1. **主视图：时间线四种布局卡片、工具栏、侧栏** — 部分覆盖。侧栏（视图筛选、内容布局树、订阅源工具栏）、时间线工具栏、空态文案均已实机截图；**四种布局的卡片本体未覆盖**，原因是下节所述的应用数据目录为空（0 订阅 / 0 文章），无卡片可渲染。
2. **阅读器：正文区、AI 区块、文章操作按钮行** — **未实机覆盖**（无文章可打开）。诚实标注：本任务对阅读器只改了字符串与按钮样式，未做任何结构性改动。
3. **设置页：各分区标签与说明文字、连接与同步区、危险操作确认** — 部分覆盖。分区标签、说明文字、统一开关、滑杆已截图；连接与同步区、危险操作确认弹窗未进入（未配置同步、无分类可删）。
4. **覆盖层：搜索、命令面板、右键菜单、灯箱、对话框** — 部分覆盖。命令面板（Ctrl+K）与添加订阅源对话框已截图；右键菜单、灯箱、全局搜索未采集。
5. **空状态与错误提示（含 toast）以及演示模式/无服务提示** — 部分覆盖。主视图空态与阅读区空态已截图；toast 与演示模式提示未触发（未做会写数据的操作）。
6. **UI 证据**：主视图 / 设置页 / 覆盖层三类截图齐备（见上表）。

## 控件一致性：具体变更与可核证据

### 布尔控件统一为同一开关语义

原先全应用有三套布尔渲染：设置卡片用 `Switch` 组件，对话框行与分类管理行用原生 `<input type="checkbox">`。
现统一为一个组件 `SwitchInline`（不渲染自身 `<label>`，供外层 `<label>` 包裹，避免 label 嵌套导致的无效 HTML 与点击双触发），
共替换 10 处原生 checkbox：

- 对话框行（常规档 40×22）：`自动摘要`、`自动翻译`、`同步到后端`、`记住我的选择` —— 见 `TASK-041-05-add-feed-dialog.png`
- 分类管理行（紧凑档 32×18）：分类与订阅源各两处 `摘要` / `翻译`

尺寸分两档是**密度取舍**而非语义分叉：分类管理是定宽密集控制列，40px 开关会把该列从 44px 撑到 60px 以上并挤掉同行的下拉与操作按钮，
故该列用紧凑档，并把列宽由 `flex: 0 0 44px` 调整为 `flex: 0 0 60px`（32 开关 + 4 间距 + 两个汉字）。
两档共用同一 DOM 结构与同一滑块语言，只覆盖尺寸变量。

**受限**：紧凑档无实机截图——需要数据库中至少一个分类，而当前库为空（见「实机发现」）。构建产物中可见 `.switch-control.compact{width:var(--switch-w-sm);height:var(--switch-h-sm)}`。

### 图标按钮尺寸与内边距收敛

新增两档令牌（`--icon-btn-hit-sm: 20px`、`--icon-btn-hit-md: 22px`），
并把 `.icon-sub-btn`、`.feed-row-act-btn`、`.folder-chevron-btn` 统一为
`padding: 0` + `inline-flex` 居中 + `flex-shrink: 0` 的同一盒模型；
其中折叠按钮由 18×18 并入 sm 档（20×20）。

刻意排除（并在 CSS 中写明理由）：`.win-btn` 是窗口标题栏按钮，尺寸随 Windows 标题栏惯例（32×26）；
`.podcast-play-circle` 与 `.player-full-play` 是圆形播放动作而非图标按钮。

### 主按钮样式统一 + 禁用态

原先 `btn-primary` / `btn-danger` / `btn-danger-text` 只对 `.toggle-action-btn` 组合生效；
现改为**不带前提的变体类**，任何元素加上该变体类都得到同一套处理，并删除了原重复声明。
同时补上禁用态（此前实心主按钮禁用后与可用状态外观完全相同）：

```
.toggle-action-btn:disabled,.btn-primary:disabled,.btn-danger:disabled,.btn-danger-text:disabled{opacity:.45;cursor:not-allowed;filter:none}
```

见 `TASK-041-01-main.png` 的实心「刷新全部订阅源」与 `TASK-041-05` 的实心「添加订阅」。

### 焦点可见性

新增统一的键盘焦点环（复用下拉触发器打开态的 accent 语言）：

```
button:focus-visible,input:focus-visible,textarea:focus-visible,select:focus-visible,
a[href]:focus-visible,[role=button]:focus-visible,[tabindex]:focus-visible{outline:2px solid var(--accent);outline-offset:1px}
.switch-control input:focus-visible+.switch-slider{outline:2px solid var(--accent);outline-offset:2px}
```

选择器含伪类，特异性 (0,1,1) 高于文件中散落的 `.setting-input { outline: none }` 一类声明 (0,1,0)，
因此无需改动那些 `outline: none` 即可覆盖（`setting-input`、`search-modal-input`、`range-input`、`setting-prompt-textarea` 原先均无可见焦点指示）。
开关的 `input` 是 `opacity:0` 且 0×0，焦点环画在滑块上。

## 焦点与交互的核对方式（诚实声明）

静帧截图**不能**证明焦点可达性与动态效果。本轮实际做了三件事：

1. **实机目视到的**：`Ctrl+K`（键盘触发）打开命令面板，其输入框自动聚焦，
   `TASK-041-03-command-palette.png` 中该输入框带一圈 accent 边框与光晕 —— 即 `outline:2px` + `offset:1px`。
   这证明新规则确实生效且在深色主题下可见（键盘触发才匹配 `:focus-visible`）。
2. **实机尝试但未成功的**：在添加订阅源对话框中发送合成 Tab×3，未出现焦点环
   （`TASK-041-06` 与 `TASK-041-05` 画面一致）。判断是合成按键未驱动 WebView2 的 DOM 焦点遍历。
   因此**按钮、开关、文本域的焦点环本轮未获实机目视确认**，仅有 CSS 规则与选择器清单。
   → 建议由使用者做一次人工键盘遍历（Tab / Shift+Tab）复核。
3. **构建产物核对**：`dist/assets/index-C5bTmR8B.css` 中含 `focus-visible` 共 8 处，
   且上述规则原文可见（`bundle` 级验证，排除「源码改了但产物没打包」）。

## 实机发现（与本次改动无关，但需使用者知悉）

启动前应用数据目录已不存在：`%APPDATA%\com.fluxreader.app\` 的目录、`fluxreader.db`、`-wal`、`-shm`、
`%LOCALAPPDATA%\com.fluxreader.app\{EBWebView,logs}` 的创建时间**全部等于本次启动时刻**（12:42）。
库内 `user_version=13`，`feeds/folders/articles/sync_queue` 均为 0 行，主库文件 4096 字节。

即：本地订阅与文章数据当前为空。可能造成这一点的常见原因是数据目录曾被清理（磁盘清理工具、卸载重装等），
**本次任务未删除任何数据**，本轮仅只读检查数据库。

排除项：`git` 历史显示 `identifier` 自 0.9.3 起一直是 `com.fluxreader.app`（路径未变），
`src-tauri/src/lib.rs:124` 的 `app_data_dir()` 解析逻辑未变；
Rust 测试只使用 `Temp\dsh-*\fluxreader_*.db` 临时库，不会触碰正式库。

若这些订阅与文章仍存在于同步后端，重新填写同步配置后可回拉恢复（同步配置本身也在该库中，同样需要重填）。

## 未覆盖 / 受限项（不冒充通过）

- 时间线四种布局的卡片本体、阅读器正文区与 AI 区块、右键菜单、灯箱、toast —— 因库为空或未触发，未实机覆盖。
- 分类管理紧凑档开关 —— 同上（无分类）。
- 浅色主题外观未核对（本轮仅深色）。
- 动画与转场不在本任务范围（属 REQ-005 / TASK-042）。
- `src/styles/base.css` 的中文注释含 115 个 U+FFFD 替换字符（HEAD 为 118，本轮删除旧按钮块时随之移除了 3 个），
  属**既有**损坏（`HEAD` 已存在），仅影响注释文本、不影响渲染；本轮未修，留待后续清理。

## 文案与样式逐条映射（「文件:行 → 原文 → 改后」）

下表由 `git diff -U0` 自动生成，行号为**改后**行号：

### src/store.ts

| 行(改后) | 原文 | 改后 |
| --- | --- | --- |
| 329 | `get().showToast('已将当前视图范围内的内容标记为已读');` | `get().showToast('已全部标为已读');` |
| 511 | `if (!silent) get().showToast('浏览器演示模式无 AI 服务');` | `if (!silent) get().showToast('演示模式不支持 AI 服务');` |
| 586 | `showToast('该条目没有原文网页地址');` | `showToast('该条目没有原文链接');` |
| 632 | `get().showToast(art.isRead ? '已标记为未读' : '已标记为已读');` | `get().showToast(art.isRead ? '已标为未读' : '已标为已读');` |
| 663 | `if (!silent) get().showToast('浏览器演示模式无 AI 服务');` | `if (!silent) get().showToast('演示模式不支持 AI 服务');` |
| 766 | `if (!silent) get().showToast('浏览器演示模式无 AI 服务');` | `if (!silent) get().showToast('演示模式不支持 AI 服务');` |
| 861 | `get().showToast(`正在播放: ${title}`);` | `get().showToast(`正在播放：${title}`);` |
| 917 | `get().showToast('关闭操作未生效，请再点一次关闭按钮'),` | `get().showToast('关闭失败，请重试'),` |
| 1180 | `get().showToast(`刷新完成：新增 ${summary.new_articles} 条，${summary.failed_feeds} 个源直连失败`);` | `get().showToast(`已刷新，新增 ${summary.new_articles} 条，${summary.failed_feeds} 个源直连失败`);` |
| 1182 | `get().showToast(`刷新完成：新增 ${summary.new_articles} 条`);` | `get().showToast(`已刷新，新增 ${summary.new_articles} 条`);` |
| 1184 | `get().showToast('刷新完成');` | `get().showToast('已刷新，无新文章');` |
| 1195 | `get().showToast('正在后台增量同步...');` | `get().showToast('刷新中…');` |
| 1198 | `get().showToast('后端同步完成');` | `get().showToast('已刷新');` |
| 1232 | `get().showToast('浏览器已打开授权页，代码已常驻显示在本页');` | `get().showToast('已在浏览器打开授权页');` |
| 1412 | `get().showToast('没有需要保存的更改');` | `get().showToast('未做任何修改');` |
| 1446 | `get().showToast('浏览器演示模式无直连能力');` | `get().showToast('演示模式不支持直连');` |
| 1449 | `get().showToast('正在刷新该订阅源...');` | `get().showToast('正在刷新该订阅源…');` |
| 1454 | `get().showToast(n > 0 ? `刷新完成：新增 ${n} 条` : '刷新完成：没有新文章');` | `get().showToast(n > 0 ? `已刷新，新增 ${n} 条` : '已刷新，无新文章');` |
| 1468 | `get().showToast('已更新分类布局并即时生效');` | `get().showToast('布局已更新');` |
| 1471 | `.catch(() => get().showToast('布局保存失败（界面已生效，重启后可能回退）'));` | `.catch(() => get().showToast('布局未能保存，重启后可能回退'));` |
| 1489 | `get().showToast('已更新订阅源布局并即时生效');` | `get().showToast('布局已更新');` |
| 1492 | `.catch(() => get().showToast('布局保存失败（界面已生效，重启后可能回退）'));` | `.catch(() => get().showToast('布局未能保存，重启后可能回退'));` |

### src/components/Reader.tsx

| 行(改后) | 原文 | 改后 |
| --- | --- | --- |
| 140 | `从列表中点击卡片即可在右侧载入正文并激活 AI 辅助阅读。` | `在左侧选择一篇文章开始阅读。` |
| 160 | `if (!art.url) { showToast('该条目没有原文网页地址'); return; }` | `if (!art.url) { showToast('该条目没有原文链接'); return; }` |
| 163 | `title="在浏览器打开源网页"` | `title="在浏览器中查看原文"` |
| 166 | `<span>源网页</span>` | `<span>查看原文</span>` |

### src/components/Sidebar.tsx

| 行(改后) | 原文 | 改后 |
| --- | --- | --- |
| 71 | `? '正在同步...'` | `? '刷新中…'` |
| 73 | `? '后台同步中...'` | `? '刷新中…'` |
| 183 | `<span className="feed-count-badge" title="当前视图筛选下的条目数">{treeCounts.get(cat.id) ?? 0}</span>` | `<span className="feed-count-badge" title="当前筛选下的条数">{treeCounts.get(cat.id) ?? 0}</span>` |
| 241 | `title="最近一次刷新抓取失败（点击 ↻ 重试）"` | `title="最近抓取失败，点击重试"` |
| 270 | `<span className="feed-count-badge" title="当前视图筛选下的条目数">{treeCounts.get(f.id) ?? 0}</span>` | `<span className="feed-count-badge" title="当前筛选下的条数">{treeCounts.get(f.id) ?? 0}</span>` |
| 291 | `title="手动同步"` | `title="刷新全部订阅源"` |
| 302 | `<span>{isBusy ? '刷新中…' : '刷新所有订阅源'}</span>` | `<span>{isBusy ? '刷新中…' : '刷新全部订阅源'}</span>` |

### src/components/Timeline.tsx

| 行(改后) | 原文 | 改后 |
| --- | --- | --- |
| 214 | `<span className="load-more-end">— 已到底 —</span>` | `<span className="load-more-end">没有更多了</span>` |
| 216 | `<span className="load-more-idle">下拉加载更多</span>` | `<span className="load-more-idle">滚动加载更多</span>` |
| 420 | `if (!item.url) { showToast('该条目没有原文网页地址'); return; }` | `if (!item.url) { showToast('该条目没有原文链接'); return; }` |
| 492 | `<span>{item.isRead ? '标未读' : '标已读'}</span>` | `<span>{item.isRead ? '标为未读' : '标为已读'}</span>` |
| 611 | `<div className="notif-ai-text ai-generating-hint">⏳ 正在生成摘要...</div>` | `<div className="notif-ai-text ai-generating-hint"> 正在生成摘要…</div>` |

### src/components/SettingsModal.tsx

| 行(改后) | 原文 | 改后 |
| --- | --- | --- |
| 5 | `import { FluxDropdown, Switch, SettingCard, ModalOverlay, ConfirmDialog } from './primitives';` | `import { FluxDropdown, Switch, SwitchInline, SettingCard, ModalOverlay, ConfirmDialog } from './primitives';` |
| 184 | `<SettingCard title="并发抓取数" desc="同时请求的订阅源数量。源多可调高；个别源站限流时调低（1 为逐个抓取）">` | `<SettingCard title="并发抓取数" desc="同时抓取的源数量。源多可调高，被限流时调低。">` |
| 404 | `if (!r) { showToast('浏览器环境不支持导入'); return; }` | `if (!r) { showToast('演示模式不支持导入'); return; }` |
| 413 | `if (!xml) { showToast('浏览器环境不支持导出'); return; }` | `if (!xml) { showToast('演示模式不支持导出'); return; }` |
| 494 | `<input` | `` |
| 494 | `type="checkbox"` | `<SwitchInline` |
| 495 | `` | `compact` |
| 497 | `onChange={(e) => toggleCatSummary(cat.id, e.target.checked)}` | `` |
| 497 | `style={{ accentColor: 'var(--accent)' }}` | `onChange={(v) => toggleCatSummary(cat.id, v)}` |
| 506 | `<input` | `` |
| 506 | `type="checkbox"` | `<SwitchInline` |
| 507 | `` | `compact` |
| 509 | `onChange={(e) => toggleCatTranslate(cat.id, e.target.checked)}` | `` |
| 509 | `style={{ accentColor: 'var(--accent)' }}` | `onChange={(v) => toggleCatTranslate(cat.id, v)}` |
| 545 | `<div className="group-mgr-empty">该分类暂无订阅源，点击「添加源」创建</div>` | `<div className="group-mgr-empty">该分类还没有订阅源</div>` |
| 571 | `<input` | `` |
| 571 | `type="checkbox"` | `<SwitchInline` |
| 572 | `` | `compact` |
| 574 | `onChange={(e) => toggleFeedSummary(cat.id, f.id, e.target.checked)}` | `` |
| 574 | `style={{ accentColor: 'var(--accent)' }}` | `onChange={(v) => toggleFeedSummary(cat.id, f.id, v)}` |
| 583 | `<input` | `` |
| 583 | `type="checkbox"` | `<SwitchInline` |
| 584 | `` | `compact` |
| 586 | `onChange={(e) => toggleFeedTranslate(cat.id, f.id, e.target.checked)}` | `` |
| 586 | `style={{ accentColor: 'var(--accent)' }}` | `onChange={(v) => toggleFeedTranslate(cat.id, f.id, v)}` |
| 712 | `if (!list) { showToast('浏览器环境无法测试'); return; }` | `if (!list) { showToast('演示模式无法测试'); return; }` |
| 937 | `showToast('浏览器演示模式无同步能力');` | `showToast('演示模式不支持同步');` |
| 993 | `desc="Google Reader 与 Fever 共用 Miniflux「集成」凭据。切协议不丢数据（remote id 同源）。"` | `desc="两种协议共用 Miniflux「集成」凭据，切换不丢数据。"` |
| 1064 | `「测试连接」只验证连通性（秒级）；「保存并同步」保存后立即在后台拉取订阅与文章状态——` | `` |
| 1064 | `期间可关闭设置继续阅读。已读/收藏等状态变更会即时推送到服务端（约 1 秒内）。` | `` |
| 1064 | `断开连接会移除服务端拉取的订阅与文章（本地直连添加的保留）。` | `「测试连接」只验证连通性（秒级）；「保存并同步」会立即在后台拉取订阅与文章状态。` |
| 1065 | `` | `已读/收藏等变更约 1 秒内推送到服务端；断开连接会移除服务端拉取的订阅与文章。` |
| 1318 | `{ghLoggingIn ? '发起中...' : '登录 GitHub'}` | `{ghLoggingIn ? '发起中…' : '登录 GitHub'}` |
| 1387 | `{busy ? '处理中...' : '上传配置'}` | `{busy ? '处理中…' : '上传配置'}` |

### src/components/Overlays.tsx

| 行(改后) | 原文 | 改后 |
| --- | --- | --- |
| 5 | `import { ModalOverlay, FluxDropdown } from './primitives';` | `import { ModalOverlay, FluxDropdown, SwitchInline } from './primitives';` |
| 128 | `{ label: '同步并刷新所有订阅源', hint: '', run: () => s.triggerManualSync() },` | `{ label: '刷新全部订阅源', hint: '', run: () => s.triggerManualSync() },` |
| 280 | `{searchingEffective ? '搜索中…' : searchErrorEffective ? '搜索失败 — 请检查网络连接' : '没有结果'}` | `{searchingEffective ? '搜索中…' : searchErrorEffective ? '搜索失败，请检查网络' : '没有结果'}` |
| 455 | `不连接后端也可添加：客户端将直连源站抓取（第一优先级），` | `` |
| 455 | `连接后自动同步订阅关系并兜底直连失败的源。` | `未连接后端也能添加：客户端会直接抓取源站，连接后端后自动同步。` |
| 488 | `<input` | `` |
| 488 | `type="checkbox"` | `` |
| 488 | `checked={autoSummary}` | `` |
| 488 | `onChange={(e) => setAutoSummary(e.target.checked)}` | `` |
| 488 | `style={{ accentColor: 'var(--accent)' }}` | `` |
| 488 | `/>` | `<SwitchInline checked={autoSummary} onChange={(v) => setAutoSummary(v)} />` |
| 492 | `<input` | `` |
| 492 | `type="checkbox"` | `` |
| 492 | `checked={autoTranslate}` | `` |
| 492 | `onChange={(e) => setAutoTranslate(e.target.checked)}` | `` |
| 492 | `style={{ accentColor: 'var(--accent)' }}` | `` |
| 492 | `/>` | `<SwitchInline checked={autoTranslate} onChange={(v) => setAutoTranslate(v)} />` |
| 499 | `<input` | `` |
| 499 | `type="checkbox"` | `` |
| 499 | `checked={syncToBackend}` | `` |
| 499 | `onChange={(e) => setSyncToBackend(e.target.checked)}` | `` |
| 499 | `style={{ accentColor: 'var(--accent)' }}` | `` |
| 499 | `/>` | `<SwitchInline checked={syncToBackend} onChange={(v) => setSyncToBackend(v)} />` |
| 628 | `<input` | `` |
| 628 | `type="checkbox"` | `` |
| 628 | `checked={autoSummary}` | `` |
| 628 | `onChange={(e) => setAutoSummary(e.target.checked)}` | `` |
| 628 | `style={{ accentColor: 'var(--accent)' }}` | `` |
| 628 | `/>` | `<SwitchInline checked={autoSummary} onChange={(v) => setAutoSummary(v)} />` |
| 632 | `<input` | `` |
| 632 | `type="checkbox"` | `` |
| 632 | `checked={autoTranslate}` | `` |
| 632 | `onChange={(e) => setAutoTranslate(e.target.checked)}` | `` |
| 632 | `style={{ accentColor: 'var(--accent)' }}` | `` |
| 632 | `/>` | `<SwitchInline checked={autoTranslate} onChange={(v) => setAutoTranslate(v)} />` |
| 734 | `<input` | `` |
| 734 | `type="checkbox"` | `` |
| 734 | `checked={remember}` | `` |
| 734 | `onChange={(e) => setRemember(e.target.checked)}` | `` |
| 734 | `style={{ accentColor: 'var(--accent)' }}` | `` |
| 734 | `/>` | `` |
| 734 | `记住我的选择（之后可在 设置 → 通用 修改）` | `<SwitchInline checked={remember} onChange={(v) => setRemember(v)} />` |
| 735 | `` | `记住我的选择（可在 设置 → 通用 修改）` |

### src/components/ContextMenu.tsx

| 行(改后) | 原文 | 改后 |
| --- | --- | --- |
| 138 | `label: '打开源网页',` | `label: '查看原文',` |
| 198 | `label: st.showFulltext ? 'RSS 原文' : '全文',` | `label: st.showFulltext ? 'RSS 原文' : '显示全文',` |
| 232 | `label: '刷新全部订阅',` | `label: '刷新全部订阅源',` |


---

# 修复轮记录（独立审查 FAIL 之后）

## 独立审查结论

第一次独立审查（新上下文审查者，非编码者）判 **FAIL**，记录为运行 `RUN-77e20f6fe37a4091bc6435f00a3c7b78`（`failure_kind=review_failure`）。要点：

- 逐文件 sha256 与候选快照一致（10/10）——审查对象确为被验证产物。
- 门禁文本核对通过（lint 0/0、build 通过、前端回归 26/26）。
- `:focus-visible` 特异性结论成立：(0,1,1) 高于既有 `outline:none` 的 (0,1,0)，覆盖有效。
- **F1（阻断）**：用户可见文案仍残留 ASCII 省略号 `...`，违反契约「不得再出现 `...`」。共 7 处。
- **F2（次要）**：同类「生成中」提示两处不一致——`Timeline` 去掉了 ⏳ 但留了多余前导空格，`Reader` 保留 ⏳。
- **F3（次要）**：本报告未披露上述残留，且其中一处（`全局搜索...`）就渲染在本报告自己的截图里。

## 本轮修复内容

| 文件 | 原文 | 改后 |
| --- | --- | --- |
| `src/components/Sidebar.tsx:94` | `<span>全局搜索...</span>` | `<span>全局搜索…</span>` |
| `src/components/Reader.tsx:258` | ` 正在根据提示词生成摘要...</span>` | `正在根据提示词生成摘要…</span>` |
| `src/components/Timeline.tsx:611` | `ai-generating-hint"> 正在生成摘要…` | `ai-generating-hint">正在生成摘要…` |
| `src/components/SettingsModal.tsx:763` | `placeholder="sk-..."` | `placeholder="sk-…"` |
| `src/components/SettingsModal.tsx:1348` | `placeholder="ghp_..."` | `placeholder="ghp_…"` |
| `src/mockData.ts`（3 处 snippet） | 摘要文本结尾 `...',` | `…',` |

F2 的取舍：两处同类提示统一为**不带 ⏳**、统一用 `…`，并去掉多余前导空格。

修复后复核：`src/` 全目录用户可见 ASCII 省略号计数 = **0**（按「`...` 紧跟 `<` 或引号」判定，排除 JS 展开语法）。

## 修复轮的门禁

`npm run lint` 0 warnings / 0 errors（23 文件）；`npm run build`（tsc + vite）通过；`npm run test:frontend` **26/26 通过**。

## 关于截图的时间性（诚实声明）

`TASK-041-01-main.png` 是**修复前**采集的，其侧栏仍显示 `全局搜索...`（即 F1 的一处现场证据）；本报告保留它并在正文中如实说明，不用它冒充修复后的状态。

修复后「侧栏搜索入口显示为 `全局搜索…`」由以下方式确认：

1. 静态复核：`src/` 用户可见 ASCII 省略号 = 0，且 `Sidebar.tsx:94` 已为 `…`；
2. 运行时目视：修复后在真实桌面应用（`npm run tauri dev`，窗口置于右侧显示器）上观察侧栏，显示为 `全局搜索…`；命令面板输入框同屏可见 accent 焦点环；
3. 门禁：修复后 lint / build / 前端回归三门禁全绿（`npm run build` 的产物已重新生成）。

未重新入库修复后的位图截图：本环境的窗口位图抓取助手在修复轮多次失败（DPI 虚拟化导致 `GetSystemMetrics` 与 DWM 物理坐标不一致，抓取脚本报 `BAD_RECT`）。为避免用一张来源不明的图充当证据，这里选择如实记录上述三种替代核对方式，并把「需一张修复后主视图位图」列为遗留待办。
