import type { StoreApi } from 'zustand';
import { extractError } from '../lib/api';
import type {
  ArticleEntry,
  CategoryGroup,
  ContentLayoutType,
  FeedItem,
  ViewFilterType,
} from '../types';
import type { AppState } from './types';

/* ============================================================
   跨 slice 共享的模块级基础设施。

   拆分前这些内容都在 store.ts 的模块区：模块级可变状态（视图缓存）
   与「直接读写 useAppStore」的工具函数（标读/收藏收口、条目标记）。
   它们被多个 slice 共用，故收口到这里，store 句柄由 store.ts 在
   create() 之后注入（bindAppStore）——与原实现「模块求值完成后才会
   调用这些函数」的时序一致。
   ============================================================ */

/** store 句柄：由 store.ts 在 create() 后注入（模块求值期不调用任何 helper） */
let appStoreRef: StoreApi<AppState> | undefined;

export function bindAppStore(api: StoreApi<AppState>): void {
  appStoreRef = api;
}

/** store 句柄读取：未注入即显式抛错。
    此前是 `appStoreRef as StoreApi<AppState>` —— 类型上由断言强行伪装成已就绪，
    一旦某个 helper 在 bindAppStore 之前被调用（模块求值期、或未来新增的调用点），
    拿到的是 undefined，报错点会漂移到 `undefined.getState is not a function`
    这种与真实原因无关的位置。改成显式抛错，把失败点固定在根因处。 */
export function appStore(): StoreApi<AppState> {
  if (!appStoreRef) {
    throw new Error('appStore() 在 bindAppStore() 之前被调用：store 尚未创建（模块求值期不得读写 store）');
  }
  return appStoreRef;
}

/** 视图切换缓存：key = 「布局 × 视图」→ 该视图最近一次拉取的 entries 快照。
    用途：视图切换（尤其「收藏19 ↔ 全部2122」这类数量悬殊的切换）不再每次
    重新从后端拉取 + 一次性渲染数百张卡片（卡顿根因），而是先同步恢复缓存
    零延迟显示，再后台异步刷新。模块级（非 store 状态）避免触发重渲染。
    TASK-100 P3-7：容量上限（LRU，8 个「布局×视图×范围」组合键，先淘汰最旧）。
    此前无上限：筛选视图缓存的是 limit=100000 的全集快照，超大库长会话按组合
    键持续累积、内存增长无界（与 coverImage 缓存的 300 上限不对称）。8 个键
    已覆盖「来回切布局/视图/范围」的真实导航深度——命中即刷新新鲜度，淘汰
    只影响再次进入该组合时的一次重拉（数据由后台刷新补齐），无正确性影响。 */
const VIEW_ENTRIES_CACHE_MAX = 8;

class LRUMap<V> extends Map<string, V> {
  private readonly max: number;

  constructor(max: number) {
    super();
    this.max = max;
  }

  /** 命中即刷新为「最新使用」（Map 迭代序 = 插入序，淘汰时删最旧） */
  get(key: string): V | undefined {
    const v = super.get(key);
    if (v !== undefined) {
      super.delete(key);
      super.set(key, v);
    }
    return v;
  }

  set(key: string, value: V): this {
    super.delete(key); // 重复写入先摘除再插入，保证 LRU 新鲜度
    super.set(key, value);
    while (super.size > this.max) {
      const oldest = super.keys().next().value;
      if (oldest === undefined) break;
      super.delete(oldest);
    }
    return this;
  }
}

export const viewEntriesCache: Map<string, ArticleEntry[]> = new LRUMap<ArticleEntry[]>(VIEW_ENTRIES_CACHE_MAX);

/** 视图缓存 key：布局 × 视图 × 订阅范围（scope）。
    TASK-052 起**必须带 scope**：条目列表现在是「该范围的首批 N 条」，缓存若不
    带范围，源A 的首批会被当成「全部」的首批恢复——列表内容与标题/角标错配。
    （范围进 key 后缓存条目仍受控：只有实际被 reload / 切视图的「布局×视图×范围」
    组合会建档，与之前「具体范围不缓存」的取舍等价。）
    TASK-094：entries 快照本就按布局分桶（键首段），格式与此前逐字一致。 */
