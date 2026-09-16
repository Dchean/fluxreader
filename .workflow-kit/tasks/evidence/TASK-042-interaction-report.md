# TASK-042 交互报告（REQ-005 交互动效补全）

> 本文件已在**审查反馈修复轮后**更新（第 1 轮修 4 处、第 2 轮修 3 处）：修正了「播放条/全屏播放器
> 只做了出现方向」「下拉菜单 transform 的 from 态被内联样式盖住」「AI/通知区块收起方向仍是硬切」
> 三处与实现不符的描述；补上第 2 轮独立审查指出的两个**漏做对象**（社交/通知译文块、设置页分类
> 分组）；修正新增 hook 的路径，以及「设置页不存在折叠语义」这处与代码不符的断言。下表每条都按
> 修复后的代码重新核对过。

## 0. 证据边界（先声明，避免把静帧当成动态证据）

- **代码路径核对**：第 3 节逐条给出「选择器 → 原状态 → 改后时长/曲线 → 触发场景」，可对着源码逐项核对。
- **实机操作观察**：`TASK-042-01-main-view.png` 与 `TASK-042-02-settings.png` 只证明**静态结果未被破坏**（布局、层级、配色与控件外观未变）。
  **静帧无法证明动态效果**——两张截图都不能证明过渡真的在播；本文档不把它们当作动态证据。
  两张截图摄于修复轮之前；本轮只改「可见性机制」，隐藏态仍然不渲染/不占位，布局与配色零改动，
  因此「静态结果未被破坏」这一结论继续成立。
- 实机环境：`npm run dev`（vite，5173）+ `src-tauri/target/debug/app.exe`（dev 构建，devUrl=http://localhost:5173）；设置页截图通过临时把 `settingsOpen` 初值置 true 触发，截图后已回滚（`git diff src/store.ts` 为空）。
- 本机数据为空（本地模式、0 订阅），因此主视图为空态；这不影响「静态结果未被破坏」这一结论，但确实使主视图截图的展示信息较少。
- 修复轮结论来源 = 三个门禁实跑 + 逐行源码核对；**未**做新的录屏或逐帧观察，因此不对「肉眼观感」作任何断言。

## 1. 时长与曲线词汇表（未新增）

- `--transition-fast` = `0.15s cubic-bezier(0.16, 1, 0.3, 1)`（`src/styles/tokens.css`）
- `0.2s cubic-bezier(0.16, 1, 0.3, 1)`（`src/styles/base.css` 既有用法：toast、分组区）
- 键帧：复用既有 `timelineFadeIn`；新增 `overlayFadeIn` / `cardScaleIn` 只定义**方向与幅度**，时长与曲线沿用上面两支。
  修复轮删除了已无引用的 `barSlideIn`（播放条与全屏播放器改用「常驻挂载 + transition」后，挂载键帧冗余）。

## 2. 逐条改动

