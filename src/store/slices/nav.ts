import type { StateCreator } from 'zustand';
import { api } from '../../lib/api';
import { markEntriesRead, scopePageKey, scopeQueryArgs, viewCacheKey, viewEntriesCache } from '../internals';
import { selectVisibleEntries } from '../selectors';
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
  | 'applyArticlesCursor'
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

  /* TASK-052 契约（per-scope 游标）：游标与列表口径绑定，切换口径时必须让两者
     一致。写入器分两条路，组件不得绕开：
     - reloadFromBackend / reloadFilteredEntries / anchorToArticle：拉回数据后用
       **该范围自己的游标**原子写入（entries 与 articlesLimit 同一次 set）；
     - applyArticlesCursor（本 action）：只做镜像，用于「已有 entries 与目标游标不
       匹配、但无需重新拉取」的同步恢复（目前只有 selectView 缓存命中走这条）。
     之所以不暴露成通用 setter：单纯写 articlesLimit 而不动 entries（D3 缺陷当时
     可达的形态）会把「游标指向的位置」与「列表里的内容」拆开，下一次 loadMore 的
     offset 就会越过列表内容、整段文章静默丢失。 */
  applyArticlesCursor: (scopeKey, limit, exhausted) =>
    set((s) => ({
      articlesLimit: limit,
      articlesCursor: { ...s.articlesCursor, [scopeKey]: limit },
      articlesExhausted: exhausted,
      articlesLoading: false,
    })),

  selectView: (view) => {
    /* 视图缓存优先：命中则同步恢复该视图上次的 entries（零延迟、无渲染卡顿），
       再后台异步刷新保证数据最新。数量悬殊切换（收藏19 ↔ 全部2122）不再经历
       「清空 → 拉取 → 一次性渲染数百张卡片」的卡顿。
       TASK-052：缓存键带上订阅范围（源A 的首批≠全部的首批）；缓存里只有内容，
       游标仍需经 applyArticlesCursor 收口写入（不裸写 articlesLimit）。
       TASK-063：恢复时必须清水合状态——缓存快照不带正文，水合守卫
       （ensureArticleContent 的 hydratedIds 短路）会把上次会话的滞留标记误判为
       「已水合」，社交/通知卡片在后台刷新落地前空白且不会重水合。 */
    const scopeKey = scopePageKey(get().activeFeedFilter);
    const cached = viewEntriesCache.get(viewCacheKey(get().activeContentLayout, view, scopeKey));
    if (cached) {
      set({ activeViewFilter: view, openedReadIds: {}, entries: cached, articlesExhausted: view !== 'all', hydratedIds: {}, hydrationErrors: {} });
      get().applyArticlesCursor(scopeKey, cached.length, view !== 'all');
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

  /* TASK-052：切换订阅范围时按 per-scope 游标恢复分页游标；该范围从未加载过
     （游标表中无该键）则从 0 起步——即「B 源从第 1 页开始」。
     TASK-063（N2）：范围切换必须让 entries 与游标重新对齐。此前只写游标镜像，
     注释宣称「由调用方随后拉取」，但 Sidebar/Overlays 的全部调用点都未接线：
     旧范围快照残留（跨范围重复卡片 + duplicate key），空列表补拉走
     loadMoreArticles 的追加路径（offset=0 与旧快照交集重复），目标源不在旧
     快照且不可滚动时其第一页永远拉不到。
     与 selectView 同构：tauri 模式下缓存命中同步恢复该范围快照（零延迟）并
     后台刷新；未命中直接后台重拉——两个 reload 都在发起时读取刚写入的
     activeFeedFilter，自带代际/竞态守卫丢弃过期结果。恢复时清水合状态
     （理由同 selectView：缓存快照不带正文，滞留的已水合标记会造成
     「永不重水合」的正文空白）。mock 模式保持纯游标镜像（不触发 IPC、
     不把 mock 会话翻成 tauri）。 */
  selectFeed: (feedId) => {
    const scopeKey = scopePageKey(feedId);
    set((s) => ({
      activeFeedFilter: feedId,
      openedReadIds: {},
      articlesLimit: s.articlesCursor[scopeKey] ?? 0,
      articlesExhausted: false,
      articlesLoading: false,
    }));
    if (get().dataMode !== 'tauri') return;
    const view = get().activeViewFilter;
    const cached = viewEntriesCache.get(viewCacheKey(get().activeContentLayout, view, scopeKey));
    if (cached) {
      set({ entries: cached, articlesExhausted: view !== 'all', hydratedIds: {}, hydrationErrors: {} });
      get().applyArticlesCursor(scopeKey, cached.length, view !== 'all');
    }
    if (view !== 'all') void get().reloadFilteredEntries(view);
    else void get().reloadFromBackend();
  },

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
         与分页/锚定共用 scopeQueryArgs（TASK-052 口径收口），feed id 形态
         （'feed-123' / 纯数字 '123'）的数字提取只此一份。 */
      const scope = get().activeFeedFilter;
      const { feed_id: feedId, folder_id: folderId } = scopeQueryArgs(scope, get().timelineSort);
      /* F8：视图口径必须与界面一致——收藏/今天视图只标该视图可见的文章，
         否则会把范围内未显示的文章一并标读（并推给远端），与文案不符 */
      const view = get().activeViewFilter;
      const starredOnly = view === 'starred';
      const sinceMs = view === 'today' ? startOfLocalDayMs() : undefined;
      /* TASK-067 N10：全部已读失败必须可见——此前静默失败会让本地已全标读、
         计数已扣，重启后全部回退未读 */
      void api.markAllRead(feedId, folderId, { starredOnly, sinceMs }).catch(() => {
        get().showToast('全部已读未能保存，重启后可能回退');
      });
    }
    markEntriesRead(ids);
    set({ openedReadIds: {} });
    get().showToast('已全部标为已读');
  },
});
