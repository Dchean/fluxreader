// 生产 store / selector / Timeline；只在 IPC 边界控制后端行、顺序和完成时机。
import assert from 'node:assert/strict';
import { componentTest as registerComponentTest, resetStore, useAppStore, storeMod, mount, flush, waitFor } from './harness.mjs';
import { installBackend, resetPlan, setPlan, setCurrentRows, mkRow, invokeCalls, clearInvokeCalls, deferred, FEEDS } from './fixtures.mjs';

const ids = () => storeMod.selectVisibleEntries(useAppStore.getState()).map((entry) => entry.id);
const state = () => useAppStore.getState();
const { viewEntriesCache } = await import('../../dist-test/store/internals.js');
const componentTest = (name, opts, run) => registerComponentTest(name, opts, async () => {
  try { await run(); } finally { resetPlan(); viewEntriesCache.clear(); }
});
const rowsOf = (count) => Array.from({ length: count }, (_, i) => mkRow({
  id: i + 1, title: `成员 ${i + 1}`, is_starred: true,
  published_at: new Date(Date.UTC(2026, 10, 8, 12, 0, i)).toISOString(),
}));
function start(rows, view = 'unread') {
  viewEntriesCache.clear();
  installBackend(); resetPlan(); resetStore({ dataMode: 'tauri', timelineFilter: 'all', activeViewFilter: view, articlesExhausted: false });
  setCurrentRows(rows);
}

for (const [view, field, backendField, command] of [
  ['unread', 'isRead', 'is_read', 'set_read'],
  ['starred', 'isStarred', 'is_starred', 'set_starred'],
]) {
  componentTest(`ReadingSession ${view}：已加载与未加载成员改变属性后仍可按原序到达，显式重筛才更新集合`, { timeout: 20000 }, async () => {
    const rows = rowsOf(502);
    start(rows, view);
    setPlan({ [command]: ({ id, read, starred }) => {
      const row = rows.find((item) => item.id === id);
      row[backendField] = command === 'set_read' ? read : starred;
      return command === 'set_read' ? [{ id, feed_id: row.feed_id }] : null;
    } });
    await state().reloadFromBackend();
    const first = ids();
    assert.equal(first.length, 500);
    assert.equal(state().readingSession.memberIds.length, 502, '首屏已捕获全部轻量成员');
    assert.equal(first[0], '502');
    state().toggleEntryFlag('502', field);
    await flush(2);
    rows[0][backendField] = view === 'unread';
    rows[1][backendField] = view === 'unread';
    rows[499].published_at = '2030-01-01T00:00:00Z';
    rows.push(mkRow({ id: 9000, is_starred: true, published_at: '2031-01-01T00:00:00Z' }));
    await state().reloadFromBackend({ keepReadingPosition: true });
    assert.deepEqual(ids(), first, '读藏/发布时间变化和后台插入都不替换会话成员');
    await state().loadMoreArticles();
    assert.deepEqual(ids(), [...first, '2', '1'], '即使尾部已读/取消收藏，ID续页仍能到达');
    assert.equal(state().articlesExhausted, true);
    const generation = state().readingSession.generation;
    state().selectView(view);
    await waitFor(() => state().readingSession?.ready && state().readingSession.generation !== generation);
    assert.ok(ids().includes('9000'));
    assert.ok(!ids().includes('502'));
    assert.ok(!state().readingSession.memberIds.includes('1'));
  });
}

