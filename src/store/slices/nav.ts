import type { StateCreator } from 'zustand';
import { api } from '../../lib/api';
import { markEntriesRead, mergeSnapshotEntries, scopePageKey, scopeQueryArgs, viewCacheKey, viewEntriesCache } from '../internals';
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
    /* TASK-094（REQ-107）：布局是后端列表查询的维度（list_articles 带 layout），
       entries 快照只含当前布局的条目——切布局后旧快照对新布局是错的，纯本地过滤
       不再成立，必须按新布局重拉（与 selectFeed 同构：命中缓存先同步恢复该
       「布局×视图×范围」快照零延迟显示，再后台刷新；游标按 (布局×范围) 恢复，
       各布局首批从自己的第 1 页开始，切回不重复不跳页）。 */
    if (get().dataMode !== 'tauri') return;
    const scopeKey = scopePageKey(get().activeFeedFilter, layout);
    set((s) => ({
      articlesLimit: s.articlesCursor[scopeKey] ?? 0,
      articlesExhausted: false,
      articlesLoading: false,
    }));
    const view = get().activeViewFilter;
    const cached = viewEntriesCache.get(viewCacheKey(layout, view, get().activeFeedFilter));
    if (cached) {
      /* TASK-103：缓存恢复同属快照替换——正文与水合终态按 id 继承（收口在
         mergeSnapshotEntries；缓存快照本身携带 reload 时继承的正文），仅裁剪
         已不在恢复快照中的滞留标记。TASK-063 的「滞留标记阻断重水合」缺陷
         由该收口统一处置，不再在此整体清空。 */
      const merged = mergeSnapshotEntries(get().entries, cached, get().hydratedIds, get().hydrationErrors);
      set({ entries: merged.entries, articlesExhausted: view !== 'all', hydratedIds: merged.hydratedIds, hydrationErrors: merged.hydrationErrors });
      get().applyArticlesCursor(scopeKey, cached.length, view !== 'all');
    }
    /* TASK-098（与 F5 同口径）：void reload 调用点必须接住 promise——失败提示由
       reload 自身的 toast 给出，这里只吞掉残余重抛，避免 unhandled rejection。 */
    if (view !== 'all') void get().reloadFilteredEntries(view).catch(() => { /* 失败已可见（reloadFilteredEntries 内 toast） */ });
    else void get().reloadFromBackend().catch(() => { /* 失败已可见（reloadFromBackend 内 toast） */ });
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
       TASK-063：恢复时必须处置滞留水合状态——否则水合守卫（ensureArticleContent
       的 hydratedIds 短路）会把不属于本快照的滞留标记误判为「已水合」，社交/通知
       卡片在后台刷新落地前空白且不会重水合。TASK-103：处置方式从「整体清空」
       收口为 mergeSnapshotEntries 的按 id 继承+裁剪（正文与终态一起继承，
       已消失条目的滞留标记移除），缓存恢复不再丢已水合正文。 */
    const scopeKey = scopePageKey(get().activeFeedFilter, get().activeContentLayout);
    const cached = viewEntriesCache.get(viewCacheKey(get().activeContentLayout, view, get().activeFeedFilter));
    if (cached) {
      /* TASK-103：同 selectLayout——快照恢复按 id 继承正文与水合终态（合并收口
         在 mergeSnapshotEntries），仅裁剪已不在恢复快照中的滞留标记。 */
      const merged = mergeSnapshotEntries(get().entries, cached, get().hydratedIds, get().hydrationErrors);
      set({ activeViewFilter: view, openedReadIds: {}, entries: merged.entries, articlesExhausted: view !== 'all', hydratedIds: merged.hydratedIds, hydrationErrors: merged.hydrationErrors });
      get().applyArticlesCursor(scopeKey, cached.length, view !== 'all');
      /* 后台静默刷新（不阻塞切换）：状态/内容可能已变 */
      /* TASK-098（与 F5 同口径）：同 selectLayout——接住 reload 重抛，失败提示由 reload 自身给出 */
      if (view !== 'all') void get().reloadFilteredEntries(view).catch(() => { /* 失败已可见（reloadFilteredEntries 内 toast） */ });
      else void get().reloadFromBackend().catch(() => { /* 失败已可见（reloadFromBackend 内 toast） */ });
      return;
    }
    set({ activeViewFilter: view, openedReadIds: {} });
    // 非「全部」视图：按后端筛选拉取完整列表替换 entries（收藏/未读的老文章
    // 不在「全部」的分页快照里）；「全部」视图：恢复分页快照。
    /* TASK-098（与 F5 同口径）：同 selectLayout——接住 reload 重抛，失败提示由 reload 自身给出 */
    if (view !== 'all') void get().reloadFilteredEntries(view).catch(() => { /* 失败已可见（reloadFilteredEntries 内 toast） */ });
    else void get().reloadFromBackend().catch(() => { /* 失败已可见（reloadFromBackend 内 toast） */ });
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
     activeFeedFilter，自带代际/竞态守卫丢弃过期结果。恢复时按 id 继承+裁剪
     水合状态（TASK-103 收口到 mergeSnapshotEntries：缓存快照携带继承的正文，
     滞留标记只裁剪不属于本快照的部分——理由同 selectView）。mock 模式保持纯游标镜像（不触发 IPC、
     不把 mock 会话翻成 tauri）。 */
  selectFeed: (feedId) => {
    const scopeKey = scopePageKey(feedId, get().activeContentLayout);
    set((s) => ({
      activeFeedFilter: feedId,
      openedReadIds: {},
      articlesLimit: s.articlesCursor[scopeKey] ?? 0,
      articlesExhausted: false,
      articlesLoading: false,
    }));
    if (get().dataMode !== 'tauri') return;
    const view = get().activeViewFilter;
    const cached = viewEntriesCache.get(viewCacheKey(get().activeContentLayout, view, feedId));
    if (cached) {
      /* TASK-103：同 selectLayout——快照恢复按 id 继承正文与水合终态（合并收口
         在 mergeSnapshotEntries），仅裁剪已不在恢复快照中的滞留标记。 */
      const merged = mergeSnapshotEntries(get().entries, cached, get().hydratedIds, get().hydrationErrors);
      set({ entries: merged.entries, articlesExhausted: view !== 'all', hydratedIds: merged.hydratedIds, hydrationErrors: merged.hydrationErrors });
      get().applyArticlesCursor(scopeKey, cached.length, view !== 'all');
    }
    /* TASK-098（与 F5 同口径）：同 selectLayout——接住 reload 重抛，失败提示由 reload 自身给出 */
    if (view !== 'all') void get().reloadFilteredEntries(view).catch(() => { /* 失败已可见（reloadFilteredEntries 内 toast） */ });
    else void get().reloadFromBackend().catch(() => { /* 失败已可见（reloadFromBackend 内 toast） */ });
  },

  toggleTimelineFilter: () =>
    set((s) => ({
      timelineFilter: s.timelineFilter === 'all' ? 'unread' : 'all',
      openedReadIds: {},
    })),

  /* 排序方向决定 offset 的含义（scopeQueryArgs 的 newest_first）：只翻转排序键
     而不重拉，已加载的快照（旧排序的首批）会与新排序的下一页错位——继续翻页
     取回的是另一端的文章，整段不可达 + 重复卡片（审计「排序切换游标错位」）。
     故丢弃各视图快照缓存，并按新排序重拉：reload 落地时原子改写 entries 与
     per-scope 游标（游标含义已随排序翻转，必须与 entries 同一次写入）。
     不清空 entries：本地选择器先按新排序就位（零延迟、无空白闪烁），重拉完成后
     整体替换——与 selectView 缓存命中路径同构。 */
  toggleTimelineSort: () => {
    viewEntriesCache.clear();
    set((s) => ({
      timelineSort: s.timelineSort === 'newest' ? 'oldest' : 'newest',
      openedReadIds: {},
    }));
    /* F5（Batch 1/2 独立审查 P3）：
       - dataMode 守卫：mock 模式没有后端，重拉不仅多余，还会在落地时把 mock
         会话翻成 tauri（reloadFromBackend 成功路径写 dataMode:'tauri'）；
       - 筛选视图（收藏/未读/今天）拉的本就是全集（limit 100000，不分页），显示
         顺序由 selectVisibleEntries 按 timelineSort 本地排序——切排序只需本地
         重排，重拉是纯浪费，不调后端；
       - reloadFromBackend 失败时 toast 后会重抛，void 调用点必须接住，否则
         unhandled rejection。失败提示仍由 reloadFromBackend 自己给出，这里只吞掉
         重抛（与 (p3) 断言「reload 失败必须可见」不冲突）。 */
    if (get().dataMode !== 'tauri') return;
    const view = get().activeViewFilter;
    if (view !== 'all') return;
    void get().reloadFromBackend().catch(() => { /* 失败已可见（reloadFromBackend 内 toast） */ });
  },

  markCurrentViewAllRead: () => {
    const ids = new Set(selectVisibleEntries(get()).map((i) => i.id));
    if (get().dataMode === 'tauri') {
      /* 范围语义与后端一致：当前 feed/分类范围（all 时两者皆 null）。
         与分页/锚定共用 scopeQueryArgs（TASK-052 口径收口），feed id 形态
         （'feed-123' / 纯数字 '123'）的数字提取只此一份。 */
      const scope = get().activeFeedFilter;
      /* 布局口径由下方 api.markAllRead 的 layout 参数承载（写入口径不变）；
         scopeQueryArgs 在此只取 feed_id / folder_id。 */
      const { feed_id: feedId, folder_id: folderId } = scopeQueryArgs(scope, get().timelineSort, get().activeContentLayout);
      /* F8：视图口径必须与界面一致——收藏/今天视图只标该视图可见的文章，
         否则会把范围内未显示的文章一并标读（并推给远端），与文案不符 */
      const view = get().activeViewFilter;
      const starredOnly = view === 'starred';
      const sinceMs = view === 'today' ? startOfLocalDayMs() : undefined;
      const layout = get().activeContentLayout;
      /* TASK-067 N10：全部已读失败必须可见——此前静默失败会让本地已全标读、
         计数已扣，重启后全部回退未读 */
      void api.markAllRead(feedId, folderId, { starredOnly, sinceMs, layout }).catch(() => {
        get().showToast('全部已读未能保存，重启后可能回退');
      });
    }
    markEntriesRead(ids);
    set({ openedReadIds: {} });
    get().showToast('已全部标为已读');
  },
});
