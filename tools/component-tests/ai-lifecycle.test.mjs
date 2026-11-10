// 2026-11-09 独审反例纳入正式回归；保留原审计目录的历史失败证据，当前文件由实现者维护。
import assert from 'node:assert/strict';
import { componentTest, mount, unmount, waitFor, resetStore, getBodyEntry, useAppStore } from './harness.mjs';
import { installBackend, seedFromRows, mkRow, invokeCount, invokeCalls, setPlan, resetPlan, deferred as makeDeferred, setCurrentRows } from './fixtures.mjs';

const { createElement, act } = await import('react');
const { Reader } = await import('../../src/components/Reader.tsx');
const { buildFeedIndex, buildSourceIndex } = await import('../../dist-test/store/internals.js');
const { markBodyLoading, applyBodyRow, invalidateContentRevision, dropBodyEntry } = await import('../../dist-test/store/bodyCache.js');
const { selectAiProductState } = await import('../../dist-test/store/slices/ai.js');

const MISS = { summaryVerified: false, translationVerified: false, translationSegments: { total: 1, completed: 0, discarded: 0 } };
const HIT = { ...MISS, summaryVerified: true, translationVerified: true };
const controlled = new WeakSet();
const gates = [];
const pending = new Set();
const runnable = new Set();
let plainInvoke;
let issued = 0;
function deferred() { const gate = makeDeferred(); controlled.add(gate.promise); gates.push(gate); return gate; }
const lifecycleTest = (name, body) => componentTest(name, async () => {
  try { await body(); await settle(); assert.equal(pending.size, 0, '本用例已发送IPC全部终结'); }
  finally {
    await act(async () => { for (const gate of gates.splice(0)) gate.resolve(null); });
    await settle(); resetPlan(); if (plainInvoke) globalThis.__INVOKE__ = plainInvoke;
  }
});
function seed({ cid = 301, layout = 'article', auto = false, row: overrides = {} } = {}) {
  assert.equal(pending.size, 0, '重新播种前上一轮IPC已释放');
  resetStore(); resetPlan(); plainInvoke = installBackend();
  globalThis.__INVOKE__ = (command, args) => {
    const result = plainInvoke(command, args); const promise = Promise.resolve(result); issued++;
    pending.add(promise); if (!controlled.has(result)) runnable.add(promise);
    const end = () => { pending.delete(promise); runnable.delete(promise); };
    void promise.then(end, end); return promise;
  };
  const feed = { article: 10, social: 11, notification: 14, image: 12, podcast: 13 }[layout];
  const row = mkRow({ id: 101, feed_id: feed, content_id: cid, content_revision: cid === null ? 0 : 1, content_html: '<p>original body</p>', ...overrides });
  seedFromRows([row], { layout });
  const categories = useAppStore.getState().categories.map(c => ({ ...c, summaryMode: 'inherit', translateMode: 'inherit', feeds: c.feeds.map(f => ({ ...f, summaryMode: 'inherit', translateMode: 'inherit' })) }));
  useAppStore.setState(s => ({ categories, feedIndex: buildFeedIndex(categories), sourceIndex: buildSourceIndex(categories),
    settings: { ...s.settings, articleAutoSummary: auto, articleAutoTranslate: auto, socialAutoTranslate: auto,
      notificationAutoSummary: auto, notificationAutoTranslate: auto, markReadOnOpen: false, markReadOnScrollBottom: false, markReadOnScrollOut: false } }));
  applyBodyRow('101', row, markBodyLoading('101'), { aiSummary: '', translatedContent: '' });
  setPlan({ ai_cache_check: () => Promise.resolve(MISS) });
  return row;
}
/** 本域IPC的受控gates由用例明确释放；等待其余真实Promise链和连续两次无新IPC的
 * event-loop边界，不用8轮定时睡眠代替终态。4秒有界失败，不更改全局看门狗。 */
async function settle() {
  await act(async () => {
    let timer; let cancelled = false;
    const timeout = new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('AI生命周期IPC未收口')), 4000); });
    const drain = async () => {
      let stable = 0;
      while (!cancelled && stable < 2) {
        const before = issued;
        await Promise.allSettled([...runnable]);
        await new Promise(resolve => setImmediate(resolve));
        stable = runnable.size === 0 && issued === before ? stable + 1 : 0;
      }
    };
    try { await Promise.race([drain(), timeout]); } finally { cancelled = true; clearTimeout(timer); }
  });
}

