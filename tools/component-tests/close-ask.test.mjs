// OPT-016B：CloseAskDialog 浮层键盘回归（真实 App 挂载——快捷键逻辑在 App 的
// window keydown 监听里，不挂 App 就测不到这条真实消费路径）。
//
// 断言结构（判别对，避免恒真；R1 复审后按「每键正负控制」重写）：
//   A. 正控制（无浮层，真实键盘消费 store）：
//      - j：中间项 → 下一项；k：回到上一项；
//      - k 在首项**回绕到末项**（首项 k 不是 no-op，浮层期的 k 断言才有判别力）；
//      - S/M 真的改这篇文章的收藏/读态（打开即标读为真实初态）。
//   B. CloseAskDialog 实际出现在 DOM 后：j/k/s/S/m/M **逐键、单次、每一步**断言
//      读/藏/导航与写 IPC 全不变（不双次按压回原值来掩盖变化）。
//   C. Escape 按原规则应答：action='tray'、remember=false（不替用户记住选择），
//      且不落入「关闭阅读器」分支；resolve_close 经真实 IPC mock 落库调用可查。
//   D. 真实点击「退出 FluxReader」按钮：action='exit'、remember=true（默认勾选）。
//   E. 其它浮层（搜索面板）同口径：打开时按键让路、Escape 关闭面板而非阅读器。
// 未预期 console/jsdom/React 错误由 componentTest 的统一账本终检（R1）。

import assert from 'node:assert/strict';
import { componentTest, mount, flush, waitFor, resetStore, useAppStore, pressKey, storeMod } from './harness.mjs';
import { installBackend, seedFromRows, mkRow, invokeCalls, clearInvokeCalls, invokeCount } from './fixtures.mjs';

const { selectVisibleEntries } = storeMod;

/** 三行文章布局夹具：published_at 递进，newest-first 顺序确定为 [103, 102, 101]。 */
function rows() {
  return [
    mkRow({ id: 101, title: 'A1', published_at: '2026-10-01T10:00:00Z' }),
    mkRow({ id: 102, title: 'A2', published_at: '2026-10-02T10:00:00Z' }),
    mkRow({ id: 103, title: 'A3', published_at: '2026-10-03T10:00:00Z' }),
  ];
}

