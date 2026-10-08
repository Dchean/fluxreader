// Note: 水合触发判定读实体缓存状态而非「快照是否被替换过」；刷新不丢正文 — 见 .agents/notes/implemented/bug-fix/2026-10-05-正文水合生命周期统一.md
import type { StateCreator } from 'zustand';
import { createInitialCategories, createInitialEntries } from '../../mockData';
import { api, articleRowToEntry, extractError, folderRowsToCategories } from '../../lib/api';
import type { ArticleListItemRow } from '../../lib/api';
import { advanceArticlesCursor, ARTICLES_PAGE_SIZE, appStore, buildFeedIndex, emptyArticlesCursor, getEntryVersion, markEntriesRead, mergeSnapshotEntries, QueryScope, reconcileCategories, rollbackEntryClaims, setViewEntriesSnapshot } from '../internals';
import type { EntryRollbackClaim } from '../internals';
import type { AppState, ArticlesCursorState } from '../types';
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
  | 'articlesCursor'
  | 'positionRestoreNonce'
  | 'reloadFromBackend'
  | 'loadMoreArticles'
  | 'reloadFilteredEntries'
  | 'anchorToArticle'
  | 'bootstrapFromBackend'
  | 'retryBootstrap'
>;

/* 文章列表分页大小：首批/每次滚动加载拉取的文章数。
   TASK-110：常量收口到 internals（筛选视图分页化后与「全部」视图共用同一页大小
   与游标口径，nav.ts 的缓存恢复 exhausted 判定也需要它），此处 import 使用。 */

/* ============================================================
   TASK-119（审计 P2-4①）：查询实例代际——统一收口（演化自原 reloadGeneration）

   缺陷：过期判断只比较查询参数（QueryScope 谓词收口了口径但没消除「同一查询
   的旧版本」窗口）——同 scope/view/sort 两次发起、新响应先落、旧响应后至，旧
   响应因「参数全同」被放行覆盖新结果（探针 P5）；A→B→A 同理不设防。审计要求
   每个查询实例维护 generation，首屏/续页/刷新/导航共享同一套过期判断。

   机制（双层级，单一体系内的两个粒度）：
   - queryGeneration：全局单调序号——**全局状态/窗口写入者**（reloadFromBackend
     写 categories/feedCounts/entries 全局态；anchorToArticle 整体替换当前窗口）
     的落地判据是「此后没有任何更新的查询发起」。纯 per-key 判据在这里不够：
     跨范围两次 reload 是不同 queryKey，旧范围响应会在新范围落地后 pass 并把
     全局态踩回旧值且无自愈（B 已落地不会再有查询覆盖）——这正是原
     reloadGeneration 的既有保护（范围切换场景），统一后原样保留。
   - firstPageSerialByKey：Map<queryKey, 序号>——queryKey 复用 TASK-117 键族
     （viewKey = pageKey×视图口径，布局×范围×视图；排序不入键：同键换序的旧
     响应由排序守卫与全局代际兜底）。记录该键最近一次「首屏类发起」的序号，
     消费方有二：
     ① reloadFilteredEntries 的落地判据——同键两次发起、先发后至的旧版本被
        识别丢弃（P5/A→B→A 修复；视图快照只写本键，不需要全局粒度，且发起
        不推全局代际——保留「后台刷新的 all 过渡落地 + 自链筛选重拉」价值，
        用户切视图不使在途后台刷新整体作废）；
     ② loadMoreArticles 续页携带的「首页代际」——续页不是新查询实例（不 bump，
        无锚回落 offset 的退化形态同样是续期），携带发起时该键现值（= 建立当前
        窗口的首屏代际；期间无更新首屏时二者恒等），落地要求键现值未变 +
        paginationStale 三元组守卫保留——新首屏落地后游标三元组恰好全等（同数据
        重拉）时参数比较不可判别，代际补上这一层（探针 P4 形态）。
   ============================================================ */
let queryGeneration = 0;
const firstPageSerialByKey = new Map<string, number>();

/** per-key 首屏代际记录：每次首屏类发起推进该键序号（键内单调）。 */
function recordFirstPageStart(queryKey: string): number {
  const serial = (firstPageSerialByKey.get(queryKey) ?? 0) + 1;
  firstPageSerialByKey.set(queryKey, serial);
  return serial;
}

/** 全局状态/窗口写入者发起（reloadFromBackend / anchorToArticle）：推进全局
    代际 + 记录该键首屏代际（使该键在途的筛选/续页响应过期）。返回全局代际。 */
function beginGlobalQuery(queryKey: string): number {
  queryGeneration += 1;
  recordFirstPageStart(queryKey);
  return queryGeneration;
}

/** 视图快照写入者发起（reloadFilteredEntries）：仅推进该键首屏代际，不推全局
    ——理由见上方机制说明（切视图不得使在途后台刷新的全局态更新作废；同键
    旧版本由 per-key 判据识别）。返回该键代际。 */
function beginViewQuery(queryKey: string): number {
  return recordFirstPageStart(queryKey);
}

/** 该键当前首屏代际（续页发起时读取 = 首页代际；落地时读取 = 现值）。 */
function currentFirstPageSerial(queryKey: string): number {
  return firstPageSerialByKey.get(queryKey) ?? 0;
}

/** TASK-117：首屏（offset=0）写入的 per-scope keyset 游标——锚取本页最后一行
    （advanceArticlesCursor），loaded 累加到 base。续拉锚从此推进，见
    loadMoreArticles。 */
function cursorAfterFirstPage(rows: ArticleListItemRow[]): ArticlesCursorState {
  return advanceArticlesCursor(emptyArticlesCursor(), rows);
}

