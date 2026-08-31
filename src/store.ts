import { create } from 'zustand';
import type {
  ArticleEntry,
  CategoryGroup,
  ContentLayoutType,
  FeedItem,
  PaletteTheme,
  ThemeMode,
  ViewFilterType,
} from './types';
import { createInitialCategories, createInitialEntries } from './mockData';
import { isSameLocalDay } from './lib/format';
import {
  api,
  folderRowsToCategories,
  articleRowToEntry,
  type RefreshSummary,
} from './lib/api';

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

export interface PodcastPlayerState {
  isActive: boolean;
  isPlaying: boolean;
  speed: number;
  title: string;
  showName: string;
  cover: string;
  /** 真实音频地址（enclosure_url）；空 = 无可播放源 */
  audioUrl: string;
  /** 当前播放位置（秒，PlayerBar 从 audio 元素同步） */
  positionSec: number;
  /** 总时长（秒；0 = 未加载） */
  durationSec: number;
  /** seek 请求（非 null 时 PlayerBar 执行 audio.currentTime 赋值后清回 null） */
  seekToSec: number | null;
}

export interface SettingsState {
  /* 通用 */
  autoRefresh: boolean;
  refreshInterval: number;
  markReadOnOpen: boolean;
  markReadOnScrollBottom: boolean;
  markReadOnScrollOut: boolean;
  autoStart: boolean;
  startupView: string;
  hideReadOnStartup: boolean;
  /* 外观 */
  themeMode: ThemeMode;
  palette: PaletteTheme;
  /* 阅读 */
  fontFamily: string;
  fontSize: number;
  lineHeight: number;
  maxWidth: number;
  showReadTime: boolean;
  defaultOpenMode: 'rss' | 'fulltext';
  /** 智能去重：同 URL 文章跨源只保留首个（入库层拦截） */
  smartDedup: boolean;
  /** 关闭按钮 → 最小化到托盘（默认开；托盘「退出」才是真退出） */
  closeToTray: boolean;
  /** 新文章到达发 Windows 系统通知（默认关；窗口隐藏时才发） */
  notifyOnNewArticles: boolean;
}

/** leaving=true 时先走 CSS 退场过渡，200ms 后再卸载 DOM */
export type ToastMessage = { id: number; text: string; leaving?: boolean };

export interface AppState {
  /* ---------- 导航与筛选 ---------- */
  activeContentLayout: ContentLayoutType;
  activeViewFilter: ViewFilterType;
  activeFeedFilter: string;            // 'all' | 'cat-xxx' | 'f-xxx'
  timelineFilter: 'all' | 'unread';
  timelineSort: 'newest' | 'oldest';

  /* ---------- 阅读器 ---------- */
  activeArticleId: string | null;
  isShowingTranslatedProse: boolean;
  isRawRenderMode: boolean;
  summaryGenerating: boolean;
  /** AI 翻译流式生成中 */
  translating: boolean;
  /* 本次列表会话中被打开过的文章 id → 未读筛选下原地保留变灰（用户决策） */
  openedReadIds: Record<string, boolean>;

  /* ---------- 数据（Tauri 环境 = SQLite 快照；浏览器环境 = 演示数据） ---------- */
  categories: CategoryGroup[];
  /** 全部条目的单一扁平集合。条目的内容布局由
      「订阅源布局绑定 → 分类布局」在查询时动态解析（selectRawEntries），
      修改绑定后条目即时跟随，无需数据迁移。 */
  entries: ArticleEntry[];

  /** feedId → feed 引用的解析表（派生 selector 的公共底座） */
  feedIndex: Map<string, { feed: FeedItem; cat: CategoryGroup }>;

  /* ---------- 播客播放器 ---------- */
  player: PodcastPlayerState;

  /* ---------- 弹层 ---------- */
  settingsOpen: boolean;
  settingsTab: string;
  searchOpen: boolean;
  lightboxUrl: string | null;
  newCategoryModalOpen: boolean;
  addFeedModalOpen: boolean;
  addFeedTargetCatId: string;
  /** 编辑源对话框：目标源 id（空串 = 关闭） */
  editFeedModalOpen: boolean;
  editFeedTargetId: string;
  /** 分类改名对话框：目标分类 id（空串 = 关闭） */
  renameCatModalOpen: boolean;
  renameCatTargetId: string;

