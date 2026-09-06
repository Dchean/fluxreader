import type {
  ArticleEntry,
  CategoryGroup,
  ContentLayoutType,
  FeedItem,
  PaletteTheme,
  ThemeMode,
  ViewFilterType,
} from '../types';

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
  /** 并发抓取上限（1–16，默认 4）：直连刷新同时请求的源站数量 */
  fetchConcurrency: number;
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
  /** 首次关闭询问已展示（true 后关窗直接按 closeToTray 走，不再问） */
  closePromptShown: boolean;
  /** 新文章到达发 Windows 系统通知（默认关；窗口隐藏时才发） */
  notifyOnNewArticles: boolean;
  /** 后台自动同步 Miniflux（默认开；到期跑轻量增量同步，状态变更另有即时推送） */
  autoSyncMiniflux: boolean;
  /** 同步模式（UI 词条：本机抓取 / 跟随服务端——命名直指差异点"内容从哪来"）：
   *  'direct' 本机抓取（默认）：全部源由 FluxReader 直连抓取，Miniflux 只同步状态；
   *  'hybrid' 跟随服务端：后台刷新跳过服务端来源的源，内容由 Miniflux 同步提供 */
  syncMode: 'direct' | 'hybrid';
}

/** leaving=true 时先走 CSS 退场过渡，200ms 后再卸载 DOM。
    action：可选操作按钮（失败 toast 的一键重试） */
