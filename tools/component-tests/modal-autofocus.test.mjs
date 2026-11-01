// 关闭态浮层输入不得抢焦点（真实组件回归）。
//
// 背景：ModalOverlay 关闭态**无条件挂载 children**，只靠 inert 挡焦点
// （见 primitives.tsx REQ-047 注释）。此前搜索/重命名输入写裸 autoFocus，
// 关闭态挂载时 React 照样执行 focus——真实引擎被 CDP 探针证明 inert 生效、
// 无焦点窃取，但 jsdom **不尊崇 inert**，该通道上关闭态挂载必抢焦点
// （修前本用例必红）；条件 autoFocus 是源码级保证，不依赖引擎 inert 差异。
//
// 断言形态（搜索与重命名同构）：
//   1. 关闭态挂载：input 存在但不持焦；
//   2. 打开后：body 按 key 重挂，autoFocus 生效，input 持焦；
//   3. 关闭后：重挂的新 input 不持焦。
//
// 通道边界：jsdom 不尊崇 inert——本用例不试图断言「inert 使关闭态不可聚焦」，
// 只断言源码级 autoFocus 条件化的焦点行为。

import assert from 'node:assert/strict';
import { componentTest, mount, flush, resetStore, useAppStore } from './harness.mjs';

function assertNotFocused(input, label) {
  assert.ok(input, `${label}：input 应存在`);
  assert.notEqual(document.activeElement, input, `${label}：input 不应持焦`);
}

componentTest('搜索浮层关闭态不抢焦，打开后持焦，关闭后不再持焦', { timeout: 20000 }, async () => {
  resetStore();
  const { createElement } = await import('react');
  const { SearchModal } = await import('../../src/components/Overlays.tsx');
  await mount(createElement(SearchModal));
  await flush(2);

  assertNotFocused(document.querySelector('.search-modal-input'), '关闭态挂载');

  useAppStore.getState().openSearch();
  await flush(2);
  const opened = document.querySelector('.search-modal-input');
  assert.equal(document.activeElement, opened, '打开后搜索 input 应持焦');

  useAppStore.getState().closeSearch();
  await flush(2);
  assertNotFocused(document.querySelector('.search-modal-input'), '关闭后重挂');
});

componentTest('重命名分类弹窗关闭态不抢焦，打开后持焦，关闭后不再持焦', { timeout: 20000 }, async () => {
  resetStore({
    categories: [{
      id: 'c1', name: '测试分类', collapsed: false, settingsCollapsed: false,
      layout: 'article', autoSummary: false, autoTranslate: false, feeds: [],
    }],
    renameCatTargetId: 'c1',
  });
  const { createElement } = await import('react');
  const { RenameCategoryModal } = await import('../../src/components/Overlays.tsx');
  await mount(createElement(RenameCategoryModal));
  await flush(2);

  assertNotFocused(document.querySelector('.mini-dialog input'), '关闭态挂载');

  useAppStore.getState().openRenameCatModal('c1');
  await flush(2);
  const opened = document.querySelector('.mini-dialog input');
  assert.equal(document.activeElement, opened, '打开后重命名 input 应持焦');

  useAppStore.getState().closeMiniModal('renameCat');
  await flush(2);
  assertNotFocused(document.querySelector('.mini-dialog input'), '关闭后重挂');
});