lifecycleTest('AI lifecycle final invoke guard: qualification withdrawn at final await costs zero and does not poison retry', async () => {
  seed({ auto: true });
  let checks = 0;
  setPlan({ ai_summarize: async () => { throw new Error('sent failure'); } });
  await act(async () => useAppStore.getState().startAutomaticAi('101', 'summary', () => ++checks < 3));
  await waitFor(() => checks === 3, { label: '最终invoke前资格检查已执行' });
  await settle();
  assert.equal(checks, 3, 'initial, after cache check, and final invoke qualification all run');
  assert.equal(invokeCount('ai_summarize'), 0);
  assert.equal(useAppStore.getState().summarizingIds['101'], undefined);
  await act(async () => useAppStore.getState().startAutomaticAi('101', 'summary', () => true));
  await waitFor(() => !!useAppStore.getState().summaryErrors['101']);
  await settle();
  assert.equal(invokeCount('ai_summarize'), 1);
  assert.equal(useAppStore.getState().summarizingIds['101'], undefined);
  await act(async () => { useAppStore.setState({ summaryErrors: {} }); useAppStore.getState().startAutomaticAi('101', 'summary', () => true); });
  await settle();
  assert.equal(invokeCount('ai_summarize'), 1, 'attempt ledger still prevents a repeat even if the visible error is gone');
  assert.equal(invokeCount('set_read') + invokeCount('set_read_bulk'), 0);
});

lifecycleTest('AI lifecycle detached manual retry: current row/source identity, released body, deletion, and unsupported layout', async () => {
  for (const outcome of ['retained', 'deleted', 'unsupported', 'unknown']) {
    const row = seed();
    setPlan({ ai_translate: async () => { throw new Error('first failure'); } });
    await act(async () => useAppStore.getState().translateEntry('101'));
    await waitFor(() => useAppStore.getState().errors.some(e => e.key === 'ai:translate:101'));
    await settle();
    const retry = useAppStore.getState().errors.find(e => e.key === 'ai:translate:101').action.run;
    const gate = deferred();
    const detached = { ...row, translated_content: '<p>detached success</p>', translated_sig: 'sig' };
    let done = false;
    await act(async () => {
      dropBodyEntry('101');
      const sourceIndex = new Map(useAppStore.getState().sourceIndex);
      if (outcome === 'unsupported') sourceIndex.set('10', { ...sourceIndex.get('10'), layout: 'image' });
      if (outcome === 'unknown') sourceIndex.delete('10');
      useAppStore.setState({ entries: [], activeArticleId: null, activeContentLayout: 'social', categories: [], feedIndex: new Map(), sourceIndex });
      setPlan({ get_article: () => gate.promise, ai_cache_check: () => Promise.resolve(HIT), ai_translate: async args => {
        args.onChannel.onmessage({ type: 'delta', data: '<p>untrusted</p>' });
        args.onChannel.onmessage({ type: 'done' }); done = true; return '';
      } });
      retry();
    });
    await settle();
    assert.equal(invokeCount('ai_translate'), 1, `${outcome}: preflight read is mandatory`);
    await act(async () => gate.resolve(outcome === 'deleted' ? null : detached));
    await waitFor(() => outcome === 'retained'
      ? done && selectAiProductState(useAppStore.getState(), '101', 'translation') === 'complete'
      : useAppStore.getState().toasts.some(t => t.text.includes('无法重试') || t.text.includes('不支持该 AI 操作')), { label: `离屏${outcome}明确终态` });
    await settle();
    assert.equal(invokeCount('ai_translate'), outcome === 'retained' ? 2 : 1, outcome);
    assert.equal(done, outcome === 'retained');
    assert.deepEqual(useAppStore.getState().entries, []);
    assert.equal(useAppStore.getState().activeArticleId, null);
    assert.equal(invokeCount('set_read') + invokeCount('set_read_bulk'), 0);
    if (outcome === 'retained') {
      assert.equal(getBodyEntry('101')?.translatedContent, '<p>detached success</p>');
      assert.equal(useAppStore.getState().rawTranslatedIds['101'], undefined);
      assert.equal(useAppStore.getState().translatingIds['101'], undefined);
      assert.equal(selectAiProductState(useAppStore.getState(), '101', 'translation'), 'complete');
    }
  }
});

