import { create } from 'zustand';
import type {
  ArticleEntry,
  CategoryGroup,
  ContentLayoutType,
  FeedItem,
  ViewFilterType,
} from './types';
import { createInitialCategories, createInitialEntries } from './mockData';
import {
  api,
  extractError,
  folderRowsToCategories,
  articleRowToEntry,
  type RefreshSummary,
} from './lib/api';
import { openExternal } from './lib/external';
import { selectVisibleEntries, numericId } from './store/selectors';
import type { AppState, SettingsState } from './store/types';

/* ============================================================
   全局客户端状态机 —— 对应规范 §2.2 + 原型交互引擎

   设计原则：
   1. store 只放「被跨组件共享的可变状态」与 action；派生数据
      （过滤/排序后的列表、计数）由 selector 钩子在组件层计算，
      避免每次渲染都全量重算。
   2. 条目数据（articles by layout）放在 store 里而不是
      模块级可变数组：消除 splice 副作用，任何变更都走 set()，
      React 才能可靠地重渲染（Tauri 环境下由 SQLite 快照整体替换）。
   3. 导航类 action 统一重置「已读保留快照」，保证未读筛选语义。
   ============================================================ */

/* GitHub 设备流轮询的模块级定时器：常驻不受组件生命周期影响。
   poll 在成功/失败/被新登录覆盖时清掉；未授权期间按 interval 自续。 */
let githubPollTimer: ReturnType<typeof setTimeout> | null = null;

function scheduleGithubPoll(intervalSec: number) {
  if (githubPollTimer) clearTimeout(githubPollTimer);
  githubPollTimer = setTimeout(() => void doGithubPoll(), Math.max(3, intervalSec) * 1000);
}

function clearGithubPoll() {
  if (githubPollTimer) {
    clearTimeout(githubPollTimer);
    githubPollTimer = null;
  }
}

/** 「智能全文」判定：正文是否已是全文（无需 Readability 提取）。
    启发式：只认明确的"正文被截断"信号——摘要型源（少数派等）的结尾标记
    "查看全文/阅读全文/继续阅读/阅读原文" 等。返回 true = 需要提取全文（是摘要）；
    false = 已是全文（跳过，省请求）。
    注意：不按"文本短"判断——论坛（Linux DO）的短帖本身就是完整全文，短 ≠ 摘要；
    也不认"阅读更多"——那是论坛/列表"去原帖看回复"链接，非正文截断。 */
function shouldExtractFulltext(html: string): boolean {
  if (!html) return false;
  const text = html.replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ').trim();
  // 明确的正文截断标记（"全文/原文"字样；不含"阅读更多"= 论坛去原帖链接）
  const truncationMarks = /查看全文|阅读全文|继续阅读|展开全文|阅读原文|查看原文|Read more|read more|Continue reading|continue reading/;
  return truncationMarks.test(text);
}

async function doGithubPoll() {
  githubPollTimer = null;
  try {
    const acc = await api.githubLoginPoll();
    if (acc) {
      clearGithubPoll();
      useAppStore.setState({ githubAccount: acc, githubFlow: null });
      useAppStore.getState().showToast(`已登录 GitHub：${acc.login}（Gist 同步已就绪）`);
    } else {
      const flow = useAppStore.getState().githubFlow;
      if (flow) scheduleGithubPoll(flow.interval);
    }
  } catch (e) {
    clearGithubPoll();
    useAppStore.setState({ githubFlow: null });
    useAppStore.getState().showToast(`GitHub 登录失败：${extractError(e)}`);
  }
}

/* 应用启动时恢复登录态（设备流 token 持久化在 SQLite，与组件无关） */
export async function bootstrapGithubAuth() {
  try {
    const acc = await api.githubLoginStatus();
    if (acc) useAppStore.setState({ githubAccount: acc });
  } catch {
    /* 后端不可用（浏览器 mock）静默忽略 */
  }
}


let toastId = 0;

/** reloadFromBackend 代际计数：并发 reload 只接受最新一次结果 */
let reloadGeneration = 0;

/** 文章列表分页大小：首批/每次滚动加载拉取的文章数 */
const ARTICLES_PAGE_SIZE = 500;

/** 视图切换缓存：key = 「布局 × 视图」→ 该视图最近一次拉取的 entries 快照。
    用途：视图切换（尤其「收藏19 ↔ 全部2122」这类数量悬殊的切换）不再每次
    重新从后端拉取 + 一次性渲染数百张卡片（卡顿根因），而是先同步恢复缓存
    零延迟显示，再后台异步刷新。模块级（非 store 状态）避免触发重渲染。 */
const viewEntriesCache = new Map<string, ArticleEntry[]>();

/** 视图缓存 key：布局 × 视图（订阅范围 'all' 单独缓存；具体 feed/分类范围不缓存——
    范围切换频繁且数据量小，直接拉取更快，避免缓存膨胀） */
function viewCacheKey(layout: ContentLayoutType, view: ViewFilterType): string {
  return `${layout}|${view}`;
}

/** 判断列表查询是否附带正文。虚拟滚动下仅视口约 30 条需要正文，由
    useLazyHydrate 按需批量水合（1 次 IPC）即可；列表查询保持轻量（不含
    正文 HTML），避免每页 500 条背 2-3MB 正文（「列表背正文」是滚动卡顿主因）。 */
function layoutNeedsBody(_layout: ContentLayoutType): boolean {
  return false;
}

/** 由 categories 构建 feedId → { feed, cat } 解析表（每次 categories 变更后重建） */
function buildFeedIndex(categories: CategoryGroup[]) {
  const map = new Map<string, { feed: FeedItem; cat: CategoryGroup }>();
  for (const cat of categories) {
    for (const f of cat.feeds) map.set(f.id, { feed: f, cat });
  }
  return map;
}

/** categories 变更后统一收口：重建解析表 + 级联清理已无归属的条目 */
function reconcileCategories(
  s: Pick<AppState, 'categories' | 'entries'>,
  nextCategories: CategoryGroup[],
): Pick<AppState, 'categories' | 'feedIndex' | 'entries'> {
  const index = buildFeedIndex(nextCategories);
  const entries = s.entries.filter((e) => index.has(e.feedId));
  return { categories: nextCategories, feedIndex: index, entries };
}

