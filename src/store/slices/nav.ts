import type { StateCreator } from 'zustand';
import { api, extractError } from '../../lib/api';
import { getEntryVersion, markEntriesRead, mergeSnapshotEntries, QueryScope, syncCurrentViewCache, viewEntriesCache } from '../internals';
import { selectVisibleEntries } from '../selectors';
import type { AppState } from '../types';

/** 导航与筛选 slice：内容布局 / 视图 / 订阅范围 / 时间流筛选状态，及其导航 action。
 *
 *  Pick 的键集即本 slice 的全部键；与其它 slice 两两不相交（合起来 = 原 useAppStore 全集）。
 */
export type NavSlice = Pick<
  AppState,
  | 'activeContentLayout'
  | 'activeViewFilter'
  | 'activeFeedFilter'
  | 'timelineFilter'
  | 'timelineSort'
  | 'selectLayout'
  | 'selectView'
  | 'selectFeed'
  | 'applyArticlesCursor'
  | 'toggleTimelineFilter'
  | 'toggleTimelineSort'
  | 'markCurrentViewAllRead'
  | 'switchRestoreNonce'
>;

/** 本地零点毫秒（F8：今天视图的标读边界，与列表「今天」筛选同口径） */
function startOfLocalDayMs(): number {
  const d = new Date();
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

export const createNavSlice: StateCreator<AppState, [], [], NavSlice> = (set, get) => ({
  activeContentLayout: 'article',
  activeViewFilter: 'all',
  activeFeedFilter: 'all',
  timelineFilter: 'unread',
  timelineSort: 'newest',
  /* TASK-115①：切换返回恢复信号（字段说明见 types.ts）。仅在下方三个导航
     action 的**缓存命中**分支 bump（entries 同步恢复时）；cache-miss 重拉、
     切排序（重拉新语境）与启动装载一律不 bump。 */
  switchRestoreNonce: 0,

  /* ================= 导航 ================= */

  selectLayout: (layout) => {
    /* TASK-115①：本次导航是否真的改变筛选上下文——布局段变化，或范围段被
       下方的 'all' 重置改变。同上下文重复导航（点击当前布局）不是「切换返回」，
       不 bump 恢复信号（否则列表会被拽回上次离开的位置）。 */
    const contextChanged = layout !== get().activeContentLayout || get().activeFeedFilter !== 'all';
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
    /* TASK-094（REQ-107）：布局是后端列表查询的维度（list_articles 带 layout），
       entries 快照只含当前布局的条目——切布局后旧快照对新布局是错的，纯本地过滤
       不再成立，必须按新布局重拉（与 selectFeed 同构：命中缓存先同步恢复该
       「布局×视图×范围」快照零延迟显示，再后台刷新；游标按 (布局×范围) 恢复，
       各布局首批从自己的第 1 页开始，切回不重复不跳页）。 */
    if (get().dataMode !== 'tauri') return;
    const scopeKey = QueryScope.pageKey(get().activeFeedFilter, layout);
    set((s) => ({
      /* TASK-117：镜像取 keyset 游标的 loaded 计数（原游标值即计数） */
      articlesLimit: s.articlesCursor[scopeKey]?.loaded ?? 0,
      articlesExhausted: false,
      articlesLoading: false,
    }));
    const view = get().activeViewFilter;
    const cached = viewEntriesCache.get(QueryScope.viewKey(layout, view, get().activeFeedFilter));
    if (cached) {
      /* TASK-103：缓存恢复同属快照替换——正文与水合终态按 id 继承（收口在
         mergeSnapshotEntries；缓存快照本身携带 reload 时继承的正文），仅裁剪
         已不在恢复快照中的滞留标记。TASK-063 的「滞留标记阻断重水合」缺陷
         由该收口统一处置，不再在此整体清空。
         TASK-109②：fromBackend=false——缓存回放是近期 UI 状态而非后端真值，
         不 bump 条目版本（在途乐观声明的回滚仍有效）。 */
      const merged = mergeSnapshotEntries(get().entries, cached.entries, get().hydratedIds, get().hydrationErrors, false);
      /* TASK-110①：exhausted 真实判定随快照长度（原筛选视图恒 true、all 视图恒
         false——两者都随分页化失效）；缓存快照即最近一次拉取的首屏/续拉结果，
         「长度 < 页大小 ⇒ 已到底」与拉取时的判定同口径。
         TASK-111①：判定与游标恢复改用缓存**记录的元数据**（exhausted /
         loadedCount）而非截断后的 entries.length——单键实体预算超限尾部截断后，
         截断长度若落在页大小边界附近会把「已到底」误判成「还有数据」（或反），
         且续拉 offset 会回退重拉已去重丢弃的区间；记录值是写入时的真实口径
         （含偏移漂移下 entries 短于游标的形态），恢复行为与写入时逐字一致。
         TASK-115①：entries 恢复与切换返回信号**同一次原子写入**（switchRestoreNonce
         bump，仅 contextChanged——同上下文重复导航不恢复）——Timeline 收到信号后
         按 per-filterKey 锚存档一次性定位（存档锚在恢复列表 → scrollToIndex
         align:start；无存档/锚丢失 → 归零回落）。
         不叠加保证：本信号独立于 TASK-111 的 positionRestoreNonce（刷新保位），
         恢复动作不 bump/消费刷新保位信号；随后的后台 reload 是导航路径调用
         （不带 keepReadingPosition），刷新保位机制对本次切换保持沉默——
         「一次性定位」与「持续跟踪」分流，见 timelineAnchor.ts 头注规则表。 */
      set((s) => ({
        entries: merged.entries,
        articlesExhausted: cached.exhausted,
        hydratedIds: merged.hydratedIds,
        hydrationErrors: merged.hydrationErrors,
        ...(contextChanged ? { switchRestoreNonce: s.switchRestoreNonce + 1 } : {}),
      }));
      get().applyArticlesCursor(scopeKey, cached.cursor, cached.exhausted);
    }
    /* TASK-098（与 F5 同口径）：void reload 调用点必须接住 promise——失败提示由
       reload 自身的 toast 给出，这里只吞掉残余重抛，避免 unhandled rejection。 */
    if (view !== 'all') void get().reloadFilteredEntries(view).catch(() => { /* 失败已可见（reloadFilteredEntries 内 toast） */ });
    else void get().reloadFromBackend().catch(() => { /* 失败已可见（reloadFromBackend 内 toast） */ });
  },

  /* TASK-052 契约（per-scope 游标）：游标与列表口径绑定，切换口径时必须让两者
     一致。写入器分两条路，组件不得绕开：
     - reloadFromBackend / reloadFilteredEntries / anchorToArticle：拉回数据后用
       **该范围自己的游标**原子写入（entries 与 articlesLimit 同一次 set）；
     - applyArticlesCursor（本 action）：只做镜像，用于「已有 entries 与目标游标不
       匹配、但无需重新拉取」的同步恢复（目前只有 selectView 缓存命中走这条）。
     之所以不暴露成通用 setter：单纯写 articlesLimit 而不动 entries（D3 缺陷当时
     可达的形态）会把「游标指向的位置」与「列表里的内容」拆开，下一次 loadMore 的
     起点就会越过列表内容、整段文章静默丢失。
     TASK-117：第二参从计数改为完整 keyset 游标（ArticlesCursorState）——
     articlesLimit 镜像写 cursor.loaded（计数口径不变），articlesCursor 存完整锚。 */
  applyArticlesCursor: (scopeKey, cursor, exhausted) =>
    set((s) => ({
      articlesLimit: cursor.loaded,
      articlesCursor: { ...s.articlesCursor, [scopeKey]: cursor },
      articlesExhausted: exhausted,
      articlesLoading: false,
    })),

  selectView: (view) => {
    /* 视图缓存优先：命中则同步恢复该视图上次的 entries（零延迟、无渲染卡顿），
       再后台异步刷新保证数据最新。数量悬殊切换（收藏19 ↔ 全部2122）不再经历
       「清空 → 拉取 → 一次性渲染数百张卡片」的卡顿。
       TASK-052：缓存键带上订阅范围（源A 的首批≠全部的首批）；缓存里只有内容，
       游标仍需经 applyArticlesCursor 收口写入（不裸写 articlesLimit）。
       TASK-063：恢复时必须处置滞留水合状态——否则水合守卫（ensureArticleContent
       的 hydratedIds 短路）会把不属于本快照的滞留标记误判为「已水合」，社交/通知
       卡片在后台刷新落地前空白且不会重水合。TASK-103：处置方式从「整体清空」
       收口为 mergeSnapshotEntries 的按 id 继承+裁剪（正文与终态一起继承，
       已消失条目的滞留标记移除），缓存恢复不再丢已水合正文。 */
    /* TASK-115①：同 selectLayout——视图段真的变化才是「切换返回」；同上下文
       重复导航（点击当前视图）不 bump 恢复信号。 */
    const contextChanged = view !== get().activeViewFilter;
    const scopeKey = QueryScope.pageKey(get().activeFeedFilter, get().activeContentLayout);
    const cached = viewEntriesCache.get(QueryScope.viewKey(get().activeContentLayout, view, get().activeFeedFilter));
    if (cached) {
      /* TASK-103：同 selectLayout——快照恢复按 id 继承正文与水合终态（合并收口
         在 mergeSnapshotEntries），仅裁剪已不在恢复快照中的滞留标记。
         TASK-109②：fromBackend=false（缓存回放非后端真值，不 bump 版本）。 */
      const merged = mergeSnapshotEntries(get().entries, cached.entries, get().hydratedIds, get().hydrationErrors, false);
      /* TASK-110①：exhausted 真实判定随快照长度（与 selectLayout 同口径收口——
         原写法 `view !== 'all'` 是「筛选视图拉全集 ⇒ 恒已到底」的旧语义：分页化后
         筛选视图的缓存快照可能是未满页的部分页，误标已到底会挡住续拉，旧文章
         在「缓存命中 + 后台刷新失败」的窗口内不可达）。缓存快照即最近一次拉取的
         首屏/续拉结果，「长度 < 页大小 ⇒ 已到底」与拉取时的判定同口径。
         TASK-111①：与 selectLayout 同口径——判定/游标恢复用缓存记录的元数据
         （exhausted / loadedCount），不重算截断后的快照长度（理由见 selectLayout）。
         TASK-115①：同 selectLayout——entries 恢复与切换返回信号同一次原子写入
         （不叠加保证见 selectLayout 注释；仅 contextChanged）。 */
      set((s) => ({
        activeViewFilter: view,
        openedReadIds: {},
        entries: merged.entries,
        articlesExhausted: cached.exhausted,
        hydratedIds: merged.hydratedIds,
        hydrationErrors: merged.hydrationErrors,
        ...(contextChanged ? { switchRestoreNonce: s.switchRestoreNonce + 1 } : {}),
      }));
      get().applyArticlesCursor(scopeKey, cached.cursor, cached.exhausted);
      /* 后台静默刷新（不阻塞切换）：状态/内容可能已变 */
      /* TASK-098（与 F5 同口径）：同 selectLayout——接住 reload 重抛，失败提示由 reload 自身给出 */
      if (view !== 'all') void get().reloadFilteredEntries(view).catch(() => { /* 失败已可见（reloadFilteredEntries 内 toast） */ });
      else void get().reloadFromBackend().catch(() => { /* 失败已可见（reloadFromBackend 内 toast） */ });
      return;
    }
    set({ activeViewFilter: view, openedReadIds: {} });
    // 非「全部」视图：按后端筛选拉取完整列表替换 entries（收藏/未读的老文章
    // 不在「全部」的分页快照里）；「全部」视图：恢复分页快照。
    /* TASK-098（与 F5 同口径）：同 selectLayout——接住 reload 重抛，失败提示由 reload 自身给出 */
    if (view !== 'all') void get().reloadFilteredEntries(view).catch(() => { /* 失败已可见（reloadFilteredEntries 内 toast） */ });
    else void get().reloadFromBackend().catch(() => { /* 失败已可见（reloadFromBackend 内 toast） */ });
  },

  /* TASK-052：切换订阅范围时按 per-scope 游标恢复分页游标；该范围从未加载过
     （游标表中无该键）则从 0 起步——即「B 源从第 1 页开始」。
     TASK-063（N2）：范围切换必须让 entries 与游标重新对齐。此前只写游标镜像，
     注释宣称「由调用方随后拉取」，但 Sidebar/Overlays 的全部调用点都未接线：
     旧范围快照残留（跨范围重复卡片 + duplicate key），空列表补拉走
     loadMoreArticles 的追加路径（offset=0 与旧快照交集重复），目标源不在旧
     快照且不可滚动时其第一页永远拉不到。
     与 selectView 同构：tauri 模式下缓存命中同步恢复该范围快照（零延迟）并
     后台刷新；未命中直接后台重拉——两个 reload 都在发起时读取刚写入的
     activeFeedFilter，自带代际/竞态守卫丢弃过期结果。恢复时按 id 继承+裁剪
     水合状态（TASK-103 收口到 mergeSnapshotEntries：缓存快照携带继承的正文，
     滞留标记只裁剪不属于本快照的部分——理由同 selectView）。mock 模式保持纯游标镜像（不触发 IPC、
     不把 mock 会话翻成 tauri）。 */
  selectFeed: (feedId) => {
    /* TASK-115①：同 selectLayout——范围段真的变化才是「切换返回」；同上下文
       重复导航（点击当前源/分类）不 bump 恢复信号。 */
    const contextChanged = feedId !== get().activeFeedFilter;
    const scopeKey = QueryScope.pageKey(feedId, get().activeContentLayout);
    set((s) => ({
      activeFeedFilter: feedId,
      openedReadIds: {},
      /* TASK-117：镜像取 keyset 游标的 loaded 计数（原游标值即计数） */
      articlesLimit: s.articlesCursor[scopeKey]?.loaded ?? 0,
      articlesExhausted: false,
      articlesLoading: false,
    }));
    if (get().dataMode !== 'tauri') return;
    const view = get().activeViewFilter;
    const cached = viewEntriesCache.get(QueryScope.viewKey(get().activeContentLayout, view, feedId));
    if (cached) {
      /* TASK-103：同 selectLayout——快照恢复按 id 继承正文与水合终态（合并收口
         在 mergeSnapshotEntries），仅裁剪已不在恢复快照中的滞留标记。
         TASK-109②：fromBackend=false（缓存回放非后端真值，不 bump 版本）。 */
      const merged = mergeSnapshotEntries(get().entries, cached.entries, get().hydratedIds, get().hydrationErrors, false);
      /* TASK-110①：exhausted 真实判定随快照长度（与 selectLayout/selectView 同口径
         收口——`view !== 'all'` 的恒真/恒 false 旧语义随分页化失效，理由见 selectLayout）。
         TASK-111①：与 selectLayout 同口径——判定/游标恢复用缓存记录的元数据
         （exhausted / loadedCount），不重算截断后的快照长度（理由见 selectLayout）。
         TASK-115①：同 selectLayout——entries 恢复与切换返回信号同一次原子写入
         （不叠加保证见 selectLayout 注释；仅 contextChanged）。 */
      set((s) => ({
        entries: merged.entries,
        articlesExhausted: cached.exhausted,
        hydratedIds: merged.hydratedIds,
        hydrationErrors: merged.hydrationErrors,
        ...(contextChanged ? { switchRestoreNonce: s.switchRestoreNonce + 1 } : {}),
      }));
      get().applyArticlesCursor(scopeKey, cached.cursor, cached.exhausted);
    }
    /* TASK-098（与 F5 同口径）：同 selectLayout——接住 reload 重抛，失败提示由 reload 自身给出 */
    if (view !== 'all') void get().reloadFilteredEntries(view).catch(() => { /* 失败已可见（reloadFilteredEntries 内 toast） */ });
    else void get().reloadFromBackend().catch(() => { /* 失败已可见（reloadFromBackend 内 toast） */ });
  },

  toggleTimelineFilter: () =>
    set((s) => ({
      timelineFilter: s.timelineFilter === 'all' ? 'unread' : 'all',
      openedReadIds: {},
    })),

  /* 排序方向决定 offset 的含义（scopeQueryArgs 的 newest_first）：只翻转排序键
     而不重拉，已加载的快照（旧排序的首批）会与新排序的下一页错位——继续翻页
     取回的是另一端的文章，整段不可达 + 重复卡片（审计「排序切换游标错位」）。
     故丢弃各视图快照缓存，并按新排序重拉：reload 落地时原子改写 entries 与
     per-scope 游标（游标含义已随排序翻转，必须与 entries 同一次写入）。
     不清空 entries：本地选择器先按新排序就位（零延迟、无空白闪烁），重拉完成后
     整体替换——与 selectView 缓存命中路径同构。
     TASK-110③：筛选视图（收藏/未读/今天）从「全集本地重排、切排序不调后端」
     改为与「全部」视图同构——分页化后全集不再在内存里，本地重排只够重排已加载
     页且续拉口径会随排序错位，排序改由服务端承载（重拉当前范围）；已加载条目的
     水合正文由 mergeSnapshotEntries(fromBackend=true) 按 id 继承，不因重拉丢失。 */
  toggleTimelineSort: () => {
    viewEntriesCache.clear();
    set((s) => ({
      timelineSort: s.timelineSort === 'newest' ? 'oldest' : 'newest',
      openedReadIds: {},
    }));
    /* F5（Batch 1/2 独立审查 P3）：
       - dataMode 守卫：mock 模式没有后端，重拉不仅多余，还会在落地时把 mock
         会话翻成 tauri（reloadFromBackend 成功路径写 dataMode:'tauri'）；
       - TASK-110③ 前：筛选视图拉的本就是全集（limit 100000，不分页），切排序只
         需本地重排、不调后端——该前提已随筛选视图分页化废除，两类视图统一按
         新排序重拉（重拉入口随视图分流：reloadFilteredEntries / reloadFromBackend）；
       - reload 失败时提示由 reload 自身给出（reloadFromBackend toast 后重抛、
         reloadFilteredEntries toast 后吞掉），void 调用点必须接住 .catch，
         否则 unhandled rejection。 */
    if (get().dataMode !== 'tauri') return;
    const view = get().activeViewFilter;
    if (view !== 'all') void get().reloadFilteredEntries(view).catch(() => { /* 失败已可见（reloadFilteredEntries 内 toast） */ });
    else void get().reloadFromBackend().catch(() => { /* 失败已可见（reloadFromBackend 内 toast） */ });
  },

  markCurrentViewAllRead: () => {
    const ids = new Set(selectVisibleEntries(get()).map((i) => i.id));
    if (get().dataMode !== 'tauri') {
      /* mock 模式：无库可写，仅本地标读 + 提示，绝不伪造落库（(f) 契约不变） */
      markEntriesRead(ids);
      set({ openedReadIds: {} });
      get().showToast('已全部标为已读');
      return;
    }
    /* 范围语义与后端一致：当前 feed/分类范围（all 时两者皆 null）。
       TASK-109①：标写范围派生收口到 QueryScope.markScope——只取订阅维度
       （feed_id/folder_id）：排序与标写无关、布局由下方 api.markAllRead 的独立
       layout 参数承载；不再借道 scopeQueryArgs 传入 layout/sort 又丢弃。 */
    const scope = get().activeFeedFilter;
    const { feed_id: feedId, folder_id: folderId } = QueryScope.markScope(scope);
    /* F8：视图口径必须与界面一致——收藏/今天视图只标该视图可见的文章，
       否则会把范围内未显示的文章一并标读（并推给远端），与文案不符 */
    const view = get().activeViewFilter;
    const starredOnly = view === 'starred';
    const sinceMs = view === 'today' ? startOfLocalDayMs() : undefined;
    const layout = get().activeContentLayout;
    /* TASK-107（REQ-003）：本地已加载条目乐观先行（读态即时变化不等落库），
       但必须保存被翻转条目的原读态——失败时逐条恢复，不允许「计数已扣、
       状态已改」的假成功。 */
    const prevReadById = new Map<string, boolean>();
    for (const e of get().entries) {
      if (ids.has(e.id)) prevReadById.set(e.id, e.isRead);
    }
    const prevOpenedReadIds = get().openedReadIds;
    markEntriesRead(ids);
    /* TASK-107 R1（F1）：乐观写入后快照各 id 的变更版本——失败回滚只恢复
       「版本仍等于快照值」的条目。仅凭「当前值仍等于乐观写入值」的值守卫
       无法区分「用户已接管（同值覆盖写入）」与「未被触碰」：在途失败期间
       用户连点两次 toggle 停在与乐观写入相同的值时，迟到回滚会误踩用户最终
       意图而 DB 已是新值（审查探针 C3 实测）。版本由 internals 的三个真实
       写入点维护：flipEntryFlag（单条 toggle）/ markEntriesRead（本操作的
       乐观写入与批量标读）/ mergeSnapshotEntries（快照替换带后端真值）。
       TASK-118（审计 P1-2）：快照与守卫都取 **isRead 字段**版本——修前文章级
       共享版本下，窗口内用户收藏（bump isStarred）会把读回滚误判成已接管而
       跳过（应恢复未读却停在乐观已读）。 */
    const optimisticVersionById = new Map<string, number>();
    for (const [id, prev] of prevReadById) {
      if (!prev) optimisticVersionById.set(id, getEntryVersion(id, 'isRead'));
    }
    set({ openedReadIds: {} });
    void api.markAllRead(feedId, folderId, { starredOnly, sinceMs, layout }).then((affected) => {
      /* TASK-107：成功以后端为准对账。mark_all_read 影响整个范围（含未加载
         条目），此前忽略返回值、只按已加载条目推算计数——审计探针：范围 600
         条未读、前端加载 1 条，后端成功 600 条后界面仍显示 599。范围总量前端
         不可知（分页只加载首批），affected 无法直接换算计数，对账收口为
         「重取 feed_counts 整体替换」（与 reloadFromBackend 同一计数来源）；
         affected 是后端对本次操作的实际影响报告（0 = 范围内本就没有未读，
         此时乐观写入也未翻转任何条目，无需重取）。 */
      get().showToast('已全部标为已读');
      if (affected === 0) return;
      const reconcileCounts = () =>
        api.feedCounts().then((rows) => {
          if (!rows) return false;
          const next = new Map<string, { total: number; unread: number; starred: number; today: number }>();
          for (const c of rows) {
            next.set(String(c.feed_id), { total: c.total, unread: c.unread, starred: c.starred, today: c.today });
          }
          set({ feedCounts: next });
          return true;
        });
      /* TASK-107 R1（F2）：对账重取失败不再完全静默——落库已成功但计数残留
         乐观值（600/1 形态下显示 599、DB 真值 0），先给一条诊断提示（≤40 字，
         与「保存失败」文案明确区分：标读本身没有失败），并安排一次 3s 延迟
         重试；重试仍失败则放弃，依赖既有 reload 自愈（下次任意
         reloadFromBackend 重取同一计数来源，审查探针 F2 证实自愈有效）。 */
      reconcileCounts().catch(() => {
        get().showToast('全部已读已保存，未读计数刷新失败');
        setTimeout(() => {
          void reconcileCounts().catch(() => { /* 重试仍失败：放弃，计数由下次 reload 自愈 */ });
        }, 3000);
      });
    }).catch((e: unknown) => {
      /* TASK-107：失败回滚——乐观翻转到原读态、逐 feed 回补未读计数、还原
         「已读保留」快照并同步视图缓存。恢复前提从「当前值仍等于乐观写入值」
         升级为 R1 的版本守卫（值守卫保留作快速短路：值已不同必然已被接管）。
         成功 toast 已移入 .then（落库确认后才弹），此处只给失败 toast + 重试。 */
      const s = get();
      let changed = false;
      const unreadRestore = new Map<string, number>();
      const entries = s.entries.map((a) => {
        if (!ids.has(a.id)) return a;
        const prev = prevReadById.get(a.id);
        if (prev !== false || a.isRead !== true) return a;
        if (getEntryVersion(a.id, 'isRead') !== optimisticVersionById.get(a.id)) return a; // R1 + TASK-118：期间已被其他**读态**写入接管（收藏 bump 的是 isStarred 字段，不再使读回滚失效）
        changed = true;
        unreadRestore.set(a.feedId, (unreadRestore.get(a.feedId) ?? 0) + 1);
        return { ...a, isRead: prev };
      });
      let feedCounts = s.feedCounts;
      if (changed) {
        feedCounts = new Map(s.feedCounts);
        for (const [fid, delta] of unreadRestore) {
          const c = feedCounts.get(fid);
          if (c) feedCounts.set(fid, { ...c, unread: Math.max(0, c.unread + delta) });
        }
      }
      set(changed ? { entries, feedCounts, openedReadIds: prevOpenedReadIds } : { openedReadIds: prevOpenedReadIds });
      if (changed) syncCurrentViewCache(entries);
      /* TASK-067 N10：失败必须可见；本地已回滚，文案不再断言「重启后可能回退」 */
      get().showToast(`全部已读保存失败：${extractError(e)}`, { label: '重试', run: () => get().markCurrentViewAllRead() });
    });
  },
});
