// 同步灯 / 账户设置入口 / AddFeed 写入反馈的真实挂载回归。
//
// 证据边界：jsdom 真实 DOM + 生产组件 + dist-test store；后端 IPC 仍是
// fixtures 的可控假后端。这里不模拟组件内部行为，只替身 IPC 边界与
// triggerManualSync 的动作对象（用于证明灯点击确实调用现有入口）。
import assert from 'node:assert/strict';
import {
  componentTest, mount, flush, waitFor, resetStore, useAppStore,
} from './harness.mjs';
import {
  installBackend, resetPlan, setPlan, seedFromRows, defaultRows, invokeCalls, deferred,
} from './fixtures.mjs';

const ZERO_OUTBOX = { pending: 0, failed: 0, local_only: 0, last_error: null };

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

componentTest('同步灯：账户名常驻、灯点击调用 triggerManualSync、文字打开同步设置、无重复刷新入口', { timeout: 30000 }, async () => {
  installBackend();
  resetPlan();
  resetStore();
  seedFromRows(defaultRows());
  useAppStore.setState({
    syncStatus: 'synced',
    backgroundSyncing: false,
    syncConnected: true,
    syncAccount: 'alice',
    syncProtocol: 'greader',
    syncSucceededAt: null,
    syncWaiting: 0,
    syncFailed: 0,
    syncLocalOnly: 0,
    syncBindableRetry: 0,
    syncExpiredUnconfirmed: 0,
    subOutboxStats: ZERO_OUTBOX,
    settingsOpen: false,
    settingsTab: '',
  });

  let manualCalls = 0;
  const originalTrigger = useAppStore.getState().triggerManualSync;
  useAppStore.setState({
    triggerManualSync: async () => { manualCalls += 1; },
  });
  try {
    const { createElement, act } = await import('react');
    const { Sidebar } = await import('../../src/components/Sidebar.tsx');
    const entry = await mount(createElement(Sidebar));
    const dot = entry.container.querySelector('.sync-indicator-button');
    const label = entry.container.querySelector('.sync-status-pill');
    assert.ok(dot, '左侧灯必须是可点击的同步按钮');
    assert.ok(label, '账户文字必须是独立设置入口');
    assert.ok(!entry.container.querySelector('.sync-refresh-btn'), '小刷新图标必须删除');
    assert.ok(!entry.container.querySelector('.sidebar-refresh-all'), '同义大按钮必须删除');
    assert.equal(label.textContent, 'alice', '已连接常驻文案显示账户名');
    assert.ok(dot.querySelector('.sync-dot--success'), '无待办账户显示绿色');

    await act(async () => { dot.click(); });
    await flush(1);
    assert.equal(manualCalls, 1, '灯点击调用现有 triggerManualSync');
    assert.equal(useAppStore.getState().syncStatus, 'synced', '动作替身不伪造同步状态');

    await act(async () => { label.click(); });
    await flush(1);
    assert.equal(useAppStore.getState().settingsOpen, true, '文字点击打开设置');
    assert.equal(useAppStore.getState().settingsTab, 'sync', '文字点击直达同步设置');

    await act(async () => {
      useAppStore.setState({ syncWaiting: 3 });
    });
    await flush(1);
    assert.equal(label.textContent, 'alice', '待推送时仍显示账户名');
    assert.ok(dot.querySelector('.sync-dot--pending'), '待推送灯为橙色');
    assert.ok(dot.getAttribute('aria-label').includes('状态待推送 3 条'), '数量进入 aria 详情');
    assert.ok(!label.textContent.includes('等待同步'), '不在常驻文字里堆数量');

    await act(async () => {
      useAppStore.setState({ syncStatus: 'syncing', backgroundSyncing: false });
    });
    await flush(1);
    assert.equal(label.textContent, '同步中', '同步中覆盖账户名');
    assert.ok(dot.querySelector('.sync-dot--syncing'), '同步中灯使用呼吸类');
    assert.equal(dot.disabled, true, '同步中灯 disabled');

    await act(async () => {
      useAppStore.setState({
        syncStatus: 'synced',
        backgroundSyncing: false,
        syncConnected: true,
        syncAccount: null,
        syncSucceededAt: null,
        syncWaiting: 0,
        syncFailed: 0,
        subOutboxStats: ZERO_OUTBOX,
      });
    });
    await flush(1);
    assert.equal(label.textContent, '本地模式', '已连接但账户缺失时回落本地模式');
    assert.ok(dot.querySelector('.sync-dot--local'), '账户缺失不得误显示成功绿');
    assert.ok(dot.getAttribute('aria-label').includes('账户缺失（未验证）'),
      '账户缺失事实进入可访问详情');
  } finally {
    useAppStore.setState({ triggerManualSync: originalTrigger });
  }
});

