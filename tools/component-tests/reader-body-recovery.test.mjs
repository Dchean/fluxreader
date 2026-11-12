// 真实 Reader + store/bodyCache；仅控制 IPC 的结果与时机，不复制恢复实现。
import assert from 'node:assert/strict';
import { componentTest, mount, unmountAll, flush, waitFor, resetStore, useAppStore, getBodyEntry, bodyCacheMod } from './harness.mjs';
import { installBackend, seedFromRows, mkRow, setCurrentRows, setPlan, resetPlan, deferred, invokeCount, invokeCalls } from './fixtures.mjs';
const { createElement, act } = await import('react');
const { Reader } = await import('../../src/components/Reader.tsx');
const media = await import('../../src/lib/imageProxy.ts');
const cover = await import('../../src/lib/coverImage.ts');
const missingRow = (overrides = {}) => mkRow({ id: 101, title: '本机缺正文', content_id: null,
  content_revision: 0, body_missing: true, content_html: null, is_read: true, ...overrides });
const success = (overrides = {}) => ({ html: '<p>从原网页获得的正文</p>', degraded: false,
  reason: null, contentId: 501, contentRevision: 2, ...overrides });
const findButton = (root, text) => [...root.querySelectorAll('button')].find(button => button.textContent.trim() === text);
const recovery = root => root.querySelector('[aria-label="本机未保存正文"]');
const extractCalls = () => invokeCalls.filter(call => call.cmd === 'extract_fulltext');
const png = () => { const bytes = new Uint8Array(64); bytes.set([137, 80, 78, 71]); return bytes.buffer; };

function recoveryTest(name, body) {
  componentTest(name, { timeout: 40000 }, async () => {
    const settings = { ...useAppStore.getState().settings };
    const pending = [];
    const gate = () => { const next = deferred(); pending.push(next); return next; };
    try {
      installBackend(); resetPlan(); resetStore(); cover.resetCoverCacheForTest();
      useAppStore.setState(s => ({ timelineFilter: 'all', settings: { ...s.settings, markReadOnOpen: false,
        markReadOnScrollBottom: false, defaultOpenMode: 'rss' } }));
      await body(gate);
    } finally {
      await unmountAll();
      await act(async () => { for (const item of pending) item.resolve(null); });
      await flush(3);
      resetPlan(); media.resumeImages();
      await act(async () => useAppStore.setState({ settings }));
    }
  });
}
async function open(rows = [missingRow()]) {
  seedFromRows(rows);
  const view = await mount(createElement(Reader));
  await act(async () => useAppStore.getState().selectArticle(String(rows[0].id)));
  await waitFor(() => getBodyEntry(String(rows[0].id))?.state === 'ready', { label: '本机详情完成' });
  return view;
}

async function evictFailedTarget() {
  const target = missingRow(); const view = await open([target]);
  setPlan({ extract_fulltext: () => Promise.reject({ message: '首次网页失败' }) });
  await act(async () => findButton(view.container, '从原网页重新获取').click()); await flush(3);
  const error = useAppStore.getState().errors.find(item => item.key === 'reader:fulltext:101');
  assert.ok(error);
  await act(async () => useAppStore.getState().clearReaderSelection()); await unmountAll();
  const rows = Array.from({ length: bodyCacheMod.BODY_CACHE_MAX }, (_, index) => mkRow({ id: 1000 + index,
    title: `其它文章${index}`, content_id: 10000 + index, content_revision: 1, content_html: '<p>其它正文</p>' }));
  await act(async () => {
    seedFromRows(rows); setCurrentRows([target, ...rows]);
    useAppStore.getState().hydrateArticleContent(rows.map(row => String(row.id)));
  }); await flush(3);
  assert.equal(getBodyEntry('101'), undefined, '经公开水合写满预算后原失败目标真实被LRU淘汰');
  return { target, error, rows };
}

recoveryTest('缺正文：存在的空正文与缺行分开，普通reload/智能全文开关不会为缺正文自动联网', async () => {
  const rows = [missingRow(), missingRow({ id: 102, title: '自然空源正文', body_missing: false, content_html: '  ' })];
  const view = await open(rows);
  assert.ok(recovery(view.container));
  assert.ok(findButton(view.container, '从原网页重新获取'));
  assert.equal(findButton(view.container, '查看原文').disabled, false);
  assert.doesNotMatch(view.container.textContent, /已清理|文章不存在/);
  await act(async () => useAppStore.getState().reloadFromBackend());
  await act(async () => {
    useAppStore.setState(s => ({ settings: { ...s.settings, defaultOpenMode: 'fulltext' } }));
    useAppStore.getState().selectArticle('102');
  });
  await flush(3);
  assert.ok(recovery(view.container), '自然空正文也如实说本机未保存，不虚称被清理');
  assert.equal(invokeCount('extract_fulltext'), 0);
  assert.equal(invokeCount('set_read') + invokeCount('set_read_bulk') + invokeCount('set_starred'), 0);
  setPlan({ get_article: () => null });
  await act(async () => useAppStore.getState().selectArticle('101'));
  await flush(3);
  assert.equal(getBodyEntry('101').state, 'missing');
  assert.match(view.container.textContent, /文章不存在/);
  assert.ok(!recovery(view.container));
  assert.ok(!findButton(view.container, '从原网页重新获取'));
});

