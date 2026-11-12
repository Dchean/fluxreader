import assert from 'node:assert/strict';
import { componentTest, mount, flush, waitFor, resetStore, useAppStore } from './harness.mjs';
import { installBackend, resetPlan, setPlan, seedFromRows, mkRow, invokeCalls, clearInvokeCalls, deferred } from './fixtures.mjs';

const EMPTY = { waiting: 0, failed: 0, local_only: 0, bindable_retry: 0, bound_pending: 0, retry_wait: 0, expired_unconfirmed: 0, last_error: null };
const STATUS = { connected: true, endpoint: 'https://a.example', account: 'alice', last_sync: 100, protocol: 'greader' };

function setup(queue = { ...EMPTY, account_generation: 7, expired_unconfirmed: 3 }) {
  installBackend();
  resetPlan();
  clearInvokeCalls();
  resetStore();
  seedFromRows([mkRow({ id: 101 })]);
  setPlan({ sync_status: () => Promise.resolve(STATUS), sync_queue_stats: () => Promise.resolve(queue) });
}

function button(container, label) {
  return [...container.querySelectorAll('button')].find((item) => item.textContent.includes(label));
}

async function setInput(input, value) {
  const { act } = await import('react');
  await act(async () => {
    Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set.call(input, value);
    input.dispatchEvent(new window.Event('input', { bubbles: true }));
  });
}

componentTest('同步恢复：摘要代际进入IPC，连续点击只重排一次且如实展示bound/unbound结果', { timeout: 40000 }, async () => {
  setup();
  const request = deferred();
  setPlan({ sync_retry_expired: () => request.promise });
  try {
    const { createElement, act } = await import('react');
    const { SyncTab } = await import('../../src/components/settings/SyncTab.tsx');
    const entry = await mount(createElement(SyncTab));
    await waitFor(() => button(entry.container, '重试过期项')?.disabled === false, { label: '恢复入口已读取代际' });
    const retry = button(entry.container, '重试过期项');
    await act(async () => { retry.click(); retry.click(); });
    assert.deepEqual(invokeCalls.filter((item) => item.cmd === 'sync_retry_expired').map((item) => item.args), [{ expectedAccountGeneration: 7 }]);
    assert.ok(button(entry.container, '正在重排…')?.disabled);
    await act(async () => {
      request.resolve({ requeued: 3, bound_pending: 1, bindable_retry: 1, local_only: 1, remaining_expired: 0 });
    });
    await waitFor(() => useAppStore.getState().toasts.some((toast) => toast.text.includes('待推送 1，等待身份 1，仅本机 1')), { label: '恢复分类结果可见' });
    assert.ok(useAppStore.getState().toasts.some((toast) => toast.text.includes('将在下次同步时重试')));
  } finally { resetPlan(); }
});

componentTest('同步恢复：换号后的迟到恢复不污染新账号，staleAccount失败不自动重放', { timeout: 40000 }, async () => {
  setup();
  const request = deferred();
  setPlan({ sync_retry_expired: () => request.promise });
  try {
    const { createElement, act } = await import('react');
    const { SyncTab } = await import('../../src/components/settings/SyncTab.tsx');
    const entry = await mount(createElement(SyncTab));
    await waitFor(() => button(entry.container, '重试过期项')?.disabled === false, { label: '旧账号摘要就绪' });
    await act(async () => { button(entry.container, '重试过期项').click(); });
    setPlan({ sync_status: () => Promise.resolve({ ...STATUS, account: 'bob' }), sync_queue_stats: () => Promise.resolve({ ...EMPTY, account_generation: 8, expired_unconfirmed: 2 }) });
    await act(async () => { useAppStore.getState().applySyncStatus({ ...STATUS, account: 'bob' }); });
    await flush(6);
    await act(async () => { request.reject({ code: 'staleAccount', message: '账号已变更' }); });
    await flush(5);
    assert.equal(invokeCalls.filter((item) => item.cmd === 'sync_retry_expired').length, 1, '拒绝后不得把旧操作自动迁给新账号');
    assert.ok(!useAppStore.getState().toasts.some((toast) => toast.text.includes('已重新排队') || toast.text.includes('账号已变更')));
    setPlan({ sync_retry_expired: () => Promise.reject({ code: 'staleAccount', message: '请刷新后重试' }) });
    await waitFor(() => button(entry.container, '重试过期项')?.disabled === false, { label: '新账号摘要就绪' });
    await act(async () => { button(entry.container, '重试过期项').click(); });
    await waitFor(() => useAppStore.getState().toasts.some((toast) => toast.text.includes('重试过期项失败')), { label: '当前账号恢复失败可见' });
    const calls = invokeCalls.filter((item) => item.cmd === 'sync_retry_expired');
    assert.deepEqual(calls.map((item) => item.args.expectedAccountGeneration), [7, 8]);
    assert.equal(calls.length, 2);
  } finally { resetPlan(); }
});