| # | 状态（契约 ui_checks） | 选择器 | 原状态 | 改后 | 触发场景 |
| --- | --- | --- | --- | --- | --- |
| 1 | 列表与视图切换 | `.timeline-scroll-body.list-entering` | CSS 里声明了却**从未被应用**（死代码） | `timelineFadeIn var(--transition-fast)` | Timeline 挂 `useEnteringClass`，键=布局\|视图筛选\|订阅筛选\|未读筛选\|排序 |
| 2 | 下拉菜单 | `.flux-dropdown-menu` / `.flux-dropdown-menu.drop-up` | 挂载即带 `open`，from 态永不渲染；且 `updatePosition` 在上下两个分支都写内联 `transform:'none'`，内联优先级高于样式表，把声明的 from 态盖死——即便补了 rAF，`transform` 仍不会过渡，实际只有 `opacity` 生效 | 修复轮：`updatePosition` **不再写内联 `transform`**，from 态因此真的渲染；向下弹出用样式表 `translateY(-4px)`，向上弹出加 `.drop-up`（`translateY(4px)`），`.drop-up.open` 回 `none`；`drop-up` 与 `open` 都用 `classList` 维护（React 重写 className 会把 rAF 加的 `open` 抹掉）。现在 `opacity` 与 `transform` 两条过渡都真正渲染 | 打开下拉菜单（下/上两种弹出方向） |
| 3 | 右键菜单 | `.ctx-menu` | 完全没有入场过渡 | `timelineFadeIn var(--transition-fast)` | 右键菜单按需挂载时播一次 |
| 4 | 播放条 | `.podcast-bottom-bar` / `.podcast-bottom-bar.active` | `display` 硬切：`if (!player.isActive) return null`，元素被立即卸载，只有挂载键帧能播出现方向，**消失是硬切** | 修复轮：改为**常驻挂载**，只用 `.active` 类切换可见性；`transition: opacity / transform / display 0.2s cubic-bezier(0.16,1,0.3,1)`，`display` 进入过渡列表并 `allow-discrete`，`@starting-style` 提供起始值 → `none↔flex` 与位移/透明度**双向**过渡。`<audio>` 仍随 `isActive` 挂载/卸载，播放生命周期与原先一致 | 开始播放（出现）/ 点 ✕ 关闭（消失） |
| 5 | 全屏播放器 | `.player-full-overlay` / `.player-full-card` | `{playerExpanded && <...>}` 条件挂载，收起即卸载，同样只有出现方向有键帧 | 修复轮：遮罩**常驻挂载**，`.open` 驱动 `display:none↔flex`（`allow-discrete`）+ `opacity var(--transition-fast)`；卡片 `opacity`/`transform var(--transition-fast)`。隐藏态 `display:none` → 不可聚焦、读屏不可达；Esc（App.tsx 全局键）/ 点遮罩 / 「收起」按钮的关闭路径与 ARIA（`role="dialog"` + `aria-modal`）均未改 | 点「展开」出现 / 「收起」或 Esc 消失 |
| 6 | 弹窗卡片 | `.modal-overlay .modal-card` | 类名在 JSX 中**不存在**（死选择器） | `transition: opacity/transform var(--transition-fast)` | ModalOverlay 卡片容器补 `modal-card` 类；遮罩常驻（`display:flex` + `opacity:0`），卡片因此能拿到真正的 from 态 |
| 7 | 设置页分区切换 | `.settings-content-pane.pane-entering` | 无过渡 | `timelineFadeIn var(--transition-fast)` | 切换左侧 8 个页签 |
| 8 | 阅读器 | `.reader-active-view.reader-entering` | 无过渡 | `timelineFadeIn var(--transition-fast)` | 文章之间切换、原文↔译文切换 |
| 9 | 侧栏订阅树 | `.feed-sub-list` | 展开无过渡（子列表按需渲染） | `timelineFadeIn var(--transition-fast)` | 展开分类 |
| 10 | 灯箱图片 | `.lightbox-img` | 无入场 | `cardScaleIn 0.2s` | 点开正文图片 |
| 11 | 确认弹窗 | `.confirm-dialog` | 无入场（挂载即带 `open`，无法用过渡） | `cardScaleIn 0.2s` | 删除分类/订阅等确认框 |
| 12 | 加载更多 | `.load-more-spinner` | 只有旋转 | 追加 `overlayFadeIn var(--transition-fast)`，旋转动画不变 | 滚动到底部加载更多 |
| 13 | 通知/AI 区块 | `.notif-ai-box` / `.ai-reader-box` | `display:none → block` 硬切；且 `display` 被漏在 `transition-property` 之外——`allow-discrete` 只对**列在过渡列表里**的离散属性生效，所以只有出现方向淡入，**收起仍是瞬时硬切** | 修复轮：`transition: opacity / transform / display var(--transition-fast)` + `transition-behavior: allow-discrete` + `@starting-style` → `display:none↔block` 与透明度/位移**双向**过渡；隐藏时仍是 `display:none`（不占位、不可聚焦、读屏不可达） | 展开 / 收起摘要 |
| 14 | 折叠与展开（社交 / 通知**译文块**） | `.social-translated-block` / `.social-translated-block.show`、`.notif-translated-block` / `.notif-translated-block.show` | `display:none → block` 硬切，**没有任何过渡**。第 2 轮独立审查指出：契约 ui_check #5 明确写了「通知/译文/AI 区块」，而第 1 轮只改了 AI 摘要框与通知容器，这两个译文块既未实现也未在报告里披露 | 第 2 轮：`transition: opacity / transform / display var(--transition-fast)` + `transition-behavior: allow-discrete` + `@starting-style` → 出现与消失**双向**过渡；隐藏时仍是 `display:none`（不占位、不可聚焦、读屏不可达） | 卡片上点「翻译 / 显示译文」显示或隐藏译文 |
| 15 | 折叠与展开（**设置页分类分组**） | `.group-mgr-body` / `.group-mgr-body.open` | 原先是「收起即卸载」（`{!cat.settingsCollapsed && …}`），展开**没有任何过渡**，只有 `.group-mgr-chevron` 的既有旋转在动；本报告第 3 节还曾错误断言设置页「不存在折叠语义」 | 第 2 轮：改为**常驻挂载**，由 `.open` 类驱动 `display:none ↔ block`（`allow-discrete`）+ `opacity` / `transform var(--transition-fast)`，展开与收起**双向**过渡；`aria-expanded`、chevron 状态与隐藏态不可聚焦的语义均未改 | 设置 → 订阅页签内点分类标题展开 / 收起 |
| 16 | 无障碍 | `@media (prefers-reduced-motion: reduce)` | 无保护 | 全局过渡/动画压到 `0.01ms`（保留 `transitionend`/`animationend` 触发）；本任务新增的**挂载即播**入场动画显式 `animation: none`；播放条 / 全屏播放器 / 下拉菜单 / AI / 通知区块 / 译文块 / 设置页分类分组走的是 transition（含 `display` 的 `allow-discrete`），由全局 `transition-duration: 0.01ms` 一并压平，两个方向都无可感知动效；**加载指示器保留旋转**（进度反馈，停转会被误判为卡死） | 系统开启「减弱动态效果」 |

