// R1 反例探针：用例 body **永不 settle**（且无 node:test timeout 兜底）。
// ref 生命周期看门狗必须强杀（exit 3）——不允许事件循环空转成自然退出。
import { after } from 'node:test';
import { armLifecycleWatchdog, disarmLifecycleWatchdog, runFinalTeardown, componentTest } from '../harness.mjs';

armLifecycleWatchdog();

componentTest('永不 settle 的用例', async () => {
  await new Promise(() => {});
});

after(async () => {
  try {
    await runFinalTeardown();
  } finally {
    disarmLifecycleWatchdog();
  }
});
