// 2026-11-04：用户文案精简的真实挂载回归。
// 证据边界：jsdom 验证 DOM 结构、可访问属性、真实 click 与异步错误/重试链；
// 不替代 Edge/CDP 的几何量测。
import assert from 'node:assert/strict';
import { componentTest, mount, flush, waitFor, resetStore, useAppStore } from './harness.mjs';
import {
  installBackend,
  seedFromRows,
  mkRow,
  setPlan,
  resetPlan,
  clearInvokeCalls,
} from './fixtures.mjs';

const ZERO_QUEUE = {
  waiting: 0, failed: 0, local_only: 0, bindable_retry: 0,
  bound_pending: 0, retry_wait: 0, expired_unconfirmed: 0, last_error: null,
};
const ZERO_OUTBOX = { pending: 0, failed: 0, local_only: 0, last_error: null };

function capsFixture(protocol) {
  if (protocol === 'fever') {
    return {
      connected: true,
      protocol: 'fever',
      overview: 'Fever 仅同步阅读状态；订阅与分类改动只保存在本机。',
      subscribe: { support: 'LocalOnly', detail: '仅本地：新订阅只保存在本机，同步服务不会改变。' },
      edit_title: { support: 'LocalOnly', detail: '仅本地：编辑只保存在本机，同步服务不会改变。' },
      move_folder: { support: 'LocalOnly', detail: '仅本地：分类变化只保存在本机，同步服务不会改变。' },
      unsubscribe: { support: 'LocalOnly', detail: '仅本地：删除只移除本机订阅，同步服务中的订阅保持不变。' },
      rename_folder: { support: 'LocalOnly', detail: '仅本地：分类改名只在本机生效，同步服务不会改变。' },
      delete_folder: { support: 'LocalOnly', detail: '仅本地：分类删除只在本机生效，同步服务中的订阅与分类保持不变。' },
      empty_folder_remote: { support: 'Unsupported', detail: '仅本地：空分类只从本机删除或改名，同步服务没有对应变更。' },
      limitations: [],
    };
  }
  return {
    connected: true,
    protocol: 'greader',
    overview: 'GReader 会写回新增、编辑、移动、退订与分类变更；分类按成员逐源处理。',
    subscribe: { support: 'Supported', detail: '会添加到同步账户；发送失败会自动重试，离线或重启后仍会继续。' },
    edit_title: { support: 'Supported', detail: '标题变更会写入同步账户；发送失败会自动重试，离线或重启后仍会继续。' },
    move_folder: { support: 'Supported', detail: '会加入目标分类并移出原分类；发送失败会自动重试。' },
    unsubscribe: { support: 'Supported', detail: '删除会尝试从同步账户退订；离线或重启后仍会继续，直到账户确认。' },
    rename_folder: { support: 'Supported', detail: '分类改名会按成员逐源处理：为每个订阅加入新分类并移出旧分类；空分类只在本机生效。' },
    delete_folder: { support: 'Supported', detail: '删除分类会按成员逐源退订；同步服务中的分类在成员全部移除后消失。' },
    empty_folder_remote: { support: 'Unsupported', detail: '空分类只在本机删除或改名；同步服务没有可对应的分类变更。' },
    limitations: [
      { code: 'folder_member_wire', detail: '分类改名/删除会按成员逐源处理，不会一次改掉同步服务中的整个分类。' },
      { code: 'folder_commands_defer_push', detail: '分类改动不会立即推送，要等下一次同步（自动或手动）。' },
    ],
  };
}