export type ToastMessage = {
  id: number;
  text: string;
  leaving?: boolean;
  action?: { label: string; run: () => void };
};

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
  /** 全文视图开关：true=显示 Readability 全文，false=显示 RSS 原文 */
  showFulltext: boolean;
  summaryGenerating: boolean;
  /** AI 翻译流式生成中 */
  translating: boolean;
  /** 摘要失败：文章 id → 错误信息（卡片内联展示 + 重试依据） */
  summaryErrors: Record<string, string>;
  /** 翻译失败：文章 id → 错误信息（Reader 内联展示 + 重试依据） */
  translateErrors: Record<string, string>;
  /* 本次列表会话中被打开过的文章 id → 未读筛选下原地保留变灰（用户决策） */
  openedReadIds: Record<string, boolean>;

  /* ---------- 数据（Tauri 环境 = SQLite 快照；浏览器环境 = 演示数据） ---------- */
  categories: CategoryGroup[];
  /** 当前视图下应展示的条目完整集合。「全部」视图 = 分页快照（最新 N 篇 +
      滚动加载）；收藏/未读/今天视图 = 切换时按后端筛选拉取完整列表替换。
      条目的内容布局由「订阅源布局绑定 → 分类布局」在查询时动态解析。 */
  entries: ArticleEntry[];

  /** feedId → feed 引用的解析表（派生 selector 的公共底座） */
  feedIndex: Map<string, { feed: FeedItem; cat: CategoryGroup }>;

  /** 后端聚合的精确计数（feedId → total/unread/starred/today）。
      侧边栏角标用——不受文章列表分页 limit 影响，保证数字准确。
      （「全部/未读数字被 limit 截断」的根因修复） */
  feedCounts: Map<string, { total: number; unread: number; starred: number; today: number }>;

  /* ---------- 播客播放器 ---------- */
  player: PodcastPlayerState;

  /* ---------- 弹层 ---------- */
  settingsOpen: boolean;
  settingsTab: string;
  searchOpen: boolean;
  /** 首次关闭询问弹窗（Rust close-ask 事件驱动） */
  closeAskVisible: boolean;
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
  /** 后台自动同步进行中（scheduler sync-running/sync-idle 事件驱动；与手动 syncStatus 独立） */
  backgroundSyncing: boolean;
  /** 后端真实的 Miniflux 连接态（bootstrap/sync 后刷新），未连接时侧栏不显示"已同步" */
  minifluxConnected: boolean;

  /** GitHub 设备流登录：等待授权态（user_code 常驻显示；组件 unmount 不影响后端轮询） */
  githubFlow: { user_code: string; verification_uri: string; interval: number } | null;
  /** 已登录账户（设置页显示「已登录：username」） */
  githubAccount: { login: string } | null;
  /** 登录进行中（按钮防抖） */
  githubLoggingIn: boolean;

  /* ---------- 数据源模式 ---------- */
  /** tauri = 真实 SQLite 后端；mock = 浏览器开发回退 */
  dataMode: 'tauri' | 'mock';
  /** 后端数据加载中（首屏骨架） */
  dataLoading: boolean;
  bootstrapFromBackend: () => Promise<void>;
  reloadFromBackend: () => Promise<void>;
  /** 已从后端加载的文章数（分页游标：reload 重置为 PAGE_SIZE，loadMore 累加）。
      避免一次性全量拉取，滚动到底部按需追加，提高同步后重载速度。 */
  articlesLimit: number;
  /** 正在加载下一批文章（列表底部加载动画） */
  articlesLoading: boolean;
  /** 已加载完所有文章（列表底部显示「到底了」） */
  articlesExhausted: boolean;
  /** 滚动到底部时按需拉取下一批文章（追加到 entries）。 */
  loadMoreArticles: () => Promise<void>;
  /** 切换视图到收藏/未读/今天时，按后端筛选拉取完整列表并替换 entries。
      这些视图需要完整数据，而「全部」视图的 entries 是分页快照。 */
  reloadFilteredEntries: (view: ViewFilterType) => Promise<void>;
  /** 搜索/深层打开文章：计算目标文章在当前筛选下的绝对位置，从该页加载列表
      （而非从头拉 500 篇），并选中该文章。解决「搜索结果是很老的文章时，
      列表还停在第 1 页、定位不到」的问题。 */
  anchorToArticle: (articleId: string) => Promise<void>;

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
  /** 全文视图切换：RSS 原文 ↔ Readability 全文（已提取则直接切换，不重复请求） */
  toggleReaderFulltext: () => void;
  /** opts.silent：源级开关自动触发时静默失败（未配置 AI 不弹 toast） */
  toggleReaderTranslation: (opts?: { silent?: boolean }) => void;
  triggerReaderSummary: (opts?: { silent?: boolean }) => void;
  /** 列表卡片就地摘要（通知布局）：按文章 id 流式生成，不依赖阅读器选中态 */
  summarizeEntry: (id: string, opts?: { silent?: boolean }) => void;
  /** 滚动触发的批量已读（滚出列表/正文到底）：静默、只标未读项 */
  markEntriesReadBulk: (ids: string[]) => void;
  /** 正文懒加载水合（选中文章 / 社交卡片挂载） */
  ensureArticleContent: (id: string, opts?: { extractFulltext?: boolean }) => void;
  /** 批量水合正文：一批 id 一次 IPC 拉取、一次 set 更新（消除逐篇洪峰） */
  hydrateArticleContent: (ids: string[]) => void;
  /** 手动全文提取（工具栏按钮；已提取时为刷新全文） */
  extractCurrentArticle: () => void;

  /* ---------- Actions: 卡片就地操作 ---------- */
  toggleEntryFlag: (id: string, field: 'isRead' | 'isStarred') => void;

  /* ---------- Actions: 播客 ---------- */
  playPodcastEpisode: (title: string, showName: string, cover: string, audioUrl: string, entryId?: string) => void;
  togglePlayerPlay: () => void;
  cyclePlaybackSpeed: () => void;
  closePodcastBar: () => void;
  /** Full Player 展开态（大播放器覆盖层；Esc/再次点击收起） */
  playerExpanded: boolean;
  togglePlayerExpanded: () => void;
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
  /** 首次关闭询问的应答（Rust resolve_close 执行隐藏/退出；remember 时同步设置镜像） */
  answerCloseAsk: (action: 'tray' | 'exit', remember: boolean) => void;
  openLightbox: (url: string) => void;
  closeLightbox: () => void;
  openNewCategoryModal: () => void;
  openAddFeedModal: (catId: string) => void;
  openEditFeedModal: (feedId: string) => void;
  openRenameCatModal: (catId: string) => void;
  closeMiniModal: (which: 'newCategory' | 'addFeed' | 'editFeed' | 'renameCat') => void;

  /* ---------- Actions: Toast / 同步 ---------- */
  /** text + 可选操作按钮（label/run：失败场景的一键重试） */
  showToast: (text: string, action?: { label: string; run: () => void }) => void;
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

  /** 发起 GitHub 设备流登录（轮询不依赖组件生命周期） */
  githubLoginStart: () => Promise<void>;
  /** 断开 GitHub 登录 */
  githubLoginDisconnect: () => Promise<void>;
}
