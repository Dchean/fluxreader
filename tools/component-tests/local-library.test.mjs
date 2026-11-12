// WebDAV store/表单与恢复忙态：真实组件挂载，只替换外部IPC数据和时序。
import assert from 'node:assert/strict';
import { componentTest, mount, flush, resetStore, useAppStore } from './harness.mjs';
import { installBackend, seedFromRows, mkRow, setPlan, resetPlan, invokeCount, deferred } from './fixtures.mjs';
const { act, createElement } = await import('react');
// 本域会改变startupView和当前设置页；结束时恢复进入前状态与IPC计划，
// 不能让紧随的IME/焦点测试隐式继承starred筛选或about页。
const libraryTest = (name, body) => componentTest(name, async (t) => {
  const before = { settings: { ...useAppStore.getState().settings }, settingsTab: useAppStore.getState().settingsTab };
  try { await body(t); }
  finally { resetPlan(); await act(async () => useAppStore.setState(before)); }
});

const { LocalSyncSection } = await import('../../src/components/settings/LocalSyncSection.tsx');
const base = { connected:false, endpoint:null, username:null, mode:'scheduled', interval_minutes:15, generation:0, library_id:'local', pending_operations:0, pending_bodies:0, pending_publications:0, last_state_sync:0, last_body_sync:0, next_retry_at:0, last_error:null };
const connected = {...base, connected:true, endpoint:'https://dav.example/a', username:'alice', generation:1};
const button = (root,text)=>[...root.querySelectorAll('button')].find(b=>b.textContent===text);
const setup = () => {resetPlan(); installBackend(); resetStore(); seedFromRows([mkRow({id:101,title:'旧库文章'})]);};
const change = async (input,value)=>act(async()=>{Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype,'value').set.call(input,value);input.dispatchEvent(new window.Event('input',{bubbles:true}));});

libraryTest('WebDAV：已配置连接不冒充同步成功，状态/正文待补分别表达且按秒显示',async()=>{
  setup();setPlan({local_sync_status:()=>Promise.resolve({...connected,pending_bodies:7,pending_operations:2,pending_publications:3,last_state_sync:1000,last_body_sync:0})});
  const entry=await mount(createElement(LocalSyncSection));await flush(3);
  assert.match(entry.container.textContent,/已配置连接/);
  assert.match(entry.container.textContent,/本机待补正文 7 篇/);
  assert.match(entry.container.textContent,/正文同步确认：尚未确认/);
  assert.ok(entry.container.textContent.includes(new Date(1000*1000).toLocaleString()));
  assert.doesNotMatch(entry.container.textContent,/%|同步已完成/);
});

libraryTest('WebDAV：逆序状态响应/连接结果不覆盖新事实，迟到读取不覆盖已编辑草稿',async(t)=>{
  setup();const first=deferred(),second=deferred();let reads=0;
  t.after(()=>{first.resolve(base);second.resolve(connected);});
  setPlan({local_sync_status:()=>++reads===1?first.promise:second.promise});
  const entry=await mount(createElement(LocalSyncSection));await flush(2);
  const endpoint=entry.container.querySelector('[aria-label="WebDAV 地址"]');
  await change(endpoint,'https://draft.example/new');
  let refresh;await act(async()=>{refresh=useAppStore.getState().refreshLocalSyncStatus();});
  await act(async()=>second.resolve({...connected,generation:3}));await refresh;await flush(2);
  await act(async()=>first.resolve({...connected,generation:1}));await flush(2);
  assert.equal(useAppStore.getState().localSyncState.generation,3);
  assert.equal(endpoint.value,'https://draft.example/new');
  setPlan({local_sync_disconnect:()=>Promise.resolve({...base,generation:4})});
  await act(async()=>button(entry.container,'断开 WebDAV').click());await flush(2);
  assert.equal(useAppStore.getState().localSyncState.connected,false);
  assert.equal(endpoint.value,'https://draft.example/new','断开状态不清掉新草稿');
});

