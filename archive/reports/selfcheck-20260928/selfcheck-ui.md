# UI 一致性审查报告（发布前全项目自检 · UI 子代理）

日期：2026-09-29　方式：只读静态审查（读组件/样式码 + 既有截图证据基线，未运行门禁、未修改任何文件）
范围：`src/components/**`、`src/styles/base.css` + `tokens.css`、`index.html`、`src/store/slices/ui.ts`；对照 `.workflow-kit/docs/UI-CONTRACT-*.md`、`.workflow-kit/docs/PRODUCT.md`、`AUDIT-2026092x-*`、`.workflow-kit/tasks/evidence/TASK-092-ui-*` 与 `TASK-094-ui-*` 截图基线。

## 总体结论

**无 P0（发布阻塞）项。** 主体一致性良好：样式确实收敛在 `base.css` + `tokens.css`（`src/index.css` 仅两个 @import；组件内 45 处内联 style 均为布局尺寸类，仅画廊星标颜色走 `var(--star-color)`，无绕过主题的硬编码色）；五种布局共用同一哨兵判定（`timelineSentinel.ts`/`timelineRefill.ts`）、同一空态容器、同一 CoverImage 体系（画廊为契约明确的豁免项，见 P2-1）；toast 文案/位置/生命周期单点收口（`ui.ts` showToast + `.toast-layer`，has-player 避让符合 REQ-004 契约）；焦点可见性、reduced-motion、inert 关闭态（REQ-047/REQ-005/REQ-008 契约）均已落地。

发现合计 **28 项：P0=0，P1=3，P2=8，P3=17。**

---

## P1（应修：用户可感知的不一致或死控件）

### P1-1 播放条时长与卡片时长格式双轨，≥1 小时剧集显示为「61:40」式歧义时间
- 文件：`src/components/PlayerBar.tsx:31-36`（本地 `formatClock`，只输出 m:ss）；对照 `src/lib/format.ts:28-36`（`formatDuration`，h:mm:ss）
- 依据：播客卡 `Timeline.tsx:751` 用 `formatDuration` 显示 `1:01:40`；同一集在迷你播放条（PlayerBar.tsx:239/271）与全屏播放器（:369-370）用 `formatClock` 显示 `61:40`。`formatClock` 无小时分支，61 分钟以上的时长达不到 h:mm:ss 口径，同一内容两处时间写法不一致且「61:40」易被误读。
- 建议：删除 PlayerBar 本地 `formatClock`，统一改用 `lib/format.ts` 的 `formatDuration`（纯展示函数，无状态）。

### P1-2 全屏播放器覆盖层（z-index 260）压住所有弹窗（z-index 150），快捷键「看似失灵」
- 文件：`src/styles/base.css:3221`（`.player-full-overlay{z-index:260}`）vs `base.css:1990`（`.modal-overlay{z-index:150}`）；`src/App.tsx:204-216`（Esc 链）
- 依据：全屏播放器展开时按 `Ctrl+K` / `Ctrl+,` 会真正打开搜索/设置浮层，但被播放器遮罩盖住完全不可见——表现为快捷键无响应；且 Esc 关闭顺序（先搜索/设置、后播放器，App.tsx:206-213）与视觉层级相反，用户按 Esc 关不掉眼前的东西。同类冲突：灯箱（150）也被播放器盖住。
- 建议：全屏播放器期间抑制其它浮层入口（shortcutYield 已有浮层让路机制，把 `playerExpanded` 一并视作浮层），或把 `.player-full-overlay` 层级降到 150 以下/弹窗抬到 260 之上（需与 REQ-004 契约 A2 的 toast 层级约定一起核对）。

