export const id = 'backend-sync-runtime';

function report(errors = []) {
  return { pushed_states: 0, pushed_feeds: 0, pulled_feeds: 0, pulled_entries: 0, merged_states: 0, errors };
}

export async function run(ctx) {
  const { store, checkNew, overrideGlobal } = ctx;
  await ctx.useMainBackend();
  await ctx.resetStore({ dataMode: 'tauri', backgroundSyncing: false, toasts: [], errors: [] });
  const baseline = store.getState().beginSyncActivity('manual');
  store.getState().finishSyncActivity(baseline, 'synced');
  let sequence = 100_000;
  let backendId = 100_000;
  const emit = (activityId, phase, extra = {}) => store.getState().applyBackendSyncActivity({
    id: activityId, sequence: ++sequence, source: 'auto', phase, ...extra,
  });
  const first = ++backendId;
  const second = ++backendId;
  emit(first, 'started');
  emit(second, 'started', { source: 'focus' });
  emit(first, 'finished', { outcome: 'success' });
  checkNew('(runtime-1) 一轮结束不能熄灭仍在运行的另一轮',
    store.getState().backgroundSyncing && store.getState().syncStatus === 'syncing');
  emit(second, 'finished', { outcome: 'partial', message: '一个订阅同步失败' });
  checkNew('(runtime-2) 后台 partial 即使队列为空仍有红灯和持久重试入口',
    !store.getState().backgroundSyncing && store.getState().syncStatus === 'error'
    && store.getState().syncFailure?.message === '一个订阅同步失败'
    && store.getState().errors.some((error) => error.key === 'sync:backend' && error.action.label === '重试'));
  emit(first, 'started');
  emit(first, 'finished', { outcome: 'success' });
  checkNew('(runtime-3) 已结束旧轮次的迟到开始/成功不能复活或覆盖新失败',
    !store.getState().backgroundSyncing && store.getState().syncStatus === 'error');

  const older = ++backendId;
  const newer = ++backendId;
  emit(older, 'started');
  emit(newer, 'started');
  emit(newer, 'finished', { outcome: 'success' });
  emit(older, 'finished', { outcome: 'error', message: '迟到旧错误' });
  checkNew('(runtime-4) 较新成功先结束、较旧失败后结束仍保留较新成功',
    store.getState().syncStatus === 'synced' && store.getState().syncFailure === null
    && !store.getState().backgroundSyncing);

  const manual = store.getState().beginSyncActivity('manual');
  store.getState().updateSyncActivity(manual, { stage: 'feeds' });
  const manualBackend = ++backendId;
  emit(manualBackend, 'started', { source: 'manual' });
  emit(manualBackend, 'stage', { source: 'manual', stage: 'states' });
  checkNew('(runtime-5) 后端 stage 事件推进同一个手动owner，不创建第二个用户活动',
    store.getState().syncActivityToken === manual && store.getState().syncStage === 'states');
  emit(manualBackend, 'finished', { source: 'manual', outcome: 'success' });
  store.getState().updateSyncActivity(manual, { stage: 'refresh' });
  emit(manualBackend, 'stage', { source: 'manual', stage: 'states' });
  checkNew('(runtime-6) 后端完成不结束手动抓取；迟到stage不能回退到states',
    store.getState().syncStatus === 'syncing' && store.getState().syncStage === 'refresh'
    && store.getState().syncActivityToken === manual);
  store.getState().finishSyncActivity(manual, 'synced');

  const localFailure = store.getState().beginSyncActivity('manual');
  store.getState().finishSyncActivity(localFailure, 'error', { stage: 'refresh', message: '后续抓取失败' });
  const lateManual = ++backendId;
  emit(lateManual, 'started', { source: 'manual' });
  emit(lateManual, 'finished', { source: 'manual', outcome: 'success' });
  checkNew('(runtime-6b) 手动IPC事件晚于抓取终态到达，不创建后台owner覆盖抓取失败',
    store.getState().syncStatus === 'error' && store.getState().syncFailure?.message === '后续抓取失败'
    && !store.getState().backgroundSyncing);

  const originalReload = store.getState().reloadFromBackend;
  store.setState({ reloadFromBackend: async () => {}, toasts: [], errors: [] });
  const calls = [];
  let result = { activity_id: ++backendId, feeds: null, states: null,
    error: { stage: 'feeds', code: 'auth', message: '认证失败，未连接到该账户' } };
  const restore = overrideGlobal(globalThis, '__INVOKE__', async (cmd, args) => {
    calls.push({ cmd, args });
    if (cmd === 'sync_run') return result;
    if (cmd === 'sync_phase') return report();
    if (cmd === 'refresh_all_feeds') return { new_articles: 0, failed_feeds: 0 };
    return null;
  });
  try {
    await store.getState().triggerManualSync();
    checkNew('(runtime-7) 空队列认证错误不因message含未连接而被静默，stage/code结果真实消费',
      store.getState().syncStatus === 'error' && store.getState().syncFailure?.stage === 'feeds'
      && store.getState().syncFailure?.message.includes('认证失败')
      && calls.filter((call) => call.cmd === 'sync_run').length === 1
      && !calls.some((call) => call.cmd === 'sync_phase'));
    result = { activity_id: ++backendId, feeds: report(['feeds partial']), states: report(['states partial']), error: null };
    await store.getState().triggerManualSync();
    checkNew('(runtime-8) 单IPC两阶段报告都消费，feeds部分失败不吞states错误',
      store.getState().syncFailure?.message.includes('feeds partial')
      && store.getState().syncFailure?.message.includes('states partial'));
    result = { activity_id: ++backendId, feeds: report(), states: null,
      error: { stage: 'states', code: 'network', message: '状态拉取超时' } };
    await store.getState().triggerManualSync();
    checkNew('(runtime-9) states Err保留已经成功的feeds并按真实阶段显示失败',
      store.getState().syncFailure?.stage === 'states'
      && store.getState().syncFailure?.message === '状态拉取超时');
    result = { activity_id: ++backendId, feeds: null, states: null,
      error: { stage: 'feeds', code: 'notConnected', message: '未配置同步服务' } };
    await store.getState().triggerManualSync();
    checkNew('(runtime-10) 明确NotConfigured仍可纯直连刷新成功', store.getState().syncStatus === 'synced');
    const { api } = await import('../../src/lib/api.ts');
    await api.syncPhase('states', true);
    checkNew('(runtime-11) 旧sync_phase API仍以原参数可调用',
      calls.at(-1)?.cmd === 'sync_phase' && calls.at(-1)?.args.which === 'states'
      && calls.at(-1)?.args.full === true);
    const failedId = ++backendId;
    emit(failedId, 'started');
    emit(failedId, 'finished', { outcome: 'error', message: '后台端点失败' });
    result = { activity_id: ++backendId, feeds: report(), states: report(), error: null };
    const before = calls.filter((call) => call.cmd === 'sync_run').length;
    const retry = store.getState().errors.find((error) => error.key === 'sync:backend');
    retry.action.run();
    await store.getState().triggerManualSync();
    checkNew('(runtime-12) 后台错误中心重试实际发起统一sync_run且成功恢复',
      calls.filter((call) => call.cmd === 'sync_run').length === before + 1
      && store.getState().syncStatus === 'synced');
  } finally {
    restore();
    store.setState({ reloadFromBackend: originalReload, toasts: [], errors: [] });
  }
}