libraryTest('WebDAV：RSS互斥/间隔校验/连接失败保草稿可重试，成功清本次密码',async()=>{
  setup();setPlan({local_sync_status:()=>Promise.resolve(base),local_sync_connect:()=>Promise.reject({code:'localSyncNetwork',message:'服务器无响应'})});
  const entry=await mount(createElement(LocalSyncSection));await flush(3);
  await change(entry.container.querySelector('[aria-label="WebDAV 地址"]'),'https://dav.example');
  await change(entry.container.querySelector('[aria-label="WebDAV 用户名"]'),'alice');
  await change(entry.container.querySelector('[aria-label="WebDAV 密码"]'),'secret');
  await change(entry.container.querySelector('[aria-label="WebDAV 同步间隔（分钟）"]'),'0');
  await act(async()=>button(entry.container,'连接 WebDAV').click());await flush(2);
  assert.equal(invokeCount('local_sync_connect'),0);assert.match(entry.container.textContent,/1–1440/);
  await change(entry.container.querySelector('[aria-label="WebDAV 同步间隔（分钟）"]'),'20');
  await act(async()=>useAppStore.setState({syncConnected:true}));
  assert.equal(button(entry.container,'连接 WebDAV').disabled,true);
  await act(async()=>useAppStore.setState({syncConnected:false}));
  await act(async()=>button(entry.container,'连接 WebDAV').click());await flush(3);
  assert.match(entry.container.textContent,/连接失败：服务器无响应/);
  assert.equal(entry.container.querySelector('[aria-label="WebDAV 密码"]').value,'secret');
  setPlan({local_sync_connect:()=>Promise.resolve(connected)});
  await act(async()=>button(entry.container,'连接 WebDAV').click());await flush(3);
  assert.equal(useAppStore.getState().localSyncState.connected,true);
  assert.equal(entry.container.querySelector('[aria-label="WebDAV 密码"]').value,'');
});

libraryTest('普通设置原子patch：未回读的远端别键更新保留，失败后同键较新意图仍生效',async()=>{
  setup();let db={fontSize:16,themeMode:'dark',lineHeight:180};const sent=[];
  setPlan({get_setting:({key})=>Promise.resolve(key==='app_settings'?JSON.stringify(db):null),patch_app_settings:({patch})=>{sent.push(patch);db={...db,...patch};return Promise.resolve(JSON.stringify(db));}});
  await act(async()=>useAppStore.getState().bootstrapSettings());
  db={...db,themeMode:'light'}; // WebDAV先改变别键，但事件/回读尚未到达。
  await act(async()=>useAppStore.getState().updateSettings({fontSize:18}));await flush(3);
  assert.deepEqual(sent,[{fontSize:18}]);
  assert.equal(db.themeMode,'light');assert.equal(useAppStore.getState().settings.themeMode,'light');
  let count=0;setPlan({patch_app_settings:({patch})=>{if(++count===1)return Promise.reject({message:'保存失败'});db={...db,...patch};return Promise.resolve(JSON.stringify(db));}});
  await act(async()=>{useAppStore.getState().updateSettings({fontSize:19});useAppStore.getState().updateSettings({fontSize:20});});await flush(5);
  assert.equal(db.fontSize,20);assert.equal(useAppStore.getState().settings.fontSize,20);
  assert.equal(db.themeMode,'light');
});

libraryTest('恢复忙态：App整体inert、Esc/Ctrl+,/读藏快捷键不穿透，完成后正常关闭设置',async(t)=>{
  setup();const restoreGate=deferred();t.after(()=>restoreGate.resolve({path:'test.flxb',digest:'x',size_bytes:1,sources:0,articles:0,starred:0,missing_bodies:0}));
  setPlan({restore_portable_backup:()=>restoreGate.promise});
  const {default:App}=await import('../../src/App.tsx');const entry=await mount(createElement(App));await flush(8);
  await act(async()=>{useAppStore.setState({settingsOpen:true,settingsTab:'about',activeArticleId:'101',portableBackup:{busy:null,preview:{path:'test.flxb',info:{digest:'x'}},result:null,error:null,refreshFailed:false}});});
  let task;await act(async()=>{task=useAppStore.getState().restorePortableBackup();});await flush(3);
  assert.ok(entry.container.querySelector('[inert] #appRoot'),'主产品树真实inert');
  assert.ok(entry.container.querySelector('[aria-label="正在恢复资料库"]'));
  await act(async()=>{for(const init of [{key:'Escape'},{key:',',ctrlKey:true},{key:'s'},{key:'m'}])document.dispatchEvent(new window.KeyboardEvent('keydown',{...init,bubbles:true,cancelable:true}));});
  assert.equal(useAppStore.getState().settingsOpen,true);
  assert.equal(invokeCount('set_starred'),0);assert.equal(invokeCount('set_read'),0);
  await act(async()=>{restoreGate.resolve({path:'test.flxb',digest:'x',size_bytes:1,sources:0,articles:0,starred:0,missing_bodies:0});await task;});await flush(5);
  assert.equal(useAppStore.getState().libraryRestoreBusy,false);
  await act(async()=>useAppStore.getState().closeSettings());assert.equal(useAppStore.getState().settingsOpen,false);
});