componentTest('SyncTab：44 条等待绑定与订阅 outbox 失败不得显示“本地已保存”', { timeout: 30000 }, async () => {
  installBackend();
  resetPlan();
  resetStore();
  seedFromRows(defaultRows());
  const queueStats = {
    waiting: 7,
    failed: 1,
    local_only: 3,
    bindable_retry: 44,
    bound_pending: 5,
    retry_wait: 2,
    expired_unconfirmed: 5,
    last_error: 'HTTP 503',
  };
  const outboxStats = {
    pending: 2, failed: 1, local_only: 1, last_error: '订阅变更 HTTP 500',
  };
  setPlan({
    sync_status: () => Promise.resolve({
      connected: true,
      endpoint: 'https://reader.example/api/greader.php',
      account: 'alice',
      last_sync: 12345,
      protocol: 'greader',
    }),
    sync_queue_stats: () => Promise.resolve(queueStats),
    subscription_outbox_stats: () => Promise.resolve(outboxStats),
  });
  useAppStore.setState({
    dataMode: 'tauri',
    syncStatus: 'synced',
    syncConnected: true,
    syncAccount: 'alice',
    syncProtocol: 'greader',
    syncLastSync: 12345,
    syncWaiting: queueStats.waiting,
    syncTerminalFailed: queueStats.failed,
    syncRetryWait: queueStats.retry_wait,
    syncLocalOnly: queueStats.local_only,
    syncBindableRetry: queueStats.bindable_retry,
    syncBoundPending: queueStats.bound_pending,
    syncExpiredUnconfirmed: queueStats.expired_unconfirmed,
    syncQueueLastError: queueStats.last_error,
    subOutboxStats: outboxStats,
  });

  const { createElement } = await import('react');
  const { SyncTab } = await import('../../src/components/settings/SyncTab.tsx');
  const entry = await mount(createElement(SyncTab));
  await waitFor(
    () => entry.container.textContent.includes('订阅变更待推送 2 项'),
    { label: 'SyncTab 摘要读取 store 完整分类', timeout: 6000 },
  );

  const text = entry.container.textContent;
  assert.ok(text.includes('状态待推送 7 条'), '状态队列待推送单独显示');
  assert.ok(text.includes('订阅变更待推送 2 项'), '订阅 outbox 待推送单独显示');
  assert.ok(text.includes('仅本机'), '仅本机项显示');
  assert.ok(text.includes('过期未确认 5 条'), '过期未确认项显示');
  assert.ok(text.includes('失败'), '失败计数显示');
  assert.ok(text.includes('等待绑定 44 条'), '等待绑定分类不混入 waiting');
  assert.ok(!text.includes('本地已保存'), '仍有 unresolved/outbox 失败时不得显示本地已保存');
  assert.ok(!text.includes('后端已同步'), '任何 unresolved 不得显示后端已同步');

  const statusCard = [...entry.container.querySelectorAll('.setting-card')]
    .find((el) => el.textContent.includes('同步状态'));
  assert.ok(statusCard, '同步状态卡存在');
  assert.notEqual(statusCard.querySelector('.about-arch-tag')?.textContent, '本地已保存');

  const { act } = await import('react');
  await act(async () => {
    useAppStore.setState({
      subOutboxStats: null,
      syncStatus: 'synced',
      syncWaiting: 0,
      syncTerminalFailed: 0,
      syncRetryWait: 0,
      syncLocalOnly: 0,
      syncBindableRetry: 0,
      syncExpiredUnconfirmed: 0,
      syncQueueLastError: null,
    });
  });
  await flush(2);
  assert.equal(statusCard.querySelector('.about-arch-tag')?.textContent, '订阅未知',
    'outbox 未加载时状态标签必须如实为「订阅未知」');
  assert.ok(statusCard.textContent.includes('订阅状态未加载（未知）'),
    'outbox 未加载时摘要必须说明未知，不得折成无待办');
  assert.ok(!statusCard.textContent.includes('本地已保存'),
    'outbox 未知时不得宣称本地已保存');
});