componentTest('ReadingSession A→B→A：旧首屏与旧属性回读均不能覆盖新实例', { timeout: 20000 }, async () => {
  start(rowsOf(3), 'all');
  const pending = [];
  setPlan({ list_article_member_ids: ({ args }) => {
    const task = deferred(); pending.push({ ...task, args }); return task.promise;
  } });
  const a1 = state().reloadFromBackend();
  state().selectView('starred');
  state().selectView('all');
  try {
    await waitFor(() => pending.length === 3);
    pending[2].resolve([3, 2]);
    await waitFor(() => state().readingSession?.ready);
    assert.deepEqual(ids(), ['3', '2']);
  } finally {
    for (const [index, task] of pending.entries()) task.resolve([index + 1]);
    await a1; await flush(3);
  }
  assert.deepEqual(ids(), ['3', '2']);
  assert.equal(state().backendReloadsInFlight, 0);

  const old = deferred();
  let readStarted = false;
  setPlan({ get_article_updates: () => { readStarted = true; return old.promise; } });
  const refresh = state().reloadFromBackend();
  await waitFor(() => readStarted);
  resetPlan();
  state().selectView('starred');
  await waitFor(() => state().readingSession?.ready && state().readingSession.query.view === 'starred');
  old.resolve({ items: [mkRow({ id: 3, title: '旧属性' })], missing_ids: [2] });
  await refresh;
  assert.equal(state().entries.find((entry) => entry.id === '3').title, '成员 3');
});

componentTest('ReadingSession 迟到续页与加载失败重试：按实例/页进度拒收，失败不消耗成员', { timeout: 20000 }, async () => {
  const rows = rowsOf(501); start(rows);
  await state().reloadFromBackend();
  const pending = deferred();
  let pageStarted = false;
  setPlan({ get_article_updates: ({ ids: requested }) => {
    if (requested.length === 1) { pageStarted = true; return pending.promise; }
    return { items: rows.filter((row) => requested.includes(row.id)), missing_ids: [] };
  } });
  const page = state().loadMoreArticles();
  await waitFor(() => pageStarted);
  state().selectView('starred');
  await waitFor(() => state().readingSession?.ready && state().readingSession.query.view === 'starred');
  pending.resolve({ items: [rows[0]], missing_ids: [] });
  await page;
  assert.equal(ids().length, 500, '旧实例尾页不能追加到新实例');
  setPlan({ get_article_updates: () => Promise.reject(new Error('受控失败')) });
  await state().loadMoreArticles();
  assert.equal(state().readingSession.loadedCount, 500);
  assert.equal(state().articlesExhausted, false);
  const retry = state().errors.find((error) => error.key.startsWith('list:load-more:'));
  assert.ok(retry);
  resetPlan(); retry.action.run();
  await waitFor(() => state().articlesExhausted);
  assert.equal(ids().at(-1), '1');
});

componentTest('ReadingSession 缺行明确占位、不缩窗、不无限请求；普通残缺响应是失败', { timeout: 20000 }, async () => {
  const rows = rowsOf(3); start(rows, 'all');
  await state().reloadFromBackend();
  setCurrentRows(rows.filter((row) => row.id !== 2));
  await state().reloadFromBackend();
  assert.deepEqual(ids(), ['3', '2', '1']);
  assert.equal(state().entries.find((entry) => entry.id === '2').unavailable, true);
  assert.equal(storeMod.entryNeedsHydration(state(), '2'), false);
  const { createElement, act } = await import('react');
  const { Timeline } = await import('../../src/components/Timeline.tsx');
  const mounted = await mount(createElement(Timeline));
  await flush(2);
  assert.match(mounted.container.querySelector('[data-id="2"]').textContent, /此条目已不在本地/);
  const previous = ids();
  setPlan({ get_article_updates: () => ({ items: [], missing_ids: [] }) });
  await act(async () => { await assert.rejects(state().reloadFromBackend(), /属性回读不完整/); });
  assert.deepEqual(ids(), previous);
});

