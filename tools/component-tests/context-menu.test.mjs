// 审计卡2-7：全局右键菜单的键盘可达性（真实 App 挂载——ContextMenuHost 在
// App 顶层，菜单经 portal 渲染到 document.body）。
//
// WAI-ARIA menu 模式逐项取证：
//   打开即焦点入首项（可用项）→ ArrowDown 顺序移动 → 末项 ArrowDown 回绕首项
//   → 首项 ArrowUp 回绕末项 → Home/End 跳首/末 → Enter 激活（真实消费 store
//   动作：收藏翻转 + 菜单关闭）→ Esc 关闭。
// 菜单本体在 portal（document.body），断言读 document 而非挂载容器。
// 断言卫生：一律 assert.ok(布尔)——不把 DOM 元素直接交给 assert.equal（失败时
// Node 的 assertion diff 会对 jsdom 元素做 Myers diff，实测耗尽 40s 量级）。

import assert from 'node:assert/strict';
import { componentTest, mount, flush, waitFor, resetStore, useAppStore } from './harness.mjs';
import { installBackend, seedFromRows, mkRow, invokeCalls, clearInvokeCalls } from './fixtures.mjs';

const ROWS = [
  mkRow({ id: 101, title: 'A1', published_at: '2026-10-01T10:00:00Z' }),
  mkRow({ id: 102, title: 'A2', published_at: '2026-10-02T10:00:00Z' }),
];

const menuItems = () => [...document.querySelectorAll('.ctx-menu [role="menuitem"]')];
const activeElement = () => document.activeElement;
/** 键盘事件派发到当前聚焦项（真实路径：打开后焦点在菜单内，事件经 document
    捕获/冒泡链消费；直接派发到 window 不会进入 document 捕获监听）。 */
const pressOnFocused = async (act, key) => {
  await act(async () => {
    (document.activeElement ?? document.body).dispatchEvent(
      new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true }));
  });
  await flush(1);
};
const openMenuOn = async (act, card) => {
  await act(async () => {
    card.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 40, clientY: 40 }));
  });
  await flush(2);
};

componentTest('ContextMenu 键盘导航：打开即焦点入首项、↑↓ 循环、Home/End、Enter 激活、Esc 关闭', { timeout: 40000 }, async (t) => {
  installBackend();
  resetStore();
  seedFromRows(ROWS, { layout: 'article' });
  t.after(() => { delete globalThis.__OPEN_URL_ERROR__; });

  const { createElement, act } = await import('react');
  const { default: App } = await import('../../src/App.tsx');
  const entry = await mount(createElement(App));

  await waitFor(() => entry.container.querySelectorAll('.article-card').length === 2,
    { label: 'App 启动后文章卡出现', timeout: 8000 });

  const card = entry.container.querySelector('.article-card[data-id="101"]');
  assert.ok(card, '文章卡应存在且带 data-id');

  // ---- 打开：焦点入首项（文章菜单 4 项：收藏/标为已读/查看原文/复制链接）----
  await openMenuOn(act, card);
  let items = menuItems();
  assert.ok(items.length === 4, 'article 上下文菜单应有 4 项');
  assert.ok(activeElement() === items[0], '打开后焦点应落在首项');

  // ---- ArrowDown 顺序移动 + 末项回绕首项 ----
  const press = async (key) => pressOnFocused(act, key);
  await press('ArrowDown');
  assert.ok(activeElement() === menuItems()[1], 'ArrowDown 应移动到第二项');
  for (let i = 0; i < 3; i++) await press('ArrowDown');
  assert.ok(activeElement() === menuItems()[0], '末项 ArrowDown 应循环回首项');

  // ---- 首项 ArrowUp 回绕末项；Home/End 跳首/末 ----
  await press('ArrowUp');
  assert.ok(activeElement() === menuItems()[3], '首项 ArrowUp 应循环到末项');
  await press('Home');
  assert.ok(activeElement() === menuItems()[0], 'Home 应跳到首项');
  await press('End');
  assert.ok(activeElement() === menuItems()[3], 'End 应跳到末项');

  // ---- Esc 关闭 ----
  await pressOnFocused(act, 'Escape');
  await flush(2);
  assert.ok(!document.querySelector('.ctx-menu'), 'Escape 应关闭菜单');

  // ---- Enter 激活：真实消费 store 动作（收藏翻转）并关闭菜单 ----
  await openMenuOn(act, card);
  items = menuItems();
  assert.ok(activeElement() === items[0], '重开后焦点仍在首项（收藏）');
  await pressOnFocused(act, 'Enter');
  await flush(2);
  assert.ok(!document.querySelector('.ctx-menu'), 'Enter 激活后菜单应关闭');
  assert.equal(useAppStore.getState().entries.find((e) => e.id === '101')?.isStarred, true,
    'Enter 激活「收藏」应真实翻转 isStarred');

  // ---- 「查看原文」：成功打开保留；opener 失败时给统一可执行提示 ----
  globalThis.__OPEN_URLS__ = [];
  delete globalThis.__OPEN_URL_ERROR__;
  await openMenuOn(act, card);
  let viewOriginal = menuItems().find((item) => item.textContent.includes('查看原文'));
  assert.ok(viewOriginal, '文章菜单包含查看原文');
  await act(async () => { viewOriginal.click(); });
  await flush(2);
  assert.ok((globalThis.__OPEN_URLS__ ?? []).includes('https://example.com/a'),
    '链接存在时右键查看原文正常打开');

  globalThis.__OPEN_URL_ERROR__ = new Error('no default browser');
  await act(async () => { useAppStore.setState({ toasts: [] }); });
  await openMenuOn(act, card);
  viewOriginal = menuItems().find((item) => item.textContent.includes('查看原文'));
  assert.ok(viewOriginal, '失败用例菜单包含查看原文');
  await act(async () => { viewOriginal.click(); });
  await flush(2);
  assert.ok(useAppStore.getState().toasts.some((toast) =>
    toast.text.includes('无法在浏览器中打开此链接')
    && toast.text.includes('检查默认浏览器设置')),
    '右键查看原文失败显示统一 toast');
});

