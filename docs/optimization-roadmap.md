# FluxReader 优化路线图

> 综合历次代码审查与 [papr](https://github.com/l0ng-ai/papr)（同栈成熟 RSS 客户端）参考，整理的项目优化项与执行顺序。

## 已完成（历史）

- ✅ 虚拟滚动（`@tanstack/react-virtual`），4 种单列布局只渲染视口 + overscan
- ✅ 列表轻量（回退 `with_content` 全量背正文，正文按需批量水合）
- ✅ 视图切换缓存（`viewEntriesCache`）
- ✅ 按钮跳动修复（border 占位）、grid 过渡移除
- ✅ sync.rs N+1 批量预取（`SyncMatchMaps`，full/增量/upsert/reconcile 四路径）
- ✅ 移除 `key={filterKey}` 全量重挂载

## 待执行（按顺序，风险从低到高）

### B. 双向分页 + 搜索锚定（后端 `article_index`）

**目标**：从搜索/深层打开文章时，只加载目标那一页，而非从头拉 500 篇。

- 后端：新增 `article_index()`（`ROW_NUMBER() OVER (ORDER BY ...)` 计算绝对位置）
- 前端：`listArticles` 支持 `offset` 锚定；搜索打开后以目标页为起点

### C. 滚动锚定细节

**目标**：动态高度虚拟滚动的两个细节坑。

- prepend（向上加载）时补偿 scrollTop，避免视口跳变
- 打开文章 `scrollToIndex(align:auto)` + 持续 re-check 直到尺寸稳定

### A. React Query 数据层分离（治本，大改动）

**目标**：服务端数据（feeds/articles）从 zustand 迁到 `@tanstack/react-query`，
zustand 只保留纯 UI 状态，彻底解决 store.ts 臃肿（1872 行 → 目标 <600 行）。

- 引入 `@tanstack/react-query`
- feeds/articles/counts 迁到 React Query（缓存、失效、无限分页）
- zustand 保留：视图选择、外观偏好、播放器、弹层、Toast

### 可选（未纳入当前批次）

- `papr-core` 独立 crate + CLI（对 FluxReader 属过度工程）
- i18n（中文产品不需要）
- 标签/规则/高亮（功能扩展，非架构优化）

## 每步验证标准

- `cargo test` 全部通过
- `cargo clippy` 无警告
- `npm run build` + `npm run test:frontend` 通过
- 重启客户端手动验证
