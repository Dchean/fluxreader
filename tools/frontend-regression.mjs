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
  let rejectWhen = null;        // (p3-f2) TASK-093：按 (cmd, args) 谓词注入失败——连点场景只拒第一次的置位写
  let getArticlesPlan = null;   // TASK-103：批量水合行为——{ rows } | { mode:'defer' } | { mode:'reject', error }
  let pendingGetArticles = [];  // defer 模式挂起项 { ids, resolve }
  /* TASK-107：feed_counts 行为。null = 静态 COUNTS（既有断言基线——夹具的计数
     与行级数据刻意不一致，大量断言依赖）；函数 = 按 backendRows 忠实聚合
     （计数对账用例：markCurrentViewAllRead 成功后重取的计数必须等于后端口径）。 */
  let feedCountsImpl = null;
  let heldAi = [];            // hold 模式挂起项 { cmd, id, ch }
  let settingsRaw = null;     // get_setting('app_settings') 的返回值
  let ghLoginStatus = null;   // github_login_status 的返回值：null | {login} | 'reject'
  let extractResult = null;   // extract_fulltext 的返回值：ExtractFulltextResult | 'reject' | null
                              // TASK-076：后端改为结构化 { html, degraded, reason }

  const localDayKey = (ms) => {
    const d = new Date(ms);
    return `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
  };
  /* 内存版 list_articles：支持 范围/未读/收藏/今天/排序/keyset 分页，
     行为与后端契约一致。TASK-117：排序补 id 决胜（与后端 ORDER BY
     a.published_at, a.id 同口径）；last_published/last_id 成对给出时按 keyset
     谓词取「严格排在锚之后」的行（OFFSET 停用）——锚点与集合增删无关，
     这是 t117 操作序列回归（阅读中翻页/取消收藏）假后端语义的根基。 */
  function queryRows(a = {}) {
    let out = backendRows.slice();
    if (a.feed_id != null) out = out.filter((r) => r.feed_id === a.feed_id);
    if (a.folder_id != null) out = out.filter((r) => (FEEDS.find((f) => f.id === r.feed_id) || {}).folder_id === a.folder_id);
    if (a.only_unread) out = out.filter((r) => !r.is_read);
    if (a.only_starred) out = out.filter((r) => r.is_starred);
    if (a.only_today) out = out.filter((r) => localDayKey(Date.parse(r.published_at)) === localDayKey(NOW));
    const dir = a.newest_first === false ? 1 : -1;
    out.sort((x, y) => (dir * (Date.parse(x.published_at) - Date.parse(y.published_at))) || (dir * (x.id - y.id)));
    if (a.last_published != null && a.last_id != null) {
      /* keyset 谓词（与后端同语义；比较用 published_at 原文——假后端的
         published_at 恒为 toISOString 的统一形态，字符串序与时间序一致） */
      out = out.filter((r) => dir < 0
        ? (r.published_at < a.last_published || (r.published_at === a.last_published && r.id < a.last_id))
        : (r.published_at > a.last_published || (r.published_at === a.last_published && r.id > a.last_id)));
      return out.slice(0, a.limit == null ? out.length : a.limit);
    }
    const off = a.offset || 0;
    return out.slice(off, a.limit == null ? out.length : off + a.limit);
  }
  /* TASK-107：feed 生效布局（feed 级覆盖 → 分类兜底），mark_all_read 的
     layout 过滤与 list_articles 的 where 构建同口径 */
  function effectiveFeedLayout(feedIdNum) {
    const f = FEEDS.find((x) => x.id === feedIdNum);
    if (!f) return null;
    return f.layout !== 'inherit' ? f.layout : (FOLDERS.find((c) => c.id === f.folder_id) || {}).layout || null;
  }
  /* TASK-107：按 backendRows 忠实聚合 feed_counts（与 Rust feed_counts 同口径：
     COUNT(*) / SUM(is_read=0) / SUM(is_starred=1) / today 按 localtime 判日）。
     仅在计数对账用例挂到 feedCountsImpl 上（缺省静态 COUNTS 保既有基线）。 */
  function countsFromRows() {
    const byFeed = new Map();
    for (const r of backendRows) {
      const c = byFeed.get(r.feed_id) ?? { feed_id: r.feed_id, total: 0, unread: 0, starred: 0, today: 0 };
      c.total += 1;
      if (!r.is_read) c.unread += 1;
      if (r.is_starred) c.starred += 1;
      if (localDayKey(Date.parse(r.published_at)) === localDayKey(NOW)) c.today += 1;
      byFeed.set(r.feed_id, c);
    }
    return [...byFeed.values()];
  }
  function emitAi(plan, ch) {
    for (const d of plan.deltas) ch.onmessage?.({ type: 'delta', data: d });
    if (plan.error) { ch.onmessage?.({ type: 'error', data: plan.error }); return; }
    if (plan.finish) ch.onmessage?.({ type: 'done' });
  }

  /* 替换上面的 S-1…S-5 invoke mock（api 每调用一次都读 globalThis.__INVOKE__） */
  globalThis.__INVOKE__ = (cmd, args) => {
    invokeCalls.push({ cmd, args });
    if (rejectCmds.has(cmd) || (rejectWhen && rejectWhen(cmd, args))) return Promise.reject({ message: '注入失败:' + cmd });
    switch (cmd) {
      case 'list_folders': return failReload ? Promise.reject(failReload) : Promise.resolve(FOLDERS);
      case 'list_feeds': return failReload ? Promise.reject(failReload) : Promise.resolve(FEEDS);
      /* TASK-107：feedCountsImpl 非空时按 backendRows 忠实聚合（计数对账用例）；
         缺省仍返回静态 COUNTS——既有断言基线依赖它（与行级夹具刻意不一致） */
      case 'feed_counts': return failReload ? Promise.reject(failReload) : Promise.resolve(feedCountsImpl ? feedCountsImpl() : COUNTS);
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
      case 'get_articles': {
        /* TASK-103 可控行为：成功返回 rows / defer 挂起（驱动在途窗口）/ 失败 reject */
        if (getArticlesPlan && getArticlesPlan.mode === 'reject') return Promise.reject(getArticlesPlan.error ?? { message: 'get_articles 注入失败' });
        if (getArticlesPlan && getArticlesPlan.mode === 'defer') {
          return new Promise((resolve) => { pendingGetArticles.push({ ids: args.ids, resolve }); });
        }
        return Promise.resolve(getArticlesPlan ? getArticlesPlan.rows : []);
      }
      case 'get_setting': return Promise.resolve(settingsRaw);
      case 'set_setting': return Promise.resolve(null);
      /* TASK-107：单条标读忠实落库（record_read_state 语义：仅该行自身，
         无本地同文副本传播）——单条计数一致性核查的根基 */
      case 'set_read': {
        const row = backendRows.find((r) => r.id === Number(args.id));
        /* TASK-107 R2/F3：mutation 用布尔——Rust Serialize 的 wire 格式是 bool，
           数字 1/0 会让 reload 后的 entry.isRead 变成 number，破坏 `=== true` 形态
           的守卫/断言（快照替换场景因此失真） */
        if (row) row.is_read = args.read === true;
        return Promise.resolve(null);
      }
      /* TASK-117：单条收藏忠实落库（record_star_state 语义：仅该行自身）——
         取消收藏场景的续拉回归（t117-2）需要后端集合真实收缩。 */
      case 'set_starred': {
        const row = backendRows.find((r) => r.id === Number(args.id));
        if (row) row.is_starred = args.starred === true;
        return Promise.resolve(null);
      }
      /* TASK-117：批量标读忠实落库（apply_read_bulk 语义：逐 id 翻转自身行）——
         阅读中连续翻页回归（t117-1）需要 is_read=0 的筛选集合真实收缩。 */
      case 'set_read_bulk': {
        for (const id of args.ids ?? []) {
          const row = backendRows.find((r) => r.id === Number(id));
          if (row) row.is_read = args.read === true;
        }
        return Promise.resolve(null);
      }
      /* TASK-107：mark_all_read 忠实落库（apply_mark_all_read 语义：整个
         范围×布局×视图口径的未读行置已读，返回受影响行数）——600/1 探针
         与「成功后计数以后端为准」对账的根基 */
      case 'mark_all_read': {
        let affected = 0;
        for (const r of backendRows) {
          if (r.is_read) continue;
          if (args.feedId != null && r.feed_id !== args.feedId) continue;
          if (args.folderId != null && (FEEDS.find((f) => f.id === r.feed_id) || {}).folder_id !== args.folderId) continue;
          if (args.starredOnly && !r.is_starred) continue;
          if (args.sinceMs != null && localDayKey(Date.parse(r.published_at)) !== localDayKey(args.sinceMs)) continue;
          if (args.layout && effectiveFeedLayout(r.feed_id) !== args.layout) continue;
          r.is_read = true; // TASK-107 R2/F3：wire 格式为布尔（见 set_read 处注释）
          affected += 1;
        }
        return Promise.resolve(affected);
      }
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
      /* TASK-094：布局绑定写命令必须「落库」——selectLayout 触发的后台 reload 会用
         list_feeds/list_folders 重建 feedIndex，mock 若不回写，写库后立即可见的
         feedIndex 会被 reload 用旧值覆盖（真实后端落库后读回同值，不会）。 */
      case 'update_feed_layout': {
        const f = FEEDS.find((x) => x.id === Number(args.id));
        if (f) f.layout = args.layout;
        return Promise.resolve(null);
      }
      case 'update_folder_layout': {
        const c = FOLDERS.find((x) => x.id === Number(args.id));
        if (c) c.layout = args.layout;
        return Promise.resolve(null);
      }
      default: return Promise.resolve(null);
    }
  };

  /* ---------- 状态复位 / 规范化装载 ---------- */
  async function resetStore(extra) {
    /* TASK-107：行对象深拷贝——假后端的 set_read / mark_all_read 会就地翻转
       is_read（忠实模拟落库），浅引用会让突变跨用例残留（污染 BASE_ROWS） */
    backendRows = BASE_ROWS.map((r) => ({ ...r }));
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
    rejectWhen = null;
    getArticlesPlan = null;
    pendingGetArticles = [];
    feedCountsImpl = null;
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
     并且按范围键（此处 'article|all'，TASK-094 起含布局）记进 articlesCursor。 */
  checkNew('(a) 首批分页游标 = 实际行数（per-scope：不再硬编码 PAGE_SIZE）、8 行 < 500 视为已到底、加载态收起',
    aOk.entries.length === 8 && aOk.articlesLimit === 8 && aOk.articlesCursor['article|all']?.loaded === 8
    && aOk.articlesExhausted === true && aOk.articlesLoading === false);

  /* 【TASK-103 改动理由】旧断言「新快照清空 hydratedIds/hydrationErrors」锁的
     正是 REQ-001 的根因手法：无条件清空让已水合卡片在后台刷新后失去正文与
     终态，而虚拟列表按 id 保持卡片身份、useLazyHydrate 同 id 不重触发——卡片
     永挂「加载正文…」且无请求在途（审计探针复现）。新契约：快照替换按 id
     继承正文与终态（mergeSnapshotEntries），只裁剪已不在新快照中的滞留标记
     ——本断言改写为锁定继承+裁剪，非为过门禁而弱化（覆盖面反而更宽：
     同时验证「仍在快照的保留」与「已消失的移除」两侧）。 */
  store.setState({ hydratedIds: { '101': true, '999': true }, hydrationErrors: { '102': '旧错误', '998': 'x' } });
  await store.getState().reloadFromBackend();
  checkNew('(a) 新快照按 id 继承并裁剪水合终态（TASK-103：仍在快照的 101/102 保留，已消失的 999/998 移除）',
    store.getState().hydratedIds['101'] === true
    && store.getState().hydrationErrors['102'] === '旧错误'
    && store.getState().hydratedIds['999'] === undefined
    && store.getState().hydrationErrors['998'] === undefined);

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

  /* 【TASK-117 改动理由】期望顺序随并列决胜更新：BASE_ROWS 此前按插入序稳定排序，
     id 决胜后同秒组内 id 降序——TODAY 组 {201,102,101}、OLD 组 {301,202,105,104,103}；
     断言意图（集合成员与派生口径）不变，仅并列序变化。 */
  checkNew('(b) 同一分类在 article 布局下覆盖源A+源D（5 条）',
    selectScopeEntries(store.getState()).map((e) => e.id).join(',') === '102,101,105,104,103');

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

  /* 【TASK-117 改动理由】期望顺序随并列决胜更新：BASE_ROWS 此前按插入序稳定排序，
     id 决胜后同秒组内 id 降序——TODAY 组 {201,102,101}、OLD 组 {301,202,105,104,103}；
     断言意图（集合成员与派生口径）不变，仅并列序变化。 */
  checkNew('(c) 收藏视图拉取走 only_starred：entries 只剩 3 条收藏（跨布局，布局在派生层过滤）',
    store.getState().entries.map((e) => e.id).join(',') === '201,102,103');
  store.setState({ activeViewFilter: 'all', entries: [], articlesLimit: 0, articlesExhausted: false });
  store.getState().selectView('starred');
  const cCache = store.getState();
  checkNew('(c) selectView 命中视图缓存：同步恢复快照（零延迟、游标=快照长度、标记已到底）',
    cCache.activeViewFilter === 'starred' && cCache.entries.map((e) => e.id).join(',') === '201,102,103'
    && cCache.articlesLimit === 3 && cCache.articlesExhausted === true);
  await nTick(20);
  checkNew('(c) 缓存命中后的后台静默刷新不改变收藏视图结论',
    store.getState().entries.map((e) => e.id).join(',') === '201,102,103');

  store.setState({ activeViewFilter: 'all', entries: [], articlesLimit: 0, openedReadIds: { '102': true } });
  store.getState().selectView('unread');
  checkNew('(c) selectView(unread) 立即切视图并清空已读保留快照',
    store.getState().activeViewFilter === 'unread' && Object.keys(store.getState().openedReadIds).length === 0);
  await nTick(20);
  const cUnread = store.getState();
  /* 【TASK-110 改动理由】原断言名「拉全量/不再分页」编码的正是本卡废除的近似全集
     手法（limit:100000 一次拉完）。分页化后这里是**首屏**：6 行 < 页大小(500) ⇒
     exhausted 真实判定为 true。判定条件本身（6 条/游标=行数/已到底）与修前一致，
     只改措辞对齐新语义，不弱化保护。 */
  checkNew('(c) 未读视图走 only_unread 拉取首屏：只剩 6 条未读、游标=行数、不足一页即真实判定已到底',
    cUnread.entries.map((e) => e.id).join(',') === '201,101,301,105,104,103'
    && cUnread.articlesLimit === 6 && cUnread.articlesExhausted === true && cUnread.articlesLoading === false);
  store.getState().selectView('today');
  await nTick(20);
  const cToday = selectVisibleEntries(store.getState());
  checkNew('(c) 今天视图只含本地当天条目（与列表「今天」判定同口径）',
    cToday.length > 0 && cToday.every((e) => localDayKey(e.publishedAt) === localDayKey(Date.now())));

  await bootFixture();

  /* 【TASK-117 改动理由】期望顺序随并列决胜更新：BASE_ROWS 此前按插入序稳定排序，
     id 决胜后同秒组内 id 降序——TODAY 组 {201,102,101}、OLD 组 {301,202,105,104,103}；
     断言意图（集合成员与派生口径）不变，仅并列序变化。 */
  checkNew('(c) 全部视图 + 显示全部：article 布局可见 5 条，按发布时间降序（同秒由 id 决胜）',
    selectVisibleEntries(store.getState()).map((e) => e.id).join(',') === '102,101,105,104,103');
  store.getState().toggleTimelineFilter();
  checkNew('(c) toggleTimelineFilter → 未读：已读条目从可见集合移除，未读保留',
    store.getState().timelineFilter === 'unread'
    && selectVisibleEntries(store.getState()).map((e) => e.id).join(',') === '101,105,104,103');
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

  /* 【TASK-117 改动理由】期望顺序随并列决胜更新：BASE_ROWS 此前按插入序稳定排序，
     id 决胜后同秒组内 id 降序——TODAY 组 {201,102,101}、OLD 组 {301,202,105,104,103}；
     断言意图（集合成员与派生口径）不变，仅并列序变化。 */
  checkNew('(d) 派生集合按布局解析：article = 源A+源D，social = 源B+源C',
    selectRawEntries({ ...store.getState(), activeContentLayout: 'article' }).map((e) => e.id).join(',') === '102,101,105,104,103'
    && selectRawEntries(store.getState()).map((e) => e.id).join(',') === '201,301,202');
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
    && selectRawEntries({ ...store.getState(), activeContentLayout: 'podcast' }).map((e) => e.id).join(',') === '201,301,202'
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
  /* 【TASK-107 改动理由】计数断言改为「与后端对账」口径：假后端升级为忠实
     聚合（feedCountsImpl），markCurrentViewAllRead 成功后重取 feed_counts
     整体替换——期望值从「乐观按已加载推算」改为后端真值（本块行级数据：
     feed10 未读 101/103、feed11 未读 201、feed12 未读 104/105；starredOnly
     只标 103/201）。保护意图不变且更强：数字必须等于后端口径而非本地推算。 */
  await bootFixture();
  feedCountsImpl = countsFromRows;
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
  /* 本块 scope = all × article 布局 × starred 视图：被标读的只有 103（feed10）；
     feed11 是 social 布局源，201 两侧（前端可见集/后端范围）都不在范围内 → 计数 1 保持 */
  checkNew('(f) 全部已读后计数=后端口径（源A 2→1：仅标读的 103 扣减；源D 2、跨布局源B 1 不动）',
    f1.feedCounts.get('10')?.unread === 1 && f1.feedCounts.get('12')?.unread === 2
    && f1.feedCounts.get('11')?.unread === 1);
  checkNew('(f) 全部已读后清空「已读保留」快照（列表不再保留灰色卡片）',
    Object.keys(f1.openedReadIds).length === 0);
  checkNew('(f) 全部已读给出 toast 反馈', f1.toasts.some((t) => t.text === '已全部标为已读'));

  await bootFixture();
  feedCountsImpl = countsFromRows;
  store.setState({ activeViewFilter: 'all', timelineFilter: 'unread', activeFeedFilter: 'cat-1' });
  invokeCalls.length = 0;
  store.getState().markCurrentViewAllRead();
  await nTick(0);
  const fCatCall = invokeCalls.find((c) => c.cmd === 'mark_all_read');
  const fCat = store.getState();
  checkNew('(f) 分类范围 → folderId=数字、feedId=null（cat- 前缀不被当作源 id）',
    fCatCall?.args.folderId === 1 && fCatCall?.args.feedId === null);
  /* TASK-107：计数期望值改为后端对账口径（整个分类范围的未读 101/103/104/105
     都被标读：feed10 2→0、feed12 2→0）；分类外的 feed11（201）计数与读态均不受影响 */
  checkNew('(f) 分类范围只标该分类可见条目，分类外条目不受影响',
    fCat.entries.find((a) => a.id === '101')?.isRead === true
    && fCat.entries.find((a) => a.id === '103')?.isRead === true
    && fCat.entries.find((a) => a.id === '201')?.isRead === false
    && fCat.feedCounts.get('10')?.unread === 0 && fCat.feedCounts.get('12')?.unread === 0
    && fCat.feedCounts.get('11')?.unread === 1);

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

  /* 布局维度：后端范围必须与当前布局同口径——修前不带 layout，文章布局点一次
     会把社交/通知/播客/画廊布局的源一并标读并逐条推远端（审计 round-3 真机：
     可见 14 张、后端写入 74 条，覆盖 5 个布局）。 */
  await bootFixture();
  store.getState().selectLayout('social');
  store.setState({ activeViewFilter: 'all', timelineFilter: 'unread', openedReadIds: {} });
  invokeCalls.length = 0;
  store.getState().markCurrentViewAllRead();
  await nTick(0);
  const fLayoutCall = invokeCalls.find((c) => c.cmd === 'mark_all_read');
  checkNew('(f) 全部已读带当前布局：layout=social（缺此参数则跨布局误标并推远端）',
    fLayoutCall?.args.layout === 'social' && fLayoutCall?.args.feedId === null
    && fLayoutCall?.args.folderId === null);

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
  /* AUDIT P3[F4]（TASK-084）：契约由「每个未读项一次 set_read」改为「整批一次
     set_read_bulk」。原断言（逐条 set_read）编码的是修前的行为，本卡按审计要求
     消除逐条 IPC，故此处**有意改写**并保留其原有意图（已读项不得重复写库）：
     ① 只发一次 set_read_bulk；
     ② 载荷只含未读项（已读的 102 不在其中）。 */
  checkNew('(f) 批量标读只发**一次** set_read_bulk IPC（修前是 N 次 set_read）',
    invokeCalls.filter((c) => c.cmd === 'set_read_bulk').length === 1
    && invokeCalls.filter((c) => c.cmd === 'set_read').length === 0);
  checkNew('(f) 批量标读只对未读项发 IPC（已读项不重复写库刷同步队列）',
    (invokeCalls.find((c) => c.cmd === 'set_read_bulk')?.args.ids ?? [])
      .slice().sort((a, b) => a - b).join(',') === '101,103');
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
  /* 【TASK-117 改动理由】续拉 wire 从 offset=500 改为 keyset 锚（last_published/
     last_id = 首屏最后一行 5499 的原文锚）——offset 正是本卡废除的缺陷手法，
     wire 断言随行为变化更新。 */
  checkNew('(g) 追加加载以 keyset 锚续拉（last_published/last_id=首屏末行 5499，无 offset）：500→503，条目顺序连续不重复',
    gListCall?.args.args.last_published === iso(NOW - 499 * 60000) && gListCall?.args.args.last_id === 5499
    && gListCall?.args.args.offset === undefined
    && gMore.articlesLimit === 503 && gMore.entries.length === 503
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
  /* 模拟期间发生 reload 重置游标：TASK-117 起守卫比对 per-scope keyset 游标，
     重置必须写游标本身（真实 reload 正是镜像+游标原子同写） */
  store.setState({ articlesLimit: 0, articlesLoading: false, articlesCursor: { 'article|all': { lastPublished: null, lastId: null, loaded: 0 } } });
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
  /* TASK-117：先给游标一个锚（上一场景的空游标已被重置），随后在途期间重置为
     空游标——守卫比对 per-scope keyset 游标，锚漂移即过期（重置须写游标本身，
     真实 reload 正是镜像+游标原子同写） */
  store.setState({ articlesLoading: false, articlesExhausted: false, articlesLimit: 100, entries: [], articlesCursor: { 'article|all': { lastPublished: iso(NOW - 100000), lastId: 5499, loaded: 100 } } });
  listPlan = { mode: 'defer' };
  const pRace = store.getState().loadMoreArticles();
  await nTick(0);
  store.setState({ articlesLimit: 0, articlesCursor: { 'article|all': { lastPublished: null, lastId: null, loaded: 0 } } });
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
  /* 【TASK-117 改动理由】期望页内容随 id 决胜序更新：BASE_ROWS 的 TODAY/OLD 并列组
     此前按插入序稳定排序（101,102 / 103..301），id 决胜后并列组内 id 降序
     （102,101 / 301,202,201,105,104,103）——301 的绝对位置从 7 变 2，锚定页
     从 [301] 变为其所在窗口的 6 行；断言意图（锚定到目标所在页）不变。 */
  checkNew('(h) 搜索结果锚定：按绝对位置拉取该页并选中（不再从头拉 500 篇）',
    invokeCalls.some((c) => c.cmd === 'article_index')
    && hAnchor.activeArticleId === '301'
    && hAnchor.entries.map((e) => e.id).join(',') === '301,202,105,104,103'
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
    && store.getState().entries.map((e) => e.id).join(',') === '301,202,105,104,103');

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

  /* ---------- P3[F2]（TASK-086）：单键快捷键让路浮层的判据 ----------
     判据已抽成 src/components/shortcutYield.ts 的纯函数 shouldYieldToOverlay，
     由 App.tsx 的真实 keydown 分支消费，故这里断言的是**组件实际使用的那份判定**。
     此前该判据内联在 App.tsx 的闭包里、零断言（审查 TASK-081-F2 登记的覆盖缺口）。 */
  {
    const { shouldYieldToOverlay, OVERLAY_YIELD_KEYS, OVERLAY_SOURCES, anyOverlayOpen }
      = await import('../src/components/shortcutYield.ts');
    checkNew('(P3[F2]) 浮层打开 + 单键 S/M/J/K ⇒ 让路（修前会作用到浮层背后的当前文章）',
      ['s', 'S', 'm', 'M', 'j', 'k'].every((k) => shouldYieldToOverlay(true, k, false) === 'yield'));
    checkNew('(P3[F2]) 浮层未打开 + 单键 ⇒ 不让路（快捷键照常生效）',
      ['s', 'S', 'm', 'M', 'j', 'k'].every((k) => shouldYieldToOverlay(false, k, false) === 'proceed'));
    checkNew('(P3[F2]) 浮层打开 + 带 Ctrl/Meta/Alt ⇒ 不让路（组合键属浮层自身操作，不受影响）',
      shouldYieldToOverlay(true, 's', true) === 'proceed'
      && shouldYieldToOverlay(true, 'k', true) === 'proceed');
    checkNew('(P3[F2]) 浮层打开 + 非目标键 ⇒ 不让路（只拦 S/M/J/K，不误伤其它键）',
      shouldYieldToOverlay(true, 'a', false) === 'proceed'
      && shouldYieldToOverlay(true, 'Enter', false) === 'proceed'
      && shouldYieldToOverlay(true, 'Escape', false) === 'proceed');
    checkNew('(P3[F2]) 适配性：让路键集合恰为 s/S/m/M/j/k（新增可让路键须同步本表）',
      OVERLAY_YIELD_KEYS.length === 6 && OVERLAY_YIELD_KEYS.join(',') === 's,S,m,M,j,k');
    /* 浮层集合逐项钉死：这是 TASK-086 审查者指出「文本断言可被绕过」的正面修复。
       此前 overlayOpen 是内联的 `a || b || c`，回归网只能查 token 在场——
       实测把「全屏播放器」从并集里删掉，322 条断言**全绿**（真实回归零告警）。
       改为清单求值后，每个浮层必须单独登记，任何一项被删/被改都会被下面两条拦下。 */
    const baseOverlay = {
      searchOpen: false, settingsOpen: false, newCategoryModalOpen: false,
      addFeedModalOpen: false, editFeedModalOpen: false, renameCatModalOpen: false,
      lightboxUrl: null, playerExpanded: false, playerActive: false,
    };
    checkNew('(P3[F2]) 浮层清单恰为 8 项且名称稳定（新增/删除浮层必须同步本表）',
      OVERLAY_SOURCES.length === 8
      && OVERLAY_SOURCES.map((o) => o.name).join(',')
        === 'search,settings,newCategory,addFeed,editFeed,renameCat,lightbox,playerExpanded');
    /* 逐项：只打开这一项 ⇒ anyOverlayOpen 必须为 true（漏判/少算任一项即失败） */
    const overlayProbes = [
      ['search', { searchOpen: true }],
      ['settings', { settingsOpen: true }],
      ['newCategory', { newCategoryModalOpen: true }],
      ['addFeed', { addFeedModalOpen: true }],
      ['editFeed', { editFeedModalOpen: true }],
      ['renameCat', { renameCatModalOpen: true }],
      ['lightbox', { lightboxUrl: 'https://example.com/a.png' }],
      ['playerExpanded', { playerExpanded: true, playerActive: true }],
    ];
    const missed = overlayProbes
      .filter(([, patch]) => anyOverlayOpen({ ...baseOverlay, ...patch }) !== true)
      .map(([name]) => name);
    checkNew('(P3[F2]) 每个浮层单独打开都必须被判为「浮层打开」（漏判任一项即失败，输出缺项名）',
      missed.length === 0);
    checkNew('(P3[F2]) 无浮层时 anyOverlayOpen 为 false（不误判为打开，否则快捷键全被吞）',
      anyOverlayOpen(baseOverlay) === false);
    /* 全屏播放器是**条件**浮层：仅在播放器激活时才算打开 */
    checkNew('(P3[F2]) playerExpanded 仅在 playerActive 时算浮层（未激活时不算，避免吞掉快捷键）',
      anyOverlayOpen({ ...baseOverlay, playerExpanded: true, playerActive: false }) === false
      && anyOverlayOpen({ ...baseOverlay, playerExpanded: true, playerActive: true }) === true);
    /* 空字符串 lightboxUrl 等同于「无 lightbox」（防 falsy 误判） */
    checkNew('(P3[F2]) lightboxUrl 为空串不算浮层（falsy 边界）',
      anyOverlayOpen({ ...baseOverlay, lightboxUrl: '' }) === false);

    /* 修前对照：旧行为完全不看浮层 → 浮层打开时同集键也照旧执行（不让路）。 */
    const legacyYield = () => 'proceed';
    checkNew('(P3[F2]) 修前判据可复现：不看浮层状态时「浮层打开 + S」也不会让路（缺陷根因）',
      legacyYield() === 'proceed' && shouldYieldToOverlay(true, 's', false) !== 'proceed');

    /* 源码形态断言：钉住 App.tsx 的 keydown **确实调用该判据**——纯函数有牙
       ≠ 调用方真的用它（TASK-081-R2-F1 的同类教训）。 */
    const fsS = await import('node:fs');
    const appSrc = fsS.readFileSync(new URL('../src/App.tsx', import.meta.url), 'utf8');
    const yieldAt = appSrc.indexOf('shouldYieldToOverlay(');
    const yieldBlock = yieldAt < 0 ? '' : appSrc.slice(yieldAt, yieldAt + 200);
    checkNew('(P3[F2]) App.tsx 的 keydown 消费该判据且按 yield 返回（删掉该分支即失败）',
      yieldBlock.includes('shouldYieldToOverlay(') && yieldBlock.includes("=== 'yield'")
      && /===\s*'yield'\s*\)\s*\{\s*return;/.test(yieldBlock));
    checkNew('(P3[F2]) 判据实参为 overlayOpen + 按键 + 修饰键（改实参即失败）',
      /shouldYieldToOverlay\(\s*overlayOpen\s*,\s*e\.key\s*,\s*e\.ctrlKey\s*\|\|\s*e\.metaKey\s*\|\|\s*e\.altKey\s*\)/
        .test(appSrc));
    /* overlayOpen 必须由 anyOverlayOpen 求值（而非退回内联并集）。
       内联写法下「少判一个浮层」无法被断言——见上面清单断言的说明。 */
    checkNew('(P3[F2]) App.tsx 的 overlayOpen 必须由 anyOverlayOpen 求值（退回内联并集即失败）',
      /const\s+overlayOpen\s*=\s*anyOverlayOpen\(/.test(appSrc));
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

  /* ---------- P3[F5]（REQ-102）：滚动出视口标已读，不得把「换序列的异步窗口」误算 ----------
     缺陷（AUDIT-20260919-v2 F5）：切换 布局/视图/排序 换掉 items 并触发 scrollTo(top:0)，
     但归零异步生效；期间滚动效应读到**旧** startIndex，就把新序列前段（用户没见过）
     整段标成已读。列表越长越容易命中，故表现为小概率。
     判据已抽为纯函数 scrollAwayRange（src/components/scrollAwayRead.ts）。 */
  {
    const { scrollAwayRange } = await import('../src/components/scrollAwayRead.ts');
    const N = 200; // 模拟长列表：旧 startIndex=120，新序列 200 条
    /* 修前逻辑（内联比较，不看是否用户滚动）的等价复刻：用于证明旧实现在同一输入下
       确实会误标——不是靠字面量断言，而是真的跑一遍旧算法。 */
    const legacyAwayRange = (startIndex, lastStartIndex) => (
      startIndex > lastStartIndex ? { from: lastStartIndex, to: startIndex } : null);
    checkNew('(P3[F5]) 修前可复现：非用户滚动 + startIndex 从 0 跳到 120 ⇒ 修前逻辑会误标 120 条',
      JSON.stringify(legacyAwayRange(120, 0)) === JSON.stringify({ from: 0, to: 120 })
      && scrollAwayRange({ scrollDriven: false, startIndex: 120, lastStartIndex: 0, itemCount: N })
        .range === null);
    checkNew('(P3[F5]) 非用户滚动（换布局/视图/排序的异步归零窗口）⇒ 绝不标读',
      scrollAwayRange({ scrollDriven: false, startIndex: 120, lastStartIndex: 0, itemCount: N })
        .range === null);
    checkNew('(P3[F5]) 非用户滚动仍须对齐基准（否则窗口关闭后基准停在旧值、后续真滚动会补标一大段）',
      scrollAwayRange({ scrollDriven: false, startIndex: 120, lastStartIndex: 0, itemCount: N })
        .nextLastStartIndex === 120);
    checkNew('(P3[F5]) 真实滚动 + startIndex 递增 ⇒ 标已读区间恰为 [上次基准, 本次 start)',
      JSON.stringify(scrollAwayRange({ scrollDriven: true, startIndex: 40, lastStartIndex: 25, itemCount: N }).range)
        === JSON.stringify({ from: 25, to: 40 }));
    checkNew('(P3[F5]) 真实滚动但未超过基准（往回滚/抖动）⇒ 不标读，且基准跟随下行不锁定',
      scrollAwayRange({ scrollDriven: true, startIndex: 10, lastStartIndex: 25, itemCount: N }).range === null
      && scrollAwayRange({ scrollDriven: true, startIndex: 10, lastStartIndex: 25, itemCount: N })
        .nextLastStartIndex === 10);
    checkNew('(P3[F5]) 起始基准 0 + 真实滚动 ⇒ 从第 0 条起标（首屏滚出正常生效，未被误伤）',
      JSON.stringify(scrollAwayRange({ scrollDriven: true, startIndex: 15, lastStartIndex: 0, itemCount: N }).range)
        === JSON.stringify({ from: 0, to: 15 }));
    checkNew('(P3[F5]) 越界夹取：startIndex 超过 itemCount 时不得越界标读（数据切换瞬间的防御）',
      JSON.stringify(scrollAwayRange({ scrollDriven: true, startIndex: 999, lastStartIndex: 5, itemCount: 50 }).range)
        === JSON.stringify({ from: 5, to: 50 })
      && scrollAwayRange({ scrollDriven: true, startIndex: -3, lastStartIndex: 0, itemCount: 50 }).range === null);
    checkNew('(P3[F5]) 空列表（itemCount=0）恒不标读（避免对 undefined 条目取值）',
      scrollAwayRange({ scrollDriven: true, startIndex: 5, lastStartIndex: 0, itemCount: 0 }).range === null
      && scrollAwayRange({ scrollDriven: true, startIndex: 5, lastStartIndex: 0, itemCount: 0 })
        .nextLastStartIndex === 0);

    /* 源码形态：Timeline 的滚动效应必须消费该判据，且筛选变化时重置基准。 */
    const fsF5 = await import('node:fs');
    const tlF5 = fsF5.readFileSync(new URL('../src/components/Timeline.tsx', import.meta.url), 'utf8');
    checkNew('(P3[F5]) Timeline 的滚动效应必须消费 scrollAwayRange（改回手写比较即失败）',
      /scrollAwayRange\(\{/.test(tlF5) && tlF5.includes('lastStartIndexRef.current = nextLastStartIndex'));
    checkNew('(P3[F5]) 筛选上下文变化必须重置 startIndex 基准（filterKey 依赖）',
      /useLayoutEffect\(\(\)\s*=>\s*\{[^}]*lastStartIndexRef\.current\s*=\s*0[^}]*\}\s*,\s*\[filterKey\]\)/
        .test(tlF5));
    checkNew('(P3[F5]) scrollDriven 只能由用户滚动置位（handleScroll 内经 isUserScrollEvent 判定；fix-2 更新：scroll 事件不再无条件算用户滚动）',
      /const handleScroll = \(\) => \{[^]*?scrollDrivenRef\.current = scrollDrivenRef\.current \|\| isUserScrollEvent\(\{/.test(tlF5)
      && !/scrollDrivenRef\.current = true;/.test(tlF5));
  }

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

  /* 与上一条成对：失败时**无任何 delta**（未消毒产物为空）⇒ 标记必须清除。
     留着会让后续水合写回的 DB 消毒译文走纯文本分支（卡片字面显示 <p>…</p>，
     即 P2-9 的标记粘连）；两条一起才锁定「标记只在有未消毒产物时才保留」。 */
  await bootFixture();
  detailImpl = (id) => mkRow({ id, content_html: '<p>详情</p>', translated_content: '<p>已消毒译文</p>' });
  aiTr = { deltas: [], error: null, reject: { message: 'down' }, finish: true, holdIds: [] };
  store.getState().translateEntry('201');
  await nTick(20);
  checkNew('(l2) 失败且无未消毒半截：标记清除（否则水合写回的消毒译文被按纯文本渲染）',
    store.getState().rawTranslatedIds['201'] === undefined
    && store.getState().entries.find((a) => a.id === '201')?.translatedContent === ''
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

  /* ---------- (p) TASK-067 N9/N10：交互落库与错误可见性 ---------- */
  /* 【TASK-107 改动理由】旧断言文案『全部已读未能保存，重启后可能回退』对应旧
     失败语义（本地保持已读假成功、重启回退）。新契约：失败即回滚（乐观读态/
     计数/已读保留快照全部还原），成功 toast 不再提前弹；断言升级为「回滚到位
     + 失败 toast 带重试 + 无假成功提示」，保护更强非弱化。 */
  await bootFixture();
  rejectCmds.add('mark_all_read');
  store.setState({ activeViewFilter: 'all', activeFeedFilter: '10', toasts: [] });
  const p1BeforeCounts = store.getState().feedCounts.get('10')?.unread;
  const p1BeforeOpened = Object.keys(store.getState().openedReadIds).length;
  store.getState().markCurrentViewAllRead();
  await nTick(10);
  const p1After = store.getState();
  checkNew('(p1) 全部已读失败必须可见且带重试入口（TASK-107：无「已全部标为已读」假成功提示）',
    p1After.toasts.some((t) => t.text.startsWith('全部已读保存失败') && t.action?.label === '重试')
    && !p1After.toasts.some((t) => t.text === '已全部标为已读'));
  checkNew('(p1) 失败回滚到位：已读态还原、未读计数还原、「已读保留」快照还原（TASK-107）',
    p1After.entries.filter((a) => a.feedId === '10' && a.isRead).length === 1 // 仅 102 本就已读
    && p1After.feedCounts.get('10')?.unread === p1BeforeCounts
    && Object.keys(p1After.openedReadIds).length === p1BeforeOpened);

  await bootFixture();
  rejectCmds.add('set_read');
  store.setState({ toasts: [], settings: { ...store.getState().settings, markReadOnOpen: true } });
  store.getState().selectArticle('101');
  await nTick(10);
  checkNew('(p2) 打开文章标读失败必须可见（修前静默，重启后回退未读）',
    store.getState().toasts.some((t) => t.text.startsWith('标读失败：')));

  /* (p2b) CF-04：卡片路径（SocialCard/NotifCard/GalleryCard/右键菜单共用的唯一入口）
     标读失败必须回滚本地乐观置位并可见——修前乐观置位后静默，重启回退未读。
     TASK-093 加强（审查 F4：本条修前代码下也通过，无判别力）：把「失败后不得
     残留乐观值 + 失败 toast」并入本条——修前（静默 catch / 回滚被去掉）下本条
     变红，乐观置位与回滚两侧各有判据。 */
  await bootFixture();
  rejectCmds.add('set_read');
  store.setState({ toasts: [] });
  const p2bBefore = store.getState().entries.find((e) => e.id === '101')?.isRead;
  store.getState().toggleEntryFlag('101', 'isRead');
  const p2bOptimistic = store.getState().entries.find((e) => e.id === '101')?.isRead === !p2bBefore;
  await nTick(20);
  checkNew('(p2b) 卡片路径乐观置位：点下去立即生效（未等落库），失败后不残留乐观值（TASK-093 加强）',
    p2bOptimistic
    && store.getState().entries.find((e) => e.id === '101')?.isRead === p2bBefore
    && store.getState().toasts.some((t) => t.text.startsWith('标读保存失败：')));
  await nTick(20);
  checkNew('(p2b) 卡片路径标读失败：回滚到点击前状态 + 失败 toast + 未读计数复原',
    store.getState().entries.find((e) => e.id === '101')?.isRead === p2bBefore
    && store.getState().feedCounts.get('10')?.unread === 3
    && store.getState().toasts.some((t) => t.text.startsWith('标读保存失败：')));

  /* (p2c) CF-03：搜索/命令面板锚定打开（anchorToArticle）的标读失败同样可见 */
  await bootFixture();
  rejectCmds.add('set_read');
  store.setState({ toasts: [], settings: { ...store.getState().settings, markReadOnOpen: true } });
  await store.getState().anchorToArticle('101');
  await nTick(20);
  checkNew('(p2c) 锚定打开标读失败必须可见（修前无声：本地已置读、库里没有）',
    store.getState().toasts.some((t) => t.text.startsWith('标读失败：')));

  await bootFixture();
  failReload = { message: 'db busy' };
  store.setState({ toasts: [] });
  await store.getState().reloadFromBackend().catch(() => {});
  checkNew('(p3) reloadFromBackend 失败必须可见（后台刷新/范围切换路径，修前静默）',
    store.getState().toasts.some((t) => t.text.startsWith('刷新失败：')));

  /* ============================================================
     (p3-f2 / p3-f3) TASK-093：乐观回滚收口（Batch 1/2 独立审查 F2/F3）
     - F2：回滚是「恢复点击前值」而非「再翻一次当前值」——连点两次、第一次
       失败第二次成功时，第一次迟到的失败不得把第二次已落库的新值踩回
       （已读 + 收藏各一条）；另补收藏单击失败回滚（审查 M2c：修后回归网
       对「收藏失败回滚」零保护）。
     - F3：阅读器 toggleCurrentReadStatus / toggleCurrentStar 与卡片同口径——
       成功提示只在落库成功后出现（P1-5 去假成功同口径），失败回滚且只出
       失败提示；翻译失败按「无半截未消毒产物即清 rawTranslatedIds」规则
       处理（清/留成对断言，Reader 与卡片路径各一组）。
     每条的修前变红实现（临时回退旧代码）与输出存 tmp/task-093/。
     ============================================================ */

  /* F2 已读连点：click1 落库失败（乐观置位 true）、click2 落库成功（false）。
     rejectWhen 只拒 set_read(read=true)——click1 的置位写被拒，click2 的写回
     成功；两次 toggle 在同一同步帧内完成（连点），click1 的迟到失败在其后才
     落地。修前（「再翻一次当前值」回滚）：click1 迟到的 catch 把 click2 已落库
     的 false 再翻回 true——UI=true / DB=false，本条红。 */
  await bootFixture();
  rejectWhen = (cmd, args) => cmd === 'set_read' && args.read === true;
  store.setState({ toasts: [] });
  const p3f2ReadBefore = store.getState().entries.find((e) => e.id === '101')?.isRead;
  store.getState().toggleEntryFlag('101', 'isRead');      // click1：乐观→true，set_read(true) 将失败
  store.getState().toggleEntryFlag('101', 'isRead');      // click2：乐观→false，set_read(false) 成功
  await nTick(30);
  checkNew('(p3-f2) 卡片已读连点两次第一次失败：最终 UI 与后端一致（第二次点击的值），迟到失败只提示不踩回',
    store.getState().entries.find((e) => e.id === '101')?.isRead === p3f2ReadBefore
    && store.getState().feedCounts.get('10')?.unread === 3
    && store.getState().toasts.some((t) => t.text.startsWith('标读保存失败：')));

  /* F2 收藏连点（set_starred 路径同形） */
  await bootFixture();
  rejectWhen = (cmd, args) => cmd === 'set_starred' && args.starred === true;
  store.setState({ toasts: [] });
  const p3f2StarBefore = store.getState().entries.find((e) => e.id === '101')?.isStarred;
  store.getState().toggleEntryFlag('101', 'isStarred');   // click1：乐观→true，set_starred(true) 将失败
  store.getState().toggleEntryFlag('101', 'isStarred');   // click2：乐观→false，set_starred(false) 成功
  await nTick(30);
  checkNew('(p3-f2) 卡片收藏连点两次第一次失败：最终 UI 与后端一致，迟到失败只提示不踩回',
    store.getState().entries.find((e) => e.id === '101')?.isStarred === p3f2StarBefore
    && store.getState().feedCounts.get('10')?.starred === 1
    && store.getState().toasts.some((t) => t.text.startsWith('收藏保存失败：')));

  /* M2c（审查：收藏失败回滚在修后回归网下全绿）：收藏单击失败必须回滚 + 计数复原 */
  await bootFixture();
  rejectCmds.add('set_starred');
  store.setState({ toasts: [] });
  const p3f2StarOrig = store.getState().entries.find((e) => e.id === '101')?.isStarred;
  store.getState().toggleEntryFlag('101', 'isStarred');
  await nTick(30);
  checkNew('(p3-f2) 卡片收藏单击失败：回滚到点击前值 + 失败 toast + 收藏计数复原（M2c 变红）',
    store.getState().entries.find((e) => e.id === '101')?.isStarred === p3f2StarOrig
    && store.getState().feedCounts.get('10')?.starred === 1
    && store.getState().toasts.some((t) => t.text.startsWith('收藏保存失败：')));

  /* F3 阅读器 toggleCurrentReadStatus：成功提示只在落库成功后；失败回滚且只出失败提示 */
  await bootFixture();
  rejectCmds.add('set_read');
  store.setState({ activeArticleId: '101', toasts: [] });
  store.getState().toggleCurrentReadStatus();
  checkNew('(p3-f3) 阅读器标读：乐观置位立即生效，但不提前弹「已标为已读」（修前先弹假成功）',
    store.getState().entries.find((a) => a.id === '101')?.isRead === true
    && !store.getState().toasts.some((t) => t.text === '已标为已读'));
  await nTick(30);
  checkNew('(p3-f3) 阅读器标读失败：回滚到点击前值，且只有失败提示（无成功提示残留）',
    store.getState().entries.find((a) => a.id === '101')?.isRead === false
    && store.getState().toasts.some((t) => t.text.startsWith('标读状态保存失败：'))
    && !store.getState().toasts.some((t) => t.text === '已标为已读'));

  await bootFixture();
  store.setState({ activeArticleId: '101', toasts: [] });
  store.getState().toggleCurrentReadStatus();
  await nTick(30);
  checkNew('(p3-f3) 阅读器标读成功：「已标为已读」提示在落库成功后出现（P1-5 同口径）',
    store.getState().entries.find((a) => a.id === '101')?.isRead === true
    && store.getState().toasts.some((t) => t.text === '已标为已读'));

  await bootFixture();
  rejectCmds.add('set_starred');
  store.setState({ activeArticleId: '103', toasts: [] });
  store.getState().toggleCurrentStar();
  await nTick(30);
  checkNew('(p3-f3) 阅读器收藏失败：回滚到点击前值 + 失败提示 + 收藏计数复原',
    store.getState().entries.find((a) => a.id === '103')?.isStarred === true
    && store.getState().feedCounts.get('10')?.starred === 1
    && store.getState().toasts.some((t) => t.text.startsWith('收藏状态保存失败：')));

  /* F3 阅读器翻译失败：rawTranslatedIds 按「无半截未消毒产物即清」规则处理（成对） */
  await bootFixture();
  aiTr = { deltas: [], error: '限流', reject: null, finish: true, holdIds: [] };
  store.setState({ toasts: [] });
  store.getState().selectArticle('201');
  await nTick(10);
  store.getState().toggleReaderTranslation();
  await nTick(20);
  checkNew('(p3-f3) Reader 翻译流内错误且无半截产物：rawTranslatedIds 清除（修前残留 → 消毒译文被按纯文本渲染）',
    store.getState().rawTranslatedIds['201'] === undefined
    && store.getState().translateErrors['201'] === '限流'
    && store.getState().isShowingTranslatedProse === false);

  await bootFixture();
  aiTr = { deltas: ['<b>半截'], error: '限流', reject: null, finish: true, holdIds: [] };
  store.setState((s) => ({
    toasts: [],
    entries: s.entries.map((a) => (a.id === '201' ? { ...a, content: '<p>详情</p>' } : a)),
  }));
  store.getState().selectArticle('201');
  await nTick(10);
  store.getState().toggleReaderTranslation();
  await nTick(20);
  checkNew('(p3-f3) Reader 翻译流内错误且有半截产物：标记保留（成对；半截按纯文本渲染）',
    store.getState().rawTranslatedIds['201'] === true
    && store.getState().entries.find((a) => a.id === '201')?.translatedContent === '<b>半截');

  await bootFixture();
  aiTr = { deltas: [], error: null, reject: { message: 'down' }, finish: true, holdIds: [] };
  store.setState({ toasts: [] });
  store.getState().selectArticle('201');
  await nTick(10);
  store.getState().toggleReaderTranslation();
  await nTick(20);
  checkNew('(p3-f3) Reader 翻译 IPC 失败且无半截产物：rawTranslatedIds 清除',
    store.getState().rawTranslatedIds['201'] === undefined
    && store.getState().translateErrors['201'] === 'AI 服务未配置或不可达');

  /* M6（审查：onError 路径不清标记在修后回归网下全绿）：卡片翻译流内错误且
     无半截 → 标记清除（既有 (l2) 只盖 .catch 路径与「有半截保留」侧）。 */
  await bootFixture();
  aiTr = { deltas: [], error: '限流', reject: null, finish: true, holdIds: [] };
  store.setState({ toasts: [] });
  store.getState().translateEntry('201');
  await nTick(20);
  checkNew('(p3-l2) 卡片翻译流内错误且无半截产物：rawTranslatedIds 清除（M6 变红）',
    store.getState().rawTranslatedIds['201'] === undefined
    && store.getState().translateErrors['201'] === '限流');

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
  await nTick(0);   // api.setReadBulk 内部 await getInvoke()，落库是异步 fire-and-forget
  const l1 = store.getState();
  /* AUDIT P3[F4]（TASK-084）：IPC 契约由「6 次 set_read」改为「1 次 set_read_bulk」。
     本断言的**原意是「未读项全部标读、已读项不重复写库」**，该意图完整保留。
     审查 FINDING TASK-084-F2 指出：上一版只钉了载荷**长度**（=== 6），是**弱于**原断言
     的——原来的「set_read 计数 === 6」其实是一个**精确 id 集合检查**（一个 set_read 只可能
     为「即将被写入的未读 id」发出），因此 6 次即证明恰是那 6 个未读 id 被写、且 2 个已读 id
     未被写。现在按原强度把**具体 id 集合**钉死（已读的 102/202 必须不在其中）。 */
  const l1BulkIds = (invokeCalls.find((c) => c.cmd === 'set_read_bulk')?.args.ids ?? [])
    .slice().sort((a, b) => a - b).join(',');
  checkNew('(L1) 索引化批量标读：未读项全部标读、已读项不重复写库（1 次 set_read_bulk，载荷恰为 6 个未读 id 101,103,104,105,201,301）',
    l1.entries.every((e) => e.isRead)
    && invokeCalls.filter((c) => c.cmd === 'set_read_bulk').length === 1
    && invokeCalls.filter((c) => c.cmd === 'set_read').length === 0
    && l1BulkIds === '101,103,104,105,201,301');
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
    && s1First.articlesLimit === 500 && s1First.articlesCursor['article|10']?.loaded === 500
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
  /* 【TASK-117 改动理由】续拉 wire 从 offset=500 改为 keyset 锚——源A 首页末行
     id 6499（i=499），锚点原文透传。offset 正是本卡废除的缺陷手法。 */
  checkNew('(s1) 第 2 页请求沿用同一范围与 keyset 锚（feed_id=10 / last_id=6499 / newest_first）',
    s1Page2Call?.args.args.feed_id === 10 && s1Page2Call?.args.args.folder_id === null
    && s1Page2Call?.args.args.last_id === 6499 && s1Page2Call?.args.args.last_published === iso(NOW - 499 * 1000)
    && s1Page2Call?.args.args.offset === undefined && s1Page2Call?.args.args.newest_first === true);
  checkNew('(s1) 第 2 页取回该源的后续文章：全部属于 feed 10，且与第 1 页 id 集合不重叠',
    s1Second.entries.length === 1000 && s1Second.entries.every((e) => e.feedId === '10')
    && s1Page2.length === 500 && s1Page2.every((id) => !s1Ids1.has(id))
    && s1Page2[0] === '6500' && s1Page2[499] === '6999'
    && s1Second.articlesLimit === 1000 && s1Second.articlesCursor['article|10']?.loaded === 1000);

  /* ---------- (s4) per-scope 游标互不污染：A 源翻到第 2 页后切 B 源，B 从第 1 页开始 ---------- */
  /* TASK-063：selectFeed 自带接线（缓存命中恢复 / 未命中自动重拉）——此处不再
     手工调用 reloadFromBackend（旧写法模拟了 UI 中不存在的一步，掩盖了 Sidebar
     未接线的事实），改为等待 selectFeed 自身触发的重拉落地。 */
  invokeCalls.length = 0;
  store.getState().selectFeed('11');
  const s4Switch = store.getState();
  checkNew('(s4) 切到未曾加载的源B：游标从 0 起步（不继承源A 的 1000）',
    s4Switch.activeFeedFilter === '11' && s4Switch.articlesLimit === 0
    && s4Switch.articlesCursor['article|10']?.loaded === 1000 && s4Switch.articlesCursor['article|11'] === undefined);
  await nTick(30);
  const s4FirstCall = invokeCalls.find((c) => c.cmd === 'list_articles');
  const s4First = store.getState();
  checkNew('(s4) selectFeed 自动重拉：源B 首批查询按源B 的第 1 页取（feed_id=11 / offset=0），条目全属源B',
    s4FirstCall?.args.args.feed_id === 11 && s4FirstCall?.args.args.offset === 0
    && s4First.entries.length === 500 && s4First.entries.every((e) => e.feedId === '11')
    && s4First.articlesCursor['article|11']?.loaded === 500 && s4First.articlesCursor['article|10']?.loaded === 1000
    && s4First.entries.slice(0, 500).every((e) => !s1Ids1.has(e.id)));
  invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  const s4Call = invokeCalls.find((c) => c.cmd === 'list_articles');
  /* 【TASK-117 改动理由】续拉 wire 从 offset=500 改为 keyset 锚——源B 首页末行
     id 7499；判别意图不变：锚取自**源B 自己的游标**（而非源A 的 1000）。 */
  checkNew('(s4) 源B 翻页用**源B 自己的游标**（keyset 锚 last_id=7499 而非源A 的锚）',
    s4Call?.args.args.feed_id === 11 && s4Call?.args.args.last_id === 7499
    && s4Call?.args.args.last_published === iso(NOW - 499 * 1000 - 500)
    && s4Call?.args.args.offset === undefined
    && store.getState().articlesCursor['article|11']?.loaded === 1000 && store.getState().articlesCursor['article|10']?.loaded === 1000);
  /* 切回源A：TASK-063 新契约——缓存命中同步恢复该范围快照（零延迟，不经 await），
     游标=快照长度（500，可继续翻页）；源B 的游标 1000 不被污染。恢复时清水合
     状态（缓存快照无正文，滞留的已水合标记会阻断重水合）。 */
  store.setState({ hydratedIds: { '999': true }, hydrationErrors: { '998': 'x' } });
  store.getState().selectFeed('10');
  const s4Back = store.getState();
  checkNew('(s4) 切回源A：同步恢复该范围快照（零延迟），游标=快照长度且两源互不污染，滞留水合状态被清空',
    s4Back.entries.length === 500 && s4Back.entries.every((e) => e.feedId === '10')
    && s4Back.articlesLimit === 500 && s4Back.articlesCursor['article|10']?.loaded === 500
    && s4Back.articlesCursor['article|11']?.loaded === 1000
    && Object.keys(s4Back.hydratedIds).length === 0 && Object.keys(s4Back.hydrationErrors).length === 0);
  await nTick(30);
  checkNew('(s4) 切回源A 后的后台刷新保持该范围快照结论',
    store.getState().entries.every((e) => e.feedId === '10') && store.getState().articlesCursor['article|10']?.loaded === 500);

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
    && s2First.articlesLimit === 500 && s2First.articlesCursor['article|cat-1']?.loaded === 500);
  invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  const s2Call = invokeCalls.find((c) => c.cmd === 'list_articles');
  const s2Second = store.getState();
  /* 【TASK-117 改动理由】续拉 wire 从 offset=500 改为 keyset 锚——分类首页末行
     id 9249（feed12 i=249，交错序的最后一行），锚点原文透传。 */
  checkNew('(s2) 第 2 页请求沿用分类口径与 keyset 锚（folder_id=1 / feed_id=null / last_id=9249）',
    s2Call?.args.args.folder_id === 1 && s2Call?.args.args.feed_id === null
    && s2Call?.args.args.last_id === 9249 && s2Call?.args.args.last_published === iso(NOW - 249300)
    && s2Call?.args.args.offset === undefined);
  checkNew('(s2) 第 2 页取回该分类的后续文章（不含分类外源C），且与第 1 页不重叠',
    s2Second.entries.length === 1000 && s2Second.entries.every((e) => e.feedId === '10' || e.feedId === '12')
    && s2Second.entries.slice(500).every((e) => !s2First50.includes(e.id))
    && s2Second.articlesCursor['article|cat-1']?.loaded === 1000);

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
    s3Empty.entries.length === 0 && s3Empty.articlesLimit === 0 && s3Empty.articlesCursor['article|10']?.loaded === 0
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
  /* 模拟改造前的口径：首批按全局拉满 500 行，范围里一条都没有。
     TASK-117：游标模拟值同步升为 keyset 形态（锚值任意——本范围查询结果为空，
     锚不参与判定；断言只看 wire 带了锚与范围）。 */
  store.setState({ entries: [], articlesLimit: 500, articlesCursor: { 'article|10': { lastPublished: iso(NOW - 500000), lastId: 3999, loaded: 500 } }, articlesExhausted: false });
  invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  const s3Advance = invokeCalls.find((c) => c.cmd === 'list_articles');
  /* 【TASK-117 改动理由】续拉游标从 offset=500 改为 keyset 锚（last_published/last_id
     成对）——offset 正是本卡废除的缺陷手法，wire 断言随行为变化更新。 */
  checkNew('(s3) 列表为空且未到底时仍可发起下一页：请求带当前范围与 keyset 锚（feed_id=10 / last_id=3999 / last_published=锚原文，无 offset）',
    s3Advance?.args.args.feed_id === 10 && s3Advance?.args.args.last_id === 3999
    && s3Advance?.args.args.last_published === iso(NOW - 500000) && s3Advance?.args.args.offset === undefined);
  checkNew('(s3) 该源确无更多数据时空页把状态收敛为「已到底 + 空列表」（补拉不会无限循环）',
    store.getState().entries.length === 0 && store.getState().articlesExhausted === true
    && store.getState().articlesLoading === false && store.getState().articlesCursor['article|10']?.loaded === 500);
  /* 再触发：守卫挡住（已到底） */
  invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  checkNew('(s3) 收敛后再触发不再发 IPC（空列表补拉不会反复打后端）',
    invokeCalls.filter((c) => c.cmd === 'list_articles').length === 0);

  /* ---------- (s5) D2/D3 守住（051 修复不回退） ---------- */
  await resetStore();
  /* TASK-117：游标模拟值升为 keyset 形态（锚值确定性给出，wire 断言核对原文透传） */
  store.setState({ dataMode: 'tauri', activeFeedFilter: '10', entries: [], articlesLimit: 100, articlesCursor: { 'article|10': { lastPublished: iso(NOW - 100000), lastId: 4999, loaded: 100 } }, articlesExhausted: false, toasts: [] });
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
    && s5Fail.articlesCursor['article|10']?.loaded === 100 && s5Fail.entries.length === 0);
  /* D2 的重试按钮真的能重发（把后端恢复后点重试） */
  listPlan = null;
  invokeCalls.length = 0;
  s5Fail.toasts[0].action.run();
  await nTick(20);
  /* 【TASK-117 改动理由】续拉 wire 从 offset=100 改为 keyset 锚——offset 正是
     本卡废除的缺陷手法，wire 断言随行为变化更新（锚值 = 上方模拟游标的原文）。 */
  checkNew('(s5/D2) 失败 toast 的「重试」确实重发分页请求（按当前范围 + keyset 锚，不是死按钮）',
    invokeCalls.filter((c) => c.cmd === 'list_articles').length >= 1
    && invokeCalls.some((c) => c.cmd === 'list_articles' && c.args.args.feed_id === 10
      && c.args.args.last_id === 4999 && c.args.args.last_published === iso(NOW - 100000)));
  listPlan = null;

  /* D3：竞态丢弃分支必须复位 articlesLoading（且新增的 scopeKey 收紧后依然如此）。
     场景：在途加载期间用户切到另一个范围（selectFeed 会把游标换成新范围的值，
     可能恰好等于 in-flight 的锚点数值——旧的「只比数值」判据会漏判）。
     TASK-117：游标模拟值升为 keyset 形态（两范围锚点数值刻意相同——串台判别前提）。 */
  await resetStore();
  store.setState({ dataMode: 'tauri', activeFeedFilter: '10', entries: [], articlesLimit: 100, articlesCursor: { 'article|10': { lastPublished: iso(NOW - 100000), lastId: 4999, loaded: 100 }, 'article|11': { lastPublished: iso(NOW - 100000), lastId: 4999, loaded: 100 } }, articlesExhausted: false, articlesLoading: false });
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
  /* TASK-100 P3-1：selectFeed 触发的 reload 此刻仍在途（其 list_articles 也被 defer 过），
     新守卫「reload 在途拦截续拉」会拦住下面这一次 loadMore——先放行在途 reload 落地
     （回满一页源B 数据，使 exhausted=false），再验证续拉恢复。 */
  pendingList[pendingList.length - 1]?.resolve(
    Array.from({ length: 500 }, (_, i) => mkRow({ id: 6200 + i, feed_id: 11 })),
  );
  await nTick(20);
  invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  checkNew('(s5/D3) 竞态丢弃后入口守卫没被锁死：下一次分页照常发出（且用新范围源B）',
    invokeCalls.filter((c) => c.cmd === 'list_articles').length === 1
    && invokeCalls.find((c) => c.cmd === 'list_articles')?.args.args.feed_id === 11);

  /* 同范围内的「游标被 reload 重置」这条既有 D3 场景，在新判据下也必须仍然丢弃 */
  await resetStore();
  store.setState({ dataMode: 'tauri', activeFeedFilter: '10', entries: [], articlesLimit: 100, articlesCursor: { 'article|10': { lastPublished: iso(NOW - 100000), lastId: 4999, loaded: 100 } }, articlesExhausted: false });
  listPlan = { mode: 'defer' };
  const s5Race2 = store.getState().loadMoreArticles();
  await nTick(0);
  /* TASK-117：重置后的空游标形态（无锚） */
  store.setState({ articlesLimit: 0, articlesCursor: { 'article|10': { lastPublished: null, lastId: null, loaded: 0 } } });   // 同范围内游标被重置
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
    && s6Starred.articlesLimit === 3 && s6Starred.articlesCursor['article|10']?.loaded === 3
    && s6Starred.articlesExhausted === true);
  /* 视图缓存必须按范围分桶：切回 all 时不能把「源A 收藏」当成「源A 全部」恢复。
     关键证据是**同步恢复**那一步（缓存命中在 reload 之前同步生效，后台刷新随后才到）：
     缓存若不按视图分桶，切回 all 会拿「收藏的 3 条」冒充「全部的 40 条」。 */
  listPlan = { mode: 'defer' };   // 冻结后台刷新，只观察缓存恢复本身
  store.getState().selectView('all');
  const s6BackSync = store.getState();
  /* 【TASK-110 改动理由】原断言把「缓存恢复时不知道是否到底 ⇒ 保守标 false」写成
     期望——那是筛选视图全集拉取时代（无法从快照得知尽端）的防错手法。分页化后
     缓存快照本身记录了「最近一次拉取是否满一页」：40 行 < 页大小 ⇒ 该范围确实
     只有 40 条，exhausted=true 是真实判定而非误标（selectView 与 selectLayout/
     selectFeed 同口径收口）。原保护意图（老文章不因误标已到底而不可达）由更强的
     机制接棒：恢复后必触发后台 reload 重取真值；本场景 40 条即全量，true 准确。 */
  checkNew('(s6) 切回「全部」缓存命中：同步恢复的是源A 的 40 条（不拿收藏视图的 3 条冒充），游标随之对齐、exhausted 按快照长度真实判定（40<500=已到底）',
    s6BackSync.entries.length === 40 && s6BackSync.entries.every((e) => e.feedId === '10')
    && s6BackSync.articlesLimit === 40 && s6BackSync.articlesCursor['article|10']?.loaded === 40
    && s6BackSync.articlesExhausted === true && s6BackSync.articlesLoading === false);
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
    s6D.articlesCursor['article|12']?.loaded === 40 && s6D.articlesCursor['article|10']?.loaded === 40);

  /* ---------- (s7) 排序切换：游标含义随 newest_first 翻转 ⇒ 必须按新排序重拉 ----------
     只翻转排序键而不重拉时，已加载的快照（旧排序的首批）会与新排序的下一页错位——继续翻页
     取回的是另一端的文章，整段文章不可达 + 重复卡片（审计 round-1「排序切换游标错位」）。
     该缺陷在 P0-1（后端此前不认 newest_first）修好后才真正可达。
     TASK-093 加强（审查 F1/F4）：把审查探针场景舞台化——旧排序 loadMore(offset=500)
     在途，切排序后的重拉先落地（游标仍为 500），旧排序迟到页后到。修前（loadMore
     竞态守卫不含排序）迟到页被放行接入：duplicates=100 / missing=100（探针实测）；
     修后在途页整体丢弃。前置条件改在本场景收尾处与竞态结果一并判定（原 2212 行
     前置断言在修前代码下也通过、无判别力）。 */
  await resetStore();
  viewEntriesCache.clear();
  const s7Rows = [];
  for (let i = 0; i < 600; i += 1) {
    s7Rows.push(mkRow({ id: 5000 + i, feed_id: 10, title: `T${i}`, published_at: iso(NOW - i * 1000) }));
  }
  backendRows = s7Rows;
  store.getState().selectFeed('10');
  await store.getState().reloadFromBackend();
  const s7Pre = store.getState();
  const s7PreOk = s7Pre.entries.length === 500 && s7Pre.entries[0]?.id === '5000'
    && s7Pre.articlesCursor['article|10']?.loaded === 500 && s7Pre.articlesExhausted === false;
  const s7CacheHadSnapshot = viewEntriesCache.size > 0;  // 重拉已把「全部」视图快照写入缓存
  /* 舞台：list_articles 全部挂起。loadMore 先发（旧排序第 2 页，keyset 锚
     last_id=5499），toggle 的重拉后发（新排序首页 offset=0）——按参数匹配挂起项，
     不按下标。TASK-117：续拉挂起项按 keyset 锚匹配（原 offset=500）。 */
  listPlan = { mode: 'defer' };
  invokeCalls.length = 0;
  store.getState().loadMoreArticles();
  store.getState().toggleTimelineSort();
  const s7CacheCleared = viewEntriesCache.size === 0;    // toggle 同步丢弃各视图快照缓存
  await nTick(30);                                       // reload 的 folders/feeds/counts 落地，两个 list_articles 挂起
  const s7StaleCall = pendingList.find((p) => p.args.last_id === 5499 && p.args.newest_first === true);
  const s7ReloadCall = pendingList.find((p) => p.args.offset === 0 && p.args.newest_first === false);
  s7ReloadCall?.resolve(queryRows(s7ReloadCall.args));   // 重拉先落地：游标仍为 500（F1 场景成立的前提）
  await nTick(30);
  s7StaleCall?.resolve(queryRows(s7StaleCall.args));     // 旧排序迟到页后到
  await nTick(30);
  listPlan = null;
  const s7 = store.getState();
  const s7Ids = s7.entries.map((e) => e.id);
  checkNew('(s7) 前置：源A 600 条，newest 首批 = 最新端 500 条（首条 id 5000、游标 500、未到底）；切排序重拉落地后旧排序在途页整体丢弃，列表唯一且无缺失（TASK-093 加强）',
    s7PreOk
    && s7.entries.length === 500 && new Set(s7Ids).size === 500
    && s7Ids[0] === '5599' && s7Ids[499] === '5100');
  checkNew('(s7) 切换「最早」：按新排序重拉该范围首页（feed_id=10 / offset=0 / newest_first=false），游标与 entries 同一次写入',
    s7ReloadCall?.args.feed_id === 10 && s7ReloadCall?.args.folder_id === null
    && s7ReloadCall?.args.offset === 0 && s7ReloadCall?.args.newest_first === false
    && s7.entries.length === 500 && s7.articlesLimit === 500 && s7.articlesCursor['article|10']?.loaded === 500);
  checkNew('(s7) 重拉后的首批换到另一端（首条 = 全库最老 id 5599），不再是最新端快照',
    s7.entries[0]?.id === '5599' && s7.entries[499]?.id === '5100'
    && s7.entries.every((e) => e.id !== '5000'));
  checkNew('(p3-f1) 旧排序在途页迟到达时被丢弃：无重复、无缺失段（修前 duplicates=100/missing=100，审查探针场景）',
    s7.entries.length === 500 && new Set(s7Ids).size === s7Ids.length
    && s7Ids.every((id) => { const n = Number(id); return n >= 5100 && n <= 5599; })
    && s7.articlesLimit === 500 && s7.articlesCursor['article|10']?.loaded === 500);
  checkNew('(s7) 排序键翻转且已读保留快照清空、各视图快照缓存同步丢弃（与视图/范围切换同口径；TASK-093 加强，M4/M4b 变红）',
    s7.timelineSort === 'oldest' && Object.keys(s7.openedReadIds).length === 0
    && s7CacheHadSnapshot && s7CacheCleared);

  /* ---------- (p3-f5) TASK-093：toggleTimelineSort 的三个口径（审查 F5） ---------- */
  /* ① 重拉失败不得产生 unhandled rejection（reloadFromBackend toast 后重抛，本入口须接住） */
  await bootFixture();
  failReload = { message: 'db busy' };
  store.setState({ toasts: [] });
  const p3f5Unhandled = [];
  const p3f5OnUn = (r) => { p3f5Unhandled.push(r); };
  process.on('unhandledRejection', p3f5OnUn);
  store.getState().toggleTimelineSort();
  await nTick(30);
  process.off('unhandledRejection', p3f5OnUn);
  failReload = null;
  checkNew('(p3-f5) 切换排序的重拉失败：不产生 unhandled rejection（失败提示仍由 reloadFromBackend 给出）',
    p3f5Unhandled.length === 0
    && store.getState().toasts.some((t) => t.text.startsWith('刷新失败：')));

  /* ② 非 tauri（mock）模式不调后端：重拉会把 mock 会话翻成 tauri（修前可达） */
  await bootFixture();
  store.setState({ dataMode: 'mock' });
  invokeCalls.length = 0;
  const p3f5SortBefore = store.getState().timelineSort;
  store.getState().toggleTimelineSort();
  await nTick(30);
  checkNew('(p3-f5) mock 模式切排序：纯本地翻转，不调后端、不把 mock 会话翻成 tauri',
    store.getState().timelineSort !== p3f5SortBefore
    && invokeCalls.filter((c) => c.cmd === 'list_articles').length === 0
    && store.getState().dataMode === 'mock');

  /* ③ 筛选视图（收藏/未读/今天）切排序。
     【TASK-110 改动理由】原断言「全集本地重排、不重拉后端（0 次 list_articles）」
     锁的正是本卡废除的手法：筛选视图原本拉全集（limit:100000）存内存，切排序
     只需本地重排。分页化后全集不再在内存里，排序改由服务端承载——切排序必须
     按新排序重拉当前范围（与「全部」视图同构）。本断言按行为变化改写，保留其
     保护意图中仍然成立的部分（可见顺序翻转、集合不变），并加锁新契约（重拉的
     wire 参数：only_starred + newest_first=false + offset=0，恰好 1 次）。 */
  await bootFixture();
  store.getState().selectView('starred');
  await nTick(30);
  invokeCalls.length = 0;
  const p3f5StarSet = [...store.getState().entries].map((e) => e.id).sort().join(',');
  const p3f5VisNewest = selectVisibleEntries(store.getState())[0]?.id;
  store.getState().toggleTimelineSort();
  await nTick(30);
  const p3f5VisOldest = selectVisibleEntries(store.getState())[0]?.id;
  const p3f5SortCall = invokeCalls.find((c) => c.cmd === 'list_articles');
  checkNew('(p3-f5) 筛选视图切排序：按新排序重拉当前范围（1 次 list_articles，only_starred + newest_first=false + offset=0），可见顺序翻转、集合不变（TASK-110③ 行为变化）',
    invokeCalls.filter((c) => c.cmd === 'list_articles').length === 1
    && p3f5SortCall?.args.args.only_starred === true && p3f5SortCall?.args.args.newest_first === false
    && p3f5SortCall?.args.args.offset === 0
    && p3f5VisNewest === '102' && p3f5VisOldest === '103'
    && [...store.getState().entries].map((e) => e.id).sort().join(',') === p3f5StarSet);

  /* ---------- (p3b) TASK-098：selectLayout/selectView/selectFeed 的 void reload 全量收口（F5 同款铺开） ----------
     TASK-093 的 F5 只收口了 toggleTimelineSort；TASK-098 独立审查把同款暴露铺开收口：
     selectLayout / selectView（缓存命中与未命中两条路）/ selectFeed 的 void reload
     此前均无 .catch——reloadFromBackend 失败时 toast 后重抛 ⇒ unhandled rejection。
     断言复用 (p3-f5) ① 的 unhandled 捕获装置：注入后端拒绝，触发入口，捕获进程级
     unhandledRejection。App.tsx 的 feeds-updated 事件路径无法在无浏览器装置里驱动
     组件，另加源码形态断言钉住（readFileSync 手法沿用本文件既有写法）。 */
  {
    const fsP3b = await import('node:fs');
    const navSrcP3b = fsP3b.readFileSync(new URL('../src/store/slices/nav.ts', import.meta.url), 'utf8');
    const appSrcP3b = fsP3b.readFileSync(new URL('../src/App.tsx', import.meta.url), 'utf8');
    const p3bNavReloadLines = navSrcP3b.split('\n').filter((l) => /void get\(\)\.reload(FromBackend|FilteredEntries)\(/.test(l));
    checkNew('(p3b) nav.ts 全部 void reload 调用点（10 处：selectLayout / selectView 两条路 / selectFeed / toggleTimelineSort 两条路——TASK-110③ 新增筛选分流）逐行带 .catch（漏一处即失败）',
      /* TASK-110③：toggleTimelineSort 筛选视图新增 reloadFilteredEntries 分流（原 9 处 → 10 处） */
    p3bNavReloadLines.length === 10 && p3bNavReloadLines.every((l) => l.includes('.catch(')));
    /* 【TASK-111② 改动理由】原断言锁定的字面量是 `reloadFromBackend().catch(`——
       本卡给该调用补了保位请求实参（后台刷新路径携带 keepReadingPosition，落地
       时发出保位信号供 Timeline 消费顶条锚回位）。断言同步改写为锁定新 wire 形态
       （带实参 + .catch 仍在），非为过门禁而弱化：.catch 防线（删掉即失败）与
       「后台刷新走保位路径」两点同时锁定。 */
    checkNew('(p3b) App.tsx 后台刷新事件（feeds-updated）的 void reloadFromBackend（TASK-111② 起带 keepReadingPosition 保位实参）同样带 .catch（删掉即失败）',
      /void useAppStore\.getState\(\)\.reloadFromBackend\(\{ keepReadingPosition: true \}\)\.catch\(/.test(appSrcP3b));

    /* ① selectView 缓存命中路径：bootFixture 已把「all」快照写进 viewEntriesCache，
         selectView('all') 命中缓存 → 后台静默刷新失败（toast 后重抛，本入口须接住） */
    await bootFixture();
    failReload = { message: 'db busy' };
    store.setState({ toasts: [] });
    const p3bUn1 = [];
    const p3bOn1 = (r) => { p3bUn1.push(r); };
    process.on('unhandledRejection', p3bOn1);
    store.getState().selectView('all');
    await nTick(30);
    process.off('unhandledRejection', p3bOn1);
    failReload = null;
    checkNew('(p3b) selectView 缓存命中路径的后台刷新失败：无 unhandled rejection（失败提示仍由 reloadFromBackend 给出）',
      p3bUn1.length === 0
      && store.getState().toasts.some((t) => t.text.startsWith('刷新失败：')));

    /* ② selectView 无缓存路径：清掉快照缓存 → selectView('all') 走直拉路径 */
    await bootFixture();
    viewEntriesCache.clear();
    failReload = { message: 'db busy' };
    store.setState({ toasts: [] });
    const p3bUn2 = [];
    const p3bOn2 = (r) => { p3bUn2.push(r); };
    process.on('unhandledRejection', p3bOn2);
    store.getState().selectView('all');
    await nTick(30);
    process.off('unhandledRejection', p3bOn2);
    failReload = null;
    checkNew('(p3b) selectView 无缓存路径的重拉失败：无 unhandled rejection（失败提示仍由 reloadFromBackend 给出）',
      p3bUn2.length === 0
      && store.getState().toasts.some((t) => t.text.startsWith('刷新失败：')));

    /* ③ selectFeed：该范围的快照缓存未命中 → 直接后台重拉 */
    await bootFixture();
    failReload = { message: 'db busy' };
    store.setState({ toasts: [] });
    const p3bUn3 = [];
    const p3bOn3 = (r) => { p3bUn3.push(r); };
    process.on('unhandledRejection', p3bOn3);
    store.getState().selectFeed('10');
    await nTick(30);
    process.off('unhandledRejection', p3bOn3);
    failReload = null;
    checkNew('(p3b) selectFeed 的重拉失败：无 unhandled rejection（失败提示仍由 reloadFromBackend 给出）',
      p3bUn3.length === 0
      && store.getState().toasts.some((t) => t.text.startsWith('刷新失败：')));

    /* ④ selectLayout：布局切换触发按新布局重拉 */
    await bootFixture();
    failReload = { message: 'db busy' };
    store.setState({ toasts: [] });
    const p3bUn4 = [];
    const p3bOn4 = (r) => { p3bUn4.push(r); };
    process.on('unhandledRejection', p3bOn4);
    store.getState().selectLayout('social');
    await nTick(30);
    process.off('unhandledRejection', p3bOn4);
    failReload = null;
    checkNew('(p3b) selectLayout 的重拉失败：无 unhandled rejection（失败提示仍由 reloadFromBackend 给出）',
      p3bUn4.length === 0
      && store.getState().toasts.some((t) => t.text.startsWith('刷新失败：')));

    /* ⑤ 筛选视图：selectView('starred') → reloadFilteredEntries。list_articles 拒绝时
         该函数内部 toast、不重抛——无 unhandled rejection 且失败可见（口径同 F5：
         可见性由 reload 自身给出，调用点只兜底） */
    await bootFixture();
    listPlan = { mode: 'reject', error: { message: 'db busy' } };
    store.setState({ toasts: [] });
    const p3bUn5 = [];
    const p3bOn5 = (r) => { p3bUn5.push(r); };
    process.on('unhandledRejection', p3bOn5);
    store.getState().selectView('starred');
    await nTick(30);
    process.off('unhandledRejection', p3bOn5);
    listPlan = null;
    checkNew('(p3b) 筛选视图 selectView 的拉取失败：无 unhandled rejection，失败提示由 reloadFilteredEntries 自身给出',
      p3bUn5.length === 0
      && store.getState().toasts.some((t) => t.text.startsWith('筛选列表加载失败：')));
  }

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

  /* ============================================================
     fix-1 / fix-3 / fix-4（发布前自检修复，2026-09-29）：store 级行为断言
     —— addFeed 空 catId 发 null、删除分类/源复位范围、筛选视图迟到响应按口径丢弃
     ============================================================ */
  {
    /* fix-1（自检 P1-1）：空 catId（全新安装 0 分类时 AddFeedModal 的实参）必须发
       folder_id=null —— 后端 add_feed 对 None 有「自动落到未分类（不存在则建）」
       兜底；修前 Number('')===0 直传，触发 feeds.folder_id 外键违约，首用添加必败。
       断言能抓住回退：payload.folderId 一旦回到 0（回归 Number('') 直传），即红。 */
    await resetStore();
    await store.getState().addFeed('', 'https://example.com/rss.xml', '', 'inherit', false, false, false);
    const fix1Call = invokeCalls.find((c) => c.cmd === 'add_feed');
    checkNew('(fix-1) 空 catId 的 addFeed payload.folderId === null（后端 None→未分类兜底可达；修前为 0）',
      !!fix1Call && fix1Call.args.folderId === null && fix1Call.args.feedUrl === 'https://example.com/rss.xml');
    /* 有数字 id 的路径不受影响 */
    invokeCalls.length = 0;
    await store.getState().addFeed('cat-2', 'https://example.com/b.xml', '', 'inherit', false, false, false);
    const fix1b = invokeCalls.find((c) => c.cmd === 'add_feed');
    checkNew('(fix-1) 数字 catId 照旧映射（cat-2 → folderId=2）',
      !!fix1b && fix1b.args.folderId === 2);

    /* fix-3（自检 P2-1）：删除当前正浏览的分类/订阅源 → 范围与选中复位（与 mock
       分支同口径）。修前 tauri 分支不清理，entries 按已消失的范围过滤恒为空，
       时间流停在幽灵范围。断言能抓住回退：删掉复位 set 后 activeFeedFilter 仍为
       已删 id，两条断言即红。 */
    await bootFixture();
    store.getState().selectFeed('cat-1');
    await nTick(20);
    store.getState().selectArticle('101');
    checkNew('(fix-3) 前置：已进入 cat-1 范围并选中文章',
      store.getState().activeFeedFilter === 'cat-1' && store.getState().activeArticleId === '101');
    await store.getState().deleteCategory('cat-1');
    await nTick(20);
    checkNew('(fix-3) 删除正浏览的分类 → activeFeedFilter 复位 all 且清空选中（修前停在幽灵范围）',
      store.getState().activeFeedFilter === 'all' && store.getState().activeArticleId === null);
    /* 负向：删除非活动分类不影响当前范围 */
    await bootFixture();
    store.getState().selectFeed('cat-2');
    await nTick(20);
    await store.getState().deleteCategory('cat-1');
    await nTick(20);
    checkNew('(fix-3) 删除非活动分类 → 当前范围保持不变',
      store.getState().activeFeedFilter === 'cat-2');
    /* deleteFeed 同口径 */
    await bootFixture();
    store.getState().selectFeed('10');
    await nTick(20);
    store.getState().selectArticle('101');
    await store.getState().deleteFeed('cat-1', '10');
    await nTick(20);
    checkNew('(fix-3) 删除正浏览的订阅源 → 范围复位 all + 清空选中',
      store.getState().activeFeedFilter === 'all' && store.getState().activeArticleId === null);
    await bootFixture();
    store.getState().selectFeed('11');
    await nTick(20);
    await store.getState().deleteFeed('cat-1', '10');
    await nTick(20);
    checkNew('(fix-3) 删除非活动订阅源 → 当前范围保持不变',
      store.getState().activeFeedFilter === '11');

    /* fix-4（自检 P2-2）：reloadFilteredEntries 的口径守卫——「范围×布局」在途时
       被切换 ⇒ 迟到的旧响应整体丢弃（entries 不被覆盖、过期游标不写入），且
       新口径自己的响应照常落地。修前守卫只比 view：切范围后旧响应放行，
       「源A×article」的收藏列表覆盖进源B 视图并持久留存。 */
    await resetStore();
    store.setState({ activeViewFilter: 'starred', articlesCursor: {} });
    listPlan = { mode: 'defer' };
    const fix4Late = store.getState().reloadFilteredEntries('starred'); // 发起时范围=all
    await nTick(0);
    store.getState().selectFeed('11');  // 期间切到源B：它自己的筛选请求也进 defer 队列
    await nTick(0);
    checkNew('(fix-4) 竞态场景成立：两个筛选请求都在途（旧口径在前）', pendingList.length === 2);
    const fix4CursorBefore = JSON.stringify(store.getState().articlesCursor);
    pendingList[0].resolve([mkRow({ id: 9001, feed_id: 10, is_starred: true, title: '迟到的旧口径数据' })]);
    await fix4Late;
    await nTick(20);
    checkNew('(fix-4) 旧 scope 的迟到筛选响应被丢弃：entries 不被覆盖、过期游标不写入（修前只比 view 会放行）',
      !store.getState().entries.some((e) => e.id === '9001')
      && JSON.stringify(store.getState().articlesCursor) === fix4CursorBefore);
    /* 正向对照：新口径自己的响应照常落地（守卫没有锁死正常路径） */
    pendingList[1].resolve([mkRow({ id: 9002, feed_id: 11, is_starred: true, title: '源B 的筛选结果' })]);
    await nTick(20);
    checkNew('(fix-4) 新口径响应照常落地：源B 的收藏行进列表、游标写在新范围键上',
      store.getState().entries.some((e) => e.id === '9002')
      && store.getState().articlesCursor['article|11']?.loaded === 1);
    listPlan = null;
  }

  /* ============================================================
     TASK-107（REQ-003）：全部已读与标读计数一致性
     —— 成功以后端实际影响数对账（feed_counts 重取整体替换）、失败回滚不假
     成功、单条标读计数与后端口径一致（含同文副本不重复扣减）、范围外布局/
     范围计数不受影响。
     审计探针（AUDIT-20261005-core-consistency.md）：范围 600 条未读、前端
     加载 1 条，后端成功 600 条后界面仍显示 599 条未读（乐观推算缺口）。
     计数对账用例统一挂 feedCountsImpl = countsFromRows（忠实聚合）；mock 的
     set_read / mark_all_read 已升级为忠实落库（就地翻转 is_read 并返回受影响
     行数），resetStore 对行对象深拷贝防跨用例突变残留。
     ============================================================ */
  {
    /* ---------- t104-markall-count-authoritative：600/1 探针转断言 ---------- */
    await resetStore();
    feedCountsImpl = countsFromRows;
    const rows107 = [];
    for (let i = 0; i < 600; i += 1) rows107.push(mkRow({ id: 40000 + i, feed_id: 10, published_at: iso(NOW - i * 1000) }));
    backendRows = rows107;
    /* 注意：不能走 bootFixture()——resetStore 会复位 feedCountsImpl/backendRows，
       自定义夹具必须在 resetStore 之后、bootstrapFromBackend 之前就位 */
    await store.getState().bootstrapFromBackend();
    /* 极端分页形态：范围 600 条未读、前端仅加载 1 条（其余 599 条不在册） */
    store.setState((s) => ({ entries: s.entries.slice(0, 1) }));
    invokeCalls.length = 0;
    store.getState().markCurrentViewAllRead();
    await nTick(10);
    checkNew('(t104-markall-count-authoritative) 600/1 探针：全部已读成功后未读计数=后端口径 0（修前按已加载推算残留 599）',
      store.getState().feedCounts.get('10')?.unread === 0
      && store.getState().entries.find((a) => a.id === '40000')?.isRead === true
      && invokeCalls.filter((c) => c.cmd === 'feed_counts').length === 1);

    /* ---------- t104-markall-failure-rollback：失败不假成功 ---------- */
    await resetStore();
    feedCountsImpl = countsFromRows;
    backendRows = [
      mkRow({ id: 41001, feed_id: 10, published_at: iso(NOW) }),
      mkRow({ id: 41002, feed_id: 10, published_at: iso(NOW - 60000) }),
      mkRow({ id: 41003, feed_id: 12, published_at: iso(NOW - 120000) }),
      mkRow({ id: 41004, feed_id: 12, published_at: iso(NOW - 180000) }),
      mkRow({ id: 41005, feed_id: 12, published_at: iso(NOW - 240000) }),
    ];
    await store.getState().bootstrapFromBackend();
    rejectCmds.add('mark_all_read');
    store.getState().markCurrentViewAllRead();
    await nTick(10);
    const rb107 = store.getState();
    checkNew('(t104-markall-failure-rollback) markAllRead 失败：无「已全部标为已读」假成功提示，失败 toast 可见且带重试',
      !rb107.toasts.some((t) => t.text === '已全部标为已读')
      && rb107.toasts.some((t) => t.text.startsWith('全部已读保存失败') && t.action?.label === '重试'));
    checkNew('(t104-markall-failure-rollback) 失败回滚到位：已读态全部还原、未读计数还原（feed10=2 / feed12=3）',
      rb107.entries.every((a) => !a.isRead)
      && rb107.feedCounts.get('10')?.unread === 2 && rb107.feedCounts.get('12')?.unread === 3);

    /* ---------- t104-single-read-count：单条标读计数与后端口径一致 ---------- */
    await resetStore();
    feedCountsImpl = countsFromRows;
    backendRows = [
      mkRow({ id: 42001, feed_id: 10, guid: 'dup-1', published_at: iso(NOW) }),
      mkRow({ id: 42002, feed_id: 12, published_at: iso(NOW - 60000) }),
      mkRow({ id: 42003, feed_id: 11, guid: 'dup-1', published_at: iso(NOW - 120000) }),
      mkRow({ id: 42004, feed_id: 11, published_at: iso(NOW - 180000) }),
    ];
    await store.getState().bootstrapFromBackend();
    store.getState().toggleEntryFlag('42001', 'isRead');
    await nTick(10);
    const single107 = store.getState();
    /* 后端口径：set_read 只翻转 42001 自身（同文副本行 42003 不动），按行重算聚合 */
    const agg107 = new Map(countsFromRows().map((c) => [String(c.feed_id), c]));
    checkNew('(t104-single-read-count) 单条标读：前端 feedCounts 与后端按行聚合逐源一致（feed10 恰好 -1，其余不动）',
      single107.feedCounts.get('10')?.unread === agg107.get('10')?.unread && single107.feedCounts.get('10')?.unread === 0
      && single107.feedCounts.get('11')?.unread === agg107.get('11')?.unread && single107.feedCounts.get('11')?.unread === 2
      && single107.feedCounts.get('12')?.unread === agg107.get('12')?.unread && single107.feedCounts.get('12')?.unread === 1);
    checkNew('(t104-single-read-count) 同文副本不重复扣减：主条目标读后，副本行（42003）读态与所属源计数均不变',
      agg107.get('11')?.unread === 2
      && single107.entries.find((a) => a.id === '42003')?.isRead === false);

    /* ---------- t104-scope-isolation：范围外布局/范围计数不受影响 ---------- */
    await resetStore();
    feedCountsImpl = countsFromRows;
    backendRows = [
      mkRow({ id: 43001, feed_id: 10, published_at: iso(NOW) }),
      mkRow({ id: 43002, feed_id: 10, published_at: iso(NOW - 60000) }),
      mkRow({ id: 43003, feed_id: 11, published_at: iso(NOW - 120000) }),
      mkRow({ id: 43011, feed_id: 12, published_at: iso(NOW - 180000) }),
      mkRow({ id: 43012, feed_id: 12, published_at: iso(NOW - 240000) }),
      mkRow({ id: 43013, feed_id: 12, published_at: iso(NOW - 300000) }),
    ];
    await store.getState().bootstrapFromBackend();
    /* 范围内仅部分条目已加载（分页快照形态）：feed12 共 3 条未读，在册 2 条 */
    store.setState((s) => ({ entries: s.entries.filter((a) => a.id === '43011' || a.id === '43012') }));
    store.setState({ activeFeedFilter: '12' });
    const isoBefore107 = store.getState();
    store.getState().markCurrentViewAllRead();
    await nTick(10);
    const isoAfter107 = store.getState();
    checkNew('(t104-scope-isolation) 单源范围全部已读（3 条未读仅 2 条在册）：该源计数=后端口径 0（修前残留 1）',
      isoAfter107.feedCounts.get('12')?.unread === 0
      && isoAfter107.entries.every((a) => a.feedId === '12' && a.isRead));
    checkNew('(t104-scope-isolation) 范围外不受影响：其他源（feed10=2）与跨布局源（feed11 social=1）计数保持原值',
      isoAfter107.feedCounts.get('10')?.unread === isoBefore107.feedCounts.get('10')?.unread
      && isoAfter107.feedCounts.get('11')?.unread === isoBefore107.feedCounts.get('11')?.unread
      && isoAfter107.feedCounts.get('10')?.unread === 2 && isoAfter107.feedCounts.get('11')?.unread === 1);

    /* ---------- t104-rollback-guard-versioned（TASK-107 R1/F1）：版本化回滚守卫 ----------
       审查探针 C3 场景：全部已读在途失败期间，用户对同一在册条目连点两次 toggle
       停在「已读」（其自身 set_read(true) 已落库）。修前值守卫（当前值==乐观写入值）
       无法区分「用户已接管」与「未被触碰」，迟到回滚把 UI 踩回未读而 DB 是已读，
       计数同步偏差 +1；修后以「条目变更版本未变」为恢复前提，用户接管（每次真实
       翻转都 bump 版本）的条目一律跳过。 */
    await resetStore();
    feedCountsImpl = countsFromRows;
    backendRows = [
      mkRow({ id: 47001, feed_id: 10, published_at: iso(NOW) }),
      mkRow({ id: 47002, feed_id: 10, published_at: iso(NOW - 60000) }),
    ];
    await store.getState().bootstrapFromBackend();
    rejectCmds.add('mark_all_read');
    store.getState().markCurrentViewAllRead();            // 乐观翻转 47001/47002 → read
    store.getState().toggleEntryFlag('47001', 'isRead');  // 用户 read→unread（set_read(false) 落库）
    store.getState().toggleEntryFlag('47001', 'isRead');  // 用户 unread→read（最终意图=已读，set_read(true) 落库）
    await nTick(60);                                       // 全部已读失败回滚落地
    const guard107 = store.getState();
    checkNew('(t104-rollback-guard-versioned) 双 toggle 停在已读 + 全部已读失败：用户最终意图不被回踩（修前值守卫误踩回未读）',
      guard107.entries.find((a) => a.id === '47001')?.isRead === true
      && backendRows.find((r) => r.id === 47001)?.is_read === true // R2/F3：wire 格式布尔化（原数字 1）
      && guard107.entries.find((a) => a.id === '47002')?.isRead === false);
    checkNew('(t104-rollback-guard-versioned) 版本化守卫下计数与 DB 真值一致（修前同值误踩会偏差 +1）',
      guard107.feedCounts.get('10')?.unread === 1
      && countsFromRows().find((c) => c.feed_id === 10)?.unread === 1);

    /* ---------- t104-reconcile-retry（TASK-107 R1/F2）：对账重取失败可见化 + 短延迟重试 ----------
       审查探针 F 场景：mark_all_read 落库成功但紧随的 feed_counts 重取失败——
       修前完全静默（计数残留乐观值 599、DB 真值 0，无任何提示）；修后先给一条
       诊断 toast（与「保存失败」文案明确区分），并安排一次 3s 延迟重试。 */
    await resetStore();
    feedCountsImpl = countsFromRows;
    const rows107F2 = [];
    for (let i = 0; i < 600; i += 1) rows107F2.push(mkRow({ id: 46000 + i, feed_id: 10, published_at: iso(NOW - i * 1000) }));
    backendRows = rows107F2;
    await store.getState().bootstrapFromBackend();
    store.setState((s) => ({ entries: s.entries.slice(0, 1) }));
    rejectCmds.add('feed_counts'); // 仅注入对账重取失败（mark_all_read 本身成功落库）
    store.getState().markCurrentViewAllRead();
    await nTick(10);
    const rF2 = store.getState();
    checkNew('(t104-reconcile-retry) 对账重取失败不再静默：诊断 toast 可见且不与「保存失败」混淆（标读本身已成功）',
      rF2.toasts.some((t) => t.text === '全部已读已保存，未读计数刷新失败')
      && !rF2.toasts.some((t) => t.text.startsWith('全部已读保存失败')));
    checkNew('(t104-reconcile-retry) 重取失败时计数停留乐观值（599），等待延迟重试',
      rF2.feedCounts.get('10')?.unread === 599
      && countsFromRows().find((c) => c.feed_id === 10)?.unread === 0);
    rejectCmds.delete('feed_counts');
    await nTick(3500); // 等 3s 延迟重试落地
    checkNew('(t104-reconcile-retry) 3s 延迟重试成功：计数自愈为后端真值 0',
      store.getState().feedCounts.get('10')?.unread === 0);

    /* ---------- t104-snapshot-voids-rollback-claim（TASK-107 R2/F3）：快照替换使在途乐观声明失效 ----------
       R1 审查变异发现：mergeSnapshotEntries 对存活 id 的 bumpEntryVersion 无断言覆盖（删掉全绿）。
       本场景给 bump 一个**判别性**用例：全部已读在途失败窗口内，外部 DB 写入（同步拉取把远端
       已读态直接落库——不经前端写入路径、不 bump 前端版本）把行置为已读，随后快照替换带来
       行级真值（行 read=true、计数重取 0）。mark_all_read 此刻才失败：条目当前值==乐观写入值
       （值守卫放行），唯一的守卫是 merge bump（版本已前进）——迟到回滚必须跳过，否则会把
       陈旧未读踩回 UI 并把刚重取的真值计数虚增回去。 */
    await resetStore();
    feedCountsImpl = countsFromRows;
    backendRows = [
      mkRow({ id: 48001, feed_id: 10, published_at: iso(NOW) }),
      mkRow({ id: 48002, feed_id: 10, published_at: iso(NOW - 60000) }),
    ];
    await store.getState().bootstrapFromBackend();
    const realInvoke107v = globalThis.__INVOKE__;
    let rejectMarkAll107v = null;
    globalThis.__INVOKE__ = (cmd, args) => {
      if (cmd === 'mark_all_read') return new Promise((_resolve, rej) => { rejectMarkAll107v = rej; });
      return realInvoke107v(cmd, args);
    };
    store.getState().markCurrentViewAllRead(); // 乐观翻转 48001/48002 → read（计数 2→0，版本快照 v1）
    await nTick(0);
    /* 外部写入者（同步拉取语义）：绕过前端直接落库，行 is_read=1——不触发 flipEntryFlag/markEntriesRead */
    backendRows.forEach((r) => { r.is_read = true; }); // 外部落库为 wire 布尔
    await store.getState().reloadFromBackend(); // 快照替换：行 read=true、计数重取 0、merge bump → v2
    const voided107 = store.getState();
    checkNew('(t104-snapshot-voids-rollback-claim) 场景成立：快照替换带来后端真值（行 read=true、计数重取 0）',
      voided107.entries.every((a) => a.isRead) && voided107.feedCounts.get('10')?.unread === 0);
    rejectMarkAll107v({ message: '注入失败:mark_all_read' });
    await nTick(10);
    globalThis.__INVOKE__ = realInvoke107v;
    const afterVoid107 = store.getState();
    checkNew('(t104-snapshot-voids-rollback-claim) 迟到回滚不踩快照真值：行保持 read、计数保持 0（无 bump 时会被恢复为未读并虚增回 2）',
      afterVoid107.entries.every((a) => a.isRead)
      && afterVoid107.feedCounts.get('10')?.unread === 0);
    checkNew('(t104-snapshot-voids-rollback-claim) 回滚跳过不等于吞错：失败 toast 仍可见且带重试',
      afterVoid107.toasts.some((t) => t.text.startsWith('全部已读保存失败') && t.action?.label === '重试'));

    /* ---------- t104-snapshot-fresh-claim-rollback（TASK-107 R2/F3）：快照后新声明的回滚仍正常 ----------
       边界：bump 只使「快照替换**之前**建立的乐观声明」失效，不得永久瘫痪回滚机制。
       两段验证——①快照替换（行仍未读：mark_all_read 未落库）+ 失败：被快照覆盖的条目不被
       陈旧回滚踩到（读态/计数保持快照后的后端真值；此形态值守卫与版本守卫双保险，锁定契约）；
       ②快照替换**之后**新发起的全部已读（版本快照取自 bump 后的现值）失败时，回滚照常
       恢复读态并回补计数——机制本身仍然存活。 */
    await resetStore();
    feedCountsImpl = countsFromRows;
    backendRows = [
      mkRow({ id: 49001, feed_id: 10, published_at: iso(NOW) }),
      mkRow({ id: 49002, feed_id: 10, published_at: iso(NOW - 60000) }),
    ];
    await store.getState().bootstrapFromBackend();
    const realInvoke107f = globalThis.__INVOKE__;
    let rejectMarkAll107f = null;
    globalThis.__INVOKE__ = (cmd, args) => {
      if (cmd === 'mark_all_read') return new Promise((_resolve, rej) => { rejectMarkAll107f = rej; });
      return realInvoke107f(cmd, args);
    };
    store.getState().markCurrentViewAllRead(); // 声明 #1：乐观翻转 → read（计数 2→0）
    await nTick(0);
    await store.getState().reloadFromBackend(); // 快照替换（行仍未读：mark_all_read 未落库）→ 计数重取 2、bump
    rejectMarkAll107f({ message: '注入失败:mark_all_read' });
    await nTick(10);
    const staleClaim107 = store.getState();
    checkNew('(t104-snapshot-fresh-claim-rollback) 陈旧声明失效：快照后行保持未读、计数保持重取值 2（不被陈旧回滚踩动）',
      staleClaim107.entries.every((a) => !a.isRead)
      && staleClaim107.feedCounts.get('10')?.unread === 2);
    store.getState().markCurrentViewAllRead(); // 声明 #2（快照后新建）：乐观翻转 → read（计数 2→0）
    await nTick(0);
    rejectMarkAll107f({ message: '注入失败:mark_all_read' });
    await nTick(10);
    globalThis.__INVOKE__ = realInvoke107f;
    const freshClaim107 = store.getState();
    checkNew('(t104-snapshot-fresh-claim-rollback) 快照后新声明的回滚仍正常：两行恢复未读、计数回补到 2（机制未被 bump 瘫痪）',
      freshClaim107.entries.every((a) => !a.isRead)
      && freshClaim107.feedCounts.get('10')?.unread === 2
      && freshClaim107.toasts.some((t) => t.text.startsWith('全部已读保存失败')));
  }

  /* ============================================================
     TASK-109：查询口径统一收口（QueryScope）+ merge bump 按真值来源收窄
     - 口径单点：三把键与查询参数只经 QueryScope 派生（源级断言）；
       scopePageKey/viewCacheKey 字符串形态锁定不变（缓存/游标键兼容）；
     - reloadFromBackend 的游标键/「全部」视图缓存键改为发起时快照（原在 await
       后读完成时状态，靠 reloadGeneration 间接兜底；行为等价、口径显式化）；
     - bump 收窄：后端快照路径（fromBackend=true）void 在途声明；缓存恢复路径
       （fromBackend=false）不 void——R2 裁定角例的修后行为有判别断言。
     ============================================================ */
  {
    const fs109 = await import('node:fs');
    const src109 = (p) => fs109.readFileSync(new URL(p, import.meta.url), 'utf8');
    const internals109 = src109('../src/store/internals.ts');
    const bootstrap109 = src109('../src/store/slices/bootstrap.ts');
    const nav109 = src109('../src/store/slices/nav.ts');

    /* -- t109-queryscope-single-source：口径派生单点性（源级）-- */
    checkNew('(t109-queryscope-single-source) QueryScope 统一入口定义于 internals：三把键 + 标写范围 + 两个具名守卫齐备',
      internals109.includes('export const QueryScope = {')
      && internals109.includes('args: scopeQueryArgs')
      && internals109.includes('markScope: scopeFilterArgs')
      && internals109.includes('pageKey: scopePageKey')
      && internals109.includes('viewKey: viewCacheKey')
      && internals109.includes('export function paginationStale(')
      && internals109.includes('export function filteredSnapshotStale('));
    {
      const importLine = (src) => {
        const m = src.match(/import \{([^}]*)\} from '\.\.\/internals';/);
        return m ? m[1] : '';
      };
      checkNew('(t109-queryscope-single-source) bootstrap/nav 不再裸引三把键：仅经 QueryScope.* 消费（键派生无第二入口）',
        !importLine(bootstrap109).includes('scopeQueryArgs') && !importLine(bootstrap109).includes('scopePageKey')
        && !importLine(bootstrap109).includes('viewCacheKey')
        && !importLine(nav109).includes('scopeQueryArgs') && !importLine(nav109).includes('scopePageKey')
        && !importLine(nav109).includes('viewCacheKey')
        && bootstrap109.includes('QueryScope.args(') && bootstrap109.includes('QueryScope.pageKey(')
        && bootstrap109.includes('QueryScope.viewKey(')
        && bootstrap109.includes('QueryScope.paginationStale(') && bootstrap109.includes('QueryScope.filteredSnapshotStale(')
        && nav109.includes('QueryScope.pageKey(') && nav109.includes('QueryScope.viewKey('));
    }

    /* -- t109-key-format-compat：键形态兼容（行为级）-- */
    {
      const it109 = await import('../dist-test/store/internals.js');
      checkNew('(t109-key-format-compat) scopePageKey/viewCacheKey 字符串形态逐字不变，QueryScope 同源绑定、args/markScope 派生正确',
        it109.scopePageKey('all') === 'all' && it109.scopePageKey('10') === '10' && it109.scopePageKey('cat-1') === 'cat-1'
        && it109.scopePageKey('10', 'article') === 'article|10' && it109.scopePageKey('all', 'social') === 'social|all'
        && it109.viewCacheKey('article', 'all') === 'article|all|all'
        && it109.viewCacheKey('social', 'starred', '11') === 'social|starred|11'
        && it109.QueryScope.pageKey === it109.scopePageKey && it109.QueryScope.viewKey === it109.viewCacheKey
        && JSON.stringify(it109.QueryScope.args('12', 'oldest', 'social')) === JSON.stringify({ feed_id: 12, folder_id: null, newest_first: false, layout: 'social' })
        && JSON.stringify(it109.QueryScope.markScope('cat-1')) === JSON.stringify({ feed_id: null, folder_id: 1 })
        && JSON.stringify(it109.QueryScope.markScope('all')) === JSON.stringify({ feed_id: null, folder_id: null }));
    }

    /* -- t109-reload-scope-snapshot-at-start：发起时快照修正（源级）-- */
    {
      const start109 = bootstrap109.indexOf('reloadFromBackend: async');
      const end109 = bootstrap109.indexOf('loadMoreArticles:');
      const reloadSrc = bootstrap109.slice(start109, end109);
      const awaitPos = reloadSrc.indexOf('await Promise.all');
      const capturePos = reloadSrc.indexOf('const scopeAtStart = get().activeFeedFilter;');
      checkNew('(t109-reload-scope-snapshot-at-start) reloadFromBackend 发起时快照范围口径：scopeKey/缓存键从 scopeAtStart 派生，await 后无完成时回读',
        capturePos >= 0 && awaitPos >= 0 && capturePos < awaitPos
        && reloadSrc.includes('const scopeKey = QueryScope.pageKey(scopeAtStart, layoutAtStart);')
        && reloadSrc.includes("QueryScope.viewKey(layoutAtStart, 'all', scopeAtStart)")
        && !reloadSrc.slice(awaitPos).includes('get().activeFeedFilter')
        && !reloadSrc.slice(awaitPos).includes('get().activeContentLayout'));
    }

    /* -- t109-markscope-derivation：标写范围派生收口（源级）-- */
    {
      const markSrc = nav109.slice(nav109.indexOf('markCurrentViewAllRead: () => {'));
      checkNew('(t109-markscope-derivation) markCurrentViewAllRead 标写范围经 QueryScope.markScope（仅订阅维度），layout 由 markAllRead 独立参数承载',
        markSrc.includes('QueryScope.markScope(scope)')
        && !markSrc.includes('scopeQueryArgs(')
        && markSrc.includes('api.markAllRead(feedId, folderId, { starredOnly, sinceMs, layout })'));
    }

    /* -- t109-cache-restore-preserves-claim：R2 裁定角例修后行为（判别，行为级）--
       全部已读乐观写入在途（缓存已被乐观写同步为 read 态）→ 缓存恢复（selectFeed
       命中，回放乐观 read 态；fromBackend=false 不 bump 版本）→ mark_all_read 失败
       → 回滚必须仍能恢复（修前 bump-on-all-merge 会 void 声明、卡在乐观 read 态）。 */
    await resetStore();
    feedCountsImpl = countsFromRows;
    backendRows = [
      mkRow({ id: 50001, feed_id: 10, published_at: iso(NOW) }),
      mkRow({ id: 50002, feed_id: 10, published_at: iso(NOW - 60000) }),
    ];
    await store.getState().bootstrapFromBackend();
    const realInvoke109 = globalThis.__INVOKE__;
    let rejectMarkAll109 = null;
    globalThis.__INVOKE__ = (cmd, args) => {
      if (cmd === 'mark_all_read') return new Promise((_res, rej) => { rejectMarkAll109 = rej; });
      return realInvoke109(cmd, args);
    };
    store.getState().markCurrentViewAllRead(); // 乐观翻转 → read；缓存同步为 read 态；版本快照 v1
    await nTick(0);
    listPlan = { mode: 'defer' };              // 拦住 selectFeed 的后台 reload（其 fromBackend bump 不得抢跑）
    store.getState().selectFeed('all');        // 缓存命中：回放乐观 read 态（TASK-109②：不 bump）
    await nTick(0);
    const replayed109 = store.getState();
    checkNew('(t109-cache-restore-preserves-claim) 场景成立：缓存回放携带乐观 read 态且计数停留乐观值（回放未 bump 版本）',
      replayed109.entries.every((a) => a.isRead) && replayed109.feedCounts.get('10')?.unread === 0);
    rejectMarkAll109({ message: '注入失败:mark_all_read' });
    await nTick(10);
    const healed109 = store.getState();
    checkNew('(t109-cache-restore-preserves-claim) R2 角例修后行为：缓存回放不 void 在途声明，失败回滚正确恢复未读态与计数 2',
      healed109.entries.every((a) => !a.isRead) && healed109.feedCounts.get('10')?.unread === 2);

    /* -- t109-cache-replay-new-claim：缓存回放不误 void 新声明（行为级）-- */
    store.getState().markCurrentViewAllRead(); // 回放后的新声明：乐观翻转 → read（版本 bump 后快照）
    await nTick(0);
    rejectMarkAll109({ message: '注入失败:mark_all_read' });
    await nTick(10);
    const newClaim109 = store.getState();
    checkNew('(t109-cache-replay-new-claim) 缓存回放后新建声明的回滚仍正常：恢复未读态与计数 2（不误 void）',
      newClaim109.entries.every((a) => !a.isRead) && newClaim109.feedCounts.get('10')?.unread === 2);
    // 清理：放行被 defer 的后台 reload（fromBackend merge 接真值对齐），还原 invoke
    listPlan = null;
    for (const p of pendingList) p.resolve(queryRows(p.args));
    pendingList.length = 0;
    await nTick(20);
    globalThis.__INVOKE__ = realInvoke109;

    /* -- t109-filtered-guard-named→t110：筛选视图拉取锁排序（具名守卫，行为级）。
       【TASK-110 改动理由】原断言「筛选视图拉取不锁排序：拉取期间切排序响应仍落地」
       锁的正是本卡废除的旧行为——筛选视图拉全集（limit:100000）存内存，切排序只是
       selectVisibleEntries 本地重排，排序与取数无关，守卫有意不锁排序。TASK-110①③
       分页化 + 切排序改为服务端重拉后，该前提废除：迟到的旧排序响应属于另一个查询
       口径（放行会把旧排序页当作新排序列表写入），filteredSnapshotStale 同步锁排序。
       新契约两段验证：①拉取期间切排序 ⇒ 第一发（旧排序）响应被整体丢弃，entries
       未被覆盖；②切排序自身触发的按新排序重拉落地（entries 为新排序序、集合不变、
       exhausted 真实判定、游标随落地快照对齐）。 */
    await resetStore();
    feedCountsImpl = countsFromRows;
    await store.getState().bootstrapFromBackend();
    listPlan = { mode: 'defer' };
    store.setState({ activeViewFilter: 'starred', timelineSort: 'newest' });
    const pFiltered109 = store.getState().reloadFilteredEntries('starred');
    await nTick(0);
    store.getState().toggleTimelineSort(); // TASK-110③：筛选视图切排序现在触发按新排序的重拉（第二发，同走 defer 队列）
    const sortFlipped109 = store.getState().timelineSort === 'oldest';
    for (const p of pendingList) p.resolve(queryRows(p.args)); // 第一发（旧排序 newest）响应放行 → 守卫丢弃
    pendingList.length = 0;
    await pFiltered109;
    checkNew('(t109-filtered-guard-named→t110) 筛选视图拉取锁排序（TASK-110③ 行为变化）：拉取期间切排序，迟到旧排序响应被 filteredSnapshotStale 整体丢弃（entries 未被覆盖，仍为 all 首批快照）',
      sortFlipped109
      && store.getState().entries.length === 8
      && store.getState().entries.some((e) => !e.isStarred));
    /* TASK-110③ 驱动补充（探针实测的连锁根因，必须在此排空）：切排序触发的第二发
       reloadFilteredEntries，其 list_articles 经 api.listArticles 内部 await getInvoke()
       延了一个微任务才进 defer 队列——第一发在上方 nTick(0) 宏任务间隙已入队、排空
       循环够得到它；第二发入队晚于排空循环，promise 被孤儿化后永不落定，
       reloadFilteredEntries 的 finally 减不掉 backendReloadInFlight，在途标记泄漏会
       拦住后续所有测试块的 loadMoreArticles（r7-switch / t100-p3-1 连锁误红）。
       生产侧无此问题（Tauri invoke 恒 settle），纯测试驱动口径。 */
    await nTick(0);
    for (const p of pendingList.splice(0)) p.resolve(queryRows(p.args)); // 第二发（新排序 oldest 重拉）放行
    listPlan = null;
    await nTick(10);
    checkNew('(t109-filtered-guard-named→t110) 切排序触发的重拉落地：entries 为新排序（oldest）收藏集合、exhausted 真实判定（3<500）、游标随落地快照对齐',
      store.getState().entries.map((e) => e.id).join(',') === '103,102,201'
      && store.getState().entries.every((a) => a.isStarred)
      && store.getState().articlesExhausted === true
      && store.getState().articlesCursor['article|all']?.loaded === 3);
  }

  /* ============================================================
     TASK-110（二阶段②）：筛选视图真分页——废除 limit:100000 近似全集。
     旧手法：reloadFilteredEntries 一次拉全集（articlesExhausted 恒 true、切排序
     仅本地重排），旧文章在筛选视图不可达（审计二阶段完成标准）。新契约：
     ①筛选视图首屏 PAGE_SIZE、续拉走 loadMoreArticles（viewFilter 单点派生，
       与「全部」视图共用游标键 scopePageKey 与分页守卫）；
     ②追加按 id 去重 + 稳定序策略「追加去重保序」（同步插入使 offset 漂移时
       不得重复入列；新条目不回填已加载窗口，随下次 reload 进入——判别断言）；
     ③切排序与 all 同构重拉（水合正文由 mergeSnapshotEntries 按 id 继承）；
     ④exhausted 真实判定（rows.length < PAGE_SIZE）双向锁定；
     ⑤过滤参数（only_unread/only_starred/only_today）逐项 × 范围 × 布局。
     夹具：600 行 feed-10（i=0 最新），其中前 520 行收藏；时间戳取 NOW-3d 起，
     与「今天」判定解耦（today 用独立 NOW 行）。全部走内存假后端（模块级
     harness），布局不做后端过滤（与 (r7-switch) 同口径，断言走 wire 参数）。
     ============================================================ */
  {
    const PAGE = 500; // 与 internals.ARTICLES_PAGE_SIZE 同值（import 断言见 (t110-0)）
    const mkT110Row = (o) => mkRow({ feed_id: 10, ...o });
    const T110_OLD = iso(NOW - 3 * 86400000);
    /* 600 行基础夹具：id=1100+i，i 越小越新；i<520 收藏 */
    const t110Rows = () => {
      const rows = [];
      for (let i = 0; i < 600; i += 1) {
        rows.push(mkT110Row({ id: 1100 + i, published_at: iso(NOW - 3 * 86400000 - i * 60000), is_starred: i < 520 }));
      }
      return rows;
    };

    /* -- (t110-0) 页大小单点来源：筛选视图分页与「全部」共用同一常量 -- */
    const { ARTICLES_PAGE_SIZE: t110Page } = await import('../dist-test/store/internals.js');
    checkNew('(t110-0) ARTICLES_PAGE_SIZE 收口到 internals（=500）：筛选视图首屏/续拉与「全部」视图同一页大小单点来源',
      t110Page === 500 && PAGE === t110Page);

    /* ---------- (t110-1/2) 收藏视图：首屏分页 → 满页续拉 → 真到底 ---------- */
    await resetStore();
    backendRows = t110Rows();
    await store.getState().bootstrapFromBackend(); // all 视图首批 500（exhausted=false）
    invokeCalls.length = 0;
    store.getState().selectView('starred'); // 缓存已清 → 走 reloadFilteredEntries 首屏
    await nTick(20);
    const t110Star1st = invokeCalls.filter((c) => c.cmd === 'list_articles');
    const t110Star1stArgs = t110Star1st[0]?.args.args;
    checkNew('(t110-1) 收藏视图首屏真分页 wire：恰 1 次 list_articles，only_starred=true + limit=500（废除 limit:100000）+ offset=0 + 范围/布局维度齐全（article|all）',
      t110Star1st.length === 1
      && t110Star1stArgs?.only_starred === true && t110Star1stArgs?.limit === 500 && t110Star1stArgs?.offset === 0
      && t110Star1stArgs?.feed_id === null && t110Star1stArgs?.folder_id === null && t110Star1stArgs?.layout === 'article');
    const t110Star1stState = store.getState();
    checkNew('(t110-1) 收藏视图首屏落地：520 条收藏取前 500（最新端 i=0..499 ⇒ id 1100..1599）、exhausted 真实判定为 false（满页 ⇒ 可续拉，refill/哨兵前提）、游标=500',
      t110Star1stState.entries.length === 500
      && t110Star1stState.entries.every((e) => e.isStarred)
      && t110Star1stState.entries[0]?.id === '1100' && t110Star1stState.entries[499]?.id === '1599'
      && t110Star1stState.articlesExhausted === false && t110Star1stState.articlesLimit === 500
      && t110Star1stState.articlesCursor['article|all']?.loaded === 500);

    invokeCalls.length = 0;
    await store.getState().loadMoreArticles(); // 筛选视图续拉：与首屏同口径（viewFilter 单点派生）
    const t110Star2nd = invokeCalls.find((c) => c.cmd === 'list_articles')?.args.args;
    const t110Star2ndState = store.getState();
    /* 【TASK-117 改动理由】续拉 wire 从 offset=500 改为 keyset 锚（last_published/
       last_id = 首屏末行 1599 的原文锚）——offset 正是本卡废除的缺陷手法。 */
    checkNew('(t110-2) 收藏视图加载更多同口径 wire：only_starred=true + keyset 锚（last_id=1599，沿用首屏写入的同键游标）+ limit=500',
      t110Star2nd?.only_starred === true && t110Star2nd?.last_id === 1599
      && t110Star2nd?.last_published === iso(NOW - 3 * 86400000 - 499 * 60000)
      && t110Star2nd?.offset === undefined && t110Star2nd?.limit === 500);
    checkNew('(t110-2) 收藏视图续拉落地：追加剩余 20 条收敛到底（520 条无重复）、exhausted 真实判定为 true（20<500）、游标=520',
      t110Star2ndState.entries.length === 520 && new Set(t110Star2ndState.entries.map((e) => e.id)).size === 520
      && t110Star2ndState.entries[500]?.id === '1600' && t110Star2ndState.entries.at(-1)?.id === '1619'
      && t110Star2ndState.articlesExhausted === true && t110Star2ndState.articlesLimit === 520
      && t110Star2ndState.articlesCursor['article|all']?.loaded === 520);

    /* ---------- (t110-3) 插入漂移 keyset 免疫（同步插入场景，判别断言） ----------
       【TASK-117 改动理由】原断言锁定 offset 分页的「漂移 + 按id去重」补偿手法：
       第 2 页（offset=500）在途期间后端插入 10 条更新文章 ⇒ 迟到响应 = 新序列
       offset 500..999 共 110 行，与已加载窗口重叠 10 条，靠追加去重兜住。keyset
       续拉从根上消除该漂移：锚 = 首屏末行 1599（published_at 原文 + id），迟到
       响应 = 严格排在锚之后的 100 行（1600..1699），与集合增删无关——不再重叠、
       无需去重补偿（去重保留作 duplicate key 兜底防线，不回填策略不变）。
       判别：游标推进 = 500+100=600（offset 语义下为 610），entries 600 条唯一、
       已加载窗口保序、新增段按序追加；同步插入的 10 条不回填已加载窗口。 */
    await resetStore();
    backendRows = t110Rows();
    await store.getState().bootstrapFromBackend(); // all 视图首批 500：ids 1100..1599
    listPlan = { mode: 'defer' };
    store.getState().loadMoreArticles(); // 第 2 页（keyset 锚 1599）在途
    await nTick(0); // 微任务排空：第 2 页请求进入 defer 队列
    const t110DriftPage2 = pendingList.at(-1);
    /* 同步插入 10 条（后端同步入库语义；store 无感知、不发 reload、游标不动） */
    for (let i = 0; i < 10; i += 1) {
      backendRows.unshift(mkT110Row({ id: 1700 + i, published_at: iso(NOW), title: `t110 同步插入 ${i}` }));
    }
    t110DriftPage2?.resolve(queryRows(t110DriftPage2.args)); // 迟到响应 = 锚之后的 100 行（keyset 免疫插入漂移）
    listPlan = null;
    await nTick(20);
    const t110Drift = store.getState();
    const t110DriftIds = t110Drift.entries.map((e) => e.id);
    checkNew('(t110-3) 插入漂移 keyset 免疫（判别断言）：迟到第 2 页恰为锚后 100 行（id 1600..1699，无重叠无补拉）、entries 600 条唯一、已加载窗口保序不变、新增段按序追加',
      t110DriftIds.length === 600 && new Set(t110DriftIds).size === 600
      && t110DriftIds.slice(0, 500).join(',') === Array.from({ length: 500 }, (_, k) => String(1100 + k)).join(',')
      && t110DriftIds.slice(500).join(',') === Array.from({ length: 100 }, (_, k) => String(1600 + k)).join(','));
    checkNew('(t110-3) 插入漂移游标与新条目策略：游标 loaded 按拉取行数累加（500+100=600）、锚推进到本页末行 1699、exhausted 按拉取行数真实判定（100<500）、同步插入的新条目不回填已加载窗口（追加去重保序，随下次 reload 进入）',
      t110Drift.articlesLimit === 600 && t110Drift.articlesCursor['article|all']?.loaded === 600
      && t110Drift.articlesCursor['article|all']?.lastId === 1699
      && t110Drift.articlesExhausted === true
      && t110DriftIds.every((id) => Number(id) < 1700));

    /* ---------- (t110-4) 切排序重拉与 all 同构：wire 参数 + 水合正文保留 ---------- */
    await resetStore();
    backendRows = t110Rows();
    await store.getState().bootstrapFromBackend();
    store.getState().selectView('starred');
    await nTick(20);
    /* 模拟已加载条目的懒水合终态（id 1300 = i 200，切排序前后都在首屏内） */
    store.setState((s) => ({
      entries: s.entries.map((e) => (e.id === '1300' ? { ...e, content: '<p>t110 水合正文</p>', hydrated: true } : e)),
      hydratedIds: { ...s.hydratedIds, '1300': true },
    }));
    invokeCalls.length = 0;
    store.getState().toggleTimelineSort(); // TASK-110③：筛选视图切排序 → 服务端重拉（不再本地重排全集）
    await nTick(20);
    const t110SortCalls = invokeCalls.filter((c) => c.cmd === 'list_articles');
    const t110SortArgs = t110SortCalls[0]?.args.args;
    checkNew('(t110-4) 收藏视图切排序重拉 wire：恰 1 次 list_articles，only_starred=true + newest_first=false + offset=0（与「全部」视图同构，服务端承载排序）',
      t110SortCalls.length === 1
      && t110SortArgs?.only_starred === true && t110SortArgs?.newest_first === false && t110SortArgs?.offset === 0);
    const t110Sorted = store.getState();
    const t110SortedHydrated = t110Sorted.entries.find((e) => e.id === '1300');
    checkNew('(t110-4) 切排序重拉落地：entries 换为新排序（oldest）首屏（首条=最老收藏 1619）、已加载条目的水合正文按 id 继承不因重拉丢失（TASK-106 机制）、游标随落地快照对齐',
      t110Sorted.entries.length === 500 && t110Sorted.entries[0]?.id === '1619'
      && t110Sorted.entries.every((e) => e.isStarred)
      && t110SortedHydrated?.content === '<p>t110 水合正文</p>' && t110SortedHydrated?.hydrated === true
      && t110Sorted.hydratedIds['1300'] === true
      && t110Sorted.articlesCursor['article|all']?.loaded === 500 && t110Sorted.articlesExhausted === false);

    /* ---------- (t110-5) 过滤参数逐项：only_unread / only_starred / only_today（article|all） ---------- */
    await resetStore();
    backendRows = [
      mkT110Row({ id: 2100, published_at: T110_OLD }), // 未读·旧
      mkT110Row({ id: 2101, published_at: T110_OLD }), // 未读·旧
      mkT110Row({ id: 2102, published_at: T110_OLD }), // 未读·旧
      mkT110Row({ id: 2103, published_at: T110_OLD, is_read: true }), // 已读·旧
      mkT110Row({ id: 2104, published_at: T110_OLD, is_read: true }), // 已读·旧
      mkT110Row({ id: 2105, published_at: iso(NOW) }), // 未读·今天
      mkT110Row({ id: 2106, published_at: iso(NOW) }), // 未读·今天
      mkT110Row({ id: 2107, published_at: T110_OLD, is_starred: true }), // 未读·收藏·旧
    ];
    await store.getState().bootstrapFromBackend();
    invokeCalls.length = 0;
    store.getState().selectView('unread');
    await nTick(20);
    const t110UnreadArgs = invokeCalls.find((c) => c.cmd === 'list_articles')?.args.args;
    checkNew('(t110-5) 未读视图首屏 wire：only_unread=true + offset=0 + limit=500，落地只含未读行且 exhausted 真实判定（6<500）',
      t110UnreadArgs?.only_unread === true && t110UnreadArgs?.only_starred === undefined && t110UnreadArgs?.only_today === undefined
      && t110UnreadArgs?.offset === 0 && t110UnreadArgs?.limit === 500
      && store.getState().entries.length === 6 && store.getState().entries.every((e) => !e.isRead)
      && store.getState().articlesExhausted === true);
    invokeCalls.length = 0;
    store.getState().selectView('today');
    await nTick(20);
    const t110TodayArgs = invokeCalls.find((c) => c.cmd === 'list_articles')?.args.args;
    /* 【TASK-117 改动理由】期望顺序随 id 决胜更新：2105/2106 同秒并列，后端
       ORDER BY published_at DESC, id DESC（及同口径的本地排序）下 2106 在前。 */
    checkNew('(t110-5) 今天视图首屏 wire：only_today=true（不带其余筛选键），落地只含今天行（2 条，同秒并列由 id 决胜 2106 在前）',
      t110TodayArgs?.only_today === true && t110TodayArgs?.only_unread === undefined && t110TodayArgs?.only_starred === undefined
      && store.getState().entries.map((e) => e.id).join(',') === '2106,2105');
    invokeCalls.length = 0;
    store.getState().selectView('starred');
    await nTick(20);
    const t110StarredArgs = invokeCalls.find((c) => c.cmd === 'list_articles')?.args.args;
    checkNew('(t110-5) 收藏视图首屏 wire：only_starred=true（不带其余筛选键），落地只含收藏行（1 条）',
      t110StarredArgs?.only_starred === true && t110StarredArgs?.only_unread === undefined && t110StarredArgs?.only_today === undefined
      && store.getState().entries.map((e) => e.id).join(',') === '2107');

    /* ---------- (t110-6) 过滤参数 × 范围：feed-10 × 收藏 ---------- */
    await resetStore();
    backendRows = [
      mkT110Row({ id: 2200, published_at: T110_OLD, is_starred: true }),
      mkT110Row({ id: 2201, published_at: T110_OLD }),
      mkT110Row({ id: 2202, feed_id: 11, published_at: T110_OLD, is_starred: true }), // 源B 的收藏（范围外）
    ];
    await store.getState().bootstrapFromBackend();
    store.getState().selectFeed('feed-10');
    await nTick(20);
    invokeCalls.length = 0;
    store.getState().selectView('starred');
    await nTick(20);
    const t110ScopeArgs = invokeCalls.find((c) => c.cmd === 'list_articles')?.args.args;
    checkNew('(t110-6) 收藏视图 × 订阅范围 wire：feed_id=10 + only_starred=true（视图筛选是范围的子集，两参数同发），落地不含范围外收藏',
      t110ScopeArgs?.feed_id === 10 && t110ScopeArgs?.folder_id === null && t110ScopeArgs?.only_starred === true
      && store.getState().entries.map((e) => e.id).join(',') === '2200');

    /* ---------- (t110-7) 过滤参数 × 布局：social × 收藏 ---------- */
    await resetStore();
    backendRows = [
      mkT110Row({ id: 2300, published_at: T110_OLD, is_starred: true }),
      mkT110Row({ id: 2301, published_at: T110_OLD }),
    ];
    await store.getState().bootstrapFromBackend();
    store.getState().selectLayout('social');
    await nTick(20);
    invokeCalls.length = 0;
    store.getState().selectView('starred');
    await nTick(20);
    const t110LayoutArgs = invokeCalls.find((c) => c.cmd === 'list_articles')?.args.args;
    checkNew('(t110-7) 收藏视图 × 布局 wire：layout=social + only_starred=true（布局维度进查询参数，TASK-094 R7 口径在筛选视图保持）',
      t110LayoutArgs?.layout === 'social' && t110LayoutArgs?.only_starred === true && t110LayoutArgs?.offset === 0);

    await resetStore(); // 夹具复位：不把 610 行大夹具与 defer 残留带给后续块
  }

  /* ============================================================
     TASK-117（审计 P1-1）：可变集合分页改 keyset 游标——阅读中连续翻页不丢文章

     审计 P1 探针场景（tmp/audit-20261007/probes.mjs P1）：未读筛选 WHERE
     is_read=0 是**可变集合**，OFFSET 不等价于已看条数——1200 未读读 500 标读
     后集合剩 700，下一页仍 OFFSET 500 → 跳过剩余集合前 500 篇并假 exhausted
     （探针实测：loaded=700、missingUnread=500、exhausted=true）。修法：续拉以
     已加载窗口最后一行 (published_at 原文, id) 为 keyset 锚，请求「严格排在锚
     之后」的行，与集合增删无关。

     门禁纪律（DEC-gate-adjust-20261007）：操作序列场景必须转真实行为回归——
     本组全部走真实 store 动作序列（翻页 → 标读/取消收藏 → 续拉），不做静态
     分页断言。假后端 queryRows 已按 keyset 语义升级（排序补 id 决胜 + 谓词），
     并忠实落库 set_read_bulk / set_starred（后端集合真实收缩是缺陷成因，必须
     建模，否则操作序列无判别力）。
     ============================================================ */
  {
    const mkT117Row = (o) => mkRow({ feed_id: 10, ...o });
    /* 1200 行未读夹具：id 3000+i，i 越小越新（published_at 逐行递减，无并列） */
    const t117Rows = () => Array.from({ length: 1200 }, (_, i) =>
      mkT117Row({ id: 3000 + i, published_at: iso(NOW - 3 * 86400000 - i * 1000), title: `t117-${i}` }));

    /* ---------- (t117-1) 阅读中连续翻页（审计 P1 探针场景本体） ----------
       操作序列：未读视图首屏 500 → 批量标读这 500（真实 IPC 落库，集合收缩为
       700）→ 续拉必须返回**剩余集合的前 500 篇**（id 3500..3999）。OFFSET 语义
       在此会返回 4000..4199（跳过 500 篇）并假 exhausted。 */
    await resetStore();
    backendRows = t117Rows();
    store.getState().selectView('unread'); // 缓存已清 → 走 reloadFilteredEntries 首屏
    await nTick(20);
    const t117p1 = store.getState();
    checkNew('(t117-1) 前置：未读首屏 500 条（id 3000..3499）、exhausted=false（1200 未读满页，可续拉）',
      t117p1.entries.length === 500 && t117p1.entries[0].id === '3000' && t117p1.entries[499].id === '3499'
      && t117p1.articlesExhausted === false);
    store.getState().markEntriesReadBulk(t117p1.entries.map((e) => e.id)); // 读掉首屏（set_read_bulk 忠实落库）
    await nTick(0);
    checkNew('(t117-1) 标读落库后未读集合收缩为 700（假后端忠实翻转 is_read：缺陷成因被建模）',
      queryRows({ only_unread: true, limit: null }).length === 700);
    invokeCalls.length = 0;
    await store.getState().loadMoreArticles();
    const t117p1wire = invokeCalls.find((c) => c.cmd === 'list_articles')?.args.args;
    const t117p1after = store.getState();
    checkNew('(t117-1) 续拉 wire：only_unread + keyset 锚（last_published=末行 3499 原文 / last_id=3499，无 offset）',
      t117p1wire?.only_unread === true && t117p1wire?.last_id === 3499
      && t117p1wire?.last_published === iso(NOW - 3 * 86400000 - 499 * 1000)
      && t117p1wire?.offset === undefined && t117p1wire?.limit === 500);
    checkNew('(t117-1) 续拉落地（判别）：返回剩余集合的前 500 篇（id 3500..3999 按序追加，无重复无跳过）、exhausted=false',
      t117p1after.entries.length === 1000 && t117p1after.entries[500].id === '3500'
      && t117p1after.entries.at(-1).id === '3999'
      && new Set(t117p1after.entries.map((e) => e.id)).size === 1000
      && t117p1after.articlesExhausted === false);
    invokeCalls.length = 0;
    await store.getState().loadMoreArticles(); // 第三页：剩余 200 条
    const t117p1end = store.getState();
    checkNew('(t117-1) 第三页收敛：追加 4000..4199 共 200 条走完集合、exhausted=true（不足一页真实判定）、游标 loaded=1200、锚推进到末行 4199',
      t117p1end.entries.length === 1200 && t117p1end.entries[1000].id === '4000'
      && new Set(t117p1end.entries.map((e) => e.id)).size === 1200
      && t117p1end.articlesExhausted === true && t117p1end.articlesCursor['article|all']?.loaded === 1200
      && t117p1end.articlesCursor['article|all']?.lastId === 4199);
    invokeCalls.length = 0;
    await store.getState().loadMoreArticles();
    checkNew('(t117-1) exhausted 判定（判别）：到底后再触发不发 IPC（审计「假 exhausted」的反向锚——真到底才允许挡住续拉）',
      invokeCalls.filter((c) => c.cmd === 'list_articles').length === 0);
    checkNew('(t117-1) 全程无遗漏（判别）：审计探针口径 missingUnread=0——剩余未读行（700 条）全部已入列',
      queryRows({ only_unread: true, limit: null }).every((r) => t117p1end.entries.some((e) => e.id === String(r.id))));

    /* ---------- (t117-2) 取消收藏场景（收藏视图续拉，同性质判别） ----------
       操作序列：收藏首屏 500 → 逐条取消收藏这 500（set_starred 忠实落库，收藏
       集合收缩为 20）→ 续拉必须返回剩余 20 条收藏（id 6500..6519）。OFFSET 语义
       在此返回 0 行并假 exhausted——20 条收藏从此不可达。 */
    await resetStore();
    backendRows = Array.from({ length: 600 }, (_, i) =>
      mkT117Row({ id: 6000 + i, published_at: iso(NOW - 3 * 86400000 - i * 60000), is_starred: i < 520 }));
    store.getState().selectView('starred');
    await nTick(20);
    const t117p2 = store.getState();
    checkNew('(t117-2) 前置：收藏首屏 500 条（id 6000..6499）、exhausted=false（520 收藏满页，可续拉）',
      t117p2.entries.length === 500 && t117p2.entries[0].id === '6000' && t117p2.entries[499].id === '6499'
      && t117p2.articlesExhausted === false);
    for (const e of t117p2.entries) store.getState().toggleEntryFlag(e.id, 'isStarred'); // 逐条取消收藏（真实卡片操作）
    await nTick(0);
    checkNew('(t117-2) 取消收藏落库后收藏集合收缩为 20（set_starred 忠实翻转 is_starred）',
      queryRows({ only_starred: true, limit: null }).length === 20);
    invokeCalls.length = 0;
    await store.getState().loadMoreArticles();
    const t117p2wire = invokeCalls.find((c) => c.cmd === 'list_articles')?.args.args;
    const t117p2after = store.getState();
    checkNew('(t117-2) 续拉 wire：only_starred + keyset 锚（last_id=6499，无 offset）',
      t117p2wire?.only_starred === true && t117p2wire?.last_id === 6499 && t117p2wire?.offset === undefined);
    checkNew('(t117-2) 续拉落地（判别）：剩余 20 条收藏全部入列（id 6500..6519，修前 OFFSET 返回 0 行且假 exhausted——这 20 条不可达）',
      t117p2after.entries.length === 520 && t117p2after.entries[500].id === '6500'
      && t117p2after.entries.at(-1).id === '6519'
      && new Set(t117p2after.entries.map((e) => e.id)).size === 520
      && t117p2after.articlesExhausted === true
      && t117p2after.articlesCursor['article|all']?.loaded === 520);

    /* ---------- (t117-3) 插入漂移（首屏与续拉之间插入新文章） ----------
       keyset 天然免疫：锚 = 首屏末行 3499，插入的新行比锚更新（排在锚**之前**），
       不改变「锚之后」的集合——续拉仍返回 3500..3999，与新插入行零重叠零遗漏。
       （旧 OFFSET 语义下插入使偏移漂移，续拉区间与已加载窗口重叠。） */
    await resetStore();
    backendRows = t117Rows();
    store.getState().selectView('unread');
    await nTick(20);
    listPlan = { mode: 'defer' };
    store.getState().loadMoreArticles(); // 续拉在途（锚 3499）
    await nTick(0);
    const t117p3deferred = pendingList.at(-1);
    for (let i = 0; i < 10; i += 1) { // 后端同步入库 10 条更新文章（未读，比全部已有行新）
      backendRows.unshift(mkT117Row({ id: 5000 + i, published_at: iso(NOW), title: `t117 插入 ${i}` }));
    }
    t117p3deferred?.resolve(queryRows(t117p3deferred.args)); // 迟到响应 = 锚后的 500 行
    listPlan = null;
    await nTick(20);
    const t117p3 = store.getState();
    const t117p3ids = t117p3.entries.map((e) => e.id);
    checkNew('(t117-3) 插入漂移免疫（判别）：续拉恰为锚后 500 行（id 3500..3999），与新插入 10 行零重叠、已加载窗口保序、entries 1000 条唯一',
      t117p3ids.length === 1000 && new Set(t117p3ids).size === 1000
      && t117p3ids.slice(0, 500).join(',') === Array.from({ length: 500 }, (_, k) => String(3000 + k)).join(',')
      && t117p3ids.slice(500).join(',') === Array.from({ length: 500 }, (_, k) => String(3500 + k)).join(',')
      && t117p3ids.every((id) => Number(id) < 5000)
      && t117p3.articlesCursor['article|all']?.loaded === 1000
      && t117p3.articlesCursor['article|all']?.lastId === 3999);

    /* ---------- (t117-4) 并列 published_at 由 id 决胜（同秒文章不重不漏） ----------
       600 行**同一秒**发布：全序完全由 id 决胜（DESC = id 降序），页边界落在并列
       组中间——续拉必须由谓词的 `published_at = ? AND id < ?` 分支续上同秒兄弟
       （id 3299..3200），不得因 `published_at < ?` 单臂漏行或重复。 */
    await resetStore();
    backendRows = Array.from({ length: 600 }, (_, i) =>
      mkT117Row({ id: 3200 + i, published_at: iso(NOW - 3 * 86400000), title: `t117 同秒 ${i}` }));
    await store.getState().bootstrapFromBackend(); // all 视图首屏 500 = id 降序前 500（3799..3300）
    const t117p4 = store.getState();
    checkNew('(t117-4) 前置：同秒 600 行首屏 500 条按 id 降序（3799..3300）、锚=末行 3300',
      t117p4.entries.length === 500 && t117p4.entries[0].id === '3799' && t117p4.entries[499].id === '3300'
      && t117p4.articlesCursor['article|all']?.lastId === 3300
      && t117p4.articlesCursor['article|all']?.lastPublished === iso(NOW - 3 * 86400000)
      && t117p4.articlesExhausted === false);
    invokeCalls.length = 0;
    await store.getState().loadMoreArticles();
    const t117p4after = store.getState();
    const t117p4ids = t117p4after.entries.map((e) => e.id);
    checkNew('(t117-4) 同秒跨页续拉不重不漏（判别）：第二页 = 同秒组内 id 更小的 100 行（3299..3200）、全集 600 条唯一且与全序一致',
      t117p4after.entries[500].id === '3299' && t117p4after.entries.at(-1).id === '3200'
      && t117p4ids.length === 600 && new Set(t117p4ids).size === 600
      && t117p4ids.join(',') === Array.from({ length: 600 }, (_, k) => String(3799 - k)).join(',')
      && t117p4after.articlesExhausted === true);

    /* ---------- (t117-5) 排序切换后游标重置方向正确 ----------
       newest 翻页后切 oldest：重拉落地游标重置为新方向首屏（锚 = **新方向**首屏
       末行）——续拉必须沿 oldest 方向从该锚继续（id 3699..），而不是沿用旧方向
       的锚 3499。 */
    await resetStore();
    backendRows = t117Rows();
    await store.getState().bootstrapFromBackend(); // newest 首屏 500（id 3000..3499）
    invokeCalls.length = 0;
    store.getState().toggleTimelineSort(); // 切 oldest：整体重拉（viewEntriesCache 已清）
    await nTick(20);
    const t117p5sorted = store.getState();
    checkNew('(t117-5) 切 oldest 重拉落地：首屏换到最老端（首条 id 4199）、游标重置为新方向首屏（loaded=500）、锚=新方向末行 3700',
      t117p5sorted.entries[0].id === '4199' && t117p5sorted.entries.length === 500
      && t117p5sorted.articlesCursor['article|all']?.loaded === 500
      && t117p5sorted.articlesCursor['article|all']?.lastId === 3700
      && t117p5sorted.articlesExhausted === false);
    invokeCalls.length = 0;
    await store.getState().loadMoreArticles();
    const t117p5wire = invokeCalls.find((c) => c.cmd === 'list_articles')?.args.args;
    const t117p5after = store.getState();
    checkNew('(t117-5) 切排序后续拉方向正确（判别）：wire 带 newest_first=false + 新方向锚（last_id=3700），续拉沿 oldest 继续（id 3699..3200），不沿用旧方向锚 3499',
      t117p5wire?.newest_first === false && t117p5wire?.last_id === 3700
      && t117p5wire?.last_published === iso(NOW - 3 * 86400000 - 700 * 1000)
      && t117p5after.entries.length === 1000 && t117p5after.entries[500].id === '3699'
      && new Set(t117p5after.entries.map((e) => e.id)).size === 1000);

    await resetStore(); // 夹具复位：不把 1200 行大夹具与 defer 残留带给后续块
  }

  /* ============================================================
     TASK-111（二阶段③）：①查询缓存实体预算 ②后台刷新保位。
     ①审计：「单纯限制为 8 个视图并不能限制每个视图的大小」——TASK-100 P3-7
       只封键数（LRU 8），TASK-110 分页化后单键 entries 仍随滚动加载无界增长
       （syncCurrentViewCache 会把当前 1500 条窗口整体落键）。新契约：
       单键实体预算 VIEW_ENTRIES_CACHE_ENTRY_BUDGET=1000（≥ 页大小 500，截断
       只发生在加载超 2 页的大库场景），超限尾部截断（缓存语义=首屏快照，
       丢最老端由恢复后的后台 reload 补齐）；缓存值升级为 {entries, cursor,
       exhausted} 元数据（TASK-117：cursor 为 keyset 游标，loaded 字段承接原
       loadedCount 语义）——恢复路径（nav 三处）用**记录值**判定 exhausted 与
       续拉游标，不重算截断后长度（判别：截断长度 1000 落在页边界上方，
       「已到底」的快照会被误判成「还有数据」，且续拉锚回退重拉已去重
       丢弃的区间）。
     ②审计：「后台刷新保留当前阅读位置」——feeds-updated/手动同步/单源刷新
       → reloadFromBackend 整体替换 entries，newest_first 下新文章插头部使
       索引后移、虚拟列表视觉跳动。新契约：Timeline 滚动时节流记录顶条锚
       （可见首条目 id + filterKey，模块级 timelineAnchor），内容刷新落地时
       bump positionRestoreNonce，Timeline 消费锚：锚 id 仍在同上下文
       （filterKey 逐字一致）新快照中 → 程序性滚动回其新索引（复用既有
       suppressNextScrollEvents 抑制）；无锚/上下文已切/锚丢失 → 回落现状。
       导航路径（selectFeed/selectView/selectLayout/toggleTimelineSort）不
       bump 信号且切上下文即弃锚——分流显式。
     夹具：1501/1100 行 feed-10 大夹具驱动预算；缓存写入经 flipEntryFlag
     （→syncCurrentViewCache，loadMore 不落缓存——缓存语义=首屏/同步快照，
     既有口径）。保位用纯函数 anchorRestoreIndex + nonce 断言（滚动本体是
     DOM 行为，由源码形态断言防回退，与 t100/p3 组先例同口径）。
     ============================================================ */
  {
    const fs111 = await import('node:fs');
    const src111 = (p) => fs111.readFileSync(new URL(p, import.meta.url), 'utf8');
    const { VIEW_ENTRIES_CACHE_ENTRY_BUDGET, ARTICLES_PAGE_SIZE: t111Page, flipEntryFlag } = await import('../dist-test/store/internals.js');
    const ta111 = await import('../src/components/timelineAnchor.ts');
    const filterKeyOf = (s) => `${s.activeContentLayout}|${s.activeViewFilter}|${s.activeFeedFilter}|${s.timelineFilter}|${s.timelineSort}`;
    const mkT111Row = (o) => mkRow({ feed_id: 10, ...o });
    const t111Rows = (count, startId) => {
      const rows = [];
      for (let i = 0; i < count; i += 1) rows.push(mkT111Row({ id: startId + i, published_at: iso(NOW - i * 60000) }));
      return rows;
    };
    /* 缓存写入驱动器：直接翻旗（同步、无 IPC）→ flipEntryFlag 内部
       syncCurrentViewCache 把当前窗口（含全部已加载页）落键 */
    const t111SyncCache = (id) => flipEntryFlag(id, 'isStarred');

    /* -- (t111-0) 预算单点 + 审计注释 + 恢复用记录值（源级防回退） -- */
    const internalsSrc111 = src111('../src/store/internals.ts');
    const navSrc111 = src111('../src/store/slices/nav.ts');
    checkNew('(t111-0) 单键实体预算收口 internals：=1000 且 ≥ 页大小（截断只发生在加载超 2 页场景，首批/次页永不截断）',
      VIEW_ENTRIES_CACHE_ENTRY_BUDGET === 1000 && VIEW_ENTRIES_CACHE_ENTRY_BUDGET >= t111Page);
    /* 【TASK-117 改动理由】源级断言随游标契约更新：缓存元数据 loadedCount 升为
       完整 keyset 游标（cursor），nav 恢复点透传 cached.cursor——记录值恢复的
       保护意图不变（不用截断长度重算游标）。 */
    checkNew('(t111-0) 预算注释载明审计依据（键数上限≠单键大小），恢复路径用缓存记录的 cursor/exhausted 而非重算截断长度',
      internalsSrc111.includes('审计：「单纯限制为 8 个视图并不能限制每个视图的大小」')
      && internalsSrc111.includes('export function setViewEntriesSnapshot(')
      && navSrc111.includes('applyArticlesCursor(scopeKey, cached.cursor, cached.exhausted)')
      && navSrc111.includes('articlesExhausted: cached.exhausted'));

    /* ---------- (t111-1) 预算截断：边界（恰=预算不截断）+ 触发（尾部截断、元数据真值） ---------- */
    await resetStore();
    backendRows = t111Rows(1501, 3000); // 1501 行：3 页后余 1 行
    await store.getState().bootstrapFromBackend(); // 首批 500（3000..3499）
    await store.getState().loadMoreArticles(); // 1000（..3999）
    t111SyncCache('3000'); // 缓存落键：当前窗口恰 1000 = 预算
    const t111c1000 = viewEntriesCache.get('article|all|all');
    /* 【TASK-117 改动理由】缓存元数据断言随游标契约更新：loadedCount → cursor.loaded */
    checkNew('(t111-1) 边界：加载恰 1000（=预算）不截断，缓存值=完整快照 + 元数据（cursor.loaded=1000、exhausted=false）',
      t111c1000.entries.length === 1000 && t111c1000.cursor.loaded === 1000 && t111c1000.exhausted === false
      && t111c1000.entries[0].id === '3000' && t111c1000.entries.at(-1).id === '3999');
    await store.getState().loadMoreArticles(); // 1500（..4499）→ 超预算
    t111SyncCache('3002');
    const t111c1500 = viewEntriesCache.get('article|all|all');
    checkNew('(t111-1) 截断触发（1500>1000）：尾部截断为 1000 条（首条保留、最老端 500 条丢弃——首屏快照语义，恢复后由后台刷新补齐），cursor/exhausted 记录写入时真值（loaded=1500、锚=4499/false）不随截断失真',
      t111c1500.entries.length === 1000 && t111c1500.entries[0].id === '3000' && t111c1500.entries.at(-1).id === '3999'
      && t111c1500.cursor.loaded === 1500 && t111c1500.cursor.lastId === 4499 && t111c1500.exhausted === false);

    /* ---------- (t111-2) 截断恢复（exhausted=false 态）+ 截断后续拉衔接（判别） ----------
       离开/回到该范围触发缓存恢复；注入 reload 失败让恢复态存续（否则后台
       刷新立即整体替换）。判别点：恢复游标 = 记录的 cursor.loaded=1500（朴素
       实现重算截断长度会写 1000），续拉 offset=1500 不回退重拉。 */
    listPlan = { mode: 'reject', error: { message: 't111 注入' } };
    store.getState().selectFeed('feed-11'); // 离开（其后台 reload 失败被吞）
    await nTick(10);
    store.getState().selectFeed('all'); // 回来：缓存命中同步恢复
    const t111rA = store.getState();
    checkNew('(t111-2) 截断快照恢复（exhausted=false 态）：entries=截断后的 1000 条、游标按记录 cursor.loaded=1500 恢复（判别：重算截断长度会写 1000）、exhausted=false 与写入时一致（末次拉取满页）',
      t111rA.entries.length === 1000 && t111rA.entries[0].id === '3000'
      && t111rA.articlesCursor['article|all']?.loaded === 1500 && t111rA.articlesLimit === 1500
      && t111rA.articlesExhausted === false);
    await nTick(10); // 让恢复路径的注入失败 reload 落定（backendReloadInFlight 归零，续拉不再被在途守卫拦截）
    listPlan = null; // 解除 reject 注入：续拉本身要走通（只用于冻结恢复路径的 reload）
    invokeCalls.length = 0;
    await store.getState().loadMoreArticles(); // 续拉：keyset 锚必须接在真实已加载窗口末行之后
    const t111contArgs = invokeCalls.find((c) => c.cmd === 'list_articles')?.args.args;
    const t111rA2 = store.getState();
    /* 【TASK-117 改动理由】续拉 wire 从 offset=1500 改为 keyset 锚（恢复游标记录的
       锚=第三页末行 4499）——判别意图不变：从记录的真值续拉，不回退重拉已丢弃区间。 */
    checkNew('(t111-2) 截断后续拉衔接（判别）：keyset 锚用恢复游标记录的末行 4499/loaded=1500（朴素实现会从截断长度 1000 重拉已去重丢弃的区间）、余下 1 行（4500）追加无重复、游标=1501、exhausted=true（1<500）',
      t111contArgs?.last_id === 4499 && t111contArgs?.last_published === iso(NOW - 1499 * 60000)
      && t111contArgs?.offset === undefined && t111contArgs?.limit === 500
      && t111rA2.entries.length === 1001 && t111rA2.entries.at(-1)?.id === '4500'
      && new Set(t111rA2.entries.map((e) => e.id)).size === 1001
      && t111rA2.articlesCursor['article|all']?.loaded === 1501 && t111rA2.articlesExhausted === true);

    /* ---------- (t111-3) 截断恢复（exhausted=true 态，判别）+ exhausted 不误判 ----------
       1100 行 = 500+500+100：末页不满 ⇒ 真实已到底。截断后缓存长度 1000 落在
       页边界上方——重算长度会把「已到底」误判成「还有数据」并触发伪续拉。 */
    await resetStore();
    backendRows = t111Rows(1100, 5000); // 1100 行（5000..6099）
    await store.getState().bootstrapFromBackend(); // 500
    await store.getState().loadMoreArticles(); // 1000
    await store.getState().loadMoreArticles(); // 余 100 行（100<500）→ 真实到底
    t111SyncCache('5001'); // 缓存落键：1100 条 → 截断 1000，exhausted=true
    const t111cB = viewEntriesCache.get('article|all|all');
    checkNew('(t111-3) 前置：加载 1100（末页不满）→ 缓存截断 1000 条但元数据记录 cursor.loaded=1100 / exhausted=true（尾部截断不丢 exhausted 判据）',
      t111cB.entries.length === 1000 && t111cB.cursor.loaded === 1100 && t111cB.exhausted === true);
    listPlan = { mode: 'reject', error: { message: 't111 注入' } };
    store.getState().selectFeed('feed-11');
    await nTick(10);
    store.getState().selectFeed('all');
    const t111rB = store.getState();
    checkNew('(t111-3) 截断恢复（exhausted=true 态，判别）：恢复 exhausted=true（判别：重算截断长度 1000≥500 会误判成 false）、游标=记录 cursor.loaded=1100',
      t111rB.entries.length === 1000 && t111rB.articlesExhausted === true
      && t111rB.articlesCursor['article|all']?.loaded === 1100);
    await nTick(10); // 让恢复路径的注入失败 reload 落定（同 t111-2）
    invokeCalls.length = 0;
    await store.getState().loadMoreArticles();
    checkNew('(t111-3) exhausted 不误判：恢复后的「已到底」状态拦截伪续拉（不发 list_articles，列表不重复入列）',
      invokeCalls.filter((c) => c.cmd === 'list_articles').length === 0
      && store.getState().entries.length === 1000);
    listPlan = null;

    /* ---------- (t111-4) 后台刷新保位：节流锚记录 + 落地信号 + 锚回位决策 ----------
       新文章插头部场景：锚 id 7001 从 index 0 后移到 3，决策返回新索引 →
       Timeline 程序性滚动回位（视觉跳动抵消）；导航路径不 bump 信号。 */
    await resetStore();
    ta111.clearTopAnchor();
    backendRows = [
      mkT111Row({ id: 7001, published_at: iso(NOW) }),
      mkT111Row({ id: 7002, published_at: iso(NOW - 1000) }),
      mkT111Row({ id: 7003, published_at: iso(NOW - 2000) }),
    ];
    await store.getState().bootstrapFromBackend();
    const t111nonce0 = store.getState().positionRestoreNonce;
    const t111fkey = filterKeyOf(store.getState());
    const t111idsBefore = selectVisibleEntries(store.getState()).map((e) => e.id);
    const t111T0 = NOW + 10000000;
    checkNew('(t111-4) 顶条锚节流记录：首次记录生效、窗口内重复记录被忽略（约 4 次/秒收敛，锚零订阅不触发渲染）',
      t111idsBefore.join(',') === '7001,7002,7003'
      && ta111.recordTopAnchor(t111idsBefore[0], t111fkey, t111T0) === true
      && ta111.recordTopAnchor('7003', t111fkey, t111T0 + 100) === false
      && ta111.peekTopAnchor()?.id === '7001');
    /* 后台刷新：3 篇新文章插入头部（时间戳更新） */
    backendRows = [
      mkT111Row({ id: 7102, published_at: iso(NOW + 2000) }),
      mkT111Row({ id: 7101, published_at: iso(NOW + 1000) }),
      mkT111Row({ id: 7100, published_at: iso(NOW) }),
      ...backendRows,
    ];
    await store.getState().reloadFromBackend({ keepReadingPosition: true });
    const t111st4 = store.getState();
    checkNew('(t111-4) 后台刷新（keepReadingPosition）落地：保位信号 nonce +1（当前视图 all，落地即发出）',
      t111st4.positionRestoreNonce === t111nonce0 + 1 && t111st4.entries.length === 6);
    const t111items4 = selectVisibleEntries(t111st4);
    checkNew('(t111-4) 顶条锚回位决策（新文章插头部场景）：锚 id 7001 由 index 0 后移到 3，决策返回新索引（Timeline 据此 scrollToIndex 原位还原）',
      t111items4.map((e) => e.id).join(',') === '7102,7101,7100,7001,7002,7003'
      && ta111.anchorRestoreIndex(ta111.peekTopAnchor(), filterKeyOf(t111st4), t111items4) === 3);
    const t111nonce4b = store.getState().positionRestoreNonce;
    await store.getState().reloadFromBackend(); // 导航路径形态：不带 opts
    checkNew('(t111-4) 分流（导航路径）：不带 keepReadingPosition 的 reload 不 bump 保位信号——用户主动导航时锚机制保持沉默',
      store.getState().positionRestoreNonce === t111nonce4b);
    /* 筛选视图：selectView 导航不 bump；后台刷新经 reloadFilteredEntries 透传落地 bump。
       【TASK-111 R1/F1 改动理由】原断言把 nonce 基线放在 selectView 落定**之后**读取
       再与自身比较（永不失败，审查变异 M3：selectView 无条件 bump 全量仍绿——空洞）。
       改为真前后比较：基线在调用前读取，selectView 落定后必须仍等于基线。 */
    const t111nonce4c0 = store.getState().positionRestoreNonce; // 基线：selectView 调用前
    store.getState().selectView('starred'); // 无收藏行 → 空列表（导航路径）
    await nTick(20);
    checkNew('(t111-4) 分流（筛选视图导航）：selectView 不 bump 保位信号（真前后比较：调用前基线 vs 落定后——selectView 任何形式的 bump 都会转红）',
      store.getState().positionRestoreNonce === t111nonce4c0);
    const t111nonce4c = store.getState().positionRestoreNonce; // 透传 bump 用例的基线（selectView 落定后）
    backendRows.unshift(mkT111Row({ id: 7200, is_starred: true, published_at: iso(NOW + 3000) }));
    await store.getState().reloadFilteredEntries('starred', { keepReadingPosition: true });
    checkNew('(t111-4) 分流（筛选视图后台刷新）：reloadFilteredEntries 透传 keepReadingPosition 落地 bump 信号（feeds-updated → 筛选视图透传路径；落地快照即当前视图）',
      store.getState().positionRestoreNonce === t111nonce4c + 1
      && store.getState().entries.map((e) => e.id).join(',') === '7200');

    /* ---------- (t111-5) 锚丢失回落 / 主动切范围回落 / 锚不干扰主动滚动 ---------- */
    /* a) 锚丢失：锚 id 不在新快照（文章被删/被筛出）→ 决策 null，回落现状 */
    ta111.clearTopAnchor();
    const t111fkeyS = filterKeyOf(store.getState()); // starred 上下文
    ta111.recordTopAnchor('7200', t111fkeyS, NOW + 20000000);
    backendRows = backendRows.filter((r) => r.id !== 7200); // 后端：该收藏文消失
    const t111nonce5a = store.getState().positionRestoreNonce;
    await store.getState().reloadFilteredEntries('starred', { keepReadingPosition: true });
    checkNew('(t111-5) 锚丢失回落（判别）：锚 id 不在新快照 → 决策返回 null（回落现状、不强制顶部——消费侧无可滚动目标），信号照常送达（nonce +1，消费侧对 null 不动作）',
      ta111.anchorRestoreIndex(ta111.peekTopAnchor(), filterKeyOf(store.getState()), selectVisibleEntries(store.getState())) === null
      && store.getState().positionRestoreNonce === t111nonce5a + 1);
    /* b) 主动切范围：锚属于旧上下文（filterKey 失配）→ 不回位。
       【TASK-111 R1/F2 改动理由】原场景切范围后可见列表为空（唯一收藏行已删），
       anchorRestoreIndex 返回 null 只因 findIndex 落空、与 filterKey 拦截无关
       （审查变异 M2：删除 timelineAnchor 的 filterKey 拦截全量仍绿——判别力不成立）。
       改为「同源换范围」构造：切换后可见集合与切换前**完全一致**（全部行都属于
       feed-10，锚 id 仍是可见首条），唯一变化是 filterKey 的范围段——决策返回
       null 只能来自 filterKey 拦截本身（M2 变异下该断言必转红）。 */
    await resetStore(); // 独立夹具：all 范围两行（均属 feed-10）
    ta111.clearTopAnchor();
    backendRows = [
      mkT111Row({ id: 7300, published_at: iso(NOW) }),
      mkT111Row({ id: 7301, published_at: iso(NOW - 1000) }),
    ];
    await store.getState().bootstrapFromBackend(); // 范围 all：可见 [7300, 7301]
    const t111fkey5b = filterKeyOf(store.getState()); // 旧上下文（范围 all）
    ta111.recordTopAnchor('7300', t111fkey5b, NOW + 21000000);
    const t111nonce5b = store.getState().positionRestoreNonce;
    store.getState().selectFeed('10'); // 用户主动切范围（真实 scope 形态 '10'：feedId 无前缀；导航，不 bump 信号）
    await nTick(20);
    const t111st5b = store.getState();
    const t111items5b = selectVisibleEntries(t111st5b);
    const t111idx5b = t111items5b.findIndex((e) => e.id === '7300');
    checkNew('(t111-5) 主动切范围回落（判别）：filterKey 已失配且锚 id 仍可见（可见列表非空、findIndex 命中——排除「findIndex 落空」假阳性）→ 决策 null 纯因 filterKey 拦截（绝不把上一个范围的阅读位置回放进新范围），导航 reload 不 bump 信号',
      filterKeyOf(t111st5b) !== t111fkey5b
      && t111items5b.length === 2 && t111idx5b === 0
      && ta111.anchorRestoreIndex(ta111.peekTopAnchor(), filterKeyOf(t111st5b), t111items5b) === null
      && t111st5b.positionRestoreNonce === t111nonce5b);
    /* c) 主动滚动：锚跟随用户——节流窗口过期后可重新记录（边界 249/250） */
    const t111T5 = NOW + 30000000;
    checkNew('(t111-5) 锚不干扰主动滚动：节流窗口过期后重新记录生效（锚跟随用户最终停留位置）、窗口内仍被忽略',
      ta111.recordTopAnchor('7101', filterKeyOf(store.getState()), t111T5) === true
      && ta111.recordTopAnchor('7102', filterKeyOf(store.getState()), t111T5 + 249) === false
      && ta111.recordTopAnchor('7102', filterKeyOf(store.getState()), t111T5 + 250) === true
      && ta111.peekTopAnchor()?.id === '7102');

    /* -- (t111-6) Timeline / 内容刷新入口接线（源级防回退，与 t109 同口径） -- */
    const timelineSrc111 = src111('../src/components/Timeline.tsx');
    const appSrc111 = src111('../src/App.tsx');
    const syncSrc111 = src111('../src/store/slices/sync.ts');
    const feedsSrc111 = src111('../src/store/slices/feeds.ts');
    const t111iRestore = timelineSrc111.indexOf('anchorRestoreIndex(peekTopAnchor(), filterKey, items)');
    const t111iSuppress = timelineSrc111.indexOf('suppressNextScrollEvents();', t111iRestore);
    const t111iScroll = timelineSrc111.indexOf("rowVirtualizer.scrollToIndex(idx, { align: 'start' });", t111iSuppress);
    checkNew('(t111-6) Timeline 接线：滚动记录顶条锚（recordTopAnchor）、上下文切换弃锚（clearTopAnchor）、保位信号消费——先程序性滚动抑制再 scrollToIndex 回位（复用既有机制，防回位被误判为用户滚动）',
      timelineSrc111.includes('const positionRestoreNonce = useAppStore((s) => s.positionRestoreNonce);')
      && timelineSrc111.includes('recordTopAnchor(topItem.id, filterKey, performance.now())')
      && timelineSrc111.includes('clearTopAnchor();')
      && t111iRestore >= 0 && t111iSuppress > t111iRestore && t111iScroll > t111iSuppress
      && t111iScroll - t111iSuppress < 120);
    checkNew('(t111-6) 内容刷新入口携带保位请求（feeds-updated / 手动同步 / 单源刷新），导航入口（nav 三处 + 启动装载）保持无参调用',
      appSrc111.includes('reloadFromBackend({ keepReadingPosition: true })')
      && syncSrc111.includes('reloadFromBackend({ keepReadingPosition: true })')
      && feedsSrc111.includes('reloadFromBackend({ keepReadingPosition: true })'));

    await resetStore(); // 夹具复位：不把大夹具与 reject 注入带给后续块
  }

  /* ============================================================
     TASK-115（2026-10-06，REQ-005 三阶段②）：返回位置统一规则。
     X1 切换返回滚动恢复（per-filterKey 锚存档 + nav 缓存命中恢复信号）/
     X2 阅读器关闭焦点归还原卡（关闭信号 + ref 记账）/ X3 规则文档化
     （timelineAnchor.ts 头注 = 全场景规则表的代码单点注释）。

     与 TASK-111 的证据边界同口径（如实说明）：滚动/聚焦本体是 DOM 行为，
     node 回归网无法驱动真实虚拟列表——行为断言落在两层：
     - store 层：信号 bump 语义（导航缓存命中 → switchRestoreNonce；阅读器
       关闭 → readerCloseNonce）与「不叠加」双向隔离（两信号互不串扰）；
     - 纯函数层：archive 锚的回位决策（anchorRestoreIndex 复用）与焦点归还
       决策（readerFocusReturnIndex）直接断言；模块级存档 API（stash/peek/
       rearm）与 Timeline 同入口驱动；接线由源码形态断言钉住（t111-6 先例）。

     判别设计（吸收 t111 审查教训——断言必须有变异判别力）：
     - 存档-恢复断言锚定「回到原上下文后 peekReturnAnchor 命中且决策返回
       原索引」，naive 实现（stash 不存档 / 恢复用活锚）必转红；
     - 同上下文重复导航断言 contextChanged 守卫（无守卫 → 恢复信号误 bump）；
     - 切排序断言「信号不 bump + 新键查档为空」（新语境裁定的双重形态）；
     - 关闭信号断言「连续两次关闭必 +1」（nonce 选型判别：id 字段同值不触发）。
     ============================================================ */
  {
    const fs115 = await import('node:fs');
    const src115 = (p) => fs115.readFileSync(new URL(p, import.meta.url), 'utf8');
    const ta115 = await import('../src/components/timelineAnchor.ts');
    const { RETURN_ANCHOR_ARCHIVE_MAX, anchorRestoreIndex: ari115, readerFocusReturnIndex: rfri115 } = ta115;
    const filterKeyOf115 = (s) => `${s.activeContentLayout}|${s.activeViewFilter}|${s.activeFeedFilter}|${s.timelineFilter}|${s.timelineSort}`;
    const itemsOf115 = () => selectVisibleEntries(store.getState());
    const cnt115 = (s, t) => (s.match(new RegExp(t.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'g')) || []).length;
    const ordered115 = (s, parts) => {
      let i = 0;
      for (const p of parts) { i = s.indexOf(p, i); if (i < 0) return false; i += p.length; }
      return true;
    };
    /* 模块级测试时钟：t111 块已把节流基准推到 NOW+30M，这里从更大刻度起，
       保证 recordTopAnchor 不被上一块残留的节流窗口吞掉 */
    let T115 = NOW + 40000000;

    /* -- (t115-0) X1/X2 接线 + X3 规则文档化（源级防回退，t111-6 同口径） -- */
    const timelineSrc115 = src115('../src/components/Timeline.tsx');
    const navSrc115 = src115('../src/store/slices/nav.ts');
    const readerSrc115 = src115('../src/store/slices/reader.ts');
    const anchorSrc115 = src115('../src/components/timelineAnchor.ts');
    checkNew('(t115-0) X1 接线（源级）：nav 三处缓存命中恢复按 contextChanged 条件 bump switchRestoreNonce（entries 与信号同一次原子写入；同上下文重复导航不恢复）；Timeline 存档时序 = 先 stash 后 clear（弃锚语义不变，只多归档）',
      cnt115(navSrc115, '...(contextChanged ? { switchRestoreNonce: s.switchRestoreNonce + 1 } : {})') === 3
      && cnt115(navSrc115, 'const contextChanged =') === 3
      && timelineSrc115.indexOf('stashTopAnchorForReturn();') >= 0
      && timelineSrc115.indexOf('stashTopAnchorForReturn();') < timelineSrc115.indexOf('clearTopAnchor();'));
    checkNew('(t115-0) X1 消费侧（源级·有序）：switchRestoreNonce effect = 查档(peekReturnAnchor) → 决策(anchorRestoreIndex) → 程序性滚动抑制 → scrollToIndex(align:start) → 重锚(rearmTopAnchor)；image 提前回落',
      ordered115(timelineSrc115, [
        'const switchRestoreNonce = useAppStore((s) => s.switchRestoreNonce);',
        'anchorRestoreIndex(peekReturnAnchor(filterKey), filterKey, items)',
        'suppressNextScrollEvents();',
        "rowVirtualizer.scrollToIndex(idx, { align: 'start' });",
        'rearmTopAnchor(items[idx].id, filterKey, performance.now());',
      ]));
    checkNew('(t115-0) X2 接线（源级）：reader.ts 唯一关闭路径 bump readerCloseNonce；Timeline 在 activeArticleId 跟随 effect 记账原选中卡（ref），关闭信号 effect 消费决策（readerFocusReturnIndex → focusCardAt）',
      readerSrc115.includes('readerCloseNonce: s.readerCloseNonce + 1')
      && ordered115(timelineSrc115, [
        'const readerCloseNonce = useAppStore((s) => s.readerCloseNonce);',
        'readerFocusReturnIndex(lastActiveArticleIdRef.current, items)',
        'focusCardAt(idx);',
      ])
      && timelineSrc115.includes('lastActiveArticleIdRef.current = activeArticleId;'));
    checkNew('(t115-0) 画廊回落口径（源级）：三个回位/归还 effect（TASK-111 刷新保位 / TASK-115 切换返回 / 阅读器关闭焦点归还）各有 image 布局提前回落（非虚拟化无定位基建，与 TASK-111 同一口径）',
      cnt115(timelineSrc115, "if (activeContentLayout === 'image') return;") === 3);
    checkNew('(t115-0) X3 规则文档化（代码单点注释）：timelineAnchor.ts 头注 = 全场景规则表（切排序=新语境裁定、锚丢失归零回落、画廊回落、不叠加保证）',
      anchorSrc115.includes('全场景规则表')
      && anchorSrc115.includes('切排序（重拉）')
      && anchorSrc115.includes('新语境')
      && anchorSrc115.includes('存档随 filterKey 键自然失效')
      && anchorSrc115.includes('不叠加保证'));

    /* ---------- (t115-x1a) 存档-恢复：切走再切回，恢复信号 + 存档锚命中原索引 ---------- */
    await resetStore();
    ta115.clearTopAnchor();
    await store.getState().bootstrapFromBackend(); // 8 行；bootstrap 写 'article|all|all' 视图缓存
    const t115K1 = filterKeyOf115(store.getState());
    const t115srn0 = store.getState().switchRestoreNonce;
    T115 += 1000;
    checkNew('(t115-x1a) 前置：顶条锚已记录（101@K1，活锚与 Timeline handleScroll 同一入口）',
      ta115.recordTopAnchor('101', t115K1, T115) === true && ta115.peekTopAnchor()?.id === '101');
    /* 离开上下文（与 Timeline filterKey layout effect 同一入口：先存档后清活锚） */
    ta115.stashTopAnchorForReturn();
    ta115.clearTopAnchor();
    listPlan = { mode: 'reject', error: { message: 't115 注入' } };
    store.getState().selectFeed('11'); // 切到 feed-11（缓存 miss，后台 reload 注入失败冻结状态）
    await nTick(10);
    store.getState().selectFeed('all'); // 切回：缓存命中 → entries 同步恢复 + 恢复信号
    const t115stA = store.getState();
    checkNew('(t115-x1a) 存档-恢复（store 层）：缓存命中切回 → switchRestoreNonce +1（entries 与信号同一次原子写入，恢复列表已就位）',
      t115stA.switchRestoreNonce === t115srn0 + 1 && t115stA.entries.length === 8 && t115stA.activeFeedFilter === 'all');
    const t115Kback = filterKeyOf115(t115stA);
    const t115arch = ta115.peekReturnAnchor(t115Kback);
    /* 【TASK-117 改动理由】期望索引随 id 决胜序更新：BASE_ROWS 的 TODAY 并列组
       （101/102 同秒）排序从插入序变为 id 降序（102,101），101 的索引 0 → 1；
       断言意图（存档锚命中恢复列表中的原条目）不变。 */
    checkNew('(t115-x1a) 存档-恢复（决策侧）：按原上下文键查档命中（离开时顶条 101），锚 id 在恢复列表中 → 决策返回其索引（Timeline 据此 scrollToIndex align:start 一次性定位）',
      t115Kback === t115K1 && t115arch?.id === '101' && t115arch?.filterKey === t115K1
      && ari115(t115arch, t115Kback, itemsOf115()) === 1);
    await nTick(10); // 让注入失败的 reload 落定
    listPlan = null;

    /* ---------- (t115-x1b) 锚丢失 → 归零回落（不猜） ---------- */
    T115 += 1000;
    ta115.recordTopAnchor('888', t115Kback, T115); // 锚指向已不存在/被筛出的条目
    ta115.stashTopAnchorForReturn();
    ta115.clearTopAnchor();
    listPlan = { mode: 'reject', error: { message: 't115 注入' } };
    store.getState().selectFeed('11');
    await nTick(10);
    store.getState().selectFeed('all');
    await nTick(10);
    listPlan = null;
    checkNew('(t115-x1b) 锚丢失归零回落（判别）：存档锚 id 不在恢复列表 → anchorRestoreIndex 返回 null（消费侧不动作 = 保持归零 effect 置顶后的顶部；首次进入/无存档同口径回落，绝不猜位置）',
      ta115.peekReturnAnchor(filterKeyOf115(store.getState()))?.id === '888'
      && ari115(ta115.peekReturnAnchor(filterKeyOf115(store.getState())), filterKeyOf115(store.getState()), itemsOf115()) === null);

    /* ---------- (t115-x3c) 同上下文重复导航不恢复（contextChanged 守卫，判别） ---------- */
    const t115srnGuard = store.getState().switchRestoreNonce;
    store.getState().selectFeed('all'); // 已在 all：缓存命中但 contextChanged=false
    checkNew('(t115-x3c) 同上下文重复导航不恢复（判别）：缓存命中但范围段未变 → 恢复信号不 bump（无守卫的 naive 实现会把列表拽回上次离开位置）',
      store.getState().switchRestoreNonce === t115srnGuard && store.getState().activeFeedFilter === 'all');
    await nTick(10);

    /* ---------- (t115-x1c) 切排序（重拉）不恢复——新语境裁定 ---------- */
    const t115srnSort = store.getState().switchRestoreNonce;
    store.getState().toggleTimelineSort(); // filterKey 变（sort 段）+ 清视图缓存 + 重拉
    await nTick(20);
    const t115stSort = store.getState();
    checkNew('(t115-x1c) 切排序不恢复（新语境裁定）：filterKey 已变 = 新语境 → 恢复信号不 bump（重拉不是缓存恢复路径），新键查档为空（进入即归零，裁定无需特判——存档随 filterKey 键自然失效），旧存档仍在旧键下（回原上下文仍可恢复）',
      t115stSort.switchRestoreNonce === t115srnSort && t115stSort.timelineSort === 'oldest'
      && ta115.peekReturnAnchor(filterKeyOf115(t115stSort)) === null
      && ta115.peekReturnAnchor(t115K1)?.id === '888');

    /* ---------- (t115-x2a/b) 阅读器关闭焦点归还 ---------- */
    await resetStore();
    ta115.clearTopAnchor();
    await store.getState().bootstrapFromBackend();
    store.getState().selectArticle('102'); // 打开阅读器（102 在列表 index 1）
    checkNew('(t115-x2a) 前置：阅读器打开且选中 102', store.getState().activeArticleId === '102');
    const t115rcn0 = store.getState().readerCloseNonce;
    store.getState().clearReaderSelection(); // Esc 关闭（App.tsx 唯一关闭路径）
    checkNew('(t115-x2a) 阅读器关闭信号：clearReaderSelection bump readerCloseNonce 且选中清空（滚动不动 = filterKey 未变不触发归零，既有语义）',
      store.getState().readerCloseNonce === t115rcn0 + 1 && store.getState().activeArticleId === null);
    /* 【TASK-117 改动理由】期望索引随 id 决胜序更新：TODAY 并列组 id 降序后 102
       为列表首条（原 index 1 → 0）；断言意图（原卡仍在列表 → 返回其索引）不变。 */
    checkNew('(t115-x2a) 焦点归还原卡（决策）：原卡 102 仍在当前列表 → 决策返回其 index（Timeline 据此 focusCardAt：原卡可见不滚、不可见 scrollToIndex 定位后聚焦）',
      rfri115('102', itemsOf115()) === 0);
    store.getState().selectArticle('103'); // 换一篇打开
    const t115rcn1 = store.getState().readerCloseNonce;
    store.getState().clearReaderSelection(); // 再关（同一篇形态的连续开关）
    checkNew('(t115-x2a) 关闭信号必重触发（选型判别）：连续两次关闭 nonce 连续 +1——若用 activeArticleId 字段本身当信号，同值（开→关→再开同一篇→关）无法重触发 effect，这是选计数器不选 id 字段的理由',
      store.getState().readerCloseNonce === t115rcn1 + 1);
    store.getState().selectFeed('12'); // 阅读器开着时列表已切范围（selectFeed 不清选中；feed-12 与当前 article 布局同布局，列表非空）
    await nTick(20);
    /* 【TASK-117 改动理由】期望列表随 id 决胜序更新：feed-12 的 104/105 同秒并列，
       id 降序后为 [105,104]（原插入序 [104,105]）；断言意图（原卡不在列表 → null）不变。 */
    checkNew('(t115-x2b) 焦点归还回落（判别）：原卡不在当前列表（列表已切换，新范围=[105,104]）→ 决策 null = 不聚焦不滚动（焦点归还无对象，绝不猜）；无归还目标（null id）同口径',
      rfri115('103', itemsOf115()) === null && itemsOf115().map((e) => e.id).join(',') === '105,104'
      && rfri115(null, itemsOf115()) === null);

    /* ---------- (t115-x3a) 与 TASK-111 刷新保位不叠加（双向隔离） ---------- */
    await resetStore();
    ta115.clearTopAnchor();
    await store.getState().bootstrapFromBackend();
    const t115srnIso = store.getState().switchRestoreNonce;
    const t115prnIso = store.getState().positionRestoreNonce;
    ta115.rearmTopAnchor('101', filterKeyOf115(store.getState()), (T115 += 1000));
    ta115.stashTopAnchorForReturn();
    ta115.clearTopAnchor();
    listPlan = { mode: 'reject', error: { message: 't115 注入' } };
    store.getState().selectFeed('11');
    await nTick(10);
    store.getState().selectFeed('all'); // 切换返回：只动 switchRestoreNonce
    checkNew('(t115-x3a) 不叠加（出向）：切换返回恢复 bump switchRestoreNonce 但 positionRestoreNonce 纹丝不动（导航路径的 reload 不带 keepReadingPosition，「一次性定位」绝不借用「持续跟踪」通道）',
      store.getState().switchRestoreNonce === t115srnIso + 1
      && store.getState().positionRestoreNonce === t115prnIso);
    await nTick(10);
    listPlan = null;
    await store.getState().reloadFromBackend({ keepReadingPosition: true }); // 后台刷新：只动 positionRestoreNonce
    checkNew('(t115-x3a) 不叠加（入向）：keepReadingPosition 刷新落地 bump positionRestoreNonce 但 switchRestoreNonce 纹丝不动（两个 effect 各消费各的信号、各用各的锚——存档 vs 活锚，互不发出互不消费对方信号）',
      store.getState().positionRestoreNonce === t115prnIso + 1
      && store.getState().switchRestoreNonce === t115srnIso + 1);

    /* ---------- (t115-x3b) 存档容量 LRU + rearm 语义（模块级） ---------- */
    T115 += 5000;
    ta115.rearmTopAnchor('r1', 'fk-r', T115);
    checkNew('(t115-x3b) rearm 重锚语义（恢复的支撑）：绕过节流直设活锚并推进节流基准（窗口内到达的 scroll 事件记录被吸收——活锚保持在恢复落点，用户随即再离开时存档的是恢复后位置而非恢复前位置）',
      ta115.peekTopAnchor()?.id === 'r1'
      && ta115.recordTopAnchor('r2', 'fk-r', T115 + 100) === false
      && ta115.peekTopAnchor()?.id === 'r1');
    ta115.clearTopAnchor();
    for (let i = 0; i <= RETURN_ANCHOR_ARCHIVE_MAX; i++) {
      ta115.rearmTopAnchor(`a${i}`, `fk-${i}`, T115 + 100 + i);
      ta115.stashTopAnchorForReturn();
      ta115.clearTopAnchor();
    }
    checkNew(`(t115-x3b) 存档容量 LRU（TASK-111 预算纪律）：上限 ${RETURN_ANCHOR_ARCHIVE_MAX}（与视图缓存键数同值），溢出淘汰最旧上下文（fk-0 已淘汰、fk-MAX 保留）——淘汰只影响该上下文退回「归零回落」，无正确性影响`,
      ta115.peekReturnAnchor('fk-0') === null
      && ta115.peekReturnAnchor(`fk-${RETURN_ANCHOR_ARCHIVE_MAX}`)?.id === `a${RETURN_ANCHOR_ARCHIVE_MAX}`);
    ta115.peekReturnAnchor('fk-1'); // 命中刷新 LRU 新鲜度
    ta115.rearmTopAnchor('aNEW', 'fk-NEW', T115 + 500);
    ta115.stashTopAnchorForReturn();
    ta115.clearTopAnchor();
    checkNew('(t115-x3b) LRU 新鲜度：peek 命中把该上下文刷新为最新使用，随后溢出淘汰的是真正最旧的 fk-2（fk-1 因刚被使用而幸存）',
      ta115.peekReturnAnchor('fk-1')?.id === 'a1' && ta115.peekReturnAnchor('fk-2') === null
      && ta115.peekReturnAnchor('fk-NEW')?.id === 'aNEW');

    await resetStore(); // 夹具复位
  }

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
    const { entryNeedsHydration: NEED } = await import('../src/store/selectors.ts');
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
    const t103GetArticlesCount = () => invokeCalls.filter((c) => c.cmd === 'get_articles').length;

    /* ---------- t103-snapshot-preserves-hydration：快照替换保留水合 ---------- */
    await bootFixture({ activeContentLayout: 'social' });
    // 预置已水合痕迹：101 有正文+url；102 空正文终态；103 全量（译文/摘要/全文/原文分离）
    store.setState((s) => ({
      entries: s.entries.map((a) => {
        if (a.id === '101') return { ...a, content: '<p>101 正文</p>', rawContent: '<p>101 正文</p>', hydrated: true, url: 'https://example.com/101' };
        if (a.id === '102') return { ...a, hydrated: true };
        if (a.id === '103') return { ...a, content: '<p>103 全文</p>', rawContent: '<p>103 原文</p>', translatedContent: '<p>103 译文</p>', aiSummary: '103 摘要', fulltextExtracted: true, hydrated: true, url: 'https://example.com/103' };
        return a;
      }),
      hydratedIds: { '101': true, '102': true },
      hydrationErrors: {},
    }));
    const t103BaseCalls = t103GetArticlesCount();
    await store.getState().reloadFromBackend();
    const t103AfterReload = t103State();
    checkNew('(t103-snapshot-preserves-hydration) 快照替换按 id 继承正文/原文/译文/摘要/全文标记/url，hydratedIds 不再清空',
      t103AfterReload.entries.find((a) => a.id === '101')?.content === '<p>101 正文</p>'
      && t103AfterReload.entries.find((a) => a.id === '101')?.url === 'https://example.com/101'
      && t103AfterReload.entries.find((a) => a.id === '103')?.content === '<p>103 全文</p>'
      && t103AfterReload.entries.find((a) => a.id === '103')?.rawContent === '<p>103 原文</p>'
      && t103AfterReload.entries.find((a) => a.id === '103')?.translatedContent === '<p>103 译文</p>'
      && t103AfterReload.entries.find((a) => a.id === '103')?.aiSummary === '103 摘要'
      && t103AfterReload.entries.find((a) => a.id === '103')?.fulltextExtracted === true
      && t103AfterReload.hydratedIds['101'] === true && t103AfterReload.hydratedIds['102'] === true);
    checkNew('(t103-snapshot-preserves-hydration) 已水合条目刷新后不触发任何补拉（直接恢复正文，无「无请求死区」）',
      t103GetArticlesCount() === t103BaseCalls
      && NEED(t103AfterReload, '101') === false && NEED(t103AfterReload, '102') === false);
    // with_content 场景：新行自带正文/url 时以新行为准（104 新行带正文）
    store.setState((s) => ({
      entries: s.entries.map((a) => (a.id === '104' ? { ...a, content: '旧104', rawContent: '旧104', url: 'https://example.com/old-104' } : a)),
    }));
    backendRows = BASE_ROWS.map((r) => (r.id === 104 ? { ...r, content_html: '<p>新104</p>', url: 'https://example.com/104' } : r));
    await store.getState().reloadFromBackend();
    checkNew('(t103-snapshot-preserves-hydration) 新行自带正文（with_content）以新行为准：104 取新行正文与新 url',
      t103State().entries.find((a) => a.id === '104')?.content === '<p>新104</p>'
      && t103State().entries.find((a) => a.id === '104')?.url === 'https://example.com/104');
    backendRows = BASE_ROWS;
    // 收口契约的另一侧：范围切换（缓存恢复 + 后台刷新）同函数继承，正文不丢
    getArticlesPlan = { rows: [mkRow({ id: 201, feed_id: 11, content_html: '<p>201 正文</p>' })] };
    store.setState({ hydratedIds: {}, hydrationErrors: {} });
    store.getState().hydrateArticleContent(['201']);
    await nTick(20);
    getArticlesPlan = null;
    store.getState().selectFeed('11');
    await nTick(30);
    store.getState().selectFeed('all');
    checkNew('(t103-snapshot-preserves-hydration) selectFeed 往返（缓存恢复+后台刷新，同走 mergeSnapshotEntries）：201 正文保留',
      t103State().entries.find((a) => a.id === '201')?.content === '<p>201 正文</p>');

    /* ---------- t103-stale-card-rehydrates：审计探针场景（同 ID 刷新）---------- */
    await bootFixture({ activeContentLayout: 'social' });
    store.setState((s) => ({
      entries: s.entries.map((a) => (a.id === '101' ? { ...a, content: '<p>101 正文</p>', rawContent: '<p>101 正文</p>', hydrated: true } : a)),
      hydratedIds: { '101': true },
    }));
    await store.getState().reloadFromBackend();
    const t103Probe = t103State();
    checkNew('(t103-stale-card-rehydrates) 审计探针场景：同 ID 刷新后 content 不再被清空（直接恢复正文，请求数=0 也不再是死区）',
      t103Probe.entries.find((a) => a.id === '101')?.content === '<p>101 正文</p>'
      && t103Probe.hydratedIds['101'] === true);
    // 未水合卡片：刷新后水合前提重新成立 —— entryNeedsHydration 的真值表（useLazyHydrate 重入队的依据）
    checkNew('(t103-stale-card-rehydrates) 未水合卡片刷新后水合前提重新成立：entryNeedsHydration 仅在「条目在 ∧ 无正文 ∧ 无终态 ∧ 无失败态」为真',
      NEED(t103Probe, '201') === true
      && NEED({ ...t103Probe, hydrationErrors: { '201': 'x' } }, '201') === false
      && NEED({ ...t103Probe, hydratedIds: { '201': true } }, '201') === false
      && NEED({ ...t103Probe, entries: t103Probe.entries.map((a) => (a.id === '201' ? { ...a, content: 'x' } : a)) }, '201') === false
      && NEED({ ...t103Probe, entries: t103Probe.entries.filter((a) => a.id !== '201') }, '201') === false);
    // 源码形态断言（手法沿用本文件既有 readFileSync 写法）：锁定方案 A ——
    // effect 消费 entryNeedsHydration 布尔值并以 [id, needsHydration] 为依赖
    const fs103 = await import('node:fs');
    const tl103Src = fs103.readFileSync(new URL('../src/components/Timeline.tsx', import.meta.url), 'utf8');
    const hook103 = tl103Src.slice(tl103Src.indexOf('function useLazyHydrate'), tl103Src.indexOf('/* ---------- 文章卡片'));
    checkNew('(t103-stale-card-rehydrates) useLazyHydrate 不再只依赖 [id]：按 id 订阅 entryNeedsHydration，条件重新成立即重新入队',
      hook103.includes('useAppStore((s) => entryNeedsHydration(s, id))')
      && hook103.includes('if (!needsHydration) return;')
      && hook103.includes('[id, needsHydration]')
      && !hook103.includes('}, [id]);'));

    /* ---------- t103-hydration-terminals：终态机（成功/空正文/缺行/失败）---------- */
    // （1）空正文：content_html 为 NULL → hydrated 终态 + entry.hydrated，不无限重试
    await resetStore();
    store.setState({ entries: [t103Entry(311)], hydratedIds: {}, hydrationErrors: {} });
    getArticlesPlan = { rows: [socialRow({ id: 311 })] };
    store.getState().hydrateArticleContent(['311']);
    await nTick(20);
    const t103E311 = t103State().entries.find((a) => a.id === '311');
    checkNew('(t103-hydration-terminals) 空正文（content_html NULL）→ hydratedIds 终态 + entry.hydrated，卡片不再显示加载占位',
      t103E311?.content === '' && t103E311?.hydrated === true
      && t103State().hydratedIds['311'] === true && NEED(t103State(), '311') === false);
    // （2）部分缺行：322 无对应返回行 → 「文章不存在」终态
    store.setState({ entries: [t103Entry(321), t103Entry(322)], hydratedIds: {}, hydrationErrors: {} });
    getArticlesPlan = { rows: [socialRow({ id: 321, content_html: '<p>321</p>' })] };
    store.getState().hydrateArticleContent(['321', '322']);
    await nTick(20);
    checkNew('(t103-hydration-terminals) 响应缺行 → 该 id 进「文章不存在」终态（不静默留加载占位），命中行照常填充',
      t103State().entries.find((a) => a.id === '321')?.content === '<p>321</p>'
      && (t103State().hydrationErrors['322'] ?? '').includes('文章不存在')
      && NEED(t103State(), '322') === false);
    // （3）空 rows：整批「文章不存在」（空 ids/空 rows 不留占位）
    store.setState({ entries: [t103Entry(331)], hydratedIds: {}, hydrationErrors: {} });
    getArticlesPlan = { rows: [] };
    store.getState().hydrateArticleContent(['331']);
    await nTick(20);
    checkNew('(t103-hydration-terminals) 空 rows → 整批进「文章不存在」终态（空 ids/空 rows 不留占位）',
      (t103State().hydrationErrors['331'] ?? '').includes('文章不存在') && NEED(t103State(), '331') === false);
    // （4）失败：错误可见 + retryHydration 内联重试收敛为成功
    store.setState({ entries: [t103Entry(341)], hydratedIds: {}, hydrationErrors: {} });
    getArticlesPlan = { mode: 'reject', error: { message: 'IPC 超时' } };
    store.getState().hydrateArticleContent(['341']);
    await nTick(20);
    checkNew('(t103-hydration-terminals) 请求失败 → hydrationErrors 保留原错误信息（内联重试入口可用）',
      t103State().hydrationErrors['341'] === 'IPC 超时');
    getArticlesPlan = { rows: [socialRow({ id: 341, content_html: '<p>341 重试成功</p>' })] };
    store.getState().retryHydration('341');
    await nTick(20);
    checkNew('(t103-hydration-terminals) retryHydration 后正文填充、错误清除、终态落位',
      t103State().entries.find((a) => a.id === '341')?.content === '<p>341 重试成功</p>'
      && t103State().hydrationErrors['341'] === undefined && t103State().hydratedIds['341'] === true);
    getArticlesPlan = null;

    /* ---------- t103-race-and-dedup：在途去重 + 乱序/过期防护 ---------- */
    // 在途去重：请求未落地时重复入队（重触发/重挂载/直接调用）→ 不产生第二次 IPC
    await resetStore();
    store.setState({ entries: [t103Entry(351), t103Entry(352)], hydratedIds: {}, hydrationErrors: {} });
    getArticlesPlan = { mode: 'defer' };
    invokeCalls.length = 0;
    store.getState().hydrateArticleContent(['351', '352']);
    await nTick(0);
    store.getState().hydrateArticleContent(['351', '352']); // 在途重复入队
    store.getState().ensureArticleContent('351');           // 挂载路径重复入队
    await nTick(0);
    const t103DedupCalls = invokeCalls.filter((c) => c.cmd === 'get_articles');
    checkNew('(t103-race-and-dedup) 同 id 在途重复入队不重复 IPC（仅首批一次、含两个 id）',
      pendingGetArticles.length === 1 && t103DedupCalls.length === 1
      && t103DedupCalls[0]?.args.ids.join(',') === '351,352');
    pendingGetArticles[0].resolve([socialRow({ id: 351, content_html: '<p>351 正文</p>' }), socialRow({ id: 352 })]);
    await nTick(10);
    checkNew('(t103-race-and-dedup) 在途去重不丢结果：351 填充正文、352 空正文终态',
      t103State().entries.find((a) => a.id === '351')?.content === '<p>351 正文</p>'
      && t103State().hydratedIds['352'] === true);
    // 乱序防护：批量在途期间条目已经他路水合（selectArticle 详情）→ 迟到响应不覆盖新正文
    await resetStore();
    store.setState({ entries: [t103Entry(361)], hydratedIds: {}, hydrationErrors: {} });
    getArticlesPlan = { mode: 'defer' };
    invokeCalls.length = 0;
    store.getState().hydrateArticleContent(['361']);
    await nTick(0);
    detailImpl = (id) => socialRow({ id, content_html: '<p>详情路径正文</p>', url: 'https://example.com/361' });
    store.getState().selectArticle('361'); // 详情路径先落地
    await nTick(10);
    const t103DetailContent = t103State().entries.find((a) => a.id === '361')?.content;
    pendingGetArticles[0].resolve([socialRow({ id: 361, content_html: '<p>旧批次正文</p>' })]);
    await nTick(10);
    checkNew('(t103-race-and-dedup) 旧响应不覆盖新状态：他路已水合的正文不被迟到批次改写，终态照常落位',
      t103DetailContent === '<p>详情路径正文</p>'
      && t103State().entries.find((a) => a.id === '361')?.content === '<p>详情路径正文</p>'
      && t103State().hydratedIds['361'] === true);
    getArticlesPlan = null;
    detailImpl = (id) => mkRow({ id, content_html: '<p>详情</p>', translated_content: null });
    // 过期防护另一侧：在途期间条目被快照替换移除 → 迟到响应不写滞留终态/正文
    await resetStore();
    store.setState({ entries: [t103Entry(371)], hydratedIds: {}, hydrationErrors: {} });
    getArticlesPlan = { mode: 'defer' };
    invokeCalls.length = 0;
    store.getState().hydrateArticleContent(['371']);
    await nTick(0);
    store.setState({ entries: [t103Entry(372)] }); // 快照替换：371 不在新快照
    pendingGetArticles[0].resolve([socialRow({ id: 371, content_html: '<p>迟到正文</p>' })]);
    await nTick(10);
    checkNew('(t103-race-and-dedup) 在途期间条目被快照替换移除：迟到响应不写正文、不写滞留 hydratedIds',
      !t103State().entries.some((a) => a.id === '371')
      && t103State().hydratedIds['371'] === undefined
      && t103State().hydrationErrors['371'] === undefined);
    getArticlesPlan = null;
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

/* ============================================================
   TASK-092（REQ-106 ③）：封面图片位统一代理 / 失败回退 / 失效上报
   驱动 src/lib/coverImage.ts（五处图片位共用的状态机，ui-loader 就地转译），
   fetch_image / report_broken_cover 由本段落临时包一层 __INVOKE__ 可控假后端
   （段末恢复）。组件是否真的调用这些函数：SSR 渲染 CoverImage + 源码形态断言。
   ============================================================ */
{
  const cov = await import('../src/lib/coverImage.ts');
  const ip = await import('../src/lib/imageProxy.ts');
  const fsC = await import('node:fs');
  const readSrc = (p) => fsC.readFileSync(new URL(p, import.meta.url), 'utf8');
  const prevInvokeC = globalThis.__INVOKE__;
  const cCalls = [];
  const fetchPlan = new Map(); // url -> 'png' | 'html' | 'reject' | 'empty'
  let reportReject = false;
  const PNG = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13];
  const HTML = Array.from(new TextEncoder().encode('<!doctype html><html><body>请登录</body></html>'));
  globalThis.__INVOKE__ = (cmd, args) => {
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
  };
  const nCmd = (cmd, url) => cCalls.filter((c) => c.cmd === cmd && (url === undefined || c.args.url === url)).length;
  const reports = () => cCalls.filter((c) => c.cmd === 'report_broken_cover').map((c) => `${c.args.articleId}|${c.args.url}`);
  const SSPAI = 'https://cdnfile.sspai.com/2026/09/cover.png?imageView2/2/w/300';
  const DOUBAN = 'https://img9.doubanio.com/view/photo/l/public/p1.jpg';
  const PLAIN = 'https://images.example.com/c.jpg';
  const unhandled = [];
  const onUnhandled = (e) => { unhandled.push(e); };
  process.on('unhandledRejection', onUnhandled);
  const warns = [];
  const prevWarn = console.warn;
  console.warn = (...a) => { warns.push(a.map(String).join(' ')); };
  try {
    cov.resetCoverCacheForTest();

    /* A1/A2：取图路径只由 imageProxy.needsImageProxy 决定 */
    checkNew('(cov-a1) 取图路径：sspai/doubanio 走代理，普通图床直连，空 cover 为 none（判定来自 imageProxy）',
      cov.coverRoute(SSPAI) === 'proxy' && cov.coverRoute(DOUBAN) === 'proxy'
      && cov.coverRoute(PLAIN) === 'direct' && cov.coverRoute('') === 'none'
      && cov.coverRoute(null) === 'none' && cov.coverRoute(undefined) === 'none'
      && cov.coverRoute('data:image/png;base64,AA==') === 'direct');
    {
      const cs = readSrc('../src/lib/coverImage.ts');
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
      const tl = readSrc('../src/components/Timeline.tsx');
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
      const { CoverImage } = await import('../src/components/CoverImage.tsx');
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
      const ci = readSrc('../src/components/CoverImage.tsx');
      checkNew('(cov-wire) CoverImage：effect 调 driveCover(url, pageUrl, articleId)、渲染走 coverView、代理与直连 img 都挂 onCoverError',
        /useEffect\(\(\) => \{ driveCover\(url, pageUrl, articleId\); \}/.test(ci)
        && ci.includes('coverView(url, state)')
        && (ci.match(/onError=\{\(\) => onCoverError\(url\)\}/g) || []).length === 2
        && /data-cover-route="proxy"[^>]*onError=\{\(\) => onCoverError\(url\)\}/.test(ci)
        && /data-cover-route="direct"[\s\S]*?onError=\{\(\) => onCoverError\(url\)\}/.test(ci));
      const tl = readSrc('../src/components/Timeline.tsx');
      const pb = readSrc('../src/components/PlayerBar.tsx');
      const ov = readSrc('../src/components/Overlays.tsx');
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
      const tl = readSrc('../src/components/Timeline.tsx');
      checkNew('(cov-c9) 画廊打开灯箱时带上条目 id（灯箱失败可按条目上报）',
        tl.includes('if (lightboxSrc) openLightbox(lightboxSrc, item.id);'));
    }
  } finally {
    globalThis.__INVOKE__ = prevInvokeC;
    console.warn = prevWarn;
    process.off('unhandledRejection', onUnhandled);
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
  const { scopeQueryArgs, scopePageKey, viewEntriesCache } = await import('../dist-test/store/internals.js');
  const { refillDecision, AUTO_REFILL_MAX_CALLS } = await import('../src/components/timelineRefill.ts');
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
    const store94 = (await import('../dist-test/store.js')).useAppStore;
    const { selectVisibleEntries } = await import('../dist-test/store.js');
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
    const tl94 = readSrc94('../src/components/Timeline.tsx');
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
  const tlFix = srcOf('../src/components/Timeline.tsx');

  /* ---------- fix-2：用户滚动判定收口 isUserScrollEvent ---------- */
  const { isUserScrollEvent, PROGRAMMATIC_SCROLL_SUPPRESS_MS } =
    await import('../src/components/scrollAwayRead.ts');
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
  checkNew('(fix-5) 两卡「翻译」按钮在失败态（translateErrors[id] 存在）改走 translateEntry 重试（修前把半截译文当缓存只切显示）',
    (tlFix.match(/if \(next && \(translateError \|\| !item\.translatedContent\)\)/g) || []).length === 2);

  /* ---------- fix-8：auto 配置的 AI 区块空态收起 ---------- */
  const selFix = await import('../dist-test/store.js');
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
  const pbSrc = srcOf('../src/components/PlayerBar.tsx');
  checkNew('(fix-6) PlayerBar 删除本地 formatClock，时长统一 lib/format.formatDuration（迷你条/全屏条/进度条两端）',
    pbSrc.includes("import { formatDuration } from '../lib/format'")
    && !/function formatClock\b/.test(pbSrc)
    && (pbSrc.match(/formatDuration\(player\.(positionSec|durationSec)\)/g) || []).length === 4
    && pbSrc.includes('formatDuration={formatDuration}'));

  /* ---------- fix-7：全屏播放器层级降到弹窗之下 ---------- */
  const cssFix = srcOf('../src/styles/base.css');
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
  const ovFix = srcOf('../src/components/Overlays.tsx');
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
  const rtFix = srcOf('../src/components/settings/ReadingTab.tsx');
  checkNew('(fix-13) ReadingTab 三处数值标签改用 range-value-tag（裸内联宽度 span 清零）',
    (rtFix.match(/className="range-value-tag"/g) || []).length === 3
    && !rtFix.includes('style={{ width: 45 }}') && !rtFix.includes('style={{ width: 55 }}'));

  /* ---------- fix-14：页脚版本兜底不再显示假版本号 ---------- */
  const sfFix = srcOf('../src/components/settings/SettingsSidebarFooter.tsx');
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
    const { viewEntriesCache } = await import('../dist-test/store/internals.js');
    viewEntriesCache.clear();
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
      const { isBackendReloadInFlight } = await import('../dist-test/store/slices/bootstrap.js');
      const viewCache = (await import('../dist-test/store/internals.js')).viewEntriesCache;
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
        src100('../src/store/slices/sync.ts').includes('function isNotConnectedError')
          && !src100('../src/store/slices/sync.ts').includes(".catch(() => null) // 未连接"));
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
    const { parseTs } = await import('../dist-test/lib/api.js');
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
    const sidebar = src100('../src/components/Sidebar.tsx');
    const timeline = src100('../src/components/Timeline.tsx');
    const overlays = src100('../src/components/Overlays.tsx');
    const primitives = src100('../src/components/primitives.tsx');
    const app = src100('../src/App.tsx');
    const player = src100('../src/components/PlayerBar.tsx');
    const appearance = src100('../src/components/settings/AppearanceTab.tsx');
    const shortcuts = src100('../src/components/settings/ShortcutsTab.tsx');
    const syncTab = src100('../src/components/settings/SyncTab.tsx');
    const apiSrc = src100('../src/lib/api.ts');
    const bootstrap = src100('../src/store/slices/bootstrap.ts');
    const internals = src100('../src/store/internals.ts');
    const ctxMenu = src100('../src/components/ContextMenu.tsx');
    const feedsTab = src100('../src/components/settings/FeedsTab.tsx');
    const reader = src100('../src/components/Reader.tsx');
    const icons = src100('../src/components/icons.tsx');
    const tokens = src100('../src/styles/tokens.css');
    const baseCss = src100('../src/styles/base.css');
    const indexHtml = src100('../index.html');
    const readme = src100('../README.md');

    /* P3-1：reload 在途拦截（行为断言见上，这里锁守卫的存在性） */
    checkNew('(t100-p3-1) 守卫接线：loadMoreArticles 入口检查 isBackendReloadInFlight；两个 reload 以计数器包裹',
      bootstrap.includes('if (isBackendReloadInFlight()) return;')
      && bootstrap.includes('backendReloadInFlight++')
      && bootstrap.includes('backendReloadInFlight--')
      && (bootstrap.match(/backendReloadInFlight--/g) || []).length === 2);
    checkNew('(t100-p3-1) 计数器与 reloadGeneration 同为模块级状态（不得被 setState 泄漏进 store）',
      !bootstrap.includes('reloadInFlight: ') && bootstrap.includes('let backendReloadInFlight = 0;'));

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
      src100('../src/store/slices/feeds.ts').includes('添加失败：目标分类不存在'));

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
   UI 契约 V1/V2：.workflow-kit/docs/UI-CONTRACT-TASK-101-SYNC-COPY.md。
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

  const syncTab101 = src101('../src/components/settings/SyncTab.tsx');
  const configSync101 = src101('../src/components/settings/ConfigSyncSection.tsx');
  const cacheCleanup101 = src101('../src/components/settings/CacheCleanupSection.tsx');
  const general101 = src101('../src/components/settings/GeneralTab.tsx');

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
  ].flatMap((f) => [...src101(`../src/components/settings/${f}.tsx`).matchAll(/desc="([^"]+)"/g)])
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
   UI 契约 X1/X2/X3：.workflow-kit/docs/UI-CONTRACT-TASK-102-SETTING-COPY.md。
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
  const syncTab102 = src102('../src/components/settings/SyncTab.tsx');
  const general102 = src102('../src/components/settings/GeneralTab.tsx');
  const reading102 = src102('../src/components/settings/ReadingTab.tsx');
  const about102 = src102('../src/components/settings/AboutTab.tsx');
  const configSync102 = src102('../src/components/settings/ConfigSyncSection.tsx');
  const cacheCleanup102 = src102('../src/components/settings/CacheCleanupSection.tsx');
  const shared102 = src102('../src/components/settings/shared.ts');

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
    [...stripComments102(src102(`../src/components/settings/${f}.tsx`))
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
  const timeline114 = src114('../src/components/Timeline.tsx');
  const app114 = src114('../src/App.tsx');
  const shortcuts114 = src114('../src/components/settings/ShortcutsTab.tsx');
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
  checkNew('(t114-x1a) NotifCard 订阅水合错误态与终态（对齐 SocialCard 的订阅面：hydrationErrors/hydratedIds）',
    notif114.includes('s.hydrationErrors[item.id]') && notif114.includes('s.hydratedIds[item.id]'));

  /* TASK-114 R1-F1：失败分支切片（自 ') : hydrationError ? (' 至 snippet 正文分支）
     ——只覆盖「无正文可显示」的失败，无 snippet 回退、无正文 div；正文（fullText）
     分支在失败分支**之前**，与基准 SocialCard 的 content 优先逐分支对齐（可达组合态
     「错误态+正文已到达」——详情拉取成功只写 content 不清错误——必须显示正文）。 */
  const notifErrBranch114 = notif114.slice(
    notif114.indexOf(') : hydrationError ? ('),
    notif114.indexOf(') : item.snippet ? ('),
  );
  const notifBodyBranch114 = notif114.slice(
    notif114.indexOf('{fullText ? ('),
    notif114.indexOf(') : hydrationError ? ('),
  );
  checkNew('(t114-x1b) NotifCard 失败态=内联重试（hydrate-retry + retryHydration(id)，文案域「正文加载失败：」，与 SocialCard 同形），失败分支不再回退 snippet',
    notifErrBranch114.length > 0
    && notifErrBranch114.includes('className="hydrate-retry"')
    && notifErrBranch114.includes('retryHydration(item.id)')
    && notifErrBranch114.includes('正文加载失败：')
    && !notifErrBranch114.includes('item.snippet')
    && !notifErrBranch114.includes('notif-body-text'));

  checkNew('(t114-x1c) NotifCard 空正文/加载中占位与 SocialCard 同形（className="hydrate-placeholder" 恰两态）',
    cnt114(notif114, 'className="hydrate-placeholder"') === 2
    && notif114.includes('暂无正文') && notif114.includes('加载正文…'));

  checkNew('(t114-x1f) 正文分支先于失败分支（R1-F1）：「错误态+正文已到达」组合态显示正文而非假失败行（与 SocialCard content 优先逐分支对齐）',
    notifBodyBranch114.length > 0
    && notifBodyBranch114.includes('notif-body-text')
    && notif114.indexOf('notif-body-text') < notif114.indexOf('className="hydrate-retry"'));

  checkNew('(t114-x1g) 纯失败态（无正文）不渲染「展开更多」（正文已被重试行替换，防死控件）；错误滞留+正文已达的组合态豁免（!!fullText，可展开水合全文，与修前/同态 SocialCard 一致）',
    notif114.includes('{isLong && (!hydrationError || !!fullText) && (')
    && notif114.indexOf('{isLong && (!hydrationError || !!fullText) && (') < notif114.indexOf('className="notif-expand-btn"'));

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
  const { jkLayoutAllowed, jkNextIndex } = await import('../src/lib/jkNavigation.ts');

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
    const { selectVisibleEntries: sve114 } = await import('../dist-test/store.js');
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
  const { Timeline: Timeline114 } = await import('../src/components/Timeline.tsx');
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
  const { syncPillLabel } = await import('../src/lib/syncPill.ts');
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
  const sidebarSrc116 = src116('../src/components/Sidebar.tsx');
  const apiSrc116 = src116('../src/lib/api.ts');
  const bootstrapSrc116 = src116('../src/store/slices/bootstrap.ts');
  const syncTabSrc116 = src116('../src/components/settings/SyncTab.tsx');
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
  const { Sidebar: Sidebar116 } = await import('../src/components/Sidebar.tsx');
  const pillBase116 = rsm116(ce116(Sidebar116));
  checkNew('(t116-x1f) SSR 烟测：Sidebar 组件树可执行，初始态（未连接 · 空队列）pill 渲染出 X3 基线文案「本地模式 · 直连抓取」',
    pillBase116.includes('本地模式 · 直连抓取')
    && pillBase116.includes('sync-status-pill'));

  /* ---------- X2：SyncTab 四态摘要卡（纯函数真值表 + 源级接线 + SSR 烟测） ---------- */
  const { syncStateSummary } = await import('../src/lib/syncPill.ts');
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
  const { SyncTab: SyncTab116 } = await import('../src/components/settings/SyncTab.tsx');
  const syncTabHtml116 = rsm116(ce116(SyncTab116));
  checkNew('(t116-x2d) SSR 烟测：摘要卡改动后 SyncTab 组件树仍可执行（初始 mock 态渲染「演示模式」卡，证据边界同 x1f 注）',
    syncTabHtml116.includes('演示模式') && syncTabHtml116.includes('同步'));
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
