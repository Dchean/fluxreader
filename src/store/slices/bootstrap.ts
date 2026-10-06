import type { StateCreator } from 'zustand';
import { createInitialCategories, createInitialEntries } from '../../mockData';
import { api, articleRowToEntry, extractError, folderRowsToCategories } from '../../lib/api';
import { ARTICLES_PAGE_SIZE, appStore, buildFeedIndex, markEntriesRead, mergeSnapshotEntries, QueryScope, reconcileCategories, viewEntriesCache } from '../internals';
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
  | 'articlesCursor'
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

/** reloadFromBackend 代际计数：并发 reload 只接受最新一次结果 */
let reloadGeneration = 0;

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
    四处均传 false —— 快照行因此从不携带正文；快照替换时的正文保留由
    mergeSnapshotEntries 按 id 继承负责（internals，REQ-001 根因修复）。 */
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

  /* ================= 数据源：后端 SQLite ⇄ mock ================= */

  /** 从后端拉全量快照（folders + feeds + articles）替换本地状态。
      代际守卫：并发调用只接受最新一次的结果——后台刷新事件与用户操作
      同时触发 reload 时，旧快照不会覆盖新快照（布局显示回退的根因）。

      TASK-052：首批查询带上**当前订阅范围**（feed_id / folder_id）。此前不带，
      于是 articlesLimit 是全局 offset，而 selectVisibleEntries 按范围过滤——
      「当前范围已加载了多少条」与列表实际能显示多少条脱节：单源视图下首批
      500 条里可能一条属于该源，且游标直接跳到 500（该源的老文章永远够不到）。
      范围由 get() 在**发起时**读取：异步等待期间用户切范围，结果交由下面
      reloadGeneration 代际守卫整体丢弃（与既有的「旧代际 reload 被丢弃」同口径）。 */
  reloadFromBackend: async () => {
    const gen = ++reloadGeneration;
    backendReloadInFlight++;
    try {
      /* TASK-109①：发起时一次性快照「范围×布局×排序」——查询参数、分页游标键
         与「全部」视图缓存键全部从该快照派生。原实现在 await 之后读「完成时」
         的 activeFeedFilter/activeContentLayout 充当游标键与缓存键口径（注释
         声称发起时），靠 reloadGeneration 间接兜底：任何范围/布局变更都经由
         selectFeed/selectLayout 触发新 reload 使旧代际整体丢弃，故行为等价；
         现改为显式成立（与 loadMoreArticles/anchorToArticle 的发起时快照同一
         形态），不再依赖默会。reloadGeneration 守卫保留。 */
      const scopeAtStart = get().activeFeedFilter;
      const layoutAtStart = get().activeContentLayout;
      const sortAtStart = get().timelineSort;
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
      if (gen !== reloadGeneration) return; // 已有更新的 reload 在途/完成
      if (!folders || !feeds || articles === null) return;

      const categories = folderRowsToCategories(folders, feeds);
      const feedCounts = new Map<string, { total: number; unread: number; starred: number; today: number }>();
      if (counts) {
        for (const c of counts) {
          feedCounts.set(String(c.feed_id), { total: c.total, unread: c.unread, starred: c.starred, today: c.today });
        }
      }
      /* TASK-103（REQ-001）：快照替换按 id 继承正文与水合终态（收口在
         mergeSnapshotEntries）。新行从不带正文（with_content 恒 false），直接
         覆盖会把已水合卡片的正文抹掉；而虚拟列表按文章 id 保持卡片身份、
         useLazyHydrate 同 id 不重触发 → 卡片永挂「加载正文…」且无请求在途。 */
      const merged = mergeSnapshotEntries(get().entries, articles.map(articleRowToEntry), get().hydratedIds, get().hydrationErrors, true);
      const nextEntries = merged.entries;
      /* 分页游标键取自**发起时快照**（TASK-109①）：游标属于发起时的查询口径，
         与完成时的 activeFeedFilter 无关（gen 匹配时两者恒等，此处为显式口径）。
         TASK-094：entries 是该布局的快照，游标键带布局（R7：切布局不串游标）。 */
      // 缓存「全部」视图快照：切回时零延迟恢复（视图切换卡顿的根治）。
      // TASK-052：快照就是**该范围**的首批，故缓存键必须带范围，否则源A 的首批
      // 会被当成「全部」的首批恢复（数据错配）。TASK-094：键首段本就是布局。
      // TASK-103：缓存写入的是继承过正文的合并结果，缓存恢复（selectFeed 等）
      // 才能零延迟还原正文。
      viewEntriesCache.set(QueryScope.viewKey(layoutAtStart, 'all', scopeAtStart), nextEntries);
      set((s) => ({
        ...reconcileCategories(s, categories),
        entries: nextEntries,
        feedCounts,
        articlesLimit: articles.length,
        articlesCursor: { ...s.articlesCursor, [scopeKey]: articles.length },
        articlesLoading: false,
        articlesExhausted: articles.length < ARTICLES_PAGE_SIZE,
        dataMode: 'tauri',
        dataLoading: false,
        /* TASK-103：水合终态不再无条件清空——mergeSnapshotEntries 已按 id 裁剪，
           只保留仍在新快照中的终态/错误标记（旧写法把终态连同正文一起抹掉，
           正是 REQ-001「刷新后社交卡片一直加载正文」的根因） */
        hydratedIds: merged.hydratedIds,
        hydrationErrors: merged.hydrationErrors,
      }));
      /* 顺带刷新连接态：连接/断开后前端标签即时一致 */
      void api.syncStatus().then((st) => {
        if (st && gen === reloadGeneration) set({ syncConnected: st.connected });
      }).catch(() => { /* TASK-067 N10：纯提示性刷新，失败不打扰 */ });
      // 当前在筛选视图（收藏/未读/今天）时，reload 后重新拉取完整筛选列表
      // （状态/内容可能变化，entries 需同步刷新为筛选结果）
      const view = get().activeViewFilter;
      if (view !== 'all') void get().reloadFilteredEntries(view);
    } finally {
      backendReloadInFlight--;
    }
  },

  /** 滚动到底部按需拉取下一批文章（追加到 entries，不覆盖已加载的）。
      TASK-052：请求带**当前订阅范围**（feed_id / folder_id）与排序，游标取自该
      范围自己的 per-scope 游标（articlesCursor[scopeKey]，经 articlesLimit 镜像），
      因此「第 2 页」= 该范围的第 501..1000 条，而不是全局序列的第 501..1000 条。
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
    const offset = st.articlesLimit;
    /* F1（Batch 1/2 独立审查 P3）：排序也必须参与竞态比较——offset 的含义随排序
       翻转（同 offset=500 在 newest/oldest 下指向不同的 500 条）。守卫原本只比
       scopeKey 与游标：旧排序在途的分页响应若晚于「切排序后的重拉」到达，且重拉
       后游标恰好仍等于该 offset，就会被放行，把旧排序第 2 页接到新排序列表后
       （审查探针实测 duplicates 100 / missing 100）。发起时快照排序，返回时排序
       已翻转 ⇒ 该响应属于另一个查询口径，整体丢弃（与 scopeKey 同一判据粒度）。 */
    const sortAtStart = st.timelineSort;
    set({ articlesLoading: true });
    try {
      const rows = await api.listArticles({ ...scopeArgs, limit: ARTICLES_PAGE_SIZE, offset, with_content: layoutNeedsBody(layoutAtStart) });
      // 竞态保护：加载期间游标被重置（reload / selectView 命中缓存恢复快照 / 切换
      // 范围或布局加载了该口径自己的游标），丢弃本次追加。必须顺手复位 articlesLoading
      // （D3）：否则该标志永久为 true，被入口守卫（articlesLoading || articlesExhausted）
      // 永久挡住后续所有 loadMoreArticles —— 列表停在半截且加载动画常驻。
      // TASK-052 把比较基准从「全局 articlesLimit」收紧为「该范围的游标」：A 源在途时
      // 切到 B 源，B 源自己的游标可能与 offset 数值相同（例如都是 500），若只比数值会
      // 把属于 A 的迟到数据错接到 B 的列表上；带上 scopeKey 后这种串台也会被丢弃。
      // F1：排序翻转同样使该响应过期（见发起时的 sortAtStart 注释）。
      // TASK-094（R7）：布局切换改变整个 entries 序列与游标键，布局在途响应一并过期。
      // TASK-109①：判据收口为具名守卫 paginationStale（语义见 internals）。
      // TASK-110①：view 入守卫——筛选视图分页化后切视图会整体替换 entries 并重置
      // 同键游标，旧视图的在途分页响应不得追加进新视图列表（游标数值可能恰好
      // 相等，须显式比较视图维度）。
      if (
        QueryScope.paginationStale(
          { scopeKey, sort: sortAtStart, offset, view: viewAtStart },
          {
            scopeKey: QueryScope.pageKey(get().activeFeedFilter, get().activeContentLayout),
            sort: get().timelineSort,
            offset: get().articlesLimit,
            view: get().activeViewFilter,
          },
        )
      ) {
        set({ articlesLoading: false });
        return;
      }
      /* TASK-110②：追加按 id 去重——稳定序策略选定「**追加去重保序**」：
         同步在已加载窗口前端插入新条目会使 offset 漂移，下一页与已加载集合
         重叠，去重保证 entries 按 id 唯一（duplicate key 防线）。策略取舍
         （审计「后台刷新保留当前阅读位置」）：插入项**不回填**已加载窗口
         （触发重拉会整体替换列表、丢失滚动位置），随下次 reloadFromBackend
         （后台刷新/切范围/切筛选）进入列表；游标按**拉取行数**推进（offset
         语义 = 已看过的后端位置），头部插入的漂移量恰等于去重量，不跳行不重复
         （删除型漂移可能跳过个位数行，由 reload 对齐——offset 分页的固有限制）。 */
      const seen = new Set(get().entries.map((e) => e.id));
      const next = (rows ? rows.map(articleRowToEntry) : []).filter((a) => {
        if (seen.has(a.id)) return false;
        seen.add(a.id);
        return true;
      });
      const fetched = rows ? rows.length : 0;
      if (fetched < ARTICLES_PAGE_SIZE) {
        // 不足一页 → 已到底（按**拉取行数**判定：偏移漂移去重后追加数变短不代表到底）
        set((s) => ({
          entries: next.length ? [...s.entries, ...next] : s.entries,
          articlesLimit: offset + fetched,
          articlesCursor: { ...s.articlesCursor, [scopeKey]: offset + fetched },
          articlesLoading: false,
          articlesExhausted: true,
        }));
      } else {
        set((s) => ({
          entries: next.length ? [...s.entries, ...next] : s.entries,
          articlesLimit: offset + fetched,
          articlesCursor: { ...s.articlesCursor, [scopeKey]: offset + fetched },
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
      是同一范围、更窄的口径（视图筛选是范围的子集），两条路径共用范围游标。 */
  reloadFilteredEntries: async (view) => {
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
    let rows;
    try {
      rows = await api.listArticles({
        ...scopeArgs,
        limit: ARTICLES_PAGE_SIZE, /* TASK-110①：真分页首屏（原 limit:100000 近似全集） */
        offset: 0,
        ...QueryScope.viewFilter(view),
        with_content: layoutNeedsBody(layoutAtStart),
      });
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
    // 替换 entries（筛选视图首屏），重置分页游标（续拉与「全部」视图同机制）
    // TASK-103：同 reloadFromBackend——快照替换按 id 继承正文与水合终态（合并
    // 逻辑收口在 mergeSnapshotEntries，不得在此另写一份），终态按 id 裁剪。
    const merged = mergeSnapshotEntries(get().entries, rows.map(articleRowToEntry), get().hydratedIds, get().hydrationErrors, true);
    viewEntriesCache.set(QueryScope.viewKey(layoutAtStart, view, scopeAtStart), merged.entries);
    set((s) => ({
      entries: merged.entries,
      articlesLimit: rows.length,
      articlesCursor: { ...s.articlesCursor, [scopeKeyAtStart]: rows.length },
      /* TASK-110①：exhausted 真实判定（原恒 true——近似全集的副产品） */
      articlesExhausted: rows.length < ARTICLES_PAGE_SIZE,
      articlesLoading: false,
      hydratedIds: merged.hydratedIds,
      hydrationErrors: merged.hydrationErrors,
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
    const gen = ++reloadGeneration;
    const st = get();
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
    if (gen !== reloadGeneration) return; // 期间又有更新的导航操作
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
    if (gen !== reloadGeneration) return;
    if (!rows) return;
    /* TASK-103：锚定分页同样是快照替换——按 id 继承正文与水合终态（收口在
       mergeSnapshotEntries），终态按 id 裁剪，不再整体清空。 */
    /* TASK-109②：后端快照路径（fromBackend=true）——行级 is_read 是后端真值，
       在途乐观声明失效（bump）。 */
    const merged = mergeSnapshotEntries(get().entries, rows.map(articleRowToEntry), get().hydratedIds, get().hydrationErrors, true);
    const next = merged.entries;
    set((s) => ({
      entries: next,
      articlesLimit: offset + next.length,
      articlesCursor: { ...s.articlesCursor, [scopeKey]: offset + next.length },
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
      hydratedIds: merged.hydratedIds,
      hydrationErrors: merged.hydrationErrors,
    }));
    // F7：与 selectArticle 同口径——打开时按设置标已读（此前搜索/命令面板
    // 打开的文章不标读，与列表点开行为分叉）
    const { settings: stSettings, dataMode: stMode } = get();
    const target = get().entries.find((a) => a.id === articleId);
    if (stMode === 'tauri' && stSettings.markReadOnOpen && target && !target.isRead) {
      void api.setRead(Number(articleId), true).catch((e) => {
        get().showToast(`标读失败：${extractError(e)}`);
      });
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