libraryTest('持续同步事件：状态/正文事件只回真值，material变化保固定成员与导航，不因新未读插队',async()=>{
  setup();let db={startupView:'unread'},local={...connected,pending_bodies:2};
  setPlan({get_setting:({key})=>Promise.resolve(key==='app_settings'?JSON.stringify(db):null),local_sync_status:()=>Promise.resolve(local)});
  const {default:App}=await import('../../src/App.tsx');await mount(createElement(App));await flush(8);
  const before=useAppStore.getState().readingSession;
  assert.deepEqual(before.memberIds,['101']);
  const {setCurrentRows}=await import('./fixtures.mjs');
  setCurrentRows([mkRow({id:101,title:'旧库文章',is_read:true}),mkRow({id:102,title:'同步来的新文章'})]);
  db={startupView:'starred',themeMode:'light'};local={...local,last_state_sync:2000};
  await act(async()=>{
    globalThis.__DISPATCH_TAURI_EVENT__('feeds-updated',{source:'local-sync'});
    globalThis.__DISPATCH_TAURI_EVENT__('local-sync-updated',{kind:'state',ok:true});
  });await flush(8);
  const state=useAppStore.getState();
  assert.deepEqual(state.readingSession.memberIds,['101']);
  assert.equal(state.readingSession.generation,before.generation);
  assert.equal(state.activeViewFilter,'unread');
  assert.equal(state.entries[0].isRead,true);
  assert.equal(state.settings.themeMode,'light');
  const membersBefore=invokeCount('list_article_member_ids');
  const readsBefore=invokeCount('get_article_updates');
  local={...local,pending_bodies:0,last_body_sync:2100};
  await act(async()=>globalThis.__DISPATCH_TAURI_EVENT__('local-sync-updated',{kind:'body',ok:true}));await flush(3);
  assert.equal(useAppStore.getState().localSyncState.pending_bodies,0);
  assert.equal(invokeCount('list_article_member_ids'),membersBefore);
  assert.equal(invokeCount('get_article_updates'),readsBefore,'状态事件不冒充material变化重刷当前阅读内容');
});

// SUI-A1：成功恢复后的数据代际也约束Reader提示/重试及嵌套链；body stamp只保护缓存写。
const { bodyCacheMod } = await import('./harness.mjs');
const { setCurrentRows, invokeCalls } = await import('./fixtures.mjs');
const RESTORE_INFO = {path:'D:/tests/reader-generation.flxb',size_bytes:100,digest:'b'.repeat(64),sources:1,articles:1,starred:1,missing_bodies:0};
const oldRow = () => mkRow({id:101,title:'旧资料库正文',url:'https://old.example/101',content_html:'<p>阅读全文</p>'});
const newRow = () => mkRow({id:90001,title:'新资料库正文',url:'https://new.example/90001',content_html:'<p>新正文</p>',is_starred:true});
function readerRestoreSetup(overrides={}) {
  setup();seedFromRows([oldRow(),mkRow({id:102,title:'同库另一篇'})]);
  useAppStore.setState(s=>({activeArticleId:'101',settings:{...s.settings,markReadOnOpen:false,defaultOpenMode:'fulltext'},
    portableBackup:{busy:null,preview:{path:RESTORE_INFO.path,info:{digest:RESTORE_INFO.digest}},result:null,error:null,refreshFailed:false}}));
  setPlan({local_sync_status:()=>Promise.resolve({...base,generation:10,library_id:'new'}),
    restore_portable_backup:()=>{setCurrentRows([newRow()]);return Promise.resolve(RESTORE_INFO);},...overrides});
}
async function commitRestore() {
  await act(async()=>useAppStore.getState().restorePortableBackup());await flush(3);
  assert.equal(useAppStore.getState().portableBackup.refreshFailed,false,useAppStore.getState().portableBackup.error??'');
  assert.equal(useAppStore.getState().libraryGeneration,1);
  assert.deepEqual(useAppStore.getState().entries.map(e=>e.id),['90001']);
}
function seedOldBody() {bodyCacheMod.applyBodyRow('101',oldRow(),bodyCacheMod.markBodyLoading('101'),{aiSummary:'',translatedContent:''});}
const settledFulltext = (outcome) => outcome==='degraded'
  ? {html:'',degraded:true,reason:'旧库降级'} : {html:'<p>旧库全文成功</p>',degraded:false,contentId:99,contentRevision:1};