componentTest('ReadingSession 今天跨零点：当前成员和后续页冻结原日界，下次查询改用新日界', { timeout: 20000 }, async () => {
  const clock = new Date(2026, 10, 8, 23, 59, 50).getTime();
  const originalNow = Date.now;
  Date.now = () => clock;
  try {
    const rows = rowsOf(501).map((row) => ({ ...row, published_at: new Date(clock - row.id * 1000).toISOString() }));
    start(rows, 'today'); await state().reloadFromBackend();
    const session = state().readingSession;
    const queryCall = invokeCalls.find((call) => call.cmd === 'list_article_member_ids');
    assert.equal(queryCall.args.args.since_ms, new Date(2026, 10, 8).getTime());
    assert.equal(queryCall.args.args.until_ms, new Date(2026, 10, 9).getTime());
    Date.now = () => clock + 20000;
    clearInvokeCalls();
    await state().reloadFromBackend({ keepReadingPosition: true });
    await state().loadMoreArticles();
    assert.equal(state().readingSession.generation, session.generation);
    assert.equal(ids().length, 501);
    assert.equal(invokeCalls.filter((call) => call.cmd === 'list_article_member_ids').length, 0);
    assert.ok(invokeCalls.some((call) => call.cmd === 'counts_snapshot'), '跨日属性刷新仍读当前计数');
    state().selectView('today');
    await waitFor(() => state().readingSession?.ready && state().readingSession.generation !== session.generation);
    assert.deepEqual(ids(), []);
    assert.equal(state().readingSession.query.dayStartMs, new Date(2026, 10, 9).getTime());
  } finally { Date.now = originalNow; }
});

componentTest('ReadingSession 退订收藏：来源独立于活动导航，列表/Reader/计数仍解析原来源', { timeout: 20000 }, async () => {
  const retained = { ...FEEDS[0], id: 99, folder_id: 0, title: '已退订的原来源', subscription_state: 'retained', effective_layout: 'article' };
  const row = mkRow({ id: 42, feed_id: 99, is_starred: true, retained: true, content_html: '<p>保留正文</p>' });
  start([row], 'starred');
  setPlan({ list_article_sources: () => [...FEEDS, retained], list_article_member_ids: () => [42],
    counts_snapshot: () => ({ feeds: [{ feed_id: 99, total: 1, unread: 1, starred: 1, today: 0 }], scope: null }) });
  await state().reloadFromBackend();
  assert.ok(!state().feedIndex.has('99'));
  assert.equal(state().sourceIndex.get('99').layout, 'article');
  assert.equal(storeMod.selectViewCounts(state()).starred, 1);
  const { createElement, act } = await import('react');
  const { Reader } = await import('../../src/components/Reader.tsx');
  const mounted = await mount(createElement(Reader));
  await act(async () => { state().selectArticle('42'); });
  await flush(3);
  assert.match(mounted.container.textContent, /已退订的原来源/);
  assert.match(mounted.container.textContent, /保留正文/);
});

componentTest('ReadingSession 旧属性快照不得覆盖读取期间成功的本机读藏写入', { timeout: 20000 }, async () => {
  const rows = rowsOf(3); start(rows, 'all'); await state().reloadFromBackend();
  const pending = deferred();
  setPlan({ get_article_updates: () => pending.promise });
  const refresh = state().reloadFromBackend();
  state().toggleEntryFlag('3', 'isRead');
  await flush(2);
  pending.resolve({ items: rows.map((row) => ({ ...row })), missing_ids: [] });
  await refresh;
  assert.equal(state().entries.find((entry) => entry.id === '3').isRead, true);
  assert.deepEqual(ids(), ['3', '2', '1']);
});

componentTest('ReadingSession 启动设置恢复先于成员查询；元数据首读仍并行', { timeout: 20000 }, async () => {
  start([mkRow({ id: 1, is_read: true, is_starred: true }), mkRow({ id: 2 })], 'all');
  const settings = deferred();
  setPlan({ get_setting: ({ key }) => key === 'app_settings' ? settings.promise : null });
  const boot = state().bootstrapFromBackend();
  try {
    await waitFor(() => invokeCalls.some((call) => call.cmd === 'list_folders'));
    assert.equal(invokeCalls.filter((call) => call.cmd === 'list_article_member_ids').length, 0);
  } finally {
    settings.resolve(JSON.stringify({ startupView: 'starred', hideReadOnStartup: false }));
    await boot;
  }
  assert.equal(state().activeViewFilter, 'starred');
  assert.deepEqual(ids(), ['1']);
  assert.equal(invokeCalls.find((call) => call.cmd === 'list_article_member_ids').args.args.only_starred, true);
});