for (const url of [null, 'javascript:alert(1)', 'ftp://example.com/a', '/relative', 'not-a-url']) {
  recoveryTest(`缺正文/无可用URL ${url}：仅显示真实不可恢复提示，不调用提取`, async () => {
    const view = await open([missingRow({ url })]);
    assert.match(recovery(view.container).textContent, /没有可用的原文链接，无法重新获取/);
    assert.ok(!findButton(view.container, '从原网页重新获取'));
    assert.equal(findButton(view.container, '查看原文').disabled, true);
    await act(async () => useAppStore.getState().extractCurrentArticle());
    assert.equal(invokeCount('extract_fulltext'), 0);
  });
}

recoveryTest('缺正文恢复成功：重复点击只提取一次，提交身份/轻媒体回读正确，reload保留新正文且不改读藏', async gate => {
  const row = missingRow({ url: 'https://sspai.com/post/recovery' });
  const request = gate();
  const image = 'https://cdnfile.sspai.com/recovery.png';
  const result = success({ html: `<p>正文恢复成功</p><img src="${image}">` });
  const restored = { ...row, content_html: result.html, content_id: result.contentId,
    content_revision: result.contentRevision, body_missing: false, fulltext_extracted: true,
    gallery_first_image_url: image, image_count: 1, media_revision: 8 };
  setPlan({ extract_fulltext: () => request.promise, fetch_image: () => png() });
  const view = await open([row]);
  await act(async () => {
    findButton(view.container, '从原网页重新获取').click();
    useAppStore.getState().extractCurrentArticle();
  });
  assert.equal(invokeCount('extract_fulltext'), 1);
  assert.equal(findButton(view.container, '正在获取…').disabled, true);
  assert.equal(recovery(view.container).getAttribute('aria-busy'), 'true');
  assert.equal(invokeCount('fetch_image'), 0);
  await act(async () => { setCurrentRows([restored]); request.resolve(result); });
  await flush(5);
  assert.ok(!recovery(view.container));
  assert.match(view.container.querySelector('.article-prose').textContent, /正文恢复成功/);
  assert.equal(getBodyEntry('101').contentId, 501);
  assert.equal(getBodyEntry('101').contentRev, 2);
  assert.equal(getBodyEntry('101').rawContent, '', '网页正文不冒充已丢失的 RSS 字节');
  const entry = useAppStore.getState().entries.find(item => item.id === '101');
  assert.equal(entry.bodyMissing, false); assert.equal(entry.mediaRevision, 8); assert.equal(entry.imageUrl, image);
  assert.equal(entry.isRead, true); assert.equal(entry.isStarred, false);
  const fetch = invokeCalls.find(call => call.cmd === 'fetch_image');
  assert.equal(fetch.args.url, image); assert.equal(fetch.args.pageUrl, row.url);
  await act(async () => useAppStore.getState().reloadFromBackend()); await flush(3);
  assert.equal(getBodyEntry('101').content, result.html);
  assert.equal(invokeCount('extract_fulltext'), 1);
  assert.equal(invokeCount('set_read') + invokeCount('set_read_bulk') + invokeCount('set_starred'), 0);
});

for (const [name, outcome, message] of [
  ['降级', success({ degraded: true, html: '', reason: '网页没有可提取正文' }), /网页没有可提取正文/],
  ['空正文', success({ html: '  \n ' }), /未返回可用正文/],
  ['空响应', null, /未获取到正文/],
  ['缺提交身份', { html: '<p>未确认字节</p>', degraded: false, reason: null }, /未能确认正文已保存/],
  ['网络失败', 'reject', /HTTP 503/],
]) {
  recoveryTest(`缺正文恢复/${name}：仍缺正文，近端错误可见且明确点击能重试`, async () => {
    const view = await open();
    setPlan({ extract_fulltext: () => outcome === 'reject' ? Promise.reject({ message: 'HTTP 503' }) : outcome });
    await act(async () => findButton(view.container, '从原网页重新获取').click()); await flush(3);
    assert.ok(recovery(view.container));
    assert.match(view.container.querySelector('[role="status"]').textContent, message);
    assert.equal(getBodyEntry('101').content, ''); assert.equal(getBodyEntry('101').fulltextExtracted, false);
    assert.equal(useAppStore.getState().entries[0].bodyMissing, true);
    assert.ok(!useAppStore.getState().toasts.some(toast => /正文已获取|全文提取完成/.test(toast.text)));
    assert.equal(findButton(view.container, '从原网页重新获取').disabled, false);
    setPlan({ extract_fulltext: () => success() });
    await act(async () => findButton(view.container, '从原网页重新获取').click()); await flush(3);
    assert.equal(invokeCount('extract_fulltext'), 2);
    assert.match(view.container.querySelector('.article-prose').textContent, /从原网页获得的正文/);
  });
}

