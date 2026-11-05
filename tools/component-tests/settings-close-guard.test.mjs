// GitHub 设备授权期间设置弹窗关闭守卫：真实 App 挂载 + 真实 DOM 事件。
//
// 覆盖三条审计路径：
// 1. 授权等待中：Sync 页真实渲染 user_code；X/遮罩/Esc/Ctrl+, 全部被守卫；
//    「取消授权」停止本机等待后四个入口恢复关闭。
// 2. github_login_start 尚未返回、githubFlow 仍为 null：关闭不得从发起中状态
//    穿透；请求完成后进入等待卡，授权结束后仍可关。
// 3. openExternal 拒绝：清理 flow/发起中状态、无孤儿轮询，并允许重新发起或关闭。

import assert from 'node:assert/strict';
import {
  componentTest, mount, flush, waitFor, resetStore, resetGithubLoginForTests,
  useAppStore, storeMod,
} from './harness.mjs';
import {
  installBackend, clearInvokeCalls, setPlan, resetPlan, deferred,
  invokeCount,
} from './fixtures.mjs';

const AUTH_FLOW = {
  user_code: 'WDJB-MJHT',
  verification_uri: 'https://github.com/login/device',
  interval: 5,
};
const POLL_FLOW = { ...AUTH_FLOW, interval: 3 };
const GUARD_TOAST = 'GitHub 授权进行中，等待网页授权完成后再关闭';
const CANCEL_TOAST = '已停止本机等待；网页上的 GitHub 授权不会被撤销';

function pressKeyOnFocused(key, init = {}) {
  const target = document.activeElement;
  assert.ok(
    target && target !== document.body,
    `键盘事件必须派发到实际焦点元素，当前 ${target?.tagName ?? 'null'}`,
  );
  const ev = new KeyboardEvent('keydown', {
    key, bubbles: true, cancelable: true, ...init,
  });
  target.dispatchEvent(ev);
  assert.equal(ev.target, target, '键盘事件目标应为实际焦点元素');
  return ev;
}

function findButton(root, text) {
  return [...root.querySelectorAll('button')].find((b) => b.textContent.trim() === text);
}

function focusSettingsControl(entry) {
  const target = entry.container.querySelector('.settings-nav-item');
  assert.ok(target, '设置页导航控件存在');
  target.focus();
  assert.equal(document.activeElement, target, '前置：焦点在真实设置控件上');
  return target;
}

async function clearToasts(act) {
  await act(async () => {
    storeMod.clearToastTimers();
    useAppStore.setState({ toasts: [] });
  });
}

async function openSettings(act, entry, {
  githubFlow = AUTH_FLOW,
  githubLoggingIn = false,
  githubLoginStartPending = false,
  githubPollPending = false,
} = {}) {
  await act(async () => {
    storeMod.clearToastTimers();
    useAppStore.setState({
      settingsOpen: true,
      settingsTab: 'sync',
      githubFlow,
      githubLoggingIn,
      githubLoginStartPending,
      githubPollPending,
      toasts: [],
    });
  });
  await flush(3);
  assert.ok(
    entry.container.querySelector('.modal-overlay.open .settings-modal'),
    '前置：设置弹窗真实打开',
  );
}

function assertBlocked(entry, label, {
  githubFlow = AUTH_FLOW,
  githubLoggingIn = false,
  githubLoginStartPending = false,
  githubPollPending = false,
} = {}) {
  const state = useAppStore.getState();
  assert.equal(state.settingsOpen, true, `${label}：授权中 settingsOpen 必须保持 true`);
  assert.deepEqual(state.githubFlow, githubFlow, `${label}：githubFlow 必须原样保留`);
  assert.equal(state.githubLoggingIn, githubLoggingIn, `${label}：githubLoggingIn 必须保持原值`);
  assert.equal(
    state.githubLoginStartPending,
    githubLoginStartPending,
    `${label}：githubLoginStartPending 必须保持原值`,
  );
  assert.equal(
    state.githubPollPending,
    githubPollPending,
    `${label}：githubPollPending 必须保持原值`,
  );
  assert.ok(
    [...entry.container.querySelectorAll('.toast-text')].some((el) => el.textContent === GUARD_TOAST),
    `${label}：必须显示统一授权中提示`,
  );
  assert.equal(state.toasts.at(-1)?.text, GUARD_TOAST, `${label}：store toast 与 DOM 同文案`);
}

