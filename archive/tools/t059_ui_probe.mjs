// TASK-059 实机验证 · CDP 诊断：确认应用已起来、设置中心可打开、同步页可达、按钮可用。
// 用法：node tools/t059_ui_probe.mjs
// 依赖：应用以 WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222 启动，
//       且 debug 构建的 devUrl（Vite，5173）在运行。

import { connect, findPageTarget, waitFor, PAGE_HELPERS } from './t059_cdp.mjs';

async function main() {
  const target = await findPageTarget();
  const cdp = await connect(target.webSocketDebuggerUrl);
  await cdp.send('Runtime.enable');
  await cdp.send('Page.enable');
  // debug 构建的前端来自 Vite dev server：若应用先于 dev server 启动，
  // 首屏是失败页，必须显式重载一次才能拿到真实应用。
  await cdp.send('Page.reload', { ignoreCache: true });
  await waitFor(cdp, `!!document.querySelector('.nav-tab-item')`, { label: '应用主导航（重载后）' });
  await cdp.evaluate(PAGE_HELPERS);

  const info = await cdp.evaluate(`JSON.stringify({
    url: location.href,
    title: document.title,
    theme: document.documentElement.getAttribute('data-theme'),
    hasNav: !!document.querySelector('.nav-tab-item'),
  }, null, 2)`);
  console.log(info);

  console.log('click 设置中心 ->', await cdp.evaluate(`window.__t059.clickText('.nav-tab-item', '设置中心')`));
  await waitFor(cdp, `!!document.querySelector('.settings-modal')`, { label: '设置弹窗' });
  console.log('click 同步 ->', await cdp.evaluate(`window.__t059.clickText('.settings-nav-item', '同步')`));
  await waitFor(cdp, `!!window.__t059.endpointCard()`, { label: 'Endpoint 卡片' });
  console.log('endpoint card ->', await cdp.evaluate(`JSON.stringify(window.__t059.endpointCard(), null, 2)`));
  console.log('action buttons ->', await cdp.evaluate(`JSON.stringify(window.__t059.actionLabels())`));
  console.log('inputs in sync tab ->', await cdp.evaluate(`JSON.stringify([...document.querySelectorAll('.setting-card input')].map(i => ({ph: i.placeholder, val: i.value})))`));

  cdp.close();
}

main().catch((e) => {
  console.error('PROBE FAILED:', e.message);
  process.exit(1);
});