componentTest('AddFeed：Fever 隐藏账户同步开关并明确仅本地', { timeout: 30000 }, async () => {
  installBackend();
  resetPlan();
  resetStore();
  seedFromRows(defaultRows());
  useAppStore.setState({
    addFeedModalOpen: true,
    addFeedTargetCatId: 'cat-1',
    syncConnected: true,
    syncProtocol: 'fever',
  });

  const { createElement } = await import('react');
  const { AddFeedModal } = await import('../../src/components/Overlays.tsx');
  const entry = await mount(createElement(AddFeedModal));
  await waitFor(
    () => entry.container.textContent.includes('添加订阅源'),
    { label: '添加订阅弹窗打开' },
  );

  assert.ok(!entry.container.textContent.includes('同步到账户'), 'Fever 不显示同步到账户开关');
  assert.ok(entry.container.textContent.includes('Fever：新增订阅仅本地保存'), 'Fever 明确仅本地');
  assert.equal(entry.container.querySelector('.mini-dialog-checkbox input[type="checkbox"]'), null,
    'Fever 不渲染同步开关 checkbox');
});

componentTest('AddFeed：GReader 默认关闭、只在本次新增勾选时触发同步，部分失败保留重试', { timeout: 40000 }, async () => {
  installBackend();
  resetPlan();
  resetStore();
  seedFromRows(defaultRows());
  setPlan({
    add_feed: () => Promise.resolve(null),
    sync_local_feeds: () => Promise.resolve({
      ok: false,
      protocol: 'greader',
      writeBack: true,
      queued: 1,
      pushed: 0,
      failed: 1,
      errors: ['订阅新增失败: HTTP 500'],
      message: '已同步 1 个本地订阅，其中 1 项未同步',
    }),
    sync_status: () => Promise.resolve({
      connected: true,
      endpoint: 'https://reader.example/api/greader.php',
      account: 'alice',
      last_sync: 12345,
      protocol: 'greader',
    }),
  });
  useAppStore.setState({
    dataMode: 'tauri',
    addFeedModalOpen: true,
    addFeedTargetCatId: 'cat-1',
    syncConnected: true,
    syncAccount: 'alice',
    syncProtocol: 'greader',
    syncStatus: 'synced',
    toasts: [],
  });

  const { createElement, act } = await import('react');
  const { AddFeedModal } = await import('../../src/components/Overlays.tsx');
  const entry = await mount(createElement(AddFeedModal));
  await waitFor(
    () => entry.container.textContent.includes('添加订阅源'),
    { label: '添加订阅弹窗打开' },
  );

  const switchInput = entry.container.querySelector('.mini-dialog-checkbox input[type="checkbox"]');
  assert.ok(switchInput, 'GReader 显示同步开关');
  assert.equal(switchInput.checked, false, 'GReader 同步开关默认关闭');
  assert.ok(entry.container.textContent.includes('同步到账户'), '开关使用账户口径');
  assert.ok(entry.container.textContent.includes('勾选「同步到账户」只会添加本次订阅，不推送已有本地订阅'),
    '开关旁说明本次新增边界');

  const urlInput = entry.container.querySelector('input[placeholder="https://example.com/rss.xml"]');
  assert.ok(urlInput, 'RSS 地址输入存在');
  const setter = Object.getOwnPropertyDescriptor(
    globalThis.window.HTMLInputElement.prototype,
    'value',
  ).set;
  await act(async () => {
    setter.call(urlInput, 'https://new.example/rss.xml');
    urlInput.dispatchEvent(new globalThis.window.Event('input', { bubbles: true }));
    switchInput.click();
  });
  await flush(1);
  assert.equal(switchInput.checked, true, '用户可显式勾选本次新增同步');

  const addButton = [...entry.container.querySelectorAll('button')]
    .find((b) => b.textContent.includes('添加订阅'));
  assert.ok(addButton, '添加按钮存在');
  await act(async () => { addButton.click(); });

  await waitFor(
    () => useAppStore.getState().toasts.some((t) => t.text.includes('已添加，1 项未同步')),
    { label: '部分失败 toast 出现', timeout: 8000 },
  );
  const toast = useAppStore.getState().toasts.find((t) => t.text.includes('已添加，1 项未同步'));
  assert.ok(toast.text.includes('HTTP 500'), 'toast 带具体错误');
  assert.equal(toast.action?.label, '重试', '部分失败保留重试入口');
  assert.equal(useAppStore.getState().syncStatus, 'error', '部分失败不得 finishSynced 全域成功');
  const addCall = invokeCalls.find((c) => c.cmd === 'add_feed');
  assert.equal(addCall?.args.syncToBackend, true, '只把用户本次勾选传给新增，不启用后台全量推送');
  const syncCall = invokeCalls.find((c) => c.cmd === 'sync_local_feeds');
  assert.equal(syncCall?.args?.feedUrl, 'https://new.example/rss.xml',
    '定向同步必须携带本次新增的 feedUrl');
  assert.ok(!('feedId' in (syncCall?.args ?? {})),
    '后端未回数字 id 时不得传伪造的 feedId');
});