async function triggerClose(entry, which, act) {
  if (which === 'backdrop') {
    const overlay = entry.container.querySelector('.modal-overlay.open');
    assert.ok(overlay, '前置：遮罩存在');
    await act(async () => { overlay.click(); });
  } else if (which === 'x') {
    const closeX = entry.container.querySelector('.settings-pane-header-right .win-btn.close');
    assert.ok(closeX, '设置页右上 X 存在');
    await act(async () => { closeX.click(); });
  } else {
    focusSettingsControl(entry);
    await act(async () => {
      pressKeyOnFocused(which === 'escape' ? 'Escape' : ',', which === 'ctrl' ? { ctrlKey: true } : {});
    });
  }
  await flush(2);
}

const CLOSE_ENTRIES = [
  ['backdrop', '遮罩点击'],
  ['x', '右上 X'],
  ['escape', 'Escape'],
  ['ctrl', 'Ctrl+,'],
];

async function mountApp() {
  const { createElement, act } = await import('react');
  const { default: App } = await import('../../src/App.tsx');
  const entry = await mount(createElement(App));
  await waitFor(() => entry.container.querySelector('.app-root'), {
    label: 'App 挂载完成',
    timeout: 8000,
  });
  return { entry, act };
}

componentTest('设置关闭守卫：授权等待卡、四入口、取消授权后恢复关闭', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetPlan();
  resetStore();
  const { entry, act } = await mountApp();

  await openSettings(act, entry);
  const code = entry.container.querySelector('.device-code-value');
  assert.equal(code?.textContent, AUTH_FLOW.user_code, 'Sync 页真实渲染设备授权码');

  for (const [which, label] of CLOSE_ENTRIES) {
    await clearToasts(act);
    await triggerClose(entry, which, act);
    assertBlocked(entry, label);
  }

  const cancel = findButton(entry.container.querySelector('.settings-modal'), '取消授权');
  assert.ok(cancel, '等待卡提供「取消授权」入口');
  await act(async () => { cancel.click(); });
  await flush(3);
  assert.equal(invokeCount('github_login_cancel'), 1, '取消授权真实调用后端 cancel');
  const cancelled = useAppStore.getState();
  assert.equal(cancelled.settingsOpen, true, '取消授权不得顺手关闭设置');
  assert.equal(cancelled.githubFlow, null, '取消授权清理 flow');
  assert.equal(cancelled.githubLoggingIn, false, '取消授权清理发起中状态');
  assert.equal(entry.container.querySelector('.device-code-value'), null, '等待卡应从真实 DOM 消失');
  assert.equal(cancelled.toasts.at(-1)?.text, CANCEL_TOAST, '取消授权说明不会撤销网页授权');

  for (const [which, label] of CLOSE_ENTRIES) {
    await openSettings(act, entry, { githubFlow: null });
    await triggerClose(entry, which, act);
    assert.equal(useAppStore.getState().settingsOpen, false, `${label}：取消后恢复关闭`);
    assert.equal(useAppStore.getState().toasts.length, 0, `${label}：正常关闭不显示授权提示`);
  }
});