/* ============================================================
   正文水合批量队列 —— 首屏/布局切换时几十张可见卡片同帧触发 ensureArticleContent，
   逐篇 getArticle（各一次 IPC + 各一次 set + 全量 selector 重算）是「加载正文
   几秒 + 切换卡顿」的根因。这里把同一帧内的请求合批：微任务 flush 成一次
   get_articles IPC + 一次 set。 */
let hydrationQueue: Set<string> | null = null;
let hydrationFlushScheduled = false;

function enqueueHydration(id: string) {
  if (!hydrationQueue) hydrationQueue = new Set();
  hydrationQueue.add(id);
  if (hydrationFlushScheduled) return;
  hydrationFlushScheduled = true;
  /* 微任务：等当前同步帧内所有卡片都入队后一次性 flush */
  Promise.resolve().then(() => {
    hydrationFlushScheduled = false;
    const q = hydrationQueue;
    hydrationQueue = null;
    if (!q || q.size === 0) return;
    useAppStore.getState().hydrateArticleContent(Array.from(q));
  });
}


export const useAppStore = create<AppState>((set, get) => ({
  activeContentLayout: 'article',
  activeViewFilter: 'all',
  activeFeedFilter: 'all',
  timelineFilter: 'unread',
  timelineSort: 'newest',

  activeArticleId: null,
  isShowingTranslatedProse: false,
  isRawRenderMode: false,
  showFulltext: false,
  summaryGenerating: false,
  translating: false,
  summaryErrors: {},
  translateErrors: {},
  openedReadIds: {},

  /* 启动用空数据 + dataLoading 骨架（不用 mock 先行渲染——曾导致卸载重装后
     「测试订阅一闪而过」，同步后被真实空库替换；蓝图中 P1/I3 要求首帧即真实态） */
  categories: [],
  entries: [],
  feedIndex: new Map(),
  feedCounts: new Map(),

  player: { isActive: false, isPlaying: false, speed: 1.0, title: '', showName: '', cover: '', audioUrl: '', positionSec: 0, durationSec: 0, seekToSec: null },
  playerExpanded: false,

  settingsOpen: false,
  settingsTab: 'general',
  searchOpen: false,
  closeAskVisible: false,
  lightboxUrl: null,
  newCategoryModalOpen: false,
  addFeedModalOpen: false,
  addFeedTargetCatId: '',
  editFeedModalOpen: false,
  editFeedTargetId: '',
  renameCatModalOpen: false,
  renameCatTargetId: '',

  toasts: [],

  syncStatus: 'synced',
  backgroundSyncing: false,
  syncConnected: false,
  githubFlow: null,
  githubAccount: null,
  githubLoggingIn: false,

  /* mock 数据先行渲染；Tauri 环境启动时 bootstrapFromBackend 会整体替换 */
  dataMode: 'mock',
  dataLoading: true,
  articlesLimit: 0,
  articlesLoading: false,
  articlesExhausted: false,

  settings: {
    autoRefresh: true,
    refreshInterval: 30,
    fetchConcurrency: 4,
    markReadOnOpen: true,
    markReadOnScrollBottom: false,
    markReadOnScrollOut: false,
    autoStart: false,
    startupView: 'unread',
    hideReadOnStartup: true,
    themeMode: 'dark',
    palette: 'blue',
    fontFamily: '"Segoe UI Variable Text", "Segoe UI", "Microsoft YaHei UI", "Microsoft YaHei", system-ui, sans-serif',
    fontSize: 16,
    lineHeight: 180,
    maxWidth: 860,
    listWidth: 320,
    showReadTime: true,
    defaultOpenMode: 'rss',
    smartDedup: false,
    closeToTray: true,
    closePromptShown: false,
    notifyOnNewArticles: false,
    autoSync: true,
    syncMode: 'direct',
  },

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
    /* 不触发 reload：entries 已统一附带正文（with_content 全量），布局切换是
       纯本地过滤（selectVisibleEntries 按新布局 resolve feed 布局），内容立即
       可见、零延迟。之前在此触发 reload 会在切换瞬间显示旧布局的空 content
       条目（「加载正文」闪动）+ 异步等待，是「切换卡顿/不直接查看」的根因。 */
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
      void api.markAllRead(feedId, folderId);
    }
    markEntriesRead(ids);
    set({ openedReadIds: {} });
    get().showToast('已将当前筛选的所有内容标记为已读');
  },

  /* ================= 阅读器 ================= */

  selectArticle: (id) => {
    const { entries, settings, dataMode } = get();
    const art = entries.find((a) => a.id === id);
    if (!art) return;
    const shouldMarkRead = dataMode === 'tauri' && settings.markReadOnOpen && !art.isRead;
    if (shouldMarkRead) {
      /* 后端模式：已读落库（不重载快照，本地同步置位即可） */
      void api.setRead(Number(id), true);
      markEntriesRead(new Set([id]));
    }
    set((s) => ({
      /* 记录"本次会话中被打开过"：即使标已读，在未读筛选下也保留显示（原地变灰） */
      openedReadIds: { ...s.openedReadIds, [id]: true },
      activeArticleId: id,
      isShowingTranslatedProse: false,
      isRawRenderMode: false,
      showFulltext: false,
    }));
    /* 打开文章：触发「智能全文」判定（摘要型源才提取原文；列表卡片水合不触发） */
    get().ensureArticleContent(id, { extractFulltext: true });
  },

  /** 正文懒加载（幂等）：列表快照不含 HTML，选中/社交卡片挂载时拉详情水合。
      守卫用「条目仍在」（不依赖 activeArticleId：社交卡片挂载时也走这里）。
      实现：并入批量水合队列（微任务合批）——首屏几十张可见卡片同帧触发时，
      合并成一次 get_articles IPC + 一次 set，避免逐篇 IPC 洪峰与逐篇 O(n) 重渲染。
      注意：extractFulltext（打开文章的智能全文）即使 content 已水合也须执行——
      列表卡片批量水合只填 content，不触发智能全文判定；打开文章时若 content
      已在（列表水合过），仍要走 extractFulltext 分支。 */
  ensureArticleContent: (id, opts) => {
    const { dataMode, entries } = get();
    if (dataMode !== 'tauri') return;
    const art = entries.find((a) => a.id === id);
    if (!art) return;
    if (opts?.extractFulltext) {
      /* 打开文章：需要完整详情（含 url/fulltext_extracted）+ 智能全文判定。
         若已水合（列表卡片批量水合过），content 已有，但仍需取详情以判断
         是否要提取全文——故不因 art.content 短路。 */
      void api.getArticle(Number(id)).then((row) => {
        if (!row) return;
        const cur = get().entries.find((a) => a.id === id);
        if (!cur) return;
        const html = row.content_html ?? '';
        /* 若已有正文（列表水合过）且详情正文相同，跳过重复 set，仅补全可能
           缺失的 url/fulltext_extracted 等字段；否则正常水合 */
        if (!cur.content || cur.content !== html) {
          set((s) => ({
            entries: s.entries.map((a) =>
              a.id === id
                ? {
                    ...a,
                    content: html,
                    rawContent: html,
                    translatedContent: row.translated_content ?? '',
                    snippet: row.snippet || a.snippet,
                    aiSummary: row.ai_summary ?? a.aiSummary,
                    url: row.url ?? a.url,
                    fulltextExtracted: row.fulltext_extracted ?? false,
                  }
                : a,
            ),
          }));
        } else {
          /* content 相同：仍补全 url（列表水合可能缺 url） */
          set((s) => ({
            entries: s.entries.map((a) =>
              a.id === id && !a.url && row.url ? { ...a, url: row.url, fulltextExtracted: row.fulltext_extracted ?? false } : a,
            ),
          }));
        }
        const mode = get().settings.defaultOpenMode;
        const alreadyExtracted = row.fulltext_extracted ?? false;
        if (mode === 'fulltext' && row.url && !alreadyExtracted && shouldExtractFulltext(html)) {
          void api
            .extractFulltext(Number(id))
            .then((full) => {
              if (!full) return;
              set((s) => ({
                entries: s.entries.map((a) => (a.id === id ? { ...a, content: full, fulltextExtracted: true } : a)),
                showFulltext: true,
              }));
            })
            .catch((e: unknown) => {
              const msg = extractError(e);
              get().showToast(`全文提取失败：${msg}`, { label: '重试', run: () => get().extractCurrentArticle() });
            });
        }
      });
      return;
    }
    /* 列表卡片（社交/通知）水合：已水合则短路，否则并入批量队列 */
    if (art.content) return;
    enqueueHydration(id);
  },

  /** 批量水合正文：一批 id 一次 IPC 拉取、一次 set 更新（消除逐篇洪峰）。 */
  hydrateArticleContent: (ids) => {
    if (get().dataMode !== 'tauri') return;
    /* 过滤出「仍存在且未水合」的 id（幂等 + 去重） */
    const pending = ids.filter((id) => {
      const a = get().entries.find((e) => e.id === id);
      return a && !a.content;
    });
    if (pending.length === 0) return;
    void api.getArticles(pending.map(Number)).then((rows) => {
      if (!rows || rows.length === 0) return;
      /* 构建 id → 详情 映射，一次性合并进 entries（单次 map，单次 set） */
      const byId = new Map(rows.map((r) => [String(r.id), r]));
      set((s) => {
        let changed = false;
        const entries = s.entries.map((a) => {
          if (a.content) return a;
          const row = byId.get(a.id);
          if (!row) return a;
          changed = true;
          const html = row.content_html ?? '';
          return {
            ...a,
            content: html,
            rawContent: html,
            translatedContent: row.translated_content ?? '',
            snippet: row.snippet || a.snippet,
            aiSummary: row.ai_summary ?? a.aiSummary,
            url: row.url ?? a.url,
            fulltextExtracted: row.fulltext_extracted ?? false,
          };
        });
        if (changed) syncCurrentViewCache(entries);
        return changed ? { entries } : s;
      });
    });
  },

  clearReaderSelection: () =>
    set({ activeArticleId: null, isShowingTranslatedProse: false, isRawRenderMode: false, showFulltext: false }),

  /** 手动全文提取：Readability 拉原文网页存 content；rawContent 始终保留 RSS 原文
      （供「全文 ↔ RSS 正文」切换回跳）。提取成功后进入全文视图。 */
  extractCurrentArticle: () => {
    const { activeArticleId, entries, dataMode, showToast } = get();
    if (!activeArticleId || dataMode !== 'tauri') return;
    const art = entries.find((a) => a.id === activeArticleId);
    if (!art) return;
    if (!art.url) {
      showToast('该条目没有原文网页地址');
      return;
    }
    showToast(art.fulltextExtracted ? '正在刷新全文…' : '正在提取全文…');
    void api
      .extractFulltext(Number(activeArticleId))
      .then((full) => {
        if (!full) return;
        const id = activeArticleId;
        set((s) => ({
          entries: s.entries.map((a) =>
            a.id === id ? { ...a, content: full, fulltextExtracted: true } : a,
          ),
          showFulltext: true,
        }));
        showToast('全文提取完成');
      })
      .catch((e: unknown) => {
        const msg = extractError(e);
        showToast(`全文提取失败：${msg}`, { label: '重试', run: () => get().extractCurrentArticle() });
      });
  },

  /** 全文视图切换：已提取全文 → 在 RSS 原文与全文间切换（不重复请求）；
      未提取 → 触发提取（同 extractCurrentArticle）。 */
  toggleReaderFulltext: () => {
    const { activeArticleId, entries, showFulltext } = get();
    if (!activeArticleId) return;
    const art = entries.find((a) => a.id === activeArticleId);
    if (!art) return;
    if (art.fulltextExtracted) {
      // 已提取：直接切换视图
      set({ showFulltext: !showFulltext });
    } else {
      // 未提取：触发提取
      get().extractCurrentArticle();
    }
  },

  toggleCurrentReadStatus: () => {
    const { activeArticleId, entries, dataMode } = get();
    if (!activeArticleId) return;
    const art = entries.find((a) => a.id === activeArticleId);
    if (!art) return;
    if (dataMode === 'tauri') void api.setRead(Number(activeArticleId), !art.isRead);
    flipEntryFlag(activeArticleId, 'isRead');
    get().showToast(art.isRead ? '已标记为未读' : '已标记为已读');
  },

  toggleCurrentStar: () => {
    const { activeArticleId, entries, dataMode } = get();
    if (!activeArticleId) return;
    const art = entries.find((a) => a.id === activeArticleId);
    if (!art) return;
    if (dataMode === 'tauri') void api.setStarred(Number(activeArticleId), !art.isStarred);
    flipEntryFlag(activeArticleId, 'isStarred');
  },

  toggleReaderRenderMode: () => set((s) => ({ isRawRenderMode: !s.isRawRenderMode })),

  toggleReaderTranslation: (opts) => {
    const silent = opts?.silent ?? false;
    const s = get();
    /* 关闭 → 直接切回原文 */
    if (s.isShowingTranslatedProse) {
      set({ isShowingTranslatedProse: false });
      return;
    }
    const art = s.activeArticleId ? s.entries.find((a) => a.id === s.activeArticleId) : null;
    if (!art) return;
    /* 已有缓存译文 → 直接切换展示 */
    if (art.translatedContent) {
      set({ isShowingTranslatedProse: true });
      return;
    }
    /* 无缓存 → 流式生成（打字机效果落到 translatedContent） */
    if (s.dataMode !== 'tauri') {
      if (!silent) get().showToast('浏览器演示模式无 AI 服务');
      return;
    }
    const articleId = art.id;
    /* 重试语义：清上次的错误与半截译文，重新走完整流 */
    set((st) => ({
      translating: true,
      isShowingTranslatedProse: true,
      translateErrors: { ...st.translateErrors, [articleId]: '' },
      entries: st.entries.map((a) => (a.id === articleId ? { ...a, translatedContent: '' } : a)),
    }));
    void api
      .aiTranslate(
        Number(articleId),
        (delta) => {
          /* 增量追加：Reader 直接渲染 translatedContent（打字机） */
          set((st) => {
            const cur = st.entries.find((a) => a.id === articleId);
            if (!cur) return st;
            const next = (cur.translatedContent || '') + delta;
            return { entries: st.entries.map((a) => (a.id === articleId ? { ...a, translatedContent: next } : a)) };
          });
        },
        () => {
          /* 流式结束：回读 DB 的消毒版译文（后端 ai_translate 落库前已 sanitize，
             流中 delta 是未消毒原样，若直接保留会把 XSS 窗口留到渲染时）。
             回读成功后用消毒版覆盖流式产物；失败则回退到已展示的流式内容。 */
          set({ translating: false });
          void api.getArticle(Number(articleId)).then((row) => {
            if (!row) return;
            const cur = get().entries.find((a) => a.id === articleId);
            if (!cur) return;
            const safe = row.translated_content ?? '';
            if (!safe) return;
            set((st) => ({
              entries: st.entries.map((a) =>
                a.id === articleId ? { ...a, translatedContent: safe } : a,
              ),
            }));
          });
        },
        (msg) => {
          /* 内联错误（Reader 正文上方展示）+ 非 silent 时 toast 带重试 */
          set((st) => ({
            translating: false,
            isShowingTranslatedProse: false,
            translateErrors: { ...st.translateErrors, [articleId]: msg },
          }));
          if (!silent) {
            get().showToast(`翻译失败：${msg}`, { label: '重试', run: () => get().toggleReaderTranslation() });
          }
        },
      )
      .catch(() => {
        set((st) => ({
          translating: false,
          isShowingTranslatedProse: false,
          translateErrors: { ...st.translateErrors, [articleId]: 'AI 服务未配置或不可达' },
        }));
        if (!silent) {
          get().showToast('翻译失败：请先在设置中配置 AI 服务', { label: '重试', run: () => get().toggleReaderTranslation() });
        }
      });
  },

  markEntriesReadBulk: (ids) => {
    if (ids.length === 0) return;
    const { dataMode, entries } = get();
    /* 只处理未读项：已读的重复 setRead 无意义（还会刷 sync_queue） */
    const unread = ids.filter((id) => {
      const e = entries.find((x) => x.id === id);
      return e ? !e.isRead : false;
    });
    if (unread.length === 0) return;
    if (dataMode === 'tauri') {
      for (const id of unread) void api.setRead(Number(id), true);
    }
    const marked = new Set(unread);
    markEntriesRead(marked);
    set((s) => ({
      /* 标读的卡片原地变灰保留（未读筛选下不消失）——合并而非替换：
         批量标读（滚动/全部已读）不能抹掉之前"打开过"的保留记录，
         否则那些卡片会在未读筛选下突然消失（体验为"已读的直接隐藏"）；
         切视图/布局/筛选时的清理逻辑统一把它们移除 */
      openedReadIds: {
        ...s.openedReadIds,
        ...Object.fromEntries(unread.map((id) => [id, true])),
      },
    }));
  },

  /** 按 id 流式生成摘要（增量落到 aiSummary，卡片实时打字机）。有缓存直接短路。
      失败记录到 summaryErrors[id]（卡片内联展示 + 重试依据）；重试前先清错误与半截文本。 */
  summarizeEntry: (id, opts) => {
    const silent = opts?.silent ?? false;
    const s = get();
    const art = s.entries.find((a) => a.id === id);
    if (!art) return;
    /* 已有缓存 → 直接展示（ai_summarize 后端也会短路，这里前端提前判断） */
    if (art.aiSummary) {
      set({ summaryGenerating: false });
      return;
    }
    if (s.dataMode !== 'tauri') {
      if (!silent) get().showToast('浏览器演示模式无 AI 服务');
      return;
    }
    /* 重试语义：清掉上次的错误与半截摘要，重新走完整流 */
    set((st) => ({
      summaryGenerating: true,
      summaryErrors: { ...st.summaryErrors, [id]: '' },
      entries: st.entries.map((a) => (a.id === id ? { ...a, aiSummary: '' } : a)),
    }));
    void api
      .aiSummarize(
        Number(id),
        (delta) => {
          set((st) => {
            const cur = st.entries.find((a) => a.id === id);
            if (!cur) return st;
            const next = (cur.aiSummary || '') + delta;
            return { entries: st.entries.map((a) => (a.id === id ? { ...a, aiSummary: next } : a)) };
          });
        },
        () => set({ summaryGenerating: false }),
        (msg) => {
          /* 内联错误（卡片上直接可见）+ 非 silent 时 toast 带重试 */
          set((st) => ({ summaryGenerating: false, summaryErrors: { ...st.summaryErrors, [id]: msg } }));
          if (!silent) {
            get().showToast(`摘要失败：${msg}`, { label: '重试', run: () => get().summarizeEntry(id) });
          }
        },
      )
      .catch(() => {
        set((st) => ({ summaryGenerating: false, summaryErrors: { ...st.summaryErrors, [id]: 'AI 服务未配置或不可达' } }));
        if (!silent) {
          get().showToast('摘要失败：请先在设置中配置 AI 服务', { label: '重试', run: () => get().summarizeEntry(id) });
        }
      });
  },

  triggerReaderSummary: (opts) => {
    const id = get().activeArticleId;
    if (!id) return;
    get().summarizeEntry(id, opts);
  },

  /* ================= 卡片就地操作 ================= */

  toggleEntryFlag: (id, field) => {
    const { dataMode, entries } = get();
    if (dataMode === 'tauri') {
      const cur = entries.find((e) => e.id === id);
      if (cur) {
        if (field === 'isRead') void api.setRead(Number(id), !cur.isRead);
        else void api.setStarred(Number(id), !cur.isStarred);
      }
    }
    flipEntryFlag(id, field);
  },

  /* ================= 播客 ================= */

  /** 播放真实剧集：audioUrl 必传（enclosure_url），PlayerBar 挂 audio 元素执行。 */
  playPodcastEpisode: (title, showName, cover, audioUrl, entryId) => {
    if (!audioUrl) {
      get().showToast('该剧集没有可播放的音频地址');
      return;
    }
    set({
      player: {
        ...get().player,
        isActive: true,
        isPlaying: true,
        title,
        showName,
        cover: cover || get().player.cover,
        audioUrl,
        positionSec: 0,
        durationSec: 0,
        seekToSec: null,
      },
    });
    get().showToast(`正在播放: ${title}`);
    /* 点播放即视为已读（与打开文章同语义） */
    if (entryId) get().markEntriesReadBulk([entryId]);
  },

  togglePlayerPlay: () =>
    set((s) => ({ player: { ...s.player, isPlaying: !s.player.isPlaying } })),

  /** audio 元素状态回写（timeupdate/loadedmetadata/durationchange 调） */
  syncPlayerProgress: (positionSec, durationSec) =>
    set((s) => ({ player: { ...s.player, positionSec, durationSec } })),

  /** 播放自然结束（ended 事件） */
  playerEnded: () =>
    set((s) => ({ player: { ...s.player, isPlaying: false, positionSec: 0, seekToSec: null } })),

  seekPlayer: (sec) =>
    set((s) => ({ player: { ...s.player, seekToSec: Math.max(0, sec), positionSec: Math.max(0, sec) } })),

  skipPlayer: (deltaSec) => {
    const p = get().player;
    set({ player: { ...p, seekToSec: Math.max(0, p.positionSec + deltaSec), positionSec: Math.max(0, p.positionSec + deltaSec) } });
  },

  cyclePlaybackSpeed: () => {
    const speeds = [1.0, 1.25, 1.5, 2.0];
    const next = speeds[(speeds.indexOf(get().player.speed) + 1) % speeds.length];
    set((s) => ({ player: { ...s.player, speed: next } }));
    get().showToast(`倍速已切换至 ${next}x`);
  },

  closePodcastBar: () =>
    set((s) => ({ player: { ...s.player, isActive: false, isPlaying: false, seekToSec: null }, playerExpanded: false })),

  togglePlayerExpanded: () => set((s) => ({ playerExpanded: !s.playerExpanded })),

  /* ================= 弹层 ================= */

  openSettings: () => set({ settingsOpen: true }),
  closeSettings: () => set({ settingsOpen: false }),
  switchSettingsTab: (tab) => set({ settingsTab: tab }),
  openSettingsTab: (tab) => set({ settingsOpen: true, settingsTab: tab }),
  openSearch: () => set({ searchOpen: true }),
  closeSearch: () => set({ searchOpen: false }),
  answerCloseAsk: (action, remember) => {
    set({ closeAskVisible: false });
    /* remember 时同步设置镜像（真值由后端 resolve_close 落库，
       这里保证当前会话的设置页开关即时一致） */
    if (remember) {
      set((s) => ({ settings: { ...s.settings, closeToTray: action === 'tray', closePromptShown: true } }));
    }
    /* invoke 失败（DB 锁超时/IPC 错）时本地重试一次，仍失败 toast 提示
       （窗口可能没关——用户再点 ✕ 会走完整流程） */
    api.resolveClose(action, remember)
      .catch(() =>
        api.resolveClose(action, remember).catch(() =>
          get().showToast('关闭操作未生效，请再点一次关闭按钮'),
        ),
      );
  },
  openLightbox: (url) => set({ lightboxUrl: url }),
  closeLightbox: () => set({ lightboxUrl: null }),
  openNewCategoryModal: () => set({ newCategoryModalOpen: true }),
  openAddFeedModal: (catId) => set({ addFeedModalOpen: true, addFeedTargetCatId: catId }),
  openEditFeedModal: (feedId) => set({ editFeedModalOpen: true, editFeedTargetId: feedId }),
  openRenameCatModal: (catId) => set({ renameCatModalOpen: true, renameCatTargetId: catId }),
  closeMiniModal: (which) =>
    set(
      which === 'newCategory' ? { newCategoryModalOpen: false }
      : which === 'addFeed' ? { addFeedModalOpen: false }
      : which === 'editFeed' ? { editFeedModalOpen: false }
      : { renameCatModalOpen: false },
    ),

  showToast: (text, action) => {
    const id = ++toastId;
    set((s) => ({
      /* 上限 4 条：错误循环（如后台刷新连续失败）不再无限堆叠；保留最新 */
      toasts: [...s.toasts, { id, text, action }].slice(-4),
    }));
    /* 两段式生命周期：2200ms 后先标 leaving（CSS 退场过渡），200ms 过渡完成再卸载；
       带操作按钮时延长停留（留出点重试的时间） */
    const stay = action ? 4200 : 2200;
    setTimeout(() => {
      set((s) => ({ toasts: s.toasts.map((t) => (t.id === id ? { ...t, leaving: true } : t)) }));
      setTimeout(() => {
        set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) }));
      }, 200);
    }, stay);
  },

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
      // 竞态保护：加载期间发生了 reload（游标被重置），丢弃本次追加
      if (get().articlesLimit !== offset) return;
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
    } catch {
      set({ articlesLoading: false });
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
    set({ entries: next, articlesLimit: rows.length, articlesExhausted: true, articlesLoading: false });
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
    });
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
      /* 后端异常时回退 mock，保证界面可用（Tauri 极少触发） */
      console.error('bootstrap from backend failed:', e);
      const cats = createInitialCategories();
      set({
        categories: cats,
        entries: createInitialEntries(),
        feedIndex: buildFeedIndex(cats),
        dataMode: 'mock',
        dataLoading: false,
      });
    }
  },

  triggerManualSync: () => {
    if (get().dataMode === 'tauri') {
      set({ syncStatus: 'syncing' });
      /* 分步同步：feeds（订阅层）→ states（状态对账，只写状态不重拉列表）→
         refreshAllFeeds（内容抓取）。状态先落库、内容再抓取，最后只 reload 一次
         带出「最新内容 + 最新状态」，避免多次 reload 造成的列表闪动
         （「获取内容后再次同步状态导致闪动」的解耦）。 */
      void api
        .syncPhase('feeds')
        .catch(() => null) // 未连接（notConnected）→ 走纯直连刷新
        .then(async (feedsReport) => {
          if (feedsReport) {
            get().showToast('订阅同步完成，正在同步文章状态…');
            return api.syncPhase('states', true);
          }
          return null;
        })
        .then(async () => {
          /* states 阶段已把最新状态落库（is_read/is_starred），不在此 reload——
             等下方内容抓取完成后一次性 reload，避免「状态同步→闪动→内容→再闪动」 */
          return api.refreshAllFeeds();
        })
        .then((summary: RefreshSummary | null) => get().reloadFromBackend().then(() => summary))
        .then((summary: RefreshSummary | null) => {
          set({ syncStatus: 'synced' });
          if (summary && summary.failed_feeds > 0) {
            get().showToast(`刷新完成：新增 ${summary.new_articles} 条，${summary.failed_feeds} 个源直连失败`);
          } else if (summary) {
            get().showToast(`刷新完成：新增 ${summary.new_articles} 条`);
          } else {
            get().showToast('刷新完成');
          }
        })
        .catch((e: unknown) => {
          const msg = extractError(e);
          set({ syncStatus: 'error' });
          get().showToast(`刷新失败：${msg}`, { label: '重试', run: () => get().triggerManualSync() });
        });
      return;
    }
    set({ syncStatus: 'syncing' });
    get().showToast('正在后台增量同步...');
    setTimeout(() => {
      set({ syncStatus: 'synced' });
      get().showToast('后端同步完成');
    }, 1100);
  },

  /* ================= GitHub 设备流登录（store 级常驻轮询） ================= *
     GitHub 登录的发起/轮询放在 store 而非设置页组件：设备流等待授权
     可能跨数十秒，期间用户可能切走/关闭设置弹窗——组件卸载即清定时器，
     网页授权完成后软件再无反应（历史复现 bug）。模块级定时器不受组件
     生命周期影响，登录状态常驻 AppState。 */

  githubLoginStart: async () => {
    if (get().githubLoggingIn) return;
    set({ githubLoggingIn: true });
    try {
      let start: Awaited<ReturnType<typeof api.githubLoginStart>>;
      try {
        start = await api.githubLoginStart();
      } catch (first) {
        /* WebDAV 冲突：确认后带 force 重发（后端错误码 webdavConflict） */
        const msg = first instanceof Error ? first.message : String(first);
        if (msg.includes('WebDAV')) {
          if (!window.confirm(`${msg}

确定切换为 GitHub Gist 同步吗？`)) return;
          start = await api.githubLoginStart(true);
        } else {
          throw first;
        }
      }
      set({ githubFlow: start });
      await openExternal(start.verification_uri);
      get().showToast('浏览器已打开授权页，代码已常驻显示在本页');
      scheduleGithubPoll(start.interval);
    } catch (e) {
      get().showToast(`发起登录失败：${extractError(e)}`);
    } finally {
      set({ githubLoggingIn: false });
    }
  },

  githubLoginDisconnect: async () => {
    set({ githubLoggingIn: true });
    try {
      await api.githubLoginDisconnect();
      clearGithubPoll();
      set({ githubAccount: null, githubFlow: null });
      set({ syncStatus: 'synced' });
      get().showToast('已断开 GitHub 登录');
    } catch (e) {
      get().showToast(`断开失败：${extractError(e)}`);
    } finally {
      set({ githubLoggingIn: false });
    }
  },

  /* ================= 订阅管理 ================= */

  createCategory: (name, layout) => {
    if (get().dataMode === 'tauri') {
      void api
        .createFolder(name, layout)
        .then(() => get().reloadFromBackend())
        .catch(() => get().showToast('创建分类失败'));
      return;
    }
    set((s) => {
      const nextCategories = [
        ...s.categories,
        {
          id: 'cat-' + Date.now(),
          name,
          collapsed: true,
          settingsCollapsed: false,
          layout,
          autoSummary: false,
          autoTranslate: false,
          feeds: [],
        },
      ];
      /* 统一走收口：新分类无订阅源，条目集合不变但解析表重建 */
      return reconcileCategories(s, nextCategories);
    });
    get().showToast(`已创建分类：${name}`);
  },

  deleteCategory: (catId) => {
    if (get().dataMode === 'tauri') {
      const id = Number(catId.replace('cat-', ''));
      void api
        .deleteFolder(id)
        .then(() => get().reloadFromBackend())
        .catch(() => get().showToast('删除分类失败'));
      return;
    }
    set((s) => {
      const nextCategories = s.categories.filter((c) => c.id !== catId);
      return {
        ...reconcileCategories(s, nextCategories),
        activeFeedFilter: s.activeFeedFilter === catId ? 'all' : s.activeFeedFilter,
      };
    });
    get().showToast('分类已删除');
  },

  renameCategory: (catId, name) => {
    const trimmed = name.trim();
    if (!trimmed) {
      get().showToast('分类名称不能为空');
      return;
    }
    if (get().dataMode === 'tauri') {
      void api
        .renameFolder(Number(catId.replace('cat-', '')), trimmed)
        .then(() => get().reloadFromBackend())
        .then(() => get().showToast(`分类已改名：${trimmed}`))
        .catch((e: unknown) => {
          const msg = extractError(e);
          get().showToast(`改名失败：${msg}`);
        });
      return;
    }
    set((s) => reconcileCategories(s, s.categories.map((c) => (c.id === catId ? { ...c, name: trimmed } : c))));
    get().showToast(`分类已改名：${trimmed}`);
  },

  addFeed: (catId, url, title, layout, autoSummary, autoTranslate, syncToBackend = true) => {
    if (get().dataMode === 'tauri') {
      const folderId = Number(catId.replace('cat-', ''));
      set({ syncStatus: 'syncing' });
      void api
        .addFeed(url, title || null, folderId, layout, autoSummary, autoTranslate, syncToBackend)
        .then(() => get().reloadFromBackend())
        .then(async () => {
          /* 勾选「同步到后端」→ 添加后立即跑 feeds 阶段推送新订阅到远端
             （add_feed 只入队，这里触发推送让勾选语义即时生效） */
          if (syncToBackend && get().syncConnected) {
            await api.syncLocalFeeds().catch(() => null);
          }
          set({ syncStatus: 'synced' });
          get().showToast(`已添加订阅源：${title || url}`);
        })
        .catch((e: unknown) => {
          set({ syncStatus: 'error' });
          const msg = extractError(e);
          get().showToast(`添加失败：${msg}`, {
            label: '重试',
            run: () => get().addFeed(catId, url, title, layout, autoSummary, autoTranslate, syncToBackend),
          });
        });
      return;
    }
    set((s) => {
      const nextCategories = s.categories.map((c) =>
        c.id === catId
          ? {
              ...c,
              feeds: [
                ...c.feeds,
                {
                  id: 'feed-' + Date.now(),
                  name: title || url,
                  url,
                  favicon: '',
                  layout: layout as 'inherit',
                  autoSummary,
                  autoTranslate,
                },
              ],
            }
          : c,
      );
      return reconcileCategories(s, nextCategories);
    });
    get().showToast(`已添加订阅源：${title || url}`);
  },

  deleteFeed: (catId, feedId) => {
    if (get().dataMode === 'tauri') {
      void api
        .deleteFeed(Number(feedId))
        .then(() => get().reloadFromBackend())
        .catch(() => get().showToast('删除订阅源失败'));
      return;
    }
    set((s) => {
      const nextCategories = s.categories.map((c) =>
        c.id === catId ? { ...c, feeds: c.feeds.filter((f) => f.id !== feedId) } : c,
      );
      return {
        ...reconcileCategories(s, nextCategories),
        activeFeedFilter: s.activeFeedFilter === feedId ? 'all' : s.activeFeedFilter,
      };
    });
    get().showToast('订阅源已删除');
  },

  editFeed: (feedId, next) => {
    if (get().dataMode === 'tauri') {
      /* 只提交变化字段：标题为空 = 不改名；分类/布局/AI 开关与当前一致则省略 */
      const binding = get().feedIndex.get(feedId);
      const cur = binding?.feed;
      if (!cur) return;
      const targetFolderId = Number(next.catId.replace('cat-', ''));
      const args: Parameters<typeof api.updateFeed>[0] = { id: Number(feedId) };
      if (next.title.trim() && next.title.trim() !== cur.name) args.title = next.title.trim();
      if (binding && binding.cat.id !== next.catId) args.folderId = targetFolderId;
      if (next.layout !== cur.layout) args.layout = next.layout;
      if (next.autoSummary !== cur.autoSummary) args.autoSummary = next.autoSummary;
      if (next.autoTranslate !== cur.autoTranslate) args.autoTranslate = next.autoTranslate;
      if (args.title === undefined && args.folderId === undefined && args.layout === undefined
        && args.autoSummary === undefined && args.autoTranslate === undefined) {
        get().showToast('没有需要保存的更改');
        return;
      }
      void api
        .updateFeed(args)
        .then(() => get().reloadFromBackend())
        .then(() => get().showToast('订阅源已更新'))
        .catch((e: unknown) => {
          const msg = extractError(e);
          get().showToast(`保存失败：${msg}`);
        });
      return;
    }
    /* mock 模式：改内存树（改名/改属性 + 跨分类移动一次完成） */
    set((s) => {
      const moving = s.categories.flatMap((c) => c.feeds).find((f) => f.id === feedId);
      if (!moving) return s;
      const updated: FeedItem = {
        ...moving,
        name: next.title.trim() || moving.name,
        layout: next.layout as FeedItem['layout'],
        autoSummary: next.autoSummary,
        autoTranslate: next.autoTranslate,
      };
      /* 先全部摘除，再放进目标分类 */
      const stripped = s.categories.map((c) => ({ ...c, feeds: c.feeds.filter((f) => f.id !== feedId) }));
      const finalCategories = stripped.map((c) => (c.id === next.catId ? { ...c, feeds: [...c.feeds, updated] } : c));
      return reconcileCategories(s, finalCategories);
    });
    get().showToast('订阅源已更新');
  },

  refreshOneFeed: (feedId) => {
    if (get().dataMode !== 'tauri') {
      get().showToast('浏览器演示模式无直连能力');
      return;
    }
    get().showToast('正在刷新该订阅源...');
    void api
      .refreshFeed(Number(feedId))
      .then((n) => {
        if (n === null) return;
        get().showToast(n > 0 ? `刷新完成：新增 ${n} 条` : '刷新完成：没有新文章');
        return get().reloadFromBackend();
      })
      .catch((e: unknown) => {
        const msg = extractError(e);
        get().showToast(`刷新失败：${msg}`, { label: '重试', run: () => get().refreshOneFeed(feedId) });
      });
  },

  /* 布局/AI 开关统一走乐观更新：先改 store（设置页与主界面同帧生效，
     不再依赖 reloadFromBackend 全量重拉——异步竞态会让设置页显示回旧值），
     落库 fire-and-forget，失败 toast 提醒（本地状态不回滚，下次同步对齐）。 */
  updateCatLayout: (catId, layout) => {
    set((s) => reconcileCategories(s, s.categories.map((c) => (c.id === catId ? { ...c, layout } : c))));
    get().showToast('已更新分类布局并即时生效');
    void api
      .updateFolderLayout(Number(catId.replace('cat-', '')), layout)
      .catch(() => get().showToast('布局保存失败（界面已生效，重启后可能回退）'));
  },

  updateFeedLayout: (catId, feedId, layout) => {
    /* layout 参数来自 LAYOUT_OPTIONS（'inherit' | 五布局之一），原样存进
       feed——此前误写 as 'inherit' 把任何选择强转成继承，导致独立布局
       设了不生效、reload 后回落到分类布局（历史 bug 模式 C 契约混淆） */
    const next = layout as FeedItem['layout'];
    set((s) =>
      reconcileCategories(
        s,
        s.categories.map((c) =>
          c.id === catId
            ? { ...c, feeds: c.feeds.map((f) => (f.id === feedId ? { ...f, layout: next } : f)) }
            : c,
        ),
      ),
    );
    get().showToast('已更新订阅源布局并即时生效');
    void api
      .updateFeedLayout(numericId(feedId), next)
      .catch(() => get().showToast('布局保存失败（界面已生效，重启后可能回退）'));
  },

  toggleCatSummary: (catId, val) => {
    set((s) => ({ categories: s.categories.map((c) => (c.id === catId ? { ...c, autoSummary: val } : c)) }));
    const cat = get().categories.find((c) => c.id === catId);
    if (cat) void api.setFolderAiFlags(numericId(catId), val, cat.autoTranslate);
  },

  toggleCatTranslate: (catId, val) => {
    set((s) => ({ categories: s.categories.map((c) => (c.id === catId ? { ...c, autoTranslate: val } : c)) }));
    const cat = get().categories.find((c) => c.id === catId);
    if (cat) void api.setFolderAiFlags(numericId(catId), cat.autoSummary, val);
  },

  toggleFeedSummary: (catId, feedId, val) => {
    set((s) => ({
      categories: s.categories.map((c) =>
        c.id === catId ? { ...c, feeds: c.feeds.map((f) => (f.id === feedId ? { ...f, autoSummary: val } : f)) } : c,
      ),
    }));
    void api.setFeedAiFlags(numericId(feedId), val, get().categories.find((c) => c.id === catId)?.feeds.find((f) => f.id === feedId)?.autoTranslate ?? false);
  },

  toggleFeedTranslate: (catId, feedId, val) => {
    set((s) => ({
      categories: s.categories.map((c) =>
        c.id === catId ? { ...c, feeds: c.feeds.map((f) => (f.id === feedId ? { ...f, autoTranslate: val } : f)) } : c,
      ),
    }));
    void api.setFeedAiFlags(numericId(feedId), get().categories.find((c) => c.id === catId)?.feeds.find((f) => f.id === feedId)?.autoSummary ?? false, val);
  },

  toggleFolderCollapse: (catId) => {
    set((s) => ({
      categories: s.categories.map((c) => (c.id === catId ? { ...c, collapsed: !c.collapsed } : c)),
    }));
    /* 落库：分类折叠状态 */
    const cat = get().categories.find((c) => c.id === catId);
    if (cat) void api.setFolderCollapsed(numericId(catId), cat.collapsed);
  },

  toggleAllFolders: () => {
    const anyOpen = get().categories.some((c) => !c.collapsed);
    set((s) => ({ categories: s.categories.map((c) => ({ ...c, collapsed: anyOpen })) }));
    get().showToast(anyOpen ? '已收起全部分类' : '已展开全部分类');
    /* 批量落库折叠状态 */
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
      for (const c of get().categories) void api.setFolderCollapsed(numericId(c.id), anyOpen);
    }
  },

  toggleSettingsCatCollapse: (catId) =>
    set((s) => ({
      categories: s.categories.map((c) =>
        c.id === catId ? { ...c, settingsCollapsed: !c.settingsCollapsed } : c,
      ),
    })),

  updateSettings: (partial) => {
    set((s) => ({ settings: { ...s.settings, ...partial } }));
    /* 持久化到后端（单键 JSON；浏览器 mock 环境无 IPC 跳过） */
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
      void api.setSetting('app_settings', JSON.stringify(get().settings));
    }
  },

  /** 启动时从后端恢复设置；并应用 startupView / hideReadOnStartup */
  bootstrapSettings: async () => {
    if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return;
    try {
      const raw = await api.getSetting('app_settings');
      if (!raw) return;
      const saved = JSON.parse(raw) as Partial<SettingsState>;
      /* 逐键合并（未来新增设置项自动落默认值）；类型不符的丢弃 */
      const merged: SettingsState = { ...get().settings };
      const target = merged as unknown as Record<string, unknown>;
      for (const [k, v] of Object.entries(saved)) {
        if (k in merged && typeof v === typeof target[k]) {
          target[k] = v;
        }
      }
      /* maxWidth 旧默认迁移：v0.10.x 默认 760，现已提升为 860。若用户从未
         主动改过（值恰等于旧默认 760），升级到新默认；主动设过的值保留。 */
      if (merged.maxWidth === 760) {
        merged.maxWidth = 860;
      }
      /* startupView：启动默认视图（未读/全部/今天/收藏） */
      const view = saved.startupView;
      const validView = view === 'all' || view === 'today' || view === 'unread' || view === 'starred';
      set((s) => ({
        settings: merged,
        activeViewFilter: validView ? (view as ViewFilterType) : s.activeViewFilter,
        /* hideReadOnStartup：启动时时间流默认筛选（unread=隐藏已读 / all=显示全部） */
        timelineFilter: saved.hideReadOnStartup === false ? 'all' : 'unread',
      }));
    } catch (e) {
      console.error('restore settings failed:', e);
    }
  },
}));