export function viewCacheKey(layout: ContentLayoutType, view: ViewFilterType, scope = 'all'): string {
  return `${layout}|${view}|${scope || 'all'}`;
}

/* ============================================================
   TASK-052：per-scope 分页游标（口径改造）

   缺陷 P1-14：「口径」半边 —— 列表口径（selectVisibleEntries：范围 × 视图 ×
   时间流筛选）与查询口径（list_articles 参数）此前不一致：分页请求只带
   limit/offset，不带 feed_id/folder_id/only_*，于是 articlesLimit 实际是
   **全局查询的 offset**，把「全局序列的第 N 条」当成了「当前范围的第 N 条」。
   后果：单源/单分类视图下滚，取回的不是该源的后续文章；且列表为空时哨兵
   不渲染（items.length > 0 才渲染），该源的老文章永远够不到。

   本模块把「查询口径」收口成一处：
   - scopeQueryArgs(scope, sort)：订阅范围 + 排序 → 后端参数（feed_id/folder_id
     /newest_first），与 anchorToArticle / markCurrentViewAllRead 同口径；
   - scopePageKey(scope, view)：分页游标键 —— 筛选口径可独立翻页，但共享
     entries，故游标键**只取订阅范围**（feed/分类），不含视图与排序。
   ============================================================ */

/** 从字符串 id（纯数字 / 'cat-' / 'feed-' 前缀）提取后端数字 id。
    （与 selectors.numericId 同形；此处模块内私有，避免 internals ↔ selectors 循环依赖） */
function scopeNumericId(raw: string): number {
  const m = raw.match(/(\d+)/);
  return m ? Number(m[1]) : NaN;
}

/** 订阅范围 → 后端范围维度（feed_id / folder_id）。
    TASK-109：从 scopeQueryArgs 拆出的最底层派生——「与排序、布局无关」的标写
    范围（mark_all_read，经 QueryScope.markScope）与列表查询共用同一份映射，
    'cat-N' → folder_id、'all' → 双 null、其余 → feed_id=数字。 */
function scopeFilterArgs(scope: string): { feed_id: number | null; folder_id: number | null } {
  const isCat = scope.startsWith('cat-');
  return {
    feed_id: scope === 'all' || isCat ? null : scopeNumericId(scope),
    folder_id: isCat ? scopeNumericId(scope) : null,
  };
}

/** 订阅范围 + 排序 → list_articles / article_index 的查询参数。
    'cat-N' → folder_id=N；'all' → 两者皆 null；其余 → feed_id=数字。
    两种 id 形态（纯数字 '12' / 前缀 'feed-12'）统一走数字提取。
    TASK-094（REQ-107）：第三参 layout（可选）→ 透传给后端 list_articles /
    article_index 的布局过滤（feed 级覆盖 → 分类兜底，与 resolveFeedLayout 同口径）。
    此前布局只在前端本地过滤：后端全局分页、稀疏布局首批撑不满容器且 onScroll
    不触发，列表永远停在首批。不传 layout 的调用（旧断言/无布局语义的调用点）
    返回值与修前逐字一致（不含 layout 键）。
    TASK-109：范围维度拆入 scopeFilterArgs；请一律经 QueryScope.args 消费。 */
export function scopeQueryArgs(
  scope: string,
  sort: 'newest' | 'oldest',
  layout?: ContentLayoutType,
): { feed_id: number | null; folder_id: number | null; newest_first: boolean; layout?: ContentLayoutType } {
  return {
    ...scopeFilterArgs(scope),
    newest_first: sort === 'newest',
    ...(layout ? { layout } : {}),
  };
}

/** 分页游标键：per-(布局 × 范围)。'all' 亦有意作为一等范围键（而非空串/缺省），
    否则不带 scope 的场景会被误并进 'all' 的游标。
    TASK-094（R7）：键必须含布局——布局切换后 entries 换成另一布局的快照，游标若
    只按范围记账，「画廊第 2 页」会接着「文章第 1 页」的全局 offset 翻，整段错位。
    不传 layout 的调用返回值与修前逐字一致（仅旧测试/兼容路径）。
    TASK-109：字符串形态锁定不变（缓存/游标键兼容，t109 断言锁定）；请一律经
    QueryScope.pageKey 消费。 */
