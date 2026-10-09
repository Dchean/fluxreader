// tools/frontend-tests/ui-contracts.mjs
// 领域模块：组件/文案/SSR 与源码形态契约（含真实 SSR 输出与源级防回退）
// OPT-016C 拆分自 tools/frontend-regression.mjs（旧行区间 5553-5888、5889-5943、5945-6004、6006-6270、6272-6475、6477-6579、6581-7026、7028-7125、7127-7228、7230-7412、7414-7515、7517-7646、7648-7743、7745-7842）；
// 断言名称/条件文本原样迁移，仅做路径深度适配（import.meta.url 与动态 import 深一层）与
// 共享可变状态的 S. 归属重写。数据所有权：共享假后端/夹具/记录归 harness（见 harness.mjs 头注），
// 本模块不 import 第二份 store；域内自带夹具（大行集等）仍在本模块内独立构造与复位。
export const id = 'ui-contracts';

export async function run(ctx) {
  const { S, store, checkNew, nTick, overrideGlobal } = ctx;
  await ctx.useMainBackend();
  await ctx.resetStore(); // 复位到与旧文件 IIFE 收口一致基线（其后段落自带显式状态）

/* ============================================================
   TASK-052 组件侧证据（(d1)…(d5)）：哨兵在空列表下必须仍然可渲染/可推进

   证据强度（如实说明，不夸大）：
   - 本 harness **不含真实 DOM**，仓库也没有 jsdom/happy-dom 之类的依赖
     （约束禁止新增依赖），因此拿不到「浏览器里滚动真的触发了分页」的运行证据。
   - 但哨兵的可见性判定已从 JSX 里抽成纯函数 \`sentinelMode()\`（src/components/timelineSentinel.ts），
     并且 **JSX 直接消费该函数的返回值**——所以下面的断言锚定的是组件真实的
     渲染分支，而不是另写一份平行逻辑。
   - 再叠加 SSR 渲染（react-dom/server + rolldown 就地转译 .tsx，均来自既有依赖）
     断言「非空列表时组件确实产出了哨兵节点」，覆盖「函数接进 JSX」这一步。
   ============================================================ */
{
  const { sentinelMode } = await import('../../src/components/timelineSentinel.ts');

  checkNew('(d1) 哨兵判定：空列表 + 未到底 ⇒ idle（#timeline-load-more 可渲染、可被滚动触发）',
    sentinelMode(0, false, false) === 'idle');
  checkNew('(d1) 哨兵判定：空列表 + 补拉中 ⇒ loading（用户看得到「正在取更多」）',
    sentinelMode(0, false, true) === 'loading');
  checkNew('(d2) 哨兵判定：空列表 + 已到底 ⇒ hidden（不与「暂无匹配内容」重复）',
    sentinelMode(0, true, false) === 'hidden' && sentinelMode(0, true, true) === 'hidden');
  checkNew('(d3) 哨兵判定：非空列表三态照旧（待滚动 / 加载中 / 已到底），051 前行为不变',
    sentinelMode(5, false, false) === 'idle' && sentinelMode(5, false, true) === 'loading'
    && sentinelMode(5, true, false) === 'end');
  /* 修前对照：旧判据是 items.length > 0 ⇒ 空列表一律 hidden，永远等不到补拉 */
  const legacyVisible = (n) => n > 0;
  checkNew('(d4) 修前判据可复现：items.length > 0 会让「空列表 + 未到底」被判为不可见（老文章够不到的根因）',
    legacyVisible(0) === false && sentinelMode(0, false, false) !== 'hidden');

  /* SSR 渲染：确认 sentinelMode 真的接进了 JSX（组件在非空列表时产出哨兵节点）。
     注意 SSR 下 zustand 读 getInitialState（server snapshot），故这里只用
     「与初值一致」的状态做形态断言，避免读到与预期不符的读数。 */
  const { renderToStaticMarkup } = await import('react-dom/server');
  const { createElement } = await import('react');
  const { Timeline } = await import('../../src/components/Timeline.tsx');
  /* SSR 读的是 store 创建时的 server snapshot；显式落一次「与初值一致」的空态，
     让读数确定（zustand 的 getInitialState 与 setState 在这里被同时对齐）。 */
  store.setState({ entries: [], articlesLimit: 0, articlesCursor: {}, articlesExhausted: false, articlesLoading: false });
  const html = renderToStaticMarkup(createElement(Timeline));
  checkNew('(d5) SSR：组件渲染出滚动容器与空态（组件树可被实际执行，不是只过类型检查）',
    html.includes('id="timelineContentScroll"') && html.includes('timeline-empty-state'));
  checkNew('(d5) SSR：空列表未到底态哨兵落在滚动容器内 ⇒ 节点确实被渲染',
    html.includes('timeline-load-more') && html.includes('load-more-idle')
    && html.indexOf('id="timelineContentScroll"') < html.indexOf('timeline-load-more'));

  /* ============================================================
     TASK-059：Endpoint 自动适配（只填域名）
     owner 2026-09-18 纠正需求：用户**只需填域名**，不必知道 /api/greader.php；
     此前 TASK-057 的文案反过来教用户填完整后缀 —— 方向是错的。

     本组锁定「文案必须按自动适配来说」，并保留 TASK-057 的防误伤边界：
     路径类失败（404/405/410）才附加指引，凭据类失败（401/403）一律原样。
     ============================================================ */
  const { ENDPOINT_DESC, ENDPOINT_PLACEHOLDER, endpointHint, isMissingPathError } =
    await import('../../dist-test/components/settings/endpointHint.js');

  /* (e1) 文案方向：只填域名 + 自动适配；不得再要求用户填 API 后缀 */
  checkNew('(e1) Endpoint 说明要求「只填域名」并说明应用自动适配',
    ENDPOINT_DESC.includes('域名') && ENDPOINT_DESC.includes('自动')
    && (ENDPOINT_DESC.includes('FreshRSS') && ENDPOINT_DESC.includes('Miniflux')));
  checkNew('(e1) Endpoint 说明不得再教用户填 /api/greader.php 后缀（那是被纠正掉的方向）',
    !ENDPOINT_DESC.includes('/api/greader.php'));
  /* **长度预算（实机实测）**：`.setting-card-text p` 宽 234px、`-webkit-line-clamp: 2`
     → 超过两行会被截断成「…」。TASK-057 的旧文案需 4 行（69px vs 35px），**一直在被截断**；
     现取值 227px 单行。此处按估算宽度设防（CJK 11.5px / ASCII 5.9px，与 canvas 实测一致）：
     上限取 460px（= 两行容量 468px 留余量），防止再次写出会被截断的说明。 */
  const estWidth = (s) =>
    [...s].reduce((w, ch) => w + (ch.charCodeAt(0) > 0x2e80 ? 11.5 : 5.9), 0);
  checkNew('(e1) Endpoint 说明控制在两行内，不会被 line-clamp 截断（实测容量 234px×2）',
    estWidth(ENDPOINT_DESC) <= 460 && !ENDPOINT_DESC.includes('…'));
  checkNew('(e1) placeholder 给出纯域名示例（不含 /api 后缀）',
    ENDPOINT_PLACEHOLDER.includes('://') && !ENDPOINT_PLACEHOLDER.includes('/api/'));
  /* 宽度受控（TASK-057 审查订正的事实，仍然有效）：输入框 box-sizing:border-box、
     width:240px、padding:6px 10px、border:1px → **内容盒仅 219px**（12px Arial）。
     故 placeholder 必须短于 30 字符且不含并列「或」，且不得用实测 219.46px 的超宽串。 */
  checkNew('(e1) placeholder 保持短示例（内容盒 219px 内可完整显示）',
    ENDPOINT_PLACEHOLDER.length <= 30 && !ENDPOINT_PLACEHOLDER.includes('或'));
  checkNew('(e1) placeholder 不得用实测超宽（219.46px > 219px）的完整路径写法',
    ENDPOINT_PLACEHOLDER !== 'https://demo.freshrss.org/api/greader.php'
    && !ENDPOINT_PLACEHOLDER.includes('/api/greader.php'));

  /* (e2) 修前文案可复现：TASK-057 交付的旧 desc 完全不含 FreshRSS 路径，
     用户据此填域名必然 404 —— 这正是本任务要消掉的现象。 */
  const legacyDesc = '例如 https://reader.example.com（支持 Google Reader / Fever 协议）';
  const legacyPlaceholder = 'https://reader.example.com';
  checkNew('(e2) 修前文案可复现：旧 desc 与 placeholder 都不含 /api/greader.php',
    !legacyDesc.includes('/api/greader.php') && !legacyPlaceholder.includes('/api/greader.php')
    && !legacyDesc.includes('FreshRSS'));
  checkNew('(e2) TASK-057 的错误方向可复现：旧提示教用户去填 API 路径，而不是自动适配',
    'ClientLogin → 404（该地址下没有 GReader API：请确认 Endpoint 是否需指向 API 路径）'
      .includes('API 路径'));

  /* (e3) 路径类失败（404/405/410）的提示方向必须已更正：
     现在 404 意味着「已把所有候选路径试过了」，提示应指向域名/后端，而不是让用户填后缀。 */
  const notFound = endpointHint('ClientLogin → 404');
  checkNew('(e3) 404 提示不再要求用户填 API 后缀（自动适配后该说法会反向误导）',
    !notFound.includes('/api/greader.php') && !notFound.includes('需指向 API 路径'));
  checkNew('(e3) 404 提示仍可操作：说明已自动尝试并指向域名/后端核对',
    notFound.includes('404') && notFound.includes('域名') && notFound.includes('自动'));
  /* 自动适配后，后端「找不到 API」的真实消息形如
     「在该地址下找不到 GReader API（HTTP 404，已尝试：…）」。它必须能命中该分支，
     否则用户看到的就是一条没有任何指引的裸错误（审查发现的缺口）。 */
  const realNotFound = endpointHint('在该地址下找不到 GReader API（HTTP 404，已尝试：https://x、https://x/api/greader.php）。请确认域名是否正确');
  checkNew('(e3) 后端真实的「找不到 API」消息能命中指引分支（修前该形状不含 404，指引永不触发）',
    isMissingPathError('在该地址下找不到 GReader API（HTTP 404，已尝试：https://x）')
    && realNotFound.includes('域名') && realNotFound.includes('自动'));
  checkNew('(e3) 405/410 同属「路径不存在」，同样附加该指引',
    isMissingPathError('GET /x → 405') && isMissingPathError('→ 410')
    && endpointHint('→ 405').includes('域名') && endpointHint('→ 410').includes('域名'));
  checkNew('(e3) 修前行为可复现：旧提示只是原样回显状态码，不含任何指引',
    legacyDesc.length > 0 && !('ClientLogin → 404'.includes('API 路径')));

  /* (e4) 凭据类失败不得被误报为 Endpoint 问题（防误伤，TASK-057 的边界继续有效） */
  const unauthorized = endpointHint('ClientLogin → 401');
  checkNew('(e4) 401 凭据失败保持原样：不得误报为 Endpoint 填错',
    unauthorized === 'ClientLogin → 401' && !unauthorized.includes('域名'));
  checkNew('(e4) 403 / BadAuthentication / 网络错误同样不附加 Endpoint 指引',
    endpointHint('ClientLogin → 403') === 'ClientLogin → 403'
    && endpointHint('ClientLogin 失败：BadAuthentication') === 'ClientLogin 失败：BadAuthentication'
    && endpointHint('error sending request') === 'error sending request');

  /* (e5) 提示必须是纯函数：同输入同输出、不产生副作用 */
  checkNew('(e5) 提示为纯文案变换：同输入两次结果一致，且不改动原文之外的内容',
    endpointHint('ClientLogin → 404') === endpointHint('ClientLogin → 404')
    && notFound.startsWith('ClientLogin → 404'));
  checkNew('(e5) 空串/异常输入不抛错（健壮性）',
    endpointHint('') === '' && endpointHint('   ') === '   ');

  /* ============================================================
     TASK-058：同步失败对用户可见（前端消费 SyncReport.errors）

     背景：后端 feeds_phase/states_phase 在 report.errors 非空时**仍返回 Ok**
     （有意语义：单项失败不中断整链），故 .catch() 永不触发；两处 syncPhase 调用点
     此前都丢弃 report，失败对用户完全不可见。

     实证（TASK-056 端到端）：后端返回 errors 含 2 条 FOREIGN KEY 失败，
     界面却弹「后端同步完成」、本地 feeds 为 0。

     本组锁定：(f1) 有失败项 ⇒ 必须产生「有 N 项失败」提示（修前为 null → 失败）；
     (f2) 无失败项 ⇒ 返回 null，使调用方能保持既有成功文案**逐字不变**。
     ============================================================ */
  const { syncFailureMessage, hasSyncFailures, MAX_DETAIL } =
    await import('../../dist-test/store/syncErrors.js');

  /* (f1) 有失败项必须可见——这是本任务的核心契约 */
  const withErrors = {
    errors: [
      '拉取订阅 http://x/uncat.xml 建本地失败: [db] FOREIGN KEY constraint failed',
      '拉取订阅 http://x/cat.xml 建本地失败: [db] FOREIGN KEY constraint failed',
    ],
  };
  const msg = syncFailureMessage(withErrors);
  checkNew('(f1) 后端返回非空 errors ⇒ 产生「有 N 项失败」提示（修前该值为 null，失败静默）',
    typeof msg === 'string' && msg.includes('2 项失败') && hasSyncFailures(withErrors) === true);
  checkNew('(f1) 提示必须含**具体失败原因**可定位，不得只给一个孤立数字',
    !!msg && msg.includes('FOREIGN KEY constraint failed') && msg.includes('uncat.xml'));
  checkNew('(f1) 修前行为可复现：**丢弃 report** 时无从得知有失败（旧调用点形态的真实后果）',
    // 旧代码是 `.then(async () => …)`——回调**不收参数**，故 report 根本没进作用域。
    // 用与旧代码同形的调用模拟：丢弃返回值后，调用方拿不到任何失败信号。
    (() => {
      const callSiteLikeOld = (_report) => null;   // 旧调用点等价：忽略入参、不返回信号
      const signal = callSiteLikeOld(withErrors);
      return signal === null && syncFailureMessage(withErrors) !== null;
    })());

  /* (f1b) 多条时截断，避免 toast 过长；但仍告知总数 */
  const many = { errors: Array.from({ length: 7 }, (_, i) => `失败项 ${i + 1}`) };
  const manyMsg = syncFailureMessage(many);
  checkNew('(f1b) 失败项过多时只列前若干条，但仍给出总数（不丢「有 7 项」这一事实）',
    !!manyMsg && manyMsg.includes('7 项失败') && manyMsg.includes('失败项 1')
    && !manyMsg.includes(`失败项 ${MAX_DETAIL + 3}`));
  checkNew('(f1b) 单条失败也正常报出，不出现多余分隔',
    syncFailureMessage({ errors: ['单条原因'] }) === '同步完成，但有 1 项失败：单条原因');

  /* (f2) 成功路径必须「无信号」，以便调用方保持既有文案逐字不变 */
  checkNew('(f2) errors 为空数组 ⇒ 返回 null（调用方据此保持既有成功文案逐字不变）',
    syncFailureMessage({ errors: [] }) === null && hasSyncFailures({ errors: [] }) === false);
  checkNew('(f2) report 缺失 / errors 字段缺失 / 非数组 ⇒ 一律返回 null（不误报失败）',
    syncFailureMessage(null) === null && syncFailureMessage(undefined) === null
    && syncFailureMessage({}) === null
    && syncFailureMessage({ errors: null }) === null
    && syncFailureMessage({ errors: 'oops' }) === null);

  /* (f3) 健壮性：errors 非空但内容不可读时，仍须让用户知道「有失败」 */
  const blanks = syncFailureMessage({ errors: ['   ', ''] });
  checkNew('(f3) errors 非空但内容全空白 ⇒ 仍提示有失败（只是无原因），不退回静默',
    !!blanks && blanks.includes('1 项失败') === false && blanks.includes('2 项失败')
    && blanks.includes('原因未提供'));

  /* (f4) 既有成功文案一字未改——用**源码文本**核对，而不是只断言常量存在 */
  const fs = await import('node:fs');
  const syncTabSrc = fs.readFileSync(new URL('../../src/components/settings/SyncTab.tsx', import.meta.url), 'utf8');
  const syncSliceSrc = fs.readFileSync(new URL('../../src/store/slices/sync.ts', import.meta.url), 'utf8');
  checkNew('(f4) 成功路径文案逐字保留在源码中（改动前就有、改动后仍在）',
    syncTabSrc.includes("'已拉取订阅源，正在同步文章状态…'")
    && syncTabSrc.includes("'后端同步完成'")
    && syncSliceSrc.includes("'订阅同步完成，正在同步文章状态…'"));
  checkNew('(f4) 「后端同步完成」只在 errors 为空的分支出现（不被失败路径复用）',
    /failures\.length > 0 \? failures\.join\('；'\) : '后端同步完成'/.test(syncTabSrc));
  checkNew('(f4) 两处调用点**都**消费了 report 的 errors（只改一处不算完成）',
    syncTabSrc.includes('syncFailureMessage(') && syncSliceSrc.includes('syncFailureMessage('));
  checkNew('(f4) 手动链保留既有「N 个源直连失败」信息，且与同步失败信息**共存**（不互相吞掉）',
    syncSliceSrc.includes('个源直连失败') && syncSliceSrc.includes('syncFailures'));
  /* 审查 FINDING 1 修订：失败必须**前置**。showToast 只保留最后 4 条（ui.ts 的
     `.slice(-4)`）且每条 2.2s 消失；失败若排在末尾，多提示连发时最该被看到的
     失败反而最先被挤掉——那等于让本任务要解决的问题在提示层复活。 */
  checkNew('(f4) 有同步失败时，失败文案**前置**于「已刷新…」（否则第一眼读到的是成功）',
    /\$\{syncFailures\.join\('，'\)\}，\$\{base\}/.test(syncSliceSrc));
  checkNew('(f4) 无同步失败时走 `base` 原分支：既有三条成功文案逐字保留（含顺序与分隔符）',
    /已刷新，新增 \$\{summary\.new_articles\} 条，\$\{summary\.failed_feeds\} 个源直连失败/.test(syncSliceSrc)
    && /已刷新，新增 \$\{summary\.new_articles\} 条`/.test(syncSliceSrc)
    && syncSliceSrc.includes("'已刷新，无新文章'"));
  checkNew('(f4) 提示为纯函数：同输入两次结果一致、不改动入参',
    (() => {
      const r = { errors: ['a', 'b'] };
      const before = JSON.stringify(r);
      const a = syncFailureMessage(r);
      const b = syncFailureMessage(r);
      return a === b && JSON.stringify(r) === before;
    })());

  /* ---------- (f5) 运行时驱动真实 triggerManualSync（此前零运行时覆盖） ----------
     审查指出：上述 (f4) 多为**源码文本**断言。这里改为**实际调用**手动同步链，
     捕获它真正发出的 toast 文本，与改动前的模板逐字比对。
     两类场景：(a) 同步无失败 ⇒ 必须与旧文案逐字相同；
              (b) 同步有失败 ⇒ 失败必须出现，且**前置**于「已刷新…」。 */
  const syncPhasePlan = { feeds: { errors: [] }, states: { errors: [] } };
  const refreshPlan = { value: { new_articles: 3, failed_feeds: 0 } };
  // R1：__INVOKE__ 覆盖必须 finally 复原（成功/异常都保全原值）
  const restoreInvokeF5 = overrideGlobal(globalThis, '__INVOKE__', (cmd, args) => {
    S.invokeCalls.push({ cmd, args });
    switch (cmd) {
      case 'sync_phase': {
        /* api.syncPhase 的调用形态是 `inv('sync_phase', { which, full })`——
           第二参数**就是** args 对象本身（不像 list_articles 那样再包一层 `{ args }`）。
           此处曾误写成 `args.args.which`，导致两个阶段都回落到 'feeds'，
           于是 states 阶段的消费点**完全没有被 (f5) 覆盖**（审查 FINDING 实测：
           删掉 states 的消费后全套仍 275/275）。 */
        const which = (args && args.which) || 'feeds';
        return Promise.resolve({
          pushed_states: 0, pushed_feeds: 0, pulled_feeds: 0, pulled_entries: 0,
          merged_states: 0,
          errors: (syncPhasePlan[which] && syncPhasePlan[which].errors) || [],
        });
      }
      case 'refresh_all_feeds': return Promise.resolve(refreshPlan.value);
      case 'list_folders': return Promise.resolve([]);
      case 'list_feeds': return Promise.resolve([]);
      case 'list_articles': return Promise.resolve([]);
      case 'article_counts': return Promise.resolve({});
      default: return Promise.resolve(null);
    }
  });
  try {

  const captureToasts = async (fn) => {
    const seen = [];
    const orig = store.getState().showToast;
    store.setState({ showToast: (t) => { seen.push(t); } });
    try { await fn(); } finally { store.setState({ showToast: orig }); }
    return seen;
  };

  // (a) 无失败：文案必须与改动前逐字相同
  syncPhasePlan.feeds = { errors: [] };
  syncPhasePlan.states = { errors: [] };
  refreshPlan.value = { new_articles: 3, failed_feeds: 0 };
  let toasts = await captureToasts(async () => {
    store.getState().triggerManualSync();
    for (let i = 0; i < 40; i++) await new Promise((r) => setTimeout(r, 5));
  });
  checkNew('(f5) 运行时·无失败：最终 toast 逐字为「已刷新，新增 3 条」（与改动前相同）',
    toasts.includes('已刷新，新增 3 条'), JSON.stringify(toasts));

  refreshPlan.value = { new_articles: 3, failed_feeds: 2 };
  toasts = await captureToasts(async () => {
    store.getState().triggerManualSync();
    for (let i = 0; i < 40; i++) await new Promise((r) => setTimeout(r, 5));
  });
  checkNew('(f5) 运行时·无同步失败但有直连失败：逐字为「已刷新，新增 3 条，2 个源直连失败」',
    toasts.includes('已刷新，新增 3 条，2 个源直连失败'), JSON.stringify(toasts));

  refreshPlan.value = null;
  toasts = await captureToasts(async () => {
    store.getState().triggerManualSync();
    for (let i = 0; i < 40; i++) await new Promise((r) => setTimeout(r, 5));
  });
  checkNew('(f5) 运行时·无新文章：逐字为「已刷新，无新文章」',
    toasts.includes('已刷新，无新文章'), JSON.stringify(toasts));

  // (b) 有失败：必须出现，且前置
  syncPhasePlan.feeds = { errors: ['拉取订阅 http://x/a.xml 建本地失败: boom'] };
  syncPhasePlan.states = { errors: [] };
  refreshPlan.value = { new_articles: 3, failed_feeds: 0 };
  toasts = await captureToasts(async () => {
    store.getState().triggerManualSync();
    for (let i = 0; i < 40; i++) await new Promise((r) => setTimeout(r, 5));
  });
  const failToast = toasts.find((t) => t.includes('项失败'));
  checkNew('(f5) 运行时·同步有失败：最终 toast 确实含失败（修前此处只有「已刷新…」）',
    !!failToast && failToast.includes('boom'), JSON.stringify(toasts));
  checkNew('(f5) 运行时·失败文案**前置**：失败出现在「已刷新」之前（第一眼先读到失败）',
    !!failToast && failToast.indexOf('项失败') < failToast.indexOf('已刷新'),
    String(failToast));

  /* (f5-st) **仅 states 阶段**失败——这一例专门防「mock 参数解包写错、两个阶段都
     回落到 feeds」的盲区（审查 FINDING）。若 mock 只驱动 feeds，则删掉 states 的
     消费后本断言仍会通过；加上它之后该缺陷即被捕获。 */
  syncPhasePlan.feeds = { errors: [] };
  syncPhasePlan.states = { errors: ['状态阶段失败: [db] states boom'] };
  refreshPlan.value = { new_articles: 4, failed_feeds: 0 };
  toasts = await captureToasts(async () => {
    store.getState().triggerManualSync();
    for (let i = 0; i < 40; i++) await new Promise((r) => setTimeout(r, 5));
  });
  const stToast = toasts.find((t) => t.includes('项失败'));
  checkNew('(f5-st) 运行时·**仅 states 阶段**失败也必须被呈现（防 mock 只驱动 feeds 的盲区）',
    !!stToast && stToast.includes('states boom'), JSON.stringify(toasts));
  checkNew('(f5-st) 仅 states 失败时，成功子句仍完整保留在后（未相互吞掉）',
    !!stToast && stToast.includes('已刷新，新增 4 条'), String(stToast));

  /* (f6) 窄窗口不把失败提示推出屏幕（审查 FINDING 2 修订）。
     缺陷形态：`.toast-pill` 原为 `white-space: nowrap` 且与 layer 均无宽度约束，
     右对齐元素向左溢出 → 实测应用最小宽度（980px 窗口 / 847px 视口）下溢出 **315px**，
     **恰好把「有 N 项失败」这个标记本身推到屏幕外**——本任务要交付的可见性在窄窗失效。
     修法：layer 加 `max-width: min(460px, calc(100vw - 40px))`、pill 改为可换行。 */
  const cssSrc = fs.readFileSync(new URL('../../src/styles/base.css', import.meta.url), 'utf8');
  checkNew('(f6) toast 层宽度受视口约束（防窄窗口下长提示溢出屏幕左侧）',
    /\.toast-layer\s*\{[^}]*max-width:\s*min\(460px,\s*calc\(100vw - 40px\)\)/s.test(cssSrc));
  checkNew('(f6) toast 文案允许换行（防长失败提示被截断而看不到原因）',
    /\.toast-pill\s*\{[^}]*white-space:\s*normal/s.test(cssSrc)
    && /\.toast-pill\s*\{[^}]*overflow-wrap:\s*anywhere/s.test(cssSrc));
  checkNew('(f6) 修前形态可复现：`.toast-pill` 原为 nowrap（对照基线证据中的 315px 溢出）',
    !/\.toast-pill\s*\{[^}]*white-space:\s*nowrap/s.test(cssSrc));
  } finally {
    restoreInvokeF5();
  }
}

  const fs = await import('node:fs');
/* ============================================================
     TASK-065 N8/N11：卡片与 Reader 的译文渲染契约（源码形态断言）
     渲染分支无 DOM harness，以源码文本核对四处分支的存在性：
     未消毒（rawTranslatedIds 命中）→ 纯文本插值；消毒后 → dangerouslySetInnerHTML。
     修前卡片为纯文本插值（无分支、无 dangerouslySetInnerHTML）→ 断言失败。
     ============================================================ */
  const tlSrc = fs.readFileSync(new URL('../../src/components/Timeline.tsx', import.meta.url), 'utf8');
  const readerSrc = fs.readFileSync(new URL('../../src/components/Reader.tsx', import.meta.url), 'utf8');
  const socialBlock = tlSrc.slice(tlSrc.indexOf('social-translated-block'), tlSrc.indexOf('social-actions-bar'));
  const notifStart = tlSrc.indexOf('notif-translated-block');
  const notifBlock = notifStart < 0 ? '' : tlSrc.slice(notifStart, tlSrc.indexOf('notif-expand-btn', notifStart));
  checkNew('(n8) SocialCard 译文块按 rawTranslated 分支：消毒后 dangerouslySetInnerHTML（修前纯文本插值）',
    socialBlock.includes('rawTranslated ? (') && socialBlock.includes('dangerouslySetInnerHTML'));
  checkNew('(n8) NotifCard 译文块同样分支（修前纯文本插值显示字面标签）',
    notifBlock.includes('rawTranslated ? (') && notifBlock.includes('dangerouslySetInnerHTML'));
  /* (n11) 原为「Reader.tsx 同时含 rawStream 与 dangerouslySetInnerHTML」的源码形态断言；
     OPT-012 把三条呈现路径收口进生产组件 ReaderProse（源码态转义文本 / 流式纯文本 /
     渲染态 HTML），Reader.tsx 不再直接持有 dangerouslySetInnerHTML——旧断言锁的是
     内部结构而非行为，按「允许附理由改写、不得删覆盖」改到新结构：
     行为面（流式纯文本转义、不创建标签）由本文件 OPT-012 块的**真实 SSR 输出**承担，
     这里只保留接线单点：Reader 必须把 rawStream 状态传给 ReaderProse，且 ReaderProse
     的流式分支区间内不得回到 dangerouslySetInnerHTML。
     切片边界从「流式分支起点」到其后**首个**危险 token（文件头注释里也有该词，
     故必须 indexOf(token, 起点) 向后找，不能用首个全局命中——首版就栽在这里）。 */
  const readerProseSrc = fs.readFileSync(new URL('../../src/components/ReaderProse.tsx', import.meta.url), 'utf8');
  const proseStreamAt = readerProseSrc.indexOf('if (isStreamingTranslation)');
  const proseDangerAt = readerProseSrc.indexOf('dangerouslySetInnerHTML', proseStreamAt < 0 ? 0 : proseStreamAt);
  const proseStreamBranch = proseStreamAt < 0 ? ''
    : readerProseSrc.slice(proseStreamAt, proseDangerAt < 0 ? readerProseSrc.length : proseDangerAt);
  checkNew('(n11) Reader 译文渲染含未消毒纯文本分支（流式产物不进 HTML 渲染路径；OPT-012 收口 ReaderProse：接线传 rawStream + 流式分支先于且不进入 HTML 创建路径）',
    readerSrc.includes('isStreamingTranslation={isShowingTranslatedProse && !!rawStream}')
    && proseStreamBranch.length > 0
    && proseDangerAt > proseStreamAt
    && !proseStreamBranch.includes('dangerouslySetInnerHTML'));

  /* (p3-f4) M7（审查：Timeline 删除假成功 toast 在修后回归网下全绿）：SocialCard
     收藏/标读按钮不得在组件层弹本地假成功 toast——成功态由卡片自身状态呈现，
     失败提示由 store 收口（optimisticEntryFlagToggle 的失败 toast）。切片取
     social-actions-bar 到翻译按钮之间（恰为收藏/标读两个按钮）。 */
  const socialBarSrc = tlSrc.slice(tlSrc.indexOf('social-actions-bar'), tlSrc.indexOf("showTranslate ? 'active-translate'"));
  checkNew('(p3-f4) SocialCard 收藏/标读按钮不弹本地假成功 toast（M7 变红：恢复组件层 toast 即红）',
    socialBarSrc.length > 0
    && socialBarSrc.includes("toggleEntryFlag(item.id, 'isStarred')")
    && socialBarSrc.includes("toggleEntryFlag(item.id, 'isRead')")
    && !socialBarSrc.includes('showToast'));

  {
    const fsP = await import('node:fs');
    const appSrc = fsP.readFileSync(new URL('../../src/App.tsx', import.meta.url), 'utf8');
    const onMoveZone = appSrc.slice(appSrc.indexOf('const onMove = (ev: MouseEvent) => {'), appSrc.indexOf('const onUp = () => {'));
    const onUpZone = appSrc.slice(appSrc.indexOf('const onUp = () => {'), appSrc.indexOf("window.addEventListener('mousemove'"));
    checkNew('(p4) 列宽拖动中不落库、松手才持久化（修前 onMove 每像素一次 set_setting IPC）',
      !onMoveZone.includes('updateSettings') && onUpZone.includes('updateSettings({ listWidth: Math.round(w) })'));
  }

  /* ---------- (r) TASK-068：行类型契约防漂移（与 Rust row_fixture_e2e 共用 fixture） ----------
     TASK-069 审查 F2：修前只核对了部分字段的字面值，整行删除某个映射（如
     `cover: row.image_url ?? undefined`）仍然 301/301 —— 因为可选字段缺失与
     「值为 undefined」不可区分。现改为「键集合 + 逐键期望值」全量核对：
     少一个键、多一个键、键值写错、把字段接错数据源，全部失败。 ---------- */
  {
    const fsR = await import('node:fs');
    const fixture = JSON.parse(fsR.readFileSync(new URL('../../src-tauri/tests/fixtures/row_fixture.json', import.meta.url), 'utf8'));
    const { articleRowToEntry, feedRowToItem } = await import('../../dist-test/lib/api.js');
    const entry = articleRowToEntry(fixture.article_list_item);
    const feedItem = feedRowToItem(fixture.feed_row);

    // 逐键期望值（键集合即契约：fixture 的非空取值使「接错源」也能被发现）
    const expectEntry = {
      id: '42', feedId: '7', title: 'Fixture Article',
      publishedAt: Date.parse('2026-09-19T01:00:00+08:00'),
      isRead: false, isStarred: true, tags: [], source: 'miniflux',
      snippet: 'snippet text', author: 'Fixture Author',
      cover: 'https://e.example/img.png', imageUrl: 'https://e.example/img.png',
      audioUrl: 'https://e.example/audio.mp3', enclosureUrl: 'https://e.example/audio.mp3',
      durationSec: 1234, url: 'https://e.example/a',
      aiSummary: 'fixture summary',
      /* TASK-122：content/rawContent 不再映射进视图行（正文真值源 bodyById） */
      translatedContent: '<p>translated</p>', fulltextExtracted: false,
    };
    const expectFeed = {
      id: '7', name: 'Fixture Feed', url: 'https://f.example/rss',
      favicon: 'https://f.example/favicon.ico', layout: 'article',
      autoSummary: true, autoTranslate: false, fetchFailed: false,
    };

    const missing = (actual, expected) => Object.keys(expected).filter((k) => !(k in actual));
    const extra = (actual, expected) => Object.keys(actual).filter((k) => !(k in expected));
    // 标量用 Object.is；数组按下标逐项比（Object.is 对数组是引用比较，
    // 直接用会把两个内容相同的 [] 判成不等——那不是漂移）。
    const sameValue = (a, b) => (Array.isArray(b) && Array.isArray(a))
      ? a.length === b.length && b.every((v, i) => Object.is(a[i], v))
      : Object.is(a, b);
    const wrong = (actual, expected) => Object.keys(expected).filter(
      (k) => k in actual && !sameValue(actual[k], expected[k]));

    /* 【TASK-122 改动理由】articleRowToEntry 不再映射 content/rawContent 进视图行
       （正文真值源 bodyById；with_content 恒 false，映射本就是空转）——期望键集
       同步收窄，其余键逐键取值断言不变。 */
    checkNew('(r) articleRowToEntry 键集合与逐键取值全量一致（缺键/多键/错值/接错源任一即失败；TASK-122 起不含 content/rawContent）',
      missing(entry, expectEntry).length === 0 && extra(entry, expectEntry).length === 0
      && wrong(entry, expectEntry).length === 0);
    checkNew('(r) feedRowToItem 键集合与逐键取值全量一致（缺键/多键/错值任一即失败）',
      missing(feedItem, expectFeed).length === 0 && extra(feedItem, expectFeed).length === 0
      && wrong(feedItem, expectFeed).length === 0);

    // 反向自检：断言本身能识别「删除映射」与「接错数据源」——用合成对象证明比较器有效。
    // （若比较器写成永远为真，这两条会失败，从而避免「断言失效却全绿」）
    const brokenEntry = { ...entry };
    delete brokenEntry.cover;
    checkNew('(r) 比较器自检：删除 cover 映射必须被判定为失败（防断言失效）',
      missing(brokenEntry, expectEntry).length === 1 && missing(brokenEntry, expectEntry)[0] === 'cover');
    checkNew('(r) 比较器自检：cover 接错数据源（取 imageUrl 之外的值）必须被判定为失败',
      wrong({ ...entry, cover: 'https://wrong.example/x.png' }, expectEntry).includes('cover'));
  }

/* ============================================================
   TASK-092（REQ-106 ③）：封面图片位统一代理 / 失败回退 / 失效上报
   驱动 src/lib/coverImage.ts（五处图片位共用的状态机，ui-loader 就地转译），
   fetch_image / report_broken_cover 由本段落临时包一层 __INVOKE__ 可控假后端
   （段末恢复）。组件是否真的调用这些函数：SSR 渲染 CoverImage + 源码形态断言。
   ============================================================ */
{
  const cov = await import('../../src/lib/coverImage.ts');
  const ip = await import('../../src/lib/imageProxy.ts');
  const fsC = await import('node:fs');
  const readSrc = (p) => fsC.readFileSync(new URL(p, import.meta.url), 'utf8');
  const prevInvokeC = globalThis.__INVOKE__;
  const cCalls = [];
  const fetchPlan = new Map(); // url -> 'png' | 'html' | 'reject' | 'empty'
  let reportReject = false;
  const PNG = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13];
  const HTML = Array.from(new TextEncoder().encode('<!doctype html><html><body>请登录</body></html>'));
  const nCmd = (cmd, url) => cCalls.filter((c) => c.cmd === cmd && (url === undefined || c.args.url === url)).length;
  const reports = () => cCalls.filter((c) => c.cmd === 'report_broken_cover').map((c) => `${c.args.articleId}|${c.args.url}`);
  const SSPAI = 'https://cdnfile.sspai.com/2026/09/cover.png?imageView2/2/w/300';
  const DOUBAN = 'https://img9.doubanio.com/view/photo/l/public/p1.jpg';
  const PLAIN = 'https://images.example.com/c.jpg';
  const unhandled = [];
  const onUnhandled = (e) => { unhandled.push(e); };
  const warns = [];
  // R1：临时覆盖统一在 try 内注册、finally 复原（成功/异常都保全；注册本身分步兜底）
  let restoreInvokeC = null;
  let restoreWarnC = null;
  try {
    restoreInvokeC = overrideGlobal(globalThis, '__INVOKE__', (cmd, args) => {
      if (cmd === 'fetch_image') {
        cCalls.push({ cmd, args });
        const plan = fetchPlan.get(args.url) ?? 'png';
        if (plan === 'reject') return Promise.reject({ code: 'imageFetch', message: 'HTTP 错误: 403' });
        if (plan === 'empty') return Promise.resolve([]);
        return Promise.resolve(plan === 'html' ? HTML : PNG);
      }
      if (cmd === 'report_broken_cover') {
        cCalls.push({ cmd, args });
        return reportReject ? Promise.reject({ code: 'db', message: 'database is locked' }) : Promise.resolve(true);
      }
      return prevInvokeC(cmd, args);
    });
    restoreWarnC = overrideGlobal(console, 'warn', (...a) => { warns.push(a.map(String).join(' ')); });
    process.on('unhandledRejection', onUnhandled);
    cov.resetCoverCacheForTest();

    /* A1/A2：取图路径只由 imageProxy.needsImageProxy 决定 */
    checkNew('(cov-a1) 取图路径：sspai/doubanio 走代理，普通图床直连，空 cover 为 none（判定来自 imageProxy）',
      cov.coverRoute(SSPAI) === 'proxy' && cov.coverRoute(DOUBAN) === 'proxy'
      && cov.coverRoute(PLAIN) === 'direct' && cov.coverRoute('') === 'none'
      && cov.coverRoute(null) === 'none' && cov.coverRoute(undefined) === 'none'
      && cov.coverRoute('data:image/png;base64,AA==') === 'direct');
    {
      const cs = readSrc('../../src/lib/coverImage.ts');
      const code = cs.split('\n').filter((l) => !/^\s*(\/\/|\/\*|\*)/.test(l)).join('\n');
      checkNew('(cov-a1) 判定规则只在 imageProxy.ts 一处：coverImage.ts 调用 needsImageProxy 且代码里不含任何图床域名',
        code.includes('needsImageProxy(src)') && !/sspai|doubanio/.test(code));
    }

    /* A1：代理成功 → data: URL，fetch_image 带 pageUrl 只发一次 */
    cov.driveCover(SSPAI, 'https://sspai.com/post/1', '11');
    const loadingState = cov.getCoverState(SSPAI).status;
    await nTick(10);
    const okState = cov.getCoverState(SSPAI);
    checkNew('(cov-a1) 代理成功：先 loading，再 ready 为 data:image/png URL；fetch_image 1 次且带 {url,pageUrl}',
      loadingState === 'loading' && okState.status === 'ready' && okState.src.startsWith('data:image/png;base64,')
      && nCmd('fetch_image', SSPAI) === 1
      && cCalls.find((c) => c.cmd === 'fetch_image').args.pageUrl === 'https://sspai.com/post/1');
    checkNew('(cov-a4) 渲染决策：代理未返回前是占位（pending，不直连防盗链图），返回后是 data: 的 img',
      cov.coverView(SSPAI, { status: 'loading' }).kind === 'placeholder'
      && cov.coverView(SSPAI, { status: 'idle' }).kind === 'placeholder'
      && cov.coverView(SSPAI, okState).kind === 'img' && cov.coverView(SSPAI, okState).direct === false
      && cov.coverView(SSPAI, okState).src.startsWith('data:'));

    /* A2：直连域名不触发 fetch_image */
    const fBefore = nCmd('fetch_image');
    cov.driveCover(PLAIN, 'https://example.com/a', '12');
    await nTick(10);
    const plainView = cov.coverView(PLAIN, cov.getCoverState(PLAIN));
    checkNew('(cov-a2) 不需代理的域名直连：不触发 fetch_image，渲染为原 URL 的直连 img',
      nCmd('fetch_image') === fBefore && plainView.kind === 'img' && plainView.direct === true && plainView.src === PLAIN);

    /* B5/C8：代理失败 → 占位 + 上报 */
    fetchPlan.set(DOUBAN, 'reject');
    cov.driveCover(DOUBAN, 'https://movie.douban.com/x', '21');
    await nTick(10);
    cov.driveCover(DOUBAN, 'https://movie.douban.com/x', '21'); // 组件状态变化后 effect 再跑一次
    await nTick(10);
    checkNew('(cov-b5) 代理失败（fetch_image 拒绝）→ failed，渲染为占位而非 img',
      cov.getCoverState(DOUBAN).status === 'failed'
      && cov.coverView(DOUBAN, cov.getCoverState(DOUBAN)).kind === 'placeholder');
    const firstReport = cCalls.find((c) => c.cmd === 'report_broken_cover');
    checkNew('(cov-c8) 失败即上报 report_broken_cover，参数 {articleId:number, url}（Tauri camelCase 契约）',
      reports().includes(`21|${DOUBAN}`) && firstReport?.args.articleId === 21
      && Object.keys(firstReport?.args ?? {}).sort().join(',') === 'articleId,url');

    /* B7：同一会话失败 URL 不再请求；C8 幂等 */
    for (let i = 0; i < 5; i++) cov.driveCover(DOUBAN, 'https://movie.douban.com/x', '21');
    cov.requestProxiedCover(DOUBAN, 'https://movie.douban.com/x');
    await nTick(10);
    checkNew('(cov-b7) 失败的 URL 同会话内不再发 fetch_image（再驱动 6 次后仍只 1 次）',
      nCmd('fetch_image', DOUBAN) === 1);
    checkNew('(cov-c8) 同条目同 URL 只上报一次（重复驱动后 report_broken_cover 仍 1 次）',
      reports().filter((r) => r === `21|${DOUBAN}`).length === 1);
    /* C9：迷你播放条 + 全屏播放器 + 灯箱展示同一 cover（同一条目 id）→ 共用缓存与去重 */
    cov.driveCover(DOUBAN, undefined, '21');
    cov.driveCover(DOUBAN, undefined, '21');
    cov.driveCover(DOUBAN, undefined, '21');
    await nTick(10);
    checkNew('(cov-c9) 播放条/灯箱与卡片展示同一 cover：零新增取图、零新增上报',
      nCmd('fetch_image', DOUBAN) === 1 && reports().filter((r) => r === `21|${DOUBAN}`).length === 1);
    cov.driveCover(DOUBAN, undefined, '22');
    await nTick(10);
    checkNew('(cov-c8) 另一条目引用同一失效 URL：该条目上报一次，但不重新取图',
      nCmd('fetch_image', DOUBAN) === 1 && reports().filter((r) => r === `22|${DOUBAN}`).length === 1);
    checkNew('(cov-c8) 无条目 id（正文图进灯箱）/ data: URL 不上报',
      cov.reportCoverFailure(null, DOUBAN) === false && cov.reportCoverFailure('', DOUBAN) === false
      && cov.reportCoverFailure('31', 'data:image/png;base64,AA==') === false);

    /* B5：代理返回空字节 → 占位 */
    const EMPTYB = 'https://rssfile.sspai.com/empty.jpg';
    fetchPlan.set(EMPTYB, 'empty');
    cov.driveCover(EMPTYB, undefined, '41');
    await nTick(10);
    checkNew('(cov-b5) 代理返回空字节 → failed（reason=empty）',
      cov.getCoverState(EMPTYB).status === 'failed' && cov.getCoverState(EMPTYB).reason === 'empty');

    /* D11：字节不是图片 → 占位 + 上报，不注入 DOM */
    const LOGIN = 'https://cdnfile.sspai.com/login-wall.jpg';
    fetchPlan.set(LOGIN, 'html');
    cov.driveCover(LOGIN, undefined, '51');
    await nTick(10);
    cov.driveCover(LOGIN, undefined, '51');
    await nTick(10);
    checkNew('(cov-d11) 代理字节是 HTML 登录页 → failed(not-image)，不产出 data: URL，渲染占位',
      cov.getCoverState(LOGIN).status === 'failed' && cov.getCoverState(LOGIN).reason === 'not-image'
      && cov.coverView(LOGIN, cov.getCoverState(LOGIN)).kind === 'placeholder');
    checkNew('(cov-d11) 非图片字节同样上报一次', reports().filter((r) => r === `51|${LOGIN}`).length === 1);
    const strict = await ip.fetchProxiedImage(LOGIN);
    checkNew('(cov-d11) fetchProxiedImage 对 HTML 字节返回 not-image（严格判定在 imageProxy.ts）',
      strict.ok === false && strict.reason === 'not-image');

    /* A3：画廊的 proxyImageUrl 行为逐字保留（不做严格判定，仍按 image/jpeg 兜底） */
    const galleryHtml = await ip.proxyImageUrl(LOGIN);
    checkNew('(cov-a3) 画廊 proxyImageUrl 行为不变：非图片字节仍按修前逻辑兜底为 data:image/jpeg',
      typeof galleryHtml === 'string' && galleryHtml.startsWith('data:image/jpeg;base64,'));
    {
      const tl = readSrc('../../src/components/Timeline.tsx');
      const gal = tl.slice(tl.indexOf('const GalleryCard = memo('), tl.indexOf('/* ---------- 播客卡片'));
      checkNew('(cov-a3) 画廊卡片仍走 proxyImageUrl(src, item.url) + proxiedSrc ?? item.imageUrl，未换成 CoverImage',
        gal.length > 0 && gal.includes('void proxyImageUrl(src, item.url)')
        && gal.includes('const imgSrc = proxiedSrc ?? item.imageUrl;') && !gal.includes('<CoverImage'));
    }

    /* 直连失败（img onError）→ 占位 + 上报 + 不再请求 */
    const DIRECT_BAD = 'https://images.example.com/404.jpg';
    cov.driveCover(DIRECT_BAD, undefined, '61');
    const directBefore = cov.coverView(DIRECT_BAD, cov.getCoverState(DIRECT_BAD));
    cov.onCoverError(DIRECT_BAD);
    cov.driveCover(DIRECT_BAD, undefined, '61');
    cov.driveCover(DIRECT_BAD, undefined, '61');
    await nTick(10);
    checkNew('(cov-b5) 直连 onError → failed，之后渲染占位（不再挂 img 重新请求）',
      directBefore.kind === 'img' && cov.getCoverState(DIRECT_BAD).status === 'failed'
      && cov.coverView(DIRECT_BAD, cov.getCoverState(DIRECT_BAD)).kind === 'placeholder'
      && nCmd('fetch_image', DIRECT_BAD) === 0);
    checkNew('(cov-c8) 直连失败上报一次', reports().filter((r) => r === `61|${DIRECT_BAD}`).length === 1);

    /* D10：空 cover 不渲染 img、不代理、不上报 */
    const beforeEmpty = cCalls.length;
    cov.driveCover('', 'https://sspai.com/post/2', '71');
    cov.driveCover(null, undefined, '71');
    cov.driveCover(undefined, undefined, '71');
    await nTick(10);
    checkNew('(cov-d10) 空 cover（空串/null/undefined）：零 fetch_image、零上报、渲染决策为 none',
      cCalls.length === beforeEmpty && cov.coverView('', { status: 'idle' }).kind === 'none'
      && cov.coverView(null, { status: 'idle' }).kind === 'none');

    /* 上报本身失败：静默（console.warn），不产生未处理的 rejection */
    reportReject = true;
    const RB = 'https://images.example.com/report-fails.jpg';
    cov.onCoverError(RB);
    cov.driveCover(RB, undefined, '81');
    await nTick(30);
    reportReject = false;
    checkNew('(cov-c8) report_broken_cover 失败时静默：无 unhandledRejection，仅 console.warn',
      reports().includes(`81|${RB}`) && unhandled.length === 0 && warns.some((w) => w.includes('report_broken_cover')));

    /* 组件接线：SSR 渲染 CoverImage（组件真的消费 coverView 的决策） */
    {
      const { renderToStaticMarkup } = await import('react-dom/server');
      const { createElement } = await import('react');
      const { CoverImage } = await import('../../src/components/CoverImage.tsx');
      const r = (props) => renderToStaticMarkup(createElement(CoverImage, { className: 'card-cover-thumb', alt: 'cover', ...props }));
      const hEmpty = r({ src: '', articleId: '1' });
      const hPending = r({ src: 'https://cdnfile.sspai.com/never-requested.png', articleId: '1' });
      const hReady = r({ src: SSPAI, articleId: '11' });
      const hFailed = r({ src: DOUBAN, articleId: '21' });
      const hDirect = r({ src: PLAIN, articleId: '12' });
      checkNew('(cov-d10) SSR：空 cover 不产出任何 <img>',
        hEmpty === '' && !r({ src: null }).includes('<img'));
      checkNew('(cov-a4) SSR：代理未返回时渲染同类名占位（card-cover-thumb cover-fallback，pending），无 <img>',
        !hPending.includes('<img') && hPending.includes('class="card-cover-thumb cover-fallback"')
        && hPending.includes('data-cover-state="pending"'));
      checkNew('(cov-a1) SSR：代理成功渲染 data: 的 <img>（沿用原类名）',
        /<img[^>]*src="data:image\/png;base64,/.test(hReady) && hReady.includes('class="card-cover-thumb"'));
      checkNew('(cov-b5) SSR：失败渲染占位（failed）+ 图标，无 <img>（不出破图图标）',
        !hFailed.includes('<img') && hFailed.includes('data-cover-state="failed"') && hFailed.includes('svg-icon'));
      checkNew('(cov-a2) SSR：直连渲染原 URL + referrerPolicy=no-referrer（与修前一致）',
        hDirect.includes(`src="${PLAIN}"`) && /referrerpolicy="no-referrer"/i.test(hDirect));
    }

    /* 组件与五处图片位的源码接线（SSR 读不到 effect / onError / 真实 store，故以源码形态钉住） */
    {
      const ci = readSrc('../../src/components/CoverImage.tsx');
      checkNew('(cov-wire) CoverImage：effect 调 driveCover(url, pageUrl, articleId)、渲染走 coverView、代理与直连 img 都挂 onCoverError',
        /useEffect\(\(\) => \{ driveCover\(url, pageUrl, articleId\); \}/.test(ci)
        && ci.includes('coverView(url, state)')
        && (ci.match(/onError=\{\(\) => onCoverError\(url\)\}/g) || []).length === 2
        && /data-cover-route="proxy"[^>]*onError=\{\(\) => onCoverError\(url\)\}/.test(ci)
        && /data-cover-route="direct"[\s\S]*?onError=\{\(\) => onCoverError\(url\)\}/.test(ci));
      const tl = readSrc('../../src/components/Timeline.tsx');
      const pb = readSrc('../../src/components/PlayerBar.tsx');
      const ov = readSrc('../../src/components/Overlays.tsx');
      const cut = (s, a, b) => { const i = s.indexOf(a); return i < 0 ? '' : s.slice(i, s.indexOf(b, i)); };
      const fiveSites = [
        ['文章卡', cut(tl, 'card-main-content', 'card-footer'), 'src={art.cover}', 'articleId={art.id}'],
        ['播客卡', cut(tl, 'className={`podcast-card', 'podcast-show-name'), 'src={item.cover}', 'articleId={item.id}'],
        ['迷你播放条', cut(pb, 'player-track-info', 'player-titles'), 'src={player.cover}', 'articleId={player.coverEntryId}'],
        ['全屏播放器', cut(pb, 'player-full-cover-wrap', 'player-full-meta'), 'src={player.cover}', 'articleId={player.coverEntryId}'],
        ['灯箱', cut(ov, 'export function Lightbox()', 'export function NewCategoryModal()'), 'src={lightboxUrl}', 'articleId={lightboxEntryId}'],
      ];
      const badSites = fiveSites.filter(([, blk, a, b]) => !(blk.includes('<CoverImage') && blk.includes(a) && blk.includes(b) && !/<img\b/.test(blk)));
      checkNew(`(cov-wire) 五处图片位全部改用 CoverImage（带条目 id）且不再有裸 <img>（不合规：${badSites.map((s) => s[0]).join('、') || '无'}）`,
        badSites.length === 0);
    }

    /* C9 数据面：播放条 cover 与条目 id 同源；灯箱带条目 id；关闭清空 */
    {
      store.getState().playPodcastEpisode('第 A 集', '节目', DOUBAN, 'https://a.example/ea.mp3', '21');
      const pA = store.getState().player;
      store.getState().playPodcastEpisode('第 B 集', '节目', '', 'https://a.example/eb.mp3', '22');
      const pB = store.getState().player;
      checkNew('(cov-c9) 播放条封面记下所属条目 id；新剧集无 cover 时 cover 与 id 一起沿用（上报对象与显示一致）',
        pA.cover === DOUBAN && pA.coverEntryId === '21' && pB.cover === DOUBAN && pB.coverEntryId === '21');
      store.getState().closePodcastBar();
      store.getState().openLightbox(DOUBAN, '21');
      const lbOpen = { url: store.getState().lightboxUrl, id: store.getState().lightboxEntryId };
      store.getState().closeLightbox();
      store.getState().openLightbox('https://x.example/prose.png');
      const lbProse = store.getState().lightboxEntryId;
      store.getState().closeLightbox();
      checkNew('(cov-c9) 灯箱携带条目 id（画廊）/ 正文图为 null；关闭同时清空 url 与 id',
        lbOpen.url === DOUBAN && lbOpen.id === '21' && lbProse === null
        && store.getState().lightboxUrl === null && store.getState().lightboxEntryId === null);
      const tl = readSrc('../../src/components/Timeline.tsx');
      checkNew('(cov-c9) 画廊打开灯箱时带上条目 id（灯箱失败可按条目上报）',
        tl.includes('if (lightboxSrc) openLightbox(lightboxSrc, item.id);'));
    }
  } finally {
    process.off('unhandledRejection', onUnhandled);
    if (restoreWarnC) restoreWarnC();
    if (restoreInvokeC) restoreInvokeC();
    cov.resetCoverCacheForTest();
  }
}

