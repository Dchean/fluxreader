import type { StateCreator } from 'zustand';
import { api, extractError } from '../../lib/api';
import { beginEntryFieldWrite, beginLocalFlagWrite, endEntryFieldWrite, endLocalFlagWrite, getEntryVersion, markEntriesRead, mergeSnapshotEntries, QueryScope, reconcileCountsOrdered, scheduleCountsReconcile, viewEntriesCache } from '../internals';
import { selectScopeEntries, selectVisibleEntries } from '../selectors';
import type { AppState } from '../types';
import type { ContentLayoutType, ViewFilterType } from '../../types';

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
  | 'switchRestoreNonce'
>;

/** 本地零点毫秒（F8：今天视图的标读边界，与列表「今天」筛选同口径） */
function startOfLocalDayMs(): number {
  const d = new Date();
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

interface MarkAllReadContext {
  scope: string;
  feedId: number | null;
  folderId: number | null;
  view: ViewFilterType;
  timelineFilter: 'all' | 'unread';
  layout: ContentLayoutType;
  starredOnly: boolean;
  sinceMs?: number;
  ids: string[];
  attemptReadById?: Record<string, boolean>;
  attemptFeedIdById?: Record<string, string>;
  operationVersionById?: Record<string, number>;
  key: string;
}

function markAllReadContextKey(
  context: Omit<MarkAllReadContext, 'ids' | 'attemptReadById' | 'attemptFeedIdById' | 'operationVersionById' | 'key'>,
): string {
  // Note: 范围类失败按“范围 × 视图 × 布局 × 时间界”分条 — 见 .agents/notes/implemented/feature/2026-10-16-重试入口持久可达与错误中心.md
  return [
    'mark:all-read',
    context.scope,
    context.feedId ?? 'none',
    context.folderId ?? 'none',
    context.view,
    context.timelineFilter,
    context.layout,
    context.sinceMs ?? 'none',
    context.starredOnly ? 'starred' : 'all',
  ].join(':');
}

export const createNavSlice: StateCreator<AppState, [], [], NavSlice> = (set, get) => {
  const runMarkAllRead = (context: MarkAllReadContext, replay: boolean): void => {
    const targetExists = context.feedId !== null
      ? get().feedIndex.has(String(context.feedId))
      : context.folderId !== null
        ? get().categories.some((c) => c.id === `cat-${context.folderId}`)
        : true;
    if (!targetExists) {
      if (replay) get().showToast('全部已读重试目标已失效，未执行');
      return;
    }
    const st = get();
    if (st.dataMode !== 'tauri') return;
    const matchesCurrentContext = (state: AppState): boolean =>
      state.activeFeedFilter === context.scope
      && state.activeViewFilter === context.view
      && state.timelineFilter === context.timelineFilter
      && state.activeContentLayout === context.layout;
    const contextIsCurrent = matchesCurrentContext(st);
    const ids = new Set(context.ids);
    const targetCacheKey = QueryScope.viewKey(context.layout, context.view, context.scope);
    const attemptSourceEntries = contextIsCurrent
      ? st.entries
      : (viewEntriesCache.get(targetCacheKey)?.entries ?? []);
    const attemptReadById = Object.fromEntries(
      context.ids.flatMap((id) => {
        const entry = attemptSourceEntries.find((candidate) => candidate.id === id);
        return entry ? [[id, entry.isRead] as const] : [];
      }),
    );
    const attemptFeedIdById = Object.fromEntries(
      context.ids.flatMap((id) => {
        const entry = attemptSourceEntries.find((candidate) => candidate.id === id);
        return entry ? [[id, entry.feedId] as const] : [];
      }),
    );
    const prevReadById = contextIsCurrent
      ? new Map(Object.entries(attemptReadById))
      : new Map<string, boolean>();
    const prevOpenedReadIds = st.openedReadIds;
    let clearedOpenedReadIds: Record<string, boolean> | null = null;
    const operationIds = new Set(context.ids);
    if (contextIsCurrent) {
      for (const entry of selectScopeEntries({
        ...st,
        activeContentLayout: context.layout,
        activeFeedFilter: context.scope,
      })) {
        operationIds.add(entry.id);
      }
    }
    if (contextIsCurrent) {
      markEntriesRead(ids);
      clearedOpenedReadIds = {};
      set({ openedReadIds: clearedOpenedReadIds });
    }
    const operationVersionById = Object.fromEntries(
      [...operationIds].map((id) => [id, getEntryVersion(id, 'isRead')] as const),
    );
    const actionContext: MarkAllReadContext = {
      ...context,
      attemptReadById,
      attemptFeedIdById,
      operationVersionById,
    };
    const optimisticVersionById = new Map<string, number>();
    for (const [id, prev] of prevReadById) {
      if (!prev) optimisticVersionById.set(id, getEntryVersion(id, 'isRead'));
    }
    const hasOptimisticWrite = optimisticVersionById.size > 0;
    if (hasOptimisticWrite) {
      beginLocalFlagWrite();
      for (const id of optimisticVersionById.keys()) beginEntryFieldWrite(id, 'isRead');
    }

    const convergeLoadedScopeToRead = () => {
      const current = get();
      const inScope = selectScopeEntries({
        ...current,
        activeContentLayout: context.layout,
        activeFeedFilter: context.scope,
      });
      const since = context.view === 'today' ? context.sinceMs : undefined;
      const targetIds = new Set(
        inScope
          .filter((e) =>
            getEntryVersion(e.id, 'isRead') === actionContext.operationVersionById?.[e.id]
            && (context.view === 'starred' ? e.isStarred
              : context.view === 'today' ? e.publishedAt >= (since ?? 0)
              : true),
          )
          .map((e) => e.id),
      );
      if (targetIds.size > 0) markEntriesRead(targetIds);
    };
    const updateCapturedViewCache = (mode: 'read' | 'restore') => {
      const cacheKey = QueryScope.viewKey(actionContext.layout, actionContext.view, actionContext.scope);
      const cached = viewEntriesCache.get(cacheKey);
      if (!cached) return;
      viewEntriesCache.set(cacheKey, {
        ...cached,
        entries: cached.entries.map((entry) => {
          if (!ids.has(entry.id)) return entry;
          const operationVersion = actionContext.operationVersionById?.[entry.id];
          if (typeof operationVersion !== 'number'
            || getEntryVersion(entry.id, 'isRead') !== operationVersion) {
            return entry;
          }
          const isRead = mode === 'read' ? true : actionContext.attemptReadById?.[entry.id];
          if (typeof isRead !== 'boolean' || entry.isRead === isRead) return entry;
          return { ...entry, isRead };
        }),
      });
    };

    void api.markAllRead(context.feedId, context.folderId, {
      starredOnly: context.starredOnly,
      sinceMs: context.sinceMs,
      layout: context.layout,
    }).then(() => {
      if (hasOptimisticWrite) {
        endLocalFlagWrite();
        for (const id of optimisticVersionById.keys()) endEntryFieldWrite(id, 'isRead');
      }
      get().showToast('已全部标为已读');
      /* affected=0 也是成功终态（目标已全读）：仍按捕获上下文收敛缓存并重新对账，
         否则「首次已提交但响应丢失 → 切 B → 重试返回 0」会留下旧缓存未读。 */
      if (matchesCurrentContext(get())) convergeLoadedScopeToRead();
      updateCapturedViewCache('read');
      const reconcileCounts = (): Promise<'applied' | 'stale' | 'empty'> =>
        reconcileCountsOrdered().then((outcome) => {
          if (outcome === 'failed') throw new Error('计数对账读取失败');
          if (outcome === 'skipped') return 'empty';
          return outcome;
        });
      const runCountReconcile = (allowRetry: boolean) => {
        reconcileCounts().then((outcome) => {
          if (outcome === 'stale' && allowRetry) {
            void reconcileCounts().catch(() => { /* 重取失败：放弃，计数由下次 reload 自愈 */ });
          }
        }).catch(() => {
          if (!allowRetry) return;
          get().showToast('全部已读已保存，未读计数刷新失败');
          setTimeout(() => {
            void reconcileCounts().catch(() => { /* 重试仍失败：放弃，计数由下次 reload 自愈 */ });
          }, 3000);
        });
      };
      runCountReconcile(true);
    }).catch((e: unknown) => {
      if (hasOptimisticWrite) {
        endLocalFlagWrite();
        for (const id of optimisticVersionById.keys()) endEntryFieldWrite(id, 'isRead');
      }
      const current = get();
      let changed = false;
      let takenOver = false;
      const unreadRestore = new Map<string, number>();
      for (const [id, version] of optimisticVersionById) {
        if (getEntryVersion(id, 'isRead') !== version) {
          takenOver = true;
          continue;
        }
        const feedId = actionContext.attemptFeedIdById?.[id];
        if (feedId) unreadRestore.set(feedId, (unreadRestore.get(feedId) ?? 0) + 1);
      }
      const entries = current.entries.map((a) => {
        if (!ids.has(a.id)) return a;
        const prev = prevReadById.get(a.id);
        if (prev !== false || a.isRead !== true) return a;
        if (getEntryVersion(a.id, 'isRead') !== optimisticVersionById.get(a.id)) return a;
        changed = true;
        return { ...a, isRead: prev };
      });
      let feedCounts = current.feedCounts;
      if (unreadRestore.size > 0) {
        feedCounts = new Map(current.feedCounts);
        for (const [fid, delta] of unreadRestore) {
          const count = feedCounts.get(fid);
          if (count) feedCounts.set(fid, { ...count, unread: Math.max(0, count.unread + delta) });
        }
      }
      const next: Partial<AppState> = {};
      if (changed) {
        next.entries = entries;
      }
      if (unreadRestore.size > 0) next.feedCounts = feedCounts;
      if (clearedOpenedReadIds && current.openedReadIds === clearedOpenedReadIds) {
        next.openedReadIds = prevOpenedReadIds;
      }
      set(next);
      updateCapturedViewCache('restore');
      scheduleCountsReconcile(takenOver ? { force: true } : undefined);
      get().showToast(`全部已读保存失败：${extractError(e)}`, {
        label: '重试',
        key: actionContext.key,
        run: () => runMarkAllRead(actionContext, true),
      });
    });
  };

  return {
  activeContentLayout: 'article',
  activeViewFilter: 'all',
  activeFeedFilter: 'all',
  timelineFilter: 'unread',
  timelineSort: 'newest',
  /* TASK-115①：切换返回恢复信号（字段说明见 types.ts）。仅在下方三个导航
     action 的**缓存命中**分支 bump（entries 同步恢复时）；cache-miss 重拉、
     切排序（重拉新语境）与启动装载一律不 bump。 */
  switchRestoreNonce: 0,

  /* ================= 导航 ================= */

  selectLayout: (layout) => {
    /* TASK-115①：本次导航是否真的改变筛选上下文——布局段变化，或范围段被
       下方的 'all' 重置改变。同上下文重复导航（点击当前布局）不是「切换返回」，
       不 bump 恢复信号（否则列表会被拽回上次离开的位置）。 */
    const contextChanged = layout !== get().activeContentLayout || get().activeFeedFilter !== 'all';
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
    const scopeKey = QueryScope.pageKey(get().activeFeedFilter, layout);
    set((s) => ({
      /* TASK-117：镜像取 keyset 游标的 loaded 计数（原游标值即计数） */
      articlesLimit: s.articlesCursor[scopeKey]?.loaded ?? 0,
      articlesExhausted: false,
      articlesLoading: false,
    }));
    const view = get().activeViewFilter;
    const cached = viewEntriesCache.get(QueryScope.viewKey(layout, view, get().activeFeedFilter));
    if (cached) {
      /* TASK-103 → TASK-122：缓存恢复同属快照替换，收口在 mergeSnapshotEntries。
         正文真值源 bodyById（快照恢复不触碰，已加载正文零丢失）；AI 列取缓存行
         真值（缓存行本身就是最近一次后端行）；fromBackend=false 不 bump 版本、
         不做 body 对齐（对齐只消费后端行，随后必触发的 reload 接手）。 */
      const merged = mergeSnapshotEntries(get().entries, cached.entries, false, 0);
      /* TASK-110①：exhausted 真实判定随快照长度（原筛选视图恒 true、all 视图恒
         false——两者都随分页化失效）；缓存快照即最近一次拉取的首屏/续拉结果，
         「长度 < 页大小 ⇒ 已到底」与拉取时的判定同口径。
         TASK-111①：判定与游标恢复改用缓存**记录的元数据**（exhausted /
         loadedCount）而非截断后的 entries.length——单键实体预算超限尾部截断后，
         截断长度若落在页大小边界附近会把「已到底」误判成「还有数据」（或反），
         且续拉 offset 会回退重拉已去重丢弃的区间；记录值是写入时的真实口径
         （含偏移漂移下 entries 短于游标的形态），恢复行为与写入时逐字一致。
         TASK-115①：entries 恢复与切换返回信号**同一次原子写入**（switchRestoreNonce
         bump，仅 contextChanged——同上下文重复导航不恢复）——Timeline 收到信号后
         按 per-filterKey 锚存档一次性定位（存档锚在恢复列表 → scrollToIndex
         align:start；无存档/锚丢失 → 归零回落）。
         不叠加保证：本信号独立于 TASK-111 的 positionRestoreNonce（刷新保位），
         恢复动作不 bump/消费刷新保位信号；随后的后台 reload 是导航路径调用
         （不带 keepReadingPosition），刷新保位机制对本次切换保持沉默——
         「一次性定位」与「持续跟踪」分流，见 timelineAnchor.ts 头注规则表。 */
      set((s) => ({
        entries: merged.entries,
        articlesExhausted: cached.exhausted,
        /* TASK-122：水合终态/失败态随 bodyById 存活，与快照恢复解耦 */
        ...(contextChanged ? { switchRestoreNonce: s.switchRestoreNonce + 1 } : {}),
      }));
      get().applyArticlesCursor(scopeKey, cached.cursor, cached.exhausted);
    }
    /* TASK-098（与 F5 同口径）：void reload 调用点必须接住 promise——失败提示由
       reload 自身的 toast 给出，这里只吞掉残余重抛，避免 unhandled rejection。 */
    /* TASK-123（审计 P2-5②）：缓存命中恢复后的后台刷新 = 内容刷新——携带
       keepReadingPosition 走保位窗口重取（恢复定位过的锚不因重拉丢失，落地时
       positionRestoreNonce 照常 bump，Timeline 按活锚校正索引漂移）；缓存未命中
       = 新语境（规则表裁定：窗口深度属于旧语境，首屏起步不保窗口）。切换动作
       本身依旧不 bump positionRestoreNonce——信号只由刷新落地发出，分流不变。 */
    if (view !== 'all') void get().reloadFilteredEntries(view, cached ? { keepReadingPosition: true } : undefined).catch(() => { /* 失败已可见（reloadFilteredEntries 内 toast） */ });
    else void get().reloadFromBackend(cached ? { keepReadingPosition: true } : undefined).catch(() => { /* 失败已可见（reloadFromBackend 内 toast） */ });
  },

  /* TASK-052 契约（per-scope 游标）：游标与列表口径绑定，切换口径时必须让两者
     一致。写入器分两条路，组件不得绕开：
     - reloadFromBackend / reloadFilteredEntries / anchorToArticle：拉回数据后用
       **该范围自己的游标**原子写入（entries 与 articlesLimit 同一次 set）；
     - applyArticlesCursor（本 action）：只做镜像，用于「已有 entries 与目标游标不
       匹配、但无需重新拉取」的同步恢复（目前只有 selectView 缓存命中走这条）。
     之所以不暴露成通用 setter：单纯写 articlesLimit 而不动 entries（D3 缺陷当时
     可达的形态）会把「游标指向的位置」与「列表里的内容」拆开，下一次 loadMore 的
     起点就会越过列表内容、整段文章静默丢失。
     TASK-117：第二参从计数改为完整 keyset 游标（ArticlesCursorState）——
     articlesLimit 镜像写 cursor.loaded（计数口径不变），articlesCursor 存完整锚。 */
  applyArticlesCursor: (scopeKey, cursor, exhausted) =>
    set((s) => ({
      articlesLimit: cursor.loaded,
      articlesCursor: { ...s.articlesCursor, [scopeKey]: cursor },
      articlesExhausted: exhausted,
      articlesLoading: false,
    })),

  selectView: (view) => {
    /* 视图缓存优先：命中则同步恢复该视图上次的 entries（零延迟、无渲染卡顿），
       再后台异步刷新保证数据最新。数量悬殊切换（收藏19 ↔ 全部2122）不再经历
       「清空 → 拉取 → 一次性渲染数百张卡片」的卡顿。
       TASK-052：缓存键带上订阅范围（源A 的首批≠全部的首批）；缓存里只有内容，
       游标仍需经 applyArticlesCursor 收口写入（不裸写 articlesLimit）。
       TASK-063（历史）：恢复时曾需处置滞留水合标记（按 id 平行 Map 会把不属于
       本快照的「已水合」误判带回来）。TASK-122 起水合状态收敛为 bodyById 记录的
       state（按文章实体记账，不随快照行传播），滞留形态在结构上不再存在；
       快照恢复只还原视图行，正文/终态由 bodyById 直接命中。 */
    /* TASK-115①：同 selectLayout——视图段真的变化才是「切换返回」；同上下文
       重复导航（点击当前视图）不 bump 恢复信号。 */
    const contextChanged = view !== get().activeViewFilter;
    const scopeKey = QueryScope.pageKey(get().activeFeedFilter, get().activeContentLayout);
    const cached = viewEntriesCache.get(QueryScope.viewKey(get().activeContentLayout, view, get().activeFeedFilter));
    if (cached) {
      /* TASK-103 → TASK-122：同 selectLayout——快照恢复收口在 mergeSnapshotEntries
         （正文真值源 bodyById，恢复不触碰；fromBackend=false 不 bump 不对齐）。 */
      const merged = mergeSnapshotEntries(get().entries, cached.entries, false, 0);
      /* TASK-110①：exhausted 真实判定随快照长度（与 selectLayout 同口径收口——
         原写法 `view !== 'all'` 是「筛选视图拉全集 ⇒ 恒已到底」的旧语义：分页化后
         筛选视图的缓存快照可能是未满页的部分页，误标已到底会挡住续拉，旧文章
         在「缓存命中 + 后台刷新失败」的窗口内不可达）。缓存快照即最近一次拉取的
         首屏/续拉结果，「长度 < 页大小 ⇒ 已到底」与拉取时的判定同口径。
         TASK-111①：与 selectLayout 同口径——判定/游标恢复用缓存记录的元数据
         （exhausted / loadedCount），不重算截断后的快照长度（理由见 selectLayout）。
         TASK-115①：同 selectLayout——entries 恢复与切换返回信号同一次原子写入
         （不叠加保证见 selectLayout 注释；仅 contextChanged）。 */
      set((s) => ({
        activeViewFilter: view,
        openedReadIds: {},
        entries: merged.entries,
        articlesExhausted: cached.exhausted,
        /* TASK-122：水合终态/失败态随 bodyById 存活，与快照恢复解耦 */
        ...(contextChanged ? { switchRestoreNonce: s.switchRestoreNonce + 1 } : {}),
      }));
      get().applyArticlesCursor(scopeKey, cached.cursor, cached.exhausted);
      /* 后台静默刷新（不阻塞切换）：状态/内容可能已变 */
      /* TASK-098（与 F5 同口径）：同 selectLayout——接住 reload 重抛，失败提示由 reload 自身给出 */
      /* TASK-123（审计 P2-5②）：同 selectLayout——缓存命中恢复后的后台刷新携带
         keepReadingPosition 走保位窗口重取（切回定位过的锚不因重拉丢失）。 */
      if (view !== 'all') void get().reloadFilteredEntries(view, { keepReadingPosition: true }).catch(() => { /* 失败已可见（reloadFilteredEntries 内 toast） */ });
      else void get().reloadFromBackend({ keepReadingPosition: true }).catch(() => { /* 失败已可见（reloadFromBackend 内 toast） */ });
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
     activeFeedFilter，自带代际/竞态守卫丢弃过期结果。恢复时快照合并收口在
     mergeSnapshotEntries（TASK-122：正文真值源 bodyById，恢复只还原视图行，
     理由同 selectView）。mock 模式保持纯游标镜像（不触发 IPC、
     不把 mock 会话翻成 tauri）。 */
  selectFeed: (feedId) => {
    /* TASK-115①：同 selectLayout——范围段真的变化才是「切换返回」；同上下文
       重复导航（点击当前源/分类）不 bump 恢复信号。 */
    const contextChanged = feedId !== get().activeFeedFilter;
    const scopeKey = QueryScope.pageKey(feedId, get().activeContentLayout);
    set((s) => ({
      activeFeedFilter: feedId,
      openedReadIds: {},
      /* TASK-117：镜像取 keyset 游标的 loaded 计数（原游标值即计数） */
      articlesLimit: s.articlesCursor[scopeKey]?.loaded ?? 0,
      articlesExhausted: false,
      articlesLoading: false,
    }));
    if (get().dataMode !== 'tauri') return;
    const view = get().activeViewFilter;
    const cached = viewEntriesCache.get(QueryScope.viewKey(get().activeContentLayout, view, feedId));
    if (cached) {
      /* TASK-103 → TASK-122：同 selectLayout——快照恢复收口在 mergeSnapshotEntries
         （正文真值源 bodyById，恢复不触碰；fromBackend=false 不 bump 不对齐）。 */
      const merged = mergeSnapshotEntries(get().entries, cached.entries, false, 0);
      /* TASK-110①：exhausted 真实判定随快照长度（与 selectLayout/selectView 同口径
         收口——`view !== 'all'` 的恒真/恒 false 旧语义随分页化失效，理由见 selectLayout）。
         TASK-111①：与 selectLayout 同口径——判定/游标恢复用缓存记录的元数据
         （exhausted / loadedCount），不重算截断后的快照长度（理由见 selectLayout）。
         TASK-115①：同 selectLayout——entries 恢复与切换返回信号同一次原子写入
         （不叠加保证见 selectLayout 注释；仅 contextChanged）。 */
      set((s) => ({
        entries: merged.entries,
        articlesExhausted: cached.exhausted,
        /* TASK-122：水合终态/失败态随 bodyById 存活，与快照恢复解耦 */
        ...(contextChanged ? { switchRestoreNonce: s.switchRestoreNonce + 1 } : {}),
      }));
      get().applyArticlesCursor(scopeKey, cached.cursor, cached.exhausted);
    }
    /* TASK-098（与 F5 同口径）：同 selectLayout——接住 reload 重抛，失败提示由 reload 自身给出 */
    /* TASK-123（审计 P2-5②）：同 selectLayout——缓存命中恢复后的后台刷新携带
       keepReadingPosition 走保位窗口重取（切回定位过的锚不因重拉丢失）；未命中
       = 新语境首屏起步（不保窗口）。 */
    if (view !== 'all') void get().reloadFilteredEntries(view, cached ? { keepReadingPosition: true } : undefined).catch(() => { /* 失败已可见（reloadFilteredEntries 内 toast） */ });
    else void get().reloadFromBackend(cached ? { keepReadingPosition: true } : undefined).catch(() => { /* 失败已可见（reloadFromBackend 内 toast） */ });
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
     整体替换——与 selectView 缓存命中路径同构。
     TASK-110③：筛选视图（收藏/未读/今天）从「全集本地重排、切排序不调后端」
     改为与「全部」视图同构——分页化后全集不再在内存里，本地重排只够重排已加载
     页且续拉口径会随排序错位，排序改由服务端承载（重拉当前范围）；已加载条目的
     正文随 bodyById 实体存活（TASK-122），重拉替换快照行不影响正文显示。 */
  toggleTimelineSort: () => {
    viewEntriesCache.clear();
    set((s) => ({
      timelineSort: s.timelineSort === 'newest' ? 'oldest' : 'newest',
      openedReadIds: {},
    }));
    /* F5（Batch 1/2 独立审查 P3）：
       - dataMode 守卫：mock 模式没有后端，重拉不仅多余，还会在落地时把 mock
         会话翻成 tauri（reloadFromBackend 成功路径写 dataMode:'tauri'）；
       - TASK-110③ 前：筛选视图拉的本就是全集（limit 100000，不分页），切排序只
         需本地重排、不调后端——该前提已随筛选视图分页化废除，两类视图统一按
         新排序重拉（重拉入口随视图分流：reloadFilteredEntries / reloadFromBackend）；
       - reload 失败时提示由 reload 自身给出（reloadFromBackend toast 后重抛、
         reloadFilteredEntries toast 后吞掉），void 调用点必须接住 .catch，
         否则 unhandled rejection。 */
    if (get().dataMode !== 'tauri') return;
    const view = get().activeViewFilter;
    if (view !== 'all') void get().reloadFilteredEntries(view).catch(() => { /* 失败已可见（reloadFilteredEntries 内 toast） */ });
    else void get().reloadFromBackend().catch(() => { /* 失败已可见（reloadFromBackend 内 toast） */ });
  },

  markCurrentViewAllRead: () => {
    const st = get();
    const ids = selectVisibleEntries(st).map((i) => i.id);
    if (st.dataMode !== 'tauri') {
      /* mock 模式：无库可写，仅本地标读 + 提示，绝不伪造落库（(f) 契约不变） */
      markEntriesRead(new Set(ids));
      set({ openedReadIds: {} });
      get().showToast('已全部标为已读');
      return;
    }
    const scope = st.activeFeedFilter;
    const { feed_id: feedId, folder_id: folderId } = QueryScope.markScope(scope);
    const view = st.activeViewFilter;
    const starredOnly = view === 'starred';
    const sinceMs = view === 'today' ? startOfLocalDayMs() : undefined;
    const layout = st.activeContentLayout;
    const contextBase = {
      scope,
      feedId,
      folderId,
      view,
      timelineFilter: st.timelineFilter,
      layout,
      starredOnly,
      sinceMs,
    };
    const context: MarkAllReadContext = {
      ...contextBase,
      ids: [...ids],
      key: markAllReadContextKey(contextBase),
    };
    runMarkAllRead(context, false);
  },
  };
};
