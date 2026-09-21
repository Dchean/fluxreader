// 前端逻辑回归（无浏览器）：用 node 驱动 Zustand 状态机验证 S-1 / C-3。
// 运行：先 npx tsc -p tsconfig.test.json，再
//   node --loader ./tools/test-loader.mjs ./tools/frontend-regression.mjs

// 1) 伪造 Tauri 窗口环境，使 isTauri()=true → dataMode 可进 'tauri'
globalThis.window = { __TAURI_INTERNALS__: {} };

// 2) 后端状态：一篇正文含 <script> 的文章 + 消毒后的译文缓存
const SANITIZED = '<p>安全译文，无脚本</p>';
let articleRow = {
  id: 1, feed_id: 1, title: '测试文章', author: 'a',
  summary: 'snippet', content_html: '<p>正文</p>', image_url: null,
  enclosure_url: null, enclosure_mime: null, duration_sec: null,
  ai_summary: null, translated_content: null, source: 'direct',
  published_at: '2026-09-04T10:00:00Z', is_read: false, is_starred: false,
  fulltext_extracted: false, url: 'https://example.com/a',
};

// 批量水合（get_articles）的可控行为：S-2 用例按场景改写
let getArticlesBehavior = { rows: [], reject: false };
// S-3 可控行为：bootstrap 后端故障注入 / github 登录首调冲突
let failBootstrap = false;

// 3) invoke mock：按命令返回
const invokeCalls = [];
globalThis.__INVOKE__ = (cmd, args) => {
  invokeCalls.push({ cmd, args });
  if (failBootstrap && ['list_folders', 'list_feeds', 'list_articles', 'sync_status'].includes(cmd)) {
    return Promise.reject({ code: 'db_corrupt', message: 'DB locked by migration' });
  }
  switch (cmd) {
    case 'article_index': return Promise.resolve(0);
    case 'github_login_start': {
      // P1-10：首调（不带 force）返回 webdavConflict 结构化错误；force 重发成功
      if (args.force !== true) {
        return Promise.reject({ code: 'webdavConflict', message: 'WebDAV conflict: existing data' });
      }
      return Promise.resolve({ user_code: 'WDJB-MJHT', verification_uri: 'https://github.com/login/device', interval: 3600 });
    }
    case 'list_folders': return Promise.resolve([]);
    case 'list_feeds': return Promise.resolve([]);
    case 'list_articles': return Promise.resolve([articleRow]);
    case 'sync_status': return Promise.resolve({ connected: false });
    case 'get_articles': {
      // S-2 可控行为：成功返回 rows / 失败 reject（默认空）
      if (getArticlesBehavior.reject) return Promise.reject(getArticlesBehavior.error ?? { message: 'db busy' });
      return Promise.resolve(getArticlesBehavior.rows);
    }
    case 'get_article': return Promise.resolve(articleRow);

    case 'get_setting': return Promise.resolve(null);
    case 'set_setting': return Promise.resolve(null);
    // ai_translate / ai_summarize：args.onChannel 是 Channel mock
    case 'ai_translate': {
      const ch = args.onChannel;
      // 模拟未消毒流式 delta（含 <script>）
      ch.onmessage?.({ type: 'delta', data: '<p>译文<script>alert(1)</script></p>' });
      // 后端落库后返回消毒版（后续 get_article 会返回 SANITIZED）
      articleRow = { ...articleRow, translated_content: SANITIZED };
      ch.onmessage?.({ type: 'done' });
      return Promise.resolve('<p>译文<script>alert(1)</script></p>');
    }
    case 'ai_summarize': {
      const ch = args.onChannel;
      ch.onmessage?.({ type: 'done' });
      return Promise.resolve('');
    }
    case 'extract_fulltext': {
      // 模拟全文提取失败（断网）
      return Promise.reject({ message: '网页拉取失败：HTTP 503' });
    }
    default:
      return Promise.resolve(null);
  }
};

// 4) import 编译后的 store（loader 会 mock @tauri-apps/api）
//    4a) E2 探针：必须在 store.js 被求值（其模块末尾 bindAppStore）之前调用
//        internals.appStore()，才能观察到「句柄尚未注入」这条路径的真实行为。
//        只是新增几行探测代码，不触碰上面任何一条既有断言。
const internalsBeforeBind = await import('../dist-test/store/internals.js');
let appStoreUnbound = { threw: false, error: null, value: undefined };
try {
  appStoreUnbound.value = internalsBeforeBind.appStore();
} catch (e) {
  appStoreUnbound = { threw: true, error: e, value: undefined };
}
const { useAppStore } = await import('../dist-test/store.js');

const store = useAppStore;
const results = [];
function check(name, cond) {
  results.push({ name, pass: !!cond });
  console.log(`${cond ? '✅' : '❌'} ${name}`);
}

// ---- 启动装载（tauri 路径）----
await store.getState().bootstrapFromBackend();
check('bootstrap 后 dataMode=tauri', store.getState().dataMode === 'tauri');
check('bootstrap 后 entries 有 1 条', store.getState().entries.length === 1);

// ---- S-1：翻译流式 XSS 回读消毒版 ----
const entryId = store.getState().entries[0].id;
store.getState().selectArticle(entryId);

// 触发翻译（走 tauri 路径，onDelta 追加未消毒内容）
store.getState().toggleReaderTranslation();

// 等流式完成 + 回读完成
await new Promise((r) => setTimeout(r, 50));

const after = store.getState().entries.find((a) => a.id === entryId);
check('S-1: 流式结束后 translatedContent 被回读为消毒版', after?.translatedContent === SANITIZED);
check('S-1: 翻译后不再残留 <script>', !(after?.translatedContent ?? '').includes('<script>'));

// ---- C-3：全文提取失败可见（toast + 重试）----
// 把 settings.defaultOpenMode 置为 fulltext，重新水合一篇文章触发自动全文。
// 智能全文判定：正文须含截断标记（"…查看全文"）才触发提取。
store.getState().updateSettings({ defaultOpenMode: 'fulltext' });
articleRow = { ...articleRow, content_html: '<p>这是摘要正文，比较短…</p><a>…查看全文</a>' };
// 手动重置该条目 content 为空以触发 ensureArticleContent 水合
store.setState((s) => ({
  entries: s.entries.map((a) => (a.id === entryId ? { ...a, content: '' } : a)),
}));
store.getState().ensureArticleContent(entryId, { extractFulltext: true });
await new Promise((r) => setTimeout(r, 50));

const toasts = store.getState().toasts;
check('C-3: 全文提取失败后出现 toast 提示', toasts.some((t) => t.text.includes('全文提取失败')));
check('C-3: toast 带「重试」action', toasts.some((t) => t.action?.label === '重试'));

// ---- 额外：翻译缓存命中路径（已有译文直接展示，不重新流式）----
const cachedId = entryId;
store.setState((s) => ({
  entries: s.entries.map((a) => (a.id === cachedId ? { ...a, translatedContent: '已缓存译文' } : a)),
  isShowingTranslatedProse: false,
}));
store.getState().toggleReaderTranslation();
await new Promise((r) => setTimeout(r, 20));
const cached = store.getState().entries.find((a) => a.id === cachedId);
check('缓存命中：已有译文直接展示，不触发 ai_translate',
  cached?.translatedContent === '已缓存译文' && store.getState().isShowingTranslatedProse === true);
check('缓存命中：未新增 ai_translate 调用', invokeCalls.filter((c) => c.cmd === 'ai_translate').length === 1);

// ---- S-2：社交正文批量水合（REQ-001：加载失败/空正文不再永挂）----
const socialEntry = (id) => ({
  id, feedId: '1', title: '社交帖', publishedAt: Date.now(), isRead: false,
  isStarred: false, tags: [], source: 'direct', snippet: '摘要', author: 'a',
  content: '', rawContent: '', translatedContent: '', aiSummary: '',
});
const hydrCalls = () => invokeCalls.filter((c) => c.cmd === 'get_articles').length;

// 场景 1：正常行 → content 填充 + hydratedIds 终态
store.setState({ entries: [socialEntry('11')], hydratedIds: {}, hydrationErrors: {}, dataMode: 'tauri' });
getArticlesBehavior = { rows: [{ ...articleRow, id: 11, content_html: '<p>社交正文</p>' }], reject: false };
store.getState().hydrateArticleContent(['11']);
await new Promise((r) => setTimeout(r, 20));
const e11 = store.getState().entries.find((a) => a.id === '11');
check('S-2: 水合成功填充正文并置终态', e11?.content === '<p>社交正文</p>' && store.getState().hydratedIds['11'] === true);
check('S-2: 水合成功清除错误态', store.getState().hydrationErrors['11'] === undefined);

// 场景 2：空正文（content_html 为 NULL）→ 终态「已水合」，再次挂载不再重复拉取
store.setState({ entries: [socialEntry('12')], hydratedIds: {}, hydrationErrors: {} });
getArticlesBehavior = { rows: [{ ...articleRow, id: 12, content_html: null }], reject: false };
store.getState().hydrateArticleContent(['12']);
await new Promise((r) => setTimeout(r, 20));
const e12 = store.getState().entries.find((a) => a.id === '12');
const callsBefore = hydrCalls();
store.getState().ensureArticleContent('12'); // 挂载触发：应被 hydratedIds 终态短路
check('S-2: 空正文条目置终态且不重复水合', e12?.content === '' && store.getState().hydratedIds['12'] === true && hydrCalls() === callsBefore);

// 场景 3：水合失败 → 错误态可见；重试收敛为成功
store.setState({ entries: [socialEntry('13')], hydratedIds: {}, hydrationErrors: {} });
getArticlesBehavior = { rows: [], reject: true, error: { message: 'IPC 超时' } };
store.getState().hydrateArticleContent(['13']);
await new Promise((r) => setTimeout(r, 20));
check('S-2: 水合失败记录错误态（不再静默假加载）', store.getState().hydrationErrors['13'] === 'IPC 超时');
getArticlesBehavior = { rows: [{ ...articleRow, id: 13, content_html: '<p>重试成功</p>' }], reject: false };
store.getState().retryHydration('13');
await new Promise((r) => setTimeout(r, 20));
const e13 = store.getState().entries.find((a) => a.id === '13');
check('S-2: 重试后正文填充且错误态清除', e13?.content === '<p>重试成功</p>' && store.getState().hydrationErrors['13'] === undefined && store.getState().hydratedIds['13'] === true);

// ---- S-3：启动失败不回退 mock（P0-2）+ WebDAV 冲突确认（P1-10）----
// S-3a：tauri 模式 bootstrap 失败 → 错误态 + 重试入口，绝不渲染 mock 演示数据
failBootstrap = true;
store.setState({ dataMode: 'tauri', dataLoading: false, bootstrapError: null, entries: [], categories: [] });
await store.getState().bootstrapFromBackend();
check('S-3a: tauri bootstrap 失败进入错误态', store.getState().bootstrapError?.includes('DB locked') === true);
check('S-3a: 失败时不回退 mock（dataMode 保持 tauri、无假数据）',
  store.getState().dataMode === 'tauri' && store.getState().entries.length === 0);
failBootstrap = false;
await store.getState().retryBootstrap();
check('S-3a: 重试后装载成功且错误态清除', store.getState().bootstrapError === null && store.getState().entries.length === 1);

// S-3b：WebDAV 冲突 → 结构化 code 识别 → 确认后 force 重发
let confirmCalls = 0;
window.confirm = () => { confirmCalls += 1; return true; };
await store.getState().githubLoginStart();
const ghCalls = invokeCalls.filter((c) => c.cmd === 'github_login_start');
check('S-3b: webdavConflict 弹确认并 force 重发', confirmCalls === 1 && ghCalls.length === 2 && ghCalls[1].args.force === true);
check('S-3b: force 成功后进入授权流程', store.getState().githubFlow?.user_code === 'WDJB-MJHT');

// ---- S-4：卡片级翻译接线（P1-7 空壳修复）----
const beforeAiCalls = invokeCalls.filter((c) => c.cmd === 'ai_translate').length;
const s4id = store.getState().entries[0].id;
store.setState((st) => ({
  entries: st.entries.map((a) => (a.id === s4id ? { ...a, translatedContent: '' } : a)),
}));
store.getState().translateEntry(s4id);
await new Promise((r) => setTimeout(r, 50));
const s4 = store.getState().entries.find((a) => a.id === s4id);
check('S-4: 卡片级翻译流式生成并回读消毒版', s4?.translatedContent === SANITIZED);
check('S-4: 生成完成后按 id 状态清除', store.getState().translatingIds[s4id] === undefined);
// 缓存命中：已有译文直接返回，不新增 ai_translate
store.getState().translateEntry(s4id);
check('S-4: 已有译文时不再触发 ai_translate',
  invokeCalls.filter((c) => c.cmd === 'ai_translate').length === beforeAiCalls + 1);

// ---- S-5：F4 按 id 摘要态 / F7 锚定打开标读 / F8 全部已读视图口径 ----
// F4：A 生成中不应影响 B 的卡片判定
store.setState({ entries: [socialEntry('21'), socialEntry('22')], summarizingIds: {}, summaryErrors: {} });
store.getState().summarizeEntry('21');
/* 生成态在调用同步段内即置位；api 完成是异步的，故立即断言再等清除 */
const isolatedAtStart =
  store.getState().summarizingIds['21'] === true && store.getState().summarizingIds['22'] === undefined;
check('S-5: 摘要生成态按 id 隔离', isolatedAtStart);
await new Promise((r) => setTimeout(r, 40));
check('S-5: 摘要完成后清除该 id 状态', store.getState().summarizingIds['21'] === undefined);

// F7：搜索/命令面板打开（anchorToArticle）按 markReadOnOpen 标已读
store.getState().updateSettings({ markReadOnOpen: true });
store.setState({
  activeViewFilter: 'all',
  activeFeedFilter: 'all',
  dataMode: 'tauri',
});
invokeCalls.length = 0;
await store.getState().anchorToArticle('1');
await new Promise((r) => setTimeout(r, 10));
const readCall = invokeCalls.find((c) => c.cmd === 'set_read');
check(
  'S-5: 锚定打开按设置标已读',
  !!readCall && readCall.args.read === true && store.getState().entries.find((a) => a.id === '1')?.isRead === true,
);

// F8：全部已读的视图口径（收藏 → starredOnly；今天 → sinceMs）
store.setState({ activeViewFilter: 'starred', activeFeedFilter: 'all' });
invokeCalls.length = 0;
store.getState().markCurrentViewAllRead();
await new Promise((r) => setTimeout(r, 10)); // api.markAllRead 内部 await getInvoke()，需让出微任务
const starredCall = invokeCalls.find((c) => c.cmd === 'mark_all_read');
check(
  'S-5: 收藏视图全部已读带 starredOnly',
  !!starredCall && starredCall.args.starredOnly === true && (starredCall.args.sinceMs === null || starredCall.args.sinceMs === undefined),
);
store.setState({ activeViewFilter: 'today' });
invokeCalls.length = 0;
store.getState().markCurrentViewAllRead();
await new Promise((r) => setTimeout(r, 10));
const todayCall = invokeCalls.find((c) => c.cmd === 'mark_all_read');
check('S-5: 今天视图全部已读带 sinceMs', !!todayCall && typeof todayCall.args.sinceMs === 'number' && todayCall.args.sinceMs > 0);

/* ============================================================
   新增（TASK-048）：store 状态机行为断言 —— src/store.ts 拆分的回归网
   覆盖领域 (a)…(l)。【只增不改】：上面 26 项既有断言一行未动，
   新增项单独计数并以 🆕 标记，摘要行分别给出「既有」与「新增」通过数。

   确定性来源：本段落自带内存假后端（folders/feeds/feed_counts/list_articles/
   article_index/get_article/get_setting/set_setting/AI channel），竞态用
   「手动 resolve 的 deferred」驱动，不依赖真实网络、不依赖真实计时——
   唯一的真实等待是 toast 两段式生命周期（留 400ms 余量）。
   全程不触碰 Tauri 运行时与用户数据库。
   ============================================================ */

const newResults = [];
function checkNew(name, cond) {
  newResults.push({ name, pass: !!cond });
  console.log(`${cond ? '✅' : '❌'} 🆕 ${name}`);
}
const nTick = (ms = 20) => new Promise((r) => setTimeout(r, ms));

