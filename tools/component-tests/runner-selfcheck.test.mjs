// OPT-016B R1：runner 自身防护的黑盒自检（childprocess 反向验证）。
//
// 为什么要有这组用例：防护本身也可能失效——「看门狗装了但进程照旧 0 退出」
// 「账本装了但判红条件写漏」。这里对每个 review 反例跑一个真实子进程探针
// （tools/component-tests/runner-probes/），以**退出码 + 输出关键字**断言防护
// 真的生效；探针全部使用真实 DOM/React 挂载或真实钩子悬挂，不是源字符串断言。
//
// 契约（与 R1/R2 review 一一对应）：
// - after 钩子 pending → exit 3（ref 看门狗强杀）
// - 用例 body pending → exit 3
// - 卸载期 cleanup 抛错 → 用例判红、退出码 1；其余树**继续清理**（marker 落盘）
// - 真实 DOM listener 抛错（jsdom Uncaught）→ exit 1，输出含真实错误消息
// - 非预期 console.error → exit 1，输出含真实错误消息
// - 显式精确例外且被消费 → exit 0（账本不是一律判红）
// - 例外未被消费（预期错误未出现）→ exit 1（不能全屏蔽）
// - R2：finalcheck 之后才到达的异步运行时错误（真实 effect cleanup setTimeout
//   晚发 console.error）→ 用例本体通过但终态守卫立即置非 0（exit 1 + 清晰诊断，
//   不 sleep、不等待、one-shot 不掩盖）

import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, rmSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { componentTest } from './harness.mjs';

const ROOT = fileURLToPath(new URL('../../', import.meta.url));
const PROBE_DIR = fileURLToPath(new URL('./runner-probes/', import.meta.url));
const MARKER = fileURLToPath(new URL('../../tmp/opt016b-runner-probes/good-cleaned.txt', import.meta.url));

/** 子进程黑盒运行探针：注入短看门狗，收集退出码与合并输出。 */
function runProbe(name) {
  const r = spawnSync(process.execPath, [PROBE_DIR + name], {
    cwd: ROOT,
    env: { ...process.env, COMPONENT_WATCHDOG_MS: '1500' },
    encoding: 'utf8',
    timeout: 30000,
  });
  return { status: r.status, signal: r.signal, output: `${r.stdout ?? ''}${r.stderr ?? ''}` };
}

componentTest('OPT-016B R1 自检：runner 防护（悬挂/清理/错误账本）在黑盒子进程中真实判红/判绿', { timeout: 60000 }, async () => {
  // 1) after 钩子 pending → 必须非 0（ref 看门狗 exit 3）
  const afterPending = runProbe('probe-after-pending.mjs');
  assert.equal(afterPending.status, 3,
    `after pending 探针必须 exit 3，实际 status=${afterPending.status} signal=${afterPending.signal}\n${afterPending.output}`);
  assert.ok(afterPending.output.includes('生命周期看门狗'),
    `exit 3 必须由生命周期看门狗产生（输出应含诊断），实际输出：\n${afterPending.output}`);

  // 2) 用例 body pending → 必须非 0（同上，watchdog 覆盖用例悬挂）
  const testHang = runProbe('probe-test-hang.mjs');
  assert.equal(testHang.status, 3,
    `body pending 探针必须 exit 3，实际 status=${testHang.status} signal=${testHang.signal}\n${testHang.output}`);
  assert.ok(testHang.output.includes('生命周期看门狗'), 'exit 3 必须由看门狗产生');

  // 3) 卸载期 cleanup 抛错 → 用例判红（exit 1），且其余树继续清理（marker 落盘）
  rmSync(MARKER, { force: true });
  const teardownErr = runProbe('probe-teardown-error.mjs');
  assert.equal(teardownErr.status, 1,
    `cleanup 抛错探针必须 exit 1，实际 status=${teardownErr.status}\n${teardownErr.output}`);
  assert.ok(teardownErr.output.includes('probe cleanup boom'),
    `失败输出必须携带真实 cleanup 错误，实际输出：\n${teardownErr.output}`);
  assert.ok(existsSync(MARKER),
    '毒树清理失败不得阻止其余树继续卸载（好树 cleanup 的 marker 必须落盘）');
  rmSync(MARKER, { force: true });

  // 4) 真实 DOM listener 抛错 → jsdom Uncaught 必须判红（exit 1，带真实消息）
  const domThrow = runProbe('probe-dom-listener-throw.mjs');
  assert.equal(domThrow.status, 1,
    `DOM listener 抛错探针必须 exit 1，实际 status=${domThrow.status}\n${domThrow.output}`);
  assert.ok(domThrow.output.includes('probe listener boom'),
    `判红输出必须携带真实 listener 错误，实际输出：\n${domThrow.output}`);

  // 5) 非预期 console.error → 判红（exit 1，带真实消息）
  const unexpected = runProbe('probe-unexpected-console.mjs');
  assert.equal(unexpected.status, 1,
    `非预期 console.error 探针必须 exit 1，实际 status=${unexpected.status}\n${unexpected.output}`);
  assert.ok(unexpected.output.includes('probe unexpected console boom'),
    `判红输出必须携带真实 console 错误，实际输出：\n${unexpected.output}`);

  // 6) 显式精确例外被消费 → 照常绿（账本不是一律判红）
  const allowed = runProbe('probe-allowed-console.mjs');
  assert.equal(allowed.status, 0,
    `预期错误 + 消费例外必须 exit 0，实际 status=${allowed.status}\n${allowed.output}`);
  assert.ok(allowed.output.includes('清理完成'), '正常路径应收尾成功（清理完成）');

  // 7) 例外未被消费 → 判红（不能全屏蔽）
  const stale = runProbe('probe-stale-allowance.mjs');
  assert.equal(stale.status, 1,
    `未消费例外探针必须 exit 1，实际 status=${stale.status}\n${stale.output}`);
  assert.ok(stale.output.includes('未消费例外'),
    `判红输出必须点明未消费例外，实际输出：\n${stale.output}`);

  // 8) R2：finalcheck 之后才到达的异步运行时错误（真实 React effect cleanup
  //    里 setTimeout 100ms 晚发 console.error）→ 终态守卫立即置非 0（不 sleep、
  //    不等待）；用例本体本身通过（pass 1）仍必须 exit 1 + 清晰诊断。
  const lateError = runProbe('probe-late-error.mjs');
  assert.equal(lateError.status, 1,
    `晚异步错误探针必须 exit 1，实际 status=${lateError.status}\n${lateError.output}`);
  assert.ok(lateError.output.includes('终态运行时错误'),
    `判红输出必须点明终态运行时错误，实际输出：\n${lateError.output}`);
  assert.ok(lateError.output.includes('probe late runtime boom'),
    `判红输出必须携带真实晚错消息，实际输出：\n${lateError.output}`);
  assert.ok(lateError.output.includes('pass 1'),
    '晚错探针的用例本体应是通过的（判红只能来自终态守卫）');
});