recoveryTest('恢复离屏完成/错误中心重试：只更新失败原ID，不抢另一篇阅读器或发起读藏', async gate => {
  const request = gate();
  const second = missingRow({ id: 102, title: '当前另一篇', content_id: 302, content_revision: 1,
    body_missing: false, content_html: '<p>第二篇正文</p>' });
  const view = await open([missingRow(), second]);
  setPlan({ extract_fulltext: () => request.promise });
  await act(async () => findButton(view.container, '从原网页重新获取').click());
  await act(async () => useAppStore.getState().selectArticle('102'));
  await act(async () => request.reject({ message: '离屏失败' })); await flush(3);
  const retry = useAppStore.getState().errors.find(error => error.key === 'reader:fulltext:101').action.run;
  assert.ok(!view.container.querySelector('[role="status"]'), '第一篇错误不污染第二篇正文');
  setPlan({ extract_fulltext: () => success() });
  await act(async () => retry()); await flush(3);
  assert.deepEqual(extractCalls().map(call => call.args.articleId), [101, 101]);
  assert.equal(getBodyEntry('101').contentId, 501);
  assert.equal(useAppStore.getState().activeArticleId, '102'); assert.equal(useAppStore.getState().showFulltext, false);
  assert.match(view.container.querySelector('.article-prose').textContent, /第二篇正文/);
  assert.equal(invokeCount('set_read') + invokeCount('set_starred'), 0);
});

for (const outcome of ['success', 'reject']) recoveryTest(`恢复与修订竞争/${outcome}：失效的结果与错误均不复活正文`, async gate => {
  const request = gate();
  const view = await open();
  setPlan({ extract_fulltext: () => request.promise });
  await act(async () => findButton(view.container, '从原网页重新获取').click());
  const oldStamp = getBodyEntry('101').contentRevision;
  const changed = missingRow({ content_revision: 9 }); setCurrentRows([changed]);
  await act(async () => useAppStore.getState().reloadFromBackend()); await flush(3);
  assert.deepEqual(useAppStore.getState().readingSession.memberIds, ['101'], '竞争前提：目标确属本次会话成员');
  assert.equal(getBodyEntry('101').contentRev, 9, '竞争前提：后端新修订已被接受');
  assert.notEqual(getBodyEntry('101').contentRevision, oldStamp, '竞争前提：旧全文请求的正文戳已失效');
  const notices = useAppStore.getState().toasts.map(toast => toast.text);
  await act(async () => outcome === 'success' ? request.resolve(success()) : request.reject({ message: '过期错误' })); await flush(3);
  assert.ok(recovery(view.container)); assert.equal(getBodyEntry('101').content, '');
  assert.equal(useAppStore.getState().errors.length, 0);
  assert.deepEqual(useAppStore.getState().toasts.map(toast => toast.text), notices);
  assert.equal(findButton(view.container, '从原网页重新获取').disabled, false);
});

recoveryTest('恢复失败后正文换代：旧错误重试闭包不再发起提取', async () => {
  const view = await open(); setPlan({ extract_fulltext: () => Promise.reject({ message: '旧失败' }) });
  await act(async () => findButton(view.container, '从原网页重新获取').click()); await flush(3);
  const retry = useAppStore.getState().errors.find(error => error.key === 'reader:fulltext:101').action.run;
  const oldStamp = getBodyEntry('101').contentRevision;
  setCurrentRows([missingRow({ content_revision: 4 })]);
  await act(async () => useAppStore.getState().reloadFromBackend()); await flush(3);
  assert.deepEqual(useAppStore.getState().readingSession.memberIds, ['101']);
  assert.equal(getBodyEntry('101').contentRev, 4);
  assert.notEqual(getBodyEntry('101').contentRevision, oldStamp, '旧重试调用前必须先真正接受新的正文修订');
  await act(async () => retry());
  assert.equal(invokeCount('extract_fulltext'), 1);
  assert.ok(!view.container.querySelector('[role="status"]'));
});

recoveryTest('LRU淘汰后原ID离屏重试：重新核对本机身份，仍能明确恢复且不抢选中或标读', async () => {
  const { error } = await evictFailedTarget();
  setPlan({ extract_fulltext: () => success() });
  await act(async () => useAppStore.getState().retryError(error.id)); await flush(5);
  assert.deepEqual(extractCalls().map(call => call.args.articleId), [101, 101]);
  assert.equal(getBodyEntry('101').content, success().html); assert.equal(getBodyEntry('101').contentId, 501);
  assert.equal(useAppStore.getState().activeArticleId, null);
  assert.equal(invokeCount('set_read') + invokeCount('set_read_bulk') + invokeCount('set_starred'), 0);
});

for (const missing of [false, true]) recoveryTest(`LRU回读${missing ? '缺行' : '失败'}：保留真实错误重试入口，随后命中仍能按原ID获取`, async () => {
  const { target, error } = await evictFailedTarget();
  setPlan({ get_article: () => missing ? null : Promise.reject({ message: '本机读取失败' }), extract_fulltext: () => success() });
  await act(async () => useAppStore.getState().retryError(error.id)); await flush(4);
  assert.equal(invokeCount('extract_fulltext'), 1);
  assert.equal(getBodyEntry('101').state, missing ? 'missing' : 'failed');
  const next = useAppStore.getState().errors.find(item => item.key === 'reader:fulltext:101');
  assert.ok(next, '读取未确认时不能消费入口后静默停止');
  assert.match(next.text, missing ? /文章不存在/ : /本机读取失败/);
  setPlan({ get_article: () => target });
  await act(async () => useAppStore.getState().retryError(next.id)); await flush(5);
  assert.equal(invokeCount('extract_fulltext'), 2);
  assert.equal(getBodyEntry('101').content, success().html); assert.equal(useAppStore.getState().activeArticleId, null);
});

