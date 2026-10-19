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
import { componentTest, mount, unmount, flush, waitFor, resetStore, useAppStore } from './harness.mjs';
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

async function setInputValue(input, value) {
  const { act } = await import('react');
  const setter = Object.getOwnPropertyDescriptor(
    globalThis.window.HTMLInputElement.prototype,
    'value',
  )?.set;
  await act(async () => {
    setter.call(input, value);
    input.dispatchEvent(new globalThis.window.Event('input', { bubbles: true }));
  });
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

componentTest('OPT-020 P1-C1：SyncTab 阶段失败只重跑同步阶段，不改写已编辑的 B 表单', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows(ROWS, { layout: 'article' });

  const saves = [];
  let phaseCalls = 0;
  setPlan({
    sync_save: (args) => {
      saves.push(args);
      return Promise.resolve(JSON.stringify({
        message: '已保存',
        firstConnect: false,
        unboundLocalFeeds: 0,
      }));
    },
    sync_phase: () => {
      phaseCalls += 1;
      return Promise.reject({ message: `同步阶段失败 ${phaseCalls}` });
    },
  });

  try {
    const { createElement, act } = await import('react');
    const { SyncTab } = await import('../../src/components/settings/SyncTab.tsx');
    const entry = await mount(createElement(SyncTab));
    await flush(8);

    const inputs = [...entry.container.querySelectorAll('input.setting-input')].slice(0, 3);
    assert.equal(inputs.length, 3, '前置：Endpoint / 用户名 / 密码三个真实受控输入');
    await setInputValue(inputs[0], 'https://a.example');
    await setInputValue(inputs[1], 'alice');
    await setInputValue(inputs[2], 'secret-a');

    const saveButton = () => [...entry.container.querySelectorAll('button')]
      .find((b) => b.textContent.includes('保存并同步'));
    await act(async () => { saveButton().click(); });
    await waitFor(
      () => useAppStore.getState().errors.some((e) => e.key.startsWith('sync:save-and-sync')),
      { label: '表单 A 的 save-and-sync 失败进入错误中心', timeout: 8000 },
    );
    await waitFor(() => !saveButton().disabled, { label: '表单 A 失败后保存按钮复位', timeout: 8000 });
    assert.equal(saves.length, 1, '前置：表单 A 已真实调用 sync_save');
    assert.equal(saves[0].endpoint, 'https://a.example');
    assert.equal(saves[0].username, 'alice');
    assert.equal(saves[0].password, 'secret-a');

    const errorA = useAppStore.getState().errors.find(
      (e) => e.key.startsWith('sync:save-and-sync'),
    );
    assert.ok(errorA, '表单 A 失败后必须保留可重试入口');
    assert.match(errorA.key, /^sync:save-and-sync:\d+:(save|sync)$/,
      'save-and-sync key 必须是阶段化的 opaque 草稿 id，不得使用密码哈希');

    /* 重试前把界面改成表单 B：旧入口必须仍调用 A 的完整草稿。 */
    await setInputValue(inputs[0], 'https://b.example');
    await setInputValue(inputs[1], 'bob');
    await setInputValue(inputs[2], 'secret-b');
    await act(async () => { useAppStore.getState().retryError(errorA.id); });
    await waitFor(
      () => phaseCalls === 2
        && useAppStore.getState().errors.some(
          (e) => e.key === errorA.key && e.id !== errorA.id,
        ),
      { label: '旧阶段重试再次失败并保留同上下文入口', timeout: 8000 },
    );
    await waitFor(() => !saveButton().disabled, { label: '旧重试失败后保存按钮复位', timeout: 8000 });
    assert.equal(saves.length, 1, 'phase 失败重试不得再次调用 sync_save（旧 activateVersion 已被消费）');
    assert.equal(inputs[0].value, 'https://b.example', '旧阶段重试不得改写当前 B Endpoint');
    assert.equal(inputs[1].value, 'bob', '旧阶段重试不得改写当前 B 用户名');
    assert.equal(inputs[2].value, 'secret-b', '旧阶段重试不得清空当前 B 密码');

    /* 新的手动提交使用 C 表单：与 A 的旧错误不得共用固定 key 合并。 */
    await setInputValue(inputs[0], 'https://c.example');
    await setInputValue(inputs[1], 'carol');
    await setInputValue(inputs[2], 'secret-c');
    await act(async () => { saveButton().click(); });
    await waitFor(
      () => saves.length === 2
        && useAppStore.getState().errors.filter(
          (e) => e.key.startsWith('sync:save-and-sync'),
        ).length === 2,
      { label: '表单 C 的失败与表单 A 上下文分条保存', timeout: 8000 },
    );
    const syncErrors = useAppStore.getState().errors.filter(
      (e) => e.key.startsWith('sync:save-and-sync'),
    );
    assert.equal(new Set(syncErrors.map((e) => e.key)).size, 2,
      '不同表单草稿的 save-and-sync 失败必须使用不同稳定 key，不得跨上下文合并');
  } finally {
    resetPlan();
  }
});

