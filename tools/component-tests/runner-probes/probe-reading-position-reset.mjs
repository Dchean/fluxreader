// RS4：黑盒检验显式复位、卸载后清理及下一用例初始化，不手动清理待验证状态。
import assert from 'node:assert/strict';
import { after } from 'node:test';
import { armLifecycleWatchdog, componentTest, disarmLifecycleWatchdog, mount, resetStore, runFinalTeardown } from '../harness.mjs';

const { createElement, useEffect } = await import('react');
const anchors = await import('../../../src/lib/readingPosition.ts');
const archiveKeys = ['rs4|archive-a', 'rs4|archive-b'];
const failBody = process.argv.includes('--body-error');

function leavePositionState(phase) {
  for (const [index, key] of archiveKeys.entries()) {
    anchors.rearmTopAnchor(`${phase}-${index}`, key, 100000 + index, 17);
    anchors.stashTopAnchorForReturn();
  }
  anchors.rearmTopAnchor(`${phase}-active`, 'rs4|active', 1000000, 23);
}

function assertFreshPosition(phase) {
  assert.equal(anchors.peekTopAnchor(), null, `${phase}: 活锚必须已清空`);
  for (const key of archiveKeys) {
    assert.equal(anchors.peekReturnAnchor(key), null, `${phase}: 返回存档 ${key} 必须已清空`);
  }
  assert.equal(anchors.recordTopAnchor('first-record', 'rs4|fresh', 250), true,
    `${phase}: 上例节流基准不得拒绝新用例首笔记录`);
  anchors.clearTopAnchor();
  console.log(`RS4_CLEAN ${phase}`);
}

function StashOnUnmount() {
  useEffect(() => () => { leavePositionState('unmount'); }, []);
  return createElement('span', null, 'position cleanup probe');
}

armLifecycleWatchdog();
componentTest('RS4 probe: resetStore and post-unmount cleanup', async (t) => {
  t.after(() => {
    assertFreshPosition('after-first-case');
    // 模拟用例 after 钩子/间隙再写入；下一例必须在执行 body 前清掉。
    leavePositionState('between-cases');
  });
  leavePositionState('before-explicit-reset');
  resetStore();
  assertFreshPosition('explicit-reset');
  await mount(createElement(StashOnUnmount));
  leavePositionState('test-body');
  if (failBody) throw new Error('RS4 expected body error');
});
componentTest('RS4 probe: next case starts clean and cleans its own residue', async (t) => {
  t.after(() => { assertFreshPosition('after-second-case'); });
  assertFreshPosition('next-case-start');
  leavePositionState('second-case');
});

after(async () => {
  try { await runFinalTeardown(); }
  finally { disarmLifecycleWatchdog(); }
});
