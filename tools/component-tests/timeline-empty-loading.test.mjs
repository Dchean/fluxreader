// 审计卡2-4：空态/加载态不混同（真实 Timeline 挂载）——列表为空且装载在途
// 时不出空态假文案，改渲染加载形态；数据落地后按真值恢复。
// 同窗口哨兵让位（不出现同屏双 spinner）；loading 结束后空态文案照旧
// （gallery-virtual 用例已锁「清空 entries（非在途）→ 文案」的既有行为）。

import assert from 'node:assert/strict';
import { componentTest, mount, flush, resetStore, useAppStore } from './harness.mjs';
import { installBackend, seedFromRows, mkRow } from './fixtures.mjs';

for (const loading of [
  { label: '分页加载', articlesLoading: true, backendReloadsInFlight: 0 },
  { label: '后端重载', articlesLoading: false, backendReloadsInFlight: 1 },
]) {
componentTest(`Timeline ${loading.label}空态分流：在途出加载形态、哨兵让位，落地后恢复文案`, { timeout: 40000 }, async () => {
  installBackend();
  resetStore();
  seedFromRows([mkRow({ id: 101, title: 'A1', published_at: '2026-10-01T10:00:00Z' })], { layout: 'article' });

  const { createElement, act } = await import('react');
  const { Timeline } = await import('../../src/components/Timeline.tsx');
  const entry = await mount(createElement(Timeline));
  await flush(3);

  // 在途 + 空列表：加载形态（spinner + aria），无空态文案、无哨兵
  await act(async () => { useAppStore.setState({
    entries: [], articlesLoading: loading.articlesLoading, backendReloadsInFlight: loading.backendReloadsInFlight,
  }); });
  await flush(2);
  const emptyState = entry.container.querySelector('.timeline-empty-state');
  assert.ok(emptyState, '空态容器应渲染');
  assert.ok(emptyState.querySelector('.load-more-spinner'), '在途时应渲染加载形态（spinner）');
  assert.equal(emptyState.getAttribute('aria-label'), '加载中', '加载形态应对读屏可用');
  assert.ok(!entry.container.textContent.includes('没有未读文章'), '在途时不得出现空态假文案');
  assert.ok(!entry.container.querySelector('.timeline-load-more'), '哨兵应让位（不出现同屏双 spinner）');

  // 落地（仍在途结束、集合为空且已到底）：空态文案恢复
  await act(async () => { useAppStore.setState({ articlesLoading: false, backendReloadsInFlight: 0, articlesExhausted: true }); });
  await flush(2);
  assert.ok(entry.container.textContent.includes('没有未读文章'), '落地后空态文案按真值恢复');
});
}