componentTest('SyncTab：延迟 sync_status 返回不覆盖用户已编辑的表单', { timeout: 30000 }, async () => {
  installBackend();
  resetPlan();
  resetStore();
  seedFromRows(defaultRows());
  const status = deferred();
  setPlan({
    sync_status: () => status.promise,
    subscription_outbox_stats: () => Promise.resolve(ZERO_OUTBOX),
    sync_queue_stats: () => Promise.resolve({
      waiting: 0, failed: 0, local_only: 0, bindable_retry: 0,
      bound_pending: 0, retry_wait: 0, expired_unconfirmed: 0, last_error: null,
    }),
  });
  useAppStore.setState({
    dataMode: 'tauri',
    settingsOpen: false,
    syncConnected: true,
    syncAccount: 'alice',
    syncProtocol: 'greader',
    subOutboxStats: ZERO_OUTBOX,
  });

  const { createElement, act } = await import('react');
  const { SyncTab } = await import('../../src/components/settings/SyncTab.tsx');
  const entry = await mount(createElement(SyncTab));
  await waitFor(
    () => invokeCalls.filter((c) => c.cmd === 'sync_status').length === 1,
    { label: 'SyncTab 挂载发起 sync_status', timeout: 6000 },
  );
  const inputs = [...entry.container.querySelectorAll('input.setting-input')].slice(0, 3);
  assert.equal(inputs.length, 3, 'Endpoint/用户名/密码三个受控输入存在');
  await setInputValue(inputs[0], 'https://draft.example');

  await act(async () => {
    status.resolve({
      connected: true,
      endpoint: 'https://reader.example/api/greader.php',
      account: 'alice',
      last_sync: 12345,
      protocol: 'greader',
    });
    await Promise.resolve();
  });
  await flush(6);
  assert.equal(inputs[0].value, 'https://draft.example', '在途响应不得覆盖已编辑 Endpoint');
  assert.equal(inputs[1].value, '', '在途响应不得顺带覆盖未提交的用户名');
});