componentTest('设置关闭守卫：github_login_start pending 不穿透，完成后仍可关', { timeout: 40000 }, async (t) => {
  installBackend();
  clearInvokeCalls();
  resetPlan();
  resetStore();
  t.after(() => { resetGithubLoginForTests(); resetPlan(); });

  const firstGate = deferred();
  const secondGate = deferred();
  let startCalls = 0;
  setPlan({
    github_login_start: () => (++startCalls === 1 ? firstGate.promise : secondGate.promise),
  });
  const { entry, act } = await mountApp();
  await openSettings(act, entry, { githubFlow: null });

  const login = findButton(entry.container.querySelector('.settings-modal'), '登录 GitHub');
  assert.ok(login, '未授权时提供 GitHub 登录按钮');
  await act(async () => { login.click(); });
  await flush(3);
  let state = useAppStore.getState();
  assert.equal(state.githubLoggingIn, true, '发起请求在途：githubLoggingIn=true');
  assert.equal(state.githubLoginStartPending, true, '底层 start IPC 在途标记为 true');
  assert.equal(state.githubFlow, null, '发起请求尚未返回：flow 仍为 null');
  assert.equal(invokeCount('github_login_start'), 1, '登录按钮发出一次真实 IPC');

  for (const [which, label] of CLOSE_ENTRIES) {
    await clearToasts(act);
    await triggerClose(entry, which, act);
    assertBlocked(entry, `${label}（start pending）`, {
      githubFlow: null,
      githubLoggingIn: true,
      githubLoginStartPending: true,
    });
  }

  const cancelPending = findButton(entry.container.querySelector('.settings-modal'), '取消发起');
  assert.ok(cancelPending, '发起中状态提供「取消发起」入口');
  await act(async () => { cancelPending.click(); });
  await flush(2);
  state = useAppStore.getState();
  assert.equal(state.githubFlow, null, '取消发起不得留下 flow');
  assert.equal(state.githubLoggingIn, false, '取消发起清理发起中状态');
  assert.equal(state.githubLoginStartPending, true, '取消后旧 start IPC 仍在途');
  assert.equal(state.toasts.at(-1)?.text, CANCEL_TOAST, '取消发起说明不会撤销网页授权');

  const waitingRetry = findButton(
    entry.container.querySelector('.settings-modal'),
    '等待上次发起结束…',
  );
  assert.ok(waitingRetry, '旧 start 未结算时显示等待状态');
  assert.equal(waitingRetry.disabled, true, '旧 start 未结算时重试按钮禁用');
  await act(async () => { waitingRetry.click(); });
  await flush(2);
  assert.equal(invokeCount('github_login_start'), 1, '禁用期间点击不得发出第二次 IPC');

  await triggerClose(entry, 'x', act);
  assert.equal(useAppStore.getState().settingsOpen, false, '取消发起后 X 恢复关闭');

  await act(async () => {
    firstGate.resolve(AUTH_FLOW);
    await firstGate.promise;
  });
  await flush(3);
  state = useAppStore.getState();
  assert.equal(state.githubFlow, null, '取消后迟到的 start 回包不得重写 flow');
  assert.equal(state.githubLoggingIn, false, '取消后迟到回包不得恢复发起中状态');
  assert.equal(state.githubLoginStartPending, false, '旧 start settle 后清理在途标记');

  await openSettings(act, entry, { githubFlow: null });
  const retryLogin = findButton(entry.container.querySelector('.settings-modal'), '登录 GitHub');
  assert.ok(retryLogin, '取消后允许再次发起');
  await act(async () => { retryLogin.click(); });
  await flush(2);
  state = useAppStore.getState();
  assert.equal(state.githubLoggingIn, true, '第二次发起进入在途状态');
  assert.equal(state.githubLoginStartPending, true, '第二次 start IPC 在途');
  assert.equal(invokeCount('github_login_start'), 2, '取消后重试真实发出第二次 IPC');

  await act(async () => {
    secondGate.resolve(AUTH_FLOW);
    await secondGate.promise;
  });
  await waitFor(() => useAppStore.getState().githubFlow?.user_code === AUTH_FLOW.user_code, {
    label: 'pending start 完成后写入 flow',
    timeout: 6000,
  });
  await flush(3);
  state = useAppStore.getState();
  assert.equal(state.githubLoggingIn, false, 'start 完成后清理发起中状态');
  assert.equal(state.githubLoginStartPending, false, 'start 完成后清理底层在途标记');
  assert.equal(entry.container.querySelector('.device-code-value')?.textContent, AUTH_FLOW.user_code,
    'start 完成后真实渲染等待卡');

  await clearToasts(act);
  await triggerClose(entry, 'x', act);
  assertBlocked(entry, 'start 完成后授权仍进行', {
    githubFlow: AUTH_FLOW,
    githubLoggingIn: false,
    githubLoginStartPending: false,
  });

  resetGithubLoginForTests();
  await act(async () => { useAppStore.setState({ githubFlow: null }); });
  await clearToasts(act);
  await triggerClose(entry, 'x', act);
  assert.equal(useAppStore.getState().settingsOpen, false, '授权结束后 X 恢复关闭');
});

