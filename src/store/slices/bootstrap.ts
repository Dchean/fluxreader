// Note: 会话成员固定、属性按 ID 回读 — 见 .agents/notes/proposed/architecture/2026-11-09-分区阅读与源内身份及持续同步契约.md
import type { StateCreator } from 'zustand';
import { api, articleRowToEntry, extractError, folderRowsToCategories } from '../../lib/api';
import type { ArticleListArgs } from '../../lib/api';
import { peekReturnAnchor } from '../../lib/readingPosition';
import {
  ARTICLES_PAGE_SIZE, appStore, beginCountsFetch, beginEntrySnapshotRead, buildFeedIndex, buildSourceIndex,
  buildScopeCountsSnapshot, canLandCounts, currentFlagWriteSettleSerial,
  currentLocalFlagWriteSerial, emptyArticlesCursor,
  feedCountsFromRows, mergeSnapshotEntries, QueryScope, reconcileCategories,
  scheduleCountsReconcile, setViewEntriesSnapshot, viewEntriesCache,
} from '../internals';
import { applyConfigRead, beginConfigRead } from '../aiConfigWrites';
import {
  acceptsReadingResponse, captureReadingQuery, createReadingSession,
  nextReadingPage, projectReadingMembers, sameReadingNavigation,
} from '../readingSession';
import type { ReadingNavigation, ReadingQuery, ReadingSession } from '../readingSession';
import type { AppState } from '../types';
import type { ArticleEntry } from '../../types';

export type BootstrapSlice = Pick<AppState,
  | 'categories' | 'entries' | 'feedIndex' | 'sourceIndex' | 'readingSession'
  | 'feedCounts' | 'scopeCounts' | 'dataMode' | 'dataLoading' | 'bootstrapError'
  | 'articlesLimit' | 'articlesLoading' | 'backendReloadsInFlight'
  | 'articlesExhausted' | 'articlesCursor' | 'positionRestoreNonce'
  | 'reloadFromBackend' | 'loadMoreArticles' | 'reloadFilteredEntries'
  | 'anchorToArticle' | 'bootstrapFromBackend' | 'retryBootstrap'
>;

let sessionGeneration = 0;
let reloadSerial = 0;
let pageSerial = 0;
const reloadsInFlight = new Set<number>();
let pendingStartupLandGate: Promise<void> | null = null;
export function isBackendReloadInFlight(): boolean { return reloadsInFlight.has(reloadSerial); }

function navigationOf(s: AppState): ReadingNavigation {
  return {
    layout: s.activeContentLayout, view: s.activeViewFilter, scope: s.activeFeedFilter,
    filter: s.timelineFilter, sort: s.timelineSort,
  };
}

/** 日期只在会话建立时捕获。只传毫秒边界，不让服务器在续页时重算“今天”。 */
function memberArgs(query: ReadingQuery): ArticleListArgs {
  return {
    ...QueryScope.args(query.scope, query.sort, query.layout),
    ...QueryScope.viewFilter(query.view),
    ...(query.view === 'all' && query.filter === 'unread' ? { only_unread: true } : {}),
    ...(query.view === 'today' ? { since_ms: query.dayStartMs, until_ms: query.dayEndMs } : {}),
  };
}

/** 明确缺行保留同一个 ID 的可见位置；空/残缺响应不被当作删除。 */
async function fetchMembers(ids: readonly string[], previous: readonly ArticleEntry[]): Promise<ArticleEntry[]> {
  if (!ids.length) return [];
  const result = await api.getArticleUpdates(ids.map(Number));
  if (!result) throw new Error('后端返回空属性快照');
  const rows = new Map(result.items.map((row) => [String(row.id), articleRowToEntry(row)]));
  const missing = new Set(result.missing_ids.map(String));
  const prior = new Map(previous.map((entry) => [entry.id, entry]));
  return ids.map((id) => {
    const row = rows.get(id);
    if (row) return row;
    if (!missing.has(id)) throw new Error(`条目 ${id} 的属性回读不完整`);
    const old = prior.get(id);
    return {
      ...(old ?? { id, feedId: '', title: '条目已不可用', publishedAt: 0,
        isRead: true, isStarred: false, tags: [], source: 'direct' as const,
        snippet: '', author: '', translatedContent: '', aiSummary: '' }),
      unavailable: true, bodyMissing: true,
      snippet: '此条目已不在本地；切换筛选后更新列表', cover: undefined, imageUrl: undefined, audioUrl: undefined,
    };
  });
}