for (const changed of ['body', 'url']) recoveryTest(`LRU回读发现${changed}已变化：旧错误不覆盖当前实体`, async () => {
  const { target, error } = await evictFailedTarget();
  const current = changed === 'body' ? { ...target, content_id: 909, content_revision: 5,
    content_html: '<p>后台新正文</p>', body_missing: false } : { ...target, url: 'https://example.com/changed-address' };
  setPlan({ get_article: () => current, extract_fulltext: () => success() });
  await act(async () => useAppStore.getState().retryError(error.id)); await flush(5);
  assert.equal(invokeCount('extract_fulltext'), 1, '只有回读身份与原失败目标相同才可继续提取');
  assert.equal(getBodyEntry('101').content, current.content_html ?? '');
  assert.ok(useAppStore.getState().toasts.some(toast => toast.text.includes('条目已更新')));
});

recoveryTest('LRU离屏回读在途时成功换库：旧详情不能重建正文或嵌套提取', async gate => {
  const { target, error } = await evictFailedTarget();
  const detail = gate(); setPlan({ get_article: () => detail.promise, extract_fulltext: () => success() });
  await act(async () => useAppStore.getState().retryError(error.id)); await flush(2);
  assert.equal(getBodyEntry('101').state, 'loading');
  const info = { path: 'D:/fixture/lru-body.flxb', size_bytes: 1, digest: 'b'.repeat(64), sources: 1, articles: 1, starred: 0, missing_bodies: 0 };
  setPlan({ restore_portable_backup: () => info }); setCurrentRows([mkRow({ id: 102, title: '新库文章' })]);
  await act(async () => {
    useAppStore.setState({ portableBackup: { busy: null, preview: { path: info.path, info }, result: null, error: null, refreshFailed: false } });
    await useAppStore.getState().restorePortableBackup();
  });
  assert.equal(useAppStore.getState().libraryGeneration, 1); assert.equal(useAppStore.getState().portableBackup.refreshFailed, false);
  const notices = useAppStore.getState().toasts.map(toast => toast.text);
  await act(async () => detail.resolve(target)); await flush(3);
  assert.equal(getBodyEntry('101'), undefined); assert.equal(invokeCount('extract_fulltext'), 1);
  assert.deepEqual(useAppStore.getState().toasts.map(toast => toast.text), notices);
  assert.equal(useAppStore.getState().activeArticleId, null);
});

for (const outcome of ['success', 'failure', 'retry']) recoveryTest(`LRU回读同戳被普通详情接管/${outcome}：旧成功、失败和已捕获重试均不再拥有正文`, async gate => {
  const { target, error, rows } = await evictFailedTarget();
  const detail = gate(); const unexpectedExtract = gate();
  const fresh = { ...target, content_id: 909, content_revision: 5, content_html: '<p>重开先接受的新正文</p>', body_missing: false };
  let reads = 0;
  await act(async () => seedFromRows([target, ...rows]));
  setPlan({ get_article: () => ++reads === 1 ? detail.promise : fresh,
    extract_fulltext: () => unexpectedExtract.promise });
  await act(async () => useAppStore.getState().retryError(error.id)); await flush(2);
  assert.equal(reads, 1); assert.equal(getBodyEntry('101').state, 'loading');
  const stamp = getBodyEntry('101').contentRevision;
  let capturedRetry;
  if (outcome === 'retry') {
    await act(async () => detail.reject({ message: '本次回读失败' })); await flush(3);
    capturedRetry = useAppStore.getState().errors.find(item => item.key === 'reader:fulltext:101').action.run;
    assert.equal(getBodyEntry('101').state, 'failed');
  }
  setCurrentRows([fresh, ...rows]);
  const view = await mount(createElement(Reader));
  await act(async () => useAppStore.getState().selectArticle('101')); await flush(3);
  assert.equal(reads, 2, '普通重开详情确已另行发出并接受');
  assert.equal(getBodyEntry('101').contentRevision, stamp, '前置：普通详情接受复用同一正文戳');
  assert.equal(getBodyEntry('101').contentId, 909); assert.equal(getBodyEntry('101').contentRev, 5);
  assert.match(view.container.querySelector('.article-prose').textContent, /重开先接受的新正文/);
  const errors = useAppStore.getState().errors.map(item => item.id);
  const notices = useAppStore.getState().toasts.map(toast => toast.text);
  await act(async () => {
    if (outcome === 'success') detail.resolve(target);
    else if (outcome === 'failure') detail.reject({ message: '旧回读迟到失败' });
    else capturedRetry();
  }); await flush(3);
  assert.equal(getBodyEntry('101').state, 'ready'); assert.equal(getBodyEntry('101').contentId, 909);
  assert.equal(getBodyEntry('101').contentRev, 5); assert.equal(getBodyEntry('101').content, fresh.content_html);
  assert.equal(reads, 2, '已捕获旧重试也不能删除接管后的记录再发详情');
  assert.equal(invokeCount('extract_fulltext'), 1, '已接管后不继续旧恢复');
  assert.deepEqual(useAppStore.getState().errors.map(item => item.id), errors);
  assert.deepEqual(useAppStore.getState().toasts.map(toast => toast.text), notices);
  assert.match(view.container.querySelector('.article-prose').textContent, /重开先接受的新正文/);
});

