// OPT-011（F13）：AI 开关三态的真实挂载回归。
//
// 取证方式（与 OPT-016B 同纪律）：挂载**真实生产组件**（Reader / FeedsTab），
// 经真实 store action 与真实 IPC 假后端（tools/component-tests/fixtures.mjs）驱动：
// - Reader 自动请求 effect 订阅**有效配置**（mode + 分类默认的唯一 resolver）：
//   关 → 尚未发起的自动请求不得发起；开 → 当前打开文章触发一次；
//   已在途的请求不撤销（政策：已发送内容不可撤回，不声称可取消）；
// - FeedsTab 三态控件（继承/开启/关闭）真实下拉点击 → store 原子更新（categories
//   与 feedIndex 同一帧）+ set_feed_ai_modes IPC 落三态真值。
// 未预期 console/jsdom/React 错误由 componentTest 统一账本终检判红。

import assert from 'node:assert/strict';
import { componentTest, mount, flush, waitFor, resetStore, getBodyEntry, useAppStore } from './harness.mjs';
import { installBackend, seedFromRows, mkRow, clearInvokeCalls, invokeCount, invokeCalls, setPlan, resetPlan, deferred, FEEDS } from './fixtures.mjs';

const ROWS = [
  mkRow({ id: 101, feed_id: 10, title: 'AI 文章一', content_html: '<p>正文一</p>' }),
  mkRow({ id: 102, feed_id: 10, title: 'AI 文章二', content_html: '<p>正文二</p>' }),
];

componentTest('OPT-011 Reader：自动请求订阅有效配置（关不发起 / 开触发 / 已在途不撤销）', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows(ROWS, { layout: 'article' });

  const { createElement, act } = await import('react');
  const { Reader } = await import('../../src/components/Reader.tsx');
  const entry = await mount(createElement(Reader));

  /* ---------- 基线：分类默认关 + 源显式 off（旧行 bool=false 映射显式）----------
     打开文章水合完成后不得发起任何 AI 请求（升级不把关闭的源推向付费 AI）。 */
  await act(async () => { useAppStore.getState().selectArticle('101'); });
  await waitFor(() => getBodyEntry('101')?.state === 'ready', { label: '101 水合完成' });
  await flush(2);
  assert.equal(invokeCount('ai_summarize'), 0, '关闭态打开文章：自动摘要不得发起（关闭是唯一真值）');
  assert.equal(useAppStore.getState().feedIndex.get('10')?.feed.summaryMode, 'off',
    '夹具旧行缺 mode：按旧 bool=false 映射为显式 off（升级不开启）');

  /* ---------- 分类开 + 源显式 off：有效值仍关（显式关闭压过分类开启）---------- */
  await act(async () => { useAppStore.getState().toggleCatSummary('cat-1', true); });
  await flush(3);
  assert.equal(invokeCount('ai_summarize'), 0,
    '源显式 off 时分类切开不得触发（不以 OR 合并——否则用户无法关闭）');

  /* ---------- 源切 inherit：有效配置由关转开 → 当前打开文章自动触发一次 ---------- */
  await act(async () => { useAppStore.getState().toggleFeedSummary('cat-1', '10', 'inherit'); });
  await waitFor(() => invokeCount('ai_summarize') === 1, { label: '开启后触发自动摘要' });
  assert.equal(invokeCount('ai_summarize'), 1, '打开中的文章在有效开关转开时触发一次');

  /* ---------- 尚未发起：水合在途时关闭 → 水合完成后不得发起 ---------- */
  // 102 的正文水合手动放行；配置此刻仍为有效开（inherit + 分类开）
  const hydrateGate = deferred();
  setPlan({
    get_article: (args) => (String(args?.id) === '102'
      ? hydrateGate.promise
      : Promise.resolve(ROWS.find((r) => String(r.id) === String(args?.id)) ?? null)),
  });
  await act(async () => { useAppStore.getState().selectArticle('102'); });
  await flush(2);
  assert.notEqual(getBodyEntry('102')?.state, 'ready', '102 水合在途（场景成立）');
  // 用户关闭源开关（显式 off）：effect 订阅有效配置变化，立即重评
  await act(async () => { useAppStore.getState().toggleFeedSummary('cat-1', '10', 'off'); });
  await flush(2);
  const summaryCallsBeforeGate = invokeCount('ai_summarize');
  await act(async () => {
    hydrateGate.resolve(ROWS.find((r) => r.id === 102));
    await Promise.resolve();
  });
  await waitFor(() => getBodyEntry('102')?.state === 'ready', { label: '102 水合完成' });
  await flush(4);
  assert.equal(invokeCount('ai_summarize'), summaryCallsBeforeGate,
    '关闭后水合完成：尚未发起的自动请求必须被阻止（不得补发）');

  /* ---------- 已在途：开启 → 发起（挂起）→ 关闭：不取消、不重复发起 ---------- */
  const heldSummary = [];
  resetPlan();
  setPlan({
    ai_summarize: (args) => new Promise((resolve) => { heldSummary.push({ args, resolve }); }),
  });
  await act(async () => { useAppStore.getState().toggleFeedSummary('cat-1', '10', 'inherit'); });
  await waitFor(() => heldSummary.length === 1, { label: '开启后自动摘要发出（挂起中）' });
  const callsInFlight = invokeCount('ai_summarize');
  // 关闭（有效值转关）：在途请求不取消（政策：已发送的内容不可撤销）
  await act(async () => { useAppStore.getState().toggleFeedSummary('cat-1', '10', 'off'); });
  await flush(3);
  assert.equal(heldSummary.length, 1, '关闭不得取消/替换已在途的自动请求（同一笔仍挂起）');
  assert.equal(invokeCount('ai_summarize'), callsInFlight, '关闭后不重复发起');
  // 再切回开：正在生成（summarizingIds 置位）不得重复发起
  await act(async () => { useAppStore.getState().toggleFeedSummary('cat-1', '10', 'inherit'); });
  await flush(3);
  assert.equal(heldSummary.length, 1, '生成中的文章再次开启不重复发起（生成态守卫）');
  // 放行收尾：channel done + resolve，避免悬挂
  await act(async () => {
    heldSummary[0].args.onChannel?.onmessage?.({ type: 'done' });
    heldSummary[0].resolve('');
    await Promise.resolve();
  });
  await flush(2);

  /* 组件仍在渲染（配置切换未破坏挂载树） */
  assert.ok(entry.container.querySelector('.reader-active-view'), '配置切换后 Reader 挂载树保持');
});