function planSync(protocol) {
  setPlan({
    sync_status: () => Promise.resolve({
      connected: true,
      endpoint: 'https://reader.example/api/greader.php',
      account: 'alice',
      last_sync: 12345,
      protocol,
    }),
    sync_queue_stats: () => Promise.resolve(ZERO_QUEUE),
    subscription_outbox_stats: () => Promise.resolve(ZERO_OUTBOX),
    subscription_capabilities: () => Promise.resolve(capsFixture(protocol)),
    update_folder_layout: () => Promise.resolve(null),
  });
  useAppStore.setState({
    dataMode: 'tauri',
    syncStatus: 'synced',
    syncConnected: true,
    syncAccount: 'alice',
    syncProtocol: protocol,
    syncEndpoint: 'https://reader.example/api/greader.php',
    syncLastSync: 12345,
    syncWaiting: 0,
    syncTerminalFailed: 0,
    syncRetryWait: 0,
    syncLocalOnly: 0,
    syncBindableRetry: 0,
    syncExpiredUnconfirmed: 0,
    syncQueueLastError: null,
    subOutboxStats: { ...ZERO_OUTBOX },
  });
}

function cardByTitle(container, title) {
  return [...container.querySelectorAll('.setting-card')]
    .find((card) => card.querySelector('h5')?.textContent === title);
}

function cardDesc(container, title) {
  return cardByTitle(container, title)?.querySelector('p')?.textContent ?? '';
}

function buttonByText(container, text) {
  return [...container.querySelectorAll('button')]
    .find((button) => button.textContent.includes(text));
}

function typeInput(input, value) {
  const setter = Object.getOwnPropertyDescriptor(
    globalThis.window.HTMLInputElement.prototype,
    'value',
  )?.set;
  assert.ok(setter, '应从 InputElement.prototype 取得受控 value setter');
  setter.call(input, value);
  input.dispatchEvent(new globalThis.window.Event('input', { bubbles: true }));
}

componentTest('文案精简：SyncTab 单句能力概述、细节折叠、按钮 title 与失败态不冒充已同步', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows([mkRow({ id: 101, title: 'A1' })]);
  planSync('greader');
  useAppStore.setState({
    syncStatus: 'error',
    syncTerminalFailed: 1,
    syncQueueLastError: '状态推送失败',
  });

  try {
    const { createElement, act } = await import('react');
    const { SyncTab } = await import('../../src/components/settings/SyncTab.tsx');
    const entry = await mount(createElement(SyncTab));
    await waitFor(
      () => cardDesc(entry.container, '订阅写回').includes('GReader 会写回'),
      { label: 'GReader 能力概述落位', timeout: 8000 },
    );

    assert.equal(
      cardDesc(entry.container, '订阅写回'),
      'GReader 会写回新增、编辑、移动、退订与分类变更；分类按成员逐源处理。',
      'GReader 主界面只保留一句能力概述',
    );
    assert.ok(!entry.container.textContent.includes('新增 已支持'), '矩阵默认收起，不占常驻高度');
    assert.ok(!entry.container.textContent.includes('后端已同步'), '失败/待处理不得显示「后端已同步」');

    const detailsBtn = buttonByText(entry.container, '查看写回细节');
    assert.ok(detailsBtn, '存在写回细节入口');
    assert.equal(detailsBtn.getAttribute('aria-expanded'), 'false');
    assert.equal(detailsBtn.getAttribute('aria-controls'), 'subscription-writeback-details');
    await act(async () => { detailsBtn.click(); });
    await flush(2);
    assert.equal(detailsBtn.getAttribute('aria-expanded'), 'true');
    assert.ok(entry.container.textContent.includes('新增 已支持'), '展开后显示逐动作矩阵');
    assert.ok(entry.container.textContent.includes('分类改名/删除会按成员逐源处理'), '展开后显示限制详情');
    assert.ok(!entry.container.textContent.includes('墓碑') && !entry.container.textContent.includes('no-op'),
      '细节也不暴露内部实现词');

    const testBtn = buttonByText(entry.container, '测试连接');
    const saveBtn = buttonByText(entry.container, '保存并同步');
    assert.ok(testBtn?.getAttribute('title')?.includes('只验证登录'), '测试连接语义在 title');
    assert.ok(testBtn.getAttribute('title').includes('不会保存配置'));
    assert.ok(saveBtn?.getAttribute('title')?.includes('拉取订阅、阅读状态与文章'), '保存并同步语义在 title');
    assert.ok(saveBtn.getAttribute('title').includes('不会推送已有本地订阅'));
    assert.equal(testBtn.getAttribute('aria-describedby'), 'sync-test-help');
    assert.equal(saveBtn.getAttribute('aria-describedby'), 'sync-save-help');
    const actionHelp = entry.container.querySelector('#sync-action-help');
    assert.ok(actionHelp, '操作说明节点存在');
    assert.equal(actionHelp.hidden, true, '说明默认不占常驻高度');
    await act(async () => { testBtn.focus(); });
    await flush(1);
    assert.equal(actionHelp.hidden, false, '键盘 focus 后说明可见');
    assert.ok(actionHelp.textContent.includes('只验证登录与地址'), '测试连接说明可见');
    assert.ok(actionHelp.textContent.includes('不会推送已有本地订阅'), '保存并同步说明可见');

    const passwordDesc = cardDesc(entry.container, '密码');
    assert.ok(passwordDesc.includes('填写新值只更新密码'), '密码新值明确为更新密码');
    assert.ok(passwordDesc.includes('换账号需同时改地址或用户名'), '换账号方向明确');
    assert.ok(!passwordDesc.includes('更换账号'), '不得把改密码写成更换账号');
    assert.ok(!entry.container.textContent.includes('「测试连接」仅验证登录；'), '常驻解释行已删除');

    const disconnectBtn = buttonByText(entry.container, '断开连接');
    assert.ok(disconnectBtn, '断开入口存在');
    await act(async () => { disconnectBtn.click(); });
    await flush(2);
    const disconnectMsg = document.body.querySelector('.confirm-message')?.textContent ?? '';
    assert.ok(disconnectMsg.includes('移除从同步服务拉取的订阅与文章'), '断开确认保留数据移除范围');
    assert.ok(disconnectMsg.includes('本地直连订阅及其已读/收藏值保留'), '断开确认保留本地值例外');
    assert.ok(disconnectMsg.includes('旧意图不会在重连后自动补推'), '保留值不等于保留旧同步意图');
  } finally {
    resetPlan();
  }
});

