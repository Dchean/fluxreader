// 独审三反例原操作/断言移入正式入口；只调整模块路径与共用harness生命周期。
import assert from 'node:assert/strict';

import {componentTest as registerComponentTest, resetStore, useAppStore, storeMod, mount, flush, waitFor} from './harness.mjs';
import {installBackend, resetPlan, setPlan, setCurrentRows, mkRow, invokeCalls, clearInvokeCalls, deferred} from './fixtures.mjs';
const {viewEntriesCache, entryFlagHistorySize, localFlagWritesPending} = await import('../../dist-test/store/internals.js');
const {createElement, act} = await import('react');
const {Timeline} = await import('../../src/components/Timeline.tsx');
const state = () => useAppStore.getState();
const members = () => storeMod.selectVisibleEntries(state()).map(x=>x.id);
const componentTest = (name, options, run) => registerComponentTest(name, options, async () => { try { await run(); } finally { resetPlan(); viewEntriesCache.clear(); } });

const rowsOf = count => Array.from({length:count},(_,i)=>mkRow({id:i+1,published_at:new Date(Date.UTC(2026,10,8,12,0,i)).toISOString()}));
async function prepare(rows) {
  viewEntriesCache.clear(); installBackend(); resetPlan();
  resetStore({dataMode:'tauri',activeViewFilter:'all',timelineFilter:'all'});
  useAppStore.setState({settings:{...state().settings,markReadOnOpen:false}});
  setCurrentRows(rows);
  setPlan({counts_snapshot:()=>({feeds:[{feed_id:10,total:rows.length,unread:rows.filter(row=>!row.is_read).length,starred:0,today:0}],scope:null})});
  await state().reloadFromBackend();
}
function commitRead(rows, ids, read) {
  const changed=[];
  for(const row of rows) if(ids.includes(row.id)) {if(row.is_read!==read)changed.push({id:row.id,feed_id:row.feed_id});row.is_read=read;}
  return changed;
}

componentTest('AUDIT detached search entity stays unread when current captured session is marked read', {timeout:15000}, async()=> {
  viewEntriesCache.clear(); installBackend(); resetPlan();
  resetStore({dataMode:'tauri',activeViewFilter:'all',timelineFilter:'all'});
  useAppStore.setState({settings:{...state().settings,markReadOnOpen:false}});
  const rows=[mkRow({id:101,title:'captured member',content_html:'<p>body</p>'})];
  setCurrentRows(rows);
  setPlan({set_read_bulk:({ids,read})=> {
    const changed=[];
    for(const row of rows) if(ids.includes(row.id)) {row.is_read=read;changed.push({id:row.id,feed_id:row.feed_id});}
    return changed;
  }});
  await state().reloadFromBackend();
  assert.deepEqual(members(),['101']);
  rows.push(mkRow({id:999,title:'new after session, opened from search',content_html:'<p>detached body</p>'}));
  await state().anchorToArticle('999');
  assert.deepEqual(members(),['101']);
  assert.equal(state().entries.find(x=>x.id==='999').isRead,false);
  const mounted=await mount(createElement(Timeline));
  clearInvokeCalls();
  await act(async()=>{mounted.container.querySelector('button[title="将当前列表全部标为已读"]').click();});
  await flush(3);
  const writes=invokeCalls.filter(x=>x.cmd==='set_read_bulk');
  console.log('AUDIT_NONMEMBER',JSON.stringify({members:state().readingSession.memberIds,writes,local:state().entries.map(x=>({id:x.id,isRead:x.isRead})),backend:rows.map(x=>({id:x.id,isRead:x.is_read}))}));
  assert.deepEqual(writes.map(x=>x.args.ids),[[101]]);
  assert.equal(rows.find(x=>x.id===999).is_read,false);
  assert.equal(state().entries.find(x=>x.id==='999').isRead,false,'a nonmember absent from the actual bulk write must stay unread in the Reader/store');
});