componentTest('GitHub 发起路径：openExternal 拒绝清理锁并可重试/关闭', { timeout: 40000 }, async (t) => {
  installBackend();
  clearInvokeCalls();
  resetPlan();
  resetStore();
  t.after(() => {
    globalThis.__OPEN_URL_ERROR__ = undefined;
    resetGithubLoginForTests();
    resetPlan();
  });

  setPlan({ github_login_start: () => Promise.resolve(AUTH_FLOW) });
  const { entry, act } = await mountApp();
  await openSettings(act, entry, { githubFlow: null });

  globalThis.__OPEN_URL_ERROR__ = new Error('opener down');
  let login = findButton(entry.container.querySelector('.settings-modal'), '登录 GitHub');
  assert.ok(login, '登录入口存在');
  await act(async () => { login.click(); });
  await flush(5);

  let state = useAppStore.getState();
  assert.equal(state.githubFlow, null, 'openExternal 拒绝不得留下 githubFlow');
  assert.equal(state.githubLoggingIn, false, 'openExternal 拒绝清理发起中状态');
  assert.equal(state.githubLoginStartPending, false, 'openExternal 拒绝清理底层在途标记');
  assert.equal(invokeCount('github_login_start'), 1, '拒绝路径已真实发起一次登录');
  assert.equal(invokeCount('github_login_poll'), 0, '拒绝路径不得留下孤儿轮询');
  assert.ok(
    state.toasts.some((toast) => toast.text.includes('发起登录失败：opener down')),
    '拒绝路径给出可见失败提示',
  );

  await triggerClose(entry, 'x', act);
  assert.equal(useAppStore.getState().settingsOpen, false, '失败后右上 X 可关闭（锁已释放）');

  await openSettings(act, entry, { githubFlow: null });
  globalThis.__OPEN_URL_ERROR__ = undefined;
  login = findButton(entry.container.querySelector('.settings-modal'), '登录 GitHub');
  assert.ok(login, '失败后登录入口恢复可用');
  await act(async () => { login.click(); });
  await waitFor(() => useAppStore.getState().githubFlow?.user_code === AUTH_FLOW.user_code, {
    label: '重试登录成功写入 flow',
    timeout: 6000,
  });
  await flush(2);
  assert.equal(invokeCount('github_login_start'), 2, '失败后允许再次发起登录');
  assert.equal(useAppStore.getState().githubLoggingIn, false, '重试完成后清理发起中状态');
  assert.equal(useAppStore.getState().githubLoginStartPending, false, '重试完成后清理底层在途标记');

  const cancel = findButton(entry.container.querySelector('.settings-modal'), '取消授权');
  assert.ok(cancel, '重试成功后提供取消入口');
  await act(async () => { cancel.click(); });
  await flush(2);
  assert.equal(useAppStore.getState().githubFlow, null, '重试成功后取消清理 flow');
});

