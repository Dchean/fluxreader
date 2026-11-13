import type { StateCreator } from 'zustand';
import { api, extractError } from '../../lib/api';
import { applyReadReceipt, asReadReceipts, beginCapturedReadWrite, markEntriesRead, QueryScope, reconcileCountsOrdered, scheduleCountsReconcile, viewEntriesCache } from '../internals';
import { selectVisibleEntries } from '../selectors';
import { captureReadingQuery, createReadingSession } from '../readingSession';
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
  memberIds: readonly string[];
  key: string;
}

function markAllReadContextKey(
  context: Omit<MarkAllReadContext, 'key'>,
): string {
  // Note: 范围类失败按“范围 × 视图 × 布局 × 时间界”分条 — 见 .agents/notes/implemented/feature/2026-11-13-前端交互契约.md
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
  // 重筛仍要新建代次；同语境暂存旧窗口成员只为新查询定位已加载边界，不能据此显示旧集合。
  const invalidatedSession = (keepReadingPosition: boolean) => {
    const session = get().readingSession;
    return keepReadingPosition && session ? { ...session, ready: false } : null;
  };
  const rebuildReadingSession = (keepReadingPosition = false): void => {
    const s = get();
    if (s.dataMode === 'tauri') {
      void s.reloadFromBackend(keepReadingPosition ? { keepReadingPosition: true } : undefined)
        .catch(() => { /* 刷新自身呈现失败，保留重试入口 */ });
      return;
    }
    const query = captureReadingQuery({ layout: s.activeContentLayout, scope: s.activeFeedFilter,
      view: s.activeViewFilter, filter: s.timelineFilter, sort: s.timelineSort });
    const ids = selectVisibleEntries(s).map((entry) => entry.id);
    set({ readingSession: { ...createReadingSession(0, query, ids), ready: true, loadedCount: ids.length }, articlesExhausted: true });
  };
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
    const prevOpenedReadIds = st.openedReadIds;
    let clearedOpenedReadIds: Record<string, boolean> | null = null;
    const memberIds = new Set(context.memberIds);
    const intent = beginCapturedReadWrite(context.memberIds);
    if (contextIsCurrent) {
      clearedOpenedReadIds = {};
      set({ openedReadIds: clearedOpenedReadIds });
    }

    const write = api.setReadBulk(context.memberIds.map(Number), true);
    void write.then((receipt) => {
      intent.finish(false);
      // 成员外只接受后端真实写入的组兄弟，不能从动态scope扩张到未写的搜索实体。
      applyReadReceipt(asReadReceipts(receipt), memberIds, intent.serial);
      get().showToast('已全部标为已读');
      /* 空回执也可能是目标已全读：完整捕获成员的成功声明仍收敛，组外只认真实回执。
         首次已提交但响应丢失后重试不能因“无新翻转”留下旧缓存未读。 */
      // 已乐观的行/离屏重试均由同一字段提交者更新现存副本；范围总计数由下方权威读取负责。
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
      const outcome = intent.finish(true);
      // 字段/计数恢复已经由完整成员声明统一投影，不能按“当前是否离屏”再回补。
      if (clearedOpenedReadIds && get().openedReadIds === clearedOpenedReadIds) {
        set({ openedReadIds: prevOpenedReadIds });
      }
      scheduleCountsReconcile(outcome.takenOver ? { force: true } : undefined);
      get().showToast(`全部已读保存失败：${extractError(e)}`, {
        label: '重试',
        key: context.key,
        run: () => runMarkAllRead(context, true),
      });
    });
  };

  return {
  activeContentLayout: 'article',
  activeViewFilter: 'all',
  activeFeedFilter: 'all',
  timelineFilter: 'unread',
  timelineSort: 'newest',
  /* 切换返回信号由新成员落地时发出；重复同上下文重筛只保当前活锚，
     不能重新消费上次离开时的旧存档。 */
  switchRestoreNonce: 0,

  /* 导航建立新会话；后台属性回读不会走此失效入口。 */
  selectLayout: (layout) => {
    const keepReadingPosition = get().activeContentLayout === layout && get().activeFeedFilter === 'all';
    set({ activeContentLayout: layout, activeFeedFilter: 'all', readingSession: invalidatedSession(keepReadingPosition),
      activeArticleId: null, isShowingTranslatedProse: false, isRawRenderMode: false,
      showFulltext: false, openedReadIds: {}, articlesLoading: false, articlesExhausted: false });
    rebuildReadingSession(keepReadingPosition);
  },
  selectView: (view) => {
    const keepReadingPosition = get().activeViewFilter === view;
    set({ activeViewFilter: view, readingSession: invalidatedSession(keepReadingPosition), openedReadIds: {}, articlesLoading: false, articlesExhausted: false });
    rebuildReadingSession(keepReadingPosition);
  },
  selectFeed: (feedId) => {
    const keepReadingPosition = get().activeFeedFilter === feedId;
    set({ activeFeedFilter: feedId, readingSession: invalidatedSession(keepReadingPosition), openedReadIds: {}, articlesLoading: false, articlesExhausted: false });
    rebuildReadingSession(keepReadingPosition);
  },
  applyArticlesCursor: (scopeKey, cursor, exhausted) => set((s) => ({
    articlesLimit: cursor.loaded, articlesCursor: { ...s.articlesCursor, [scopeKey]: cursor },
    articlesExhausted: exhausted, articlesLoading: false,
  })),
  toggleTimelineFilter: () => {
    set((s) => ({ timelineFilter: s.timelineFilter === 'all' ? 'unread' : 'all',
      readingSession: null, openedReadIds: {}, articlesLoading: false, articlesExhausted: false }));
    rebuildReadingSession();
  },
  toggleTimelineSort: () => {
    viewEntriesCache.clear();
    set((s) => ({ timelineSort: s.timelineSort === 'newest' ? 'oldest' : 'newest',
      readingSession: null, openedReadIds: {}, articlesLoading: false, articlesExhausted: false }));
    rebuildReadingSession();
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
    if (!st.readingSession?.ready) {
      get().showToast('列表尚未就绪，请加载后重试');
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
      memberIds: [...st.readingSession.memberIds],
    };
    const context: MarkAllReadContext = {
      ...contextBase,
      key: markAllReadContextKey(contextBase),
    };
    runMarkAllRead(context, false);
  },
  };
};
