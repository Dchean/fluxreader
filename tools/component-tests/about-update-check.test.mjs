// tools/component-tests/about-update-check.test.mjs
// OPT-018 A-1：更新检查「实际入口」的真实挂载回归。
//
// 取证边界：不触网。`check_for_updates` 是受控 IPC（Rust 侧固定目的地 HTTP 的
// 本地单测已由 OPT-014 覆盖），本文件只证明生产组件 AboutTab 的真实点击 →
// api.checkForUpdates → 三种结果文案与错误账本行为；@tauri-apps/api/app 的
// getVersion 由 tauri-compat-loader 桩为可比版本（0.17.1）。
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { componentTest, mount, flush, waitFor, resetStore, useAppStore } from './harness.mjs';
import { installBackend, clearInvokeCalls, setPlan, resetPlan, invokeCount } from './fixtures.mjs';

/* 本地版本以 package.json 为准，远端「有更新」版本按 minor 递增派生：
   版本 bump 后两条用例的前提仍成立（不再写死 0.17.1 / 0.18.0）。 */
const { version: LOCAL_VERSION } = JSON.parse(
  readFileSync(new URL('../../package.json', import.meta.url), 'utf8'),
);
function nextMinor(version) {
  const [major, minor] = version.split('.').map(Number);
  return `${major}.${minor + 1}.0`;
}
const REMOTE_VERSION = nextMinor(LOCAL_VERSION);
const REMOTE_URL = `https://github.com/Dchean/fluxreader/releases/tag/v${REMOTE_VERSION}`;

async function mountAbout() {
  const { createElement } = await import('react');
  const { AboutTab } = await import('../../src/components/settings/AboutTab.tsx');
  const entry = await mount(createElement(AboutTab));
  await waitFor(() => entry.container.textContent.includes(`FluxReader v${LOCAL_VERSION}`), {
    label: '本地版本经 getVersion 就绪',
  });
  return entry;
}

async function clickCheck(entry) {
  const { act } = await import('react');
  const btn = entry.container.querySelector('.about-update-btn');
  assert.ok(btn, '「检查更新」按钮必须真实渲染');
  assert.ok(btn.textContent.includes('检查更新'), `初始按钮文案应为检查更新：${btn.textContent}`);
  await act(async () => { btn.click(); });
  await flush(4);
}

componentTest('OPT-018 A-1 AboutTab：有可用版本时真实调用 IPC 并显示下载入口', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  setPlan({
    check_for_updates: () => Promise.resolve({ version: REMOTE_VERSION, url: REMOTE_URL }),
  });
  try {
    const entry = await mountAbout();
    await clickCheck(entry);

    assert.equal(invokeCount('check_for_updates'), 1, '点击必须真实调用一次 check_for_updates');
    const btn = entry.container.querySelector('.about-update-btn');
    assert.ok(btn, '可用版本必须显示下载按钮');
    assert.ok(btn.textContent.includes(`v${REMOTE_VERSION} 可用`), `必须显示远端版本：${btn.textContent}`);
    assert.ok(btn.textContent.includes('前往下载'), `必须给出下载入口：${btn.textContent}`);
    assert.equal(useAppStore.getState().errors.length, 0, '成功路径不得写入错误中心');
  } finally {
    resetPlan();
  }
});

componentTest('OPT-018 A-1 AboutTab：远端与本地同版本时显示「已是最新版本」', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  setPlan({
    check_for_updates: () => Promise.resolve({ version: LOCAL_VERSION, url: REMOTE_URL }),
  });
  try {
    const entry = await mountAbout();
    await clickCheck(entry);

    assert.equal(invokeCount('check_for_updates'), 1, '点击必须真实调用一次 check_for_updates');
    assert.ok(entry.container.textContent.includes('已是最新版本'),
      `同版本必须显示已是最新：${entry.container.textContent}`);
    assert.ok(!entry.container.textContent.includes('检查失败'), '同版本不得显示失败');
    assert.equal(useAppStore.getState().errors.length, 0, '成功路径不得写入错误中心');
  } finally {
    resetPlan();
  }
});

componentTest('OPT-018 A-1 AboutTab：检查失败显示失败文案与 toast，且不污染错误账本', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  setPlan({
    check_for_updates: () => Promise.reject(new Error('受控网络失败')),
  });
  try {
    const entry = await mountAbout();
    await clickCheck(entry);

    assert.equal(invokeCount('check_for_updates'), 1, '点击必须真实调用一次 check_for_updates');
    assert.ok(entry.container.textContent.includes('检查失败'),
      `失败必须显示可见文案：${entry.container.textContent}`);
    const toasts = useAppStore.getState().toasts;
    assert.ok(
      toasts.some((t) => t.text.includes('检查更新失败') && t.text.includes('受控网络失败')),
      `失败必须给出可读 toast：${JSON.stringify(toasts)}`,
    );
    assert.equal(useAppStore.getState().errors.length, 0,
      '更新检查失败是普通提示，不得写入错误中心（错误账本保持干净）');
    // 组件用例的 harness 终检同时兜住 console.error / jsdom / React 三通道；
    // 这里再显式断言没有把失败升级成未捕获异常。
    assert.equal(entry.records.uncaught.length + entry.records.caught.length + entry.records.recoverable.length, 0,
      '失败路径不得产生未捕获/可捕获 React 错误');
  } finally {
    resetPlan();
  }
});