componentTest('OPT-020 P1-C1：save 失败重试仍保留入口，旧草稿成功不污染 B 凭据且清理已消费激活令牌', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows(ROWS, { layout: 'article' });

  const saves = [];
  let saveCalls = 0;
  let phaseCalls = 0;
  setPlan({
    sync_pending_connection: () => Promise.resolve({
      version: 77,
      sync_protocol: 'greader',
      greader_endpoint: 'https://a.example',
      greader_username: 'alice',
    }),
    sync_save: (args) => {
      saveCalls += 1;
      saves.push(args);
      if (saveCalls <= 2) {
        return Promise.reject({ message: `保存失败 ${saveCalls}` });
      }
      return Promise.resolve(JSON.stringify({
        message: '已保存',
        firstConnect: false,
        unboundLocalFeeds: 0,
      }));
    },
    sync_phase: () => {
      phaseCalls += 1;
      if (phaseCalls === 1) return Promise.reject({ message: '同步阶段失败' });
      return Promise.resolve({ errors: [], removed_feeds: 0 });
    },
  });

  try {
    const { createElement, act } = await import('react');
    const { SyncTab } = await import('../../src/components/settings/SyncTab.tsx');
    const entry = await mount(createElement(SyncTab));
    await flush(8);

    const inputs = [...entry.container.querySelectorAll('input.setting-input')].slice(0, 3);
    const applyButton = () => [...entry.container.querySelectorAll('button')]
      .find((b) => b.textContent.includes('填入表单并激活'));
    const saveButton = () => [...entry.container.querySelectorAll('button')]
      .find((b) => b.textContent.includes('保存并同步'));
    const connectionTag = () => {
      const card = [...entry.container.querySelectorAll('.setting-card')]
        .find((c) => c.querySelector('h5')?.textContent === '连接状态');
      return card?.querySelector('.about-arch-tag')?.textContent ?? '';
    };

    await waitFor(() => !!applyButton(), { label: '待确认连接建议已挂载', timeout: 8000 });
    await act(async () => { applyButton().click(); });
    await flush(3);
    assert.equal(inputs[0].value, 'https://a.example');
    assert.equal(inputs[1].value, 'alice');
    await setInputValue(inputs[2], 'secret-a');

    await act(async () => { saveButton().click(); });
    await waitFor(
      () => useAppStore.getState().errors.some((e) => e.key.startsWith('sync:save-and-sync')),
      { label: '首次 sync_save 失败必须留下重试入口', timeout: 8000 },
    );
    await waitFor(() => !saveButton().disabled, { label: '首次保存失败后按钮复位', timeout: 8000 });
    const saveError1 = useAppStore.getState().errors.find(
      (e) => e.key.startsWith('sync:save-and-sync'),
    );
    assert.match(saveError1.key, /^sync:save-and-sync:\d+:save$/,
      'sync_save 失败 key 必须带 opaque 草稿 id 与 save 阶段');

    await setInputValue(inputs[0], 'https://b.example');
    await setInputValue(inputs[1], 'bob');
    await setInputValue(inputs[2], 'secret-b');

    await act(async () => { useAppStore.getState().retryError(saveError1.id); });
    await waitFor(
      () => saveCalls === 2
        && useAppStore.getState().errors.some(
          (e) => e.key === saveError1.key && e.id !== saveError1.id,
        ),
      { label: 'save 重试再次失败仍保留同 key 入口', timeout: 8000 },
    );
    const saveError2 = useAppStore.getState().errors.find((e) => e.key === saveError1.key);

    await act(async () => { useAppStore.getState().retryError(saveError2.id); });
    await waitFor(
      () => saveCalls === 3
        && phaseCalls === 1
        && useAppStore.getState().errors.some((e) => !e.key.endsWith(':save')),
      { label: '旧草稿保存成功、阶段失败进入新入口', timeout: 8000 },
    );
    await waitFor(() => !saveButton().disabled, { label: '旧草稿阶段失败后按钮复位', timeout: 8000 });

    assert.equal(inputs[0].value, 'https://b.example', '旧草稿成功不得覆盖 B Endpoint');
    assert.equal(inputs[1].value, 'bob', '旧草稿成功不得覆盖 B 用户名');
    assert.equal(inputs[2].value, 'secret-b', '旧草稿成功不得清空 B 密码');
    assert.equal(connectionTag(), '未连接', '旧草稿成功不得把 B 的连接状态改成已连接');

    const phaseError = useAppStore.getState().errors.find((e) => e.key.endsWith(':sync'));
    assert.ok(phaseError, '激活草稿 save 成功、phase 失败后必须保留 phase 入口');
    await act(async () => { useAppStore.getState().retryError(phaseError.id); });
    await waitFor(
      () => saveCalls === 3
        && phaseCalls === 3
        && !useAppStore.getState().errors.some((e) => e.key === phaseError.key),
      { label: '激活成功的旧草稿重试只跑 phase 且成功清除入口', timeout: 8000 },
    );
    assert.equal(saves.length, 3,
      '激活成功后 phase 重试不得再次 sync_save（旧 activateVersion 已被消费）');

    await act(async () => { saveButton().click(); });
    await waitFor(
      () => saveCalls === 4,
      { label: 'B 表单手动保存调用 sync_save', timeout: 8000 },
    );
    await flush(8);
    assert.equal(phaseCalls, 5,
      `A 的失败/成功 phase 与 B 的 feeds+states 合计应为 5 次（实际 ${phaseCalls}）`);
    assert.equal(saves[3].activatePendingVersion, undefined,
      'A 已消费版本 77 后，B 后续保存不得再次携带旧激活令牌');
    await waitFor(() => connectionTag() === '已连接' || connectionTag() === 'alice',
      { label: 'B 保存成功后连接态更新', timeout: 8000 });
  } finally {
    resetPlan();
  }
});

