import type {
  ArticleEntry,
  CategoryGroup,
  ContentLayoutType,
  FeedItem,
  ViewFilterType,
} from '../types';
import { isSameLocalDay } from '../lib/format';
import type { AppState } from './types';

/* ============================================================
   Selector 钩子 —— 派生数据在组件层计算，store 保持精简
   ============================================================ */


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

  /* 时间流排序：真实客户端按时间戳降序/升序，不依赖数据插入顺序 */
  list = [...list].sort((a, b) => (s.timelineSort === 'newest' ? b.publishedAt - a.publishedAt : a.publishedAt - b.publishedAt));
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
