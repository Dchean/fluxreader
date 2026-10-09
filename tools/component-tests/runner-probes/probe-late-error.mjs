// R2 反例探针：finalcheck 之后才到达的异步运行时错误（假绿）。
//
// 真实形态：真实 React 组件在 effect cleanup（卸载期）里 setTimeout 100ms 后
// console.error——componentTest 的终检、runFinalTeardown 的终检都已跑完，
// 该错误才到达。修前：console 采集器只记录、退出码 0（假绿）；修后：终态
// runtime error 守卫必须立即置非 0 并打印清晰信息（exit 1）。
//
// 不 sleep、不无限等：探针自身不做任何等待，让真实定时器按 100ms 自然触发。
import { after } from 'node:test';
import { armLifecycleWatchdog, disarmLifecycleWatchdog, runFinalTeardown, componentTest, mount } from '../harness.mjs';

armLifecycleWatchdog();

const { createElement, useEffect } = await import('react');

function LateBoom() {
  useEffect(() => () => {
    /* 卸载期安排的晚异步错误：落在 finalcheck 之后 */
    setTimeout(() => { console.error('probe late runtime boom'); }, 100);
  }, []);
  return createElement('div', null, 'late');
}

componentTest('用例本体通过，finalcheck 后 100ms 才有异步 console.error', { timeout: 10000 }, async () => {
  await mount(createElement(LateBoom));
});

after(async () => {
  try {
    await runFinalTeardown();
  } finally {
    disarmLifecycleWatchdog();
  }
});
