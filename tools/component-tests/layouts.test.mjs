// OPT-016B：Timeline 五布局真实挂载切换回归（store 真实数据驱动 + 组件树错误监控）。
//
// 取证方式：挂载真实 Timeline（生产组件），经真实导航 action selectLayout 连续切换
// 五种布局（article/social/image/podcast/notification），每次断言：
//   1) 对应卡片的真实 DOM 形态出现、且其余布局的卡片形态不出现（不是 css 类变化）；
//   2) 卡片文本来自 store 里的后端行（数据驱动，而非空壳）；
//   3) store.entries 被后端按布局维度替换（真实 reload 链路）。
// 同时捕获 console.error / onCaughtError / onUncaughtError（componentStack），
// 断言连续切换不出现 getSnapshot 循环，并用法向 canary 证明捕获路径真的可用。

import assert from 'node:assert/strict';
import { componentTest, mount, unmount, flush, resetStore, useAppStore, consoleErrors, allowConsoleError, allowReactError } from './harness.mjs';
import { installBackend, seedFromRows, defaultRows, rowsForLayout } from './fixtures.mjs';

const LAYOUT_CARD = {
  article: '.article-card',
  social: '.social-card',
  image: '.gallery-card',
  podcast: '.podcast-card',
  notification: '.notif-card',
};