export function scopePageKey(scope: string, layout?: ContentLayoutType): string {
  return layout ? `${layout}|${scope || 'all'}` : scope || 'all';
}

/* TASK-109：查询口径统一派生入口（QueryScope）——三把键与查询参数只从这里派生：
   - args：后端查询参数（范围×排序×可选布局）→ list_articles / article_index；
   - markScope：标写范围维度（仅 feed_id/folder_id，排序/布局不进标写口径——
     布局由 api.markAllRead 的独立 layout 参数承载）；
   - viewFilter：视图筛选参数（only_unread/only_starred/only_today）——筛选视图
     首屏与续拉共用（TASK-110）；
   - pageKey：分页游标键（布局×范围）；
   - viewKey：视图快照缓存键（布局×视图×范围）；
   - paginationStale / filteredSnapshotStale：两类快照响应的具名竞态守卫。
   pageKey / viewKey 的字符串形态锁定不变（缓存/游标键兼容，t109 断言锁定）。 */

/** 列表分页大小：首批/每次滚动加载拉取的文章数（「全部」视图与筛选视图共用，
    TASK-110 自 bootstrap.ts 收口到此处——筛选视图分页化后两条路径共享同一页大小
    与游标口径，常量必须有单一来源）。 */
export const ARTICLES_PAGE_SIZE = 500;

/** TASK-110：视图筛选参数（only_unread / only_starred / only_today）——筛选视图
    首屏（reloadFilteredEntries）与续拉（loadMoreArticles）共用同一派生，保证
    「加载更多」取到的集合与首屏同口径；'all' → 空对象（wire 形态与不传键逐字
    一致）。 */
export function viewFilterArgs(view: ViewFilterType): { only_unread?: boolean; only_starred?: boolean; only_today?: boolean } {
  return {
    ...(view === 'unread' ? { only_unread: true } : {}),
    ...(view === 'starred' ? { only_starred: true } : {}),
    ...(view === 'today' ? { only_today: true } : {}),
  };
}

/** TASK-109：分页续拉（loadMoreArticles）的竞态守卫——具名化收口。
    响应落地时，发起时快照的「范围×布局（scopeKey）× 排序 × 游标 × 视图」任一
    漂移即整页丢弃：
    - scopeKey 漂移：查询口径已换（切范围/切布局）；
    - 游标漂移：reload / 缓存恢复已重置该范围的分页进度（D3 / TASK-052）；
    - 排序漂移：offset 的含义随排序翻转（F1），旧排序的响应属于另一查询口径；
    - 视图漂移（TASK-110）：筛选视图分页化后切视图会整体替换 entries 并重置
      同键游标，旧视图的在途分页响应不得追加进新视图列表（游标数值可能恰好
      相等，须显式比较视图维度）。 */
export function paginationStale(
  atStart: { scopeKey: string; sort: 'newest' | 'oldest'; offset: number; view: ViewFilterType },
  now: { scopeKey: string; sort: 'newest' | 'oldest'; offset: number; view: ViewFilterType },
): boolean {
  return now.scopeKey !== atStart.scopeKey || now.offset !== atStart.offset
    || now.sort !== atStart.sort || now.view !== atStart.view;
}

/** TASK-109：筛选视图拉取（reloadFilteredEntries）的竞态守卫——具名化收口。
    范围×布局漂移使响应过期（另一个查询口径的列表）。
    TASK-110：排序维度入守卫——筛选视图分页化 + 切排序改为重拉后，排序由服务端
    承载，迟到旧排序响应不得覆盖新排序列表（旧「全集本地重排」语义下排序无关，
    该前提已随 TASK-110 废除）。 */
export function filteredSnapshotStale(
  atStart: { scopeKey: string; sort: 'newest' | 'oldest' },
  now: { scopeKey: string; sort: 'newest' | 'oldest' },
): boolean {
  return now.scopeKey !== atStart.scopeKey || now.sort !== atStart.sort;
}

export const QueryScope = {
  args: scopeQueryArgs,
  markScope: scopeFilterArgs,
  viewFilter: viewFilterArgs,
  pageKey: scopePageKey,
  viewKey: viewCacheKey,
  paginationStale,
  filteredSnapshotStale,
} as const;

