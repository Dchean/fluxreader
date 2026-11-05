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
//   C. 遮罩与 Escape 只取消：隐藏弹窗、不发 resolve_close、不写记住选择，
//      焦点回到打开前的窗口控件，且不落入「关闭阅读器」分支。
//   D. 真实点击「退出 FluxReader」按钮：action='exit'、remember=true（默认勾选）。
//   E. 显式取消勾选后点「最小化到托盘」：action='tray'、remember=false。
//   F. 其它浮层（搜索面板）同口径：打开时按键让路、Escape 关闭面板而非阅读器。
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

componentTest('OPT-016B CloseAskDialog：实际出现时键盘不改背后文章，遮罩/Esc 只取消、显式按钮走真实 IPC', { timeout: 40000 }, async () => {
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

  // ---- C. 遮罩 / Escape 只取消，不提交关闭动作 ----
  clearInvokeCalls();
  const openAsk = entry.container.querySelector('.modal-overlay.open');
  assert.ok(openAsk, '前置：关闭确认框处于打开态');
  await act(async () => { openAsk.click(); });
  await flush(2);
  assert.equal(useAppStore.getState().closeAskVisible, false, '遮罩点击应隐藏确认框');
  assert.equal(invokeCalls.filter((c) => c.cmd === 'resolve_close').length, 0, '遮罩点击不得提交 resolve_close');
  assert.equal(active(), '103', '遮罩取消不得落入「关闭阅读器」分支（文章仍选中）');
  assert.equal(useAppStore.getState().settings.closePromptShown, false, '不记住选择：closePromptShown 不得被写成 true');
  assert.equal(useAppStore.getState().settings.closeToTray, true, '不记住选择：closeToTray 保持原值（默认 true）');

  const focusTrigger = entry.container.querySelector('.immersive-win-controls .win-btn');
  assert.ok(focusTrigger, '窗口控制按钮存在（焦点归还目标）');
  focusTrigger.focus();
  await act(async () => { useAppStore.setState({ closeAskVisible: true }); });
  await flush(2);
  const escCard = entry.container.querySelector('.modal-overlay.open .modal-card');
  assert.equal(document.activeElement, escCard, '前置：确认框打开后焦点移入面板');
  await act(async () => { pressKey('Escape'); });
  await flush(2);
  assert.equal(useAppStore.getState().closeAskVisible, false, 'Escape 应隐藏确认框');
  assert.equal(invokeCalls.filter((c) => c.cmd === 'resolve_close').length, 0, 'Escape 不得提交 resolve_close');
  assert.equal(document.activeElement, focusTrigger, 'Escape 取消后焦点应回到原窗口控件');
  assert.equal(active(), '103', 'Escape 取消不得落入「关闭阅读器」分支');
  assert.equal(useAppStore.getState().settings.closePromptShown, false, 'Escape 取消不得写 closePromptShown');
  assert.equal(useAppStore.getState().settings.closeToTray, true, 'Escape 取消不得写 closeToTray');

  focusTrigger.focus();
  await act(async () => { useAppStore.setState({ closeAskVisible: true }); });
  await flush(2);
  const backdropCard = entry.container.querySelector('.modal-overlay.open .modal-card');
  assert.equal(document.activeElement, backdropCard, '前置：确认框打开后焦点移入面板');
  await act(async () => { backdropCard.parentElement.click(); });
  await flush(2);
  assert.equal(useAppStore.getState().closeAskVisible, false, '遮罩点击应隐藏确认框');
  assert.equal(invokeCalls.filter((c) => c.cmd === 'resolve_close').length, 0, '遮罩取消不得提交 resolve_close');
  assert.equal(document.activeElement, focusTrigger, '遮罩取消后焦点应回到原窗口控件');
  assert.equal(invokeCalls.filter((c) => c.cmd === 'set_setting').length, 0, '取消询问不得写 set_setting');

  // ---- D. 显式按钮四组合：tray / exit × remember true / false ----
  const combinations = [
    { action: 'exit', remember: true, label: '退出 FluxReader' },
    { action: 'tray', remember: false, label: '最小化到托盘' },
    { action: 'tray', remember: true, label: '最小化到托盘' },
    { action: 'exit', remember: false, label: '退出 FluxReader' },
  ];
  for (const combo of combinations) {
    await act(async () => {
      useAppStore.setState((s) => ({
        closeAskVisible: true,
        settings: { ...s.settings, closeToTray: true, closePromptShown: false },
      }));
    });
    await flush(2);
    const askOverlay = entry.container.querySelector('.modal-overlay.open');
    const rememberBox = askOverlay.querySelector('.mini-dialog-checkbox input[type="checkbox"]');
    if (rememberBox.checked !== combo.remember) {
      await act(async () => { rememberBox.click(); });
      await flush(1);
    }
    assert.equal(rememberBox.checked, combo.remember, `前置：记住选择=${combo.remember}`);

    clearInvokeCalls();
    const actionBtn = [...askOverlay.querySelectorAll('.mini-dialog-actions button')]
      .find((b) => b.textContent.includes(combo.label));
    assert.ok(actionBtn, `确认框应有「${combo.label}」按钮`);
    await act(async () => { actionBtn.click(); });
    await flush(2);

    assert.deepEqual(
      invokeCalls.filter((c) => c.cmd === 'resolve_close').map((c) => c.args),
      [{ action: combo.action, remember: combo.remember }],
      `${combo.label} / remember=${combo.remember}：resolve_close 参数正确`,
    );
    assert.equal(invokeCount('set_setting'), 0, '显式关闭按钮不得发 set_setting IPC');
    assert.equal(useAppStore.getState().closeAskVisible, false, '显式按钮应关闭确认框');
    assert.equal(
      useAppStore.getState().settings.closeToTray,
      combo.remember ? combo.action === 'tray' : true,
      `remember=${combo.remember} 时 closeToTray 镜像正确`,
    );
    assert.equal(
      useAppStore.getState().settings.closePromptShown,
      combo.remember,
      `remember=${combo.remember} 时 closePromptShown 镜像正确`,
    );
  }

  // ---- F. 其它浮层（搜索面板）同口径回归 ----
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