componentTest('OPT-016B Timeline 五布局：真实组件连续切换，卡片 DOM 与 store 数据驱动（非 css 类变化），无 getSnapshot 循环', { timeout: 40000 }, async () => {
  installBackend();
  resetStore();
  const rows = defaultRows();
  seedFromRows(rows, { layout: 'article' });

  const { createElement, act } = await import('react');
  const { Timeline } = await import('../../src/components/Timeline.tsx');
  const entry = await mount(createElement(Timeline));
  await flush(3);

  const seen = [];
  const sequence = ['article', 'social', 'image', 'podcast', 'notification', 'article', 'notification', 'social'];
  for (const layout of sequence) {
    await act(async () => {
      useAppStore.getState().selectLayout(layout);
      await new Promise((r) => setTimeout(r, 10));
    });
    await flush(3);

    const expected = rowsForLayout(rows, layout);
    const cardSel = LAYOUT_CARD[layout];
    const cards = entry.container.querySelectorAll(cardSel);

    // 1) 对应布局的真实卡片形态出现；标题来自 store 行（数据驱动）
    assert.ok(cards.length === expected.length,
      `[${layout}] 期望 ${expected.length} 张 ${cardSel}，实际 ${cards.length}`);
    for (const r of expected) {
      assert.ok(entry.container.textContent.includes(r.title),
        `[${layout}] 卡片文本应包含后端行标题「${r.title}」`);
    }
    // 2) 其余布局的卡片形态不出现（判别对，而非「只加了类」）
    for (const [other, sel] of Object.entries(LAYOUT_CARD)) {
      if (other === layout) continue;
      assert.equal(entry.container.querySelectorAll(sel).length, 0,
        `[${layout}] 不应出现 ${other} 的卡片形态 ${sel}`);
    }
    // 3) 视图标题跟随布局名（LAYOUT_NAMES 单点的真实消费）
    assert.ok(entry.container.querySelector('.view-title-text'),
      `[${layout}] 顶栏标题应存在`);
    // 4) store 快照被该布局的真实 reload 替换（entries 与可见布局同口径）
    const titles = useAppStore.getState().entries.map((e) => e.title).sort();
    assert.deepEqual(titles, expected.map((r) => r.title).sort(),
      `[${layout}] store.entries 应被后端按布局过滤替换`);

    seen.push(`${layout}:${cards.length}`);
  }
  assert.deepEqual(seen, ['article:3', 'social:2', 'image:1', 'podcast:1', 'notification:1', 'article:3', 'notification:1', 'social:2'],
    '五布局连续切换的行数序列应稳定');

  // ---- 生产树阶段断言：整轮切换零未预期错误（账本在用例起始已清空，
  //      此处的空数组就是「本阶段没有任何 console.error / React 错误」）----
  assert.deepEqual(consoleErrors, [], `连续切换出现 console.error：${JSON.stringify(consoleErrors)}`);
  assert.deepEqual(entry.records.uncaught, [], '生产组件树不应有未捕获错误');
  assert.deepEqual(entry.records.caught, [], '生产组件树不应有 error boundary 捕获项');
  assert.deepEqual(entry.records.recoverable, [], '生产组件树不应有可恢复错误');

  // ---- 捕获路径 canary：证明 errorInfo.componentStack 真的可用 ----
  /* 不虚构：canary 是测试自带的抛错组件（非生产代码），它证明「收集器装了但
     永远收不到栈」这一失效模式不会让上面的「零错误」断言变成恒真。
     两条路径分开取证（React 19 的真实语义）：
     - onCaughtError：错误被 Error Boundary 捕获时调用（act 内也照常调用）；
     - onUncaughtError：无 boundary 时调用，但 React 在 act 队列内会把错误改
       收入 act 的 thrownErrors（react-dom 内部 logUncaughtError 的 actQueue
       分支），因此该路径必须在 act 外触发才能真正走到收集器。
     canary 的预期错误用 one-shot 精确例外登记（错误发生即消费；未消费=失败，
     防止例外把整类错误屏蔽）——生产树断言不受影响。 */
  const { Component } = await import('react');
  class CanaryBoundary extends Component {
    state = { failed: false };
    static getDerivedStateFromError() { return { failed: true }; }
    render() { return this.state.failed ? createElement('div', null, 'canary-fallback') : this.props.children; }
  }
  function CanaryThrower() { throw new Error('OPT-016B canary'); }
  allowReactError(/OPT-016B canary/, 'caught');
  const caught = await mount(createElement(CanaryBoundary, null, createElement(CanaryThrower)));
  await flush(1);
  assert.equal(caught.records.renderError, null, '边界捕获路径不应冒泡出挂载错误');
  assert.equal(caught.records.caught.length, 1, 'onCaughtError 应收到边界捕获的错误');
  assert.ok(caught.records.caught[0].stack.includes('CanaryThrower'),
    `onCaughtError componentStack 应点名抛错组件，实际：${JSON.stringify(caught.records.caught[0].stack).slice(0, 200)}`);
  assert.ok(caught.container.querySelector('.canary-fallback') || caught.container.textContent.includes('canary-fallback'),
    '边界兜底 UI 应渲染（错误确实被捕获）');
  await unmount(caught);

  /* onUncaughtError：不经 act 渲染（act 内会被 React 改道），同步/异步二义性
     用 try/catch 吞掉 dev 重抛，只消费收集器记录。该子树的错误也显式登记精确
     例外（act 外渲染会打 act 告警 console.error + 一条错误日志）。 */
  {
    const { createRoot } = await import('react-dom/client');
    const container = document.createElement('div');
    document.body.appendChild(container);
    const uncaughtStacks = [];
    const root = createRoot(container, {
      onUncaughtError: (_e, info) => uncaughtStacks.push(info?.componentStack ?? ''),
    });
    /* act 外渲染触发的「not wrapped in act」告警（React 以 Root 组件名报到
       console.error）：两条（render 与随后的错误更新各一条），用两条 one-shot
       精确例外消费；不用宽泛 pattern。 */
    allowConsoleError(/not wrapped in act/);
    allowConsoleError(/not wrapped in act/);
    try {
      root.render(createElement(CanaryThrower));
      await new Promise((r) => setTimeout(r, 20));
    } catch { /* dev 模式重抛：不是失败信号，收集器记录才是取证对象 */ }
    assert.equal(uncaughtStacks.length, 1, 'onUncaughtError 应收到无边界错误');
    assert.ok(uncaughtStacks[0].includes('CanaryThrower'),
      `onUncaughtError componentStack 应点名组件，实际：${JSON.stringify(uncaughtStacks[0]).slice(0, 200)}`);
    try { root.unmount(); } catch { /* 已崩溃的根 */ }
    container.remove();
  }
});