componentTest('GitHub 轮询竞态：取消后旧 poll 未结算不得重发，迟到结果不复活 flow/account', { timeout: 40000 }, async (t) => {
  installBackend();
  clearInvokeCalls();
  resetPlan();
  resetStore();
  t.after(() => { resetGithubLoginForTests(); resetPlan(); });

  const pollGate = deferred();
  let startCalls = 0;
  setPlan({
    github_login_start: () => {
      startCalls += 1;
      return Promise.resolve(POLL_FLOW);
    },
    github_login_poll: () => pollGate.promise,
  });
  const { entry, act } = await mountApp();
  await openSettings(act, entry, { githubFlow: null });

  const login = findButton(entry.container.querySelector('.settings-modal'), '登录 GitHub');
  await act(async () => { login.click(); });
  await waitFor(() => useAppStore.getState().githubFlow?.user_code === POLL_FLOW.user_code, {
    label: '授权流进入轮询等待',
    timeout: 6000,
  });
  await waitFor(() => useAppStore.getState().githubPollPending === true, {
    label: 'github_login_poll 进入在途',
    timeout: 6000,
  });
  assert.equal(startCalls, 1, '首次发起只调用一次 start');
  assert.equal(invokeCount('github_login_poll'), 1, '真实轮询已发出');

  const cancel = findButton(entry.container.querySelector('.settings-modal'), '取消授权');
  assert.ok(cancel, '等待卡提供取消授权');
  await act(async () => { cancel.click(); });
  await flush(2);
  let state = useAppStore.getState();
  assert.equal(state.githubFlow, null, '取消后 flow 清空');
  assert.equal(state.githubLoggingIn, false, '取消后可关闭状态恢复');
  assert.equal(state.githubLoginStartPending, false, '取消不影响已结算的 start 标记');
  assert.equal(state.githubPollPending, true, '旧 poll 仍在途');

  const waitingPoll = findButton(
    entry.container.querySelector('.settings-modal'),
    '等待上次轮询结束…',
  );
  assert.ok(waitingPoll, '旧 poll 未结算时显示轮询等待态');
  assert.equal(waitingPoll.disabled, true, '旧 poll 未结算时禁止新 start');
  await act(async () => { waitingPoll.click(); });
  await flush(2);
  assert.equal(startCalls, 1, '禁用期间点击不得发出第二次 start');

  await triggerClose(entry, 'x', act);
  assert.equal(useAppStore.getState().settingsOpen, false, 'poll 在途不阻止设置关闭');

  await act(async () => {
    pollGate.resolve({ login: 'late-account' });
    await pollGate.promise;
  });
  await flush(3);
  state = useAppStore.getState();
  assert.equal(state.githubPollPending, false, '旧 poll settle 后清在途标记');
  assert.equal(state.githubFlow, null, '旧 poll 迟到结果不得复活 flow');
  assert.equal(state.githubAccount, null, '旧 poll 迟到账户不得落库');
  assert.equal(startCalls, 1, '旧 poll settle 前没有额外 start');

  await openSettings(act, entry, { githubFlow: null });
  const retry = findButton(entry.container.querySelector('.settings-modal'), '登录 GitHub');
  assert.ok(retry, '旧 poll settle 后登录入口恢复');
  assert.equal(retry.disabled, false, '旧 poll settle 后按钮可用');
  await act(async () => { retry.click(); });
  await flush(2);
  assert.equal(startCalls, 2, '旧 poll settle 后允许重新发起');
  await waitFor(() => useAppStore.getState().githubFlow?.user_code === POLL_FLOW.user_code, {
    label: '重试后进入新授权流',
    timeout: 6000,
  });

  const cancelRetry = findButton(entry.container.querySelector('.settings-modal'), '取消授权');
  await act(async () => { cancelRetry.click(); });
  await flush(2);
});

componentTest('GitHub 发起：取消后迟到的 WebDAV 拒绝不得弹确认或 force 重发', { timeout: 40000 }, async (t) => {
  installBackend();
  clearInvokeCalls();
  resetPlan();
  resetStore();

  let confirmCalls = 0;
  const originalConfirm = window.confirm;
  window.confirm = () => {
    confirmCalls += 1;
    return true;
  };
  const firstStart = deferred();
  const startArgs = [];
  setPlan({
    github_login_start: (args) => {
      startArgs.push(args);
      if (args?.force === true) return Promise.resolve(AUTH_FLOW);
      return firstStart.promise;
    },
  });
  t.after(() => {
    window.confirm = originalConfirm;
    resetGithubLoginForTests();
    resetPlan();
  });

  const { entry, act } = await mountApp();
  await openSettings(act, entry, { githubFlow: null });
  const login = findButton(entry.container.querySelector('.settings-modal'), '登录 GitHub');
  await act(async () => { login.click(); });
  await flush(2);
  assert.equal(useAppStore.getState().githubLoginStartPending, true, '首次 start 在途');

  const cancel = findButton(entry.container.querySelector('.settings-modal'), '取消发起');
  assert.ok(cancel, '发起中提供取消入口');
  await act(async () => { cancel.click(); });
  await flush(2);
  assert.equal(useAppStore.getState().githubLoggingIn, false, '取消后许可关闭状态');

  await act(async () => {
    firstStart.reject({ code: 'webdavConflict', message: 'WebDAV conflict: existing data' });
    await firstStart.promise.catch(() => {});
  });
  await flush(3);

  const state = useAppStore.getState();
  assert.equal(confirmCalls, 0, '取消后的迟到 WebDAV 拒绝不得弹确认');
  assert.equal(startArgs.length, 1, '不得发出 force=true 第二次 start');
  assert.equal(startArgs.some((args) => args?.force === true), false, '首次调用之外无 force 参数');
  assert.equal(state.githubFlow, null, '迟到拒绝不得留下 flow');
  assert.equal(state.githubLoggingIn, false, '迟到拒绝不得恢复登录中状态');
  assert.equal(state.githubLoginStartPending, false, '迟到拒绝结算后在途标记清理');
});

