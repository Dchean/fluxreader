import type { StoreApi } from 'zustand';
import { extractError } from '../lib/api';
import type { ArticleListItemRow } from '../lib/api';
import type {
  ArticleEntry,
  CategoryGroup,
  ContentLayoutType,
  FeedItem,
  ViewFilterType,
} from '../types';
import type { AppState, ArticlesCursorState } from './types';

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

/** TASK-111①：单键实体预算（审计：「单纯限制为 8 个视图并不能限制每个视图的大小」——
    TASK-100 P3-7 只封了键数上界，TASK-110 分页化后单键 entries 仍随「滚动加载」
    无界增长：1200 条的库滚三页，该键就背 1200 个条目对象，8 键 × 无上界 = 内存
    回到无界形态）。缓存语义 = 首屏快照（命中即零延迟显示、随后必触发后台 reload
    重取真值），故超限从**尾部截断**安全：丢的是最老端条目，恢复后的后台刷新会
    立即补齐完整口径。
    取值 1000 ≥ ARTICLES_PAGE_SIZE(500)：截断只发生在「单键加载超过 2 页」的
    大库场景，首批/次页快照永不截断；预算与页大小的关系在 t111-* 断言中锁定。
    截断的正确性边界（exhausted/续拉衔接）见 setViewEntriesSnapshot 与缓存值
    元数据（ViewEntriesSnapshot）——恢复时用**记录值**而非截断长度判定，避免
    截断长度落在 PAGE_SIZE 边界附近造成 exhausted 误判。 */
export const VIEW_ENTRIES_CACHE_ENTRY_BUDGET = 1000;

/** TASK-111①：视图缓存值 = 条目快照 + 分页元数据。
    TASK-117：cursor = 写入时该 scope 的 keyset 游标（ArticlesCursorState，原
    loadedCount 的超集：loaded 字段承接原「已加载总数」语义，lastPublished/lastId
    是续拉锚）。恢复路径（nav 三处缓存命中）必须用这两个**记录值**恢复游标与
    exhausted，不得用截断后的 entries.length 重算——截断后长度若 < 页大小
    （预算更小的未来取值）或边界相邻，都会把「还有数据」误判成「已到底」（或反），
    且续拉锚会回退重拉已去重丢弃的区间。 */
export interface ViewEntriesSnapshot {
  entries: ArticleEntry[];
  cursor: ArticlesCursorState;
  exhausted: boolean;
}

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

export const viewEntriesCache: Map<string, ViewEntriesSnapshot> = new LRUMap<ViewEntriesSnapshot>(VIEW_ENTRIES_CACHE_MAX);

/** TASK-111①：视图缓存写入唯一收口（三个写入点共用：reloadFromBackend /
    reloadFilteredEntries / syncCurrentViewCache）——单键实体预算在此一处执行，
    超限尾部截断后连同分页元数据（cursor / exhausted，取写入时真值）落键。
    TASK-117：第二参从 loadedCount 改为完整 keyset 游标；loaded 以
    max(记录值, 截断长度) 兜底：正常路径记录值 ≥ 截断长度（entries ⊆ 已加载
    窗口）；mock 模式无游标（articlesCursor 空表），调用方以
    emptyArticlesCursor(articlesLimit) 兜底，让恢复游标退回「快照长度」——与
    预算引入前的恢复行为逐字一致。 */
export function setViewEntriesSnapshot(key: string, entries: ArticleEntry[], cursor: ArticlesCursorState, exhausted: boolean): void {
  const trimmed = entries.length > VIEW_ENTRIES_CACHE_ENTRY_BUDGET ? entries.slice(0, VIEW_ENTRIES_CACHE_ENTRY_BUDGET) : entries;
  viewEntriesCache.set(key, { entries: trimmed, cursor: { ...cursor, loaded: Math.max(cursor.loaded, trimmed.length) }, exhausted });
}

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

   TASK-117（审计 P1-1）：游标值从「已加载条数（OFFSET）」改为 keyset 锚
   （ArticlesCursorState：lastPublished/lastId/loaded）——可变筛选集合上 OFFSET
   不等价于已看条数（读 500 标读后集合剩 700，下一页仍 OFFSET 500 → 跳过 500 篇
   并假 exhausted）。锚 = 已加载窗口最后一行的 (published_at 原文, id)，续拉请求
   「严格排在锚之后」，与集合增删无关。

   本模块把「查询口径」收口成一处：
   - scopeQueryArgs(scope, sort)：订阅范围 + 排序 → 后端参数（feed_id/folder_id
     /newest_first），与 anchorToArticle / markCurrentViewAllRead 同口径；
   - scopePageKey(scope, view)：分页游标键 —— 筛选口径可独立翻页，但共享
     entries，故游标键**只取订阅范围**（feed/分类），不含视图与排序。
   ============================================================ */

