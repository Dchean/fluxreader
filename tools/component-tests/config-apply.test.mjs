// 便携预览/恢复替代旧ConfigSync快照UI；真实按钮与已有写队列、实体缓存共同验收。
import assert from 'node:assert/strict';
import { componentTest, mount, flush, resetStore, useAppStore, bodyCacheMod } from './harness.mjs';
import { installBackend, seedFromRows, mkRow, setPlan, resetPlan, invokeCount, invokeCalls, deferred, setCurrentRows, FOLDERS, FEEDS } from './fixtures.mjs';
const { act, createElement } = await import('react');
// 本域会改变startupView和当前设置页；结束时恢复进入前状态与IPC计划，
// 不能让紧随的IME/焦点测试隐式继承starred筛选或about页。
const libraryTest = (name, body) => componentTest(name, async (t) => {
  const before = { settings: { ...useAppStore.getState().settings }, settingsTab: useAppStore.getState().settingsTab };
  try { await body(t); }
  finally { resetPlan(); await act(async () => useAppStore.setState(before)); }
});

const { PortableBackupSection } = await import('../../src/components/settings/PortableBackupSection.tsx');
const { api } = await import('../../dist-test/lib/api.js');
const { enqueueAiConfigWrite } = await import('../../dist-test/store.js');
const preview = { digest: 'a'.repeat(64), format: 4, created_at: '2026-11-09T01:00:00Z', folders: 2, sources: 3, articles: 10, starred: 7, protected_bodies: 8, missing_bodies: 2, replaces_current_library: true, disconnects_services: true };
const info = { path: 'D:/backups/reviewed.flxb', size_bytes: 4096, digest: preview.digest, sources: 3, articles: 10, starred: 7, missing_bodies: 2 };
const button = (root, text) => [...root.querySelectorAll('button')].find((item) => item.textContent === text);
const setup = async (extra = {}) => {
  resetPlan(); installBackend(); resetStore(); seedFromRows([mkRow({ id: 101, title: '旧资料库' })]);
  setPlan({ choose_portable_backup_file: () => Promise.resolve(info.path), preview_portable_backup: () => Promise.resolve(preview), ...extra });
  return mount(createElement(PortableBackupSection));
};
const review = async (entry) => { await act(async () => button(entry.container, '选择备份并预览').click()); await flush(3); };
const confirm = async (entry) => { await act(async () => button(entry.container, '确认替换并恢复').click()); await flush(3); };

libraryTest('便携预览：取消选择与取消预览均零写，数量/替换/断开/凭据后果可见', async () => {
  const entry = await setup({ choose_portable_backup_file: () => Promise.resolve(null) });
  await review(entry);
  assert.equal(invokeCount('preview_portable_backup'), 0);
  setPlan({ choose_portable_backup_file: () => Promise.resolve(info.path) });
  await review(entry);
  const card = entry.container.querySelector('[aria-label="便携备份预览"]');
  assert.match(card.textContent, /订阅 3 个.*分类 2 个.*收藏 7 篇/);
  assert.match(card.textContent, /缺少正文 2 篇/);
  assert.match(card.textContent, /替换当前资料库.*RSS 与 WebDAV 均会断开.*凭据需要重新输入/);
  await act(async () => button(card, '取消').click());
  assert.equal(entry.container.querySelector('[aria-label="便携备份预览"]'), null);
  assert.equal(invokeCount('restore_portable_backup'), 0);
  assert.equal(useAppStore.getState().entries[0].title, '旧资料库');
});