componentTest('同步恢复：旧账号补刷摘要迟到失败不能禁用新账号恢复入口', { timeout: 40000 }, async () => {
  setup();
  const retry = deferred();
  const oldStats = deferred();
  const bobStatus = { ...STATUS, account: 'bob', endpoint: 'https://b.example' };
  let phase = 'alice';
  let aliceFinalStats = 0;
  setPlan({
    sync_status: () => Promise.resolve(phase === 'bob' ? bobStatus : STATUS),
    sync_queue_stats: () => {
      if (phase === 'alice-final') {
        aliceFinalStats += 1;
        return oldStats.promise;
      }
      return Promise.resolve({
        ...EMPTY,
        account_generation: phase === 'bob' ? 8 : 7,
        expired_unconfirmed: phase === 'bob' ? 2 : 3,
      });
    },
    sync_retry_expired: () => retry.promise,
  });
  try {
    const { createElement, act } = await import('react');
    const { SyncTab } = await import('../../src/components/settings/SyncTab.tsx');
    const entry = await mount(createElement(SyncTab));
    await waitFor(() => button(entry.container, '重试过期项')?.disabled === false, { label: 'Alice 摘要 generation 7 就绪' });
    await act(async () => { button(entry.container, '重试过期项').click(); });
    phase = 'alice-final';
    await act(async () => {
      retry.resolve({ requeued: 3, bound_pending: 1, bindable_retry: 1, local_only: 1, remaining_expired: 0 });
    });
    await waitFor(() => aliceFinalStats >= 2, { label: 'Alice 恢复后的 store 与局部摘要都已发出' });
    phase = 'bob';
    await act(async () => { useAppStore.getState().applySyncStatus(bobStatus); });
    await flush(6);
    await waitFor(() => button(entry.container, '重试过期项')?.disabled === false, { label: 'Bob 摘要 generation 8 使恢复入口可用' });
    await act(async () => { oldStats.reject({ code: 'database', message: 'Alice 旧摘要失败' }); });
    await flush(6);
    assert.equal(useAppStore.getState().syncAccount, 'bob');
    assert.equal(button(entry.container, '重试过期项')?.disabled, false, '旧 Alice 摘要失败不能清空 Bob 已成功的快照');
    assert.deepEqual(invokeCalls.filter((call) => call.cmd === 'sync_retry_expired').map((call) => call.args.expectedAccountGeneration), [7], '旧失败不得自动重放恢复操作');
    setPlan({ sync_retry_expired: () => Promise.resolve({ requeued: 0, bound_pending: 0, bindable_retry: 0, local_only: 0, remaining_expired: 2 }) });
    await act(async () => { button(entry.container, '重试过期项').click(); });
    assert.deepEqual(invokeCalls.filter((call) => call.cmd === 'sync_retry_expired').map((call) => call.args.expectedAccountGeneration), [7, 8], 'Bob 主动重试必须使用新摘要的 generation');
  } finally { resetPlan(); }
});