componentTest('SyncTab：未编辑时延迟 sync_status 正常应用', { timeout: 30000 }, async () => {
  installBackend();
  resetPlan();
  resetStore();
  seedFromRows(defaultRows());
  const status = deferred();
  setPlan({
    sync_status: () => status.promise,
    subscription_outbox_stats: () => Promise.resolve(ZERO_OUTBOX),
    sync_queue_stats: () => Promise.resolve({
      waiting: 0, failed: 0, local_only: 0, bindable_retry: 0,
      bound_pending: 0, retry_wait: 0, expired_unconfirmed: 0, last_error: null,
    }),
  });
  useAppStore.setState({
    dataMode: 'tauri',
    settingsOpen: false,
    syncConnected: true,
    syncAccount: 'alice',
    syncProtocol: 'greader',
    subOutboxStats: ZERO_OUTBOX,
  });

  const { createElement, act } = await import('react');
  const { SyncTab } = await import('../../src/components/settings/SyncTab.tsx');
  const entry = await mount(createElement(SyncTab));
  await waitFor(
    () => invokeCalls.filter((c) => c.cmd === 'sync_status').length === 1,
    { label: 'SyncTab 挂载发起 sync_status', timeout: 6000 },
  );
  const inputs = [...entry.container.querySelectorAll('input.setting-input')].slice(0, 3);

  await act(async () => {
    status.resolve({
      connected: true,
      endpoint: 'https://reader.example/api/greader.php',
      account: 'alice',
      last_sync: 12345,
      protocol: 'greader',
    });
    await Promise.resolve();
  });
  await waitFor(
    () => inputs[0].value === 'https://reader.example/api/greader.php'
      && inputs[1].value === 'alice',
    { label: '未编辑表单接收延迟响应', timeout: 6000 },
  );
});

componentTest('SyncTab：双在途 sync_status 只应用最新响应，不触发第三次请求', { timeout: 30000 }, async () => {
  installBackend();
  resetPlan();
  resetStore();
  seedFromRows(defaultRows());
  const gates = [];
  setPlan({
    sync_status: () => {
      const gate = deferred();
      gates.push(gate);
      return gate.promise;
    },
    subscription_outbox_stats: () => Promise.resolve(ZERO_OUTBOX),
    sync_queue_stats: () => Promise.resolve({
      waiting: 0, failed: 0, local_only: 0, bindable_retry: 0,
      bound_pending: 0, retry_wait: 0, expired_unconfirmed: 0, last_error: null,
    }),
  });
  useAppStore.setState({
    dataMode: 'tauri',
    settingsOpen: true,
    syncConnected: true,
    syncAccount: 'alice',
    syncProtocol: 'greader',
    subOutboxStats: ZERO_OUTBOX,
  });

  try {
    const { createElement, act } = await import('react');
    const { SyncTab } = await import('../../src/components/settings/SyncTab.tsx');
    const entry = await mount(createElement(SyncTab));
    await waitFor(() => gates.length === 2, { label: '挂载 + 打开设置形成双在途', timeout: 6000 });
    const inputs = [...entry.container.querySelectorAll('input.setting-input')].slice(0, 3);
    assert.equal(inputs.length, 3);

    await act(async () => {
      gates[0].resolve({
        connected: true,
        endpoint: 'https://old.example',
        account: 'old',
        last_sync: 1,
        protocol: 'greader',
      });
      await Promise.resolve();
    });
    await flush(2);
    assert.equal(inputs[0].value, '', '较早的旧响应不得写入表单');

    await act(async () => {
      gates[1].resolve({
        connected: true,
        endpoint: 'https://new.example',
        account: 'new',
        last_sync: 2,
        protocol: 'greader',
      });
      await Promise.resolve();
    });
    await waitFor(
      () => inputs[0].value === 'https://new.example' && inputs[1].value === 'new',
      { label: '最新响应回填表单', timeout: 6000 },
    );
    await flush(8);
    assert.equal(gates.length, 2, '最新响应造成的 identity 变化不得追发第三次请求');
  } finally {
    resetPlan();
  }
});