componentTest('OPT-020 P1-C1：最新提交代际使旧 save/phase action 跨重挂失效', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows(ROWS, { layout: 'article' });

  let saveCalls = 0;
  let phaseCalls = 0;
  setPlan({
    sync_save: () => {
      saveCalls += 1;
      if (saveCalls === 1) return Promise.reject({ message: 'A1 保存失败' });
      return Promise.resolve(JSON.stringify({
        message: '已保存',
        firstConnect: false,
        unboundLocalFeeds: 0,
      }));
    },
    sync_phase: () => {
      phaseCalls += 1;
      if (phaseCalls === 1) return Promise.reject({ message: 'A2 阶段失败' });
      return Promise.resolve({ errors: [], removed_feeds: 0 });
    },
  });

  try {
    const { createElement, act } = await import('react');
    const { SyncTab } = await import('../../src/components/settings/SyncTab.tsx');
    const first = await mount(createElement(SyncTab));
    await flush(8);
    const firstInputs = [...first.container.querySelectorAll('input.setting-input')].slice(0, 3);
    const firstSave = () => [...first.container.querySelectorAll('button')]
      .find((b) => b.textContent.includes('保存并同步'));

    await setInputValue(firstInputs[0], 'https://a1.example');
    await setInputValue(firstInputs[1], 'alice1');
    await setInputValue(firstInputs[2], 'secret-a1');
    await act(async () => { firstSave().click(); });
    await waitFor(
      () => useAppStore.getState().errors.some((e) => e.key.endsWith(':save')),
      { label: 'A1 save 失败入口生成', timeout: 8000 },
    );
    const oldSaveError = useAppStore.getState().errors.find((e) => e.key.endsWith(':save'));

    await waitFor(() => !firstSave().disabled, { label: 'A1 失败后按钮复位', timeout: 8000 });
    await setInputValue(firstInputs[0], 'https://a2.example');
    await setInputValue(firstInputs[1], 'alice2');
    await setInputValue(firstInputs[2], 'secret-a2');
    await act(async () => { firstSave().click(); });
    await waitFor(
      () => saveCalls === 2 && phaseCalls === 1
        && useAppStore.getState().errors.some((e) => e.key.endsWith(':sync')),
      { label: 'A2 save 成功、phase 失败入口生成', timeout: 8000 },
    );
    const oldPhaseError = useAppStore.getState().errors.find((e) => e.key.endsWith(':sync'));
    await unmount(first);

    const second = await mount(createElement(SyncTab));
    await flush(8);
    const secondInputs = [...second.container.querySelectorAll('input.setting-input')].slice(0, 3);
    const secondSave = () => [...second.container.querySelectorAll('button')]
      .find((b) => b.textContent.includes('保存并同步'));
    await setInputValue(secondInputs[0], 'https://b.example');
    await setInputValue(secondInputs[1], 'bob');
    await setInputValue(secondInputs[2], 'secret-b');
    await act(async () => { secondSave().click(); });
    await waitFor(
      () => saveCalls === 3 && phaseCalls === 3,
      { label: 'B 草稿 save + feeds/states 成功提交', timeout: 8000 },
    );

    const saveCallsAtCommit = saveCalls;
    const phaseCallsAtCommit = phaseCalls;
    await act(async () => {
      useAppStore.getState().retryError(oldSaveError.id);
      useAppStore.getState().retryError(oldPhaseError.id);
    });
    await flush(6);
    assert.equal(saveCalls, saveCallsAtCommit,
      'B 成功提交后，旧 save action 不得再调用 sync_save');
    assert.equal(phaseCalls, phaseCallsAtCommit,
      'B 成功提交后，旧 phase action 不得再调用 sync_phase');
    assert.ok(useAppStore.getState().toasts.some((t) => t.text.includes('保存草稿已失效')),
      '旧草稿重试必须提示失效，而不是静默执行');
  } finally {
    resetPlan();
  }
});