componentTest('GitHub 取消：后端失败可见，仍可关闭设置并重试', { timeout: 40000 }, async (t) => {
  installBackend();
  clearInvokeCalls();
  resetPlan();
  resetStore();
  t.after(() => { resetGithubLoginForTests(); resetPlan(); });

  const cancelGate = deferred();
  let startCalls = 0;
  setPlan({
    github_login_start: () => {
      startCalls += 1;
      return Promise.resolve(AUTH_FLOW);
    },
    github_login_cancel: () => cancelGate.promise,
  });
  const { entry, act } = await mountApp();
  await openSettings(act, entry, { githubFlow: null });

  const login = findButton(entry.container.querySelector('.settings-modal'), '登录 GitHub');
  assert.ok(login, '首次登录入口存在');
  await act(async () => { login.click(); });
  await waitFor(() => useAppStore.getState().githubFlow?.user_code === AUTH_FLOW.user_code, {
    label: '首次启动进入等待卡',
    timeout: 6000,
  });

  const cancel = findButton(entry.container.querySelector('.settings-modal'), '取消授权');
  assert.ok(cancel, '等待卡提供取消授权');
  await act(async () => { cancel.click(); });
  await flush(2);
  assert.equal(invokeCount('github_login_cancel'), 1, '取消按钮真实发出后端 cancel IPC');
  assert.equal(useAppStore.getState().githubLoginCancelPending, true, '后端 cancel 未结算时置 pending');
  const cancelling = findButton(entry.container.querySelector('.settings-modal'), '取消中…');
  assert.ok(cancelling, '取消中显示真实等待文案');
  assert.equal(cancelling.disabled, true, 'cancel pending 时禁止重复取消');

  await act(async () => {
    cancelGate.reject({ code: 'internal', message: 'cancel exploded' });
    await cancelGate.promise.catch(() => {});
  });
  await flush(3);
  let state = useAppStore.getState();
  assert.equal(state.githubLoginCancelPending, false, '失败结算后清除 cancel pending');
  assert.equal(state.githubFlow, null, '失败后停止本机展示，避免设置被永久锁住');
  assert.equal(state.githubLoggingIn, false, '失败后恢复可关闭状态');
  assert.ok(
    state.toasts.some((toast) => toast.text.includes('取消失败：cancel exploded')),
    '后端取消失败必须可见',
  );
  assert.equal(
    state.toasts.some((toast) => toast.text === CANCEL_TOAST),
    false,
    '后端取消失败不得显示取消成功',
  );

  await triggerClose(entry, 'x', act);
  assert.equal(useAppStore.getState().settingsOpen, false, '后端取消失败后仍可关闭设置');

  await openSettings(act, entry, { githubFlow: null });
  const retry = findButton(entry.container.querySelector('.settings-modal'), '登录 GitHub');
  assert.ok(retry, '失败后登录入口可重试');
  await act(async () => { retry.click(); });
  await waitFor(() => useAppStore.getState().githubFlow?.user_code === AUTH_FLOW.user_code, {
    label: '取消失败后重试进入新授权流',
    timeout: 6000,
  });
  assert.equal(startCalls, 2, '取消失败后真实重试第二次 start');

  await act(async () => {
    resetGithubLoginForTests();
    useAppStore.setState({ githubFlow: null });
  });
});
