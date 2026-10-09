// R1 反例探针：卸载期 effect cleanup 抛错（毒树）+ 另一棵好树。
// 旧实现 unmountAll catch{} 吞错 → exit 0；修复后：用例判红、最终收尾非 0
// （exit 1），且**其余树继续清理**（好树的 cleanup 落盘 marker 供父用例核对）。
import { after } from 'node:test';
import { mkdirSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { armLifecycleWatchdog, disarmLifecycleWatchdog, runFinalTeardown, componentTest, mount } from '../harness.mjs';

const OUT_DIR = fileURLToPath(new URL('../../../tmp/opt016b-runner-probes/', import.meta.url));
const MARKER = fileURLToPath(new URL('../../../tmp/opt016b-runner-probes/good-cleaned.txt', import.meta.url));

armLifecycleWatchdog();

const { createElement, useEffect } = await import('react');

function GoodOne() {
  useEffect(() => () => { writeFileSync(MARKER, 'good-cleaned', 'utf8'); }, []);
  return createElement('div', null, 'good');
}
function PoisonCleanup() {
  useEffect(() => () => { throw new Error('probe cleanup boom'); }, []);
  return createElement('div', null, 'poison');
}

componentTest('毒 cleanup 必须判红（且其余树继续清理）', { timeout: 10000 }, async () => {
  mkdirSync(OUT_DIR, { recursive: true });
  await mount(createElement(GoodOne));
  await mount(createElement(PoisonCleanup));
});

after(async () => {
  try {
    await runFinalTeardown();
  } finally {
    disarmLifecycleWatchdog();
  }
});
