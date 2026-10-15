// OPT-013 B1：AI 产物缓存签名——「未标记版本」标记与命中判定的真实挂载回归。
//
// 取证方式（与 OPT-016B 同纪律）：挂载**真实 Reader 组件**，数据经真实 store 与
// 真实 IPC 假后端（fixtures.mjs）路径：
// - 产物非空且无签名（存量行）→ 摘要卡头与译文工具栏出现「未标记版本」标记；
//   产物带签名 → 不出现（标记不是常显装饰）；
// - 命中判定以后端 ai_cache_check 为准：未命中（旧签名/无签名/输入变化）时
//   真实点击「摘要」/「翻译」必须发起生成请求（不以「产物非空」短路）；
//   命中时同一个入口不得重复计费。
// 未预期 console/jsdom/React 错误由 componentTest 统一账本终检判红。

import assert from 'node:assert/strict';
import { componentTest, mount, flush, waitFor, resetStore, getBodyEntry, useAppStore } from './harness.mjs';
import { installBackend, seedFromRows, mkRow, clearInvokeCalls, invokeCount, setPlan, resetPlan, deferred } from './fixtures.mjs';

const UNSIGNED_ROW = mkRow({
  id: 101, title: '未标记版本文章', content_html: '<p>正文</p>',
  ai_summary: '存量摘要（无签名）',
  translated_content: '<p>存量译文（无签名）</p>',
  // ai_summary_sig/translated_sig 缺省 = null（存量无签名产物）
});

const SIGNED_ROW = mkRow({
  id: 102, title: '已验证版本文章', content_html: '<p>正文</p>',
  ai_summary: '已验证摘要', ai_summary_sig: 'sig-summary-1',
  translated_content: '<p>已验证译文</p>', translated_sig: 'sig-translation-1',
});

componentTest('OPT-013 B1：无签名产物呈现「未标记版本」标记，带签名产物不呈现', { timeout: 30000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows([UNSIGNED_ROW, SIGNED_ROW]);

  const { createElement, act } = await import('react');
  const { Reader } = await import('../../src/components/Reader.tsx');
  const entry = await mount(createElement(Reader));

  /* ---- 无签名产物：摘要卡头 + 译文工具栏都有「未标记版本」 ---- */
  await act(async () => { useAppStore.getState().selectArticle('101'); });
  await waitFor(() => getBodyEntry('101')?.state === 'ready', { label: '101 水合完成' });
  await act(async () => { useAppStore.setState({ isShowingTranslatedProse: true }); });
  await flush(2);

  const headTag = entry.container.querySelector('.ai-box-head .ai-unverified-tag');
  assert.ok(headTag, '无签名摘要应出现「未标记版本」标记（数据可显示、不冒充已验证缓存）');
  assert.equal(headTag.textContent, '未标记版本');
  const toolbarTag = entry.container.querySelector('.reader-actions-left .ai-unverified-tag');
  assert.ok(toolbarTag, '无签名译文展示时应出现「未标记版本」标记');
  assert.equal(toolbarTag.textContent, '未标记版本');
  /* 产物本体仍可见（保留数据） */
  assert.equal(entry.container.querySelector('.ai-body-content')?.textContent, '存量摘要（无签名）',
    '未标记版本不隐藏数据（摘要照常显示）');

  /* ---- 带签名产物：同一挂载树不出现标记 ---- */
  await act(async () => { useAppStore.getState().selectArticle('102'); });
  await waitFor(() => getBodyEntry('102')?.state === 'ready', { label: '102 水合完成' });
  await act(async () => { useAppStore.setState({ isShowingTranslatedProse: true }); });
  await flush(2);
  assert.equal(entry.container.querySelector('.ai-unverified-tag'), null,
    '带签名产物不得出现「未标记版本」标记（标记不是常显装饰）');
});

