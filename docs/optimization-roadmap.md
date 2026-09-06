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

### A. store 解耦拆分（已执行，替代 React Query 迁移）

**原方案（React Query 迁移）评估后放弃**：FluxReader 的 `entries` 混合了服务端快照、
客户端水合、乐观更新、会话状态，迁到 React Query 需重设计数据流，风险高且单机
本地 SQLite 无缓存/去重诉求，收益有限。

**改为更安全的 store 拆分**（对外 API 不变，`from '../store'` 继续工作）：

- ✅ `src/store/selectors.ts`（191 行）：派生 selector + 常量（CONTENT_LAYOUTS 等）
- ✅ `src/store/types.ts`（274 行）：类型契约（AppState/PodcastPlayerState/SettingsState 等）
- ✅ `src/store.ts`（1920 → 1482 行）：核心 action 逻辑，re-export 上述两层

**停在此边界的原因**：剩余 action 深度耦合（`showToast` 被 50 处调用、`reloadFromBackend`
11 处），继续拆 slice 会引入循环依赖，风险显著而收益边际递减。selector/类型分离
已实现最有价值的边界划分。

### 可选（未纳入当前批次）

- `papr-core` 独立 crate + CLI（对 FluxReader 属过度工程）
- i18n（中文产品不需要）
- 标签/规则/高亮（功能扩展，非架构优化）

## 每步验证标准

- `cargo test` 全部通过
- `cargo clippy` 无警告
- `npm run build` + `npm run test:frontend` 通过
- 重启客户端手动验证