for(const outcome of ['success','missing','reject']) libraryTest(`SUI-A1详情迟到/${outcome}：成功恢复后不再嵌套提取、写旧正文或登记旧重试`,async(t)=>{
  const gate=deferred();t.after(()=>gate.resolve(null));readerRestoreSetup({get_article:()=>gate.promise});
  await act(async()=>useAppStore.getState().ensureArticleContent('101',{extractFulltext:true,allowDetached:true}));await flush(2);
  assert.equal(invokeCount('get_article'),1);await commitRestore();
  const notices=useAppStore.getState().toasts.map(x=>x.text);
  await act(async()=>outcome==='reject'?gate.reject({message:'旧库详情拒绝'}):gate.resolve(outcome==='missing'?null:oldRow()));await flush(3);
  assert.deepEqual(useAppStore.getState().errors,[]);
  assert.deepEqual(useAppStore.getState().toasts.map(x=>x.text),notices);
  assert.equal(invokeCount('extract_fulltext'),0,'旧详情成功也不能开始后继全文请求');
  assert.equal(bodyCacheMod.getBodyEntry('101'),undefined);
  assert.deepEqual(useAppStore.getState().entries.map(e=>e.id),['90001']);
});

for(const entry of ['manual','automatic']) for(const outcome of ['success','degraded','reject']) libraryTest(`SUI-A1全文迟到/${entry}/${outcome}：恢复后无旧完成提示、降级重试或失败回调`,async(t)=>{
  const gate=deferred();t.after(()=>gate.resolve(null));readerRestoreSetup({get_article:()=>Promise.resolve(oldRow()),extract_fulltext:()=>gate.promise});
  await act(async()=>{if(entry==='manual'){seedOldBody();useAppStore.getState().extractCurrentArticle();}else useAppStore.getState().ensureArticleContent('101',{extractFulltext:true});});await flush(3);
  assert.equal(invokeCount('extract_fulltext'),1);await commitRestore();
  const notices=useAppStore.getState().toasts.map(x=>x.text);
  await act(async()=>outcome==='reject'?gate.reject({message:'旧库全文拒绝'}):gate.resolve(settledFulltext(outcome)));await flush(3);
  assert.deepEqual(useAppStore.getState().errors,[]);
  assert.deepEqual(useAppStore.getState().toasts.map(x=>x.text),notices);
  assert.equal(bodyCacheMod.getBodyEntry('101'),undefined);
  assert.equal(useAppStore.getState().showFulltext,false);
});

for(const outcome of ['success','missing','reject']) libraryTest(`SUI-A1批量正文迟到/${outcome}：旧批次不覆盖新库loading或正文`,async(t)=>{
  const gate=deferred();t.after(()=>gate.resolve([]));readerRestoreSetup({get_articles:()=>gate.promise});
  await act(async()=>useAppStore.getState().hydrateArticleContent(['101']));await flush(2);await commitRestore();
  const stamp=bodyCacheMod.markBodyLoading('90001');
  await act(async()=>outcome==='reject'?gate.reject({message:'旧库批次拒绝'}):gate.resolve(outcome==='missing'?[]:[oldRow()]));await flush(3);
  assert.equal(bodyCacheMod.getBodyEntry('90001').state,'loading');
  assert.equal(bodyCacheMod.getBodyEntry('90001').contentRevision,stamp);
  assert.equal(bodyCacheMod.getBodyEntry('101'),undefined);
  assert.deepEqual(useAppStore.getState().errors,[]);
  assert.equal(bodyCacheMod.applyBodyRow('90001',newRow(),stamp,{aiSummary:'',translatedContent:''}),true);
  assert.equal(bodyCacheMod.getBodyEntry('90001').content,'<p>新正文</p>');
});

