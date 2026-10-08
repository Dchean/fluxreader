// Note: selectArticleBody 的订阅必须包 useShallow（zustand v5 快照不缓存 → React #185） — 见 .agents/notes/implemented/architecture/2026-10-06-正文与-AI-产物的实体缓存.md
import type {
  ArticleEntry,
  CategoryGroup,
  ContentLayoutType,
  FeedItem,
  ViewFilterType,
} from '../types';
import { isSameLocalDay } from '../lib/format';
import { getBodyEntry } from './bodyCache';
import type { BodyEntry, BodyState } from './bodyCache';
import type { AppState } from './types';

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

/* ============================================================
   TASK-122：正文/AI 读取单点（真值源 bodyById，store/bodyCache.ts）

   selectArticleBody 是卡片与阅读器取正文/AI 产物的**唯一**入口：
   - 记录存在（loading/ready/cleared/missing/failed）→ 一切以记录为准；
   - 无记录（未请求 / LRU 淘汰）→ content/rawContent 为空；AI 字段回退视图行
     （列表行携带的 ai_summary/translated_content 是同一 DB 的查询真值，未水合
     卡片靠它即时显示摘要/译文；水合后记录接管）。该回退是过渡兼容位：列表行
     停止携带 AI 列（Rust DTO 分离，另卡）后即可删除；
   - mock 演示模式无后端 IPC、bodyById 永不落记录 → 回退视图行的
     content/rawContent（mock 数据的内联正文，ArticleEntry 上仅存的两处正文
     兼容位）。tauri 链路不得依赖该回退。
   ============================================================ */

/** 正文/AI 读取快照（组件订阅形态；记录引用稳定，写入即换引用） */
export interface ArticleBodyView {
  /** 判别态：unrequested = 无记录（未请求/已淘汰）；其余透传 BodyState */
  state: BodyState | 'unrequested';
  content: string;
  rawContent: string;
  translatedContent: string;
  aiSummary: string;
  fulltextExtracted: boolean;
  /** missing/failed 的呈现文案（其余态为空） */
  message: string;
  /** 显式失效计数（t122 断言/调试观察用） */
  contentRevision: number;
}

const UNREQUESTED_BODY: ArticleBodyView = {
  state: 'unrequested',
  content: '',
  rawContent: '',
  translatedContent: '',
  aiSummary: '',
  fulltextExtracted: false,
  message: '',
  contentRevision: 0,
};

function bodyViewFrom(rec: BodyEntry): ArticleBodyView {
  return {
    state: rec.state,
    content: rec.content,
    rawContent: rec.rawContent,
    translatedContent: rec.translatedContent,
    aiSummary: rec.aiSummary,
    fulltextExtracted: rec.fulltextExtracted,
    message: rec.message,
    contentRevision: rec.contentRevision,
  };
}

/** 单篇文章的正文/AI 视图（读取单点；语义见本文件 TASK-122 头注）。
    订阅依赖：本函数读取的 bodyById 是模块级状态，组件经 useAppStore 传整个
    state 进来——此处显式消费 s.bodyCacheNonce（bodyCache 每次真实写入 bump，
    store.ts 注入 notify）建立订阅依赖，记录变化才能触发 selector 重算。 */
export function selectArticleBody(s: Pick<AppState, 'entries' | 'dataMode' | 'bodyCacheNonce'>, id: string | null | undefined): ArticleBodyView {
  void s.bodyCacheNonce; // 订阅依赖（见上）：值本身无意义，真值经 getBodyEntry 读取
  if (!id) return UNREQUESTED_BODY;
  const rec = getBodyEntry(id);
  if (rec) return bodyViewFrom(rec);
  const entry = s.entries.find((a) => a.id === id);
  if (!entry) return UNREQUESTED_BODY;
  /* 无记录回退：mock = 演示正文全量；tauri = 仅 AI 列与全文标记（行携带真值），
     正文恒空（未请求态——懒水合会按 entryNeedsHydration 入队） */
  if (s.dataMode === 'mock') {
    return {
      state: 'ready',
      content: entry.content ?? '',
      rawContent: entry.rawContent ?? '',
      translatedContent: entry.translatedContent,
      aiSummary: entry.aiSummary,
      fulltextExtracted: entry.fulltextExtracted ?? false,
      message: '',
      contentRevision: 0,
    };
  }
  return {
    state: 'unrequested',
    content: '',
    rawContent: '',
    translatedContent: entry.translatedContent,
    aiSummary: entry.aiSummary,
    fulltextExtracted: entry.fulltextExtracted ?? false,
    message: '',
    contentRevision: 0,
  };
}

