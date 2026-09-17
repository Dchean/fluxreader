import type { StoreApi } from 'zustand';
import type {
  ArticleEntry,
  CategoryGroup,
  ContentLayoutType,
  FeedItem,
  ViewFilterType,
} from '../types';
import type { AppState } from './types';

/* ============================================================
   跨 slice 共享的模块级基础设施。

   拆分前这些内容都在 store.ts 的模块区：模块级可变状态（视图缓存）
   与「直接读写 useAppStore」的工具函数（标读/收藏收口、条目标记）。
   它们被多个 slice 共用，故收口到这里，store 句柄由 store.ts 在
   create() 之后注入（bindAppStore）——与原实现「模块求值完成后才会
   调用这些函数」的时序一致。
   ============================================================ */

/** store 句柄：由 store.ts 在 create() 后注入（模块求值期不调用任何 helper） */
let appStoreRef: StoreApi<AppState> | undefined;

export function bindAppStore(api: StoreApi<AppState>): void {
  appStoreRef = api;
}

/** store 句柄读取：未注入即显式抛错。
    此前是 `appStoreRef as StoreApi<AppState>` —— 类型上由断言强行伪装成已就绪，
    一旦某个 helper 在 bindAppStore 之前被调用（模块求值期、或未来新增的调用点），
    拿到的是 undefined，报错点会漂移到 `undefined.getState is not a function`
    这种与真实原因无关的位置。改成显式抛错，把失败点固定在根因处。 */
export function appStore(): StoreApi<AppState> {
  if (!appStoreRef) {
    throw new Error('appStore() 在 bindAppStore() 之前被调用：store 尚未创建（模块求值期不得读写 store）');
  }
  return appStoreRef;
}

/** 视图切换缓存：key = 「布局 × 视图」→ 该视图最近一次拉取的 entries 快照。
    用途：视图切换（尤其「收藏19 ↔ 全部2122」这类数量悬殊的切换）不再每次
    重新从后端拉取 + 一次性渲染数百张卡片（卡顿根因），而是先同步恢复缓存
    零延迟显示，再后台异步刷新。模块级（非 store 状态）避免触发重渲染。 */
export const viewEntriesCache = new Map<string, ArticleEntry[]>();

/** 视图缓存 key：布局 × 视图 × 订阅范围（scope）。
    TASK-052 起**必须带 scope**：条目列表现在是「该范围的首批 N 条」，缓存若不
    带范围，源A 的首批会被当成「全部」的首批恢复——列表内容与标题/角标错配。
    （范围进 key 后缓存条目仍受控：只有实际被 reload / 切视图的「布局×视图×范围」
    组合会建档，与之前「具体范围不缓存」的取舍等价。） */
export function viewCacheKey(layout: ContentLayoutType, view: ViewFilterType, scope = 'all'): string {
  return `${layout}|${view}|${scopePageKey(scope)}`;
}

/* ============================================================
   TASK-052：per-scope 分页游标（口径改造）

   缺陷 P1-14：「口径」半边 —— 列表口径（selectVisibleEntries：范围 × 视图 ×
   时间流筛选）与查询口径（list_articles 参数）此前不一致：分页请求只带
   limit/offset，不带 feed_id/folder_id/only_*，于是 articlesLimit 实际是
   **全局查询的 offset**，把「全局序列的第 N 条」当成了「当前范围的第 N 条」。
   后果：单源/单分类视图下滚，取回的不是该源的后续文章；且列表为空时哨兵
   不渲染（items.length > 0 才渲染），该源的老文章永远够不到。

   本模块把「查询口径」收口成一处：
   - scopeQueryArgs(scope, sort)：订阅范围 + 排序 → 后端参数（feed_id/folder_id
     /newest_first），与 anchorToArticle / markCurrentViewAllRead 同口径；
   - scopePageKey(scope, view)：分页游标键 —— 筛选口径可独立翻页，但共享
     entries，故游标键**只取订阅范围**（feed/分类），不含视图与排序。
   ============================================================ */

/** 从字符串 id（纯数字 / 'cat-' / 'feed-' 前缀）提取后端数字 id。
    （与 selectors.numericId 同形；此处模块内私有，避免 internals ↔ selectors 循环依赖） */
function scopeNumericId(raw: string): number {
  const m = raw.match(/(\d+)/);
  return m ? Number(m[1]) : NaN;
}

/** 订阅范围 + 排序 → list_articles / article_index 的查询参数。
    'cat-N' → folder_id=N；'all' → 两者皆 null；其余 → feed_id=数字。
    两种 id 形态（纯数字 '12' / 前缀 'feed-12'）统一走数字提取。 */