/** TASK-117：空游标（无 keyset 锚：lastPublished/lastId 为 null，仅计 loaded）。
    首屏 / mock 模式 / 游标缺省回落用。 */
export function emptyArticlesCursor(loaded = 0): ArticlesCursorState {
  return { lastPublished: null, lastId: null, loaded };
}

/** TASK-117：由后端行推进 keyset 游标——锚 = 最后一行的 (published_at 原文, id)。
    base：续拉传游标现值（loaded 累加），首屏传 emptyArticlesCursor()。
    空页保留原锚（loaded 不变）：空页即到底（fetched < PAGE_SIZE → exhausted），
    该锚不会再被消费；保留原值使游标仍是「已看过的最后一篇」。 */
export function advanceArticlesCursor(base: ArticlesCursorState, rows: ArticleListItemRow[]): ArticlesCursorState {
  const last = rows.length ? rows[rows.length - 1] : null;
  return {
    lastPublished: last ? (last.published_at ?? null) : base.lastPublished,
    lastId: last ? last.id : base.lastId,
    loaded: base.loaded + rows.length,
  };
}

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
    - 排序漂移：keyset 锚的含义随排序翻转（F1——同锚点在 newest/oldest 下指向
      不同的后续集合），旧排序的响应属于另一查询口径；
    - 视图漂移（TASK-110）：筛选视图分页化后切视图会整体替换 entries 并重置
      同键游标，旧视图的在途分页响应不得追加进新视图列表（锚点数值可能恰好
      相等，须显式比较视图维度）。
    TASK-117：游标从单值 offset 改为 keyset 三元组，任一字段漂移都判过期
    （锚点被推进 / reload 重置，两种竞态都覆盖）。 */
export function paginationStale(
  atStart: { scopeKey: string; sort: 'newest' | 'oldest'; cursor: ArticlesCursorState; view: ViewFilterType },
  now: { scopeKey: string; sort: 'newest' | 'oldest'; cursor: ArticlesCursorState; view: ViewFilterType },
): boolean {
  return now.scopeKey !== atStart.scopeKey
    || now.cursor.lastPublished !== atStart.cursor.lastPublished
    || now.cursor.lastId !== atStart.cursor.lastId
    || now.cursor.loaded !== atStart.cursor.loaded
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
       reload，其 fromBackend=true 的 merge 接手真值对齐）。
     TASK-118：后端快照路径对 isRead 与 isStarred **两字段都** bump——快照
     整体替换携带的是后端行级真值，is_read 与 is_starred 同时被覆盖，两字段
     的在途乐观声明一并失效；且 bump 收敛到字段键后不再有跨字段误伤（此前
     文章级版本下，任一字段的快照替换都会 void 掉另一字段的在途声明）。 */
  if (fromBackend) {
    for (const a of entries) {
      bumpEntryVersion(a.id, 'isRead');
      bumpEntryVersion(a.id, 'isStarred');
    }
  }
  return { entries, hydratedIds, hydrationErrors };
}

/** 把当前 entries 同步进「当前布局 × 当前视图」的缓存。
    乐观更新（标读/收藏/水合）只改 store.entries，缓存若不联动，切走视图再
    切回会用旧快照覆盖新状态（正文丢失、标读回退）。
    TASK-111①：经 setViewEntriesSnapshot 收口——同一单键实体预算 + 元数据。
    TASK-117：元数据取当前 scope 的 keyset 游标现值（原 articlesLimit 镜像的
    超集）；mock 模式无游标（articlesCursor 空表），以 emptyArticlesCursor
    (articlesLimit) 兜底，loaded 由收口内的 max 兜底回快照长度（恢复行为不变）。 */
