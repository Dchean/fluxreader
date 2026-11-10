import assert from 'node:assert/strict';
import { componentTest, mount, unmount, flush, waitFor, resetStore, getBodyEntry, useAppStore } from './harness.mjs';
import { installBackend, seedFromRows, mkRow, invokeCount, invokeCalls, setPlan, resetPlan, deferred, currentRows } from './fixtures.mjs';

const { createElement, act } = await import('react');
const { Reader } = await import('../../src/components/Reader.tsx');
const { Timeline } = await import('../../src/components/Timeline.tsx');
const { ReadingTab } = await import('../../src/components/settings/ReadingTab.tsx');
const { FeedsTab } = await import('../../src/components/settings/FeedsTab.tsx');
const { selectFeedConfig, selectAiCacheStatus } = await import('../../dist-test/store.js');
const { resolveAiPolicy } = await import('../../dist-test/store/aiPolicy.js');
const { buildFeedIndex, buildSourceIndex } = await import('../../dist-test/store/internals.js');
const { invalidateContentRevision, markBodyLoading, applyBodyRow } = await import('../../dist-test/store/bodyCache.js');

const aiTest = (name, body) => componentTest(name, async () => {
  try { await body();
    await waitFor(() => !Object.keys(useAppStore.getState().summarizingIds).length
      && !Object.keys(useAppStore.getState().translatingIds).length && !useAppStore.getState().translating, { label: '本域AI终态标记全部释放' });
    await flush(8);
  } finally { resetPlan(); if (Object.hasOwn(document, 'visibilityState')) delete document.visibilityState; }
});

const MISS = { summaryVerified: false, translationVerified: false, translationSegments: { total: 1, completed: 0, discarded: 0 } };
const enabled = { articleAutoSummary: true, articleAutoTranslate: true, socialAutoTranslate: true,
  notificationAutoSummary: true, notificationAutoTranslate: true, markReadOnOpen: false,
  markReadOnScrollBottom: false, markReadOnScrollOut: false };
function setup(layout = 'article', rows) {
  resetStore(); resetPlan(); installBackend();
  // jsdom没有原生窗口前台状态；真实focus/blur事件仍驱动生产控制器。
  document.body.tabIndex = -1; document.body.focus();
  const feed = { article: 10, social: 11, notification: 14, image: 12, podcast: 13 }[layout];
  rows ??= [mkRow({ id: 101, feed_id: feed, content_id: 301, content_revision: 1, content_html: '<p>Hello <b>world</b></p>' })];
  seedFromRows(rows, { layout });
  const categories = useAppStore.getState().categories.map(c => ({ ...c, summaryMode: 'inherit', translateMode: 'inherit', feeds: c.feeds.map(f => ({ ...f, summaryMode: 'inherit', translateMode: 'inherit' })) }));
  useAppStore.setState({ categories, feedIndex: buildFeedIndex(categories), sourceIndex: buildSourceIndex(categories) });
  useAppStore.setState((s) => ({ settings: { ...s.settings, ...enabled } }));
  setPlan({ ai_cache_check: () => Promise.resolve(MISS),
    ai_summarize: async (args) => { args.onChannel.onmessage({ type: 'error', data: '测试失败' }); return ''; },
    ai_translate: async (args) => { args.onChannel.onmessage({ type: 'error', data: '测试失败' }); return ''; },
  });
  return rows;
}
async function focus() { await act(async () => window.dispatchEvent(new Event('focus'))); }
async function pause(ms = 780) { await act(async () => { await new Promise((resolve) => setTimeout(resolve, ms)); }); }
function rect(top, height, width = 700) { return { top, bottom: top + height, left: 0, right: width, width, height, x: 0, y: top }; }
async function center(mounted, id = '101') {
  const root = mounted.container.querySelector('#timelineContentScroll');
  root.getBoundingClientRect = () => rect(0, 768);
  const card = root.querySelector(`[data-id="${id}"]`);
  assert.ok(card, '卡片真实挂载');
  card.getBoundingClientRect = () => rect(220, 350);
  await act(async () => window.dispatchEvent(new Event('resize')));
  await focus();
  return { root, card };
}