componentTest('AUDIT full-session mark read protects an in-flight unresident tail snapshot', {timeout:15000}, async()=> {
  viewEntriesCache.clear(); installBackend(); resetPlan();
  resetStore({dataMode:'tauri',activeViewFilter:'all',timelineFilter:'all'});
  useAppStore.setState({settings:{...state().settings,markReadOnOpen:false}});
  const rows=Array.from({length:501},(_,i)=>mkRow({id:i+1,published_at:new Date(Date.UTC(2026,10,8,12,0,i)).toISOString()}));
  setCurrentRows(rows);
  await state().reloadFromBackend();
  assert.equal(members().length,500);
  const tail=deferred();let requested=false;
  setPlan({get_article_updates:({ids})=>{assert.deepEqual(ids,[1]);requested=true;return tail.promise;},set_read_bulk:({ids,read})=> {
    const changed=[];
    for(const row of rows) if(ids.includes(row.id)) {row.is_read=read;changed.push({id:row.id,feed_id:row.feed_id});}
    return changed;
  }});
  const oldTail={...rows[0]};
  const page=state().loadMoreArticles();await waitFor(()=>requested);
  try {
    state().markCurrentViewAllRead();await flush(3);
    assert.ok(rows.every(x=>x.is_read));
  } finally {tail.resolve({items:[oldTail],missing_ids:[]});await page;await flush(2);}
  console.log('AUDIT_TAIL',JSON.stringify({captured:state().readingSession.memberIds.length,loaded:members().length,localTail:state().entries.find(x=>x.id==='1')?.isRead,backendTail:rows[0].is_read}));
  assert.equal(state().entries.find(x=>x.id==='1').isRead,true,'the older tail response must not undo the confirmed full-session read intent');
});

componentTest('AUDIT deep return keeps a surviving real anchor if only window-start member vanished', {timeout:15000}, async()=> {
  viewEntriesCache.clear(); installBackend(); resetPlan();
  resetStore({dataMode:'tauri',activeViewFilter:'all',timelineFilter:'all'});
  useAppStore.setState({settings:{...state().settings,markReadOnOpen:false}});
  let rows=Array.from({length:1600},(_,i)=>mkRow({id:i+1,is_starred:true,published_at:new Date(Date.UTC(2026,10,8,12,0,i)).toISOString()}));
  setCurrentRows(rows);await state().reloadFromBackend();await state().anchorToArticle('50');
  assert.equal(state().readingSession.windowStart,1550);
  const anchors=await import('../../src/lib/readingPosition.ts');
  anchors.clearTopAnchor();
  const proto=window.Element.prototype;
  const original=Object.getOwnPropertyDescriptor(proto,'scrollHeight');
  Object.defineProperty(proto,'scrollHeight',{configurable:true,get(){return this.id==='timelineContentScroll'?members().length*160:(original?.get?.call(this)??0);}});
  let mounted;
  try {mounted=await mount(createElement(Timeline));}
  finally {if(original)Object.defineProperty(proto,'scrollHeight',original);else delete proto.scrollHeight;}
  const scroller=mounted.container.querySelector('#timelineContentScroll');
  Object.defineProperty(scroller,'scrollHeight',{configurable:true,get:()=>Number.parseFloat(scroller.querySelector('.timeline-virtual-wrap')?.style.height??'0')});
  Object.defineProperty(scroller,'clientHeight',{configurable:true,get:()=>768});
  const key='article|all|all|all|newest';
  anchors.rearmTopAnchor('40',key,performance.now(),17);
  scroller.scrollTop=10*160+17;
  await act(async()=>{state().selectFeed('12');});
  await waitFor(()=>state().readingSession?.ready&&state().readingSession.query.scope==='12');
  assert.equal(anchors.peekReturnAnchor(key)?.id,'40');
  rows=rows.filter(x=>x.id!==50);setCurrentRows(rows);clearInvokeCalls();
  await act(async()=>{state().selectFeed('all');});
  await waitFor(()=>state().readingSession?.ready&&state().readingSession.query.scope==='all');
  await flush(2);
  console.log('AUDIT_RETURN',JSON.stringify({memberIncludes40:state().readingSession.memberIds.includes('40'),visibleIncludes40:members().includes('40'),windowStart:state().readingSession.windowStart,loadedCount:state().readingSession.loadedCount,first:members()[0],archived:anchors.peekReturnAnchor(key),scrollTop:scroller.scrollTop,fetchSizes:invokeCalls.filter(x=>x.cmd==='get_article_updates').map(x=>x.args.ids.length)}));
  assert.ok(state().readingSession.memberIds.includes('40'));
  assert.ok(members().includes('40'),'existing saved anchor must remain in restored deep window; deleting another boundary ID does not delete the real anchor');
  assert.equal(scroller.scrollTop,members().indexOf('40')*160+17);
});