componentTest('OPT-013 B1：未命中不短路（真实点击发起生成），命中不重复计费', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows([UNSIGNED_ROW]);

  const { createElement, act } = await import('react');
  const { Reader } = await import('../../src/components/Reader.tsx');
  const entry = await mount(createElement(Reader));

  await act(async () => { useAppStore.getState().selectArticle('101'); });
  await waitFor(() => getBodyEntry('101')?.state === 'ready', { label: '101 水合完成' });
  await flush(2);

  /* 后端判定「未命中」（旧签名/无签名/输入变化）——生成请求由用例挂起观察 */
  const held = [];
  /* 提交契约忠实模拟：生成成功后 DB 行即含新产物与签名（真实后端在 Done 前
     已提交；回读行必须反映它，否则是假后端不忠实）。 */
  const committedSummaryRow = mkRow({
    id: 101, title: '未标记版本文章', content_html: '<p>正文</p>',
    ai_summary: '重新生成的摘要', ai_summary_sig: 'sig-summary-new',
    translated_content: '<p>存量译文（无签名）</p>',
  });
  setPlan({
    ai_cache_check: () => Promise.resolve({ summaryVerified: false, translationVerified: false }),
    get_article: () => Promise.resolve(committedSummaryRow),
    ai_summarize: (args) => new Promise((resolve) => { held.push({ cmd: 'ai_summarize', args, resolve }); }),
    ai_translate: (args) => new Promise((resolve) => { held.push({ cmd: 'ai_translate', args, resolve }); }),
  });
  try {
    /* ---- 真实点击「摘要」按钮：旧产物不得短路，必须发起 ai_summarize ---- */
    const summaryBtn = [...entry.container.querySelectorAll('.reader-actions-left button')]
      .find((b) => b.textContent.includes('摘要'));
    assert.ok(summaryBtn, 'Reader 工具栏应有摘要按钮');
    await act(async () => { summaryBtn.click(); });
    await waitFor(() => held.some((h) => h.cmd === 'ai_summarize'), { label: '未命中后发出摘要请求' });
    assert.equal(held.filter((h) => h.cmd === 'ai_summarize').length, 1,
      '无签名/旧签名产物不得被前端非空短路——真实点击必须发起生成');
    /* 收尾该笔：流式 + done + resolve（读回缺行不清刚生成的摘要） */
    await act(async () => {
      const h = held.find((x) => x.cmd === 'ai_summarize');
      h.args.onChannel?.onmessage?.({ type: 'delta', data: '重新生成的摘要' });
      h.args.onChannel?.onmessage?.({ type: 'done' });
      h.resolve('');
      await Promise.resolve();
    });
    await flush(4);
    assert.equal(getBodyEntry('101')?.aiSummary, '重新生成的摘要',
      '重新生成落地（Done 后安全回读 DB 真值——含新签名）');
    assert.equal(getBodyEntry('101')?.aiSummarySig, 'sig-summary-new',
      '回读行的缓存签名与产物成对落账（未标记版本标记随新签名消失）');

    /* ---- 同产物再触发（后端判定命中）：不得重复计费 ---- */
    const summaryCalls = invokeCount('ai_summarize');
    setPlan({ ai_cache_check: () => Promise.resolve({ summaryVerified: true, translationVerified: true }) });
    await act(async () => { useAppStore.getState().triggerReaderSummary(); });
    await flush(4);
    assert.equal(invokeCount('ai_summarize'), summaryCalls,
      '后端判定命中 → 前端短路展示、不重复发起生成');

    /* ---- 翻译入口同口径：未命中 → 真实点击发起 ai_translate ---- */
    setPlan({
      ai_cache_check: () => Promise.resolve({ summaryVerified: true, translationVerified: false }),
      ai_summarize: (args) => new Promise((resolve) => { held.push({ cmd: 'ai_summarize', args, resolve }); }),
      ai_translate: (args) => new Promise((resolve) => { held.push({ cmd: 'ai_translate', args, resolve }); }),
    });
    const translateBtn = [...entry.container.querySelectorAll('.reader-actions-left button')]
      .find((b) => b.textContent.includes('翻译'));
    assert.ok(translateBtn, 'Reader 工具栏应有翻译按钮');
    await act(async () => { translateBtn.click(); });
    await waitFor(() => held.some((h) => h.cmd === 'ai_translate'), { label: '未命中后发出翻译请求' });
    assert.equal(held.filter((h) => h.cmd === 'ai_translate').length, 1,
      '无签名译文不得被前端非空短路——真实点击必须发起翻译');
    await act(async () => {
      const h = held.find((x) => x.cmd === 'ai_translate');
      h.args.onChannel?.onmessage?.({ type: 'delta', data: '<p>新</p>' });
      h.args.onChannel?.onmessage?.({ type: 'done' });
      h.resolve('');
      await Promise.resolve();
    });
    await flush(4);
    assert.ok(entry.container.querySelector('.reader-active-view'), 'Reader 挂载树保持');
  } finally {
    resetPlan();
  }
});