for (const outcome of ['success', 'failure', 'retry']) recoveryTest(`全文命令同戳被新目标接管/${outcome}：旧结果与错误重试共享目标资格`, async gate => {
  const original = missingRow(); const view = await open([original]);
  const extraction = gate(); let failure;
  setPlan({ extract_fulltext: () => extraction.promise });
  await act(async () => useAppStore.getState().extractCurrentArticle()); await flush(2);
  const stamp = getBodyEntry('101').contentRevision;
  if (outcome === 'retry') {
    await act(async () => extraction.reject({ message: '原始提取失败' })); await flush(3);
    failure = useAppStore.getState().errors.find(item => item.key === 'reader:fulltext:101');
    assert.ok(failure);
  }
  const fresh = { ...original, content_id: 909, content_revision: 5, body_missing: false, content_html: '<p>命令期间重开取得的新正文</p>' };
  setCurrentRows([fresh]);
  await act(async () => useAppStore.getState().selectArticle('101')); await flush(3);
  assert.equal(getBodyEntry('101').contentRevision, stamp, '前置：普通详情实际沿用正文戳');
  assert.equal(getBodyEntry('101').contentId, 909);
  assert.match(view.container.querySelector('.article-prose').textContent, /命令期间重开取得的新正文/);
  const notices = useAppStore.getState().toasts.map(item => item.text);
  await act(async () => {
    if (outcome === 'success') extraction.resolve(success());
    else if (outcome === 'failure') extraction.reject({ message: '旧提取失败迟到' });
    else useAppStore.getState().retryError(failure.id);
  }); await flush(3);
  assert.equal(getBodyEntry('101').contentId, 909); assert.equal(getBodyEntry('101').contentRev, 5);
  assert.equal(getBodyEntry('101').content, fresh.content_html);
  assert.equal(getBodyEntry('101').fulltextRequest, undefined); assert.equal(getBodyEntry('101').fulltextFailure, undefined);
  assert.equal(invokeCount('extract_fulltext'), 1);
  assert.deepEqual(useAppStore.getState().toasts.map(item => item.text), notices);
  if (outcome !== 'retry') assert.equal(useAppStore.getState().errors.length, 0);
});

for (const change of ['same-detail', 'media-only']) for (const outcome of ['success', 'retry']) {
  recoveryTest(`合法全文命令/${change}/${outcome}：同目标详情及独立媒体更新不误拒成功或失败后重试`, async gate => {
    const original = missingRow(); const view = await open([original]);
    await act(async () => useAppStore.getState().reloadFromBackend());
    const extraction = gate();
    setPlan({ extract_fulltext: () => extraction.promise });
    await act(async () => useAppStore.getState().extractCurrentArticle()); await flush(2);
    const stamp = getBodyEntry('101').contentRevision;
    const ticket = getBodyEntry('101').fulltextRequest;
    let failure;
    if (outcome === 'retry') {
      await act(async () => extraction.reject({ message: '可继续的提取失败' })); await flush(3);
      failure = useAppStore.getState().errors.find(item => item.key === 'reader:fulltext:101');
      assert.ok(failure);
    }
    const fresh = change === 'media-only' ? { ...original, media_revision: 9,
      image_url: 'https://images.example/current.png', enclosure_url: 'https://audio.example/current.mp3', duration_sec: 20 } : original;
    setCurrentRows([fresh]);
    const before = getBodyEntry('101');
    await act(async () => change === 'same-detail'
      ? useAppStore.getState().selectArticle('101') : useAppStore.getState().reloadFromBackend()); await flush(3);
    if (change === 'same-detail') assert.notEqual(getBodyEntry('101'), before, '普通详情确实写入新对象');
    else assert.equal(useAppStore.getState().entries[0].mediaRevision, 9, '媒体版本已真实被接受');
    assert.equal(getBodyEntry('101').contentRevision, stamp);
    if (outcome === 'success') assert.equal(getBodyEntry('101').fulltextRequest, ticket);
    else assert.equal(getBodyEntry('101').fulltextFailure.ticket, ticket);
    const result = success();
    setCurrentRows([{ ...fresh, content_id: result.contentId, content_revision: result.contentRevision,
      content_html: result.html, body_missing: false, fulltext_extracted: true }]);
    if (outcome === 'retry') setPlan({ extract_fulltext: () => result });
    await act(async () => outcome === 'success' ? extraction.resolve(result) : useAppStore.getState().retryError(failure.id));
    await flush(4);
    assert.equal(getBodyEntry('101').contentId, result.contentId); assert.equal(getBodyEntry('101').content, result.html);
    assert.match(view.container.querySelector('.article-prose').textContent, /从原网页获得的正文/);
    assert.equal(invokeCount('extract_fulltext'), outcome === 'success' ? 1 : 2);
    assert.equal(getBodyEntry('101').fulltextReadback, undefined, '媒体回读完成后释放轻owner');
    if (change === 'media-only') assert.equal(useAppStore.getState().entries[0].mediaRevision, 9);
  });
}