componentTest('ReadingSession 全部标读只写捕获的完整成员，后台新增不混入当前动作', { timeout: 20000 }, async () => {
  const rows = rowsOf(502); start(rows); await state().reloadFromBackend();
  const expected = [...state().readingSession.memberIds];
  rows.push(mkRow({ id: 9000 }));
  clearInvokeCalls(); state().markCurrentViewAllRead(); await flush(3);
  const write = invokeCalls.find((call) => call.cmd === 'set_read_bulk');
  assert.ok(write);
  assert.deepEqual(write.args.ids.map(String), expected);
  assert.equal(invokeCalls.filter((call) => call.cmd === 'mark_all_read').length, 0);
  assert.deepEqual(ids(), expected.slice(0, 500));
});

componentTest('ReadingSession App 恢复焦点跨日后更新计数，当前今天成员不跳走', { timeout: 20000 }, async () => {
  const originalNow = Date.now;
  let now = new Date(2026, 10, 8, 23, 59, 50).getTime();
  Date.now = () => now;
  try {
    start([mkRow({ id: 1, published_at: new Date(now).toISOString() })], 'all');
    setPlan({ get_setting: ({ key }) => key === 'app_settings'
      ? JSON.stringify({ startupView: 'today', hideReadOnStartup: false }) : null });
    const { createElement, act } = await import('react');
    const { default: App } = await import('../../src/App.tsx');
    await mount(createElement(App));
    await waitFor(() => state().readingSession?.ready && !state().dataLoading);
    assert.deepEqual(ids(), ['1']);
    const generation = state().readingSession.generation;
    clearInvokeCalls(); now += 20000;
    await act(async () => { window.dispatchEvent(new Event('focus')); });
    await waitFor(() => state().backendReloadsInFlight === 0);
    assert.ok(invokeCalls.some((call) => call.cmd === 'counts_snapshot'));
    assert.equal(invokeCalls.filter((call) => call.cmd === 'list_article_member_ids').length, 0);
    assert.equal(state().readingSession.generation, generation);
    assert.deepEqual(ids(), ['1']);
  } finally { Date.now = originalNow; }
});

componentTest('ReadingSession 五分区×四视图×全部/组/源：成员查询完整传递同一范围', { timeout: 20000 }, async () => {
  const layouts = ['article', 'social', 'image', 'podcast', 'notification'];
  const current = new Date().toISOString();
  const old = new Date(Date.now() - 3 * 86400000).toISOString();
  const rows = layouts.flatMap((layout, index) => [
    mkRow({ id: index * 10 + 1, feed_id: index + 10, published_at: current, is_starred: true }),
    mkRow({ id: index * 10 + 2, feed_id: index + 10, published_at: old, is_read: true }),
  ]);
  start(rows, 'all');
  for (const [index, layout] of layouts.entries()) {
    for (const view of ['all', 'today', 'unread', 'starred']) {
      for (const scope of ['all', 'cat-1', String(index + 10)]) {
        useAppStore.setState({ readingSession: null, activeContentLayout: layout, activeViewFilter: view, activeFeedFilter: scope });
        await state().reloadFromBackend();
        const expected = view === 'all' ? [String(index * 10 + 1), String(index * 10 + 2)] : [String(index * 10 + 1)];
        assert.deepEqual(ids(), expected, `${layout}/${view}/${scope}`);
      }
    }
  }
});

