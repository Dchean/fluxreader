// 回归：显式添加订阅源后必须重新捕获成员。
// 原缺陷形态：新文章已入库、计数变成 N，但列表仍停留在旧会话显示空态，
// 因为 reloadFromBackend 的 refresh 分支（为后台同步保成员）不会重捕获成员。
import assert from 'node:assert/strict';
import { componentTest as register, resetStore, useAppStore, storeMod, flush } from './harness.mjs';
import { installBackend, resetPlan, setPlan, setCurrentRows, mkRow } from './fixtures.mjs';

const state = () => useAppStore.getState();
const ids = () => storeMod.selectVisibleEntries(state()).map((entry) => entry.id);
const { viewEntriesCache } = await import('../../dist-test/store/internals.js');

const componentTest = (name, opts, run) => register(name, opts, async () => {
  try { await run(); } finally { resetPlan(); viewEntriesCache.clear(); }
});

componentTest('添加订阅源：计数更新与新成员进入列表必须同时发生', { timeout: 20000 }, async () => {
  viewEntriesCache.clear();
  installBackend();
  resetPlan();
  resetStore({ dataMode: 'tauri', timelineFilter: 'all', activeViewFilter: 'unread', articlesExhausted: false });
  setCurrentRows([]);

  await state().reloadFromBackend();
  assert.deepEqual(ids(), [], '空库时列表为空');
  assert.equal(state().readingSession.memberIds.length, 0, '空库时没有捕获到成员');

  // 后端在 add_feed 期间完成抓取并入库；随后同一导航下重新回读。
  const fetched = mkRow({
    id: 1, feed_id: 10, title: '新抓到的文章', is_read: false,
    published_at: new Date().toISOString(), url: 'https://example.com/new',
  });
  setCurrentRows([fetched]);
  setPlan({ add_feed: () => Promise.resolve({ id: 10 }) });

  await state().addFeed('', 'https://example.invalid/feed.xml', '新源', 'inherit', 'inherit', 'inherit', false);
  await flush(4);

  assert.deepEqual(ids(), ['1'], '添加订阅源后新文章必须出现在当前列表');
  assert.equal(state().readingSession.memberIds.length, 1, '成员集合必须重新捕获');
  assert.equal(storeMod.selectVisibleEntries(state())[0].title, '新抓到的文章', '列表内容必须来自新抓到的行');
});

componentTest('添加订阅源不改变导航时仍保持既有成员的顺序语义', { timeout: 20000 }, async () => {
  viewEntriesCache.clear();
  installBackend();
  resetPlan();
  resetStore({ dataMode: 'tauri', timelineFilter: 'all', activeViewFilter: 'unread', articlesExhausted: false });
  const base = [
    mkRow({ id: 5, title: '较新', published_at: '2026-11-20T10:00:00Z', url: 'https://example.com/5' }),
    mkRow({ id: 4, title: '较旧', published_at: '2026-11-19T10:00:00Z', url: 'https://example.com/4' }),
  ];
  setCurrentRows(base);
  await state().reloadFromBackend();
  assert.deepEqual(ids(), ['5', '4']);

  // 更旧的一条入队后，重捕获必须沿用 newest_first 排序，不出现倒序或重复。
  setCurrentRows([...base, mkRow({ id: 3, title: '更旧', published_at: '2026-11-18T10:00:00Z', url: 'https://example.com/3' })]);
  setPlan({ add_feed: () => Promise.resolve({ id: 11 }) });

  await state().addFeed('', 'https://example.invalid/two.xml', '第二个源', 'inherit', 'inherit', 'inherit', false);
  await flush(4);

  assert.deepEqual(ids(), ['5', '4', '3'], '重捕获后仍按原排序契约排列');
  assert.equal(new Set(ids()).size, 3, '成员不得重复');
});