// 2026-11-08 审计反例转正式门禁：真实 App + 设置浮层 + 控件键盘冒泡。
// IPC 仅作外部边界替身；不替换 App 的快捷键、store 导航或读藏实现。
import assert from 'node:assert/strict';
import { componentTest, mount, flush, waitFor, resetStore, useAppStore } from './harness.mjs';
import { installBackend, seedFromRows, mkRow, invokeCalls, resetPlan } from './fixtures.mjs';

for (const { key, modifier, label } of [
  { key: 's', modifier: { ctrlKey: true }, label: 'Ctrl+S' },
  { key: 'j', modifier: { altKey: true }, label: 'Alt+J' },
]) {
  componentTest(`设置浮层内 ${label} 不改变背后文章或发送读藏写入`, { timeout: 40000 }, async () => {
    installBackend();
    resetPlan();
    resetStore();
    seedFromRows([mkRow({ id: 101 }), mkRow({ id: 102 })]);
    const { createElement, act } = await import('react');
    const { default: App } = await import('../../src/App.tsx');
    const entry = await mount(createElement(App));
    await waitFor(() => entry.container.querySelectorAll('.article-card').length === 2,
      { label: '真实 App 启动并呈现两篇文章', timeout: 8000 });
    await act(async () => { useAppStore.getState().selectArticle('101'); });
    await flush(3);
    await act(async () => { useAppStore.getState().openSettings(); });
    await flush(2);
    const target = entry.container.querySelector('[role="dialog"][aria-label="设置"] button');
    assert.ok(target, '打开的是实际设置对话框，焦点目标为按钮而非文本输入');
    await act(async () => { target.focus(); });
    assert.ok(document.activeElement === target, '真实焦点处于设置按钮');
    const snapshot = () => ({
      active: useAppStore.getState().activeArticleId,
      articles: useAppStore.getState().entries.map(({ id, isRead, isStarred }) => ({ id, isRead, isStarred })),
      writes: invokeCalls.filter((c) => ['set_read', 'set_read_bulk', 'set_starred'].includes(c.cmd)),
    });
    const before = snapshot();
    await act(async () => {
      target.dispatchEvent(new KeyboardEvent('keydown', { key, ...modifier, bubbles: true, cancelable: true }));
    });
    await flush(2);
    assert.equal(useAppStore.getState().settingsOpen, true, '未声明的组合键不关闭设置');
    assert.deepEqual(snapshot(), before, `${label} 不得穿透设置去执行单键动作`);
  });
}
