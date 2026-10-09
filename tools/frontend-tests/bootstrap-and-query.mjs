// tools/frontend-tests/bootstrap-and-query.mjs
// 领域模块：启动装载、范围/视图/布局选择与查询口径
// OPT-016C 拆分自 tools/frontend-regression.mjs（旧行区间 635-705、707-756、758-836、838-887、1148-1206、1208-1268、1270-1298、2101-2107、2109-2129）；
// 断言名称/条件文本原样迁移，仅做路径深度适配（import.meta.url 与动态 import 深一层）与
// 共享可变状态的 S. 归属重写。数据所有权：共享假后端/夹具/记录归 harness（见 harness.mjs 头注），
// 本模块不 import 第二份 store；域内自带夹具（大行集等）仍在本模块内独立构造与复位。
export const id = 'bootstrap-and-query';

export async function run(ctx) {
  const { S, store, checkNew, nTick, resetStore, bootFixture, NOW, iso, mkRow, BASE_ROWS, localDayKey, selectVisibleEntries, selectScopeEntries, selectRawEntries, selectTreeCounts, selectViewCounts, bodyOf, getBodyEntry, entryNeedsHydrationT122, appStoreUnbound, internalsBeforeBind, overrideGlobal } = ctx;
  await ctx.useMainBackend();

  /* ============================================================
     (a) bootstrapFromBackend / dataMode 与错误路径
     ============================================================ */
  // R1：window 覆盖必须 finally 复原（跨 await/reject 也必须保全原 window）
  const restoreWindow = overrideGlobal(globalThis, 'window', {});   // 抹掉 __TAURI_INTERNALS__ → 浏览器预览分支
  let aBrowser;
  try {
    await resetStore({ dataMode: 'tauri', dataLoading: true, entries: [], categories: [], feedIndex: new Map() });
    await store.getState().bootstrapFromBackend();
    aBrowser = store.getState();
  } finally {
    restoreWindow();
  }
  checkNew('(a) 无 Tauri IPC 时 bootstrap 回退 mock 并装载演示数据（骨架不永挂）',
    aBrowser.dataMode === 'mock' && aBrowser.dataLoading === false
    && aBrowser.categories.length > 0 && aBrowser.entries.length > 0 && aBrowser.feedIndex.size > 0);

  await bootFixture();
  const aOk = store.getState();
  checkNew('(a) tauri bootstrap 建立 categories/feedIndex/feedCounts 并进入 tauri 模式',
    aOk.dataMode === 'tauri' && aOk.dataLoading === false && aOk.bootstrapError === null
    && aOk.categories.map((c) => c.id).join(',') === 'cat-1,cat-2'
    && aOk.feedIndex.size === 4 && aOk.feedCounts.get('10')?.unread === 3);
  /* 【TASK-052 改动理由】(a) 组这一条把「游标 = PAGE_SIZE，与实际拉回的行数无关」
     写成了期望——那正是缺陷 P1-14「口径」半边本身：游标是**全局查询的 offset**，
     于是首批无论真实返回多少行，游标都停在 500（后续 loadMore 从全局第 500 条
     继续），与「当前范围已加载 8 条」脱节。per-scope 游标下首批游标 = 实际行数，
     并且按范围键（此处 'article|all'，TASK-094 起含布局）记进 articlesCursor。 */
  checkNew('(a) 首批分页游标 = 实际行数（per-scope：不再硬编码 PAGE_SIZE）、8 行 < 500 视为已到底、加载态收起',
    aOk.entries.length === 8 && aOk.articlesLimit === 8 && aOk.articlesCursor['article|all']?.loaded === 8
    && aOk.articlesExhausted === true && aOk.articlesLoading === false);

  /* 【TASK-103 → TASK-122 改动理由】旧断言（两代）分别锁「清空」与「按 id 继承+
     裁剪 hydratedIds/hydrationErrors 平行 Map」。TASK-122 起水合终态收敛为
     bodyById 记录的 state（按文章实体记账，不随快照行传播），滞留标记在结构上
     不存在（记录键 = 文章实体 id，重现即命中自己的正文与终态）。保护意图
     （刷新不丢已水合正文、无「无请求死区」）改由 bodyById 断言承载：
     快照替换前后记录保持、正文照常可读、无需补拉。 */
  S.getArticlesPlan = { rows: [mkRow({ id: 101, feed_id: 10, content_html: '<p>101 水合正文</p>' })] };
  store.getState().hydrateArticleContent(['101']);
  await nTick(20);
  S.getArticlesPlan = null;
  const t122aCallsBefore = S.invokeCalls.filter((c) => c.cmd === 'get_articles').length;
  await store.getState().reloadFromBackend();
  checkNew('(a) 快照替换后 bodyById 记录保持（水合终态随实体不随快照行，TASK-122），已水合卡片正文照常可读且无需补拉',
    bodyOf(store.getState(), '101').content === '<p>101 水合正文</p>'
    && getBodyEntry('101')?.state === 'ready'
    && entryNeedsHydrationT122(store.getState(), '101') === false
    && S.invokeCalls.filter((c) => c.cmd === 'get_articles').length === t122aCallsBefore);

  await resetStore();
  S.failReload = { code: 'db_corrupt', message: '数据库损坏' };
  await store.getState().bootstrapFromBackend();
  const aFail = store.getState();
  checkNew('(a) tauri bootstrap 失败：错误态可见、骨架收起、绝不回退 mock 假数据',
    aFail.bootstrapError === '数据库损坏' && aFail.dataLoading === false
    && aFail.dataMode === 'tauri' && aFail.entries.length === 0 && aFail.categories.length === 0);
  S.failReload = null;
  await store.getState().retryBootstrap();
  checkNew('(a) retryBootstrap 清错误态并重新装载成功',
    store.getState().bootstrapError === null && store.getState().entries.length === 8);

  /* 代际守卫：并发 reload 只接受最新一次结果 */
  await resetStore();
  S.listPlan = { mode: 'defer' };
  const pReloadOld = store.getState().reloadFromBackend();
  const pReloadNew = store.getState().reloadFromBackend();
  await nTick(0);
  checkNew('(a) 并发 reload 时两次 list_articles 同时在途（代际守卫场景成立）', S.pendingList.length === 2);
  S.pendingList[1].resolve([mkRow({ id: 301, feed_id: 20, title: '新代际' })]);
  S.pendingList[0].resolve([mkRow({ id: 101, feed_id: 10, title: '旧代际' })]);
  await Promise.all([pReloadOld, pReloadNew]);
  S.listPlan = null;
  checkNew('(a) 旧代际 reload 结果被丢弃：entries 保持最新代际快照（布局不回退）',
    store.getState().entries.length === 1 && store.getState().entries[0].id === '301');

  /* ============================================================
     (b) 订阅源/分类选择与派生树计数
     ============================================================ */
  await bootFixture();
  store.setState({ activeFeedFilter: 'all', openedReadIds: { '999': true } });
  store.getState().selectFeed('11');
  checkNew('(b) selectFeed 写入 activeFeedFilter 并清空「已读保留」快照',
    store.getState().activeFeedFilter === '11' && Object.keys(store.getState().openedReadIds).length === 0);
  checkNew('(b) 范围过滤叠加布局解析：源B 是 social，article 布局下 scope 为空',
    selectScopeEntries(store.getState()).length === 0);
  store.setState({ activeContentLayout: 'social' });
  checkNew('(b) 切到 social 布局后 scope 出现源B 两条（范围 × 布局双重过滤）',
    selectScopeEntries(store.getState()).map((e) => e.id).join(',') === '201,202');
  store.getState().selectFeed('cat-1');
  checkNew('(b) selectFeed(分类) 覆盖该分类下当前布局的源，不含其他分类',
    selectScopeEntries(store.getState()).map((e) => e.id).join(',') === '201,202');
  store.setState({ activeContentLayout: 'article' });
  store.getState().selectFeed('cat-1');

  /* 【TASK-117 改动理由】期望顺序随并列决胜更新：BASE_ROWS 此前按插入序稳定排序，
     id 决胜后同秒组内 id 降序——TODAY 组 {201,102,101}、OLD 组 {301,202,105,104,103}；
     断言意图（集合成员与派生口径）不变，仅并列序变化。 */
  checkNew('(b) 同一分类在 article 布局下覆盖源A+源D（5 条）',
    selectScopeEntries(store.getState()).map((e) => e.id).join(',') === '102,101,105,104,103');

  store.setState({ activeViewFilter: 'all', timelineFilter: 'unread', activeFeedFilter: 'all' });
  const treeUnread = selectTreeCounts(store.getState());
  checkNew('(b) 树角标 = 布局 × 视图 × 显示筛选后的后端计数（「显示: 未读」→ 未读数）',
    treeUnread.get('10') === 3 && treeUnread.get('12') === 2 && treeUnread.get('cat-1') === 5
    && treeUnread.get('all') === 5 && treeUnread.get('11') === undefined);
  store.setState({ timelineFilter: 'all' });
  const treeAll = selectTreeCounts(store.getState());
  checkNew('(b) 显示切回「全部」后树角标变总数（分类聚合 = 各源之和）',
    treeAll.get('10') === 5 && treeAll.get('cat-1') === 7 && treeAll.get('all') === 7);
  store.setState({ activeViewFilter: 'starred' });
  const treeStar = selectTreeCounts(store.getState());
  checkNew('(b) 视图为收藏时树角标按收藏数（0 也建档，行与源一一对应）',
    treeStar.get('10') === 1 && treeStar.get('12') === 0 && treeStar.get('cat-1') === 1 && treeStar.get('all') === 1);
  store.setState({ activeContentLayout: 'social', activeViewFilter: 'all', timelineFilter: 'unread', activeFeedFilter: 'cat-1' });
  const vCounts = selectViewCounts(store.getState());
  checkNew('(b) selectFeed 后视图计数随订阅范围收敛（cat-1 只算源B）',
    vCounts.all === 2 && vCounts.unread === 2 && vCounts.starred === 2 && vCounts.today === 1);
  store.setState({ activeFeedFilter: '20' });
  const vCounts2 = selectViewCounts(store.getState());
  checkNew('(b) 单源范围视图计数 = 该源计数（源C：显示未读 → 1）',
    vCounts2.all === 1 && vCounts2.unread === 1 && vCounts2.starred === 0 && vCounts2.today === 0);
  store.setState({ activeContentLayout: 'article' });
  checkNew('(b) 选中的源不属于当前布局时：scope 空、计数全 0（布局不串台）',
    selectScopeEntries(store.getState()).length === 0
    && selectViewCounts(store.getState()).all === 0 && selectViewCounts(store.getState()).unread === 0);

  /* ============================================================
     (c) 视图筛选 / 时间线筛选对可见条目集合的影响
     ============================================================ */
  await bootFixture();
  store.setState({ activeViewFilter: 'starred' });
  await store.getState().reloadFilteredEntries('starred');

  /* 【TASK-117 改动理由】期望顺序随并列决胜更新：BASE_ROWS 此前按插入序稳定排序，
     id 决胜后同秒组内 id 降序——TODAY 组 {201,102,101}、OLD 组 {301,202,105,104,103}；
     断言意图（集合成员与派生口径）不变，仅并列序变化。 */
  checkNew('(c) 收藏视图拉取走 only_starred：entries 只剩 3 条收藏（跨布局，布局在派生层过滤）',
    store.getState().entries.map((e) => e.id).join(',') === '201,102,103');
  store.setState({ activeViewFilter: 'all', entries: [], articlesLimit: 0, articlesExhausted: false });
  store.getState().selectView('starred');
  const cCache = store.getState();
  checkNew('(c) selectView 命中视图缓存：同步恢复快照（零延迟、游标=快照长度、标记已到底）',
    cCache.activeViewFilter === 'starred' && cCache.entries.map((e) => e.id).join(',') === '201,102,103'
    && cCache.articlesLimit === 3 && cCache.articlesExhausted === true);
  await nTick(20);
  checkNew('(c) 缓存命中后的后台静默刷新不改变收藏视图结论',
    store.getState().entries.map((e) => e.id).join(',') === '201,102,103');

  store.setState({ activeViewFilter: 'all', entries: [], articlesLimit: 0, openedReadIds: { '102': true } });
  store.getState().selectView('unread');
  checkNew('(c) selectView(unread) 立即切视图并清空已读保留快照',
    store.getState().activeViewFilter === 'unread' && Object.keys(store.getState().openedReadIds).length === 0);
  await nTick(20);
  const cUnread = store.getState();
  /* 【TASK-110 改动理由】原断言名「拉全量/不再分页」编码的正是本卡废除的近似全集
     手法（limit:100000 一次拉完）。分页化后这里是**首屏**：6 行 < 页大小(500) ⇒
     exhausted 真实判定为 true。判定条件本身（6 条/游标=行数/已到底）与修前一致，
     只改措辞对齐新语义，不弱化保护。 */
  checkNew('(c) 未读视图走 only_unread 拉取首屏：只剩 6 条未读、游标=行数、不足一页即真实判定已到底',
    cUnread.entries.map((e) => e.id).join(',') === '201,101,301,105,104,103'
    && cUnread.articlesLimit === 6 && cUnread.articlesExhausted === true && cUnread.articlesLoading === false);
  store.getState().selectView('today');
  await nTick(20);
  const cToday = selectVisibleEntries(store.getState());
  checkNew('(c) 今天视图只含本地当天条目（与列表「今天」判定同口径）',
    cToday.length > 0 && cToday.every((e) => localDayKey(e.publishedAt) === localDayKey(Date.now())));

  await bootFixture();

  /* 【TASK-117 改动理由】期望顺序随并列决胜更新：BASE_ROWS 此前按插入序稳定排序，
     id 决胜后同秒组内 id 降序——TODAY 组 {201,102,101}、OLD 组 {301,202,105,104,103}；
     断言意图（集合成员与派生口径）不变，仅并列序变化。 */
  checkNew('(c) 全部视图 + 显示全部：article 布局可见 5 条，按发布时间降序（同秒由 id 决胜）',
    selectVisibleEntries(store.getState()).map((e) => e.id).join(',') === '102,101,105,104,103');
  store.getState().toggleTimelineFilter();
  checkNew('(c) toggleTimelineFilter → 未读：已读条目从可见集合移除，未读保留',
    store.getState().timelineFilter === 'unread'
    && selectVisibleEntries(store.getState()).map((e) => e.id).join(',') === '101,105,104,103');
  store.getState().toggleTimelineFilter();
  checkNew('(c) 再次 toggle 回「全部」：已读条目重新可见（可逆）',
    store.getState().timelineFilter === 'all' && selectVisibleEntries(store.getState()).length === 5);
  store.getState().toggleTimelineSort();
  checkNew('(c) toggleTimelineSort 反向排序：oldest 为时间升序',
    store.getState().timelineSort === 'oldest'
    && selectVisibleEntries(store.getState()).map((e) => e.id).join(',') === '103,104,105,101,102');
  store.getState().toggleTimelineSort();
  store.setState({ activeViewFilter: 'starred', timelineFilter: 'unread', openedReadIds: {} });
  checkNew('(c) 收藏视图不受「显示: 未读」影响：已读但收藏的 102 仍在可见集合',
    selectVisibleEntries(store.getState()).map((e) => e.id).join(',') === '102,103');
  store.setState({ activeViewFilter: 'all', timelineFilter: 'unread', openedReadIds: { '102': true } });
  checkNew('(c) 本次会话打开过的已读条目在未读筛选下原地保留（openedReadIds 生效）',
    selectVisibleEntries(store.getState()).map((e) => e.id).includes('102'));

  await bootFixture();
  S.listPlan = { mode: 'defer' };
  store.setState({ activeViewFilter: 'unread', entries: [], articlesLimit: 0 });
  const pFiltered = store.getState().reloadFilteredEntries('unread');
  await nTick(0);
  store.setState({ activeViewFilter: 'starred' });   // 拉取期间用户又切了视图
  S.pendingList[0].resolve(BASE_ROWS.filter((r) => !r.is_read));
  await pFiltered;
  S.listPlan = null;
  checkNew('(c) 拉取期间切走视图：过期筛选结果被丢弃（entries/游标不被旧视图覆盖）',
    store.getState().entries.length === 0 && store.getState().articlesLimit === 0
    && store.getState().activeViewFilter === 'starred');

  /* ============================================================
     (d) 内容布局切换（article/social/image/podcast）与派生状态
     ============================================================ */
  await bootFixture();
  const dEntriesRef = store.getState().entries;
  store.setState({
    activeFeedFilter: '11', activeArticleId: '101', openedReadIds: { '102': true },
    isShowingTranslatedProse: true, showFulltext: true, isRawRenderMode: true,
  });
  store.getState().selectLayout('social');
  const dAfter = store.getState();
  checkNew('(d) selectLayout 切布局并复位范围/选中/阅读器态（视图筛选保留）',
    dAfter.activeContentLayout === 'social' && dAfter.activeFeedFilter === 'all' && dAfter.activeArticleId === null
    && dAfter.isShowingTranslatedProse === false && dAfter.showFulltext === false && dAfter.isRawRenderMode === false
    && Object.keys(dAfter.openedReadIds).length === 0 && dAfter.activeViewFilter === 'all');
  checkNew('(d) 布局切换是纯本地过滤：entries 引用不变（不触发 reload、不闪空）',
    store.getState().entries === dEntriesRef && store.getState().articlesLoading === false);

  /* 【TASK-117 改动理由】期望顺序随并列决胜更新：BASE_ROWS 此前按插入序稳定排序，
     id 决胜后同秒组内 id 降序——TODAY 组 {201,102,101}、OLD 组 {301,202,105,104,103}；
     断言意图（集合成员与派生口径）不变，仅并列序变化。 */
  checkNew('(d) 派生集合按布局解析：article = 源A+源D，social = 源B+源C',
    selectRawEntries({ ...store.getState(), activeContentLayout: 'article' }).map((e) => e.id).join(',') === '102,101,105,104,103'
    && selectRawEntries(store.getState()).map((e) => e.id).join(',') === '201,301,202');
  store.setState({ activeContentLayout: 'image' });
  checkNew('(d) image 布局无绑定源 → 派生集合为空（不串其他布局条目）',
    selectRawEntries(store.getState()).length === 0 && selectVisibleEntries(store.getState()).length === 0);
  store.setState({ activeContentLayout: 'podcast' });
  checkNew('(d) podcast 布局初始为空（布局是绑定派生的解析结果，不是条目属性）',
    selectRawEntries(store.getState()).length === 0);
  store.setState({ activeContentLayout: 'social' });
  S.invokeCalls.length = 0;
  store.getState().updateFeedLayout('cat-1', '11', 'podcast');
  await nTick(0);   // api.updateFeedLayout 内部 await getInvoke()，落库是异步 fire-and-forget
  const dCall = S.invokeCalls.find((c) => c.cmd === 'update_feed_layout');
  checkNew('(d) updateFeedLayout 落库：纯数字 id 提取 + 布局原样写入（不被强转 inherit）',
    !!dCall && dCall.args.id === 11 && dCall.args.layout === 'podcast'
    && store.getState().feedIndex.get('11')?.feed.layout === 'podcast');
  checkNew('(d) 改绑定后条目即时迁移到新布局视图（不搬动数据：entries 仍 8 条）',
    selectRawEntries({ ...store.getState(), activeContentLayout: 'podcast' }).map((e) => e.id).join(',') === '201,202'
    && selectRawEntries(store.getState()).map((e) => e.id).join(',') === '301'
    && store.getState().entries.length === 8);
  S.invokeCalls.length = 0;
  store.getState().updateCatLayout('cat-2', 'podcast');
  await nTick(0);
  checkNew('(d) updateCatLayout 后 inherit 源的条目跟随分类布局迁移（源C 进 podcast）',
    store.getState().feedIndex.get('20')?.cat.layout === 'podcast'
    && selectRawEntries({ ...store.getState(), activeContentLayout: 'podcast' }).map((e) => e.id).join(',') === '201,301,202'
    && selectRawEntries(store.getState()).length === 0
    && S.invokeCalls.some((c) => c.cmd === 'update_folder_layout' && c.args.id === 2 && c.args.layout === 'podcast'));

  /* ============================================================
     (h) 搜索开关与结果竞态守卫
     ============================================================ */
  await bootFixture();
  store.setState({ searchOpen: false, settingsOpen: true });
  store.getState().openSearch();
  const hOpened = store.getState().searchOpen;
  store.getState().closeSearch();
  checkNew('(h) openSearch/closeSearch 只切换搜索面板态，不动其他弹层与列表',
    hOpened === true && store.getState().searchOpen === false && store.getState().settingsOpen === true);
  store.setState({ settingsOpen: false });

  S.invokeCalls.length = 0;
  await store.getState().anchorToArticle('301');
  const hAnchor = store.getState();
  /* 【TASK-117 改动理由】期望页内容随 id 决胜序更新：BASE_ROWS 的 TODAY/OLD 并列组
     此前按插入序稳定排序（101,102 / 103..301），id 决胜后并列组内 id 降序
     （102,101 / 301,202,201,105,104,103）——301 的绝对位置从 7 变 2，锚定页
     从 [301] 变为其所在窗口的 6 行；断言意图（锚定到目标所在页）不变。 */
  checkNew('(h) 搜索结果锚定：按绝对位置拉取该页并选中（不再从头拉 500 篇）',
    S.invokeCalls.some((c) => c.cmd === 'article_index')
    && hAnchor.activeArticleId === '301'
    && hAnchor.entries.map((e) => e.id).join(',') === '301,202,105,104,103'
    && hAnchor.articlesLimit === 8 && hAnchor.articlesExhausted === true
    && hAnchor.openedReadIds['301'] === true);

  await bootFixture();
  S.indexPlan = { mode: 'defer' };
  S.invokeCalls.length = 0;
  const pAnchorOld = store.getState().anchorToArticle('101');
  await nTick(0);
  const pAnchorNew = store.getState().anchorToArticle('301');
  await nTick(0);
  checkNew('(h) 两次导航同时在途（article_index 各一次，竞态场景成立）', S.pendingIndex.length === 2);
  S.pendingIndex[0].resolve(S.pendingIndex[0].value);   // 旧导航先返回 → 代际已过期
  await nTick(0);
  checkNew('(h) 过期的搜索定位结果被代际守卫丢弃：不再发 list_articles、不抢选中态',
    S.invokeCalls.filter((c) => c.cmd === 'list_articles').length === 0
    && store.getState().activeArticleId === null);
  S.pendingIndex[1].resolve(S.pendingIndex[1].value);
  await Promise.all([pAnchorOld, pAnchorNew]);
  S.indexPlan = null;
  checkNew('(h) 只有最新一次导航生效（activeArticleId=301、列表为其所在页）',
    store.getState().activeArticleId === '301'
    && store.getState().entries.map((e) => e.id).join(',') === '301,202,105,104,103');

  await bootFixture();
  store.setState({ activeFeedFilter: '12' });
  S.invokeCalls.length = 0;
  await store.getState().anchorToArticle('301');   // 301 不在源12 的筛选范围内
  checkNew('(h) 目标不在当前筛选内（article_index=null）：不改列表与选中态',
    S.invokeCalls.filter((c) => c.cmd === 'list_articles').length === 0
    && store.getState().activeArticleId === null && store.getState().entries.length === 8);

  store.setState({ dataMode: 'mock', activeArticleId: null, entries: [], articlesLimit: 0 });
  S.invokeCalls.length = 0;
  await store.getState().anchorToArticle('101');
  checkNew('(h) mock 模式下锚定打开为 no-op（无 IPC，不伪造定位结果）',
    S.invokeCalls.length === 0 && store.getState().activeArticleId === null);

  /* ============================================================
     (h2) TASK-052 锚定顺序契约：命令面板必须「先导航、后锚定」

     这是一处**行为变化**：改造前顺序是先 anchorToArticle、再前置导航。两种顺序在
     改造前等价（分页/锚定查询都不带订阅范围）；per-scope 落地后 anchorToArticle
     按**调用时**的范围构造 article_index / list_articles，旧顺序会让锚定按**旧范围**
     取位置 —— 位置与该位置的列表不同口径，锚定错位甚至直接失败。
     A/B 证据见 tmp/task052/anchor-order-*.json 与 anchor-order-diff.txt（5 项翻转）。
     ============================================================ */
  {
    const { anchorScopeNav } = await import('../../src/components/anchorScopeNav.ts');

    /* 源码顺序契约：直接读取 Overlays.tsx 里「文章」命令项 run() 的实际调用顺序。
       上面的端到端复刻只证明「这套顺序是对的」，证明不了**组件里确实这么写**——
       把顺序改回去它照样通过。这条读源码，因此能真正钉住组件实现（修前失败）。 */
    {
      const { readFileSync } = await import('node:fs');
      const ov = readFileSync(new URL('../../src/components/Overlays.tsx', import.meta.url), 'utf8');
      const artAt = ov.indexOf('id: `art-${a.id}`');
      const navAt = ov.indexOf('anchorScopeNav(st)', artAt);
      const anchorAt = ov.indexOf('void st.anchorToArticle(a.id);', artAt);
      checkNew('(h2) 源码顺序：命令面板「文章」项先归一导航、后调用 anchorToArticle（修前：先 anchor、后导航）',
        artAt >= 0 && navAt > artAt && anchorAt > navAt);
    }

    checkNew('(h2) 归一动作顺序：范围 → 视图 → 时间流筛选（数组序即执行序）',
      anchorScopeNav({ activeFeedFilter: '10', activeViewFilter: 'starred', timelineFilter: 'unread' })
        .map((s) => s.action).join(',') === 'selectFeed,selectView,toggleTimelineFilter');
    checkNew('(h2) 已在「全部范围 × 全部视图 × 非未读」时无需任何前置导航（幂等，不产生多余切换）',
      anchorScopeNav({ activeFeedFilter: 'all', activeViewFilter: 'all', timelineFilter: 'all' }).length === 0);

    /* 端到端：复刻命令面板点击，按修后顺序执行，断言锚定用的是**新范围** */
    await resetStore();
    S.backendRows = [
      mkRow({ id: 101, feed_id: 10, published_at: iso(NOW) }),
      mkRow({ id: 201, feed_id: 11, published_at: iso(NOW - 1000) }),
    ];
    store.setState({ activeFeedFilter: '10', activeViewFilter: 'starred', timelineFilter: 'unread' });
    await store.getState().reloadFromBackend();
    S.invokeCalls.length = 0;
    /* 修后顺序：先导航（用 anchorScopeNav 的产物），后锚定 */
    const st0 = store.getState();
    for (const step of anchorScopeNav(st0)) {
      if (step.action === 'selectFeed') store.getState().selectFeed(step.arg ?? 'all');
      else if (step.action === 'selectView') store.getState().selectView('all');
      else store.getState().toggleTimelineFilter();
    }
    await store.getState().anchorToArticle('201');
    const h2Idx = S.invokeCalls.find((c) => c.cmd === 'article_index');
    const h2List = S.invokeCalls.find((c) => c.cmd === 'list_articles');
    checkNew('(h2) 锚定的 article_index 按**新范围**取（feed_id=null；修前：feed_id=10 旧范围）',
      (h2Idx?.args?.args?.feed_id ?? null) === null && (h2Idx?.args?.args?.folder_id ?? null) === null);
    checkNew('(h2) article_index 与随后的 list_articles 同口径（位置与列表对齐，不会错位）',
      (h2Idx?.args?.args?.feed_id ?? null) === (h2List?.args?.args?.feed_id ?? null)
      && (h2Idx?.args?.args?.folder_id ?? null) === (h2List?.args?.args?.folder_id ?? null));
    checkNew('(h2) 目标文章确实被锚定打开（修前旧顺序下 article_index 落在源10 内查不到 201 ⇒ pos=null ⇒ 静默不打开）',
      store.getState().activeArticleId === '201' && store.getState().entries.some((e) => e.id === '201'));
    checkNew('(h2) 锚定完成后范围/视图/筛选均已归一（命令面板点击的最终态）',
      store.getState().activeFeedFilter === 'all' && store.getState().activeViewFilter === 'all'
      && store.getState().timelineFilter === 'all');
  }

  /* ============================================================
     (i) toast 的生成与消失
     ============================================================ */
  await resetStore();
  store.setState({ toasts: [] });
  store.getState().showToast('新增-A', { label: '重试', run: () => {} });
  const iFirst = store.getState().toasts;
  store.getState().showToast('新增-B');
  const iSecond = store.getState().toasts;
  checkNew('(i) showToast 追加条目、id 单调递增、action 负载原样保留',
    iSecond.length === 2 && iSecond[1].id > iSecond[0].id
    && iFirst[0].action?.label === '重试' && iSecond[1].action === undefined);
  for (let i = 0; i < 3; i += 1) store.getState().showToast(`新增-上限${i}`);
  const iCap = store.getState().toasts;
  checkNew('(i) toast 上限 4 条：超出丢弃最旧、保留最新（错误循环不无限堆叠）',
    iCap.length === 4
    && iCap.map((t) => t.text).join(',') === '新增-B,新增-上限0,新增-上限1,新增-上限2');
  store.setState({ toasts: [] });
  store.getState().showToast('新增-短');
  store.getState().showToast('新增-长', { label: '重试', run: () => {} });
  await new Promise((r) => setTimeout(r, 2800));
  const iMid = store.getState().toasts;
  checkNew('(i) 无 action 的 toast 在 ~2.4s 后自动卸载', iMid.some((t) => t.text === '新增-短') === false);
  checkNew('(i) 带 action 的 toast 停留更久（2.8s 时仍在且未进入退场）',
    iMid.some((t) => t.text === '新增-长')
    && iMid.find((t) => t.text === '新增-长')?.leaving !== true);
  await new Promise((r) => setTimeout(r, 1900));
  checkNew('(i) 带 action 的 toast 最终也会消失（两段式生命周期收敛，不泄漏 DOM）',
    store.getState().toasts.length === 0);

  /* ---------- E2：internals.appStore() 未注入时必须显式抛错 ---------- */
  checkNew('(E2) internals.appStore() 在 bindAppStore 之前调用 → 显式抛错（不再静默返回 undefined 冒充 StoreApi）',
    appStoreUnbound.threw === true
    && /bindAppStore/.test(String(appStoreUnbound.error && appStoreUnbound.error.message)));
  let e2BoundOk = false;
  try { e2BoundOk = typeof internalsBeforeBind.appStore().getState === 'function'; } catch { e2BoundOk = false; }
  checkNew('(E2) store 创建之后（bind 之后）句柄正常可用，不误抛', e2BoundOk === true);

  /* ---------- E1：bootstrapGithubAuth（唯一「slice 导出 + 晚绑定句柄」迁移点） ---------- */
  await resetStore();
  const { bootstrapGithubAuth } = await import('../../dist-test/store.js');
  S.ghLoginStatus = { login: 'octocat' };
  store.setState({ githubAccount: null });
  await bootstrapGithubAuth();
  checkNew('(E1) 后端有登录态 → 经晚绑定句柄写入 store.githubAccount（登录态启动即恢复）',
    store.getState().githubAccount?.login === 'octocat'
    && S.invokeCalls.some((c) => c.cmd === 'github_login_status'));
  S.ghLoginStatus = null;
  store.setState({ githubAccount: { login: 'keep' } });
  await bootstrapGithubAuth();
  checkNew('(E1) 后端未登录（null）→ 不误清空现有登录态',
    store.getState().githubAccount?.login === 'keep');
  S.ghLoginStatus = 'reject';
  store.setState({ githubAccount: { login: 'keep' } });
  let e1Threw = false;
  try { await bootstrapGithubAuth(); } catch { e1Threw = true; }
  checkNew('(E1) 后端不可用（IPC 失败）→ 静默忽略：不抛出、不污染状态',
    e1Threw === false && store.getState().githubAccount?.login === 'keep');
  S.ghLoginStatus = null;
}