/** 乐观更新某篇条目的 isRead/isStarred，并同步 feedCounts 的未读/收藏计数。
    侧边栏数字基于 feedCounts（后端精确计数），若不联动，标读/收藏后角标
    不立即变化（与乐观更新的列表脱节）。total/today 不受影响。 */
function flipEntryFlag(id: string, field: 'isRead' | 'isStarred') {
  const s = useAppStore.getState();
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
  useAppStore.setState({ entries, feedCounts });
  /* 同步当前视图缓存：避免切走再切回时，缓存恢复旧状态（标读/收藏回退闪烁） */
  syncCurrentViewCache(entries);
}

/** 把当前 entries 同步进「当前布局 × 当前视图」的缓存。
    乐观更新（标读/收藏/水合）只改 store.entries，缓存若不联动，切走视图再
    切回会用旧快照覆盖新状态（正文丢失、标读回退）。 */
function syncCurrentViewCache(entries: ArticleEntry[]) {
  const s = useAppStore.getState();
  viewEntriesCache.set(viewCacheKey(s.activeContentLayout, s.activeViewFilter), entries);
}

/** 批量标已读：同步 feedCounts 的未读计数（每篇 -1）。
    高效实现：一次遍历 entries 构建新数组，一次聚合 feedId 的未读减少数，
    避免循环内多次 Map 复制 / entries.map（「全部已读」几百篇时 O(n²) 卡顿）。 */
function markEntriesRead(ids: Set<string>) {
  const s = useAppStore.getState();
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
  useAppStore.setState({ entries, feedCounts });
  syncCurrentViewCache(entries);
}

export * from './store/selectors';

export type { AppState, PodcastPlayerState, SettingsState, ToastMessage } from './store/types';