/** 原字段版本/写入前沿仍保护在途乐观写，固定会话只接管成员关系。 */
function readFrontier() {
  return beginEntrySnapshotRead();
}

export async function bootstrapGithubAuth() {
  try {
    const acc = await api.githubLoginStatus();
    if (acc) appStore().setState({ githubAccount: acc });
  } catch { /* 浏览器预览无后端 */ }
}

export async function confirmAppliedConfig(): Promise<void> {
  const cred = beginConfigRead();
  const [folders, feeds, sources] = await Promise.all([api.listFolders(), api.listFeeds(), api.listArticleSources()]);
  if (!folders || !feeds || !sources) throw new Error('后端不可用（配置回读失败）');
  const categories = folderRowsToCategories(folders, feeds);
  applyConfigRead(categories, cred);
  appStore().setState({ ...reconcileCategories(appStore().getState(), categories), sourceIndex: buildSourceIndex(categories, sources) });
}

export const createBootstrapSlice: StateCreator<AppState, [], [], BootstrapSlice> = (set, get) => {
  const beginReload = (request: number) => { reloadsInFlight.add(request); set({ backendReloadsInFlight: isBackendReloadInFlight() ? 1 : 0 }); };
  const endReload = (request: number) => { reloadsInFlight.delete(request); set({ backendReloadsInFlight: isBackendReloadInFlight() ? 1 : 0 }); };
  const isCurrent = (session: ReadingSession) => {
    const current = get().readingSession;
    return current != null && acceptsReadingResponse(current, session.generation, session.query)
      && sameReadingNavigation(session.query, navigationOf(get()));
  };
  const landRows = (session: ReadingSession, rows: ArticleEntry[], frontier: ReturnType<typeof readFrontier>, opts?: { restore?: boolean; refresh?: boolean }) => {
    const merged = mergeSnapshotEntries(get().entries, rows, true, frontier.rowsFetchedAt, frontier).entries;
    const entries = projectReadingMembers(session, get().entries, merged);
    const selected = get().entries.find((entry) => entry.id === get().activeArticleId);
    if (selected && !entries.some((entry) => entry.id === selected.id)) entries.push(selected);
    const cursor = emptyArticlesCursor(session.loadedCount);
    const exhausted = session.loadedCount >= session.memberIds.length;
    const scopeKey = QueryScope.pageKey(session.query.scope, session.query.layout);
    setViewEntriesSnapshot(QueryScope.viewKey(session.query.layout, session.query.view, session.query.scope), entries, cursor, exhausted, session.windowStart);
    set((s) => ({
      readingSession: session, entries,
      articlesLimit: session.loadedCount, articlesCursor: { ...s.articlesCursor, [scopeKey]: cursor },
      articlesExhausted: exhausted, articlesLoading: false,
      ...(opts?.restore ? { switchRestoreNonce: s.switchRestoreNonce + 1 } : {}),
      ...(opts?.refresh ? { positionRestoreNonce: s.positionRestoreNonce + 1 } : {}),
    }));
  };
  const runPage = async (session: ReadingSession, replay = false): Promise<void> => {
    const current = get().readingSession;
    if (!isCurrent(session) || current?.loadedCount !== session.loadedCount || isBackendReloadInFlight()) {
      if (replay) get().showToast('加载更多重试上下文已失效，未执行');
      return;
    }
    if (get().articlesLoading || get().articlesExhausted) {
      if (replay) get().showToast('加载更多重试尚不能执行，请稍后重试', {
        label: '重试', key: `list:load-more:${session.generation}:${session.loadedCount}`,
        run: () => { void runPage(session, true); },
      });
      return;
    }
    const ids = nextReadingPage(session, ARTICLES_PAGE_SIZE);
    const request = ++pageSerial;
    const frontier = readFrontier();
    set({ articlesLoading: true });
    try {
      const rows = await fetchMembers(ids, get().entries);
      if (request !== pageSerial || !isCurrent(session) || get().readingSession?.loadedCount !== session.loadedCount) return;
      landRows({ ...session, loadedCount: session.loadedCount + ids.length }, rows, frontier);
    } catch (error) {
      if (request !== pageSerial || !isCurrent(session)) return;
      get().showToast(`加载更多失败：${extractError(error)}`, {
        label: '重试', key: `list:load-more:${session.generation}:${session.loadedCount}`,
        run: () => { void runPage(session, true); },
      });
    } finally {
      frontier.release();
      if (request === pageSerial) set({ articlesLoading: false });
    }
  };

  return {
    categories: [], entries: [], feedIndex: new Map(), sourceIndex: new Map(), readingSession: null,
    feedCounts: new Map(), scopeCounts: null,
    dataMode: 'mock', dataLoading: true, bootstrapError: null,
    articlesLimit: 0, articlesCursor: {}, articlesLoading: false,
    backendReloadsInFlight: 0, articlesExhausted: false, positionRestoreNonce: 0,

    reloadFromBackend: async (opts) => {
      const request = ++reloadSerial;
      ++pageSerial;
      const initialNavigation = navigationOf(get());
      const startupGate = pendingStartupLandGate;
      const aiCred = beginConfigRead();
      const countsToken = beginCountsFetch();
      const frontier = readFrontier();
      const settleSerial = currentFlagWriteSettleSerial();
      beginReload(request);
      set({ articlesLoading: false });
      // 元数据首读与设置并行；成员查询必须看到启动筛选已恢复的输入。
      const metadata = Promise.all([api.listFolders(), api.listFeeds(), api.listArticleSources(), api.countsSnapshot(initialNavigation.layout)]);
      // 等设置期间也立即接住元数据拒绝，避免早拒绝变成 unhandled rejection。
      const metadataSettled = metadata.then((value) => ({ value }), (error: unknown) => ({ error }));
      try {
        if (startupGate) await startupGate.catch(() => undefined);
        if (request !== reloadSerial) return;
        if (!startupGate && !sameReadingNavigation(initialNavigation, navigationOf(get()))) return;
        const nav = navigationOf(get());
        const previousSession = get().readingSession;
        const refresh = previousSession?.ready === true && sameReadingNavigation(previousSession.query, nav);
        const requeryWindow = !refresh && opts?.keepReadingPosition && previousSession
          && sameReadingNavigation(previousSession.query, nav) ? previousSession : undefined;
        let session = refresh ? previousSession : createReadingSession(++sessionGeneration, captureReadingQuery(nav));
        if (requeryWindow) session = { ...session, memberIds: requeryWindow.memberIds,
          windowStart: requeryWindow.windowStart, loadedCount: requeryWindow.loadedCount };
        if (!refresh) set({ readingSession: session });
        const cache = !refresh ? viewEntriesCache.get(QueryScope.viewKey(nav.layout, nav.view, nav.scope)) : undefined;
        if (!refresh) {
          const ids = await api.listArticleMemberIds(memberArgs(session.query));
          if (request !== reloadSerial || !isCurrent(session)) return;
          if (!ids) throw new Error('后端返回空成员快照');
          session = { ...session, ready: true, memberIds: [...new Set(ids.map(String))] };
          const cachedStart = cache?.windowStart ?? 0;
          const startId = cachedStart > 0 ? cache?.entries[0]?.id : undefined;
          const newStart = startId ? session.memberIds.indexOf(startId) : 0;
          session.windowStart = Math.max(0, newStart);
          const depth = Math.max(ARTICLES_PAGE_SIZE, (cache?.cursor.loaded ?? 0) - cachedStart);
          if (cache && !requeryWindow) {
            const key = `${nav.layout}|${nav.view}|${nav.scope}|${nav.filter}|${nav.sort}`;
            const anchor = peekReturnAnchor(key);
            const anchorPosition = anchor ? session.memberIds.indexOf(anchor.id) : -1;
            if (anchorPosition >= 0
              && (anchorPosition < session.windowStart || anchorPosition >= session.windowStart + depth)) {
              // 真实存档锚决定返回窗口；边界首项消失不等于用户锚消失。
              // 保留缓存中的相对邻域，锚不在缓存截片时从锚开始，不从0拉至深锚。
              const offset = cache.entries.findIndex((entry) => entry.id === anchor?.id);
              session.windowStart = Math.max(0, anchorPosition - Math.max(0, offset));
            }
          }
          session.loadedCount = Math.min(session.memberIds.length, session.windowStart + depth);
          if (requeryWindow) {
            // 头部插入不能把仍存活的页尾活锚挤出已加载区；只定位旧窗口边界，不复用旧成员关系。
            // 深搜索窗口从其新位置起读，不为保位加载0到深锚之间的属性，更不回读正文。
            const positions = new Map(session.memberIds.map((id, index) => [id, index]));
            let firstRetained: number | undefined;
            let lastRetained = -1;
            for (const id of requeryWindow.memberIds.slice(requeryWindow.windowStart, requeryWindow.loadedCount)) {
              const position = positions.get(id);
              if (position === undefined) continue;
              firstRetained = Math.min(firstRetained ?? position, position);
              lastRetained = Math.max(lastRetained, position);
            }
            if (firstRetained !== undefined) {
              session.windowStart = requeryWindow.windowStart > 0 ? firstRetained : 0;
              session.loadedCount = Math.min(session.memberIds.length,
                Math.max(session.windowStart + depth, lastRetained + 1));
            }
          }
        }
        const ids = session.memberIds.slice(session.windowStart, session.loadedCount);
        const [rows, metaResult] = await Promise.all([fetchMembers(ids, get().entries), metadataSettled]);
        if (request !== reloadSerial || !isCurrent(session)) return;
        if ('error' in metaResult) throw metaResult.error;
        const [folders, feeds, sources, counts] = metaResult.value;
        if (!folders || !feeds || !sources) throw new Error('后端返回空快照，装载中止');
        const categories = folderRowsToCategories(folders, feeds);
        applyConfigRead(categories, aiCred);
        const settleAdvanced = currentFlagWriteSettleSerial() !== settleSerial;
        const countsLand = canLandCounts(countsToken)
          && currentLocalFlagWriteSerial() === frontier.writtenAfterSerial && !settleAdvanced;
        if (!countsLand && settleAdvanced && canLandCounts(countsToken) && currentLocalFlagWriteSerial() === frontier.writtenAfterSerial) {
          scheduleCountsReconcile({ force: true });
        }
        const feedCounts = countsLand ? feedCountsFromRows(counts?.feeds ?? null) : null;
        set({
          ...reconcileCategories(get(), categories), sourceIndex: buildSourceIndex(categories, sources),
          ...(feedCounts ? {
            feedCounts,
            scopeCounts: buildScopeCountsSnapshot(counts?.scope ?? null, nav.layout),
          } : {}),
          dataMode: 'tauri', dataLoading: false, bootstrapError: null,
        });
        // Note: 同上下文重筛保当前活锚，跨上下文返回才消费存档 — 见 .agents/notes/implemented/architecture/2026-10-05-查询与分页契约.md
        const keepReadingPosition = refresh || opts?.keepReadingPosition === true;
        landRows(session, rows, frontier, {
          refresh: keepReadingPosition,
          restore: !keepReadingPosition && cache != null,
        });
        void get().refreshSyncStatus();
        void get().refreshSyncQueueStats();
      } catch (error) {
        if (request !== reloadSerial) return;
        set({ dataLoading: false });
        get().showToast(`刷新失败：${extractError(error)}`);
        throw error;
      } finally { frontier.release(); endReload(request); }
    },

    loadMoreArticles: async () => {
      const s = get();
      if (s.dataMode !== 'tauri' || !s.readingSession?.ready || s.articlesLoading || s.articlesExhausted) return;
      await runPage(s.readingSession);
    },

    reloadFilteredEntries: async (view) => {
      if (get().dataMode !== 'tauri' || view !== get().activeViewFilter) return;
      await get().reloadFromBackend();
    },

    anchorToArticle: async (articleId) => {
      if (get().dataMode !== 'tauri') return;
      const request = ++reloadSerial;
      const nav = navigationOf(get());
      const existing = get().readingSession;
      const reuse = existing?.ready === true && sameReadingNavigation(existing.query, nav);
      let session = reuse ? existing : createReadingSession(++sessionGeneration, captureReadingQuery(nav));
      if (!reuse) ++pageSerial;
      set({ readingSession: session, ...(!reuse ? { articlesLoading: false } : {}) });
      const frontier = readFrontier();
      beginReload(request);
      try {
        if (!session.ready) {
          const ids = await api.listArticleMemberIds(memberArgs(session.query));
          if (request !== reloadSerial || !isCurrent(session)) return;
          if (!ids) throw new Error('后端返回空成员快照');
          session = { ...session, ready: true, memberIds: [...new Set(ids.map(String))] };
        }
        const position = session.memberIds.indexOf(articleId);
        if (position < 0) {
          if (!reuse) {
            session = { ...session, loadedCount: Math.min(session.memberIds.length, ARTICLES_PAGE_SIZE) };
            const rows = await fetchMembers(session.memberIds.slice(0, session.loadedCount), get().entries);
            if (request !== reloadSerial || !isCurrent(session)) return;
            landRows(session, rows, frontier);
          }
          const [entry] = await fetchMembers([articleId], get().entries);
          if (request !== reloadSerial || !isCurrent(session)) return;
          if (!entry || entry.unavailable) throw new Error('文章不存在');
          const entries = mergeSnapshotEntries(get().entries, [entry], true, frontier.rowsFetchedAt, frontier).entries;
          set((s) => ({ entries: [...s.entries.filter((item) => item.id !== articleId), ...entries] }));
        } else if (position < session.windowStart || position >= session.loadedCount || !get().entries.some((entry) => entry.id === articleId)) {
          ++pageSerial;
          const start = position;
          session = { ...session, windowStart: start, loadedCount: Math.min(session.memberIds.length, start + ARTICLES_PAGE_SIZE) };
          const rows = await fetchMembers(session.memberIds.slice(start, session.loadedCount), get().entries);
          if (request !== reloadSerial || !isCurrent(session)) return;
          landRows(session, rows, frontier);
        }
        if (request !== reloadSerial || !isCurrent(session)) return;
        get().selectArticle(articleId);
      } catch (error) {
        if (request === reloadSerial) get().showToast(`打开文章失败：${extractError(error)}`);
      } finally { frontier.release(); endReload(request); }
    },
  /** 启动装载：Tauri 环境下从 SQLite 拉数据；仅浏览器开发回退 mock。
    注意：Tauri 启动填充真实数据（dataLoading 骨架期间不渲染任何默认数据，
    防止历史「测试订阅一闪而过」——卸载重装后真实空库替换 mock 的时序问题）。 */
  bootstrapFromBackend: async () => {
    if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) {
      /* 浏览器开发预览：亮出 mock 数据供看效果。动态 import（仅本分支可达）：
         生产构建（Tauri）走不到这里，vite 把 mockData 切成独立 chunk，
         主 bundle 不再携带 10KB 级演示数据。chunk 加载失败与后端分支同契约
         （复核 P3-2）：bootstrapError + 重试入口，dataLoading 不永卡骨架。 */
      try {
        const { createInitialCategories, createInitialEntries } = await import('../../mockData');
        const cats = createInitialCategories();
        set({
          categories: cats,
          entries: createInitialEntries(),
          feedIndex: buildFeedIndex(cats),
          sourceIndex: buildSourceIndex(cats),
          dataMode: 'mock',
          dataLoading: false,
        });
      } catch (e) {
        console.error('mock data chunk load failed:', e);
        set({ dataLoading: false, bootstrapError: extractError(e) });
      }
      return;
    }
    /* 迁移前自动备份失败不阻塞启动，但必须在首次界面上可见；后端以一次性
       settings 警告承接，读取即消费，避免每次启动重复提示。 */
    void api.takeStartupBackupWarning()
      .then((warning) => {
        if (warning) get().showToast(warning);
      })
      .catch(() => { /* 警告通道失败不阻塞启动装载 */ });
    /* 设置与元数据并行；成员查询在设置接受后开始，捕获真实启动筛选。 */
    const settingsTask = get().bootstrapSettings();
    pendingStartupLandGate = settingsTask;
    const releaseSettingsGate = () => {
      if (pendingStartupLandGate === settingsTask) pendingStartupLandGate = null;
    };
    void settingsTask.then(releaseSettingsGate, releaseSettingsGate);
    try {
      await get().reloadFromBackend();
    } catch (e) {
      /* P0-2：后端异常时展示错误态 + 重试入口，绝不回退 mock 演示数据——
         假订阅/假文章会让用户误以为数据还在，随后任何操作都写库失败 */
      console.error('bootstrap from backend failed:', e);
      set({ dataLoading: false, bootstrapError: extractError(e) });
    }
  },

  /** 启动失败重试：清错误态后重新装载（不重载页面） */
  retryBootstrap: async () => {
    set({ bootstrapError: null, dataLoading: true });
    await get().bootstrapFromBackend();
  },
  };
};