  /* ---------- Toast 与同步 ---------- */
  toasts: ToastMessage[];
  syncStatus: 'synced' | 'syncing' | 'error';
  /** 后端真实的 Miniflux 连接态（bootstrap/sync 后刷新），未连接时侧栏不显示"已同步" */
  minifluxConnected: boolean;

  /* ---------- 数据源模式 ---------- */
  /** tauri = 真实 SQLite 后端；mock = 浏览器开发回退 */
  dataMode: 'tauri' | 'mock';
  /** 后端数据加载中（首屏骨架） */
  dataLoading: boolean;
  bootstrapFromBackend: () => Promise<void>;
  reloadFromBackend: () => Promise<void>;

  /* ---------- 设置 ---------- */
  settings: SettingsState;
  updateSettings: (partial: Partial<SettingsState>) => void;
  /** 启动时从后端恢复设置（持久化） */
  bootstrapSettings: () => Promise<void>;

  /* ---------- Actions: 导航 ---------- */
  selectLayout: (layout: ContentLayoutType) => void;
  selectView: (view: ViewFilterType) => void;
  selectFeed: (feedId: string) => void;
  toggleTimelineFilter: () => void;
  toggleTimelineSort: () => void;
  markCurrentViewAllRead: () => void;

  /* ---------- Actions: 阅读器 ---------- */
  selectArticle: (id: string) => void;
  clearReaderSelection: () => void;
  toggleCurrentReadStatus: () => void;
  toggleCurrentStar: () => void;
  toggleReaderRenderMode: () => void;
  /** opts.silent：源级开关自动触发时静默失败（未配置 AI 不弹 toast） */
  toggleReaderTranslation: (opts?: { silent?: boolean }) => void;
  triggerReaderSummary: (opts?: { silent?: boolean }) => void;
  /** 列表卡片就地摘要（通知布局）：按文章 id 流式生成，不依赖阅读器选中态 */
  summarizeEntry: (id: string, opts?: { silent?: boolean }) => void;
  /** 滚动触发的批量已读（滚出列表/正文到底）：静默、只标未读项 */
  markEntriesReadBulk: (ids: string[]) => void;
  /** 正文懒加载水合（选中文章 / 社交卡片挂载） */
  ensureArticleContent: (id: string) => void;
  /** 手动全文提取（工具栏按钮；已提取时为刷新全文） */
  extractCurrentArticle: () => void;

  /* ---------- Actions: 卡片就地操作 ---------- */
  toggleEntryFlag: (id: string, field: 'isRead' | 'isStarred') => void;

  /* ---------- Actions: 播客 ---------- */
  playPodcastEpisode: (title: string, showName: string, cover: string, audioUrl: string, entryId?: string) => void;
  togglePlayerPlay: () => void;
  cyclePlaybackSpeed: () => void;
  closePodcastBar: () => void;
  /** audio 元素进度回写 */
  syncPlayerProgress: (positionSec: number, durationSec: number) => void;
  playerEnded: () => void;
  seekPlayer: (sec: number) => void;
  /** 相对快进/快退（秒） */
  skipPlayer: (deltaSec: number) => void;

  /* ---------- Actions: 弹层 ---------- */
  openSettings: () => void;
  closeSettings: () => void;
  switchSettingsTab: (tab: string) => void;
  openSettingsTab: (tab: string) => void;
  openSearch: () => void;
  closeSearch: () => void;
  openLightbox: (url: string) => void;
  closeLightbox: () => void;
  openNewCategoryModal: () => void;
  openAddFeedModal: (catId: string) => void;
  openEditFeedModal: (feedId: string) => void;
  openRenameCatModal: (catId: string) => void;
  closeMiniModal: (which: 'newCategory' | 'addFeed' | 'editFeed' | 'renameCat') => void;

  /* ---------- Actions: Toast / 同步 ---------- */
  showToast: (text: string) => void;
  triggerManualSync: () => void;

