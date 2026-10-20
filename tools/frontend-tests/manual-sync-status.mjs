// OPT-022：手动同步阶段、终态与重入行为回归。
// 证据边界：IPC 由可控的 deferred/reject 夹具驱动，真实执行 store 中的
// triggerManualSync；不依赖源码字符串，也不以固定短延时冒充整轮完成。
export const id = 'manual-sync-status';

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function report(extra = {}) {
  return {
    pushed_states: 0,
    pushed_feeds: 0,
    pulled_feeds: 0,
    pulled_entries: 0,
    merged_states: 0,
    errors: [],
    ...extra,
  };
}

async function resetManualSyncState(ctx, extra = {}) {
  await ctx.resetStore({
    dataMode: 'tauri',
    dataLoading: false,
    bootstrapError: null,
    syncStatus: 'synced',
    syncActivityOwner: null,
    syncActivityToken: null,
    syncStage: null,
    syncStartedAt: null,
    syncRemoteWriteConfirmed: false,
    syncFailure: null,
    ...extra,
  });
}

async function waitUntil(predicate, timeoutMs = 500) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (predicate()) return true;
    await new Promise((resolve) => setTimeout(resolve, 2));
  }
  return predicate();
}

async function settleWithin(promise, timeoutMs = 500) {
  if (!promise || typeof promise.then !== 'function') return false;
  return Promise.race([
    promise.then(() => true, () => true),
    new Promise((resolve) => setTimeout(() => resolve(false), timeoutMs)),
  ]);
}

function installCommands(ctx, handlers, calls) {
  return ctx.overrideGlobal(globalThis, '__INVOKE__', (cmd, args) => {
    calls.push({ cmd, args });
    const handler = handlers[cmd];
    return handler ? handler(args) : Promise.resolve(null);
  });
}

async function withReload(store, reload) {
  const original = store.getState().reloadFromBackend;
  store.setState({ reloadFromBackend: reload });
  return () => store.setState({ reloadFromBackend: original });
}

