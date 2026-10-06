# UI 契约：TASK-114 五布局状态与快捷键统一

- 依据：AUDIT-20261005-core-consistency.md 第三阶段 + 探查事实（2026-10-06 dev）
- 范围：NotifCard 水合三态、五卡 Enter 绑定、J/K 布局门控

## X1.notif-hydration-states

NotifCard 与 SocialCard 的水合状态 UI 同构（SocialCard 现状为基准，Timeline.tsx:660-671）：

| 状态 | 条件 | 呈现 |
| --- | --- | --- |
| 加载中 | entryNeedsHydration 为真（无正文/未水合/无终态/无失败/无在途） | 与 SocialCard 同形的「加载正文…」占位 |
| 失败 | hydrationErrors[id] 存在 | 内联错误行 + 「重试」按钮（retryHydration(id)），文案域：正文加载失败：{原因} |
| 空正文 | hydrated 且无正文 | 「暂无正文」 |
| 失败不再静默回退 | — | 现状 snippet 回退仅在非失败态保留 |

## X2.enter-binding-uniform

| 卡片 | Enter/Space 行为 | role/tabIndex 基准 |
| --- | --- | --- |
| ArticleCard | 选中（onSelect） | 现状（button 语义容器） |
| PodcastCard | play（领域语义保持） | 现状 |
| GalleryCard | 开灯箱（领域语义保持） | 现状（img/占位为焦点载体） |
| SocialCard | **选中（新增）** | 对齐 ArticleCard 的可交互形态 |
| NotifCard | **选中（新增）** | 同上 |

方向键导航各卡保持现状；不新增其他键。

## X3.jk-all-virtualized

- J/K 选中切换（循环回绕）从「仅 article」扩展到 article/social/podcast/notification（全部虚拟化布局，复用 moveCardFocus/focusIndex 基建）。
- 画廊（image）非虚拟化：**不支持 J/K**，ShortcutsTab 明示「画廊布局不支持」。
- 输入框守卫（inInput）与浮层让路（shouldYieldToOverlay）规则不变，不新增让路键。
- App.tsx 的布局门控由 `!== 'article'` 改为「image 之外全部放行」。

## 验收取证

SSR 形态断言（renderToStaticMarkup 先例）+ 源级结构断言可作 UI 证据通道；真机截图非必需，但若实施中涉及样式新类名需在卡片内注明。

## 实施记录（TASK-114，2026-10-06）

两条实施裁定，收窄/澄清上表歧义处，行为均以上表其余条款为准：

- **X1「加载中」与「失败不再静默回退」的交叠**：通知卡的主正文本就是列表快照的
  snippet（社交卡正文则只有 content 一源）。裁定为——失败分支整体替换正文
  （snippet 不出现）；**非失败态保留 snippet 展示**（加载中/空终态有 snippet 时
  不换占位）；「加载正文…」「暂无正文」占位在**无可展示文本**时出现（与
  SocialCard 占位同形：.hydrate-retry / .hydrate-placeholder，通知卡语境同款
  min-height 防抖动，样式镜像为 .notif-card 前缀新规则）。该读法直接落在
  「现状 snippet 回退仅在非失败态保留」一行上。
- **X2「对齐 ArticleCard 的可交互形态」的 role 裁定**：Social/Notif 维持
  role="article"（不改 button）——卡内嵌套动作按钮/链接/重试控件，role=button
  会按 ARIA 规则把交互后代从可达性树掩蔽掉，且「订阅流中的文章」语义本就是
  article。对齐的是可交互行为：Enter/Space=选中（onSelect，与 ArticleCard 同
  动作）、tabIndex roving 不变、Enter/Space 仅在焦点落卡片本体时生效
  （e.target===e.currentTarget 守卫，嵌套控件键盘激活不被劫持）。

## 取证边界（实施实测）

renderToStaticMarkup 走 zustand 服务端快照（getInitialState，createStore 时捕
获，setState 不可达；实施时以 node 探针实测确认 SSR 不随 setState 变化），SSR
只能呈现与 store 初值一致的形态。X1 三态为运行时状态驱动分支，逐态取证走「源级
结构断言（t102-x1 先例）+ 纯函数/store 层行为断言」，SSR 仅作组件树可执行的烟测
（t114-x1d）；X3 的门控/推进判定抽为 src/lib/jkNavigation.ts 纯函数，回归网直
接断言真值表与五布局 store 模拟（t114-x3a..f）。


## R2 修订（2026-10-06）

- 展开按钮门控最终形态：`{isLong && (!hydrationError || !!fullText) && (`——组合态（错误滞留+fullText 到达）下钳制 snippet 可展开（与修前及同态 SocialCard 一致），纯失败态不渲染防死控件，常态不变（t114-x1g 变异自证）。
- reader.ts 详情成功路径清 `hydrationErrors[id]` 的状态卫生项（R1 选项 b）未纳入本卡，登记为独立卫生项候选（滞留错误在分支重排后不可见，收益仅为状态卫生）。