for(const pageTiming of ['before-write','during-write']) for(const fail of [false,true]) {
  componentTest(`RS2 未驻留尾页${pageTiming}发起，全读${fail?'失败':'成功'}后仍按真实终态收敛`, {timeout:15000}, async()=>{
    const rows=rowsOf(501);await prepare(rows);
    const pageReply=deferred(),writeReply=deferred();let pageStarted=false,writeStarted=false;
    const oldTail={...rows[0]};
    setPlan({get_article_updates:({ids})=>{assert.deepEqual(ids,[1]);pageStarted=true;return pageReply.promise;},
      set_read_bulk:()=>{writeStarted=true;return writeReply.promise;}});
    let page;
    if(pageTiming==='before-write'){page=state().loadMoreArticles();await waitFor(()=>pageStarted);}
    state().markCurrentViewAllRead();await waitFor(()=>writeStarted);
    if(pageTiming==='during-write'){page=state().loadMoreArticles();await waitFor(()=>pageStarted);}
    try {
      // 属性先到、写仍在途：缺旧实体也必须显示pending意图；失败后须可恢复真实旧值。
      pageReply.resolve({items:[oldTail],missing_ids:[]});await page;await flush(2);
      assert.equal(state().entries.find(row=>row.id==='1').isRead,true);
    } finally {
      if(fail)writeReply.reject({message:'受控事务失败'});
      else writeReply.resolve(commitRead(rows,rows.map(row=>row.id),true));
      await flush(3);
    }
    assert.equal(state().entries.find(row=>row.id==='1').isRead,!fail);
    assert.equal(rows[0].is_read,!fail);
    assert.equal(state().feedCounts.get('10').unread,fail?501:0);
    assert.equal(localFlagWritesPending(),0);
    // 更晚新鲜读取仍能接受外部值，shadow不能把本地值永久变成权威。
    rows[0].is_read=false; resetPlan();setCurrentRows(rows);await state().reloadFromBackend();
    assert.equal(state().entries.find(row=>row.id==='1').isRead,false);
  });
}

componentTest('RS2 全读失败先于未驻留旧页到达，未知基线撤销后接受真实未读', {timeout:15000}, async()=>{
  const rows=rowsOf(501);await prepare(rows);
  const pageReply=deferred();let pageStarted=false;
  setPlan({get_article_updates:()=>{pageStarted=true;return pageReply.promise;},set_read_bulk:()=>Promise.reject({message:'未提交失败'})});
  const page=state().loadMoreArticles();await waitFor(()=>pageStarted);
  try {state().markCurrentViewAllRead();await flush(3);}
  finally {pageReply.resolve({items:[{...rows[0]}],missing_ids:[]});await page;}
  assert.equal(state().entries.find(row=>row.id==='1').isRead,false);
  assert.equal(state().feedCounts.get('10').unread,501);
  assert.equal(localFlagWritesPending(),0);
});

componentTest('RS2 尾页在pending期间首次出现后全读失败，即使对账不可用也不多回补一个未读', {timeout:15000}, async()=>{
  const rows=rowsOf(501);await prepare(rows);
  const tail=deferred(),bulk=deferred();let started=false;
  setPlan({get_article_updates:()=>tail.promise,set_read_bulk:()=>{started=true;return bulk.promise;}});
  const page=state().loadMoreArticles();state().markCurrentViewAllRead();await waitFor(()=>started);
  tail.resolve({items:[{...rows[0]}],missing_ids:[]});await page;
  assert.equal(state().entries.find(row=>row.id==='1').isRead,true);
  setPlan({counts_snapshot:()=>Promise.reject({message:'计数暂不可用'})});
  bulk.reject({message:'事务未提交'});await flush(3);
  assert.ok(state().entries.every(row=>!row.isRead));
  assert.equal(state().feedCounts.get('10').unread,501,'首次出现的尾行乐观差值/回滚只能各计一次');
});