### P1-3 通知/社交卡开启 autoSummary/autoTranslate 后，AI 区块默认展开但永远空白（空壳 UI）
- 文件：`src/components/Timeline.tsx:787`+`836-851`（NotifCard 摘要框）、`:459`+`538-546`（SocialCard 译文块）；`src/components/Reader.tsx:98-108`
- 依据：`summaryOpen = summaryOverride ?? (feedConfig.autoSummary || …)`，开启自动摘要的源在通知布局下摘要框直接 open；但自动生成只在 Reader 打开文章时触发（Reader.tsx:103 `triggerReaderSummary({silent:true})`），通知卡挂载只水合正文（useLazyHydrate），从不触发 `summarizeEntry` → 卡片上长期渲染一个只有「摘要」标签、内容为 `{item.aiSummary}`（空串）的死框。SocialCard 译文块同理：`showTranslate` 跟随 `autoTranslate` 时渲染空 bordered 块，且无「翻译中…」标记。用户看到的是一个空盒子，只能靠手动点两次按钮才触发生成。
- 建议：卡片侧跟随 auto 配置展开时若 `!aiSummary && !generating && !error` 就地触发生成（与 Reader 同口径 silent），或在无内容时按未展开处理。

---

## P2（宜修）

### P2-1 画廊卡片图片无 onError 回退，可出现浏览器破图（五处 CoverImage 体系外的唯一例外）
- 文件：`src/components/Timeline.tsx:641-656`（`<img>` 无 onError；代理失败回退直连，直连再失败即破图）；对照 `CoverImage.tsx:51-62`
- 依据：REQ-106 契约（TASK-092）明确「画廊卡片既有行为逐字保留」，属契约豁免；但结果是画廊布局的失败封面显示破图图标，文章/播客/播放条/灯箱四处都是 `cover-fallback` 占位——同一概念（封面加载失败）两种视觉。
- 建议：给画廊 img 补 `onError → 封面失败态`（复用 `lib/coverImage.ts` 的 `onCoverError`），视觉语言即可五处统一；若坚持契约豁免，记为已知差异。

### P2-2 键盘导航口径：Social/Notif 卡不可聚焦，J/K 实际仅文章布局可用，与快捷键页文案不符
- 文件：`src/App.tsx:264-265`（`if (s.activeContentLayout !== 'article') return;`）；`src/components/Timeline.tsx:490/794`（SocialCard/NotifCard 无 role/tabIndex，对照 ArticleCard:396-399、PodcastCard:725-733、GalleryCard:646-668 的 roving tabindex）；`src/components/settings/ShortcutsTab.tsx`（「J / K 上下切换卡片 · 时间流」）
- 依据：快捷键页把 J/K 标为「时间流」范围，但社交/画廊/播客/通知四种布局按 J/K 直接 return；社交/通知卡本身不可 Tab 进入（内部原生按钮可 Tab，但破坏了卡片级 roving 语义的统一性）。
- 建议：或在 J/K 支持的布局里放开（用 moveCardFocus 同一机制），或把 ShortcutsTab 范围改准为「文章布局」；Social/Notif 卡补 `role="article"`/tabIndex 与 roving 对齐。

### P2-3 必填输入的空值处理三弹窗三种行为
- 文件：`src/components/Overlays.tsx:721`（RenameCategoryModal 空名 → 保存按钮 disabled）、`:384-386`（NewCategoryModal 空名 → 按钮可点但静默 return）、`:525-527`（AddFeedModal 空 URL → 静默 return）
- 依据：同一「主按钮 + 必填输入」模式，一处禁用、两处点了没反应（无 toast、无抖动），用户不知道为什么提交不了。
- 建议：统一为「空值禁用主按钮」（与 RenameCategoryModal 一致），或统一 toast 提示（`'分类名称不能为空'` toast 已存在却未在此用）。

### P2-4 AI 区块配色硬编码蓝紫，不随调色盘/主题变化
- 文件：`src/styles/base.css:1467-1468`（`.notif-ai-box` rgba(120,115,184,…)）、`:1754-1755`（`.ai-reader-box` rgba(72,128,200)/(120,115,184) 渐变+边框）
- 依据：全仓背景/文字/强调色已全量 token 化（tokens.css 五调色盘 × 双主题），唯独 AI 卡片底色/边框写死蓝紫；zinc/emerald/terracotta 调色盘下 AI 区块仍是另一套色相，与「配色只走变量」的既有口径不一致（同为硬编码的 `#e67e22`/rgba(229,72,77) 见 P2-5、P3-6）。
- 建议：改用 `color-mix(in srgb, var(--accent) N%, transparent)` 或新增 `--ai-block-bg/border` token。