componentTest('OPT-020 P1-C1：sync_save 在途编辑字段仍保持全局忙锁且不能二次保存', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows(ROWS, { layout: 'article' });

  let saveCalls = 0;
  let resolveSave = null;
  let phaseCalls = 0;
  setPlan({
    sync_save: () => {
      saveCalls += 1;
      return new Promise((resolve) => { resolveSave = resolve; });
    },
    sync_phase: () => {
      phaseCalls += 1;
      return Promise.reject({ message: '阶段失败' });
    },
  });

  try {
    const { createElement, act } = await import('react');
    const { SyncTab } = await import('../../src/components/settings/SyncTab.tsx');
    const entry = await mount(createElement(SyncTab));
    await flush(8);
    const inputs = [...entry.container.querySelectorAll('input.setting-input')].slice(0, 3);
    const saveButton = () => [...entry.container.querySelectorAll('button')]
      .find((b) => b.textContent.includes('保存中…') || b.textContent.includes('保存并同步'));

    await setInputValue(inputs[0], 'https://busy.example');
    await setInputValue(inputs[1], 'busy-user');
    await setInputValue(inputs[2], 'busy-secret');
    await act(async () => { saveButton().click(); });
    await waitFor(() => saveCalls === 1 && saveButton().disabled,
      { label: 'sync_save 在途时按钮禁用', timeout: 8000 });

    await setInputValue(inputs[2], 'edited-during-save');
    assert.equal(saveButton().disabled, true,
      '编辑字段只应让旧草稿失去 UI 写权，不得提前释放全局忙锁');
    await act(async () => { saveButton().click(); });
    await flush(2);
    assert.equal(saveCalls, 1, '忙锁期间按钮不可用，不能发起第二次 sync_save');

    await act(async () => {
      resolveSave(JSON.stringify({
        message: '已保存',
        firstConnect: false,
        unboundLocalFeeds: 0,
      }));
    });
    await waitFor(() => !saveButton().disabled && phaseCalls === 1,
      { label: 'save+phase settle 后忙锁释放', timeout: 8000 });
  } finally {
    resetPlan();
  }
});