componentTest('ContextMenu 打开时 j/k/s/m 被截停：不冒泡到 App 全局监听、背后文章状态不变（P2-2）', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows(ROWS, { layout: 'article' });

  const { createElement, act } = await import('react');
  const { default: App } = await import('../../src/App.tsx');
  const entry = await mount(createElement(App));

  await waitFor(() => entry.container.querySelectorAll('.article-card').length === 2,
    { label: 'App 启动后文章卡出现', timeout: 8000 });

  /* 背后文章选中：j（移动）/ s（收藏）/ m（读态）都有真实的可变状态 */
  await act(async () => { useAppStore.getState().selectArticle('102'); });
  await flush(2);
  const art = (id) => useAppStore.getState().entries.find((e) => e.id === id);
  const snapshot = () => JSON.stringify({
    active: useAppStore.getState().activeArticleId,
    read: art('102')?.isRead,
    starred: art('102')?.isStarred,
    writes: invokeCalls.filter((c) => ['set_read', 'set_starred'].includes(c.cmd)).length,
  });
  const before = snapshot();

  const card = entry.container.querySelector('.article-card[data-id="101"]');
  await openMenuOn(act, card);
  assert.ok(menuItems().length === 4, '菜单应打开');

  for (const key of ['j', 'k', 's', 'm', 'J', 'K', 'S', 'M']) {
    await pressOnFocused(act, key);
    assert.equal(snapshot(), before, `菜单打开时按「${key}」背后状态与写 IPC 不得变化（不冒泡到 App 全局监听）`);
  }

  /* 正控制：关菜单后同一路径按键必须真实生效（截停只发生在菜单打开时） */
  await pressOnFocused(act, 'Escape');
  await flush(2);
  assert.ok(!document.querySelector('.ctx-menu'), '菜单应已关闭');
  /* 向卡片派发而非 activeElement：正控制要验证的是「事件不再被菜单截停」，
     目标用非输入元素保证 inInput 判定与键路干净（关闭态 autoFocus 加固后
     挂载时焦点在 body，此处派发目标与焦点状态解耦，两种行为下都成立）。 */
  await act(async () => {
    card.dispatchEvent(new KeyboardEvent('keydown', { key: 's', bubbles: true, cancelable: true }));
  });
  await flush(2);
  assert.equal(art('102')?.isStarred, true, '关菜单后 s 应真实收藏当前文章（判别键路本身畅通）');
});

componentTest('刷新文案随 store.syncMode 在真实右键菜单与命令面板中同步切换', { timeout: 40000 }, async () => {
  installBackend();
  resetStore();
  seedFromRows(ROWS, { layout: 'article' });

  const { createElement, act } = await import('react');
  const { default: App } = await import('../../src/App.tsx');
  const entry = await mount(createElement(App));
  await waitFor(() => entry.container.querySelectorAll('.article-card').length === 2,
    { label: 'App 启动后文章卡出现', timeout: 8000 });

  const setMode = async (syncMode) => {
    await act(async () => {
      useAppStore.setState((s) => ({
        settings: { ...s.settings, syncMode },
      }));
    });
    await flush(2);
  };
  const openGlobalMenu = async () => {
    await act(async () => {
      document.body.dispatchEvent(new MouseEvent('contextmenu', {
        bubbles: true, cancelable: true, clientX: 40, clientY: 40,
      }));
    });
    await flush(2);
  };
  const globalRefreshLabel = () => menuItems()
    .map((item) => item.textContent.trim())
    .find((text) => text.includes('刷新'));
  const closeMenu = async () => {
    await pressOnFocused(act, 'Escape');
    assert.ok(!document.querySelector('.ctx-menu'), '菜单应已关闭');
  };

  await setMode('direct');
  await openGlobalMenu();
  assert.equal(globalRefreshLabel(), '刷新全部订阅源',
    'direct 模式右键菜单应真实渲染「全部」');
  await closeMenu();

  await setMode('hybrid');
  await openGlobalMenu();
  assert.equal(globalRefreshLabel(), '刷新本地订阅源',
    'hybrid 模式右键菜单应真实渲染「本地」');
  await closeMenu();

  await act(async () => { useAppStore.getState().openSearch(); });
  await flush(2);
  const paletteRefreshLabel = () => [...document.querySelectorAll('.cp-label')]
    .map((node) => node.textContent.trim())
    .find((text) => text.includes('刷新'));
  assert.equal(paletteRefreshLabel(), '刷新本地订阅源',
    '命令面板打开时应按当前 hybrid 模式渲染');
  await setMode('direct');
  assert.equal(paletteRefreshLabel(), '刷新全部订阅源',
    '命令面板保持打开时也必须跟随 store.syncMode 更新');
});