componentTest('OPT-011 R2：连续两笔失败回滚到确认值——真实 Reader 不因乐观残留触发自动请求', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows(ROWS, { layout: 'article' });

  const { createElement, act } = await import('react');
  const { Reader } = await import('../../src/components/Reader.tsx');
  const entry = await mount(createElement(Reader));

  /* 受控 IPC：同键两笔挂起，由用例逐笔失败（真实 store action + 真实队列） */
  const writes = [];
  setPlan({
    set_feed_ai_modes: () => new Promise((resolve, reject) => writes.push({ resolve, reject })),
  });
  try {
    await act(async () => { useAppStore.getState().toggleFeedSummary('cat-1', '10', 'on'); });
    await flush(2);
    await act(async () => { useAppStore.getState().toggleFeedSummary('cat-1', '10', 'off'); });
    await flush(2);
    assert.equal(writes.length, 1, '前置：同键第二笔排队（串行）');
    await act(async () => { writes[0].reject(new Error('controlled failure #1')); });
    await flush(3);
    assert.equal(writes.length, 2, '前置：第一笔落定后第二笔发出');
    await act(async () => { writes[1].reject(new Error('controlled failure #2')); });
    await flush(3);

    const { selectFeedConfig } = await import('../../dist-test/store/selectors.js');
    const cfg = selectFeedConfig(useAppStore.getState(), '10');
    assert.equal(cfg.summaryMode, 'off', '两笔均失败后回到持久化 off（不是第一笔乐观 on）');
    assert.equal(cfg.autoSummary, false, '有效配置回到关（消费者读到的不是中间乐观残留）');

    /* 消费者行为：此刻打开文章，有效配置为关 → 不得发起自动摘要 */
    await act(async () => { useAppStore.getState().selectArticle('101'); });
    await waitFor(() => getBodyEntry('101')?.state === 'ready', { label: '101 水合完成' });
    await flush(2);
    assert.equal(invokeCount('ai_summarize'), 0,
      '乐观残留被回滚后：打开文章不得触发自动摘要（修前停在 on 会触发）');
    assert.ok(entry.container.querySelector('.reader-active-view'), 'Reader 挂载树保持');
  } finally {
    resetPlan();
  }
});