libraryTest('便携恢复：四条写前沿均结清才提交，提交后新ID/选择/正文/AI/播放/锚缓存重建', async (t) => {
  const mode = deferred(), setting = deferred(), provider = deferred(), flag = deferred();
  let committed = false;
  const newRows = [mkRow({ id: 90001, feed_id: 90010, title: '恢复后的收藏', is_starred: true })];
  const entry = await setup({
    get_setting: ({ key }) => Promise.resolve(key === 'app_settings' ? JSON.stringify(committed ? { fontSize: 21, startupView: 'starred', syncMode: 'hybrid' } : { fontSize: 16 }) : null),
    set_feed_ai_modes: () => mode.promise,
    patch_app_settings: () => setting.promise,
    set_starred: () => flag.promise,
    restore_portable_backup: ({ path, expectedDigest }) => {
      assert.equal(path, info.path); assert.equal(expectedDigest, preview.digest);
      committed = true; setCurrentRows(newRows); return Promise.resolve(info);
    },
    list_folders: () => Promise.resolve(committed ? [{ ...FOLDERS[0], id: 9001 }] : FOLDERS),
    list_feeds: () => Promise.resolve(committed ? [{ ...FEEDS[0], id: 90010, folder_id: 9001 }] : FEEDS),
    list_article_sources: () => Promise.resolve(committed ? [{ ...FEEDS[0], id: 90010, folder_id: 9001 }] : FEEDS),
  });
  t.after(async () => { mode.resolve(null); setting.resolve('{"fontSize":18}'); provider.resolve(); flag.resolve(null); await flush(4); });
  const oldStamp = bodyCacheMod.markBodyLoading('101');
  bodyCacheMod.applyBodyRow('101', { content_html: '旧正文' }, oldStamp, { aiSummary: '', translatedContent: '' });
  await act(async () => {
    useAppStore.setState({ activeArticleId: '101', rawTranslatedIds: {101:true}, aiStatusById: {101:{}}, player: {...useAppStore.getState().player,isActive:true,isPlaying:true,audioUrl:'old.mp3',positionSec:8}, settings: {...useAppStore.getState().settings,autoStart:true} });
    useAppStore.getState().toggleFeedSummary('cat-1', '10', 'on');
    useAppStore.getState().updateSettings({ fontSize: 18 });
  });
  const providerWrite = enqueueAiConfigWrite(() => provider.promise);
  const flagWrite = api.setStarred(101, true);
  await flush(3); await review(entry); await confirm(entry);
  assert.equal(invokeCount('restore_portable_backup'), 0);
  assert.equal(useAppStore.getState().libraryRestoreBusy, true);
  await act(async () => mode.resolve(null)); await flush(2);
  assert.equal(invokeCount('restore_portable_backup'), 0, '设置/provider/flag仍待结清');
  await act(async () => setting.resolve('{"fontSize":18}')); await flush(2);
  assert.equal(invokeCount('restore_portable_backup'), 0, 'provider/flag仍待结清');
  await act(async () => provider.resolve()); await providerWrite; await flush(2);
  assert.equal(invokeCount('restore_portable_backup'), 0, '最后一条读藏前沿仍挂起');
  let staleWriteRan = false;
  const staleProvider = enqueueAiConfigWrite(async () => { staleWriteRan = true; }).catch((error) => error.message);
  await act(async () => { flag.resolve(null); await flagWrite; }); await flush(12);
  assert.match(await staleProvider, /资料库已恢复/);
  const state = useAppStore.getState();
  assert.equal(staleWriteRan, false);
  assert.equal(state.libraryRestoreBusy, false);
  assert.equal(state.portableBackup.refreshFailed, false, state.portableBackup.error ?? '');
  assert.equal(state.activeArticleId, null);
  assert.equal(state.player.audioUrl, '');
  assert.equal(bodyCacheMod.getBodyEntry('101'), undefined);
  assert.deepEqual(state.rawTranslatedIds, {});
  assert.deepEqual(state.aiStatusById, {});
  assert.deepEqual(state.readingSession.memberIds, ['90001']);
  assert.equal(state.entries[0].title, '恢复后的收藏');
  assert.equal(state.settings.fontSize, 21);
  assert.equal(state.settings.autoStart, true);
  assert.equal(state.settings.syncMode, 'hybrid');
  assert.equal(state.syncConnected, false);
  assert.equal(state.localSyncState.connected, false);
  assert.equal(bodyCacheMod.applyBodyRow('101', { content_html: '迟到旧正文' }, oldStamp, { aiSummary: '', translatedContent: '' }), false);
});

libraryTest('便携恢复：提交后回读失败保已恢复结果，重试只刷新且读取新库', async () => {
  let committed = false, failRead = true;
  const entry = await setup({
    restore_portable_backup: () => { committed = true; setCurrentRows([mkRow({id:90002,title:'新资料仍在'})]); return Promise.resolve(info); },
    get_setting: ({ key }) => committed && failRead && key === 'app_settings' ? Promise.reject({message:'回读中断'}) : Promise.resolve(null),
  });
  await review(entry); await confirm(entry); await flush(8);
  assert.equal(useAppStore.getState().portableBackup.result.kind, 'restore');
  assert.match(entry.container.textContent, /资料库恢复已完成，但界面刷新失败/);
  assert.equal(invokeCount('restore_portable_backup'), 1);
  failRead = false;
  await act(async () => button(entry.container, '重试界面刷新').click()); await flush(8);
  assert.equal(invokeCount('restore_portable_backup'), 1, '刷新重试不重新替换数据');
  assert.equal(useAppStore.getState().portableBackup.refreshFailed, false);
  assert.equal(useAppStore.getState().entries[0].title, '新资料仍在');
});

libraryTest('便携恢复：签名改变拒绝保原，预览消费且不能直接重试写入', async () => {
  const entry = await setup({ restore_portable_backup: () => Promise.reject({code:'staleBackupPreview',message:'文件签名不同'}) });
  await review(entry); await confirm(entry); await flush(5);
  assert.equal(useAppStore.getState().entries[0].title, '旧资料库');
  assert.equal(useAppStore.getState().portableBackup.result, null);
  assert.equal(useAppStore.getState().portableBackup.preview, null);
  assert.match(entry.container.textContent, /重新选择并预览/);
  assert.equal(useAppStore.getState().libraryRestoreBusy, false);
  await act(async () => useAppStore.getState().restorePortableBackup());
  assert.equal(invokeCount('restore_portable_backup'), 1);
});

