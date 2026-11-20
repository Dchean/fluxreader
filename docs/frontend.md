# 前端结构台账

前端组件、样式类与令牌的唯一台账。改前端结构（增删组件、类名体系调整、令牌增删）时同批更新本页。前端分层方案与架构归属见 [架构](architecture.md) 的前端节；测试分层见 [开发与门禁](development.md)。

## 规模与分层

- 源码：`src/` 下 81 个 `.ts/.tsx`。样式只有三处：`src/styles/base.css`（主样式）、`src/styles/tokens.css`（令牌）、`src/index.css`（入口）。无 CSS Modules，类名为全局命名。
- 令牌层：35 个 `--token` 全部有 `var()` 消费，无闲置令牌；令牌只定义在 `tokens.css` 与 `base.css`。
- 控件层：雏形在 `src/components/primitives.tsx`：`Switch`、`SwitchInline`、`SwitchTrack`、`FluxDropdown`、`AiModeSelect`、`SettingCard`、`ConfirmDialog`、`ModalOverlay`。控件层非统一实现，改动时按模块处理。
- 复合块：无统一层。复合结构分散在页面组件内：`Timeline` 的五种卡片（`ArticleCard`、`SocialCard`、`GalleryCard`、`PodcastCard`、`NotifCard`）与独立行项 `TimelineRowItem`、`Overlays` 的七个弹窗（`SearchModal`、`Lightbox`、`NewCategoryModal`、`AddFeedModal`、`EditFeedModal`、`RenameCategoryModal`、`CloseAskDialog`），其中 Add/Edit Feed 的摘要与翻译模式字段复用局部无状态呈现块、各自表单持有状态与提交；`SyncTab` 的分区、`PlayerBar` 的全屏覆盖层。
- 阅读操作：`src/store/selectors.ts` 的 `selectReaderFulltextAction` 是 Reader 工具栏和文章右键菜单共享的全文入口判据；入口资格、恢复/提取忙态与展示标签由该 selector 统一计算。

## 类名账目

- 已定义 CSS 类 354 个（2026-11-19 基线 `30cfd65` 按同一口径复扫；2026-11-15 基线 `77c1964` 为 366 个，S3a 删除 12 个零引用类后为 354；扫描口径见文末）。
- 零引用类：无（原 12 个零引用类已于 2026-11-15 批 F-1 删除，见 docs/audits.md；此后新增死类按清理批次流程处理并同步本台账）。
- 动态拼接契约：同步指示器变体类由 `src/components/Sidebar.tsx` 以 `sync-status-row--{tone}` / `sync-dot--{tone}` 模板拼接；tone 取值以 `src/lib/syncPill.ts` 的 `SyncIndicatorTone` 为准（`local`/`pending`/`syncing`/`success`/`error` 五值）。四值各有变体类定义；`local` 值复用基类灰样式、无 `--local` 变体。改动 tone 集合必须同步：`syncPill.ts`、`Sidebar.tsx`、`base.css`、`tools/component-tests/sync-indicator.test.mjs`。
- 无样式 className 锚点：`error-center-dialog`、`hydrate-failure-text`、`load-more-btn` 是组件/契约测试的选择器或字符串锚（`tools/component-tests/`、`tools/frontend-tests/ui-contracts.mjs`）；`player-full-progress` 是 PlayerBar 的无样式包装层。这些类名不是样式载体，重命名前先查 `tools/` 引用。

## 组件清单

- `src/App.tsx`：`App`、`ToastPill`
- `src/components/Timeline.tsx`：`Timeline`；卡片 `ArticleCard`、`SocialCard`、`GalleryCard`、`PodcastCard`、`NotifCard`；行项 `TimelineRowItem`
- `src/components/Overlays.tsx`：`SearchModal`、`Lightbox`、`NewCategoryModal`、`AddFeedModal`、`EditFeedModal`、`RenameCategoryModal`、`CloseAskDialog` 及各 `*ModalBody`
- `src/components/PlayerBar.tsx`：`PlayerBar`、`PlayerFullOverlay`
- `src/components/primitives.tsx`：见"控件层"
- `src/components/Reader.tsx` / `src/components/ReaderProse.tsx`：`Reader`、`ReaderProse`
- `src/components/Sidebar.tsx`：`Sidebar`、`FeedFavicon`
- `src/components/ContextMenu.tsx`：`ContextMenuHost`
- `src/components/ErrorCenter.tsx`、`ErrorBoundary.tsx`、`HydrateFailure.tsx`、`CoverImage.tsx`、`icons.tsx`：`ErrorCenterDialog`、`ErrorBoundary`、`HydrateFailure`、`CoverImage`、`Icons`
- `src/components/SettingsModal.tsx` 与 `src/components/settings/`：13 个 Tab/Section 组件及共享常量（`shared.ts` 的 TAB_META、PALETTES、AI_PRESETS 等）
- Hooks：`useAutomaticAi`、`useEnteringClass`、`useFocusTrap`、`useImageResource`、`useProseImages`、`useTauriEvent`
- 状态层：`src/store/` 10 个 slice 与 `internals.ts`/`bootstrap.ts`，数据流见 [架构](architecture.md)
- 口径边界：`src/components/GalleryLayout.ts`（画廊几何与挂载上界纯函数）与 `src/components/shortcutYield.ts`（浮层快捷键让路纯函数）不列入本台账；两者没有组件或样式载体，按纯函数模块对待。

## 扫描口径

重新生成台账：扫描 `src/**/*.{ts,tsx}` 的 className 属性串与模板字面量静态片段，对照 `src/styles/*.css` 的类选择器与 `--token` 定义，统计使用次数与归属文件，并对模板静态片段做动态拼接排查。数值分三段基数：2026-11-15 全量扫描（基线 `77c1964`）= 366 类/80 文件；S3a（`55e9e5c`）删除 12 个零引用类后 354 类；2026-11-19 基线 `30cfd65` 按同一口径复扫 = 354 类/81 文件（`0d5a8a2` 新增 `src/lib/lru.ts` 后源文件增到 81）。脚本与原始输出在被忽略的 `tmp/s0-2026-11-15/`（`scan-classes.mjs`，不入库）。