import type { StateCreator } from 'zustand';
import { createInitialCategories, createInitialEntries } from '../../mockData';
import { api, articleRowToEntry, extractError, folderRowsToCategories } from '../../lib/api';
import { appStore, buildFeedIndex, markEntriesRead, reconcileCategories, viewCacheKey, viewEntriesCache } from '../internals';
import { numericId } from '../selectors';
import type { AppState } from '../types';
import type { ContentLayoutType } from '../../types';

/** 启动与数据快照 slice：SQLite ⇄ mock 数据源、全量/分页/筛选拉取与锚定打开。
 *
 *  Pick 的键集即本 slice 的全部键；与其它 slice 两两不相交（合起来 = 原 useAppStore 全集）。
 */
export type BootstrapSlice = Pick<
  AppState,
  | 'categories'
  | 'entries'
  | 'feedIndex'
  | 'feedCounts'
  | 'dataMode'
  | 'dataLoading'
  | 'bootstrapError'
  | 'articlesLimit'
  | 'articlesLoading'
  | 'articlesExhausted'
  | 'reloadFromBackend'
  | 'loadMoreArticles'
  | 'reloadFilteredEntries'
  | 'anchorToArticle'
  | 'bootstrapFromBackend'
  | 'retryBootstrap'
>;

/** 文章列表分页大小：首批/每次滚动加载拉取的文章数 */
const ARTICLES_PAGE_SIZE = 500;

/** reloadFromBackend 代际计数：并发 reload 只接受最新一次结果 */
let reloadGeneration = 0;

/** 判断列表查询是否附带正文。虚拟滚动下仅视口约 30 条需要正文，由
    useLazyHydrate 按需批量水合（1 次 IPC）即可；列表查询保持轻量（不含
    正文 HTML），避免每页 500 条背 2-3MB 正文（「列表背正文」是滚动卡顿主因）。 */
function layoutNeedsBody(_layout: ContentLayoutType): boolean {
  return false;
}

/** 应用启动时恢复登录态（设备流 token 持久化在 SQLite，与组件无关） */
export async function bootstrapGithubAuth() {
  try {
    const acc = await api.githubLoginStatus();
    if (acc) appStore().setState({ githubAccount: acc });
  } catch {
    /* 后端不可用（浏览器 mock）静默忽略 */
  }
}