for(const entry of ['detail','manual','automatic']) libraryTest(`SUI-A1已捕获重试/${entry}：同代可重试，恢复后旧闭包不能再invoke旧ID`,async()=>{
  readerRestoreSetup({get_article:()=>entry==='detail'?Promise.reject({message:'详情失败'}):Promise.resolve(oldRow()),
    extract_fulltext:()=>Promise.resolve(settledFulltext('degraded'))});
  await act(async()=>{if(entry==='manual'){seedOldBody();useAppStore.getState().extractCurrentArticle();}else useAppStore.getState().ensureArticleContent('101',{extractFulltext:true});});await flush(3);
  const retry=useAppStore.getState().errors.find(e=>e.key===`reader:${entry==='detail'?'content':'fulltext'}:101`).action.run;
  setPlan({get_article:()=>Promise.resolve({...oldRow(),content_html:'<p>完整RSS</p>'}),extract_fulltext:()=>Promise.resolve(settledFulltext('success'))});
  const command=entry==='detail'?'get_article':'extract_fulltext';const before=invokeCount(command);
  await act(async()=>retry());await flush(3);assert.equal(invokeCount(command),before+1,'当前资料库内离屏重试仍有效');
  await commitRestore();const after=invokeCount(command);
  await act(async()=>retry());await flush(3);
  assert.equal(invokeCount(command),after,'资料库替换后即使外部保留旧闭包也不得继续请求旧ID');
  assert.deepEqual(useAppStore.getState().errors,[]);
});

for(const entry of ['detail','fulltext']) libraryTest(`SUI-A1恢复失败/${entry}：未换代的迟到错误和重试保持可见有效`,async(t)=>{
  const gate=deferred();t.after(()=>gate.resolve(null));readerRestoreSetup({get_article:()=>gate.promise,extract_fulltext:()=>gate.promise,
    restore_portable_backup:()=>Promise.reject({code:'staleBackupPreview',message:'未提交恢复'})});
  await act(async()=>{if(entry==='fulltext'){seedOldBody();useAppStore.getState().extractCurrentArticle();}else useAppStore.getState().ensureArticleContent('101',{extractFulltext:true});});await flush(2);
  await act(async()=>useAppStore.getState().restorePortableBackup());assert.equal(useAppStore.getState().libraryGeneration,0);
  await act(async()=>gate.reject({message:'原库请求失败'}));await flush(3);
  const retry=useAppStore.getState().errors.find(e=>e.key===`reader:${entry==='detail'?'content':'fulltext'}:101`);assert.ok(retry,'恢复未提交不能吞掉当前库错误');
  setPlan({get_article:()=>Promise.resolve({...oldRow(),content_html:'<p>完整RSS</p>'}),extract_fulltext:()=>Promise.resolve(settledFulltext('success'))});
  await act(async()=>retry.action.run());await flush(3);
  assert.equal(bodyCacheMod.getBodyEntry('101').state,'ready');assert.equal(useAppStore.getState().activeArticleId,'101');
});

for(const entry of ['detail','fulltext']) libraryTest(`SUI-A1普通切篇/${entry}：同库旧文章失败仍可按原ID重试且不抢当前选择`,async(t)=>{
  const gate=deferred();t.after(()=>gate.resolve(null));readerRestoreSetup({get_article:()=>gate.promise,extract_fulltext:()=>gate.promise});
  await act(async()=>{if(entry==='fulltext'){seedOldBody();useAppStore.getState().extractCurrentArticle();}else useAppStore.getState().ensureArticleContent('101',{extractFulltext:true});});await flush(2);
  await act(async()=>useAppStore.getState().selectArticle('102'));assert.equal(useAppStore.getState().libraryGeneration,0);
  await act(async()=>gate.reject({message:'离屏文章失败'}));await flush(3);
  const error=useAppStore.getState().errors.find(e=>e.key===`reader:${entry==='detail'?'content':'fulltext'}:101`);assert.ok(error);
  setPlan({get_article:()=>Promise.resolve({...oldRow(),content_html:'<p>完整RSS</p>'}),extract_fulltext:()=>Promise.resolve(settledFulltext('success'))});
  await act(async()=>error.action.run());await flush(3);
  const command=entry==='detail'?'get_article':'extract_fulltext';
  assert.equal(invokeCalls.filter(x=>x.cmd===command).at(-1).args.id??invokeCalls.filter(x=>x.cmd===command).at(-1).args.articleId,101);
  assert.equal(useAppStore.getState().activeArticleId,'102');assert.equal(useAppStore.getState().showFulltext,false);
  assert.equal(bodyCacheMod.getBodyEntry('101').state,'ready');
});
