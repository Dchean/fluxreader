// R1 反例探针：非预期 console.error（旧实现只查 getSnapshot 特定 regex → exit 0）。
// 修复后统一错误账本必须判红（exit 1）。
import { after } from 'node:test';
import { armLifecycleWatchdog, disarmLifecycleWatchdog, runFinalTeardown, componentTest } from '../harness.mjs';

armLifecycleWatchdog();

componentTest('未预期 console.error 必须判红', { timeout: 10000 }, () => {
  console.error('probe unexpected console boom');
});

after(async () => {
  try {
    await runFinalTeardown();
  } finally {
    disarmLifecycleWatchdog();
  }
});
