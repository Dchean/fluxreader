// tools/frontend-tests/ai-cache-signature.mjs
// 领域模块：OPT-013 B1 AI 产物缓存签名（前端命中判定 / 请求代次 / 在途复用）
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
  const { S, store, checkNew, nTick, bootFixture, mkRow, BASE_ROWS, bodyOf } = ctx;
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
}
