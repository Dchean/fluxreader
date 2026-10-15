// tools/component-tests/subscription-writeback.test.mjs
// OPT-009 B3：订阅写回能力声明与持久回执的**真实挂载**回归。
//
// 卡片验收对应：
// - 删除源确认文案不再含「同步服务端不受影响」且包含持久退订语义（挂载真实
//   FeedsTab，经删除钮真实点击打开 ConfirmDialog，读 DOM 文案）；
// - 分类删除说明成员逐源退订；空分类走 empty_folder_remote 的本地-only 边界
//   （经真实 createCategory + reload 造出空分类）；
// - Fever 连接下能力提示为「仅本地」，删除弹窗不宣称远端退订；
// - 订阅 outbox 状态事件（subscription-outbox-changed）经**真实 App 监听器**
//   驱动真实 syncOutboxStats → SyncTab 摘要卡刷新；无待办不新增噪音行。
//
// 证据边界：能力矩阵/回执文案的**真实取值**由 Rust 侧单测锁定（枚举与 detail
// 文案），本文件用同形夹具验证「后端数据 → UI 呈现」的接线与行为，不代替
// Rust 侧对文案本体的断言。
import assert from 'node:assert/strict';
import { componentTest, mount, flush, waitFor, resetStore, useAppStore } from './harness.mjs';
import { installBackend, seedFromRows, mkRow, clearInvokeCalls, setPlan, resetPlan, FOLDERS } from './fixtures.mjs';

const ROWS = [mkRow({ id: 101, feed_id: 10, title: 'A1 文章' })];

/* 与后端 subscription_capabilities 同形的夹具（字段名/枚举值逐字对应 Rust
   serde 输出；detail 文案与 Rust 侧保持一致，但文案本体由 Rust 单测锁定）。 */
function capsFixture(protocol) {
  const greader = protocol === 'greader';
  const support = greader ? 'Supported' : 'LocalOnly';
  const cap = (detail) => ({ support, detail });
  return {
    connected: true,
    protocol,
    overview: greader
      ? 'GReader：新增/编辑/移动/退订会持久写回远端（失败自动重试、离线与重启不丢）；分类改名/删除按成员逐源表达（无分类级端点）。'
      : 'Fever 协议不支持订阅写回：新增/编辑/退订及分类变更仅本地生效，不会同步到服务端（也不会重试或冒充成功）。',
    subscribe: cap(greader
      ? '远端写回：新增订阅会持久保存并自动重试（离线或重启不丢）。'
      : '仅本地：Fever 协议不支持远端新增，订阅不会出现在服务端。'),
    edit_title: cap(greader
      ? '远端写回：标题变更持久保存并自动重试（离线或重启不丢）。'
      : '仅本地：Fever 协议不支持远端编辑，标题变化不会同步到服务端。'),
    move_folder: cap(greader
      ? '远端写回：按移动语义表达（加入目标分类、移除原分类），失败自动重试。'
      : '仅本地：Fever 协议不支持远端移动，分类变化不会同步到服务端。'),
    unsubscribe: cap(greader
      ? '远端退订：删除会保存持久的退订意图，离线或重启后自动重试，直到服务端确认。'
      : '仅本地：Fever 协议不支持远端退订，远端订阅保持不变（不回写、不重试）。'),
    rename_folder: cap(greader
      ? '远端表达：逐成员「加入新分类 + 移出旧分类」（无分类级端点）；空分类无可表达成员。'
      : '仅本地：Fever 协议不支持远端写回，改名仅本地生效（墓碑防复活）。'),
    delete_folder: cap(greader
      ? '远端退订：为每个已绑定的成员订阅保存持久退订意图（离线或重启后自动重试）；远端分类标签在成员全部移除后消失。'
      : '仅本地：Fever 协议不支持远端退订，远端订阅与分类保持不变。'),
    empty_folder_remote: cap(greader
      ? '远端无表达：空分类没有可逐源表达的成员，协议也没有分类级端点——删除/改名仅本地生效（墓碑防复活），不宣称远端已改。'
      : '仅本地：Fever 没有写订阅端点，空分类也没有可表达成员——不宣称远端已改。'),
    limitations: greader
      ? [{ code: 'folder_member_wire', detail: '分类级改名/删除没有分类级端点，按成员逐源表达。' }]
      : [],
  };
}

function remoteLineOf(msg) {
  return msg.includes('同步服务端不受影响');
}