componentTest('ReadingSession 排序/筛选重新捕获；今天/收藏不展示无效的未读切换', { timeout: 20000 }, async () => {
  start(rowsOf(3), 'all'); await state().reloadFromBackend();
  const generation = state().readingSession.generation;
  state().toggleTimelineSort();
  await waitFor(() => state().readingSession?.ready && state().readingSession.generation !== generation);
  assert.deepEqual(ids(), ['1', '2', '3']);
  const rows = rowsOf(3); rows[0].is_read = true; setCurrentRows(rows);
  state().toggleTimelineFilter();
  await waitFor(() => state().readingSession?.ready && state().readingSession.query.filter === 'unread');
  assert.deepEqual(ids(), ['2', '3']);
  const { createElement, act } = await import('react');
  const { Timeline } = await import('../../src/components/Timeline.tsx');
  const mounted = await mount(createElement(Timeline));
  assert.ok(mounted.container.querySelector('button[title="仅显示未读"]'));
  await act(async () => { state().selectView('starred'); });
  await waitFor(() => state().readingSession?.ready);
  assert.equal(mounted.container.querySelector('button[title="仅显示未读"]'), null);
  assert.equal(mounted.container.querySelector('button[title="显示全部"]'), null);
});

componentTest('ReadingSession 旧范围未返回不锁住新范围分页', { timeout: 20000 }, async () => {
  const rows = rowsOf(501); start(rows, 'all');
  const old = deferred(); let first = true;
  setPlan({ list_article_member_ids: () => {
    if (first) { first = false; return old.promise; }
    return rows.map(row => row.id).reverse();
  } });
  const oldReload = state().reloadFromBackend();
  await waitFor(() => !first);
  try {
    state().selectView('starred');
    await waitFor(() => state().readingSession?.ready);
    assert.equal(state().backendReloadsInFlight, 0, '新会话已可交互，不展示旧请求的加载态');
    await state().loadMoreArticles();
    assert.equal(ids().length, 501, '旧A未完成仍能分页新B');
  } finally { old.resolve([1]); await oldReload; }
  assert.equal(state().readingSession.query.view, 'starred');
});

componentTest('ReadingSession 同源多个身份回执不重复扣显示组计数，随后权威快照收敛', { timeout: 20000 }, async () => {
  start([mkRow({ id: 1 }), mkRow({ id: 3 })], 'all');
  const beforeCounts = { feeds: [{ feed_id: 10, total: 2, unread: 2, starred: 0, today: 0 }], scope: null };
  setPlan({ counts_snapshot: () => beforeCounts });
  await state().reloadFromBackend();
  const counts = deferred();
  setPlan({ set_read: () => [{ id: 1, feed_id: 10 }, { id: 2, feed_id: 10 }], counts_snapshot: () => counts.promise });
  try {
    state().toggleEntryFlag('1', 'isRead'); await flush(2);
    assert.equal(state().feedCounts.get('10').unread, 1, '两个身份回执只代表一个显示组变化');
  } finally { counts.resolve({ feeds: [{ feed_id: 10, total: 2, unread: 1, starred: 0, today: 0 }], scope: null }); await flush(2); }
  assert.equal(storeMod.selectViewCounts(state()).unread, 1);
});

componentTest('ReadingSession 深页返回按新成员重建并保留已加载深度；搜索深窗口不拉其前方全部属性', { timeout: 20000 }, async () => {
  start(rowsOf(1600), 'all'); await state().reloadFromBackend();
  await state().loadMoreArticles(); await state().loadMoreArticles();
  assert.equal(ids().length, 1500);
  state().selectView('starred'); await waitFor(() => state().readingSession?.ready);
  state().selectView('all'); await waitFor(() => state().readingSession?.ready && state().readingSession.query.view === 'all');
  assert.equal(ids().length, 1500, '缓存实体只留1000，加载深度仍由独立元数据保留');
  await state().anchorToArticle('50');
  assert.equal(state().readingSession.windowStart, 1550);
  state().selectView('starred'); await waitFor(() => state().readingSession?.ready);
  clearInvokeCalls(); state().selectView('all');
  await waitFor(() => state().readingSession?.ready && state().readingSession.query.view === 'all');
  assert.equal(state().readingSession.windowStart, 1550);
  assert.equal(ids()[0], '50');
  const fetched = invokeCalls.filter(call => call.cmd === 'get_article_updates').flatMap(call => call.args.ids);
  assert.equal(fetched.length, 50, '深窗口回访只读窗口属性');
});

