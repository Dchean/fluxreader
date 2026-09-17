import type { StateCreator } from 'zustand';
import { api } from '../../lib/api';
import { markEntriesRead, viewCacheKey, viewEntriesCache } from '../internals';
import { numericId, selectVisibleEntries } from '../selectors';
import type { AppState } from '../types';

/** 导航与筛选 slice：内容布局 / 视图 / 订阅范围 / 时间流筛选状态，及其导航 action。
 *
 *  Pick 的键集即本 slice 的全部键；与其它 slice 两两不相交（合起来 = 原 useAppStore 全集）。
 */
export type NavSlice = Pick<
  AppState,
  | 'activeContentLayout'
  | 'activeViewFilter'
  | 'activeFeedFilter'
  | 'timelineFilter'
  | 'timelineSort'
  | 'selectLayout'
  | 'selectView'
  | 'selectFeed'
  | 'toggleTimelineFilter'
  | 'toggleTimelineSort'
  | 'markCurrentViewAllRead'
>;

/** 本地零点毫秒（F8：今天视图的标读边界，与列表「今天」筛选同口径） */
function startOfLocalDayMs(): number {
  const d = new Date();
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

export const createNavSlice: StateCreator<AppState, [], [], NavSlice> = (set, get) => ({
  activeContentLayout: 'article',
  activeViewFilter: 'all',
  activeFeedFilter: 'all',
  timelineFilter: 'unread',
  timelineSort: 'newest',

  /* ================= 导航 ================= */

  selectLayout: (layout) => {
    set({
      activeContentLayout: layout,
      activeFeedFilter: 'all',
      /* 视图筛选（全部/今天/未读/收藏）独立于布局，切换布局时保留用户选择 */
      activeArticleId: null,
      isShowingTranslatedProse: false,
      isRawRenderMode: false,
      showFulltext: false,
      /* 切换布局 = 刷新列表，清除"已读保留"快照 */
      openedReadIds: {},
    });
    /* 不触发 reload：布局切换是纯本地过滤（selectVisibleEntries 按新布局
       resolve feed 布局），零延迟。列表快照本身不带正文（with_content 恒 false，
       正文由 useLazyHydrate 按视口批量水合，见 bootstrap.ts 的 layoutNeedsBody），
       因此新布局的卡片正文会在挂载时水合——不会出现「切换瞬间渲染旧布局正文」
       的错配。之前在此触发 reload 会带来异步等待 + 空列表闪动，是「切换卡顿」
       的根因。 */
  },

  selectView: (view) => {
    /* 视图缓存优先：命中则同步恢复该视图上次的 entries（零延迟、无渲染卡顿），
       再后台异步刷新保证数据最新。数量悬殊切换（收藏19 ↔ 全部2122）不再经历
       「清空 → 拉取 → 一次性渲染数百张卡片」的卡顿。 */
    const cached = viewEntriesCache.get(viewCacheKey(get().activeContentLayout, view));
    if (cached) {
      set({ activeViewFilter: view, openedReadIds: {}, entries: cached, articlesLimit: cached.length, articlesExhausted: view !== 'all' });
      /* 后台静默刷新（不阻塞切换）：状态/内容可能已变 */
      if (view !== 'all') void get().reloadFilteredEntries(view);
      else void get().reloadFromBackend();
      return;
    }
    set({ activeViewFilter: view, openedReadIds: {} });
    // 非「全部」视图：按后端筛选拉取完整列表替换 entries（收藏/未读的老文章
    // 不在「全部」的分页快照里）；「全部」视图：恢复分页快照。
    if (view !== 'all') void get().reloadFilteredEntries(view);
    else void get().reloadFromBackend();
  },

  selectFeed: (feedId) => set({ activeFeedFilter: feedId, openedReadIds: {} }),

  toggleTimelineFilter: () =>
    set((s) => ({
      timelineFilter: s.timelineFilter === 'all' ? 'unread' : 'all',
      openedReadIds: {},
    })),

  toggleTimelineSort: () =>
    set((s) => ({
      timelineSort: s.timelineSort === 'newest' ? 'oldest' : 'newest',
      openedReadIds: {},
    })),

  markCurrentViewAllRead: () => {
    const ids = new Set(selectVisibleEntries(get()).map((i) => i.id));
    if (get().dataMode === 'tauri') {
      /* 范围语义与后端一致：当前 feed/分类范围（all 时两者皆 null）。
         feed id 三种形态（'feed-123' / 纯数字 '123' / 旧 'f-123'）统一数字提取，
         避免定长前缀 slice 在纯数字 id 下截出 NaN（历史契约断裂 bug） */
      const scope = get().activeFeedFilter;
      const feedId = scope.startsWith('cat-') ? null : scope === 'all' ? null : numericId(scope);
      const folderId = scope.startsWith('cat-') ? numericId(scope) : null;
      /* F8：视图口径必须与界面一致——收藏/今天视图只标该视图可见的文章，
         否则会把范围内未显示的文章一并标读（并推给远端），与文案不符 */
      const view = get().activeViewFilter;
      const starredOnly = view === 'starred';
      const sinceMs = view === 'today' ? startOfLocalDayMs() : undefined;
      void api.markAllRead(feedId, folderId, { starredOnly, sinceMs });
    }
    markEntriesRead(ids);
    set({ openedReadIds: {} });
    get().showToast('已全部标为已读');
  },
});