for(const bulkFails of [false,true]) for(const unreadFails of [false,true]) {
  componentTest(`RS2 尾页pending读后用户改未读，bulk失败=${bulkFails}/unread失败=${unreadFails}遵守末成功意图`, {timeout:15000}, async()=>{
    const rows=rowsOf(501);await prepare(rows);
    const pageReply=deferred(),bulkReply=deferred(),unreadReply=deferred();let requested=false,bulkStarted=false,unreadStarted=false;
    setPlan({get_article_updates:()=>{requested=true;return pageReply.promise;},set_read_bulk:()=>{bulkStarted=true;return bulkReply.promise;},set_read:()=>{unreadStarted=true;return unreadReply.promise;}});
    const page=state().loadMoreArticles();await waitFor(()=>requested);
    state().markCurrentViewAllRead();await waitFor(()=>bulkStarted);
    pageReply.resolve({items:[{...rows[0]}],missing_ids:[]});await page;
    assert.equal(state().entries.find(row=>row.id==='1').isRead,true);
    state().toggleEntryFlag('1','isRead');await flush(1);
    assert.equal(state().entries.find(row=>row.id==='1').isRead,false);
    assert.equal(unreadStarted,false,'同字段后写仍排在bulk后');
    if(bulkFails)bulkReply.reject({message:'bulk事务失败'});else bulkReply.resolve(commitRead(rows,rows.map(row=>row.id),true));
    await waitFor(()=>unreadStarted);
    if(unreadFails)unreadReply.reject({message:'unread事务失败'});else unreadReply.resolve(commitRead(rows,[1],false));
    await flush(4);
    const expected=!bulkFails&&unreadFails;
    assert.equal(state().entries.find(row=>row.id==='1').isRead,expected);
    assert.equal(rows[0].is_read,expected);
    assert.equal(state().feedCounts.get('10').unread,rows.filter(row=>!row.is_read).length);
    assert.equal(localFlagWritesPending(),0);
  });
}

componentTest('RS1 真实同源兄弟receipt可更新搜索实体，未实际写的同源/跨源实体不跟随且不按回执数扣计数', {timeout:15000}, async()=>{
  const rows=[mkRow({id:101})];await prepare(rows);
  rows.push(mkRow({id:999}),mkRow({id:777}),mkRow({id:888,feed_id:11}));
  await state().anchorToArticle('999');await state().anchorToArticle('777');await state().anchorToArticle('888');
  useAppStore.setState({feedCounts:new Map([['10',{total:2,unread:2,starred:0,today:0}],['11',{total:1,unread:1,starred:0,today:0}]])});
  const counts=deferred();
  setPlan({set_read_bulk:({ids})=>{assert.deepEqual(ids,[101]);return commitRead(rows,[101,999],true);},counts_snapshot:()=>counts.promise});
  try {
    state().markCurrentViewAllRead();await flush(3);
    assert.equal(state().entries.find(row=>row.id==='999').isRead,true,'真实组回执必须闭合实体');
    for(const id of ['777','888'])assert.equal(state().entries.find(row=>row.id===id).isRead,false);
    assert.equal(state().feedCounts.get('10').unread,1,'两个身份仅一个组，不重复扣');
  } finally {counts.resolve({feeds:[{feed_id:10,total:2,unread:1,starred:0,today:0},{feed_id:11,total:1,unread:1,starred:0,today:0}],scope:null});await flush(2);}
});

componentTest('RS2 已确认全读后较新unread不被先发属性刷新翻回', {timeout:15000}, async()=>{
  const rows=rowsOf(501);await prepare(rows);
  setPlan({set_read_bulk:({ids,read})=>commitRead(rows,ids,read),set_read:({id,read})=>commitRead(rows,[id],read)});
  state().markCurrentViewAllRead();await flush(3);await state().loadMoreArticles();
  const oldRows=rows.map(row=>({...row}));const oldRead=deferred();let started=false;
  setPlan({get_article_updates:()=>{started=true;return oldRead.promise;}});
  const refresh=state().reloadFromBackend();await waitFor(()=>started);
  state().toggleEntryFlag('1','isRead');await flush(2);
  oldRead.resolve({items:oldRows,missing_ids:[]});await refresh;
  assert.equal(state().entries.find(row=>row.id==='1').isRead,false);
  assert.equal(rows[0].is_read,false);
});