aiTest('AI policy：5布局×3组态×3源态×2默认，能力/实际布局与retained来源', async () => {
  setup();
  for (const layout of ['article', 'social', 'notification', 'image', 'podcast']) {
    for (const group of ['inherit', 'on', 'off']) for (const source of ['inherit', 'on', 'off']) for (const value of [false, true]) {
      const settings = { ...useAppStore.getState().settings, ...Object.fromEntries(Object.keys(enabled).filter(k => k.includes('Auto')).map(k => [k, value])) };
      const p = resolveAiPolicy(settings, layout, { summaryMode: group, translateMode: group }, { summaryMode: source, translateMode: source });
      const expected = source === 'on' || (source === 'inherit' && (group === 'on' || (group === 'inherit' && value)));
      assert.equal(p.autoSummary, ['article', 'notification'].includes(layout) && expected, `${layout}/${group}/${source}/${value}:summary`);
      assert.equal(p.autoTranslate, ['article', 'social', 'notification'].includes(layout) && expected, `${layout}/${group}/${source}/${value}:translate`);
    }
  }
  useAppStore.setState((s) => ({ settings: { ...s.settings, articleAutoTranslate: false, socialAutoTranslate: true } }));
  assert.equal(selectFeedConfig(useAppStore.getState(), '11').autoTranslate, true, '源覆盖到社交，继承社交默认');
  const source = useAppStore.getState().sourceIndex.get('11');
  useAppStore.setState({ feedIndex: new Map(), sourceIndex: new Map([['11', { ...source, folderId: 'removed' }]]), categories: [] });
  assert.equal(selectFeedConfig(useAppStore.getState(), '11').autoTranslate, true, '退订仍按真实源布局');
  assert.equal(selectFeedConfig(useAppStore.getState(), 'missing').canTranslate, false, '未知来源不猜配置');
});

aiTest('AI Reader：实际打开才启动，失败不自动重试，手动重试独立于auto与标读', async () => {
  setup();
  const mounted = await mount(createElement(Reader));
  await focus(); await pause(80);
  assert.equal(invokeCount('ai_summarize'), 0);
  await act(async () => useAppStore.getState().selectArticle('101'));
  await waitFor(() => invokeCount('ai_summarize') === 1 && invokeCount('ai_translate') === 1);
  await waitFor(() => !!useAppStore.getState().summaryErrors['101'] && !!useAppStore.getState().translateErrors['101']
    && !useAppStore.getState().summarizingIds['101'] && !useAppStore.getState().translatingIds['101']
    && !!selectAiCacheStatus(useAppStore.getState(), '101'), { label: '失败及新验签终态' });
  await flush(8);
  assert.equal(useAppStore.getState().entries[0].isRead, false);
  await act(async () => useAppStore.setState((s) => ({ entries: s.entries.map(e => ({ ...e, isStarred: true })) })));
  await pause(100);
  assert.equal(invokeCount('ai_summarize'), 1);
  await act(async () => useAppStore.setState((s) => ({ settings: { ...s.settings, articleAutoSummary: false, articleAutoTranslate: false } })));
  const retry = [...mounted.container.querySelectorAll('button')].find(b => b.textContent === '重试');
  assert.ok(retry);
  await act(async () => retry.click());
  await waitFor(() => invokeCount('ai_summarize') + invokeCount('ai_translate') === 3);
  await waitFor(() => !!useAppStore.getState().summaryErrors['101'] && !!useAppStore.getState().translateErrors['101']
    && !useAppStore.getState().summarizingIds['101'] && !useAppStore.getState().translatingIds['101']
    && !!selectAiCacheStatus(useAppStore.getState(), '101'), { label: '失败及新验签终态' });
  await flush(8);
  assert.equal(useAppStore.getState().entries[0].isRead, false);
});

aiTest('AI Reader：隐藏/切篇/迟到正文不会把旧自动意图发送', async () => {
  const rows = setup('article', [mkRow({ id: 101, content_html: '<p>A</p>' }), mkRow({ id: 102, content_html: '<p>B</p>' })]);
  const gate = deferred();
  setPlan({ get_article: (args) => String(args.id) === '101' ? gate.promise : Promise.resolve(rows[1]) });
  const mounted = await mount(createElement(Reader)); await focus();
  await act(async () => useAppStore.getState().selectArticle('101'));
  await flush(2);
  await act(async () => useAppStore.getState().selectArticle('102'));
  await waitFor(() => invokeCount('ai_summarize') === 1);
  await act(async () => gate.resolve(rows[0])); await pause(80);
  assert.deepEqual(invokeCalls.filter(c => c.cmd === 'ai_summarize').map(c => c.args.articleId), [102]);
  await act(async () => { mounted.container.style.display = 'none'; useAppStore.getState().selectArticle('101'); });
  await pause(80);
  assert.equal(invokeCount('ai_summarize'), 1);
});