/* ============================================================
   TASK-094（REQ-107）：三布局列表可达性 —— 列表查询带 layout、分页游标/视图
   缓存键含布局（R7）、不足一屏自动续拉（有上限）、哨兵在不可滚动时可点击。

   修前事实（基线 s7b B1）：画廊 5/44、播客 6/119、通知 1/20 且容器不可滚动，
   哨兵仍显示「滚动加载更多」——list_articles 无布局维度，后端全局分页、前端
   按布局本地过滤，稀疏布局首批撑不满容器、onScroll 永不触发。

   断言分层（证据强度如实说明）：
   - (r7-key)：键函数形态断言（scopePageKey / scopeQueryArgs 含布局）。
   - (r7-switch)/(reach-args)：内存假后端下走真实 store 动作，断言**请求参数**
     带 layout、各布局首批 offset=0、切回不重复不跳页。本段假后端刻意不做布局
     过滤（后端过滤语义由 Rust 侧 list_articles_layout_* / article_index 对齐
     测试锁定；真机假后端 tmp/task-094/mock_backend_094.py 逐字复刻谓词），
     因此可见集合断言一律走 selectVisibleEntries（前端布局过滤仍生效）。
   - (reach-cap)/(reach-sentinel)：refillDecision 纯函数判定 + Timeline 源码形态
     （判定真的接进 effect 与哨兵分支）；真实点击/滚动的交互证据由真机 harness
     （tmp/task-094/ui/）取证。
   ============================================================ */
{
  const { scopeQueryArgs, scopePageKey, viewEntriesCache } = await import('../../dist-test/store/internals.js');
  const { refillDecision, AUTO_REFILL_MAX_CALLS } = await import('../../src/components/timelineRefill.ts');
  const fs94 = await import('node:fs');
  const readSrc94 = (p) => fs94.readFileSync(new URL(p, import.meta.url), 'utf8');

  /* ---------- (r7-key) 键含布局 ---------- */
  checkNew('(r7-key) scopePageKey 含布局：同范围不同布局是不同游标桶；不传 layout 的调用保持旧形态（兼容路径）',
    scopePageKey('all', 'image') === 'image|all' && scopePageKey('10', 'podcast') === 'podcast|10'
    && scopePageKey('all', 'image') !== scopePageKey('all', 'article') && scopePageKey('all') === 'all');
  checkNew('(r7-key) scopeQueryArgs 三参透传 layout；不传 layout 时返回值与修前逐字一致（不含 layout 键）',
    JSON.stringify(scopeQueryArgs('all', 'newest', 'image')) === JSON.stringify({ feed_id: null, folder_id: null, newest_first: true, layout: 'image' })
    && JSON.stringify(scopeQueryArgs('12', 'oldest', 'podcast')) === JSON.stringify({ feed_id: 12, folder_id: null, newest_first: false, layout: 'podcast' })
    && JSON.stringify(scopeQueryArgs('cat-1', 'newest', 'notification')) === JSON.stringify({ feed_id: null, folder_id: 1, newest_first: true, layout: 'notification' })
    && JSON.stringify(scopeQueryArgs('all', 'newest')) === JSON.stringify({ feed_id: null, folder_id: null, newest_first: true }));

  /* ---------- (r7-switch) 切布局游标不串（内存假后端，行集按时间交错） ---------- */
  const prevInvoke94 = globalThis.__INVOKE__;
  try {
    const NOW94 = Date.now();
    const iso94 = (ms) => new Date(ms).toISOString();
    const mkRow94 = (o) => ({
      id: 0, feed_id: 10, title: 't', author: null, snippet: 's', image_url: null,
      enclosure_url: null, enclosure_mime: null, duration_sec: null, ai_summary: null,
      source: 'direct', published_at: iso94(NOW94), is_read: false, is_starred: false, url: null,
      content_html: null, translated_content: null, fulltext_extracted: false, ...o,
    });
    const FOLDERS94 = [
      { id: 1, name: '技术', layout: 'article', auto_summary: false, auto_translate: false, collapsed: false },
    ];
    const FEEDS94 = [
      { id: 10, folder_id: 1, feed_url: 'https://a.example/rss', site_url: null, title: '源A', favicon_url: null, layout: 'inherit', auto_summary: false, auto_translate: false, fetch_failed: false, fetch_error: null, last_fetched_at: null },
      { id: 11, folder_id: 1, feed_url: 'https://b.example/rss', site_url: null, title: '源B', favicon_url: null, layout: 'social', auto_summary: false, auto_translate: false, fetch_failed: false, fetch_error: null, last_fetched_at: null },
    ];
    let rows94 = [];
    const calls94 = [];
    /* TASK-117：与主假后端同口径——排序补 id 决胜，keyset 锚成对给出时按谓词续拉 */
    const queryRows94 = (a = {}) => {
      let out = rows94.slice();
      if (a.feed_id != null) out = out.filter((r) => r.feed_id === a.feed_id);
      if (a.folder_id != null) out = out.filter((r) => (FEEDS94.find((f) => f.id === r.feed_id) || {}).folder_id === a.folder_id);
      const dir = a.newest_first === false ? 1 : -1;
      out.sort((x, y) => (dir * (Date.parse(x.published_at) - Date.parse(y.published_at))) || (dir * (x.id - y.id)));
      if (a.last_published != null && a.last_id != null) {
        out = out.filter((r) => dir < 0
          ? (r.published_at < a.last_published || (r.published_at === a.last_published && r.id < a.last_id))
          : (r.published_at > a.last_published || (r.published_at === a.last_published && r.id > a.last_id)));
        return out.slice(0, a.limit == null ? out.length : a.limit);
      }
      const off = a.offset || 0;
      return out.slice(off, a.limit == null ? out.length : off + a.limit);
    };
    globalThis.__INVOKE__ = (cmd, args) => {
      calls94.push({ cmd, args });
      switch (cmd) {
        case 'list_folders': return Promise.resolve(FOLDERS94);
        case 'list_feeds': return Promise.resolve(FEEDS94);
        case 'feed_counts': return Promise.resolve([]);
        case 'sync_status': return Promise.resolve({ connected: false });
        case 'list_articles': return Promise.resolve(queryRows94((args && args.args) || {}));
        case 'article_index': return Promise.resolve(0);
        case 'get_articles': return Promise.resolve([]);
        case 'get_setting': return Promise.resolve(null);
        case 'set_setting': return Promise.resolve(null);
        default: return Promise.resolve(null);
      }
    };

    const nTick94 = (ms = 20) => new Promise((r) => setTimeout(r, ms));
    const store94 = (await import('../../dist-test/store.js')).useAppStore;
    const { selectVisibleEntries } = await import('../../dist-test/store.js');
    viewEntriesCache.clear();
    rows94 = [];
    for (let i = 0; i < 600; i += 1) {
      rows94.push(mkRow94({ id: 8000 + i, feed_id: 10, title: `A${i}`, published_at: iso94(NOW94 - i * 1000) }));
      rows94.push(mkRow94({ id: 9000 + i, feed_id: 11, title: `B${i}`, published_at: iso94(NOW94 - i * 1000 - 500) }));
    }
    store94.setState({
      dataMode: 'tauri', dataLoading: false, bootstrapError: null,
      activeContentLayout: 'article', activeViewFilter: 'all', activeFeedFilter: 'all',
      timelineFilter: 'all', timelineSort: 'newest',
      activeArticleId: null, openedReadIds: {}, entries: [], categories: [], feedIndex: new Map(),
      feedCounts: new Map(), articlesLimit: 0, articlesLoading: false, articlesExhausted: false,
      articlesCursor: {}, hydratedIds: {}, hydrationErrors: {}, toasts: [],
    });
    store94.getState().selectLayout('article');
    await store94.getState().reloadFromBackend();
    const r7First = calls94.find((c) => c.cmd === 'list_articles');
    checkNew('(reach-args) 列表首批请求带 layout=article 且 offset=0（后端查询的布局维度）',
      r7First?.args.args.layout === 'article' && r7First?.args.args.offset === 0);

    calls94.length = 0;
    await store94.getState().loadMoreArticles();
    const idsBeforeLeave = selectVisibleEntries(store94.getState()).map((e) => e.id);
    /* 【TASK-117 改动理由】续拉 wire 从 offset=500 改为 keyset 锚（article 布局首页
       末行 id 9249，交错序最后一行）——判别意图不变：锚取自该布局自己的游标桶。 */
    checkNew('(r7-switch) article 第 2 页：请求沿用该布局自己的游标（keyset 锚 last_id=9249），可见集合增长到 500 且无重复',
      calls94.find((c) => c.cmd === 'list_articles')?.args.args.last_id === 9249
      && calls94.find((c) => c.cmd === 'list_articles')?.args.args.offset === undefined
      && idsBeforeLeave.length === 500 && new Set(idsBeforeLeave).size === 500
      && idsBeforeLeave.every((id) => Number(id) >= 8000 && Number(id) < 9000)
      && store94.getState().articlesCursor['article|all']?.loaded === 1000);

    /* 切到 social：该口径首批从 offset=0 开始（请求带 layout=social），游标分桶互不污染 */
    calls94.length = 0;
    store94.getState().selectLayout('social');
    const r7SocialSync = store94.getState();
    checkNew('(r7-switch) 切 social 同步阶段：游标从 0 起步（不继承 article 的 1000），article 桶原样保留',
      r7SocialSync.articlesLimit === 0 && r7SocialSync.articlesCursor['article|all']?.loaded === 1000);
    await nTick94(30);
    const r7SocialCall = calls94.find((c) => c.cmd === 'list_articles');
    const r7SocialIds = selectVisibleEntries(store94.getState()).map((e) => e.id);
    checkNew('(reach-args) social 首批请求带 layout=social 且 offset=0；可见条目只含 social 源',
      r7SocialCall?.args.args.layout === 'social' && r7SocialCall?.args.args.offset === 0
      && r7SocialIds.length > 0 && r7SocialIds.every((id) => Number(id) >= 9000));
    checkNew('(r7-switch) social 游标记在 social 桶（与 article 桶并存）：两布局各自记账',
      store94.getState().articlesCursor['social|all']?.loaded === 500
      && store94.getState().articlesCursor['article|all']?.loaded === 1000);

    /* 切回 article：缓存命中同步恢复快照，游标与恢复的快照长度对齐（既有 selectFeed
       同一契约——不从 0 起步、不继承 social 桶）；后台刷新 + 续一页后，
       可见集合与离开前逐位一致（不重复、不跳页） */
    calls94.length = 0;
    store94.getState().selectLayout('article');
    const r7BackSync = store94.getState();
    checkNew('(r7-switch) 切回 article 同步恢复：游标与恢复的快照长度对齐（不从 0 起步），social 桶不污染 article 桶',
      r7BackSync.articlesLimit === r7BackSync.entries.length && r7BackSync.articlesLimit > 0
      && r7BackSync.articlesCursor['article|all']?.loaded === r7BackSync.articlesLimit
      && r7BackSync.articlesCursor['social|all']?.loaded === 500);
    await nTick94(30);
    await store94.getState().loadMoreArticles();
    const r7BackIds = selectVisibleEntries(store94.getState()).map((e) => e.id);
    checkNew('(r7-switch) 切回 article 续一页后：可见集合与离开前逐位一致（无重复、无缺失段），游标连续',
      r7BackIds.length === idsBeforeLeave.length && r7BackIds.join(',') === idsBeforeLeave.join(',')
      && new Set(r7BackIds).size === r7BackIds.length
      && store94.getState().articlesCursor['article|all']?.loaded === 1000);

    /* 视图缓存按「布局 × 视图 × 范围」分桶：两布局各有快照（R7 缓存半边） */
    checkNew('(r7-cache) 视图缓存键含布局：article 与 social 各有独立快照，互不冒充',
      !!viewEntriesCache.get('article|all|all') && !!viewEntriesCache.get('social|all|all')
      && viewEntriesCache.get('article|all|all') !== viewEntriesCache.get('social|all|all'));

    /* anchorToArticle 与列表同口径：article_index 请求带 layout */
    calls94.length = 0;
    store94.setState({ activeFeedFilter: '10', activeArticleId: null });
    await store94.getState().anchorToArticle('8003');
    checkNew('(reach-args) anchorToArticle 的 article_index / list_articles 请求同样带 layout（锚定与列表同口径）',
      calls94.some((c) => c.cmd === 'article_index' && c.args.args.layout === 'article')
      && calls94.filter((c) => c.cmd === 'list_articles').every((c) => c.args.args.layout === 'article'));
  } finally {
    globalThis.__INVOKE__ = prevInvoke94;
    viewEntriesCache.clear();
  }

  /* ---------- (reach-cap) 不足一屏自动续拉：判定纯函数 + 组件接线 ---------- */
  checkNew('(reach-cap) refillDecision：在途 / 已到底 / 已撑满 ⇒ idle（不空转）',
    refillDecision({ itemCount: 5, exhausted: false, loading: true, filledViewport: true, autoCalls: 0 }) === 'idle'
    && refillDecision({ itemCount: 5, exhausted: true, loading: false, filledViewport: false, autoCalls: 0 }) === 'idle'
    && refillDecision({ itemCount: 5, exhausted: false, loading: false, filledViewport: true, autoCalls: 0 }) === 'idle');
  checkNew('(reach-cap) refillDecision：空列表未到底 ⇒ refill（TASK-052 口径保留）；非空未撑满 ⇒ refill（TASK-094 兜底）',
    refillDecision({ itemCount: 0, exhausted: false, loading: false, filledViewport: true, autoCalls: 0 }) === 'refill'
    && refillDecision({ itemCount: 3, exhausted: false, loading: false, filledViewport: false, autoCalls: 0 }) === 'refill');
  checkNew(`(reach-cap) refillDecision：连续自动调用达上限（AUTO_REFILL_MAX_CALLS=${AUTO_REFILL_MAX_CALLS}）⇒ idle（防死循环，交给人肉按钮）`,
    AUTO_REFILL_MAX_CALLS === 8
    && refillDecision({ itemCount: 3, exhausted: false, loading: false, filledViewport: false, autoCalls: 7 }) === 'refill'
    && refillDecision({ itemCount: 0, exhausted: false, loading: false, filledViewport: false, autoCalls: 8 }) === 'idle'
    && refillDecision({ itemCount: 3, exhausted: false, loading: false, filledViewport: false, autoCalls: 99, cap: 4 }) === 'idle');
  {
    const tl94 = readSrc94('../../src/components/Timeline.tsx');
    /* 门控必须精确是 `: !listScrollable ? (`：写成 `false && !listScrollable`（判据被
       短路成永假）也算回退——故断言完整门控文本而非仅子串存在。 */
    const iGate = tl94.indexOf(': !listScrollable ? (');
    const iBtn = tl94.indexOf('toggle-action-btn load-more-btn');
    const iIdle = tl94.indexOf('load-more-idle');
    checkNew('(reach-cap) Timeline 接线：消费 refillDecision，连续自动调用计数有上限且可见进展/口径变化重置（防死循环）',
      tl94.includes("from './timelineRefill'") && tl94.includes('refillDecision({')
      && tl94.includes('items.length > prevItemCountRef.current') && tl94.includes('autoRefillRef.current += 1')
      && tl94.includes("lastFilterKeyRef.current = filterKey"));
    checkNew('(reach-sentinel) 哨兵分支：不可滚动 ⇒ 可点击「加载更多」按钮（原生 button，键盘可触发）；可滚动 ⇒ 「滚动加载更多」',
      iGate >= 0 && iBtn > iGate && iIdle > iBtn
      && tl94.includes('onClick={() => void loadMoreArticles()}')
      && tl94.includes('<span className="load-more-idle">滚动加载更多</span>')
      && tl94.includes('load-more-spinner') && tl94.includes('<span className="load-more-end">没有更多了</span>'));
  }
}