componentTest('文案精简：Fever 主句只讲阅读状态与本地边界，细节按需展开', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows([mkRow({ id: 101, title: 'A1' })]);
  planSync('fever');

  try {
    const { createElement, act } = await import('react');
    const { SyncTab } = await import('../../src/components/settings/SyncTab.tsx');
    const entry = await mount(createElement(SyncTab));
    await waitFor(
      () => cardDesc(entry.container, '订阅写回').includes('Fever 仅同步阅读状态'),
      { label: 'Fever 能力概述落位', timeout: 8000 },
    );

    assert.equal(
      cardDesc(entry.container, '订阅写回'),
      'Fever 仅同步阅读状态；订阅与分类改动只保存在本机。',
      'Fever 主界面只保留一句边界说明',
    );
    assert.ok(cardByTitle(entry.container, '订阅写回')?.textContent.includes('仅本地'));
    assert.ok(!entry.container.textContent.includes('新增 仅本地'), 'Fever 矩阵默认收起');
    const detailsBtn = buttonByText(entry.container, '查看写回细节');
    await act(async () => { detailsBtn.click(); });
    await flush(2);
    assert.ok(entry.container.textContent.includes('新增 仅本地'), '展开后逐动作明确仅本地');
  } finally {
    resetPlan();
  }
});

