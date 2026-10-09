// tools/frontend-tests/pagination-and-cache.mjs
// 领域模块：分页游标（keyset/per-scope）、视图缓存与竞态代际、窗口重取
// OPT-016C 拆分自 tools/frontend-regression.mjs（旧行区间 1055-1146、2311-2976、3309-3475、3477-3685、3687-3862、4034-4218）；
// 断言名称/条件文本原样迁移，仅做路径深度适配（import.meta.url 与动态 import 深一层）与
// 共享可变状态的 S. 归属重写。数据所有权：共享假后端/夹具/记录归 harness（见 harness.mjs 头注），
// 本模块不 import 第二份 store；域内自带夹具（大行集等）仍在本模块内独立构造与复位。
export const id = 'pagination-and-cache';

export async function run(ctx) {
  const { S, store, checkNew, nTick, resetStore, bootFixture, NOW, OLD, iso, mkRow, queryRows, countsFromRows, selectVisibleEntries, bodyOf, getBodyEntry, getBodyEntryT122, entryNeedsHydrationT122, overrideGlobal } = ctx;
  await ctx.useMainBackend();

  /* ============================================================
     (g) 分页 loadMoreArticles 的游标推进与失败路径
     ============================================================ */
  await resetStore();
  const pageRows = [];
  for (let i = 0; i < 503; i += 1) {
    pageRows.push(mkRow({ id: 5000 + i, feed_id: 10, title: `分页${i}`, published_at: iso(NOW - i * 60000) }));
  }
  S.backendRows = pageRows;
  await store.getState().bootstrapFromBackend();
  const gFirst = store.getState();
  checkNew('(g) 首批 = 500 行：游标 500、未到底、加载态已复位',
    gFirst.entries.length === 500 && gFirst.articlesLimit === 500
    && gFirst.articlesExhausted === false && gFirst.articlesLoading === false);
  S.invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  const gMore = store.getState();
  const gListCall = S.invokeCalls.find((c) => c.cmd === 'list_articles');
  /* 【TASK-117 改动理由】续拉 wire 从 offset=500 改为 keyset 锚（last_published/
     last_id = 首屏最后一行 5499 的原文锚）——offset 正是本卡废除的缺陷手法，
     wire 断言随行为变化更新。 */
  checkNew('(g) 追加加载以 keyset 锚续拉（last_published/last_id=首屏末行 5499，无 offset）：500→503，条目顺序连续不重复',
    gListCall?.args.args.last_published === iso(NOW - 499 * 60000) && gListCall?.args.args.last_id === 5499
    && gListCall?.args.args.offset === undefined
    && gMore.articlesLimit === 503 && gMore.entries.length === 503
    && gMore.entries[500].id === '5500' && gMore.entries[502].id === '5502');
  checkNew('(g) 不足一页 → 置 articlesExhausted 且游标按实际行数推进',
    gMore.articlesExhausted === true && gMore.articlesLoading === false);
  S.invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  checkNew('(g) 已到底后再触发不产生新 IPC（无效请求被挡）',
    S.invokeCalls.length === 0 && store.getState().articlesLimit === 503);

  store.setState({ articlesExhausted: false, articlesLoading: true, articlesLimit: 503 });
  S.invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  checkNew('(g) 在途加载中重复触发被防抖（不产生并发 IPC）',
    S.invokeCalls.length === 0 && store.getState().articlesLoading === true);

  store.setState({ articlesLoading: false, articlesExhausted: false, articlesLimit: 100, entries: [], toasts: [] });
  S.listPlan = { mode: 'reject', error: { message: 'db busy' } };
  S.invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  const gFail = store.getState();
  checkNew('(g) 分页失败：articlesLoading 复位（不永挂加载动画）且游标/条目不被破坏',
    gFail.articlesLoading === false && gFail.articlesLimit === 100
    && gFail.entries.length === 0 && gFail.articlesExhausted === false);
  /* 【改动理由】原断言把 D2 的现状（失败被静默吞掉、用户侧零提示）写成了期望，
     标注为「观察项」。D2 修复后失败路径给出可见 toast + 一键重试，故改为断言修复后的行为。 */
  checkNew('(g) 分页失败给出可见 toast（不再是静默吞错：文案 + 「重试」action —— D2 修复项）',
    gFail.toasts.length === 1 && gFail.toasts[0].text.includes('加载更多失败')
    && gFail.toasts[0].action?.label === '重试');
  checkNew('(g) 失败 toast 的「重试」直接重发分页请求（不是死按钮）',
    typeof gFail.toasts[0]?.action?.run === 'function');
  S.listPlan = null;

  store.setState({ articlesLoading: false, articlesExhausted: false, articlesLimit: 100, entries: [] });
  S.listPlan = { mode: 'defer' };
  const pMore = store.getState().loadMoreArticles();
  await nTick(0);
  /* 模拟期间发生 reload 重置游标：TASK-117 起守卫比对 per-scope keyset 游标，
     重置必须写游标本身（真实 reload 正是镜像+游标原子同写） */
  store.setState({ articlesLimit: 0, articlesLoading: false, articlesCursor: { 'article|all': { lastPublished: null, lastId: null, loaded: 0 } } });
  S.pendingList[0].resolve([mkRow({ id: 7001, feed_id: 10 })]);
  await pMore;
  S.listPlan = null;
  checkNew('(g) 加载期间游标被 reload 重置：过期追加被丢弃（不产生错位条目）',
    store.getState().entries.length === 0 && store.getState().articlesLimit === 0
    && store.getState().articlesLoading === false);

  /* D3：竞态丢弃分支必须复位 articlesLoading。上面那条场景在丢弃前手动置了
     articlesLoading=false，把这个缺陷掩盖了 —— 这里保留在途加载态（模拟真实
     竞态：加载期间 selectView 命中视图缓存恢复快照，只写游标不碰 loading），
     修前 articlesLoading 会永久停在 true，入口守卫随即永久挡住后续所有分页。 */
  /* TASK-117：先给游标一个锚（上一场景的空游标已被重置），随后在途期间重置为
     空游标——守卫比对 per-scope keyset 游标，锚漂移即过期（重置须写游标本身，
     真实 reload 正是镜像+游标原子同写） */
  store.setState({ articlesLoading: false, articlesExhausted: false, articlesLimit: 100, entries: [], articlesCursor: { 'article|all': { lastPublished: iso(NOW - 100000), lastId: 5499, loaded: 100 } } });
  S.listPlan = { mode: 'defer' };
  const pRace = store.getState().loadMoreArticles();
  await nTick(0);
  store.setState({ articlesLimit: 0, articlesCursor: { 'article|all': { lastPublished: null, lastId: null, loaded: 0 } } });
  S.pendingList[S.pendingList.length - 1].resolve([mkRow({ id: 7100, feed_id: 10 })]);
  await pRace;
  S.listPlan = null;
  checkNew('(g) 过期追加被丢弃时复位 articlesLoading（D3 修复项：加载态不永久为真）',
    store.getState().articlesLoading === false && store.getState().entries.length === 0
    && store.getState().articlesLimit === 0);
  S.invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  checkNew('(g) 竞态丢弃后入口守卫不再被永久锁死：下一次分页请求照常发出（D3 修复项）',
    S.invokeCalls.filter((c) => c.cmd === 'list_articles').length === 1);

  /* ============================================================
     TASK-052：per-scope 分页游标（缺陷 P1-14「口径」半边）
     覆盖 (s1)…(s6)，全部为本轮新增断言（checkNew）。

     被验证的口径：分页请求必须带当前订阅范围（feed_id / folder_id），
     游标按范围分桶；单源/单分类视图下「第 2 页」= 该范围的第 501..1000 条，
     而不是全局序列的第 501..1000 条。同时锚定 051 的 D2/D3 两条修复不回退。

     修前失败证据（见回归产出 run-before.log）：本组在改造前跑，
     §(s1) 会因 `loadMoreArticles` 不发 feed_id 而拿到全局下一页（含其他源的行）；
     §(s4) 会因只有单一全局游标而让 B 源续着 A 源的游标；§(s3) 会因游标恒为
     首批 PAGE_SIZE 而永远「未到底」，空列表场景无法收敛。
     ============================================================ */
  const { scopeQueryArgs, scopePageKey, viewEntriesCache } = await import('../../dist-test/store/internals.js');
  checkNew('(s0) 范围键与查询参数口径：纯数字/前缀 id 都归一到数字，cat- 走 folder_id，all 两者皆 null，排序进 newest_first',
    scopePageKey('all') === 'all' && scopePageKey('10') === '10' && scopePageKey('cat-1') === 'cat-1'
    && JSON.stringify(scopeQueryArgs('all', 'newest')) === JSON.stringify({ feed_id: null, folder_id: null, newest_first: true })
    && JSON.stringify(scopeQueryArgs('12', 'oldest')) === JSON.stringify({ feed_id: 12, folder_id: null, newest_first: false })
    && JSON.stringify(scopeQueryArgs('feed-12', 'newest')) === JSON.stringify({ feed_id: 12, folder_id: null, newest_first: true })
    && JSON.stringify(scopeQueryArgs('cat-1', 'newest')) === JSON.stringify({ feed_id: null, folder_id: 1, newest_first: true }));

  /* ---------- (s1) 单源视图：连续翻页取回该源的后续文章，且两页不重叠 ----------
     数据：源A(feed 10) 1200 条（按时间降序，id 6000+i），源B(feed 11) 1200 条
     （id 7000+i）。全局序列是「源A 6000.. 与 源B 7000.. 交错」——改造前分页
     不带 feed_id，第 2 页会取到全局第 500..999 条（混着源B），断言随即失败。 */
  await resetStore();
  const s1Rows = [];
  for (let i = 0; i < 1200; i += 1) {
    s1Rows.push(mkRow({ id: 6000 + i, feed_id: 10, title: `A${i}`, published_at: iso(NOW - i * 1000) }));
    s1Rows.push(mkRow({ id: 7000 + i, feed_id: 11, title: `B${i}`, published_at: iso(NOW - i * 1000 - 500) }));
  }
  S.backendRows = s1Rows;
  store.getState().selectFeed('10');
  await store.getState().reloadFromBackend();
  const s1First = store.getState();
  checkNew('(s1) 单源视图首批：查询带 feed_id，条目全部属于该源，游标 = 该范围已加载数（500）',
    s1First.entries.length === 500 && s1First.entries.every((e) => e.feedId === '10')
    && s1First.articlesLimit === 500 && s1First.articlesCursor['article|10']?.loaded === 500
    && s1First.articlesExhausted === false);
  S.invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  const s1Page2Call = S.invokeCalls.find((c) => c.cmd === 'list_articles');
  const s1Second = store.getState();
  /* 源A 的第 1 页 = 源A 里时间最新的 500 条（s1Rows 是源A/源B 交错的，不能直接切前 500 行） */
  const s1Ids1 = new Set(
    s1Rows.filter((r) => r.feed_id === 10)
      .sort((x, y) => Date.parse(y.published_at) - Date.parse(x.published_at))
      .slice(0, 500)
      .map((r) => String(r.id)),
  );
  const s1Page2 = s1Second.entries.slice(500).map((e) => e.id);
  /* 【TASK-117 改动理由】续拉 wire 从 offset=500 改为 keyset 锚——源A 首页末行
     id 6499（i=499），锚点原文透传。offset 正是本卡废除的缺陷手法。 */
  checkNew('(s1) 第 2 页请求沿用同一范围与 keyset 锚（feed_id=10 / last_id=6499 / newest_first）',
    s1Page2Call?.args.args.feed_id === 10 && s1Page2Call?.args.args.folder_id === null
    && s1Page2Call?.args.args.last_id === 6499 && s1Page2Call?.args.args.last_published === iso(NOW - 499 * 1000)
    && s1Page2Call?.args.args.offset === undefined && s1Page2Call?.args.args.newest_first === true);
  checkNew('(s1) 第 2 页取回该源的后续文章：全部属于 feed 10，且与第 1 页 id 集合不重叠',
    s1Second.entries.length === 1000 && s1Second.entries.every((e) => e.feedId === '10')
    && s1Page2.length === 500 && s1Page2.every((id) => !s1Ids1.has(id))
    && s1Page2[0] === '6500' && s1Page2[499] === '6999'
    && s1Second.articlesLimit === 1000 && s1Second.articlesCursor['article|10']?.loaded === 1000);

  /* ---------- (s4) per-scope 游标互不污染：A 源翻到第 2 页后切 B 源，B 从第 1 页开始 ---------- */
  /* TASK-063：selectFeed 自带接线（缓存命中恢复 / 未命中自动重拉）——此处不再
     手工调用 reloadFromBackend（旧写法模拟了 UI 中不存在的一步，掩盖了 Sidebar
     未接线的事实），改为等待 selectFeed 自身触发的重拉落地。 */
  S.invokeCalls.length = 0;
  store.getState().selectFeed('11');
  const s4Switch = store.getState();
  checkNew('(s4) 切到未曾加载的源B：游标从 0 起步（不继承源A 的 1000）',
    s4Switch.activeFeedFilter === '11' && s4Switch.articlesLimit === 0
    && s4Switch.articlesCursor['article|10']?.loaded === 1000 && s4Switch.articlesCursor['article|11'] === undefined);
  await nTick(30);
  const s4FirstCall = S.invokeCalls.find((c) => c.cmd === 'list_articles');
  const s4First = store.getState();
  checkNew('(s4) selectFeed 自动重拉：源B 首批查询按源B 的第 1 页取（feed_id=11 / offset=0），条目全属源B',
    s4FirstCall?.args.args.feed_id === 11 && s4FirstCall?.args.args.offset === 0
    && s4First.entries.length === 500 && s4First.entries.every((e) => e.feedId === '11')
    && s4First.articlesCursor['article|11']?.loaded === 500 && s4First.articlesCursor['article|10']?.loaded === 1000
    && s4First.entries.slice(0, 500).every((e) => !s1Ids1.has(e.id)));
  S.invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  const s4Call = S.invokeCalls.find((c) => c.cmd === 'list_articles');
  /* 【TASK-117 改动理由】续拉 wire 从 offset=500 改为 keyset 锚——源B 首页末行
     id 7499；判别意图不变：锚取自**源B 自己的游标**（而非源A 的 1000）。 */
  checkNew('(s4) 源B 翻页用**源B 自己的游标**（keyset 锚 last_id=7499 而非源A 的锚）',
    s4Call?.args.args.feed_id === 11 && s4Call?.args.args.last_id === 7499
    && s4Call?.args.args.last_published === iso(NOW - 499 * 1000 - 500)
    && s4Call?.args.args.offset === undefined
    && store.getState().articlesCursor['article|11']?.loaded === 1000 && store.getState().articlesCursor['article|10']?.loaded === 1000);
  /* 切回源A：TASK-063 新契约——缓存命中同步恢复该范围快照（零延迟，不经 await），
     游标=快照长度（500，可继续翻页）；源B 的游标 1000 不被污染。
     【TASK-103 → TASK-122 改动理由】「恢复时清滞留水合状态」锁的是 hydratedIds
     平行 Map 的滞留危害（不属于本快照的「已水合」标记阻断重水合 → 空卡死区）。
     TASK-122 起水合状态收敛为 bodyById 记录（键 = 文章实体 id，与视图行无关）：
     滞留形态在结构上不存在——记录只可能对「确实水合过的文章」为 ready，该文章
     重现（任何快照）时正文与终态同体命中，正是所需行为。保护意图改由
     「恢复后已水合文章正文立即可读、未水合文章正常判定需要水合」承载：
     先对源A 的 10 号行水合，切走再切回，正文零丢失且不重拉。 */
  /* 当前视图 = 源B（7000 号行在册）：水合其一 → selectFeed('10') 缓存命中恢复源A */
  S.getArticlesPlan = { rows: [mkRow({ id: 7000, feed_id: 11, content_html: '<p>源B 7000 号水合正文</p>' })] };
  store.getState().hydrateArticleContent(['7000']);
  await nTick(20);
  S.getArticlesPlan = null;
  const t122s4CallsBefore = S.invokeCalls.filter((c) => c.cmd === 'get_articles').length;
  store.getState().selectFeed('10');
  const s4Back = store.getState();
  checkNew('(s4) 切回源A：同步恢复该范围快照（零延迟），游标=快照长度且两源互不污染；已水合文章（源B 7000）正文随 bodyById 零丢失、不重拉，源A 未水合条目照常判定需要水合（TASK-122）',
    s4Back.entries.length === 500 && s4Back.entries.every((e) => e.feedId === '10')
    && s4Back.articlesLimit === 500 && s4Back.articlesCursor['article|10']?.loaded === 500
    && s4Back.articlesCursor['article|11']?.loaded === 1000
    && bodyOf(s4Back, '7000').content === '<p>源B 7000 号水合正文</p>'
    && getBodyEntry('7000')?.state === 'ready'
    && entryNeedsHydrationT122(s4Back, '7000') === false
    && entryNeedsHydrationT122(s4Back, '6000') === true
    && S.invokeCalls.filter((c) => c.cmd === 'get_articles').length === t122s4CallsBefore);
  await nTick(30);
  checkNew('(s4) 切回源A 后的后台刷新保持该范围快照结论',
    store.getState().entries.every((e) => e.feedId === '10') && store.getState().articlesCursor['article|10']?.loaded === 500);

  /* ---------- (s4b) TASK-063 附加契约：selectView 缓存恢复不丢已水合正文；mock 模式不接线 ----------
     【TASK-103 → TASK-122 改动理由】同 (s4)：滞留 hydratedIds 的危害形态随平行
     Map 移除而结构消失，保护意图（恢复后正文可用、不死区）改由 bodyById 断言
     承载：水合收藏视图条目 → 切走 → selectView 缓存命中恢复 → 正文零丢失且
     不重拉。 */
  await bootFixture();
  store.setState({ activeViewFilter: 'starred' });
  await store.getState().reloadFilteredEntries('starred');
  S.getArticlesPlan = { rows: [mkRow({ id: 103, feed_id: 10, is_starred: true, content_html: '<p>103 水合正文</p>' })] };
  store.getState().hydrateArticleContent(['103']);
  await nTick(20);
  S.getArticlesPlan = null;
  const t122s4bCallsBefore = S.invokeCalls.filter((c) => c.cmd === 'get_articles').length;
  store.setState({ activeViewFilter: 'all', entries: [] });
  store.getState().selectView('starred');
  checkNew('(s4b) selectView 缓存命中恢复：已水合正文随 bodyById 零丢失、无需补拉（防「永不重水合」空窗，TASK-122）',
    store.getState().activeViewFilter === 'starred'
    && store.getState().entries.length > 0
    && bodyOf(store.getState(), '103').content === '<p>103 水合正文</p>'
    && getBodyEntry('103')?.state === 'ready'
    && entryNeedsHydrationT122(store.getState(), '103') === false
    && S.invokeCalls.filter((c) => c.cmd === 'get_articles').length === t122s4bCallsBefore);

  await resetStore({ dataMode: 'mock' });
  S.invokeCalls.length = 0;
  store.getState().selectFeed('11');
  checkNew('(s4b) mock 模式 selectFeed 保持纯游标镜像：不触发 IPC、不翻转数据模式',
    store.getState().dataMode === 'mock' && store.getState().activeFeedFilter === '11'
    && !S.invokeCalls.some((c) => c.cmd === 'list_articles'));

  /* ---------- (s2) 单分类视图：同一口径成立（folder_id） ----------
     分类 cat-1 = 源10 + 源12；每源 600 条，全局 1200 条。分类的第 2 页
     必须是该分类内第 500..999 条（改造前会取到全局第 500..999 条 = 混入源20）。 */
  await resetStore();
  const s2Rows = [];
  for (let i = 0; i < 600; i += 1) {
    s2Rows.push(mkRow({ id: 8000 + i, feed_id: 10, title: `A${i}`, published_at: iso(NOW - i * 1000) }));
    s2Rows.push(mkRow({ id: 9000 + i, feed_id: 12, title: `D${i}`, published_at: iso(NOW - i * 1000 - 300) }));
    s2Rows.push(mkRow({ id: 9500 + i, feed_id: 20, title: `C${i}`, published_at: iso(NOW - i * 1000 - 600) }));
  }
  S.backendRows = s2Rows;
  store.getState().selectFeed('cat-1');
  await store.getState().reloadFromBackend();
  const s2First = store.getState();
  const s2First50 = s2First.entries.map((e) => e.id);
  checkNew('(s2) 单分类视图首批：查询带 folder_id，条目只含该分类的源，游标 500',
    s2First.entries.length === 500 && s2First.entries.every((e) => e.feedId === '10' || e.feedId === '12')
    && s2First.articlesLimit === 500 && s2First.articlesCursor['article|cat-1']?.loaded === 500);
  S.invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  const s2Call = S.invokeCalls.find((c) => c.cmd === 'list_articles');
  const s2Second = store.getState();
  /* 【TASK-117 改动理由】续拉 wire 从 offset=500 改为 keyset 锚——分类首页末行
     id 9249（feed12 i=249，交错序的最后一行），锚点原文透传。 */
  checkNew('(s2) 第 2 页请求沿用分类口径与 keyset 锚（folder_id=1 / feed_id=null / last_id=9249）',
    s2Call?.args.args.folder_id === 1 && s2Call?.args.args.feed_id === null
    && s2Call?.args.args.last_id === 9249 && s2Call?.args.args.last_published === iso(NOW - 249300)
    && s2Call?.args.args.offset === undefined);
  checkNew('(s2) 第 2 页取回该分类的后续文章（不含分类外源C），且与第 1 页不重叠',
    s2Second.entries.length === 1000 && s2Second.entries.every((e) => e.feedId === '10' || e.feedId === '12')
    && s2Second.entries.slice(500).every((e) => !s2First50.includes(e.id))
    && s2Second.articlesCursor['article|cat-1']?.loaded === 1000);

  /* ---------- (s3) 列表为空也能推进：单源视图下该源没有文章时，游标不再被首批截断 ----------
     改造前：首批游标恒为 PAGE_SIZE(500)，即使 `entries` 里一条该源的文章都没有，
     `articlesLimit=500`、`articlesExhausted=false`；列表为空 ⇒ 哨兵不渲染 ⇒ 无滚动 ⇒
     老文章永远够不到。改造后单源首批查询带 feed_id：该源确实没有文章时后端返回
     空页 ⇒ 立即收敛为「已到底」，空列表有终态而不是假装还有 500 条。 */
  await resetStore();
  S.backendRows = [mkRow({ id: 111, feed_id: 20, title: '只有源C 有文章' })];
  store.getState().selectFeed('10');
  await store.getState().reloadFromBackend();
  const s3Empty = store.getState();
  checkNew('(s3) 空范围首批：查询带 feed_id、游标 = 实际 0 行、立即收敛为已到底（不再假称还有 500 条）',
    s3Empty.entries.length === 0 && s3Empty.articlesLimit === 0 && s3Empty.articlesCursor['article|10']?.loaded === 0
    && s3Empty.articlesExhausted === true && s3Empty.articlesLoading === false);
  S.invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  checkNew('(s3) 空范围 + 已到底：入口守卫拦住无意义请求（不产生 IPC 空转）',
    S.invokeCalls.filter((c) => c.cmd === 'list_articles').length === 0);
  /* 反例（改造前会停在这里）：首批满页但全部被范围筛掉 ⇒ 未到底 + 空列表 ⇒ 必须还能推进 */
  S.backendRows = [];
  for (let i = 0; i < 503; i += 1) {
    S.backendRows.push(mkRow({ id: 4000 + i, feed_id: 20, title: `C${i}`, published_at: iso(NOW - i * 1000) }));
  }
  await resetStore();
  S.backendRows = [];
  for (let i = 0; i < 503; i += 1) {
    S.backendRows.push(mkRow({ id: 4000 + i, feed_id: 20, title: `C${i}`, published_at: iso(NOW - i * 1000) }));
  }
  store.setState({ dataMode: 'tauri', activeFeedFilter: '10' });
  /* 模拟改造前的口径：首批按全局拉满 500 行，范围里一条都没有。
     TASK-117：游标模拟值同步升为 keyset 形态（锚值任意——本范围查询结果为空，
     锚不参与判定；断言只看 wire 带了锚与范围）。 */
  store.setState({ entries: [], articlesLimit: 500, articlesCursor: { 'article|10': { lastPublished: iso(NOW - 500000), lastId: 3999, loaded: 500 } }, articlesExhausted: false });
  S.invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  const s3Advance = S.invokeCalls.find((c) => c.cmd === 'list_articles');
  /* 【TASK-117 改动理由】续拉游标从 offset=500 改为 keyset 锚（last_published/last_id
     成对）——offset 正是本卡废除的缺陷手法，wire 断言随行为变化更新。 */
  checkNew('(s3) 列表为空且未到底时仍可发起下一页：请求带当前范围与 keyset 锚（feed_id=10 / last_id=3999 / last_published=锚原文，无 offset）',
    s3Advance?.args.args.feed_id === 10 && s3Advance?.args.args.last_id === 3999
    && s3Advance?.args.args.last_published === iso(NOW - 500000) && s3Advance?.args.args.offset === undefined);
  checkNew('(s3) 该源确无更多数据时空页把状态收敛为「已到底 + 空列表」（补拉不会无限循环）',
    store.getState().entries.length === 0 && store.getState().articlesExhausted === true
    && store.getState().articlesLoading === false && store.getState().articlesCursor['article|10']?.loaded === 500);
  /* 再触发：守卫挡住（已到底） */
  S.invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  checkNew('(s3) 收敛后再触发不再发 IPC（空列表补拉不会反复打后端）',
    S.invokeCalls.filter((c) => c.cmd === 'list_articles').length === 0);

  /* ---------- (s5) D2/D3 守住（051 修复不回退） ---------- */
  await resetStore();
  /* TASK-117：游标模拟值升为 keyset 形态（锚值确定性给出，wire 断言核对原文透传） */
  store.setState({ dataMode: 'tauri', activeFeedFilter: '10', entries: [], articlesLimit: 100, articlesCursor: { 'article|10': { lastPublished: iso(NOW - 100000), lastId: 4999, loaded: 100 } }, articlesExhausted: false, toasts: [] });
  S.listPlan = { mode: 'reject', error: { message: 'db busy' } };
  S.invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  const s5Fail = store.getState();
  checkNew('(s5/D2) 分页失败仍给出可见 toast（文案 + 「重试」action），不是静默吞错',
    s5Fail.toasts.length === 1 && s5Fail.toasts[0].text.includes('加载更多失败')
    && s5Fail.toasts[0].text.includes('db busy') && s5Fail.toasts[0].action?.label === '重试'
    && typeof s5Fail.toasts[0].action?.run === 'function');
  checkNew('(s5/D2) 失败路径同样复位加载态且不破坏游标/条目',
    s5Fail.articlesLoading === false && s5Fail.articlesLimit === 100
    && s5Fail.articlesCursor['article|10']?.loaded === 100 && s5Fail.entries.length === 0);
  /* D2 的重试按钮真的能重发（把后端恢复后点重试） */
  S.listPlan = null;
  S.invokeCalls.length = 0;
  s5Fail.toasts[0].action.run();
  await nTick(20);
  /* 【TASK-117 改动理由】续拉 wire 从 offset=100 改为 keyset 锚——offset 正是
     本卡废除的缺陷手法，wire 断言随行为变化更新（锚值 = 上方模拟游标的原文）。 */
  checkNew('(s5/D2) 失败 toast 的「重试」确实重发分页请求（按当前范围 + keyset 锚，不是死按钮）',
    S.invokeCalls.filter((c) => c.cmd === 'list_articles').length >= 1
    && S.invokeCalls.some((c) => c.cmd === 'list_articles' && c.args.args.feed_id === 10
      && c.args.args.last_id === 4999 && c.args.args.last_published === iso(NOW - 100000)));
  S.listPlan = null;

  /* D3：竞态丢弃分支必须复位 articlesLoading（且新增的 scopeKey 收紧后依然如此）。
     场景：在途加载期间用户切到另一个范围（selectFeed 会把游标换成新范围的值，
     可能恰好等于 in-flight 的锚点数值——旧的「只比数值」判据会漏判）。
     TASK-117：游标模拟值升为 keyset 形态（两范围锚点数值刻意相同——串台判别前提）。 */
  await resetStore();
  store.setState({ dataMode: 'tauri', activeFeedFilter: '10', entries: [], articlesLimit: 100, articlesCursor: { 'article|10': { lastPublished: iso(NOW - 100000), lastId: 4999, loaded: 100 }, 'article|11': { lastPublished: iso(NOW - 100000), lastId: 4999, loaded: 100 } }, articlesExhausted: false, articlesLoading: false });
  S.listPlan = { mode: 'defer' };
  const s5Race = store.getState().loadMoreArticles();
  await nTick(0);
  checkNew('(s5/D3) 竞态场景成立：分页请求在途且加载态为真', store.getState().articlesLoading === true && S.pendingList.length === 1);
  store.getState().selectFeed('11');   // 切范围：游标换成源B 的 100（数值与 offset 相同）
  S.pendingList[0].resolve([mkRow({ id: 6001, feed_id: 10, title: '源A 的迟到数据' })]);
  await s5Race;
  const s5RaceAfter = store.getState();
  checkNew('(s5/D3) 范围已变（scopeKey 不同）⇒ 迟到页被丢弃，且 articlesLoading 复位（不永久为真）',
    s5RaceAfter.articlesLoading === false && s5RaceAfter.activeFeedFilter === '11'
    && s5RaceAfter.entries.length === 0);
  S.listPlan = null;   // 退出 defer 模式：下面这一次必须真的走完（否则永远挂起）
  /* TASK-100 P3-1：selectFeed 触发的 reload 此刻仍在途（其 list_articles 也被 defer 过），
     新守卫「reload 在途拦截续拉」会拦住下面这一次 loadMore——先放行在途 reload 落地
     （回满一页源B 数据，使 exhausted=false），再验证续拉恢复。 */
  S.pendingList[S.pendingList.length - 1]?.resolve(
    Array.from({ length: 500 }, (_, i) => mkRow({ id: 6200 + i, feed_id: 11 })),
  );
  await nTick(20);
  S.invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  checkNew('(s5/D3) 竞态丢弃后入口守卫没被锁死：下一次分页照常发出（且用新范围源B）',
    S.invokeCalls.filter((c) => c.cmd === 'list_articles').length === 1
    && S.invokeCalls.find((c) => c.cmd === 'list_articles')?.args.args.feed_id === 11);

  /* 同范围内的「游标被 reload 重置」这条既有 D3 场景，在新判据下也必须仍然丢弃 */
  await resetStore();
  store.setState({ dataMode: 'tauri', activeFeedFilter: '10', entries: [], articlesLimit: 100, articlesCursor: { 'article|10': { lastPublished: iso(NOW - 100000), lastId: 4999, loaded: 100 } }, articlesExhausted: false });
  S.listPlan = { mode: 'defer' };
  const s5Race2 = store.getState().loadMoreArticles();
  await nTick(0);
  /* TASK-117：重置后的空游标形态（无锚） */
  store.setState({ articlesLimit: 0, articlesCursor: { 'article|10': { lastPublished: null, lastId: null, loaded: 0 } } });   // 同范围内游标被重置
  S.pendingList[S.pendingList.length - 1].resolve([mkRow({ id: 6100, feed_id: 10 })]);
  await s5Race2;
  S.listPlan = null;
  checkNew('(s5/D3) 同范围内游标被重置：迟到页仍被丢弃并复位加载态（既有竞态语义不变）',
    store.getState().articlesLoading === false && store.getState().entries.length === 0
    && store.getState().articlesLimit === 0);

  /* ---------- (s6) 视图/范围切换时游标与内容一致（缓存也不串范围） ---------- */
  await resetStore();
  /* 清掉前面用例留下的视图缓存：本组要独立验证「缓存按 布局×视图×范围 分桶」 */
  viewEntriesCache.clear();
  const s6Rows = [];
  for (let i = 0; i < 40; i += 1) {
    s6Rows.push(mkRow({ id: 2000 + i, feed_id: 10, title: `A${i}`, published_at: iso(NOW - i * 1000), is_starred: i < 3 }));
    s6Rows.push(mkRow({ id: 3000 + i, feed_id: 12, title: `D${i}`, published_at: iso(NOW - i * 1000 - 400), is_starred: i < 2 }));
  }
  S.backendRows = s6Rows;
  store.getState().selectFeed('10');
  await store.getState().reloadFromBackend();
  const s6A = store.getState();
  checkNew('(s6) 单源首批：40 条全属源A、游标 40、因不足一页而到底',
    s6A.entries.length === 40 && s6A.entries.every((e) => e.feedId === '10')
    && s6A.articlesLimit === 40 && s6A.articlesExhausted === true);
  /* 同源切视图（收藏）：口径 = 同一范围 + 更窄筛选，游标 = 该筛选结果行数 */
  store.getState().selectView('starred');
  await nTick(20);
  const s6Starred = store.getState();
  checkNew('(s6) 同源切「收藏」：查询仍带 feed_id，游标 = 该筛选实际行数（不是 40）',
    s6Starred.entries.length === 3 && s6Starred.entries.every((e) => e.feedId === '10')
    && s6Starred.articlesLimit === 3 && s6Starred.articlesCursor['article|10']?.loaded === 3
    && s6Starred.articlesExhausted === true);
  /* 视图缓存必须按范围分桶：切回 all 时不能把「源A 收藏」当成「源A 全部」恢复。
     关键证据是**同步恢复**那一步（缓存命中在 reload 之前同步生效，后台刷新随后才到）：
     缓存若不按视图分桶，切回 all 会拿「收藏的 3 条」冒充「全部的 40 条」。 */
  S.listPlan = { mode: 'defer' };   // 冻结后台刷新，只观察缓存恢复本身
  store.getState().selectView('all');
  const s6BackSync = store.getState();
  /* 【TASK-110 改动理由】原断言把「缓存恢复时不知道是否到底 ⇒ 保守标 false」写成
     期望——那是筛选视图全集拉取时代（无法从快照得知尽端）的防错手法。分页化后
     缓存快照本身记录了「最近一次拉取是否满一页」：40 行 < 页大小 ⇒ 该范围确实
     只有 40 条，exhausted=true 是真实判定而非误标（selectView 与 selectLayout/
     selectFeed 同口径收口）。原保护意图（老文章不因误标已到底而不可达）由更强的
     机制接棒：恢复后必触发后台 reload 重取真值；本场景 40 条即全量，true 准确。 */
  checkNew('(s6) 切回「全部」缓存命中：同步恢复的是源A 的 40 条（不拿收藏视图的 3 条冒充），游标随之对齐、exhausted 按快照长度真实判定（40<500=已到底）',
    s6BackSync.entries.length === 40 && s6BackSync.entries.every((e) => e.feedId === '10')
    && s6BackSync.articlesLimit === 40 && s6BackSync.articlesCursor['article|10']?.loaded === 40
    && s6BackSync.articlesExhausted === true && s6BackSync.articlesLoading === false);
  S.listPlan = null;
  await nTick(20);
  checkNew('(s6) 后台静默刷新完成后结论不变（仍是源A 的 40 条）',
    store.getState().entries.length === 40 && store.getState().entries.every((e) => e.feedId === '10'));
  /* 切到另一范围：列表必须换成新范围，且不继承源A 的游标 */
  store.getState().selectFeed('12');
  checkNew('(s6) 切源D：游标从 0 起步（源D 尚未加载，不继承源A 的 40）', store.getState().articlesLimit === 0);
  await store.getState().reloadFromBackend();
  const s6D = store.getState();
  checkNew('(s6) 源D 首批：只含源D 条目（源A 的列表不残留），游标按源D 起步',
    s6D.entries.length === 40 && s6D.entries.every((e) => e.feedId === '12')
    && s6D.articlesLimit === 40 && s6D.articlesExhausted === true);
  checkNew('(s6) per-scope 游标并存：源A 的 40 与源D 的 40 各自记账，互不覆盖',
    s6D.articlesCursor['article|12']?.loaded === 40 && s6D.articlesCursor['article|10']?.loaded === 40);

  /* ---------- (s7) 排序切换：游标含义随 newest_first 翻转 ⇒ 必须按新排序重拉 ----------
     只翻转排序键而不重拉时，已加载的快照（旧排序的首批）会与新排序的下一页错位——继续翻页
     取回的是另一端的文章，整段文章不可达 + 重复卡片（审计 round-1「排序切换游标错位」）。
     该缺陷在 P0-1（后端此前不认 newest_first）修好后才真正可达。
     TASK-093 加强（审查 F1/F4）：把审查探针场景舞台化——旧排序 loadMore(offset=500)
     在途，切排序后的重拉先落地（游标仍为 500），旧排序迟到页后到。修前（loadMore
     竞态守卫不含排序）迟到页被放行接入：duplicates=100 / missing=100（探针实测）；
     修后在途页整体丢弃。前置条件改在本场景收尾处与竞态结果一并判定（原 2212 行
     前置断言在修前代码下也通过、无判别力）。 */
  await resetStore();
  viewEntriesCache.clear();
  const s7Rows = [];
  for (let i = 0; i < 600; i += 1) {
    s7Rows.push(mkRow({ id: 5000 + i, feed_id: 10, title: `T${i}`, published_at: iso(NOW - i * 1000) }));
  }
  S.backendRows = s7Rows;
  store.getState().selectFeed('10');
  await store.getState().reloadFromBackend();
  const s7Pre = store.getState();
  const s7PreOk = s7Pre.entries.length === 500 && s7Pre.entries[0]?.id === '5000'
    && s7Pre.articlesCursor['article|10']?.loaded === 500 && s7Pre.articlesExhausted === false;
  const s7CacheHadSnapshot = viewEntriesCache.size > 0;  // 重拉已把「全部」视图快照写入缓存
  /* 舞台：list_articles 全部挂起。loadMore 先发（旧排序第 2 页，keyset 锚
     last_id=5499），toggle 的重拉后发（新排序首页 offset=0）——按参数匹配挂起项，
     不按下标。TASK-117：续拉挂起项按 keyset 锚匹配（原 offset=500）。 */
  S.listPlan = { mode: 'defer' };
  S.invokeCalls.length = 0;
  store.getState().loadMoreArticles();
  store.getState().toggleTimelineSort();
  const s7CacheCleared = viewEntriesCache.size === 0;    // toggle 同步丢弃各视图快照缓存
  await nTick(30);                                       // reload 的 folders/feeds/counts 落地，两个 list_articles 挂起
  const s7StaleCall = S.pendingList.find((p) => p.args.last_id === 5499 && p.args.newest_first === true);
  const s7ReloadCall = S.pendingList.find((p) => p.args.offset === 0 && p.args.newest_first === false);
  s7ReloadCall?.resolve(queryRows(s7ReloadCall.args));   // 重拉先落地：游标仍为 500（F1 场景成立的前提）
  await nTick(30);
  s7StaleCall?.resolve(queryRows(s7StaleCall.args));     // 旧排序迟到页后到
  await nTick(30);
  S.listPlan = null;
  const s7 = store.getState();
  const s7Ids = s7.entries.map((e) => e.id);
  checkNew('(s7) 前置：源A 600 条，newest 首批 = 最新端 500 条（首条 id 5000、游标 500、未到底）；切排序重拉落地后旧排序在途页整体丢弃，列表唯一且无缺失（TASK-093 加强）',
    s7PreOk
    && s7.entries.length === 500 && new Set(s7Ids).size === 500
    && s7Ids[0] === '5599' && s7Ids[499] === '5100');
  checkNew('(s7) 切换「最早」：按新排序重拉该范围首页（feed_id=10 / offset=0 / newest_first=false），游标与 entries 同一次写入',
    s7ReloadCall?.args.feed_id === 10 && s7ReloadCall?.args.folder_id === null
    && s7ReloadCall?.args.offset === 0 && s7ReloadCall?.args.newest_first === false
    && s7.entries.length === 500 && s7.articlesLimit === 500 && s7.articlesCursor['article|10']?.loaded === 500);
  checkNew('(s7) 重拉后的首批换到另一端（首条 = 全库最老 id 5599），不再是最新端快照',
    s7.entries[0]?.id === '5599' && s7.entries[499]?.id === '5100'
    && s7.entries.every((e) => e.id !== '5000'));
  checkNew('(p3-f1) 旧排序在途页迟到达时被丢弃：无重复、无缺失段（修前 duplicates=100/missing=100，审查探针场景）',
    s7.entries.length === 500 && new Set(s7Ids).size === s7Ids.length
    && s7Ids.every((id) => { const n = Number(id); return n >= 5100 && n <= 5599; })
    && s7.articlesLimit === 500 && s7.articlesCursor['article|10']?.loaded === 500);
  checkNew('(s7) 排序键翻转且已读保留快照清空、各视图快照缓存同步丢弃（与视图/范围切换同口径；TASK-093 加强，M4/M4b 变红）',
    s7.timelineSort === 'oldest' && Object.keys(s7.openedReadIds).length === 0
    && s7CacheHadSnapshot && s7CacheCleared);

  /* ---------- (p3-f5) TASK-093：toggleTimelineSort 的三个口径（审查 F5） ---------- */
  /* ① 重拉失败不得产生 unhandled rejection（reloadFromBackend toast 后重抛，本入口须接住） */
  await bootFixture();
  S.failReload = { message: 'db busy' };
  store.setState({ toasts: [] });
  const p3f5Unhandled = [];
  const p3f5OnUn = (r) => { p3f5Unhandled.push(r); };
  // R1：临时 process 监听必须 finally 摘除（成功/异常都不留全局监听残留）
  process.on('unhandledRejection', p3f5OnUn);
  try {
    store.getState().toggleTimelineSort();
    await nTick(30);
  } finally {
    process.off('unhandledRejection', p3f5OnUn);
  }
  S.failReload = null;
  checkNew('(p3-f5) 切换排序的重拉失败：不产生 unhandled rejection（失败提示仍由 reloadFromBackend 给出）',
    p3f5Unhandled.length === 0
    && store.getState().toasts.some((t) => t.text.startsWith('刷新失败：')));

  /* ② 非 tauri（mock）模式不调后端：重拉会把 mock 会话翻成 tauri（修前可达） */
  await bootFixture();
  store.setState({ dataMode: 'mock' });
  S.invokeCalls.length = 0;
  const p3f5SortBefore = store.getState().timelineSort;
  store.getState().toggleTimelineSort();
  await nTick(30);
  checkNew('(p3-f5) mock 模式切排序：纯本地翻转，不调后端、不把 mock 会话翻成 tauri',
    store.getState().timelineSort !== p3f5SortBefore
    && S.invokeCalls.filter((c) => c.cmd === 'list_articles').length === 0
    && store.getState().dataMode === 'mock');

  /* ③ 筛选视图（收藏/未读/今天）切排序。
     【TASK-110 改动理由】原断言「全集本地重排、不重拉后端（0 次 list_articles）」
     锁的正是本卡废除的手法：筛选视图原本拉全集（limit:100000）存内存，切排序
     只需本地重排。分页化后全集不再在内存里，排序改由服务端承载——切排序必须
     按新排序重拉当前范围（与「全部」视图同构）。本断言按行为变化改写，保留其
     保护意图中仍然成立的部分（可见顺序翻转、集合不变），并加锁新契约（重拉的
     wire 参数：only_starred + newest_first=false + offset=0，恰好 1 次）。 */
  await bootFixture();
  store.getState().selectView('starred');
  await nTick(30);
  S.invokeCalls.length = 0;
  const p3f5StarSet = [...store.getState().entries].map((e) => e.id).sort().join(',');
  const p3f5VisNewest = selectVisibleEntries(store.getState())[0]?.id;
  store.getState().toggleTimelineSort();
  await nTick(30);
  const p3f5VisOldest = selectVisibleEntries(store.getState())[0]?.id;
  const p3f5SortCall = S.invokeCalls.find((c) => c.cmd === 'list_articles');
  checkNew('(p3-f5) 筛选视图切排序：按新排序重拉当前范围（1 次 list_articles，only_starred + newest_first=false + offset=0），可见顺序翻转、集合不变（TASK-110③ 行为变化）',
    S.invokeCalls.filter((c) => c.cmd === 'list_articles').length === 1
    && p3f5SortCall?.args.args.only_starred === true && p3f5SortCall?.args.args.newest_first === false
    && p3f5SortCall?.args.args.offset === 0
    && p3f5VisNewest === '102' && p3f5VisOldest === '103'
    && [...store.getState().entries].map((e) => e.id).sort().join(',') === p3f5StarSet);

  /* ---------- (p3b) TASK-098：selectLayout/selectView/selectFeed 的 void reload 全量收口（F5 同款铺开） ----------
     TASK-093 的 F5 只收口了 toggleTimelineSort；TASK-098 独立审查把同款暴露铺开收口：
     selectLayout / selectView（缓存命中与未命中两条路）/ selectFeed 的 void reload
     此前均无 .catch——reloadFromBackend 失败时 toast 后重抛 ⇒ unhandled rejection。
     断言复用 (p3-f5) ① 的 unhandled 捕获装置：注入后端拒绝，触发入口，捕获进程级
     unhandledRejection。App.tsx 的 feeds-updated 事件路径无法在无浏览器装置里驱动
     组件，另加源码形态断言钉住（readFileSync 手法沿用本文件既有写法）。 */
  {
    const fsP3b = await import('node:fs');
    const navSrcP3b = fsP3b.readFileSync(new URL('../../src/store/slices/nav.ts', import.meta.url), 'utf8');
    const appSrcP3b = fsP3b.readFileSync(new URL('../../src/App.tsx', import.meta.url), 'utf8');
    const p3bNavReloadLines = navSrcP3b.split('\n').filter((l) => /void get\(\)\.reload(FromBackend|FilteredEntries)\(/.test(l));
    checkNew('(p3b) nav.ts 全部 void reload 调用点（10 处：selectLayout / selectView 两条路 / selectFeed / toggleTimelineSort 两条路——TASK-110③ 新增筛选分流）逐行带 .catch（漏一处即失败）',
      /* TASK-110③：toggleTimelineSort 筛选视图新增 reloadFilteredEntries 分流（原 9 处 → 10 处） */
    p3bNavReloadLines.length === 10 && p3bNavReloadLines.every((l) => l.includes('.catch(')));
    /* 【TASK-111② 改动理由】原断言锁定的字面量是 `reloadFromBackend().catch(`——
       本卡给该调用补了保位请求实参（后台刷新路径携带 keepReadingPosition，落地
       时发出保位信号供 Timeline 消费顶条锚回位）。断言同步改写为锁定新 wire 形态
       （带实参 + .catch 仍在），非为过门禁而弱化：.catch 防线（删掉即失败）与
       「后台刷新走保位路径」两点同时锁定。 */
    checkNew('(p3b) App.tsx 后台刷新事件（feeds-updated）的 void reloadFromBackend（TASK-111② 起带 keepReadingPosition 保位实参）同样带 .catch（删掉即失败）',
      /void useAppStore\.getState\(\)\.reloadFromBackend\(\{ keepReadingPosition: true \}\)\.catch\(/.test(appSrcP3b));

    /* ① selectView 缓存命中路径：bootFixture 已把「all」快照写进 viewEntriesCache，
         selectView('all') 命中缓存 → 后台静默刷新失败（toast 后重抛，本入口须接住） */
    await bootFixture();
    S.failReload = { message: 'db busy' };
    store.setState({ toasts: [] });
    const p3bUn1 = [];
    const p3bOn1 = (r) => { p3bUn1.push(r); };
    process.on('unhandledRejection', p3bOn1);
    try {
      store.getState().selectView('all');
      await nTick(30);
    } finally {
      process.off('unhandledRejection', p3bOn1);
    }
    S.failReload = null;
    checkNew('(p3b) selectView 缓存命中路径的后台刷新失败：无 unhandled rejection（失败提示仍由 reloadFromBackend 给出）',
      p3bUn1.length === 0
      && store.getState().toasts.some((t) => t.text.startsWith('刷新失败：')));

    /* ② selectView 无缓存路径：清掉快照缓存 → selectView('all') 走直拉路径 */
    await bootFixture();
    viewEntriesCache.clear();
    S.failReload = { message: 'db busy' };
    store.setState({ toasts: [] });
    const p3bUn2 = [];
    const p3bOn2 = (r) => { p3bUn2.push(r); };
    process.on('unhandledRejection', p3bOn2);
    try {
      store.getState().selectView('all');
      await nTick(30);
    } finally {
      process.off('unhandledRejection', p3bOn2);
    }
    S.failReload = null;
    checkNew('(p3b) selectView 无缓存路径的重拉失败：无 unhandled rejection（失败提示仍由 reloadFromBackend 给出）',
      p3bUn2.length === 0
      && store.getState().toasts.some((t) => t.text.startsWith('刷新失败：')));

    /* ③ selectFeed：该范围的快照缓存未命中 → 直接后台重拉 */
    await bootFixture();
    S.failReload = { message: 'db busy' };
    store.setState({ toasts: [] });
    const p3bUn3 = [];
    const p3bOn3 = (r) => { p3bUn3.push(r); };
    process.on('unhandledRejection', p3bOn3);
    try {
      store.getState().selectFeed('10');
      await nTick(30);
    } finally {
      process.off('unhandledRejection', p3bOn3);
    }
    S.failReload = null;
    checkNew('(p3b) selectFeed 的重拉失败：无 unhandled rejection（失败提示仍由 reloadFromBackend 给出）',
      p3bUn3.length === 0
      && store.getState().toasts.some((t) => t.text.startsWith('刷新失败：')));

    /* ④ selectLayout：布局切换触发按新布局重拉 */
    await bootFixture();
    S.failReload = { message: 'db busy' };
    store.setState({ toasts: [] });
    const p3bUn4 = [];
    const p3bOn4 = (r) => { p3bUn4.push(r); };
    process.on('unhandledRejection', p3bOn4);
    try {
      store.getState().selectLayout('social');
      await nTick(30);
    } finally {
      process.off('unhandledRejection', p3bOn4);
    }
    S.failReload = null;
    checkNew('(p3b) selectLayout 的重拉失败：无 unhandled rejection（失败提示仍由 reloadFromBackend 给出）',
      p3bUn4.length === 0
      && store.getState().toasts.some((t) => t.text.startsWith('刷新失败：')));

    /* ⑤ 筛选视图：selectView('starred') → reloadFilteredEntries。list_articles 拒绝时
         该函数内部 toast、不重抛——无 unhandled rejection 且失败可见（口径同 F5：
         可见性由 reload 自身给出，调用点只兜底） */
    await bootFixture();
    S.listPlan = { mode: 'reject', error: { message: 'db busy' } };
    store.setState({ toasts: [] });
    const p3bUn5 = [];
    const p3bOn5 = (r) => { p3bUn5.push(r); };
    process.on('unhandledRejection', p3bOn5);
    try {
      store.getState().selectView('starred');
      await nTick(30);
    } finally {
      process.off('unhandledRejection', p3bOn5);
    }
    S.listPlan = null;
    checkNew('(p3b) 筛选视图 selectView 的拉取失败：无 unhandled rejection，失败提示由 reloadFilteredEntries 自身给出',
      p3bUn5.length === 0
      && store.getState().toasts.some((t) => t.text.startsWith('筛选列表加载失败：')));
  }

  /* ============================================================
     TASK-052 §A/B：口径修复的可观察判据（同一组判据在修前/修后分别跑过）
     每条都对应「修前为假 → 修后为真」的可观察量。完整 A/B 明细见
     tmp/task052/ab-before.json / ab-after.json / ab-diff.txt（11 项翻转）。
     ============================================================ */
  {
    const scopeArgsOf = (calls) => [...calls].reverse().find((c) => c.cmd === 'list_articles')?.args?.args;

    /* A1：单源视图连续翻页取回该源的后续文章 */
    await resetStore();
    S.backendRows = [];
    for (let i = 0; i < 1200; i += 1) {
      S.backendRows.push(mkRow({ id: 6000 + i, feed_id: 10, published_at: iso(NOW - i * 1000) }));
      S.backendRows.push(mkRow({ id: 7000 + i, feed_id: 11, published_at: iso(NOW - i * 1000 - 500) }));
    }
    store.getState().selectFeed('10');
    await store.getState().reloadFromBackend();
    const a1First = store.getState().entries.map((e) => e.id);
    S.invokeCalls.length = 0;
    await store.getState().loadMoreArticles();
    const a1Args = scopeArgsOf(S.invokeCalls);
    const a1Second = store.getState().entries.slice(a1First.length).map((e) => e.id);
    checkNew('(A1) 单源翻页带上范围参数 feed_id=10（修前：不带 ⇒ 拿全局下一页）',
      a1Args?.feed_id === 10);
    checkNew('(A1) 单源两页条目全部属于该源（修前：混入源B）',
      store.getState().entries.every((e) => e.feedId === '10'));
    checkNew('(A1) 第 2 页是该源的后续序列（修前：取到全局第 501 条，不是 6500）',
      a1Second[0] === '6500' && a1Second.length === 500);
    checkNew('(A1) 两页 id 集合不重叠', a1Second.every((id) => !a1First.includes(id)));

    /* A2：单分类视图（folder_id 口径） */
    await resetStore();
    S.backendRows = [];
    for (let i = 0; i < 600; i += 1) {
      S.backendRows.push(mkRow({ id: 8000 + i, feed_id: 10, published_at: iso(NOW - i * 1000) }));
      S.backendRows.push(mkRow({ id: 9000 + i, feed_id: 12, published_at: iso(NOW - i * 1000 - 300) }));
      S.backendRows.push(mkRow({ id: 9500 + i, feed_id: 20, published_at: iso(NOW - i * 1000 - 600) }));
    }
    store.getState().selectFeed('cat-1');
    await store.getState().reloadFromBackend();
    S.invokeCalls.length = 0;
    await store.getState().loadMoreArticles();
    const a2Args = scopeArgsOf(S.invokeCalls);
    checkNew('(A2) 单分类翻页带上 folder_id=1 且 feed_id=null（修前：两者都不带）',
      a2Args?.folder_id === 1 && a2Args?.feed_id === null);
    checkNew('(A2) 分类两页条目只含该分类的源（修前：混入分类外源C）',
      store.getState().entries.every((e) => e.feedId === '10' || e.feedId === '12')
      && !store.getState().entries.some((e) => e.feedId === '20'));

    /* A4：per-scope 游标互不污染 */
    await resetStore();
    S.backendRows = [];
    for (let i = 0; i < 1200; i += 1) {
      S.backendRows.push(mkRow({ id: 6000 + i, feed_id: 10, published_at: iso(NOW - i * 1000) }));
      S.backendRows.push(mkRow({ id: 7000 + i, feed_id: 11, published_at: iso(NOW - i * 1000 - 500) }));
    }
    store.getState().selectFeed('10');
    await store.getState().reloadFromBackend();
    await store.getState().loadMoreArticles();
    store.getState().selectFeed('11');
    checkNew('(A4) 切到源B 后游标从 0 起步（修前：继承源A 的 1000）',
      store.getState().articlesLimit === 0);
    S.invokeCalls.length = 0;
    await store.getState().reloadFromBackend();
    checkNew('(A4) 源B 首批按 feed_id=11 取（修前：不带范围 ⇒ 拿全局首批，混入源A）',
      scopeArgsOf(S.invokeCalls)?.feed_id === 11
      && store.getState().entries.every((e) => e.feedId === '11'));

    /* A3：空范围首批收敛（修前：游标硬编码 500、永远假装还有数据） */
    await resetStore();
    S.backendRows = [mkRow({ id: 111, feed_id: 20 })];
    store.getState().selectFeed('10');
    await store.getState().reloadFromBackend();
    checkNew('(A3) 空范围首批立即收敛为「已到底」（修前：articlesExhausted=false，空列表却假装还有 500 条）',
      store.getState().entries.length === 0 && store.getState().articlesExhausted === true);

    /* A5：D2/D3 —— A/B 探针显示修前修后一致（均 true），固化为防回退断言 */
    await resetStore();
    store.setState({ articlesLimit: 100 });
    S.listPlan = { mode: 'reject', error: { message: 'db busy' } };
    await store.getState().loadMoreArticles();
    S.listPlan = null;
    const a5 = store.getState();
    checkNew('(A5/D2) 失败 toast + 可调用重试（修前修后一致：不回退）',
      a5.toasts.length === 1 && a5.toasts[0].text.includes('加载更多失败')
      && a5.toasts[0].text.includes('db busy')
      && typeof a5.toasts[0].action?.run === 'function');
    checkNew('(A5/D3) 竞态丢弃复位 articlesLoading（修前修后一致：不回退）',
      a5.articlesLoading === false);
  }

  /* ============================================================
     TASK-109：查询口径统一收口（QueryScope）+ merge bump 按真值来源收窄
     - 口径单点：三把键与查询参数只经 QueryScope 派生（源级断言）；
       scopePageKey/viewCacheKey 字符串形态锁定不变（缓存/游标键兼容）；
     - reloadFromBackend 的游标键/「全部」视图缓存键改为发起时快照（原在 await
       后读完成时状态，靠 reloadGeneration 间接兜底；行为等价、口径显式化）；
     - bump 收窄：后端快照路径（fromBackend=true）void 在途声明；缓存恢复路径
       （fromBackend=false）不 void——R2 裁定角例的修后行为有判别断言。
     ============================================================ */
  {
    const fs109 = await import('node:fs');
    const src109 = (p) => fs109.readFileSync(new URL(p, import.meta.url), 'utf8');
    const internals109 = src109('../../src/store/internals.ts');
    const bootstrap109 = src109('../../src/store/slices/bootstrap.ts');
    const nav109 = src109('../../src/store/slices/nav.ts');

    /* -- t109-queryscope-single-source：口径派生单点性（源级）-- */
    checkNew('(t109-queryscope-single-source) QueryScope 统一入口定义于 internals：三把键 + 标写范围 + 两个具名守卫齐备',
      internals109.includes('export const QueryScope = {')
      && internals109.includes('args: scopeQueryArgs')
      && internals109.includes('markScope: scopeFilterArgs')
      && internals109.includes('pageKey: scopePageKey')
      && internals109.includes('viewKey: viewCacheKey')
      && internals109.includes('export function paginationStale(')
      && internals109.includes('export function filteredSnapshotStale('));
    {
      const importLine = (src) => {
        const m = src.match(/import \{([^}]*)\} from '\.\.\/internals';/);
        return m ? m[1] : '';
      };
      checkNew('(t109-queryscope-single-source) bootstrap/nav 不再裸引三把键：仅经 QueryScope.* 消费（键派生无第二入口）',
        !importLine(bootstrap109).includes('scopeQueryArgs') && !importLine(bootstrap109).includes('scopePageKey')
        && !importLine(bootstrap109).includes('viewCacheKey')
        && !importLine(nav109).includes('scopeQueryArgs') && !importLine(nav109).includes('scopePageKey')
        && !importLine(nav109).includes('viewCacheKey')
        && bootstrap109.includes('QueryScope.args(') && bootstrap109.includes('QueryScope.pageKey(')
        && bootstrap109.includes('QueryScope.viewKey(')
        && bootstrap109.includes('QueryScope.paginationStale(') && bootstrap109.includes('QueryScope.filteredSnapshotStale(')
        && nav109.includes('QueryScope.pageKey(') && nav109.includes('QueryScope.viewKey('));
    }

    /* -- t109-key-format-compat：键形态兼容（行为级）-- */
    {
      const it109 = await import('../../dist-test/store/internals.js');
      checkNew('(t109-key-format-compat) scopePageKey/viewCacheKey 字符串形态逐字不变，QueryScope 同源绑定、args/markScope 派生正确',
        it109.scopePageKey('all') === 'all' && it109.scopePageKey('10') === '10' && it109.scopePageKey('cat-1') === 'cat-1'
        && it109.scopePageKey('10', 'article') === 'article|10' && it109.scopePageKey('all', 'social') === 'social|all'
        && it109.viewCacheKey('article', 'all') === 'article|all|all'
        && it109.viewCacheKey('social', 'starred', '11') === 'social|starred|11'
        && it109.QueryScope.pageKey === it109.scopePageKey && it109.QueryScope.viewKey === it109.viewCacheKey
        && JSON.stringify(it109.QueryScope.args('12', 'oldest', 'social')) === JSON.stringify({ feed_id: 12, folder_id: null, newest_first: false, layout: 'social' })
        && JSON.stringify(it109.QueryScope.markScope('cat-1')) === JSON.stringify({ feed_id: null, folder_id: 1 })
        && JSON.stringify(it109.QueryScope.markScope('all')) === JSON.stringify({ feed_id: null, folder_id: null }));
    }

    /* -- t109-reload-scope-snapshot-at-start：发起时快照修正（源级）-- */
    {
      const start109 = bootstrap109.indexOf('reloadFromBackend: async');
      const end109 = bootstrap109.indexOf('loadMoreArticles:');
      const reloadSrc = bootstrap109.slice(start109, end109);
      const awaitPos = reloadSrc.indexOf('await Promise.all');
      const capturePos = reloadSrc.indexOf('const scopeAtStart = get().activeFeedFilter;');
      checkNew('(t109-reload-scope-snapshot-at-start) reloadFromBackend 发起时快照范围口径：scopeKey/缓存键从 scopeAtStart 派生，await 后无完成时回读',
        capturePos >= 0 && awaitPos >= 0 && capturePos < awaitPos
        && reloadSrc.includes('const scopeKey = QueryScope.pageKey(scopeAtStart, layoutAtStart);')
        && reloadSrc.includes("QueryScope.viewKey(layoutAtStart, 'all', scopeAtStart)")
        && !reloadSrc.slice(awaitPos).includes('get().activeFeedFilter')
        && !reloadSrc.slice(awaitPos).includes('get().activeContentLayout'));
    }

    /* -- t109-markscope-derivation：标写范围派生收口（源级）-- */
    {
      const markSrc = nav109.slice(nav109.indexOf('markCurrentViewAllRead: () => {'));
      checkNew('(t109-markscope-derivation) markCurrentViewAllRead 标写范围经 QueryScope.markScope（仅订阅维度），layout 由 markAllRead 独立参数承载',
        markSrc.includes('QueryScope.markScope(scope)')
        && !markSrc.includes('scopeQueryArgs(')
        && markSrc.includes('api.markAllRead(feedId, folderId, { starredOnly, sinceMs, layout })'));
    }

    /* -- t109-cache-restore-preserves-claim：R2 裁定角例修后行为（判别，行为级）--
       全部已读乐观写入在途（缓存已被乐观写同步为 read 态）→ 缓存恢复（selectFeed
       命中，回放乐观 read 态；fromBackend=false 不 bump 版本）→ mark_all_read 失败
       → 回滚必须仍能恢复（修前 bump-on-all-merge 会 void 声明、卡在乐观 read 态）。 */
    await resetStore();
    S.feedCountsImpl = countsFromRows;
    S.backendRows = [
      mkRow({ id: 50001, feed_id: 10, published_at: iso(NOW) }),
      mkRow({ id: 50002, feed_id: 10, published_at: iso(NOW - 60000) }),
    ];
    await store.getState().bootstrapFromBackend();
    const realInvoke109 = globalThis.__INVOKE__;
    let rejectMarkAll109 = null;
    // R1：__INVOKE__ 覆盖必须 finally 复原（含中途 reject 与 defer 清理路径）
    const restoreInvoke109 = overrideGlobal(globalThis, '__INVOKE__', (cmd, args) => {
      if (cmd === 'mark_all_read') return new Promise((_res, rej) => { rejectMarkAll109 = rej; });
      return realInvoke109(cmd, args);
    });
    try {
      store.getState().markCurrentViewAllRead(); // 乐观翻转 → read；缓存同步为 read 态；版本快照 v1
      await nTick(0);
      S.listPlan = { mode: 'defer' };              // 拦住 selectFeed 的后台 reload（其 fromBackend bump 不得抢跑）
      store.getState().selectFeed('all');        // 缓存命中：回放乐观 read 态（TASK-109②：不 bump）
      await nTick(0);
      const replayed109 = store.getState();
      checkNew('(t109-cache-restore-preserves-claim) 场景成立：缓存回放携带乐观 read 态且计数停留乐观值（回放未 bump 版本）',
        replayed109.entries.every((a) => a.isRead) && replayed109.feedCounts.get('10')?.unread === 0);
      rejectMarkAll109({ message: '注入失败:mark_all_read' });
      await nTick(10);
      const healed109 = store.getState();
      checkNew('(t109-cache-restore-preserves-claim) R2 角例修后行为：缓存回放不 void 在途声明，失败回滚正确恢复未读态与计数 2',
        healed109.entries.every((a) => !a.isRead) && healed109.feedCounts.get('10')?.unread === 2);

      /* -- t109-cache-replay-new-claim：缓存回放不误 void 新声明（行为级）-- */
      store.getState().markCurrentViewAllRead(); // 回放后的新声明：乐观翻转 → read（版本 bump 后快照）
      await nTick(0);
      rejectMarkAll109({ message: '注入失败:mark_all_read' });
      await nTick(10);
      const newClaim109 = store.getState();
      checkNew('(t109-cache-replay-new-claim) 缓存回放后新建声明的回滚仍正常：恢复未读态与计数 2（不误 void）',
        newClaim109.entries.every((a) => !a.isRead) && newClaim109.feedCounts.get('10')?.unread === 2);
      // 清理：放行被 defer 的后台 reload（fromBackend merge 接真值对齐）
      S.listPlan = null;
      for (const p of S.pendingList) p.resolve(queryRows(p.args));
      S.pendingList.length = 0;
      await nTick(20);
    } finally {
      restoreInvoke109();
    }

    /* -- t109-filtered-guard-named→t110：筛选视图拉取锁排序（具名守卫，行为级）。
       【TASK-110 改动理由】原断言「筛选视图拉取不锁排序：拉取期间切排序响应仍落地」
       锁的正是本卡废除的旧行为——筛选视图拉全集（limit:100000）存内存，切排序只是
       selectVisibleEntries 本地重排，排序与取数无关，守卫有意不锁排序。TASK-110①③
       分页化 + 切排序改为服务端重拉后，该前提废除：迟到的旧排序响应属于另一个查询
       口径（放行会把旧排序页当作新排序列表写入），filteredSnapshotStale 同步锁排序。
       新契约两段验证：①拉取期间切排序 ⇒ 第一发（旧排序）响应被整体丢弃，entries
       未被覆盖；②切排序自身触发的按新排序重拉落地（entries 为新排序序、集合不变、
       exhausted 真实判定、游标随落地快照对齐）。 */
    await resetStore();
    S.feedCountsImpl = countsFromRows;
    await store.getState().bootstrapFromBackend();
    S.listPlan = { mode: 'defer' };
    store.setState({ activeViewFilter: 'starred', timelineSort: 'newest' });
    const pFiltered109 = store.getState().reloadFilteredEntries('starred');
    await nTick(0);
    store.getState().toggleTimelineSort(); // TASK-110③：筛选视图切排序现在触发按新排序的重拉（第二发，同走 defer 队列）
    const sortFlipped109 = store.getState().timelineSort === 'oldest';
    for (const p of S.pendingList) p.resolve(queryRows(p.args)); // 第一发（旧排序 newest）响应放行 → 守卫丢弃
    S.pendingList.length = 0;
    await pFiltered109;
    checkNew('(t109-filtered-guard-named→t110) 筛选视图拉取锁排序（TASK-110③ 行为变化）：拉取期间切排序，迟到旧排序响应被 filteredSnapshotStale 整体丢弃（entries 未被覆盖，仍为 all 首批快照）',
      sortFlipped109
      && store.getState().entries.length === 8
      && store.getState().entries.some((e) => !e.isStarred));
    /* TASK-110③ 驱动补充（探针实测的连锁根因，必须在此排空）：切排序触发的第二发
       reloadFilteredEntries，其 list_articles 经 api.listArticles 内部 await getInvoke()
       延了一个微任务才进 defer 队列——第一发在上方 nTick(0) 宏任务间隙已入队、排空
       循环够得到它；第二发入队晚于排空循环，promise 被孤儿化后永不落定，
       reloadFilteredEntries 的 finally 减不掉 backendReloadInFlight，在途标记泄漏会
       拦住后续所有测试块的 loadMoreArticles（r7-switch / t100-p3-1 连锁误红）。
       生产侧无此问题（Tauri invoke 恒 settle），纯测试驱动口径。 */
    await nTick(0);
    for (const p of S.pendingList.splice(0)) p.resolve(queryRows(p.args)); // 第二发（新排序 oldest 重拉）放行
    S.listPlan = null;
    await nTick(10);
    checkNew('(t109-filtered-guard-named→t110) 切排序触发的重拉落地：entries 为新排序（oldest）收藏集合、exhausted 真实判定（3<500）、游标随落地快照对齐',
      store.getState().entries.map((e) => e.id).join(',') === '103,102,201'
      && store.getState().entries.every((a) => a.isStarred)
      && store.getState().articlesExhausted === true
      && store.getState().articlesCursor['article|all']?.loaded === 3);
  }

  /* ============================================================
     TASK-110（二阶段②）：筛选视图真分页——废除 limit:100000 近似全集。
     旧手法：reloadFilteredEntries 一次拉全集（articlesExhausted 恒 true、切排序
     仅本地重排），旧文章在筛选视图不可达（审计二阶段完成标准）。新契约：
     ①筛选视图首屏 PAGE_SIZE、续拉走 loadMoreArticles（viewFilter 单点派生，
       与「全部」视图共用游标键 scopePageKey 与分页守卫）；
     ②追加按 id 去重 + 稳定序策略「追加去重保序」（同步插入使 offset 漂移时
       不得重复入列；新条目不回填已加载窗口，随下次 reload 进入——判别断言）；
     ③切排序与 all 同构重拉（水合正文由 mergeSnapshotEntries 按 id 继承）；
     ④exhausted 真实判定（rows.length < PAGE_SIZE）双向锁定；
     ⑤过滤参数（only_unread/only_starred/only_today）逐项 × 范围 × 布局。
     夹具：600 行 feed-10（i=0 最新），其中前 520 行收藏；时间戳取 NOW-3d 起，
     与「今天」判定解耦（today 用独立 NOW 行）。全部走内存假后端（模块级
     harness），布局不做后端过滤（与 (r7-switch) 同口径，断言走 wire 参数）。
     ============================================================ */
  {
    const PAGE = 500; // 与 internals.ARTICLES_PAGE_SIZE 同值（import 断言见 (t110-0)）
    const mkT110Row = (o) => mkRow({ feed_id: 10, ...o });
    const T110_OLD = iso(NOW - 3 * 86400000);
    /* 600 行基础夹具：id=1100+i，i 越小越新；i<520 收藏 */
    const t110Rows = () => {
      const rows = [];
      for (let i = 0; i < 600; i += 1) {
        rows.push(mkT110Row({ id: 1100 + i, published_at: iso(NOW - 3 * 86400000 - i * 60000), is_starred: i < 520 }));
      }
      return rows;
    };

    /* -- (t110-0) 页大小单点来源：筛选视图分页与「全部」共用同一常量 -- */
    const { ARTICLES_PAGE_SIZE: t110Page } = await import('../../dist-test/store/internals.js');
    checkNew('(t110-0) ARTICLES_PAGE_SIZE 收口到 internals（=500）：筛选视图首屏/续拉与「全部」视图同一页大小单点来源',
      t110Page === 500 && PAGE === t110Page);

    /* ---------- (t110-1/2) 收藏视图：首屏分页 → 满页续拉 → 真到底 ---------- */
    await resetStore();
    S.backendRows = t110Rows();
    await store.getState().bootstrapFromBackend(); // all 视图首批 500（exhausted=false）
    S.invokeCalls.length = 0;
    store.getState().selectView('starred'); // 缓存已清 → 走 reloadFilteredEntries 首屏
    await nTick(20);
    const t110Star1st = S.invokeCalls.filter((c) => c.cmd === 'list_articles');
    const t110Star1stArgs = t110Star1st[0]?.args.args;
    checkNew('(t110-1) 收藏视图首屏真分页 wire：恰 1 次 list_articles，only_starred=true + limit=500（废除 limit:100000）+ offset=0 + 范围/布局维度齐全（article|all）',
      t110Star1st.length === 1
      && t110Star1stArgs?.only_starred === true && t110Star1stArgs?.limit === 500 && t110Star1stArgs?.offset === 0
      && t110Star1stArgs?.feed_id === null && t110Star1stArgs?.folder_id === null && t110Star1stArgs?.layout === 'article');
    const t110Star1stState = store.getState();
    checkNew('(t110-1) 收藏视图首屏落地：520 条收藏取前 500（最新端 i=0..499 ⇒ id 1100..1599）、exhausted 真实判定为 false（满页 ⇒ 可续拉，refill/哨兵前提）、游标=500',
      t110Star1stState.entries.length === 500
      && t110Star1stState.entries.every((e) => e.isStarred)
      && t110Star1stState.entries[0]?.id === '1100' && t110Star1stState.entries[499]?.id === '1599'
      && t110Star1stState.articlesExhausted === false && t110Star1stState.articlesLimit === 500
      && t110Star1stState.articlesCursor['article|all']?.loaded === 500);

    S.invokeCalls.length = 0;
    await store.getState().loadMoreArticles(); // 筛选视图续拉：与首屏同口径（viewFilter 单点派生）
    const t110Star2nd = S.invokeCalls.find((c) => c.cmd === 'list_articles')?.args.args;
    const t110Star2ndState = store.getState();
    /* 【TASK-117 改动理由】续拉 wire 从 offset=500 改为 keyset 锚（last_published/
       last_id = 首屏末行 1599 的原文锚）——offset 正是本卡废除的缺陷手法。 */
    checkNew('(t110-2) 收藏视图加载更多同口径 wire：only_starred=true + keyset 锚（last_id=1599，沿用首屏写入的同键游标）+ limit=500',
      t110Star2nd?.only_starred === true && t110Star2nd?.last_id === 1599
      && t110Star2nd?.last_published === iso(NOW - 3 * 86400000 - 499 * 60000)
      && t110Star2nd?.offset === undefined && t110Star2nd?.limit === 500);
    checkNew('(t110-2) 收藏视图续拉落地：追加剩余 20 条收敛到底（520 条无重复）、exhausted 真实判定为 true（20<500）、游标=520',
      t110Star2ndState.entries.length === 520 && new Set(t110Star2ndState.entries.map((e) => e.id)).size === 520
      && t110Star2ndState.entries[500]?.id === '1600' && t110Star2ndState.entries.at(-1)?.id === '1619'
      && t110Star2ndState.articlesExhausted === true && t110Star2ndState.articlesLimit === 520
      && t110Star2ndState.articlesCursor['article|all']?.loaded === 520);

    /* ---------- (t110-3) 插入漂移 keyset 免疫（同步插入场景，判别断言） ----------
       【TASK-117 改动理由】原断言锁定 offset 分页的「漂移 + 按id去重」补偿手法：
       第 2 页（offset=500）在途期间后端插入 10 条更新文章 ⇒ 迟到响应 = 新序列
       offset 500..999 共 110 行，与已加载窗口重叠 10 条，靠追加去重兜住。keyset
       续拉从根上消除该漂移：锚 = 首屏末行 1599（published_at 原文 + id），迟到
       响应 = 严格排在锚之后的 100 行（1600..1699），与集合增删无关——不再重叠、
       无需去重补偿（去重保留作 duplicate key 兜底防线，不回填策略不变）。
       判别：游标推进 = 500+100=600（offset 语义下为 610），entries 600 条唯一、
       已加载窗口保序、新增段按序追加；同步插入的 10 条不回填已加载窗口。 */
    await resetStore();
    S.backendRows = t110Rows();
    await store.getState().bootstrapFromBackend(); // all 视图首批 500：ids 1100..1599
    S.listPlan = { mode: 'defer' };
    store.getState().loadMoreArticles(); // 第 2 页（keyset 锚 1599）在途
    await nTick(0); // 微任务排空：第 2 页请求进入 defer 队列
    const t110DriftPage2 = S.pendingList.at(-1);
    /* 同步插入 10 条（后端同步入库语义；store 无感知、不发 reload、游标不动） */
    for (let i = 0; i < 10; i += 1) {
      S.backendRows.unshift(mkT110Row({ id: 1700 + i, published_at: iso(NOW), title: `t110 同步插入 ${i}` }));
    }
    t110DriftPage2?.resolve(queryRows(t110DriftPage2.args)); // 迟到响应 = 锚之后的 100 行（keyset 免疫插入漂移）
    S.listPlan = null;
    await nTick(20);
    const t110Drift = store.getState();
    const t110DriftIds = t110Drift.entries.map((e) => e.id);
    checkNew('(t110-3) 插入漂移 keyset 免疫（判别断言）：迟到第 2 页恰为锚后 100 行（id 1600..1699，无重叠无补拉）、entries 600 条唯一、已加载窗口保序不变、新增段按序追加',
      t110DriftIds.length === 600 && new Set(t110DriftIds).size === 600
      && t110DriftIds.slice(0, 500).join(',') === Array.from({ length: 500 }, (_, k) => String(1100 + k)).join(',')
      && t110DriftIds.slice(500).join(',') === Array.from({ length: 100 }, (_, k) => String(1600 + k)).join(','));
    checkNew('(t110-3) 插入漂移游标与新条目策略：游标 loaded 按拉取行数累加（500+100=600）、锚推进到本页末行 1699、exhausted 按拉取行数真实判定（100<500）、同步插入的新条目不回填已加载窗口（追加去重保序，随下次 reload 进入）',
      t110Drift.articlesLimit === 600 && t110Drift.articlesCursor['article|all']?.loaded === 600
      && t110Drift.articlesCursor['article|all']?.lastId === 1699
      && t110Drift.articlesExhausted === true
      && t110DriftIds.every((id) => Number(id) < 1700));

    /* ---------- (t110-4) 切排序重拉与 all 同构：wire 参数 + 水合正文保留 ---------- */
    await resetStore();
    S.backendRows = t110Rows();
    await store.getState().bootstrapFromBackend();
    store.getState().selectView('starred');
    await nTick(20);
    /* 模拟已加载条目的懒水合终态（id 1300 = i 200，切排序前后都在首屏内）。
       【TASK-122 改动理由】水合真值落 bodyById 记录（旧写法直接在条目上摆
       content/hydratedIds——不再是真值位）；保护意图（重拉不丢正文）不变。 */
    S.getArticlesPlan = { rows: [mkRow({ id: 1300, feed_id: 12, is_starred: true, content_html: '<p>t110 水合正文</p>' })] };
    store.getState().hydrateArticleContent(['1300']);
    await nTick(20);
    S.getArticlesPlan = null;
    S.invokeCalls.length = 0;
    store.getState().toggleTimelineSort(); // TASK-110③：筛选视图切排序 → 服务端重拉（不再本地重排全集）
    await nTick(20);
    const t110SortCalls = S.invokeCalls.filter((c) => c.cmd === 'list_articles');
    const t110SortArgs = t110SortCalls[0]?.args.args;
    checkNew('(t110-4) 收藏视图切排序重拉 wire：恰 1 次 list_articles，only_starred=true + newest_first=false + offset=0（与「全部」视图同构，服务端承载排序）',
      t110SortCalls.length === 1
      && t110SortArgs?.only_starred === true && t110SortArgs?.newest_first === false && t110SortArgs?.offset === 0);
    const t110Sorted = store.getState();
    checkNew('(t110-4) 切排序重拉落地：entries 换为新排序（oldest）首屏（首条=最老收藏 1619）、已加载条目的水合正文随 bodyById 不因重拉丢失（TASK-106 意图，TASK-122 真值源）、游标随落地快照对齐',
      t110Sorted.entries.length === 500 && t110Sorted.entries[0]?.id === '1619'
      && t110Sorted.entries.every((e) => e.isStarred)
      && bodyOf(t110Sorted, '1300').content === '<p>t110 水合正文</p>'
      && getBodyEntryT122('1300')?.state === 'ready'
      && t110Sorted.articlesCursor['article|all']?.loaded === 500 && t110Sorted.articlesExhausted === false);

    /* ---------- (t110-5) 过滤参数逐项：only_unread / only_starred / only_today（article|all） ---------- */
    await resetStore();
    S.backendRows = [
      mkT110Row({ id: 2100, published_at: T110_OLD }), // 未读·旧
      mkT110Row({ id: 2101, published_at: T110_OLD }), // 未读·旧
      mkT110Row({ id: 2102, published_at: T110_OLD }), // 未读·旧
      mkT110Row({ id: 2103, published_at: T110_OLD, is_read: true }), // 已读·旧
      mkT110Row({ id: 2104, published_at: T110_OLD, is_read: true }), // 已读·旧
      mkT110Row({ id: 2105, published_at: iso(NOW) }), // 未读·今天
      mkT110Row({ id: 2106, published_at: iso(NOW) }), // 未读·今天
      mkT110Row({ id: 2107, published_at: T110_OLD, is_starred: true }), // 未读·收藏·旧
    ];
    await store.getState().bootstrapFromBackend();
    S.invokeCalls.length = 0;
    store.getState().selectView('unread');
    await nTick(20);
    const t110UnreadArgs = S.invokeCalls.find((c) => c.cmd === 'list_articles')?.args.args;
    checkNew('(t110-5) 未读视图首屏 wire：only_unread=true + offset=0 + limit=500，落地只含未读行且 exhausted 真实判定（6<500）',
      t110UnreadArgs?.only_unread === true && t110UnreadArgs?.only_starred === undefined && t110UnreadArgs?.only_today === undefined
      && t110UnreadArgs?.offset === 0 && t110UnreadArgs?.limit === 500
      && store.getState().entries.length === 6 && store.getState().entries.every((e) => !e.isRead)
      && store.getState().articlesExhausted === true);
    S.invokeCalls.length = 0;
    store.getState().selectView('today');
    await nTick(20);
    const t110TodayArgs = S.invokeCalls.find((c) => c.cmd === 'list_articles')?.args.args;
    /* 【TASK-117 改动理由】期望顺序随 id 决胜更新：2105/2106 同秒并列，后端
       ORDER BY published_at DESC, id DESC（及同口径的本地排序）下 2106 在前。 */
    checkNew('(t110-5) 今天视图首屏 wire：only_today=true（不带其余筛选键），落地只含今天行（2 条，同秒并列由 id 决胜 2106 在前）',
      t110TodayArgs?.only_today === true && t110TodayArgs?.only_unread === undefined && t110TodayArgs?.only_starred === undefined
      && store.getState().entries.map((e) => e.id).join(',') === '2106,2105');
    S.invokeCalls.length = 0;
    store.getState().selectView('starred');
    await nTick(20);
    const t110StarredArgs = S.invokeCalls.find((c) => c.cmd === 'list_articles')?.args.args;
    checkNew('(t110-5) 收藏视图首屏 wire：only_starred=true（不带其余筛选键），落地只含收藏行（1 条）',
      t110StarredArgs?.only_starred === true && t110StarredArgs?.only_unread === undefined && t110StarredArgs?.only_today === undefined
      && store.getState().entries.map((e) => e.id).join(',') === '2107');

    /* ---------- (t110-6) 过滤参数 × 范围：feed-10 × 收藏 ---------- */
    await resetStore();
    S.backendRows = [
      mkT110Row({ id: 2200, published_at: T110_OLD, is_starred: true }),
      mkT110Row({ id: 2201, published_at: T110_OLD }),
      mkT110Row({ id: 2202, feed_id: 11, published_at: T110_OLD, is_starred: true }), // 源B 的收藏（范围外）
    ];
    await store.getState().bootstrapFromBackend();
    store.getState().selectFeed('feed-10');
    await nTick(20);
    S.invokeCalls.length = 0;
    store.getState().selectView('starred');
    await nTick(20);
    const t110ScopeArgs = S.invokeCalls.find((c) => c.cmd === 'list_articles')?.args.args;
    checkNew('(t110-6) 收藏视图 × 订阅范围 wire：feed_id=10 + only_starred=true（视图筛选是范围的子集，两参数同发），落地不含范围外收藏',
      t110ScopeArgs?.feed_id === 10 && t110ScopeArgs?.folder_id === null && t110ScopeArgs?.only_starred === true
      && store.getState().entries.map((e) => e.id).join(',') === '2200');

    /* ---------- (t110-7) 过滤参数 × 布局：social × 收藏 ---------- */
    await resetStore();
    S.backendRows = [
      mkT110Row({ id: 2300, published_at: T110_OLD, is_starred: true }),
      mkT110Row({ id: 2301, published_at: T110_OLD }),
    ];
    await store.getState().bootstrapFromBackend();
    store.getState().selectLayout('social');
    await nTick(20);
    S.invokeCalls.length = 0;
    store.getState().selectView('starred');
    await nTick(20);
    const t110LayoutArgs = S.invokeCalls.find((c) => c.cmd === 'list_articles')?.args.args;
    checkNew('(t110-7) 收藏视图 × 布局 wire：layout=social + only_starred=true（布局维度进查询参数，TASK-094 R7 口径在筛选视图保持）',
      t110LayoutArgs?.layout === 'social' && t110LayoutArgs?.only_starred === true && t110LayoutArgs?.offset === 0);

    await resetStore(); // 夹具复位：不把 610 行大夹具与 defer 残留带给后续块
  }

  /* ============================================================
     TASK-117（审计 P1-1）：可变集合分页改 keyset 游标——阅读中连续翻页不丢文章

     审计 P1 探针场景（tmp/audit-20261007/probes.mjs P1）：未读筛选 WHERE
     is_read=0 是**可变集合**，OFFSET 不等价于已看条数——1200 未读读 500 标读
     后集合剩 700，下一页仍 OFFSET 500 → 跳过剩余集合前 500 篇并假 exhausted
     （探针实测：loaded=700、missingUnread=500、exhausted=true）。修法：续拉以
     已加载窗口最后一行 (published_at 原文, id) 为 keyset 锚，请求「严格排在锚
     之后」的行，与集合增删无关。

     门禁纪律（DEC-gate-adjust-20261007）：操作序列场景必须转真实行为回归——
     本组全部走真实 store 动作序列（翻页 → 标读/取消收藏 → 续拉），不做静态
     分页断言。假后端 queryRows 已按 keyset 语义升级（排序补 id 决胜 + 谓词），
     并忠实落库 set_read_bulk / set_starred（后端集合真实收缩是缺陷成因，必须
     建模，否则操作序列无判别力）。
     ============================================================ */
  {
    const mkT117Row = (o) => mkRow({ feed_id: 10, ...o });
    /* 1200 行未读夹具：id 3000+i，i 越小越新（published_at 逐行递减，无并列） */
    const t117Rows = () => Array.from({ length: 1200 }, (_, i) =>
      mkT117Row({ id: 3000 + i, published_at: iso(NOW - 3 * 86400000 - i * 1000), title: `t117-${i}` }));

    /* ---------- (t117-1) 阅读中连续翻页（审计 P1 探针场景本体） ----------
       操作序列：未读视图首屏 500 → 批量标读这 500（真实 IPC 落库，集合收缩为
       700）→ 续拉必须返回**剩余集合的前 500 篇**（id 3500..3999）。OFFSET 语义
       在此会返回 4000..4199（跳过 500 篇）并假 exhausted。 */
    await resetStore();
    S.backendRows = t117Rows();
    store.getState().selectView('unread'); // 缓存已清 → 走 reloadFilteredEntries 首屏
    await nTick(20);
    const t117p1 = store.getState();
    checkNew('(t117-1) 前置：未读首屏 500 条（id 3000..3499）、exhausted=false（1200 未读满页，可续拉）',
      t117p1.entries.length === 500 && t117p1.entries[0].id === '3000' && t117p1.entries[499].id === '3499'
      && t117p1.articlesExhausted === false);
    store.getState().markEntriesReadBulk(t117p1.entries.map((e) => e.id)); // 读掉首屏（set_read_bulk 忠实落库）
    await nTick(0);
    checkNew('(t117-1) 标读落库后未读集合收缩为 700（假后端忠实翻转 is_read：缺陷成因被建模）',
      queryRows({ only_unread: true, limit: null }).length === 700);
    S.invokeCalls.length = 0;
    await store.getState().loadMoreArticles();
    const t117p1wire = S.invokeCalls.find((c) => c.cmd === 'list_articles')?.args.args;
    const t117p1after = store.getState();
    checkNew('(t117-1) 续拉 wire：only_unread + keyset 锚（last_published=末行 3499 原文 / last_id=3499，无 offset）',
      t117p1wire?.only_unread === true && t117p1wire?.last_id === 3499
      && t117p1wire?.last_published === iso(NOW - 3 * 86400000 - 499 * 1000)
      && t117p1wire?.offset === undefined && t117p1wire?.limit === 500);
    checkNew('(t117-1) 续拉落地（判别）：返回剩余集合的前 500 篇（id 3500..3999 按序追加，无重复无跳过）、exhausted=false',
      t117p1after.entries.length === 1000 && t117p1after.entries[500].id === '3500'
      && t117p1after.entries.at(-1).id === '3999'
      && new Set(t117p1after.entries.map((e) => e.id)).size === 1000
      && t117p1after.articlesExhausted === false);
    S.invokeCalls.length = 0;
    await store.getState().loadMoreArticles(); // 第三页：剩余 200 条
    const t117p1end = store.getState();
    checkNew('(t117-1) 第三页收敛：追加 4000..4199 共 200 条走完集合、exhausted=true（不足一页真实判定）、游标 loaded=1200、锚推进到末行 4199',
      t117p1end.entries.length === 1200 && t117p1end.entries[1000].id === '4000'
      && new Set(t117p1end.entries.map((e) => e.id)).size === 1200
      && t117p1end.articlesExhausted === true && t117p1end.articlesCursor['article|all']?.loaded === 1200
      && t117p1end.articlesCursor['article|all']?.lastId === 4199);
    S.invokeCalls.length = 0;
    await store.getState().loadMoreArticles();
    checkNew('(t117-1) exhausted 判定（判别）：到底后再触发不发 IPC（审计「假 exhausted」的反向锚——真到底才允许挡住续拉）',
      S.invokeCalls.filter((c) => c.cmd === 'list_articles').length === 0);
    checkNew('(t117-1) 全程无遗漏（判别）：审计探针口径 missingUnread=0——剩余未读行（700 条）全部已入列',
      queryRows({ only_unread: true, limit: null }).every((r) => t117p1end.entries.some((e) => e.id === String(r.id))));

    /* ---------- (t117-2) 取消收藏场景（收藏视图续拉，同性质判别） ----------
       操作序列：收藏首屏 500 → 逐条取消收藏这 500（set_starred 忠实落库，收藏
       集合收缩为 20）→ 续拉必须返回剩余 20 条收藏（id 6500..6519）。OFFSET 语义
       在此返回 0 行并假 exhausted——20 条收藏从此不可达。 */
    await resetStore();
    S.backendRows = Array.from({ length: 600 }, (_, i) =>
      mkT117Row({ id: 6000 + i, published_at: iso(NOW - 3 * 86400000 - i * 60000), is_starred: i < 520 }));
    store.getState().selectView('starred');
    await nTick(20);
    const t117p2 = store.getState();
    checkNew('(t117-2) 前置：收藏首屏 500 条（id 6000..6499）、exhausted=false（520 收藏满页，可续拉）',
      t117p2.entries.length === 500 && t117p2.entries[0].id === '6000' && t117p2.entries[499].id === '6499'
      && t117p2.articlesExhausted === false);
    for (const e of t117p2.entries) store.getState().toggleEntryFlag(e.id, 'isStarred'); // 逐条取消收藏（真实卡片操作）
    await nTick(0);
    checkNew('(t117-2) 取消收藏落库后收藏集合收缩为 20（set_starred 忠实翻转 is_starred）',
      queryRows({ only_starred: true, limit: null }).length === 20);
    S.invokeCalls.length = 0;
    await store.getState().loadMoreArticles();
    const t117p2wire = S.invokeCalls.find((c) => c.cmd === 'list_articles')?.args.args;
    const t117p2after = store.getState();
    checkNew('(t117-2) 续拉 wire：only_starred + keyset 锚（last_id=6499，无 offset）',
      t117p2wire?.only_starred === true && t117p2wire?.last_id === 6499 && t117p2wire?.offset === undefined);
    checkNew('(t117-2) 续拉落地（判别）：剩余 20 条收藏全部入列（id 6500..6519，修前 OFFSET 返回 0 行且假 exhausted——这 20 条不可达）',
      t117p2after.entries.length === 520 && t117p2after.entries[500].id === '6500'
      && t117p2after.entries.at(-1).id === '6519'
      && new Set(t117p2after.entries.map((e) => e.id)).size === 520
      && t117p2after.articlesExhausted === true
      && t117p2after.articlesCursor['article|all']?.loaded === 520);

    /* ---------- (t117-3) 插入漂移（首屏与续拉之间插入新文章） ----------
       keyset 天然免疫：锚 = 首屏末行 3499，插入的新行比锚更新（排在锚**之前**），
       不改变「锚之后」的集合——续拉仍返回 3500..3999，与新插入行零重叠零遗漏。
       （旧 OFFSET 语义下插入使偏移漂移，续拉区间与已加载窗口重叠。） */
    await resetStore();
    S.backendRows = t117Rows();
    store.getState().selectView('unread');
    await nTick(20);
    S.listPlan = { mode: 'defer' };
    store.getState().loadMoreArticles(); // 续拉在途（锚 3499）
    await nTick(0);
    const t117p3deferred = S.pendingList.at(-1);
    for (let i = 0; i < 10; i += 1) { // 后端同步入库 10 条更新文章（未读，比全部已有行新）
      S.backendRows.unshift(mkT117Row({ id: 5000 + i, published_at: iso(NOW), title: `t117 插入 ${i}` }));
    }
    t117p3deferred?.resolve(queryRows(t117p3deferred.args)); // 迟到响应 = 锚后的 500 行
    S.listPlan = null;
    await nTick(20);
    const t117p3 = store.getState();
    const t117p3ids = t117p3.entries.map((e) => e.id);
    checkNew('(t117-3) 插入漂移免疫（判别）：续拉恰为锚后 500 行（id 3500..3999），与新插入 10 行零重叠、已加载窗口保序、entries 1000 条唯一',
      t117p3ids.length === 1000 && new Set(t117p3ids).size === 1000
      && t117p3ids.slice(0, 500).join(',') === Array.from({ length: 500 }, (_, k) => String(3000 + k)).join(',')
      && t117p3ids.slice(500).join(',') === Array.from({ length: 500 }, (_, k) => String(3500 + k)).join(',')
      && t117p3ids.every((id) => Number(id) < 5000)
      && t117p3.articlesCursor['article|all']?.loaded === 1000
      && t117p3.articlesCursor['article|all']?.lastId === 3999);

    /* ---------- (t117-4) 并列 published_at 由 id 决胜（同秒文章不重不漏） ----------
       600 行**同一秒**发布：全序完全由 id 决胜（DESC = id 降序），页边界落在并列
       组中间——续拉必须由谓词的 `published_at = ? AND id < ?` 分支续上同秒兄弟
       （id 3299..3200），不得因 `published_at < ?` 单臂漏行或重复。 */
    await resetStore();
    S.backendRows = Array.from({ length: 600 }, (_, i) =>
      mkT117Row({ id: 3200 + i, published_at: iso(NOW - 3 * 86400000), title: `t117 同秒 ${i}` }));
    await store.getState().bootstrapFromBackend(); // all 视图首屏 500 = id 降序前 500（3799..3300）
    const t117p4 = store.getState();
    checkNew('(t117-4) 前置：同秒 600 行首屏 500 条按 id 降序（3799..3300）、锚=末行 3300',
      t117p4.entries.length === 500 && t117p4.entries[0].id === '3799' && t117p4.entries[499].id === '3300'
      && t117p4.articlesCursor['article|all']?.lastId === 3300
      && t117p4.articlesCursor['article|all']?.lastPublished === iso(NOW - 3 * 86400000)
      && t117p4.articlesExhausted === false);
    S.invokeCalls.length = 0;
    await store.getState().loadMoreArticles();
    const t117p4after = store.getState();
    const t117p4ids = t117p4after.entries.map((e) => e.id);
    checkNew('(t117-4) 同秒跨页续拉不重不漏（判别）：第二页 = 同秒组内 id 更小的 100 行（3299..3200）、全集 600 条唯一且与全序一致',
      t117p4after.entries[500].id === '3299' && t117p4after.entries.at(-1).id === '3200'
      && t117p4ids.length === 600 && new Set(t117p4ids).size === 600
      && t117p4ids.join(',') === Array.from({ length: 600 }, (_, k) => String(3799 - k)).join(',')
      && t117p4after.articlesExhausted === true);

    /* ---------- (t117-5) 排序切换后游标重置方向正确 ----------
       newest 翻页后切 oldest：重拉落地游标重置为新方向首屏（锚 = **新方向**首屏
       末行）——续拉必须沿 oldest 方向从该锚继续（id 3699..），而不是沿用旧方向
       的锚 3499。 */
    await resetStore();
    S.backendRows = t117Rows();
    await store.getState().bootstrapFromBackend(); // newest 首屏 500（id 3000..3499）
    S.invokeCalls.length = 0;
    store.getState().toggleTimelineSort(); // 切 oldest：整体重拉（viewEntriesCache 已清）
    await nTick(20);
    const t117p5sorted = store.getState();
    checkNew('(t117-5) 切 oldest 重拉落地：首屏换到最老端（首条 id 4199）、游标重置为新方向首屏（loaded=500）、锚=新方向末行 3700',
      t117p5sorted.entries[0].id === '4199' && t117p5sorted.entries.length === 500
      && t117p5sorted.articlesCursor['article|all']?.loaded === 500
      && t117p5sorted.articlesCursor['article|all']?.lastId === 3700
      && t117p5sorted.articlesExhausted === false);
    S.invokeCalls.length = 0;
    await store.getState().loadMoreArticles();
    const t117p5wire = S.invokeCalls.find((c) => c.cmd === 'list_articles')?.args.args;
    const t117p5after = store.getState();
    checkNew('(t117-5) 切排序后续拉方向正确（判别）：wire 带 newest_first=false + 新方向锚（last_id=3700），续拉沿 oldest 继续（id 3699..3200），不沿用旧方向锚 3499',
      t117p5wire?.newest_first === false && t117p5wire?.last_id === 3700
      && t117p5wire?.last_published === iso(NOW - 3 * 86400000 - 700 * 1000)
      && t117p5after.entries.length === 1000 && t117p5after.entries[500].id === '3699'
      && new Set(t117p5after.entries.map((e) => e.id)).size === 1000);

    await resetStore(); // 夹具复位：不把 1200 行大夹具与 defer 残留带给后续块
  }

  /* ============================================================
     TASK-119（2026-10-07，审计 P2-4）：请求与计数过期覆盖——迟到响应不得
     覆盖新状态（探针 P5/P6 场景转真实行为回归）。

     ①查询旧版本窗口（P5）：过期判断此前只比较查询参数——QueryScope 收口了
       谓词但没消除「同一查询的旧版本」窗口：同 scope/view/sort 两次发起、
       新响应先落、旧响应后至，旧响应因「参数全同」被放行覆盖新结果；
       A→B→A 同理不设防。修法：查询实例代际（bootstrap 双层级——全局
       queryGeneration + firstPageSerialByKey Map，键复用 TASK-117 viewKey
       键族；reloadFromBackend / reloadFilteredEntries / loadMoreArticles /
       anchorToArticle 四处发起点统一接入）。
     ②计数对账窗口（P6）：markCurrentViewAllRead 成功后的 feed_counts 对账
       在途期间用户改回未读，迟到计数整体替换把乐观计数踩回旧值（文章未读
       但未读数 0）。修法：对账发起时快照本地读/藏写入序号
       （currentLocalFlagWriteSerial，bump 点矩阵见 internals），落地仅当
       期间无本地写入才应用，否则丢弃并立即重取恰一次。

     判别设计（变异自证）：查询场景全部构造「参数比较不可判别」形态——同键
     旧实例迟到落地时 view/scopeKey/sort/游标守卫全放行，唯一判据是代际；
     删掉任一落地判据的代际比较，对应断言必红。计数竞态用本卡新增的
     feedCountsPlan/pendingCounts 手动放行（一次性 defer，重取需再挂一次）。
     ============================================================ */
  {
    /* ---------- (t119-1) A→B→A 旧响应丢弃（探针 P5 本体） ----------
       starred(A1) 请求在途 → 切 unread(B) → 切回 starred(A2)。同键旧实例（A1）
       迟到落地时：activeViewFilter 恰为 starred、scopeKey/排序全同——既有守卫
       全部放行，唯一判据是 per-key 首屏代际（A2 发起已 bump 同键序号）。 */
    await resetStore();
    S.listPlan = { mode: 'defer' };
    store.getState().selectView('starred'); // A1：旧实例发起
    await nTick(0);
    store.getState().selectView('unread'); // B：中间视图（邻键，各自代际）
    await nTick(0);
    store.getState().selectView('starred'); // A2：同键新实例（代际 bump）
    await nTick(0);
    checkNew('(t119-1) 场景成立：A1/B/A2 三个筛选请求全部在途（同键两实例 + 邻键一次）',
      S.pendingList.length === 3);
    S.pendingList[2].resolve([mkRow({ id: 8102, feed_id: 10, is_starred: true, published_at: OLD })]); // A2 新响应先落
    S.pendingList[1].resolve([mkRow({ id: 8201, feed_id: 10, published_at: OLD })]); // B（已切回 starred，视图守卫弃）
    await nTick(10);
    checkNew('(t119-1) 新实例先落地：entries 为 A2 快照（8102）',
      store.getState().entries.map((e) => e.id).join(',') === '8102');
    S.pendingList[0].resolve([mkRow({ id: 8101, feed_id: 10, is_starred: true, published_at: OLD })]); // A1 旧响应后至
    S.listPlan = null;
    await nTick(10);
    checkNew('(t119-1) A→B→A 旧响应丢弃（判别）：迟到的首个 starred 响应不覆盖新实例结果（修前「参数全同」放行 → entries 回退 8101）',
      store.getState().entries.map((e) => e.id).join(',') === '8102');

    /* ---------- (t119-2) 同查询旧版本先发后至不覆盖（探针 P5 纯形态） ----------
       同键两次发起（同 scope/view/sort，如刷新重发）：新实例先落、旧实例后至。
       参数逐字全同，唯一判据是代际。 */
    await resetStore();
    S.listPlan = { mode: 'defer' };
    store.getState().selectView('starred'); // 旧实例发起
    await nTick(0);
    const t119SameOld = S.pendingList.at(-1);
    void store.getState().reloadFilteredEntries('starred'); // 同键新实例（同查询重发）
    await nTick(0);
    const t119SameNew = S.pendingList.at(-1);
    t119SameNew.resolve([mkRow({ id: 8302, feed_id: 10, is_starred: true, published_at: OLD })]); // 新先落
    await nTick(10);
    t119SameOld.resolve([mkRow({ id: 8301, feed_id: 10, is_starred: true, published_at: OLD })]); // 旧后至（载荷不同）
    S.listPlan = null;
    await nTick(10);
    checkNew('(t119-2) 同查询旧版本先发后至不覆盖（判别）：两次同参数 starred 请求，旧实例迟到响应被代际丢弃、entries 保持新实例结果（修前回退 8301）',
      store.getState().entries.map((e) => e.id).join(',') === '8302');

    /* ---------- (t119-3) 计数竞态：改回未读后旧计数不应用且重取一次（探针 P6 本体） ----------
       全部已读成功 → feed_counts 对账在途（defer 挂起，载荷=发起时后端全读态
       unread 0）→ 用户把文章改回未读（本地读态写入：序号 bump + set_read 落库）
       → 迟到计数落地：必须判过期丢弃、立即重取恰一次；重取（期间无新写入）
       落地后端口径真值。 */
    await resetStore();
    S.feedCountsImpl = countsFromRows;
    S.backendRows = Array.from({ length: 600 }, (_, i) =>
      mkRow({ id: 66000 + i, feed_id: 10, published_at: iso(NOW - i * 1000) }));
    await store.getState().bootstrapFromBackend();
    store.setState((s) => ({ entries: s.entries.slice(0, 1) })); // 600/1 形态（t104 同款）
    S.feedCountsPlan = { mode: 'defer' }; // 挂起对账请求（一次性 defer）
    store.getState().markCurrentViewAllRead();
    await nTick(10); // mark_all_read 落库（600 行翻已读）→ 对账请求进入 defer
    const t119StaleCounts = countsFromRows(); // 对账响应载荷 = 发起时后端真值（全读态：unread 0）
    checkNew('(t119-3) 场景成立（600/1）：对账在途挂起、计数停留乐观值 599、迟到载荷为全读态 0',
      S.pendingCounts.length === 1
      && store.getState().feedCounts.get('10')?.unread === 599
      && t119StaleCounts.find((c) => c.feed_id === 10)?.unread === 0);
    store.getState().toggleEntryFlag(store.getState().entries[0].id, 'isRead'); // 改回未读：本地读态写入（序号 bump）+ set_read 落库
    await nTick(10); // set_read 落库（DB 真值 = 1 篇未读）
    checkNew('(t119-3) 用户改回未读：条目未读、计数乐观回补 600',
      store.getState().entries[0].isRead === false
      && store.getState().feedCounts.get('10')?.unread === 600);
    S.feedCountsPlan = { mode: 'defer' }; // 重取也挂起：可断言「恰重取一次」且不级联
    S.pendingCounts[0].resolve(t119StaleCounts); // 迟到计数落地
    await nTick(10);
    checkNew('(t119-3) 迟到计数不应用（判别）：期间有本地读写入 → 全读态旧计数（unread 0）被丢弃，计数保持乐观值 600、条目仍未读（修前整体替换 → 文章未读但未读数 0）',
      store.getState().feedCounts.get('10')?.unread === 600
      && store.getState().entries[0].isRead === false);
    checkNew('(t119-3) 过期即重取恰一次：对账立即重取且不级联（重取恰一次在途）',
      S.pendingCounts.length === 2);
    S.pendingCounts[1].resolve(countsFromRows()); // 重取落地（载荷 = 含改回未读的后端真值）
    await nTick(10);
    checkNew('(t119-3) 重取落地自愈：计数收敛到含改回未读的后端口径 unread 1',
      store.getState().feedCounts.get('10')?.unread === 1
      && countsFromRows().find((c) => c.feed_id === 10)?.unread === 1);

    /* ---------- (t119-4) 续页旧代际丢弃（探针 P4 形态） ----------
       续拉在途期间同数据重拉落地：游标三元组（last_published/last_id/loaded）
       与续页发起时全等 → paginationStale 参数比较不可判别（审计原话：不能只
       比较查询参数），唯一判据是续页携带的首屏代际（重拉发起已 bump 同键序号）。 */
    await resetStore();
    S.backendRows = Array.from({ length: 600 }, (_, i) =>
      mkRow({ id: 67000 + i, feed_id: 10, published_at: iso(NOW - i * 60000) }));
    await store.getState().bootstrapFromBackend(); // 首屏 500（67000..67499）
    S.listPlan = { mode: 'defer' };
    const t119LmP = store.getState().loadMoreArticles(); // 续页在途（携带首屏代际）
    await nTick(0);
    const t119LmPending = S.pendingList[0];
    void store.getState().reloadFromBackend(); // 同数据重拉（其 list 请求同样 defer）
    await nTick(0);
    const t119ReloadPending = S.pendingList[1];
    checkNew('(t119-4) 场景成立：续页与重拉两请求在途（续页先发）',
      S.pendingList.length === 2 && !!t119LmPending && !!t119ReloadPending);
    t119ReloadPending.resolve(queryRows(t119ReloadPending.args)); // 重拉先落（同数据 → 游标三元组与续页发起时全等）
    await nTick(10);
    checkNew('(t119-4) 同数据重拉落地：首屏游标三元组与续页发起时全等（参数比较不可判别形态成立）',
      store.getState().entries.length === 500
      && store.getState().articlesCursor['article|all']?.loaded === 500
      && store.getState().articlesCursor['article|all']?.lastId === 67499);
    t119LmPending.resolve(queryRows(t119LmPending.args)); // 续页旧代际响应后至（= 锚后 100 行 67500..67599）
    await t119LmP;
    S.listPlan = null;
    await nTick(10);
    checkNew('(t119-4) 续页旧代际丢弃（判别）：期间发生了更新的首屏发起 → 迟到续页整页丢弃、entries 保持首屏 500 条、加载态复位（修前参数全同放行 → 追加至 600）',
      store.getState().entries.length === 500
      && store.getState().entries.every((e) => Number(e.id) <= 67499)
      && store.getState().articlesLoading === false);

    /* ---------- (t119-5) 统一后既有 reloadGeneration 保护场景仍绿 ----------
       (a) 跨范围 reload：两个 reload 是不同 queryKey（per-key 判据不设防），
       全局代际承担原 reloadGeneration 的范围切换保护；(b) 锚定使在途 reload
       过期（anchorToArticle 注释声明的既有竞态语义）。统一收口后两类保护
       原样保留，不得回退。 */
    await resetStore();
    S.listPlan = { mode: 'defer' };
    const t119ScopeOldP = store.getState().reloadFromBackend(); // 范围 all（旧实例）
    await nTick(0);
    const t119ScopeOldPending = S.pendingList[0];
    store.getState().selectFeed('feed-10'); // 范围切换 → 自带新 reload（新实例）
    await nTick(0);
    const t119ScopeNewPending = S.pendingList[1];
    t119ScopeNewPending.resolve(queryRows(t119ScopeNewPending.args)); // 新范围先落（feed-10 三条）
    await nTick(10);
    checkNew('(t119-5) 场景成立：切范围后新范围首屏落地（feed-10 三条）',
      store.getState().entries.length === 3
      && store.getState().entries.every((e) => e.feedId === '10'));
    t119ScopeOldPending.resolve(queryRows(t119ScopeOldPending.args)); // 旧范围（8 行全量）后至
    await t119ScopeOldP;
    S.listPlan = null;
    await nTick(10);
    checkNew('(t119-5) 跨范围旧 reload 丢弃（统一守卫，原 reloadGeneration 范围切换保护不回退）：entries 保持新范围快照',
      store.getState().entries.length === 3
      && store.getState().entries.every((e) => e.feedId === '10'));

    await resetStore();
    S.listPlan = { mode: 'defer' };
    void store.getState().reloadFromBackend(); // reload 在途（旧实例）
    await nTick(0);
    const t119AnchorReloadPending = S.pendingList[0];
    const t119AnchorP = store.getState().anchorToArticle('301'); // 锚定：全局代际 bump + 同键首屏序号 bump
    await nTick(0);
    const t119AnchorPending = S.pendingList[1];
    t119AnchorPending.resolve(queryRows(t119AnchorPending.args)); // 锚定窗口先落（pos 3 起 5 行）
    await t119AnchorP;
    await nTick(10);
    checkNew('(t119-5) 锚定落地：entries 为锚定窗口（301,202,105,104,103）、activeArticleId=301',
      store.getState().activeArticleId === '301'
      && store.getState().entries.map((e) => e.id).join(',') === '301,202,105,104,103');
    t119AnchorReloadPending.resolve(queryRows(t119AnchorReloadPending.args)); // reload 旧响应后至（8 行全量）
    S.listPlan = null;
    await nTick(10);
    checkNew('(t119-5) 在途 reload 被锚定过期（判别，原 anchorToArticle 竞态语义）：迟到的 reload 快照不覆盖锚定窗口',
      store.getState().entries.map((e) => e.id).join(',') === '301,202,105,104,103'
      && store.getState().activeArticleId === '301');
    await resetStore(); // 夹具复位：不把 defer 残留带给后续块
  }
}