### P2-5 错误/危险色三轨：rgba(229,72,77) vs `--danger:#d2694d` vs fallback #e5484d
- 文件：`src/styles/base.css:3079-3080`（`.reader-translate-error`）、`:3539`（`.ctx-menu-item.danger{color:var(--danger,#e5484d)}`）、`src/styles/tokens.css:24`（`--danger:#d2694d`）
- 依据：`--danger` 是赤陶色，但 Reader 翻译错误行底/边框写死正红 rgba(229,72,77)（`.ai-error-row` 内的 `.ai-error-text` 走 `var(--danger)`，同排并存的边框却是另一种红）；同一界面两种「危险红」。
- 建议：错误行背景/边框统一走 `--danger` 的 color-mix。

### P2-6 设置页滑杆数值标签两种写法：`.range-value-tag` vs 内联裸 span
- 文件：`src/components/settings/GeneralTab.tsx:28/42`（用 `range-value-tag`）vs `src/components/settings/ReadingTab.tsx:27/38/50`（`<span style={{width:45}}>` 无类）
- 依据：同一设置页里「刷新间隔/并发抓取数」的数值有 tag 样式（base.css:2275），“字号/行高/正文最大宽度”的数值是裸文本，同组件不同视觉。
- 建议：ReadingTab 三处改用 `range-value-tag`。

### P2-7 浮层打开无焦点移入/焦点陷阱/关闭后焦点归还
- 文件：`src/components/primitives.tsx:307-334`（ModalOverlay）、`src/components/Overlays.tsx:323-341`（Lightbox）
- 依据：REQ-047 只承诺「关闭态不可聚焦」（inert 已落地）；打开态既不把焦点移入弹窗，也没有焦点陷阱，Tab 可从弹窗游走到背后三栏内容；灯箱关闭后焦点回到 body 而非触发元素。与既有 REQ-008 焦点环投入不成比例（环做好了，但落点管理缺位）。
- 建议：open 时聚焦弹窗容器（tabIndex=-1）并在关闭时归还触发元素焦点；最小实现是 Lightbox 与四个 mini-dialog。

### P2-8 SettingsSidebarFooter 版本兜底硬编码 '0.8.0'，与 AboutTab 的「不显示假版本」决策相悖
- 文件：`src/components/settings/SettingsSidebarFooter.tsx:10`（`.catch(() => alive && setVersion('0.8.0'))`）；对照 `AboutTab.tsx:10-19` 注释「不再回退硬编码 '0.8.0'：假版本号会让『检查更新』拿错误的基准去比较」
- 依据：getVersion 失败时侧栏脚部显示 `FluxReader v0.8.0`——若实际是 0.9.x 即展示错误版本号；AboutTab 已改为显示 `…`。同一应用两处版本显示口径不同。
- 建议：footer 兜底改为保持 `…`。

---

## P3（备忘）

