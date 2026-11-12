// 固定成员分页的生产store回归。
// 映射旧(g/s1-s7/A1-A5/t110/t117)为全成员ID切片；旧t119/OPT-020保护保留请求交错。
// keyset内部字符串外形不再是前端契约，QueryScope的公开范围/键接口仍验证。
export const id='pagination-and-cache';
export async function run(ctx) {
  const {S,store,checkNew,nTick,resetStore,waitUntil,mkRow,NOW,iso,bodyOf,getBodyEntry}=ctx;
  ctx.useMainBackend();
  const {QueryScope,ARTICLES_PAGE_SIZE,viewEntriesCache}=await import('../../dist-test/store/internals.js');
  const {awaitLocalFlagWriteFrontier}=await import('../../dist-test/lib/api.js');
  const ids=()=>ctx.selectVisibleEntries(store.getState()).map(row=>row.id);
  const calls=cmd=>S.invokeCalls.filter(call=>call.cmd===cmd);
  const ready=()=>waitUntil(()=>store.getState().readingSession?.ready&&store.getState().backendReloadsInFlight===0,'会话就绪');
  const navigate=async fn=>{fn();await ready();};
  const rows=(n,start=1000,feed=10)=>Array.from({length:n},(_,i)=>mkRow({id:start+i,feed_id:feed,title:`entry-${start+i}`,is_starred:true,published_at:iso(NOW-i*1000)}));
  const setup=async(data,extra={})=>{await resetStore(extra);S.backendRows=data;S.feedCountsImpl=ctx.countsFromRows;await store.getState().reloadFromBackend();};
  const updates=pending=>({items:S.backendRows.filter(row=>pending.ids.includes(row.id)),missing_ids:pending.ids.filter(id=>!S.backendRows.some(row=>row.id===id))});
  checkNew('(t110-0) 首屏/续页共享500轻属性预算',ARTICLES_PAGE_SIZE===500);
  checkNew('(s0/t109) QueryScope范围/布局/排序派生与公开缓存键',JSON.stringify(QueryScope.args('cat-2','oldest','social'))===JSON.stringify({feed_id:null,folder_id:2,newest_first:false,layout:'social'})&&QueryScope.args('feed-12','newest','article').feed_id===12&&QueryScope.pageKey('10','article')==='article|10'&&QueryScope.viewKey('article','starred','10')==='article|starred|10');

  await setup(rows(503));
  checkNew('(g) 首屏只读500属性但已捕获503成员，进度/锁/未到底一致',ids().length===500&&store.getState().readingSession.memberIds.length===503&&store.getState().articlesLimit===500&&!store.getState().articlesExhausted&&!store.getState().articlesLoading);
  S.invokeCalls.length=0;await store.getState().loadMoreArticles();
  checkNew('(g) 第二页精确ID[1500,1501,1502]，不再向动态未读/收藏查询续页',calls('get_article_updates')[0]?.args.ids.join(',')==='1500,1501,1502'&&calls('list_article_member_ids').length===0&&calls('list_articles').length===0);
  checkNew('(g) 第二页追加连续唯一且进度按成员位置推进',ids().length===503&&ids()[500]==='1500'&&ids().at(-1)==='1502'&&new Set(ids()).size===503&&store.getState().articlesLimit===503);
  checkNew('(g) 全成员尽才exhausted，锁收起',store.getState().articlesExhausted&&!store.getState().articlesLoading);
  S.invokeCalls.length=0;await store.getState().loadMoreArticles();checkNew('(g) 到底后零IPC',S.invokeCalls.length===0);

  await setup(rows(503));S.updatesPlan={mode:'defer'};
  const page=store.getState().loadMoreArticles();await waitUntil(()=>S.pendingUpdates.length===1);
  await store.getState().loadMoreArticles();
  checkNew('(g) 在途续页单飞且加载态为真',S.pendingUpdates.length===1&&store.getState().articlesLoading);
  S.pendingUpdates[0].resolve(updates(S.pendingUpdates[0]));await page;S.updatesPlan=null;
  checkNew('(g) 当前请求完成才释放加载态且成员齐全',!store.getState().articlesLoading&&ids().length===503);

  await setup(rows(503));const before=ids().join(',');S.updatesPlan={mode:'reject',error:{message:'页失败'}};
  await store.getState().loadMoreArticles();const error=store.getState().errors.find(e=>e.key.startsWith('list:load-more:'));
  checkNew('(g/s5/D2) 续页失败不消耗位置、不缩窗、不假到底',ids().join(',')===before&&store.getState().readingSession.loadedCount===500&&!store.getState().articlesLoading&&!store.getState().articlesExhausted);
  checkNew('(g/s5/D2) 失败有可达错误中心及toast重试',!!error?.action.run&&store.getState().toasts.some(t=>t.text.includes('加载更多失败')));
  const generation=store.getState().readingSession.generation;
  checkNew('(OPT-020) retry key绑定会话代次与进度，查询身份由代次拥有',error?.key===`list:load-more:${generation}:500`);
  store.getState().retryError(error.id);await nTick();
  checkNew('(OPT-020) 同页重试再失败恢复相同key入口',store.getState().errors.some(e=>e.key===error.key&&e.id!==error.id));
  S.updatesPlan={mode:'defer'};const busy=store.getState().loadMoreArticles();await waitUntil(()=>S.pendingUpdates.length===1);
  const busyRetry=store.getState().errors.find(e=>e.key===error.key);store.getState().retryError(busyRetry.id);await nTick();
  checkNew('(OPT-020 busy) 忙页拒绝重试仍留入口，且无重复IPC',S.pendingUpdates.length===1&&store.getState().errors.some(e=>e.key===error.key));
  S.pendingUpdates[0].resolve(updates(S.pendingUpdates[0]));await busy;S.updatesPlan=null;
  S.invokeCalls.length=0;error.action.run();await nTick();
  checkNew('(OPT-020 exhausted) 过时/已到底retry明确提示且不再请求',S.invokeCalls.length===0&&store.getState().toasts.some(t=>t.text.includes('重试上下文已失效')));

  await setup([...rows(503,2000,10),...rows(503,3000,12)],{activeFeedFilter:'10'});
  S.updatesPlan={mode:'reject',error:{message:'A失败'}};await store.getState().loadMoreArticles();const aError=store.getState().errors.at(-1);
  S.updatesPlan=null;await navigate(()=>store.getState().selectFeed('10'));
  S.updatesPlan={mode:'reject',error:{message:'A新实例失败'}};await store.getState().loadMoreArticles();const a2Error=store.getState().errors.at(-1);
  checkNew('(OPT-020 generation) 同范围同进度重筛后失败key不同',aError.key!==a2Error.key);
  S.updatesPlan=null;await navigate(()=>store.getState().selectFeed('12'));const bBefore=ids().join(',');
  S.invokeCalls.length=0;aError.action.run();await nTick();
  checkNew('(OPT-020 scope) 跨范围旧retry不发B请求、不拼A实体',S.invokeCalls.length===0&&ids().join(',')===bBefore&&store.getState().entries.every(e=>e.feedId==='12'));

  // A尾页晚于B尾页：旧finally也不能清除B的锁。
  await navigate(()=>store.getState().selectFeed('10'));S.updatesPlan={mode:'defer'};const aPage=store.getState().loadMoreArticles();await waitUntil(()=>S.pendingUpdates.length===1);const heldA=S.pendingUpdates[0];
  S.updatesPlan=null;await navigate(()=>store.getState().selectFeed('12'));S.updatesPlan={mode:'defer'};const bPage=store.getState().loadMoreArticles();await waitUntil(()=>S.pendingUpdates.length===2);const heldB=S.pendingUpdates[1];
  checkNew('(OPT-020 A/B) 两范围尾页真实重叠且B持锁',store.getState().articlesLoading&&heldA.ids[0]===2500&&heldB.ids[0]===3500);
  heldA.resolve(updates(heldA));await aPage;
  checkNew('(OPT-020 A/B) 旧A完成不能清B锁或覆盖B成员',store.getState().articlesLoading&&ids().length===500&&store.getState().entries.every(e=>e.feedId==='12'));
  heldB.resolve(updates(heldB));await bPage;S.updatesPlan=null;
  checkNew('(OPT-020 A/B) B完成才释放锁并追加正确尾部',!store.getState().articlesLoading&&ids().length===503&&ids().at(-1)==='3502');

  for(const scope of ['10','cat-1']) {
    await setup([...rows(600,4000,10),...rows(600,5000,12),...rows(600,6000,20)],{activeFeedFilter:scope});
    const first=ids();const captured=[...store.getState().readingSession.memberIds];
    checkNew(`(s1/s2/A1/A2) ${scope}首屏查询范围正确且完整成员有分区边界`,calls('list_article_member_ids').at(-1).args.args[scope==='10'?'feed_id':'folder_id']===Number(scope.replace('cat-',''))&&captured.length===(scope==='10'?600:1200)&&captured.every(id=>Number(id)<6000));
    S.invokeCalls.length=0;await store.getState().loadMoreArticles();
    checkNew(`(s1/s2/A1/A2) ${scope}续页精确消费该范围尾片，无前页重复`,JSON.stringify(calls('get_article_updates')[0].args.ids.map(String))===JSON.stringify(captured.slice(500,1000))&&ids().slice(500).every(id=>!first.includes(id)));
    checkNew(`(s1/s2/A1/A2) ${scope}实体来源无串范围且进度匹配`,store.getState().entries.every(e=>scope==='10'?e.feedId==='10':['10','12'].includes(e.feedId))&&store.getState().articlesLimit===Math.min(1000,captured.length));
  }
  await setup(rows(5),{activeFeedFilter:'20'});S.invokeCalls.length=0;await store.getState().loadMoreArticles();
  checkNew('(s3/A3) 空范围零成员立即到底且重复触底零IPC',ids().length===0&&store.getState().articlesExhausted&&store.getState().articlesLimit===0&&S.invokeCalls.length===0);
  await setup(rows(501));S.backendRows=S.backendRows.slice(0,500);await store.getState().loadMoreArticles();
  checkNew('(s3 missing) 删除尾部仍消费固定位置并保不可用占位，不反复空页',ids().length===501&&store.getState().entries.find(e=>e.id==='1500')?.unavailable&&store.getState().articlesExhausted);

  // 读取时当前全部ID都在固定范围；已看与未看读/藏变化均不能使尾部消失。
  for(const view of ['unread','starred']) {
    await setup(rows(1200),{activeViewFilter:view});const captured=[...store.getState().readingSession.memberIds];
    for(const row of S.backendRows) {if(view==='unread')row.is_read=true;else row.is_starred=false;}
    S.backendRows.unshift(mkRow({id:90000,is_starred:true,published_at:iso(NOW+1000)}));
    await store.getState().reloadFromBackend({keepReadingPosition:true});
    checkNew(`(t110/t117) ${view}后台属性变化不重筛首屏、不插入新成员`,ids().join(',')===captured.slice(0,500).join(','));
    await store.getState().loadMoreArticles();
    checkNew(`(t117) ${view}第二页500尾部即使全变属性仍连续可达`,ids().slice(500).join(',')===captured.slice(500,1000).join(',')&&!store.getState().articlesExhausted);
    await store.getState().loadMoreArticles();
    checkNew(`(t117) ${view}第三页到完整1200，唯一且真到底`,ids().join(',')===captured.join(',')&&new Set(ids()).size===1200&&store.getState().articlesExhausted);
    await navigate(()=>store.getState().selectView(view));
    checkNew(`(t110/t117) ${view}显式重筛才接纳新数据、移除过时筛选成员`,ids().join(',')==='90000');
  }
  await setup(rows(600).map(row=>({...row,published_at:iso(NOW)})));
  checkNew('(t117 tie) 同秒首屏ID降序且完整成员全序确定',ids()[0]==='1599'&&ids().at(-1)==='1100');await store.getState().loadMoreArticles();
  checkNew('(t117 tie) 同秒跨页600不重不漏',ids().at(-1)==='1000'&&ids().length===600&&new Set(ids()).size===600);
  await navigate(()=>store.getState().toggleTimelineSort());await store.getState().loadMoreArticles();
  checkNew('(s7/t117 oldest) 切排序重建完整方向，续页按新ID序列',ids()[0]==='1000'&&ids().at(-1)==='1599'&&store.getState().readingSession.query.sort==='oldest');

  await setup(rows(600));S.updatesPlan={mode:'defer'};const oldPage=store.getState().loadMoreArticles();await waitUntil(()=>S.pendingUpdates.length===1);const oldPageRead=S.pendingUpdates[0];
  S.updatesPlan=null;await navigate(()=>store.getState().toggleTimelineSort());const sorted=ids().join(',');oldPageRead.resolve(updates(oldPageRead));await oldPage;
  checkNew('(s7/p3-f1) 旧排序尾页不得追加新排序，锁不残留',ids().join(',')===sorted&&ids()[0]==='1599'&&!store.getState().articlesLoading);

  // 首屏代次A→B→A与同参数属性请求的先发后至分开验证。
  await resetStore({activeViewFilter:'starred'});S.backendRows=rows(3);S.memberPlan={mode:'defer'};
  const first=store.getState().reloadFromBackend();await waitUntil(()=>S.pendingMembers.length===1);store.getState().selectView('all');await waitUntil(()=>S.pendingMembers.length===2);store.getState().selectView('starred');await waitUntil(()=>S.pendingMembers.length===3);
  checkNew('(t119 A/B/A) 三次真实成员请求都在途，第三次代次独立',S.pendingMembers.length===3);
  S.pendingMembers[2].resolve([1002]);await ready();S.pendingMembers[1].resolve([1001]);S.pendingMembers[0].resolve([1000]);await first;await nTick();S.memberPlan=null;
  checkNew('(t119 A/B/A) 旧A和B晚到不覆盖最新A',ids().join(',')==='1002');
  S.updatesPlan={mode:'defer'};const r1=store.getState().reloadFromBackend();await waitUntil(()=>S.pendingUpdates.length===1);const r2=store.getState().reloadFromBackend();await waitUntil(()=>S.pendingUpdates.length===2);
  S.pendingUpdates[1].resolve({items:[mkRow({id:1002,title:'fresh'})],missing_ids:[]});await r2;
  S.pendingUpdates[0].resolve({items:[mkRow({id:1002,title:'stale'})],missing_ids:[]});await r1;S.updatesPlan=null;
  checkNew('(t119 same query) 同会话两次属性回读逆序只接受更新请求',store.getState().entries[0].title==='fresh');

  await setup(rows(600));S.updatesPlan={mode:'defer'};const heldPage=store.getState().loadMoreArticles();await waitUntil(()=>S.pendingUpdates.length===1);const stalePage=S.pendingUpdates[0];S.updatesPlan=null;
  await navigate(()=>store.getState().selectView('all'));stalePage.resolve(updates(stalePage));await heldPage;
  checkNew('(t119-4/D3) 同参数重新建会话使旧尾页失效，加载锁可恢复',ids().length===500&&!store.getState().articlesLoading);
  await store.getState().loadMoreArticles();checkNew('(t119-4/D3) 丢旧页后当前尾页仍能达到600',ids().length===600&&store.getState().articlesExhausted);

  // 读取/本地写前沿：旧属性与计数不能把已确认写反转。
  await setup(rows(2));S.updatesPlan={mode:'defer'};const read=store.getState().reloadFromBackend();await waitUntil(()=>S.pendingUpdates.length===1);const oldRows=S.backendRows.map(row=>({...row}));
  store.getState().toggleEntryFlag('1000','isStarred');await awaitLocalFlagWriteFrontier();
  S.pendingUpdates[0].resolve({items:oldRows,missing_ids:[]});await read;S.updatesPlan=null;
  checkNew('(t119-3 read frontier) 读取期间成功的收藏写不被旧属性翻回',store.getState().entries.find(e=>e.id==='1000').isStarred===false);

  // 旧导航失败全部具有catch：实际监听unhandled，而非数源码里的10处catch外形。
  const unhandled=[];const handler=error=>unhandled.push(error);process.on('unhandledRejection',handler);
  try {
    for(const [label,action] of [
      ['selectView miss',()=>store.getState().selectView('starred')],
      ['selectView return',()=>store.getState().selectView('all')],
      ['selectFeed',()=>store.getState().selectFeed('12')],
      ['selectLayout',()=>store.getState().selectLayout('social')],
      ['sort',()=>store.getState().toggleTimelineSort()],
      ['filter',()=>store.getState().toggleTimelineFilter()],
    ]) {
      S.memberPlan={mode:'reject',error:{message:'受控导航失败'}};const toastCount=store.getState().toasts.length;action();await nTick();
      checkNew(`(p3b/p3-f5) ${label}失败可见且不抛未处理拒绝`,unhandled.length===0&&store.getState().toasts.some(t=>t.text.includes('受控导航失败'))&&store.getState().backendReloadsInFlight===0);
      void toastCount;
    }
  } finally {S.memberPlan=null;process.removeListener('unhandledRejection',handler);}

  await setup(rows(3));S.getArticlesPlan={rows:[{...S.backendRows.find(row=>row.id===1000),content_html:'<p>正文不丢</p>'}]};store.getState().hydrateArticleContent(['1000']);await nTick();const cachedBody=getBodyEntry('1000');S.getArticlesPlan=null;
  await navigate(()=>store.getState().selectFeed('12'));await navigate(()=>store.getState().selectFeed('10'));
  checkNew('(s4/s4b/t109) 源往返重新查询成员但正文缓存引用不丢',getBodyEntry('1000')===cachedBody&&bodyOf(store.getState(),'1000').content==='<p>正文不丢</p>'&&ids().length===3);
  checkNew('(s6 cache) 范围与视图键不混用，真实all和单源快照独立',!!viewEntriesCache.get('article|all|all')&&!!viewEntriesCache.get('article|all|10')&&viewEntriesCache.get('article|all|all')!==viewEntriesCache.get('article|all|10'));

  // t109原缓存回放角例：现在返回会新捕获成员，但在途声明和已存副本仍不能被导航误作后端确认。
  await setup(rows(2));
  const originalInvoke = globalThis.__INVOKE__;
  const pendingBulk = [];
  const restoreInvoke = ctx.overrideGlobal(globalThis, '__INVOKE__', (cmd, args) => cmd === 'set_read_bulk'
    ? new Promise((resolve, reject) => pendingBulk.push({ args, resolve, reject })) : originalInvoke(cmd, args));
  try {
    store.getState().markCurrentViewAllRead();
    await waitUntil(() => pendingBulk.length === 1);
    S.memberPlan = { mode: 'defer' };
    store.getState().selectFeed('all');
    await waitUntil(() => S.pendingMembers.length === 1);
    checkNew('(t109 cache pending) 同范围重筛在途，实体和现存缓存仍是乐观读态且计数0',
      store.getState().entries.every((row) => row.isRead)
      && viewEntriesCache.get('article|all|all').entries.every((row) => row.isRead)
      && store.getState().feedCounts.get('10').unread === 0);
    pendingBulk[0].reject({ message: '受控保存失败' });
    await awaitLocalFlagWriteFrontier();
    await waitUntil(() => store.getState().feedCounts.get('10').unread === 2);
    checkNew('(t109 cache pending) 新成员未回应也不使旧声明失效，失败恢复所有缓存/实体/计数',
      store.getState().entries.every((row) => !row.isRead)
      && viewEntriesCache.get('article|all|all').entries.every((row) => !row.isRead));
    S.pendingMembers[0].resolve([1000, 1001]);
    S.memberPlan = null;
    await ready();
    store.getState().markCurrentViewAllRead();
    await waitUntil(() => pendingBulk.length === 2);
    pendingBulk[1].reject({ message: '新声明也失败' });
    await awaitLocalFlagWriteFrontier();
    await waitUntil(() => store.getState().feedCounts.get('10').unread === 2);
    checkNew('(t109 cache new claim) 返回完成后的新声明仍能回滚，两行未读且计数2',
      store.getState().entries.every((row) => !row.isRead)
      && viewEntriesCache.get('article|all|all').entries.every((row) => !row.isRead));
  } finally { S.memberPlan = null; restoreInvoke(); }

  // t119-3：600成员但只驻留1实体，全读成功的旧计数必须被后续unread作废，并且只补查一次。
  await setup(rows(600));
  store.setState((state) => ({ entries: state.entries.slice(0, 1) }));
  viewEntriesCache.clear(); // 真正600/1：旧缓存不能额外提供499个已知旗标。
  S.invokeCalls.length = 0;
  S.feedCountsPlan = { mode: 'defer' };
  store.getState().markCurrentViewAllRead();
  await waitUntil(() => S.pendingCounts.length === 1);
  const oldCounts = ctx.countsFromRows();
  checkNew('(t119-3) 600/1全读已提交，权威0快照在途而乐观未读599',
    oldCounts.find((row) => row.feed_id === 10).unread === 0
    && store.getState().feedCounts.get('10').unread === 599);
  S.feedCountsPlan = { mode: 'defer' };
  store.getState().toggleEntryFlag('1000', 'isRead');
  await waitUntil(() => S.pendingCounts.length === 2);
  checkNew('(t119-3) 后续改回未读已提交，旧计数未接受前本地意图仍可见',
    !store.getState().entries[0].isRead && !S.backendRows[0].is_read
    && store.getState().feedCounts.get('10').unread === 600);
  S.feedCountsPlan = { mode: 'defer' };
  S.pendingCounts[0].resolve(oldCounts);
  await waitUntil(() => S.pendingCounts.length === 3);
  checkNew('(t119-3) 迟到全读0被拒落，只补一个有界对账且未回踩未读意图',
    !store.getState().entries[0].isRead && store.getState().feedCounts.get('10').unread === 600
    && calls('counts_snapshot').length === 3);
  S.pendingCounts[1].resolve(ctx.countsFromRows());
  await nTick();
  checkNew('(t119-3) 第二个旧计数也不越过更新令牌，没有级联重试',
    calls('counts_snapshot').length === 3 && store.getState().feedCounts.get('10').unread === 600);
  S.pendingCounts[2].resolve(ctx.countsFromRows());
  await waitUntil(() => store.getState().feedCounts.get('10').unread === 1);
  checkNew('(t119-3) 最新有界重取收敛真实未读1，计数/实体/后端一致',
    !store.getState().entries[0].isRead && ctx.countsFromRows()[0].unread === 1 && calls('counts_snapshot').length === 3);

  await setup([...rows(2, 1000, 10), ...rows(2, 2000, 12)]);
  S.updatesPlan = { mode: 'defer' };
  const oldScopeRead = store.getState().reloadFromBackend();
  await waitUntil(() => S.pendingUpdates.length === 1);
  const oldScope = S.pendingUpdates[0];
  S.updatesPlan = null;
  await navigate(() => store.getState().selectFeed('12'));
  oldScope.resolve(updates(oldScope));
  await oldScopeRead;
  checkNew('(t119-5 scope) 跨范围旧属性回读不得覆盖新范围，已驻留与成员均限源12',
    ids().join(',') === '2000,2001' && store.getState().entries.every((row) => row.feedId === '12'));
  S.updatesPlan = { mode: 'defer' };
  const oldAnchorRead = store.getState().reloadFromBackend();
  await waitUntil(() => S.pendingUpdates.length === 2);
  S.updatesPlan = null;
  store.setState({ settings: { ...store.getState().settings, markReadOnOpen: false } });
  await store.getState().anchorToArticle('2001');
  S.pendingUpdates[1].resolve({ items: S.backendRows.filter((row) => row.feed_id === 12).map((row) => ({ ...row, title: '旧锚定前属性' })), missing_ids: [] });
  await oldAnchorRead;
  checkNew('(t119-5 anchor) 锚定使先发属性回读过期，选中/原窗口/属性都不被迟到响应覆盖',
    store.getState().activeArticleId === '2001' && ids().join(',') === '2000,2001'
    && store.getState().entries.every((row) => row.title !== '旧锚定前属性'));

  await setup(rows(3));
  S.memberPlan = { mode: 'defer' };
  store.getState().selectView('starred');
  await waitUntil(() => S.pendingMembers.length === 1);
  store.getState().toggleTimelineSort();
  await waitUntil(() => S.pendingMembers.length === 2);
  const readsBeforeOldSort = calls('get_article_updates').length;
  S.pendingMembers[0].resolve([1000, 1001, 1002]);
  await nTick();
  checkNew('(t109 filtered sort) 收藏首屏旧排序成员晚到被拒绝，不发属性页且新查询仍在途',
    calls('get_article_updates').length === readsBeforeOldSort && store.getState().backendReloadsInFlight === 1);
  S.pendingMembers[1].resolve([1002, 1001, 1000]);
  S.memberPlan = null;
  await ready();
  checkNew('(t109 filtered sort) 新排序收藏查询按原样成员序落地，三条完整且真到底',
    ids().join(',') === '1002,1001,1000' && store.getState().articlesExhausted
    && store.getState().readingSession.query.sort === 'oldest');

  store.setState({dataMode:'mock'});S.invokeCalls.length=0;store.getState().toggleTimelineSort();store.getState().selectFeed('10');
  checkNew('(s4 mock/p3-f5) mock导航和排序无IPC且不切tauri',S.invokeCalls.length===0&&store.getState().dataMode==='mock');
}