lifecycleTest('AI lifecycle stale stream terminal: a new body with signed clean translation must leave raw rendering mode', async () => {
  const original = seed();
  const request = deferred(); let channel;
  setPlan({ ai_translate: args => { channel = args.onChannel; return request.promise; } });
  await act(async () => useAppStore.setState({ activeArticleId: '101' }));
  const mounted = await mount(createElement(Reader));
  await act(async () => useAppStore.getState().toggleReaderTranslation());
  await waitFor(() => !!channel);
  await act(async () => channel.onmessage({ type: 'delta', data: '<p>old raw fragment</p>' }));
  const fresh = { ...original, content_revision: 2, content_html: '<p>new body</p>', translated_content: '<h2>new verified translation</h2>', translated_sig: 'new-sig' };
  await act(async () => {
    setCurrentRows([fresh]);
    useAppStore.setState(s => ({ entries: s.entries.map(e => ({ ...e, contentRevision: 2 })) }));
    invalidateContentRevision('101', 301, 2);
    applyBodyRow('101', fresh, markBodyLoading('101'), { aiSummary: '', translatedContent: '' });
    setPlan({ ai_cache_check: () => Promise.resolve(HIT) });
    channel.onmessage({ type: 'delta', data: 'LATE OLD DATA' });
    channel.onmessage({ type: 'done' }); request.resolve('');
    useAppStore.getState().refreshAiStatus('101');
  });
  await settle();
  assert.equal(getBodyEntry('101').translatedContent, '<h2>new verified translation</h2>', 'old result must not overwrite fresh data');
  assert.equal(useAppStore.getState().translatingIds['101'], undefined);
  console.log('[AUDIT-A1]', JSON.stringify({ raw: useAppStore.getState().rawTranslatedIds['101'], state: selectAiProductState(useAppStore.getState(), '101', 'translation'), h2: mounted.container.querySelector('.article-prose h2')?.textContent ?? null, text: mounted.container.querySelector('.article-prose')?.textContent }));
  assert.equal(useAppStore.getState().rawTranslatedIds['101'], undefined, 'the old request no longer owns the new signed translation');
  assert.equal(mounted.container.querySelector('.article-prose h2')?.textContent, 'new verified translation');
  assert.equal(selectAiProductState(useAppStore.getState(), '101', 'translation'), 'complete');
  await unmount(mounted);
});

lifecycleTest('AI lifecycle no-CID input: changed HTML rejects stale delta, Done and delayed safe read', async () => {
  const original = seed({ cid: null });
  const request = deferred(); const read = deferred(); let channel;
  setPlan({ ai_translate: args => { channel = args.onChannel; return request.promise; }, get_article: () => read.promise });
  await act(async () => useAppStore.getState().translateEntry('101'));
  await waitFor(() => !!channel);
  await act(async () => { channel.onmessage({ type: 'delta', data: 'old fragment' }); channel.onmessage({ type: 'done' }); request.resolve(''); });
  await settle();
  const fresh = { ...original, content_html: '<p>replacement no-CID body</p>', translated_content: '' };
  await act(async () => {
    applyBodyRow('101', fresh, getBodyEntry('101').contentRevision, { aiSummary: '', translatedContent: '' });
    read.resolve({ ...original, translated_content: '<p>OLD CLEAN</p>', translated_sig: 'old-sig' });
  });
  await settle();
  assert.equal(getBodyEntry('101').content, fresh.content_html);
  assert.equal(getBodyEntry('101').translatedContent, '');
  assert.equal(invokeCount('ai_translate'), 1);
});

