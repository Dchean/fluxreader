// tools/component-tests/error-center.test.mjs
// OPT-017E：错误中心真实挂载回归——入口可见/可关闭、面板逐条可重试、重试与忽略
// 真实改变 store、普通 toast 形态不回归。
//
// 证据边界：jsdom 真实 DOM + React 真实挂载生产 App（Sidebar 入口在真实布局树上）；
// 断言引用的是 store 的同一份 dist-test 实例与真实 DOM 节点（非 SSR、非数组长度自证）。
// 未预期 console/jsdom/React 错误由 componentTest 的统一账本终检。
//
// 保险丝（先红后绿）：旧实现上 `.sidebar-error-center` / `.error-center-dialog`
// 不存在，本用例以 assertion 失败判红（不是导入缺失崩运行）。
import assert from 'node:assert/strict';
import { componentTest, mount, flush, waitFor, resetStore, useAppStore } from './harness.mjs';
import { installBackend, clearInvokeCalls, seedFromRows, defaultRows } from './fixtures.mjs';

componentTest('OPT-017E 错误中心：入口在侧栏可见、面板可关闭、重试/忽略真实生效、普通 toast 不回归', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows(defaultRows());

  const { createElement, act } = await import('react');
  const { default: App } = await import('../../src/App.tsx');
  const entry = await mount(createElement(App));

  const $ = (sel) => entry.container.querySelector(sel);
  const $$ = (sel) => [...entry.container.querySelectorAll(sel)];
  const dialogOverlay = () => $('.error-center-dialog')?.closest('.modal-overlay') ?? null;
  const panelOpen = () => !!dialogOverlay()?.classList.contains('open');
  const openPanelIfNeeded = async () => {
    if (panelOpen()) return;
    const trigger = $('.sidebar-error-center');
    assert.ok(trigger, '侧栏错误中心入口应可见（errors 非空）');
    await act(async () => { trigger.click(); });
    await flush(2);
  };

  // 等真实启动链把主框架装好
  await waitFor(() => !!$('.sidebar-fixed-bottom'), { label: 'App 启动后侧栏出现', timeout: 8000 });

  /* ---------- 0. 干净态：无错误时入口不存在（不制造常驻噪音） ---------- */
  /* 注意断言写法：布尔断言（assert.ok(!el)）而非 assert.equal(el, null)——
     后者在判红时会 deep-inspect 整个 jsdom 节点（含 React fiber 环），
     慢且无意义（M6 变异实测：断言消息格式化拖垮运行）。 */
  assert.ok(!$('.sidebar-error-center'), '无错误时错误中心入口不应存在（条目清空即入口消失）');

  /* ---------- 1. 连续 10 条带重试错误：入口出现、坏数徽标 = 10 ---------- */
  const retried = [];
  await act(async () => {
    for (let i = 0; i < 10; i += 1) {
      useAppStore.getState().showToast(`错误 ${i}：操作失败`, { label: '重试', run: () => retried.push(i) });
    }
  });
  await flush(2);
  assert.equal(useAppStore.getState().errors?.length ?? 0, 10, '10 条错误应全部进入错误中心（toast 上限不挤掉入口）');
  const trigger = $('.sidebar-error-center');
  assert.ok(trigger, '有错误时侧栏入口应真实渲染');
  assert.equal($('.error-center-badge')?.textContent, '10', '入口徽标应显示待处理条数 10');

  /* ---------- 2. 打开面板：10 行、最新在前 ---------- */
  await act(async () => { trigger.click(); });
  await flush(2);
  assert.equal(useAppStore.getState().errorCenterOpen, true, '点击入口应打开面板（store 开态）');
  assert.ok(panelOpen(), '面板所在浮层应处于 open 态（真实 DOM）');
  assert.equal($$('.error-center-item').length, 10, '面板应逐条列出 10 条错误');
  assert.ok($$('.error-center-item')[0].textContent.includes('错误 9'), '最新错误应在最前（合并/顺序口径与 store 一致）');

  /* ---------- 3. 点击重试：真实执行动作、对应条目消失 ---------- */
  await act(async () => { $$('.error-center-item')[0].querySelector('.error-center-retry').click(); });
  await flush(2);
  assert.deepEqual(retried, [9], '重试按钮应执行对应条目的动作（最新那条）');
  assert.equal(useAppStore.getState().errors?.length ?? 0, 9, '重试派发后条目应从错误中心清除');
  assert.equal($$('.error-center-item').length, 9, '面板应即时反映清除（DOM 行数 9）');

  /* ---------- 4. 忽略单条：只删目标行 ---------- */
  const beforeDismiss = $$('.error-center-item').map((el) => el.textContent);
  await act(async () => { $$('.error-center-item')[0].querySelector('.error-center-dismiss').click(); });
  await flush(2);
  assert.equal(useAppStore.getState().errors?.length ?? 0, 8, '忽略应只移除该条');
  assert.equal($$('.error-center-item').length, 8, 'DOM 行数应同步减一');
  assert.notEqual($$('.error-center-item').map((el) => el.textContent).join('|'), beforeDismiss.join('|'), '被忽略的应是目标行');

  /* ---------- 4b. 同文案不同实体（R1）：面板逐条并列，不被合并成一条 ---------- */
  await act(async () => {
    for (let i = 0; i < 3; i += 1) {
      useAppStore.getState().showToast('刷新失败：网络不可达', { label: '重试', key: `feed:refresh:${i}`, run: () => retried.push(`feed${i}`) });
    }
  });
  await flush(2);
  const sameTextRows = $$('.error-center-item').filter((el) => el.textContent.includes('刷新失败：网络不可达'));
  assert.equal(sameTextRows.length, 3, '同文案不同实体应各占一行（显式实体 key，不被合并）');

  /* ---------- 5. 全部清除：空态出现、入口随条目清空而消失 ---------- */
  await act(async () => { $$('.mini-dialog-actions button').find((b) => b.textContent.includes('全部清除')).click(); });
  await flush(2);
  assert.equal(useAppStore.getState().errors?.length ?? 0, 0, '全部清除应清空条目');
  assert.equal($$('.error-center-item').length, 0, '面板不应再有行');
  assert.ok($('.error-center-empty'), '清空后应显示空态文案');
  assert.ok(!$('.sidebar-error-center'), '条目清空后侧栏入口应消失（可关闭语义的另一半）');

  /* ---------- 6. 关闭面板：✕ 与 Escape 两条路径 ---------- */
  await act(async () => { useAppStore.getState().showToast('清理失败：磁盘忙', { label: '重试', run: () => {} }); });
  await flush(2);
  await openPanelIfNeeded();
  assert.ok(panelOpen(), '重新打开面板');
  await act(async () => { $('.error-center-close').click(); });
  await flush(2);
  assert.equal(useAppStore.getState().errorCenterOpen, false, '✕ 应关闭面板');
  assert.equal(panelOpen(), false, '关闭后浮层应退出 open 态');

  await openPanelIfNeeded();
  const { pressKey } = await import('./harness.mjs');
  await act(async () => { pressKey('Escape'); });
  await flush(2);
  assert.equal(useAppStore.getState().errorCenterOpen, false, 'Escape 应经 App Esc 链关闭面板');

  /* ---------- 6.5 toast 表面上的一键重试与面板同一语义（重试即消费条目） ---------- */
  await act(async () => { useAppStore.getState().clearErrors(); useAppStore.setState({ toasts: [] }); });
  let toastRetried = 0;
  await act(async () => {
    useAppStore.getState().showToast('同步失败：超时', { label: '重试', run: () => { toastRetried += 1; } });
  });
  await flush(2);
  assert.equal(useAppStore.getState().errors?.length ?? 0, 1, '前置：带动作 toast 已写入错误中心');
  const toastRetryBtn = $$('.toast-pill.with-action')
    .find((el) => el.textContent.includes('同步失败：超时'))
    ?.querySelector('.toast-action-btn');
  assert.ok(toastRetryBtn, '带动作 toast 的重试按钮应在 DOM');
  await act(async () => { toastRetryBtn.click(); });
  await flush(2);
  assert.equal(toastRetried, 1, 'toast 重试按钮应执行动作（既有行为保持）');
  assert.equal(useAppStore.getState().errors?.length ?? 0, 0, 'toast 表面重试成功同样消费对应条目（两个表示面一致）');

  /* ---------- 7. 普通 toast 形态不回归（含动作 toast 的既有形态） ---------- */
  await act(async () => { useAppStore.getState().showToast('普通提示'); });
  await flush(2);
  const plainPill = $$('.toast-pill').find((el) => el.textContent === '普通提示');
  assert.ok(plainPill, '普通 toast 应仍以 .toast-pill 形态渲染');
  assert.equal(plainPill.classList.contains('with-action'), false, '普通 toast 不应带操作按钮形态');
  await act(async () => { useAppStore.getState().showToast('刷新失败：超时', { label: '重试', run: () => {} }); });
  await flush(2);
  const actionPill = $$('.toast-pill.with-action').find((el) => el.textContent.includes('刷新失败：超时'));
  assert.ok(actionPill, '带动作 toast 应仍是 .toast-pill.with-action 形态');
  assert.ok(actionPill.querySelector('.toast-action-btn'), '带动作 toast 内应仍有一键重试按钮');
});