export async function run(ctx) {
  const { store, checkNew, nTick } = ctx;
  await ctx.useMainBackend();

  const syncModule = await import('../../dist-test/store/slices/sync.js');

  /* ---------- push 完成、pull 尚未返回：不得伪装成整轮完成 ---------- */
  {
    await resetManualSyncState(ctx);
    const statesGate = deferred();
    const refreshGate = deferred();
    const reloadGate = deferred();
    const calls = [];
    let statesCalls = 0;
    let refreshCalls = 0;
    let reloadCalls = 0;
    const restoreInvoke = installCommands(ctx, {
      sync_phase: (args) => {
        if (args?.which === 'feeds') return Promise.resolve(report());
        statesCalls += 1;
        return statesGate.promise;
      },
      refresh_all_feeds: () => {
        refreshCalls += 1;
        return refreshGate.promise;
      },
    }, calls);
    const restoreReload = await withReload(store, () => {
      reloadCalls += 1;
      return reloadGate.promise;
    });
    try {
      const pending = store.getState().triggerManualSync();
      await waitUntil(() => statesCalls === 1);
      const pulling = store.getState();
      checkNew('(opt022-a1) triggerManualSync 返回可等待的 Promise；states 在途时状态为 syncing/states',
        typeof pending?.then === 'function'
          && pulling.syncStatus === 'syncing'
          && pulling.syncStage === 'states'
          && typeof pulling.syncStartedAt === 'number');

      store.getState().applySyncQueueChanged({ waiting: 0, failed: 0, last_error: null });
      const confirmed = store.getState();
      checkNew('(opt022-a2) push 快速清空队列后只确认远端写回，仍在 states 拉取且绝不提前写 synced',
        confirmed.syncRemoteWriteConfirmed === true
          && confirmed.syncStatus === 'syncing'
          && confirmed.syncStage === 'states');

      await nTick(40);
      checkNew('(opt022-a3) pull Promise 未返回时保持可解释的等待阶段，不用固定延时伪造完成',
        store.getState().syncStatus === 'syncing'
          && store.getState().syncStage === 'states'
          && refreshCalls === 0);

      statesGate.resolve(report());
      await waitUntil(() => refreshCalls === 1);
      checkNew('(opt022-a4) states 返回后推进到 refresh，并保留“远端已写回”事实',
        store.getState().syncStatus === 'syncing'
          && store.getState().syncStage === 'refresh'
          && store.getState().syncRemoteWriteConfirmed === true);

      refreshGate.resolve({ new_articles: 0, failed_feeds: 0 });
      await waitUntil(() => reloadCalls === 1);
      checkNew('(opt022-a5) refresh 返回后推进到 reload，终态前仍为 syncing',
        store.getState().syncStatus === 'syncing'
          && store.getState().syncStage === 'reload');

      reloadGate.resolve();
      checkNew('(opt022-a6) 全链成功后可等待地收敛到 synced，且清空活动阶段/失败摘要',
        await settleWithin(pending)
          && store.getState().syncStatus === 'synced'
          && store.getState().syncStage === null
          && store.getState().syncStartedAt === null
          && store.getState().syncFailure === null);
    } finally {
      restoreReload();
      restoreInvoke();
      store.setState({ toasts: [] });
    }
  }

  /* ---------- refresh / reload / IPC reject：终态必须退出 syncing ---------- */
  {
    await resetManualSyncState(ctx);
    const calls = [];
    const restoreInvoke = installCommands(ctx, {
      sync_phase: () => Promise.resolve(report()),
      refresh_all_feeds: () => Promise.reject({ message: '源站超时' }),
    }, calls);
    const restoreReload = await withReload(store, async () => {});
    try {
      const pending = store.getState().triggerManualSync();
      await waitUntil(() => store.getState().syncStatus === 'error');
      const failed = store.getState();
      checkNew('(opt022-b1) refresh reject：等待 Promise 收敛为 error，退出 syncing 并保留 refresh 阶段与原因',
        await settleWithin(pending)
          && failed.syncStatus === 'error'
          && failed.syncStage === null
          && failed.syncStartedAt === null
          && failed.syncFailure?.stage === 'refresh'
          && failed.syncFailure?.message.includes('源站超时'));
    } finally {
      restoreReload();
      restoreInvoke();
      store.setState({ toasts: [] });
    }
  }

  {
    await resetManualSyncState(ctx);
    const calls = [];
    const restoreInvoke = installCommands(ctx, {
      sync_phase: () => Promise.resolve(report()),
      refresh_all_feeds: () => Promise.resolve({ new_articles: 1, failed_feeds: 0 }),
    }, calls);
    const restoreReload = await withReload(store, async () => {
      throw { message: '本地快照读取失败' };
    });
    try {
      const pending = store.getState().triggerManualSync();
      await waitUntil(() => store.getState().syncStatus === 'error');
      const failed = store.getState();
      checkNew('(opt022-b2) reload reject：等待 Promise 收敛为 error，退出 syncing 并保留 reload 阶段与原因',
        await settleWithin(pending)
          && failed.syncStatus === 'error'
          && failed.syncStage === null
          && failed.syncFailure?.stage === 'reload'
          && failed.syncFailure?.message.includes('本地快照读取失败'));
    } finally {
      restoreReload();
      restoreInvoke();
      store.setState({ toasts: [] });
    }
  }

  {
    await resetManualSyncState(ctx);
    const calls = [];
    let statesCalls = 0;
    const restoreInvoke = installCommands(ctx, {
      sync_phase: (args) => {
        if (args?.which === 'feeds') return Promise.resolve(report());
        statesCalls += 1;
        if (statesCalls === 1) {
          return Promise.reject({ code: 'timeout', message: '状态拉取超时' });
        }
        return Promise.resolve(report());
      },
      refresh_all_feeds: () => Promise.resolve({ new_articles: 0, failed_feeds: 0 }),
    }, calls);
    const restoreReload = await withReload(store, async () => {});
    try {
      const first = store.getState().triggerManualSync();
      await waitUntil(() => store.getState().syncStatus === 'error');
      const failed = store.getState();
      checkNew('(opt022-b3) states 请求 reject（timeout）：收敛为 error，阶段/原因可见且没有残留在途锁',
        await settleWithin(first)
          && failed.syncStatus === 'error'
          && failed.syncStage === null
          && failed.syncFailure?.stage === 'states'
          && failed.syncFailure?.message.includes('状态拉取超时'));

      const second = store.getState().triggerManualSync();
      checkNew('(opt022-b4) 失败终态后可再次发起；后一轮成功后恢复 synced，忙锁不是不可恢复的全局残留',
        await settleWithin(second)
          && store.getState().syncStatus === 'synced'
          && store.getState().syncStage === null
          && statesCalls === 2);
    } finally {
      restoreReload();
      restoreInvoke();
      store.setState({ toasts: [] });
    }
  }

  /* ---------- P1-1：不同生产写者共享 syncStatus，旧请求迟到失败不得覆盖新终态 ---------- */
  {
    await resetManualSyncState(ctx);
    const statesGate = deferred();
    const calls = [];
    let statesCalls = 0;
    const restoreInvoke = installCommands(ctx, {
      sync_phase: (args) => {
        if (args?.which === 'feeds') return Promise.resolve(report());
        statesCalls += 1;
        return statesGate.promise;
      },
      refresh_all_feeds: () => Promise.reject({ message: '旧手动同步迟到的抓取失败' }),
      add_feed: () => Promise.resolve(null),
    }, calls);
    const restoreReload = await withReload(store, async () => {});
    try {
      const oldManual = store.getState().triggerManualSync();
      await waitUntil(() => statesCalls === 1);

      store.getState().addFeed(
        'cat-1',
        'https://new.example/rss',
        '新源',
        'inherit',
        'inherit',
        'inherit',
        false,
      );
      await waitUntil(() => calls.some((c) => c.cmd === 'add_feed'));
      await nTick(50);

      statesGate.resolve(report());
      await nTick(80);
      const final = store.getState();
      checkNew('(opt022-f1) addFeed 的新终态不被较早手动同步的迟到 error 覆盖（所有 syncStatus 写者共享 owner/generation）',
        typeof oldManual?.then === 'function'
          && final.syncStatus === 'synced'
          && final.syncStage === null
          && final.syncFailure === null);
    } finally {
      restoreReload();
      restoreInvoke();
      store.setState({ toasts: [] });
    }
  }

  /* ---------- 重复点击：单飞，不能并行跑两轮或让旧请求覆盖新终态 ---------- */
  {
    await resetManualSyncState(ctx);
    const statesGate = deferred();
    const calls = [];
    let statesCalls = 0;
    const restoreInvoke = installCommands(ctx, {
      sync_phase: (args) => {
        if (args?.which === 'feeds') return Promise.resolve(report());
        statesCalls += 1;
        return statesGate.promise;
      },
      refresh_all_feeds: () => Promise.resolve({ new_articles: 0, failed_feeds: 0 }),
    }, calls);
    const restoreReload = await withReload(store, async () => {});
    try {
      const first = store.getState().triggerManualSync();
      await waitUntil(() => statesCalls === 1);
      const second = store.getState().triggerManualSync();
      await nTick(30);
      checkNew('(opt022-c1) 重复点击复用同一在途 Promise，states 只执行一次',
        typeof first?.then === 'function'
          && first === second
          && statesCalls === 1);

      statesGate.resolve(report());
      checkNew('(opt022-c2) 重入调用的两个等待方都随同一次同步收敛，不留下第二个后台写者',
        await settleWithin(first)
          && await settleWithin(second)
          && store.getState().syncStatus === 'synced'
          && store.getState().syncStage === null);
    } finally {
      restoreReload();
      restoreInvoke();
      store.setState({ toasts: [] });
    }
  }

  /* ---------- 切页/卸载页面组件：状态由 store 持有，回来后仍能解释同一阶段 ---------- */
  {
    await resetManualSyncState(ctx);
    const statesGate = deferred();
    const calls = [];
    const restoreInvoke = installCommands(ctx, {
      sync_phase: (args) => args?.which === 'feeds' ? Promise.resolve(report()) : statesGate.promise,
      refresh_all_feeds: () => Promise.resolve({ new_articles: 0, failed_feeds: 0 }),
    }, calls);
    const restoreReload = await withReload(store, async () => {});
    try {
      const pending = store.getState().triggerManualSync();
      await waitUntil(() => store.getState().syncStage === 'states');
      store.getState().applySyncQueueChanged({ waiting: 0, failed: 0, last_error: null });
      const beforeSwitch = store.getState();

      store.setState({ settingsOpen: false, settingsTab: 'general' });
      await nTick(20);
      store.setState({ settingsOpen: true, settingsTab: 'sync' });
      const afterReturn = store.getState();
      checkNew('(opt022-d1) 切走再返回设置页不丢阶段：仍是 states/syncing，远端写回确认也保留',
        beforeSwitch.syncStatus === 'syncing'
          && beforeSwitch.syncStage === 'states'
          && afterReturn.syncStatus === 'syncing'
          && afterReturn.syncStage === 'states'
          && afterReturn.syncRemoteWriteConfirmed === true
          && afterReturn.syncStartedAt === beforeSwitch.syncStartedAt);

      statesGate.resolve(report());
      await settleWithin(pending);
      checkNew('(opt022-d2) 返回后原同步仍正常收敛，不因组件生命周期留下忙锁',
        store.getState().syncStatus === 'synced'
          && store.getState().syncStage === null);
    } finally {
      restoreReload();
      restoreInvoke();
      store.setState({ toasts: [] });
    }
  }

  /* ---------- 单一文案源：阶段标签明确区分远端确认与整轮继续执行 ---------- */
  {
    const pulling = syncModule.manualSyncStageLabel?.('states', {
      remoteConfirmed: false,
      elapsedMs: 0,
    });
    const confirmedPulling = syncModule.manualSyncStageLabel?.('states', {
      remoteConfirmed: true,
      elapsedMs: 0,
    });
    const refreshing = syncModule.manualSyncStageLabel?.('refresh', {
      remoteConfirmed: true,
      elapsedMs: 0,
    });
    checkNew('(opt022-e1) 阶段文案区分“状态拉取中 / 远端已写回但仍拉取 / 正在抓取内容”',
      typeof syncModule.manualSyncStageLabel === 'function'
        && pulling === '同步状态并拉取条目中…'
        && confirmedPulling === '远端已写回，仍在拉取条目…'
        && refreshing === '远端已写回，正在抓取内容…');

    const unresponsive = syncModule.syncActivityLabel?.({
      owner: 'manual',
      stage: 'states',
      remoteConfirmed: true,
      elapsedMs: 35_000,
    });
    checkNew('(opt022-e2) 真正长期 pending 显示“同步未返回/已等待”，保留 busy 语义但不冒充普通同步中',
      typeof syncModule.syncActivityLabel === 'function'
        && unresponsive === '远端已写回，同步未返回（状态与条目同步，已等待 35 秒）');

    const fs = await import('node:fs');
    const feedsSrc = fs.readFileSync(new URL('../../src/store/slices/feeds.ts', import.meta.url), 'utf8');
    const syncTabSrc = fs.readFileSync(new URL('../../src/components/settings/SyncTab.tsx', import.meta.url), 'utf8');
    checkNew('(opt022-f2) 生产写入者不再直写 syncStatus：addFeed 与保存后同步都经 begin/update/finish generation',
      !/syncStatus\s*:/.test(feedsSrc)
        && !/syncStatus\s*:/.test(syncTabSrc)
        && feedsSrc.includes("beginSyncActivity('feed-add')")
        && syncTabSrc.includes("beginSyncActivity('save-and-sync')")
        && syncTabSrc.includes('finishSyncActivity(syncToken'));

    const older = store.getState().beginSyncActivity('feed-add');
    const newer = store.getState().beginSyncActivity('save-and-sync');
    store.getState().finishSyncActivity(newer, 'error', {
      stage: 'states',
      message: '较新活动失败',
    });
    store.getState().finishSyncActivity(older, 'synced');
    checkNew('(opt022-f3) 反序终态同样按 generation 收敛：较早活动迟到成功不得覆盖较新失败',
      store.getState().syncStatus === 'error'
        && store.getState().syncActivityToken === null
        && store.getState().syncFailure?.message.includes('较新活动失败'));
    const cleanup = store.getState().beginSyncActivity('feed-add');
    store.getState().finishSyncActivity(cleanup, 'synced');
  }

  /* ---------- P2-1：GitHub 断开也使用同一 generation，不覆盖更新的活动 ---------- */
  {
    await resetManualSyncState(ctx);
    const disconnectGate = deferred();
    const calls = [];
    const restoreInvoke = installCommands(ctx, {
      github_login_disconnect: () => disconnectGate.promise,
    }, calls);
    try {
      const disconnect = store.getState().githubLoginDisconnect();
      checkNew('(opt022-g1) GitHub 断开开始即登记独立 generation/owner',
        store.getState().syncStatus === 'syncing'
          && store.getState().syncActivityOwner === 'github-disconnect'
          && store.getState().syncActivityToken != null);

      const newer = store.getState().beginSyncActivity('save-and-sync');
      store.getState().finishSyncActivity(newer, 'synced');
      disconnectGate.reject({ message: 'GitHub 断开失败' });
      checkNew('(opt022-g2) GitHub 断开 reject 后 finish 自己的 token，且不覆盖更新活动已发布的 synced',
        await settleWithin(disconnect)
          && store.getState().syncStatus === 'synced'
          && store.getState().syncActivityOwner === null
          && store.getState().syncActivityToken === null
          && store.getState().syncFailure === null
          && store.getState().githubLoggingIn === false);

      const fs = await import('node:fs');
      const syncSliceSrc = fs.readFileSync(
        new URL('../../src/store/slices/sync.ts', import.meta.url),
        'utf8',
      );
      const disconnectBlock = syncSliceSrc.slice(
        syncSliceSrc.indexOf('githubLoginDisconnect:'),
        syncSliceSrc.indexOf('githubLoginDisconnect:') + 900,
      );
      checkNew('(opt022-g3) 源码守卫：GitHub 断开 begin 与 reject/finish 接线仍存在',
        disconnectBlock.includes("beginSyncActivity('github-disconnect')")
          && disconnectBlock.includes("finishSyncActivity(syncToken, 'error')")
          && disconnectBlock.includes("finishSyncActivity(syncToken, 'synced')"));
    } finally {
      restoreInvoke();
      store.setState({ toasts: [], githubLoggingIn: false });
    }
  }

}
