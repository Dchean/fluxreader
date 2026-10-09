// R1 反例探针：真实 DOM listener 抛错（jsdom 以 jsdomError 记录 Uncaught）。
// 旧实现只记录不参与失败 → exit 0；修复后统一错误账本必须判红（exit 1）。
import { after } from 'node:test';
import { armLifecycleWatchdog, disarmLifecycleWatchdog, runFinalTeardown, componentTest } from '../harness.mjs';

armLifecycleWatchdog();

componentTest('DOM listener 抛错必须判红', { timeout: 10000 }, () => {
  const el = document.createElement('button');
  document.body.appendChild(el);
  el.addEventListener('click', () => { throw new Error('probe listener boom'); });
  el.click();
  el.remove();
});

after(async () => {
  try {
    await runFinalTeardown();
  } finally {
    disarmLifecycleWatchdog();
  }
});