  /* ---------- Actions: 订阅管理 ---------- */
  createCategory: (name: string, layout: ContentLayoutType) => void;
  deleteCategory: (catId: string) => void;
  /** 分类改名（连接 Miniflux 时同步远端） */
  renameCategory: (catId: string, name: string) => void;
  addFeed: (catId: string, url: string, title: string, layout: string, autoSummary: boolean, autoTranslate: boolean) => void;
  deleteFeed: (catId: string, feedId: string) => void;
  /** 编辑源：改名/移动分类/布局/AI 开关一次性提交 */
  editFeed: (feedId: string, next: { title: string; catId: string; layout: string; autoSummary: boolean; autoTranslate: boolean }) => void;
  /** 单源手动刷新（直连），toast 反馈新增条数 */
  refreshOneFeed: (feedId: string) => void;
  updateCatLayout: (catId: string, layout: ContentLayoutType) => void;
  updateFeedLayout: (catId: string, feedId: string, layout: string) => void;
  toggleCatSummary: (catId: string, val: boolean) => void;
  toggleCatTranslate: (catId: string, val: boolean) => void;
  toggleFeedSummary: (catId: string, feedId: string, val: boolean) => void;
  toggleFeedTranslate: (catId: string, feedId: string, val: boolean) => void;
  toggleFolderCollapse: (catId: string) => void;
  toggleAllFolders: () => void;
  toggleSettingsCatCollapse: (catId: string) => void;
}

let toastId = 0;

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