componentTest('RS2 完整成员影子仅随有效成员/缓存/在途读取存活，释放后回收', {timeout:20000}, async()=>{
  const rows=rowsOf(2500);await prepare(rows);
  const oldPage=deferred();let requested=false;
  setPlan({get_article_updates:()=>{requested=true;return oldPage.promise;},set_read_bulk:({ids,read})=>commitRead(rows,ids,read)});
  const page=state().loadMoreArticles();await waitFor(()=>requested);
  state().markCurrentViewAllRead();await flush(3);
  assert.equal(entryFlagHistorySize(),2500);
  resetPlan();setCurrentRows([]);state().selectFeed('12');await waitFor(()=>state().readingSession?.ready);
  viewEntriesCache.clear();await flush(2);
  assert.equal(entryFlagHistorySize(),2500,'旧页尚在途时必须留前沿');
  oldPage.resolve({items:[],missing_ids:Array.from({length:500},(_,i)=>2000-i)});await page;await flush(2);
  assert.equal(entryFlagHistorySize(),0,'过时读取释放后，离开的未驻留成员不会永久留影子');
  assert.equal(localFlagWritesPending(),0);
});

componentTest('RS2 原600成员/1驻留/500缓存形态仍投影缓存并最终精确对账', {timeout:15000}, async()=>{
  const rows=rowsOf(600);await prepare(rows);
  useAppStore.setState(state=>({entries:state.entries.slice(0,1)}));
  assert.equal(viewEntriesCache.get('article|all|all').entries.length,500,'保留原缓存前提，不通过清缓存避开保护');
  const counts=deferred();
  setPlan({set_read_bulk:({ids,read})=>commitRead(rows,ids,read),counts_snapshot:()=>counts.promise});
  try {
    state().markCurrentViewAllRead();await flush(3);
    assert.ok(viewEntriesCache.get('article|all|all').entries.every(row=>row.isRead));
    assert.ok(rows.every(row=>row.is_read));
    assert.equal(state().feedCounts.get('10').unread,100,'500个有属性的独立身份都已投影，未重复按成员600扣');
  } finally {counts.resolve({feeds:[{feed_id:10,total:600,unread:0,starred:0,today:0}],scope:null});await flush(2);}
  assert.equal(state().feedCounts.get('10').unread,0);
});

componentTest('RS1 组兄弟未驻留且旧搜索属性先发，真实receipt仍保护随后首次出现的实体', {timeout:15000}, async()=>{
  const rows=[mkRow({id:101})];await prepare(rows);
  rows.push(mkRow({id:999}));const old={...rows[1]},detail=deferred();let requested=false;
  setPlan({get_article_updates:({ids})=>{assert.deepEqual(ids,[999]);requested=true;return detail.promise;},set_read_bulk:()=>commitRead(rows,[101,999],true)});
  const search=state().anchorToArticle('999');await waitFor(()=>requested);
  state().markCurrentViewAllRead();await flush(3);
  detail.resolve({items:[old],missing_ids:[]});await search;
  assert.equal(state().entries.find(row=>row.id==='999').isRead,true);
  assert.deepEqual(members(),['101']);
});

componentTest('RS1 较新unread接管组兄弟后，迟到bulk receipt不覆盖该字段', {timeout:15000}, async()=>{
  const rows=[mkRow({id:101})];await prepare(rows);rows.push(mkRow({id:999}));await state().anchorToArticle('999');
  const bulk=deferred();let started=false;
  setPlan({set_read_bulk:()=>{started=true;return bulk.promise;},set_read:({id,read})=>commitRead(rows,[id],read)});
  state().markCurrentViewAllRead();await waitFor(()=>started);
  const receipt=commitRead(rows,[101,999],true); // 服务器已提交，唯回应延迟；后续真实意图排在其后。
  state().toggleEntryFlag('999','isRead');state().toggleEntryFlag('999','isRead');
  assert.equal(state().entries.find(row=>row.id==='999').isRead,false);
  bulk.resolve(receipt);await flush(5);
  assert.equal(state().entries.find(row=>row.id==='999').isRead,false);
  assert.equal(rows[1].is_read,false);
});