componentTest('OPT-011 R3：pending→bootstrap→旧失败不得把 Reader 重新开启', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows(ROWS, { layout: 'article' });

  const { createElement, act } = await import('react');
  const { Reader } = await import('../../src/components/Reader.tsx');
  const entry = await mount(createElement(Reader));

  const writes = [];
  setPlan({
    set_feed_ai_modes: () => new Promise((resolve, reject) => writes.push({ resolve, reject })),
  });
  try {
    /* 确认 on：成功一笔（真实 store action + 真实队列） */
    await act(async () => { useAppStore.getState().toggleFeedSummary('cat-1', '10', 'on'); });
    await flush(2);
    assert.equal(writes.length, 1, '前置：开启写发出');
    await act(async () => { writes[0].resolve(null); });
    await flush(2);
    assert.equal(useAppStore.getState().feedIndex.get('10')?.feed.summaryMode, 'on', '前置：确认为 on');

    /* 写 off 挂起 → 真实 bootstrap 接受后端 off（夹具真值）→ 原请求 reject */
    await act(async () => { useAppStore.getState().toggleFeedSummary('cat-1', '10', 'off'); });
    await flush(2);
    assert.equal(writes.length, 2, '前置：关闭写挂起（受控 IPC 未落定）');
    await act(async () => { await useAppStore.getState().bootstrapFromBackend(); });
    await flush(2);
    const { selectFeedConfig } = await import('../../dist-test/store/selectors.js');
    assert.equal(selectFeedConfig(useAppStore.getState(), '10').summaryMode, 'off', 'bootstrap 接受后端 off');
    await act(async () => { writes[1].reject(new Error('controlled post-bootstrap failure')); });
    await flush(3);
    assert.equal(selectFeedConfig(useAppStore.getState(), '10').summaryMode, 'off',
      '接受点后的旧失败不得把 on 踩回新确认态（修前 apply 旧 base on）');
    assert.equal(selectFeedConfig(useAppStore.getState(), '10').autoSummary, false, '有效配置保持关');

    /* 消费者行为：打开文章不得因旧失败被重新开启 */
    await act(async () => { useAppStore.getState().selectArticle('101'); });
    await waitFor(() => getBodyEntry('101')?.state === 'ready', { label: '101 水合完成' });
    await flush(2);
    assert.equal(invokeCount('ai_summarize'), 0, '旧失败回滚后：打开文章不得触发自动摘要');
    assert.ok(entry.container.querySelector('.reader-active-view'), 'Reader 挂载树保持');
  } finally {
    resetPlan();
  }
});

componentTest('OPT-011 R4：读取在途期间开启成功——快照不吞意图、Reader 按最终确认态触发', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows(ROWS, { layout: 'article' });

  const { createElement, act } = await import('react');
  const { Reader } = await import('../../src/components/Reader.tsx');
  const entry = await mount(createElement(Reader));

  /* 受控：list_feeds 返回**冻结副本**（off），放行时刻由用例控制（读取接受暂缓）；
     set_feed_ai_modes 的回应也由用例控制。全部用受控 Promise，不用随机 sleep。 */
  const writes = [];
  let released = false;
  let releaseFeedsRead = null;
  setPlan({
    list_feeds: () => {
      const frozen = FEEDS.map((f) => ({ ...f, auto_summary: false }));
      if (released) return Promise.resolve(frozen);
      return new Promise((resolve) => { releaseFeedsRead = () => { released = true; resolve(frozen); }; });
    },
    set_feed_ai_modes: (args) => new Promise((resolve, reject) => writes.push({ args, resolve, reject })),
  });
  try {
    /* 读取在途（接受暂缓）期间点击开启：写入挂起、意图乐观可见 */
    const bootP = useAppStore.getState().bootstrapFromBackend();
    await flush(2);
    assert.ok(releaseFeedsRead, '前置：list_feeds 已发出并挂起（冻结副本未放行）');
    await act(async () => { useAppStore.getState().toggleFeedSummary('cat-1', '10', 'on'); });
    await flush(2);
    assert.equal(writes.length, 1, '前置：开启写挂起');
    assert.equal(useAppStore.getState().feedIndex.get('10')?.feed.summaryMode, 'on', '前置：乐观值当帧可见');

    /* 放行旧 off 快照 → 接受不得吞掉未落定意图 */
    await act(async () => { releaseFeedsRead(); });
    await bootP;
    await flush(3);
    assert.equal(useAppStore.getState().feedIndex.get('10')?.feed.summaryMode, 'on',
      '接受 off 快照后：未落定意图 on 保留（消费者读到的是意图，不是旧快照）');

    /* 写成功：确认态与可视值对齐 on */
    await act(async () => { writes[0].resolve(null); });
    await flush(3);
    const { selectFeedConfig } = await import('../../dist-test/store/selectors.js');
    assert.equal(selectFeedConfig(useAppStore.getState(), '10').summaryMode, 'on', '成功后确认态 = on');

    /* 消费者：有效配置为开 → 打开文章触发一次自动摘要 */
    await act(async () => { useAppStore.getState().selectArticle('101'); });
    await waitFor(() => getBodyEntry('101')?.state === 'ready', { label: '101 水合完成' });
    await waitFor(() => invokeCount('ai_summarize') === 1, { label: '最终确认态为开：触发自动摘要' });
    assert.equal(invokeCount('ai_summarize'), 1, 'Reader 按最终确认态（on）触发一次自动摘要');
    assert.ok(entry.container.querySelector('.reader-active-view'), 'Reader 挂载树保持');
  } finally {
    resetPlan();
  }
});