componentTest('OPT-009 B3 FeedsTab：删除源确认弹窗如实呈现持久退订语义（GReader）', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows(ROWS, { layout: 'article' });
  setPlan({ subscription_capabilities: () => Promise.resolve(capsFixture('greader')) });
  try {
    const { createElement, act } = await import('react');
    const { FeedsTab } = await import('../../src/components/settings/FeedsTab.tsx');
    const entry = await mount(createElement(FeedsTab));
    await flush(6);
    assert.equal(useAppStore.getState().subscriptionCapabilities?.protocol, 'greader',
      '前置：能力矩阵经挂载 effect 真实 IPC 装入 store');

    const delBtn = entry.container.querySelector('.group-mgr-feed-controls .btn-danger-text');
    assert.ok(delBtn, '源行删除按钮存在');
    await act(async () => { delBtn.click(); });
    await flush(2);
    const msg = document.body.querySelector('.confirm-message')?.textContent ?? '';
    assert.ok(msg.includes('删除订阅源'), `确认弹窗已打开：${msg}`);
    assert.ok(!remoteLineOf(msg), `不得再写「同步服务端不受影响」：${msg}`);
    assert.ok(msg.includes('持久') && msg.includes('退订'), `必须反映持久退订意图：${msg}`);
    assert.ok(msg.includes('离线') && msg.includes('重启') && msg.includes('重试'), `必须说明离线/重启后可重试：${msg}`);
  } finally {
    resetPlan();
  }
});

componentTest('OPT-009 B3 FeedsTab：分类删除弹窗覆盖成员逐源退订与空分类本地-only 边界', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows(ROWS, { layout: 'article' });
  const foldersWithEmpty = [
    ...FOLDERS,
    { id: 99, name: '空分类', layout: 'article', auto_summary: false, auto_translate: false, collapsed: false },
  ];
  setPlan({
    subscription_capabilities: () => Promise.resolve(capsFixture('greader')),
    create_folder: () => Promise.resolve(99),
    list_folders: () => Promise.resolve(foldersWithEmpty),
  });
  try {
    const { createElement, act } = await import('react');
    const { FeedsTab } = await import('../../src/components/settings/FeedsTab.tsx');
    const entry = await mount(createElement(FeedsTab));
    await flush(6);
    /* 真实 store 动作造出空分类（经 create_folder + reload 落到 categories） */
    await act(async () => { useAppStore.getState().createCategory('空分类', 'article'); });
    await waitFor(() => useAppStore.getState().categories.some((c) => c.name === '空分类'), { label: '空分类就位' });

    const boxOf = (name) => [...entry.container.querySelectorAll('.feed-group-mgr-box')]
      .find((b) => b.querySelector('.group-mgr-title')?.textContent.includes(name));

    /* 有成员的分类：成员逐源退订语义 */
    await act(async () => { boxOf('技术').querySelector('.group-mgr-controls .btn-danger-text').click(); });
    await flush(2);
    const msg1 = document.body.querySelector('.confirm-message')?.textContent ?? '';
    assert.ok(msg1.includes('确定删除分类'), `分类删除弹窗已打开：${msg1}`);
    assert.ok(!remoteLineOf(msg1), `不得写「同步服务端不受影响」：${msg1}`);
    assert.ok(msg1.includes('成员') && msg1.includes('退订'), `必须说明成员逐源退订：${msg1}`);

    /* 空分类：本地-only 边界（不编造成员退订） */
    await act(async () => { boxOf('空分类').querySelector('.group-mgr-controls .btn-danger-text').click(); });
    await flush(2);
    const msg2 = document.body.querySelector('.confirm-message')?.textContent ?? '';
    assert.ok(msg2.includes('确定删除分类'), `空分类删除弹窗已打开：${msg2}`);
    assert.ok(msg2.includes('空分类') && (msg2.includes('仅本地') || msg2.includes('不宣称远端')),
      `空分类必须如实说明无可表达的远端改动（本地-only 边界）：${msg2}`);
  } finally {
    resetPlan();
  }
});

