// 固定会话保位/返回深度与readingPosition边界；不冒充真实浏览器几何。
// 旧t111/t115/t123源内动态keyset重取被ID属性回读取代，锚节流/偏移/LRU/关闭归还原保护保留。
export const id='scroll-position-restore';
export async function run(ctx) {
  const {S,store,checkNew,nTick,resetStore,waitUntil,mkRow,NOW,iso,viewEntriesCache}=ctx;ctx.useMainBackend();
  const {VIEW_ENTRIES_CACHE_ENTRY_BUDGET,ARTICLES_PAGE_SIZE,flipEntryFlag}=await import('../../dist-test/store/internals.js');
  const ta=await import('../../src/lib/readingPosition.ts');
  const fs=await import('node:fs');const read=p=>fs.readFileSync(new URL(p,import.meta.url),'utf8');
  const key=()=>{const s=store.getState();return `${s.activeContentLayout}|${s.activeViewFilter}|${s.activeFeedFilter}|${s.timelineFilter}|${s.timelineSort}`;};
  const items=()=>ctx.selectVisibleEntries(store.getState());
  const rows=(n,start=3000)=>Array.from({length:n},(_,i)=>mkRow({id:start+i,is_starred:true,published_at:iso(NOW-i*1000)}));
  const ready=()=>waitUntil(()=>store.getState().readingSession?.ready&&store.getState().backendReloadsInFlight===0);
  const nav=async fn=>{fn();await ready();};
  const setup=async data=>{await resetStore();ta.clearTopAnchor();S.backendRows=data;await store.getState().reloadFromBackend();};
  checkNew('(t111-0) 实体缓存1000预算≥单页500',VIEW_ENTRIES_CACHE_ENTRY_BUDGET===1000&&ARTICLES_PAGE_SIZE===500);
  await setup(rows(1501));await store.getState().loadMoreArticles();flipEntryFlag('3000','isStarred');
  let cache=viewEntriesCache.get('article|all|all');
  checkNew('(t111-1) 恰1000不截断，独立元数据记录1000/未到底',cache.entries.length===1000&&cache.cursor.loaded===1000&&!cache.exhausted&&cache.entries.at(-1).id==='3999');
  await store.getState().loadMoreArticles();flipEntryFlag('3001','isStarred');cache=viewEntriesCache.get('article|all|all');
  checkNew('(t111-1) 超预算截断实体但保留1500进度与未到底',cache.entries.length===1000&&cache.cursor.loaded===1500&&!cache.exhausted&&cache.entries[0].id==='3000'&&cache.entries.at(-1).id==='3999');
  await nav(()=>store.getState().selectFeed('12'));await nav(()=>store.getState().selectFeed('all'));
  checkNew('(t111-2) 返回重新捕获成员，恢复1500深度而不误用截断1000',items().length===1500&&store.getState().readingSession.loadedCount===1500&&!store.getState().articlesExhausted);
  S.invokeCalls.length=0;await store.getState().loadMoreArticles();
  checkNew('(t111-2) 返回后续页只取真正余下的4500，不漏掉被缓存截断的中段',S.invokeCalls.find(c=>c.cmd==='get_article_updates')?.args.ids.join(',')==='4500'&&items().length===1501&&new Set(items().map(e=>e.id)).size===1501&&store.getState().articlesExhausted);
  await setup(rows(1100,5000));await store.getState().loadMoreArticles();await store.getState().loadMoreArticles();cache=viewEntriesCache.get('article|all|all');
  checkNew('(t111-3) 1100加载完，缓存1000实体仍记录exhausted及1100进度',cache.entries.length===1000&&cache.cursor.loaded===1100&&cache.exhausted);
  await nav(()=>store.getState().selectFeed('12'));await nav(()=>store.getState().selectFeed('all'));S.invokeCalls.length=0;await store.getState().loadMoreArticles();
  checkNew('(t111-3) 返回恢复完整1100且真到底，重复续页零IPC',items().length===1100&&store.getState().articlesExhausted&&S.invokeCalls.length===0);

  await setup(rows(1250,1000));await store.getState().loadMoreArticles();const captured=items().map(e=>e.id);const t0=NOW+1000000;const originalKey=key();
  checkNew('(t111-4) 节流首次写锚生效，窗口内采样不覆盖',ta.recordTopAnchor('1749',originalKey,t0,137)&&!ta.recordTopAnchor('1750',originalKey,t0+100,0)&&ta.peekTopAnchor()?.id==='1749');
  const n0=store.getState().positionRestoreNonce;S.backendRows.unshift(mkRow({id:90000,published_at:iso(NOW+1)}));
  await store.getState().reloadFromBackend({keepReadingPosition:true});
  checkNew('(t111-4/t123-1) 后台插入只回读旧1000成员，不缩窗不插头部',items().map(e=>e.id).join(',')===captured.join(',')&&store.getState().readingSession.loadedCount===1000&&!store.getState().articlesExhausted);
  checkNew('(t111-4/t123-1) 深锚id/偏移137原样、索引749不动，刷新信号+1',ta.peekTopAnchor()?.offsetPx===137&&ta.anchorRestoreIndex(ta.peekTopAnchor(),key(),items())===749&&store.getState().positionRestoreNonce===n0+1);
  const refreshN=store.getState().positionRestoreNonce;await nav(()=>store.getState().selectView('starred'));
  checkNew('(t111-4) 显式视图导航不借用后台刷新信号',store.getState().positionRestoreNonce===refreshN);
  const starIds=items().map(e=>e.id).join(',');const starN=store.getState().positionRestoreNonce;S.backendRows.unshift(mkRow({id:90001,is_starred:true,published_at:iso(NOW+2)}));
  await store.getState().reloadFilteredEntries('starred',{keepReadingPosition:true});
  checkNew('(t111-4 filtered) 筛选会话刷新保成员而更新属性信号',items().map(e=>e.id).join(',')===starIds&&store.getState().positionRestoreNonce===starN+1);

  ta.clearTopAnchor();ta.recordTopAnchor(items()[0].id,key(),t0+10000,29);const missingId=ta.peekTopAnchor().id;S.backendRows=S.backendRows.filter(row=>String(row.id)!==missingId);await store.getState().reloadFromBackend();
  checkNew('(t111-5 deleted) 明确删除保不可用占位和锚位置，避免过程抽走',items()[0].unavailable===true&&ta.anchorRestoreIndex(ta.peekTopAnchor(),key(),items())===0);
  await nav(()=>store.getState().selectView('starred'));
  checkNew('(t111-5 requery) 重筛后锚确实不在成员才回null，不猜索引',ta.anchorRestoreIndex(ta.peekTopAnchor(),key(),items())===null);
  await setup(rows(2));ta.recordTopAnchor('3000',key(),t0+20000);const oldKey=key();const navN=store.getState().positionRestoreNonce;await nav(()=>store.getState().selectFeed('10'));
  checkNew('(t111-5 scope) 同ID仍在新范围时旧filterKey单独阻止回位',items()[0].id==='3000'&&oldKey!==key()&&ta.anchorRestoreIndex(ta.peekTopAnchor(),key(),items())===null&&store.getState().positionRestoreNonce===navN);
  checkNew('(t111-5 throttle) 249ms仍抑制，250ms允许用户新位置',ta.recordTopAnchor('3000',key(),t0+30000,1)&&!ta.recordTopAnchor('3001',key(),t0+30249,2)&&ta.recordTopAnchor('3001',key(),t0+30250,3));

  // 真实存档/新成员落地信号，恢复不在同步缓存回放里发生。
  await setup(rows(1000,4000));await store.getState().loadMoreArticles();const returnKey=key();ta.recordTopAnchor('4749',returnKey,t0+40000,60);ta.stashTopAnchorForReturn();ta.clearTopAnchor();
  await nav(()=>store.getState().selectFeed('12'));const returnN=store.getState().switchRestoreNonce;const refreshBeforeReturn=store.getState().positionRestoreNonce;
  S.backendRows.unshift(mkRow({id:91000,published_at:iso(NOW+100)}));await nav(()=>store.getState().selectFeed('all'));
  checkNew('(t115-x1/t123-2) 显式返回新成员重筛且恢复1000深度，存档锚仍能找到',items().length===1000&&items()[0].id==='91000'&&ta.anchorRestoreIndex(ta.peekReturnAnchor(returnKey),key(),items())===750&&ta.peekReturnAnchor(returnKey)?.offsetPx===60);
  checkNew('(t115-x3a) 返回信号+1且不借后台信号',store.getState().switchRestoreNonce===returnN+1&&store.getState().positionRestoreNonce===refreshBeforeReturn);
  const oldReturn=store.getState().switchRestoreNonce;const oldRefresh=store.getState().positionRestoreNonce;await store.getState().reloadFromBackend({keepReadingPosition:true});
  checkNew('(t115-x3a) 后台刷新只推进刷新信号，返回信号不叠加',store.getState().switchRestoreNonce===oldReturn&&store.getState().positionRestoreNonce===oldRefresh+1);
  // 原t115-x3c保护仍适用：相同范围重筛可以捕获新成员，但不得消费上次离开时的旧存档而把用户拉回。
  // 模拟返回后用户已滚到4760：filterKey未变，Timeline不会再次stash当前活锚。
  ta.commitTopAnchor('4760',returnKey,t0+40500,17);
  const repeatReturn=store.getState().switchRestoreNonce;
  const repeatPosition=store.getState().positionRestoreNonce;
  const repeatGeneration=store.getState().readingSession.generation;
  await nav(()=>store.getState().selectFeed('all'));
  checkNew('(t115-x3c) 同上下文重选允许重捕获成员，但不发旧存档返回信号（保持当前读位）',
    store.getState().readingSession.generation>repeatGeneration
    &&store.getState().switchRestoreNonce===repeatReturn
    &&store.getState().positionRestoreNonce===repeatPosition+1
    &&ta.peekTopAnchor()?.id==='4760'&&ta.peekReturnAnchor(returnKey)?.id==='4749',
    JSON.stringify({before:repeatReturn,after:store.getState().switchRestoreNonce,live:ta.peekTopAnchor(),archived:ta.peekReturnAnchor(returnKey)}));
  // 当前锚靠近已加载窗口末端时，头部插入不得把仍有效的锚挤到未加载区。
  ta.commitTopAnchor('4998',returnKey,t0+41000,23);
  S.backendRows.unshift(...rows(20,92000).map((row,i)=>({...row,published_at:iso(NOW+1000+i)})));
  const edgePosition=store.getState().positionRestoreNonce;
  await nav(()=>store.getState().selectFeed('all'));
  checkNew('(t115-x3c window edge) 重筛有头部插入仍覆盖先前已加载尾部，当前活锚不被挤出',
    items().some(entry=>entry.id==='4998')&&ta.anchorRestoreIndex(ta.peekTopAnchor(),key(),items())!==null
    &&store.getState().positionRestoreNonce===edgePosition+1);
  const beforeSort=store.getState().switchRestoreNonce;await nav(()=>store.getState().toggleTimelineSort());
  checkNew('(t115-x3c sort) 切排序新语境不恢复旧存档',store.getState().switchRestoreNonce===beforeSort&&ta.peekReturnAnchor(key())==null);

  // 关闭阅读器独立于列表/回位，连续打开关闭同一实体也要发信号。
  await setup(rows(3));store.getState().selectArticle('3001');await nTick();const closeN=store.getState().readerCloseNonce;const closeKey=key();store.getState().clearReaderSelection();
  checkNew('(t115-x2) 关闭清选中并推进独立nonce，不改阅读查询',store.getState().activeArticleId===null&&store.getState().readerCloseNonce===closeN+1&&key()===closeKey);
  checkNew('(t115-x2) 关闭焦点归还已有原卡，缺目标/无ID返回null',ta.readerFocusReturnIndex('3001',items())===1&&ta.readerFocusReturnIndex('missing',items())===null&&ta.readerFocusReturnIndex(null,items())===null);
  store.getState().selectArticle('3001');store.getState().clearReaderSelection();checkNew('(t115-x2) 相同文章连续关闭再次推进nonce',store.getState().readerCloseNonce===closeN+2);

  ta.clearTopAnchor();const t=t0+50000;ta.recordTopAnchor('a','key',t,13);ta.stashTopAnchorForReturn();
  checkNew('(t123-3) 像素偏移随记录和存档传递',ta.peekTopAnchor()?.offsetPx===13&&ta.peekReturnAnchor('key')?.offsetPx===13);
  ta.rearmTopAnchor('b','key',t+10,31);
  checkNew('(t115-x3b/t123-3) rearm绕节流设恢复位置并推进节流基准',ta.peekTopAnchor()?.id==='b'&&ta.peekTopAnchor()?.offsetPx===31&&!ta.recordTopAnchor('c','key',t+11,99));
  ta.commitTopAnchor('c','key',t+20,57);
  checkNew('(t123-4) 尾沿commit不被节流丢弃，记录最终停留偏移',ta.peekTopAnchor()?.id==='c'&&ta.peekTopAnchor()?.offsetPx===57&&!ta.recordTopAnchor('d','key',t+21,0));
  checkNew('(t123-4) commit后的节流窗口结束继续接受用户位置',ta.recordTopAnchor('d','key',t+270,2)&&ta.peekTopAnchor()?.id==='d');
  for(let i=0;i<=ta.RETURN_ANCHOR_ARCHIVE_MAX;i++){ta.rearmTopAnchor(`id${i}`,`fk${i}`,t+1000+i, i);ta.stashTopAnchorForReturn();}
  checkNew('(t115 LRU) 返回锚容量8，最旧淘汰而最新偏移保留',ta.RETURN_ANCHOR_ARCHIVE_MAX===8&&ta.peekReturnAnchor('fk0')==null&&ta.peekReturnAnchor('fk8')?.offsetPx===8);
  ta.peekReturnAnchor('fk1');ta.rearmTopAnchor('next','fk9',t+2000,9);ta.stashTopAnchorForReturn();
  checkNew('(t115 LRU) 命中刷新新鲜度，淘汰真正最旧fk2',ta.peekReturnAnchor('fk1')!=null&&ta.peekReturnAnchor('fk2')==null);

  await setup(rows(1501));await store.getState().loadMoreArticles();await store.getState().loadMoreArticles();ta.recordTopAnchor('3999',key(),t+3000,25);const unchanged=items().map(e=>e.id).join(',');const failureN=store.getState().positionRestoreNonce;
  S.updatesPlan={mode:'reject',error:{message:'属性刷新失败'}};let caught=false;try{await store.getState().reloadFromBackend({keepReadingPosition:true});}catch{caught=true;}finally{S.updatesPlan=null;}
  checkNew('(t123-6) 属性刷新失败有toast且Promise拒绝，不缩窗/不假到底/不发成功回位',caught&&store.getState().toasts.some(toast=>toast.text.startsWith('刷新失败'))&&items().map(e=>e.id).join(',')===unchanged&&store.getState().articlesLimit===1500&&!store.getState().articlesExhausted&&store.getState().positionRestoreNonce===failureN);
  await store.getState().reloadFromBackend();await store.getState().loadMoreArticles();
  checkNew('(t123-5/6) 失败恢复后完整1501中段及最后成员仍可达',items().length===1501&&items().some(e=>e.id==='4100')&&items().at(-1).id==='4500'&&store.getState().articlesExhausted);

  // 未修改的真实几何消费者接线仍保留；上述JS结果不证明真实Edge过程零漂移。
  const timeline=read('../../src/components/Timeline.tsx');
  const restoreAt=timeline.indexOf('anchorRestoreIndex(peekTopAnchor(), filterKey, items)');
  const suppressAt=timeline.indexOf('suppressNextScrollEvents();',restoreAt);
  const scrollAt=timeline.indexOf("rowVirtualizer.scrollToIndex(idx, { align: 'start' });",suppressAt);
  checkNew('(t111-6) 当前消费者仍按锚决策→程序滚动抑制→回位有序接线',restoreAt>=0&&suppressAt>restoreAt&&scrollAt>suppressAt);
  checkNew('(t123-3/4) 当前消费者保留像素偏移测量/补偏与尾沿定稿',timeline.includes('measureAnchorOffsetPx(topIndex)')&&timeline.includes('applyAnchorOffsetPx(')&&timeline.includes('commitTopAnchor('));
  const stash=timeline.indexOf('stashTopAnchorForReturn();');const clear=timeline.indexOf('clearTopAnchor();',stash);
  checkNew('(t115-0) 上下文切换先存档再清活锚，关闭仍走focusCardAt',stash>=0&&clear>stash&&timeline.includes('readerFocusReturnIndex(')&&timeline.includes('focusCardAt('));
  const switchBlock=timeline.slice(timeline.indexOf('const switchRestoreNonce ='),timeline.indexOf('const readerCloseNonce ='));
  const switchDecision=switchBlock.indexOf('anchorRestoreIndex(peekReturnAnchor(filterKey), filterKey, items)');
  const switchSuppress=switchBlock.indexOf('suppressNextScrollEvents();',switchDecision);
  const switchScroll=switchBlock.indexOf("rowVirtualizer.scrollToIndex(idx, { align: 'start' });",switchSuppress);
  const switchRearm=switchBlock.indexOf('rearmTopAnchor(items[idx].id',switchScroll);
  checkNew('(t115-0 consumer) 存档返回消费顺序仍为查档决策→抑制→滚动→含偏移重锚',
    switchDecision>=0&&switchSuppress>switchDecision&&switchScroll>switchSuppress&&switchRearm>switchScroll
    &&switchBlock.includes('peekReturnAnchor(filterKey)?.offsetPx ?? 0'));
  const commitStart=timeline.indexOf('const scheduleAnchorCommit =');
  const commitBlock=timeline.slice(commitStart,timeline.indexOf('/* 筛选上下文变化',commitStart));
  checkNew('(t123-0 consumer) 尾沿计时清旧任务，回调重读实时上下文并阻止旧范围复活锚',
    commitBlock.includes('clearTimeout(anchorCommitTimerRef.current)')&&commitBlock.includes('const live = useAppStore.getState()')
    &&commitBlock.includes('if (liveFilterKey !== scheduledFilterKey) return;')&&commitBlock.includes('}, ANCHOR_RECORD_THROTTLE_MS)')
    &&timeline.includes('scheduleAnchorCommit(filterKey)')&&timeline.includes('recordTopAnchor(topItem.id, filterKey, performance.now(), measureAnchorOffsetPx(topIndex))'));
  const refreshBlock=timeline.slice(timeline.indexOf('const positionRestoreNonce ='),timeline.indexOf('const switchRestoreNonce ='));
  const closeBlock=timeline.slice(timeline.indexOf('const readerCloseNonce ='),timeline.indexOf('/* 滚动到底部附近'));
  checkNew('(t115-0 gallery) 三个单列恢复消费者均保留image提前返回，画廊不误用行索引',
    [refreshBlock,switchBlock,closeBlock].every(block=>block.includes("if (activeContentLayout === 'image') return;")));
  checkNew('(t111-6 entrances) App事件、手动同步和单源刷新仍走保位刷新且App接住拒绝',
    read('../../src/App.tsx').includes('reloadFromBackend({ keepReadingPosition: true }).catch(')
    &&read('../../src/store/slices/feeds.ts').includes('reloadFromBackend({ keepReadingPosition: true })')
    &&read('../../src/store/slices/sync.ts').includes('reloadFromBackend({ keepReadingPosition: true })'));
}