componentTest('OPT-016B CloseAskDialog：实际出现时键盘不改背后文章，Escape/按钮应答走真实 IPC', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows(rows(), { layout: 'article' });

  const { createElement, act } = await import('react');
  const { default: App } = await import('../../src/App.tsx');
  const entry = await mount(createElement(App));

  /* 等待真实启动链（bootstrapSettings → bootstrapFromBackend）把列表装好。 */
  await waitFor(() => entry.container.querySelectorAll('.article-card').length === 3,
    { label: 'App 启动后文章卡出现', timeout: 8000 });

  const visible = () => selectVisibleEntries(useAppStore.getState()).map((e) => e.id);
  assert.deepEqual(visible(), ['103', '102', '101'], '列表顺序应确定（newest-first）');
  const art = (id) => useAppStore.getState().entries.find((e) => e.id === id);
  const active = () => useAppStore.getState().activeArticleId;
  const selectAt = async (id) => {
    await act(async () => { useAppStore.getState().selectArticle(id); });
    await flush(2);
  };

  // ---- A. 正控制：真实键盘真的消费 J/K 与 S/M（无浮层）----
  await selectAt('102'); // 中间项
  await act(async () => { pressKey('j'); });
  await flush(2);
  assert.equal(active(), '101', '正控制：j 应移动到下一项（102 → 101）');
  await act(async () => { pressKey('k'); });
  await flush(2);
  assert.equal(active(), '102', '正控制：k 应回到上一项（101 → 102）');
  await selectAt('103'); // 首项（index 0）
  await act(async () => { pressKey('k'); });
  await flush(2);
  assert.equal(active(), '101', '正控制：k 在首项应回绕到末项（首项 k 不是 no-op——浮层期同键断言因此有判别力）');
  await selectAt('102');
  assert.equal(art('102')?.isRead, true, '打开即标读（markReadOnOpen 真实生效，作为 M 正控制的初态）');
  await act(async () => { pressKey('s'); });
  await flush(1);
  assert.equal(art('102')?.isStarred, true, '正控制：S 应收藏当前文章');
  await act(async () => { pressKey('s'); });
  await flush(1);
  assert.equal(art('102')?.isStarred, false, '正控制：S 再按取消收藏');
  await act(async () => { pressKey('m'); });
  await flush(1);
  assert.equal(art('102')?.isRead, false, '正控制：M 应把已读文章标回未读');
  await act(async () => { pressKey('m'); });
  await flush(1);
  assert.equal(art('102')?.isRead, true, '正控制：M 再按回已读');

  // ---- B. CloseAskDialog 实际出现后：逐键、单次、每步状态不变 ----
  await selectAt('103'); // 首项：j 会前进、k 会回绕——若浮层守卫失效，两者都是真实状态变化
  await act(async () => { useAppStore.setState({ closeAskVisible: true }); });
  await flush(2);
  const dialogVisible = [...entry.container.querySelectorAll('.mini-dialog-title')]
    .some((el) => el.textContent.includes('关闭 FluxReader'));
  assert.ok(dialogVisible, 'CloseAskDialog 应真实渲染在 DOM（不是仅 store 布尔）');
  assert.ok(entry.container.querySelector('.modal-overlay.open .mini-dialog'), '确认框所在浮层应处于 open 态');

  const snapshot = () => JSON.stringify({
    active: active(),
    read: art('103')?.isRead,
    starred: art('103')?.isStarred,
    opened: Object.keys(useAppStore.getState().openedReadIds).sort(),
  });
  const baseline = snapshot();
  const writesSoFar = () => invokeCount('set_read') + invokeCount('set_starred') + invokeCount('set_read_bulk');
  const writesBefore = writesSoFar();
  for (const key of ['j', 'k', 's', 'S', 'm', 'M']) {
    await act(async () => { pressKey(key); });
    await flush(1);
    assert.equal(snapshot(), baseline, `确认框打开时按「${key}」后状态必须与按前逐字段一致（单次按压、逐步断言）`);
    assert.equal(writesSoFar(), writesBefore, `确认框打开时按「${key}」不得产生读/藏写 IPC`);
  }

  // ---- C. Escape 按原规则应答（tray + 不记住）----
  await act(async () => { pressKey('Escape'); });
  await flush(2);
  assert.equal(useAppStore.getState().closeAskVisible, false, 'Escape 应关闭确认框');
  const resolveCalls = invokeCalls.filter((c) => c.cmd === 'resolve_close');
  assert.equal(resolveCalls.length, 1, '应答应经真实 resolve_close IPC 发出一次');
  assert.deepEqual(resolveCalls[0].args, { action: 'tray', remember: false }, 'Escape = 本次最小化到托盘且不记住选择');
  assert.equal(active(), '103', 'Escape 不得落入「关闭阅读器」分支（文章仍选中）');
  assert.equal(useAppStore.getState().settings.closePromptShown, false, '不记住选择：closePromptShown 不得被写成 true');
  assert.equal(useAppStore.getState().settings.closeToTray, true, '不记住选择：closeToTray 保持原值（默认 true）');

  // ---- D. 真实点击「退出 FluxReader」按钮 ----
  await act(async () => { useAppStore.setState({ closeAskVisible: true }); });
  await flush(1);
  clearInvokeCalls();
  const exitBtn = [...entry.container.querySelectorAll('.mini-dialog-actions button')]
    .find((b) => b.textContent.includes('退出 FluxReader'));
  assert.ok(exitBtn, '确认框应有「退出 FluxReader」按钮');
  await act(async () => { exitBtn.click(); });
  await flush(2);
  const exitCall = invokeCalls.filter((c) => c.cmd === 'resolve_close');
  assert.deepEqual(exitCall.map((c) => c.args), [{ action: 'exit', remember: true }],
    '点击退出：action=exit、remember=true（默认勾选记住）');
  assert.equal(useAppStore.getState().closeAskVisible, false);
  assert.equal(useAppStore.getState().settings.closeToTray, false, 'remember=true 时应同步设置镜像 closeToTray=false');
  assert.equal(useAppStore.getState().settings.closePromptShown, true, 'remember=true 应标记已询问');

  // ---- E. 其它浮层（搜索面板）同口径回归 ----
  await act(async () => { useAppStore.getState().openSearch(); });
  await flush(2);
  assert.ok(entry.container.querySelector('.search-modal-input'), '搜索面板应真实出现');
  clearInvokeCalls();
  const beforeSearch = snapshot();
  for (const key of ['s', 'm']) {
    await act(async () => { pressKey(key); });
    await flush(1);
    assert.equal(snapshot(), beforeSearch, `搜索面板打开时按「${key}」后状态不变（逐步断言）`);
  }
  await act(async () => { pressKey('Escape'); });
  await flush(1);
  assert.equal(useAppStore.getState().searchOpen, false, 'Escape 应关闭搜索面板（第二分支）');
  assert.equal(active(), '103', '关闭搜索不得动到阅读器选中文章');
});