aiTest('AI 社交：只有中心连续停顿，快速滑过/JK焦点不会即时生成，属性更新不重发', async () => {
  setup('social');
  const mounted = await mount(createElement(Timeline));
  await waitFor(() => getBodyEntry('101')?.state === 'ready');
  const { root, card } = await center(mounted);
  await pause(200);
  await act(async () => { card.getBoundingClientRect = () => rect(-500, 350); root.dispatchEvent(new Event('scroll')); useAppStore.setState({ activeArticleId: '101' }); });
  await pause();
  assert.equal(invokeCount('ai_translate'), 0, '快速经过和仅焦点不花费');
  await act(async () => { card.getBoundingClientRect = () => rect(220, 350); root.dispatchEvent(new Event('scroll')); });
  await pause(300);
  assert.equal(invokeCount('ai_translate'), 0, '不足700ms不开始');
  await pause(480);
  assert.equal(invokeCount('ai_translate'), 1);
  assert.equal(invokeCount('ai_summarize'), 0, '社交能力禁止摘要');
  await act(async () => useAppStore.setState((s) => ({ entries: s.entries.map(e => ({ ...e, isStarred: true })) })));
  await pause();
  assert.equal(invokeCount('ai_translate'), 1, '普通属性更新不重发失败');
  assert.equal(invokeCount('set_read') + invokeCount('set_read_bulk'), 0, 'AI不标读');
});

aiTest('AI 通知：浮层/blur/hidden/会话切换均撤销候选，恢复需重新停顿', async () => {
  setup('notification');
  const mounted = await mount(createElement(Timeline));
  await waitFor(() => getBodyEntry('101')?.state === 'ready');
  const { root } = await center(mounted);
  for (const [stop, resume] of [
    [() => useAppStore.setState({ settingsOpen: true }), () => useAppStore.setState({ settingsOpen: false })],
    [() => window.dispatchEvent(new Event('blur')), () => window.dispatchEvent(new Event('focus'))],
    [() => { Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'hidden' }); document.dispatchEvent(new Event('visibilitychange')); },
      () => { Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'visible' }); document.dispatchEvent(new Event('visibilitychange')); }],
  ]) {
    await pause(120); await act(async () => stop()); await pause();
    assert.equal(invokeCount('ai_summarize') + invokeCount('ai_translate'), 0);
    await act(async () => resume());
  }
  await pause(100);
  await act(async () => useAppStore.setState({ activeFeedFilter: '14' }));
  await pause(350);
  assert.equal(invokeCount('ai_summarize'), 0, '切scope重新计时');
  await act(async () => root.dispatchEvent(new Event('scroll')));
  await pause();
  assert.equal(invokeCount('ai_summarize'), 1);
  assert.equal(invokeCount('ai_translate'), 1);
  delete document.visibilityState;
});

aiTest('AI pending：dwell后验签在途再关闭或离开，尚未发送的收费请求撤销', async () => {
  setup('social');
  const gate = deferred(); setPlan({ ai_cache_check: () => gate.promise });
  const mounted = await mount(createElement(Timeline));
  await waitFor(() => getBodyEntry('101')?.state === 'ready');
  const { root, card } = await center(mounted);
  await pause();
  assert.equal(invokeCount('ai_translate'), 0);
  await act(async () => { card.getBoundingClientRect = () => rect(-500, 350); root.dispatchEvent(new Event('scroll')); });
  await act(async () => gate.resolve(MISS)); await pause(80);
  assert.equal(invokeCount('ai_translate'), 0);
  await act(async () => { useAppStore.setState((s) => ({ settings: { ...s.settings, socialAutoTranslate: false } })); card.getBoundingClientRect = () => rect(220, 350); root.dispatchEvent(new Event('scroll')); });
  await pause();
  assert.equal(invokeCount('ai_translate'), 0);
  await act(async () => useAppStore.getState().translateEntry('101'));
  await waitFor(() => invokeCount('ai_translate') === 1);
});

