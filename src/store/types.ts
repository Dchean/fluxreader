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
  /** cover 所属文章条目 id（TASK-092：封面失效上报用）。新剧集没给 cover 时沿用旧 cover，
      id 也随之沿用旧值，保证上报的 (id, url) 与实际显示的封面一致；空 = 未知 */
  coverEntryId?: string;
  /** 真实音频地址（enclosure_url）；空 = 无可播放源 */
  audioUrl: string;
  /** 当前播放位置（秒，PlayerBar 从 audio 元素同步） */
  positionSec: number;
  /** 总时长（秒；0 = 未加载） */
  durationSec: number;
  /** seek 请求（非 null 时 PlayerBar 执行 audio.currentTime 赋值后清回 null） */
  seekToSec: number | null;
}

/** OPT-015：SMTC 媒体键动作（Rust `player-media` 事件的 payload 域）。
 *  play/pause 是「目标态」语义（重复按不翻转），toggle 才是切换。 */
export type MediaAction = 'play' | 'pause' | 'toggle' | 'stop';

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
  /** 文章列表列宽（px，可拖动调整，280–560，默认 380） */
  listWidth: number;
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
  autoSync: boolean;
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

/** TASK-117：per-scope keyset 分页游标（articlesCursor 的值类型）。
 *  - lastPublished：已加载窗口最后一行的 published_at **原文**（RFC3339 字符串，
 *    后端返回什么就存什么——后端谓词与排序共用同一字符串比较口径，任何本地
 *    重格式化都会让游标错位）；空窗口为 null（无锚，续拉回落首页语义）。
 *  - lastId：最后一行的后端行 id（INTEGER PRIMARY KEY），并列 published_at 的
 *    决胜键；空窗口为 null。
 *  - loaded：已加载总行数。不再承担分页语义（OFFSET 已废除），仅供 exhausted
 *    镜像（articlesLimit）与视图缓存元数据（ViewEntriesSnapshot）使用。 */