1. **Esc 关不掉 CloseAskDialog**：`App.tsx:204-216` Esc 链未含 `closeAskVisible`，ModalOverlay 自身不处理 Esc（ConfirmDialog 自带、其余浮层靠 App 链）——唯一 Esc 无效的浮层。`Overlays.tsx:736-765`、`App.tsx`。
2. **同概念两叫法**：右键菜单「新建订阅源」（`ContextMenu.tsx:240`）vs 侧栏/命令面板/弹窗标题「添加订阅源」（`Sidebar.tsx:140`、`Overlays.tsx:138/448`）、FeedsTab「添加源」。REQ-006 契约要求同一动作一套说法。
3. **生成中文案不统一**：Reader「正在根据提示词生成摘要…」（`Reader.tsx:268`）vs NotifCard「正在生成摘要…」（`Timeline.tsx:847`）；错误前缀「摘要生成失败：」vs「生成失败：」。
4. **中英混杂**：Reader 署名 `By {author}`（`Reader.tsx:198`），其余 UI 全中文。
5. **快捷键提示格式三种**：`Ctrl K`（Sidebar:97）、`Ctrl ,`（Sidebar:309）、`Ctrl+,`（SettingsModal:51）、`Ctrl + K`（ShortcutsTab）、`ESC 关闭`（Overlays:288）。
6. **散点硬编码色**：`#e67e22`（feed-error-dot，base.css:663）、rgba(229,72,77)（见 P2-5）未 token 化。
7. **收藏星标三套视觉**：SocialCard `Icons.star`+文字（Timeline:549-555）、GalleryCard ★/☆ 字符（:681）、ArticleCard 页脚文字「★ 已收藏」（:431）、Reader/ContextMenu `Icons.star/starFilled`。同一状态四种表达。
8. **卡片截断文本无 title**：card-title/card-snippet/gallery-title/podcast-title 均 line-clamp 截断且无 `title` 属性；画廊标题被截后无任何查看全文入口（点卡片开灯箱不开阅读器）。`Timeline.tsx:423/672/753-754`。
9. **tokens.css 头注释过时**：「浅色模式当前仅实现 blue 调色盘」，实际浅色 5 盘已全部实现（tokens.css:4 vs :140-248）。
10. **index.html theme-color #0a1936** 与任何主题 bg 不符（dark blue `--bg-base:#14161a`），启动闪色可能与实际背景不一致。`index.html:6`。
11. **favicon 失败只隐藏不留占位**：onError 直接 `display:none`，不回退到 dot 占位，行首留空槽（对照无 favicon 时有 dot fallback）。`Sidebar.tsx:232-234`。
12. **LAYOUT_NO_AI 重复定义**：`Overlays.tsx:12` 与 `settings/shared.ts` 各一份，改一处漏一处的风险。
13. **播放器字符图标与 SVG 体系混用**：'⏸'/'▶'/'✕'/'↺ 15'/'30 ↻'/'⛶'（PlayerBar.tsx:227-237/277/285）vs PodcastCard 用 `Icons.play`；AppearanceTab 主题按钮用 emoji ☀️🌙💻（:15-17）。
14. **侧栏双刷新入口**：小图标按钮 busy 时仅转圈不禁用、大按钮 busy 时 disabled——同一动作两种 busy 反馈（Sidebar.tsx:290-305；store 有入口守卫，不会重复触发，纯视觉不一致）。
15. **SocialCard 占位 opacity 0.45 内联两处**（Timeline.tsx:528/530），应并入 `.hydrate-placeholder` 类。
16. **stale 注释**：primitives.tsx:245 ConfirmDialog 注释「z-index 300」，实际 `.confirm-overlay` 为 3000（base.css:2591-2596）。
17. **对比度边缘项**：`--text-tertiary`（dark #7a8494 on #15171c ≈ 4:1）大量用于 10.5–11.5px 的 meta 文本，处于 WCAG AA 边缘；浅色模式同 token ≈4.6:1 可接受。读码判断，建议发布前真机扫一眼深色 zinc/terracotta 盘。

---

## 文案口径对照表

| 概念 | 出现位置与写法 | 结论 |
| --- | --- | --- |
| 标已读/未读 | 卡片/Reader/右键菜单统一「标为已读 / 标为未读」；toast「已标为已读 / 已标为未读」「已全部标为已读」；批量失败「标读失败」「批量标读失败」 | ✅ 一致 |
| 收藏 | 「收藏 / 取消收藏」全仓一致；状态展示有 ★ 已收藏 / ★☆字符 / 图标四种（P3-7） | 文案 ✅，视觉 ⚠️ |
| 查看原文 | Reader/SocialCard/右键菜单「查看原文」；无链接 toast「该条目没有原文链接」；失败「打开失败」 | ✅ 一致 |
| 刷新（抓取动作） | 「刷新全部订阅源 / 刷新此源 / 刷新中… / 已刷新 / 刷新失败：」 | ✅ 一致 |
| 同步（后端状态） | 「同步中… / 同步失败 / 后端已同步 / 本地模式 · 直连抓取」——侧栏注释明确与「刷新」分开 | ✅ 一致（刻意区分） |
| 添加订阅源 | 「添加订阅源」多数 vs 右键菜单「新建订阅源」vs FeedsTab「添加源」 | ⚠️ 见 P3-2 |
| 全文 | 「提取全文 / 显示全文 / RSS 原文 / 全文提取完成」Reader 与右键菜单一致 | ✅ 一致 |
| 空态 | 「暂无匹配内容 / 暂无收藏内容 / 今天暂无新内容 / 没有结果 / 该布局下暂无分类 / 未选择文章 / 暂无正文 / 没有更多了」 | ✅ 同一口径（「暂无…」系），哨兵空到底不与空态重复（timelineSentinel 契约） |
| 生成中 | 「正在生成摘要…」vs「正在根据提示词生成摘要…」；「翻译中…」 | ⚠️ 见 P3-3 |
| 错误提示 | 统一「失败：{原因}」+ toast 重试 action；Reader 另有内联错误行 | ✅ 结构一致 |
| 时间格式 | 相对时间统一走 `formatRelativeTime`（刚刚/N 分钟前/昨天/N 天前/YYYY-MM-DD），无绝对时间混入；时长 `formatDuration` vs `formatClock` | ⚠️ 见 P1-1 |
| 标点 | 全仓省略号统一 `…`，未发现 `...` 尾巴与英文感叹号（grep 验证） | ✅ 符合 REQ-006 契约 |