aiTest('AI version：旧流delta/Done/回读不能覆盖新正文与配置，手动仍能重试', async () => {
  const rows = setup('article');
  useAppStore.setState((s) => ({ settings: { ...s.settings, articleAutoSummary: false, articleAutoTranslate: false } }));
  const held = [];
  setPlan({ ai_translate: (args) => new Promise(resolve => held.push({ args, resolve })) });
  const mounted = await mount(createElement(Reader)); await focus();
  await act(async () => useAppStore.getState().selectArticle('101'));
  await waitFor(() => getBodyEntry('101')?.state === 'ready');
  await act(async () => useAppStore.getState().toggleReaderTranslation());
  await waitFor(() => held.length === 1);
  await act(async () => {
    const row = { ...rows[0], content_revision: 2, content_html: '<p>new body</p>' };
    currentRows[0] = row;
    useAppStore.setState((s) => ({ entries: s.entries.map(e => ({ ...e, contentRevision: 2 })) }));
    invalidateContentRevision('101', 301, 2);
    const stamp = markBodyLoading('101'); applyBodyRow('101', row, stamp, { aiSummary: '', translatedContent: '' });
    held[0].args.onChannel.onmessage({ type: 'delta', data: '<p>OLD</p>' });
    held[0].args.onChannel.onmessage({ type: 'done' }); held[0].resolve('');
  });
  await flush(3);
  assert.equal(getBodyEntry('101').content, '<p>new body</p>');
  assert.equal(getBodyEntry('101').translatedContent, '');
  await act(async () => useAppStore.getState().toggleReaderTranslation({ force: true }));
  await waitFor(() => held.length === 2);
  await act(async () => { useAppStore.setState(s => ({ aiStatusRevision: s.aiStatusRevision + 1 })); held[1].args.onChannel.onmessage({ type: 'delta', data: 'old config' }); held[1].args.onChannel.onmessage({ type: 'done' }); held[1].resolve(''); });
  await flush(3);
  assert.equal(getBodyEntry('101').translatedContent, '');
  assert.equal(useAppStore.getState().translatingIds['101'], undefined);
  assert.ok(mounted.container.querySelector('.reader-col'));
});

aiTest('AI settings：布局五开关与组三态真实控件发当前字段patch，源off覆盖group on', async () => {
  setup();
  const reading = await mount(createElement(ReadingTab));
  assert.equal(reading.container.querySelectorAll('[aria-label$="自动摘要"],[aria-label$="自动翻译"]').length, 5);
  const feeds = await mount(createElement(FeedsTab));
  const group = feeds.container.querySelector('[aria-label="技术默认自动摘要"]');
  assert.ok(group); await act(async () => group.click()); await flush();
  const option = [...document.querySelectorAll('[role="option"]')].find(e => e.textContent === '开启');
  assert.ok(option); await act(async () => option.click()); await flush(3);
  assert.equal(useAppStore.getState().categories[0].summaryMode, 'on');
  assert.deepEqual(invokeCalls.find(c => c.cmd === 'set_folder_ai_modes').args, { id: 1, summary: 'on' });
  await act(async () => useAppStore.getState().toggleFeedSummary('cat-1', '10', 'off'));
  assert.equal(selectFeedConfig(useAppStore.getState(), '10').autoSummary, false);
  // Configuration adapter keeps the monotonic write protocol; failed latest intent rolls back mode.
  setPlan({ set_folder_ai_modes: () => Promise.reject(new Error('write failed')) });
  await act(async () => useAppStore.getState().toggleCatSummary('cat-1', 'off')); await flush(4);
  assert.equal(useAppStore.getState().categories[0].summaryMode, 'on');
  assert.equal(useAppStore.getState().feedIndex.get('10').cat.summaryMode, 'on');
});

aiTest('AI manual capabilities：社交只翻译，画廊/播客零收费入口，未知来源拒绝', async () => {
  for (const layout of ['social', 'image', 'podcast']) {
    setup(layout);
    useAppStore.getState().summarizeEntry('101');
    useAppStore.getState().translateEntry('101');
    await flush(4);
    assert.equal(invokeCount('ai_summarize'), 0);
    assert.equal(invokeCount('ai_translate'), layout === 'social' ? 1 : 0);
  }
});

