# TASK-094 基线：画廊/播客/通知三布局列表不可达（REQ-107，2026-09-24 主控只读核对）

## 修前真机证据（审计 round-4 §2.2，s7b B1；脚本 tmp/audit-r3/harness/s7b_writes.mjs，输出 s7b_stdout.log / s7b_result.json）

| 布局 | 可见卡片 | 后端 | 容器可滚动 |
| --- | --- | --- | --- |
| 文章 | 14 | 1268 | 是 |
| 社交 | 12 | 1554 | 是 |
| 画廊 | 5 | 44 | 否（scrollHeight==clientHeight） |
| 播客 | 6 | 119 | 否 |
| 通知 | 1 | 20 | 否 |

空态或卡片下方仍渲染「滚动加载更多」（Timeline.tsx:304），而容器不可滚动，用户无法执行该提示。

注意：上表取证时后端还存在 P0-1（ArticleListArgs 字段被丢弃），该问题已在 061760e 修复。修复后后端会遵守 feed_id / folder_id / only_* / newest_first，但**仍然没有布局维度**，因此本卡须先在当前代码上重新复现。

## 修前代码事实（当前 HEAD 94681af）

- `list_articles` / `article_index` 的查询参数 ArticleListArgs（src-tauri/src/commands/articles.rs:29-62）没有 layout 字段；布局只在前端 selectScopeEntries / resolveFeedLayout 做本地过滤。
- 分页游标键 `scopePageKey(scope)`（src/store/internals.ts:96）与视图缓存键 `viewCacheKey(layout, view, scope)`（:51）不一致：游标不含布局，缓存含布局。
- 空列表补拉 effect（Timeline.tsx:197-200）只在 `items.length === 0` 时触发；列表非空但不足一屏时，onScroll（:180-190）不会触发，分页停在首批。
- 后端已有经独立审查的布局谓词：mark_all_read（db/articles.rs）与 list_unread_ids_scoped（db/sync_map.rs）中逐字相同的 layout 子查询（feed 级覆盖 → 分类兜底，与前端 resolveFeedLayout 等价）。

## 修前门禁

与 TASK-092 基线相同（cargo 213/0/9、frontend 348/348、lint/build/fmt/clippy exit 0）；本卡 prepare 时以依赖任务 TASK-093 的验证结果为准。