/** 由 categories 构建 feedId → { feed, cat } 解析表（每次 categories 变更后重建） */
export function buildFeedIndex(categories: CategoryGroup[]) {
  const map = new Map<string, { feed: FeedItem; cat: CategoryGroup }>();
  for (const cat of categories) {
    for (const f of cat.feeds) map.set(f.id, { feed: f, cat });
  }
  return map;
}

/** categories 变更后统一收口：重建解析表 + 级联清理已无归属的条目 */
export function reconcileCategories(
  s: Pick<AppState, 'categories' | 'entries'>,
  nextCategories: CategoryGroup[],
): Pick<AppState, 'categories' | 'feedIndex' | 'entries'> {
  const index = buildFeedIndex(nextCategories);
  const entries = s.entries.filter((e) => index.has(e.feedId));
  return { categories: nextCategories, feedIndex: index, entries };
}

/** TASK-103（REQ-001）：快照替换时的正文/水合终态合并 —— 所有「entries 整体
    替换」的调用点（reloadFromBackend / reloadFilteredEntries / anchorToArticle /
    selectLayout·selectView·selectFeed 的缓存恢复）共用这**一份**实现，不得各自为政。

    缺陷：快照行从不携带正文（with_content 恒 false，见 bootstrap.layoutNeedsBody
    的实证），此前替换直接丢弃旧条目的正文并无条件清空 hydratedIds /
    hydrationErrors；而虚拟列表按文章 id 保持卡片身份、useLazyHydrate 同 id 不再
    重触发——卡片停留在「加载正文…」且无任何请求在途（审计探针复现的死区）。

    语义：
    - 按 id 继承旧条目的正文痕迹（content/rawContent/translatedContent/aiSummary/
      fulltextExtracted/hydrated，另含 url——它是详情行字段、列表行不带，不继承
      会让刷新后的「查看原文/全文提取」失效）；
    - 新行自带正文（with_content 场景）时以新行为准，旧值仅作缺省兜底；
    - hydratedIds / hydrationErrors 不再整体清空：按 id 裁剪，只保留仍存在于新
      快照中的标记（终态与正文一起继承，防止「空正文终态被清 → 卡片回退加载
      占位」；已消失条目的滞留标记移除，与 TASK-063 清滞留的契约同口径）。 */
export function mergeSnapshotEntries(
  prevEntries: ArticleEntry[],
  nextEntries: ArticleEntry[],
  prevHydratedIds: Record<string, true>,
  prevHydrationErrors: Record<string, string>,
  fromBackend: boolean,
): { entries: ArticleEntry[]; hydratedIds: Record<string, true>; hydrationErrors: Record<string, string> } {
  const prevById = new Map(prevEntries.map((a) => [a.id, a] as const));
  const entries = nextEntries.map((a) => {
    const prev = prevById.get(a.id);
    /* 无旧条目 / 新行自带正文（with_content）→ 以新行为准 */
    if (!prev || a.content) return a;
    /* 旧条目没有任何可继承的正文痕迹 → 原样返回（保持引用稳定，避免无谓重渲染） */
    if (!prev.content && !prev.hydrated && !prev.url && !prev.translatedContent && !prev.aiSummary && !prev.fulltextExtracted) {
      return a;
    }
    return {
      ...a,
      content: prev.content,
      rawContent: prev.rawContent,
      translatedContent: a.translatedContent || prev.translatedContent,
      aiSummary: a.aiSummary || prev.aiSummary,
      fulltextExtracted: a.fulltextExtracted || prev.fulltextExtracted,
      hydrated: prev.hydrated,
      url: a.url ?? prev.url,
    };
  });
  const surviving = new Set(nextEntries.map((a) => a.id));
  const hydratedIds: Record<string, true> = {};
  for (const id of Object.keys(prevHydratedIds)) {
    if (surviving.has(id)) hydratedIds[id] = true;
  }
  const hydrationErrors: Record<string, string> = {};
  for (const id of Object.keys(prevHydrationErrors)) {
    if (surviving.has(id)) hydrationErrors[id] = prevHydrationErrors[id];
  }
  /* TASK-109②：版本 bump 按真值来源收窄（TASK-107 R2 审查裁定）——
     - fromBackend=true（bootstrap 三处后端快照路径 reloadFromBackend /
       reloadFilteredEntries / anchorToArticle）：行级 is_read 是后端真值，任何
       在途乐观写入对这些 id 的回滚声明随之失效，bump 让迟到回滚全部跳过
       （否则回滚会把陈旧读态踩到新快照上、把刚重取的真值计数虚增回去）；
     - fromBackend=false（nav 三处缓存恢复路径 selectLayout / selectView /
       selectFeed）：缓存回放是近期 UI 状态而非后端真值——唯一不带后端真值的
       缓存行恰是乐观态本身（经 syncCurrentViewCache 落进缓存），不 bump 以
       保留本应正确的在途回滚；经 reload 落进缓存的行在 bootstrap merge 时已
       bump 过，此处不 bump 不会重开踩踏缺口（缓存恢复点随后必触发后台
       reload，其 fromBackend=true 的 merge 接手真值对齐）。 */
  if (fromBackend) for (const a of entries) bumpEntryVersion(a.id);
  return { entries, hydratedIds, hydrationErrors };
}