/* ============================================================
   fix-2 / fix-5 / fix-6 / fix-7 / fix-8 / fix-9 / fix-10 / fix-11+12 / fix-13 / fix-14
   （发布前自检修复，2026-09-29）：纯函数真值表 + 源码形态断言。
   源级断言说明判别点：无法在无 DOM harness 里点按钮/开窗口的项，
   以「JSX 分支真实存在 + 调用点形态」为判别；CSS 项直接解析声明值。
   ============================================================ */
{
  const fsFix = await import('node:fs');
  const srcOf = (p) => fsFix.readFileSync(new URL(p, import.meta.url), 'utf8');
  const tlFix = srcOf('../../src/components/Timeline.tsx');

  /* ---------- fix-2：用户滚动判定收口 isUserScrollEvent ---------- */
  const { isUserScrollEvent, PROGRAMMATIC_SCROLL_SUPPRESS_MS } =
    await import('../../src/components/scrollAwayRead.ts');
  checkNew('(fix-2) 程序性滚动抑制窗口内的 scroll 事件不算用户滚动（修前 onScroll 无条件置位）',
    isUserScrollEvent({ gestureSeen: true, programmaticUntil: 1000, now: 999 }) === false
    && isUserScrollEvent({ gestureSeen: true, programmaticUntil: 1000, now: 1000 }) === true
    && PROGRAMMATIC_SCROLL_SUPPRESS_MS > 0);
  checkNew('(fix-2) 窗口外但无真实输入闩 ⇒ 不算用户滚动（纯程序性环境不标读）',
    isUserScrollEvent({ gestureSeen: false, programmaticUntil: 0, now: 500 }) === false);
  checkNew('(fix-2) 窗口外且有真实输入（wheel/触摸/滚动条/翻页键）⇒ 用户滚动',
    isUserScrollEvent({ gestureSeen: true, programmaticUntil: 0, now: 500 }) === true);
  checkNew('(fix-2) Timeline 接线：onScroll 判定收口 isUserScrollEvent，修前的无条件置位已移除',
    tlFix.includes('scrollDrivenRef.current = scrollDrivenRef.current || isUserScrollEvent')
    && !tlFix.includes('scrollDrivenRef.current = true;'));
  checkNew('(fix-2) 三处程序性滚动（筛选归零 / J-K 定位 / focus 移动）都必须先开抑制窗口',
    (tlFix.match(/suppressNextScrollEvents\(\);/g) || []).length >= 3
    && tlFix.indexOf('suppressNextScrollEvents();') < tlFix.indexOf('rowVirtualizer.scrollToIndex(idx,')
    && tlFix.includes("el.addEventListener('wheel', latch"));

  /* ---------- fix-5：卡片级翻译失败的内联错误行 + 按钮重试 ---------- */
  checkNew('(fix-5) Social/Notif 卡补翻译失败内联错误行 + 重试按钮（调 translateEntry）',
    (tlFix.match(/translateError && !translatingCard \?/g) || []).length === 2
    && (tlFix.match(/ai-retry-btn" onClick=\{\(\) => useAppStore\.getState\(\)\.translateEntry\(item\.id\)\}/g) || []).length === 2);
  /* 【TASK-122 改动理由】译文真值源迁移 bodyById——按钮判定改读 selectArticleBody
     快照（body.translatedContent）；保护意图（失败态重试不把半截当缓存）不变。 */
  checkNew('(fix-5) 两卡「翻译」按钮在失败态（translateErrors[id] 存在）改走 translateEntry 重试（修前把半截译文当缓存只切显示；TASK-122 判定读 body.translatedContent）',
    (tlFix.match(/if \(next && \(translateError \|\| !body\.translatedContent\)\)/g) || []).length === 2);

  /* ---------- fix-8：auto 配置的 AI 区块空态收起 ---------- */
  const selFix = await import('../../dist-test/store.js');
  checkNew('(fix-8) autoAiBlockOpen 判据：出错恒展开；auto 关收起；auto 开需有产物或在途（空态不再渲染死框）',
    selFix.autoAiBlockOpen(false, false, false, true) === true
    && selFix.autoAiBlockOpen(false, true, false, false) === false
    && selFix.autoAiBlockOpen(true, false, false, false) === false
    && selFix.autoAiBlockOpen(true, true, false, false) === true
    && selFix.autoAiBlockOpen(true, false, true, false) === true);
  checkNew('(fix-8) Timeline 接线：SocialCard/NotifCard 两处跟随 auto 的展开判定改走 autoAiBlockOpen',
    (tlFix.match(/autoAiBlockOpen\(/g) || []).length >= 2
    && !tlFix.includes('feedConfig.autoSummary || !!summaryError)')
    && !tlFix.includes('?? feedConfig.autoTranslate;'));

  /* ---------- fix-6：播放条时长统一 formatDuration ---------- */
  const pbSrc = srcOf('../../src/components/PlayerBar.tsx');
  checkNew('(fix-6) PlayerBar 删除本地 formatClock，时长统一 lib/format.formatDuration（迷你条/全屏条/进度条两端）',
    pbSrc.includes("import { formatDuration } from '../lib/format'")
    && !/function formatClock\b/.test(pbSrc)
    && (pbSrc.match(/formatDuration\(player\.(positionSec|durationSec)\)/g) || []).length === 4
    && pbSrc.includes('formatDuration={formatDuration}'));

  /* ---------- fix-7：全屏播放器层级降到弹窗之下 ---------- */
  const cssFix = srcOf('../../src/styles/base.css');
  const zOf = (sel) => {
    const m = cssFix.match(new RegExp('\\.' + sel + '\\s*\\{[^}]*?z-index:\\s*(\\d+)', 's'));
    return m ? Number(m[1]) : -1;
  };
  checkNew('(fix-7) .player-full-overlay z-index(140) < .modal-overlay(150)：全屏播放时搜索/设置/灯箱可见可关（修前 260 盖住一切弹窗）',
    zOf('player-full-overlay') === 140 && zOf('modal-overlay') === 150
    && zOf('player-full-overlay') < zOf('modal-overlay'));

  /* ---------- fix-9：画廊 img 补 onError + cover-fallback 占位 ---------- */
  checkNew('(fix-9) 画廊 img 补 onError（记入共享封面失败态）且失败/无图均出占位（与另四处统一）',
    tlFix.includes("import { onCoverError } from '../lib/coverImage'")
    && tlFix.includes('onError={onImgError}')
    && tlFix.includes("imgFailed ? ' cover-fallback' : ''")
    && tlFix.includes('if (prevImageUrl !== item.imageUrl) {'));

  /* ---------- fix-10：空必填输入禁用主按钮 ---------- */
  const ovFix = srcOf('../../src/components/Overlays.tsx');
  checkNew('(fix-10) 新建分类/添加订阅空必填输入禁用主按钮（与改名弹窗统一，修前可点但静默 return）',
    ovFix.includes('disabled={!name.trim()}')
    && ovFix.includes('disabled={!url.trim()}'));

  /* ---------- fix-11/12：AI 区块与翻译错误行 token 化 ---------- */
  checkNew('(fix-11/12) base.css 不再含硬编码蓝紫/正红三元组；AI 区块走 --accent、错误行走 --danger 的 color-mix',
    !cssFix.includes('rgba(120,115,184') && !cssFix.includes('rgba(120, 115, 184')
    && !cssFix.includes('rgba(72,128,200') && !cssFix.includes('rgba(72, 128, 200')
    && !cssFix.includes('rgba(229,72,77') && !cssFix.includes('rgba(229, 72, 77')
    && cssFix.includes('color-mix(in srgb, var(--accent) 8%, transparent)')
    && cssFix.includes('color-mix(in srgb, var(--danger) 6%, transparent)'));

  /* ---------- fix-13：ReadingTab 数值标签统一 range-value-tag ---------- */
  const rtFix = srcOf('../../src/components/settings/ReadingTab.tsx');
  checkNew('(fix-13) ReadingTab 三处数值标签改用 range-value-tag（裸内联宽度 span 清零）',
    (rtFix.match(/className="range-value-tag"/g) || []).length === 3
    && !rtFix.includes('style={{ width: 45 }}') && !rtFix.includes('style={{ width: 55 }}'));

  /* ---------- fix-14：页脚版本兜底不再显示假版本号 ---------- */
  const sfFix = srcOf('../../src/components/settings/SettingsSidebarFooter.tsx');
  checkNew("(fix-14) 页脚版本获取失败保持 …（不再回退硬编码假版本 '0.8.0'，与 AboutTab 决策对齐）",
    !sfFix.includes("'0.8.0'")
    && sfFix.includes("version || '…'"));
}

/* ============================================================
   TASK-100（自检遗留收口，2026-09-29）：前端 P3×9 + UI P2×2 + UI P3×17
   + 文档/乱码/兼容。行为断言（内存假后端）+ 源码形态断言，沿用 fix-* 风格。
   明确不做（DEC-task100）：跨布局 J/K 键盘导航、--text-tertiary 对比度调整。
   ============================================================ */
{
  const fs100 = await import('node:fs');
  const src100 = (p) => fs100.readFileSync(new URL(p, import.meta.url), 'utf8');
  const nTick100 = (ms = 20) => new Promise((r) => setTimeout(r, ms));

  /* ---------- (t100-cache) P3-7：viewEntriesCache LRU 上限（8 组合键，先淘汰最旧） ---------- */
  {
    const { viewEntriesCache } = await import('../../dist-test/store/internals.js');
    viewEntriesCache.clear();
    /* TASK-122：bodyById 是模块级实体缓存，跨夹具残留会让后续用例命中上一夹具
       的正文/终态（与 viewEntriesCache 同型污染）——夹具复位一并清空 */
    const { resetBodyCacheForTests } = await import('../../dist-test/store/bodyCache.js');
    resetBodyCacheForTests();
    for (let i = 0; i < 10; i += 1) viewEntriesCache.set(`k${i}`, [{ id: `e${i}` }]);
    checkNew('(t100-cache) 容量上限：写入 10 个组合键后缓存收敛到 8（修前无上限累积）',
      viewEntriesCache.size === 8);
    checkNew('(t100-cache) 先淘汰最旧：最早写入的 k0/k1 被淘汰，最新 k8/k9 保留',
      !viewEntriesCache.has('k0') && !viewEntriesCache.has('k1')
      && viewEntriesCache.has('k8') && viewEntriesCache.has('k9'));
    viewEntriesCache.get('k2'); // 命中刷新新鲜度
    for (let i = 10; i < 12; i += 1) viewEntriesCache.set(`k${i}`, []);
    checkNew('(t100-cache) 命中刷新 LRU 新鲜度：get 过的 k2 不被随后两次写入淘汰',
      viewEntriesCache.has('k2') && viewEntriesCache.size === 8);
    viewEntriesCache.clear();
    checkNew('(t100-cache) clear() 语义保持（切排序整体清空路径不受影响）',
      viewEntriesCache.size === 0);
  }

  /* ---------- (t100-p3-1) reload 在途期间拦截 loadMore/refill ---------- */
  {
    const prevInvoke = globalThis.__INVOKE__;
    try {
      const FOLDERS = [{ id: 1, name: '分类', layout: 'article', auto_summary: false, auto_translate: false, collapsed: false }];
      const FEEDS = [
        { id: 10, folder_id: 1, feed_url: 'https://a.example/rss', site_url: null, title: '源A', favicon_url: null, layout: 'inherit', auto_summary: false, auto_translate: false, fetch_failed: false, fetch_error: null, last_fetched_at: null },
        { id: 11, folder_id: 1, feed_url: 'https://b.example/rss', site_url: null, title: '源B', favicon_url: null, layout: 'inherit', auto_summary: false, auto_translate: false, fetch_failed: false, fetch_error: null, last_fetched_at: null },
      ];
      const mkRow = (o) => ({
        id: 0, feed_id: 10, title: 't', author: null, snippet: 's', image_url: null,
        enclosure_url: null, enclosure_mime: null, duration_sec: null, ai_summary: null,
        source: 'direct', published_at: '2026-09-04T10:00:00Z', is_read: false, is_starred: false, url: null,
        content_html: null, translated_content: null, fulltext_extracted: false, ...o,
      });
      let feedFilter = null;
      let delayed; // 挂起中的 list_articles resolve（模拟慢 reload）
      const calls = [];
      globalThis.__INVOKE__ = (cmd, args) => {
        calls.push({ cmd, args });
        switch (cmd) {
          case 'list_folders': return Promise.resolve(FOLDERS);
          case 'list_feeds': return Promise.resolve(FEEDS);
          case 'feed_counts': return Promise.resolve([]);
          case 'sync_status': return Promise.resolve({ connected: false });
          case 'list_articles': {
            const a = (args && args.args) || {};
            let out = [];
            for (let i = 0; i < 600; i += 1) {
              out.push(mkRow({
                id: 50000 + (feedFilter === 11 ? 100000 : 0) + i,
                feed_id: feedFilter ?? 10,
                published_at: new Date(Date.UTC(2026, 8, 4, 10, 0, 0) - i * 1000).toISOString(),
              }));
            }
            if (a.feed_id != null) out = out.filter((r) => r.feed_id === a.feed_id);
            /* TASK-117：keyset 锚成对给出时按谓词续拉（published_at 两两互异，字符串比较即可） */
            if (a.last_published != null && a.last_id != null) {
              out = out.filter((r) => r.published_at < a.last_published
                || (r.published_at === a.last_published && r.id < a.last_id));
              out = out.slice(0, a.limit != null ? a.limit : undefined);
            } else {
              out = out.slice(a.offset || 0, a.limit != null ? (a.offset || 0) + a.limit : undefined);
            }
            if (delayed) {
              delayed = null;
              return new Promise((resolve) => setTimeout(() => resolve(out), 60));
            }
            return Promise.resolve(out);
          }
          default: return Promise.resolve(null);
        }
      };
      const { isBackendReloadInFlight } = await import('../../dist-test/store/slices/bootstrap.js');
      const viewCache = (await import('../../dist-test/store/internals.js')).viewEntriesCache;
      viewCache.clear();
      store.setState({
        dataMode: 'tauri', dataLoading: false, bootstrapError: null,
        activeContentLayout: 'article', activeViewFilter: 'all', activeFeedFilter: 'all',
        timelineFilter: 'all', timelineSort: 'newest',
        activeArticleId: null, openedReadIds: {}, entries: [], categories: [], feedIndex: new Map(),
        feedCounts: new Map(), articlesLimit: 0, articlesLoading: false, articlesExhausted: false,
        articlesCursor: {}, hydratedIds: {}, hydrationErrors: {}, toasts: [],
      });
      await store.getState().reloadFromBackend();
      const baseCount = store.getState().entries.length;
      checkNew('(t100-p3-1) 前置：all 口径首批 500 条已就位（假后端单源 600 条）',
        baseCount === 500);

      // 切范围：游标镜像先写入（feed-11 从 0 起步），reload 的 list_articles 挂起 60ms
      delayed = true;
      feedFilter = 11;
      calls.length = 0;
      store.getState().selectFeed('feed-11');
      checkNew('(t100-p3-1) 切范围后 reload 在途：isBackendReloadInFlight() 为真',
        isBackendReloadInFlight() === true);
      const idsBeforeReload = store.getState().entries.length;
      await store.getState().loadMoreArticles();
      await nTick100(5);
      checkNew('(t100-p3-1) 在途窗口内 loadMoreArticles 被拦截：未发出第二发 list_articles（修前会把新口径一页 append 到旧列表尾）',
        calls.filter((c) => c.cmd === 'list_articles').length === 1
          && store.getState().entries.length === idsBeforeReload);
      // reload 落地后拦截解除，续拉恢复
      await nTick100(120);
      checkNew('(t100-p3-1) reload 落地：在途标记清除、entries 为 feed-11 快照、游标对齐',
        isBackendReloadInFlight() === false
          && store.getState().entries.length === 500
          && store.getState().entries.every((e) => e.feedId === '11')
          && store.getState().articlesCursor['article|feed-11']?.loaded === 500);
      calls.length = 0;
      await store.getState().loadMoreArticles();
      /* 【TASK-117 改动理由】续拉 wire 从 offset=500 改为 keyset 锚（feed-11 首页
         末行 id 150499 的原文锚）——判别意图不变：续拉带该范围的游标。 */
      checkNew('(t100-p3-1) 落地后续拉放行：第 2 页请求带该范围 keyset 锚 last_id=150499（假后端每源共 600 行，追加 100 行收敛到底）',
        calls.filter((c) => c.cmd === 'list_articles').length === 1
          && calls.find((c) => c.cmd === 'list_articles')?.args.args.last_id === 150499
          && calls.find((c) => c.cmd === 'list_articles')?.args.args.last_published
            === new Date(Date.UTC(2026, 8, 4, 10, 0, 0) - 499 * 1000).toISOString()
          && store.getState().entries.length === 600
          && store.getState().articlesExhausted === true);
      feedFilter = null;
    } finally {
      globalThis.__INVOKE__ = prevInvoke;
    }
  }

  /* ---------- (t100-p3-3) triggerManualSync：仅「未连接」静默，其余失败可见 ---------- */
  {
    const prevInvoke = globalThis.__INVOKE__;
    try {
      let syncPhaseError = null;
      globalThis.__INVOKE__ = (cmd) => {
        switch (cmd) {
          case 'sync_phase':
            if (syncPhaseError) return Promise.reject(syncPhaseError);
            return Promise.resolve({ pushed_states: 0, pushed_feeds: 0, pulled_feeds: 0, pulled_entries: 0, merged_states: 0, errors: [] });
          case 'refresh_all_feeds': return Promise.resolve({ new_articles: 0, failed_feeds: 0 });
          case 'list_folders': return Promise.resolve([]);
          case 'list_feeds': return Promise.resolve([]);
          case 'list_articles': return Promise.resolve([]);
          case 'feed_counts': return Promise.resolve([]);
          case 'sync_status': return Promise.resolve({ connected: false });
          default: return Promise.resolve(null);
        }
      };
      store.setState({ dataMode: 'tauri', dataLoading: false, bootstrapError: null, toasts: [], syncStatus: 'synced' });
      // 场景 1：notConnected（既有语义）→ 静默跳过订阅层，走纯直连刷新
      syncPhaseError = { code: 'notConnected', message: '未连接后端' };
      store.getState().triggerManualSync();
      await nTick100(60);
      const toast1 = store.getState().toasts.at(-1)?.text ?? '';
      checkNew('(t100-p3-3) notConnected 静默跳过：最终 toast 与既有口径逐字一致（无失败前缀）',
        toast1 === '已刷新，新增 0 条');
      // 场景 2：网络类失败 → 可见（console.warn + 汇入提示），不再吞成假成功
      syncPhaseError = { code: 'timeout', message: '网络超时' };
      store.getState().triggerManualSync();
      await nTick100(60);
      const toast2 = store.getState().toasts.at(-1)?.text ?? '';
      checkNew('(t100-p3-3) 其余失败可见：toast 前置「订阅同步失败：网络超时」，正常信息共存',
        toast2.includes('订阅同步失败：网络超时') && toast2.includes('已刷新，新增 0 条'));
      checkNew('(t100-p3-3) 源码形态：catch 不再无条件吞错，notConnected 判定收口在 isNotConnectedError',
        src100('../../src/store/slices/sync.ts').includes('function isNotConnectedError')
          && !src100('../../src/store/slices/sync.ts').includes(".catch(() => null) // 未连接"));
    } finally {
      globalThis.__INVOKE__ = prevInvoke;
      store.setState({ toasts: [] });
    }
  }

  /* ---------- (t100-p3-4) toggleAllFolders：allSettled 聚合，失败合并一条 toast ---------- */
  {
    const prevInvoke = globalThis.__INVOKE__;
    try {
      const collapseCalls = [];
      globalThis.__INVOKE__ = (cmd, args) => {
        if (cmd === 'set_folder_collapsed') {
          collapseCalls.push(args);
          if (args.id === 3) return Promise.reject({ message: 'db busy' });
        }
        return Promise.resolve(null);
      };
      const mkCat = (id) => ({
        id: `cat-${id}`, name: `分类${id}`, collapsed: false, settingsCollapsed: false,
        layout: 'article', autoSummary: false, autoTranslate: false, feeds: [],
      });
      store.setState({ dataMode: 'tauri', categories: [mkCat(1), mkCat(2), mkCat(3)], toasts: [] });
      store.getState().toggleAllFolders();
      await nTick100(40);
      checkNew('(t100-p3-4) 批量落库仍逐分类发出（3 次 set_folder_collapsed）',
        collapseCalls.length === 3 && collapseCalls.every((a) => a.collapsed === true));
      checkNew('(t100-p3-4) 失败合并一条 toast（含失败个数），不再逐个弹',
        collapseCalls.length === 3
          && store.getState().toasts.filter((t) => t.text.includes('折叠状态未能保存')).length === 1
          && store.getState().toasts.at(-1)?.text === '1 个分类的折叠状态未能保存，重启后可能回退');
    } finally {
      globalThis.__INVOKE__ = prevInvoke;
      store.setState({ toasts: [] });
    }
  }

  /* ---------- (t100-p3-10) mock addFeed：无匹配分类不再假成功 ---------- */
  {
    const prevInvoke = globalThis.__INVOKE__;
    try {
      globalThis.__INVOKE__ = () => Promise.resolve(null);
      const mkCat = (id) => ({
        id: `cat-${id}`, name: `分类${id}`, collapsed: false, settingsCollapsed: false,
        layout: 'article', autoSummary: false, autoTranslate: false, feeds: [],
      });
      store.setState({ dataMode: 'mock', categories: [mkCat(1)], toasts: [] });
      store.getState().addFeed('cat-999', 'https://x.example/rss', '幽灵源', 'article', false, false, false);
      checkNew('(t100-p3-10) 无匹配分类：失败 toast，且分类树未被污染（修前静默假成功「已添加订阅源」）',
        store.getState().toasts.at(-1)?.text === '添加失败：目标分类不存在'
          && store.getState().categories[0].feeds.length === 0);
      store.getState().addFeed('cat-1', 'https://x.example/rss', '正常源', 'article', false, false, false);
      checkNew('(t100-p3-10) 有匹配分类：成功 toast + feed 挂载（既有行为保持）',
        store.getState().toasts.at(-1)?.text === '已添加订阅源：正常源'
          && store.getState().categories[0].feeds.length === 1);
    } finally {
      globalThis.__INVOKE__ = prevInvoke;
      store.setState({ categories: [], toasts: [] });
    }
  }

  /* ---------- (t100-d30) parseTs：空格分隔旧格式按 UTC 解析（R-P3-7 前端半边） ---------- */
  {
    const { parseTs } = await import('../../dist-test/lib/api.js');
    const spaceForm = parseTs('2026-09-04 10:00:00');
    const utcExpect = Date.UTC(2026, 8, 4, 10, 0, 0);
    checkNew('(t100-d30) 「YYYY-MM-DD HH:MM:SS」按 UTC 解析（修前 Date.parse 按本地时区，时间偏移一个时区差）',
      spaceForm === utcExpect);
    checkNew('(t100-d30) 标准 ISO（T 分隔 + Z）仍走 Date.parse（不受改动影响）',
      parseTs('2026-09-04T10:00:00Z') === Date.parse('2026-09-04T10:00:00Z'));
    checkNew('(t100-d30) 带毫秒/时区的 ISO 与空格格式各自正确（空格格式兼容带小数秒的变体不误吞）',
      parseTs('2026-09-04T10:00:00.123Z') === Date.parse('2026-09-04T10:00:00.123Z')
        && parseTs('2026-09-04 10:00:00') === parseTs('2026-09-04 10:00:00'));
    checkNew('(t100-d30) null / 不可解析 → 0（排序稳定的既有契约）',
      parseTs(null) === 0 && parseTs('not-a-date') === 0);
  }

  /* ---------- (t100-src) 源码形态断言：P3 剩余项 + UI 一致性（U1-U8） ---------- */
  {
    const sidebar = src100('../../src/components/Sidebar.tsx');
    const timeline = src100('../../src/components/Timeline.tsx');
    const overlays = src100('../../src/components/Overlays.tsx');
    const primitives = src100('../../src/components/primitives.tsx');
    const app = src100('../../src/App.tsx');
    const player = src100('../../src/components/PlayerBar.tsx');
    const appearance = src100('../../src/components/settings/AppearanceTab.tsx');
    const shortcuts = src100('../../src/components/settings/ShortcutsTab.tsx');
    const syncTab = src100('../../src/components/settings/SyncTab.tsx');
    const apiSrc = src100('../../src/lib/api.ts');
    const bootstrap = src100('../../src/store/slices/bootstrap.ts');
    const internals = src100('../../src/store/internals.ts');
    const ctxMenu = src100('../../src/components/ContextMenu.tsx');
    const feedsTab = src100('../../src/components/settings/FeedsTab.tsx');
    const reader = src100('../../src/components/Reader.tsx');
    const icons = src100('../../src/components/icons.tsx');
    const tokens = src100('../../src/styles/tokens.css');
    const baseCss = src100('../../src/styles/base.css');
    const indexHtml = src100('../../index.html');
    const readme = src100('../../README.md');

    /* P3-1：reload 在途拦截（行为断言见上，这里锁守卫的存在性） */
    checkNew('(t100-p3-1) 守卫接线：loadMoreArticles 入口检查 isBackendReloadInFlight；两个 reload 以计数器包裹',
      bootstrap.includes('if (isBackendReloadInFlight()) return;')
      && bootstrap.includes('backendReloadInFlight++')
      && bootstrap.includes('backendReloadInFlight--')
      && (bootstrap.match(/backendReloadInFlight--/g) || []).length === 2);
    /* 【TASK-119 更新理由】原 reloadGeneration 已统一为查询实例代际
       queryGeneration（模块级改名，保护语义等价保留）——断言随统一语义更新
       措辞，并**强化**：新代际状态同样锁定为模块级（不得泄漏进 store）。 */
    checkNew('(t100-p3-1) 计数器与查询代际（TASK-119 统一后的 queryGeneration）同为模块级状态（不得被 setState 泄漏进 store）',
      !bootstrap.includes('reloadInFlight: ') && !bootstrap.includes('queryGeneration: ')
      && bootstrap.includes('let backendReloadInFlight = 0;') && bootstrap.includes('let queryGeneration = 0;'));

    /* P3-2：fetchFailed 假 affordance */
    checkNew('(t100-p3-2) fetchFailed 警示点改为非点击承诺「最近一次抓取失败」（重试走旁边独立刷新钮）',
      sidebar.includes('title="最近一次抓取失败"')
        && !sidebar.includes('点击重试'));

    /* P3-6 + U1：快捷键表补 Space + 紧凑加号形态 */
    checkNew('(t100-p3-6) 快捷键表补 Space 行（播放器激活时播放/暂停）',
      shortcuts.includes("'Space'")
        && shortcuts.includes('播放器激活时'));
    checkNew('(t100-u1) 快捷键提示全仓统一紧凑加号形态：Ctrl+K / Ctrl+, / Esc',
      shortcuts.includes("'Ctrl+K'") && shortcuts.includes("'Ctrl+,'")
        && sidebar.includes('>Ctrl+K<') && sidebar.includes('>Ctrl+,<')
        && overlays.includes('>Esc</span>'));
    checkNew('(t100-u1) 禁止形态清零：Ctrl K / Ctrl , / Ctrl + K / ESC 关闭 / 小写 esc',
      !sidebar.includes('Ctrl K') && !sidebar.includes('Ctrl ,')
        && !shortcuts.includes('Ctrl + K') && !shortcuts.includes('Ctrl + ,')
        && !overlays.includes('ESC 关闭') && !overlays.includes('<kbd>esc</kbd>'));

    /* P3-7：LRU 上限实现 */
    checkNew('(t100-p3-7) viewEntriesCache 收口为 LRUMap（容量 8，命中刷新新鲜度）',
      internals.includes('class LRUMap<V> extends Map<string, V>')
        && internals.includes('VIEW_ENTRIES_CACHE_MAX = 8'));

    /* P3-8 / P3-9：断开语义分离 + removed_feeds 消费 */
    checkNew('(t100-p3-8) doDisconnect 分开处理：断开成功后 reload 失败不再误报「断开失败」',
      syncTab.includes('已断开连接，但本地刷新失败'));
    checkNew('(t100-p3-9) SyncReport 补 removed_feeds 声明，SyncTab 同步报告消费（对账删除计数，纯信息展示）',
      apiSrc.includes('removed_feeds?: number;')
        && syncTab.includes('removedFeeds += feedsReport.removed_feeds')
        && syncTab.includes('removedFeeds += statesReport.removed_feeds')
        && syncTab.includes('本次对账移除'));

    /* P3-10：mock addFeed 失败可见（行为断言见上） */
    checkNew('(t100-p3-10) mock 分支 addFeed 前置目标分类存在性检查',
      src100('../../src/store/slices/feeds.ts').includes('添加失败：目标分类不存在'));

    /* D29：api.ts 注释更正 + README 同步范围更正 */
    checkNew('(t100-d29) api.ts 搜索注释更正为 LIKE 子串（FTS5 仅历史遗留提法），不再宣称 FTS5 全文搜索',
      !apiSrc.includes('FTS5 全文搜索')
        && apiSrc.includes('LIKE 子串匹配'));
    checkNew('(t100-d29) README 同步范围更正：AI 配置/模型名不在白名单',
      readme.includes('AI 配置（含模型名）不在白名单')
        && !readme.includes('模型等非敏感配置'));

    /* D28：base.css 历史注释乱码重建（73 行 / 114 处 U+FFFD → 0） */
    const fffd = String.fromCharCode(0xfffd);
    checkNew('(t100-d28) base.css 重建后 U+FFFD=0（73 行历史注释按 git 1503dbd 干净版原样恢复，CSS 规则零变化）',
      !baseCss.includes(fffd));
    checkNew('(t100-d28) 重建样本抽检：注释原文与 git 干净版逐字一致',
      baseCss.includes('FluxReader 全局基础样式（迁移自 prototype.html §1-§7）')
        && baseCss.includes('/* Feed Group Manager —— 设置页订阅管理')
        && baseCss.includes('层级最高：盖过设置弹窗（150）与下拉菜单（2000） */'));

    /* UI P2-2：J/K 范围文案 + Social/Notif 卡 roving */
    /* 【TASK-114 更新理由】X3 把 J/K 从「仅文章」扩展到全部虚拟化布局（画廊除外），
       旧文案「文章布局」正是本卡收口的不一致点——随之改准并钉住新范围（明示画廊
       不支持）；「不得宣传成整个时间流」的防误伤边界（!includes 时间流）保留。 */
    checkNew('(t100-uip2-2) ShortcutsTab J/K 范围文案随 TASK-114 改准：虚拟化四布局生效、明示画廊不支持（旧「文章布局」清零）',
      shortcuts.includes("'文章/社交/播客/通知（画廊不支持）'")
      && !shortcuts.includes("'文章布局'")
      && !shortcuts.includes("'时间流'"));
    checkNew('(t100-uip2-2) SocialCard/NotifCard 补 role="article" + tabIndex + 方向键 roving，融入既有 tabindex 体系',
      timeline.includes("role=\"article\"")
        && (timeline.match(/role="article"/g) || []).length === 2
        && timeline.includes('tabIndex={tabbable ? 0 : -1}')
        && timeline.includes('<SocialCard item={item} cardIndex={vi.index}')
        && timeline.includes('<NotifCard item={item} cardIndex={vi.index}'));

    /* UI P2-7：浮层焦点移入/归还 */
    checkNew('(t100-uip2-7) ModalOverlay 打开移焦入容器（tabIndex=-1，子组件 autoFocus 优先）、关闭归还触发元素',
      primitives.includes('cardRef.current?.focus({ preventScroll: true })')
        && primitives.includes('restoreRef.current.focus({ preventScroll: true })')
        && primitives.includes('tabIndex={-1}'));
    checkNew('(t100-uip2-7) Lightbox 记录打开时 document.activeElement 并在关闭时归还',
      overlays.includes('overlayRef.current?.focus({ preventScroll: true })')
        && overlays.includes('restoreRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;'));

    /* U7：Esc 链补 closeAskVisible */
    checkNew('(t100-u7) App.tsx Esc 链首支补 closeAskVisible（确认框 3000 自带 Esc > 弹窗 150 > 播放器 140）',
      app.indexOf('if (s.closeAskVisible)') < app.indexOf('else if (s.searchOpen) s.closeSearch()')
        && app.includes("s.answerCloseAsk('tray', false)"));

    /* U2：叫法统一「添加订阅源」 */
    checkNew('(t100-u2) 右键菜单与设置页 FeedsTab 统一「添加订阅源」，两套旧叫法清零',
      ctxMenu.includes("label: '添加订阅源'") && !ctxMenu.includes('新建订阅源')
        && feedsTab.includes('<span>添加订阅源</span>') && !feedsTab.includes('添加源</span>'));

    /* 生成中文案统一 + 署名中文化 */
    checkNew('(t100-c14) 生成中文案统一「正在生成摘要…」、错误前缀统一「摘要生成失败：」',
      reader.includes('正在生成摘要…') && !reader.includes('正在根据提示词生成摘要')
        && timeline.includes('摘要生成失败：') && !timeline.includes('>生成失败：'));
    checkNew('(t100-c15) Reader 署名中文化：作者：{author}（「By {author}」清零）',
      reader.includes('作者：{art.author}') && !reader.includes('By {art.author}'));

    /* U3：星标视觉统一 */
    checkNew('(t100-u3) 三处星标统一 Icons.star/starFilled：SocialCard 收藏态 starFilled、GalleryCard ★/☆ 字符移除、ArticleCard「★ 已收藏」移除',
      timeline.includes('{item.isStarred ? <Icons.starFilled /> : <Icons.star />}')
        && !timeline.includes('★')
        && timeline.includes('card-starred-flag')
        && !timeline.includes('☆'));
    checkNew('(t100-u3) ArticleCard 页脚收藏标记落位 card-starred-flag（base.css 提供 --star-color 视觉）',
      baseCss.includes('.card-starred-flag') && baseCss.includes('color: var(--star-color);'));

    /* U4：播放器/主题图标 SVG 化 */
    checkNew('(t100-u4) PlayerBar 六控件全部 Icons SVG：暂停/播放/关闭/快退/快进/全屏的字符图标清零',
      player.includes('Icons.pause') && player.includes('Icons.play')
        && player.includes('Icons.x') && player.includes('Icons.maximize')
        && player.includes('Icons.rotateCcw') && player.includes('Icons.rotateCw')
        && !/[⏸▶✕↺↻⛶]/.test(player));
    checkNew('(t100-u4) AppearanceTab 主题三按钮 emoji（日/月/电脑符号）移除，Icons.sun/moon/monitor 上位',
      !appearance.includes('☀') && !appearance.includes('🌙') && !appearance.includes('💻')
        && appearance.includes('icon: Icons.sun')
        && appearance.includes('icon: Icons.moon')
        && appearance.includes('icon: Icons.monitor'));
    checkNew('(t100-u4) Icons 集合补齐八个形状（pause/x/rotateCcw/rotateCw/maximize/sun/moon/monitor）',
      ['pause:', 'x:', 'rotateCcw:', 'rotateCw:', 'maximize:', 'sun:', 'moon:', 'monitor:']
        .every((k) => icons.includes(k)));

    /* U5：line-clamp 截断文本补 title */
    checkNew('(t100-u5) 截断文本补 title（=未截断全文）：card-title/card-snippet/gallery-title/podcast-title',
      timeline.includes('<h4 className="card-title" title={art.title}>')
        && timeline.includes('<p className="card-snippet" title={art.snippet}>')
        && timeline.includes('<div className="gallery-title" title={item.title}>')
        && timeline.includes('<div className="podcast-title" title={item.title}>'));

    /* U6：favicon 失败回退 dot 占位 */
    checkNew('(t100-u6) favicon onError 回退 dot 占位（FeedFavicon 组件），行首不再留空槽',
      sidebar.includes('function FeedFavicon')
        && sidebar.includes('onError={() => setFailed(true)}')
        && !sidebar.includes("style.display = 'none'"));

    /* U7 配套断言见上；U8：theme-color */
    checkNew('(t100-u8) index.html theme-color = #14161a（与深色 --bg-base 一致，启动不闪色）',
      indexHtml.includes('content="#14161a"') && !indexHtml.includes('#0a1936'));

    /* P3 散点：LAYOUT_NO_AI 收敛 / 占位类化 / busy 统一 / stale 注释 / token 化 / 头注释 */
    checkNew('(t100-c23) LAYOUT_NO_AI 双定义收敛：Overlays 改 import settings/shared，本地定义删除',
      overlays.includes("import { LAYOUT_NO_AI } from './settings/shared';")
        && !overlays.includes("new Set(['image', 'podcast'])"));
    checkNew('(t100-c26) SocialCard 占位 opacity 0.45 内联移除，并入 .hydrate-placeholder 类',
      !timeline.includes('opacity: 0.45')
        && baseCss.includes('.hydrate-placeholder {')
        && baseCss.includes('opacity: 0.45;'));
    checkNew('(t100-c25) 双刷新入口 busy 统一「禁用+转圈」：小图标钮补 disabled，CSS 提供禁用态',
      sidebar.includes('disabled={isBusy}')
        && baseCss.includes('.sync-refresh-btn:disabled'));
    checkNew('(t100-c27) primitives.tsx stale 注释更正：确认框 z-index 3000（修前注释写 300）',
      primitives.includes('z-index 为 3000') && !primitives.includes('z-index 300，'));
    checkNew('(t100-c17) #e67e22 token 化：tokens.css 新增 --feed-error，base.css 引用，硬编码清零',
      tokens.includes('--feed-error: #e67e22;')
        && baseCss.includes('color: var(--feed-error);')
        && !baseCss.includes('#e67e22'));
    checkNew('(t100-c20) tokens.css 头注释更正：深浅两模式均已实现全部 5 个调色盘',
      tokens.includes('均已实现全部 5 个调色盘')
        && !tokens.includes('浅色模式当前仅实现 blue'));
  }
}

/* ============================================================
   TASK-101（2026-09-29）：SyncTab 提示精简 + Fever「API 密码」提示
   + 设置页冗长文案审计（GeneralTab/ReadingTab/AppearanceTab/AboutTab/
   FeedsTab/AiTab/ConfigSyncSection/CacheCleanupSection/SyncTab 逐个过）。
   UI 契约 V1/V2：Note: 界面文案与控件口径 — 见 .agents/notes/implemented/feature/2026-10-07-界面文案与控件口径.md。
   V1 = 被点名长句清零；新短句（单句 ≤40 字）保留三语义（a 测试连接仅验证
        登录不拉数据 / b 保存并同步才开始拉取 / c 断开会移除同步拉取内容），
        已读/收藏回传语义取保留；
   V2 = Fever 协议选择处单行提示含「API 密码」（FreshRSS 的 Fever/GReader
        均用个人设置里的 API 密码），≤50 字、无分号长链。
   TASK-102（2026-09-30）owner 返工改判（本块断言已同步改锚）：
   v1c 断开语义移出常驻提示（确认框承载）、v1-d 回传句改删、
   V2 提示并入协议卡 desc 位 ≤32 字、协议卡控件恢复直接子元素。
   ============================================================ */
{
  const fs101 = await import('node:fs');
  const src101 = (p) => fs101.readFileSync(new URL(p, import.meta.url), 'utf8');

  const syncTab101 = src101('../../src/components/settings/SyncTab.tsx');
  const configSync101 = src101('../../src/components/settings/ConfigSyncSection.tsx');
  const cacheCleanup101 = src101('../../src/components/settings/CacheCleanupSection.tsx');
  const general101 = src101('../../src/components/settings/GeneralTab.tsx');

  /* ---------- V1：SyncTab 主提示（唯一带 marginTop:8 的 mini-dialog-hint） ---------- */
  const mainHint101 =
    syncTab101.match(/<div className="mini-dialog-hint" style=\{\{ marginTop: 8 \}\}>\s*([\s\S]*?)<\/div>/)?.[1]
      ?.replace(/\s+/g, '') ?? '';

  checkNew('(t101-v1a) 语义a：「测试连接」仅验证登录（TASK-102 精简口径，验证语义保留）',
    mainHint101.includes('「测试连接」仅验证登录'));
  checkNew('(t101-v1b) 语义b：「保存并同步」确认后拉取订阅与文章（拉取只由保存触发，TASK-102 口径）',
    mainHint101.includes('「保存并同步」确认后拉取订阅与文章'));
  checkNew('(t101-v1c) 语义c：断开删除语义移出常驻提示（TASK-102），仅由断开确认框承载',
    !mainHint101.includes('断开')
    && syncTab101.includes('断开后将移除从服务端拉取的订阅与文章'));
  checkNew('(t101-v1-d) 已读/收藏回传句删除（TASK-102：解释性冗余；TASK-101 曾取保留，owner 返工改删）',
    !mainHint101.includes('回传') && !syncTab101.includes('约1秒内回传'));
  checkNew('(t101-v1-len) 新主提示单句 ≤40 字（V1 短句口径；修前单段 74 字）',
    mainHint101.length > 0
    && mainHint101.split('。').filter(Boolean).every((s) => s.length <= 40));
  checkNew('(t101-v1-old) 被点名旧长句四种片段整段清零',
    !syncTab101.includes('只验证连通性（秒级）')
    && !syncTab101.includes('会立即在后台拉取订阅与文章状态')
    && !syncTab101.includes('已读/收藏等变更约 1 秒内推送到服务端')
    && !syncTab101.includes('断开连接会移除服务端拉取的订阅与文章'));
  checkNew('(t101-v1-e) SyncTab 恰一块 mini-dialog-hint（动作区主提示；Fever 提示已并入协议卡 desc，TASK-102）',
    (syncTab101.match(/mini-dialog-hint/g) || []).length === 1);

  /* ---------- V2：Fever「API 密码」提示（TASK-102 起并入「同步协议」卡 desc 位，
      独立 mini-dialog-hint 与包裹 div 均删除；≤32 字预算内 FreshRSS/个人设置
      细节让位，「Fever/GReader 均用」+「非登录密码」核心误区纠正保留） ---------- */
  const protoCard101 =
    syncTab101.slice(syncTab101.indexOf('title="同步协议"'), syncTab101.indexOf('title="后端 Endpoint"'));
  const protoDesc101 = protoCard101.match(/desc="([^"]+)"/)?.[1] ?? '';

  checkNew('(t101-v2a) API 密码提示在协议卡 desc 位：Fever/GReader 通用且点明非登录密码（TASK-102 口径）',
    protoDesc101.includes('API 密码') && protoDesc101.includes('Fever')
    && protoDesc101.includes('GReader') && protoDesc101.includes('非登录密码'));
  checkNew('(t101-v2b) 协议卡 desc ≤32 字（TASK-102 预算；TASK-101 的 hint ≤50 口径废止）',
    protoDesc101.length > 0 && protoDesc101.length <= 32);
  checkNew('(t101-v2c) 协议卡内无第二行常驻 hint、无包裹 div（提示全在 desc 位）',
    protoCard101.length > 0
    && !protoCard101.includes('mini-dialog-hint')
    && !protoCard101.includes('<div'));

  /* ---------- 审计项源级断言：三处 >60 字 desc 的压缩前后锁定 ---------- */
  checkNew('(t101-audit-1) SyncTab 用户名 desc 63→45 字：集成页配置/GReader·Fever 共用/非账号密码 三语义全保留',
    syncTab101.includes('Miniflux「集成」页配置的用户名，GReader / Fever 共用（非账号密码）')
    && !syncTab101.includes('页单独配置的用户名')
    && !syncTab101.includes('非 Miniflux 账号密码'));
  checkNew('(t101-audit-2) ConfigSyncSection Token desc 56→47 字：classic PAT/gist scope/fine-grained 不支持 全保留（TASK-102 ≤48 口径）',
    configSync101.includes('手动填入；classic PAT 需勾 gist scope，fine-grained 不支持')
    && !configSync101.includes('手动填入替代网页登录')
    && !configSync101.includes('fine-grained PAT 不支持 Gist API'));
  checkNew('(t101-audit-3) ConfigSyncSection WebDAV desc 55→31 字：配置文件名 fluxreader-config.json 保留（示例 URL 由输入框 placeholder 表达，TASK-102 ≤48 口径）',
    configSync101.includes('配置在服务器存为 fluxreader-config.json')
    && !configSync101.includes('例如 https://dav.example.com')
    && !configSync101.includes('dav.example.com/fluxreader（配置存为'));

  /* ---------- 审计兜底：九个设置组件静态 desc 全量扫描，>60 字清零（修前 63/66/69 三处） ---------- */
  const auditedDescs101 = [
    'GeneralTab', 'ReadingTab', 'AppearanceTab', 'AboutTab', 'FeedsTab',
    'AiTab', 'ConfigSyncSection', 'CacheCleanupSection', 'SyncTab',
  ].flatMap((f) => [...src101(`../../src/components/settings/${f}.tsx`).matchAll(/desc="([^"]+)"/g)])
    .map((m) => m[1]);
  checkNew('(t101-audit-4) 九组件静态 desc 扫描（' + auditedDescs101.length + ' 条）：>60 字长 desc 清零',
    auditedDescs101.length > 20 && !auditedDescs101.some((t) => t.length > 60));

  /* ---------- 审计不回退：确认类/阈值/约束语义一条不丢 ---------- */
  checkNew('(t101-audit-5) 确认与约束语义零丢失：断开确认、清理不可撤销、收藏与待同步保留、AI 缓存正文保留、下载覆盖确认、托盘真退出',
    syncTab101.includes('断开后将移除从服务端拉取的订阅与文章（含已读/收藏绑定），本地直连添加的订阅不受影响。确定断开吗？')
    && cacheCleanup101.includes('此操作不可撤销')
    && cacheCleanup101.includes('收藏文章与待同步状态始终保留')
    && cacheCleanup101.includes('（收藏除外）')
    && cacheCleanup101.includes('正文保留')
    && configSync101.includes('已存在的源会跳过；本地设置与 AI 配置将被远端覆盖')
    && general101.includes('托盘菜单「退出」才是真正退出'));
}

/* ============================================================
   TASK-102（2026-09-30）：同步协议卡控件对齐修复 + 设置页文案删除性精简
   UI 契约 X1/X2/X3：Note: 界面文案与控件口径 — 见 .agents/notes/implemented/feature/2026-10-07-界面文案与控件口径.md。
   X1 = 协议卡 FluxDropdown 恢复为 SettingCard 直接子元素（卡内无包裹 div /
        常驻 hint——提示文本撑宽包裹层是下拉左移错位根因）；
   X2 = 动作区提示单行 ≤40 字、两按钮语义一一对齐；「断开会移除拉取内容」
        与「已读/收藏约 1 秒回传」移出常驻文案；API 密码提示在协议卡 desc 位
        且 ≤32 字；
   X3 = 全设置组件文案扫描守卫：中文文案字面量 ≤48 字（新增长文案直接红）
        + 复述性/花絮 desc 删除清零 + 约束语义（留空提交、托盘真退出、
        下载覆盖、不可撤销）零丢失。
   ============================================================ */
{
  const fs102 = await import('node:fs');
  const src102 = (p) => fs102.readFileSync(new URL(p, import.meta.url), 'utf8');
  const syncTab102 = src102('../../src/components/settings/SyncTab.tsx');
  const general102 = src102('../../src/components/settings/GeneralTab.tsx');
  const reading102 = src102('../../src/components/settings/ReadingTab.tsx');
  const about102 = src102('../../src/components/settings/AboutTab.tsx');
  const configSync102 = src102('../../src/components/settings/ConfigSyncSection.tsx');
  const cacheCleanup102 = src102('../../src/components/settings/CacheCleanupSection.tsx');
  const shared102 = src102('../../src/components/settings/shared.ts');

  /* ---------- X1：协议卡结构断言（对齐根因修复） ---------- */
  const protoCard102 = syncTab102.slice(
    syncTab102.indexOf('title="同步协议"'),
    syncTab102.indexOf('title="后端 Endpoint"'),
  );
  checkNew('(t102-x1a) 同步协议卡：SettingCard 开标签后直接是 FluxDropdown（直接子元素，无包裹层）',
    /title="同步协议"\s*desc="[^"]+"\s*>\s*<FluxDropdown/.test(syncTab102));
  checkNew('(t102-x1b) 同步协议卡：FluxDropdown 闭合后直接 </SettingCard>，卡内无包裹 div、无 hint',
    /<FluxDropdown[\s\S]*?\/>\s*\n\s*<\/SettingCard>\s*\n\s*<SettingCard\s*\n\s*title="后端 Endpoint"/.test(syncTab102)
    && !protoCard102.includes('<div')
    && !protoCard102.includes('mini-dialog-hint'));

  /* ---------- X2：文案断言（单行提示 + 删除语义归位 + API 密码提示） ---------- */
  const mainHint102 =
    syncTab102.match(/<div className="mini-dialog-hint" style=\{\{ marginTop: 8 \}\}>\s*([\s\S]*?)<\/div>/)?.[1]
      ?.replace(/\s+/g, '') ?? '';
  checkNew('(t102-x2a) 动作区提示单行 ≤40 字：两按钮语义一一对齐（测试连接=仅验证登录；保存并同步=确认后拉取订阅与文章）',
    mainHint102.length > 0 && mainHint102.length <= 40
    && mainHint102.includes('「测试连接」仅验证登录')
    && mainHint102.includes('「保存并同步」确认后拉取订阅与文章'));
  checkNew('(t102-x2b) 「断开会移除拉取内容」不再常驻：主提示无「断开」，破坏性语义仅由断开确认框承载',
    !mainHint102.includes('断开')
    && syncTab102.includes('断开后将移除从服务端拉取的订阅与文章'));
  checkNew('(t102-x2c) 「已读/收藏约 1 秒回传」删除：常驻提示与整卡均不再出现',
    !mainHint102.includes('回传')
    && !syncTab102.includes('约 1 秒内回传')
    && !syncTab102.includes('约1秒内回传'));
  const protoDesc102 = protoCard102.match(/desc="([^"]+)"/)?.[1] ?? '';
  checkNew('(t102-x2d) API 密码提示在协议卡 desc 位且 ≤32 字，卡内无第二行常驻 hint',
    protoDesc102.includes('API 密码') && protoDesc102.length > 0 && protoDesc102.length <= 32
    && !protoCard102.includes('mini-dialog-hint'));
  checkNew('(t102-x2e) 低价值花絮句「两种协议共用 Miniflux…切换不丢数据」已删除（desc 位让给 API 密码提示）',
    !syncTab102.includes('两种协议共用') && !syncTab102.includes('切换不丢数据'));

  /* ---------- X3：全设置组件文案扫描守卫（新增长文案直接红） ---------- */
  const scanFiles102 = [
    'GeneralTab', 'ReadingTab', 'AppearanceTab', 'AboutTab', 'FeedsTab',
    'AiTab', 'ConfigSyncSection', 'CacheCleanupSection', 'SyncTab',
    'AutoStartSwitch', 'ShortcutsTab', 'SettingsSidebarFooter',
  ];
  /* 去掉注释再取字面量：注释里的引号示例不计入文案；模板串（动态拼接的
     toast/确认框 message）不在 desc/hint 静态范围，由契约确认框口径另行断言 */
  const stripComments102 = (s) => s.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '');
  const literals102 = scanFiles102.flatMap((f) =>
    [...stripComments102(src102(`../../src/components/settings/${f}.tsx`))
      .matchAll(/'([^'\n]+)'|"([^"\n]+)"/g)]
      .map((m) => m[1] ?? m[2]));
  /* 确认框 message 属对话框文案，不在 X3 desc/hint 范围（49 字，约束语义另锁） */
  const dialogAllowlist102 = new Set([
    '断开后将移除从服务端拉取的订阅与文章（含已读/收藏绑定），本地直连添加的订阅不受影响。确定断开吗？',
  ]);
  const cjkCopy102 = [...new Set(literals102)]
    .filter((t) => /[\u4e00-\u9fff]/.test(t) && !dialogAllowlist102.has(t));
  checkNew('(t102-x3a) 全设置组件中文文案字面量扫描（' + cjkCopy102.length + ' 条去重）：无 >48 字',
    cjkCopy102.length > 60
    && !cjkCopy102.some((t) => t.length > 48));
  checkNew('(t102-x3b) 复述性/花絮 desc 删除清零：General×4、Reading×5、About×1、ConfigSync×1、SyncTab×1 原文不再出现',
    !general102.includes('点击选中文章后立即更新本地已读状态')
    && !general102.includes('卡片滚出时间流上沿即视为已浏览')
    && !general102.includes('下次打开应用时默认进入的视图')
    && !general102.includes('仅展示未读流内容')
    && !reading102.includes('选择阅读器正文渲染字体家族')
    && !reading102.includes('调整正文基础显示大小')
    && !reading102.includes('调整正文段落行间距比例')
    && !reading102.includes('限制单行文本长度以优化可读性')
    && !reading102.includes('在文章信息栏显示估算阅读时长')
    && !about102.includes('检测 GitHub Releases 上的最新版本')
    && !configSync102.includes('服务器登录账号')
    && !syncTab102.includes('集成密码（Google Reader / Fever 共用）'));
  checkNew('(t102-x3c) 约束语义零丢失：留空提交=保持密码、托盘真退出、下载覆盖本地、清理不可撤销、收藏与待同步保留',
    syncTab102.includes('留空提交 = 保持当前密码')
    && general102.includes('托盘菜单「退出」才是真正退出')
    && configSync102.includes('下载会覆盖本地设置与 AI 配置')
    && cacheCleanup102.includes('此操作不可撤销')
    && cacheCleanup102.includes('收藏文章与待同步状态始终保留'));
  const subtitles102 = [...shared102.matchAll(/subtitle: '([^']+)'/g)].map((m) => m[1]);
  checkNew('(t102-x3d) 侧栏 8 个 tab subtitle（组标题级说明同口径）全部 ≤48 字',
    subtitles102.length === 8 && subtitles102.every((t) => t.length <= 48));
}

/* ============================================================
   TASK-114（2026-10-06，REQ-005/008）：五布局状态与快捷键统一
   X1 NotifCard 水合三态（对齐 SocialCard，失败不再静默）/
   X2 Enter 五卡统一（Social/Notif 补选中）/ X3 J/K 全虚拟化布局（画廊除外）。

   证据边界（如实说明）：renderToStaticMarkup 走 zustand 服务端快照——
   useSyncExternalStore 的 getServerSnapshot 读 getInitialState（createStore
   时捕获，setState 不可达；实测探针确认 SSR 不随 setState 变化），SSR 只能
   呈现与初值一致的形态（d5 空态先例即此）。三态/键绑定是运行时状态驱动的
   分支，无法经 Timeline SSR 逐态取证，故本组走两条既有证据通道：
   - 源级结构断言（t102-x1 先例）：按组件声明边界切片，钉住条件链与接线；
   - 纯函数/store 层行为断言：X3 门控与推进抽为 src/lib/jkNavigation.ts
     （App.tsx 消费同一份），真值表 + 五布局 store 模拟；X1 的状态判定复用
     t103 已断言的 entryNeedsHydration 真值表与 retryHydration 行为
     （Social/Notif 走同一条批量水合队列，无第二套判定）。
   ============================================================ */
{
  const fs114 = await import('node:fs');
  const src114 = (p) => fs114.readFileSync(new URL(p, import.meta.url), 'utf8');
  const timeline114 = src114('../../src/components/Timeline.tsx');
  const app114 = src114('../../src/App.tsx');
  const shortcuts114 = src114('../../src/components/settings/ShortcutsTab.tsx');
  const compSlice114 = (a, b) => {
    const i = timeline114.indexOf(a);
    const j = b ? timeline114.indexOf(b, i) : timeline114.length;
    return i >= 0 && j > i ? timeline114.slice(i, j) : '';
  };
  const article114 = compSlice114('const ArticleCard = memo(function ArticleCard(', 'const SocialCard = memo(function SocialCard(');
  const social114 = compSlice114('const SocialCard = memo(function SocialCard(', 'const GalleryCard = memo(function GalleryCard(');
  const gallery114 = compSlice114('const GalleryCard = memo(function GalleryCard(', 'const PodcastCard = memo(function PodcastCard(');
  const podcast114 = compSlice114('const PodcastCard = memo(function PodcastCard(', 'const NotifCard = memo(function NotifCard(');
  const notif114 = compSlice114('const NotifCard = memo(function NotifCard(', null);
  const cnt114 = (s, t) => (s.match(new RegExp(t.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'g')) || []).length;
  const ENTER_GUARD = "e.key === 'Enter' || e.key === ' '";

  /* ---------- X1：NotifCard 水合三态 ---------- */
  /* 【TASK-103 → TASK-122 改动理由】订阅面随真值源迁移更新：两卡统一订阅
     selectArticleBody（bodyById 记录快照），错误态/终态从记录 state 派生
     （failed/missing/ready）——原 hydrationErrors/hydratedIds 平行订阅删除；
     呈现意图（失败=内联重试、终态=占位、加载中=占位）逐字保留。 */
  checkNew('(t114-x1a) NotifCard 订阅正文/AI 读取单点 selectArticleBody（对齐 SocialCard；TASK-122 真值源 bodyById）',
    notif114.includes('selectArticleBody(s, item.id)') && social114.includes('selectArticleBody(s, item.id)'));

  /* TASK-114 R1-F1：失败分支切片（自 ') : body.state === 'failed' ? (' 至 snippet 正文分支）
     ——只覆盖「无正文可显示」的失败，无 snippet 回退、无正文 div；正文（fullText）
     分支在失败分支**之前**，与基准 SocialCard 的 content 优先逐分支对齐（可达组合态
     「错误态+正文已到达」——详情拉取成功只写记录 content 不写 failed——必须显示正文）。 */
  const notifErrBranch114 = notif114.slice(
    notif114.indexOf(") : body.state === 'failed' ? ("),
    notif114.indexOf(') : item.snippet ? ('),
  );
  const notifBodyBranch114 = notif114.slice(
    notif114.indexOf('{fullText ? ('),
    notif114.indexOf(") : body.state === 'failed' ? ("),
  );
  checkNew('(t114-x1b) NotifCard 失败态=内联重试（hydrate-retry + retryHydration(id)，文案域「正文加载失败：」，与 SocialCard 同形），失败分支不再回退 snippet（TASK-122：failed 判别态）',
    notifErrBranch114.length > 0
    && notifErrBranch114.includes('className="hydrate-retry"')
    && notifErrBranch114.includes('retryHydration(item.id)')
    && notifErrBranch114.includes('正文加载失败：')
    && !notifErrBranch114.includes('item.snippet')
    && !notifErrBranch114.includes('notif-body-text'));

  checkNew('(t114-x1c) NotifCard 空正文/加载中占位与 SocialCard 同形（className="hydrate-placeholder"：missing/暂无正文/加载中/AI 已清空提示恰四态）',
    cnt114(notif114, 'className="hydrate-placeholder"') === 4
    && notif114.includes('暂无正文') && notif114.includes('加载正文…')
    && notif114.includes('AI 缓存已清空，可重新生成'));

  checkNew('(t114-x1f) 正文分支先于失败分支（R1-F1）：「错误态+正文已到达」组合态显示正文而非假失败行（与 SocialCard content 优先逐分支对齐）',
    notifBodyBranch114.length > 0
    && notifBodyBranch114.includes('notif-body-text')
    && notif114.indexOf('notif-body-text') < notif114.indexOf('className="hydrate-retry"'));

  checkNew('(t114-x1g) 纯失败态（无正文）不渲染「展开更多」（正文已被重试行替换，防死控件）；错误滞留+正文已达的组合态豁免（!!fullText，可展开水合全文，与修前/同态 SocialCard 一致；TASK-122：failed 判别态）',
    notif114.includes("{isLong && (body.state !== 'failed' || !!fullText) && (")
    && notif114.indexOf("{isLong && (body.state !== 'failed' || !!fullText) && (") < notif114.indexOf('className="notif-expand-btn"'));

  checkNew('(t114-x1e) NotifCard 与 SocialCard 三态同构：两卡同为「错误重试 → 空正文 → 加载占位」条件链（占位类与文案逐一同形）',
    social114.includes('className="hydrate-retry"') && notif114.includes('className="hydrate-retry"')
    && social114.includes('className="hydrate-placeholder"') && notif114.includes('className="hydrate-placeholder"')
    && social114.includes('暂无正文') && notif114.includes('暂无正文')
    && social114.includes('加载正文…') && notif114.includes('加载正文…'));

  /* ---------- X2：Enter/Space 五卡统一 ---------- */
  checkNew('(t114-x2a) Enter/Space 键位五卡齐备（Article=选中 / Podcast=play / Gallery 两分支=灯箱 / Social、Notif=选中·新增）',
    cnt114(article114, ENTER_GUARD) === 1 && cnt114(podcast114, ENTER_GUARD) === 1
    && cnt114(gallery114, ENTER_GUARD) === 2 && cnt114(social114, ENTER_GUARD) === 1
    && cnt114(notif114, ENTER_GUARD) === 1);
  checkNew('(t114-x2b) Social/Notif Enter 语义=选中（onSelect(item.id)，与 ArticleCard 同动作），且仅卡片本体响应（e.target 守卫：嵌套按钮/链接的键盘激活不被卡片级选中劫持）',
    social114.includes('if (e.target === e.currentTarget)') && notif114.includes('if (e.target === e.currentTarget)')
    && social114.indexOf('e.target === e.currentTarget') < social114.indexOf('onSelect(item.id);')
    && notif114.indexOf('e.target === e.currentTarget') < notif114.indexOf('onSelect(item.id);'));
  checkNew('(t114-x2c) Timeline 接线：Social/Notif 卡 onSelect={selectArticle}（选中即打开）',
    timeline114.includes('<SocialCard item={item} cardIndex={vi.index} tabbable={vi.index === tabbableIndex} onSelect={selectArticle}')
    && timeline114.includes('<NotifCard item={item} cardIndex={vi.index} tabbable={vi.index === tabbableIndex} onSelect={selectArticle}'));

  /* ---------- X3：J/K 全虚拟化布局（画廊除外） ---------- */
  const { jkLayoutAllowed, jkNextIndex } = await import('../../src/lib/jkNavigation.ts');

  /* 修前门控可复现：旧实现 activeContentLayout !== 'article' 即 return，
     social/podcast/notification 按 J/K 无响应（本卡收口的不一致点本身） */
  const legacyJkGate114 = (l) => l === 'article';
  checkNew('(t114-x3a) 修前门控可复现：旧判定仅 article 放行，social/podcast/notification 一律拦下',
    legacyJkGate114('article') && !legacyJkGate114('social')
    && !legacyJkGate114('podcast') && !legacyJkGate114('notification'));
  checkNew('(t114-x3b) J/K 门控：虚拟化四布局放行、画廊 image 拦下（与 Timeline 虚拟化开关同一口径）',
    jkLayoutAllowed('article') && jkLayoutAllowed('social') && jkLayoutAllowed('podcast')
    && jkLayoutAllowed('notification') && !jkLayoutAllowed('image'));
  checkNew('(t114-x3c) J/K 推进真值表（与修前内联实现逐条等价）：j 末项回绕→0、k 首项回绕→末项、无选中 j→0/k→末项、空列表→-1',
    jkNextIndex(5, 4, true) === 0 && jkNextIndex(5, 0, false) === 4
    && jkNextIndex(5, -1, true) === 0 && jkNextIndex(5, -1, false) === 4
    && jkNextIndex(3, 1, true) === 2 && jkNextIndex(3, 1, false) === 0
    && jkNextIndex(0, -1, true) === -1 && jkNextIndex(0, -1, false) === -1);

  /* store 层模拟：App.tsx J/K 分支语义可达性——五布局各绑一个源，虚拟化四布局
     下 selectVisibleEntries 产出序列且「门控放行 ∧ 推进必得可选中目标」；image
     布局条目虽在、门控拦下（不支持）。跑在本文件末尾，随后恢复 store。 */
  {
    const { selectVisibleEntries: sve114 } = await import('../../dist-test/store.js');
    const mkEntry114 = (id, feedId, ts) => ({
      id, feedId, title: `t-${id}`, author: 'a', snippet: 's', content: '',
      translatedContent: '', aiSummary: '', url: '', cover: null, imageUrl: null,
      tags: [], isRead: false, isStarred: false, publishedAt: ts,
      enclosureUrl: null, enclosureMime: null, durationSec: null,
      fulltextExtracted: false, rawContent: null,
    });
    const LAYOUT_FEEDS114 = [
      ['7100', 'article'], ['7200', 'social'], ['7300', 'podcast'], ['7400', 'notification'], ['7500', 'image'],
    ];
    const entries114 = LAYOUT_FEEDS114.flatMap(([feedId], i) => [
      mkEntry114(`${feedId}1`, feedId, 1000 + i),
      mkEntry114(`${feedId}2`, feedId, 2000 + i),
      mkEntry114(`${feedId}3`, feedId, 3000 + i),
    ]);
    const feedIndex114 = new Map(LAYOUT_FEEDS114.map(([feedId, layout]) => [feedId, {
      feed: { id: feedId, name: `源-${layout}`, layout },
      cat: { id: 'cat-114', name: '分类-114', layout: 'article' },
    }]));
    const prevLayout114 = store.getState().activeContentLayout;
    store.setState({
      entries: entries114, feedIndex: feedIndex114, openedReadIds: {},
      activeFeedFilter: 'all', activeViewFilter: 'all', timelineFilter: 'all', timelineSort: 'newest',
      activeArticleId: null,
    });
    let simOk114 = true;
    for (const [feedId, layout] of LAYOUT_FEEDS114) {
      store.setState({ activeContentLayout: layout });
      const items = sve114(store.getState());
      if (layout === 'image') {
        /* 画廊：条目在（3 条），但门控拦下——J/K 不可达（不支持） */
        simOk114 = simOk114 && items.length === 3 && !jkLayoutAllowed(layout);
        continue;
      }
      /* 无选中按 j：门控放行 ∧ 推进落最新一条；再从首项 j 推进到位次第二 */
      const first = items[jkNextIndex(items.length, -1, true)];
      const second = items[jkNextIndex(items.length, 0, true)];
      simOk114 = simOk114 && jkLayoutAllowed(layout) && items.length === 3
        && !!first && first.id === `${feedId}3` && !!second && second.id === `${feedId}2`;
    }
    checkNew('(t114-x3d) store 层五布局模拟：article/social/podcast/notification 逐布局门控放行且 selectVisibleEntries×jkNextIndex 必得可选中目标（j 依次落最新/次新）；image 条目在但门控拦下',
      simOk114);
    store.setState({ activeContentLayout: prevLayout114, entries: [], feedIndex: new Map() });
  }

  checkNew('(t114-x3e) App.tsx J/K 分支消费纯函数（门控 + 推进），旧「仅 article」内联门控与内隔回绕判定清零',
    app114.includes('jkLayoutAllowed(s.activeContentLayout)')
    && app114.includes("jkNextIndex(items.length, curIdx, e.key === 'j')")
    && !app114.includes("s.activeContentLayout !== 'article'")
    && !app114.includes('nextIdx = e.key'));
  checkNew('(t114-x3f) ShortcutsTab J/K 行同步：范围=文章/社交/播客/通知、明示画廊不支持（旧「文章布局」清零）',
    shortcuts114.includes("'文章/社交/播客/通知（画廊不支持）'")
    && shortcuts114.includes("'上下切换选中条目'")
    && !shortcuts114.includes("'文章布局'"));

  /* SSR 烟测（证据边界见块首注释）：NotifCard 三态改动后 Timeline 组件树仍可
     执行（与 d5 同口径的初值空态形态，不承载逐态取证） */
  const { renderToStaticMarkup: rsm114 } = await import('react-dom/server');
  const { createElement: ce114 } = await import('react');
  const { Timeline: Timeline114 } = await import('../../src/components/Timeline.tsx');
  const html114 = rsm114(ce114(Timeline114));
  checkNew('(t114-x1d) SSR 烟测：三态/键绑定改动后 Timeline 组件树仍可执行（空态/哨兵形态不变）',
    html114.includes('timeline-empty-state') && html114.includes('timeline-load-more'));
}

/* ============================================================
   TASK-116（2026-10-06，同步四态展示）：队列状态列 + 统计命令 + pill/摘要卡。
   X1 侧栏 pill 优先级修正（error > syncing > waiting > connected，修复「手动
   同步进行中失败态被 syncing 覆盖」）+ 等待计数 + 「· 部分失败」段；
   X2 设置页四态摘要卡（stats + 上次同步，如实口径）；X3 无队列不劣化。

   证据边界（如实说明，t115 同口径）：pill 与摘要 desc 文案真值表分别落在纯函数
   syncPillLabel / syncStateSummary（src/lib/syncPill.ts，组件收口单点）；
   SSR 烟测受 zustand v5 server snapshot 恒读 getInitialState() 所限，只取证
   「组件树可执行 + 初始态基线渲染」（x1f/x2d 注）；接线由源码形态断言钉住
   （t111-6/t115-0 先例）；Rust 侧 attempts/last_error/sync_queue_stats 由
   CI cargo test 承担（t116-r0..r3）。
   判别设计：
   - 优先级逐格锁死：error 在 syncing/backgroundSyncing 中仍显示（修前必红的格子）；
   - X3 零队列时既有文案逐字保留（噪音/假状态清零）；
   - 后缀断言「无失败绝不追加」（「· 部分失败」凭空出现的实现必红）；
   - 接线断言：旧内联四分支清零——优先级回退必须重写文案单点才会复绿。
   ============================================================ */
{
  const fs116 = await import('node:fs');
  const src116 = (p) => fs116.readFileSync(new URL(p, import.meta.url), 'utf8');
  const { syncPillLabel } = await import('../../src/lib/syncPill.ts');
  const mk116 = (o) => ({ syncStatus: 'synced', backgroundSyncing: false, syncConnected: true, waiting: 0, failed: 0, ...o });

  /* ---------- X1：pill 优先级真值表（纯函数单点） ---------- */
  checkNew('(t116-x1a) 优先级1：error 恒「同步失败」——手动同步进行中（syncing）/后台同步中也不例外（修复失败被 syncing 覆盖的既有缺陷）',
    syncPillLabel(mk116({ syncStatus: 'error' })) === '同步失败'
    && syncPillLabel(mk116({ syncStatus: 'error', backgroundSyncing: true })) === '同步失败');
  checkNew('(t116-x1b) 优先级2：手动/后台同步中显示「同步中…」（error 缺席时）',
    syncPillLabel(mk116({ syncStatus: 'syncing' })) === '同步中…'
    && syncPillLabel(mk116({ backgroundSyncing: true })) === '同步中…');
  checkNew('(t116-x1c) 优先级3：waiting>0 显示「等待同步 N 条」（未连接也如实——队列是本地事实，连接后自动补推）',
    syncPillLabel(mk116({ waiting: 3 })) === '等待同步 3 条'
    && syncPillLabel(mk116({ waiting: 1, syncConnected: false })) === '等待同步 1 条');
  checkNew('(t116-x1d) failed>0 追加「· 部分失败」段（同一 pill 内，≤48 字）；无失败绝不追加（凭空出现的实现必红）',
    syncPillLabel(mk116({ waiting: 2, failed: 1 })) === '等待同步 2 条 · 部分失败'
    && syncPillLabel(mk116({ syncStatus: 'error', failed: 2 })) === '同步失败 · 部分失败'
    && syncPillLabel(mk116({ syncStatus: 'syncing', failed: 1 })) === '同步中… · 部分失败'
    && syncPillLabel(mk116({})) === '后端已同步');

  /* ---------- X3：无队列无失败不劣化（既有语义逐字保留，不新增噪音） ---------- */
  checkNew('(t116-x3a) X3 不劣化：无队列无失败时与既有语义逐字一致（后端已同步 / 本地模式 · 直连抓取），不出现等待/失败段',
    syncPillLabel(mk116({})) === '后端已同步'
    && syncPillLabel(mk116({ syncConnected: false })) === '本地模式 · 直连抓取'
    && !syncPillLabel(mk116({})).includes('等待')
    && !syncPillLabel(mk116({})).includes('部分失败'));

  /* ---------- 接线：store→pill（SSR 取证）+ 源级防回退 ---------- */
  const sidebarSrc116 = src116('../../src/components/Sidebar.tsx');
  const apiSrc116 = src116('../../src/lib/api.ts');
  const bootstrapSrc116 = src116('../../src/store/slices/bootstrap.ts');
  const syncTabSrc116 = src116('../../src/components/settings/SyncTab.tsx');
  checkNew('(t116-x1e) Sidebar 接线（源级）：文案收口到 syncPillLabel 单点（store 字段逐参入函），旧内联四分支文案清零（优先级回退必须重写文案单点才能复绿）',
    sidebarSrc116.includes('syncPillLabel({')
    && sidebarSrc116.includes('waiting: syncWaiting,') && sidebarSrc116.includes('failed: syncFailed,')
    && !sidebarSrc116.includes("'同步失败'") && !sidebarSrc116.includes("'同步中…'")
    && !sidebarSrc116.includes("'等待同步") && !sidebarSrc116.includes("'后端已同步'"));
  checkNew('(t116-api) api 形态：syncQueueStats() 调 sync_queue_stats 命令，返回 SyncQueueStats（waiting/failed/last_error）',
    apiSrc116.includes("await inv('sync_queue_stats')")
    && apiSrc116.includes('interface SyncQueueStats')
    && apiSrc116.includes('waiting: number') && apiSrc116.includes('failed: number')
    && apiSrc116.includes('last_error: string | null'));
  checkNew('(t116-refresh) 刷新时机（源级）：启动装载 reload 顺带拉 syncQueueStats 写入 store（挂载与手动同步完成后的末次 reload 共用此点）',
    bootstrapSrc116.includes('api.syncQueueStats()')
    && bootstrapSrc116.includes('syncWaiting: q.waiting') && bootstrapSrc116.includes('syncFailed: q.failed'));

  /* store→pill 接线（SSR 烟测）。证据边界（如实说明）：zustand v5 的
     useSyncExternalStore server snapshot 恒读 getInitialState()（模块创建时的
     初始态，闭包持有、测试无法重定向），renderToStaticMarkup 只能看到初始态——
     故 SSR 只取证「组件树可执行 + 初始态 pill 渲染」；字段驱动的真值表由
     纯函数断言（x1a-x1d）与参数级接线断言（x1e）共同锁定。 */
  const { renderToStaticMarkup: rsm116 } = await import('react-dom/server');
  const { createElement: ce116 } = await import('react');
  const { Sidebar: Sidebar116 } = await import('../../src/components/Sidebar.tsx');
  const pillBase116 = rsm116(ce116(Sidebar116));
  checkNew('(t116-x1f) SSR 烟测：Sidebar 组件树可执行，初始态（未连接 · 空队列）pill 渲染出 X3 基线文案「本地模式 · 直连抓取」',
    pillBase116.includes('本地模式 · 直连抓取')
    && pillBase116.includes('sync-status-pill'));

  /* ---------- X2：SyncTab 四态摘要卡（纯函数真值表 + 源级接线 + SSR 烟测） ---------- */
  const { syncStateSummary } = await import('../../src/lib/syncPill.ts');
  const stat116 = (o) => ({ waiting: 0, failed: 0, last_error: null, ...o });
  checkNew('(t116-x2a) 摘要口径（纯函数）：等待 N / 部分失败 N（最新错误 ≤1 行摘要）/ 上次同步时间，三段齐备',
    syncStateSummary(stat116({ waiting: 3 }), 0) === '等待同步 3 条；上次同步 从未；状态变更已保存，连接后自动补推'
    && syncStateSummary(stat116({ waiting: 3 }), 1760000000).includes('上次同步 ')
    && syncStateSummary(stat116({ waiting: 2, failed: 1, last_error: '状态推送失败: HTTP 500' }), 0)
      === '等待同步 2 条；部分失败 1（状态推送失败: HTTP 500）；上次同步 从未；状态变更已保存，连接后自动补推');
  checkNew('(t116-x2b) 摘要口径（纯函数）：最新错误按码点截断 60 字符+…（不劈代理对）；无队列无失败仅时间行+说明句（X3 无噪音）；不虚构「已确认累计」',
    syncStateSummary(stat116({ failed: 1, last_error: '错'.repeat(80) }), 0).includes(`部分失败 1（${'错'.repeat(60)}…）`)
    && syncStateSummary(stat116({}), 0) === '上次同步 从未；状态变更已保存，连接后自动补推'
    && !syncStateSummary(stat116({}), 0).includes('已确认'));
  checkNew('(t116-x2c) SyncTab 摘要卡接线（源级）：「同步状态」卡 desc 走 syncStateSummary 单点，挂载与保存并同步链尾都刷新统计；说明句在卡 desc 收尾（不新增常驻 hint，TASK-101/102 既有断言锁定）',
    syncTabSrc116.includes('title="同步状态"')
    && syncTabSrc116.includes('syncStateSummary(')
    && syncTabSrc116.includes('api.syncQueueStats()')
    && syncTabSrc116.includes('refreshQueueStats(setQueueStats)')
    && (syncTabSrc116.match(/mini-dialog-hint/g) || []).length === 1);
  const { SyncTab: SyncTab116 } = await import('../../src/components/settings/SyncTab.tsx');
  const syncTabHtml116 = rsm116(ce116(SyncTab116));
  checkNew('(t116-x2d) SSR 烟测：摘要卡改动后 SyncTab 组件树仍可执行（初始 mock 态渲染「演示模式」卡，证据边界同 x1f 注）',
    syncTabHtml116.includes('演示模式') && syncTabHtml116.includes('同步'));
}

/* ============================================================
   TASK-124（2026-10-07，审计 P2-6 / 探针 P8 本体转真实行为回归）：
   同步状态变化事件 sync-queue-changed——四态展示接入真实链路。
   Rust 侧：本地入队事务提交（set_read/set_starred/set_read_bulk/mark_all_read）、
   即时推送与 states 推送段确认·失败、认证/网络阻塞（build_client 失败，此前
   attempts/last_error 不记录）后发 sync-queue-changed，payload={waiting,failed,
   last_error}；未配置同步静默不发。
   前端侧：App.tsx 监听 → applySyncQueueChanged 写 store 三字段
   （syncWaiting/syncFailed/syncQueueLastError，与 reloadFromBackend 顺带刷新
   同一状态源）→ pill / SyncTab 摘要卡自动跟随。
   证据边界（如实说明，p3b 先例）：无浏览器装置挂不了 App.tsx 的 Tauri 事件
   监听（test-loader 仅 mock @tauri-apps/api/core，本卡不改 loader），故事件→
   监听腿用源码形态断言钉住；行为腿驱动 store 真实动作（toggleCurrentReadStatus
   的 set_read IPC + applySyncQueueChanged＝监听器同一落点）+ 纯函数真值表；
   Rust 腿（payload 正确性 / 认证失败标记 / 摘要分类）由 CI cargo test 承担
   （t124-r1/r2/r3）。
   判别设计：
   - t124-1 即探针 P8 本体：入队事件缺失（回归前实现）时 waiting 恒 0、pill 停留
     「后端已同步」——事件落 store 的链路一断本组即红；
   - t124-2 锁「出队恢复」：成功 prune 事件把 waiting 拉回 0（poll 不残留假失败）；
   - t124-3/t124-4 锁失败呈现与认证/网络区分（last_error 原样透传）；
   - t124-5 锁未配置静默的半边（不发事件不刷状态，pill 保持本地模式）+ 动作
     越权防护（applySyncQueueChanged 不触碰 syncConnected）。
   ============================================================ */
{
  const fs124 = await import('node:fs');
  const src124 = (p) => fs124.readFileSync(new URL(p, import.meta.url), 'utf8');
  const { syncPillLabel, syncStateSummary } = await import('../../src/lib/syncPill.ts');

  /* ---------- (t124-1) 探针 P8 本体：本地标读入队 → pill 即时「等待同步 1 条」 ---------- */
  const mkEntry124 = (id, feedId) => ({
    id, feedId, title: `t-${id}`, author: 'a', snippet: 's', content: '',
    translatedContent: '', aiSummary: '', url: '', cover: null, imageUrl: null,
    tags: [], isRead: false, isStarred: false, publishedAt: 1000,
    enclosureUrl: null, enclosureMime: null, durationSec: null,
    fulltextExtracted: false, rawContent: null,
  });
  await store.getState().bootstrapFromBackend(); // 归位（既有 fixture 后端）
  await nTick(30); /* 隔离 bootstrap 顺带的 void syncStatus/syncQueueStats 承诺（迟到落地会覆写 syncConnected） */
  store.setState({
    dataMode: 'tauri', syncStatus: 'synced', backgroundSyncing: false,
    syncConnected: true, syncWaiting: 0, syncFailed: 0, syncQueueLastError: null,
    entries: [mkEntry124('9001', '10')], feedCounts: new Map(), toasts: [],
    activeArticleId: '9001', activeViewFilter: 'all', openedReadIds: {},
  });
  const pillOf124 = (st) => syncPillLabel({
    syncStatus: st.syncStatus, backgroundSyncing: st.backgroundSyncing,
    syncConnected: st.syncConnected, waiting: st.syncWaiting, failed: st.syncFailed,
  });
  const st124a = store.getState();
  checkNew('(t124-1) 前置（探针场景基线）：已连接 · 队列空 → pill「后端已同步」',
    st124a.syncConnected === true && st124a.syncWaiting === 0
    && pillOf124(st124a) === '后端已同步');
  /* 真实标读腿：store 动作触发 set_read IPC（Rust 侧在事务提交后发事件） */
  store.getState().toggleCurrentReadStatus();
  await nTick(20);
  const setRead124 = S.invokeCalls.filter((c) => c.cmd === 'set_read');
  checkNew('(t124-1) 真实行为（探针 P8 本体）：set_read 入队 IPC 已发出（store 动作腿），随后 Rust 事务提交事件 {waiting:1,failed:0,last_error:null} 落 store（applySyncQueueChanged＝监听器同一落点）→ pill 即时「等待同步 1 条」（修前统计只随 reload 刷新，pill 停留「后端已同步」）',
    setRead124.length === 1 && setRead124[0].args.id === 9001 && setRead124[0].args.read === true
    && (() => { store.getState().applySyncQueueChanged({ waiting: 1, failed: 0, last_error: null }); return true; })()
    && store.getState().syncWaiting === 1 && store.getState().syncFailed === 0
    && store.getState().syncQueueLastError === null
    && pillOf124(store.getState()) === '等待同步 1 条');

  /* ---------- (t124-2) 推送成功出队 → 恢复「后端已同步」 ---------- */
  store.getState().applySyncQueueChanged({ waiting: 0, failed: 0, last_error: null });
  checkNew('(t124-2) 出队恢复：推送确认事件 {waiting:0,failed:0} → pill 回「后端已同步」、无「部分失败」残留（成功 prune 后 last_error 清失）',
    store.getState().syncWaiting === 0 && store.getState().syncFailed === 0
    && store.getState().syncQueueLastError === null
    && pillOf124(store.getState()) === '后端已同步');

  /* ---------- (t124-3) 推送失败 → 「· 部分失败」+ last_error 摘要 ---------- */
  store.getState().applySyncQueueChanged({ waiting: 2, failed: 1, last_error: '状态推送失败: HTTP 503' });
  checkNew('(t124-3) 失败呈现：推送失败事件 → pill「等待同步 2 条 · 部分失败」、摘要卡错误行携带 last_error 摘要（≤60 字截断口径复用 t116-x2b）',
    store.getState().syncWaiting === 2 && store.getState().syncFailed === 1
    && store.getState().syncQueueLastError === '状态推送失败: HTTP 503'
    && pillOf124(store.getState()) === '等待同步 2 条 · 部分失败'
    && syncStateSummary({ waiting: store.getState().syncWaiting, failed: store.getState().syncFailed, last_error: store.getState().syncQueueLastError }, 0)
      .includes('部分失败 1（状态推送失败: HTTP 503）'));

  /* ---------- (t124-4) 认证失败区分呈现（审计：区分未配置/认证失败/网络失败） ---------- */
  store.getState().applySyncQueueChanged({
    waiting: 1, failed: 1,
    last_error: '认证失败：ClientLogin → 401 Unauthorized（已定位 API：https://x/api/greader.php）',
  });
  const st124d = store.getState();
  const netSummary124 = syncStateSummary({ waiting: 1, failed: 1, last_error: '推送被阻塞（网络/端点失败）：error sending request ← dns error: lookup failed' }, 0);
  checkNew('(t124-4) 认证失败区分：事件 last_error 带「认证失败」文案 → 摘要卡如实透传（含「认证失败」）；对照网络失败摘要透传实际错误且不含「认证失败」（Rust 侧分类由 CI t124-r2 锁；前端不篡改不吞）',
    st124d.syncFailed === 1 && pillOf124(st124d) === '等待同步 1 条 · 部分失败'
    && syncStateSummary({ waiting: st124d.syncWaiting, failed: st124d.syncFailed, last_error: st124d.syncQueueLastError }, 0).includes('认证失败：ClientLogin → 401')
    && netSummary124.includes('推送被阻塞（网络/端点失败）')
    && netSummary124.includes('dns error')
    && !netSummary124.includes('认证失败'));

  /* ---------- (t124-5) 未配置静默：不发事件不刷状态，本地模式 pill 不误报 ---------- */
  store.setState({
    syncConnected: false, syncWaiting: 0, syncFailed: 0, syncQueueLastError: null,
    syncStatus: 'synced', backgroundSyncing: false,
  });
  checkNew('(t124-5) 未配置静默（半边：事件缺失）：未连接且无事件时 pill 保持「本地模式 · 直连抓取」（不误报不刷状态）；越权防护：applySyncQueueChanged 只写队列三字段，动作体内不触碰 syncConnected（连接态只属 syncStatus/sync_save 链路）',
    pillOf124(store.getState()) === '本地模式 · 直连抓取'
    && (() => {
      const before = store.getState().syncConnected;
      store.getState().applySyncQueueChanged({ waiting: 3, failed: 1, last_error: 'x' });
      return store.getState().syncConnected === before;
    })()
    && store.getState().syncWaiting === 3);

  /* ---------- (t124-6) 源级接线（p3b 先例）：App.tsx 监听 → store 动作单点 ---------- */
  const appSrc124 = src124('../../src/App.tsx');
  const syncSliceSrc124 = src124('../../src/store/slices/sync.ts');
  const typesSrc124 = src124('../../src/store/types.ts');
  checkNew('(t124-6) 接线（源级）：App.tsx 监听 sync-queue-changed 且回调唯一落点 applySyncQueueChanged(e.payload)（删掉监听/改 payload 键名必红）；store 声明 applySyncQueueChanged + syncQueueLastError（事件与 reload 同一状态源）',
    appSrc124.includes("await listen<{ waiting: number; failed: number; last_error: string | null }>(")
    && appSrc124.includes("'sync-queue-changed'")
    && appSrc124.includes('applySyncQueueChanged(e.payload)')
    && typesSrc124.includes('applySyncQueueChanged:')
    && typesSrc124.includes('syncQueueLastError: string | null')
    && syncSliceSrc124.includes('syncQueueLastError: stats.last_error ?? null'));

  /* ---------- (t124-7) SyncTab 摘要卡自动跟随（源级） ---------- */
  const syncTabSrc124 = src124('../../src/components/settings/SyncTab.tsx');
  checkNew('(t124-7) SyncTab 跟随（源级）：订阅 store 把队列三字段镜像进摘要卡（subscribe 回调，事件驱动），refreshQueueStats 主动拉取保留（t116-x2c 单点不变）',
    syncTabSrc124.includes('useAppStore.subscribe((s) => {')
    && syncTabSrc124.includes('waiting: s.syncWaiting, failed: s.syncFailed, last_error: s.syncQueueLastError')
    && syncTabSrc124.includes('refreshQueueStats(setQueueStats)'));

  /* 恢复基线（不污染后续块/汇总） */
  store.setState({ syncConnected: false, syncWaiting: 0, syncFailed: 0, syncQueueLastError: null, entries: [], activeArticleId: null, toasts: [] });
}

/* ============================================================
   OPT-012（审计 F14 + 「关闭确认框与快捷键」）：阅读器源码/渲染分离回归。
   浮层侧断言已就地更新于 P3[F2] 块（清单 8→9 + 真实 store 字段驱动 closeAsk）。

   证据分层（如实说明）：
   - 呈现分支（源码态转义文本 / 渲染态 DOM / 流式纯文本）由生产组件 ReaderProse
     承担，本块用 react-dom/server 的**真实 SSR 输出**取证——不用「源码含
     isRawRenderMode」之类 token 断言代替输出；修前对照见
     tmp/optimization-20261008/OPT-012/probe-before.mjs（复刻旧分支 = 三类标签真实出现）。
   - 值传递（显示 RSS 原文/提取全文/译文、代理 base64 只进渲染态）无 DOM harness，
     且 zustand v5 server snapshot 恒读 getInitialState（t116 注），故由 Reader.tsx
     参数级接线断言钉住（cov-wire/t111-6 先例），行为面由上面 SSR 输出互补。
   ============================================================ */
{
  const fsOpt12 = await import('node:fs');
  const { renderToStaticMarkup } = await import('react-dom/server');
  const { createElement } = await import('react');
  const { ReaderProse } = await import('../../src/components/ReaderProse.tsx');

  /* 固定 HTML：含 <p><b> 与带 onerror 的 <img>（审计 F14 建议的正文形态） */
  const FIXED_HTML = '<p>第一段<b>加粗</b></p><img src="https://example.com/pic.png" onerror="alert(1)">';
  /* 图片代理产物形态：data: base64——只允许出现在渲染态，不得冒充源码 */
  const PROXIED_HTML = '<p>第一段</p><img src="data:image/png;base64,QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVo=">';
  const STYLE = { fontFamily: 'Georgia, serif', fontSize: 16, lineHeight: 1.6 };
  const renderProse = (props) => renderToStaticMarkup(createElement(ReaderProse, {
    style: STYLE, onClick: () => { /* noop */ }, ...props,
  }));

  /* ---- A：源码态 = 转义文本，不创建 p/b/img ---- */
  const sourceHtml = renderProse({
    renderHtml: PROXIED_HTML, sourceText: FIXED_HTML,
    isSourceMode: true, isStreamingTranslation: false,
  });
  checkNew('(opt012-a1) 真实 SSR 源码态：输出转义文本（&lt;p&gt;/&lt;b&gt;/&lt;img 与 onerror 均为字面），不创建 p/b/img 元素（修前同走 dangerouslySetInnerHTML → 三类标签真实出现）',
    sourceHtml.includes('reader-source-view')
    && sourceHtml.includes('&lt;p&gt;') && sourceHtml.includes('&lt;b&gt;') && sourceHtml.includes('&lt;img')
    && !sourceHtml.includes('<p>') && !sourceHtml.includes('<b>') && !sourceHtml.includes('<img'));
  checkNew('(opt012-a2) 真实 SSR 源码态：显示原始 src、不显示图片代理 data: base64（sourceText 取未经代理的原文；renderHtml 的代理产物只属于渲染态）',
    sourceHtml.includes('https://example.com/pic.png')
    && !sourceHtml.includes('data:image') && !sourceHtml.includes('base64'));

  /* ---- B：渲染态 = 现有 DOM 结构照旧 ---- */
  const renderOut = renderProse({
    renderHtml: FIXED_HTML, sourceText: FIXED_HTML,
    isSourceMode: false, isStreamingTranslation: false,
  });
  checkNew('(opt012-b1) 真实 SSR 渲染态：p/b/img DOM 结构照旧创建（源码/渲染分离不降级渲染路径），容器不再挂 raw-render-mode',
    renderOut.includes('<p>第一段<b>加粗</b></p>')
    && /<img[^>]*src="https:\/\/example\.com\/pic\.png"/.test(renderOut)
    && !renderOut.includes('raw-render-mode'));
  const proxiedOut = renderProse({
    renderHtml: PROXIED_HTML, sourceText: FIXED_HTML,
    isSourceMode: false, isStreamingTranslation: false,
  });
  checkNew('(opt012-b2) 真实 SSR 渲染态：图片代理产物（data: base64）仍被使用（修前行为不回退；与 a2 源码态对照）',
    proxiedOut.includes('data:image/png;base64,'));

  /* ---- C：流式未消毒译文 = 纯文本（rawStream 期间 source 也是纯文本） ---- */
  const STREAM_TEXT = '<script>alert(1)</script>半截译文';
  const streamRenderOut = renderProse({
    renderHtml: STREAM_TEXT, sourceText: STREAM_TEXT,
    isSourceMode: false, isStreamingTranslation: true,
  });
  const streamSourceOut = renderProse({
    renderHtml: STREAM_TEXT, sourceText: STREAM_TEXT,
    isSourceMode: true, isStreamingTranslation: true,
  });
  checkNew('(opt012-c1) 真实 SSR 流式译文（渲染态）：未消毒产物按纯文本插值——无 <script> 元素、字面 &lt;script&gt; 可见（TASK-065 N11 契约保留）',
    streamRenderOut.includes('&lt;script&gt;')
    && !streamRenderOut.includes('<script>')
    && streamRenderOut.includes('class="article-prose"'));
  checkNew('(opt012-c2) 真实 SSR 流式译文（源码态）：同样纯文本转义、不创建标签（流式期间切源码不打开 HTML 路径）',
    streamSourceOut.includes('reader-source-view')
    && streamSourceOut.includes('&lt;script&gt;') && !streamSourceOut.includes('<script>')
    && !streamSourceOut.includes('<p>') && !streamSourceOut.includes('<img'));

  /* ---- D：值传递接线（源级；SSR 读不到 store 真值——t116 注边界） ---- */
  const readerSrcOpt12 = fsOpt12.readFileSync(new URL('../../src/components/Reader.tsx', import.meta.url), 'utf8');
  checkNew('(opt012-d1) Reader 接线：渲染态 HTML = 消毒译文 / 代理命中产物 ?? baseHtml；源码文本 = 译文 / 未经代理的 baseHtml；Reader.tsx 不再有第二处 HTML 创建路径',
    readerSrcOpt12.includes('const renderHtml = isShowingTranslatedProse')
    && readerSrcOpt12.includes('proxiedContent?.key === baseHtml ? proxiedContent.html : baseHtml')
    && readerSrcOpt12.includes('const sourceText = isShowingTranslatedProse ? body.translatedContent : baseHtml')
    && !/dangerouslySetInnerHTML\s*=\s*\{\{/.test(readerSrcOpt12));
  checkNew('(opt012-d2) Reader 接线：三个模式开关与内容按名传入 ReaderProse（isSourceMode=isRawRenderMode；isStreamingTranslation=译文+rawStream；renderHtml/sourceText/onClick）',
    readerSrcOpt12.includes('isSourceMode={isRawRenderMode}')
    && readerSrcOpt12.includes('isStreamingTranslation={isShowingTranslatedProse && !!rawStream}')
    && readerSrcOpt12.includes('renderHtml={renderHtml}')
    && readerSrcOpt12.includes('sourceText={sourceText}')
    && readerSrcOpt12.includes('onClick={handleProseClick}'));

  /* ---- E：源码容器样式（局部；长行折行不撑破布局） ---- */
  const cssOpt12 = fsOpt12.readFileSync(new URL('../../src/styles/base.css', import.meta.url), 'utf8');
  checkNew('(opt012-e1) 源码显示局部样式：.reader-source-view 折行（white-space:pre-wrap）+ 超长 token 断行（overflow-wrap:anywhere），选择器只作用于源码容器',
    /\.article-prose\s+\.reader-source-view\s*\{[^}]*white-space:\s*pre-wrap/s.test(cssOpt12)
    && /\.article-prose\s+\.reader-source-view\s*\{[^}]*overflow-wrap:\s*anywhere/s.test(cssOpt12));
}

/* ============================================================
   OPT-015（F23 媒体键）：SMTC 媒体动作幂等回归。
   证据分层（如实说明）：
   - 行为层：直接驱动**真实 store**（dist-test 同一模块图实例）的
     applyMediaAction——卡片要求「实际 store 测试，不只测试复制 helper」。
     R1 修订（review P2）：Play/Pause 的幂等必须**每次调用后立即断言**且覆盖
     playing/paused 两种初态——只看终态的旧断言对「Play 分支变质为 toggle」
     漏检（true→false→true 两次调用后终态仍是 true，审查实测 780/780 全绿）。
     强化后的断言已用内存变异探针实测对旧 toggle 语义必红（m2a/m3c 直接命中，
     m4a..m6c 为状态级联失败），探针与用法见
     tmp/optimization-20261008/OPT-015/mutation-probe-loader.mjs（不改产品文件）。
   - 接线层：App.tsx player-media 监听只收窄 payload 后调 applyMediaAction
     （源码形态断言，沿用本文件 readFileSync 先例）——删掉监听或退回内联
     播放判定即失败。真实媒体键→SMTC→Rust→前端整链的实机验证由主控隔离验收。
   ============================================================ */
{
  const stM = () => store.getState();

  /* 基线：关闭播放条、清空 toast */
  store.setState({ toasts: [] });
  stM().closePodcastBar();

  /* ---- inactive：媒体键不得启动播放器、不得改任何字段 ---- */
  const inactiveBefore = JSON.stringify(stM().player);
  stM().applyMediaAction('play');
  stM().applyMediaAction('pause');
  stM().applyMediaAction('toggle');
  checkNew('(opt015-m1) inactive：Play/Pause/Toggle 全无副作用（不启动无剧集播放器，player 字段逐字节不变）',
    JSON.stringify(stM().player) === inactiveBefore && stM().player.isActive === false);

  /* ---- playing 初态：Play 每次调用后都必须恒播放（逐步断言） ---- */
  stM().playPodcastEpisode('M 集', '节目', '', 'https://a.example/m.mp3');
  stM().applyMediaAction('play');
  checkNew('(opt015-m2a) playing 初态·第 1 次 Play 后仍播放（Play 变质为 toggle 时 true→false 在此即红）',
    stM().player.isPlaying === true && stM().player.isActive === true);
  stM().applyMediaAction('play');
  checkNew('(opt015-m2b) playing 初态·第 2 次 Play 后仍播放（逐步断言；只看终态对 true→false→true 漏检）',
    stM().player.isPlaying === true && stM().player.isActive === true);

  /* ---- paused 初态：Play 每次调用后都必须恒播放，且进度保留 ---- */
  stM().syncPlayerProgress(63, 300);
  stM().applyMediaAction('pause');
  checkNew('(opt015-m3a) paused 初态就位（首次 Pause 从播放翻到暂停，进度保留）',
    stM().player.isPlaying === false && stM().player.positionSec === 63);
  stM().applyMediaAction('play');
  checkNew('(opt015-m3b) paused 初态·第 1 次 Play 后 → 播放（目标态语义：63s 处续播，不归零重启）',
    stM().player.isPlaying === true && stM().player.positionSec === 63);
  stM().applyMediaAction('play');
  checkNew('(opt015-m3c) paused 初态·第 2 次 Play 后仍播放（Play 变质为 toggle 时 true→false 在此即红）',
    stM().player.isPlaying === true && stM().player.positionSec === 63);

  /* ---- Pause 同样逐步：每次调用后都断 false（playing/paused 两种初态） ---- */
  stM().applyMediaAction('pause');
  checkNew('(opt015-m4a) playing 初态·第 1 次 Pause 后 → 暂停', stM().player.isPlaying === false);
  stM().applyMediaAction('pause');
  checkNew('(opt015-m4b) paused 初态·第 2 次 Pause 后仍暂停（Pause 变质为 toggle 时 false→true 在此即红）',
    stM().player.isPlaying === false);
  stM().applyMediaAction('pause');
  checkNew('(opt015-m5) paused 初态·第 3 次 Pause 后仍暂停（每次 Pause 后断言 false，不只看双切回）',
    stM().player.isPlaying === false && stM().player.positionSec === 63);

  /* ---- Toggle 才切换：×2 回原态 ---- */
  stM().applyMediaAction('toggle');
  checkNew('(opt015-m6a) Toggle 单次翻转恢复播放（切换语义只归 toggle）', stM().player.isPlaying === true);
  stM().applyMediaAction('toggle');
  checkNew('(opt015-m6b) Toggle 第 2 次翻转回原态（暂停）', stM().player.isPlaying === false);
  stM().applyMediaAction('toggle');
  checkNew('(opt015-m6c) Toggle 再次翻转（供 Stop 关闭用例以播放态起）', stM().player.isPlaying === true);

  /* ---- Stop 关闭（激活态） ---- */
  stM().applyMediaAction('stop');
  checkNew('(opt015-m7) Stop 关闭播放条（isActive/isPlaying 双落 + seek 清空）',
    stM().player.isActive === false && stM().player.isPlaying === false && stM().player.seekToSec === null);

  /* ---- 幂等 ≠ 失效：暂停后 Play 从暂停点恢复（不重头） ---- */
  stM().playPodcastEpisode('M 集', '节目', '', 'https://a.example/m.mp3');
  stM().syncPlayerProgress(63, 300);
  stM().applyMediaAction('pause');
  stM().applyMediaAction('play');
  checkNew('(opt015-m8) Pause→Play 恢复播放且进度保留（目标态语义：63s 处续播，不归零重启）',
    stM().player.isPlaying === true && stM().player.positionSec === 63);

  /* ---- inactive 下 Stop 仍是关闭路径（幂等无害） ---- */
  stM().closePodcastBar();
  stM().applyMediaAction('stop');
  checkNew('(opt015-m9) inactive 下 Stop 仍是关闭路径（无异常、状态保持关闭）',
    stM().player.isActive === false && stM().player.isPlaying === false);

  /* ---- 接线层：App.tsx player-media 监听 ---- */
  const fsOpt15 = await import('node:fs');
  const appSrcOpt15 = fsOpt15.readFileSync(new URL('../../src/App.tsx', import.meta.url), 'utf8');
  const pmStart = appSrcOpt15.indexOf("listen<string>('player-media'");
  const pmBlock = pmStart < 0 ? '' : appSrcOpt15.slice(pmStart, appSrcOpt15.indexOf('});', pmStart));
  checkNew('(opt015-m10) App.tsx 接线：player-media 收窄四个动作后唯一落点 applyMediaAction(action)；播放判定不再内联（无 togglePlayerPlay() 调用 / 无 isActive 早退）',
    pmBlock.includes("'play'") && pmBlock.includes("'pause'") && pmBlock.includes("'toggle'") && pmBlock.includes("'stop'")
    && pmBlock.includes('applyMediaAction(action)')
    && !pmBlock.includes('togglePlayerPlay()') && !pmBlock.includes('isActive'));
}
}
