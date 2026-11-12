// 启动、查询、导航与搜索：生产store/API，IPC仅控制数据与完成时机。
// 迁移映射：(a)启动与代次→members/updates；(b/c/d)查询受分区且显式导航才重筛；
// (h/h2)位置来自固定members，搜索注入与分页独立。原toast/句柄/auth检查保留。
export const id = 'bootstrap-and-query';
export async function run(ctx) {
  const { S, store, checkNew, nTick, resetStore, bootFixture, waitUntil, NOW, mkRow,
    selectVisibleEntries, selectScopeEntries, selectTreeCounts, selectViewCounts,
    bodyOf, getBodyEntry, entryNeedsHydrationT122, appStoreUnbound, internalsBeforeBind, overrideGlobal } = ctx;
  await ctx.useMainBackend();
  const ids = () => selectVisibleEntries(store.getState()).map(row => row.id).join(',');
  const calls = cmd => S.invokeCalls.filter(call => call.cmd === cmd);
  const landed = () => waitUntil(() => store.getState().readingSession?.ready && store.getState().backendReloadsInFlight === 0, '会话落地');
  const nav = async action => { action(); await landed(); };
  const restoreWindow = overrideGlobal(globalThis, 'window', {});
  let aBrowser;
  try { await resetStore({ dataLoading: true }); await store.getState().bootstrapFromBackend(); aBrowser = store.getState(); }
  finally { restoreWindow(); }
  checkNew('(a) 浏览器启动加载真实演示模块并结束骨架', aBrowser.dataMode === 'mock' && !aBrowser.dataLoading && aBrowser.entries.length > 0 && aBrowser.feedIndex.size > 0);
  await bootFixture();
  const aOk = store.getState();
  checkNew('(a) tauri启动建立活动导航与来源索引、计数', aOk.dataMode === 'tauri' && !aOk.dataLoading && aOk.bootstrapError === null && aOk.feedIndex.size === 4 && aOk.sourceIndex.size === 4 && aOk.feedCounts.get('10')?.unread === 3);
  checkNew('(a) 首屏只含文章分区5成员，进度与真实成员数相同且已到底', aOk.entries.length === 5 && aOk.articlesLimit === 5 && aOk.articlesCursor['article|all']?.loaded === 5 && aOk.articlesExhausted && !aOk.articlesLoading);
  checkNew('(a) 完整成员命令传分区且轻属性按固定ID读取', calls('list_article_member_ids').at(-1)?.args.args.layout === 'article' && calls('get_article_updates').at(-1)?.args.ids.length === 5 && calls('list_articles').length === 0);
  S.getArticlesPlan = { rows: [mkRow({ id:101, content_html:'<p>水合正文</p>' })] };
  store.getState().hydrateArticleContent(['101']); await nTick(); S.getArticlesPlan = null;
  const hydratedCalls = calls('get_articles').length;
  await store.getState().reloadFromBackend();
  checkNew('(a) 属性刷新不丢已水合正文、终态或额外发正文请求', bodyOf(store.getState(),'101').content === '<p>水合正文</p>' && getBodyEntry('101')?.state === 'ready' && !entryNeedsHydrationT122(store.getState(),'101') && calls('get_articles').length === hydratedCalls);
  await resetStore(); S.failReload = { message:'数据库损坏' }; await store.getState().bootstrapFromBackend();
  checkNew('(a) 启动失败显示错误、关闭骨架，绝不回退假mock', store.getState().bootstrapError === '数据库损坏' && !store.getState().dataLoading && store.getState().dataMode === 'tauri' && store.getState().entries.length === 0);
  S.failReload = null; await store.getState().retryBootstrap();
  checkNew('(a) 启动重试恢复正确分区并清错误', store.getState().bootstrapError === null && ids() === '102,101,105,104,103');

  await resetStore(); S.memberPlan = { mode:'defer' };
  const oldReload = store.getState().reloadFromBackend();
  await waitUntil(() => S.pendingMembers.length === 1);
  const newReload = store.getState().reloadFromBackend();
  await waitUntil(() => S.pendingMembers.length === 2);
  checkNew('(a) 同范围两次完整成员请求确实同时在途', S.pendingMembers.length === 2);
  S.pendingMembers[1].resolve([105]); await newReload;
  S.pendingMembers[0].resolve([101]); await oldReload;
  checkNew('(a) 迟到旧首屏不能覆盖同参数新实例', ids() === '105'); S.memberPlan = null;

  await resetStore({ dataLoading:true }); S.memberPlan = { mode:'defer' };
  const staleBoot = store.getState().bootstrapFromBackend(); await waitUntil(() => S.pendingMembers.length === 1);
  const freshBoot = store.getState().reloadFromBackend(); await waitUntil(() => S.pendingMembers.length === 2);
  S.pendingMembers[0].resolve([101]); await staleBoot;
  checkNew('(a) 旧启动被接手后不能提前结束最新请求的骨架', store.getState().dataLoading && store.getState().bootstrapError === null);
  S.pendingMembers[1].resolve([104]); await freshBoot;
  checkNew('(a) 接手请求关闭骨架并落新成员，不永挂splash', !store.getState().dataLoading && ids() === '104' && store.getState().bootstrapError === null); S.memberPlan = null;

  await resetStore({ dataLoading:true }); S.memberPlan = { mode:'defer' };
  const nullBoot = store.getState().bootstrapFromBackend(); await waitUntil(() => S.pendingMembers.length === 1);
  checkNew('(a) 空成员响应反例确有请求在途', S.pendingMembers.length === 1);
  S.pendingMembers[0].resolve(null); await nullBoot;
  checkNew('(a) null成员快照不是空库成功，呈现可重试启动错误', !store.getState().dataLoading && !!store.getState().bootstrapError);
  S.memberPlan = null; await store.getState().retryBootstrap();
  checkNew('(a) null快照失败后可重试恢复，成员齐全', store.getState().bootstrapError === null && ids() === '102,101,105,104,103');

  await resetStore({ dataLoading:true });
  const original = globalThis.__INVOKE__; let releaseSettings;
  const restore = overrideGlobal(globalThis,'__INVOKE__',(cmd,args) => cmd === 'get_setting' && args.key === 'app_settings'
    ? new Promise(resolve => { releaseSettings = resolve; }) : original(cmd,args));
  let boot;
  try {
    boot = store.getState().bootstrapFromBackend(); await waitUntil(() => releaseSettings != null);
    checkNew('(a) 设置在途时来源/计数元数据已并行发出', calls('list_folders').length === 1 && calls('list_feeds').length === 1 && calls('counts_snapshot').length === 1);
    checkNew('(a) 设置未接受不能捕获默认成员，骨架保持', calls('list_article_member_ids').length === 0 && store.getState().dataLoading);
    releaseSettings(JSON.stringify({startupView:'starred'})); await boot;
    checkNew('(a) 设置接受后只查询真实starred成员，不用all过渡窗口', calls('list_article_member_ids').length === 1 && calls('list_article_member_ids')[0].args.args.only_starred === true);
    checkNew('(a) 启动starred只含当前分区两篇，完成有序落地', store.getState().activeViewFilter === 'starred' && ids() === '102,103' && !store.getState().dataLoading);
  } finally { releaseSettings?.(null); if (boot) await boot; restore(); }

  await bootFixture();
  store.setState({ openedReadIds:{999:true} });
  await nav(() => store.getState().selectFeed('11'));
  checkNew('(b) 选源清已打开标记但保分区边界，文章分区不混社交源', store.getState().activeFeedFilter === '11' && Object.keys(store.getState().openedReadIds).length === 0 && ids() === '');
  await nav(() => store.getState().selectLayout('social'));
  await nav(() => store.getState().selectFeed('11'));
  checkNew('(b) 社交分区源B得到两篇', ids() === '201,202');
  await nav(() => store.getState().selectFeed('cat-1'));
  checkNew('(b) 组范围和分区共同筛选，不含另一组社交源', ids() === '201,202');
  await nav(() => store.getState().selectLayout('article'));
  await nav(() => store.getState().selectFeed('cat-1'));
  checkNew('(b) 相同组在文章分区只包含源A和D五篇', ids() === '102,101,105,104,103');
  store.setState({timelineFilter:'unread'});
  let tree = selectTreeCounts(store.getState());
  checkNew('(b) 未读树计数按分区/组聚合精确源计数', tree.get('10') === 3 && tree.get('12') === 2 && tree.get('cat-1') === 5 && tree.get('11') === undefined);
  store.setState({timelineFilter:'all'}); tree=selectTreeCounts(store.getState());
  checkNew('(b) all树计数独立于分页数', tree.get('10') === 5 && tree.get('cat-1') === 7 && tree.get('all') === 7);
  store.setState({activeViewFilter:'starred'}); tree=selectTreeCounts(store.getState());
  checkNew('(b) 收藏树计数保留零值源', tree.get('10') === 1 && tree.get('12') === 0 && tree.get('all') === 1);
  store.setState({activeContentLayout:'social',activeFeedFilter:'cat-1'});
  checkNew('(b) 组计数只算当前社交源B', selectViewCounts(store.getState()).all === 4);
  store.setState({activeFeedFilter:'20',timelineFilter:'unread'});
  checkNew('(b) 单源计数按源C未读值', selectViewCounts(store.getState()).all === 1);
  store.setState({activeContentLayout:'article'});
  checkNew('(b) 分区不匹配源时范围和计数为空', selectScopeEntries(store.getState()).length === 0 && selectViewCounts(store.getState()).all === 0);

  await bootFixture(); await nav(() => store.getState().selectView('starred'));
  checkNew('(c) 收藏成员谓词与分区同时传递且已读收藏可见', ids() === '102,103' && calls('list_article_member_ids').at(-1).args.args.only_starred === true);
  const starGen=store.getState().readingSession.generation;
  await nav(() => store.getState().selectView('starred'));
  checkNew('(c) 重选相同视图也按最新状态建立新代次', store.getState().readingSession.generation > starGen && ids() === '102,103');
  store.getState().toggleEntryFlag('102','isStarred'); await nTick();
  checkNew('(c) 取消收藏保持当前成员位置、只改属性', ids() === '102,103' && !store.getState().entries.find(row=>row.id==='102').isStarred);
  await nav(() => store.getState().selectView('starred'));
  checkNew('(c) 显式重筛收藏后才移除已取消成员', ids() === '103');
  await nav(() => store.getState().selectView('unread'));
  checkNew('(c) 未读首屏只读当前分区四篇，进度/exhausted一致', ids() === '101,105,104,103' && store.getState().articlesLimit === 4 && store.getState().articlesExhausted);
  store.getState().selectArticle('101'); await nTick();
  checkNew('(c) 文章打开标读仍留在未读会话原位', ids() === '101,105,104,103' && store.getState().entries.find(row=>row.id==='101').isRead);
  await nav(() => store.getState().selectView('today'));
  const dayArgs=calls('list_article_member_ids').at(-1).args.args;
  checkNew('(c) 今天传明确半开日界且只显示分区当天两篇', ids() === '102,101' && dayArgs.since_ms < NOW && dayArgs.until_ms > NOW);
  await nav(() => store.getState().selectView('all'));
  checkNew('(c) all时间/同秒ID排序完整', ids() === '102,101,105,104,103');
  await nav(() => store.getState().toggleTimelineFilter());
  checkNew('(c) all未读切换真实重筛，已读不在新成员', ids() === '105,104,103');
  await nav(() => store.getState().toggleTimelineFilter());
  checkNew('(c) all筛选可逆，恢复已读成员', ids() === '102,101,105,104,103');
  await nav(() => store.getState().toggleTimelineSort());
  checkNew('(c) oldest重建后以时间和ID同向升序', ids() === '103,104,105,101,102');

  await bootFixture(); store.setState({activeArticleId:'101',isShowingTranslatedProse:true,isRawRenderMode:true,showFulltext:true});
  await nav(() => store.getState().selectLayout('social'));
  checkNew('(d) 分区切换重置范围/阅读器状态并真实拉对应成员', ids() === '201,301,202' && store.getState().activeFeedFilter === 'all' && store.getState().activeArticleId === null && !store.getState().isShowingTranslatedProse && !store.getState().isRawRenderMode && !store.getState().showFulltext);
  await nav(() => store.getState().selectLayout('image'));
  checkNew('(d) 无画廊源时真实查询为空，不串社交卡', ids() === '' && calls('list_article_member_ids').at(-1).args.args.layout === 'image');
  await nav(() => store.getState().selectLayout('podcast'));
  checkNew('(d) 无播客源时成员为空', ids() === '');
  await store.getState().updateFeedLayout('cat-1','11','podcast'); await nTick();
  await nav(() => store.getState().selectLayout('podcast'));
  checkNew('(d) 修改源绑定后下次显式导航按最新绑定筛选', ids() === '201,202' && calls('update_feed_layout').at(-1)?.args.layout === 'podcast');
  await store.getState().updateCatLayout('cat-2','podcast'); await nTick();
  await nav(() => store.getState().selectLayout('podcast'));
  checkNew('(d) inherit源跟随组绑定，显式源保自己的分区', ids() === '201,301,202');
  await store.getState().updateFeedLayout('cat-1','11','social'); await store.getState().updateCatLayout('cat-2','social'); await nTick();

  await bootFixture(); const before=store.getState().entries;
  store.getState().openSearch(); store.getState().closeSearch();
  checkNew('(h) 搜索开关只改面板状态，不动已加载实体', !store.getState().searchOpen && store.getState().entries === before);
  await store.getState().anchorToArticle('103'); await nTick();
  checkNew('(h) 已加载成员锚定只选中打开、不重建窗口', store.getState().activeArticleId === '103' && ids() === '102,101,105,104,103');
  await resetStore(); S.memberPlan={mode:'defer'};
  const oldAnchor=store.getState().anchorToArticle('101'); await waitUntil(()=>S.pendingMembers.length===1);
  const newAnchor=store.getState().anchorToArticle('105'); await waitUntil(()=>S.pendingMembers.length===2);
  checkNew('(h) 两次锚定完整成员读取在途，代次场景成立',S.pendingMembers.length===2);
  S.pendingMembers[0].resolve([101]); await oldAnchor;
  checkNew('(h) 旧锚定不发属性读取、不抢选中',calls('get_article_updates').length===0 && store.getState().activeArticleId===null);
  S.pendingMembers[1].resolve([105]); await newAnchor; await nTick(); S.memberPlan=null;
  checkNew('(h) 最新锚定才打开目标',store.getState().activeArticleId==='105' && ids()==='105');
  await bootFixture(); await store.getState().anchorToArticle('201'); await nTick();
  checkNew('(h) 会话外搜索结果按ID取属性并打开但不伪造文章分区成员',store.getState().activeArticleId==='201' && store.getState().entries.some(row=>row.id==='201') && ids()==='102,101,105,104,103');
  store.setState({dataMode:'mock',activeArticleId:null}); S.invokeCalls.length=0; await store.getState().anchorToArticle('101');
  checkNew('(h) mock锚定no-op，没有IPC或选中',S.invokeCalls.length===0 && store.getState().activeArticleId===null);

  const {anchorScopeNav}=await import('../../src/components/anchorScopeNav.ts');
  checkNew('(h2) 导航归一顺序仍为范围、视图、all筛选',anchorScopeNav({activeFeedFilter:'10',activeViewFilter:'starred',timelineFilter:'unread'}).map(step=>step.action).join(',')==='selectFeed,selectView,toggleTimelineFilter');
  checkNew('(h2) 已归一范围不产生多余动作',anchorScopeNav({activeFeedFilter:'all',activeViewFilter:'all',timelineFilter:'all'}).length===0);
  await resetStore({activeFeedFilter:'10',activeViewFilter:'starred',timelineFilter:'unread'}); await store.getState().reloadFromBackend(); S.invokeCalls.length=0;
  for(const step of anchorScopeNav(store.getState())) {
    if(step.action==='selectFeed') store.getState().selectFeed(step.arg??'all');
    else if(step.action==='selectView') store.getState().selectView('all');
    else store.getState().toggleTimelineFilter();
  }
  await store.getState().anchorToArticle('201'); await nTick();
  const searchQuery=calls('list_article_member_ids').at(-1)?.args.args;
  checkNew('(h2) 真实成员查询在导航后使用新范围与视图，不以缺命令冒充null参数',!!searchQuery && searchQuery.feed_id===null && searchQuery.folder_id===null && !searchQuery.only_starred && !searchQuery.only_unread);
  checkNew('(h2) 属性请求与捕获成员同一ID集合，非成员目标独立注入',calls('get_article_updates').some(call=>call.args.ids.length===5) && calls('get_article_updates').some(call=>call.args.ids.join(',')==='201'));
  checkNew('(h2) 导航归一后目标可打开，列表仍限当前分区',store.getState().activeArticleId==='201' && ids()==='102,101,105,104,103' && store.getState().timelineFilter==='all');
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

}
