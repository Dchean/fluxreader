// F6/F7：沿生产 hydrate/reload/clear/生成入口验证显示校验的有效期。
// IPC 受控只改变输入与落地时机；不连接 AI 服务、不把显示刷新变成生成。
export const id = 'ai-status-lifetime';

function deferred() {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
}

export async function run(ctx) {
  const { store, S, mkRow, nTick, checkNew, getBodyEntry, selectAiProductState } = ctx;
  const { api } = await import('../../dist-test/lib/api.js');
  const { selectAiStatusInput } = await import('../../dist-test/store.js');
  const status = (verified = false, completed = 0) => ({
    summaryVerified: verified, translationVerified: verified,
    translationSegments: { total: 4, completed, discarded: 0 },
  });
  const visible = () => selectAiProductState(store.getState(), '101', 'translation');
  async function prepare(overrides = {}) {
    await ctx.useMainBackend();
    await ctx.resetStore({ settings: { ...store.getState().settings, markReadOnOpen: false, smartDedup: false, defaultOpenMode: 'rss' } });
    const row = mkRow({
      id: 101, feed_id: 10, title: 'AI lifetime', content_id: 71, content_revision: 1,
      content_html: '<p>revision 1</p>', translated_content: '<p>old translation</p>',
      translated_sig: 'old-signature', ...overrides,
    });
    S.backendRows = [row]; S.feedCountsImpl = ctx.countsFromRows;
    S.getArticlesPlan = { rows: [{ ...row }] };
    await store.getState().reloadFromBackend();
    store.getState().hydrateArticleContent(['101']);
    await nTick(5);
    return row;
  }

  // 原 F7：清除成功后才到的旧显示响应，无论 complete/partial 都没有资格。
  await prepare({ translated_content: '', translated_sig: null });
  let original = globalThis.__INVOKE__;
  let read = deferred();
  globalThis.__INVOKE__ = (cmd, args) => cmd === 'ai_cache_check' ? read.promise : original(cmd, args);
  try {
    store.getState().refreshAiStatus('101'); await nTick(2);
    checkNew('(f7) clear 确实成功且将分段归零', await store.getState().clearAiProduct('101', 'translation')
      && store.getState().aiStatusById['101'].translationSegments.completed === 0);
    read.resolve(status(true, 2)); await nTick(4);
    checkNew('(f7) 清除前的迟到 status 不得复活 partial/complete', visible() === null
      && store.getState().aiStatusById['101'].translationSegments.completed === 0);
  } finally { globalThis.__INVOKE__ = original; }

  // 原 F6：同条目正文新修订，沿真实 reload + hydrate，而非直接篡改状态。
  const row = await prepare();
  let verified = true; let checks = 0;
  original = globalThis.__INVOKE__;
  globalThis.__INVOKE__ = (cmd, args) => {
    if (cmd === 'ai_cache_check') { checks++; return Promise.resolve(status(verified)); }
    return original(cmd, args);
  };
  try {
    store.getState().ensureAiStatus('101'); await nTick(3);
    checkNew('(f6) 正对照：当前正文的已验证译文可 complete', visible() === 'complete');
    const revised = { ...row, content_revision: 2, content_html: '<p>revision 2</p>' };
    S.backendRows = [revised]; S.getArticlesPlan = { rows: [revised] }; verified = false;
    await store.getState().reloadFromBackend();
    checkNew('(f6) 正文失效时立即拒旧 complete，不等新请求返回', visible() !== 'complete');
    store.getState().hydrateArticleContent(['101']); await nTick(4);
    store.getState().ensureAiStatus('101'); await nTick(3);
    checkNew('(f6) 新正文惰性再查且旧译文保留为 stale', checks === 2 && visible() === 'stale'
      && getBodyEntry('101').content === '<p>revision 2</p>'
      && getBodyEntry('101').translatedContent === '<p>old translation</p>');
    store.getState().ensureAiStatus('101'); await nTick(2);
    checkNew('(f6) 同输入 ensure 去重，不反复 IPC', checks === 2);
    checkNew('(f6) 显示校验不生成 AI', S.invokeCalls.every((call) => call.cmd !== 'ai_translate' && call.cmd !== 'ai_summarize'));
  } finally { globalThis.__INVOKE__ = original; }

  // 旧读晚于新读：不同输入不能 join；旧 finally 不摘新读；旧 res 不覆盖新 res。
  await prepare();
  const old = deferred(); const fresh = deferred(); checks = 0;
  original = globalThis.__INVOKE__;
  globalThis.__INVOKE__ = (cmd, args) => cmd === 'ai_cache_check'
    ? (++checks === 1 ? old.promise : fresh.promise) : original(cmd, args);
  try {
    store.getState().refreshAiStatus('101'); await nTick(2);
    await api.saveAiConfig('{"model":"new"}');
    store.getState().ensureAiStatus('101'); await nTick(2);
    checkNew('(f6/f7) 配置改变后新读不 join 旧输入在途', checks === 2);
    old.resolve(status(true, 2)); await nTick(3);
    store.getState().ensureAiStatus('101'); await nTick(2);
    checkNew('(f6/f7) 旧读 finally 不移除较新读，旧结果不落地', checks === 2
      && store.getState().aiStatusById['101'] === undefined);
    fresh.resolve(status(false, 0)); await nTick(3);
    checkNew('(f6/f7) 新输入状态独占落地', visible() === 'stale'
      && store.getState().aiStatusById['101'].input.revision === store.getState().aiStatusRevision);
  } finally { globalThis.__INVOKE__ = original; }

  // 生成前的旧状态不能在新生成期间/完成后重新注入；包括独立 hintSegmentPlan。
  await prepare({ translated_content: '', translated_sig: null });
  const preGeneration = deferred(); const hint = deferred(); checks = 0;
  original = globalThis.__INVOKE__;
  globalThis.__INVOKE__ = (cmd, args) => cmd === 'ai_cache_check'
    ? (++checks === 1 ? preGeneration.promise : checks === 2 ? hint.promise : Promise.resolve(status(false, 1)))
    : original(cmd, args);
  try {
    store.getState().refreshAiStatus('101'); await nTick(2);
    S.aiTr = { deltas: [], error: null, reject: null, finish: true, holdIds: [], deferIds: [101] };
    store.getState().translateEntry('101'); await nTick(3);
    const generation = S.heldAi.find((h) => h.cmd === 'ai_translate');
    checkNew('(f7) 生成正对照：真实生成已开始', !!generation && visible() === 'generating');
    preGeneration.resolve(status(true, 3)); await nTick(3);
    checkNew('(f7) 生成前旧校验不能在新生成阶段落地', store.getState().aiStatusById['101'] === undefined);
    generation.ch.onmessage({ type: 'error', data: 'provider interrupted' }); generation.resolve(''); await nTick(4);
    checkNew('(f7) 新生成终态刷新得到当前 partial', visible() === 'partial'
      && store.getState().aiStatusById['101'].translationSegments.completed === 1);
    hint.resolve(status(false, 3)); await nTick(3);
    checkNew('(f7) 同输入的早期 hint 也不能覆盖终态刷新',
      store.getState().aiStatusById['101'].translationSegments.completed === 1);
    await store.getState().clearAiProduct('101', 'translation');
    await nTick(3);
    checkNew('(f7) 清除当前 partial 后回到未生成', visible() === null
      && store.getState().aiStatusById['101'].translationSegments.completed === 0);
  } finally { globalThis.__INVOKE__ = original; }

  // 真实 API 成功点覆盖配置应用和批清；失败不伪造已保存配置。
  await prepare();
  S.aiCacheCheck = status(true); store.getState().ensureAiStatus('101'); await nTick(3);
  original = globalThis.__INVOKE__;
  globalThis.__INVOKE__ = (cmd, args) => ['save_ai_config', 'config_sync_apply', 'cache_cleanup'].includes(cmd)
    ? Promise.reject(new Error('rejected')) : original(cmd, args);
  const initial = store.getState().aiStatusRevision;
  try {
    await api.saveAiConfig('{}').catch(() => {});
    await api.configSyncApply('{}', 'merge', 'hash', 'version').catch(() => {});
    await api.cacheCleanup(7, 'ai').catch(() => {});
    checkNew('(f6) 三条失败写路径均不推进已保存配置代次', store.getState().aiStatusRevision === initial && visible() === 'complete');
  } finally { globalThis.__INVOKE__ = original; }
  await api.configSyncApply('{}', 'merge', 'hash', 'version');
  checkNew('(f6) 配置下载应用成功使旧验签过时', store.getState().aiStatusRevision === initial + 1 && visible() === 'stale');
  await api.cacheCleanup(7, 'ai');
  checkNew('(f7) 批量清理 AI 成功使旧分段与完整状态过时', store.getState().aiStatusRevision === initial + 2);
  const input = selectAiStatusInput(store.getState(), '101');
  checkNew('(f6) 验签输入保持正文身份，未复用 AI 写入缓存戳', input.contentId === 71 && input.contentRevision === 1);
}