export const useAppStore = create<AppState>((set, get) => ({
  activeContentLayout: 'article',
  activeViewFilter: 'all',
  activeFeedFilter: 'all',
  timelineFilter: 'unread',
  timelineSort: 'newest',

  activeArticleId: null,
  isShowingTranslatedProse: false,
  isRawRenderMode: false,
  summaryGenerating: false,
  translating: false,
  openedReadIds: {},

  categories: createInitialCategories(),
  entries: createInitialEntries(),
  feedIndex: buildFeedIndex(createInitialCategories()),

  player: { isActive: false, isPlaying: false, speed: 1.0, title: '', showName: '', cover: '', audioUrl: '', positionSec: 0, durationSec: 0, seekToSec: null },

  settingsOpen: false,
  settingsTab: 'general',
  searchOpen: false,
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
  minifluxConnected: false,

  /* mock 数据先行渲染；Tauri 环境启动时 bootstrapFromBackend 会整体替换 */
  dataMode: 'mock',
  dataLoading: true,

  settings: {
    autoRefresh: true,
    refreshInterval: 30,
    markReadOnOpen: true,
    markReadOnScrollBottom: false,
    markReadOnScrollOut: false,
    autoStart: false,
    startupView: 'unread',
    hideReadOnStartup: true,
    themeMode: 'dark',
    palette: 'blue',
    fontFamily: "'Plus Jakarta Sans', -apple-system, 'Segoe UI', sans-serif",
    fontSize: 16,
    lineHeight: 180,
    maxWidth: 760,
    showReadTime: true,
    defaultOpenMode: 'rss',
    smartDedup: false,
    closeToTray: true,
    notifyOnNewArticles: false,
  },

  /* ================= 导航 ================= */

  selectLayout: (layout) =>
    set({
      activeContentLayout: layout,
      activeFeedFilter: 'all',
      /* 视图筛选（全部/今天/未读/收藏）独立于布局，切换布局时保留用户选择 */
      activeArticleId: null,
      isShowingTranslatedProse: false,
      isRawRenderMode: false,
      /* 切换布局 = 刷新列表，清除"已读保留"快照 */
      openedReadIds: {},
    }),

  selectView: (view) => set({ activeViewFilter: view, openedReadIds: {} }),

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
      /* 范围语义与后端一致：当前 feed/分类范围（all 时两者皆 null） */
      const scope = get().activeFeedFilter;
      const feedId = scope.startsWith('f-') ? Number(scope.slice(2)) : null;
      const folderId = scope.startsWith('cat-') ? Number(scope.slice(4)) : null;
      void api.markAllRead(feedId, folderId);
    }
    set((s) => ({
      entries: s.entries.map((e) => (ids.has(e.id) ? { ...e, isRead: true } : e)),
      openedReadIds: {},
    }));
    get().showToast('已将当前筛选的所有内容标记为已读');
  },

  /* ================= 阅读器 ================= */

  selectArticle: (id) => {
    const { entries, settings, dataMode } = get();
    const art = entries.find((a) => a.id === id);
    if (!art) return;
    if (dataMode === 'tauri' && settings.markReadOnOpen && !art.isRead) {
      /* 后端模式：已读落库（不重载快照，本地同步置位即可） */
      void api.setRead(Number(id), true);
    }
    set((s) => ({
      /* 记录"本次会话中被打开过"：即使标已读，在未读筛选下也保留显示（原地变灰） */
      openedReadIds: { ...s.openedReadIds, [id]: true },
      entries:
        settings.markReadOnOpen && !art.isRead
          ? s.entries.map((a) => (a.id === id ? { ...a, isRead: true } : a))
          : s.entries,
      activeArticleId: id,
      isShowingTranslatedProse: false,
      isRawRenderMode: false,
    }));
    get().ensureArticleContent(id);
  },

  /** 正文懒加载（幂等）：列表快照不含 HTML，选中/社交卡片挂载时拉详情水合。
      守卫用「条目仍在且仍未水合」（不依赖 activeArticleId：社交卡片挂载时也走这里）。 */
  ensureArticleContent: (id) => {
    const { dataMode, entries } = get();
    if (dataMode !== 'tauri') return;
    const art = entries.find((a) => a.id === id);
    if (!art || art.content) return;
    void api.getArticle(Number(id)).then((row) => {
      if (!row) return;
      const cur = get().entries.find((a) => a.id === id);
      if (!cur || cur.content) return;
      const html = row.content_html ?? '';
      set((s) => ({
        entries: s.entries.map((a) =>
          a.id === id
            ? {
                ...a,
                content: html,
                rawContent: html,
                translatedContent: row.translated_content ?? '',
                /* 详情里的 summary 可能比列表 snippet 更完整（列表是截断的 body_text 兜底） */
                snippet: row.snippet || a.snippet,
                aiSummary: row.ai_summary ?? a.aiSummary,
                /* 原文网页地址（「源网页」「查看原文」外链） */
                url: row.url ?? a.url,
                /* 全文提取标志（DB 持久化：按钮状态与设置「自动全文」共用） */
                fulltextExtracted: row.fulltext_extracted ?? false,
              }
            : a,
        ),
      }));
      /* 「自动全文」模式：摘要型正文（<600 字符且非空）自动触发 Readability 提取 */
      const mode = get().settings.defaultOpenMode;
      if (mode === 'fulltext' && row.url && html.length > 0 && html.length < 600) {
        void api
          .extractFulltext(Number(id))
          .then((full) => {
            if (!full) return;
            set((s) => ({
              entries: s.entries.map((a) => (a.id === id ? { ...a, content: full, rawContent: full, fulltextExtracted: true } : a)),
            }));
          })
          .catch(() => {/* 提取失败保留 RSS 摘要正文，不打扰 */ });
      }
    });
  },

  clearReaderSelection: () =>
    set({ activeArticleId: null, isShowingTranslatedProse: false, isRawRenderMode: false }),

  /** 手动全文提取：Readability 拉原文网页覆盖正文；状态源与设置「自动全文」一致 */
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
            a.id === id ? { ...a, content: full, rawContent: full, fulltextExtracted: true } : a,
          ),
        }));
        showToast('全文提取完成');
      })
      .catch(() => showToast('全文提取失败，保留 RSS 正文'));
  },

  toggleCurrentReadStatus: () => {
    const { activeArticleId, entries, dataMode } = get();
    if (!activeArticleId) return;
    const art = entries.find((a) => a.id === activeArticleId);
    if (!art) return;
    if (dataMode === 'tauri') void api.setRead(Number(activeArticleId), !art.isRead);
    set((s) => ({
      entries: s.entries.map((a) => (a.id === activeArticleId ? { ...a, isRead: !a.isRead } : a)),
    }));
    get().showToast(art.isRead ? '已标记为未读' : '已标记为已读');
  },

  toggleCurrentStar: () => {
    const { activeArticleId, entries, dataMode } = get();
    if (!activeArticleId) return;
    const art = entries.find((a) => a.id === activeArticleId);
    if (!art) return;
    if (dataMode === 'tauri') void api.setStarred(Number(activeArticleId), !art.isStarred);
    set((s) => ({
      entries: s.entries.map((a) => (a.id === activeArticleId ? { ...a, isStarred: !a.isStarred } : a)),
    }));
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
    set({ translating: true, isShowingTranslatedProse: true });
    const articleId = art.id;
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
        () => set({ translating: false }),
        (msg) => {
          set({ translating: false, isShowingTranslatedProse: false });
          if (!silent) get().showToast(`翻译失败：${msg}`);
        },
      )
      .catch(() => {
        set({ translating: false, isShowingTranslatedProse: false });
        if (!silent) get().showToast('翻译失败：请先在设置中配置 AI 服务');
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
    set((s) => ({
      entries: s.entries.map((e) => (marked.has(e.id) ? { ...e, isRead: true } : e)),
      /* 标读的卡片原地变灰保留（未读筛选下不消失）；
         切视图/布局/筛选时的清理逻辑统一把它们移除 */
      openedReadIds: Object.fromEntries(unread.map((id) => [id, true])),
    }));
  },

  /** 按 id 流式生成摘要（增量落到 aiSummary，卡片实时打字机）。有缓存直接短路。 */
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
    set({ summaryGenerating: true });
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
          set({ summaryGenerating: false });
          if (!silent) get().showToast(`摘要失败：${msg}`);
        },
      )
      .catch(() => {
        set({ summaryGenerating: false });
        if (!silent) get().showToast('摘要失败：请先在设置中配置 AI 服务');
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
    set((s) => ({
      entries: s.entries.map((e) => (e.id === id ? { ...e, [field]: !e[field] } : e)),
    }));
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
    set((s) => ({ player: { ...s.player, isActive: false, isPlaying: false, seekToSec: null } })),

  /* ================= 弹层 ================= */

  openSettings: () => set({ settingsOpen: true }),
  closeSettings: () => set({ settingsOpen: false }),
  switchSettingsTab: (tab) => set({ settingsTab: tab }),
  openSettingsTab: (tab) => set({ settingsOpen: true, settingsTab: tab }),
  openSearch: () => set({ searchOpen: true }),
  closeSearch: () => set({ searchOpen: false }),
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

  showToast: (text) => {
    const id = ++toastId;
    set((s) => ({ toasts: [...s.toasts, { id, text }] }));
    /* 两段式生命周期：2200ms 后先标 leaving（CSS 退场过渡），200ms 过渡完成再卸载 */
    setTimeout(() => {
      set((s) => ({ toasts: s.toasts.map((t) => (t.id === id ? { ...t, leaving: true } : t)) }));
      setTimeout(() => {
        set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) }));
      }, 200);
    }, 2200);
  },

  /* ================= 数据源：后端 SQLite ⇄ mock ================= */

  /** 从后端拉全量快照（folders + feeds + articles）替换本地状态 */
  reloadFromBackend: async () => {
    const [folders, feeds, articles] = await Promise.all([
      api.listFolders(),
      api.listFeeds(),
      api.listArticles({ limit: 1000 }),
    ]);
    if (!folders || !feeds || articles === null) return;

    const categories = folderRowsToCategories(folders, feeds);
    set((s) => ({
      ...reconcileCategories(s, categories),
      entries: articles.map(articleRowToEntry),
      dataMode: 'tauri',
      dataLoading: false,
    }));
    /* 顺带刷新连接态：连接/断开后前端标签即时一致 */
    void api.syncStatus().then((st) => {
      if (st) set({ minifluxConnected: st.connected });
    });
  },

  /** 启动装载：Tauri 环境下从 SQLite 拉数据；浏览器保持 mock */
  bootstrapFromBackend: async () => {
    if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) {
      set({ dataMode: 'mock', dataLoading: false });
      return;
    }
    try {
      await get().reloadFromBackend();
    } catch (e) {
      /* 后端异常时回退 mock，保证界面可用 */
      console.error('bootstrap from backend failed:', e);
      set({ dataMode: 'mock', dataLoading: false });
    }
  },

  triggerManualSync: () => {
    if (get().dataMode === 'tauri') {
      set({ syncStatus: 'syncing' });
      /* 已连接 Miniflux → 推拉同步；未连接 → 纯直连刷新 */
      void api
        .syncNow()
        .catch(() => null) // 未连接（notConnected）→ 走纯直连刷新
        .then(() => api.refreshAllFeeds())
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
        .catch(() => {
          set({ syncStatus: 'error' });
          get().showToast('刷新失败');
        });
      return;
    }
    set({ syncStatus: 'syncing' });
    get().showToast('正在后台增量同步 Miniflux...');
    setTimeout(() => {
      set({ syncStatus: 'synced' });
      get().showToast('Miniflux 同步完成');
    }, 1100);
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
          collapsed: false,
          settingsCollapsed: false,
          layout,
          autoSummary: true,
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
          const msg = e instanceof Error ? e.message : String(e);
          get().showToast(`改名失败：${msg}`);
        });
      return;
    }
    set((s) => reconcileCategories(s, s.categories.map((c) => (c.id === catId ? { ...c, name: trimmed } : c))));
    get().showToast(`分类已改名：${trimmed}`);
  },

  addFeed: (catId, url, title, layout, autoSummary, autoTranslate) => {
    if (get().dataMode === 'tauri') {
      const folderId = Number(catId.replace('cat-', ''));
      set({ syncStatus: 'syncing' });
      void api
        .addFeed(url, title || null, folderId, layout, autoSummary, autoTranslate)
        .then(() => get().reloadFromBackend())
        .then(() => {
          set({ syncStatus: 'synced' });
          get().showToast(`已添加订阅源：${title || url}`);
        })
        .catch((e: unknown) => {
          set({ syncStatus: 'error' });
          const msg = e instanceof Error ? e.message : String(e);
          get().showToast(`添加失败：${msg}`);
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
          const msg = e instanceof Error ? e.message : String(e);
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
      .catch(() => get().showToast('刷新失败：源站不可达或地址失效'));
  },

  updateCatLayout: (catId, layout) => {
    if (get().dataMode === 'tauri') {
      void api
        .updateFolderLayout(Number(catId.replace('cat-', '')), layout)
        .then(() => get().reloadFromBackend())
        .catch(() => get().showToast('更新布局失败'));
      return;
    }
    set((s) => reconcileCategories(s, s.categories.map((c) => (c.id === catId ? { ...c, layout } : c))));
    get().showToast('已更新分类布局并即时生效');
  },

  updateFeedLayout: (catId, feedId, layout) => {
    if (get().dataMode === 'tauri') {
      void api
        .updateFeedLayout(Number(feedId), layout)
        .then(() => get().reloadFromBackend())
        .catch(() => get().showToast('更新布局失败'));
      return;
    }
    set((s) =>
      reconcileCategories(
        s,
        s.categories.map((c) =>
          c.id === catId
            ? { ...c, feeds: c.feeds.map((f) => (f.id === feedId ? { ...f, layout: layout as 'inherit' } : f)) }
            : c,
        ),
      ),
    );
    get().showToast('已更新订阅源布局并即时生效');
  },

  toggleCatSummary: (catId, val) => {
    set((s) => ({ categories: s.categories.map((c) => (c.id === catId ? { ...c, autoSummary: val } : c)) }));
    /* 落库：分类级 AI 标志 */
    const cat = get().categories.find((c) => c.id === catId);
    if (cat) void api.setFolderAiFlags(Number(catId.slice(4)), val, cat.autoTranslate);
  },

  toggleCatTranslate: (catId, val) => {
    set((s) => ({ categories: s.categories.map((c) => (c.id === catId ? { ...c, autoTranslate: val } : c)) }));
    const cat = get().categories.find((c) => c.id === catId);
    if (cat) void api.setFolderAiFlags(Number(catId.slice(4)), cat.autoSummary, val);
  },

  toggleFeedSummary: (catId, feedId, val) => {
    set((s) => ({
      categories: s.categories.map((c) =>
        c.id === catId ? { ...c, feeds: c.feeds.map((f) => (f.id === feedId ? { ...f, autoSummary: val } : f)) } : c,
      ),
    }));
    void api.setFeedAiFlags(Number(feedId.slice(2)), val, get().categories.find((c) => c.id === catId)?.feeds.find((f) => f.id === feedId)?.autoTranslate ?? false);
  },

  toggleFeedTranslate: (catId, feedId, val) => {
    set((s) => ({
      categories: s.categories.map((c) =>
        c.id === catId ? { ...c, feeds: c.feeds.map((f) => (f.id === feedId ? { ...f, autoTranslate: val } : f)) } : c,
      ),
    }));
    void api.setFeedAiFlags(Number(feedId.slice(2)), get().categories.find((c) => c.id === catId)?.feeds.find((f) => f.id === feedId)?.autoSummary ?? false, val);
  },

  toggleFolderCollapse: (catId) => {
    set((s) => ({
      categories: s.categories.map((c) => (c.id === catId ? { ...c, collapsed: !c.collapsed } : c)),
    }));
    /* 落库：分类折叠状态 */
    const cat = get().categories.find((c) => c.id === catId);
    if (cat) void api.setFolderCollapsed(Number(catId.slice(4)), cat.collapsed);
  },

  toggleAllFolders: () => {
    const anyOpen = get().categories.some((c) => !c.collapsed);
    set((s) => ({ categories: s.categories.map((c) => ({ ...c, collapsed: anyOpen })) }));
    get().showToast(anyOpen ? '已收起全部分类' : '已展开全部分类');
    /* 批量落库折叠状态 */
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
      for (const c of get().categories) void api.setFolderCollapsed(Number(c.id.slice(4)), anyOpen);
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

/* ============================================================
   Selector 钩子 —— 派生数据在组件层计算，store 保持精简
   ============================================================ */

export const CONTENT_LAYOUTS: ContentLayoutType[] = ['article', 'social', 'image', 'podcast', 'notification'];

export const LAYOUT_NAMES: Record<ContentLayoutType, string> = {
  article: '文章', social: '社交', image: '画廊', podcast: '播客', notification: '通知',
};

export const VIEW_NAMES: Record<string, string> = { all: '全部', today: '今天', unread: '未读', starred: '收藏' };

/** 解析订阅源生效的内容布局（feed 级覆盖 → 分类布局） */
export function resolveFeedLayout(feed: { layout: string } | undefined, catLayout: ContentLayoutType): ContentLayoutType {
  return feed && feed.layout !== 'inherit' ? (feed.layout as ContentLayoutType) : catLayout;
}

/** 视图筛选语义：与列表/树角标共用同一判定，保证数字与内容一致 */
export function matchesViewFilter(entry: ArticleEntry, view: ViewFilterType, now: number): boolean {
  switch (view) {
    case 'today': return isSameLocalDay(entry.publishedAt, now);
    case 'unread': return !entry.isRead;
    case 'starred': return entry.isStarred;
    default: return true;
  }
}

/** 当前布局下的全部条目（含已读）。
    布局不是条目属性，而是「feed 布局绑定 → 分类布局」的动态解析结果，
    修改绑定后条目即时迁移到新布局视图，无需数据搬迁。 */
export function selectRawEntries(
  s: Pick<AppState, 'activeContentLayout' | 'entries' | 'feedIndex'>,
): ArticleEntry[] {
  return s.entries.filter((e) => {
    const binding = s.feedIndex.get(e.feedId);
    return binding && resolveFeedLayout(binding.feed, binding.cat.layout) === s.activeContentLayout;
  });
}

/** 当前订阅范围（全部/某分类/某订阅源）内的条目（布局 + 范围双重过滤，含已读）。
    "视图"的计数与列表都以同一范围语义联动：选中单个订阅源时，
    全部/今天/未读/收藏的数字反映该订阅源内的条目。 */
export function selectScopeEntries(
  s: Pick<AppState, 'activeContentLayout' | 'activeFeedFilter' | 'entries' | 'feedIndex'>,
): ArticleEntry[] {
  let list = selectRawEntries(s);
  if (s.activeFeedFilter.startsWith('cat-')) {
    list = list.filter((i) => s.feedIndex.get(i.feedId)?.cat.id === s.activeFeedFilter);
  } else if (s.activeFeedFilter !== 'all') {
    list = list.filter((i) => i.feedId === s.activeFeedFilter);
  }
  return list;
}

/** 当前视图下应展示的条目（应用视图筛选 + 时间流筛选 + 排序） */
export function selectVisibleEntries(s: AppState): ArticleEntry[] {
  const now = Date.now();
  let list = selectScopeEntries(s);

  if (s.activeViewFilter === 'today') list = list.filter((i) => isSameLocalDay(i.publishedAt, now));
  else if (s.activeViewFilter === 'unread') list = list.filter((i) => !i.isRead || s.openedReadIds[i.id]);
  else if (s.activeViewFilter === 'starred') list = list.filter((i) => i.isStarred);

  if (s.timelineFilter === 'unread') list = list.filter((i) => !i.isRead || s.openedReadIds[i.id]);

  /* 时间流排序：真实客户端按时间戳降序/升序，不依赖数据插入顺序 */
  list = [...list].sort((a, b) => (s.timelineSort === 'newest' ? b.publishedAt - a.publishedAt : a.publishedAt - b.publishedAt));
  return list;
}

/** 侧边栏视图角标计数（跟随当前订阅范围）。
    口径与列表完全一致：范围 × 布局 × 视图 × 时间流筛选——
    角标数字 = 点击后列表里实际出现的条目数。 */
export function selectViewCounts(
  s: Pick<AppState, 'activeContentLayout' | 'activeFeedFilter' | 'activeViewFilter' | 'timelineFilter' | 'openedReadIds' | 'entries' | 'feedIndex'>,
) {
  const now = Date.now();
  const raw = selectScopeEntries(s).filter((i) => matchesTimelineFilter(i, s.timelineFilter, s.openedReadIds));
  return {
    all: raw.length,
    today: raw.filter((i) => isSameLocalDay(i.publishedAt, now)).length,
    unread: raw.filter((i) => !i.isRead).length,
    starred: raw.filter((i) => i.isStarred).length,
  };
}

/** 时间流筛选（显示: 全部/未读）的判定；未读模式下保留本会话打开过的条目（原地变灰） */
function matchesTimelineFilter(entry: ArticleEntry, filter: 'all' | 'unread', openedReadIds: Record<string, boolean>): boolean {
  return filter === 'all' || !entry.isRead || Boolean(openedReadIds[entry.id]);
}

/** 订阅树角标 —— 统一口径：数字 = 该行在「当前布局 × 当前视图」筛选下的条目数。
    布局与视图构成两道筛选条件，树不再另设 unread/total 之类的第二套数字。
    计数用严格判定（不含 openedReadIds 会话保留）：侧栏数字是导航概览，
    不随点开文章逐条抖动，与 Miniflux 未读语义一致。 */
export function selectTreeCounts(
  s: Pick<AppState, 'activeContentLayout' | 'activeViewFilter' | 'timelineFilter' | 'openedReadIds' | 'entries' | 'feedIndex'>,
): Map<string, number> {
  const now = Date.now();
  const counts = new Map<string, number>();
  const bump = (key: string) => counts.set(key, (counts.get(key) ?? 0) + 1);
  for (const e of s.entries) {
    const binding = s.feedIndex.get(e.feedId);
    if (!binding) continue;
    if (resolveFeedLayout(binding.feed, binding.cat.layout) !== s.activeContentLayout) continue;
    if (!matchesViewFilter(e, s.activeViewFilter, now)) continue;
    if (!matchesTimelineFilter(e, s.timelineFilter, s.openedReadIds)) continue;
    bump(e.feedId);
    bump(binding.cat.id);
    bump('all');
  }
  return counts;
}

/** 订阅源 → AI 配置解析（feed 值总是存在，绑定缺失即无配置） */
export function selectFeedConfig(s: Pick<AppState, 'feedIndex'>, feedId: string) {
  const binding = s.feedIndex.get(feedId);
  return {
    autoSummary: binding?.feed.autoSummary ?? false,
    autoTranslate: binding?.feed.autoTranslate ?? false,
  };
}