/** 把当前 entries 同步进「当前布局 × 当前视图」的缓存。
    乐观更新（标读/收藏/水合）只改 store.entries，缓存若不联动，切走视图再
    切回会用旧快照覆盖新状态（正文丢失、标读回退）。 */
export function syncCurrentViewCache(entries: ArticleEntry[]) {
  const s = appStore().getState();
  viewEntriesCache.set(viewCacheKey(s.activeContentLayout, s.activeViewFilter, s.activeFeedFilter), entries);
}

/* TASK-107 R1（F1）：条目变更版本号——乐观回滚的归属判定。
   既有「仅当当前值仍等于乐观写入值」的值守卫无法区分「用户已接管（同值覆盖
   写入）」与「未被触碰」：全部已读在途失败期间，用户对同一在册条目连点两次
   toggle 停在与乐观写入相同的值（其自身 set_read 已落库），迟到的回滚会把
   UI 踩回旧值而 DB 是新值（审查探针 C3 实测）。规则：任何真实的条目标志写入
   （flipEntryFlag / markEntriesRead / 快照替换 mergeSnapshotEntries）都必须
   bump 该条目版本，回滚方以「版本未变」为恢复前提。Map 随会话内被写过的
   条目增长（与 entries 同量级），无需清理。 */
const entryMutationVersion = new Map<string, number>();

/** 读条目当前变更版本（未被写过的条目为 0）。回滚方在乐观写入后快照各 id 的
    版本，失败回滚时仅恢复「版本仍相等」的条目。 */
export function getEntryVersion(id: string): number {
  return entryMutationVersion.get(id) ?? 0;
}

function bumpEntryVersion(id: string): void {
  entryMutationVersion.set(id, getEntryVersion(id) + 1);
}

/** 乐观更新某篇条目的 isRead/isStarred，并同步 feedCounts 的未读/收藏计数。
    侧边栏数字基于 feedCounts（后端精确计数），若不联动，标读/收藏后角标
    不立即变化（与乐观更新的列表脱节）。total/today 不受影响。
    TASK-107 核查结论（REQ-003 单条口径）：后端 set_read/record_read_state 只
    翻转该行自身（UPDATE articles ... WHERE id = ?），feed_counts 按「每行
    is_read=0」聚合——本函数按该行所属 feed 恰好 ±1 与后端口径一致；同文副本
    （跨源同 guid 的行）在后端各自独立成行、互不联动（本地传播不存在，仅同步
    推送广播到远端，属 TASK-107 non_goals），前端也只动主条目所属源（t104
    断言锁定：标读后前端计数 == 后端按行聚合）。
    TASK-107 R1：每次真实翻转都 bump 条目版本（entryMutationVersion），供
    多条目乐观操作（全部已读）的失败回滚做归属判定。 */