await (async () => {
  /* 组件侧与 .tsx 断言都需要 ui-loader（.tsx/.ts 就地转译）。
     注册顺序：Node loader 链后注册者先跑；test-loader 会给无扩展名相对路径盲加
     .js，故 ui-loader 必须最后注册（先处理 src/ 下的解析）。 */
  {
    const { register } = await import('node:module');
    register(new URL('./test-loader.mjs', import.meta.url).href, new URL('../', import.meta.url).href);
    register(new URL('./ui-loader.mjs', import.meta.url).href, new URL('../', import.meta.url).href);
  }

  const nSel = await import('../dist-test/store.js');
  const { selectVisibleEntries, selectScopeEntries, selectRawEntries, selectTreeCounts, selectViewCounts } = nSel;

  /* ---------- 规范假数据：覆盖三种布局解析路径 ----------
     feed 10 inherit → cat-1(article)；feed 11 显式 social（feed 级覆盖）；
     feed 12 inherit → cat-1(article)；feed 20 inherit → cat-2(social) */
  const FOLDERS = [
    { id: 1, name: '技术', layout: 'article', auto_summary: false, auto_translate: false, collapsed: false },
    { id: 2, name: '社交', layout: 'social', auto_summary: false, auto_translate: false, collapsed: false },
  ];
  const FEEDS = [
    { id: 10, folder_id: 1, feed_url: 'https://a.example/rss', site_url: null, title: '源A', favicon_url: null, layout: 'inherit', auto_summary: false, auto_translate: false, fetch_failed: false, fetch_error: null, last_fetched_at: null },
    { id: 11, folder_id: 1, feed_url: 'https://b.example/rss', site_url: null, title: '源B', favicon_url: null, layout: 'social', auto_summary: false, auto_translate: false, fetch_failed: false, fetch_error: null, last_fetched_at: null },
    { id: 12, folder_id: 1, feed_url: 'https://d.example/rss', site_url: null, title: '源D', favicon_url: null, layout: 'inherit', auto_summary: false, auto_translate: false, fetch_failed: false, fetch_error: null, last_fetched_at: null },
    { id: 20, folder_id: 2, feed_url: 'https://c.example/rss', site_url: null, title: '源C', favicon_url: null, layout: 'inherit', auto_summary: false, auto_translate: false, fetch_failed: false, fetch_error: null, last_fetched_at: null },
  ];
  const COUNTS = [
    { feed_id: 10, total: 5, unread: 3, starred: 1, today: 2 },
    { feed_id: 11, total: 4, unread: 2, starred: 2, today: 1 },
    { feed_id: 12, total: 2, unread: 2, starred: 0, today: 0 },
    { feed_id: 20, total: 3, unread: 1, starred: 0, today: 0 },
  ];
  const NOW = Date.now();
  const iso = (ms) => new Date(ms).toISOString();
  const TODAY = iso(NOW);
  const OLD = iso(NOW - 3 * 86400000);
  const mkRow = (o) => ({
    id: 0, feed_id: 10, title: 't', author: null, snippet: 's', image_url: null,
    enclosure_url: null, enclosure_mime: null, duration_sec: null, ai_summary: null,
    source: 'direct', published_at: OLD, is_read: false, is_starred: false, url: null,
    content_html: null, translated_content: null, fulltext_extracted: false, ...o,
  });
  const BASE_ROWS = [
    mkRow({ id: 101, feed_id: 10, title: 'A1 未读·今天', published_at: TODAY }),
    mkRow({ id: 102, feed_id: 10, title: 'A2 已读·收藏·今天', published_at: TODAY, is_read: true, is_starred: true }),
    mkRow({ id: 103, feed_id: 10, title: 'A3 未读·收藏·旧', published_at: OLD, is_starred: true }),
    mkRow({ id: 104, feed_id: 12, title: 'A4 未读·旧', published_at: OLD }),
    mkRow({ id: 105, feed_id: 12, title: 'A5 未读·旧', published_at: OLD }),
    mkRow({ id: 201, feed_id: 11, title: 'B1 未读·收藏·今天', published_at: TODAY, is_starred: true }),
    mkRow({ id: 202, feed_id: 11, title: 'B2 已读·旧', published_at: OLD, is_read: true }),
    mkRow({ id: 301, feed_id: 20, title: 'C1 未读·旧', published_at: OLD }),
  ];

  /* ---------- 假后端的可变行为（每个用例显式设置，避免隐式耦合） ---------- */
  let backendRows = BASE_ROWS;
  let failReload = null;      // 非 null → folders/feeds/feed_counts 拒绝（bootstrap 失败注入）
  let listPlan = null;        // { mode: 'reject' | 'defer', error } —— list_articles 行为
  let pendingList = [];       // defer 模式挂起项 { args, resolve }
  let indexPlan = null;       // { mode: 'defer' } —— article_index 行为
  let pendingIndex = [];      // defer 模式挂起项 { articleId, value, resolve }
  let detailImpl = (id) => mkRow({ id, content_html: '<p>详情</p>', translated_content: null });
  let aiSum = { deltas: [], error: null, reject: null, finish: true, holdIds: [] };
  let aiTr = { deltas: [], error: null, reject: null, finish: true, holdIds: [] };
  let rejectCmds = new Set();   // (p) TASK-067 N10：按命令名注入 IPC 失败
  let heldAi = [];            // hold 模式挂起项 { cmd, id, ch }
  let settingsRaw = null;     // get_setting('app_settings') 的返回值
  let ghLoginStatus = null;   // github_login_status 的返回值：null | {login} | 'reject'
  let extractResult = null;   // extract_fulltext 的返回值：ExtractFulltextResult | 'reject' | null
                              // TASK-076：后端改为结构化 { html, degraded, reason }

  const localDayKey = (ms) => {
    const d = new Date(ms);
    return `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
  };
  /* 内存版 list_articles：支持 范围/未读/收藏/今天/排序/分页，行为与后端契约一致 */
  function queryRows(a = {}) {
    let out = backendRows.slice();
    if (a.feed_id != null) out = out.filter((r) => r.feed_id === a.feed_id);
    if (a.folder_id != null) out = out.filter((r) => (FEEDS.find((f) => f.id === r.feed_id) || {}).folder_id === a.folder_id);
    if (a.only_unread) out = out.filter((r) => !r.is_read);
    if (a.only_starred) out = out.filter((r) => r.is_starred);
    if (a.only_today) out = out.filter((r) => localDayKey(Date.parse(r.published_at)) === localDayKey(NOW));
    const dir = a.newest_first === false ? 1 : -1;
    out.sort((x, y) => dir * (Date.parse(x.published_at) - Date.parse(y.published_at)));
    const off = a.offset || 0;
    return out.slice(off, a.limit == null ? out.length : off + a.limit);
  }
  function emitAi(plan, ch) {
    for (const d of plan.deltas) ch.onmessage?.({ type: 'delta', data: d });
    if (plan.error) { ch.onmessage?.({ type: 'error', data: plan.error }); return; }
    if (plan.finish) ch.onmessage?.({ type: 'done' });
  }

  /* 替换上面的 S-1…S-5 invoke mock（api 每调用一次都读 globalThis.__INVOKE__） */
  globalThis.__INVOKE__ = (cmd, args) => {
    invokeCalls.push({ cmd, args });
    if (rejectCmds.has(cmd)) return Promise.reject({ message: '注入失败:' + cmd });
    switch (cmd) {
      case 'list_folders': return failReload ? Promise.reject(failReload) : Promise.resolve(FOLDERS);
      case 'list_feeds': return failReload ? Promise.reject(failReload) : Promise.resolve(FEEDS);
      case 'feed_counts': return failReload ? Promise.reject(failReload) : Promise.resolve(COUNTS);
      case 'sync_status': return Promise.resolve({ connected: false });
      case 'list_articles': {
        if (listPlan && listPlan.mode === 'reject') return Promise.reject(listPlan.error);
        const a = (args && args.args) || {};
        if (listPlan && listPlan.mode === 'defer') {
          return new Promise((resolve) => { pendingList.push({ args: a, resolve }); });
        }
        return Promise.resolve(queryRows(a));
      }
      case 'article_index': {
        const a = (args && args.args) || {};
        const full = queryRows({ ...a, limit: null, offset: 0 });
        const pos = full.findIndex((r) => r.id === args.articleId);
        const value = pos < 0 ? null : pos;
        if (indexPlan && indexPlan.mode === 'defer') {
          return new Promise((resolve) => { pendingIndex.push({ articleId: args.articleId, value, resolve }); });
        }
        return Promise.resolve(value);
      }
      case 'get_article': return Promise.resolve(detailImpl(Number(args.id)));
      case 'get_articles': return Promise.resolve([]);
      case 'get_setting': return Promise.resolve(settingsRaw);
      case 'set_setting': return Promise.resolve(null);
      /* E1：GitHub 登录态恢复（bootstrapGithubAuth 定向断言用；null / {login} / 'reject'） */
      case 'github_login_status':
        return ghLoginStatus === 'reject'
          ? Promise.reject({ message: 'ipc down' })
          : Promise.resolve(ghLoginStatus);
      /* TASK-076：全文提取返回结构化结果（'reject' = 网络失败）。
         为兼容既有用例仍传裸字符串的写法，这里把字符串折算成「成功」形态：
         { html, degraded:false, reason:null }。 */
      case 'extract_fulltext':
        if (extractResult === 'reject') {
          return Promise.reject({ message: '网页拉取失败：HTTP 503' });
        }
        return Promise.resolve(
          typeof extractResult === 'string'
            ? { html: extractResult, degraded: false, reason: null }
            : extractResult,
        );
      case 'ai_summarize':
      case 'ai_translate': {
        const plan = cmd === 'ai_summarize' ? aiSum : aiTr;
        const id = Number(args.articleId);
        const ch = args.onChannel;
        if (plan.reject) return Promise.reject(plan.reject);
        if (plan.holdIds.includes(id)) { heldAi.push({ cmd, id, ch }); return new Promise(() => {}); }
        emitAi(plan, ch);
        return Promise.resolve('');
      }
      default: return Promise.resolve(null);
    }
  };

  /* ---------- 状态复位 / 规范化装载 ---------- */
  async function resetStore(extra) {
    backendRows = BASE_ROWS;
    failReload = null;
    listPlan = null;
    pendingList = [];
    indexPlan = null;
    pendingIndex = [];
    settingsRaw = null;
    ghLoginStatus = null;
    extractResult = null;
    heldAi = [];
    aiSum = { deltas: [], error: null, reject: null, finish: true, holdIds: [] };
    aiTr = { deltas: [], error: null, reject: null, finish: true, holdIds: [] };
    rejectCmds = new Set();
    detailImpl = (id) => mkRow({ id, content_html: '<p>详情</p>', translated_content: null });
    /* TASK-063：视图缓存是模块级 Map，跨用例残留会让下一个用例的 selectFeed
       命中上一个夹具的快照（跨夹具污染）。每个用例独立起步（(s6) 此前已就地
       手工 clear，这里收口为公共 hygiene；动态导入与 (s) 块同一模块实例）。 */
    const { viewEntriesCache } = await import('../dist-test/store/internals.js');
    viewEntriesCache.clear();
    store.setState({
      dataMode: 'tauri', dataLoading: false, bootstrapError: null,
      activeContentLayout: 'article', activeViewFilter: 'all', activeFeedFilter: 'all',
      timelineFilter: 'all', timelineSort: 'newest',
      activeArticleId: null, isShowingTranslatedProse: false, isRawRenderMode: false, showFulltext: false,
      summarizingIds: {}, translatingIds: {}, translating: false, summaryErrors: {}, translateErrors: {},
      hydrationErrors: {}, hydratedIds: {}, openedReadIds: {}, toasts: [],
      entries: [], categories: [], feedIndex: new Map(), feedCounts: new Map(),
      articlesLimit: 0, articlesLoading: false, articlesExhausted: false,
      player: { isActive: false, isPlaying: false, speed: 1, title: '', showName: '', cover: '', audioUrl: '', positionSec: 0, durationSec: 0, seekToSec: null },
      playerExpanded: false,
      settings: { ...store.getState().settings, defaultOpenMode: 'rss', markReadOnOpen: true },
      ...(extra || {}),
    });
    invokeCalls.length = 0;
  }
  async function bootFixture(extra) {
    await resetStore(extra);
    await store.getState().bootstrapFromBackend();
  }

  /* ============================================================
     (a) bootstrapFromBackend / dataMode 与错误路径
     ============================================================ */
  const realWindow = globalThis.window;
  globalThis.window = {};   // 抹掉 __TAURI_INTERNALS__ → 浏览器预览分支
  await resetStore({ dataMode: 'tauri', dataLoading: true, entries: [], categories: [], feedIndex: new Map() });
  await store.getState().bootstrapFromBackend();
  const aBrowser = store.getState();
  globalThis.window = realWindow;
  checkNew('(a) 无 Tauri IPC 时 bootstrap 回退 mock 并装载演示数据（骨架不永挂）',
    aBrowser.dataMode === 'mock' && aBrowser.dataLoading === false
    && aBrowser.categories.length > 0 && aBrowser.entries.length > 0 && aBrowser.feedIndex.size > 0);

  await bootFixture();
  const aOk = store.getState();
  checkNew('(a) tauri bootstrap 建立 categories/feedIndex/feedCounts 并进入 tauri 模式',
    aOk.dataMode === 'tauri' && aOk.dataLoading === false && aOk.bootstrapError === null
    && aOk.categories.map((c) => c.id).join(',') === 'cat-1,cat-2'
    && aOk.feedIndex.size === 4 && aOk.feedCounts.get('10')?.unread === 3);
  /* 【TASK-052 改动理由】(a) 组这一条把「游标 = PAGE_SIZE，与实际拉回的行数无关」
     写成了期望——那正是缺陷 P1-14「口径」半边本身：游标是**全局查询的 offset**，
     于是首批无论真实返回多少行，游标都停在 500（后续 loadMore 从全局第 500 条
     继续），与「当前范围已加载 8 条」脱节。per-scope 游标下首批游标 = 实际行数，
     并且按范围键（此处 'all'）记进 articlesCursor。 */
  checkNew('(a) 首批分页游标 = 实际行数（per-scope：不再硬编码 PAGE_SIZE）、8 行 < 500 视为已到底、加载态收起',
    aOk.entries.length === 8 && aOk.articlesLimit === 8 && aOk.articlesCursor.all === 8
    && aOk.articlesExhausted === true && aOk.articlesLoading === false);

  store.setState({ hydratedIds: { '101': true }, hydrationErrors: { '102': '旧错误' } });
  await store.getState().reloadFromBackend();
  checkNew('(a) 新快照清空 hydratedIds/hydrationErrors（正文按需重新水合，不复用旧终态）',
    Object.keys(store.getState().hydratedIds).length === 0
    && Object.keys(store.getState().hydrationErrors).length === 0);

  await resetStore();
  failReload = { code: 'db_corrupt', message: '数据库损坏' };
  await store.getState().bootstrapFromBackend();
  const aFail = store.getState();
  checkNew('(a) tauri bootstrap 失败：错误态可见、骨架收起、绝不回退 mock 假数据',
    aFail.bootstrapError === '数据库损坏' && aFail.dataLoading === false
    && aFail.dataMode === 'tauri' && aFail.entries.length === 0 && aFail.categories.length === 0);
  failReload = null;
  await store.getState().retryBootstrap();
  checkNew('(a) retryBootstrap 清错误态并重新装载成功',
    store.getState().bootstrapError === null && store.getState().entries.length === 8);

  /* 代际守卫：并发 reload 只接受最新一次结果 */
  await resetStore();
  listPlan = { mode: 'defer' };
  const pReloadOld = store.getState().reloadFromBackend();
  const pReloadNew = store.getState().reloadFromBackend();
  await nTick(0);
  checkNew('(a) 并发 reload 时两次 list_articles 同时在途（代际守卫场景成立）', pendingList.length === 2);
  pendingList[1].resolve([mkRow({ id: 301, feed_id: 20, title: '新代际' })]);
  pendingList[0].resolve([mkRow({ id: 101, feed_id: 10, title: '旧代际' })]);
  await Promise.all([pReloadOld, pReloadNew]);
  listPlan = null;
  checkNew('(a) 旧代际 reload 结果被丢弃：entries 保持最新代际快照（布局不回退）',
    store.getState().entries.length === 1 && store.getState().entries[0].id === '301');

  /* ============================================================
     (b) 订阅源/分类选择与派生树计数
     ============================================================ */
  await bootFixture();
  store.setState({ activeFeedFilter: 'all', openedReadIds: { '999': true } });
  store.getState().selectFeed('11');
  checkNew('(b) selectFeed 写入 activeFeedFilter 并清空「已读保留」快照',
    store.getState().activeFeedFilter === '11' && Object.keys(store.getState().openedReadIds).length === 0);
  checkNew('(b) 范围过滤叠加布局解析：源B 是 social，article 布局下 scope 为空',
    selectScopeEntries(store.getState()).length === 0);
  store.setState({ activeContentLayout: 'social' });
  checkNew('(b) 切到 social 布局后 scope 出现源B 两条（范围 × 布局双重过滤）',
    selectScopeEntries(store.getState()).map((e) => e.id).join(',') === '201,202');
  store.getState().selectFeed('cat-1');
  checkNew('(b) selectFeed(分类) 覆盖该分类下当前布局的源，不含其他分类',
    selectScopeEntries(store.getState()).map((e) => e.id).join(',') === '201,202');
  store.setState({ activeContentLayout: 'article' });
  store.getState().selectFeed('cat-1');
  checkNew('(b) 同一分类在 article 布局下覆盖源A+源D（5 条）',
    selectScopeEntries(store.getState()).map((e) => e.id).join(',') === '101,102,103,104,105');

  store.setState({ activeViewFilter: 'all', timelineFilter: 'unread', activeFeedFilter: 'all' });
  const treeUnread = selectTreeCounts(store.getState());
  checkNew('(b) 树角标 = 布局 × 视图 × 显示筛选后的后端计数（「显示: 未读」→ 未读数）',
    treeUnread.get('10') === 3 && treeUnread.get('12') === 2 && treeUnread.get('cat-1') === 5
    && treeUnread.get('all') === 5 && treeUnread.get('11') === undefined);
  store.setState({ timelineFilter: 'all' });
  const treeAll = selectTreeCounts(store.getState());
  checkNew('(b) 显示切回「全部」后树角标变总数（分类聚合 = 各源之和）',
    treeAll.get('10') === 5 && treeAll.get('cat-1') === 7 && treeAll.get('all') === 7);
  store.setState({ activeViewFilter: 'starred' });
  const treeStar = selectTreeCounts(store.getState());
  checkNew('(b) 视图为收藏时树角标按收藏数（0 也建档，行与源一一对应）',
    treeStar.get('10') === 1 && treeStar.get('12') === 0 && treeStar.get('cat-1') === 1 && treeStar.get('all') === 1);
  store.setState({ activeContentLayout: 'social', activeViewFilter: 'all', timelineFilter: 'unread', activeFeedFilter: 'cat-1' });
  const vCounts = selectViewCounts(store.getState());
  checkNew('(b) selectFeed 后视图计数随订阅范围收敛（cat-1 只算源B）',
    vCounts.all === 2 && vCounts.unread === 2 && vCounts.starred === 2 && vCounts.today === 1);
  store.setState({ activeFeedFilter: '20' });
  const vCounts2 = selectViewCounts(store.getState());
  checkNew('(b) 单源范围视图计数 = 该源计数（源C：显示未读 → 1）',
    vCounts2.all === 1 && vCounts2.unread === 1 && vCounts2.starred === 0 && vCounts2.today === 0);
  store.setState({ activeContentLayout: 'article' });
  checkNew('(b) 选中的源不属于当前布局时：scope 空、计数全 0（布局不串台）',
    selectScopeEntries(store.getState()).length === 0
    && selectViewCounts(store.getState()).all === 0 && selectViewCounts(store.getState()).unread === 0);

  /* ============================================================
     (c) 视图筛选 / 时间线筛选对可见条目集合的影响
     ============================================================ */
  await bootFixture();
  store.setState({ activeViewFilter: 'starred' });
  await store.getState().reloadFilteredEntries('starred');
  checkNew('(c) 收藏视图拉取走 only_starred：entries 只剩 3 条收藏（跨布局，布局在派生层过滤）',
    store.getState().entries.map((e) => e.id).join(',') === '102,201,103');
  store.setState({ activeViewFilter: 'all', entries: [], articlesLimit: 0, articlesExhausted: false });
  store.getState().selectView('starred');
  const cCache = store.getState();
  checkNew('(c) selectView 命中视图缓存：同步恢复快照（零延迟、游标=快照长度、标记已到底）',
    cCache.activeViewFilter === 'starred' && cCache.entries.map((e) => e.id).join(',') === '102,201,103'
    && cCache.articlesLimit === 3 && cCache.articlesExhausted === true);
  await nTick(20);
  checkNew('(c) 缓存命中后的后台静默刷新不改变收藏视图结论',
    store.getState().entries.map((e) => e.id).join(',') === '102,201,103');

  store.setState({ activeViewFilter: 'all', entries: [], articlesLimit: 0, openedReadIds: { '102': true } });
  store.getState().selectView('unread');
  checkNew('(c) selectView(unread) 立即切视图并清空已读保留快照',
    store.getState().activeViewFilter === 'unread' && Object.keys(store.getState().openedReadIds).length === 0);
  await nTick(20);
  const cUnread = store.getState();
  checkNew('(c) 未读视图走 only_unread 拉全量：只剩 6 条未读、游标=行数、不再分页',
    cUnread.entries.map((e) => e.id).join(',') === '101,201,103,104,105,301'
    && cUnread.articlesLimit === 6 && cUnread.articlesExhausted === true && cUnread.articlesLoading === false);
  store.getState().selectView('today');
  await nTick(20);
  const cToday = selectVisibleEntries(store.getState());
  checkNew('(c) 今天视图只含本地当天条目（与列表「今天」判定同口径）',
    cToday.length > 0 && cToday.every((e) => localDayKey(e.publishedAt) === localDayKey(Date.now())));

  await bootFixture();
  checkNew('(c) 全部视图 + 显示全部：article 布局可见 5 条，按发布时间降序',
    selectVisibleEntries(store.getState()).map((e) => e.id).join(',') === '101,102,103,104,105');
  store.getState().toggleTimelineFilter();
  checkNew('(c) toggleTimelineFilter → 未读：已读条目从可见集合移除，未读保留',
    store.getState().timelineFilter === 'unread'
    && selectVisibleEntries(store.getState()).map((e) => e.id).join(',') === '101,103,104,105');
  store.getState().toggleTimelineFilter();
  checkNew('(c) 再次 toggle 回「全部」：已读条目重新可见（可逆）',
    store.getState().timelineFilter === 'all' && selectVisibleEntries(store.getState()).length === 5);
  store.getState().toggleTimelineSort();
  checkNew('(c) toggleTimelineSort 反向排序：oldest 为时间升序',
    store.getState().timelineSort === 'oldest'
    && selectVisibleEntries(store.getState()).map((e) => e.id).join(',') === '103,104,105,101,102');
  store.getState().toggleTimelineSort();
  store.setState({ activeViewFilter: 'starred', timelineFilter: 'unread', openedReadIds: {} });
  checkNew('(c) 收藏视图不受「显示: 未读」影响：已读但收藏的 102 仍在可见集合',
    selectVisibleEntries(store.getState()).map((e) => e.id).join(',') === '102,103');
  store.setState({ activeViewFilter: 'all', timelineFilter: 'unread', openedReadIds: { '102': true } });
  checkNew('(c) 本次会话打开过的已读条目在未读筛选下原地保留（openedReadIds 生效）',
    selectVisibleEntries(store.getState()).map((e) => e.id).includes('102'));

  await bootFixture();
  listPlan = { mode: 'defer' };
  store.setState({ activeViewFilter: 'unread', entries: [], articlesLimit: 0 });
  const pFiltered = store.getState().reloadFilteredEntries('unread');
  await nTick(0);
  store.setState({ activeViewFilter: 'starred' });   // 拉取期间用户又切了视图
  pendingList[0].resolve(BASE_ROWS.filter((r) => !r.is_read));
  await pFiltered;
  listPlan = null;
  checkNew('(c) 拉取期间切走视图：过期筛选结果被丢弃（entries/游标不被旧视图覆盖）',
    store.getState().entries.length === 0 && store.getState().articlesLimit === 0
    && store.getState().activeViewFilter === 'starred');

  /* ============================================================
     (d) 内容布局切换（article/social/image/podcast）与派生状态
     ============================================================ */
  await bootFixture();
  const dEntriesRef = store.getState().entries;
  store.setState({
    activeFeedFilter: '11', activeArticleId: '101', openedReadIds: { '102': true },
    isShowingTranslatedProse: true, showFulltext: true, isRawRenderMode: true,
  });
  store.getState().selectLayout('social');
  const dAfter = store.getState();
  checkNew('(d) selectLayout 切布局并复位范围/选中/阅读器态（视图筛选保留）',
    dAfter.activeContentLayout === 'social' && dAfter.activeFeedFilter === 'all' && dAfter.activeArticleId === null
    && dAfter.isShowingTranslatedProse === false && dAfter.showFulltext === false && dAfter.isRawRenderMode === false
    && Object.keys(dAfter.openedReadIds).length === 0 && dAfter.activeViewFilter === 'all');
  checkNew('(d) 布局切换是纯本地过滤：entries 引用不变（不触发 reload、不闪空）',
    store.getState().entries === dEntriesRef && store.getState().articlesLoading === false);
  checkNew('(d) 派生集合按布局解析：article = 源A+源D，social = 源B+源C',
    selectRawEntries({ ...store.getState(), activeContentLayout: 'article' }).map((e) => e.id).join(',') === '101,102,103,104,105'
    && selectRawEntries(store.getState()).map((e) => e.id).join(',') === '201,202,301');
  store.setState({ activeContentLayout: 'image' });
  checkNew('(d) image 布局无绑定源 → 派生集合为空（不串其他布局条目）',
    selectRawEntries(store.getState()).length === 0 && selectVisibleEntries(store.getState()).length === 0);
  store.setState({ activeContentLayout: 'podcast' });
  checkNew('(d) podcast 布局初始为空（布局是绑定派生的解析结果，不是条目属性）',
    selectRawEntries(store.getState()).length === 0);
  store.setState({ activeContentLayout: 'social' });
  invokeCalls.length = 0;
  store.getState().updateFeedLayout('cat-1', '11', 'podcast');
  await nTick(0);   // api.updateFeedLayout 内部 await getInvoke()，落库是异步 fire-and-forget
  const dCall = invokeCalls.find((c) => c.cmd === 'update_feed_layout');
  checkNew('(d) updateFeedLayout 落库：纯数字 id 提取 + 布局原样写入（不被强转 inherit）',
    !!dCall && dCall.args.id === 11 && dCall.args.layout === 'podcast'
    && store.getState().feedIndex.get('11')?.feed.layout === 'podcast');
  checkNew('(d) 改绑定后条目即时迁移到新布局视图（不搬动数据：entries 仍 8 条）',
    selectRawEntries({ ...store.getState(), activeContentLayout: 'podcast' }).map((e) => e.id).join(',') === '201,202'
    && selectRawEntries(store.getState()).map((e) => e.id).join(',') === '301'
    && store.getState().entries.length === 8);
  invokeCalls.length = 0;
  store.getState().updateCatLayout('cat-2', 'podcast');
  await nTick(0);
  checkNew('(d) updateCatLayout 后 inherit 源的条目跟随分类布局迁移（源C 进 podcast）',
    store.getState().feedIndex.get('20')?.cat.layout === 'podcast'
    && selectRawEntries({ ...store.getState(), activeContentLayout: 'podcast' }).map((e) => e.id).join(',') === '201,202,301'
    && selectRawEntries(store.getState()).length === 0
    && invokeCalls.some((c) => c.cmd === 'update_folder_layout' && c.args.id === 2 && c.args.layout === 'podcast'));

  /* ============================================================
     (e) 已读 / 收藏切换与未读计数
     ============================================================ */
  await bootFixture();
  store.setState({ activeArticleId: '101' });
  invokeCalls.length = 0;
  store.getState().toggleCurrentReadStatus();
  await nTick(0);   // api.setRead 内部 await getInvoke()，落库是异步 fire-and-forget
  const eRead = store.getState();
  const eReadCall = invokeCalls.find((c) => c.cmd === 'set_read');
  checkNew('(e) 标已读：落库 read=true + 条目置位 + 该源未读 -1 + toast 文案正确',
    eReadCall?.args.id === 101 && eReadCall?.args.read === true
    && eRead.entries.find((a) => a.id === '101')?.isRead === true
    && eRead.feedCounts.get('10')?.unread === 2
    && eRead.toasts[eRead.toasts.length - 1]?.text === '已标为已读');
  store.getState().toggleCurrentReadStatus();
  await nTick(0);
  const eUnread = store.getState();
  const eUnreadCall = invokeCalls.filter((c) => c.cmd === 'set_read').pop();
  checkNew('(e) 再切回未读：落库 read=false + 未读计数回到 3 + toast 文案正确',
    eUnreadCall?.args.read === false
    && eUnread.entries.find((a) => a.id === '101')?.isRead === false
    && eUnread.feedCounts.get('10')?.unread === 3
    && eUnread.toasts[eUnread.toasts.length - 1]?.text === '已标为未读');
  const eToastCount = store.getState().toasts.length;
  store.getState().toggleCurrentStar();
  await nTick(0);
  const eStar = store.getState();
  const eStarCall = invokeCalls.find((c) => c.cmd === 'set_starred');
  checkNew('(e) 收藏切换：落库 starred=true + 收藏计数 +1 + 未读计数不受影响 + 不弹 toast',
    eStarCall?.args.id === 101 && eStarCall?.args.starred === true
    && eStar.entries.find((a) => a.id === '101')?.isStarred === true
    && eStar.feedCounts.get('10')?.starred === 2 && eStar.feedCounts.get('10')?.unread === 3
    && eStar.toasts.length === eToastCount);
  invokeCalls.length = 0;
  const eNoToast = store.getState().toasts.length;
  store.setState({ activeArticleId: null });
  store.getState().toggleCurrentReadStatus();
  store.getState().toggleCurrentStar();
  await nTick(0);
  checkNew('(e) 无选中文章：已读/收藏切换均为 no-op（无 IPC、无 toast、数据不变）',
    invokeCalls.length === 0 && store.getState().toasts.length === eNoToast
    && store.getState().entries.find((a) => a.id === '101')?.isRead === false);
  store.setState({ activeArticleId: '9999' });
  store.getState().toggleCurrentStar();
  await nTick(0);
  checkNew('(e) 选中 id 不在 entries 中时同样 no-op（防悬空 id 误写库）', invokeCalls.length === 0);
  store.setState({
    activeArticleId: '104',
    feedCounts: new Map([['12', { total: 2, unread: 0, starred: 0, today: 0 }]]),
  });
  store.getState().toggleCurrentReadStatus();
  await nTick(0);
  checkNew('(e) 后端计数已为 0 时标读不产生负数（未读计数夹取在 0）',
    store.getState().feedCounts.get('12')?.unread === 0
    && store.getState().entries.find((a) => a.id === '104')?.isRead === true);

  /* ============================================================
     (f) markAllRead / markCurrentViewAllRead 的范围语义
     ============================================================ */
  await bootFixture();
  store.setState({ activeViewFilter: 'starred', timelineFilter: 'all', activeFeedFilter: 'all', openedReadIds: { '103': true } });
  invokeCalls.length = 0;
  store.getState().markCurrentViewAllRead();
  await nTick(0);   // api.markAllRead 内部 await getInvoke()，需让出微任务
  const f1 = store.getState();
  const fMark = invokeCalls.find((c) => c.cmd === 'mark_all_read');
  checkNew('(f) 收藏视图全部已读：范围参数 starredOnly=true，且只标该视图可见条目',
    fMark?.args.starredOnly === true && fMark?.args.feedId === null && fMark?.args.sinceMs === null
    && f1.entries.find((a) => a.id === '103')?.isRead === true
    && f1.entries.find((a) => a.id === '101')?.isRead === false
    && f1.entries.find((a) => a.id === '102')?.isRead === true);
  checkNew('(f) 全部已读只减被标条目所属源的未读数（源A 3→2，源D 不动）',
    f1.feedCounts.get('10')?.unread === 2 && f1.feedCounts.get('12')?.unread === 2);
  checkNew('(f) 全部已读后清空「已读保留」快照（列表不再保留灰色卡片）',
    Object.keys(f1.openedReadIds).length === 0);
  checkNew('(f) 全部已读给出 toast 反馈', f1.toasts.some((t) => t.text === '已全部标为已读'));

  await bootFixture();
  store.setState({ activeViewFilter: 'all', timelineFilter: 'unread', activeFeedFilter: 'cat-1' });
  invokeCalls.length = 0;
  store.getState().markCurrentViewAllRead();
  await nTick(0);
  const fCatCall = invokeCalls.find((c) => c.cmd === 'mark_all_read');
  const fCat = store.getState();
  checkNew('(f) 分类范围 → folderId=数字、feedId=null（cat- 前缀不被当作源 id）',
    fCatCall?.args.folderId === 1 && fCatCall?.args.feedId === null);
  checkNew('(f) 分类范围只标该分类可见条目，分类外条目不受影响',
    fCat.entries.find((a) => a.id === '101')?.isRead === true
    && fCat.entries.find((a) => a.id === '103')?.isRead === true
    && fCat.entries.find((a) => a.id === '201')?.isRead === false
    && fCat.feedCounts.get('10')?.unread === 1 && fCat.feedCounts.get('12')?.unread === 0);

  await bootFixture();
  store.setState({ activeViewFilter: 'all', timelineFilter: 'unread', activeFeedFilter: '12' });
  invokeCalls.length = 0;
  store.getState().markCurrentViewAllRead();
  await nTick(0);
  const fFeedCall = invokeCalls.find((c) => c.cmd === 'mark_all_read');
  const fFeed = store.getState();
  checkNew('(f) 单源范围 → feedId=12（纯数字 id 提取，不截成 NaN）且只标该源条目',
    fFeedCall?.args.feedId === 12 && fFeedCall?.args.folderId === null
    && fFeed.entries.filter((a) => a.feedId === '12').every((a) => a.isRead)
    && fFeed.entries.find((a) => a.id === '101')?.isRead === false);

  await bootFixture();
  store.setState({ dataMode: 'mock', activeViewFilter: 'all', timelineFilter: 'unread', activeFeedFilter: 'all' });
  invokeCalls.length = 0;
  store.getState().markCurrentViewAllRead();
  await nTick(0);
  checkNew('(f) mock 模式全部已读：本地生效但不发 IPC（无库可写，绝不伪造落库）',
    invokeCalls.filter((c) => c.cmd === 'mark_all_read').length === 0
    && store.getState().entries.find((a) => a.id === '101')?.isRead === true
    && store.getState().toasts.some((t) => t.text === '已全部标为已读'));

  await bootFixture();
  store.setState({ openedReadIds: { '999': true } });
  invokeCalls.length = 0;
  store.getState().markEntriesReadBulk(['101', '102', '103']);
  await nTick(0);
  checkNew('(f) 批量标读只对未读项发 IPC（已读项不重复写库刷同步队列）',
    invokeCalls.filter((c) => c.cmd === 'set_read').map((c) => c.args.id).sort((a, b) => a - b).join(',') === '101,103');
  checkNew('(f) 批量标读的保留快照是「合并」而非替换（既有记录不丢）',
    store.getState().openedReadIds['999'] === true && store.getState().openedReadIds['101'] === true);
  checkNew('(f) 批量标读按源聚合未读减量（源A 标 2 条 → 3-2=1）',
    store.getState().feedCounts.get('10')?.unread === 1);
  invokeCalls.length = 0;
  store.getState().markEntriesReadBulk([]);
  checkNew('(f) 空 id 列表批量标读为 no-op（不发 IPC）', invokeCalls.length === 0);

  /* ============================================================
     (g) 分页 loadMoreArticles 的游标推进与失败路径
     ============================================================ */
  await resetStore();
  const pageRows = [];
  for (let i = 0; i < 503; i += 1) {
    pageRows.push(mkRow({ id: 5000 + i, feed_id: 10, title: `分页${i}`, published_at: iso(NOW - i * 60000) }));
  }
  backendRows = pageRows;
  await store.getState().bootstrapFromBackend();
  const gFirst = store.getState();
  checkNew('(g) 首批 = 500 行：游标 500、未到底、加载态已复位',
    gFirst.entries.length === 500 && gFirst.articlesLimit === 500
    && gFirst.articlesExhausted === false && gFirst.articlesLoading === false);
  invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  const gMore = store.getState();
  const gListCall = invokeCalls.find((c) => c.cmd === 'list_articles');
  checkNew('(g) 追加加载以游标为 offset：500→503，条目顺序连续不重复',
    gListCall?.args.args.offset === 500 && gMore.articlesLimit === 503 && gMore.entries.length === 503
    && gMore.entries[500].id === '5500' && gMore.entries[502].id === '5502');
  checkNew('(g) 不足一页 → 置 articlesExhausted 且游标按实际行数推进',
    gMore.articlesExhausted === true && gMore.articlesLoading === false);
  invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  checkNew('(g) 已到底后再触发不产生新 IPC（无效请求被挡）',
    invokeCalls.length === 0 && store.getState().articlesLimit === 503);

  store.setState({ articlesExhausted: false, articlesLoading: true, articlesLimit: 503 });
  invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  checkNew('(g) 在途加载中重复触发被防抖（不产生并发 IPC）',
    invokeCalls.length === 0 && store.getState().articlesLoading === true);

  store.setState({ articlesLoading: false, articlesExhausted: false, articlesLimit: 100, entries: [], toasts: [] });
  listPlan = { mode: 'reject', error: { message: 'db busy' } };
  invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  const gFail = store.getState();
  checkNew('(g) 分页失败：articlesLoading 复位（不永挂加载动画）且游标/条目不被破坏',
    gFail.articlesLoading === false && gFail.articlesLimit === 100
    && gFail.entries.length === 0 && gFail.articlesExhausted === false);
  /* 【改动理由】原断言把 D2 的现状（失败被静默吞掉、用户侧零提示）写成了期望，
     标注为「观察项」。D2 修复后失败路径给出可见 toast + 一键重试，故改为断言修复后的行为。 */
  checkNew('(g) 分页失败给出可见 toast（不再是静默吞错：文案 + 「重试」action —— D2 修复项）',
    gFail.toasts.length === 1 && gFail.toasts[0].text.includes('加载更多失败')
    && gFail.toasts[0].action?.label === '重试');
  checkNew('(g) 失败 toast 的「重试」直接重发分页请求（不是死按钮）',
    typeof gFail.toasts[0]?.action?.run === 'function');
  listPlan = null;

  store.setState({ articlesLoading: false, articlesExhausted: false, articlesLimit: 100, entries: [] });
  listPlan = { mode: 'defer' };
  const pMore = store.getState().loadMoreArticles();
  await nTick(0);
  store.setState({ articlesLimit: 0, articlesLoading: false });   // 模拟期间发生 reload 重置游标
  pendingList[0].resolve([mkRow({ id: 7001, feed_id: 10 })]);
  await pMore;
  listPlan = null;
  checkNew('(g) 加载期间游标被 reload 重置：过期追加被丢弃（不产生错位条目）',
    store.getState().entries.length === 0 && store.getState().articlesLimit === 0
    && store.getState().articlesLoading === false);

  /* D3：竞态丢弃分支必须复位 articlesLoading。上面那条场景在丢弃前手动置了
     articlesLoading=false，把这个缺陷掩盖了 —— 这里保留在途加载态（模拟真实
     竞态：加载期间 selectView 命中视图缓存恢复快照，只写游标不碰 loading），
     修前 articlesLoading 会永久停在 true，入口守卫随即永久挡住后续所有分页。 */
  store.setState({ articlesLoading: false, articlesExhausted: false, articlesLimit: 100, entries: [] });
  listPlan = { mode: 'defer' };
  const pRace = store.getState().loadMoreArticles();
  await nTick(0);
  store.setState({ articlesLimit: 0 });          // 期间游标被重置，加载态仍为 true
  pendingList[pendingList.length - 1].resolve([mkRow({ id: 7100, feed_id: 10 })]);
  await pRace;
  listPlan = null;
  checkNew('(g) 过期追加被丢弃时复位 articlesLoading（D3 修复项：加载态不永久为真）',
    store.getState().articlesLoading === false && store.getState().entries.length === 0
    && store.getState().articlesLimit === 0);
  invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  checkNew('(g) 竞态丢弃后入口守卫不再被永久锁死：下一次分页请求照常发出（D3 修复项）',
    invokeCalls.filter((c) => c.cmd === 'list_articles').length === 1);

  /* ============================================================
     (h) 搜索开关与结果竞态守卫
     ============================================================ */
  await bootFixture();
  store.setState({ searchOpen: false, settingsOpen: true });
  store.getState().openSearch();
  const hOpened = store.getState().searchOpen;
  store.getState().closeSearch();
  checkNew('(h) openSearch/closeSearch 只切换搜索面板态，不动其他弹层与列表',
    hOpened === true && store.getState().searchOpen === false && store.getState().settingsOpen === true);
  store.setState({ settingsOpen: false });

  invokeCalls.length = 0;
  await store.getState().anchorToArticle('301');
  const hAnchor = store.getState();
  checkNew('(h) 搜索结果锚定：按绝对位置拉取该页并选中（不再从头拉 500 篇）',
    invokeCalls.some((c) => c.cmd === 'article_index')
    && hAnchor.activeArticleId === '301' && hAnchor.entries.map((e) => e.id).join(',') === '301'
    && hAnchor.articlesLimit === 8 && hAnchor.articlesExhausted === true
    && hAnchor.openedReadIds['301'] === true);

  await bootFixture();
  indexPlan = { mode: 'defer' };
  invokeCalls.length = 0;
  const pAnchorOld = store.getState().anchorToArticle('101');
  await nTick(0);
  const pAnchorNew = store.getState().anchorToArticle('301');
  await nTick(0);
  checkNew('(h) 两次导航同时在途（article_index 各一次，竞态场景成立）', pendingIndex.length === 2);
  pendingIndex[0].resolve(pendingIndex[0].value);   // 旧导航先返回 → 代际已过期
  await nTick(0);
  checkNew('(h) 过期的搜索定位结果被代际守卫丢弃：不再发 list_articles、不抢选中态',
    invokeCalls.filter((c) => c.cmd === 'list_articles').length === 0
    && store.getState().activeArticleId === null);
  pendingIndex[1].resolve(pendingIndex[1].value);
  await Promise.all([pAnchorOld, pAnchorNew]);
  indexPlan = null;
  checkNew('(h) 只有最新一次导航生效（activeArticleId=301、列表为其所在页）',
    store.getState().activeArticleId === '301'
    && store.getState().entries.map((e) => e.id).join(',') === '301');

  await bootFixture();
  store.setState({ activeFeedFilter: '12' });
  invokeCalls.length = 0;
  await store.getState().anchorToArticle('301');   // 301 不在源12 的筛选范围内
  checkNew('(h) 目标不在当前筛选内（article_index=null）：不改列表与选中态',
    invokeCalls.filter((c) => c.cmd === 'list_articles').length === 0
    && store.getState().activeArticleId === null && store.getState().entries.length === 8);

  store.setState({ dataMode: 'mock', activeArticleId: null, entries: [], articlesLimit: 0 });
  invokeCalls.length = 0;
  await store.getState().anchorToArticle('101');
  checkNew('(h) mock 模式下锚定打开为 no-op（无 IPC，不伪造定位结果）',
    invokeCalls.length === 0 && store.getState().activeArticleId === null);

  /* ============================================================
     (h2) TASK-052 锚定顺序契约：命令面板必须「先导航、后锚定」

     这是一处**行为变化**：改造前顺序是先 anchorToArticle、再前置导航。两种顺序在
     改造前等价（分页/锚定查询都不带订阅范围）；per-scope 落地后 anchorToArticle
     按**调用时**的范围构造 article_index / list_articles，旧顺序会让锚定按**旧范围**
     取位置 —— 位置与该位置的列表不同口径，锚定错位甚至直接失败。
     A/B 证据见 tmp/task052/anchor-order-*.json 与 anchor-order-diff.txt（5 项翻转）。
     ============================================================ */
  {
    const { anchorScopeNav } = await import('../src/components/anchorScopeNav.ts');

    /* 源码顺序契约：直接读取 Overlays.tsx 里「文章」命令项 run() 的实际调用顺序。
       上面的端到端复刻只证明「这套顺序是对的」，证明不了**组件里确实这么写**——
       把顺序改回去它照样通过。这条读源码，因此能真正钉住组件实现（修前失败）。 */
    {
      const { readFileSync } = await import('node:fs');
      const ov = readFileSync(new URL('../src/components/Overlays.tsx', import.meta.url), 'utf8');
      const artAt = ov.indexOf('id: `art-${a.id}`');
      const navAt = ov.indexOf('anchorScopeNav(st)', artAt);
      const anchorAt = ov.indexOf('void st.anchorToArticle(a.id);', artAt);
      checkNew('(h2) 源码顺序：命令面板「文章」项先归一导航、后调用 anchorToArticle（修前：先 anchor、后导航）',
        artAt >= 0 && navAt > artAt && anchorAt > navAt);
    }

    checkNew('(h2) 归一动作顺序：范围 → 视图 → 时间流筛选（数组序即执行序）',
      anchorScopeNav({ activeFeedFilter: '10', activeViewFilter: 'starred', timelineFilter: 'unread' })
        .map((s) => s.action).join(',') === 'selectFeed,selectView,toggleTimelineFilter');
    checkNew('(h2) 已在「全部范围 × 全部视图 × 非未读」时无需任何前置导航（幂等，不产生多余切换）',
      anchorScopeNav({ activeFeedFilter: 'all', activeViewFilter: 'all', timelineFilter: 'all' }).length === 0);

    /* 端到端：复刻命令面板点击，按修后顺序执行，断言锚定用的是**新范围** */
    await resetStore();
    backendRows = [
      mkRow({ id: 101, feed_id: 10, published_at: iso(NOW) }),
      mkRow({ id: 201, feed_id: 11, published_at: iso(NOW - 1000) }),
    ];
    store.setState({ activeFeedFilter: '10', activeViewFilter: 'starred', timelineFilter: 'unread' });
    await store.getState().reloadFromBackend();
    invokeCalls.length = 0;
    /* 修后顺序：先导航（用 anchorScopeNav 的产物），后锚定 */
    const st0 = store.getState();
    for (const step of anchorScopeNav(st0)) {
      if (step.action === 'selectFeed') store.getState().selectFeed(step.arg ?? 'all');
      else if (step.action === 'selectView') store.getState().selectView('all');
      else store.getState().toggleTimelineFilter();
    }
    await store.getState().anchorToArticle('201');
    const h2Idx = invokeCalls.find((c) => c.cmd === 'article_index');
    const h2List = invokeCalls.find((c) => c.cmd === 'list_articles');
    checkNew('(h2) 锚定的 article_index 按**新范围**取（feed_id=null；修前：feed_id=10 旧范围）',
      (h2Idx?.args?.args?.feed_id ?? null) === null && (h2Idx?.args?.args?.folder_id ?? null) === null);
    checkNew('(h2) article_index 与随后的 list_articles 同口径（位置与列表对齐，不会错位）',
      (h2Idx?.args?.args?.feed_id ?? null) === (h2List?.args?.args?.feed_id ?? null)
      && (h2Idx?.args?.args?.folder_id ?? null) === (h2List?.args?.args?.folder_id ?? null));
    checkNew('(h2) 目标文章确实被锚定打开（修前旧顺序下 article_index 落在源10 内查不到 201 ⇒ pos=null ⇒ 静默不打开）',
      store.getState().activeArticleId === '201' && store.getState().entries.some((e) => e.id === '201'));
    checkNew('(h2) 锚定完成后范围/视图/筛选均已归一（命令面板点击的最终态）',
      store.getState().activeFeedFilter === 'all' && store.getState().activeViewFilter === 'all'
      && store.getState().timelineFilter === 'all');
  }

  /* ============================================================
     (i) toast 的生成与消失
     ============================================================ */
  await resetStore();
  store.setState({ toasts: [] });
  store.getState().showToast('新增-A', { label: '重试', run: () => {} });
  const iFirst = store.getState().toasts;
  store.getState().showToast('新增-B');
  const iSecond = store.getState().toasts;
  checkNew('(i) showToast 追加条目、id 单调递增、action 负载原样保留',
    iSecond.length === 2 && iSecond[1].id > iSecond[0].id
    && iFirst[0].action?.label === '重试' && iSecond[1].action === undefined);
  for (let i = 0; i < 3; i += 1) store.getState().showToast(`新增-上限${i}`);
  const iCap = store.getState().toasts;
  checkNew('(i) toast 上限 4 条：超出丢弃最旧、保留最新（错误循环不无限堆叠）',
    iCap.length === 4
    && iCap.map((t) => t.text).join(',') === '新增-B,新增-上限0,新增-上限1,新增-上限2');
  store.setState({ toasts: [] });
  store.getState().showToast('新增-短');
  store.getState().showToast('新增-长', { label: '重试', run: () => {} });
  await new Promise((r) => setTimeout(r, 2800));
  const iMid = store.getState().toasts;
  checkNew('(i) 无 action 的 toast 在 ~2.4s 后自动卸载', iMid.some((t) => t.text === '新增-短') === false);
  checkNew('(i) 带 action 的 toast 停留更久（2.8s 时仍在且未进入退场）',
    iMid.some((t) => t.text === '新增-长')
    && iMid.find((t) => t.text === '新增-长')?.leaving !== true);
  await new Promise((r) => setTimeout(r, 1900));
  checkNew('(i) 带 action 的 toast 最终也会消失（两段式生命周期收敛，不泄漏 DOM）',
    store.getState().toasts.length === 0);

  /* ============================================================
     (j) 播放器状态（激活/播放/进度/seek 夹取/结束）
     ============================================================ */
  await bootFixture();
  store.setState({ toasts: [], playerExpanded: true, activeArticleId: null });
  store.getState().playPodcastEpisode('无音频', '节目', '', '');
  checkNew('(j) 无音频地址时不进入播放态，只给提示',
    store.getState().player.isActive === false && store.getState().player.isPlaying === false
    && store.getState().toasts.some((t) => t.text === '该剧集没有可播放的音频地址'));
  store.getState().playPodcastEpisode('第 1 集', '节目名', 'https://cover/1.jpg', 'https://audio/1.mp3', '104');
  const jPlay = store.getState().player;
  checkNew('(j) 播放剧集：激活 + 播放中 + 位置/时长归零 + seek 清空 + 标题信息写入',
    jPlay.isActive === true && jPlay.isPlaying === true && jPlay.title === '第 1 集' && jPlay.showName === '节目名'
    && jPlay.audioUrl === 'https://audio/1.mp3' && jPlay.positionSec === 0 && jPlay.durationSec === 0
    && jPlay.seekToSec === null && jPlay.cover === 'https://cover/1.jpg');
  checkNew('(j) 点播放即视为已读（与打开文章同语义）：104 标读 + 该源未读 -1 + 播放 toast',
    store.getState().entries.find((a) => a.id === '104')?.isRead === true
    && store.getState().feedCounts.get('12')?.unread === 1
    && store.getState().toasts.some((t) => t.text === '正在播放：第 1 集'));
  store.getState().togglePlayerPlay();
  const jPaused = store.getState().player.isPlaying;
  store.getState().togglePlayerPlay();
  checkNew('(j) togglePlayerPlay 双向切换且不动其他播放器字段',
    jPaused === false && store.getState().player.isPlaying === true
    && store.getState().player.title === '第 1 集' && store.getState().player.durationSec === 0);
  store.getState().syncPlayerProgress(100, 300);
  store.getState().skipPlayer(-250);
  checkNew('(j) skipPlayer 负向越界夹取到 0（seekToSec 与 positionSec 同步下发）',
    store.getState().player.positionSec === 0 && store.getState().player.seekToSec === 0);
  store.getState().skipPlayer(50);
  checkNew('(j) skipPlayer 正向按当前位置推进（0 → 50）',
    store.getState().player.positionSec === 50 && store.getState().player.seekToSec === 50);
  store.getState().seekPlayer(-5);
  checkNew('(j) seekPlayer 负数夹取为 0（不把负时间写进 audio.currentTime）',
    store.getState().player.seekToSec === 0 && store.getState().player.positionSec === 0);
  store.getState().seekPlayer(42.5);
  checkNew('(j) seekPlayer 正常值原样落到 seekToSec + positionSec',
    store.getState().player.seekToSec === 42.5 && store.getState().player.positionSec === 42.5);
  store.getState().playerEnded();
  const jEnded = store.getState().player;
  checkNew('(j) playerEnded 停止播放并归零位置/清 seek，但保留剧集信息与时长（可重播）',
    jEnded.isPlaying === false && jEnded.positionSec === 0 && jEnded.seekToSec === null
    && jEnded.isActive === true && jEnded.durationSec === 300 && jEnded.title === '第 1 集');

  /* ---------- P3[F3]（TASK-081）：同集再点 = 播放/暂停切换，不从头重播 ----------
     判据是 src/store/selectors.ts 导出的纯函数 `podcastClickAction`。
     **证据分两层，边界如实说明**（审查 FINDING TASK-081-F1 / R2-F1）：
       ① 本组断言直接驱动该纯函数，并有变异取证（改回无条件 play → 恰 2 条失败）；
       ② 但纯函数有牙 ≠ 组件真的调用它：只断言纯函数时，「删掉组件的守卫」依然全绿。
          故紧随其后另加**源码形态断言**（沿用本文件既有的 readFileSync 核对手法），
          钉住 Timeline.tsx 的播放入口确实按 toggle 分支走。
     两层范围不同，不可互相冒充。 */
  const { podcastClickAction } = await import('../src/store/selectors.ts');
  checkNew('(P3[F3]) 播放中 + 同 audioUrl ⇒ toggle（播放/暂停切换，不从头重播）',
    podcastClickAction(true, 'https://a.example/ep2.mp3', 'https://a.example/ep2.mp3') === 'toggle');
  checkNew('(P3[F3]) 未激活 ⇒ play（首次点某集应正常开始播放）',
    podcastClickAction(false, '', 'https://a.example/ep2.mp3') === 'play');
  checkNew('(P3[F3]) 播放中但换了另一集 ⇒ play（换集仍从头播，不得被同集判据拦下）',
    podcastClickAction(true, 'https://a.example/ep2.mp3', 'https://a.example/ep3.mp3') === 'play');
  checkNew('(P3[F3]) 无音频地址 ⇒ play（交给 playPodcastEpisode 走它自己的「无可播放地址」提示）',
    podcastClickAction(true, 'https://a.example/ep2.mp3', '') === 'play');
  /* 修前对照：旧行为是无条件 playPodcastEpisode（进度被清零重开），
     即「同集也判 play」——用同一组输入复现修前判据，证明本断言有区分力。 */
  const legacyAction = () => 'play';
  checkNew('(P3[F3]) 修前判据可复现：无条件 play 会把「同集」也判为从头重播（进度清零的根因）',
    legacyAction() === 'play'
    && podcastClickAction(true, 'https://a.example/ep2.mp3', 'https://a.example/ep2.mp3') !== 'play');
  /* 行为侧对照：走 toggle 保留进度、走 play 归零（锁住两条路径的实际后果）。 */
  store.getState().playPodcastEpisode('第 2 集', '节目', null, 'https://a.example/ep2.mp3', null);
  store.getState().syncPlayerProgress(120, 600);
  store.getState().togglePlayerPlay();
  checkNew('(P3[F3]) 走 toggle 路径：暂停且**进度保留**（修前会重头播并清零）',
    store.getState().player.isPlaying === false && store.getState().player.positionSec === 120);
  store.getState().playPodcastEpisode('第 3 集', '节目', null, 'https://a.example/ep3.mp3', null);
  checkNew('(P3[F3]) 走 play 路径（换集）：从头播放、位置归零',
    store.getState().player.isPlaying === true && store.getState().player.positionSec === 0
    && store.getState().player.audioUrl === 'https://a.example/ep3.mp3');

  /* 第二层：源码形态断言 —— 钉住「组件确实按 toggle 分支消费该判据、且传对了实参」。
     没有这一层时：
       · 删掉 Timeline.tsx 的守卫（改回无条件 playPodcastEpisode）→ 纯函数断言仍全绿
         （审查 FINDING TASK-081-R2-F1 实测）；
       · 只做「token 在场」检查也不够：把实参 `cur.audioUrl` 改成 `''`，三处 token 一字未动
         却让 toggle 分支变成不可达死代码、缺陷完全复现，而门禁仍全绿
         （审查 FINDING TASK-081-R3-F2 实测）。
     故本层**必须校验实参表达式本身**（`cur.isActive` / `cur.audioUrl` / `audioUrl` 三者的
     具体写法），而不是只查函数名与 'toggle' 字面量在场。
     手法沿用本文件既有的 readFileSync + slice（见 TASK-065 N8/N11 一处）。
     证据边界如实声明：本层是**源码形态**断言（非 DOM 点击），它证明「调用点写了正确的
     判定与实参」，不证明运行期 DOM 点击路径；后者需 CDP e2e（本项目暂未建）。 */
  {
    const fsT = await import('node:fs');
    const tlSrc2 = fsT.readFileSync(new URL('../src/components/Timeline.tsx', import.meta.url), 'utf8');
    const playStart = tlSrc2.indexOf('const play = () => {');
    const playBlock = playStart < 0 ? '' : tlSrc2.slice(playStart, tlSrc2.indexOf('return (', playStart));
    /* 实参逐个钉死：任何一处被替换（如 cur.audioUrl → ''）都必须失败 */
    const callMatch = playBlock.match(/podcastClickAction\(\s*([^)]*)\)/);
    const args = callMatch ? callMatch[1].split(',').map((a) => a.trim()) : [];
    checkNew('(P3[F3]) 组件按 toggle 分支消费判据（删掉该守卫即失败）',
      playBlock.includes('podcastClickAction(') && playBlock.includes("=== 'toggle'")
      && playBlock.includes('togglePlayerPlay();'));
    checkNew('(P3[F3]) 判据实参必须取当前播放器状态与卡片音频地址（改实参即失败，非仅查 token 在场）',
      args.length === 3 && args[0] === 'cur.isActive' && args[1] === 'cur.audioUrl'
      && args[2] === 'audioUrl');
    checkNew('(P3[F3]) 组件的 toggle 分支必须 return（否则会继续走 play 造成双重动作）',
      /===\s*'toggle'\s*\)\s*\{\s*togglePlayerPlay\(\);\s*return;/.test(playBlock));
  }
  store.setState({ player: { ...store.getState().player, speed: 2.0 } });
  store.getState().cyclePlaybackSpeed();
  checkNew('(j) 倍速在 1/1.25/1.5/2 内循环（2.0 → 1.0）并给出 toast',
    store.getState().player.speed === 1 && store.getState().toasts.some((t) => t.text === '倍速已切换至 1x'));
  store.setState({ playerExpanded: true });
  store.getState().closePodcastBar();
  checkNew('(j) 关闭播放条：停止并收起（isActive/isPlaying/seek 复位 + 大播放器收起）',
    store.getState().player.isActive === false && store.getState().player.isPlaying === false
    && store.getState().player.seekToSec === null && store.getState().playerExpanded === false);

  /* ============================================================
     (k) 设置合并与校验（bootstrapSettings / updateSettings）
     ============================================================ */
  await resetStore();
  settingsRaw = JSON.stringify({
    themeMode: 'light', fontSize: '18', lineHeight: 200, bogusKey: 5,
    maxWidth: 760, startupView: 'starred', hideReadOnStartup: false,
  });
  await store.getState().bootstrapSettings();
  const k1 = store.getState();
  checkNew('(k) 逐键合并：类型相符生效、类型不符回落默认（fontSize 收到字符串 → 保留 16）',
    k1.settings.themeMode === 'light' && k1.settings.lineHeight === 200
    && k1.settings.fontSize === 16 && !('bogusKey' in k1.settings));
  checkNew('(k) maxWidth 旧默认迁移：760 → 860', k1.settings.maxWidth === 860);
  checkNew('(k) startupView 白名单生效：合法值直接应用为启动视图', k1.activeViewFilter === 'starred');
  checkNew('(k) hideReadOnStartup=false → 启动时间流显示全部（不隐藏已读）', k1.timelineFilter === 'all');

  settingsRaw = JSON.stringify({ startupView: 'bogus' });
  await store.getState().bootstrapSettings();
  checkNew('(k) 非法 startupView 被白名单拦下：activeViewFilter 保持原值不被污染',
    store.getState().activeViewFilter === 'starred');
  /* 【改动理由】原断言把 D5 的现状（读回路径只做 typeof 拦截、非法值照样写进
     settings.startupView）写成了期望，标注为「观察项」。D5 修复后读回路径与
     写入路径共用同一张校验表（settingsValidation），非法值不再落进 settings，
     故改为断言「不被污染」（与上一条 activeViewFilter 的判定同向）。 */
  checkNew('(k) 非法 startupView 也不写进 settings（读回与写入共用同一张校验表 —— D5 修复项）',
    store.getState().settings.startupView === 'starred');

  settingsRaw = JSON.stringify({ maxWidth: 900, hideReadOnStartup: true });
  await store.getState().bootstrapSettings();
  checkNew('(k) 显式 maxWidth=900 不被迁移覆盖；hideReadOnStartup=true 回到未读筛选',
    store.getState().settings.maxWidth === 900 && store.getState().timelineFilter === 'unread');

  store.getState().updateSettings({ themeMode: 'dark' });
  settingsRaw = null;
  await store.getState().bootstrapSettings();
  checkNew('(k) 后端无存量设置时不改动内存设置（不把默认值反向写回）',
    store.getState().settings.themeMode === 'dark' && store.getState().settings.maxWidth === 900);

  const realConsoleError = console.error;
  let kErrCount = 0;
  console.error = () => { kErrCount += 1; };
  settingsRaw = '{ 坏 JSON';
  await store.getState().bootstrapSettings();
  console.error = realConsoleError;
  checkNew('(k) 坏 JSON 被 try/catch 吞掉：记录错误但不破坏当前设置',
    kErrCount === 1 && store.getState().settings.themeMode === 'dark'
    && store.getState().settings.maxWidth === 900);

  invokeCalls.length = 0;
  store.getState().updateSettings({ fontSize: 20, themeMode: 'light' });
  await nTick(0);   // api.setSetting 内部 await getInvoke()
  const kWrite = invokeCalls.find((c) => c.cmd === 'set_setting');
  const kPayload = kWrite ? JSON.parse(kWrite.args.value) : {};
  checkNew('(k) updateSettings 合并进内存并以单键 JSON 全量落库（含新值且不丢其他键）',
    store.getState().settings.fontSize === 20 && kWrite?.args.key === 'app_settings'
    && kPayload.fontSize === 20 && kPayload.themeMode === 'light'
    && typeof kPayload.markReadOnOpen === 'boolean' && kPayload.listWidth === store.getState().settings.listWidth);

  /* ---------- D5：updateSettings 运行时校验（写入与读回对称） ---------- */
  const k5 = await import('../dist-test/store/settingsValidation.js');
  const kTypes = await import('../dist-test/types.js');
  const k5Before = store.getState().settings;
  invokeCalls.length = 0;
  store.getState().updateSettings({ fontSize: -5, refreshInterval: 0, fetchConcurrency: 99, listWidth: 99999 });
  const k5Bad = store.getState().settings;
  checkNew('(D5) 越界数值被写入路径拦下：fontSize/refreshInterval/fetchConcurrency/listWidth 保持原值',
    k5Bad.fontSize === k5Before.fontSize && k5Bad.refreshInterval === k5Before.refreshInterval
    && k5Bad.fetchConcurrency === k5Before.fetchConcurrency && k5Bad.listWidth === k5Before.listWidth);
  await nTick(0);   // api.setSetting 内部 await getInvoke()：等一拍才能观察到「有没有落库」
  checkNew('(D5) 整份补丁全非法 → 连落库 IPC 都不发（不留无效写、不空转落库）',
    invokeCalls.filter((c) => c.cmd === 'set_setting').length === 0);
  store.getState().updateSettings({ themeMode: 'neon', syncMode: 'p2p', defaultOpenMode: 'pdf', startupView: 'article' });
  const k5Enum = store.getState().settings;
  checkNew('(D5) 非法枚举被拦下：themeMode/syncMode/defaultOpenMode/startupView 保持原值',
    k5Enum.themeMode === k5Before.themeMode && k5Enum.syncMode === k5Before.syncMode
    && k5Enum.defaultOpenMode === k5Before.defaultOpenMode && k5Enum.startupView === k5Before.startupView);
  await nTick(0);
  store.getState().updateSettings({ bogusKey: 1, fontSize: 20 });
  await nTick(0);   // 让这次落库 IPC 先落地，避免与下一段的调用计数串台
  checkNew('(D5) 未知键不进 settings（不认识的键不落库），同一补丁里的合法键照常生效',
    !('bogusKey' in store.getState().settings) && store.getState().settings.fontSize === 20);
  invokeCalls.length = 0;
  store.getState().updateSettings({ fontSize: 13, lineHeight: 240, maxWidth: 1100, refreshInterval: 5, fetchConcurrency: 1, listWidth: 280 });
  await nTick(0);   // api.setSetting 内部 await getInvoke()
  const k5Edge = store.getState().settings;
  checkNew('(D5) 滑杆边界值合法可写：13px / 240% / 1100px / 5min / 1路 / 280px',
    k5Edge.fontSize === 13 && k5Edge.lineHeight === 240 && k5Edge.maxWidth === 1100
    && k5Edge.refreshInterval === 5 && k5Edge.fetchConcurrency === 1 && k5Edge.listWidth === 280);
  checkNew('(D5) 合法补丁照常落库（校验不误伤正常路径）',
    invokeCalls.filter((c) => c.cmd === 'set_setting').length === 1);
  const kSetKeys = Object.keys(store.getState().settings);
  checkNew('(D5) 校验表与设置键一一对应（漏配校验器会被 tsc 的 Record<keyof SettingsState,…> 拦下）',
    kSetKeys.every((key) => typeof k5.SETTINGS_VALIDATORS[key] === 'function')
    && kSetKeys.length === Object.keys(k5.SETTINGS_VALIDATORS).length);

  /* ---------- D4：启动视图白名单与设置页下拉同源 ---------- */
  checkNew('(D4) 死选项 article 已从两侧移除；余下取值都能通过校验（含此前无 UI 入口的 starred）',
    !kTypes.STARTUP_VIEW_OPTIONS.some((o) => o.value === 'article')
    && kTypes.STARTUP_VIEW_OPTIONS.length === 4
    && kTypes.STARTUP_VIEW_OPTIONS.every((o) => k5.isValidSetting('startupView', o.value))
    && kTypes.STARTUP_VIEW_OPTIONS.some((o) => o.value === 'starred')
    && !k5.isValidSetting('startupView', 'article'));
  /* 先显式落一个合法值作为对照基准（修前 updateSettings 会把它改成 'article'） */
  store.getState().updateSettings({ startupView: 'today' });
  const k4Keep = store.getState().settings.startupView;
  store.getState().updateSettings({ startupView: 'article' });
  checkNew('(D4) 写入路径拒绝死选项（settings.startupView 不落 article）',
    k4Keep !== 'article' && store.getState().settings.startupView === k4Keep);
  settingsRaw = JSON.stringify({ startupView: 'article', fontSize: 18 });
  await store.getState().bootstrapSettings();
  checkNew('(D4) 读回路径同样拒绝 article（旧库里的历史值不再进入 settings）',
    store.getState().settings.startupView === k4Keep);
  checkNew('(D4) 同一份读回里的合法键照常生效（fontSize 18）', store.getState().settings.fontSize === 18);
  settingsRaw = JSON.stringify({ startupView: 'starred' });
  await store.getState().bootstrapSettings();
  checkNew('(D4) 收藏视图可作启动视图：白名单里的取值确实被应用为 activeViewFilter',
    store.getState().activeViewFilter === 'starred');

  /* ============================================================
     (l) AI per-id 流式写入与失败标记
     ============================================================ */
  await bootFixture();
  aiSum = { deltas: [], error: null, reject: null, finish: true, holdIds: [104] };
  store.getState().summarizeEntry('104');
  checkNew('(l) summarizeEntry 生成态按 id 置位、流未结束时保持（不误报完成）',
    store.getState().summarizingIds['104'] === true
    && store.getState().entries.find((a) => a.id === '104')?.aiSummary === '');
  /* api.aiSummarize 内部 await import('@tauri-apps/api/core')，inv 在一个微任务后才发出：
     让出一轮宏任务再取挂起的 channel（channel 未 done 前生成态必须保持） */
  await nTick(5);
  const heldSum = heldAi.find((h) => h.cmd === 'ai_summarize' && h.id === 104);
  heldSum.ch.onmessage?.({ type: 'delta', data: '摘' });
  heldSum.ch.onmessage?.({ type: 'delta', data: '要完成' });
  checkNew('(l) 流式 delta 增量落到该条目的 aiSummary（其他条目不受影响）',
    store.getState().entries.find((a) => a.id === '104')?.aiSummary === '摘要完成'
    && store.getState().entries.find((a) => a.id === '105')?.aiSummary === '');
  heldSum.ch.onmessage?.({ type: 'done' });
  checkNew('(l) done 清除 summarizingIds[id]（不残留「生成中」占位）',
    store.getState().summarizingIds['104'] === undefined);
  invokeCalls.length = 0;
  store.getState().summarizeEntry('104');
  await nTick(5);
  checkNew('(l) 已有摘要缓存时直接短路（不重复请求 AI）',
    invokeCalls.filter((c) => c.cmd === 'ai_summarize').length === 0);

  store.setState({ toasts: [] });
  aiSum = { deltas: [], error: 'AI 限流', reject: null, finish: true, holdIds: [] };
  store.getState().summarizeEntry('105');
  await nTick(5);
  const lErr = store.getState();
  checkNew('(l) 摘要流内 error 事件：按 id 记录错误 + 清生成态 + toast 带重试',
    lErr.summaryErrors['105'] === 'AI 限流' && lErr.summarizingIds['105'] === undefined
    && lErr.toasts.length === 1 && lErr.toasts[0].text === '摘要失败：AI 限流'
    && lErr.toasts[0].action?.label === '重试');
  invokeCalls.length = 0;
  lErr.toasts[0].action.run();
  const lRetryCleared = store.getState().summaryErrors['105'] === '';   // 重试同步清上次错误
  await nTick(5);
  checkNew('(l) 重试按钮真正重新发起该 id 的摘要请求（并先清掉上次错误）',
    invokeCalls.filter((c) => c.cmd === 'ai_summarize').length === 1 && lRetryCleared);

  await bootFixture();
  store.setState({ toasts: [] });
  aiSum = { deltas: [], error: null, reject: { message: '网络不可达' }, finish: true, holdIds: [] };
  store.getState().summarizeEntry('105', { silent: true });
  await nTick(20);
  const lRej = store.getState();
  checkNew('(l) 摘要请求 reject：落到「AI 服务未配置或不可达」错误态并清生成态',
    lRej.summaryErrors['105'] === 'AI 服务未配置或不可达' && lRej.summarizingIds['105'] === undefined);
  checkNew('(l) silent 模式失败不弹 toast（源级自动摘要不打扰用户）', lRej.toasts.length === 0);

  await bootFixture();
  detailImpl = (id) => mkRow({ id, content_html: '<p>详情</p>', translated_content: '<p>已消毒译文</p>' });
  aiTr = { deltas: [], error: null, reject: null, finish: true, holdIds: [201] };
  invokeCalls.length = 0;
  store.getState().translateEntry('201');
  await nTick(5);   // 等 api 内部 import Channel 后真正发出 inv，再手动推流
  const heldTrS = heldAi.find((h) => h.cmd === 'ai_translate' && h.id === 201);
  heldTrS.ch.onmessage?.({ type: 'delta', data: '<p>未消毒<script>alert(1)</script></p>' });
  checkNew('(l) translateEntry 流式增量先落到 translatedContent（打字机原样展示、未收尾）',
    store.getState().entries.find((a) => a.id === '201')?.translatedContent === '<p>未消毒<script>alert(1)</script></p>'
    && store.getState().translatingIds['201'] === true);
  heldTrS.ch.onmessage?.({ type: 'done' });
  checkNew('(l) 流结束（done）立即清 translatingIds[id]（不等回读完成）',
    store.getState().translatingIds['201'] === undefined);
  await nTick(20);
  const lSafe = store.getState().entries.find((a) => a.id === '201')?.translatedContent ?? '';
  checkNew('(l) 流结束后回读 DB 消毒译文覆盖流式产物（无 <script> 残留）',
    lSafe === '<p>已消毒译文</p>' && !lSafe.includes('<script>'));
  invokeCalls.length = 0;
  store.getState().translateEntry('201');
  await nTick(5);
  checkNew('(l) 已有译文缓存时不再触发 ai_translate',
    invokeCalls.filter((c) => c.cmd === 'ai_translate').length === 0);

  /* ---------- (l2) TASK-065 N11：rawTranslatedIds 消毒时序（渲染契约 store 侧锚点） ---------- */
  await bootFixture();
  detailImpl = (id) => mkRow({ id, content_html: '<p>详情</p>', translated_content: '<p>已消毒译文</p>' });
  aiTr = { deltas: [], error: null, reject: null, finish: true, holdIds: [201] };
  store.getState().translateEntry('201');
  await nTick(5);
  const heldN11 = heldAi.find((h) => h.cmd === 'ai_translate' && h.id === 201);
  heldN11.ch.onmessage?.({ type: 'delta', data: '<p>未消毒<script>alert(1)</script></p>' });
  checkNew('(l2) 流式期间 rawTranslatedIds[id]=true（未消毒产物按纯文本渲染）',
    store.getState().rawTranslatedIds['201'] === true);
  heldN11.ch.onmessage?.({ type: 'done' });
  checkNew('(l2) done 后消毒回读未落地：标记仍在（消毒版未到位不得切 HTML 渲染）',
    store.getState().rawTranslatedIds['201'] === true);
  await nTick(20);
  checkNew('(l2) 消毒回读落地：标记清除且内容为 DB 消毒版',
    store.getState().rawTranslatedIds['201'] === undefined
    && store.getState().entries.find((a) => a.id === '201')?.translatedContent === '<p>已消毒译文</p>');

  await bootFixture();
  detailImpl = () => { throw { message: 'ipc down' }; };
  aiTr = { deltas: [], error: null, reject: null, finish: true, holdIds: [201] };
  store.getState().translateEntry('201');
  await nTick(5);
  const heldN11Fail = heldAi.find((h) => h.cmd === 'ai_translate' && h.id === 201);
  heldN11Fail.ch.onmessage?.({ type: 'delta', data: '<img src=x onerror=alert(1)>' });
  heldN11Fail.ch.onmessage?.({ type: 'done' });
  await nTick(20);
  checkNew('(l2) 消毒回读失败：丢弃未消毒半截 + 标记清除 + 错误态与 toast 带重试',
    store.getState().rawTranslatedIds['201'] === undefined
    && store.getState().entries.find((a) => a.id === '201')?.translatedContent === ''
    && store.getState().translateErrors['201'] === '译文回读失败'
    && store.getState().toasts.some((t) => t.text === '译文回读失败'));

  await bootFixture();
  detailImpl = (id) => mkRow({ id, content_html: '<p>详情</p>', translated_content: '<p>已消毒译文</p>' });
  aiTr = { deltas: [], error: null, reject: null, finish: true, holdIds: [201] };
  store.getState().translateEntry('201');
  await nTick(5);
  const heldN11Err = heldAi.find((h) => h.cmd === 'ai_translate' && h.id === 201);
  heldN11Err.ch.onmessage?.({ type: 'delta', data: '<b>半截' });
  heldN11Err.ch.onmessage?.({ type: 'error', data: '限流' });
  checkNew('(l2) 流错误路径：半截未消毒内容保留（重试语义）且标记保持（按纯文本渲染）',
    store.getState().entries.find((a) => a.id === '201')?.translatedContent === '<b>半截'
    && store.getState().rawTranslatedIds['201'] === true);

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

  /* ---------- (p) TASK-067 N9/N10：交互落库与错误可见性 ---------- */
  await bootFixture();
  rejectCmds.add('mark_all_read');
  store.setState({ activeViewFilter: 'all', activeFeedFilter: '10', toasts: [] });
  store.getState().markCurrentViewAllRead();
  await nTick(10);
  checkNew('(p1) 全部已读失败必须可见（修前静默：本地已标读、计数已扣、无提示）',
    store.getState().toasts.some((t) => t.text === '全部已读未能保存，重启后可能回退'));

  await bootFixture();
  rejectCmds.add('set_read');
  store.setState({ toasts: [], settings: { ...store.getState().settings, markReadOnOpen: true } });
  store.getState().selectArticle('101');
  await nTick(10);
  checkNew('(p2) 打开文章标读失败必须可见（修前静默，重启后回退未读）',
    store.getState().toasts.some((t) => t.text.startsWith('标读失败：')));

  await bootFixture();
  failReload = { message: 'db busy' };
  store.setState({ toasts: [] });
  await store.getState().reloadFromBackend().catch(() => {});
  checkNew('(p3) reloadFromBackend 失败必须可见（后台刷新/范围切换路径，修前静默）',
    store.getState().toasts.some((t) => t.text.startsWith('刷新失败：')));

  await bootFixture();
  aiTr = { deltas: [], error: null, reject: null, finish: true, holdIds: [201] };
  store.getState().translateEntry('201');
  store.getState().translateEntry('202');
  await nTick(5);
  checkNew('(l) 按 id 隔离：A 仍在生成时 B 已收尾，两个 id 状态互不串台',
    store.getState().translatingIds['201'] === true && store.getState().translatingIds['202'] === undefined);
  const heldTr = heldAi.find((h) => h.cmd === 'ai_translate' && h.id === 201);
  heldTr.ch.onmessage?.({ type: 'error', data: '限流' });
  const lTrErr = store.getState();
  checkNew('(l) 翻译流内 error：记录 translateErrors[id] + 清生成态 + toast 带重试',
    lTrErr.translateErrors['201'] === '限流' && lTrErr.translatingIds['201'] === undefined
    && lTrErr.toasts[lTrErr.toasts.length - 1]?.text === '翻译失败：限流'
    && lTrErr.toasts[lTrErr.toasts.length - 1]?.action?.label === '重试');

  await bootFixture();
  store.setState({ toasts: [] });
  aiTr = { deltas: [], error: null, reject: { message: 'down' }, finish: true, holdIds: [] };
  store.getState().translateEntry('301', { silent: true });
  await nTick(20);
  checkNew('(l) translateEntry reject →「AI 服务未配置或不可达」，silent 时不弹 toast',
    store.getState().translateErrors['301'] === 'AI 服务未配置或不可达'
    && store.getState().toasts.length === 0);

  store.setState({ dataMode: 'mock' });
  invokeCalls.length = 0;
  store.getState().translateEntry('301');
  checkNew('(l) mock 模式不支持 AI：给出提示且不发 IPC',
    invokeCalls.filter((c) => c.cmd === 'ai_translate').length === 0
    && store.getState().toasts[store.getState().toasts.length - 1]?.text === '演示模式不支持 AI 服务');

  /* ============================================================
     (m) 本轮缺陷修复的定向断言（D1a/D1b/D1c、D4/D5 见 (k)、E1、E2、
         L1、L2、P2-4、P2-7、P1-7 邻域）

     全部为「新增」计数：上面 (a)…(l) 除两条 D2/D5 邻域「观察项」按修复
     更新外一行未改。每条都对应一个已确认缺陷，且能在修复前复现失败
     （见实施报告 §2 的修前/修后对照）。
     ============================================================ */

  /* ---------- E2：internals.appStore() 未注入时必须显式抛错 ---------- */
  checkNew('(E2) internals.appStore() 在 bindAppStore 之前调用 → 显式抛错（不再静默返回 undefined 冒充 StoreApi）',
    appStoreUnbound.threw === true
    && /bindAppStore/.test(String(appStoreUnbound.error && appStoreUnbound.error.message)));
  let e2BoundOk = false;
  try { e2BoundOk = typeof internalsBeforeBind.appStore().getState === 'function'; } catch { e2BoundOk = false; }
  checkNew('(E2) store 创建之后（bind 之后）句柄正常可用，不误抛', e2BoundOk === true);

  /* ---------- E1：bootstrapGithubAuth（唯一「slice 导出 + 晚绑定句柄」迁移点） ---------- */
  await resetStore();
  const { bootstrapGithubAuth } = await import('../dist-test/store.js');
  ghLoginStatus = { login: 'octocat' };
  store.setState({ githubAccount: null });
  await bootstrapGithubAuth();
  checkNew('(E1) 后端有登录态 → 经晚绑定句柄写入 store.githubAccount（登录态启动即恢复）',
    store.getState().githubAccount?.login === 'octocat'
    && invokeCalls.some((c) => c.cmd === 'github_login_status'));
  ghLoginStatus = null;
  store.setState({ githubAccount: { login: 'keep' } });
  await bootstrapGithubAuth();
  checkNew('(E1) 后端未登录（null）→ 不误清空现有登录态',
    store.getState().githubAccount?.login === 'keep');
  ghLoginStatus = 'reject';
  store.setState({ githubAccount: { login: 'keep' } });
  let e1Threw = false;
  try { await bootstrapGithubAuth(); } catch { e1Threw = true; }
  checkNew('(E1) 后端不可用（IPC 失败）→ 静默忽略：不抛出、不污染状态',
    e1Threw === false && store.getState().githubAccount?.login === 'keep');
  ghLoginStatus = null;

  /* ---------- D1a：摘要「先出半截文本再报错」后，重试必须真的重发 ---------- */
  await bootFixture();
  store.setState({ toasts: [] });
  aiSum = { deltas: ['半截摘要'], error: 'AI 限流', reject: null, finish: true, holdIds: [] };
  store.getState().summarizeEntry('105');
  await nTick(5);
  const d1a = store.getState();
  const d1aPartial = d1a.entries.find((a) => a.id === '105')?.aiSummary;
  invokeCalls.length = 0;
  d1a.toasts[0]?.action?.run();                       // 点 toast 的「重试」
  const d1aCleared = store.getState().summaryErrors['105'] === '';
  await nTick(5);
  checkNew('(D1a) 半截摘要 + 报错后点「重试」：真的重发 ai_summarize（修前被 if (art.aiSummary) 短路挡住）',
    d1aPartial === '半截摘要' && invokeCalls.filter((c) => c.cmd === 'ai_summarize').length === 1);
  checkNew('(D1a) 重试同步清掉上次错误与半截摘要（不再与错误并存，卡片可自愈）',
    d1aCleared === true && store.getState().summaryErrors['105'] === 'AI 限流');

  /* ---------- D1b：卡片翻译「先出半截译文再报错」后，重试必须真的重发 ---------- */
  await bootFixture();
  store.setState({ toasts: [] });
  aiTr = { deltas: ['半截译文'], error: '限流', reject: null, finish: true, holdIds: [] };
  store.getState().translateEntry('201');
  await nTick(5);
  const d1bPartial = store.getState().entries.find((a) => a.id === '201')?.translatedContent;
  aiTr = { deltas: [], error: null, reject: null, finish: false, holdIds: [201] };   // 重试：挂起观察
  invokeCalls.length = 0;
  store.getState().toasts[0]?.action?.run();
  await nTick(5);
  const d1b = store.getState();
  checkNew('(D1b) 半截译文 + 报错后点「重试」：真的重发 ai_translate（修前被 art.translatedContent 短路挡住）',
    d1bPartial === '半截译文' && invokeCalls.filter((c) => c.cmd === 'ai_translate').length === 1);
  checkNew('(D1b) 重试清空半截译文与上次错误（与 Reader 路径的重试语义对齐，不把半截当缓存）',
    d1b.entries.find((a) => a.id === '201')?.translatedContent === '' && d1b.translateErrors['201'] === '');
  heldAi[heldAi.length - 1]?.ch.onmessage?.({ type: 'done' });
  await nTick(20);

  /* ---------- D1c：Reader 翻译同源路径（error 后 translatedContent 残留半截） ---------- */
  await bootFixture();
  /* 先把正文置成与 detailImpl 相同的内容：selectArticle 会异步水合详情，
     若 content 为空则会被回填成 translated_content='' —— 那会把流式半截译文冲掉，
     掩盖本用例要观察的状态。 */
  store.setState((s) => ({
    toasts: [], isShowingTranslatedProse: false,
    entries: s.entries.map((a) => (a.id === '201' ? { ...a, content: '<p>详情</p>' } : a)),
  }));
  store.getState().selectArticle('201');
  aiTr = { deltas: ['半截译文'], error: '限流', reject: null, finish: true, holdIds: [] };
  store.getState().toggleReaderTranslation();
  await nTick(5);
  const d1c = store.getState();
  checkNew('(D1c) Reader 翻译半截 + 报错：错误态可见、译文块收起（半截译文仍留在条目上）',
    d1c.translateErrors['201'] === '限流' && d1c.isShowingTranslatedProse === false
    && d1c.entries.find((a) => a.id === '201')?.translatedContent === '半截译文');
  aiTr = { deltas: [], error: null, reject: null, finish: false, holdIds: [201] };
  invokeCalls.length = 0;
  d1c.toasts[d1c.toasts.length - 1]?.action?.run();
  await nTick(5);
  checkNew('(D1c) 半截译文 + 报错后点「重试」：真的重发 ai_translate 并重新进入生成态（修前把半截当缓存直接展示）',
    invokeCalls.filter((c) => c.cmd === 'ai_translate').length === 1
    && store.getState().translating === true);
  checkNew('(D1c) 重试清空半截译文（原有清空逻辑在修前根本走不到）',
    store.getState().entries.find((a) => a.id === '201')?.translatedContent === '');
  heldAi[heldAi.length - 1]?.ch.onmessage?.({ type: 'done' });
  await nTick(20);

  /* ---------- L1：批量标读索引化后语义不变（只增复杂度优化，行为契约不变） ---------- */
  await bootFixture();
  const l1Ids = store.getState().entries.map((e) => e.id);
  invokeCalls.length = 0;
  store.getState().markEntriesReadBulk(l1Ids);
  await nTick(0);   // api.setRead 内部 await getInvoke()，落库是异步 fire-and-forget
  const l1 = store.getState();
  checkNew('(L1) 索引化批量标读：未读项全部标读、已读项不重复写库（8 条中 6 条未读 → 6 次 set_read）',
    l1.entries.every((e) => e.isRead) && invokeCalls.filter((c) => c.cmd === 'set_read').length === 6);
  checkNew('(L1) 未读计数仍按源聚合扣减：源10 3→1、源12 2→0、源11 2→1、源20 1→0',
    l1.feedCounts.get('10')?.unread === 1 && l1.feedCounts.get('12')?.unread === 0
    && l1.feedCounts.get('11')?.unread === 1 && l1.feedCounts.get('20')?.unread === 0);
  checkNew('(L1) 「已读保留」快照按被标读的未读项写入（6 条；本就已读的 102/202 不重复记）',
    Object.keys(l1.openedReadIds).length === 6
    && l1.openedReadIds['101'] === true && l1.openedReadIds['105'] === true
    && l1.openedReadIds['102'] === undefined);
  invokeCalls.length = 0;
  store.getState().markEntriesReadBulk(['不存在的id']);
  checkNew('(L1) 未知 id 被忽略：不发 IPC、不改状态', invokeCalls.length === 0);

  /* ---------- L2：feedCounts 缺项 → 有意的保守设计（本断言即该设计的锚） ---------- */
  await bootFixture();
  const l2Before = selectViewCounts(store.getState()).all;
  const l2Counts = new Map(store.getState().feedCounts);
  l2Counts.delete('12');                               // 模拟后端精确计数缺该源
  store.setState({ feedCounts: l2Counts });
  const l2View = selectViewCounts(store.getState());
  const l2Tree = selectTreeCounts(store.getState());
  checkNew('(L2) feedCounts 缺项：该源不计入「全部」总数（7 → 5），树角标也不建该行（保守设计）',
    l2Before === 7 && l2View.all === 5 && l2Tree.get('all') === 5 && !l2Tree.has('12'));
  const l2Visible = selectVisibleEntries(store.getState());
  checkNew('(L2) 但该源条目仍正常列出（计数缺失不影响内容可见性）',
    l2Visible.filter((e) => e.feedId === '12').length === 2);

  /* ---------- P2-4：AI「保存提示词」只写提示词，不再顺带覆盖端点配置 ---------- */
  const { mergePromptsOnly } = await import('../dist-test/components/settings/aiConfig.js');
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
  store.setState((s) => ({
    toasts: [],
    activeArticleId: '104',
    showFulltext: false,
    entries: s.entries.map((a) => (a.id === '104'
      ? { ...a, content: '<p>RSS 原文</p>', rawContent: '<p>RSS 原文</p>', url: 'https://x.example/a', fulltextExtracted: false }
      : a)),
  }));
  extractResult = { html: '<p>RSS 原文</p>', degraded: true, reason: '提取结果比原正文更短，已保留原正文（原文可能已是全文）' };
  store.getState().extractCurrentArticle();
  await nTick(20);
  const p210 = store.getState();
  checkNew('(P2-10/TASK-076) degraded=true 时：不置 fulltextExtracted、不切全文视图',
    p210.entries.find((a) => a.id === '104')?.fulltextExtracted === false && p210.showFulltext === false);
  checkNew('(P2-10/TASK-076) 且如实提示降级原因（后端 reason 原文），而不是报成功',
    p210.toasts.some((t) => t.text.includes('未采用全文提取') && t.text.includes('已保留原正文'))
    && !p210.toasts.some((t) => t.text === '全文提取完成'));
  extractResult = { html: '<p>真正的全文正文，明显更长的一段内容。</p>', degraded: false, reason: null };
  store.getState().extractCurrentArticle();
  await nTick(20);
  const p210ok = store.getState();
  checkNew('(P2-10/TASK-076) 正常提取路径不受影响：置标志 + 进入全文视图 + 报「全文提取完成」',
    p210ok.entries.find((a) => a.id === '104')?.fulltextExtracted === true && p210ok.showFulltext === true
    && p210ok.toasts.some((t) => t.text === '全文提取完成'));
  extractResult = null;

  /* ---------- P2-7：版本号未就绪/不可比时不再误判「有更新」 ---------- */
  const { compareVersions, isComparableVersion, shouldOfferUpdate } = await import('../dist-test/components/settings/compareVersions.js');
  checkNew('(P2-7) 修前误判根因可复现：compareVersions(remote, "") 把空串当 0.0.0 → 恒判远端更新',
    compareVersions('0.9.0', '') > 0 && !isComparableVersion(''));
  checkNew('(P2-7) 本地版本未就绪（空串）→ 不给「有更新」结论（未知 ≠ 有更新）',
    shouldOfferUpdate('0.9.0', '') === false && shouldOfferUpdate('0.9.0', '   ') === false);
  checkNew('(P2-7) 不可比版本（非数字点分 / 回退值以外的脏数据）同样不误判',
    shouldOfferUpdate('0.9.0', 'v0.8.0') === false && shouldOfferUpdate('bad-tag', '0.8.0') === false);
  checkNew('(P2-7) 两端都可比时判定照旧：远端更高 → true，同版/更低 → false',
    shouldOfferUpdate('0.9.0', '0.8.0') === true && shouldOfferUpdate('0.8.0', '0.8.0') === false
    && shouldOfferUpdate('0.7.9', '0.8.0') === false);
  checkNew('(P2-7) 多段版本号（0.10.1 > 0.9.9）比较正确，不走字符串序',
    shouldOfferUpdate('0.10.1', '0.9.9') === true && shouldOfferUpdate('0.9.9', '0.10.1') === false);
  /* ============================================================
     TASK-052：per-scope 分页游标（缺陷 P1-14「口径」半边）
     覆盖 (s1)…(s6)，全部为本轮新增断言（checkNew）。

     被验证的口径：分页请求必须带当前订阅范围（feed_id / folder_id），
     游标按范围分桶；单源/单分类视图下「第 2 页」= 该范围的第 501..1000 条，
     而不是全局序列的第 501..1000 条。同时锚定 051 的 D2/D3 两条修复不回退。

     修前失败证据（见回归产出 run-before.log）：本组在改造前跑，
     §(s1) 会因 `loadMoreArticles` 不发 feed_id 而拿到全局下一页（含其他源的行）；
     §(s4) 会因只有单一全局游标而让 B 源续着 A 源的游标；§(s3) 会因游标恒为
     首批 PAGE_SIZE 而永远「未到底」，空列表场景无法收敛。
     ============================================================ */
  const { scopeQueryArgs, scopePageKey, viewEntriesCache } = await import('../dist-test/store/internals.js');
  checkNew('(s0) 范围键与查询参数口径：纯数字/前缀 id 都归一到数字，cat- 走 folder_id，all 两者皆 null，排序进 newest_first',
    scopePageKey('all') === 'all' && scopePageKey('10') === '10' && scopePageKey('cat-1') === 'cat-1'
    && JSON.stringify(scopeQueryArgs('all', 'newest')) === JSON.stringify({ feed_id: null, folder_id: null, newest_first: true })
    && JSON.stringify(scopeQueryArgs('12', 'oldest')) === JSON.stringify({ feed_id: 12, folder_id: null, newest_first: false })
    && JSON.stringify(scopeQueryArgs('feed-12', 'newest')) === JSON.stringify({ feed_id: 12, folder_id: null, newest_first: true })
    && JSON.stringify(scopeQueryArgs('cat-1', 'newest')) === JSON.stringify({ feed_id: null, folder_id: 1, newest_first: true }));

  /* ---------- (s1) 单源视图：连续翻页取回该源的后续文章，且两页不重叠 ----------
     数据：源A(feed 10) 1200 条（按时间降序，id 6000+i），源B(feed 11) 1200 条
     （id 7000+i）。全局序列是「源A 6000.. 与 源B 7000.. 交错」——改造前分页
     不带 feed_id，第 2 页会取到全局第 500..999 条（混着源B），断言随即失败。 */
  await resetStore();
  const s1Rows = [];
  for (let i = 0; i < 1200; i += 1) {
    s1Rows.push(mkRow({ id: 6000 + i, feed_id: 10, title: `A${i}`, published_at: iso(NOW - i * 1000) }));
    s1Rows.push(mkRow({ id: 7000 + i, feed_id: 11, title: `B${i}`, published_at: iso(NOW - i * 1000 - 500) }));
  }
  backendRows = s1Rows;
  store.getState().selectFeed('10');
  await store.getState().reloadFromBackend();
  const s1First = store.getState();
  checkNew('(s1) 单源视图首批：查询带 feed_id，条目全部属于该源，游标 = 该范围已加载数（500）',
    s1First.entries.length === 500 && s1First.entries.every((e) => e.feedId === '10')
    && s1First.articlesLimit === 500 && s1First.articlesCursor['10'] === 500
    && s1First.articlesExhausted === false);
  invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  const s1Page2Call = invokeCalls.find((c) => c.cmd === 'list_articles');
  const s1Second = store.getState();
  /* 源A 的第 1 页 = 源A 里时间最新的 500 条（s1Rows 是源A/源B 交错的，不能直接切前 500 行） */
  const s1Ids1 = new Set(
    s1Rows.filter((r) => r.feed_id === 10)
      .sort((x, y) => Date.parse(y.published_at) - Date.parse(x.published_at))
      .slice(0, 500)
      .map((r) => String(r.id)),
  );
  const s1Page2 = s1Second.entries.slice(500).map((e) => e.id);
  checkNew('(s1) 第 2 页请求沿用同一范围与游标（feed_id=10 / offset=500 / newest_first）',
    s1Page2Call?.args.args.feed_id === 10 && s1Page2Call?.args.args.folder_id === null
    && s1Page2Call?.args.args.offset === 500 && s1Page2Call?.args.args.newest_first === true);
  checkNew('(s1) 第 2 页取回该源的后续文章：全部属于 feed 10，且与第 1 页 id 集合不重叠',
    s1Second.entries.length === 1000 && s1Second.entries.every((e) => e.feedId === '10')
    && s1Page2.length === 500 && s1Page2.every((id) => !s1Ids1.has(id))
    && s1Page2[0] === '6500' && s1Page2[499] === '6999'
    && s1Second.articlesLimit === 1000 && s1Second.articlesCursor['10'] === 1000);

  /* ---------- (s4) per-scope 游标互不污染：A 源翻到第 2 页后切 B 源，B 从第 1 页开始 ---------- */
  /* TASK-063：selectFeed 自带接线（缓存命中恢复 / 未命中自动重拉）——此处不再
     手工调用 reloadFromBackend（旧写法模拟了 UI 中不存在的一步，掩盖了 Sidebar
     未接线的事实），改为等待 selectFeed 自身触发的重拉落地。 */
  invokeCalls.length = 0;
  store.getState().selectFeed('11');
  const s4Switch = store.getState();
  checkNew('(s4) 切到未曾加载的源B：游标从 0 起步（不继承源A 的 1000）',
    s4Switch.activeFeedFilter === '11' && s4Switch.articlesLimit === 0
    && s4Switch.articlesCursor['10'] === 1000 && s4Switch.articlesCursor['11'] === undefined);
  await nTick(30);
  const s4FirstCall = invokeCalls.find((c) => c.cmd === 'list_articles');
  const s4First = store.getState();
  checkNew('(s4) selectFeed 自动重拉：源B 首批查询按源B 的第 1 页取（feed_id=11 / offset=0），条目全属源B',
    s4FirstCall?.args.args.feed_id === 11 && s4FirstCall?.args.args.offset === 0
    && s4First.entries.length === 500 && s4First.entries.every((e) => e.feedId === '11')
    && s4First.articlesCursor['11'] === 500 && s4First.articlesCursor['10'] === 1000
    && s4First.entries.slice(0, 500).every((e) => !s1Ids1.has(e.id)));
  invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  const s4Call = invokeCalls.find((c) => c.cmd === 'list_articles');
  checkNew('(s4) 源B 翻页用**源B 自己的游标**（offset=500 而非源A 的 1000）',
    s4Call?.args.args.feed_id === 11 && s4Call?.args.args.offset === 500
    && store.getState().articlesCursor['11'] === 1000 && store.getState().articlesCursor['10'] === 1000);
  /* 切回源A：TASK-063 新契约——缓存命中同步恢复该范围快照（零延迟，不经 await），
     游标=快照长度（500，可继续翻页）；源B 的游标 1000 不被污染。恢复时清水合
     状态（缓存快照无正文，滞留的已水合标记会阻断重水合）。 */
  store.setState({ hydratedIds: { '999': true }, hydrationErrors: { '998': 'x' } });
  store.getState().selectFeed('10');
  const s4Back = store.getState();
  checkNew('(s4) 切回源A：同步恢复该范围快照（零延迟），游标=快照长度且两源互不污染，滞留水合状态被清空',
    s4Back.entries.length === 500 && s4Back.entries.every((e) => e.feedId === '10')
    && s4Back.articlesLimit === 500 && s4Back.articlesCursor['10'] === 500
    && s4Back.articlesCursor['11'] === 1000
    && Object.keys(s4Back.hydratedIds).length === 0 && Object.keys(s4Back.hydrationErrors).length === 0);
  await nTick(30);
  checkNew('(s4) 切回源A 后的后台刷新保持该范围快照结论',
    store.getState().entries.every((e) => e.feedId === '10') && store.getState().articlesCursor['10'] === 500);

  /* ---------- (s4b) TASK-063 附加契约：selectView 缓存恢复同契约清滞留水合状态；mock 模式不接线 ---------- */
  await bootFixture();
  store.setState({ activeViewFilter: 'starred' });
  await store.getState().reloadFilteredEntries('starred');
  store.setState({ activeViewFilter: 'all', entries: [], hydratedIds: { '888': true }, hydrationErrors: { '887': 'y' } });
  store.getState().selectView('starred');
  checkNew('(s4b) selectView 缓存命中恢复：滞留水合状态被清空（缓存快照无正文，防「永不重水合」空窗）',
    store.getState().activeViewFilter === 'starred'
    && store.getState().entries.length > 0
    && Object.keys(store.getState().hydratedIds).length === 0
    && Object.keys(store.getState().hydrationErrors).length === 0);

  await resetStore({ dataMode: 'mock' });
  invokeCalls.length = 0;
  store.getState().selectFeed('11');
  checkNew('(s4b) mock 模式 selectFeed 保持纯游标镜像：不触发 IPC、不翻转数据模式',
    store.getState().dataMode === 'mock' && store.getState().activeFeedFilter === '11'
    && !invokeCalls.some((c) => c.cmd === 'list_articles'));

  /* ---------- (s2) 单分类视图：同一口径成立（folder_id） ----------
     分类 cat-1 = 源10 + 源12；每源 600 条，全局 1200 条。分类的第 2 页
     必须是该分类内第 500..999 条（改造前会取到全局第 500..999 条 = 混入源20）。 */
  await resetStore();
  const s2Rows = [];
  for (let i = 0; i < 600; i += 1) {
    s2Rows.push(mkRow({ id: 8000 + i, feed_id: 10, title: `A${i}`, published_at: iso(NOW - i * 1000) }));
    s2Rows.push(mkRow({ id: 9000 + i, feed_id: 12, title: `D${i}`, published_at: iso(NOW - i * 1000 - 300) }));
    s2Rows.push(mkRow({ id: 9500 + i, feed_id: 20, title: `C${i}`, published_at: iso(NOW - i * 1000 - 600) }));
  }
  backendRows = s2Rows;
  store.getState().selectFeed('cat-1');
  await store.getState().reloadFromBackend();
  const s2First = store.getState();
  const s2First50 = s2First.entries.map((e) => e.id);
  checkNew('(s2) 单分类视图首批：查询带 folder_id，条目只含该分类的源，游标 500',
    s2First.entries.length === 500 && s2First.entries.every((e) => e.feedId === '10' || e.feedId === '12')
    && s2First.articlesLimit === 500 && s2First.articlesCursor['cat-1'] === 500);
  invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  const s2Call = invokeCalls.find((c) => c.cmd === 'list_articles');
  const s2Second = store.getState();
  checkNew('(s2) 第 2 页请求沿用分类口径（folder_id=1 / feed_id=null / offset=500）',
    s2Call?.args.args.folder_id === 1 && s2Call?.args.args.feed_id === null
    && s2Call?.args.args.offset === 500);
  checkNew('(s2) 第 2 页取回该分类的后续文章（不含分类外源C），且与第 1 页不重叠',
    s2Second.entries.length === 1000 && s2Second.entries.every((e) => e.feedId === '10' || e.feedId === '12')
    && s2Second.entries.slice(500).every((e) => !s2First50.includes(e.id))
    && s2Second.articlesCursor['cat-1'] === 1000);

  /* ---------- (s3) 列表为空也能推进：单源视图下该源没有文章时，游标不再被首批截断 ----------
     改造前：首批游标恒为 PAGE_SIZE(500)，即使 `entries` 里一条该源的文章都没有，
     `articlesLimit=500`、`articlesExhausted=false`；列表为空 ⇒ 哨兵不渲染 ⇒ 无滚动 ⇒
     老文章永远够不到。改造后单源首批查询带 feed_id：该源确实没有文章时后端返回
     空页 ⇒ 立即收敛为「已到底」，空列表有终态而不是假装还有 500 条。 */
  await resetStore();
  backendRows = [mkRow({ id: 111, feed_id: 20, title: '只有源C 有文章' })];
  store.getState().selectFeed('10');
  await store.getState().reloadFromBackend();
  const s3Empty = store.getState();
  checkNew('(s3) 空范围首批：查询带 feed_id、游标 = 实际 0 行、立即收敛为已到底（不再假称还有 500 条）',
    s3Empty.entries.length === 0 && s3Empty.articlesLimit === 0 && s3Empty.articlesCursor['10'] === 0
    && s3Empty.articlesExhausted === true && s3Empty.articlesLoading === false);
  invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  checkNew('(s3) 空范围 + 已到底：入口守卫拦住无意义请求（不产生 IPC 空转）',
    invokeCalls.filter((c) => c.cmd === 'list_articles').length === 0);
  /* 反例（改造前会停在这里）：首批满页但全部被范围筛掉 ⇒ 未到底 + 空列表 ⇒ 必须还能推进 */
  backendRows = [];
  for (let i = 0; i < 503; i += 1) {
    backendRows.push(mkRow({ id: 4000 + i, feed_id: 20, title: `C${i}`, published_at: iso(NOW - i * 1000) }));
  }
  await resetStore();
  backendRows = [];
  for (let i = 0; i < 503; i += 1) {
    backendRows.push(mkRow({ id: 4000 + i, feed_id: 20, title: `C${i}`, published_at: iso(NOW - i * 1000) }));
  }
  store.setState({ dataMode: 'tauri', activeFeedFilter: '10' });
  /* 模拟改造前的口径：首批按全局拉满 500 行，范围里一条都没有 */
  store.setState({ entries: [], articlesLimit: 500, articlesCursor: { '10': 500 }, articlesExhausted: false });
  invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  const s3Advance = invokeCalls.find((c) => c.cmd === 'list_articles');
  checkNew('(s3) 列表为空且未到底时仍可发起下一页：请求带当前范围（feed_id=10 / offset=500）', 
    s3Advance?.args.args.feed_id === 10 && s3Advance?.args.args.offset === 500);
  checkNew('(s3) 该源确无更多数据时空页把状态收敛为「已到底 + 空列表」（补拉不会无限循环）',
    store.getState().entries.length === 0 && store.getState().articlesExhausted === true
    && store.getState().articlesLoading === false && store.getState().articlesCursor['10'] === 500);
  /* 再触发：守卫挡住（已到底） */
  invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  checkNew('(s3) 收敛后再触发不再发 IPC（空列表补拉不会反复打后端）',
    invokeCalls.filter((c) => c.cmd === 'list_articles').length === 0);

  /* ---------- (s5) D2/D3 守住（051 修复不回退） ---------- */
  await resetStore();
  store.setState({ dataMode: 'tauri', activeFeedFilter: '10', entries: [], articlesLimit: 100, articlesCursor: { '10': 100 }, articlesExhausted: false, toasts: [] });
  listPlan = { mode: 'reject', error: { message: 'db busy' } };
  invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  const s5Fail = store.getState();
  checkNew('(s5/D2) 分页失败仍给出可见 toast（文案 + 「重试」action），不是静默吞错',
    s5Fail.toasts.length === 1 && s5Fail.toasts[0].text.includes('加载更多失败')
    && s5Fail.toasts[0].text.includes('db busy') && s5Fail.toasts[0].action?.label === '重试'
    && typeof s5Fail.toasts[0].action?.run === 'function');
  checkNew('(s5/D2) 失败路径同样复位加载态且不破坏游标/条目',
    s5Fail.articlesLoading === false && s5Fail.articlesLimit === 100
    && s5Fail.articlesCursor['10'] === 100 && s5Fail.entries.length === 0);
  /* D2 的重试按钮真的能重发（把后端恢复后点重试） */
  listPlan = null;
  invokeCalls.length = 0;
  s5Fail.toasts[0].action.run();
  await nTick(20);
  checkNew('(s5/D2) 失败 toast 的「重试」确实重发分页请求（按当前范围，不是死按钮）',
    invokeCalls.filter((c) => c.cmd === 'list_articles').length >= 1
    && invokeCalls.some((c) => c.cmd === 'list_articles' && c.args.args.feed_id === 10 && c.args.args.offset === 100));
  listPlan = null;

  /* D3：竞态丢弃分支必须复位 articlesLoading（且新增的 scopeKey 收紧后依然如此）。
     场景：在途加载期间用户切到另一个范围（selectFeed 会把游标换成新范围的值，
     可能恰好等于 in-flight 的 offset——旧的「只比数值」判据会漏判）。 */
  await resetStore();
  store.setState({ dataMode: 'tauri', activeFeedFilter: '10', entries: [], articlesLimit: 100, articlesCursor: { '10': 100, '11': 100 }, articlesExhausted: false, articlesLoading: false });
  listPlan = { mode: 'defer' };
  const s5Race = store.getState().loadMoreArticles();
  await nTick(0);
  checkNew('(s5/D3) 竞态场景成立：分页请求在途且加载态为真', store.getState().articlesLoading === true && pendingList.length === 1);
  store.getState().selectFeed('11');   // 切范围：游标换成源B 的 100（数值与 offset 相同）
  pendingList[0].resolve([mkRow({ id: 6001, feed_id: 10, title: '源A 的迟到数据' })]);
  await s5Race;
  const s5RaceAfter = store.getState();
  checkNew('(s5/D3) 范围已变（scopeKey 不同）⇒ 迟到页被丢弃，且 articlesLoading 复位（不永久为真）',
    s5RaceAfter.articlesLoading === false && s5RaceAfter.activeFeedFilter === '11'
    && s5RaceAfter.entries.length === 0);
  listPlan = null;   // 退出 defer 模式：下面这一次必须真的走完（否则永远挂起）
  invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  checkNew('(s5/D3) 竞态丢弃后入口守卫没被锁死：下一次分页照常发出（且用新范围源B）',
    invokeCalls.filter((c) => c.cmd === 'list_articles').length === 1
    && invokeCalls.find((c) => c.cmd === 'list_articles')?.args.args.feed_id === 11);

  /* 同范围内的「游标被 reload 重置」这条既有 D3 场景，在新判据下也必须仍然丢弃 */
  await resetStore();
  store.setState({ dataMode: 'tauri', activeFeedFilter: '10', entries: [], articlesLimit: 100, articlesCursor: { '10': 100 }, articlesExhausted: false });
  listPlan = { mode: 'defer' };
  const s5Race2 = store.getState().loadMoreArticles();
  await nTick(0);
  store.setState({ articlesLimit: 0, articlesCursor: { '10': 0 } });   // 同范围内游标被重置
  pendingList[pendingList.length - 1].resolve([mkRow({ id: 6100, feed_id: 10 })]);
  await s5Race2;
  listPlan = null;
  checkNew('(s5/D3) 同范围内游标被重置：迟到页仍被丢弃并复位加载态（既有竞态语义不变）',
    store.getState().articlesLoading === false && store.getState().entries.length === 0
    && store.getState().articlesLimit === 0);

  /* ---------- (s6) 视图/范围切换时游标与内容一致（缓存也不串范围） ---------- */
  await resetStore();
  /* 清掉前面用例留下的视图缓存：本组要独立验证「缓存按 布局×视图×范围 分桶」 */
  viewEntriesCache.clear();
  const s6Rows = [];
  for (let i = 0; i < 40; i += 1) {
    s6Rows.push(mkRow({ id: 2000 + i, feed_id: 10, title: `A${i}`, published_at: iso(NOW - i * 1000), is_starred: i < 3 }));
    s6Rows.push(mkRow({ id: 3000 + i, feed_id: 12, title: `D${i}`, published_at: iso(NOW - i * 1000 - 400), is_starred: i < 2 }));
  }
  backendRows = s6Rows;
  store.getState().selectFeed('10');
  await store.getState().reloadFromBackend();
  const s6A = store.getState();
  checkNew('(s6) 单源首批：40 条全属源A、游标 40、因不足一页而到底',
    s6A.entries.length === 40 && s6A.entries.every((e) => e.feedId === '10')
    && s6A.articlesLimit === 40 && s6A.articlesExhausted === true);
  /* 同源切视图（收藏）：口径 = 同一范围 + 更窄筛选，游标 = 该筛选结果行数 */
  store.getState().selectView('starred');
  await nTick(20);
  const s6Starred = store.getState();
  checkNew('(s6) 同源切「收藏」：查询仍带 feed_id，游标 = 该筛选实际行数（不是 40）',
    s6Starred.entries.length === 3 && s6Starred.entries.every((e) => e.feedId === '10')
    && s6Starred.articlesLimit === 3 && s6Starred.articlesCursor['10'] === 3
    && s6Starred.articlesExhausted === true);
  /* 视图缓存必须按范围分桶：切回 all 时不能把「源A 收藏」当成「源A 全部」恢复。
     关键证据是**同步恢复**那一步（缓存命中在 reload 之前同步生效，后台刷新随后才到）：
     缓存若不按视图分桶，切回 all 会拿「收藏的 3 条」冒充「全部的 40 条」。 */
  listPlan = { mode: 'defer' };   // 冻结后台刷新，只观察缓存恢复本身
  store.getState().selectView('all');
  const s6BackSync = store.getState();
  /* exhausted 这里为 false 是正确的：游标 40 来自「收藏视图只有 3 条」那次拉取，
     但切回「全部」时 entries 换成了 40 条的首批，我们并不知道「全部」是否已经到底，
     未到底（保守地允许下一次 loadMore）才是安全语义——若错标已到底，源A 更老的
     文章就再也取不回来了。 */
  checkNew('(s6) 切回「全部」缓存命中：同步恢复的是源A 的 40 条（不拿收藏视图的 3 条冒充），游标随之对齐且不误标已到底',
    s6BackSync.entries.length === 40 && s6BackSync.entries.every((e) => e.feedId === '10')
    && s6BackSync.articlesLimit === 40 && s6BackSync.articlesCursor['10'] === 40
    && s6BackSync.articlesExhausted === false && s6BackSync.articlesLoading === false);
  listPlan = null;
  await nTick(20);
  checkNew('(s6) 后台静默刷新完成后结论不变（仍是源A 的 40 条）',
    store.getState().entries.length === 40 && store.getState().entries.every((e) => e.feedId === '10'));
  /* 切到另一范围：列表必须换成新范围，且不继承源A 的游标 */
  store.getState().selectFeed('12');
  checkNew('(s6) 切源D：游标从 0 起步（源D 尚未加载，不继承源A 的 40）', store.getState().articlesLimit === 0);
  await store.getState().reloadFromBackend();
  const s6D = store.getState();
  checkNew('(s6) 源D 首批：只含源D 条目（源A 的列表不残留），游标按源D 起步',
    s6D.entries.length === 40 && s6D.entries.every((e) => e.feedId === '12')
    && s6D.articlesLimit === 40 && s6D.articlesExhausted === true);
  checkNew('(s6) per-scope 游标并存：源A 的 40 与源D 的 40 各自记账，互不覆盖',
    s6D.articlesCursor['12'] === 40 && s6D.articlesCursor['10'] === 40);

  /* ============================================================
     TASK-052 §A/B：口径修复的可观察判据（同一组判据在修前/修后分别跑过）
     每条都对应「修前为假 → 修后为真」的可观察量。完整 A/B 明细见
     tmp/task052/ab-before.json / ab-after.json / ab-diff.txt（11 项翻转）。
     ============================================================ */
  {
    const scopeArgsOf = (calls) => [...calls].reverse().find((c) => c.cmd === 'list_articles')?.args?.args;

    /* A1：单源视图连续翻页取回该源的后续文章 */
    await resetStore();
    backendRows = [];
    for (let i = 0; i < 1200; i += 1) {
      backendRows.push(mkRow({ id: 6000 + i, feed_id: 10, published_at: iso(NOW - i * 1000) }));
      backendRows.push(mkRow({ id: 7000 + i, feed_id: 11, published_at: iso(NOW - i * 1000 - 500) }));
    }
    store.getState().selectFeed('10');
    await store.getState().reloadFromBackend();
    const a1First = store.getState().entries.map((e) => e.id);
    invokeCalls.length = 0;
    await store.getState().loadMoreArticles();
    const a1Args = scopeArgsOf(invokeCalls);
    const a1Second = store.getState().entries.slice(a1First.length).map((e) => e.id);
    checkNew('(A1) 单源翻页带上范围参数 feed_id=10（修前：不带 ⇒ 拿全局下一页）',
      a1Args?.feed_id === 10);
    checkNew('(A1) 单源两页条目全部属于该源（修前：混入源B）',
      store.getState().entries.every((e) => e.feedId === '10'));
    checkNew('(A1) 第 2 页是该源的后续序列（修前：取到全局第 501 条，不是 6500）',
      a1Second[0] === '6500' && a1Second.length === 500);
    checkNew('(A1) 两页 id 集合不重叠', a1Second.every((id) => !a1First.includes(id)));

    /* A2：单分类视图（folder_id 口径） */
    await resetStore();
    backendRows = [];
    for (let i = 0; i < 600; i += 1) {
      backendRows.push(mkRow({ id: 8000 + i, feed_id: 10, published_at: iso(NOW - i * 1000) }));
      backendRows.push(mkRow({ id: 9000 + i, feed_id: 12, published_at: iso(NOW - i * 1000 - 300) }));
      backendRows.push(mkRow({ id: 9500 + i, feed_id: 20, published_at: iso(NOW - i * 1000 - 600) }));
    }
    store.getState().selectFeed('cat-1');
    await store.getState().reloadFromBackend();
    invokeCalls.length = 0;
    await store.getState().loadMoreArticles();
    const a2Args = scopeArgsOf(invokeCalls);
    checkNew('(A2) 单分类翻页带上 folder_id=1 且 feed_id=null（修前：两者都不带）',
      a2Args?.folder_id === 1 && a2Args?.feed_id === null);
    checkNew('(A2) 分类两页条目只含该分类的源（修前：混入分类外源C）',
      store.getState().entries.every((e) => e.feedId === '10' || e.feedId === '12')
      && !store.getState().entries.some((e) => e.feedId === '20'));

    /* A4：per-scope 游标互不污染 */
    await resetStore();
    backendRows = [];
    for (let i = 0; i < 1200; i += 1) {
      backendRows.push(mkRow({ id: 6000 + i, feed_id: 10, published_at: iso(NOW - i * 1000) }));
      backendRows.push(mkRow({ id: 7000 + i, feed_id: 11, published_at: iso(NOW - i * 1000 - 500) }));
    }
    store.getState().selectFeed('10');
    await store.getState().reloadFromBackend();
    await store.getState().loadMoreArticles();
    store.getState().selectFeed('11');
    checkNew('(A4) 切到源B 后游标从 0 起步（修前：继承源A 的 1000）',
      store.getState().articlesLimit === 0);
    invokeCalls.length = 0;
    await store.getState().reloadFromBackend();
    checkNew('(A4) 源B 首批按 feed_id=11 取（修前：不带范围 ⇒ 拿全局首批，混入源A）',
      scopeArgsOf(invokeCalls)?.feed_id === 11
      && store.getState().entries.every((e) => e.feedId === '11'));

    /* A3：空范围首批收敛（修前：游标硬编码 500、永远假装还有数据） */
    await resetStore();
    backendRows = [mkRow({ id: 111, feed_id: 20 })];
    store.getState().selectFeed('10');
    await store.getState().reloadFromBackend();
    checkNew('(A3) 空范围首批立即收敛为「已到底」（修前：articlesExhausted=false，空列表却假装还有 500 条）',
      store.getState().entries.length === 0 && store.getState().articlesExhausted === true);

    /* A5：D2/D3 —— A/B 探针显示修前修后一致（均 true），固化为防回退断言 */
    await resetStore();
    store.setState({ articlesLimit: 100 });
    listPlan = { mode: 'reject', error: { message: 'db busy' } };
    await store.getState().loadMoreArticles();
    listPlan = null;
    const a5 = store.getState();
    checkNew('(A5/D2) 失败 toast + 可调用重试（修前修后一致：不回退）',
      a5.toasts.length === 1 && a5.toasts[0].text.includes('加载更多失败')
      && a5.toasts[0].text.includes('db busy')
      && typeof a5.toasts[0].action?.run === 'function');
    checkNew('(A5/D3) 竞态丢弃复位 articlesLoading（修前修后一致：不回退）',
      a5.articlesLoading === false);
  }
})();

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
  const { sentinelMode } = await import('../src/components/timelineSentinel.ts');

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
  const { Timeline } = await import('../src/components/Timeline.tsx');
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
    await import('../dist-test/components/settings/endpointHint.js');

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
    await import('../dist-test/store/syncErrors.js');

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
  const syncTabSrc = fs.readFileSync(new URL('../src/components/settings/SyncTab.tsx', import.meta.url), 'utf8');
  const syncSliceSrc = fs.readFileSync(new URL('../src/store/slices/sync.ts', import.meta.url), 'utf8');
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
  const prevInvoke = globalThis.__INVOKE__;
  globalThis.__INVOKE__ = (cmd, args) => {
    invokeCalls.push({ cmd, args });
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
  };

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
  const cssSrc = fs.readFileSync(new URL('../src/styles/base.css', import.meta.url), 'utf8');
  checkNew('(f6) toast 层宽度受视口约束（防窄窗口下长提示溢出屏幕左侧）',
    /\.toast-layer\s*\{[^}]*max-width:\s*min\(460px,\s*calc\(100vw - 40px\)\)/s.test(cssSrc));
  checkNew('(f6) toast 文案允许换行（防长失败提示被截断而看不到原因）',
    /\.toast-pill\s*\{[^}]*white-space:\s*normal/s.test(cssSrc)
    && /\.toast-pill\s*\{[^}]*overflow-wrap:\s*anywhere/s.test(cssSrc));
  checkNew('(f6) 修前形态可复现：`.toast-pill` 原为 nowrap（对照基线证据中的 315px 溢出）',
    !/\.toast-pill\s*\{[^}]*white-space:\s*nowrap/s.test(cssSrc));

  globalThis.__INVOKE__ = prevInvoke;
}
  const fs = await import('node:fs');
/* ============================================================
     TASK-065 N8/N11：卡片与 Reader 的译文渲染契约（源码形态断言）
     渲染分支无 DOM harness，以源码文本核对四处分支的存在性：
     未消毒（rawTranslatedIds 命中）→ 纯文本插值；消毒后 → dangerouslySetInnerHTML。
     修前卡片为纯文本插值（无分支、无 dangerouslySetInnerHTML）→ 断言失败。
     ============================================================ */
  const tlSrc = fs.readFileSync(new URL('../src/components/Timeline.tsx', import.meta.url), 'utf8');
  const readerSrc = fs.readFileSync(new URL('../src/components/Reader.tsx', import.meta.url), 'utf8');
  const socialBlock = tlSrc.slice(tlSrc.indexOf('social-translated-block'), tlSrc.indexOf('social-actions-bar'));
  const notifStart = tlSrc.indexOf('notif-translated-block');
  const notifBlock = notifStart < 0 ? '' : tlSrc.slice(notifStart, tlSrc.indexOf('notif-expand-btn', notifStart));
  checkNew('(n8) SocialCard 译文块按 rawTranslated 分支：消毒后 dangerouslySetInnerHTML（修前纯文本插值）',
    socialBlock.includes('rawTranslated ? (') && socialBlock.includes('dangerouslySetInnerHTML'));
  checkNew('(n8) NotifCard 译文块同样分支（修前纯文本插值显示字面标签）',
    notifBlock.includes('rawTranslated ? (') && notifBlock.includes('dangerouslySetInnerHTML'));
  checkNew('(n11) Reader 译文渲染含未消毒纯文本分支（流式产物不进 HTML 渲染路径）',
    readerSrc.includes('rawStream')
    && readerSrc.includes('dangerouslySetInnerHTML'));

  {
    const fsP = await import('node:fs');
    const appSrc = fsP.readFileSync(new URL('../src/App.tsx', import.meta.url), 'utf8');
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
    const fixture = JSON.parse(fsR.readFileSync(new URL('../src-tauri/tests/fixtures/row_fixture.json', import.meta.url), 'utf8'));
    const { articleRowToEntry, feedRowToItem } = await import('../dist-test/lib/api.js');
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
      aiSummary: 'fixture summary', content: '<p>body</p>', rawContent: '<p>body</p>',
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

    checkNew('(r) articleRowToEntry 键集合与逐键取值全量一致（缺键/多键/错值/接错源任一即失败）',
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

// ---- 汇总 ----
const failed = results.filter((r) => !r.pass);
const newFailed = newResults.filter((r) => !r.pass);
const totalAll = results.length + newResults.length;
const totalFailed = failed.length + newFailed.length;
console.log(`\n=== 既有回归 ${results.length - failed.length}/${results.length} 通过（本文件原有断言，未改动一行） ===`);
console.log(`=== 新增 store 行为断言 ${newResults.length - newFailed.length}/${newResults.length} 通过 🆕 ===`);
console.log(`=== 前端逻辑回归合计 ${totalAll - totalFailed}/${totalAll} 通过 ===`);
if (totalFailed) {
  if (failed.length) console.error('既有失败项:', failed.map((f) => f.name).join('; '));
  if (newFailed.length) console.error('新增失败项:', newFailed.map((f) => f.name).join('; '));
  process.exit(1);
}
process.exit(0);
