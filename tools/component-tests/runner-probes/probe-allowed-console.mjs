// R1 对照探针：显式精确例外应被**消费**（预期错误出现且被登记），用例照常绿。
// 证明账本不是「一律判红」，而是「未预期判红、预期用精确例外」。
import { after } from 'node:test';
import { armLifecycleWatchdog, disarmLifecycleWatchdog, runFinalTeardown, componentTest, allowConsoleError } from '../harness.mjs';

armLifecycleWatchdog();

componentTest('显式例外消费后应通过', { timeout: 10000 }, () => {
  allowConsoleError(/probe expected boom/);
  console.error('probe expected boom');
});

after(async () => {
  try {
    await runFinalTeardown();
  } finally {
    disarmLifecycleWatchdog();
  }
});