export const createBootstrapSlice: StateCreator<AppState, [], [], BootstrapSlice> = (set, get) => ({
  /* 启动用空数据 + dataLoading 骨架（不用 mock 先行渲染——曾导致卸载重装后
     「测试订阅一闪而过」，同步后被真实空库替换；蓝图中 P1/I3 要求首帧即真实态） */
  categories: [],
  entries: [],
  feedIndex: new Map(),
  feedCounts: new Map(),

  /* mock 数据先行渲染；Tauri 环境启动时 bootstrapFromBackend 会整体替换 */
  dataMode: 'mock',
  dataLoading: true,
  bootstrapError: null,
  articlesLimit: 0,
  articlesLoading: false,
  articlesExhausted: false,

  /* ================= 数据源：后端 SQLite ⇄ mock ================= */

  /** 从后端拉全量快照（folders + feeds + articles）替换本地状态。
      代际守卫：并发调用只接受最新一次的结果——后台刷新事件与用户操作
      同时触发 reload 时，旧快照不会覆盖新快照（布局显示回退的根因）。 */
  reloadFromBackend: async () => {
    const gen = ++reloadGeneration;
    const layout = get().activeContentLayout;
    const [folders, feeds, articles, counts] = await Promise.all([
      api.listFolders(),
      api.listFeeds(),
      api.listArticles({ limit: ARTICLES_PAGE_SIZE, offset: 0, newest_first: true, with_content: layoutNeedsBody(layout) }),
      api.feedCounts(),
    ]);
    if (gen !== reloadGeneration) return; // 已有更新的 reload 在途/完成
    if (!folders || !feeds || articles === null) return;

    const categories = folderRowsToCategories(folders, feeds);
    const feedCounts = new Map<string, { total: number; unread: number; starred: number; today: number }>();
    if (counts) {
      for (const c of counts) {
        feedCounts.set(String(c.feed_id), { total: c.total, unread: c.unread, starred: c.starred, today: c.today });
      }
    }
    const nextEntries = articles.map(articleRowToEntry);
    // 缓存「全部」视图快照：切回时零延迟恢复（视图切换卡顿的根治）
    viewEntriesCache.set(viewCacheKey(get().activeContentLayout, 'all'), nextEntries);
    set((s) => ({
      ...reconcileCategories(s, categories),
      entries: nextEntries,
      feedCounts,
      articlesLimit: ARTICLES_PAGE_SIZE,
      articlesLoading: false,
      articlesExhausted: articles.length < ARTICLES_PAGE_SIZE,
      dataMode: 'tauri',
      dataLoading: false,
      /* 新快照不带正文：清空水合终态，让社交/通知卡片重新水合 */
      hydratedIds: {},
      hydrationErrors: {},
    }));
    /* 顺带刷新连接态：连接/断开后前端标签即时一致 */
    void api.syncStatus().then((st) => {
      if (st && gen === reloadGeneration) set({ syncConnected: st.connected });
    });
    // 当前在筛选视图（收藏/未读/今天）时，reload 后重新拉取完整筛选列表
    // （状态/内容可能变化，entries 需同步刷新为筛选结果）
    const view = get().activeViewFilter;
    if (view !== 'all') void get().reloadFilteredEntries(view);
  },

  /** 滚动到底部按需拉取下一批文章（追加到 entries，不覆盖已加载的）。
      分页游标 articlesLimit 随每次加载累加；若已有更多在途则跳过（防抖）。 */
  loadMoreArticles: async () => {
    const st = get();
    if (st.dataMode !== 'tauri') return;
    if (st.articlesLoading || st.articlesExhausted) return; // 已在加载 / 已到底
    const offset = st.articlesLimit;
    set({ articlesLoading: true });
    try {
      const rows = await api.listArticles({ limit: ARTICLES_PAGE_SIZE, offset, newest_first: true, with_content: layoutNeedsBody(get().activeContentLayout) });
      // 竞态保护：加载期间游标被重置（reload / selectView 命中缓存恢复快照），
      // 丢弃本次追加。必须顺手复位 articlesLoading（D3）：否则该标志永久为 true，
      // 被入口守卫（articlesLoading || articlesExhausted）永久挡住后续所有
      // loadMoreArticles —— 列表停在半截且加载动画常驻。此前"自愈"只因所有写
      // articlesLimit 的路径都顺手置了 false，一旦有不置位的写路径就会锁死。
      if (get().articlesLimit !== offset) {
        set({ articlesLoading: false });
        return;
      }
      const next = rows ? rows.map(articleRowToEntry) : [];
      if (next.length < ARTICLES_PAGE_SIZE) {
        // 不足一页 → 已到底
        set((s) => ({
          entries: next.length ? [...s.entries, ...next] : s.entries,
          articlesLimit: offset + next.length,
          articlesLoading: false,
          articlesExhausted: true,
        }));
      } else {
        set((s) => ({
          entries: [...s.entries, ...next],
          articlesLimit: offset + next.length,
          articlesLoading: false,
        }));
      }
    } catch (e) {
      /* D2：此前这里完全吞错（只复位加载态）——用户侧零提示，滚动加载静默
         停摆、也无人知道原因。与 refreshOneFeed / extractCurrentArticle 同口径：
         复位加载态 + 可见 toast + 一键重试。 */
      set({ articlesLoading: false });
      get().showToast(`加载更多失败：${extractError(e)}`, { label: '重试', run: () => void get().loadMoreArticles() });
    }
  },

  /** 切换视图到收藏/未读/今天时，按后端筛选拉取完整列表（不分页）。
      这些视图的文章数通常远小于「全部」，一次性拉全可接受；且必须拉全——
      「全部」视图的 entries 是分页快照，收藏/未读的老文章（排在最新 N 篇外）
      不在其中，否则筛选视图会漏显示（「收藏视图不显示列表」的根因）。 */
  reloadFilteredEntries: async (view) => {
    if (get().dataMode !== 'tauri') return;
    const rows = await api.listArticles({
      limit: 100000,
      offset: 0,
      newest_first: true,
      only_unread: view === 'unread' ? true : undefined,
      only_starred: view === 'starred' ? true : undefined,
      only_today: view === 'today' ? true : undefined,
      with_content: layoutNeedsBody(get().activeContentLayout),
    });
    if (!rows) return;
    // 竞态保护：拉取期间用户又切了视图，丢弃过期结果
    if (get().activeViewFilter !== view) return;
    // 替换 entries（筛选视图的完整列表），重置分页游标（筛选视图不分页）
    const next = rows.map(articleRowToEntry);
    viewEntriesCache.set(viewCacheKey(get().activeContentLayout, view), next);
    set({
      entries: next,
      articlesLimit: rows.length,
      articlesExhausted: true,
      articlesLoading: false,
      /* 新快照不带正文：清空水合终态，让卡片重新水合 */
      hydratedIds: {},
      hydrationErrors: {},
    });
  },

  /** 搜索/深层打开文章：计算目标文章在当前筛选下的绝对位置，从该页加载列表
      （而非从头拉 500 篇），并选中该文章。解决「搜索结果是很老的文章时，
      列表还停在第 1 页、定位不到」的问题。
      注意：article_index 与 list_articles 用同一组筛选参数（where + 排序），
      保证「位置」与「该位置的列表」对齐。 */
  anchorToArticle: async (articleId) => {
    if (get().dataMode !== 'tauri') return;
    // 递增代际：使先前触发的 reloadFromBackend（如 selectView('all') 触发的）
    // 结果失效，避免其覆盖本锚定结果（竞态）。
    const gen = ++reloadGeneration;
    const st = get();
    // 映射当前订阅范围 → feed_id / folder_id（与 markCurrentViewAllRead 同口径）
    const scope = st.activeFeedFilter;
    const feedId = scope.startsWith('cat-') ? null : scope === 'all' ? null : numericId(scope);
    const folderId = scope.startsWith('cat-') ? numericId(scope) : null;
    const args = {
      feed_id: feedId,
      folder_id: folderId,
      newest_first: st.timelineSort === 'newest',
      limit: ARTICLES_PAGE_SIZE,
      offset: 0,
      with_content: layoutNeedsBody(st.activeContentLayout),
    };
    const pos = await api.articleIndex(args, Number(articleId));
    if (gen !== reloadGeneration) return; // 期间又有更新的导航操作
    if (pos == null) return;
    // 从目标位置加载一页（若位置靠前，offset 为负会被 SQLite 截断为 0，安全）
    const offset = Math.max(0, pos);
    const rows = await api.listArticles({ ...args, offset });
    if (gen !== reloadGeneration) return;
    if (!rows) return;
    const next = rows.map(articleRowToEntry);
    set({
      entries: next,
      articlesLimit: offset + next.length,
      articlesExhausted: next.length < ARTICLES_PAGE_SIZE,
      articlesLoading: false,
      activeArticleId: articleId,
      openedReadIds: { ...get().openedReadIds, [articleId]: true },
      /* 新快照不带正文：清空水合终态，让卡片重新水合 */
      hydratedIds: {},
      hydrationErrors: {},
    });
    // F7：与 selectArticle 同口径——打开时按设置标已读（此前搜索/命令面板
    // 打开的文章不标读，与列表点开行为分叉）
    const { settings: stSettings, dataMode: stMode } = get();
    const target = get().entries.find((a) => a.id === articleId);
    if (stMode === 'tauri' && stSettings.markReadOnOpen && target && !target.isRead) {
      void api.setRead(Number(articleId), true);
      markEntriesRead(new Set([articleId]));
    }
    // 打开文章：触发智能全文（与 selectArticle 一致）
    get().ensureArticleContent(articleId, { extractFulltext: true });
  },

  /** 启动装载：Tauri 环境下从 SQLite 拉数据；浏览器开发/后端异常回退 mock。
    注意：Tauri 启动填充真实数据（dataLoading 骨架期间不渲染任何默认数据，
    防止历史「测试订阅一闪而过」——卸载重装后真实空库替换 mock 的时序问题）。 */
  bootstrapFromBackend: async () => {
    if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) {
      /* 浏览器开发预览：亮出 mock 数据供看效果 */
      const cats = createInitialCategories();
      set({
        categories: cats,
        entries: createInitialEntries(),
        feedIndex: buildFeedIndex(cats),
        dataMode: 'mock',
        dataLoading: false,
      });
      return;
    }
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
});