componentTest('RS2 跨会话重叠读取与全读结算，离屏未驻留成员返回仍拒绝旧属性', {timeout:15000}, async()=>{
  const rows=rowsOf(501);await prepare(rows);
  const bulk=deferred();let writeStarted=false;
  setPlan({set_read_bulk:()=>{writeStarted=true;return bulk.promise;}});
  state().markCurrentViewAllRead();await waitFor(()=>writeStarted);
  state().selectFeed('12');await waitFor(()=>state().readingSession?.ready&&state().readingSession.query.scope==='12');
  const old=rows.map(row=>({...row})),back=deferred();let readStarted=false;
  setPlan({get_article_updates:({ids})=>{readStarted=true;return back.promise.then(()=>({items:old.filter(row=>ids.includes(row.id)),missing_ids:[]}));}});
  state().selectFeed('all');await waitFor(()=>readStarted);
  bulk.resolve(commitRead(rows,rows.map(row=>row.id),true));await flush(3);
  back.resolve();await waitFor(()=>state().readingSession?.ready);await flush(2);
  assert.ok(state().entries.every(row=>row.isRead));
  resetPlan();setCurrentRows(rows);await state().loadMoreArticles();
  assert.equal(state().entries.find(row=>row.id==='1').isRead,true);
  assert.equal(localFlagWritesPending(),0);
});

for(const deleted of [50,40]) {
  componentTest(`RS3 深窗口返回删除${deleted}，真实锚${deleted===40?'缺失保持null':'仍在则恢复'}且不读前方1500行`, {timeout:15000}, async()=>{
    let rows=rowsOf(1600);await prepare(rows);await state().anchorToArticle('50');
    const anchors=await import('../../src/lib/readingPosition.ts');const key='article|all|all|all|newest';
    anchors.clearTopAnchor();
    const proto=window.Element.prototype;const descriptor=Object.getOwnPropertyDescriptor(proto,'scrollHeight');
    Object.defineProperty(proto,'scrollHeight',{configurable:true,get(){return this.id==='timelineContentScroll'?members().length*160:(descriptor?.get?.call(this)??0);}});
    let mounted;try{mounted=await mount(createElement(Timeline));}finally{if(descriptor)Object.defineProperty(proto,'scrollHeight',descriptor);else delete proto.scrollHeight;}
    const scroller=mounted.container.querySelector('#timelineContentScroll');
    Object.defineProperty(scroller,'scrollHeight',{configurable:true,get:()=>Number.parseFloat(scroller.querySelector('.timeline-virtual-wrap')?.style.height??'0')});
    Object.defineProperty(scroller,'clientHeight',{configurable:true,get:()=>768});
    anchors.rearmTopAnchor('40',key,performance.now(),17);
    await act(async()=>{state().selectFeed('12');});await waitFor(()=>state().readingSession?.ready);
    rows=rows.filter(row=>row.id!==deleted);setCurrentRows(rows);clearInvokeCalls();
    await act(async()=>{state().selectFeed('all');});await waitFor(()=>state().readingSession?.ready);await flush(2);
    const restored=anchors.anchorRestoreIndex(anchors.peekReturnAnchor(key),key,state().entries);
    if(deleted===40){assert.equal(restored,null);assert.equal(scroller.scrollTop,0);}
    else {assert.notEqual(restored,null);assert.equal(scroller.scrollTop,restored*160+17);}
    const fetchIds=invokeCalls.filter(call=>call.cmd==='get_article_updates').flatMap(call=>call.args.ids);
    assert.ok(fetchIds.length<=50&&fetchIds.every(id=>id<=51),'只回深窗口邻域');
    anchors.clearTopAnchor();
  });
}