componentTest('OPT-009 B3 FeedsTab：Fever 连接下能力提示「仅本地」，删除弹窗不宣称远端退订', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows(ROWS, { layout: 'article' });
  setPlan({ subscription_capabilities: () => Promise.resolve(capsFixture('fever')) });
  try {
    const { createElement, act } = await import('react');
    const { FeedsTab } = await import('../../src/components/settings/FeedsTab.tsx');
    const entry = await mount(createElement(FeedsTab));
    await flush(6);
    assert.equal(useAppStore.getState().subscriptionCapabilities?.protocol, 'fever', '前置：Fever 能力矩阵已装入');

    const hint = entry.container.querySelector('.mini-dialog-hint');
    assert.ok(hint, 'Fever 下必须常驻「仅本地」能力提示（不得让用户以为会同步）');
    assert.ok(hint.textContent.includes('仅本地'), `提示必须含「仅本地」：${hint.textContent}`);
    assert.ok(hint.textContent.includes('Fever'), `提示必须点名 Fever：${hint.textContent}`);

    const delBtn = entry.container.querySelector('.group-mgr-feed-controls .btn-danger-text');
    await act(async () => { delBtn.click(); });
    await flush(2);
    const msg = document.body.querySelector('.confirm-message')?.textContent ?? '';
    assert.ok(msg.includes('仅本地'), `Fever 删除弹窗必须说明仅本地：${msg}`);
    assert.ok(!msg.includes('持久退订意图') && !msg.includes('直到服务端确认'),
      `Fever 不得宣称远端退订持久重试：${msg}`);
  } finally {
    resetPlan();
  }
});

componentTest('OPT-009 B3：subscription-outbox-changed 事件链驱动 SyncTab 摘要卡（等待/失败/仅本地/无噪音）', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows(ROWS, { layout: 'article' });
  setPlan({
    subscription_capabilities: () => Promise.resolve(capsFixture('fever')),
    subscription_outbox_stats: () => Promise.resolve({ pending: 0, failed: 0, local_only: 0, last_error: null }),
  });
  try {
    const { createElement, act } = await import('react');
    const { default: App } = await import('../../src/App.tsx');
    const appEntry = await mount(createElement(App));
    await waitFor(() => appEntry.container.querySelectorAll('.article-card').length === 1,
      { label: 'App 启动并渲染文章卡', timeout: 8000 });

    const { SyncTab } = await import('../../src/components/settings/SyncTab.tsx');
    const syncEntry = await mount(createElement(SyncTab));
    await flush(8);
    const cardDesc = (title) => {
      const card = [...syncEntry.container.querySelectorAll('.setting-card')]
        .find((c) => c.querySelector('h5')?.textContent === title);
      return card?.querySelector('p')?.textContent ?? '';
    };

    /* 无待办：不新增噪音行（X3 口径） */
    assert.ok(!cardDesc('同步状态').includes('订阅写回'),
      `无待办不得新增「订阅写回」噪音行：${cardDesc('同步状态')}`);

    /* 事件链：真实 App 监听器消费事件 → store → 摘要卡刷新 */
    let listeners = 0;
    await act(async () => {
      listeners = globalThis.__DISPATCH_TAURI_EVENT__('subscription-outbox-changed', {
        pending: 2, failed: 1, local_only: 0, last_error: '订阅推送失败: HTTP 500',
      });
    });
    await flush(3);
    assert.ok(listeners >= 1, 'subscription-outbox-changed 必须存在在册监听器（App.tsx 事件接线）');
    assert.ok(cardDesc('同步状态').includes('订阅写回：等待 2 项，其中 1 项失败（订阅推送失败: HTTP 500）'),
      `事件必须驱动摘要卡显示等待/失败与最近错误：${cardDesc('同步状态')}`);

    /* Fever：local_only 项必须显式「仅本地」，不得写成成功也不得隐藏 */
    await act(async () => {
      globalThis.__DISPATCH_TAURI_EVENT__('subscription-outbox-changed', {
        pending: 0, failed: 0, local_only: 3, last_error: null,
      });
    });
    await flush(3);
    assert.ok(cardDesc('同步状态').includes('3 项仅本地生效（Fever 协议不支持远端写回）'),
      `local_only 必须如实呈现：${cardDesc('同步状态')}`);
    assert.ok(cardDesc('订阅写回').includes('仅本地') || cardDesc('订阅写回').includes('不支持订阅写回'),
      `Fever 能力卡必须看出仅本地（不得出现「已同步」错觉）：${cardDesc('订阅写回')}`);

    /* 回空：噪音行随事件消失 */
    await act(async () => {
      globalThis.__DISPATCH_TAURI_EVENT__('subscription-outbox-changed', {
        pending: 0, failed: 0, local_only: 0, last_error: null,
      });
    });
    await flush(3);
    assert.ok(!cardDesc('同步状态').includes('订阅写回'),
      `清空后不得残留噪音行：${cardDesc('同步状态')}`);
  } finally {
    resetPlan();
  }
});