aiTest('AI existing：旧签名/无签名/失败/partial均不因打开重烧，明确manual能恢复', async () => {
  for (const state of ['stale', 'unsigned', 'partial', 'failed']) {
    const row = mkRow({ id: 101, content_html: '<p>body</p>',
      translated_content: state === 'stale' || state === 'unsigned' ? '<p>旧产物</p>' : null,
      translated_sig: state === 'stale' ? 'old-signature' : null });
    setup('article', [row]);
    useAppStore.setState(s => ({ settings: { ...s.settings, articleAutoSummary: false },
      translateErrors: state === 'failed' ? { 101: '失败' } : {} }));
    if (state === 'partial') setPlan({ ai_cache_check: () => Promise.resolve({ ...MISS,
      translationSegments: { total: 3, completed: 1, discarded: 0 } }) });
    const mounted = await mount(createElement(Reader)); await focus();
    await act(async () => useAppStore.getState().selectArticle('101'));
    await waitFor(() => getBodyEntry('101')?.state === 'ready'); await pause(80);
    assert.equal(invokeCount('ai_translate'), 0, `${state}不自动重烧`);
    await act(async () => useAppStore.getState().toggleReaderTranslation({ force: true }));
    await waitFor(() => invokeCount('ai_translate') === 1 && !!useAppStore.getState().translateErrors['101']
      && !useAppStore.getState().translatingIds['101'] && !!selectAiCacheStatus(useAppStore.getState(), '101'), { label: '手动恢复失败/分段验签已落定' });
    await flush(8);
    await unmount(mounted);
  }
});

aiTest('AI cancel：验签前关开不能复活旧intent，多次scroll/unmount无后续收费', async () => {
  setup('social'); const gate = deferred(); setPlan({ ai_cache_check: () => gate.promise });
  const mounted = await mount(createElement(Timeline));
  await waitFor(() => getBodyEntry('101')?.state === 'ready');
  const { root } = await center(mounted); await pause();
  await act(async () => {
    useAppStore.setState(s => ({ settings: { ...s.settings, socialAutoTranslate: false } }));
    useAppStore.setState(s => ({ settings: { ...s.settings, socialAutoTranslate: true } }));
    gate.resolve(MISS);
  });
  await pause(100); assert.equal(invokeCount('ai_translate'), 0, '旧验签不能在关开后复活');
  await act(async () => { for (let i = 0; i < 8; i++) root.dispatchEvent(new Event('scroll')); });
  await pause(200); await unmount(mounted); await pause();
  assert.equal(invokeCount('ai_translate'), 0, '卸载取消唯一候选timer');
});

for (const firstKind of ['summary', 'translation']) aiTest(`AI dual：${firstKind}先流动不取消另一产物验签，净化HTML结构保留`, async () => {
  const rows = setup(); const checks = [];
  setPlan({ ai_cache_check: () => new Promise(resolve => checks.push(resolve)),
    ai_summarize: async args => { args.onChannel.onmessage({ type: 'delta', data: '摘要' }); args.onChannel.onmessage({ type: 'done' }); return ''; },
    ai_translate: async args => {
      args.onChannel.onmessage({ type: 'delta', data: '<script>bad()</script><p>raw</p>' });
      currentRows[0] = { ...rows[0], translated_content: '<h2>标题</h2><ol start="3"><li><b>译文</b></li></ol>', translated_sig: 'current' };
      args.onChannel.onmessage({ type: 'done' }); return '';
    },
  });
  const mounted = await mount(createElement(Reader)); await focus();
  await act(async () => useAppStore.getState().selectArticle('101'));
  await waitFor(() => checks.length >= 3);
  const [first, second] = firstKind === 'summary' ? [1, 2] : [2, 1];
  await act(async () => { checks[first](MISS); await new Promise(r => setTimeout(r, 30)); });
  assert.equal(invokeCount(firstKind === 'summary' ? 'ai_summarize' : 'ai_translate'), 1);
  await act(async () => { checks[second](MISS); await new Promise(r => setTimeout(r, 30)); });
  assert.equal(invokeCount('ai_summarize'), 1);
  assert.equal(invokeCount('ai_translate'), 1);
  assert.equal(mounted.container.querySelector('ol')?.start, 3);
  assert.equal(mounted.container.querySelector('ol b')?.textContent, '译文');
  assert.equal(mounted.container.querySelector('script'), null);
  await act(async () => { checks.forEach(resolve => resolve(MISS)); await new Promise(r => setTimeout(r, 20)); });
});