lifecycleTest('AI lifecycle complete lifecycle: dual generation, async read and verification all settle before leaving test', async () => {
  const row = seed({ auto: true });
  const channels = {}; const requests = { summary: deferred(), translation: deferred() };
  const safeReads = [];
  setPlan({ ai_summarize: args => { channels.summary = args.onChannel; return requests.summary.promise; },
    ai_translate: args => { channels.translation = args.onChannel; return requests.translation.promise; },
    get_article: () => { const gate = deferred(); safeReads.push(gate); return gate.promise; } });
  await act(async () => {
    useAppStore.getState().startAutomaticAi('101', 'summary', () => true);
    useAppStore.getState().startAutomaticAi('101', 'translation', () => true);
  });
  await waitFor(() => !!channels.summary && !!channels.translation);
  await act(async () => {
    channels.translation.onmessage({ type: 'delta', data: '<p>raw translation</p>' });
    channels.summary.onmessage({ type: 'delta', data: 'raw summary' });
    channels.summary.onmessage({ type: 'done' }); requests.summary.resolve('');
    channels.translation.onmessage({ type: 'done' }); requests.translation.resolve('');
  });
  await waitFor(() => safeReads.length === 2);
  const finalRow = { ...row, ai_summary: 'final summary', ai_summary_sig: 'summary-sig', translated_content: '<p>final translation</p>', translated_sig: 'translation-sig' };
  await act(async () => { setPlan({ ai_cache_check: () => Promise.resolve(HIT) }); safeReads.forEach(g => g.resolve(finalRow)); });
  await settle();
  assert.equal(getBodyEntry('101').aiSummary, 'final summary');
  assert.equal(getBodyEntry('101').translatedContent, '<p>final translation</p>');
  assert.equal(useAppStore.getState().summarizingIds['101'], undefined);
  assert.equal(useAppStore.getState().translatingIds['101'], undefined);
  assert.equal(useAppStore.getState().rawTranslatedIds['101'], undefined);
  assert.equal(selectAiProductState(useAppStore.getState(), '101', 'summary'), 'complete');
  assert.equal(selectAiProductState(useAppStore.getState(), '101', 'translation'), 'complete');
  await act(async () => {
    useAppStore.getState().startAutomaticAi('101', 'summary', () => true);
    useAppStore.getState().startAutomaticAi('101', 'translation', () => true);
  });
  await settle();
  assert.deepEqual(invokeCalls.filter(c => c.cmd === 'ai_translate' || c.cmd === 'ai_summarize').map(c => c.args.articleId), [101, 101]);
});

lifecycleTest('AI lifecycle ownership: old read and error must not clear a newer translation stream or overwrite its completion', async () => {
  const row = seed();
  const runs = []; const reads = [];
  setPlan({ ai_translate: args => { const gate = deferred(); runs.push({ channel: args.onChannel, ...gate }); return gate.promise; },
    get_article: () => { const gate = deferred(); reads.push(gate); return gate.promise; } });
  await act(async () => { useAppStore.setState({ activeArticleId: '101' }); useAppStore.getState().toggleReaderTranslation(); });
  await waitFor(() => runs.length === 1);
  await act(async () => { runs[0].channel.onmessage({ type: 'delta', data: 'first raw' }); runs[0].channel.onmessage({ type: 'done' }); runs[0].resolve(''); });
  await settle();
  assert.equal(reads.length, 1);
  await act(async () => { useAppStore.setState(s => ({ aiStatusRevision: s.aiStatusRevision + 1 })); useAppStore.getState().toggleReaderTranslation({ force: true }); });
  await waitFor(() => runs.length === 2);
  await act(async () => {
    runs[1].channel.onmessage({ type: 'delta', data: 'new raw' });
    reads[0].resolve({ ...row, translated_content: '<h1>old safe result</h1>', translated_sig: 'old' });
    runs[0].channel.onmessage({ type: 'error', data: 'late old error' });
  });
  await settle();
  assert.equal(getBodyEntry('101').translatedContent, 'new raw');
  assert.equal(useAppStore.getState().rawTranslatedIds['101'], true);
  assert.equal(useAppStore.getState().translatingIds['101'], true);
  assert.equal(useAppStore.getState().translateErrors['101'], '');
  await act(async () => { runs[1].channel.onmessage({ type: 'done' }); runs[1].resolve(''); });
  await waitFor(() => reads.length === 2);
  await act(async () => { setPlan({ ai_cache_check: () => Promise.resolve(HIT) }); reads[1].resolve({ ...row, translated_content: '<h2>new safe result</h2>', translated_sig: 'new' }); });
  await settle();
  assert.equal(getBodyEntry('101').translatedContent, '<h2>new safe result</h2>');
  assert.equal(useAppStore.getState().rawTranslatedIds['101'], undefined);
  assert.equal(useAppStore.getState().translatingIds['101'], undefined);
  assert.equal(selectAiProductState(useAppStore.getState(), '101', 'translation'), 'complete');
});