for (const action of ['selectFeed', 'selectView', 'selectLayout']) {
  componentTest(`ReadingSession ${action}同上下文重筛消费当前活锚，跨范围返回才恢复存档`, { timeout: 20000 }, async () => {
    const { createElement, act } = await import('react');
    const { Timeline } = await import('../../src/components/Timeline.tsx');
    const anchors = await import('../../src/lib/readingPosition.ts');
    const rows = rowsOf(600);
    start(rows, 'all'); await state().reloadFromBackend();
    // 首次mount的refill effect就会读scrollHeight；应在它运行前补jsdom缺失几何，
    // 避免零高度被当作“尚未撑满”自动把600条全拉完，掩盖500页尾反例。
    const elementProto = window.Element.prototype;
    const originalScrollHeight = Object.getOwnPropertyDescriptor(elementProto, 'scrollHeight');
    Object.defineProperty(elementProto, 'scrollHeight', { configurable: true, get() {
      return this.id === 'timelineContentScroll' ? ids().length * 160 : (originalScrollHeight?.get?.call(this) ?? 0);
    } });
    let mounted;
    try { mounted = await mount(createElement(Timeline)); }
    finally {
      if (originalScrollHeight) Object.defineProperty(elementProto, 'scrollHeight', originalScrollHeight);
      else delete elementProto.scrollHeight;
    }
    const scroller = mounted.container.querySelector('#timelineContentScroll');
    // jsdom无滚动几何；只补容器高度，真实virtualizer仍自行求index位置并发scrollTo。
    Object.defineProperty(scroller, 'scrollHeight', { configurable: true,
      get: () => Number.parseFloat(scroller.querySelector('.timeline-virtual-wrap')?.style.height ?? '0') });
    Object.defineProperty(scroller, 'clientHeight', { configurable: true, get: () => 768 });
    const filterKey = 'article|all|all|all|newest';
    const savedNow = performance.now();
    anchors.rearmTopAnchor('590', filterKey, savedNow, 60);
    anchors.stashTopAnchorForReturn();
    anchors.commitTopAnchor('109', filterKey, savedNow + 1, 17);
    const returned = state().switchRestoreNonce;
    const refreshed = state().positionRestoreNonce;
    const generation = state().readingSession.generation;
    rows.push(...Array.from({ length: 20 }, (_, i) => mkRow({ id: 9000 + i, is_starred: true, published_at: '2031-01-01T00:00:00Z' })));
    clearInvokeCalls();
    await act(async () => { state()[action](action === 'selectLayout' ? 'article' : 'all'); });
    await waitFor(() => state().readingSession?.ready && state().readingSession.generation > generation && state().backendReloadsInFlight === 0);
    await flush(2);
    assert.equal(ids()[0], '9019', '重复点击仍重新捕获新增成员');
    assert.equal(state().readingSession.loadedCount, 520, '保留原500成员窗口的有效尾部，不因20个新头部挤掉活锚');
    assert.equal(state().switchRestoreNonce, returned, '不消费旧存档返回信号');
    assert.equal(state().positionRestoreNonce, refreshed + 1, '当前活锚保位信号只发一次');
    assert.equal(anchors.peekTopAnchor()?.id, '109');
    assert.equal(anchors.peekTopAnchor()?.offsetPx, 17);
    assert.equal(anchors.peekReturnAnchor(filterKey)?.id, '590');
    assert.equal(invokeCalls.filter((call) => call.cmd === 'list_article_member_ids').length, 1);
    assert.equal(scroller.scrollTop, ids().indexOf('109') * 160 + 17, '真实Timeline effect滚到活锚及偏移，不是旧存档590');

    await act(async () => { state().selectFeed('12'); });
    await waitFor(() => state().readingSession?.ready && state().readingSession.query.scope === '12');
    const beforeReturn = state().switchRestoreNonce;
    const beforePosition = state().positionRestoreNonce;
    await act(async () => { state().selectFeed('all'); });
    await waitFor(() => state().readingSession?.ready && state().readingSession.query.scope === 'all');
    await flush(2);
    assert.equal(state().switchRestoreNonce, beforeReturn + 1, '真实离开再返回仍消费存档');
    assert.equal(state().positionRestoreNonce, beforePosition, '返回不叠加当前活锚通道');
    assert.equal(anchors.peekReturnAnchor(filterKey)?.id, '109', '离开时已存入当前活锚');
    assert.equal(scroller.scrollTop, ids().indexOf('109') * 160 + 17);
    anchors.clearTopAnchor();
  });
}

