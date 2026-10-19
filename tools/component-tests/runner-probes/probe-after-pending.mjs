// R1 反例探针：after 钩子**永不 settle**。
// 旧实现（unref 看门狗）实测本探针 exit 0（5pass 假绿）；ref 生命周期看门狗
// 必须让本探针非 0 退出（exit 3）——由 runner-selfcheck.test.mjs 黑盒断言。
import { after } from 'node:test';
import { armLifecycleWatchdog, componentTest } from '../harness.mjs';

armLifecycleWatchdog(); // 时长取 COMPONENT_WATCHDOG_MS（自检用例注入 1500ms）

componentTest('快速通过用例', { timeout: 5000 }, () => { /* pass */ });

after(() => new Promise(() => {})); // 永不 settle：收尾永不发生