componentTest('同步恢复：换号和断开明确放弃未确认队列，直连值保留不承诺自动补推', { timeout: 40000 }, async () => {
  setup({ ...EMPTY, account_generation: 7 });
  try {
    const { createElement, act } = await import('react');
    const { SyncTab } = await import('../../src/components/settings/SyncTab.tsx');
    const entry = await mount(createElement(SyncTab));
    await waitFor(() => [...entry.container.querySelectorAll('input.setting-input')][0]?.value === STATUS.endpoint, { label: '当前连接回填' });
    const inputs = [...entry.container.querySelectorAll('input.setting-input')].slice(0, 3);
    await setInput(inputs[0], 'https://b.example');
    await setInput(inputs[1], 'bob');
    await setInput(inputs[2], 'new-secret');
    await act(async () => { button(entry.container, '保存并同步').click(); });
    await waitFor(() => button(document.body, '更换并清理'), { label: '换号后果确认' });
    assert.ok(document.body.textContent.includes('放弃尚未确认的已读/收藏同步队列'));
    assert.ok(document.body.textContent.includes('旧意图不会迁移或自动补推到新账户'));
    assert.equal(invokeCalls.filter((item) => item.cmd === 'sync_save').length, 0);
    const dialog = [...document.body.querySelectorAll('[role="dialog"]')].find((item) => item.textContent.includes('更换同步账户'));
    await act(async () => { button(dialog, '取消').click(); });
    await act(async () => { button(entry.container, '断开连接').click(); });
    await waitFor(() => button(document.body, '断开并清理'), { label: '断开后果确认' });
    assert.ok(document.body.textContent.includes('旧意图不会在重连后自动补推'));
    assert.ok(document.body.textContent.includes('直连订阅及其已读/收藏值保留'));
    assert.equal(invokeCalls.filter((item) => item.cmd === 'sync_disconnect').length, 0);
  } finally { resetPlan(); }
});

componentTest('同步恢复：保存后单次sync_run保留partial错误与阶段重试，不重放保存或覆盖新草稿', { timeout: 40000 }, async () => {
  setup({ ...EMPTY, account_generation: 7 });
  let runs = 0;
  const report = { pushed_states: 0, pushed_feeds: 0, pulled_feeds: 0, pulled_entries: 0, merged_states: 0, errors: [] };
  setPlan({
    sync_save: () => Promise.resolve(JSON.stringify({ message: '已保存', firstConnect: false, unboundLocalFeeds: 0 })),
    sync_run: () => {
      runs += 1;
      return Promise.resolve({ activity_id: runs, feeds: { ...report, errors: ['订阅部分失败'] }, states: report, error: null });
    },
    refresh_all_feeds: () => Promise.resolve({ new_articles: 0, failed_feeds: 0 }),
  });
  try {
    const { createElement, act } = await import('react');
    const { SyncTab } = await import('../../src/components/settings/SyncTab.tsx');
    const entry = await mount(createElement(SyncTab));
    await waitFor(() => [...entry.container.querySelectorAll('input.setting-input')][0]?.value === STATUS.endpoint, { label: '当前表单就绪' });
    const inputs = [...entry.container.querySelectorAll('input.setting-input')].slice(0, 3);
    await setInput(inputs[2], 'secret');
    await act(async () => { button(entry.container, '保存并同步').click(); });
    await waitFor(() => useAppStore.getState().errors.some((error) => error.key.startsWith('sync:save-and-sync')), { label: 'partial失败保留重试' });
    assert.equal(runs, 1);
    assert.equal(invokeCalls.filter((item) => item.cmd === 'sync_phase').length, 0);
    assert.ok(!useAppStore.getState().toasts.some((toast) => toast.text.includes('账户同步完成')));
    const retry = useAppStore.getState().errors.find((error) => error.key.startsWith('sync:save-and-sync'));
    await setInput(inputs[0], 'https://b.example');
    await setInput(inputs[1], 'bob');
    await setInput(inputs[2], 'new-secret');
    await act(async () => { useAppStore.getState().retryError(retry.id); });
    await waitFor(() => runs === 2 && useAppStore.getState().saveAndSyncBusyOwner === null, { label: '阶段重试收尾' });
    assert.equal(invokeCalls.filter((item) => item.cmd === 'sync_save').length, 1);
    assert.deepEqual(inputs.map((input) => input.value), ['https://b.example', 'bob', 'new-secret']);
  } finally { resetPlan(); }
});