componentTest('SyncTab：断开期间旧 sync_status 响应不得写回，请求序号先失效', { timeout: 40000 }, async () => {
  installBackend();
  resetPlan();
  resetStore();
  seedFromRows(defaultRows());
  const statusGate = deferred();
  const disconnectGate = deferred();
  let statusCalls = 0;
  setPlan({
    sync_status: () => {
      statusCalls += 1;
      if (statusCalls === 1) return statusGate.promise;
      return Promise.resolve({
        connected: false,
        endpoint: null,
        account: null,
        last_sync: 0,
        protocol: null,
      });
    },
    sync_disconnect: () => disconnectGate.promise,
    subscription_outbox_stats: () => Promise.resolve(ZERO_OUTBOX),
    sync_queue_stats: () => Promise.resolve({
      waiting: 0, failed: 0, local_only: 0, bindable_retry: 0,
      bound_pending: 0, retry_wait: 0, expired_unconfirmed: 0, last_error: null,
    }),
  });
  useAppStore.setState({
    dataMode: 'tauri',
    settingsOpen: false,
    syncConnected: true,
    syncAccount: 'alice',
    syncProtocol: 'greader',
    subOutboxStats: ZERO_OUTBOX,
  });

  try {
    const { createElement, act } = await import('react');
    const { SyncTab } = await import('../../src/components/settings/SyncTab.tsx');
    const entry = await mount(createElement(SyncTab));
    await waitFor(() => statusCalls === 1, { label: '挂载形成在途 status', timeout: 6000 });
    const inputs = [...entry.container.querySelectorAll('input.setting-input')].slice(0, 3);
    const disconnectButton = [...entry.container.querySelectorAll('button')]
      .find((b) => b.textContent.includes('断开连接'));
    assert.ok(disconnectButton, '已连接时存在断开按钮');

    await act(async () => { disconnectButton.click(); });
    await flush(2);
    const confirmButton = [...document.body.querySelectorAll('button')]
      .find((b) => b.textContent.includes('断开并清理'));
    assert.ok(confirmButton, '断开确认按钮存在');
    await act(async () => { confirmButton.click(); });
    await flush(2);

    await act(async () => {
      statusGate.resolve({
        connected: true,
        endpoint: 'https://stale.example',
        account: 'stale',
        last_sync: 99,
        protocol: 'greader',
      });
      await Promise.resolve();
    });
    await flush(4);
    assert.equal(inputs[0].value, '', '断开开始后旧 status 不得写回 Endpoint');
    assert.equal(inputs[1].value, '', '断开开始后旧 status 不得写回用户名');

    await act(async () => {
      disconnectGate.resolve('已断开连接');
      await Promise.resolve();
    });
    await waitFor(
      () => useAppStore.getState().syncConnected === false,
      { label: '断开完成写入 store', timeout: 6000 },
    );
    await flush(6);
    assert.equal(inputs[0].value, '', '断开后的表单保持空白');
    assert.equal(inputs[1].value, '', '断开后的用户名保持空白');
    assert.equal(useAppStore.getState().syncConnected, false);
  } finally {
    resetPlan();
  }
});