export function syncCurrentViewCache(entries: ArticleEntry[]) {
  const s = appStore().getState();
  const cursor = s.articlesCursor[QueryScope.pageKey(s.activeFeedFilter, s.activeContentLayout)]
    ?? emptyArticlesCursor(s.articlesLimit);
  setViewEntriesSnapshot(
    viewCacheKey(s.activeContentLayout, s.activeViewFilter, s.activeFeedFilter),
    entries,
    cursor,
    s.articlesExhausted,
  );
}

/* TASK-107 R1（F1）：条目变更版本号——乐观回滚的归属判定。
   既有「仅当当前值仍等于乐观写入值」的值守卫无法区分「用户已接管（同值覆盖
   写入）」与「未被触碰」：全部已读在途失败期间，用户对同一在册条目连点两次
   toggle 停在与乐观写入相同的值（其自身 set_read 已落库），迟到的回滚会把
   UI 踩回旧值而 DB 是新值（审查探针 C3 实测）。规则：任何真实的条目标志写入
   （flipEntryFlag / markEntriesRead / 快照替换 mergeSnapshotEntries）都必须
   bump 对应字段版本，回滚方以「版本未变」为恢复前提。Map 随会话内被写过的
   条目增长（与 entries 同量级，键为 条目×字段 至多两倍），无需清理。
   TASK-118（审计 P1-2）：版本键从「文章级共享」升级为 articleId × field——
   isRead / isStarred 各自独立单调。修前两字段共用同一文章版本：全部已读在途
   时用户收藏该文，收藏（flipEntryFlag isStarred）bump 使读状态的迟到回滚被
   误判成「已被接管」而跳过（审计探针实测：应 isRead=false/unread=1，实际
   isRead=true/isStarred=true/unread=0）——收藏并没有接管 isRead，字段间不得
   互相失效。键形态 `${id}|${field}`：id 是数字字符串、field 名不含 `|`，无歧义。 */
const entryMutationVersion = new Map<string, number>();

/** TASK-118：版本键 = articleId × field。 */
const entryVersionKey = (id: string, field: 'isRead' | 'isStarred'): string => `${id}|${field}`;

/** 读条目当前字段变更版本（未被写过的条目/字段为 0）。回滚方在乐观写入后
    快照各 (id, field) 的版本，失败回滚时仅恢复「版本仍相等」的字段声明。 */
export function getEntryVersion(id: string, field: 'isRead' | 'isStarred'): number {
  return entryMutationVersion.get(entryVersionKey(id, field)) ?? 0;
}

function bumpEntryVersion(id: string, field: 'isRead' | 'isStarred'): void {
  const key = entryVersionKey(id, field);
  entryMutationVersion.set(key, (entryMutationVersion.get(key) ?? 0) + 1);
}

/* ============================================================
   TASK-119（审计 P2-4②）：本地读/藏写入序号——计数对账的过期判据。

   feed_counts 对账（nav.markCurrentViewAllRead 成功路径）的响应在途期间，任何
   本地读/藏写入都会使「发起时的计数快照」对当前状态过期：对账整体替换会把乐观
   计数踩回旧值（审计探针 P6：全部已读成功 → 对账在途 → 用户改回未读 → 迟到计数
   落地 → 文章未读但未读数 0）。

   写入点矩阵（bumpLocalFlagWrite 的全部调用点）：
   - flipEntryFlag：单条 toggle（卡片/阅读器；乐观回滚的重翻同径）——每次真实
     翻转（isRead 与 isStarred 都算：收藏数同样由对账整体替换承载）；
   - markEntriesRead：批量乐观标读（markEntriesReadBulk / markCurrentViewAllRead
     的乐观段 / 播放器播完标读）——仅实际翻转时（changed 早退之后）；
   - rollbackEntryClaims：失败回滚（TASK-118 统一助手，乐观 toggle /
     markEntriesReadBulk / selectArticle·anchorToArticle 打开即标读的回滚路径）
     ——实际恢复时。
   markCurrentViewAllRead 经 markEntriesRead（乐观段）覆盖，不单独 bump；其失败
   回滚是 nav 内联恢复（不经 rollbackEntryClaims），但只恢复**自身乐观段已翻转**
   的条目（changed 早退两侧对称）——回滚发生 ⇒ 同一操作的乐观段必已 bump，在途
   对账已因该 bump 过期，无需为回滚单独 bump。mergeSnapshotEntries 是后端真值
   落地、非本地未确认写，不 bump（其伴随的 reload 自带同源计数，不存在对账窗口）。
   序号只单调不清理：与会话内旗标写入同量级，消费方只做相等比较，无溢出顾虑。
   ============================================================ */