## 五布局能力矩阵

| 能力 | 文章 | 社交 | 画廊 | 播客 | 通知 |
| --- | --- | --- | --- | --- | --- |
| 卡片主行为 | 选中→阅读器 | 无（正文内图片→灯箱） | 灯箱+标已读 | 播放/暂停 | 无 |
| 收藏（卡内按钮） | —（页脚★标记） | ✓ | ✓（★/☆） | — | — |
| 标已读（卡内按钮） | — | ✓ | ✓ | — | ✓ |
| 翻译（卡内按钮） | —（Reader 内） | ✓ | ✗（LAYOUT_NO_AI 不适用但未提供） | —（布局禁 AI） | ✓ |
| 摘要（卡内按钮） | —（Reader 内） | — | — | —（布局禁 AI） | ✓ |
| 查看原文（卡内） | — | ✓ | — | — | — |
| 键盘聚焦（roving） | ✓ | ✗（P2-2） | ✓ | ✓ | ✗（P2-2） |
| 右键菜单（收藏/已读/原文/复制） | ✓ | ✓ | ✓ | ✓ | ✓ |
| 封面占位组件 | CoverImage | 正文内联 img | 自管 proxy+「无图」文本占位（P2-1） | CoverImage+empty | 不适用 |
| 时间显示 | formatRelativeTime | formatRelativeTime | 无 | formatDuration（卡）+formatClock（条）（P1-1） | formatRelativeTime |
| 分页哨兵 | 同源（timelineSentinel+refillDecision）：loading spinner /「没有更多了」/「滚动加载更多」（可滚动）/「加载更多」按钮（不可滚动） | 同左 | 同左 | 同左 | 同左 |

矩阵说明：卡内按钮差异多数是布局语义使然（播客=播放、画廊=图、通知=AI 卡），且右键菜单对五种布局补齐了收藏/已读/原文/复制链接，能力上限一致；真正的异常项是 P1-3（自动 AI 区块空壳）与 P2-2（键盘口径）。

## 附：核对过的契约与证据

- REQ-004/008（toast 位置+控件统一）：`.toast-layer` has-player 96px / has-player-expanded 回贴底、z-index 500 > 播放器 260 < 确认框 3000；设置页已无原生 select/checkbox（SyncTab 注释确认），FluxDropdown 全覆盖。✅
- REQ-005（动效）：list-entering/pane-entering/reader-entering 均已真实应用；display 离散过渡 + @starting-style + reduced-motion 兜底，播放条双向过渡。✅
- REQ-047（inert）：ModalOverlay/Lightbox 关闭态 inert 落地。✅（打开态焦点管理缺口见 P2-7）
- REQ-106（TASK-092 封面）：五处统一 CoverImage + cover-fallback + report_broken_cover 幂等；画廊豁免在案（P2-1）。证据 TASK-092-ui-covers-dark/light.png 等。
- REQ-107（TASK-094 哨兵）：sentinelMode/refillDecision 纯函数收口，「滚动加载更多」仅在可滚动时出现、不可滚动出按钮。证据 TASK-094-ui-sentinel-*.png。✅