export function scopeQueryArgs(
  scope: string,
  sort: 'newest' | 'oldest',
): { feed_id: number | null; folder_id: number | null; newest_first: boolean } {
  const isCat = scope.startsWith('cat-');
  return {
    feed_id: scope === 'all' || isCat ? null : scopeNumericId(scope),
    folder_id: isCat ? scopeNumericId(scope) : null,
    newest_first: sort === 'newest',
  };
}

/** 分页游标键：per-scope。'all' 亦有意作为一等范围键（而非空串/缺省），
    否则不带 scope 的场景会被误并进 'all' 的游标。 */
export function scopePageKey(scope: string): string {
  return scope || 'all';
}

/** 由 categories 构建 feedId → { feed, cat } 解析表（每次 categories 变更后重建） */
export function buildFeedIndex(categories: CategoryGroup[]) {
  const map = new Map<string, { feed: FeedItem; cat: CategoryGroup }>();
  for (const cat of categories) {
    for (const f of cat.feeds) map.set(f.id, { feed: f, cat });
  }
  return map;
}

/** categories 变更后统一收口：重建解析表 + 级联清理已无归属的条目 */
export function reconcileCategories(
  s: Pick<AppState, 'categories' | 'entries'>,
  nextCategories: CategoryGroup[],
): Pick<AppState, 'categories' | 'feedIndex' | 'entries'> {
  const index = buildFeedIndex(nextCategories);
  const entries = s.entries.filter((e) => index.has(e.feedId));
  return { categories: nextCategories, feedIndex: index, entries };
}

/** 把当前 entries 同步进「当前布局 × 当前视图」的缓存。
    乐观更新（标读/收藏/水合）只改 store.entries，缓存若不联动，切走视图再
    切回会用旧快照覆盖新状态（正文丢失、标读回退）。 */
export function syncCurrentViewCache(entries: ArticleEntry[]) {
  const s = appStore().getState();
  viewEntriesCache.set(viewCacheKey(s.activeContentLayout, s.activeViewFilter, s.activeFeedFilter), entries);
}

/** 乐观更新某篇条目的 isRead/isStarred，并同步 feedCounts 的未读/收藏计数。
    侧边栏数字基于 feedCounts（后端精确计数），若不联动，标读/收藏后角标
    不立即变化（与乐观更新的列表脱节）。total/today 不受影响。 */
export function flipEntryFlag(id: string, field: 'isRead' | 'isStarred') {
  const s = appStore().getState();
  const entry = s.entries.find((e) => e.id === id);
  if (!entry) return;
  const nextVal = !entry[field];
  const entries = s.entries.map((e) => (e.id === id ? { ...e, [field]: nextVal } : e));
  const c = s.feedCounts.get(entry.feedId);
  let feedCounts = s.feedCounts;
  if (c) {
    const key = field === 'isRead' ? 'unread' : 'starred';
    const delta = field === 'isRead' ? (nextVal ? -1 : 1) : (nextVal ? 1 : -1);
    feedCounts = new Map(s.feedCounts);
    feedCounts.set(entry.feedId, { ...c, [key]: Math.max(0, c[key] + delta) });
  }
  appStore().setState({ entries, feedCounts });
  /* 同步当前视图缓存：避免切走再切回时，缓存恢复旧状态（标读/收藏回退闪烁） */
  syncCurrentViewCache(entries);
}

/** 批量标已读：同步 feedCounts 的未读计数（每篇 -1）。
    高效实现：一次遍历 entries 构建新数组，一次聚合 feedId 的未读减少数，
    避免循环内多次 Map 复制 / entries.map（「全部已读」几百篇时 O(n²) 卡顿）。 */
export function markEntriesRead(ids: Set<string>) {
  const s = appStore().getState();
  let entries = s.entries;
  const unreadDeltas = new Map<string, number>();
  let changed = false;
  // 一次遍历：标记 entries + 聚合每个 feed 的未读减少数
  entries = entries.map((e) => {
    if (ids.has(e.id) && !e.isRead) {
      changed = true;
      unreadDeltas.set(e.feedId, (unreadDeltas.get(e.feedId) ?? 0) + 1);
      return { ...e, isRead: true };
    }
    return e;
  });
  if (!changed) return;
  // 一次更新 feedCounts
  let feedCounts = s.feedCounts;
  for (const [feedId, delta] of unreadDeltas) {
    const c = feedCounts.get(feedId);
    if (c) {
      if (feedCounts === s.feedCounts) feedCounts = new Map(s.feedCounts); // 惰性复制
      feedCounts.set(feedId, { ...c, unread: Math.max(0, c.unread - delta) });
    }
  }
  appStore().setState({ entries, feedCounts });
  syncCurrentViewCache(entries);
}
