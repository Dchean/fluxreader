// tools/frontend-tests/reader-and-ai.mjs
// 领域模块：阅读器正文/AI 流式与真值源（bodyById 实体缓存与失效）
// OPT-016C 拆分自 tools/frontend-regression.mjs（旧行区间 1708-1787、1789-1845、1847-1856、2131-2146、2148-2165、2167-2197、2244-2261、2263-2296、4981-5221、5223-5548）；
// 断言名称/条件文本原样迁移，仅做路径深度适配（import.meta.url 与动态 import 深一层）与
// 共享可变状态的 S. 归属重写。数据所有权：共享假后端/夹具/记录归 harness（见 harness.mjs 头注），
// 本模块不 import 第二份 store；域内自带夹具（大行集等）仍在本模块内独立构造与复位。
export const id = 'reader-and-ai';

export async function run(ctx) {
  const { S, store, checkNew, nTick, resetStore, bootFixture, mkRow, BASE_ROWS, nSel, bodyOf, bodyOfT122, getBodyEntryT122, markBodyLoadingT122, dropBodyEntryT122 } = ctx;
  await ctx.useMainBackend();

  /* ============================================================
     (l) AI per-id 流式写入与失败标记
     ============================================================ */
  await bootFixture();
  S.aiSum = { deltas: [], error: null, reject: null, finish: true, holdIds: [104] };
  store.getState().summarizeEntry('104');
  checkNew('(l) summarizeEntry 生成态按 id 置位、流未结束时保持（不误报完成）',
    store.getState().summarizingIds['104'] === true
    && store.getState().entries.find((a) => a.id === '104')?.aiSummary === '');
  /* api.aiSummarize 内部 await import('@tauri-apps/api/core')，inv 在一个微任务后才发出：
     让出一轮宏任务再取挂起的 channel（channel 未 done 前生成态必须保持） */
  await nTick(5);
  const heldSum = S.heldAi.find((h) => h.cmd === 'ai_summarize' && h.id === 104);
  heldSum.ch.onmessage?.({ type: 'delta', data: '摘' });
  heldSum.ch.onmessage?.({ type: 'delta', data: '要完成' });
  checkNew('(l) 流式 delta 增量落到该条目的 aiSummary（其他条目不受影响）',
    store.getState().entries.find((a) => a.id === '104')?.aiSummary === '摘要完成'
    && store.getState().entries.find((a) => a.id === '105')?.aiSummary === '');
  heldSum.ch.onmessage?.({ type: 'done' });
  checkNew('(l) done 清除 summarizingIds[id]（不残留「生成中」占位）',
    store.getState().summarizingIds['104'] === undefined);
  S.invokeCalls.length = 0;
  store.getState().summarizeEntry('104');
  await nTick(5);
  checkNew('(l) 已有摘要缓存时直接短路（不重复请求 AI）',
    S.invokeCalls.filter((c) => c.cmd === 'ai_summarize').length === 0);

  store.setState({ toasts: [] });
  S.aiSum = { deltas: [], error: 'AI 限流', reject: null, finish: true, holdIds: [] };
  store.getState().summarizeEntry('105');
  await nTick(5);
  const lErr = store.getState();
  checkNew('(l) 摘要流内 error 事件：按 id 记录错误 + 清生成态 + toast 带重试',
    lErr.summaryErrors['105'] === 'AI 限流' && lErr.summarizingIds['105'] === undefined
    && lErr.toasts.length === 1 && lErr.toasts[0].text === '摘要失败：AI 限流'
    && lErr.toasts[0].action?.label === '重试');
  S.invokeCalls.length = 0;
  lErr.toasts[0].action.run();
  const lRetryCleared = store.getState().summaryErrors['105'] === '';   // 重试同步清上次错误
  await nTick(5);
  checkNew('(l) 重试按钮真正重新发起该 id 的摘要请求（并先清掉上次错误）',
    S.invokeCalls.filter((c) => c.cmd === 'ai_summarize').length === 1 && lRetryCleared);

  await bootFixture();
  store.setState({ toasts: [] });
  S.aiSum = { deltas: [], error: null, reject: { message: '网络不可达' }, finish: true, holdIds: [] };
  store.getState().summarizeEntry('105', { silent: true });
  await nTick(20);
  const lRej = store.getState();
  checkNew('(l) 摘要请求 reject：落到「AI 服务未配置或不可达」错误态并清生成态',
    lRej.summaryErrors['105'] === 'AI 服务未配置或不可达' && lRej.summarizingIds['105'] === undefined);
  checkNew('(l) silent 模式失败不弹 toast（源级自动摘要不打扰用户）', lRej.toasts.length === 0);

  await bootFixture();
  S.detailImpl = (id) => mkRow({ id, content_html: '<p>详情</p>', translated_content: '<p>已消毒译文</p>' });
  S.aiTr = { deltas: [], error: null, reject: null, finish: true, holdIds: [201] };
  S.invokeCalls.length = 0;
  store.getState().translateEntry('201');
  await nTick(5);   // 等 api 内部 import Channel 后真正发出 inv，再手动推流
  const heldTrS = S.heldAi.find((h) => h.cmd === 'ai_translate' && h.id === 201);
  heldTrS.ch.onmessage?.({ type: 'delta', data: '<p>未消毒<script>alert(1)</script></p>' });
  /* 【TASK-122 改动理由】AI 产物真值源迁移 bodyById——本 fixture 此前经
     anchorToArticle/selectArticle 建立过记录时流式增量落记录，经 selectArticleBody
     读取（无记录时同值回退视图行，两侧断言等价）；保护意图（打字机原样、未收尾）
     逐字保留。下同（(l2)/(p3-f3)/(D1b)/(D1c) 各条）。 */
  checkNew('(l) translateEntry 流式增量先落到 translatedContent（打字机原样展示、未收尾；TASK-122 真值源 bodyById）',
    bodyOf(store.getState(), '201').translatedContent === '<p>未消毒<script>alert(1)</script></p>'
    && store.getState().translatingIds['201'] === true);
  heldTrS.ch.onmessage?.({ type: 'done' });
  checkNew('(l) 流结束（done）立即清 translatingIds[id]（不等回读完成）',
    store.getState().translatingIds['201'] === undefined);
  await nTick(20);
  const lSafe = bodyOf(store.getState(), '201').translatedContent;
  checkNew('(l) 流结束后回读 DB 消毒译文覆盖流式产物（无 <script> 残留；TASK-122 真值源 bodyById）',
    lSafe === '<p>已消毒译文</p>' && !lSafe.includes('<script>'));
  S.invokeCalls.length = 0;
  store.getState().translateEntry('201');
  await nTick(5);
  checkNew('(l) 已有译文缓存时不再触发 ai_translate',
    S.invokeCalls.filter((c) => c.cmd === 'ai_translate').length === 0);

  /* ---------- (l2) TASK-065 N11：rawTranslatedIds 消毒时序（渲染契约 store 侧锚点） ---------- */
  await bootFixture();
  S.detailImpl = (id) => mkRow({ id, content_html: '<p>详情</p>', translated_content: '<p>已消毒译文</p>' });
  S.aiTr = { deltas: [], error: null, reject: null, finish: true, holdIds: [201] };
  store.getState().translateEntry('201');
  await nTick(5);
  const heldN11 = S.heldAi.find((h) => h.cmd === 'ai_translate' && h.id === 201);
  heldN11.ch.onmessage?.({ type: 'delta', data: '<p>未消毒<script>alert(1)</script></p>' });
  checkNew('(l2) 流式期间 rawTranslatedIds[id]=true（未消毒产物按纯文本渲染）',
    store.getState().rawTranslatedIds['201'] === true);
  heldN11.ch.onmessage?.({ type: 'done' });
  checkNew('(l2) done 后消毒回读未落地：标记仍在（消毒版未到位不得切 HTML 渲染）',
    store.getState().rawTranslatedIds['201'] === true);
  await nTick(20);
  checkNew('(l2) 消毒回读落地：标记清除且内容为 DB 消毒版（TASK-122 真值源 bodyById）',
    store.getState().rawTranslatedIds['201'] === undefined
    && bodyOf(store.getState(), '201').translatedContent === '<p>已消毒译文</p>');

  await bootFixture();
  S.detailImpl = () => { throw { message: 'ipc down' }; };
  S.aiTr = { deltas: [], error: null, reject: null, finish: true, holdIds: [201] };
  store.getState().translateEntry('201');
  await nTick(5);
  const heldN11Fail = S.heldAi.find((h) => h.cmd === 'ai_translate' && h.id === 201);
  heldN11Fail.ch.onmessage?.({ type: 'delta', data: '<img src=x onerror=alert(1)>' });
  heldN11Fail.ch.onmessage?.({ type: 'done' });
  await nTick(20);
  checkNew('(l2) 消毒回读失败：丢弃未消毒半截 + 标记清除 + 错误态与 toast 带重试（TASK-122 真值源 bodyById）',
    store.getState().rawTranslatedIds['201'] === undefined
    && bodyOf(store.getState(), '201').translatedContent === ''
    && store.getState().translateErrors['201'] === '译文回读失败'
    && store.getState().toasts.some((t) => t.text === '译文回读失败'));

  await bootFixture();
  S.detailImpl = (id) => mkRow({ id, content_html: '<p>详情</p>', translated_content: '<p>已消毒译文</p>' });
  S.aiTr = { deltas: [], error: null, reject: null, finish: true, holdIds: [201] };
  store.getState().translateEntry('201');
  await nTick(5);
  const heldN11Err = S.heldAi.find((h) => h.cmd === 'ai_translate' && h.id === 201);
  heldN11Err.ch.onmessage?.({ type: 'delta', data: '<b>半截' });
  heldN11Err.ch.onmessage?.({ type: 'error', data: '限流' });
  checkNew('(l2) 流错误路径：半截未消毒内容保留（重试语义）且标记保持（按纯文本渲染；TASK-122 真值源 bodyById）',
    bodyOf(store.getState(), '201').translatedContent === '<b>半截'
    && store.getState().rawTranslatedIds['201'] === true);

  /* 与上一条成对：失败时**无任何 delta**（未消毒产物为空）⇒ 标记必须清除。
     留着会让后续水合写回的 DB 消毒译文走纯文本分支（卡片字面显示 <p>…</p>，
     即 P2-9 的标记粘连）；两条一起才锁定「标记只在有未消毒产物时才保留」。 */
  await bootFixture();
  S.detailImpl = (id) => mkRow({ id, content_html: '<p>详情</p>', translated_content: '<p>已消毒译文</p>' });
  S.aiTr = { deltas: [], error: null, reject: { message: 'down' }, finish: true, holdIds: [] };
  store.getState().translateEntry('201');
  await nTick(20);
  checkNew('(l2) 失败且无未消毒半截：标记清除（否则水合写回的消毒译文被按纯文本渲染；TASK-122 真值源 bodyById）',
    store.getState().rawTranslatedIds['201'] === undefined
    && bodyOf(store.getState(), '201').translatedContent === ''
    && store.getState().translateErrors['201'] === 'AI 服务未配置或不可达');

  /* ---------- (n7) TASK-065：锚定打开复位阅读视图标志（与 selectArticle 同口径） ---------- */
  await bootFixture();
  store.setState({ isShowingTranslatedProse: true, isRawRenderMode: true, showFulltext: true, activeArticleId: null });
  await store.getState().anchorToArticle('101');
  await nTick(20);
  checkNew('(n7) 锚定打开复位阅读视图标志（译文/全文/原始渲染——修前残留使新文章正文空白）',
    store.getState().isShowingTranslatedProse === false
    && store.getState().isRawRenderMode === false
    && store.getState().showFulltext === false
    && store.getState().activeArticleId === '101');

  /* ---------- D1a：摘要「先出半截文本再报错」后，重试必须真的重发 ---------- */
  await bootFixture();
  store.setState({ toasts: [] });
  S.aiSum = { deltas: ['半截摘要'], error: 'AI 限流', reject: null, finish: true, holdIds: [] };
  store.getState().summarizeEntry('105');
  await nTick(5);
  const d1a = store.getState();
  const d1aPartial = d1a.entries.find((a) => a.id === '105')?.aiSummary;
  S.invokeCalls.length = 0;
  d1a.toasts[0]?.action?.run();                       // 点 toast 的「重试」
  const d1aCleared = store.getState().summaryErrors['105'] === '';
  await nTick(5);
  checkNew('(D1a) 半截摘要 + 报错后点「重试」：真的重发 ai_summarize（修前被 if (art.aiSummary) 短路挡住）',
    d1aPartial === '半截摘要' && S.invokeCalls.filter((c) => c.cmd === 'ai_summarize').length === 1);
  checkNew('(D1a) 重试同步清掉上次错误与半截摘要（不再与错误并存，卡片可自愈）',
    d1aCleared === true && store.getState().summaryErrors['105'] === 'AI 限流');

  /* ---------- D1b：卡片翻译「先出半截译文再报错」后，重试必须真的重发 ---------- */
  await bootFixture();
  store.setState({ toasts: [] });
  S.aiTr = { deltas: ['半截译文'], error: '限流', reject: null, finish: true, holdIds: [] };
  store.getState().translateEntry('201');
  await nTick(5);
  const d1bPartial = bodyOf(store.getState(), '201').translatedContent;
  S.aiTr = { deltas: [], error: null, reject: null, finish: false, holdIds: [201] };   // 重试：挂起观察
  S.invokeCalls.length = 0;
  store.getState().toasts[0]?.action?.run();
  await nTick(5);
  const d1b = store.getState();
  checkNew('(D1b) 半截译文 + 报错后点「重试」：真的重发 ai_translate（修前被 art.translatedContent 短路挡住；TASK-122 生效值经 selectArticleBody）',
    d1bPartial === '半截译文' && S.invokeCalls.filter((c) => c.cmd === 'ai_translate').length === 1);
  checkNew('(D1b) 重试清空半截译文与上次错误（与 Reader 路径的重试语义对齐，不把半截当缓存；TASK-122 真值源 bodyById）',
    bodyOf(d1b, '201').translatedContent === '' && d1b.translateErrors['201'] === '');
  S.heldAi[S.heldAi.length - 1]?.ch.onmessage?.({ type: 'done' });
  await nTick(20);

  /* ---------- D1c：Reader 翻译同源路径（error 后译文残留半截） ---------- */
  await bootFixture();
  /* 先把正文置成与 detailImpl 相同的内容：selectArticle 会异步水合详情，
     若 content 为空则会被回填成 translated_content='' —— 那会把流式半截译文冲掉，
     掩盖本用例要观察的状态。
     【TASK-122】selectArticle 详情水合落 bodyById 记录；生成与水合并发时，
     记录落地以「生成中」为回退种子（aiFallback）保住流式半截——读取经
     selectArticleBody（记录存在时记录为真值）。 */
  store.setState((s) => ({
    toasts: [], isShowingTranslatedProse: false,
    entries: s.entries.map((a) => (a.id === '201' ? { ...a, content: '<p>详情</p>' } : a)),
  }));
  store.getState().selectArticle('201');
  S.aiTr = { deltas: ['半截译文'], error: '限流', reject: null, finish: true, holdIds: [] };
  store.getState().toggleReaderTranslation();
  await nTick(5);
  const d1c = store.getState();
  checkNew('(D1c) Reader 翻译半截 + 报错：错误态可见、译文块收起（半截译文仍留在条目上；TASK-122 真值源 bodyById）',
    d1c.translateErrors['201'] === '限流' && d1c.isShowingTranslatedProse === false
    && bodyOf(d1c, '201').translatedContent === '半截译文');
  S.aiTr = { deltas: [], error: null, reject: null, finish: false, holdIds: [201] };
  S.invokeCalls.length = 0;
  d1c.toasts[d1c.toasts.length - 1]?.action?.run();
  await nTick(5);
  checkNew('(D1c) 半截译文 + 报错后点「重试」：真的重发 ai_translate 并重新进入生成态（修前把半截当缓存直接展示）',
    S.invokeCalls.filter((c) => c.cmd === 'ai_translate').length === 1
    && store.getState().translating === true);
  checkNew('(D1c) 重试清空半截译文（原有清空逻辑在修前根本走不到；TASK-122 真值源 bodyById）',
    bodyOf(store.getState(), '201').translatedContent === '');
  S.heldAi[S.heldAi.length - 1]?.ch.onmessage?.({ type: 'done' });
  await nTick(20);

  /* ---------- P2-4：AI「保存提示词」只写提示词，不再顺带覆盖端点配置 ---------- */
  const { mergePromptsOnly } = await import('../../dist-test/components/settings/aiConfig.js');
  const p24 = JSON.parse(mergePromptsOnly(
    JSON.stringify({ preset: 'deepseek', baseUrl: 'https://api.deepseek.com', apiKey: 'sk-keep', model: 'deepseek-chat', summaryPrompt: '旧摘要', translatePrompt: '旧翻译' }),
    { summaryPrompt: '新摘要', translatePrompt: '新翻译' },
  ));
  checkNew('(P2-4) 保存提示词保留库里的端点配置（preset/baseUrl/apiKey/model 原样不动）',
    p24.preset === 'deepseek' && p24.baseUrl === 'https://api.deepseek.com'
    && p24.apiKey === 'sk-keep' && p24.model === 'deepseek-chat');
  checkNew('(P2-4) 两个提示词字段被更新为表单值',
    p24.summaryPrompt === '新摘要' && p24.translatePrompt === '新翻译');
  const p24Fresh = JSON.parse(mergePromptsOnly(null, { summaryPrompt: 'a', translatePrompt: 'b' }));
  checkNew('(P2-4) 库里尚无配置时只落提示词：不把未确认可用的 baseUrl/apiKey 顺带写库',
    !('apiKey' in p24Fresh) && !('baseUrl' in p24Fresh)
    && p24Fresh.summaryPrompt === 'a' && p24Fresh.translatePrompt === 'b');
  const p24Broken = JSON.parse(mergePromptsOnly('{ 坏 JSON', { summaryPrompt: 'a', translatePrompt: 'b' }));
  checkNew('(P2-4) 库里 JSON 损坏时保存仍成功（以提示词重建，不让保存动作失败）',
    p24Broken.summaryPrompt === 'a' && p24Broken.translatePrompt === 'b');

  /* ---------- P2-10 / TASK-076：降级（degraded）不得显示成「提取成功」 ----------
     TASK-076（DEC-req104-p2-10b-fulltext-degraded-20260920）把判据由「返回内容 ==
     当前正文」的字符串比对改为后端结构化 degraded 标志；本段随之改为直接驱动该标志
     （mock 现按 { html, degraded, reason } 返回）。保护意图不变。 */
  await bootFixture();
  /* 【TASK-122 改动理由】提取真值源迁移 bodyById：先经详情链路建立记录（旧用例
     直接在条目上摆 content/rawContent/fulltextExtracted——该字段组已不再是真值）；
     degraded/正常两分支的保护意图（不假报成功 / 置标志+进全文视图）逐字保留，
     标志改从 bodyById 记录断言。 */
  store.setState((s) => ({
    toasts: [],
    activeArticleId: '104',
    showFulltext: false,
    entries: s.entries.map((a) => (a.id === '104' ? { ...a, url: 'https://x.example/a' } : a)),
  }));
  store.getState().ensureArticleContent('104', { extractFulltext: true });
  await nTick(20);
  S.extractResult = { html: '<p>RSS 原文</p>', degraded: true, reason: '提取结果比原正文更短，已保留原正文（原文可能已是全文）' };
  store.getState().extractCurrentArticle();
  await nTick(20);
  const p210 = store.getState();
  checkNew('(P2-10/TASK-076) degraded=true 时：不置 fulltextExtracted、不切全文视图（TASK-122：标志在 bodyById 记录）',
    bodyOf(p210, '104').fulltextExtracted === false && p210.showFulltext === false);
  checkNew('(P2-10/TASK-076) 且如实提示降级原因（后端 reason 原文），而不是报成功',
    p210.toasts.some((t) => t.text.includes('未采用全文提取') && t.text.includes('已保留原正文'))
    && !p210.toasts.some((t) => t.text === '全文提取完成'));
  S.extractResult = { html: '<p>真正的全文正文，明显更长的一段内容。</p>', degraded: false, reason: null };
  store.getState().extractCurrentArticle();
  await nTick(20);
  const p210ok = store.getState();
  checkNew('(P2-10/TASK-076) 正常提取路径不受影响：置标志 + 进入全文视图 + 报「全文提取完成」（TASK-122：标志在 bodyById 记录）',
    bodyOf(p210ok, '104').fulltextExtracted === true && p210ok.showFulltext === true
    && p210ok.toasts.some((t) => t.text === '全文提取完成'));
  S.extractResult = null;

  /* ============================================================
     TASK-103（REQ-001）：文章快照与正文水合生命周期统一
     —— 刷新不丢正文、同 id 刷新后重新水合、终态机完备、乱序防护与在途去重。
     审计探针场景（AUDIT-20261005-core-consistency.md「社交正文问题链路」）：
     reloadFromBackend 替换快照并清空 hydratedIds × 虚拟列表按 id 保持卡片身份
     × useLazyHydrate 依赖仅 [id] ⇒ 卡片停留「加载正文…」且新增正文请求数=0。
     实证口径：with_content 恒 false（bootstrap.layoutNeedsBody 五布局全 false），
     快照行从不携带正文，正文由懒水合按需拉取；mergeSnapshotEntries 负责
     快照替换时的正文/终态继承（收口单点，六个调用点共用）。
     ============================================================ */
  {
    /* 【TASK-122 改动理由】entryNeedsHydration 改从 dist-test 实例取：它现在读取
       模块级 bodyById（真值源迁移），必须与 store 同一模块图才能看到记录
       （原 src/*.ts 原始转译会得到第二份模块状态）。 */
    const { entryNeedsHydration: NEED } = nSel;
    const socialRow = (o) => mkRow({ feed_id: 11, ...o });
    /* store.entries 侧的条目形状（id 为字符串、content/content_html 分离）——
       与首段 S-2 的 socialEntry 同构；getArticlesPlan 返回的才是后端行形状 */
    const t103Entry = (id, extra = {}) => ({
      id: String(id), feedId: '11', title: 't103 帖', publishedAt: Date.now(), isRead: false,
      isStarred: false, tags: [], source: 'direct', snippet: '摘要', author: 'a',
      content: '', rawContent: '', translatedContent: '', aiSummary: '',
      ...extra,
    });
    const t103State = () => store.getState();
    const t103GetArticlesCount = () => S.invokeCalls.filter((c) => c.cmd === 'get_articles').length;

    /* ---------- t103-snapshot-preserves-hydration：快照替换保留水合 ----------
       【TASK-122 改动理由】本组断言原锁 mergeSnapshotEntries 的「按 id 从旧快照行
       继承正文/AI/终态」。继承机制被 bodyById 实体缓存**取代**（审计目标结构：
       正文/AI 随文章实体存活，不随视图行传播；行间继承正是清理失效后旧产物复活
       的通道）。保护意图（刷新不丢已加载正文、无「无请求死区」）原样保留，改由
       bodyById 断言承载：水合建立记录 → reloadFromBackend 替换快照 → 记录不动、
       正文照常可读、零补拉；视图行不再携带正文（字段已瘦身）。 */
    await bootFixture({ activeContentLayout: 'social' });
    // 经真实水合链路建立记录：101 有正文+url；102 空正文终态；103 全量（译文/摘要/全文标记）
    S.getArticlesPlan = { rows: [
      socialRow({ id: 101, content_html: '<p>101 正文</p>', url: 'https://example.com/101' }),
      socialRow({ id: 102, content_html: null }),
      socialRow({ id: 103, content_html: '<p>103 正文</p>', translated_content: '<p>103 译文</p>', ai_summary: '103 摘要', fulltext_extracted: true, url: 'https://example.com/103' }),
    ] };
    store.getState().hydrateArticleContent(['101', '102', '103']);
    await nTick(20);
    S.getArticlesPlan = null;
    /* 列表行与水合行同源（同一 DB 真值）：103 的 AI 列在两处一致——不一致时
       reconcile 以**更新的行真值**对齐（清理失效语义），那由 t122 专属用例覆盖 */
    S.backendRows = BASE_ROWS.map((r) => (r.id === 103
      ? { ...r, translated_content: '<p>103 译文</p>', ai_summary: '103 摘要', fulltext_extracted: true }
      : r));
    const t103BaseCalls = t103GetArticlesCount();
    await store.getState().reloadFromBackend();
    const t103AfterReload = t103State();
    const t103b101 = bodyOfT122(t103AfterReload, '101');
    const t103b102 = bodyOfT122(t103AfterReload, '102');
    const t103b103 = bodyOfT122(t103AfterReload, '103');
    checkNew('(t103-snapshot-preserves-hydration) 快照替换后 bodyById 记录原样保持（正文/原文/译文/摘要/全文标记/url 随实体存活，终态不清空；视图行不再复制正文）',
      t103b101.content === '<p>101 正文</p>' && t103b101.rawContent === '<p>101 正文</p>'
      && t103AfterReload.entries.find((a) => a.id === '101')?.url === 'https://example.com/101'
      && t103b102.content === '' && getBodyEntryT122('102')?.state === 'ready'
      && t103b103.content === '<p>103 正文</p>' && t103b103.rawContent === '<p>103 正文</p>'
      && t103b103.translatedContent === '<p>103 译文</p>'
      && t103b103.aiSummary === '103 摘要'
      && t103b103.fulltextExtracted === true
      && !('content' in (t103AfterReload.entries.find((a) => a.id === '101') || {})));
    checkNew('(t103-snapshot-preserves-hydration) 已水合条目刷新后不触发任何补拉（直接恢复正文，无「无请求死区」）',
      t103GetArticlesCount() === t103BaseCalls
      && NEED(t103AfterReload, '101') === false && NEED(t103AfterReload, '102') === false);
    /* 【TASK-103 → TASK-122 改动理由】「新行自带正文（with_content）以新行为准」
       场景随真值源迁移失效：with_content 恒 false（layoutNeedsBody 五布局全
       false），列表行从不携带正文，视图行也已瘦身不存 content——「行 vs 行」的
       正文优先级问题在结构上消失。改为锁定等价保护：快照行即使带 content_html
       也不写 bodyById/视图行（正文只经水合链路进入记录），url 轻字段照常取行。 */
    S.backendRows = BASE_ROWS.map((r) => (r.id === 104 ? { ...r, content_html: '<p>新104</p>', url: 'https://example.com/104' } : r));
    await store.getState().reloadFromBackend();
    checkNew('(t103-snapshot-preserves-hydration) 快照行携带的 content_html 不再进视图行/记录（正文只经水合链路落 bodyById），url 轻字段照常取行',
      t103State().entries.find((a) => a.id === '104')?.url === 'https://example.com/104'
      && !('content' in (t103State().entries.find((a) => a.id === '104') || {}))
      && getBodyEntryT122('104') === undefined
      && NEED(t103State(), '104') === true);
    S.backendRows = BASE_ROWS;
    // 收口契约的另一侧：范围切换（缓存恢复 + 后台刷新）不触碰记录，正文不丢
    S.getArticlesPlan = { rows: [mkRow({ id: 201, feed_id: 11, content_html: '<p>201 正文</p>' })] };
    store.getState().hydrateArticleContent(['201']);
    await nTick(20);
    S.getArticlesPlan = null;
    store.getState().selectFeed('11');
    await nTick(30);
    store.getState().selectFeed('all');
    checkNew('(t103-snapshot-preserves-hydration) selectFeed 往返（缓存恢复+后台刷新，同走 mergeSnapshotEntries）：201 正文保留（bodyById）',
      bodyOfT122(t103State(), '201').content === '<p>201 正文</p>');

    /* ---------- t103-stale-card-rehydrates：审计探针场景（同 ID 刷新）---------- */
    await bootFixture({ activeContentLayout: 'social' });
    S.getArticlesPlan = { rows: [socialRow({ id: 101, content_html: '<p>101 正文</p>' })] };
    store.getState().hydrateArticleContent(['101']);
    await nTick(20);
    S.getArticlesPlan = null;
    await store.getState().reloadFromBackend();
    const t103Probe = t103State();
    checkNew('(t103-stale-card-rehydrates) 审计探针场景：同 ID 刷新后 content 不再被清空（正文随 bodyById 存活，请求数=0 也不再是死区）',
      bodyOfT122(t103Probe, '101').content === '<p>101 正文</p>'
      && getBodyEntryT122('101')?.state === 'ready');
    // 未水合卡片：刷新后水合前提重新成立 —— entryNeedsHydration 的真值表（useLazyHydrate 重入队的依据）
    /* 【TASK-122 改动理由】真值表随判定收窄更新：旧五元条件（无正文 ∧ 无终态 ∧
       无失败态…）收敛为「条目在 ∧ bodyById 无记录」——终态/失败/在途都在记录
       state 上（loading=在途替代原「不观察在途」的模块级 Set）。 */
    {
      const t103Pending = t103State().entries.find((a) => a.id === '201');
      const t103ProbeNo201 = { ...t103Probe, entries: t103Probe.entries.filter((a) => a.id !== '201') };
      checkNew('(t103-stale-card-rehydrates) 未水合卡片刷新后水合前提重新成立：entryNeedsHydration 仅在「条目在 ∧ 无记录」为真（loading/ready/cleared/missing/failed 均不需要）',
        NEED(t103Probe, '201') === true
        && NEED(t103ProbeNo201, '201') === false);
      // 记录各态逐一验证（loading 在途 / ready / failed 终态都不再入队）
      markBodyLoadingT122('201');
      const t103Loading = t103State();
      checkNew('(t103-stale-card-rehydrates) loading（在途）记录使水合判定为假：重复入队被状态拦下（在途去重的可见化）',
        NEED(t103Loading, '201') === false);
      dropBodyEntryT122('201');
      void t103Pending;
    }
    // 源码形态断言（手法沿用本文件既有 readFileSync 写法）：锁定方案 A ——
    // effect 消费 entryNeedsHydration 布尔值并以 [id, needsHydration] 为依赖
    const fs103 = await import('node:fs');
    const tl103Src = fs103.readFileSync(new URL('../../src/components/Timeline.tsx', import.meta.url), 'utf8');
    const hook103 = tl103Src.slice(tl103Src.indexOf('function useLazyHydrate'), tl103Src.indexOf('/* ---------- 文章卡片'));
    checkNew('(t103-stale-card-rehydrates) useLazyHydrate 不再只依赖 [id]：按 id 订阅 entryNeedsHydration，条件重新成立即重新入队',
      hook103.includes('useAppStore((s) => entryNeedsHydration(s, id))')
      && hook103.includes('if (!needsHydration) return;')
      && hook103.includes('[id, needsHydration]')
      && !hook103.includes('}, [id]);'));

    /* ---------- t103-hydration-terminals：终态机（成功/空正文/缺行/失败）----------
       【TASK-103 → TASK-122 改动理由】终态从 hydratedIds/hydrationErrors 平行 Map
       收敛为 bodyById 记录的判别态（ready/missing/failed——状态机单点文档见
       bodyCache 模块头注）。逐一对应：空正文终态 = ready + content ''；「文章
       不存在」= missing；失败 = failed + message。保护意图（各终态可见、不静默
       留占位、重试收敛）逐字保留。 */
    // （1）空正文：content_html 为 NULL → ready 终态（content ''），不无限重试
    await resetStore();
    store.setState({ entries: [t103Entry(311)] });
    S.getArticlesPlan = { rows: [socialRow({ id: 311 })] };
    store.getState().hydrateArticleContent(['311']);
    await nTick(20);
    const t103E311 = bodyOfT122(t103State(), '311');
    checkNew('(t103-hydration-terminals) 空正文（content_html NULL）→ bodyById ready 终态（content 空、无错误文案），卡片不再显示加载占位',
      t103E311.content === '' && getBodyEntryT122('311')?.state === 'ready' && t103E311.message === ''
      && NEED(t103State(), '311') === false);
    // （2）部分缺行：322 无对应返回行 → missing「文章不存在」终态
    store.setState({ entries: [t103Entry(321), t103Entry(322)] });
    S.getArticlesPlan = { rows: [socialRow({ id: 321, content_html: '<p>321</p>' })] };
    store.getState().hydrateArticleContent(['321', '322']);
    await nTick(20);
    checkNew('(t103-hydration-terminals) 响应缺行 → 该 id 进 missing「文章不存在」终态（不静默留加载占位），命中行照常填充',
      bodyOfT122(t103State(), '321').content === '<p>321</p>'
      && getBodyEntryT122('322')?.state === 'missing'
      && bodyOfT122(t103State(), '322').message.includes('文章不存在')
      && NEED(t103State(), '322') === false);
    // （3）空 rows：整批 missing（空 ids/空 rows 不留占位）
    store.setState({ entries: [t103Entry(331)] });
    S.getArticlesPlan = { rows: [] };
    store.getState().hydrateArticleContent(['331']);
    await nTick(20);
    checkNew('(t103-hydration-terminals) 空 rows → 整批进 missing「文章不存在」终态（空 ids/空 rows 不留占位）',
      getBodyEntryT122('331')?.state === 'missing'
      && bodyOfT122(t103State(), '331').message.includes('文章不存在') && NEED(t103State(), '331') === false);
    // （4）失败：failed 终态 + 错误文案可见 + retryHydration 内联重试收敛为成功
    store.setState({ entries: [t103Entry(341)] });
    S.getArticlesPlan = { mode: 'reject', error: { message: 'IPC 超时' } };
    store.getState().hydrateArticleContent(['341']);
    await nTick(20);
    checkNew('(t103-hydration-terminals) 请求失败 → failed 终态保留原错误信息（内联重试入口可用）',
      getBodyEntryT122('341')?.state === 'failed' && bodyOfT122(t103State(), '341').message === 'IPC 超时');
    S.getArticlesPlan = { rows: [socialRow({ id: 341, content_html: '<p>341 重试成功</p>' })] };
    store.getState().retryHydration('341');
    await nTick(20);
    checkNew('(t103-hydration-terminals) retryHydration 后正文填充、failed 态清除、ready 终态落位',
      bodyOfT122(t103State(), '341').content === '<p>341 重试成功</p>'
      && getBodyEntryT122('341')?.state === 'ready' && bodyOfT122(t103State(), '341').message === '');
    S.getArticlesPlan = null;

    /* ---------- t103-race-and-dedup：在途去重 + 乱序/过期防护 ---------- */
    // 在途去重：请求未落地时重复入队（重触发/重挂载/直接调用）→ 不产生第二次 IPC
    await resetStore();
    store.setState({ entries: [t103Entry(351), t103Entry(352)] });
    S.getArticlesPlan = { mode: 'defer' };
    S.invokeCalls.length = 0;
    store.getState().hydrateArticleContent(['351', '352']);
    await nTick(0);
    store.getState().hydrateArticleContent(['351', '352']); // 在途重复入队
    store.getState().ensureArticleContent('351');           // 挂载路径重复入队
    await nTick(0);
    const t103DedupCalls = S.invokeCalls.filter((c) => c.cmd === 'get_articles');
    checkNew('(t103-race-and-dedup) 同 id 在途重复入队不重复 IPC（仅首批一次、含两个 id）',
      S.pendingGetArticles.length === 1 && t103DedupCalls.length === 1
      && t103DedupCalls[0]?.args.ids.join(',') === '351,352');
    S.pendingGetArticles[0].resolve([socialRow({ id: 351, content_html: '<p>351 正文</p>' }), socialRow({ id: 352 })]);
    await nTick(10);
    checkNew('(t103-race-and-dedup) 在途去重不丢结果：351 填充正文、352 空正文 ready 终态（TASK-122：真值在 bodyById）',
      bodyOfT122(t103State(), '351').content === '<p>351 正文</p>'
      && getBodyEntryT122('352')?.state === 'ready');
    // 乱序防护：批量在途期间条目已经他路水合（selectArticle 详情）→ 迟到响应不覆盖新正文
    await resetStore();
    store.setState({ entries: [t103Entry(361)] });
    S.getArticlesPlan = { mode: 'defer' };
    S.invokeCalls.length = 0;
    store.getState().hydrateArticleContent(['361']);
    await nTick(0);
    S.detailImpl = (id) => socialRow({ id, content_html: '<p>详情路径正文</p>', url: 'https://example.com/361' });
    store.getState().selectArticle('361'); // 详情路径先落地
    await nTick(10);
    const t103DetailContent = bodyOfT122(t103State(), '361').content;
    S.pendingGetArticles[0].resolve([socialRow({ id: 361, content_html: '<p>旧批次正文</p>' })]);
    await nTick(10);
    checkNew('(t103-race-and-dedup) 旧响应不覆盖新状态：他路（详情）已水合的正文不被迟到批次改写，ready 终态保持（TASK-122：记录状态守卫）',
      t103DetailContent === '<p>详情路径正文</p>'
      && bodyOfT122(t103State(), '361').content === '<p>详情路径正文</p>'
      && getBodyEntryT122('361')?.state === 'ready');
    S.getArticlesPlan = null;
    S.detailImpl = (id) => mkRow({ id, content_html: '<p>详情</p>', translated_content: null });
    // 过期防护另一侧：在途期间条目被快照替换移除 → 迟到响应不写视图行
    /* 【TASK-103 → TASK-122 改动理由】旧断言锁「不写滞留 hydratedIds」——平行
       Map 的滞留标记会让条目重现时被误判已水合而正文为空（死区）。TASK-122 起
       记录按文章实体记账：迟到响应把 DB 真值落进实体缓存是**合法且有益**的
       （条目在任何快照重现都直接命中自己的正文与终态，危害形态结构消失）；
       视图行侧不受影响（该条目已不在 entries）。断言改为：视图行不复活 +
       记录按真值落账（ready）+ 重现时无需重拉。 */
    await resetStore();
    store.setState({ entries: [t103Entry(371)] });
    S.getArticlesPlan = { mode: 'defer' };
    S.invokeCalls.length = 0;
    store.getState().hydrateArticleContent(['371']);
    await nTick(0);
    store.setState({ entries: [t103Entry(372)] }); // 快照替换：371 不在新快照
    S.pendingGetArticles[0].resolve([socialRow({ id: 371, content_html: '<p>迟到正文</p>' })]);
    await nTick(10);
    checkNew('(t103-race-and-dedup) 在途期间条目被快照替换移除：视图行不复活；迟到行真值落实体缓存（ready），条目重现即命中正文无需重拉（TASK-122 实体缓存语义）',
      !t103State().entries.some((a) => a.id === '371')
      && bodyOfT122(t103State(), '371').content === '<p>迟到正文</p>'
      && getBodyEntryT122('371')?.state === 'ready');
    S.getArticlesPlan = null;
  }

  /* ============================================================
     TASK-122（审计 P2-3）：正文/AI 实体缓存分离与显式失效
     —— 审计探针 P3/P4 本体转真实行为回归 + bodyById 状态机契约。
     探针 P3：清理 AI 缓存把 DB ai_summary/translated_content 置 NULL → reload
     列表行空摘要被转 '' → merge `a.aiSummary || prev.aiSummary` 复活旧值
     （UI 与 DB 分离，实测 old summary/old translation）。修后：merge 不再继承
     （视图行取行真值），bodyById 记录由 reconcileBodyEntities 按行真值显式失效
     （cleared + bump contentRevision）。
     探针 P4：列表 snippet 已更新时正文仍为 old body、entryNeedsHydration=false
     不重取。修后：行 snippet 相对被替换视图行变化 → 记录失效（回未请求态）
     → 懒水合重取。
     失效链路：CacheCleanupSection.run → api.cacheCleanup(…,'ai') 成功 →
     reloadFromBackend（该 reload 的行即清理后 DB 真值）→ mergeSnapshotEntries
     (fromBackend=true) → bodyCache.reconcileBodyEntities（机制与状态机单点
     文档见 src/store/bodyCache.ts 模块头注）。本组断言驱动同一 store 级链路。
     ============================================================ */
  {
    const {
      BODY_CACHE_MAX, getBodyEntry: REC, resetBodyCacheForTests,
      markBodyLoading: markLoading122, applyBodyRow: applyRow122, applyAiProduct: applyAi122,
    } = await import('../../dist-test/store/bodyCache.js');
    const NEED122 = nSel.entryNeedsHydration;
    const socialRow122 = (o) => mkRow({ feed_id: 11, ...o });
    const t122Entry = (id, extra = {}) => ({
      id: String(id), feedId: '11', title: 't122 帖', publishedAt: Date.now(), isRead: false,
      isStarred: false, tags: [], source: 'direct', snippet: 's', author: 'a',
      translatedContent: '', aiSummary: '',
      ...extra,
    });

    /* ---------- (t122-1) 清理 AI 缓存 → UI 空/已清空而非旧值（探针 P3 本体） ---------- */
    await bootFixture({ activeContentLayout: 'social' });
    S.backendRows = BASE_ROWS.map((r) => (r.id === 103
      ? { ...r, ai_summary: '旧摘要', translated_content: '<p>旧译文</p>' }
      : r));
    await store.getState().reloadFromBackend(); // 行带旧产物（与 DB 一致的起点）
    S.getArticlesPlan = { rows: [socialRow122({ id: 103, content_html: '<p>103 正文</p>', ai_summary: '旧摘要', translated_content: '<p>旧译文</p>' })] };
    store.getState().hydrateArticleContent(['103']);
    await nTick(20);
    S.getArticlesPlan = null;
    const t122revBefore = REC('103')?.contentRevision ?? 0;
    checkNew('(t122-1) 前置：水合后记录 ready 且携带旧摘要/旧译文（清理前基线）',
      REC('103')?.state === 'ready'
      && bodyOfT122(store.getState(), '103').aiSummary === '旧摘要'
      && bodyOfT122(store.getState(), '103').translatedContent === '<p>旧译文</p>');
    /* 清理 AI 缓存链路：后端置 NULL → 清理动作完成后的 reload 带回 NULL 行 */
    S.backendRows = BASE_ROWS.map((r) => (r.id === 103 ? { ...r, ai_summary: null, translated_content: null } : r));
    await store.getState().reloadFromBackend();
    const t122c1 = store.getState();
    checkNew('(t122-1) 清理 AI 缓存 → 记录置 cleared + bump contentRevision（正文保留），AI 产物呈现空而非旧值（探针 P3 修复本体）',
      REC('103')?.state === 'cleared'
      && (REC('103')?.contentRevision ?? 0) === t122revBefore + 1
      && bodyOfT122(t122c1, '103').aiSummary === '' && bodyOfT122(t122c1, '103').translatedContent === ''
      && bodyOfT122(t122c1, '103').content === '<p>103 正文</p>');
    checkNew('(t122-1) 清理后视图行 AI 列 = 行真值（空串），merge 不再从旧快照行复活旧产物',
      t122c1.entries.find((a) => a.id === '103')?.aiSummary === ''
      && t122c1.entries.find((a) => a.id === '103')?.translatedContent === '');
    /* cleared 不被 reload 复活：先用清理后行再刷一次；再用「清理前旧行」刷一次 */
    await store.getState().reloadFromBackend();
    checkNew('(t122-1) cleared 态不被后续 reload 复活（行 NULL → 保持 cleared）',
      REC('103')?.state === 'cleared' && bodyOfT122(store.getState(), '103').aiSummary === '');
    S.backendRows = BASE_ROWS.map((r) => (r.id === 103
      ? { ...r, ai_summary: '旧摘要', translated_content: '<p>旧译文</p>' }
      : r));
    await store.getState().reloadFromBackend(); // 模拟「清理前抓取的在途 reload」迟到落地
    checkNew('(t122-1) 清理前的旧行迟到落地也不复活 cleared（显式态只能被再次生成解除）',
      REC('103')?.state === 'cleared' && bodyOfT122(store.getState(), '103').aiSummary === '');
    /* 再次生成（AI slice 落账路径）是唯一解除路径 */
    S.aiSum = { deltas: ['新'], error: null, reject: null, finish: true, holdIds: [] };
    store.getState().summarizeEntry('103');
    await nTick(20);
    checkNew('(t122-1) cleared 后重新生成摘要 → applyAiProduct 解除 cleared 回 ready（唯一解除路径）',
      REC('103')?.state === 'ready' && bodyOfT122(store.getState(), '103').aiSummary === '新');
    S.aiSum = { deltas: [], error: null, reject: null, finish: true, holdIds: [] };
    S.backendRows = BASE_ROWS;

    /* ---------- (t122-2) snippet 更新 → 正文重取（探针 P4 本体） ---------- */
    await bootFixture({ activeContentLayout: 'social' });
    S.getArticlesPlan = { rows: [socialRow122({ id: 104, content_html: '<p>旧正文</p>', snippet: '旧摘要行' })] };
    store.getState().hydrateArticleContent(['104']);
    await nTick(20);
    S.getArticlesPlan = null;
    checkNew('(t122-2) 前置：记录 ready、正文已加载、无需水合',
      REC('104')?.state === 'ready' && bodyOfT122(store.getState(), '104').content === '<p>旧正文</p>'
      && NEED122(store.getState(), '104') === false);
    /* 源站重新同步了该文章：正文变 → 列表行 snippet 随之更新 → reload 带回新行 */
    S.backendRows = BASE_ROWS.map((r) => (r.id === 104 ? { ...r, snippet: '新摘要行' } : r));
    await store.getState().reloadFromBackend();
    checkNew('(t122-2) 行 snippet 变化 → 记录失效（回未请求态），懒水合判定重新成立（探针 P4：正文不再陈旧）',
      REC('104') === undefined && NEED122(store.getState(), '104') === true
      && store.getState().entries.find((a) => a.id === '104')?.snippet === '新摘要行');
    S.getArticlesPlan = { rows: [socialRow122({ id: 104, content_html: '<p>新正文</p>', snippet: '新摘要行' })] };
    store.getState().hydrateArticleContent(['104']);
    await nTick(20);
    S.getArticlesPlan = null;
    checkNew('(t122-2) 重取落新正文（失效后可重取，新行真值进记录）',
      REC('104')?.state === 'ready' && bodyOfT122(store.getState(), '104').content === '<p>新正文</p>');
    S.backendRows = BASE_ROWS;

    /* ---------- (t122-3) '' vs cleared 语义区分 ---------- */
    await bootFixture({ activeContentLayout: 'social' });
    S.backendRows = [...BASE_ROWS,
      mkRow({ id: 111, feed_id: 11, ai_summary: '待清理摘要' }),
      mkRow({ id: 112, feed_id: 11 })];
    await store.getState().reloadFromBackend(); // 111/112 进当前视图（水合前提：条目在册）
    S.getArticlesPlan = { rows: [
      socialRow122({ id: 111, content_html: '<p>111 正文</p>', ai_summary: '待清理摘要' }),
      socialRow122({ id: 112, content_html: '<p>112 正文</p>' }),
    ] };
    store.getState().hydrateArticleContent(['111', '112']);
    await nTick(20);
    S.getArticlesPlan = null;
    S.backendRows = [...BASE_ROWS,
      mkRow({ id: 111, feed_id: 11, ai_summary: null }),
      mkRow({ id: 112, feed_id: 11 })];
    await store.getState().reloadFromBackend(); // 111 的摘要被清理；112 本就没有摘要
    const t122b111 = bodyOfT122(store.getState(), '111');
    const t122b112 = bodyOfT122(store.getState(), '112');
    checkNew('(t122-3) 「已清空」（had → cleared，可呈现已清空提示）与「从未生成」（ready 且 AI 空）是两个可区分状态',
      t122b111.state === 'cleared' && t122b111.aiSummary === ''
      && t122b112.state === 'ready' && t122b112.aiSummary === '');
    /* 无记录（未请求）也是第三种可区分形态：AI 列回退视图行（行真值） */
    store.setState({ entries: [...store.getState().entries, t122Entry(113)] });
    const t122bUnreq = bodyOfT122(store.getState(), '113');
    checkNew('(t122-3) 无记录（未请求）= 第三种形态：state unrequested、AI 列回退视图行 DB 真值',
      t122bUnreq.state === 'unrequested'
      && t122bUnreq.aiSummary === (store.getState().entries.find((a) => a.id === '113')?.aiSummary ?? ''));
    S.backendRows = BASE_ROWS;

    /* ---------- (t122-4) 内存预算：LRU 淘汰 → 回未请求态可重取 ---------- */
    await resetStore({ activeContentLayout: 'social' });
    checkNew('(t122-4) 预算常量：BODY_CACHE_MAX = 2000（模块头注「内存预算」的锁定值）',
      BODY_CACHE_MAX === 2000);
    resetBodyCacheForTests();
    const t122evictRow = { content_html: '<p>预算</p>' };
    const t122evBase = 900000;
    for (let i = 0; i < BODY_CACHE_MAX; i += 1) {
      const eid = String(t122evBase + i);
      markLoading122(eid);
      applyRow122(eid, t122evictRow, 0, { aiSummary: '', translatedContent: '' });
    }
    /* 插入第 2001 条 → 最久未使用（id 900000）被淘汰；期间不做任何读取
       （getBodyEntry 命中会刷新 LRU 新鲜度，影响淘汰序） */
    markLoading122(String(t122evBase + BODY_CACHE_MAX));
    applyRow122(String(t122evBase + BODY_CACHE_MAX), t122evictRow, 0, { aiSummary: '', translatedContent: '' });
    checkNew('(t122-4) 超限插入 → 最久未使用条目被淘汰（LRU；上限恰 2000 条）',
      REC(String(t122evBase)) === undefined
      && REC(String(t122evBase + 1))?.state === 'ready'
      && REC(String(t122evBase + BODY_CACHE_MAX))?.state === 'ready');
    /* 淘汰 = 回未请求态 → 可重取（懒水合判定重新成立 + IPC 重新发生） */
    store.setState({ entries: [t122Entry(t122evBase)] });
    checkNew('(t122-4) 被淘汰条目回未请求态：懒水合判定为真（淘汰无正确性影响，只付一次 IPC）',
      NEED122(store.getState(), String(t122evBase)) === true);
    S.getArticlesPlan = { rows: [socialRow122({ id: t122evBase, content_html: '<p>重取正文</p>' })] };
    store.getState().hydrateArticleContent([String(t122evBase)]);
    await nTick(20);
    S.getArticlesPlan = null;
    checkNew('(t122-4) 淘汰后重取成功（新记录落位）',
      REC(String(t122evBase))?.state === 'ready'
      && bodyOfT122(store.getState(), String(t122evBase)).content === '<p>重取正文</p>');
    resetBodyCacheForTests();

    /* ---------- (t122-5) 快照替换 bodyById 引用稳定（正文随实体，不随快照行） ---------- */
    await bootFixture({ activeContentLayout: 'social' });
    S.backendRows = [...BASE_ROWS, mkRow({ id: 121, feed_id: 11, ai_summary: '121 摘要' })];
    await store.getState().reloadFromBackend(); // 121 进当前视图
    S.getArticlesPlan = { rows: [socialRow122({ id: 121, content_html: '<p>121 正文</p>', ai_summary: '121 摘要' })] };
    store.getState().hydrateArticleContent(['121']);
    await nTick(20);
    S.getArticlesPlan = null;
    const t122recBefore = REC('121');
    await store.getState().reloadFromBackend(); // 行真值与记录一致 → 对齐为 no-op，引用稳定
    store.getState().selectFeed('11');
    await nTick(30);
    store.getState().selectFeed('all');
    checkNew('(t122-5) 快照替换/缓存恢复不触碰 bodyById：记录引用稳定、正文与 AI 产物原样（零补拉，无死区）',
      REC('121') === t122recBefore
      && bodyOfT122(store.getState(), '121').content === '<p>121 正文</p>'
      && bodyOfT122(store.getState(), '121').aiSummary === '121 摘要');

    /* ---------- (t122-6) 水合死区回归（审计探针场景）：不存在「正文缺失 + 无记录 + 无请求」组合 ---------- */
    await bootFixture({ activeContentLayout: 'social' });
    S.backendRows = [...BASE_ROWS, mkRow({ id: 131, feed_id: 11 })];
    await store.getState().reloadFromBackend(); // 131 进当前视图
    S.getArticlesPlan = { mode: 'defer' };
    store.getState().hydrateArticleContent(['131']);
    await nTick(0);
    const t122deadLoading = NEED122(store.getState(), '131');
    checkNew('(t122-6) 在途 = loading 记录可见：水合判定为假（重复入队被状态拦下），卡片有「请求在途」可依赖',
      REC('131')?.state === 'loading' && t122deadLoading === false);
    S.pendingGetArticles.at(-1)?.resolve([socialRow122({ id: 131, content_html: '<p>131 正文</p>' })]);
    await nTick(10);
    S.getArticlesPlan = null;
    await store.getState().reloadFromBackend(); // 同 ID 快照替换（审计死区场景：刷新后不再有请求也不再重触发）
    const t122deadCalls = S.invokeCalls.filter((c) => c.cmd === 'get_articles').length;
    await nTick(20);
    checkNew('(t122-6) 同 ID 刷新：记录 ready + 正文可读 + 水合判定为假 + 零补拉（「加载正文…且无请求在途」死区不复发）',
      REC('131')?.state === 'ready'
      && bodyOfT122(store.getState(), '131').content === '<p>131 正文</p>'
      && NEED122(store.getState(), '131') === false
      && S.invokeCalls.filter((c) => c.cmd === 'get_articles').length === t122deadCalls);

    /* ---------- (t122-7) 失效戳守卫：显式写入后，携带旧戳的响应整体丢弃 ---------- */
    await bootFixture({ activeContentLayout: 'social' });
    S.backendRows = [...BASE_ROWS, mkRow({ id: 141, feed_id: 11, ai_summary: 'DB 旧摘要' })];
    await store.getState().reloadFromBackend(); // 141 进当前视图
    S.getArticlesPlan = { rows: [socialRow122({ id: 141, content_html: '<p>141 正文</p>', ai_summary: 'DB 旧摘要' })] };
    store.getState().hydrateArticleContent(['141']);
    await nTick(20);
    S.getArticlesPlan = null;
    /* 用户重新生成摘要（applyAiProduct bump revision）——期间一次携带旧戳的
       详情/批量响应迟到：整体丢弃，不覆盖新产物 */
    applyAi122('141', 'aiSummary', '新生成摘要');
    applyRow122('141', { content_html: '<p>141 正文</p>', ai_summary: 'DB 旧摘要' }, 0, { aiSummary: '', translatedContent: '' });
    checkNew('(t122-7) 旧戳响应丢弃：生成落账后迟到的旧行不覆盖新产物（revision 守卫）',
      REC('141')?.state === 'ready'
      && bodyOfT122(store.getState(), '141').aiSummary === '新生成摘要'
      && (REC('141')?.contentRevision ?? 0) >= 1);
    /* 清理失效（cleared，revision bump）后，旧戳响应同样丢弃 */
    S.backendRows = [...BASE_ROWS, mkRow({ id: 141, feed_id: 11, ai_summary: null })];
    await store.getState().reloadFromBackend(); // cleared + bump
    applyRow122('141', { content_html: '<p>141 正文</p>', ai_summary: 'DB 旧摘要' }, 0, { aiSummary: '', translatedContent: '' });
    checkNew('(t122-7) cleared 后旧戳响应丢弃：cleared 不被在途响应复活（清理后并发水合的复活窗口被 stamp 比对关死）',
      REC('141')?.state === 'cleared' && bodyOfT122(store.getState(), '141').aiSummary === '');
    S.backendRows = BASE_ROWS;

    /* ---------- (t122-8) 清理链路接线（源级）：CacheCleanupSection 清空动作完成后必 reload ---------- */
    const fs122 = await import('node:fs');
    const ccSrc122 = fs122.readFileSync(new URL('../../src/components/settings/CacheCleanupSection.tsx', import.meta.url), 'utf8');
    checkNew('(t122-8) 清理 AI 缓存链路（源级）：cacheCleanup 成功 → reloadFromBackend（其落地行真值驱动 bodyById 显式失效），注释载明机制单点',
      ccSrc122.includes('api.cacheCleanup(days, scope)')
      && ccSrc122.includes('await reloadFromBackend()')
      && ccSrc122.includes('reconcileBodyEntities'));

    /* ---------- (t122-9) 源级锁：订阅 selectArticleBody 必须包 useShallow ---------- */
    /* 依据（P0 运行期缺陷，真实 headless Chrome 复现）：selectArticleBody 在有记录时
       每次调用都经 bodyViewFrom(rec) 返回**新对象**；zustand v5 的 useStore
       （node_modules/zustand/react.js）把 selector 直接交给
       React.useSyncExternalStore 且不做快照缓存 → React 每轮 getSnapshot 都拿到新
       引用 → console.error「The result of getSnapshot should be cached to avoid an
       infinite loop」→「Uncaught Error: Maximum update depth exceeded」（React
       #185）。Reader 挂载（打开任意文章）与社交/通知卡渲染都会崩。
       约定与 Sidebar.tsx 既有注记一致：返回新引用的派生 selector 必须包 useShallow。
       本断言为**补充性静态锁**（权威证明是管理器跑的真实浏览器检查）：对 src/ 全
       树做去注释/去字符串后的**括号配平**扫描，取出每个 useAppStore(...) 的完整
       实参，凡实参中出现 selectArticleBody 者必须形如 useShallow(...)。空白容忍
       （缩进/换行/括号内空白任意），不锁死措辞；删掉 useShallow 或新写一处裸订阅
       必红。 */
    const maskCode122 = (src) => {
      let out = '';
      let i = 0;
      while (i < src.length) {
        const c = src[i];
        const n = src[i + 1];
        if (c === '/' && n === '/') { // 行注释整体置空（避免注释里的 useAppStore( 误命中）
          const e = src.indexOf('\n', i);
          const end = e === -1 ? src.length : e;
          out += ' '.repeat(end - i);
          i = end;
          continue;
        }
        if (c === '/' && n === '*') { // 块注释整体置空
          const e = src.indexOf('*/', i + 2);
          const end = e === -1 ? src.length : e + 2;
          out += ' '.repeat(end - i);
          i = end;
          continue;
        }
        if (c === '"' || c === "'" || c === '`') { // 字符串内部置空，保留引号
          let j = i + 1;
          let closed = false;
          while (j < src.length) {
            if (src[j] === '\\') { j += 2; continue; }
            if (src[j] === c) { j += 1; closed = true; break; }
            j += 1;
          }
          out += c + ' '.repeat(Math.max(0, j - i - (closed ? 2 : 1))) + (closed ? c : '');
          i = j;
          continue;
        }
        out += c;
        i += 1;
      }
      return out;
    };
    const scanBodySubs122 = (src) => {
      const code = maskCode122(src);
      const NEEDLE = 'useAppStore(';
      const found = [];
      let i = 0;
      while ((i = code.indexOf(NEEDLE, i)) !== -1) {
        const open = i + NEEDLE.length - 1; // '(' 的下标
        let depth = 0;
        let j = open;
        for (; j < code.length; j += 1) {
          if (code[j] === '(') depth += 1;
          else if (code[j] === ')') { depth -= 1; if (depth === 0) break; }
        }
        const arg = code.slice(open + 1, j);
        i = j + 1;
        if (/\bselectArticleBody\b/.test(arg)) {
          found.push({ arg: arg.replace(/\s+/g, ' ').trim(), wrapped: /^useShallow\s*\(/.test(arg.trim()) });
        }
      }
      return found;
    };
    const walk122 = (dir) => {
      const files = [];
      for (const e of fs122.readdirSync(dir, { withFileTypes: true })) {
        const child = new URL(e.isDirectory() ? `${e.name}/` : e.name, dir);
        if (e.isDirectory()) files.push(...walk122(child));
        else if (/\.tsx?$/.test(e.name)) files.push(child);
      }
      return files;
    };
    const subs122 = [];
    for (const f of walk122(new URL('../../src/', import.meta.url))) {
      for (const s of scanBodySubs122(fs122.readFileSync(f, 'utf8'))) {
        subs122.push({ where: `${f.pathname.split('/src/').pop()} → ${s.arg}`, wrapped: s.wrapped });
      }
    }
    const bare122 = subs122.filter((s) => !s.wrapped);
    if (bare122.length) console.error('裸订阅（缺 useShallow）:', bare122.map((b) => b.where).join(' | '));
    checkNew('(t122-9) selectArticleBody 订阅必须包 useShallow（源级锁）：src/ 全树每个引用 selectArticleBody 的 useAppStore(...) 实参均以 useShallow(...) 包裹（裸订阅 = zustand v5 快照不缓存 → React #185 无限重渲染；至少覆盖 Reader/SocialCard/NotifCard 三处）',
      subs122.length >= 3 && bare122.length === 0);
  }
}
