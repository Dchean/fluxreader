import { pathToFileURL } from 'node:url';
globalThis.window={__TAURI_INTERNALS__:{}};
let handler=async()=>null;
globalThis.__INVOKE__=(cmd,args)=>handler(cmd,args);
const base='D:/soft/fluxreader/dist-test/';
const {useAppStore:s}=await import(pathToFileURL(base+'store.js'));
const {articleRowToEntry}=await import(pathToFileURL(base+'lib/api.js'));
const {entryNeedsHydration}=await import(pathToFileURL(base+'store/selectors.js'));
const {viewEntriesCache}=await import(pathToFileURL(base+'store/internals.js'));
const {anchorRestoreIndex}=await import(pathToFileURL('D:/soft/fluxreader/src/components/timelineAnchor.ts'));
const tick=()=>new Promise(r=>setTimeout(r,10));
const row=(id,extra={})=>({id,feed_id:1,title:`row-${id}`,published_at:new Date(Date.UTC(2026,9,7)-id*1000).toISOString(),is_read:false,is_starred:false,content_html:null,snippet:'sample',source:'remote',...extra});
const folder={id:1,name:'article',layout:'article',collapsed:false};
const feed={id:1,folder_id:1,title:'feed',feed_url:'https://example.invalid/feed',layout:'inherit'};
const cat={id:'cat-1',name:'article',layout:'article',feeds:[{id:'1',name:'feed',layout:'inherit'}]};
function reset(rows){viewEntriesCache.clear();s.setState({dataMode:'tauri',activeContentLayout:'article',activeViewFilter:'all',activeFeedFilter:'all',timelineSort:'newest',timelineFilter:'all',categories:[cat],feedIndex:new Map([['1',{cat,feed:cat.feeds[0]}]]),entries:rows.map(articleRowToEntry),hydratedIds:{},hydrationErrors:{},openedReadIds:{},feedCounts:new Map([['1',{total:rows.length,unread:rows.length,starred:0,today:0}]]),articlesLimit:rows.length,articlesCursor:{},articlesExhausted:false,articlesLoading:false,toasts:[],syncWaiting:0,syncFailed:0});}
// 1: mutable unread filter + OFFSET.
let db=Array.from({length:1200},(_,i)=>row(i+1));const calls=[];
handler=async(cmd,args)=>{calls.push({cmd,args}); if(cmd==='list_articles'){let rows=db.filter(r=>!args.args.only_unread||!r.is_read);return rows.slice(args.args.offset,args.args.offset+args.args.limit).map(r=>({...r}));} if(cmd==='set_read_bulk'){for(const r of db)if(args.ids.includes(r.id))r.is_read=args.read;return null;} return null;};
reset([]);s.setState({activeViewFilter:'unread'});await s.getState().reloadFilteredEntries('unread');s.getState().markEntriesReadBulk(s.getState().entries.map(e=>e.id));await tick();await s.getState().loadMoreArticles();
console.log('P1 unread paging',JSON.stringify({loaded:s.getState().entries.length,missingUnread:db.filter(r=>!r.is_read&&!s.getState().entries.some(e=>e.id===String(r.id))).length,exhausted:s.getState().articlesExhausted,lastListArgs:calls.filter(c=>c.cmd==='list_articles').at(-1)?.args}));
// 2: unrelated star write invalidates read rollback.
reset([row(1)]);let rejectAll;
handler=(cmd)=>cmd==='mark_all_read'?new Promise((_,reject)=>rejectAll=reject):Promise.resolve(null);
s.getState().markCurrentViewAllRead();await tick();s.getState().toggleEntryFlag('1','isStarred');await tick();rejectAll({message:'injected all-read failure'});await tick();
console.log('P2 cross-field rollback',JSON.stringify({uiRead:s.getState().entries[0].isRead,uiStar:s.getState().entries[0].isStarred,uiUnread:s.getState().feedCounts.get('1').unread,expectedRead:false,expectedUnread:1}));
// 3: preserve content with no content revision.
reset([row(1)]);s.setState({entries:[{...articleRowToEntry(row(1)),content:'<p>old body</p>',hydrated:true}],hydratedIds:{'1':true}});
let bodyCalls=0;
handler=async(cmd)=>{if(cmd==='list_folders')return [folder];if(cmd==='list_feeds')return [feed];if(cmd==='list_articles')return [row(1,{snippet:'new body from backend'})];if(cmd==='feed_counts')return [];if(cmd==='get_articles'){bodyCalls++;return [row(1,{content_html:'<p>new body</p>'})];}return null;};
await s.getState().reloadFromBackend();
console.log('P3 content freshness',JSON.stringify({content:s.getState().entries[0].content,snippet:s.getState().entries[0].snippet,needsHydration:entryNeedsHydration(s.getState(),'1'),bodyCalls}));
// 4: preserve reading position outside first page.
reset(Array.from({length:1000},(_,i)=>row(i+1)));
handler=async(cmd)=>{if(cmd==='list_folders')return [folder];if(cmd==='list_feeds')return [feed];if(cmd==='list_articles')return Array.from({length:500},(_,i)=>row(i+1));if(cmd==='feed_counts')return [];return null;};
await s.getState().reloadFromBackend({keepReadingPosition:true});
console.log('P4 deep reading refresh',JSON.stringify({loaded:s.getState().entries.length,anchorIndex:anchorRestoreIndex({id:'750',filterKey:'article|all|all|all|newest',at:1},'article|all|all|all|newest',s.getState().entries)}));
// 5: two requests for identical scope resolve out of order.
reset([row(1)]);s.setState({activeViewFilter:'starred'});const pending=[];
handler=(cmd)=>cmd==='list_articles'?new Promise(resolve=>pending.push(resolve)):Promise.resolve(null);
const pOld=s.getState().reloadFilteredEntries('starred');const pNew=s.getState().reloadFilteredEntries('starred');await tick();pending[1]([row(2,{is_starred:true})]);await pNew;pending[0]([row(1,{is_starred:true})]);await pOld;
console.log('P5 same-query response race',JSON.stringify({finalIds:s.getState().entries.map(e=>e.id),expectedIds:['2']}));
// 6: authoritative count captured before newer user action.
reset([row(1)]);let resolveCounts;
handler=(cmd)=>cmd==='mark_all_read'?Promise.resolve(1):cmd==='feed_counts'?new Promise(resolve=>resolveCounts=resolve):Promise.resolve(null);
s.getState().markCurrentViewAllRead();await tick();s.getState().toggleEntryFlag('1','isRead');await tick();resolveCounts([{feed_id:1,total:1,unread:0,starred:0,today:0}]);await tick();
console.log('P6 count response race',JSON.stringify({uiRead:s.getState().entries[0].isRead,uiUnread:s.getState().feedCounts.get('1').unread,expectedUnread:1}));
// 7: explicit AI cache invalidation is overwritten by snapshot inheritance.
reset([row(1)]);s.setState({entries:[{...articleRowToEntry(row(1)),aiSummary:'old summary',translatedContent:'old translation',content:'<p>body</p>'}],hydratedIds:{'1':true}});
handler=async(cmd)=>{if(cmd==='list_folders')return [folder];if(cmd==='list_feeds')return [feed];if(cmd==='list_articles')return [row(1,{ai_summary:null,translated_content:null})];if(cmd==='feed_counts')return [];return null;};
await s.getState().reloadFromBackend();
console.log('P7 after AI-cache clearing reload',JSON.stringify({uiSummary:s.getState().entries[0].aiSummary,uiTranslation:s.getState().entries[0].translatedContent,expectedSummary:''}));
// 8: ordinary local state mutation never refreshes queue statistics.
reset([row(1)]);s.setState({syncConnected:true,syncStatus:'synced',backgroundSyncing:false});let actualQueue=0;let statsRequests=0;
handler=async(cmd)=>{if(cmd==='set_read'){actualQueue++;return null;}if(cmd==='sync_queue_stats'){statsRequests++;return {waiting:actualQueue,failed:0,last_error:null};}return null;};
s.getState().toggleEntryFlag('1','isRead');await tick();
const {syncPillLabel}=await import(pathToFileURL('D:/soft/fluxreader/src/lib/syncPill.ts'));
console.log('P8 queue UI freshness',JSON.stringify({actualQueue,statsRequests,storeWaiting:s.getState().syncWaiting,label:syncPillLabel({syncStatus:s.getState().syncStatus,backgroundSyncing:false,syncConnected:true,waiting:s.getState().syncWaiting,failed:s.getState().syncFailed})}));
process.exit(0);