let localFlagWriteSerial = 0;

/** TASK-119：本地读/藏真实写入（翻转或恢复）时推进序号。 */
function bumpLocalFlagWrite(): void {
  localFlagWriteSerial += 1;
}

/** TASK-119：读当前本地读/藏写入序号（对账发起时快照、落地时比对）。 */
export function currentLocalFlagWriteSerial(): number {
  return localFlagWriteSerial;
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
    多条目乐观操作（全部已读）的失败回滚做归属判定。
    TASK-118：按旗标类型 bump 对应字段版本（isRead / isStarred 各自单调）——
    收藏只进 isStarred，不再使同文章在途的读状态回滚失效（审计 P1-2）。 */
export function flipEntryFlag(id: string, field: 'isRead' | 'isStarred') {
  const s = appStore().getState();
  const entry = s.entries.find((e) => e.id === id);
  if (!entry) return;
  const nextVal = !entry[field];
  const entries = s.entries.map((e) => (e.id === id ? { ...e, [field]: nextVal } : e));
  bumpEntryVersion(id, field); // TASK-118：按旗标类型 bump 对应字段版本
  bumpLocalFlagWrite(); // TASK-119：本地读/藏写入（计数对账过期判据，矩阵见上）
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
    全部已读的乐观写入本身也走这里，其失败回滚以「版本未变」为恢复前提。
    TASK-118：标读只 bump isRead 字段版本（收藏写入不再牵连本字段的回滚判定）。 */
export function markEntriesRead(ids: Set<string>) {
  const s = appStore().getState();
  let entries = s.entries;
  const unreadDeltas = new Map<string, number>();
  let changed = false;
  // 一次遍历：标记 entries + 聚合每个 feed 的未读减少数
  entries = entries.map((e) => {
    if (ids.has(e.id) && !e.isRead) {
      changed = true;
      bumpEntryVersion(e.id, 'isRead'); // TASK-107 R1 + TASK-118：真实翻转必 bump isRead 字段版本（回滚归属判定）
      unreadDeltas.set(e.feedId, (unreadDeltas.get(e.feedId) ?? 0) + 1);
      return { ...e, isRead: true };
    }
    return e;
  });
  if (!changed) return;
  bumpLocalFlagWrite(); // TASK-119：本地批量读态写入（markEntriesReadBulk / 全部已读乐观段共用此径）
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

/** TASK-118：乐观写失败的统一回滚助手——三条乐观路径共用这**一份**实现，
    不得各写一份（审计相邻缺口：修前单条 toggle 走值比较、批量标读与打开即
    标读失败只提示不回滚，「所有乐观写入都有统一版本回滚」不成立）：
    - 单条 toggle：optimisticEntryFlagToggle（卡片 / 阅读器 / 标读与收藏）；
    - 批量标读：markEntriesReadBulk（滚动标读 / 全部已读的本地路径 / 播放器）；
    - 打开即标读：selectArticle 与 anchorToArticle 的 markReadOnOpen。

    声明形态（调用方在乐观写入**落地后**快照）：id + 字段 + 写入前值 + 该字段
    版本。恢复守卫两道、顺序固定：
    - 值等快速短路：当前值已等于回滚目标（prev）→ 无需动、也不动计数；
    - 版本守卫（TASK-107 R1 语义，TASK-118 起按字段比较）：该字段版本 ≠ 快照值
      → 期间已被其他真实写入接管，跳过——迟到回滚不得踩掉用户已落库的最终意图
      （审查探针 C3），也不得被**另一字段**的写入 void（审计 P1-2）。
    计数回补：按行恢复方向逐 feed 聚合，与 flipEntryFlag 的乐观方向互逆
    （isRead 恢复未读 → unread +1、恢复已读 → unread -1；isStarred 对称），
    Math.max(0,) 钳制与 markCurrentViewAllRead 的回滚形态一致。
    返回是否有行被恢复：无恢复不写 store、不刷视图缓存（与「回滚跳过 ≠ 吞错」
    分离——失败提示始终由调用方给出）。 */
export interface EntryRollbackClaim {
  id: string;
  field: 'isRead' | 'isStarred';
  /** 乐观写入前的值（回滚目标） */
  prev: boolean;
  /** 乐观写入 bump 完成后快照的字段版本 */
  version: number;
}

export function rollbackEntryClaims(claims: readonly EntryRollbackClaim[]): boolean {
  if (claims.length === 0) return false;
  const s = appStore().getState();
  const claimByKey = new Map(claims.map((c) => [entryVersionKey(c.id, c.field), c] as const));
  let changed = false;
  const unreadRestore = new Map<string, number>();
  const starredRestore = new Map<string, number>();
  const entries = s.entries.map((a) => {
    let next = a;
    const readClaim = claimByKey.get(entryVersionKey(a.id, 'isRead'));
    if (readClaim && a.isRead !== readClaim.prev && getEntryVersion(a.id, 'isRead') === readClaim.version) {
      changed = true;
      next = { ...next, isRead: readClaim.prev };
      unreadRestore.set(a.feedId, (unreadRestore.get(a.feedId) ?? 0) + (readClaim.prev ? -1 : 1));
    }
    const starClaim = claimByKey.get(entryVersionKey(a.id, 'isStarred'));
    if (starClaim && a.isStarred !== starClaim.prev && getEntryVersion(a.id, 'isStarred') === starClaim.version) {
      changed = true;
      next = { ...next, isStarred: starClaim.prev };
      starredRestore.set(a.feedId, (starredRestore.get(a.feedId) ?? 0) + (starClaim.prev ? 1 : -1));
    }
    return next;
  });
  if (!changed) return false;
  bumpLocalFlagWrite(); // TASK-119：回滚恢复也是本地读/藏写入（失败回滚后使在途计数对账过期）
  let feedCounts = s.feedCounts;
  const applyDelta = (feedId: string, key: 'unread' | 'starred', delta: number) => {
    const c = feedCounts.get(feedId);
    if (!c) return;
    if (feedCounts === s.feedCounts) feedCounts = new Map(s.feedCounts); // 惰性复制
    feedCounts.set(feedId, { ...c, [key]: Math.max(0, c[key] + delta) });
  };
  for (const [fid, delta] of unreadRestore) applyDelta(fid, 'unread', delta);
  for (const [fid, delta] of starredRestore) applyDelta(fid, 'starred', delta);
  appStore().setState({ entries, feedCounts });
  syncCurrentViewCache(entries);
  return true;
}

/** F2/F3（Batch 1/2 独立审查 P3）收口：乐观标志写的唯一实现——卡片
    （toggleEntryFlag）与阅读器（toggleCurrentReadStatus / toggleCurrentStar）共用。
    - 乐观翻转立即生效（点下去不等落库，flipEntryFlag 连动 feedCounts 与视图缓存）；
    - 失败回滚的归属判定：修前是「仅当当前值仍等于乐观写入值」（回滚若是
      「再翻一次当前值」，连点两次且第一次失败、第二次成功时，第一次的迟到
      catch 会把第二次已落库的新值再踩回旧值——UI 与 DB 脱节，审查探针实测）；
      TASK-107 R1 起为版本守卫，TASK-118 起收口到统一助手 rollbackEntryClaims
      （字段级版本守卫 + 值等快速短路）——与批量标读、打开即标读同一份实现；
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
  /* TASK-118：乐观翻转（flipEntryFlag 已 bump 对应字段）后快照该字段版本，
     失败交给统一回滚助手——跨字段写入（如收藏）不再使本字段的回滚失效。 */
  const claim: EntryRollbackClaim = { id, field, prev, version: getEntryVersion(id, field) };
  void request(optimistic).then(() => {
    onSuccess?.(optimistic);
  }).catch((e: unknown) => {
    rollbackEntryClaims([claim]);
    appStore().getState().showToast(failureText(extractError(e)));
  });
}