/** TASK-122：单张卡片「需要水合正文」的判定 —— 条目仍在 && bodyById 无记录。
    记录存在即不需要水合：loading=在途（重复入队由记录状态拦下，替代原模块级
    hydrationInFlight）、ready=已水合（含空正文终态）、cleared=AI 失效但正文
    保留、missing/failed=终态（失败走内联重试 retryHydration 删记录重入队，
    不自动重试）。useLazyHydrate 按 id 订阅本判定的布尔值：快照替换（reload /
    缓存恢复）或 LRU 淘汰让条件重新成立时布尔翻转 → effect 重新入队，消除
    「显示加载中但无请求在途」的死区（REQ-001）；预算淘汰（记录被删）→ 回
    未请求态 → 自动重取（TASK-111 内存预算纪律的淘汰语义）。
    重复入队由 hydrateArticleContent 的记录守卫兜底，这里不观察请求本身。 */
export function entryNeedsHydration(
  s: Pick<AppState, 'entries' | 'bodyCacheNonce'>,
  id: string,
): boolean {
  void s.bodyCacheNonce; // 订阅依赖：记录写入 bump nonce，useLazyHydrate 的订阅才能重算
  if (getBodyEntry(id)) return false;
  return s.entries.some((a) => a.id === id);
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

/** 当前视图下应展示的条目（应用视图筛选 + 时间流筛选 + 排序）。
    结果按输入引用缓存：同一份 entries/feedIndex/筛选标量下多次调用返回同一数组
    引用，useShallow 逐元素浅比较直接命中（元素引用相同）→ 跳过整表 diff。
    水合/标读会产生新的 entries 数组（引用变化），缓存自然失效重算一次——但
    批量水合已把「每篇一次 set」收敛为「每批一次 set」，重算频率大幅下降。 */
let visibleEntriesCache: {
  entries: ArticleEntry[];
  feedIndex: Map<string, { feed: FeedItem; cat: CategoryGroup }>;
  openedReadIds: Record<string, boolean>;
  key: string;
  result: ArticleEntry[];
} | null = null;

export function selectVisibleEntries(s: AppState): ArticleEntry[] {
  const key = `${s.activeContentLayout}|${s.activeFeedFilter}|${s.activeViewFilter}|${s.timelineFilter}|${s.timelineSort}`;
  if (
    visibleEntriesCache &&
    visibleEntriesCache.entries === s.entries &&
    visibleEntriesCache.feedIndex === s.feedIndex &&
    visibleEntriesCache.openedReadIds === s.openedReadIds &&
    visibleEntriesCache.key === key
  ) {
    return visibleEntriesCache.result;
  }

  const now = Date.now();
  let list = selectScopeEntries(s);

  if (s.activeViewFilter === 'today') list = list.filter((i) => isSameLocalDay(i.publishedAt, now));
  else if (s.activeViewFilter === 'unread') list = list.filter((i) => !i.isRead || s.openedReadIds[i.id]);
  else if (s.activeViewFilter === 'starred') list = list.filter((i) => i.isStarred);

  // 「显示: 全部/未读」只在「全部」视图下生效：今天/未读/收藏视图已有各自
  // 明确的筛选语义，再叠加「显示未读」会把收藏(全已读)、今天等视图误筛成空
  // （「收藏视图不显示列表」的根因）。
  if (s.activeViewFilter === 'all' && s.timelineFilter === 'unread') {
    list = list.filter((i) => !i.isRead || s.openedReadIds[i.id]);
  }

  /* 时间流排序：真实客户端按时间戳降序/升序，不依赖数据插入顺序。
     TASK-117：并列 publishedAt 由 id 决胜——与后端 ORDER BY a.published_at, a.id
     同向同口径（keyset 续拉锚依赖**全序**：同秒文章的服务器顺序由 id 定，
     本地排序若不补同一决胜，续拉追加会与已加载窗口交错乱序）。
     id 是后端行 id 的十进制字符串，数值比较与 SQLite 的 id 序一致；
     mock 的非数字 id 落回 0（保持既有相对顺序，不受影响）。 */
  const dir = s.timelineSort === 'newest' ? -1 : 1;
  list = [...list].sort((a, b) => {
    const byTime = dir * (a.publishedAt - b.publishedAt);
    if (byTime !== 0) return byTime;
    const na = Number(a.id);
    const nb = Number(b.id);
    if (!Number.isFinite(na) || !Number.isFinite(nb)) return 0;
    return dir * (na - nb);
  });
  visibleEntriesCache = { entries: s.entries, feedIndex: s.feedIndex, openedReadIds: s.openedReadIds, key, result: list };
  return list;
}

/** 侧边栏视图角标计数（跟随当前订阅范围）。
    口径与列表一致（范围 × 布局 × 时间流筛选）；「全部」键受「显示: 全部/未读」
    （timelineFilter）影响：显示全部时 = 全部文章数，显示未读时 = 未读数；
    其余 view filter（今天/未读/收藏）各自独立计数。 */
export function selectViewCounts(
  s: Pick<AppState, 'activeContentLayout' | 'activeFeedFilter' | 'timelineFilter' | 'feedIndex' | 'feedCounts'>,
) {
  // 用后端精确计数（feedCounts）聚合，而非前端 entries——entries 受分页 limit
  // 截断，会导致「全部/未读数字不准确」。遍历 feedIndex，按「当前布局 × 订阅范围」
  // 累加各 feed 的 total/unread/starred/today。
  let total = 0, today = 0, unread = 0, starred = 0;
  for (const [feedId, binding] of s.feedIndex) {
    if (resolveFeedLayout(binding.feed, binding.cat.layout) !== s.activeContentLayout) continue;
    // 订阅范围过滤：'cat-xxx' 只算该分类，单个 feed 只算该 feed
    if (s.activeFeedFilter.startsWith('cat-') && binding.cat.id !== s.activeFeedFilter) continue;
    if (s.activeFeedFilter !== 'all' && !s.activeFeedFilter.startsWith('cat-') && feedId !== s.activeFeedFilter) continue;
    /* L2（有意为之的保守设计，保持不改）：feedCounts 缺该源时直接跳过，不退回
       按 entries 计数。原因：entries 是「当前分页快照」（列表最多 500 条/页），
       按它计数会重新引入本函数注释开头点明的「数字不准确」根因；feedCounts 是
       后端精确计数，缺项只会让数字偏小（保守），不会虚高或与列表口径打架。
       代价：该源的角标为空、其条目不计入「全部」数字，但条目仍正常列出——
       由回归断言「(L2) feedCounts 缺项：该源不计入「全部」总数，树角标也不建
       该行（保守设计）」+「(L2) 但该源条目仍正常列出」锚定。 */
    const c = s.feedCounts.get(feedId);
    if (!c) continue;
    total += c.total;
    today += c.today;
    unread += c.unread;
    starred += c.starred;
  }
  // 「全部」键的数字随「显示: 全部/未读」动态变化
  const all = s.timelineFilter === 'unread' ? unread : total;
  return { all, today, unread, starred };
}

/** 订阅树角标 —— 统一口径：数字 = 该行在「当前布局 × 当前视图」筛选下的条目数。
    布局与视图构成两道筛选条件，树不再另设 unread/total 之类的第二套数字。
    计数用严格判定（不含 openedReadIds 会话保留）：侧栏数字是导航概览，
    不随点开文章逐条抖动，与 Miniflux 未读语义一致。 */
/** 从 store 的字符串 id（纯数字 / 'cat-'/'feed-' 前缀两种形态）提取后端数字 id。
    历史 bug：mock 用 'feed-'+Date.now() 前缀、真实数据用 String(row.id) 纯数字，
    定长 slice(2)/slice(4) 会把 '26'.slice(2) 截成 ''→NaN，布局/AI 开关等
    全部写库失败且被乐观更新掩盖（7 类模式 C 契约断裂变体）。 */
export function numericId(id: string): number {
  const m = id.match(/(\d+)/);
  return m ? Number(m[1]) : NaN;
}

export function selectTreeCounts(
  s: Pick<AppState, 'activeContentLayout' | 'activeViewFilter' | 'timelineFilter' | 'feedIndex' | 'feedCounts'>,
): Map<string, number> {
  // 订阅树角标：数字 = 该行在「当前布局 × 当前视图 × 显示: 全部/未读」筛选下的
  // 条目数。用后端精确计数聚合（feedCounts），不受文章列表分页 limit 影响。
  // 口径：timelineFilter（显示: 未读）优先——切到未读时订阅源数字随之变未读，
  // 与「全部」键动态数字一致；否则按 activeViewFilter（全部→total、未读→unread…）。
  const counts = new Map<string, number>();
  const bump = (key: string, n: number) => counts.set(key, (counts.get(key) ?? 0) + n);
  for (const [feedId, binding] of s.feedIndex) {
    if (resolveFeedLayout(binding.feed, binding.cat.layout) !== s.activeContentLayout) continue;
    /* L2：feedCounts 缺该源 → 不建角标（与 selectViewCounts 同一取舍：精确计数
       缺失时保守留空，不退回受分页截断的 entries 计数）。 */
    const c = s.feedCounts.get(feedId);
    if (!c) continue;
    // 「显示: 未读」只在「全部」视图生效（与 selectVisibleEntries 同口径），
    // 其余视图按各自筛选语义。
    const n = s.activeViewFilter === 'all' && s.timelineFilter === 'unread'
      ? c.unread
      : s.activeViewFilter === 'unread' ? c.unread
      : s.activeViewFilter === 'starred' ? c.starred
      : s.activeViewFilter === 'today' ? c.today
      : c.total;
    bump(feedId, n);
    bump(binding.cat.id, n);
    bump('all', n);
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

/* ============================================================
   P3[F3]（REQ-104 / TASK-081）：播客卡片「再点同一集」的点击判定

   放在本文件（而非组件内）的理由：组件文件里 export 非组件函数会触发 oxlint 的
   react/only-export-components（Fast Refresh 约束，与 timelineSentinel.ts 的
   处理一致）；本文件本就承载纯函数（如 numericId），且被回归网直接断言。

   契约：
   - 播放器未激活 → 'play'（首次点某集正常从头开始）；
   - 已激活且是**同一集**（audioUrl 相同）→ 'toggle'（播放/暂停切换，**保留进度**）；
   - 点了**另一集** → 'play'（正常换集，仍从头播）；
   - 该集无音频地址 → 'play'（交给 playPodcastEpisode 走它的「无可播放地址」提示）。

   修前行为：卡片点击无条件调 playPodcastEpisode，而该 action 会
   `positionSec: 0, isPlaying: true` —— 播放中再点同一集会把进度清零重开。
   ============================================================ */
export type PodcastClickAction = 'toggle' | 'play';

export function podcastClickAction(
  playerActive: boolean,
  currentAudioUrl: string,
  clickedAudioUrl: string,
): PodcastClickAction {
  if (!clickedAudioUrl) return 'play';
  return playerActive && currentAudioUrl === clickedAudioUrl ? 'toggle' : 'play';
}

/* ============================================================
   fix-8（自检 UI-P1-3）：通知/社交卡「跟随 auto 配置」的 AI 区块展开判定。

   缺陷背景：summaryOpen / showTranslate 跟随 feedConfig.auto* 直接展开——
   但自动生成本身只在 Reader 打开文章时触发（卡片挂载只水合正文，刻意不
   就地发起生成，防滚动 IPC 风暴），于是「auto 开 + 尚无产物」的卡片长期
   渲染一个空壳框。

   契约：
   - 出错 → 恒展开（错误行 + 重试按钮必须可见，与 Reader 失败态同口径）；
   - 非 auto 且无错 → 收起；
   - auto 开：已有产物 / 正在生成 ⇒ 展开；三者皆无 ⇒ 收起（用户点卡片上的
     摘要/翻译按钮仍会就地触发生成并展开——手动路径不受影响）。
   ============================================================ */
export function autoAiBlockOpen(
  autoOn: boolean,
  hasOutput: boolean,
  generating: boolean,
  hasError: boolean,
): boolean {
  if (hasError) return true;
  if (!autoOn) return false;
  return hasOutput || generating;
}