lifecycleTest('AI lifecycle adjacent products: summary rejection must leave translation ownership intact', async () => {
  const row = seed(); const summarize = deferred(); const translate = deferred(); let channel;
  setPlan({ ai_summarize: () => summarize.promise, ai_translate: args => { channel = args.onChannel; return translate.promise; } });
  await act(async () => { useAppStore.getState().summarizeEntry('101'); useAppStore.getState().translateEntry('101'); });
  await waitFor(() => !!channel && invokeCount('ai_summarize') === 1);
  await act(async () => { channel.onmessage({ type: 'delta', data: 'translation still streaming' }); summarize.reject(new Error('summary service failure')); });
  await settle();
  assert.ok(useAppStore.getState().summaryErrors['101']);
  assert.equal(useAppStore.getState().summarizingIds['101'], undefined);
  assert.equal(useAppStore.getState().translatingIds['101'], true);
  assert.equal(useAppStore.getState().rawTranslatedIds['101'], true);
  await act(async () => {
    setCurrentRows([{ ...row, translated_content: '<p>translation complete</p>', translated_sig: 'new' }]);
    setPlan({ ai_cache_check: () => Promise.resolve(HIT) });
    channel.onmessage({ type: 'done' }); translate.resolve('');
  });
  await settle();
  assert.equal(getBodyEntry('101').translatedContent, '<p>translation complete</p>');
  assert.equal(useAppStore.getState().rawTranslatedIds['101'], undefined);
  assert.equal(selectAiProductState(useAppStore.getState(), '101', 'translation'), 'complete');
});

for (const path of ['summary', 'card', 'reader']) lifecycleTest(`AI-A2 ${path}: 同输入失败后再次请求被末端取消，释放本次全部标记`, async () => {
  seed(); const kind = path === 'summary' ? 'summary' : 'translation';
  const cmd = kind === 'summary' ? 'ai_summarize' : 'ai_translate';
  const invoke = () => path === 'summary' ? useAppStore.getState().summarizeEntry('101')
    : path === 'card' ? useAppStore.getState().translateEntry('101')
      : useAppStore.getState().toggleReaderTranslation({ force: true });
  setPlan({ [cmd]: async () => { throw new Error('first failure'); } });
  await act(async () => { if (path === 'reader') useAppStore.setState({ activeArticleId: '101' }); invoke(); });
  await waitFor(() => !!(kind === 'summary' ? useAppStore.getState().summaryErrors['101'] : useAppStore.getState().translateErrors['101']));
  await settle();
  assert.equal(invokeCount(cmd), 1);
  await act(async () => {
    invoke();
    const categories = useAppStore.getState().categories.map(c => ({ ...c, feeds: c.feeds.map(f => f.id === '10' ? { ...f, layout: 'image' } : f) }));
    useAppStore.setState({ categories, feedIndex: buildFeedIndex(categories), sourceIndex: buildSourceIndex(categories) });
  });
  await waitFor(() => !useAppStore.getState().summarizingIds['101'] && !useAppStore.getState().translatingIds['101'], { label: '未invoke取消的生成标记已释放' });
  await settle();
  assert.equal(invokeCount(cmd), 1, '最终资格检查阻止第二次费用');
  assert.equal(useAppStore.getState().summarizingIds['101'], undefined);
  assert.equal(useAppStore.getState().translatingIds['101'], undefined);
  assert.equal(useAppStore.getState().rawTranslatedIds['101'], undefined);
  assert.equal(useAppStore.getState().translating, false);
  assert.notEqual(selectAiProductState(useAppStore.getState(), '101', kind), 'generating');
});

