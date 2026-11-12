// 同步灯/账户状态机与账户事实源回归。
//
// 契约（见 .agents/notes/implemented/feature/2026-11-04-同步灯与账户状态.md）：
// - 常驻文案只显示账户名/本地模式/同步中/同步完成/同步失败，数量只进详情；
// - 同步中优先于旧失败；空闲后 error/队列失败才进入失败态；
// - 成功窗口由 store 的 syncSucceededAt 驱动，到期恢复账户名；
// - sync_status 的 account/protocol/last_sync 只写同步字段，不得复用 WebDAV 资料库状态；
// - SyncQueueStats / SubscriptionOutboxStats 都进入灯色与详情。
export const id = 'sync-indicator';

export async function run(ctx) {
  const { checkNew, store, nTick, bootFixture, overrideGlobal, S } = ctx;
  await ctx.useMainBackend();
  await bootFixture();
  await nTick(20);

  const {
    syncPillLabel,
    syncPillView,
    syncStateSummary,
    SYNC_SUCCESS_LABEL_MS,
  } = await import('../../src/lib/syncPill.ts');
  const { api } = await import('../../src/lib/api.ts');
  const fs = await import('node:fs');
  const baseCss = fs.readFileSync(new URL('../../src/styles/base.css', import.meta.url), 'utf8');

  const now = 1_800_000_000_000;
  const base = {
    syncStatus: 'synced',
    backgroundSyncing: false,
    syncConnected: true,
    account: 'alice',
    protocol: 'greader',
    waiting: 0,
    failed: 0,
    subscription: { pending: 0, failed: 0, local_only: 0, last_error: null },
    now,
  };

  checkNew('(sync-light-1) 文案优先级：同步中 > 空闲失败 > 成功短窗 > 账户名；无账户回落本地模式',
    syncPillLabel({ ...base, syncStatus: 'error' }) === '同步失败'
    && syncPillLabel({ ...base, syncStatus: 'error', backgroundSyncing: true }) === '同步中'
    && syncPillView({ ...base, syncStatus: 'error', backgroundSyncing: true }).tone === 'syncing'
    && syncPillLabel({ ...base, syncStatus: 'syncing' }) === '同步中'
    && syncPillLabel({
      ...base,
      syncStatus: 'synced',
      succeededAt: now - SYNC_SUCCESS_LABEL_MS + 1,
    }) === '同步完成'
    && syncPillLabel({
      ...base,
      syncStatus: 'synced',
      succeededAt: now - SYNC_SUCCESS_LABEL_MS,
    }) === 'alice'
    && syncPillLabel({ ...base, syncConnected: false, account: null }) === '本地模式');

  checkNew('(sync-light-1c) 已连接但账户缺失：回落本地模式/灰色，不显示成功绿；设置摘要另行标“未验证（账户缺失）”',
    syncPillLabel({ ...base, account: null, succeededAt: now }) === '本地模式'
    && syncPillView({ ...base, account: null }).tone === 'local'
    && syncPillView({ ...base, account: null }).title.includes('账户缺失（未验证）'));

  checkNew('(sync-light-1g) 账户缺失且 outbox 未知仍是灰色本地模式，不因未知 outbox 误判为 pending',
    syncPillLabel({ ...base, account: null, subscription: null }) === '本地模式'
    && syncPillView({ ...base, account: null, subscription: null }).tone === 'local');

  checkNew('(sync-light-1d) outbox 未加载不得折成 0/成功：保持橙灯并在 details/tooltip/aria 标明未知',
    syncPillLabel({ ...base, subscription: null, succeededAt: now }) === 'alice'
    && syncPillView({ ...base, subscription: null }).tone === 'pending'
    && syncPillView({ ...base, subscription: null }).details.includes('订阅状态未加载（未知）')
    && syncPillView({ ...base, subscription: null }).announcement.includes('订阅状态未加载（未知）'));

  checkNew('(sync-light-1e) 同步中携带旧失败：文案仍是同步中，失败数量与最近错误进 title/aria/live region',
    syncPillLabel({
      ...base,
      syncStatus: 'error',
      backgroundSyncing: true,
      failed: 2,
      lastError: 'HTTP 503',
    }) === '同步中'
    && syncPillView({
      ...base,
      syncStatus: 'error',
      backgroundSyncing: true,
      failed: 2,
      lastError: 'HTTP 503',
    }).tone === 'syncing'
    && syncPillView({
      ...base,
      syncStatus: 'error',
      backgroundSyncing: true,
      failed: 2,
      lastError: 'HTTP 503',
    }).title.includes('失败 2 条')
    && syncPillView({
      ...base,
      syncStatus: 'error',
      backgroundSyncing: true,
      failed: 2,
      lastError: 'HTTP 503',
    }).title.includes('最近错误：HTTP 503')
    && syncPillView({
      ...base,
      syncStatus: 'error',
      backgroundSyncing: true,
      failed: 2,
      lastError: 'HTTP 503',
    }).announcement.includes('HTTP 503'));

  checkNew('(sync-light-1f) 最近错误合并状态队列与订阅 outbox：两条不同原因都进 title/aria，不因一个来源覆盖另一个',
    (() => {
      const view = syncPillView({
        ...base,
        failed: 1,
        lastError: '状态推送 HTTP 503',
        subscription: {
          pending: 1,
          failed: 1,
          local_only: 0,
          last_error: '订阅写回 HTTP 500',
        },
      });
      return view.title.includes('最近错误：状态推送 HTTP 503；订阅写回 HTTP 500')
        && view.announcement.includes('状态推送 HTTP 503')
        && view.announcement.includes('订阅写回 HTTP 500');
    })());

  checkNew('(sync-light-1b) 成功短窗只在无待处理/失败时显示；仍有欠账保持待处理色',
    syncPillLabel({
      ...base,
      succeededAt: now,
      waiting: 1,
      now,
    }) === 'alice'
    && syncPillView({
      ...base,
      succeededAt: now,
      waiting: 1,
      now,
    }).tone === 'pending'
    && syncPillLabel({
      ...base,
      succeededAt: now,
      subscription: { pending: 1, failed: 0, local_only: 0, last_error: null },
      now,
    }) === 'alice');

  checkNew('(sync-light-2) 队列/outbox 待推送只改橙色 tone 与详情，常驻文案不出现“等待 N 条”',
    syncPillLabel({
      ...base,
      waiting: 7,
      bindableRetry: 44,
      localOnly: 2,
      expiredUnconfirmed: 3,
      subscription: { pending: 4, failed: 0, local_only: 1, last_error: null },
    }) === 'alice'
    && syncPillView({
      ...base,
      waiting: 7,
      bindableRetry: 44,
      localOnly: 2,
      expiredUnconfirmed: 3,
      subscription: { pending: 4, failed: 0, local_only: 1, last_error: null },
    }).tone === 'pending'
    && syncPillView({
      ...base,
      waiting: 7,
      bindableRetry: 44,
      localOnly: 2,
      expiredUnconfirmed: 3,
      subscription: { pending: 4, failed: 0, local_only: 1, last_error: null },
    }).details.includes('状态待推送 7 条')
    && syncPillView({
      ...base,
      subscription: { pending: 4, failed: 0, local_only: 1, last_error: null },
    }).details.includes('订阅变更待推送 4 项'));

  checkNew('(sync-light-3) 失败色优先；同步中不被旧队列失败抢走“同步中”文案（事件与手动轮次可共存）',
    syncPillView({ ...base, failed: 1 }).tone === 'error'
    && syncPillLabel({ ...base, failed: 1 }) === '同步失败'
    && syncPillLabel({ ...base, syncStatus: 'syncing', failed: 1 }) === '同步中'
    && syncPillView({ ...base, syncStatus: 'syncing', failed: 1 }).tone === 'syncing'
    && syncPillView({ ...base, subscription: { pending: 1, failed: 1, local_only: 0, last_error: 'x' } }).tone === 'error');

  checkNew('(sync-light-4) reduced-motion 关闭呼吸，但保留成功/待处理/失败语义色',
    baseCss.includes('@media (prefers-reduced-motion: reduce)')
    && baseCss.includes('.sync-dot--syncing {\n    animation: none !important;')
    && baseCss.includes('.sync-dot--success')
    && baseCss.includes('.sync-dot--pending')
    && baseCss.includes('.sync-dot--error'));

  checkNew('(sync-light-5) SyncTab 分类摘要逐项出现，且 unresolved 时不会回到“后端已同步”',
    syncStateSummary({
      waiting: 7,
      failed: 1,
      retry_wait: 2,
      local_only: 3,
      bindable_retry: 44,
      expired_unconfirmed: 5,
      last_error: 'HTTP 503',
    }, 0).includes('状态待推送 7 条')
    && syncStateSummary({
      waiting: 7,
      failed: 1,
      retry_wait: 2,
      local_only: 3,
      bindable_retry: 44,
      expired_unconfirmed: 5,
      last_error: 'HTTP 503',
    }, 0).includes('失败 3（HTTP 503）')
    && syncStateSummary({
      waiting: 0,
      failed: 0,
      local_only: 3,
      bindable_retry: 44,
      expired_unconfirmed: 5,
      last_error: null,
    }, 0).includes('仅本机 3 条')
    && !syncStateSummary({
      waiting: 0,
      failed: 0,
      local_only: 3,
      bindable_retry: 44,
      expired_unconfirmed: 5,
      last_error: null,
    }, 0).includes('后端已同步'));

  /* ---------- store：sync_status 是账户事实源，不碰 WebDAV 状态 ---------- */
  let statusReply = {
    connected: true,
    endpoint: 'https://reader.example/api/greader.php',
    account: 'alice',
    last_sync: 12345,
    protocol: 'greader',
  };
  let statusFails = false;
  let queueReply = {
    waiting: 2,
    failed: 1,
    local_only: 3,
    bindable_retry: 44,
    bound_pending: 2,
    retry_wait: 0,
    expired_unconfirmed: 5,
    last_error: 'HTTP 503',
  };
  const prevInvoke = globalThis.__INVOKE__;
  const restoreInvoke = overrideGlobal(globalThis, '__INVOKE__', (cmd, args) => {
    S.invokeCalls.push({ cmd, args });
    if (cmd === 'sync_status') {
      return statusFails ? Promise.reject({ message: 'status unavailable' }) : Promise.resolve(statusReply);
    }
    if (cmd === 'sync_queue_stats') return Promise.resolve(queueReply);
    return prevInvoke(cmd, args);
  });
  try {
    store.setState({
      localSyncState: { connected: false, library_id: 'local' },
      syncAccount: null,
      syncProtocol: null,
      syncEndpoint: null,
      syncLastSync: 0,
      syncConnected: false,
    });
    const status = await store.getState().refreshSyncStatus();
    const st1 = store.getState();
    checkNew('(sync-light-6) refreshSyncStatus 原子写入 connected/account/protocol/endpoint/last_sync，WebDAV 资料库状态不受影响',
      status?.account === 'alice'
      && st1.syncConnected === true
      && st1.syncAccount === 'alice'
      && st1.syncProtocol === 'greader'
      && st1.syncEndpoint === 'https://reader.example/api/greader.php'
      && st1.syncLastSync === 12345
      && st1.localSyncState?.library_id === 'local');

    statusFails = true;
    await store.getState().refreshSyncStatus();
    checkNew('(sync-light-6b) sync_status 读取失败保持原值（不把连接态折叠成未连接）',
      store.getState().syncConnected === true
      && store.getState().syncAccount === 'alice'
      && store.getState().syncLastSync === 12345);

    statusFails = false;
    await store.getState().refreshSyncQueueStats();
    const qs = store.getState();
    checkNew('(sync-light-7) refreshSyncQueueStats 写入 waiting/terminal/retry/local-only/bindable/expired（单一落点与事件一致）',
      qs.syncWaiting === 2
      && qs.syncTerminalFailed === 1
      && qs.syncRetryWait === 0
      && qs.syncFailed === 1
      && qs.syncLocalOnly === 3
      && qs.syncBindableRetry === 44
      && qs.syncExpiredUnconfirmed === 5
      && qs.syncQueueLastError === 'HTTP 503');
  } finally {
    restoreInvoke();
  }

  /* ---------- store：成功短窗口与 owner 边界 ---------- */
  store.setState({ syncStatus: 'synced', syncSucceededAt: null });
  const manual = store.getState().beginSyncActivity('manual');
  store.getState().finishSyncActivity(manual, 'synced');
  const successAt = store.getState().syncSucceededAt;
  checkNew('(sync-light-8) 手动同步成功写入 syncSucceededAt；纯函数在窗口内显示“同步完成”',
    successAt != null
    && syncPillLabel({
      ...base,
      succeededAt: successAt,
      now: successAt + 1,
    }) === '同步完成');

  store.setState({ syncSucceededAt: null });
  const add = store.getState().beginSyncActivity('feed-add');
  store.getState().finishSyncActivity(add, 'synced');
  checkNew('(sync-light-9) 添加订阅的本地成功不冒充同步完成（只有手动/保存并同步 owner 写成功时刻）',
    store.getState().syncSucceededAt === null);

  const failed = store.getState().beginSyncActivity('manual');
  store.getState().finishSyncActivity(failed, 'error', { stage: 'states', message: 'HTTP 500' });
  checkNew('(sync-light-10) 失败终态清除成功窗口并发布 error（灯不会再显示“同步完成”）',
    store.getState().syncSucceededAt === null
    && store.getState().syncStatus === 'error');

  const continuedAfterFailure = store.getState().beginSyncActivity('manual');
  checkNew('(sync-light-10b) 新同步开始仍保留上一失败摘要供 tooltip/aria 查询，成功后清除',
    store.getState().syncStatus === 'syncing'
    && store.getState().syncFailure?.message === 'HTTP 500'
    && syncPillView({
      ...base,
      syncStatus: 'syncing',
      failed: 1,
      lastError: store.getState().syncFailure?.message,
    }).title.includes('最近错误：HTTP 500'));
  store.getState().finishSyncActivity(continuedAfterFailure, 'synced');
  checkNew('(sync-light-10c) 新一轮成功后清除旧失败摘要，不把历史错误常驻化',
    store.getState().syncStatus === 'synced'
    && store.getState().syncFailure === null);

  store.setState({ syncSucceededAt: null });
  const olderFailure = store.getState().beginSyncActivity('manual');
  const newerSuccess = store.getState().beginSyncActivity('save-and-sync');
  store.getState().finishSyncActivity(newerSuccess, 'synced');
  const newerSuccessAt = store.getState().syncSucceededAt;
  store.getState().finishSyncActivity(olderFailure, 'error', {
    stage: 'states',
    message: '旧活动迟到失败',
  });
  checkNew('(sync-light-11) 旧活动的迟到失败不得清掉较新成功终态/成功窗口',
    newerSuccessAt != null
    && store.getState().syncStatus === 'synced'
    && store.getState().syncSucceededAt === newerSuccessAt
    && store.getState().syncFailure === null);

  const olderSuccess = store.getState().beginSyncActivity('manual');
  const newerFailure = store.getState().beginSyncActivity('save-and-sync');
  store.getState().finishSyncActivity(newerFailure, 'error', {
    stage: 'states',
    message: '较新活动失败',
  });
  store.getState().finishSyncActivity(olderSuccess, 'synced');
  checkNew('(sync-light-12) 旧活动的迟到成功不得覆盖较新失败，也不得点亮“同步完成”',
    store.getState().syncStatus === 'error'
    && store.getState().syncSucceededAt === null
    && store.getState().syncFailure?.message === '较新活动失败');

  {
    const prevInvoke = globalThis.__INVOKE__;
    const restore = overrideGlobal(globalThis, '__INVOKE__', (cmd, args) => {
      if (cmd === 'sync_local_feeds') {
        S.invokeCalls.push({ cmd, args });
        if (args?.feedId === 42) return Promise.resolve({ message: '缺少 ok 字段' });
        return Promise.resolve(args?.feedUrl
          ? '已同步 1 个本地订阅'
          : JSON.stringify({
              ok: true,
              protocol: 'greader',
              writeBack: true,
              queued: 1,
              pushed: 1,
              failed: 0,
              errors: [],
              message: '已同步 1 个本地订阅',
            }));
      }
      return prevInvoke(cmd, args);
    });
    try {
      const legacy = await api.syncLocalFeeds({ feedUrl: 'https://new.example/rss.xml' });
      checkNew('(sync-light-13) 旧后端纯文本回执按 unknown 处理：ok=false、structured=false、提示无法确认，目标过滤参数原样发出',
        legacy?.ok === false
        && legacy?.structured === false
        && legacy?.message.includes('无法确认同步结果')
        && legacy?.errors.includes('已同步 1 个本地订阅')
        && S.invokeCalls.some((c) => c.cmd === 'sync_local_feeds'
          && c.args?.feedUrl === 'https://new.example/rss.xml'));
      const structured = await api.syncLocalFeeds();
      checkNew('(sync-light-13b) 旧后端 JSON 字符串回执仍按结构化成功解析（兼容不回退）',
        structured?.ok === true && structured?.pushed === 1);
      const malformed = await api.syncLocalFeeds({ feedId: 42 });
      checkNew('(sync-light-13c) 直接对象回执缺少 boolean ok 时同按 unknown 处理，不把缺字段对象当成功',
        malformed?.ok === false
        && malformed?.structured === false
        && malformed?.message.includes('无法确认同步结果')
        && malformed?.errors.some((e) => e.includes('缺少 ok 字段')));
      const callsBeforeBlank = S.invokeCalls.length;
      let blankError = null;
      try {
        await api.syncLocalFeeds({ feedUrl: '   ' });
      } catch (error) {
        blankError = error;
      }
      checkNew('(sync-light-13d) 显式空 feedUrl 在发出 IPC 前拒绝，不得回退为全量同步',
        blankError instanceof Error
        && blankError.message.includes('feedUrl 不能为空')
        && S.invokeCalls.length === callsBeforeBlank
        && !S.invokeCalls.slice(callsBeforeBlank).some((c) => c.cmd === 'sync_local_feeds'));
    } finally {
      restore();
    }
  }

  S.invokeCalls.length = 0;
  store.setState({
    dataMode: 'tauri',
    syncConnected: true,
    syncAccount: 'alice',
    syncProtocol: 'fever',
    syncStatus: 'synced',
    toasts: [],
  });
  store.getState().addFeed(
    'cat-1',
    'https://fever-local.example/rss.xml',
    'Fever 本地源',
    'inherit',
    'inherit',
    'inherit',
    true,
  );
  await nTick(40);
  const feverAdd = S.invokeCalls.find((c) => c.cmd === 'add_feed');
  checkNew('(sync-light-14) Fever 前端门控：即使调用方传 syncToBackend=true，也不发 sync_local_feeds，不把仅本地显示成同步失败',
    feverAdd?.args?.syncToBackend === true
    && !S.invokeCalls.some((c) => c.cmd === 'sync_local_feeds')
    && store.getState().toasts.some((t) => t.text.includes('已添加订阅源：Fever 本地源'))
    && store.getState().syncStatus === 'synced');

  await ctx.resetStore();
}