新增文件：`src/components/useEnteringClass.ts`（ref + `useLayoutEffect`，切换时先摘类名、强制重排、再挂上，使动画可重放；不经 state，避免整列表额外渲染）。修复轮把它从 `src/hooks/` 移到 `src/components/`，使其落入 TASK-042 的 `snapshot_paths`（含 `src/components`）覆盖范围；三个引用方（`Timeline.tsx` / `Reader.tsx` / `SettingsModal.tsx`）改为 `./useEnteringClass`。

第 2 轮另加了一处防御：`animationend` 会**冒泡**，因此摘类名的回调只在 `event.target === el` 时生效。否则 `.timeline-scroll-body` 的子孙 `.load-more-spinner`（本轮新追加了有限时长的 `overlayFadeIn`）动画结束时会把宿主的 `list-entering` 提前摘掉，使列表淡入被中途截断——这是本轮新增动画自己引入的耦合计时缺陷，由第 2 轮独立审查发现。

## 3. 未覆盖 / 不适用（诚实说明）

- 设置页**存在**带 `aria-expanded` 的分类折叠分组（`.group-mgr-title` 切换 `.group-mgr-body`），第 2 轮已补上双向过渡，见第 15 条。**更正**：本行此前写「设置页的『分组』…不存在折叠语义」，与代码不符（`SettingsModal.tsx` 的分类管理里确有展开/收起），该断言已删除；分区切换仍由第 7 条单独覆盖。
- 2col/3col 布局切换**不加** `grid-template-columns` 过渡（`base.css` 有既有性能取舍记录），改为让**内容**淡入，即第 1 条。
- 未引入动画库、未做视差或装饰性动画；未改 REQ-008 的静态控件外观（属 TASK-041）。
- 依赖 `transition-behavior: allow-discrete` 与 `@starting-style`（WebView2 / Chromium 现代内核）：不支持时的降级就是原来的瞬时切换，不影响正确性；本轮未在旧内核上实测降级行为。

## 4. 门禁结果（修复轮实跑）

- `npm run lint`：`Found 0 warnings and 0 errors.`（oxlint，24 files / 116 rules）
- `npm run build`：通过（`tsc -b && vite build`，`✓ built`，dist CSS 59.90 kB）
- `npm run test:frontend`：26/26 通过（`=== 前端逻辑回归 26/26 通过 ===`）

以上为**第 2 轮修复后**实跑结果；`verify` 会在账本里另存一份带时间戳的门禁日志。
