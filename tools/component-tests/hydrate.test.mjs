// OPT-016B：正文水合（bodyCache + 批量 IPC）的真实挂载回归。
//
// 覆盖卡片要求：bodyCache 记录更新与卡片 hydrate 前后**同一实例**；不用
// 「先 unmount 再 render」绕过 effect；假后端返回不同延迟/缺行/失败可重试，
// 不是每次都回完美 fixture。
//
// 取证方式：挂载真实 Timeline（social 布局——卡片挂载即经 useLazyHydrate 触发
// 真实批量水合），get_articles 由 deferred 手动放行：先制造在途（延迟），再按
// 场景 reject（失败）/缺行（missing）/命中（ready）。断言同时读 DOM 与
// dist-test bodyCache 记录——组件与断言读的是同一份实例（同挂载树不重建）。

import assert from 'node:assert/strict';
import { componentTest, mount, flush, waitFor, resetStore, getBodyEntry, useAppStore } from './harness.mjs';
import { installBackend, seedFromRows, mkRow, setPlan, deferred, invokeCalls, clearInvokeCalls } from './fixtures.mjs';

const ROWS = [
  mkRow({ id: 201, feed_id: 11, title: '社交 201', snippet: 's201' }),
  mkRow({ id: 202, feed_id: 11, title: '社交 202', snippet: 's202' }),
  mkRow({ id: 203, feed_id: 11, title: '社交 203', snippet: 's203' }),
];
const contentRow = (row, html) => ({ ...row, content_html: html });

componentTest('OPT-016B 正文水合：跨延迟/失败/缺行的真实 effect 链，bodyCache 与卡片同实例', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows(ROWS, { layout: 'social' });

  /* get_articles 全部手动放行：每次调用登记一个 deferred。 */
  const batches = [];
  setPlan({
    get_articles: (args) => {
      const d = deferred();
      batches.push({ ids: args.ids.map(String), d });
      return d.promise;
    },
  });

  const { createElement, act } = await import('react');
  const { Timeline } = await import('../../src/components/Timeline.tsx');
  const entry = await mount(createElement(Timeline));
  await flush(3);

  // ---- 场景 1：延迟（在途）——loading 记录 + 卡片加载占位 ----
  await waitFor(() => {
    const ids = new Set(batches.flatMap((b) => b.ids));
    return ids.has('201') && ids.has('202') && ids.has('203');
  }, { label: '三张社交卡触发批量水合' });

  assert.equal(invokeCalls.filter((c) => c.cmd === 'get_articles').length, 1,
    '同帧挂载的三张卡应合批为一次 get_articles（不逐篇洪峰）');
  for (const id of ['201', '202', '203']) {
    assert.equal(getBodyEntry(id)?.state, 'loading', `id=${id} 在途时应落 loading 记录（断言的实例与组件同一份）`);
  }
  for (const id of ['201', '202', '203']) {
    const card = entry.container.querySelector(`.social-card[data-id="${id}"]`);
    assert.ok(card, `卡片 ${id} 应已挂载`);
    assert.ok(card.querySelector('.hydrate-placeholder')?.textContent.includes('加载正文…'),
      `id=${id} 在途时卡片应显示加载占位（延迟真实可见）`);
  }
  /* 卡片引用留证：后续所有断言都在同一挂载树上进行（不 unmount、不重挂载）。 */
  const card202Before = entry.container.querySelector('.social-card[data-id="202"]');

  // ---- 场景 2：批量失败——failed 记录 + 卡片内联重试入口 ----
  await act(async () => {
    for (const b of batches) b.d.reject(new Error('注入失败：db busy'));
    await Promise.resolve();
  });
  await flush(3);
  for (const id of ['201', '202', '203']) {
    assert.equal(getBodyEntry(id)?.state, 'failed', `id=${id} 失败应落 failed 记录`);
    const card = entry.container.querySelector(`.social-card[data-id="${id}"]`);
    const retry = card.querySelector('.hydrate-retry');
    assert.ok(retry?.textContent.includes('正文加载失败'), `id=${id} 卡片应出现失败文案与重试入口`);
    assert.equal(retry.textContent.includes(getBodyEntry(id).message), true, `id=${id} 卡片文案取自同一记录 message`);
  }

  // ---- 场景 3：失败重试（延迟→命中）——同一挂载树内恢复 ----
  clearInvokeCalls();
  await act(async () => {
    entry.container.querySelector('.social-card[data-id="202"] .hydrate-retry').click();
  });
  await flush(2);
  assert.equal(invokeCalls.filter((c) => c.cmd === 'get_articles').length, 1, '重试应重新发起一次 get_articles');
  assert.deepEqual(batches.at(-1).ids, ['202'], '重试只请求该卡片 id');
  assert.equal(getBodyEntry('202')?.state, 'loading', '重试后先回 loading');
  const card202 = entry.container.querySelector('.social-card[data-id="202"]');
  assert.equal(card202, card202Before, '重试不得重建卡片节点（同一挂载树）');
  assert.ok(card202.querySelector('.hydrate-placeholder')?.textContent.includes('加载正文…'));

  await act(async () => {
    batches.at(-1).d.resolve([contentRow(ROWS.find((r) => r.id === 202), '<p>202 正文已到达</p>')]);
    await Promise.resolve();
  });
  await flush(3);
  assert.equal(getBodyEntry('202')?.state, 'ready', '重试命中行应落 ready');
  assert.ok(card202.querySelector('.social-text')?.innerHTML.includes('202 正文已到达'),
    '卡片应渲染记录里的真实正文');
  assert.equal(useAppStore.getState().entries.find((e) => e.id === '202')?.snippet, 's202', '视图行轻字段不被水合破坏');

  // ---- 场景 4：缺行——missing 终态（响应里没有该行）----
  await act(async () => {
    entry.container.querySelector('.social-card[data-id="203"] .hydrate-retry').click();
  });
  await flush(2);
  await act(async () => {
    batches.at(-1).d.resolve([]); // 空 rows = 该批不存在
    await Promise.resolve();
  });
  await flush(3);
  assert.equal(getBodyEntry('203')?.state, 'missing', '缺行应落 missing 终态');
  assert.ok(getBodyEntry('203')?.message.includes('文章不存在'), 'missing 应带固定文案');
  const card203 = entry.container.querySelector('.social-card[data-id="203"]');
  assert.ok(card203.textContent.includes('文章不存在'), '卡片应呈现「文章不存在」而非永挂加载');

  // ---- 场景 5：延迟再命中——201 全链路（loading → ready，DOM 真实刷新）----
  await act(async () => {
    entry.container.querySelector('.social-card[data-id="201"] .hydrate-retry').click();
  });
  await flush(2);
  assert.equal(getBodyEntry('201')?.state, 'loading');
  await act(async () => {
    batches.at(-1).d.resolve([contentRow(ROWS.find((r) => r.id === 201), '<p>201 正文</p>')]);
    await Promise.resolve();
  });
  await flush(3);
  assert.equal(getBodyEntry('201')?.state, 'ready');
  assert.ok(entry.container.querySelector('.social-card[data-id="201"] .social-text')?.innerHTML.includes('201 正文'));

  /* 同一实例收口：三次真实写入（failed/ready/missing）都发生在同一份 bodyCache，
     组件 DOM 与记录逐一对上；202 的卡片节点自始至终未被替换。未预期
     console/jsdom/React 错误由 componentTest 统一账本终检判红（R1）。 */
  assert.equal(entry.container.querySelector('.social-card[data-id="202"]'), card202Before, '整轮水合后 202 卡片仍为同一节点');
});
