// OPT-007（F11）：`feeds-updated` 数据变化事件的**真实监听器**回归（挂载生产 App）。
//
// 卡片要求「最终必须前端监听实发数据事件能保位更新」：本用例经
// `__DISPATCH_TAURI_EVENT__` 把一次后端 emit 按真实事件载荷形态派发给 App.tsx
// 动态 import 的 event 桩（与生产监听器同一条消费路径），断言：
//   A. 纯状态对账变化（merged_states>0、new_articles=0）到达监听器 → 真实
//      reloadFromBackend({keepReadingPosition:true})：列表行状态更新、计数刷新、
//      保位信号 positionRestoreNonce +1，且不误报「新文章」toast；
//   B. 事件确实由在册监听器消费（派发返回命中数 ≥ 1，防止「测了个没接线的桩」）；
//   C. 对照：new_articles>0 的载荷照常弹「新文章 N 篇」（载荷语义分流判别）。
//
// 证据边界：滚动/虚拟列表本体是 DOM 行为（见 scroll-position-restore 域）；
// 本用例取证到「事件 → 保位刷新落地」为止，回位消费由 t111-6 源级有序链锁定。

import assert from 'node:assert/strict';
import { componentTest, mount, flush, waitFor, resetStore, useAppStore } from './harness.mjs';
import { installBackend, seedFromRows, setPlan, setCurrentRows, mkRow, clearInvokeCalls, invokeCount } from './fixtures.mjs';

/** 三行文章布局夹具：newest-first 顺序确定为 [103, 102, 101]。 */
function rows() {
  return [
    mkRow({ id: 101, title: 'A1', published_at: '2026-10-01T10:00:00Z' }),
    mkRow({ id: 102, title: 'A2', published_at: '2026-10-02T10:00:00Z' }),
    mkRow({ id: 103, title: 'A3', published_at: '2026-10-03T10:00:00Z' }),
  ];
}

const COUNTS_AFTER_MERGE = [
  { feed_id: 10, total: 3, unread: 2, starred: 1, today: 0 },
  { feed_id: 11, total: 0, unread: 0, starred: 0, today: 0 },
  { feed_id: 12, total: 0, unread: 0, starred: 0, today: 0 },
  { feed_id: 13, total: 0, unread: 0, starred: 0, today: 0 },
  { feed_id: 14, total: 0, unread: 0, starred: 0, today: 0 },
];

componentTest('OPT-007 feeds-updated：纯 merged 变化经真实监听器保位 reload、不误报新文章', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows(rows(), { layout: 'article' });

  const { createElement, act } = await import('react');
  const { default: App } = await import('../../src/App.tsx');
  const entry = await mount(createElement(App));

  /* 等待真实启动链（bootstrapSettings → bootstrapFromBackend）把列表装好。 */
  await waitFor(() => entry.container.querySelectorAll('.article-card').length === 3,
    { label: 'App 启动后文章卡出现', timeout: 8000 });

  const art = (id) => useAppStore.getState().entries.find((e) => e.id === id);

  /* 后端「纯状态对账」变化：无新文章，只翻转一行状态 + 计数随之变化。
     （模拟 merged_states>0 的后台同步落库结果。） */
  const before = rows();
  before[0] = { ...before[0], is_read: true, is_starred: true }; // id 101
  setCurrentRows(before);
  /* OPT-008B R2（N2/P3-4）：reload 的计数读取改为**单次 counts_snapshot**
     （源键 + scope 键同一时刻）；原 feed_counts plan 已不被 reload 消费。 */
  setPlan({ counts_snapshot: () => Promise.resolve({ feeds: COUNTS_AFTER_MERGE, scope: null }) });

  const nonce0 = useAppStore.getState().positionRestoreNonce;
  const listCalls0 = invokeCount('list_articles');
  const toasts0 = useAppStore.getState().toasts.length;

  /* 派发一次真实的 feeds-updated（merged-only 载荷）——App.tsx 的监听器应消费。 */
  let listeners = 0;
  await act(async () => {
    listeners = globalThis.__DISPATCH_TAURI_EVENT__('feeds-updated', {
      new_articles: 0,
      failed_feeds: 0,
      pulled_entries: 0,
      merged_states: 2,
    });
  });
  await flush(3);
  await waitFor(() => invokeCount('list_articles') > listCalls0, { label: '事件触发的 reload 已发出' });

  assert.ok(listeners >= 1, 'feeds-updated 必须存在在册监听器（App.tsx 事件接线）');
  assert.ok(invokeCount('list_articles') > listCalls0, '事件必须触发一次真实 list_articles 重拉（不限 pulled_entries 的旧判定会漏）');
  assert.equal(art('101')?.isRead, true, '纯对账变化的已读状态随 reload 更新到列表');
  assert.equal(art('101')?.isStarred, true, '纯对账变化的收藏状态随 reload 更新到列表');
  assert.equal(useAppStore.getState().feedCounts.get('10')?.starred, 1, '计数随 reload 以后端为准刷新');
  assert.equal(useAppStore.getState().positionRestoreNonce, nonce0 + 1, '落地携带保位请求（信号 +1，Timeline 消费顶条锚）');
  assert.equal(useAppStore.getState().toasts.length, toasts0, 'merged-only 变化不得误报「新文章」toast');

  /* 对照：new_articles>0 的载荷照常弹「新文章 N 篇」（载荷语义分流）。
     ToastPill 进场是「真实 rAF 状态更新」：组件挂载后 useEffect 用
     requestAnimationFrame(() => setShown(true)) 切 visible。帧回调若落在
     act 窗口之外，React 按「act 外更新」打一条环境告警并进错误账本（历史
     上此处按「先 dispatch、再 flush、最后补一帧」编排，Node22 帧定时与 act
     边界竞态下偶发判红——已实测）。现把「派发 → 提交 → 帧回调」收进同一
     act 窗口：提交后先让出一轮让 effect 注册 rAF，再在同一 act 内等到帧
     回调执行；不关告警、不加例外。 */
  await act(async () => {
    globalThis.__DISPATCH_TAURI_EVENT__('feeds-updated', {
      new_articles: 2,
      failed_feeds: 0,
      pulled_entries: 2,
      merged_states: 0,
    });
    await new Promise((resolve) => setTimeout(resolve, 0));
    await new Promise((resolve) => requestAnimationFrame(() => resolve()));
  });
  await flush(2);
  const last = useAppStore.getState().toasts.at(-1);
  assert.ok(last && last.text.includes('新文章 2 篇'), 'new_articles>0 载荷应弹新文章 toast（对照，防把两类载荷压成同一条路径）');
  assert.ok(document.querySelector('.toast-pill.visible'), 'ToastPill 应在下一帧切入 visible（真实过渡状态）');
});