for (const change of ['body', 'url', 'reload-url']) recoveryTest(`已知${change}接管后再LRU：原失败凭据不因缺缓存重新有效`, async () => {
  const original = missingRow(); await open([original]);
  setPlan({ extract_fulltext: () => Promise.reject({ message: '旧目标失败' }) });
  await act(async () => useAppStore.getState().extractCurrentArticle()); await flush(3);
  const error = useAppStore.getState().errors.find(item => item.key === 'reader:fulltext:101');
  assert.ok(error);
  const stamp = getBodyEntry('101').contentRevision;
  const fresh = change === 'body' ? { ...original, content_id: 909, content_revision: 5, body_missing: false,
    content_html: '<p>已接管的新正文</p>' } : { ...original, url: 'https://example.com/new-target' };
  setCurrentRows([fresh]);
  await act(async () => change === 'reload-url'
    ? useAppStore.getState().reloadFromBackend() : useAppStore.getState().selectArticle('101')); await flush(3);
  assert.equal(getBodyEntry('101').contentRevision, stamp);
  assert.equal(getBodyEntry('101').fulltextFailure, undefined, '已接受的实际详情撤销旧失败凭据');
  await act(async () => useAppStore.getState().clearReaderSelection()); await unmountAll();
  const rows = Array.from({ length: bodyCacheMod.BODY_CACHE_MAX }, (_, index) => mkRow({ id: 1000 + index,
    content_id: 10000 + index, content_revision: 1, content_html: '<p>填充LRU</p>' }));
  await act(async () => {
    seedFromRows(rows); setCurrentRows([original, ...rows]);
    useAppStore.getState().hydrateArticleContent(rows.map(row => String(row.id)));
  }); await flush(3);
  assert.equal(getBodyEntry('101'), undefined);
  const details = invokeCount('get_article');
  await act(async () => useAppStore.getState().retryError(error.id)); await flush(3);
  assert.equal(invokeCount('get_article'), details, '已知接管不同于未知LRU，不回读旧目标');
  assert.equal(invokeCount('extract_fulltext'), 1); assert.equal(useAppStore.getState().activeArticleId, null);
});

for (const supplied of [false, true]) recoveryTest(`已接受轻属性URL/${supplied ? '明确null' : '缺省'}：仅明确位置事实撤销失败凭据`, async () => {
  const original = missingRow(); await open([original]);
  setPlan({ extract_fulltext: () => Promise.reject({ message: '位置边界失败' }) });
  await act(async () => useAppStore.getState().extractCurrentArticle()); await flush(3);
  const error = useAppStore.getState().errors.find(item => item.key === 'reader:fulltext:101');
  assert.ok(error);
  const row = { ...original };
  if (supplied) row.url = null;
  else delete row.url;
  setCurrentRows([row]);
  await act(async () => useAppStore.getState().reloadFromBackend()); await flush(3);
  assert.equal(useAppStore.getState().entries[0].urlKnown, supplied);
  assert.equal(useAppStore.getState().entries[0].url, original.url, '不改变原有URL展示继承');
  if (supplied) assert.equal(getBodyEntry('101').fulltextFailure, undefined);
  else assert.ok(getBodyEntry('101').fulltextFailure, '本次缺省URL不是已确认无URL');
  setPlan({ extract_fulltext: () => success() });
  await act(async () => useAppStore.getState().retryError(error.id)); await flush(3);
  assert.equal(invokeCount('extract_fulltext'), supplied ? 1 : 2);
});

recoveryTest('提交后的媒体回读遇同戳新正文接管：旧媒体只释放自己的owner，不改当前实体', async gate => {
  const original = missingRow(); await open([original]);
  const metadata = gate(); const result = success();
  setPlan({ extract_fulltext: () => result, get_article: () => metadata.promise });
  await act(async () => useAppStore.getState().extractCurrentArticle()); await flush(2);
  assert.ok(getBodyEntry('101').fulltextReadback);
  const stamp = getBodyEntry('101').contentRevision;
  const fresh = { ...original, content_id: 909, content_revision: 5, content_html: '<p>媒体回读期间的新正文</p>',
    body_missing: false, image_url: 'https://images.example/latest.png', media_revision: 9 };
  setPlan({ get_article: () => fresh });
  await act(async () => useAppStore.getState().selectArticle('101')); await flush(3);
  assert.equal(getBodyEntry('101').contentRevision, stamp); assert.equal(getBodyEntry('101').contentId, 909);
  const mediaBefore = useAppStore.getState().entries[0].cover;
  await act(async () => metadata.resolve({ ...original, content_id: 501, content_revision: 2,
    content_html: result.html, image_url: 'https://images.example/stale.png', media_revision: 20 })); await flush(3);
  assert.equal(getBodyEntry('101').contentId, 909); assert.equal(getBodyEntry('101').content, fresh.content_html);
  assert.equal(useAppStore.getState().entries[0].cover, mediaBefore);
  assert.equal(getBodyEntry('101').fulltextReadback, undefined);
});

