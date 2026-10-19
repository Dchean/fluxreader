// R1 反例探针：登记了例外但预期错误**未出现**（未消费）——必须是失败，
// 防止用宽泛/失效的例外把整类错误全屏蔽（「不能全屏蔽」的机械保障）。
import { after } from 'node:test';
import { armLifecycleWatchdog, disarmLifecycleWatchdog, runFinalTeardown, componentTest, allowConsoleError } from '../harness.mjs';

armLifecycleWatchdog();

componentTest('未消费例外必须判红', { timeout: 10000 }, () => {
  allowConsoleError(/never appears/);
});

after(async () => {
  try {
    await runFinalTeardown();
  } finally {
    disarmLifecycleWatchdog();
  }
});