componentTest('文案精简：FeedsTab 删除顶栏教学句，Fever 操作附近写明仅本地', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows([mkRow({ id: 101, title: 'A1' })]);
  setPlan({ subscription_capabilities: () => Promise.resolve(capsFixture('fever')) });

  try {
    const { createElement, act } = await import('react');
    const { FeedsTab } = await import('../../src/components/settings/FeedsTab.tsx');
    const entry = await mount(createElement(FeedsTab));
    await waitFor(
      () => entry.container.textContent.includes('Fever 仅同步阅读状态'),
      { label: 'Fever 能力提示落位', timeout: 8000 },
    );

    assert.ok(!entry.container.textContent.includes('点击分类名可展开/收起管理组内源'),
      '顶栏教学句已删除');
    assert.ok(entry.container.querySelector('.feeds-tab-actions'), '工具栏按钮仍成组贴右');
    assert.ok(entry.container.textContent.includes('Fever 仅同步阅读状态；订阅与分类改动只保存在本机。'),
      'Fever 常驻句如实');

    const addButton = entry.container.querySelector('.group-mgr-controls .toggle-action-btn');
    const editButton = entry.container.querySelector('.group-mgr-feed-controls .icon-btn');
    assert.ok(addButton?.getAttribute('title')?.includes('仅本地'), '新增附近写明仅本地');
    assert.ok(editButton?.getAttribute('title')?.includes('仅本地'), '编辑附近写明仅本地');

    await act(async () => {
      useAppStore.setState({
        syncConnected: true,
        syncProtocol: 'greader',
        subscriptionCapabilities: null,
      });
    });
    await flush(2);
    const categoryDelete = [...entry.container.querySelectorAll('.group-mgr-controls .btn-danger-text')]
      .find((button) => button.textContent.includes('删除'));
    assert.ok(categoryDelete, '分类删除入口存在');
    await act(async () => { categoryDelete.click(); });
    await flush(2);
    const unknownCapsMsg = document.body.querySelector('.confirm-message')?.textContent ?? '';
    assert.ok(unknownCapsMsg.includes('无法确认此操作对同步账户的影响'),
      '能力未加载时明确账户影响未知');
    assert.ok(unknownCapsMsg.includes('仅按本地后果继续'), '仍给出可执行的继续边界');
  } finally {
    resetPlan();
  }
});

componentTest('文案精简：AddFeed 不承诺自动同步，Fever 只写本机', { timeout: 30000 }, async () => {
  installBackend();
  resetStore();
  seedFromRows([mkRow({ id: 101, title: 'A1' })]);
  useAppStore.setState({
    addFeedModalOpen: true,
    addFeedTargetCatId: 'cat-1',
    syncConnected: false,
    syncProtocol: null,
  });

  try {
    const { createElement, act } = await import('react');
    const { AddFeedModal } = await import('../../src/components/Overlays.tsx');
    const entry = await mount(createElement(AddFeedModal));
    await waitFor(
      () => entry.container.textContent.includes('添加订阅源'),
      { label: 'AddFeed 打开' },
    );

    assert.ok(!entry.container.textContent.includes('自动同步'), '不得承诺连接后自动同步');
    assert.ok(entry.container.textContent.includes('连接同步服务后，可在「同步本地订阅」中确认是否推送'),
      '未连接只给可选未来动作');
    assert.ok(!entry.container.textContent.includes('请在同步页'), '未连接不得承诺页面自动引导');

    await act(async () => {
      useAppStore.setState({ syncConnected: true, syncProtocol: 'greader' });
    });
    await flush(2);
    assert.ok(entry.container.textContent.includes('勾选「同步到账户」只会添加本次订阅，不推送已有本地订阅'),
      'GReader 只同步本次新增的边界保留');
    assert.ok(entry.container.textContent.includes('同步到账户'), 'GReader 显示同步开关');

    await act(async () => {
      useAppStore.setState({ syncProtocol: 'fever' });
    });
    await flush(2);
    assert.ok(entry.container.textContent.includes('Fever：新增订阅仅本地保存'),
      'Fever 操作旁明确仅本地');
    assert.ok(!entry.container.textContent.includes('连接同步服务后'), 'Fever 不继承未连接提示');
    assert.ok(!entry.container.textContent.includes('同步本地订阅'), 'Fever 不承诺后续推送');
    assert.ok(!entry.container.textContent.includes('同步到账户'), 'Fever 不显示同步开关');
    assert.ok(!entry.container.textContent.includes('自动同步'), 'Fever 同样不得出现自动同步');
  } finally {
    resetPlan();
  }
});