componentTest('OPT-013 B1：同时在途同一 (条目, 产物) 只发一次请求（自动与手动复用同一在途）', { timeout: 30000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows([UNSIGNED_ROW]);

  const { createElement, act } = await import('react');
  const { Reader } = await import('../../src/components/Reader.tsx');
  const entry = await mount(createElement(Reader));

  await act(async () => { useAppStore.getState().selectArticle('101'); });
  await waitFor(() => getBodyEntry('101')?.state === 'ready', { label: '101 水合完成' });
  await flush(2);

  const gate = deferred();
  let translateCalls = 0;
  setPlan({
    ai_cache_check: () => Promise.resolve({ summaryVerified: false, translationVerified: false }),
    ai_translate: (args) => { translateCalls += 1; return gate.promise.then(() => { args.onChannel?.onmessage?.({ type: 'done' }); return ''; }); },
  });
  try {
    /* 第一笔：手动入口（真实点击「翻译」）挂起在途 */
    const translateBtn = [...entry.container.querySelectorAll('.reader-actions-left button')]
      .find((b) => b.textContent.includes('翻译'));
    await act(async () => { translateBtn.click(); });
    await waitFor(() => translateCalls === 1, { label: '第一笔翻译发出' });

    /* 第二笔：自动触发入口（Reader 自动翻译 effect 消费的同一 action）——
       同一 (条目, 产物) 在途 → 复用，不重复计费 */
    await act(async () => { useAppStore.getState().toggleReaderTranslation({ silent: true }); });
    await flush(3);
    assert.equal(translateCalls, 1, '自动触发与手动入口复用同一在途请求（只发一次）');

    /* 放行收尾，避免悬挂 */
    await act(async () => { gate.resolve(); await Promise.resolve(); });
    await flush(3);
    assert.ok(entry.container.querySelector('.reader-active-view'), 'Reader 挂载树保持');
  } finally {
    resetPlan();
  }
});

componentTest('OPT-013 B2：整篇重新生成的最小提示真实出现在组件链路（真实点击「翻译」）', { timeout: 30000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows([UNSIGNED_ROW]);

  const { createElement, act } = await import('react');
  const { Reader } = await import('../../src/components/Reader.tsx');
  const entry = await mount(createElement(Reader));

  await act(async () => { useAppStore.getState().selectArticle('101'); });
  await waitFor(() => getBodyEntry('101')?.state === 'ready', { label: '101 水合完成' });
  await act(async () => { useAppStore.setState({ toasts: [] }); });

  /* 后端计划：旧片段因输入变化作废、当前签名无可续段（completed = 0）→ 必须
     整篇重新生成 + 告知（纯作废形态；R1 复审 P3-4：作废与可续并存走合并文案，
     见 frontend-tests 域用例）。生成请求挂起（只取证提示链，不让流式副作用混进
     断言）。 */
  setPlan({
    ai_cache_check: () => Promise.resolve({
      summaryVerified: true,
      translationVerified: false,
      translationSegments: { total: 3, completed: 0, discarded: 1 },
    }),
    ai_translate: () => new Promise(() => {}),
  });
  try {
    const translateBtn = [...entry.container.querySelectorAll('.reader-actions-left button')]
      .find((b) => b.textContent.includes('翻译'));
    assert.ok(translateBtn, 'Reader 工具栏应有翻译按钮');
    await act(async () => { translateBtn.click(); });
    await flush(6);
    const toasts = useAppStore.getState().toasts.map((t) => t.text);
    assert.ok(
      toasts.some((t) => t.includes('整篇重新翻译')),
      `真实点击「翻译」后必须出现整篇重生成提示（实际 toasts=${JSON.stringify(toasts)}）`,
    );
    assert.ok(entry.container.querySelector('.reader-active-view'), 'Reader 挂载树保持');
  } finally {
    resetPlan();
  }
});