aiTest('AI hydration：手动请求先发，当前CID/rev普通水合不会丢完成产物', async () => {
  const rows = setup();
  useAppStore.setState(s => ({ settings: { ...s.settings, articleAutoSummary: false, articleAutoTranslate: false } }));
  const body = deferred(); const request = deferred(); let channel;
  setPlan({ get_article: () => body.promise, ai_translate: args => { channel = args.onChannel; return request.promise; } });
  await act(async () => useAppStore.getState().selectArticle('101'));
  await act(async () => useAppStore.getState().toggleReaderTranslation());
  await waitFor(() => !!channel);
  const clean = { ...rows[0], translated_content: '<p>late hydration safe</p>', translated_sig: 'current' };
  await act(async () => body.resolve(clean)); await flush(3);
  await act(async () => { channel.onmessage({ type: 'delta', data: 'raw delta' }); channel.onmessage({ type: 'done' }); request.resolve(''); });
  await flush(4);
  assert.equal(getBodyEntry('101').translatedContent, '<p>late hydration safe</p>');
  assert.equal(useAppStore.getState().rawTranslatedIds['101'], undefined);
  assert.equal(useAppStore.getState().translatingIds['101'], undefined);
});

aiTest('AI mixed group：画廊组含文章/社交子源，组继承标识与关闭费用按实际能力生效', async () => {
  setup();
  const categories = useAppStore.getState().categories.map(c => ({ ...c, layout: 'image',
    feeds: c.feeds.filter(f => f.id === '10' || f.id === '11').map(f => ({ ...f, layout: f.id === '10' ? 'article' : 'social' })) }));
  useAppStore.setState(s => ({ categories, feedIndex: buildFeedIndex(categories), sourceIndex: buildSourceIndex(categories),
    settings: { ...s.settings, articleAutoSummary: true, articleAutoTranslate: false, socialAutoTranslate: true } }));
  const mounted = await mount(createElement(FeedsTab));
  const control = name => mounted.container.querySelector(`[aria-label="${name}"]`);
  const choose = async (name, label) => {
    await act(async () => control(name).click()); await flush();
    const option = [...document.querySelectorAll('[role="option"]')].find(e => e.textContent === label);
    assert.ok(option, `${name} ${label}`); await act(async () => option.click()); await flush(3);
  };
  for (const name of ['技术默认自动摘要', '技术默认自动翻译']) {
    assert.notEqual(control(name).closest('label').style.display, 'none', '非AI组有支持成员时组控件可用');
    assert.equal(control(name).textContent, '继承布局', '混合成员不能显示统一继承开关值');
  }
  assert.ok(control('源A自动摘要').textContent.includes('继承（开）'));
  assert.ok(control('源A自动翻译').textContent.includes('继承（关）'));
  assert.ok(control('源B自动翻译').textContent.includes('继承（开）'));
  assert.equal(control('源B自动摘要').closest('.mgr-ai-mode-label').style.display, 'none', '社交无摘要能力');
  await choose('技术默认自动翻译', '关闭');
  assert.equal(selectFeedConfig(useAppStore.getState(), '10').autoTranslate, false);
  assert.equal(selectFeedConfig(useAppStore.getState(), '11').autoTranslate, false);
  await choose('源A自动摘要', '关闭');
  await choose('技术默认自动摘要', '开启');
  assert.equal(selectFeedConfig(useAppStore.getState(), '10').autoSummary, false, '组on不改源显式off');
  assert.equal(selectFeedConfig(useAppStore.getState(), '11').autoSummary, false, '能力限制最后钳制');
  await choose('技术默认自动摘要', '关闭');
  assert.deepEqual(invokeCalls.filter(c => c.cmd === 'set_folder_ai_modes').map(c => c.args), [
    { id: 1, translate: 'off' }, { id: 1, summary: 'on' }, { id: 1, summary: 'off' },
  ]);
  await choose('源A自动摘要', '继承（关）');
  await act(async () => useAppStore.getState().ensureArticleContent('101'));
  await waitFor(() => getBodyEntry('101')?.state === 'ready');
  useAppStore.getState().startAutomaticAi('101', 'summary', () => true);
  useAppStore.getState().startAutomaticAi('101', 'translation', () => true);
  await flush(3);
  assert.equal(invokeCount('ai_summarize') + invokeCount('ai_translate'), 0, 'group off无收费请求');
  await act(async () => {
    const noAi = categories.map(c => ({ ...c, feeds: c.feeds.map(f => ({ ...f, layout: 'image' })) }));
    useAppStore.setState({ categories: noAi, feedIndex: buildFeedIndex(noAi), sourceIndex: buildSourceIndex(noAi) });
  });
  for (const name of ['技术默认自动摘要', '技术默认自动翻译']) {
    assert.equal(control(name).closest('label').style.display, 'none', '全组无能力则隐藏');
  }
});