/* ============================================================
   TASK-123（审计 P2-5①②）：保位窗口重取——keepReadingPosition 路径的
   刷新不再只拉首屏。

   审计探针 P7：加载 1000 篇读到第 750 篇，后台刷新固定 offset=0/limit=500
   → 列表缩回前 500，顶条锚（id 在第 750 篇）丢失——anchorRestoreIndex 返回
   null。注意这不是文章删除，而是**刷新策略主动丢弃已加载窗口**（TASK-117
   keyset 化后 loadMoreArticles 续拉已正确，但 reload 首屏路径仍缩窗）。

   修法（在 TASK-111② keepReadingPosition 信号 + TASK-117 keyset 游标 +
   TASK-119 查询代际之上扩展，不推翻）：keepReadingPosition 的 reload 以
   「发起时的游标 loaded」为窗口目标、「游标窗口底锚 lastId」为底边界，
   keyset 分页循环重取 [首屏..窗口末]，一次性原子落地（插头部使窗口按实际
   插入量自然生长——只拉满旧长度会把窗口尾部挤出重取范围，深页锚照样丢失，
   见 fetchWindowRows 头注「窗口底边界」）。锚 id 在窗口内 ⇒ 刷新后必在
   （除非真被删除/被筛出——那是真丢失，回落现状的既有语义不变）。

   与查询代际的协同（TASK-119）：窗口重取是**同一查询实例的多次分页取数**
   ——发起时 beginGlobalQuery/beginViewQuery 已 bump 并记录首屏代际，续页
   与 loadMoreArticles 同理**不再 bump**（续页不是新查询实例）；每页 await
   后由调用方经 isStale 复核代际/口径，任何更新的查询发起（用户导航、
   同键筛选）都使整个实例（含已取页）作废——守卫粒度与单页形态逐字一致。

   与 TASK-117 的协同：续页请求 = loadMoreArticles 同一 keyset 形态
   （last_published/last_id 成对，严格排在锚之后），游标终值 = 累计行末行锚
   （advanceArticlesCursor 口径），续拉无缝衔接；终页不足一页 = exhausted。

   窗口底边界（bottomId）——为什么「长度达标」不够（TASK-123 断言 t123-1 的
   判别核心）：刷新常伴随新文章插头部，窗口整体后移；若只拉满「发起时 loaded」
   条，新插入的 N 行会把旧窗口**尾部 N 行挤出重取范围**——锚恰在其间的深页
   读者照样丢锚（探针 P7 换个形态复发）。因此续页循环的继续条件是「长度未达
   目标 **或** 发起时窗口底锚（游标 lastId）尚未重取到」——插头部使窗口按插入
   量自然生长，锚 id 必在（除非真被删除/被筛出）。keyset 语义保证终止与
   exhausted 判定仍然精确：短页 = 集合真末尾（bottomId 提前命中只会发生在
   满页后，此时 lastPageCount = PAGE_SIZE → exhausted=false，集合若真尽于
   该页则页短 → true，与单页形态同口径）。

   与 TASK-122 的协同：rowsFetchedAt 取**发起时刻**（首屏抓取前）——窗口内
   任何一行的「行真值时刻」都不早于它，bodyById 对齐的新鲜度判据按最保守
   口径成立（记录晚于发起时刻的写入不回退）。

   内存预算：窗口目标 = 刷新前真实 loaded（≤ 用户实际加载的页数），不做
   无界重取；插头部只使窗口按**实际插入量**生长（底锚命中即收敛）；防御性
   页数上限仅防后端病态（全页恒返/底锚被删时的无限追取），触限时按已取行
   落地（极端形态下锚丢失回落现状 = 与「真被删除」同口径）。单页失败（含
   IPC 异常）向上抛给调用方既有 catch（toast + 旧 entries 原样保留——
   **失败的刷新同样不缩窗**，保位语义对失败路径天然成立）。 */
async function fetchWindowRows(
  baseArgs: Record<string, unknown>,
  firstRows: ArticleListItemRow[],
  windowTarget: number,
  bottomId: number | null,
  isStale: () => boolean,
): Promise<{ rows: ArticleListItemRow[]; exhausted: boolean }> {
  const rows = firstRows.slice();
  let lastPageCount = firstRows.length;
  let pages = 1;
  /* 防御性页数上限：目标页数 + 1（keyset 严格递进 + 终页不足一页保证终止，
     上限仅防后端病态全页返回 / 底锚被删后的无界追取）。 */
  const maxPages = Math.ceil(windowTarget / ARTICLES_PAGE_SIZE) + 1;
  /* 继续条件（命中其一即停）：代际过期 / 末页不满（集合尽）/ 页数触限 /
     长度达标且底锚已覆盖（窗口完整重取）。 */
  while (
    !isStale()
    && lastPageCount >= ARTICLES_PAGE_SIZE
    && pages <= maxPages
    && (rows.length < windowTarget || (bottomId != null && !rows.some((r) => r.id === bottomId)))
  ) {
    const last = rows[rows.length - 1];
    const next = await api.listArticles({
      ...baseArgs,
      limit: ARTICLES_PAGE_SIZE,
      /* TASK-117：keyset 续页锚（与 loadMoreArticles 同形态；OFFSET 停用） */
      last_published: last.published_at,
      last_id: last.id,
    });
    if (!next || next.length === 0) { lastPageCount = 0; break; } // 空页（含浏览器 mock null）→ 集合尽，停止重取
    rows.push(...next);
    lastPageCount = next.length;
    pages += 1;
  }
  return { rows, exhausted: lastPageCount < ARTICLES_PAGE_SIZE };
}

/** TASK-100 P3-1：后端 reload（全量/筛选）在途计数。
 *  selectLayout/selectFeed/selectView 都是「先写游标镜像、再异步 reload」：
 *  该窗口内 entries 仍是旧口径快照，若放行 loadMoreArticles（含自动续拉），
 *  新口径的一页会被 append 到旧口径列表尾（瞬态错排，随后才被 reload 整体替换）。
 *  在途期间一律拦截续拉——reload 落地时 entries 与游标原子对齐，之后哨兵/
 *  refill effect 随 items 变化重新触发，不会丢加载。 */
let backendReloadInFlight = 0;

/** 是否有后端 reload 在途（loadMoreArticles 的入口守卫之一） */
export function isBackendReloadInFlight(): boolean {
  return backendReloadInFlight > 0;
}