componentTest('文案精简：本地搜索失败提供重试，不再误报网络问题', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows([mkRow({ id: 101, title: 'A1', published_at: '2026-10-01T10:00:00Z' })]);
  let attempts = 0;
  setPlan({
    search_articles: () => {
      attempts += 1;
      return attempts === 1
        ? Promise.reject(new Error('本地 DB busy'))
        : Promise.resolve([mkRow({ id: 101, title: 'A1' })]);
    },
  });

  try {
    const { createElement, act } = await import('react');
    const { default: App } = await import('../../src/App.tsx');
    const entry = await mount(createElement(App));
    await waitFor(() => entry.container.querySelectorAll('.article-card').length === 1,
      { label: 'App 启动', timeout: 8000 });
    await act(async () => { useAppStore.getState().openSearch(); });
    await flush(2);

    const input = entry.container.querySelector('.search-modal-input');
    assert.ok(input, '搜索面板出现');
    await act(async () => { typeInput(input, '添加'); });
    await flush(2, 300);
    await waitFor(() => entry.container.querySelector('.search-error-state'),
      { label: '本地搜索失败态', timeout: 8000 });
    assert.ok(entry.container.textContent.includes('本地搜索失败'));
    assert.ok(entry.container.textContent.includes('添加订阅源'),
      '命令结果仍显示时错误也独立可见');
    assert.ok(!entry.container.textContent.includes('请检查网络'), '不得把本地错误指向网络');
    assert.equal(attempts, 1);

    const retry = entry.container.querySelector('.search-error-state button');
    assert.ok(retry, '搜索错误提供重试按钮');
    await act(async () => { retry.click(); });
    await waitFor(() => attempts === 2, { label: '重试发出第二次搜索', timeout: 8000 });
    await waitFor(() => !entry.container.querySelector('.search-error-state'),
      { label: '重试后错误态消失', timeout: 8000 });
    assert.ok(entry.container.textContent.includes('A1'), '重试命中后展示搜索结果');
  } finally {
    resetPlan();
  }
});

componentTest('文案精简：时间线空态区分没有订阅源与当前布局没有绑定', { timeout: 30000 }, async () => {
  installBackend();
  resetStore();
  useAppStore.setState({
    dataMode: 'tauri',
    categories: [],
    entries: [],
    articlesLoading: false,
    backendReloadsInFlight: 0,
    articlesExhausted: true,
  });

  const { createElement, act } = await import('react');
  const { Timeline } = await import('../../src/components/Timeline.tsx');
  const entry = await mount(createElement(Timeline));
  await flush(2);
  assert.ok(entry.container.textContent.includes('还没有订阅源，先添加一个'),
    '完全无订阅源时给出添加方向');

  await act(async () => {
    useAppStore.setState({
      categories: [{
        id: 'cat-1',
        name: '技术',
        collapsed: false,
        settingsCollapsed: false,
        layout: 'article',
        autoSummary: false,
        autoTranslate: false,
        feeds: [{
          id: '11',
          name: '源B',
          url: 'https://b.example/rss',
          favicon: '',
          layout: 'social',
          autoSummary: false,
          autoTranslate: false,
          summaryMode: 'inherit',
          translateMode: 'inherit',
        }],
      }],
    });
  });
  await flush(2);
  assert.ok(entry.container.textContent.includes('当前布局还没有绑定订阅源'),
    '有订阅但当前布局没有绑定时说明布局原因');
});

