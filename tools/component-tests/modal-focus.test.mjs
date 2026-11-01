// W4（2026-11-01 前端体验批次）：模态焦点陷阱 + dialog 语义回归。
//
// 取证方式（与 OPT-016B 同纪律）：真实挂载生产组件。
// - CloseAskDialog（ModalOverlay 面，App 真实挂载）：role="dialog" + aria-modal +
//   aria-label；打开焦点移入面板；Tab 在末元素回绕到首元素、Shift+Tab 在首元素
//   回绕到末元素（边界由 trap 代码处理——jsdom 无默认 Tab 遍历，恰好只考我们
//   自己的代码路径）；关闭后焦点还原到打开前的触发元素。
// - ConfirmDialog（FeedsTab 删除源确认，portal 挂 body）：同契约抽查。
// 证据边界：jsdom 无布局/无焦点遍历，「浏览器默认 Tab 会走到背景」这一半只能由
//   源码形态断言（ui-contracts 域）+ 真实浏览器行为佐证，本文件锁的是 trap 自身
//   的拦截与回绕逻辑（不依赖 jsdom 遍历）。

import assert from 'node:assert/strict';
import { componentTest, mount, flush, waitFor, resetStore, useAppStore } from './harness.mjs';
import { installBackend, seedFromRows, mkRow, clearInvokeCalls } from './fixtures.mjs';

const ROWS = [
  mkRow({ id: 101, title: 'A1', published_at: '2026-10-01T10:00:00Z' }),
  mkRow({ id: 102, title: 'A2', published_at: '2026-10-02T10:00:00Z' }),
  mkRow({ id: 103, title: 'A3', published_at: '2026-10-03T10:00:00Z' }),
];

/** 在元素上派发 Tab 键（React 合成事件走冒泡）。返回事件以便断言 defaultPrevented。 */
function pressTab(el, { shift = false } = {}) {
  const ev = new KeyboardEvent('keydown', { key: 'Tab', shiftKey: shift, bubbles: true, cancelable: true });
  el.dispatchEvent(ev);
  return ev;
}

componentTest('W4 模态焦点陷阱：CloseAskDialog 打开移入/边界回绕/关闭还原 + dialog 语义', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows(ROWS, { layout: 'article' });

  const { createElement, act } = await import('react');
  const { default: App } = await import('../../src/App.tsx');
  const entry = await mount(createElement(App));
  await waitFor(() => entry.container.querySelectorAll('.article-card').length === 3,
    { label: 'App 启动后文章卡出现', timeout: 8000 });

  /* 触发元素：窗口控制按钮（在 ErrorBoundary 之外、App 之内——关闭后焦点还原目标） */
  const trigger = entry.container.querySelector('.immersive-win-controls .win-btn');
  assert.ok(trigger, '窗口控制按钮存在');
  trigger.focus();
  assert.equal(document.activeElement, trigger, '前置：焦点在触发元素上');

  await act(async () => { useAppStore.setState({ closeAskVisible: true }); });
  await flush(2);

  /* dialog 语义（W4 第 3 项）：role + aria-modal + aria-label */
  const card = entry.container.querySelector('.modal-overlay.open .modal-card');
  assert.ok(card, '浮层卡真实渲染');
  assert.equal(card.getAttribute('role'), 'dialog', 'ModalOverlay 容器应带 role="dialog"');
  assert.equal(card.getAttribute('aria-modal'), 'true', '应带 aria-modal="true"');
  assert.equal(card.getAttribute('aria-label'), '关闭 FluxReader', 'aria-label 应为面板标题');

  /* 打开移入：焦点在面板上（tabIndex=-1 容器），不在触发元素 */
  assert.equal(document.activeElement, card, '打开后焦点应移入面板容器');

  const focusables = [...card.querySelectorAll('button:not([disabled]), input:not([disabled])')];
  assert.ok(focusables.length >= 3, `面板内可聚焦元素（开关 + 两按钮）应存在，实际 ${focusables.length}`);
  const first = focusables[0];
  const last = focusables[focusables.length - 1];

  /* 边界回绕：末元素 Tab → 首元素（trap 拦截 + preventDefault） */
  last.focus();
  const tabEv = pressTab(last);
  await flush(1);
  assert.equal(tabEv.defaultPrevented, true, '末元素 Tab 应被 trap 拦截（不穿透到背景）');
  assert.equal(document.activeElement, first, '末元素 Tab 应回绕到首个可聚焦元素');

  /* 非边界不拦截：中部元素前进 Tab 交还默认行为（jsdom 无遍历 → 焦点原地） */
  const mid = focusables[1];
  mid.focus();
  const midEv = pressTab(mid);
  await flush(1);
  assert.equal(midEv.defaultPrevented, false, '非边界 Tab 不得被拦截');

  /* 首元素 Shift+Tab → 末元素 */
  first.focus();
  const shiftEv = pressTab(first, { shift: true });
  await flush(1);
  assert.equal(shiftEv.defaultPrevented, true, '首元素 Shift+Tab 应被 trap 拦截');
  assert.equal(document.activeElement, last, '首元素 Shift+Tab 应回绕到最后一个可聚焦元素');

  /* 关闭还原：遮罩点击关闭（真实关闭路径）→ 焦点回到打开前的触发元素 */
  await act(async () => { card.parentElement.click(); });
  await flush(2);
  assert.equal(useAppStore.getState().closeAskVisible, false, '遮罩点击应关闭确认框');
  assert.equal(document.activeElement, trigger, '关闭后焦点应还原到触发元素');
});

componentTest('W4 模态焦点陷阱：ConfirmDialog 同契约（role/aria-label + 打开移入 + 边界回绕）', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows(ROWS, { layout: 'article' });

  const { createElement, act } = await import('react');
  const { FeedsTab } = await import('../../src/components/settings/FeedsTab.tsx');
  const entry = await mount(createElement(FeedsTab));
  await flush(4);

  const trigger = entry.container.querySelector('.group-mgr-feed-controls .btn-danger-text');
  assert.ok(trigger, '源行删除按钮存在');
  trigger.focus();
  await act(async () => { trigger.click(); });
  await flush(2);

  /* ConfirmDialog portal 挂 body；role="dialog" + aria-modal + aria-label=标题 */
  const dialog = document.body.querySelector('.confirm-overlay .confirm-dialog');
  assert.ok(dialog, '确认弹窗真实渲染（portal）');
  assert.equal(dialog.getAttribute('role'), 'dialog', 'ConfirmDialog 面板应带 role="dialog"');
  assert.equal(dialog.getAttribute('aria-modal'), 'true', '应带 aria-modal="true"');
  assert.ok(dialog.getAttribute('aria-label'), 'aria-label 应为确认标题');

  assert.equal(document.activeElement, dialog, '打开后焦点应移入确认面板');

  const focusables = [...dialog.querySelectorAll('button:not([disabled])')];
  const first = focusables[0];
  const last = focusables[focusables.length - 1];
  last.focus();
  const tabEv = pressTab(last);
  await flush(1);
  assert.equal(tabEv.defaultPrevented, true, '末元素 Tab 应被 trap 拦截');
  assert.equal(document.activeElement, first, '末元素 Tab 应回绕到首个按钮');
});