componentTest('ReadingSession 同范围深窗口重筛只回读新位置附近，重复在途重选不丢原窗口边界', { timeout: 20000 }, async () => {
  const rows = rowsOf(1600); start(rows, 'all'); await state().reloadFromBackend();
  await state().anchorToArticle('50');
  assert.equal(state().readingSession.windowStart, 1550);
  rows.push(...Array.from({length:20},(_,i)=>mkRow({id:9000+i,is_starred:true,published_at:'2031-01-01T00:00:00Z'})));
  const old = deferred(); let memberCalls = 0;
  setPlan({ list_article_member_ids: () => ++memberCalls === 1 ? old.promise : [...rows].sort((a,b)=>Date.parse(b.published_at)-Date.parse(a.published_at)||b.id-a.id).map(row=>row.id) });
  clearInvokeCalls();
  state().selectFeed('all');
  await waitFor(() => memberCalls === 1);
  try {
    state().selectFeed('all');
    await waitFor(() => memberCalls === 2 && state().readingSession?.ready);
    assert.equal(state().readingSession.windowStart, 1570, '深窗口按新成员位置平移，未从0重读');
    assert.equal(ids()[0], '50');
    assert.equal(ids().at(-1), '1');
    const fetched = invokeCalls.filter(call=>call.cmd==='get_article_updates').flatMap(call=>call.args.ids);
    assert.deepEqual(fetched, Array.from({length:50},(_,i)=>50-i));
    assert.equal(invokeCalls.filter(call=>call.cmd==='get_articles').length, 0, '保位不触发正文回读');
  } finally { old.resolve([50]); await flush(3); }
  assert.equal(state().readingSession.windowStart, 1570, '迟到旧重筛不覆盖新深窗口');
});

componentTest('ReadingSession 新范围成员未就绪时全部已读不退到动态范围写，完成后只写捕获成员', { timeout: 20000 }, async () => {
  const {createElement,act} = await import('react');
  const {Timeline} = await import('../../src/components/Timeline.tsx');
  start([mkRow({id:1,is_starred:false}),mkRow({id:2,is_starred:true})],'all');
  await state().reloadFromBackend();
  const mounted=await mount(createElement(Timeline));
  const members=deferred();
  setPlan({list_article_member_ids:()=>members.promise});
  await act(async()=>{state().selectView('starred');});
  await waitFor(()=>state().readingSession?.ready===false);
  clearInvokeCalls();
  try {
    await act(async()=>{mounted.container.querySelector('button[title="将当前列表全部标为已读"]').click();});
    await flush(2);
    assert.equal(invokeCalls.filter(call=>['set_read','set_read_bulk','mark_all_read'].includes(call.cmd)).length,0,
      '未捕获成员不得消费旧entries或执行新范围动态全读');
    assert.ok(state().entries.every(entry=>!entry.isRead),'等待期间不伪造旧实体乐观读态');
    assert.ok(!state().toasts.some(toast=>toast.text==='已全部标为已读'));
  } finally {await act(async()=>{members.resolve([2]);});await waitFor(()=>state().readingSession?.ready);}
  clearInvokeCalls();
  await act(async()=>{mounted.container.querySelector('button[title="将当前列表全部标为已读"]').click();});
  await flush(2);
  assert.deepEqual(invokeCalls.find(call=>call.cmd==='set_read_bulk')?.args.ids,[2]);
  assert.equal(invokeCalls.filter(call=>call.cmd==='mark_all_read').length,0);
  assert.equal(state().entries.find(entry=>entry.id==='2').isRead,true);
});
