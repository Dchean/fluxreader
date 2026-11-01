// 审计卡2-1：IME 合成期的全局 Escape 不得关闭浮层（真实 App 挂载——
// 全局 Escape 分支在 App 的 window keydown 监听里，不挂 App 测不到）。
//
// 两条 IME 形态都验，且都携带 key='Escape'（真实误关的形态：分支判 key 命中、
// 但事件属输入法合成）：
//   A. isComposing=true —— Safari 等合成期原样上送 key 的形态；
//   B. keyCode=229 —— Chromium/WebView2 合成期统一上报 229 的形态。
// 口径与 Overlays 命令面板 Enter 的 isComposing 判一致（审计卡2-1）。
// 负控制：不带 IME 标记的 Escape 仍按序关闭浮层（只关浮层、不动阅读器）。

import assert from 'node:assert/strict';
import { componentTest, mount, flush, waitFor, resetStore, useAppStore, pressKey } from './harness.mjs';
import { installBackend, seedFromRows, mkRow } from './fixtures.mjs';

const ROWS = [
  mkRow({ id: 101, title: 'A1', published_at: '2026-10-01T10:00:00Z' }),
  mkRow({ id: 102, title: 'A2', published_at: '2026-10-02T10:00:00Z' }),
];

componentTest('IME 合成中的 Escape 不关浮层（isComposing / keyCode 229 两形态），正常 Esc 仍关', { timeout: 40000 }, async () => {
  installBackend();
  resetStore();
  seedFromRows(ROWS, { layout: 'article' });

  const { createElement, act } = await import('react');
  const { default: App } = await import('../../src/App.tsx');
  const entry = await mount(createElement(App));

  await waitFor(() => entry.container.querySelectorAll('.article-card').length === 2,
    { label: 'App 启动后文章卡出现', timeout: 8000 });

  // 搜索浮层：Esc 关闭链上的真实浮层（closeAsk 之后的第二分支）
  await act(async () => { useAppStore.getState().openSearch(); });
  await flush(2);
  assert.ok(entry.container.querySelector('.search-modal-input'), '搜索浮层应真实出现');

  // 形态 A：isComposing=true（合成中取消选词）
  await act(async () => { pressKey('Escape', { isComposing: true }); });
  await flush(2);
  assert.equal(useAppStore.getState().searchOpen, true, 'isComposing 的 Escape 不得关闭搜索浮层');
  assert.ok(entry.container.querySelector('.search-modal-input'), '浮层 DOM 仍在');

  // 形态 B：keyCode=229（Chromium/WebView2 合成期统一上报）
  await act(async () => { pressKey('Escape', { keyCode: 229 }); });
  await flush(2);
  assert.equal(useAppStore.getState().searchOpen, true, 'keyCode 229 的 Escape 不得关闭搜索浮层');

  // 负控制：正常 Escape 仍按序关闭（且只关浮层，不动阅读器选中）
  await act(async () => { pressKey('Escape'); });
  await flush(2);
  assert.equal(useAppStore.getState().searchOpen, false, '正常 Escape 仍应关闭搜索浮层');
  assert.equal(useAppStore.getState().activeArticleId, null, '关闭搜索不得动阅读器选中');
});
