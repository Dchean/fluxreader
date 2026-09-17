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
  let heldAi = [];            // hold 模式挂起项 { cmd, id, ch }
  let settingsRaw = null;     // get_setting('app_settings') 的返回值

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
    heldAi = [];
    aiSum = { deltas: [], error: null, reject: null, finish: true, holdIds: [] };
    aiTr = { deltas: [], error: null, reject: null, finish: true, holdIds: [] };
    detailImpl = (id) => mkRow({ id, content_html: '<p>详情</p>', translated_content: null });
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
  checkNew('(a) 首批分页游标 = PAGE_SIZE(500)、8 行 < 500 视为已到底、加载态收起',
    aOk.entries.length === 8 && aOk.articlesLimit === 500
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
  checkNew('(g) 分页失败被静默吞掉：无 toast、无错误态（用户侧零提示——记录为观察项）',
    gFail.toasts.length === 0);
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
  checkNew('(k) …但非法值仍原样写进 settings.startupView（只拦「应用」不校验「值」——观察项）',
    store.getState().settings.startupView === 'bogus');

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
})();

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