componentTest('文案精简：布局、全部折叠与正文翻译成功态不再弹低价值 toast', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows([
    mkRow({
      id: 201,
      feed_id: 11,
      title: '社交卡',
      snippet: '已有译文',
      translated_content: '<p>已有译文</p>',
    }),
  ], { layout: 'social' });
  setPlan({ update_folder_layout: () => Promise.resolve(null) });
  const mediaProto = globalThis.window.HTMLMediaElement.prototype;
  const originalPlay = mediaProto.play;
  const originalPause = mediaProto.pause;

  try {
    const { createElement, act } = await import('react');
    const { default: App } = await import('../../src/App.tsx');
    const { FeedsTab } = await import('../../src/components/settings/FeedsTab.tsx');
    const app = await mount(createElement(App));
    await waitFor(() => app.container.querySelector('.social-card'),
      { label: '社交卡挂载', timeout: 8000 });

    const collapseAll = app.container.querySelector('.icon-sub-btn[title="展开/收起全部"]');
    await act(async () => { collapseAll.click(); });
    await flush(2);
    assert.ok(!useAppStore.getState().toasts.some((t) =>
      t.text.includes('已收起全部分类') || t.text.includes('已展开全部分类')),
      '全部折叠不弹成功 toast');

    const translate = [...app.container.querySelectorAll('.social-card .social-act-item')]
      .find((button) => button.textContent.includes('翻译'));
    assert.ok(translate, '翻译入口存在');
    await act(async () => { translate.click(); });
    await flush(2);
    assert.ok(!useAppStore.getState().toasts.some((t) =>
      t.text.includes('已显示正文翻译') || t.text.includes('已隐藏正文翻译')),
      '正文翻译显隐不弹成功 toast');

    const feeds = await mount(createElement(FeedsTab));
    await flush(2);
    const trigger = feeds.container.querySelector('.group-mgr-layout-control .flux-dropdown-trigger');
    assert.ok(trigger, '分类布局下拉存在');
    await act(async () => { trigger.click(); });
    await flush(2);
    const option = [...document.body.querySelectorAll('.flux-dropdown-option')]
      .find((node) => node.textContent.includes('社交'));
    assert.ok(option, '布局选项出现');
    await act(async () => { option.click(); });
    await flush(2);
    assert.equal(useAppStore.getState().categories[0]?.layout, 'social', '布局真实更新');
    assert.ok(!useAppStore.getState().toasts.some((t) => t.text === '布局已更新'),
      '布局已更新不再弹成功 toast');

    globalThis.__OPEN_URL_ERROR__ = new Error('no default browser');
    const external = [...app.container.querySelectorAll('.social-card .social-act-item')]
      .find((button) => button.textContent.includes('查看原文'));
    assert.ok(external, '外链入口存在');
    await act(async () => { external.click(); });
    await flush(2);
    const externalToast = useAppStore.getState().toasts.find((t) =>
      t.text.includes('无法在浏览器中打开此链接'));
    assert.ok(externalToast, '外链失败可见');
    assert.ok(externalToast.text.includes('检查默认浏览器设置'), '外链失败给出下一步');
    assert.ok(externalToast.text.includes('稍后重试'), '外链失败保留重试方向');

    mediaProto.play = () => Promise.reject(new Error('blocked'));
    mediaProto.pause = () => {};
    await act(async () => {
      useAppStore.setState((s) => ({
        player: {
          ...s.player,
          isActive: true,
          isPlaying: true,
          audioUrl: 'https://audio.example/fail.mp3',
          title: '失败音频',
          showName: '测试',
        },
      }));
    });
    await flush(2);
    const audio = app.container.querySelector('.podcast-bottom-bar audio');
    assert.ok(audio, '播放器音频节点存在');
    await act(async () => { audio.dispatchEvent(new globalThis.window.Event('error')); });
    await flush(2);
    const audioToasts = useAppStore.getState().toasts.filter((t) =>
      t.text.includes('无法播放音频'));
    assert.equal(audioToasts.length, 1, '同一次音频失败只弹一条 toast');
    assert.equal(audioToasts[0].action?.label, '重试播放', '音频失败保留一次重试动作');
  } finally {
    mediaProto.play = originalPlay;
    mediaProto.pause = originalPause;
    delete globalThis.__OPEN_URL_ERROR__;
    resetPlan();
  }
});