componentTest('OPT-020 P1-C1：stale draft 重试挂起时全局忙锁阻止 B 再次保存', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows(ROWS, { layout: 'article' });

  let saveCalls = 0;
  let resolveStaleRetry = null;
  setPlan({
    sync_save: () => {
      saveCalls += 1;
      if (saveCalls === 1) return Promise.reject({ message: 'A 首次失败' });
      if (saveCalls === 2) {
        return new Promise((resolve) => { resolveStaleRetry = resolve; });
      }
      return Promise.resolve(JSON.stringify({
        message: '新保存',
        firstConnect: false,
        unboundLocalFeeds: 0,
      }));
    },
    sync_phase: () => Promise.resolve({ errors: [], removed_feeds: 0 }),
  });

  try {
    const { createElement, act } = await import('react');
    const { SyncTab } = await import('../../src/components/settings/SyncTab.tsx');
    const entry = await mount(createElement(SyncTab));
    await flush(8);
    const inputs = [...entry.container.querySelectorAll('input.setting-input')].slice(0, 3);
    const saveButton = () => [...entry.container.querySelectorAll('button')]
      .find((b) => b.textContent.includes('保存中…') || b.textContent.includes('保存并同步'));

    await setInputValue(inputs[0], 'https://stale-a.example');
    await setInputValue(inputs[1], 'stale-a');
    await setInputValue(inputs[2], 'stale-a-pass');
    await act(async () => { saveButton().click(); });
    await waitFor(
      () => useAppStore.getState().errors.some((e) => e.key.endsWith(':save')),
      { label: 'A 首次失败入口生成', timeout: 8000 },
    );
    const staleSaveError = useAppStore.getState().errors.find((e) => e.key.endsWith(':save'));

    await setInputValue(inputs[0], 'https://b-locked.example');
    await setInputValue(inputs[1], 'b-locked');
    await setInputValue(inputs[2], 'b-locked-pass');
    await act(async () => { useAppStore.getState().retryError(staleSaveError.id); });
    await waitFor(() => saveCalls === 2,
      { label: 'stale A retry 进入 sync_save', timeout: 8000 });
    assert.equal(saveButton().disabled, true,
      'stale draft retry 在途也必须禁用当前 B 的保存按钮');

    await act(async () => { saveButton().click(); });
    await flush(2);
    assert.equal(saveCalls, 2, 'stale retry 在途时再点保存必须 0 新 IPC');

    await act(async () => {
      resolveStaleRetry(JSON.stringify({
        message: 'A 重试保存成功',
        firstConnect: false,
        unboundLocalFeeds: 0,
      }));
    });
    await waitFor(() => !saveButton().disabled,
      { label: 'stale retry settle 后忙锁释放', timeout: 8000 });
  } finally {
    if (resolveStaleRetry) {
      await Promise.resolve();
    }
    resetPlan();
  }
});

componentTest('OPT-020 P1-C1：A 在途卸载重挂仍持锁，A settle 后 B 才可保存', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows(ROWS, { layout: 'article' });

  let saveCalls = 0;
  let phaseCalls = 0;
  let resolveInFlightSave = null;
  let resolveInFlightPhase = null;
  setPlan({
    sync_save: () => {
      saveCalls += 1;
      if (saveCalls === 1) {
        return new Promise((resolve) => { resolveInFlightSave = resolve; });
      }
      return Promise.resolve(JSON.stringify({
        message: 'B 已保存',
        firstConnect: false,
        unboundLocalFeeds: 0,
      }));
    },
    sync_phase: () => {
      phaseCalls += 1;
      if (phaseCalls === 1) {
        return new Promise((resolve) => { resolveInFlightPhase = resolve; });
      }
      return Promise.resolve({ errors: [], removed_feeds: 0 });
    },
  });

  try {
    const { createElement, act } = await import('react');
    const { SyncTab } = await import('../../src/components/settings/SyncTab.tsx');
    const first = await mount(createElement(SyncTab));
    await flush(8);
    const firstInputs = [...first.container.querySelectorAll('input.setting-input')].slice(0, 3);
    const firstSave = () => [...first.container.querySelectorAll('button')]
      .find((b) => b.textContent.includes('保存中…') || b.textContent.includes('保存并同步'));
    await setInputValue(firstInputs[0], 'https://unmounted-a.example');
    await setInputValue(firstInputs[1], 'unmounted-a');
    await setInputValue(firstInputs[2], 'unmounted-a-pass');
    await act(async () => { firstSave().click(); });
    await waitFor(() => saveCalls === 1 && firstSave().disabled,
      { label: 'A 在途进入全局忙锁', timeout: 8000 });
    await unmount(first);

    const second = await mount(createElement(SyncTab));
    await flush(8);
    const secondInputs = [...second.container.querySelectorAll('input.setting-input')].slice(0, 3);
    const secondSave = () => [...second.container.querySelectorAll('button')]
      .find((b) => b.textContent.includes('保存中…') || b.textContent.includes('保存并同步'));
    assert.equal(secondSave().disabled, true,
      'A 在途时卸载重挂，B 必须看到同一全局忙锁');
    await setInputValue(secondInputs[0], 'https://after-a.example');
    await setInputValue(secondInputs[1], 'after-a');
    await setInputValue(secondInputs[2], 'after-a-pass');
    await act(async () => { secondSave().click(); });
    await flush(2);
    assert.equal(saveCalls, 1, 'A settle 前 B 不得发 sync_save');

    await act(async () => {
      resolveInFlightSave(JSON.stringify({
        message: 'A 已保存',
        firstConnect: false,
        unboundLocalFeeds: 0,
      }));
    });
    await waitFor(() => phaseCalls === 1,
      { label: 'A 进入 phase 且仍持锁', timeout: 8000 });
    assert.equal(secondSave().disabled, true,
      'A 的 phase 在途时 B 仍必须被全局忙锁挡住');

    await act(async () => {
      resolveInFlightPhase({ errors: [], removed_feeds: 0 });
    });
    await waitFor(() => !secondSave().disabled,
      { label: 'A 的 save+phase settle 后释放全局忙锁', timeout: 8000 });
    await act(async () => { secondSave().click(); });
    await waitFor(() => saveCalls === 2,
      { label: 'A settle 后 B 才发 sync_save', timeout: 8000 });
    assert.equal(saveCalls, 2,
      '不允许 B 在 A 迟到完成前成功提交');
  } finally {
    if (resolveInFlightSave) await Promise.resolve();
    if (resolveInFlightPhase) await Promise.resolve();
    resetPlan();
  }
});