export function flipEntryFlag(id: string, field: 'isRead' | 'isStarred') {
  const s = appStore().getState();
  const entry = s.entries.find((e) => e.id === id);
  if (!entry) return;
  const nextVal = !entry[field];
  const entries = s.entries.map((e) => (e.id === id ? { ...e, [field]: nextVal } : e));
  bumpEntryVersion(id);
  const c = s.feedCounts.get(entry.feedId);
  let feedCounts = s.feedCounts;
  if (c) {
    const key = field === 'isRead' ? 'unread' : 'starred';
    const delta = field === 'isRead' ? (nextVal ? -1 : 1) : (nextVal ? 1 : -1);
    feedCounts = new Map(s.feedCounts);
    feedCounts.set(entry.feedId, { ...c, [key]: Math.max(0, c[key] + delta) });
  }
  appStore().setState({ entries, feedCounts });
  /* 同步当前视图缓存：避免切走再切回时，缓存恢复旧状态（标读/收藏回退闪烁） */
  syncCurrentViewCache(entries);
}

/** 批量标已读：同步 feedCounts 的未读计数（每篇 -1）。
    高效实现：一次遍历 entries 构建新数组，一次聚合 feedId 的未读减少数，
    避免循环内多次 Map 复制 / entries.map（「全部已读」几百篇时 O(n²) 卡顿）。
    TASK-107 核查结论：单条/批量标读路径（set_read / set_read_bulk）后端都是
    逐 id 翻转自身行（apply_read_bulk = 循环 record_read_state），本函数按被
    翻转行逐 feed 聚合 -1 与后端口径一致；「整个范围一起标读」的 mark_all_read
    不走本函数做计数（范围总量前端不可知），由 markCurrentViewAllRead 成功后
    重取 feed_counts 对账（t104 断言锁定）。
    TASK-107 R1：每次真实翻转都 bump 条目版本（entryMutationVersion）——
    全部已读的乐观写入本身也走这里，其失败回滚以「版本未变」为恢复前提。 */
export function markEntriesRead(ids: Set<string>) {
  const s = appStore().getState();
  let entries = s.entries;
  const unreadDeltas = new Map<string, number>();
  let changed = false;
  // 一次遍历：标记 entries + 聚合每个 feed 的未读减少数
  entries = entries.map((e) => {
    if (ids.has(e.id) && !e.isRead) {
      changed = true;
      bumpEntryVersion(e.id); // TASK-107 R1：真实翻转必 bump 版本（回滚归属判定）
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
  appStore().setState({ entries, feedCounts });
  syncCurrentViewCache(entries);
}

/** F2/F3（Batch 1/2 独立审查 P3）收口：乐观标志写的唯一实现——卡片
    （toggleEntryFlag）与阅读器（toggleCurrentReadStatus / toggleCurrentStar）共用。
    - 乐观翻转立即生效（点下去不等落库，flipEntryFlag 连动 feedCounts 与视图缓存）；
    - 失败时**仅当当前值仍等于乐观写入值**才恢复点击前值：回滚若是「再翻一次当前值」，
      连点两次且第一次失败、第二次成功时，第一次的迟到 catch 会把第二次已落库的
      新值再踩回旧值——UI 与 DB 脱节（审查探针实测 UI isRead=true / DB is_read=false）；
    - 失败 toast 由调用方给文案模板（保持各入口既有文案）；成功提示（若有）通过
      onSuccess 在**落库成功后**出现——与 P1-5「去假成功」同口径，不得提前乐观弹。 */
export function optimisticEntryFlagToggle(
  id: string,
  field: 'isRead' | 'isStarred',
  request: (next: boolean) => Promise<unknown>,
  failureText: (msg: string) => string,
  onSuccess?: (next: boolean) => void,
): void {
  const s = appStore().getState();
  const entry = s.entries.find((e) => e.id === id);
  if (!entry) return;
  const prev = entry[field];
  const optimistic = !prev;
  flipEntryFlag(id, field);
  void request(optimistic).then(() => {
    onSuccess?.(optimistic);
  }).catch((e: unknown) => {
    const cur = appStore().getState().entries.find((x) => x.id === id);
    /* 仅当当前值仍等于乐观写入值时才恢复原值；否则后续点击已接管状态，只提示 */
    if (cur && cur[field] === optimistic) flipEntryFlag(id, field);
    appStore().getState().showToast(failureText(extractError(e)));
  });
}