for (const sig of [null, 'new-signed']) lifecycleTest(`AI-A1 相同HTML由安全水合替换raw归属（signature=${sig}）立即恢复HTML`, async () => {
  const row = seed(); const request = deferred(); let channel;
  const html = '<h2>same text</h2>';
  setPlan({ ai_translate: args => { channel = args.onChannel; return request.promise; } });
  await act(async () => useAppStore.setState({ activeArticleId: '101' }));
  const mounted = await mount(createElement(Reader));
  await act(async () => useAppStore.getState().toggleReaderTranslation());
  await waitFor(() => !!channel);
  try {
  await act(async () => channel.onmessage({ type: 'delta', data: html }));
  assert.equal(useAppStore.getState().rawTranslatedIds['101'], true);
  const fresh = { ...row, content_revision: 2, content_html: '<p>new body</p>', translated_content: html, translated_sig: sig };
  await act(async () => {
    setCurrentRows([fresh]);
    useAppStore.setState(s => ({ entries: s.entries.map(e => ({ ...e, contentRevision: 2 })) }));
    invalidateContentRevision('101', 301, 2);
    applyBodyRow('101', fresh, markBodyLoading('101'), { aiSummary: '', translatedContent: '' });
  });
  await settle();
  assert.equal(getBodyEntry('101').translatedContent, html);
  assert.equal(useAppStore.getState().rawTranslatedIds['101'], undefined, '不用等旧请求终态才解除旧raw');
  assert.equal(mounted.container.querySelector('h2')?.textContent, 'same text');
  await act(async () => { channel.onmessage({ type: 'delta', data: 'OLD' }); channel.onmessage({ type: 'done' }); request.resolve(''); });
  await settle();
  assert.equal(getBodyEntry('101').translatedContent, html);
  assert.equal(useAppStore.getState().translatingIds['101'], undefined);
  } finally {
    await act(async () => request.resolve('')); await settle(); await unmount(mounted);
  }
});

lifecycleTest('AI-A1 失效仍自有raw被丢弃，新正文水合的loading/stamp不受终态清理影响', async () => {
  const row = seed(); const request = deferred(); let channel;
  setPlan({ ai_translate: args => { channel = args.onChannel; return request.promise; } });
  await act(async () => useAppStore.getState().translateEntry('101'));
  await waitFor(() => !!channel);
  await act(async () => channel.onmessage({ type: 'delta', data: '<script>untrusted()</script>' }));
  let stamp;
  await act(async () => {
    useAppStore.setState(s => ({ entries: s.entries.map(e => ({ ...e, contentRevision: 2 })) }));
    invalidateContentRevision('101', 301, 2); stamp = markBodyLoading('101');
    channel.onmessage({ type: 'done' }); request.resolve('');
  });
  await settle();
  assert.equal(getBodyEntry('101').translatedContent, '');
  assert.equal(getBodyEntry('101').state, 'loading');
  assert.equal(getBodyEntry('101').contentRevision, stamp);
  assert.equal(useAppStore.getState().rawTranslatedIds['101'], undefined);
  let accepted;
  await act(async () => { accepted = applyBodyRow('101', { ...row, content_revision: 2, content_html: '<p>new body</p>' }, stamp, { aiSummary: '', translatedContent: '' }); });
  await settle();
  assert.equal(accepted, true);
  assert.equal(getBodyEntry('101').content, '<p>new body</p>');
});

lifecycleTest('AI 同输入真实失败保留partial/raw，明确手动继续完成后才切换净化HTML', async () => {
  const row = seed(); const first = deferred(); let channel;
  const plan = { ...MISS, translationSegments: { total: 3, completed: 1, discarded: 0 } };
  setPlan({ ai_cache_check: () => Promise.resolve(plan), ai_translate: args => { channel = args.onChannel; return first.promise; } });
  await act(async () => useAppStore.getState().translateEntry('101'));
  await waitFor(() => !!channel);
  await act(async () => { channel.onmessage({ type: 'delta', data: '<p>partial' }); channel.onmessage({ type: 'error', data: 'interrupted' }); first.resolve(''); });
  await settle();
  assert.equal(getBodyEntry('101').translatedContent, '<p>partial');
  assert.equal(useAppStore.getState().rawTranslatedIds['101'], true);
  assert.equal(useAppStore.getState().translatingIds['101'], undefined);
  assert.equal(selectAiProductState(useAppStore.getState(), '101', 'translation'), 'partial');
  await act(async () => {
    setCurrentRows([{ ...row, translated_content: '<p>complete</p>', translated_sig: 'new' }]);
    setPlan({ ai_cache_check: () => Promise.resolve(HIT), ai_translate: async args => { args.onChannel.onmessage({ type: 'delta', data: 'new part' }); args.onChannel.onmessage({ type: 'done' }); return ''; } });
    useAppStore.getState().translateEntry('101');
  });
  await waitFor(() => getBodyEntry('101')?.translatedContent === '<p>complete</p>'
    && selectAiProductState(useAppStore.getState(), '101', 'translation') === 'complete', { label: '续译安全回读及验签完整终态' });
  await settle();
  assert.equal(invokeCount('ai_translate'), 2);
  assert.equal(useAppStore.getState().rawTranslatedIds['101'], undefined);
  assert.equal(getBodyEntry('101').translatedContent, '<p>complete</p>');
  assert.equal(selectAiProductState(useAppStore.getState(), '101', 'translation'), 'complete');
});


