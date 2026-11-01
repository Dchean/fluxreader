// 审计返工 P2-1：防抖窗口内「删回同值」不得重置 searching——
// 用户搜 abc（已落地）→ 输入 abcd → 250ms 内删回 abc：fire 时
// next === debounced，搜索 effect 不重跑，若此处无条件 setSearching(true)
// 则 finally 永不执行，「搜索中…」卡真永挂。
//
// 取证形态：夹具让命令/订阅源都不匹配 'abc' 且 search_articles 返回空行 →
// items 为空，列表空态文案在「搜索中… / 没有结果」之间切换，DOM 可直接判读：
//   首查落地后应显示「没有结果」（searching 复位真实发生）；
//   删回同值越窗后仍必须是「没有结果」（修前此处卡「搜索中…」），
//   且 search_articles 不重发（debounced 未变）。

import assert from 'node:assert/strict';
import { componentTest, mount, flush, waitFor, resetStore, useAppStore } from './harness.mjs';
import { installBackend, seedFromRows, mkRow, setPlan, resetPlan, invokeCalls, clearInvokeCalls } from './fixtures.mjs';

/** 真实输入：绕过 React value tracker 后派发 input 事件（触发 onchange 真实路径）。 */
function typeInput(input, value) {
  const setter = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(input), 'value')?.set;
  assert.ok(setter, '应从 InputElement.prototype 取得受控 value setter');
  setter.call(input, value);
  input.dispatchEvent(new Event('input', { bubbles: true }));
}

componentTest('搜索防抖窗口内删回同值：不重发请求、「搜索中」不卡真（P2-1）', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows([mkRow({ id: 101, title: 'A1', published_at: '2026-10-01T10:00:00Z' })], { layout: 'article' });
  try {
    /* 空结果：文章组为空；'abc' 不匹配任何命令/订阅源 → items 为空，空态文案可见 */
    setPlan({ search_articles: () => Promise.resolve([]) });

    const { createElement, act } = await import('react');
    const { default: App } = await import('../../src/App.tsx');
    const entry = await mount(createElement(App));
    await waitFor(() => entry.container.querySelectorAll('.article-card').length === 1,
      { label: 'App 启动后文章卡出现', timeout: 8000 });

    await act(async () => { useAppStore.getState().openSearch(); });
    await flush(2);
    const input = entry.container.querySelector('.search-modal-input');
    assert.ok(input, '搜索面板应真实出现');

    /* 首查：abc → 越过防抖窗口 → 发出并落地（空结果 → searching 复位） */
    await act(async () => { typeInput(input, 'abc'); });
    await flush(2, 300);
    await waitFor(() => entry.container.textContent.includes('没有结果'),
      { label: '首查落地为空结果（searching 已复位）', timeout: 8000 });
    assert.ok(entry.container.textContent.includes('没有结果'), '落地后应显示「没有结果」而非「搜索中…」');
    const firstCalls = invokeCalls.filter((c) => c.cmd === 'search_articles').length;
    assert.ok(firstCalls === 1, `首查恰发一次（实际 ${firstCalls}）`);

    /* 窗口内删回同值：abcd → abc 落在同一防抖窗口（timer 重置，一次 fire） */
    clearInvokeCalls();
    await act(async () => { typeInput(input, 'abcd'); });
    await act(async () => { typeInput(input, 'abc'); });
    await flush(2, 300);
    await flush(2);
    assert.ok(!entry.container.textContent.includes('搜索中…'),
      '删回同值后「搜索中」不得卡真（修前 searching 卡真、文案永挂）');
    assert.ok(entry.container.textContent.includes('没有结果'), '同值不重查：仍显示「没有结果」');
    const reCalls = invokeCalls.filter((c) => c.cmd === 'search_articles').length;
    assert.ok(reCalls === 0, `debounced 未变不得重发（实际 ${reCalls}）`);
  } finally {
    resetPlan();
  }
});