componentTest('OPT-020 P3：全局忙锁拒绝 retry 时按原 key 重报可重试入口', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows(ROWS, { layout: 'article' });

  let saveCalls = 0;
  let resolveBlockingSave = null;
  setPlan({
    sync_save: () => {
      saveCalls += 1;
      if (saveCalls === 1) return Promise.reject({ message: '首次失败' });
      return new Promise((resolve) => { resolveBlockingSave = resolve; });
    },
    sync_phase: () => Promise.resolve({ errors: [], removed_feeds: 0 }),
  });

  try {
    const { createElement, act } = await import('react');
    const { SyncTab } = await import('../../src/components/settings/SyncTab.tsx');
    const entry = await mount(createElement(SyncTab));
    await flush(8);
    const inputs = [...entry.container.querySelectorAll('input.setting-input')].slice(0, 3);
    const saveButton = () => [...entry.container.querySelectorAll('button')]
      .find((b) => b.textContent.includes('保存中…') || b.textContent.includes('保存并同步'));
    await setInputValue(inputs[0], 'https://retry-busy.example');
    await setInputValue(inputs[1], 'retry-busy');
    await setInputValue(inputs[2], 'retry-busy-pass');

    await act(async () => { saveButton().click(); });
    await waitFor(() => useAppStore.getState().errors.some((e) => e.key.endsWith(':save')),
      { label: '首次失败入口', timeout: 8000 });
    const retryBusyError = useAppStore.getState().errors.find((e) => e.key.endsWith(':save'));

    await act(async () => { saveButton().click(); }); // 新草稿占住全局锁
    await waitFor(() => saveCalls === 2 && saveButton().disabled,
      { label: '新草稿持锁', timeout: 8000 });

    await act(async () => { useAppStore.getState().retryError(retryBusyError.id); });
    await flush(3);
    const requeued = useAppStore.getState().errors.find((e) => e.key === retryBusyError.key);
    assert.ok(requeued?.action?.run,
      'begin 被全局忙锁拒绝时必须按原 key 重报入口，不能因 retryError 已消费而丢失');

    await act(async () => {
      resolveBlockingSave(JSON.stringify({
        message: '阻塞操作完成',
        firstConnect: false,
        unboundLocalFeeds: 0,
      }));
    });
    await waitFor(() => !saveButton().disabled,
      { label: '阻塞操作 settle', timeout: 8000 });
  } finally {
    if (resolveBlockingSave) await Promise.resolve();
    resetPlan();
  }
});