libraryTest('便携导出：等待已有设置/AI前沿，不挡导出期间之后的用户设置写入', async (t) => {
  const setting = deferred(), exporting = deferred(); let calls = 0;
  const entry = await setup({ patch_app_settings: ({patch}) => ++calls === 1 ? setting.promise : Promise.resolve(JSON.stringify(patch)), export_portable_backup: () => exporting.promise });
  t.after(async()=>{ setting.resolve('{"fontSize":18}'); exporting.resolve(info); await flush(4); });
  await act(async()=>useAppStore.getState().updateSettings({fontSize:18})); await flush(2);
  await act(async()=>button(entry.container,'导出 .flxb 文件').click()); await flush(3);
  assert.equal(invokeCount('export_portable_backup'),0);
  await act(async()=>setting.resolve('{"fontSize":18}')); await flush(3);
  assert.equal(invokeCount('export_portable_backup'),1);
  await act(async()=>useAppStore.getState().updateSettings({lineHeight:200})); await flush(3);
  assert.equal(calls,2,'导出网络/磁盘工作不占设置写门');
  await act(async()=>exporting.resolve(info)); await flush(3);
  assert.match(entry.container.textContent,/便携备份已导出.*缺正文 2 篇/);
  assert.deepEqual(invokeCalls.find(x=>x.cmd==='export_portable_backup').args,{path:info.path});
});

libraryTest('AI写屏障通用回归：A失败、两门之间的写和B按序结算，A不能打开B的门',async(t)=>{
  await setup();
  const {runConfigApplyBarrier,awaitAiWriteFrontier}=await import('../../dist-test/store.js');
  const gateA=deferred(),gateB=deferred(),writeGate=deferred();const order=[];
  setPlan({set_feed_ai_modes:()=>{order.push('write');return writeGate.promise;},set_folder_ai_modes:()=>{order.push('after-B');return Promise.resolve(null);}});
  t.after(async()=>{gateA.resolve();writeGate.resolve(null);gateB.resolve();await flush(4);});
  const a=runConfigApplyBarrier(async()=>{order.push('A');await gateA.promise;}).catch(()=>{order.push('A-failed');});
  await flush(2);
  await act(async()=>useAppStore.getState().toggleFeedSummary('cat-1','10','on'));
  const b=runConfigApplyBarrier(async()=>{order.push('B');await gateB.promise;});
  await act(async()=>useAppStore.getState().toggleCatTranslate('cat-1','on'));
  await flush(2);assert.deepEqual(order,['A']);
  await act(async()=>gateA.reject({message:'A failed'}));await a;await flush(2);
  assert.deepEqual(order,['A','A-failed','write']);
  await act(async()=>writeGate.resolve(null));await flush(2);
  assert.deepEqual(order,['A','A-failed','write','B']);
  assert.equal(invokeCount('set_folder_ai_modes'),0,'A settle未打开B仍持有的gate');
  await act(async()=>gateB.resolve());await b;await awaitAiWriteFrontier();await flush(2);
  assert.deepEqual(order,['A','A-failed','write','B','after-B']);
  assert.equal(useAppStore.getState().feedIndex.get('10').feed.summaryMode,'on');
  assert.equal(useAppStore.getState().feedIndex.get('10').cat.translateMode,'on');
});

libraryTest('恢复单飞与换页：第二确认不重复替换；前沿后迟到被动标读被拒，完成后新ID正常可写',async(t)=>{
  const gate=deferred();t.after(()=>gate.resolve(info));
  const entry=await setup({restore_portable_backup:()=>gate.promise});
  await review(entry);await confirm(entry);
  assert.equal(invokeCount('restore_portable_backup'),1);
  await act(async()=>{useAppStore.getState().markEntriesReadBulk(['101']);useAppStore.getState().restorePortableBackup();});
  assert.equal(invokeCount('set_read_bulk'),0,'已捕获写前沿后的被动scroll不入旧库写队列');
  assert.equal(useAppStore.getState().entries[0].isRead,false,'未制造乐观读态');
  const {unmount}=await import('./harness.mjs');await unmount(entry);
  const next=await mount(createElement(PortableBackupSection));
  assert.equal(button(next.container,'选择备份并预览').disabled,true);
  setCurrentRows([mkRow({id:90003,title:'新库可继续标读'})]);
  await act(async()=>gate.resolve(info));await flush(8);
  assert.equal(invokeCount('restore_portable_backup'),1);
  assert.match(next.container.textContent,/资料库恢复已完成/);
  await act(async()=>useAppStore.getState().markEntriesReadBulk(['90003']));await flush(3);
  assert.equal(invokeCount('set_read_bulk'),1);
  assert.equal(useAppStore.getState().entries[0].isRead,true);
});