lifecycleTest('AI fallback owner：未水合行raw随NULL产物水合进入缓存，不能误作净化HTML', async () => {
  const row = seed(); const request = deferred(); const hydration = deferred(); let channel;
  await act(async () => dropBodyEntry('101'));
  setPlan({ ai_translate: args => { channel = args.onChannel; return request.promise; }, get_article: () => hydration.promise });
  await act(async () => useAppStore.getState().translateEntry('101'));
  await waitFor(() => !!channel);
  const html = '<h2>raw fallback</h2><script>unsafe()</script>';
  await act(async () => channel.onmessage({ type: 'delta', data: html }));
  assert.equal(useAppStore.getState().entries[0].translatedContent, html);
  assert.equal(typeof useAppStore.getState().entries[0].translationDraftOwner, 'symbol');
  await act(async () => useAppStore.getState().ensureArticleContent('101'));
  await act(async () => hydration.resolve({ ...row, translated_content: null }));
  await settle();
  assert.equal(getBodyEntry('101').translatedContent, html);
  assert.equal(typeof getBodyEntry('101').translationDraftOwner, 'symbol');
  assert.equal(useAppStore.getState().rawTranslatedIds['101'], true);
  await act(async () => useAppStore.setState({ activeArticleId: '101', isShowingTranslatedProse: true }));
  const mounted = await mount(createElement(Reader));
  assert.equal(mounted.container.querySelector('.article-prose h2'), null);
  assert.equal(mounted.container.querySelector('.article-prose script'), null);
  assert.ok(mounted.container.querySelector('.article-prose').textContent.includes('<h2>raw fallback</h2>'));
  await act(async () => { channel.onmessage({ type: 'error', data: 'partial' }); request.resolve(''); }); await settle();
  assert.equal(useAppStore.getState().rawTranslatedIds['101'], true, '真实失败保未消毒残段');
  const safe = { ...row, translated_content: '<h2>safe finish</h2>', translated_sig: 'safe' };
  await act(async () => {
    setPlan({ ai_cache_check: () => Promise.resolve(HIT), get_article: () => Promise.resolve(safe), ai_translate: async args => { args.onChannel.onmessage({ type: 'done' }); return ''; } });
    useAppStore.getState().toggleReaderTranslation({ force: true });
  });
  await waitFor(() => selectAiProductState(useAppStore.getState(), '101', 'translation') === 'complete', { label: 'fallback重试安全完成' });
  await settle();
  assert.equal(getBodyEntry('101').translatedContent, '<h2>safe finish</h2>');
  assert.equal(useAppStore.getState().entries[0].translationDraftOwner, undefined);
  assert.equal(useAppStore.getState().rawTranslatedIds['101'], undefined);
  assert.equal(mounted.container.querySelector('.article-prose h2')?.textContent, 'safe finish');
  await unmount(mounted);
});


lifecycleTest('AI fallback safe transfer：安全水合接管行草稿后缓存释放不会复活旧raw', async () => {
  const row = seed(); const request = deferred(); let channel;
  await act(async () => dropBodyEntry('101'));
  setPlan({ ai_translate: args => { channel = args.onChannel; return request.promise; } });
  await act(async () => useAppStore.getState().translateEntry('101'));
  await waitFor(() => !!channel);
  const html = '<h2>same safe text</h2>';
  await act(async () => channel.onmessage({ type: 'delta', data: html }));
  await act(async () => applyBodyRow('101', { ...row, translated_content: html, translated_sig: 'safe' }, markBodyLoading('101'), { aiSummary: '', translatedContent: html }));
  await settle();
  assert.equal(useAppStore.getState().rawTranslatedIds['101'], undefined);
  await act(async () => dropBodyEntry('101'));
  await settle();
  assert.equal(useAppStore.getState().entries[0].translatedContent, html);
  assert.equal(useAppStore.getState().entries[0].translationDraftOwner, undefined, '安全水合接管后旧行草稿owner已解除');
  assert.equal(useAppStore.getState().rawTranslatedIds['101'], undefined, '缓存释放不能复活旧raw');
  await act(async () => { channel.onmessage({ type: 'error', data: 'terminal' }); request.resolve(''); });
  await settle();
});