componentTest('SettingsModal：关闭重开同步账户 A→B 显示 B，dirty 时保留编辑', { timeout: 40000 }, async () => {
  installBackend();
  resetPlan();
  resetStore();
  seedFromRows(defaultRows());
  let statusReply = {
    connected: true,
    endpoint: 'https://a.example',
    account: 'alice',
    last_sync: 1,
    protocol: 'greader',
  };
  setPlan({
    sync_status: () => Promise.resolve(statusReply),
    subscription_outbox_stats: () => Promise.resolve(ZERO_OUTBOX),
    sync_queue_stats: () => Promise.resolve({
      waiting: 0, failed: 0, local_only: 0, bindable_retry: 0,
      bound_pending: 0, retry_wait: 0, expired_unconfirmed: 0, last_error: null,
    }),
  });
  useAppStore.setState({
    dataMode: 'tauri',
    settingsOpen: true,
    settingsTab: 'sync',
    syncConnected: true,
    syncAccount: 'alice',
    syncProtocol: 'greader',
    syncEndpoint: 'https://a.example',
    subOutboxStats: ZERO_OUTBOX,
  });

  const { createElement, act } = await import('react');
  const { SettingsModal } = await import('../../src/components/SettingsModal.tsx');
  const entry = await mount(createElement(SettingsModal));
  const inputs = () => [...entry.container.querySelectorAll('input.setting-input')].slice(0, 3);
  await waitFor(
    () => inputs()[0]?.value === 'https://a.example' && inputs()[1]?.value === 'alice',
    { label: '设置初次打开显示 A 账户', timeout: 6000 },
  );

  await act(async () => { useAppStore.setState({ settingsOpen: false }); });
  await flush(2);
  statusReply = {
    connected: true,
    endpoint: 'https://b.example',
    account: 'bob',
    last_sync: 2,
    protocol: 'greader',
  };
  await act(async () => { useAppStore.setState({ settingsOpen: true }); });
  await waitFor(
    () => inputs()[0]?.value === 'https://b.example' && inputs()[1]?.value === 'bob',
    { label: '重开设置应用外部 A→B 身份', timeout: 6000 },
  );

  await setInputValue(inputs()[0], 'https://draft.example');
  await act(async () => { useAppStore.setState({ settingsOpen: false }); });
  await flush(2);
  statusReply = {
    connected: true,
    endpoint: 'https://c.example',
    account: 'carol',
    last_sync: 3,
    protocol: 'greader',
  };
  await act(async () => { useAppStore.setState({ settingsOpen: true }); });
  await waitFor(
    () => invokeCalls.filter((c) => c.cmd === 'sync_status').length >= 4,
    { label: '重开设置发起新一轮 sync_status', timeout: 6000 },
  );
  await flush(4);
  assert.equal(inputs()[0].value, 'https://draft.example',
    'dirty 表单不得被重开触发的刷新覆盖');
  assert.equal(inputs()[1].value, 'bob',
    'dirty 表单保留当前用户名，不跳到后端新账户');
});