recoveryTest('未读会话先标读再更新正文：既定成员继续回读，失效的恢复结果不覆盖后台新正文', async gate => {
  const row = missingRow({ is_read: false });
  useAppStore.setState({ timelineFilter: 'unread' });
  const view = await open([row]);
  await act(async () => useAppStore.getState().reloadFromBackend());
  assert.deepEqual(useAppStore.getState().readingSession.memberIds, ['101']);
  const session = useAppStore.getState().readingSession;
  setPlan({ set_read: () => { setCurrentRows([{ ...row, is_read: true }]); return null; } });
  await act(async () => useAppStore.getState().toggleCurrentReadStatus()); await flush(3);
  assert.equal(useAppStore.getState().entries[0].isRead, true);
  const request = gate(); setPlan({ extract_fulltext: () => request.promise });
  await act(async () => findButton(view.container, '从原网页重新获取').click());
  const oldStamp = getBodyEntry('101').contentRevision;
  setCurrentRows([{ ...row, is_read: true, body_missing: false, content_id: 909,
    content_revision: 1, content_html: '<p>后台补回的新正文</p>' }]);
  const memberReads = invokeCount('list_article_member_ids');
  await act(async () => useAppStore.getState().reloadFromBackend()); await flush(3);
  assert.equal(invokeCount('list_article_member_ids'), memberReads, '同会话不得重新按当前未读属性筛成员');
  assert.equal(useAppStore.getState().readingSession.generation, session.generation);
  assert.deepEqual(useAppStore.getState().readingSession.memberIds, ['101']);
  assert.notEqual(getBodyEntry('101').contentRevision, oldStamp);
  assert.equal(getBodyEntry('101').contentId, 909);
  await act(async () => request.resolve(success())); await flush(3);
  assert.match(view.container.querySelector('.article-prose').textContent, /后台补回的新正文/);
  assert.equal(getBodyEntry('101').contentId, 909);
  assert.equal(invokeCount('set_read'), 1, '仅显式标读执行一次，恢复不新增标读');
});

recoveryTest('恢复已提交后旧reload晚到：正文持续可读，不退回空白或追加水合', async gate => {
  const row = missingRow(); const view = await open([row]);
  await act(async () => useAppStore.getState().reloadFromBackend());
  assert.deepEqual(useAppStore.getState().readingSession.memberIds, ['101']);
  const oldReload = gate(); const futureHydration = gate(); let pendingReload;
  setPlan({ get_article_updates: () => oldReload.promise });
  const updateCalls = invokeCount('get_article_updates');
  await act(async () => { pendingReload = useAppStore.getState().reloadFromBackend(); });
  await waitFor(() => invokeCount('get_article_updates') === updateCalls + 1, { label: '旧属性查询已发出' });
  const result = success();
  setCurrentRows([{ ...row, content_id: result.contentId, content_revision: result.contentRevision,
    content_html: result.html, fulltext_extracted: true, body_missing: false }]);
  setPlan({ extract_fulltext: () => result });
  await act(async () => findButton(view.container, '从原网页重新获取').click()); await flush(3);
  const stamp = getBodyEntry('101').contentRevision;
  assert.equal(getBodyEntry('101').contentId, 501);
  assert.match(view.container.querySelector('.article-prose').textContent, /从原网页获得的正文/);
  const hydrationCalls = invokeCount('get_articles');
  setPlan({ get_articles: () => futureHydration.promise });
  await act(async () => { oldReload.resolve({ items: [row], missing_ids: [] }); await pendingReload; }); await flush(3);
  assert.equal(getBodyEntry('101').content, result.html);
  assert.equal(getBodyEntry('101').contentRevision, stamp);
  assert.equal(invokeCount('get_articles'), hydrationCalls, '旧reload不得误清后再补取掩盖过程');
  assert.match(view.container.querySelector('.article-prose').textContent, /从原网页获得的正文/);
});

recoveryTest('恢复媒体回读迟到：独立媒体修订不倒退，原文URL不受媒体版本支配', async gate => {
  const row = missingRow(); await open([row]);
  await act(async () => useAppStore.getState().reloadFromBackend());
  const metadata = gate(); const result = success();
  const old = { ...row, content_id: result.contentId, content_revision: result.contentRevision,
    content_html: result.html, body_missing: false, fulltext_extracted: true, media_revision: 8,
    image_url: 'https://images.example/old.png', gallery_first_image_url: 'https://images.example/gallery-old.png',
    image_count: 1, enclosure_url: 'https://audio.example/old.mp3', duration_sec: 10 };
  setCurrentRows([old]); setPlan({ extract_fulltext: () => result, get_article: () => metadata.promise });
  await act(async () => useAppStore.getState().extractCurrentArticle()); await flush(3);
  const stamp = getBodyEntry('101').contentRevision;
  const current = { ...old, media_revision: 9, url: 'https://example.com/updated-url',
    image_url: 'https://images.example/new.png', gallery_first_image_url: 'https://images.example/gallery-new.png',
    image_count: 2, enclosure_url: 'https://audio.example/new.mp3', duration_sec: 20 };
  setCurrentRows([current]);
  await act(async () => useAppStore.getState().reloadFromBackend()); await flush(3);
  assert.equal(useAppStore.getState().entries[0].mediaRevision, 9, '前置：较新媒体版本已真实摄入');
  assert.equal(getBodyEntry('101').contentRevision, stamp, '前置：独立媒体更新不推进正文戳');
  await act(async () => metadata.resolve(old)); await flush(3);
  const entry = useAppStore.getState().entries[0];
  assert.equal(entry.mediaRevision, 9); assert.equal(entry.cover, current.image_url);
  assert.equal(entry.imageUrl, current.gallery_first_image_url); assert.equal(entry.imageCount, 2);
  assert.equal(entry.audioUrl, current.enclosure_url); assert.equal(entry.enclosureUrl, current.enclosure_url);
  assert.equal(entry.durationSec, 20); assert.equal(entry.url, current.url);
  assert.equal(getBodyEntry('101').contentRevision, stamp);
});