/** 判断列表查询是否附带正文。虚拟滚动下仅视口约 30 条需要正文，由
    useLazyHydrate 按需批量水合（1 次 IPC）即可；列表查询保持轻量（不含
    正文 HTML），避免每页 500 条背 2-3MB 正文（「列表背正文」是滚动卡顿主因）。
    TASK-103 实证：五布局一律走懒水合（本函数恒 false），with_content 在
    reloadFromBackend / loadMoreArticles / reloadFilteredEntries / anchorToArticle
    四处均传 false —— 快照行因此从不携带正文；正文由懒水合落 bodyById
    （TASK-122 实体缓存），快照替换不触碰它（刷新不丢已加载正文）。 */
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
  /* TASK-052：per-scope 游标表（'all' | feedId | 'cat-N' → 已加载条数） */
  articlesCursor: {},
  articlesLoading: false,
  articlesExhausted: false,
  /* TASK-111②：后台刷新保位信号（nonce）。仅 reloadFromBackend /
     reloadFilteredEntries 以 keepReadingPosition 落地时 bump（feeds-updated /
     手动同步 / 单源刷新路径）；导航路径（selectFeed/selectView/selectLayout/
     toggleTimelineSort）与启动装载不 bump——Timeline 订阅该 nonce，变化时消费
     顶条锚（timelineAnchor）做程序性回位。nonce 而非布尔：连续多次后台刷新
     每次都要触发一次消费，即使 entries 引用恰未变化。 */
  positionRestoreNonce: 0,

  /* ================= 数据源：后端 SQLite ⇄ mock ================= */

  /** 从后端拉全量快照（folders + feeds + articles）替换本地状态。
      代际守卫：并发调用只接受最新一次的结果——后台刷新事件与用户操作
      同时触发 reload 时，旧快照不会覆盖新快照（布局显示回退的根因）。

      TASK-052：首批查询带上**当前订阅范围**（feed_id / folder_id）。此前不带，
      于是 articlesLimit 是全局 offset，而 selectVisibleEntries 按范围过滤——
      「当前范围已加载了多少条」与列表实际能显示多少条脱节：单源视图下首批
      500 条里可能一条属于该源，且游标直接跳到 500（该源的老文章永远够不到）。
      范围由 get() 在**发起时**读取：异步等待期间用户切范围，结果交由下面
      查询代际守卫整体丢弃（TASK-119 统一后的查询实例代际，语义同原 reloadGeneration）。

      TASK-111②：opts.keepReadingPosition —— 仅内容刷新路径（feeds-updated /
      手动同步 / 单源刷新）传入。落地时 bump positionRestoreNonce 让 Timeline
      消费顶条锚回位（审计「后台刷新保留当前阅读位置」）；导航路径不传，
      锚机制对用户主动切换保持沉默（分流显式化）。 */
  reloadFromBackend: async (opts) => {
    /* TASK-109①：发起时一次性快照「范围×布局×排序」——查询参数、分页游标键
       与「全部」视图缓存键全部从该快照派生。原实现在 await 之后读「完成时」
       的 activeFeedFilter/activeContentLayout 充当游标键与缓存键口径（注释
       声称发起时），靠 reloadGeneration 间接兜底：任何范围/布局变更都经由
       selectFeed/selectLayout 触发新 reload 使旧代际整体丢弃，故行为等价；
       现改为显式成立（与 loadMoreArticles/anchorToArticle 的发起时快照同一
       形态），不再依赖默会。TASK-119 起代际守卫统一为查询实例代际
       （gen !== queryGeneration 即全局已有更新发起，见文件头机制说明）。 */
    const scopeAtStart = get().activeFeedFilter;
    const layoutAtStart = get().activeContentLayout;
    const sortAtStart = get().timelineSort;
    /* TASK-123：视图口径也在发起时快照——窗口重取资格按它判定（'all' 快照
       落地后可能被紧随的筛选重拉替换，筛选视图的窗口由该路径重取）。 */
    const viewAtStart = get().activeViewFilter;
    /* TASK-119：发起即取全局查询代际（原 reloadGeneration 的统一形态）——
       本入口是全局状态写入者（categories/feedCounts/entries），落地要求
       「此后没有任何更新的查询发起」（范围切换保护语义原样保留）；同时记录
       该键首屏代际（使在途续页/筛选响应过期）。 */
    const gen = beginGlobalQuery(QueryScope.viewKey(layoutAtStart, 'all', scopeAtStart));
    const keepReadingPosition = opts?.keepReadingPosition === true;
    /* TASK-123：保位窗口重取参数（发起时真值）——目标 = 该口径游标 loaded
       （缺键回落镜像 articlesLimit），底边界 = 游标窗口底锚 lastId。仅
       keepReadingPosition 且当前为 'all' 视图时启用；导航路径 windowTarget=0
       → 完全跳过窗口重取（行为与 TASK-123 前逐字一致）。 */
    const windowEligible = keepReadingPosition && viewAtStart === 'all';
    const cursorAtStart = get().articlesCursor[QueryScope.pageKey(scopeAtStart, layoutAtStart)];
    const windowTarget = windowEligible ? (cursorAtStart?.loaded ?? get().articlesLimit) : 0;
    const windowBottomId = windowEligible ? (cursorAtStart?.lastId ?? null) : null;
    /* TASK-122：行抓取时刻（await 前取）——bodyById AI 产物对齐的新鲜度判据
       （晚于该时刻的写入不回退，见 bodyCache.reconcileBodyEntities） */
    const rowsFetchedAt = Date.now();
    backendReloadInFlight++;
    try {
      const scopeKey = QueryScope.pageKey(scopeAtStart, layoutAtStart);
      const scopeArgs = QueryScope.args(scopeAtStart, sortAtStart, layoutAtStart);
      let folders, feeds, articles, counts;
      try {
        [folders, feeds, articles, counts] = await Promise.all([
          api.listFolders(),
          api.listFeeds(),
          api.listArticles({ ...scopeArgs, limit: ARTICLES_PAGE_SIZE, offset: 0, with_content: layoutNeedsBody(layoutAtStart) }),
          api.feedCounts(),
        ]);
      } catch (e) {
        /* TASK-067 N10：后台刷新事件/范围切换路径的失败此前完全不可见（bootstrap
           路径另有 bootstrapError，但 void 调用点无人接住）。toast 后 rethrow——
           bootstrapFromBackend 的错误态语义保持。 */
        get().showToast(`刷新失败：${extractError(e)}`);
        throw e;
      }
      if (gen !== queryGeneration) return; // TASK-119：全局已有更新的查询发起（统一查询代际，语义同原 reloadGeneration）
      if (!folders || !feeds || articles === null) return;

      /* TASK-123：保位窗口重取——keepReadingPosition 不再只拉首屏：keyset 续页
         重取 [首屏..窗口末]（底边界 = 发起时游标窗口底锚，插头部使窗口按实际
         插入量自然生长，锚 id 必在——除非真被删除/被筛出；机制见 fetchWindowRows
         头注）。续页与 loadMoreArticles 同为「同一查询实例的分页取数」：不 bump
         代际，每页由循环内 isStale 复核；重取期间任何更新的查询发起（用户导航、
         同键筛选）都使整个实例作废（下方 gen 复核含已取页）。backendReloadInFlight
         仍持有（finally 在函数尾）——「reload 在途拦截续拉」不变式覆盖多页形态。 */
      let windowExhausted = articles.length < ARTICLES_PAGE_SIZE;
      if (windowTarget > 0) {
        let win;
        try {
          win = await fetchWindowRows(scopeArgs, articles, windowTarget, windowBottomId, () => gen !== queryGeneration);
        } catch (e) {
          /* TASK-123 R1（R0 审查）：续页 IPC 失败与首屏失败同一可见性——上方
             TASK-067 N10 的 toast+rethrow 只包住首屏 Promise.all，窗口重取若在
             其外，失败会被调用方的 .catch(() => {}) 静默吞掉（N10 缺陷类经新
             代码路径重开）。同文案（前缀 ≤40 字）toast 后 rethrow：调用方语义
             与首屏失败逐字一致；失败不落地——旧 entries/游标原样保留（失败的
             刷新同样不缩窗，保位语义对失败路径天然成立）。与 reloadFilteredEntries
             的失败可见性形态对称（差异仅在其 catch 不重抛，见其 N10 注）。
             回归：t123-6（续页 reject → toast + 重抛 + 不缩窗不假 exhausted）。 */
          get().showToast(`刷新失败：${extractError(e)}`);
          throw e;
        }
        if (gen !== queryGeneration) return; // 窗口重取期间有更新查询发起 → 整体作废（含已取续页）
        articles = win.rows;
        windowExhausted = win.exhausted;
      }

      const categories = folderRowsToCategories(folders, feeds);
      const feedCounts = new Map<string, { total: number; unread: number; starred: number; today: number }>();
      if (counts) {
        for (const c of counts) {
          feedCounts.set(String(c.feed_id), { total: c.total, unread: c.unread, starred: c.starred, today: c.today });
        }
      }
      /* TASK-103（REQ-001）→ TASK-122：快照替换收口在 mergeSnapshotEntries。
         正文不再随行继承（真值源 bodyById，快照替换不触碰）；fromBackend=true
         同时执行 bodyById 的 AI 产物显式失效/对齐（清理 AI 缓存 → cleared）。 */
      const merged = mergeSnapshotEntries(get().entries, articles.map(articleRowToEntry), true, rowsFetchedAt);
      const nextEntries = merged.entries;
      /* TASK-117：首屏游标 = keyset 锚（本页最后一行的 (published_at 原文, id)），
         loaded 承接原「已加载条数」镜像语义（articlesLimit）。
         TASK-123：窗口重取后同一推进口径作用于**整个窗口**——锚 = 窗口末行，
         loaded = 窗口行数（续拉自窗口末无缝衔接；底部生长的行随窗口一并入列，
         游标与 entries 原子一致）。 */
      const windowCursor = cursorAfterFirstPage(articles);
      /* 分页游标键取自**发起时快照**（TASK-109①）：游标属于发起时的查询口径，
         与完成时的 activeFeedFilter 无关（gen 匹配时两者恒等，此处为显式口径）。
         TASK-094：entries 是该布局的快照，游标键带布局（R7：切布局不串游标）。 */
      // 缓存「全部」视图快照：切回时零延迟恢复（视图切换卡顿的根治）。
      // TASK-052：快照就是**该范围**的首批，故缓存键必须带范围，否则源A 的首批
      // 会被当成「全部」的首批恢复（数据错配）。TASK-094：键首段本就是布局。
      // TASK-122：正文真值源 bodyById（不随快照行传播），缓存恢复零延迟还原
      // 视图行；已加载正文由 bodyById 直接命中，无需随快照复制。
      // TASK-111①：经 setViewEntriesSnapshot 收口——单键实体预算（超限尾部截断）
      // + 分页元数据（cursor/exhausted 取写入时真值），恢复路径用记录值判定。
      setViewEntriesSnapshot(QueryScope.viewKey(layoutAtStart, 'all', scopeAtStart), nextEntries, windowCursor, windowExhausted);
      set((s) => ({
        ...reconcileCategories(s, categories),
        entries: nextEntries,
        feedCounts,
        articlesLimit: windowCursor.loaded,
        articlesCursor: { ...s.articlesCursor, [scopeKey]: windowCursor },
        articlesLoading: false,
        /* TASK-123：exhausted 随窗口重取的 keyset 真实判定（末页不满 = 集合尽），
           不再按首屏行数近似；windowTarget=0（导航/启动路径）时与旧形态逐字一致
           （= 首屏行数判定）。 */
        articlesExhausted: windowExhausted,
        dataMode: 'tauri',
        dataLoading: false,
        /* TASK-122：水合终态/失败态随 bodyById（state 判别态）存活，与快照
           替换解耦——旧「merged.hydratedIds / hydrationErrors 随 set 写回」删除。
           已水合卡片刷新后正文照常显示（bodyById 记录仍在），无「无请求死区」。 */
        /* TASK-111②：保位信号仅在「落地快照就是当前视图」时发出（'all'）。
           筛选视图下本落地的 'all' 快照会被紧随的 reloadFilteredEntries 整体
           替换，信号由后者发出——Timeline 消费时 items 已是最终快照，不会对
           过渡快照做错误回位。 */
        ...(keepReadingPosition && s.activeViewFilter === 'all'
          ? { positionRestoreNonce: s.positionRestoreNonce + 1 }
          : {}),
      }));
      /* 顺带刷新连接态：连接/断开后前端标签即时一致 */
      void api.syncStatus().then((st) => {
        if (st && gen === queryGeneration) set({ syncConnected: st.connected });
      }).catch(() => { /* TASK-067 N10：纯提示性刷新，失败不打扰 */ });
      /* TASK-116 四态展示：顺带刷新同步队列统计（侧栏 pill 的「等待同步 N 条 /
         部分失败」口径）。挂载（bootstrap 首次 reload）与手动同步完成（末次
         reload）都经过这里——契约约定的两个刷新点，无需另设事件。
         TASK-124：补写 lastError（syncQueueLastError，摘要卡错误行）——队列
         统计的 store 字段自此为事件与 reload 共用的单一状态源。 */
      void api.syncQueueStats().then((q) => {
        if (q && gen === queryGeneration) set({ syncWaiting: q.waiting, syncFailed: q.failed, syncQueueLastError: q.last_error ?? null });
      }).catch(() => { /* 纯提示性刷新，失败不打扰 */ });
      // 当前在筛选视图（收藏/未读/今天）时，reload 后重新拉取完整筛选列表
      // （状态/内容可能变化，entries 需同步刷新为筛选结果）
      // TASK-111②：保位请求随路径透传——'all' 快照只是过渡态，信号由筛选
      // 快照落地时发出。
      const view = get().activeViewFilter;
      if (view !== 'all') void get().reloadFilteredEntries(view, keepReadingPosition ? { keepReadingPosition: true } : undefined);
    } finally {
      backendReloadInFlight--;
    }
  },

  /** 滚动到底部按需拉取下一批文章（追加到 entries，不覆盖已加载的）。
      TASK-052：请求带**当前订阅范围**（feed_id / folder_id）与排序，游标取自该
      范围自己的 per-scope 游标（articlesCursor[scopeKey]，经 articlesLimit 镜像）。
      TASK-117（审计 P1-1）：续拉从 OFFSET 语义改 **keyset**——可变筛选集合
      （未读视图的 WHERE is_read=0、收藏视图的 is_starred=1）上 OFFSET 不等价于
      已看条数：1200 未读读 500 标读后集合剩 700，下一页仍 OFFSET 500 → 跳过
      剩余集合前 500 篇并假 exhausted。keyset 以已加载窗口最后一行的
      (published_at 原文, id) 为锚（last_published/last_id 成对发送），请求
      「严格排在锚之后」的行，与集合增删无关。OFFSET 键停用（后端保留兼容）；
      游标缺锚（lastPublished null，正常 tauri 流程不可达）才回落 offset。
      若已有更多在途则跳过（防抖）。
      TASK-110①：筛选视图（收藏/未读/今天）共用本路径续拉——请求携带与首屏同源
      的视图筛选参数（QueryScope.viewFilter 单点派生），「全部」视图空参数与旧
      wire 形态逐字一致；游标键复用 scopePageKey（快照恢复时游标随快照长度原子
      对齐，两视图共享键不串）。 */
  loadMoreArticles: async () => {
    const st = get();
    if (st.dataMode !== 'tauri') return;
    if (st.articlesLoading || st.articlesExhausted) return; // 已在加载 / 已到底
    /* TASK-100 P3-1：reload（全量/筛选）在途期间拦截续拉——此刻 entries 仍是
       旧口径快照（切布局/切范围先写游标镜像、reload 未返回），放行会把新口径
       一页 append 到旧列表尾。reload 落地后 refill/哨兵 effect 会重新触发。 */
    if (isBackendReloadInFlight()) return;
    /* 发起时快照「范围 + 布局 + 排序 + 游标 + 视图」：五者必须来自同一时刻，否则请求参数与
       竞态比较的基准会互相错位（例如请求用旧范围、比较用新范围）。 */
    const scope = st.activeFeedFilter;
    const layoutAtStart = st.activeContentLayout;
    const viewAtStart = st.activeViewFilter;
    const scopeKey = QueryScope.pageKey(scope, layoutAtStart);
    const scopeArgs = { ...QueryScope.args(scope, st.timelineSort, layoutAtStart), ...QueryScope.viewFilter(viewAtStart) };
    /* TASK-119：续页携带「首页代际」= 该键最近一次首屏类发起的序号（续页不是新
       查询实例：不 bump——无锚回落 offset 的退化形态同样是窗口续期）。期间若无
       更新首屏（reload/锚定/同键筛选），落地时键现值与携带值恒等。 */
    const queryKey = QueryScope.viewKey(layoutAtStart, viewAtStart, scope);
    const firstPageGen = currentFirstPageSerial(queryKey);
    /* TASK-117：keyset 锚 = 该 scope 游标现值（最后一行锚点 + loaded 计数）。
       游标表缺键（理论上仅 mock 残留/异常窗口）时以 articlesLimit 兜底——
       lastPublished 为 null → 请求不带 keyset 键，后端回落 OFFSET 语义。 */
    const cursorAtStart = st.articlesCursor[scopeKey] ?? emptyArticlesCursor(st.articlesLimit);
    /* F1（Batch 1/2 独立审查 P3）：排序也必须参与竞态比较——keyset 锚的含义随排序
       翻转（同锚点在 newest/oldest 下指向不同的后续集合）。守卫原本只比
       scopeKey 与游标：旧排序在途的分页响应若晚于「切排序后的重拉」到达，且重拉
       后游标恰好仍等于该锚点，就会被放行，把旧排序第 2 页接到新排序列表后
       （审查探针实测 duplicates 100 / missing 100）。发起时快照排序，返回时排序
       已翻转 ⇒ 该响应属于另一个查询口径，整体丢弃（与 scopeKey 同一判据粒度）。 */
    const sortAtStart = st.timelineSort;
    set({ articlesLoading: true });
    try {
      const rows = await api.listArticles({
        ...scopeArgs,
        limit: ARTICLES_PAGE_SIZE,
        /* TASK-117：OFFSET 停用（缺陷手法本身），由 keyset 锚承接续拉位置；
           无锚回落（见 cursorAtStart 注释）才带 offset。 */
        ...(cursorAtStart.lastPublished != null && cursorAtStart.lastId != null
          ? { last_published: cursorAtStart.lastPublished, last_id: cursorAtStart.lastId }
          : { offset: cursorAtStart.loaded }),
        with_content: layoutNeedsBody(layoutAtStart),
      });
      // 竞态保护：加载期间游标被重置（reload / selectView 命中缓存恢复快照 / 切换
      // 范围或布局加载了该口径自己的游标），丢弃本次追加。必须顺手复位 articlesLoading
      // （D3）：否则该标志永久为 true，被入口守卫（articlesLoading || articlesExhausted）
      // 永久挡住后续所有 loadMoreArticles —— 列表停在半截且加载动画常驻。
      // TASK-052 把比较基准从「全局 articlesLimit」收紧为「该范围的游标」：A 源在途时
      // 切到 B 源，B 源自己的游标可能与锚点数值相同（例如都是 500），若只比数值会
      // 把属于 A 的迟到数据错接到 B 的列表上；带上 scopeKey 后这种串台也会被丢弃。
      // F1：排序翻转同样使该响应过期（见发起时的 sortAtStart 注释）。
      // TASK-094（R7）：布局切换改变整个 entries 序列与游标键，布局在途响应一并过期。
      // TASK-109①：判据收口为具名守卫 paginationStale（语义见 internals）。
      // TASK-110①：view 入守卫——筛选视图分页化后切视图会整体替换 entries 并重置
      // 同键游标，旧视图的在途分页响应不得追加进新视图列表（锚点数值可能恰好
      // 相等，须显式比较视图维度）。
      // TASK-117：游标比较改为 keyset 三元组（lastPublished/lastId/loaded 任一
      // 漂移即过期——锚点推进与 reload 重置两种竞态都覆盖）。
      if (
        QueryScope.paginationStale(
          { scopeKey, sort: sortAtStart, cursor: cursorAtStart, view: viewAtStart },
          {
            scopeKey: QueryScope.pageKey(get().activeFeedFilter, get().activeContentLayout),
            sort: get().timelineSort,
            cursor: get().articlesCursor[scopeKey] ?? emptyArticlesCursor(get().articlesLimit),
            view: get().activeViewFilter,
          },
        )
      ) {
        set({ articlesLoading: false });
        return;
      }
      /* TASK-119（审计 P2-4①）：续页代际守卫——落地仅当该键此后没有更新的首屏类
         发起（reload/锚定/同键筛选）。三元组守卫在「新首屏（同数据重拉）落地后
         游标与发起时全等」时放行——参数比较识别不了同一查询的旧版本（审计原话：
         不能只比较查询参数是否相同），代际判据补上这一层。 */
      if (currentFirstPageSerial(queryKey) !== firstPageGen) {
        set({ articlesLoading: false });
        return;
      }
      /* TASK-110②：追加按 id 去重——稳定序策略「**追加去重保序**」。TASK-117 后
         续拉锚是最后一行锚点而非偏移量，插入漂移不再使续拉区间与已加载集合重叠
         （keyset 天然免疫，t117-3 断言锁定）；去重保留作 duplicate key 的兜底防线
         （集合收缩场景锚点前移等极端窗口）。游标推进 = 锚更新为**本页最后一行**，
         loaded 累加拉取行数。 */
      const seen = new Set(get().entries.map((e) => e.id));
      const next = (rows ? rows.map(articleRowToEntry) : []).filter((a) => {
        if (seen.has(a.id)) return false;
        seen.add(a.id);
        return true;
      });
      const fetched = rows ? rows.length : 0;
      const nextCursor = advanceArticlesCursor(cursorAtStart, rows ?? []);
      if (fetched < ARTICLES_PAGE_SIZE) {
        // 不足一页 → 已到底（按**拉取行数**判定：与追加去重无关，见 t110 既有语义）
        set((s) => ({
          entries: next.length ? [...s.entries, ...next] : s.entries,
          articlesLimit: nextCursor.loaded,
          articlesCursor: { ...s.articlesCursor, [scopeKey]: nextCursor },
          articlesLoading: false,
          articlesExhausted: true,
        }));
      } else {
        set((s) => ({
          entries: next.length ? [...s.entries, ...next] : s.entries,
          articlesLimit: nextCursor.loaded,
          articlesCursor: { ...s.articlesCursor, [scopeKey]: nextCursor },
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

  /** 切换视图到收藏/未读/今天时，按后端筛选拉取列表首屏。
      TASK-110①：真分页——废除 limit:100000 近似全集（审计「旧文章在筛选视图
      不可达」根因）：首屏只取 ARTICLES_PAGE_SIZE，articlesExhausted 改真实判定
      （rows.length < 页大小），续拉由 loadMoreArticles 携带同一组筛选参数完成
      （QueryScope.viewFilter 单点派生，游标键复用 scopePageKey）。

      TASK-052：查询同样带**当前订阅范围**（feed_id / folder_id）。同源下切视图
      是同一范围、更窄的口径（视图筛选是范围的子集），两条路径共用范围游标。

      TASK-111②：opts.keepReadingPosition —— 仅 reloadFromBackend 的后台刷新
      透传（feeds-updated / 手动同步 / 单源刷新）；selectView / toggleTimelineSort
      等导航路径不传。落地时 bump positionRestoreNonce（本快照即当前视图，
      守卫已确保 activeViewFilter === view）。 */
  reloadFilteredEntries: async (view, opts) => {
    if (get().dataMode !== 'tauri') return;
    backendReloadInFlight++;
    /* fix-4（自检 P2-2）：发起时快照「范围×布局×排序」口径——与 loadMoreArticles 的
       守卫判据对齐。此前只比较 view：切范围/切布局后的旧响应仍会放行，把
       「源A × 旧布局」的收藏列表覆盖进新口径（新请求先返回时旧响应晚到，
       错列表一直留存），并把游标写过期键。 */
    const scopeAtStart = get().activeFeedFilter;
    const layoutAtStart = get().activeContentLayout;
    const sortAtStart = get().timelineSort;
    const scopeKeyAtStart = QueryScope.pageKey(scopeAtStart, layoutAtStart);
    const scopeArgs = QueryScope.args(scopeAtStart, sortAtStart, layoutAtStart);
    /* TASK-123：保位窗口重取参数（发起时真值）——筛选视图与「全部」共用 scope
       游标（TASK-110①），目标 = 该游标 loaded、底边界 = 窗口底锚 lastId；仅
       keepReadingPosition 透传时启用（导航路径 undefined → 完全跳过，行为与
       TASK-123 前逐字一致）。 */
    const keepWindow = opts?.keepReadingPosition === true;
    const cursorAtStart = get().articlesCursor[scopeKeyAtStart];
    const windowTarget = keepWindow ? (cursorAtStart?.loaded ?? get().articlesLimit) : 0;
    const windowBottomId = keepWindow ? (cursorAtStart?.lastId ?? null) : null;
    /* TASK-119（审计 P2-4①探针 P5）：筛选视图发起 = 视图快照写入者，取 per-key
       查询代际（不推全局代际——切视图不得使在途后台刷新的全局态更新作废，
       理由见文件头机制说明）。queryKey = viewKey = pageKey×视图口径（TASK-117
       键族）：同键两次发起、先发后至的旧版本由落地判据识别丢弃；A→B→A 的再次
       发起 bump 同键代际，使最初那次发起过期。 */
    const queryKey = QueryScope.viewKey(layoutAtStart, view, scopeAtStart);
    const gen = beginViewQuery(queryKey);
    /* TASK-122：行抓取时刻（await 前取）——bodyById 对齐的新鲜度判据 */
    const rowsFetchedAt = Date.now();
    let rows;
    let windowExhausted = true;
    try {
      rows = await api.listArticles({
        ...scopeArgs,
        limit: ARTICLES_PAGE_SIZE, /* TASK-110①：真分页首屏（原 limit:100000 近似全集） */
        offset: 0,
        ...QueryScope.viewFilter(view),
        with_content: layoutNeedsBody(layoutAtStart),
      });
      /* TASK-123：保位窗口重取（与 reloadFromBackend 同机制，见 fetchWindowRows
         头注）——keepReadingPosition 的筛选刷新不再只拉首屏，锚 id 在窗口内 ⇒
         落地后必在。在途计数跨整个窗口重取持有（「reload 在途拦截续拉」不变式
         覆盖多页形态）；首屏为空（空列表）不做续取。视图/口径/代际守卫在重取后
         由下方既有守卫组统一复核（任何一条不满足即整体丢弃，与单页形态同粒度）。 */
      if (rows && rows.length > 0 && windowTarget > 0) {
        const win = await fetchWindowRows(
          { ...scopeArgs, ...QueryScope.viewFilter(view) },
          rows,
          windowTarget,
          windowBottomId,
          () => get().activeViewFilter !== view
            || QueryScope.filteredSnapshotStale(
              { scopeKey: scopeKeyAtStart, sort: sortAtStart },
              { scopeKey: QueryScope.pageKey(get().activeFeedFilter, get().activeContentLayout), sort: get().timelineSort },
            )
            || currentFirstPageSerial(queryKey) !== gen,
        );
        rows = win.rows;
        windowExhausted = win.exhausted;
      } else if (rows) {
        windowExhausted = rows.length < ARTICLES_PAGE_SIZE;
      }
    } catch (e) {
      /* TASK-067 N10：筛选视图拉取失败对用户可见（此前静默，列表停留旧快照） */
      get().showToast(`筛选列表加载失败：${extractError(e)}`);
      return;
    } finally {
      backendReloadInFlight--;
    }
    if (!rows) return;
    // 竞态保护：拉取期间用户又切了视图，丢弃过期结果
    if (get().activeViewFilter !== view) return;
    // fix-4：拉取期间订阅范围或布局也变了 ⇒ 该响应属于另一个查询口径，整体丢弃
    // TASK-109①：判据收口为具名守卫 filteredSnapshotStale。
    // TASK-110③：切排序改为重拉后，排序由服务端承载——守卫同步锁排序，迟到
    // 旧排序响应不得覆盖新排序列表（旧「全集本地重排」语义已随分页化废除）。
    if (QueryScope.filteredSnapshotStale(
      { scopeKey: scopeKeyAtStart, sort: sortAtStart },
      { scopeKey: QueryScope.pageKey(get().activeFeedFilter, get().activeContentLayout), sort: get().timelineSort },
    )) {
      return;
    }
    /* TASK-119：同查询旧版本丢弃——同键此后已有更新的首屏发起（P5 的两次同查询、
       A→B→A 的回跳、或该键上更新的锚定/重拉），本次响应属于过期查询实例，整体
       丢弃。既有 view/scopeKey/sort 守卫在「参数全同」时不可判别，由代际补齐。 */
    if (currentFirstPageSerial(queryKey) !== gen) return;
    // 替换 entries（筛选视图首屏），重置分页游标（续拉与「全部」视图同机制）
    // TASK-103 → TASK-122：快照替换收口在 mergeSnapshotEntries（正文真值源
    // bodyById，不随行继承；fromBackend=true 同时执行 AI 产物显式失效/对齐）。
    // TASK-111①：经 setViewEntriesSnapshot 收口——单键实体预算 + 分页元数据。
    // TASK-117：首屏游标 = keyset 锚（本页最后一行），与「全部」视图同一推进语义。
    // TASK-123：窗口重取后同口径作用于整个窗口（锚 = 窗口末行，loaded = 窗口
    // 行数，exhausted = keyset 真实判定；windowTarget=0 时与旧形态逐字一致）。
    const merged = mergeSnapshotEntries(get().entries, rows.map(articleRowToEntry), true, rowsFetchedAt);
    const windowCursor = cursorAfterFirstPage(rows);
    setViewEntriesSnapshot(QueryScope.viewKey(layoutAtStart, view, scopeAtStart), merged.entries, windowCursor, windowExhausted);
    set((s) => ({
      entries: merged.entries,
      articlesLimit: windowCursor.loaded,
      articlesCursor: { ...s.articlesCursor, [scopeKeyAtStart]: windowCursor },
      /* TASK-110①：exhausted 真实判定（原恒 true——近似全集的副产品） */
      articlesExhausted: windowExhausted,
      articlesLoading: false,
      /* TASK-111②：本快照即当前视图（守卫已确保 activeViewFilter === view），
         保位信号在此发出——Timeline 消费时 items 已是最终快照。 */
      ...(opts?.keepReadingPosition === true ? { positionRestoreNonce: s.positionRestoreNonce + 1 } : {}),
    }));
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
    // TASK-119：统一为 beginGlobalQuery——锚定是窗口替换写入者（entries/游标
    // 整体替换），取全局代际（此后任何更新的查询发起都使本锚定过期，两阶段
    // 各查一次），同时记录该键首屏代际（使该键在途续页/筛选响应过期）。
    const st = get();
    const gen = beginGlobalQuery(QueryScope.viewKey(st.activeContentLayout, st.activeViewFilter, st.activeFeedFilter));
    /* TASK-122：行抓取时刻（await 前取）——bodyById 对齐的新鲜度判据 */
    const rowsFetchedAt = Date.now();
    /* 映射当前订阅范围 → feed_id / folder_id（与 markCurrentViewAllRead 同口径）。
       TASK-052：与 loadMoreArticles 共用 scopeQueryArgs，两个入口的口径不再各写一份。
       注意顺序契约：本 action 按**调用时**的范围/排序构造查询，调用方（命令面板）
       必须先完成 selectFeed/selectView 的前置导航。 */
    const scope = st.activeFeedFilter;
    const scopeKey = QueryScope.pageKey(scope, st.activeContentLayout);
    const args = {
      ...QueryScope.args(scope, st.timelineSort, st.activeContentLayout),
      limit: ARTICLES_PAGE_SIZE,
      offset: 0,
      with_content: layoutNeedsBody(st.activeContentLayout),
    };
    let pos;
    try {
      pos = await api.articleIndex(args, Number(articleId));
    } catch (e) {
      get().showToast(`打开文章失败：${extractError(e)}`);
      return;
    }
    if (gen !== queryGeneration) return; // 期间又有更新的导航操作
    if (pos == null) return;
    // 从目标位置加载一页（若位置靠前，offset 为负会被 SQLite 截断为 0，安全）
    const offset = Math.max(0, pos);
    let rows;
    try {
      rows = await api.listArticles({ ...args, offset });
    } catch (e) {
      get().showToast(`打开文章失败：${extractError(e)}`);
      return;
    }
    if (gen !== queryGeneration) return;
    if (!rows) return;
    /* TASK-103 → TASK-122：锚定分页同样是快照替换——收口在 mergeSnapshotEntries
       （正文真值源 bodyById，不随行继承；fromBackend=true 同时执行 AI 产物
       显式失效/对齐）。 */
    /* TASK-109②：后端快照路径（fromBackend=true）——行级 is_read 是后端真值，
       在途乐观声明失效（bump）。 */
    const merged = mergeSnapshotEntries(get().entries, rows.map(articleRowToEntry), true, rowsFetchedAt);
    const next = merged.entries;
    /* TASK-117：锚定窗口的续拉游标 = 窗口最后一行的 keyset 锚；base.loaded 取
       offset（窗口前的行未进 entries，但「已看过的位置」计数口径与修前
       articlesLimit=offset+行数 逐字一致）。窗口为空（pos 越界）保留空锚，
       续拉回落 offset 语义（anchorToArticle 的请求本就带 offset）。 */
    const anchorCursor = advanceArticlesCursor(emptyArticlesCursor(offset), rows ?? []);
    set((s) => ({
      entries: next,
      articlesLimit: anchorCursor.loaded,
      articlesCursor: { ...s.articlesCursor, [scopeKey]: anchorCursor },
      articlesExhausted: next.length < ARTICLES_PAGE_SIZE,
      articlesLoading: false,
      activeArticleId: articleId,
      openedReadIds: { ...get().openedReadIds, [articleId]: true },
      /* TASK-065 N7：与 selectArticle 同口径复位阅读视图标志——搜索/命令面板
         打开新文章时残留的「译文模式/全文视图/原始渲染」会让 Reader 按旧标志
         渲染新文章（译文通常为空 → 正文整块空白）。 */
      isShowingTranslatedProse: false,
      isRawRenderMode: false,
      showFulltext: false,
    }));
    // F7：与 selectArticle 同口径——打开时按设置标已读（此前搜索/命令面板
    // 打开的文章不标读，与列表点开行为分叉）
    const { settings: stSettings, dataMode: stMode } = get();
    const target = get().entries.find((a) => a.id === articleId);
    if (stMode === 'tauri' && stSettings.markReadOnOpen && target && !target.isRead) {
      /* TASK-118（审计相邻缺口）：与 selectArticle 同口径——打开即标读失败走
         统一回滚助手 rollbackEntryClaims（乐观置位后快照 isRead 字段版本；
         恢复读态 + 逐 feed 回补 unread，版本守卫：期间已被接管则跳过），
         不再只提示。边界裁定同 selectArticle：失败只回滚读态与计数，不清
         activeArticleId（文章刚被锚定打开，用户还在读）。 */
      markEntriesRead(new Set([articleId]));
      const claim: EntryRollbackClaim = { id: articleId, field: 'isRead', prev: false, version: getEntryVersion(articleId, 'isRead') };
      void api.setRead(Number(articleId), true).catch((e) => {
        rollbackEntryClaims([claim]);
        get().showToast(`标读失败：${extractError(e)}`);
      });
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