componentTest('AddFeed：GReader 勾选后协议切 Fever 自动清空，提交不写回', { timeout: 30000 }, async () => {
  installBackend();
  resetPlan();
  resetStore();
  seedFromRows(defaultRows());
  setPlan({ add_feed: () => Promise.resolve({ id: 77 }) });
  useAppStore.setState({
    dataMode: 'tauri',
    addFeedModalOpen: true,
    addFeedTargetCatId: 'cat-1',
    syncConnected: true,
    syncProtocol: 'greader',
    toasts: [],
  });

  const { createElement, act } = await import('react');
  const { AddFeedModal } = await import('../../src/components/Overlays.tsx');
  const entry = await mount(createElement(AddFeedModal));
  await waitFor(
    () => entry.container.textContent.includes('添加订阅源'),
    { label: '添加订阅弹窗打开' },
  );
  const urlInput = entry.container.querySelector('input[placeholder="https://example.com/rss.xml"]');
  await setInputValue(urlInput, 'https://fever-race.example/rss.xml');

  const syncSwitch = () => entry.container
    .querySelector('.mini-dialog-checkbox input[type="checkbox"]');
  assert.equal(syncSwitch()?.checked, false, 'GReader 同步开关默认关闭');
  await act(async () => { syncSwitch().click(); });
  await flush(1);
  assert.equal(syncSwitch()?.checked, true, '前置：用户已勾选同步到账户');

  await act(async () => { useAppStore.setState({ syncProtocol: 'fever' }); });
  await flush(2);
  assert.equal(syncSwitch(), null, '协议切 Fever 后同步开关立即隐藏');
  assert.ok(entry.container.textContent.includes('Fever：新增订阅仅本地保存'));

  await act(async () => { useAppStore.setState({ syncProtocol: 'greader' }); });
  await flush(2);
  assert.ok(syncSwitch(), '切回 GReader 后同步开关重新出现');
  assert.equal(syncSwitch().checked, false, 'Fever 切换已清空旧勾选，不得残留');

  const addButton = [...entry.container.querySelectorAll('button')]
    .find((b) => b.textContent.includes('添加订阅'));
  await act(async () => { addButton.click(); });
  await waitFor(
    () => invokeCalls.some((c) => c.cmd === 'add_feed'),
    { label: 'add_feed 已发出', timeout: 6000 },
  );
  const addCall = invokeCalls.find((c) => c.cmd === 'add_feed');
  assert.equal(addCall.args.syncToBackend, false,
    '协议往返后旧勾选不得随提交写回');
  assert.ok(!invokeCalls.some((c) => c.cmd === 'sync_local_feeds'),
    '未勾选本次同步时不得调用 sync_local_feeds');
});

componentTest('AddFeed：旧纯文本同步回执显示“同步状态未知”，不再冒充成功', { timeout: 30000 }, async () => {
  installBackend();
  resetPlan();
  resetStore();
  seedFromRows(defaultRows());
  setPlan({
    add_feed: () => Promise.resolve({ id: 88 }),
    sync_local_feeds: () => Promise.resolve('已同步 1 个本地订阅'),
    sync_status: () => Promise.resolve({
      connected: true,
      endpoint: 'https://reader.example/api/greader.php',
      account: 'alice',
      last_sync: 12345,
      protocol: 'greader',
    }),
  });
  useAppStore.setState({
    dataMode: 'tauri',
    addFeedModalOpen: true,
    addFeedTargetCatId: 'cat-1',
    syncConnected: true,
    syncAccount: 'alice',
    syncProtocol: 'greader',
    toasts: [],
  });

  const { createElement, act } = await import('react');
  const { AddFeedModal } = await import('../../src/components/Overlays.tsx');
  const entry = await mount(createElement(AddFeedModal));
  await waitFor(
    () => entry.container.textContent.includes('添加订阅源'),
    { label: '添加订阅弹窗打开' },
  );
  await setInputValue(
    entry.container.querySelector('input[placeholder="https://example.com/rss.xml"]'),
    'https://legacy.example/rss.xml',
  );
  const switchInput = entry.container.querySelector('.mini-dialog-checkbox input[type="checkbox"]');
  await act(async () => { switchInput.click(); });
  await flush(1);
  const addButton = [...entry.container.querySelectorAll('button')]
    .find((b) => b.textContent.includes('添加订阅'));
  await act(async () => { addButton.click(); });

  await waitFor(
    () => useAppStore.getState().toasts.some((t) => t.text.includes('同步状态未知')),
    { label: '旧回执 toast 标为未知', timeout: 8000 },
  );
  const toast = useAppStore.getState().toasts.find((t) => t.text.includes('同步状态未知'));
  assert.ok(toast.text.includes('已添加，同步状态未知'),
    `不得显示“已添加订阅源”式假成功：${toast.text}`);
  assert.equal(toast.action?.label, '重试', '未知回执保留可重试入口');
  assert.equal(useAppStore.getState().syncStatus, 'error',
    '无法确认的同步结果不得发布成功终态');
});