recoveryTest('恢复期间资料库替换：busy拒绝新提取，成功恢复后旧成功/错误重试与旧媒体请求全部失效', async gate => {
  const view = await open();
  setPlan({ extract_fulltext: () => Promise.reject({ message: '先前失败' }) });
  await act(async () => findButton(view.container, '从原网页重新获取').click()); await flush(3);
  const oldRetry = useAppStore.getState().errors.find(error => error.key === 'reader:fulltext:101').action.run;
  const extract = gate(); const restore = gate();
  setPlan({ extract_fulltext: () => extract.promise, restore_portable_backup: () => restore.promise });
  await act(async () => oldRetry());
  const info = { path: 'D:/fixture/body.flxb', size_bytes: 1, digest: 'a'.repeat(64), sources: 1, articles: 1, starred: 0, missing_bodies: 0 };
  let task;
  await act(async () => {
    useAppStore.setState({ portableBackup: { busy: null, preview: { path: info.path, info }, result: null, error: null, refreshFailed: false } });
    task = useAppStore.getState().restorePortableBackup();
  }); await flush(3);
  assert.equal(useAppStore.getState().libraryRestoreBusy, true);
  const before = invokeCount('extract_fulltext');
  await act(async () => useAppStore.getState().extractCurrentArticle()); assert.equal(invokeCount('extract_fulltext'), before);
  setCurrentRows([mkRow({ id: 102, title: '新资料库正文', content_html: '<p>新库</p>' })]);
  await act(async () => { restore.resolve(info); await task; }); await flush(3);
  assert.equal(useAppStore.getState().portableBackup.refreshFailed, false);
  const notices = useAppStore.getState().toasts.map(toast => toast.text);
  await act(async () => { extract.resolve(success({ html: '<p>旧库迟到</p><img src="https://cdnfile.sspai.com/stale.png">' })); oldRetry(); });
  await flush(3);
  assert.equal(getBodyEntry('101'), undefined); assert.equal(invokeCount('extract_fulltext'), before);
  assert.equal(invokeCount('fetch_image'), 0); assert.deepEqual(useAppStore.getState().errors, []);
  assert.deepEqual(useAppStore.getState().toasts.map(toast => toast.text), notices);
});

recoveryTest('恢复正文媒体：切篇取消旧图片，迟到字节不挂新篇；源码与未净化流式译文零取图', async gate => {
  const imageRequest = gate();
  const image = 'https://cdnfile.sspai.com/recovered-image.png';
  const view = await open([missingRow(), missingRow({ id: 102, title: '第二篇' })]);
  setPlan({ extract_fulltext: () => success({ html: `<p>已恢复</p><img src="${image}">` }),
    fetch_image: args => { args.started.onmessage({ registered: true }); return imageRequest.promise; } });
  await act(async () => findButton(view.container, '从原网页重新获取').click()); await flush(4);
  assert.equal(invokeCount('fetch_image'), 1);
  await act(async () => useAppStore.getState().selectArticle('102')); await flush(3);
  assert.equal(invokeCount('cancel_image'), 1); assert.equal(media.imageResourceStats().leases, 0);
  await act(async () => imageRequest.resolve(png())); await flush(3);
  assert.ok(!view.container.querySelector('.article-prose img'));
  const source = success({ html: '<p>源码恢复</p><img src="https://cdnfile.sspai.com/source-only.png">' });
  setPlan({ extract_fulltext: () => source });
  await act(async () => useAppStore.setState({ isRawRenderMode: true }));
  await act(async () => findButton(view.container, '从原网页重新获取').click()); await flush(3);
  assert.match(view.container.querySelector('.reader-source-view').textContent, /source-only/);
  assert.equal(invokeCount('fetch_image'), 1);
  await act(async () => {
    bodyCacheMod.applyAiProduct('102', 'translatedContent', '<img src="https://cdnfile.sspai.com/stream-only.png">');
    useAppStore.setState({ isRawRenderMode: false, isShowingTranslatedProse: true, rawTranslatedIds: { '102': true } });
  }); await flush(3);
  assert.equal(invokeCount('fetch_image'), 1); assert.ok(!view.container.querySelector('.article-prose img'));
});