componentTest('OPT-011 FeedsTab：三态下拉真实点击 → store 原子更新 + 三态 IPC 落库', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows(ROWS, { layout: 'article' });

  const { createElement, act } = await import('react');
  const { FeedsTab } = await import('../../src/components/settings/FeedsTab.tsx');
  const entry = await mount(createElement(FeedsTab));
  await flush(2);

  /* 分类默认先开：源仍显式 off → 有效值关；三态控件显示「关闭」 */
  await act(async () => { useAppStore.getState().toggleCatSummary('cat-1', true); });
  await flush(2);
  const summaryLabel = entry.container.querySelector('.mgr-ai-mode-label');
  assert.ok(summaryLabel, '源行应有摘要三态控件');
  const trigger = summaryLabel.querySelector('.flux-dropdown-trigger');
  assert.ok(trigger, '三态控件应渲染 FluxDropdown 触发器');
  assert.ok(trigger.textContent.includes('继承') === false && trigger.textContent.includes('关闭'),
    `夹具旧行 bool=false → 显示显式「关闭」（实际 ${trigger.textContent}）`);

  /* 真实下拉：打开 → 选「继承（开）」→ store 当帧同值 + IPC 三态落库 */
  await act(async () => { trigger.click(); });
  await flush(2);
  const menu = document.body.querySelector('.flux-dropdown-menu');
  assert.ok(menu, '下拉菜单应经 portal 打开');
  const inheritOption = [...menu.querySelectorAll('.flux-dropdown-option')]
    .find((o) => o.textContent.includes('继承'));
  assert.ok(inheritOption, '菜单应含「继承」选项（标签携带分类当前默认）');
  await act(async () => { inheritOption.click(); });
  await flush(3);

  const st = useAppStore.getState();
  const feedIdx = st.feedIndex.get('10')?.feed;
  const cat = st.feedIndex.get('10')?.cat;
  assert.equal(feedIdx?.summaryMode, 'inherit', '点击后 feedIndex 当帧看到 inherit（原子索引更新）');
  assert.equal(cat?.autoSummary, true, 'feedIndex 的分类引用与 categories 同一份（默认开）');

  const modeCalls = invokeCalls.filter((c) => c.cmd === 'set_feed_ai_modes').map((c) => c.args);
  assert.equal(modeCalls.length, 1, '真实点击应发一次 set_feed_ai_modes');
  assert.equal(modeCalls[0].summaryMode, 'inherit', '落库为三态真值（inherit）');
  assert.equal(modeCalls[0].translateMode, null, '未改动的翻译字段不带（只写被改字段）');

  /* 再选「关闭」：显式关闭压过分类开启（有效值关） */
  await act(async () => { summaryLabel.querySelector('.flux-dropdown-trigger').click(); });
  await flush(2);
  const menu2 = document.body.querySelector('.flux-dropdown-menu');
  const offOption = [...menu2.querySelectorAll('.flux-dropdown-option')].find((o) => o.textContent.trim() === '关闭');
  assert.ok(offOption, '菜单应含「关闭」选项');
  await act(async () => { offOption.click(); });
  await flush(3);
  assert.equal(useAppStore.getState().feedIndex.get('10')?.feed.summaryMode, 'off', '显式关闭落库为 off');
  const { selectFeedConfig } = await import('../../dist-test/store/selectors.js');
  assert.equal(selectFeedConfig(useAppStore.getState(), '10').autoSummary, false,
    '分类开 + 源显式 off → 有效值关（可在分类开启下明确关闭）');
});
