# OPT-017C：画廊布局有界渲染

Status: queued
父卡：OPT-017。前置：OPT-016B/C（真实组件回归与域回归已可用）。

## 主控读码确认的事实（2026-10-16）

- `src/components/Timeline.tsx`：`activeContentLayout === 'image'` 分支把
  `items.map` 全量渲染进 `.gallery-masonry-grid`，注释自述「不虚拟化，保持全量
  渲染（图片数量通常较少）」；其余 4 种布局走 `useVirtualizer`（同文件已引入
  `@tanstack/react-virtual`）。
- 样式为 CSS 多列瀑布流（`column-count: 4/3/2` + `break-inside: avoid`），
  不是行模型；直接套用现有行虚拟化不能保持列布局。
- 规模证据缺失：当前没有「画廊项数上限/实测 DOM 节点数」的数据；`docs/performance.md`
  的历史实机数字只覆盖文本布局，不能外推到图片布局。

## 交付

1. **有界渲染**：把画廊改为有可验证窗口上限的渲染（列模型 + 每列垂直虚拟化，
   或等效的行/窗口方案），给出「最大同时挂载卡片数」的常量与依据；不得以
   「图片通常较少」作为无上限理由。
2. **行为保持**：图片比例与瀑布流列观感、键盘焦点（`tabbableIndex`/`moveCardFocus`）、
   灯箱打开与关闭返回位置、滚动位置恢复（`timelineAnchor` 相关）、触底加载更多、
   空态/已到底文案与现有四布局互不影响。
3. **可测证据**：组件回归覆盖「窗口内挂载数不随 items 线性增长」；
   域回归覆盖滚动/焦点/灯箱/加载更多状态迁移；给出真实渲染节点数的测量方法
   （jsdom 不能证明布局性能，需在文档里明确未测边界与测量方式）。
4. **文档**：`docs/performance.md` 或归属 Note 记录窗口上限、选择依据与未测边界。

## 写集

- `src/components/Timeline.tsx`、`src/components/Gallery*`、`src/styles/base.css`
  （仅画廊相关）、`src/store/selectors.ts`/`internals.ts`（仅确需的锚点计算）
- `tools/component-tests/*`（画廊用例）、`tools/frontend-tests/*`（画廊域用例）
- `docs/performance.md`、归属 Note

不触及同步、AI、协议路径。

## 强制反例（先红后绿，证据落 tmp/optimization-20261008/OPT-017C/evidence/）

1. 5000 项画廊：同时挂载的 `GalleryCard` 数有上限且远小于总数（先写失败断言）。
2. 键盘 Tab/方向键焦点在窗口滚动后仍可达下一卡，不跳卡、不丢失当前焦点。
3. 灯箱打开 → 关闭：滚动位置与焦点回到原卡；跨窗口滚动后仍然成立。
4. 触底加载更多：追加批次后新项可见可交互；窗口边界不错位、不重复渲染。
5. 列布局回归：图片比例与现有列观感保持（组件快照/列分配断言）。

## 门禁

`npm run lint`、`npm run build`、`npm run test:frontend`、`npm run test:components`、
`cargo fmt --all -- --check`（Rust 未改动时可注明沿用）。真实 WebView 滚动/媒体
解码性能不在本卡边界内，作为验收缺口如实记录。
