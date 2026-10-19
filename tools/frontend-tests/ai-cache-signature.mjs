// tools/frontend-tests/ai-cache-signature.mjs
// 领域模块：OPT-013 B1/B2 AI 产物缓存签名（前端命中判定 / 请求代次 / 在途复用）
// B2 追加：翻译分段计划的最小提示（整篇重生成必告知 / 继续提示 / 静默边界）
// 覆盖：旧签名/无签名产物不得被前端非空短路（真实行为断言，不以源码 contains
// 代替）；后端判定命中才短路；逆序安全回读守卫（可控 Promise 时序，不靠 sleep
// 竞态）；过时 Done 不清除较新请求的标记；自动触发与手动入口复用同一在途请求。
// 数据所有权：共享假后端/夹具归 harness（见 harness.mjs 头注），本模块不 import
// 第二份 store；夹具复位经 bootFixture/resetStore（含 resetAiRequestStateForTests）。
export const id = 'ai-cache-signature';

/** 本地 deferred（与 component-tests/fixtures.mjs 同形；本模块只用它做可控时序）。 */
function deferred() {
  let resolve; let reject;
  const promise = new Promise((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

export async function run(ctx) {
  const { S, store, checkNew, nTick, bootFixture, mkRow, BASE_ROWS, bodyOf, selectAiProductState } = ctx;
  await ctx.useMainBackend();

  /* ---------- (b1-1) 无签名/旧签名产物不得被前端非空短路（后端判定未命中） ----------
     行为断言：条目上已显示旧摘要（列表行携带、无签名），此时调用摘要入口必须
     真的发出 ai_summarize（mock 记录请求）；把后端判定置回「命中」后同一入口
     不得重复计费。判别力：修前（非空即短路）第一条的 invoke 计数为 0 即红。 */
  await bootFixture();
  S.backendRows = BASE_ROWS.map((r) => (r.id === 104 ? { ...r, ai_summary: '旧模型生成的摘要' } : r));
  await store.getState().reloadFromBackend();
  checkNew('(b1-1) 前置：条目显示旧摘要（无签名字段 = 未标记版本）',
    (store.getState().entries.find((a) => a.id === '104')?.aiSummary ?? '') === '旧模型生成的摘要');
  S.aiCacheCheck = { summaryVerified: false, translationVerified: false };
  S.aiSum = { deltas: ['新摘要'], error: null, reject: null, finish: true, holdIds: [] };
  S.invokeCalls.length = 0;
  store.getState().summarizeEntry('104');
  await nTick(20);
  checkNew('(b1-1) 后端判定未命中：必须真实发起 ai_summarize（旧产物不得当可复用缓存短路）',
    S.invokeCalls.filter((c) => c.cmd === 'ai_summarize').length === 1);
  checkNew('(b1-1) 命中判定经后端 ai_cache_check（不是前端自算或源码文本判断）',
    S.invokeCalls.filter((c) => c.cmd === 'ai_cache_check').length >= 1
    && bodyOf(store.getState(), '104').aiSummary === '新摘要');
  S.aiCacheCheck = { summaryVerified: true, translationVerified: true };
  S.invokeCalls.length = 0;
  store.getState().summarizeEntry('104');
  await nTick(20);
  checkNew('(b1-1) 对照：后端判定命中 → 短路展示、不重复发起（计数 0）',
    S.invokeCalls.filter((c) => c.cmd === 'ai_summarize').length === 0);
  checkNew('(b1-1) 对照：短路前确实问了后端（校验 IPC 发出）',
    S.invokeCalls.filter((c) => c.cmd === 'ai_cache_check').length === 1);
  S.backendRows = BASE_ROWS;

  /* ---------- (b1-2) 两次安全回读逆序返回 → 新结果不被旧回包覆盖 ----------
     可控 Promise 时序：读回 A（第一笔，gen 1）与读回 B（第二笔，gen 2）都挂起，
     先放行 B、再放行 A；代次守卫必须让 A 整体丢弃（不覆盖 B 的产物与签名）。 */
  await bootFixture();
  const readA = deferred();
  const readB = deferred();
  let readCount = 0;
  S.detailImpl = (id) => {
    readCount += 1;
    const p = readCount === 1 ? readA.promise : readB.promise;
    void id;
    return p;
  };
  S.aiCacheCheck = { summaryVerified: true, translationVerified: false };
  /* deferIds：第一笔的 invoke 由用例在 done 之后手动落定——「流已 done、invoke
     已返回、读回仍在途」是真实时序（读回在 invoke 之后发出）；同键在途表在
     invoke settle 后摘除，第二笔才可能发起。 */
  S.aiTr = { deltas: [], error: null, reject: null, finish: true, holdIds: [], deferIds: [201] };
  store.getState().translateEntry('201');
  await nTick(10);
  const b12h1 = S.heldAi.find((h) => h.cmd === 'ai_translate' && h.id === 201);
  b12h1.ch.onmessage?.({ type: 'delta', data: '第一笔' });
  b12h1.ch.onmessage?.({ type: 'done' }); // → 读回 A 发出（挂起）
  b12h1.resolve('');
  await nTick(10);
  checkNew('(b1-2) 前置：第一笔 done 后读回 A 挂起（未落地）',
    readCount === 1 && store.getState().rawTranslatedIds['201'] === true);
  /* 第二笔：产品非空（第一笔流式文本）但后端判定未命中 → 重新生成 */
  store.getState().translateEntry('201');
  await nTick(10);
  const b12h2 = S.heldAi.find((h) => h !== b12h1 && h.cmd === 'ai_translate' && h.id === 201);
  checkNew('(b1-2) 前置：第二笔真实发起（未命中不短路，同键前一笔已 settle）', !!b12h2);
  b12h2.ch.onmessage?.({ type: 'delta', data: '第二笔' });
  /* 过时 done 守卫：旧代次（第一笔）的 done 在第二笔流式期间到达——不得清除
     第二笔的生成/未消毒标记（translatingIds/rawTranslatedIds 仍为第二笔置位） */
  checkNew('(b1-2) 前置：第二笔在途标记置位（生成中 + 未消毒）',
    store.getState().translatingIds['201'] === true && store.getState().rawTranslatedIds['201'] === true);
  b12h1.ch.onmessage?.({ type: 'done' });
  await nTick(5);
  checkNew('(b1-2) 过时 Done 不得清除较新请求的标记（translatingIds/rawTranslatedIds 保留）',
    store.getState().translatingIds['201'] === true && store.getState().rawTranslatedIds['201'] === true);
  b12h2.ch.onmessage?.({ type: 'done' }); // → 读回 B 发出（挂起）
  await nTick(10);
  checkNew('(b1-2) 前置：读回 B 挂起（两笔回读都在途，时序可逆）', readCount === 2);
  /* 先放行新回读（B），再放行旧回读（A） */
  readB.resolve(mkRow({ id: 201, content_html: '<p>详情</p>', translated_content: '<p>B 新结果</p>', translated_sig: 'sig-b' }));
  await nTick(10);
  checkNew('(b1-2) 新回读落地：B 的产物与签名写入记录',
    bodyOf(store.getState(), '201').translatedContent === '<p>B 新结果</p>');
  readA.resolve(mkRow({ id: 201, content_html: '<p>详情</p>', translated_content: '<p>A 旧结果</p>', translated_sig: 'sig-a' }));
  await nTick(10);
  checkNew('(b1-2) 逆序回读：迟到旧回包被代次守卫整体丢弃（不覆盖新结果/不清新标记）',
    bodyOf(store.getState(), '201').translatedContent === '<p>B 新结果</p>'
    && store.getState().rawTranslatedIds['201'] === undefined);
  await bootFixture();

  /* ---------- (b1-3) 同一 (条目, 产物) 在途：自动触发与手动入口复用，只发一次 ---------- */
  await bootFixture();
  store.setState({ activeArticleId: '201' });
  S.aiCacheCheck = { summaryVerified: true, translationVerified: false };
  S.aiTr = { deltas: [], error: null, reject: null, finish: true, holdIds: [201] };
  S.invokeCalls.length = 0;
  store.getState().translateEntry('201');                      // 手动入口
  await nTick(10);
  checkNew('(b1-3) 前置：手动入口发出（挂起在途，计数 1）',
    S.invokeCalls.filter((c) => c.cmd === 'ai_translate').length === 1);
  store.getState().toggleReaderTranslation({ silent: true });  // 自动触发入口（Reader effect 同 action）
  await nTick(10);
  checkNew('(b1-3) 自动触发与手动入口复用同一在途请求（ai_translate 仍为 1 次，不重复计费）',
    S.invokeCalls.filter((c) => c.cmd === 'ai_translate').length === 1);
  checkNew('(b1-3) 复用路径只对齐展示态（译文视图打开，未另起请求）',
    store.getState().isShowingTranslatedProse === true);
  await bootFixture();

  /* ---------- (b1-4) 清除后签名失效的消费面：产物被清（行真值空）→ 命中判定不再短路 ----------
     模拟「清缓存」：行真值清空 + 后端判定未命中 → 入口直接发起生成（不经校验短路）。 */
  await bootFixture();
  S.backendRows = BASE_ROWS.map((r) => (r.id === 105 ? { ...r, ai_summary: '将清空的摘要' } : r));
  await store.getState().reloadFromBackend();
  S.backendRows = BASE_ROWS.map((r) => (r.id === 105 ? { ...r, ai_summary: null } : r));
  await store.getState().reloadFromBackend();
  S.aiCacheCheck = { summaryVerified: false, translationVerified: false };
  S.aiSum = { deltas: ['清空后重生'], error: null, reject: null, finish: true, holdIds: [] };
  S.invokeCalls.length = 0;
  store.getState().summarizeEntry('105');
  await nTick(20);
  checkNew('(b1-4) 清缓存后：产物清空的条目重新生成（前端不残留旧产物当缓存）',
    S.invokeCalls.filter((c) => c.cmd === 'ai_summarize').length === 1
    && bodyOf(store.getState(), '105').aiSummary === '清空后重生');
  S.backendRows = BASE_ROWS;
  await bootFixture();

  /* ---------- (b1-5) 验签在途窗口：第二次触发复用整链，不产生第二笔请求 ----------
     R1 复审 P2-1 的判别用例：修前 `aiInFlight` 只在 run* 内登记，而入口闸门在
     验签之前检查——校验在途期间第二次触发同样穿过闸门、同样拿到「未命中」，
     同一 (条目, 产物) 各发一笔生成（双倍计费）。现在「验签 + 条件生成」整链
     登记，第二次触发直接复用（不重复校验、不重复生成）。 */
  await bootFixture();
  S.backendRows = BASE_ROWS.map((r) => (r.id === 104 ? { ...r, ai_summary: '旧摘要（待验签）' } : r));
  await store.getState().reloadFromBackend();
  S.aiCacheCheck = { summaryVerified: false, translationVerified: false };
  S.aiCacheCheckPlan = { mode: 'defer' }; // 校验挂起（粘性计划）
  S.aiSum = { deltas: ['新摘要'], error: null, reject: null, finish: true, holdIds: [] };
  S.invokeCalls.length = 0;
  store.getState().summarizeEntry('104'); // 触发 A → 校验在途
  await nTick(10);
  checkNew('(b1-5) 前置：校验在途且尚未发起生成（窗口成立）',
    S.pendingAiChecks.length === 1 && S.invokeCalls.filter((c) => c.cmd === 'ai_summarize').length === 0);
  store.getState().summarizeEntry('104'); // 触发 B（校验仍在途）
  await nTick(10);
  checkNew('(b1-5) 验签在途期间第二次触发复用整链（校验 IPC 仍为 1 次）',
    S.pendingAiChecks.length === 1);
  for (const p of [...S.pendingAiChecks]) p.resolve();
  await nTick(20);
  checkNew('(b1-5) 未命中后整链只发一笔生成请求（同 (条目, 产物) 不重复计费）',
    S.invokeCalls.filter((c) => c.cmd === 'ai_summarize').length === 1);
  S.aiCacheCheckPlan = null;
  S.pendingAiChecks = [];
  S.backendRows = BASE_ROWS;
  await bootFixture();

  /* ---------- (b2-1) OPT-013 B2：整篇重生成必须如实告知（最小提示） ----------
     旧片段因输入变化作废（discarded > 0）→ 无论有无前端产物，触发翻译都必须
     出现告知提示（定版 §3 的界面告知义务）。R1 复审 P3-4：作废与可续并存
     （discarded > 0 且当前签名 completed > 0）时实际从第 completed+1 段继续、
     复用已完成段——文案必须二分：纯作废才称「整篇重新翻译」，混合态称
     「旧片段已作废 + 从第 N 段继续」。 */
  await bootFixture();
  const toastTexts = () => store.getState().toasts.map((t) => t.text);
  const clearToasts = () => store.setState({ toasts: [] });
  /* 变体 A（混合态）：前端已有（未验证）产物 + 当前签名有 1 段可续、另有 1 段作废 */
  S.backendRows = BASE_ROWS.map((r) => (r.id === 201 ? { ...r, translated_content: '<p>旧译文</p>' } : r));
  await store.getState().reloadFromBackend();
  S.aiCacheCheck = {
    summaryVerified: true, translationVerified: false,
    translationSegments: { total: 3, completed: 1, discarded: 1 },
  };
  S.aiTr = { deltas: ['新译文'], error: null, reject: null, finish: true, holdIds: [] };
  clearToasts();
  store.getState().translateEntry('201');
  await nTick(20);
  checkNew('(b2-1) 作废与可续并存：提示必须如实为「旧片段已作废 + 从第 2 段继续」（旧产物在也不豁免）',
    toastTexts().some((t) => t.includes('旧片段已作废') && t.includes('第 2 段') && t.includes('共 3 段')));
  checkNew('(b2-1) 混合态不得出现「整篇重新翻译」（文案与复用已完成段的实际行为必须一致）',
    !toastTexts().some((t) => t.includes('整篇重新翻译')));
  checkNew('(b2-1) 提示与生成并行：生成请求照常发出（提示不阻塞）',
    S.invokeCalls.filter((c) => c.cmd === 'ai_translate').length === 1);

  /* 变体 B：前端无产物且无可续段（completed = 0，旧片段只在后端）→ 纯作废，
     必须提示整篇重新生成 */
  await bootFixture();
  S.aiCacheCheck = {
    summaryVerified: true, translationVerified: false,
    translationSegments: { total: 3, completed: 0, discarded: 1 },
  };
  S.aiTr = { deltas: ['新译文'], error: null, reject: null, finish: true, holdIds: [] };
  clearToasts();
  store.getState().translateEntry('201');
  await nTick(20);
  checkNew('(b2-1) 纯作废（completed = 0）且前端无产物：提示整篇重生成',
    toastTexts().some((t) => t.includes('整篇重新翻译')));
  S.backendRows = BASE_ROWS;
  await bootFixture();

  /* ---------- (b2-2) 从已完成片段继续：非静默入口提示从哪一段继续 ---------- */
  await bootFixture();
  S.aiCacheCheck = {
    summaryVerified: true, translationVerified: false,
    translationSegments: { total: 3, completed: 1, discarded: 0 },
  };
  S.aiTr = { deltas: ['继续段'], error: null, reject: null, finish: true, holdIds: [] };
  clearToasts();
  store.getState().translateEntry('201');
  await nTick(20);
  checkNew('(b2-2) 有已完成片段且未作废：提示从第 2 段继续（只重发未完成段）',
    toastTexts().some((t) => t.includes('第 2 段') && t.includes('共 3 段')));

  /* 静默（自动触发）不提示「继续」（避免按文章刷屏），但作废仍必须提示 */
  await bootFixture();
  store.setState({ activeArticleId: '201' });
  S.aiCacheCheck = {
    summaryVerified: true, translationVerified: false,
    translationSegments: { total: 3, completed: 1, discarded: 0 },
  };
  S.aiTr = { deltas: ['继续段'], error: null, reject: null, finish: true, holdIds: [] };
  clearToasts();
  store.getState().toggleReaderTranslation({ silent: true });
  await nTick(20);
  checkNew('(b2-2) 静默自动触发不提示「继续」（继续是常态信息，不做刷屏）',
    !toastTexts().some((t) => t.includes('继续')));
  await bootFixture();
  store.setState({ activeArticleId: '201' });
  S.aiCacheCheck = {
    summaryVerified: true, translationVerified: false,
    translationSegments: { total: 3, completed: 0, discarded: 2 },
  };
  S.aiTr = { deltas: ['重生成'], error: null, reject: null, finish: true, holdIds: [] };
  clearToasts();
  store.getState().toggleReaderTranslation({ silent: true });
  await nTick(20);
  checkNew('(b2-2) 作废（整篇重生成）即使静默自动触发也必须提示（告知义务不让位）',
    toastTexts().some((t) => t.includes('整篇重新翻译')));
  S.backendRows = BASE_ROWS;
  await bootFixture();
  store.setState({ activeArticleId: '201' });
  S.aiCacheCheck = {
    summaryVerified: true, translationVerified: false,
    translationSegments: { total: 3, completed: 3, discarded: 1 },
  };
  S.aiTr = { deltas: ['直接提交'], error: null, reject: null, finish: true, holdIds: [] };
  clearToasts();
  store.getState().toggleReaderTranslation();
  await nTick(20);
  checkNew('(b2-2) 作废但全部段已在（completed == total）：文案不得再说「整篇重新翻译」（B3 收口，零重发直接提交）',
    toastTexts().some((t) => t.includes('全部片段就绪'))
    && !toastTexts().some((t) => t.includes('整篇重新翻译')));
  await bootFixture();

  /* ============================================================
     OPT-013 B3：五态单点真值、刷新一致性、三入口（真实调用断言）
     ============================================================ */

  /* ---------- (b3-1) 五态判定表（切片层；唯一且互斥，不存在「产物非空=complete」回退） ---------- */
  await bootFixture();
  const stateOf = (id, kind) => selectAiProductState(store.getState(), id, kind);
  const zeroPlan = { total: 1, completed: 0, discarded: 0 };
  /* generating：在途登记优先于一切产物/校验数据 */
  store.setState({ summarizingIds: { 104: true } });
  checkNew('(b3-1) generating：该产物有在途请求（不受产物与校验缓存影响）',
    stateOf('104', 'summary') === 'generating');
  /* complete：产物 + 签名 + 后端校验通过 */
  S.backendRows = BASE_ROWS.map((r) => (r.id === 104 ? { ...r, ai_summary: '已生成摘要', ai_summary_sig: 'sig-ok' } : r));
  await store.getState().reloadFromBackend();
  store.setState({ summarizingIds: {}, aiStatusById: { 104: { summaryVerified: true, translationVerified: false, translationSegments: zeroPlan } } });
  checkNew('(b3-1) complete：产物非空且签名经后端校验通过（唯一 complete 路径）',
    stateOf('104', 'summary') === 'complete');
  checkNew('(b3-1) complete 与 stale 互斥：校验通过的产物不得同时被判已过期',
    stateOf('104', 'summary') !== 'stale');
  /* stale（校验不匹配：换模型/提示词/正文修订） */
  store.setState({ aiStatusById: { 104: { summaryVerified: false, translationVerified: false, translationSegments: zeroPlan } } });
  checkNew('(b3-1) stale：产物非空但校验不匹配',
    stateOf('104', 'summary') === 'stale');
  /* stale（签名缺失）——旧无签名产物；没有校验数据也必须如实标注 */
  await bootFixture();
  S.backendRows = BASE_ROWS.map((r) => (r.id === 104 ? { ...r, ai_summary: '存量摘要（无签名）' } : r));
  await store.getState().reloadFromBackend();
  checkNew('(b3-1) stale：产物非空但签名缺失（旧无签名产物）',
    stateOf('104', 'summary') === 'stale');
  /* 关键负断言：产物非空 + 有签名 + 无校验数据 → 未判定，绝不回退成 complete */
  await bootFixture();
  S.backendRows = BASE_ROWS.map((r) => (r.id === 104 ? { ...r, ai_summary: '未校验摘要', ai_summary_sig: 'sig-unknown' } : r));
  await store.getState().reloadFromBackend();
  checkNew('(b3-1) 不存在「产物非空=complete」回退：未校验时不宣称完成（结论 = 未判定）',
    stateOf('104', 'summary') === null);
  /* partial：无干净产物 + 当前签名 0 < completed < total */
  await bootFixture();
  store.setState({ aiStatusById: { 201: { summaryVerified: false, translationVerified: false, translationSegments: { total: 3, completed: 1, discarded: 0 } } } });
  checkNew('(b3-1) partial：无完整产物但当前签名有可续片段（0<completed<total）',
    stateOf('201', 'translation') === 'partial');
  checkNew('(b3-1) partial 与 stale 互斥：无产物不得误标已过期', stateOf('201', 'translation') !== 'stale');
  /* failed：无产物、无可用片段、有明确失败 */
  await bootFixture();
  store.setState({ translateErrors: { 201: '限流' } });
  checkNew('(b3-1) failed：无产物、无可用已完成片段、存在明确失败',
    stateOf('201', 'translation') === 'failed');
  store.setState({ translateErrors: {} });
  checkNew('(b3-1) 五态外：无产物/无在途/无进度/无错误 → 未判定（不冒充任何态）',
    stateOf('201', 'translation') === null && stateOf('201', 'summary') === null);
  await bootFixture();

  /* ---------- (b3-2) partial 刷新重启后仍为 partial（不被当完整缓存） ---------- */
  await bootFixture();
  S.backendRows = BASE_ROWS.map((r) => ({ ...r })); // 无产物行（干净重启）
  S.aiCacheCheck = {
    summaryVerified: true, translationVerified: false,
    translationSegments: { total: 3, completed: 1, discarded: 0 },
  };
  await store.getState().reloadFromBackend();
  checkNew('(b3-2) 前置：重启后产物为空（partial 不得被当完整缓存）',
    (store.getState().entries.find((a) => a.id === '201')?.translatedContent ?? '') === '');
  store.getState().refreshAiStatus('201'); // Reader 打开文章路径的状态刷新
  await nTick(20);
  checkNew('(b3-2) 重启后刷新状态：仍为 partial 且已完成段数不减少（1/3）',
    stateOf('201', 'translation') === 'partial'
    && store.getState().aiStatusById['201']?.translationSegments.completed === 1);
  S.invokeCalls.length = 0;
  S.aiTr = { deltas: ['续译'], error: null, reject: null, finish: true, holdIds: [] };
  store.getState().translateEntry('201');
  await nTick(20);
  checkNew('(b3-2) partial 不被 complete 短路命中：继续入口真实调用后端（只重发缺失段由 B2 命令层恢复语义保证）',
    S.invokeCalls.filter((c) => c.cmd === 'ai_translate').length === 1);
  await bootFixture();

  /* ---------- (b3-3) 清除入口：真实调用后端 + 状态回未生成 + 再继续零恢复 ---------- */
  await bootFixture();
  S.aiCacheCheck = {
    summaryVerified: true, translationVerified: false,
    translationSegments: { total: 3, completed: 1, discarded: 0 },
  };
  store.getState().refreshAiStatus('201');
  await nTick(20);
  checkNew('(b3-3) 前置：partial（可清除）', stateOf('201', 'translation') === 'partial');
  S.invokeCalls.length = 0;
  const cleared = await store.getState().clearAiProduct('201', 'translation');
  const clearCall = S.invokeCalls.find((c) => c.cmd === 'ai_clear_cache');
  checkNew('(b3-3) 清除必须真实调用后端命令（articleId + product=translation，不是只清前端文字）',
    cleared === true && !!clearCall && clearCall.args.articleId === 201 && clearCall.args.product === 'translation');
  checkNew('(b3-3) 清除后回到「无产物/未生成」：不得再显示 partial（进度归零）',
    stateOf('201', 'translation') === null
    && store.getState().aiStatusById['201']?.translationSegments.completed === 0);
  /* 后端清除后计划归零（夹具如实跟进）；再「继续」= 后端零恢复（全段重发由
     Rust 命令层测试断言），前端侧断言真实发起且不再显示「继续」类提示。 */
  S.aiCacheCheck = {
    summaryVerified: true, translationVerified: false,
    translationSegments: { total: 3, completed: 0, discarded: 0 },
  };
  S.invokeCalls.length = 0;
  store.setState({ toasts: [] });
  S.aiTr = { deltas: ['全量'], error: null, reject: null, finish: true, holdIds: [] };
  store.getState().translateEntry('201');
  await nTick(20);
  checkNew('(b3-3) 清除后再继续：真实发起生成（零恢复语义由命令层全段重发证据承担）',
    S.invokeCalls.filter((c) => c.cmd === 'ai_translate').length === 1
    && !store.getState().toasts.some((t) => t.text.includes('继续')));
  await bootFixture();

  /* ---------- (b3-4) stale：如实标注 + 重新生成真实发起（不再被旧产物短路） ---------- */
  await bootFixture();
  S.backendRows = BASE_ROWS.map((r) => (r.id === 201 ? { ...r, translated_content: '<p>旧译文</p>', translated_sig: 'sig-old' } : r));
  await store.getState().reloadFromBackend();
  S.aiCacheCheck = {
    summaryVerified: true, translationVerified: false,
    translationSegments: { total: 1, completed: 0, discarded: 0 },
  };
  store.getState().refreshAiStatus('201');
  await nTick(20);
  checkNew('(b3-4) stale：签名不匹配的旧产物如实标注（不再冒充可复用缓存）',
    stateOf('201', 'translation') === 'stale');
  S.invokeCalls.length = 0;
  S.aiTr = { deltas: [], error: null, reject: null, finish: true, holdIds: [201] };
  store.getState().translateEntry('201');
  await nTick(20);
  checkNew('(b3-4) 重新生成前不得被当完整缓存短路：入口真实发起生成',
    S.invokeCalls.filter((c) => c.cmd === 'ai_translate').length === 1);
  checkNew('(b3-4) 生成起点清旧产物并进入 generating（未消毒半截按纯文本渲染口径）',
    bodyOf(store.getState(), '201').translatedContent === ''
    && store.getState().rawTranslatedIds['201'] === true
    && stateOf('201', 'translation') === 'generating');
  /* 失败收尾（不悬挂）：流内错误 → 残段按 partial/failed 如实呈现 */
  const heldB34 = S.heldAi.find((h) => h.cmd === 'ai_translate' && h.id === 201);
  heldB34.ch.onmessage?.({ type: 'delta', data: '<b>半截' });
  heldB34.ch.onmessage?.({ type: 'error', data: '限流' });
  await nTick(20);
  checkNew('(b3-4) 失败残段按 partial/failed 呈现（半截文本仍按纯文本渲染口径）',
    store.getState().rawTranslatedIds['201'] === true
    && ['partial', 'failed'].includes(stateOf('201', 'translation')));
  S.backendRows = BASE_ROWS;
  await bootFixture();

  /* ---------- (b3-5) 卡片翻译 IPC 拒绝：失败分型落定后校验状态同口径刷新 ----------
     R1 复审 P3-2 的判别用例：runCardTranslation 的 promise-rejection 分支曾缺
     refreshAiStatus（summary catch / Reader catch / 三处错误回调均有）。修前该
     路径的校验状态只来自生成入口的计划取回（hintSegmentPlan，1 次 IPC），失败
     落定后不再刷新；修后应有第 2 次 ai_cache_check（失败分型与后端真实进度对账）。 */
  await bootFixture();
  S.aiCacheCheck = {
    summaryVerified: true, translationVerified: false,
    translationSegments: { total: 3, completed: 1, discarded: 0 },
  };
  S.aiTr = { deltas: [], error: null, reject: { message: 'down' }, finish: true, holdIds: [] };
  S.invokeCalls.length = 0;
  store.getState().translateEntry('201');
  await nTick(20);
  checkNew('(b3-5) 卡片翻译 IPC 拒绝：错误态落定且校验状态同步刷新（与 summary/Reader 同口径）',
    store.getState().translateErrors['201'] === 'AI 服务未配置或不可达'
    && S.invokeCalls.filter((c) => c.cmd === 'ai_cache_check').length === 2);
  checkNew('(b3-5) 失败后状态如实呈现（可继续）：partial 按同一响应面的进度落账',
    store.getState().aiStatusById['201']?.translationSegments.completed === 1
    && stateOf('201', 'translation') === 'partial');
  S.backendRows = BASE_ROWS;
  await bootFixture();
}