export interface ArticlesCursorState {
  lastPublished: string | null;
  lastId: number | null;
  loaded: number;
}

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
  /** 正在生成摘要的文章 id 集合（F4：此前为全局单布尔，任一文章生成时
      所有空摘要卡片同时显示「正在生成…」） */
  summarizingIds: Record<string, true>;
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
  /** 灯箱图片所属文章条目 id（封面失效上报用；正文图等非封面场景为 null） */
  lightboxEntryId: string | null;
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
  syncConnected: boolean;
  /** TASK-116 四态展示：同步队列现存行数（「等待同步 N 条」）。
   *  启动装载（bootstrap/reload）与手动同步完成后的 reload 刷新；
   *  TASK-124 起 sync-queue-changed 事件即时刷新（同一状态源）。 */
  syncWaiting: number;
  /** TASK-116：队列中推送失败过的行数（attempts>0，「部分失败」） */
  syncFailed: number;
  /** TASK-124：队列最近一次推送失败的错误摘要（「部分失败」的错误行）。
   *  sync-queue-changed 事件 / reload 顺带刷新写入；未失败为 null。 */
  syncQueueLastError: string | null;

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
  /** 启动装载失败信息（tauri 模式后端异常时展示错误态+重试，绝不回退 mock 演示数据——P0-2） */
  bootstrapError: string | null;
  retryBootstrap: () => Promise<void>;
  /** 从后端拉全量快照替换本地状态。TASK-111②：opts.keepReadingPosition 仅由
      内容刷新路径（feeds-updated / 手动同步 / 单源刷新）传入，落地时发出保位
      信号；导航路径（selectFeed/selectView/selectLayout 等）不传，不触发回位。 */
  reloadFromBackend: (opts?: { keepReadingPosition?: boolean }) => Promise<void>;
  /** 当前视图的分页游标镜像（= 当前范围已从后端加载的文章总数）。每次 reload /
      视图切换 / 范围切换时，按该范围自己的 per-scope 游标恢复（TASK-052）。
      TASK-117：仅作 loaded 计数镜像（供 mock 兜底与调试观察），分页续拉不再
      消费它——续拉游标见 articlesCursor。 */
  articlesLimit: number;
  /** per-scope 分页游标表：scopeKey（'all' | feedId | 'cat-N'）→ keyset 游标。
      TASK-117（审计 P1-1）：从「已加载条数（OFFSET）」改为 keyset 锚
      （ArticlesCursorState）——可变筛选集合（WHERE is_read=0 / is_starred=1）上
      OFFSET 不等价于已看条数（读 500 标读后集合收缩，下一页仍 OFFSET 500 会跳过
      500 篇并假 exhausted）；keyset 以已加载窗口最后一行的 (published_at 原文, id)
      为锚，与集合增删无关地指向「已看过的最后一篇」。
      详见 internals.scopePageKey 的取舍说明与 ArticlesCursorState 契约注释。 */
  articlesCursor: Record<string, ArticlesCursorState>;
  /** 正在加载下一批文章（列表底部加载动画） */
  articlesLoading: boolean;
  /** 已加载完所有文章（列表底部显示「到底了」） */
  articlesExhausted: boolean;
  /** TASK-111②：后台刷新保位信号（nonce）。内容刷新路径（feeds-updated /
      手动同步 / 单源刷新）的 reload 落地时 bump；导航路径不 bump。Timeline
      订阅它，变化时消费顶条锚（timelineAnchor）做程序性回位。 */
  positionRestoreNonce: number;
  /** TASK-115①：切换返回恢复信号（nonce）。nav 三路径（selectLayout /
      selectView / selectFeed）**缓存命中**同步恢复 entries 时 bump；cache-miss
      （重拉新语境）与切排序不 bump。Timeline 订阅它，变化时消费 per-filterKey
      锚存档（timelineAnchor.peekReturnAnchor）做一次性定位。
      与 positionRestoreNonce（TASK-111 刷新保位）是两个独立信号：切换返回是
      「一次性定位」，刷新保位是「持续跟踪」——触发源/消费路径/锚来源（存档 vs
      活锚）完全分离，互不发出、互不消费对方信号（不叠加保证，见
      timelineAnchor.ts 头注规则表）。 */
  switchRestoreNonce: number;
  /** TASK-115②：阅读器关闭信号（nonce，不透明计数器）。clearReaderSelection
      关闭阅读器（App.tsx 唯一关闭路径）时 bump。原选中卡 id 不经 store 传递：
      由 Timeline 在 activeArticleId 跟随 effect 里用 ref 记账（关闭 commit 时
      该 ref 保留关闭前值），Timeline 的 readerClose effect 消费本信号把焦点
      归还原选中卡（原卡不在当前列表 → 归零回落）。选型与消费细节见
      Timeline.tsx readerClose effect 注释与 timelineAnchor.ts 头注 X2 节；
      nonce 而非布尔/id 字段：连续两次开关同一篇文章也要每次触发归还
      （id 字段值不变无法重触发 effect）。 */
  readerCloseNonce: number;
  /** 滚动到底部时按需拉取下一批文章（追加到 entries）。 */
  loadMoreArticles: () => Promise<void>;
  /** 切换视图到收藏/未读/今天时，按后端筛选拉取完整列表并替换 entries。
      这些视图需要完整数据，而「全部」视图的 entries 是分页快照。
      TASK-111②：opts.keepReadingPosition 仅由后台刷新透传（保位信号）。 */
  reloadFilteredEntries: (view: ViewFilterType, opts?: { keepReadingPosition?: boolean }) => Promise<void>;
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
  /** 同步恢复分页游标（per-scope 游标表的唯一镜像写入点）：写 articlesLimit /
      articlesCursor / articlesExhausted / articlesLoading 四处，entries 由调用方
      保证与该游标匹配（缓存恢复路径）。
      TASK-117：第二参从计数改为完整 keyset 游标（ArticlesCursorState）。 */
  applyArticlesCursor: (scopeKey: string, cursor: ArticlesCursorState, exhausted: boolean) => void;
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
  /** 卡片级翻译（社交/通知卡）：按 id 流式生成该条目译文，不依赖 Reader 选中态（P1-7） */
  translateEntry: (id: string, opts?: { silent?: boolean }) => void;
  /** 正在按 id 生成译文（卡片级状态，与全局 translating 单布尔隔离，避免多卡互串） */
  translatingIds: Record<string, true>;
  /** TASK-065 N11：该 id 的 translatedContent 当前是未消毒的流式产物——渲染契约：
      有标记按纯文本渲染（模型原始输出不得进 HTML 渲染路径），消毒回读成功后清除 */
  rawTranslatedIds: Record<string, true>;
  /** 正文懒加载水合（选中文章 / 社交卡片挂载） */
  ensureArticleContent: (id: string, opts?: { extractFulltext?: boolean }) => void;
  /** 批量水合正文：一批 id 一次 IPC 拉取、一次 set 更新（消除逐篇洪峰） */
  hydrateArticleContent: (ids: string[]) => void;
  /** 正文水合失败重试（卡片内联重试入口）：清错误态后重新入队 */
  retryHydration: (id: string) => void;
  /** TASK-122：正文/AI/水合状态的真值源迁往 bodyById（store/bodyCache.ts，
      state: loading|ready|cleared|missing|failed 判别态）——原 hydrationErrors /
      hydratedIds 两个按 id 平行 Map（正文侧状态分散，审计点名）随之移除，
      读取一律经 selectors.selectArticleBody / entryNeedsHydration。
      bodyCacheNonce：bodyById 是模块级缓存（不在 store 快照内），每次真实写入
      由 bodyCache 的 notify 回调 bump 本计数（store.ts 注入）——组件 selector
      引用它建立订阅依赖，记录变化才能触发重渲染。它只是通知序号，不承载任何
      领域状态（真值一律经 getBodyEntry 读取）。 */
  bodyCacheNonce: number;
  /** 手动全文提取（工具栏按钮；已提取时为刷新全文） */
  extractCurrentArticle: () => void;

  /* ---------- Actions: 卡片就地操作 ---------- */
  toggleEntryFlag: (id: string, field: 'isRead' | 'isStarred') => void;

  /* ---------- Actions: 播客 ---------- */
  playPodcastEpisode: (title: string, showName: string, cover: string, audioUrl: string, entryId?: string) => void;
  togglePlayerPlay: () => void;
  /** OPT-015：媒体键动作幂等消费（App.tsx `player-media` 事件唯一落点）。
   *  play/pause = 目标态（已在目标态则 no-op，不翻转）；toggle = 切换；
   *  stop = 关闭播放条。未激活（无剧集）时 play/pause/toggle 无任何副作用
   *  ——媒体键不得凭空启动无源播放器。 */
  applyMediaAction: (action: MediaAction) => void;
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
  openLightbox: (url: string, entryId?: string) => void;
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
  /** TASK-124：sync-queue-changed 事件落地（App.tsx 监听器调用）。
   *  payload = Rust sync_queue_stats 三元组 {waiting, failed, last_error}。
   *  只写队列三字段，绝不触碰 syncConnected——未配置静默语义在 Rust 发射点
   *  （无凭据不发事件），pill 的本地模式分支不因本动作破坏。 */
  applySyncQueueChanged: (stats: { waiting: number; failed: number; last_error: string | null }) => void;

  /* ---------- Actions: 订阅管理 ---------- */
  createCategory: (name: string, layout: ContentLayoutType) => void;
  deleteCategory: (catId: string) => void;
  /** 分类改名（连接 Miniflux 时同步远端） */
  renameCategory: (catId: string, name: string) => void;
  addFeed: (catId: string, url: string, title: string, layout: string, autoSummary: boolean, autoTranslate: boolean, syncToBackend?: boolean) => void;
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
