// tools/frontend-tests/harness.mjs
// 共享 harness（OPT-016C 从 tools/frontend-regression.mjs 拆分：共享假后端 / 可变状态 / 断言记录 / 复位）。
//
// 所有权与读写接口（域模块只经 ctx 访问，不 import 第二份 store）：
//   - ctx.S  ：全部跨用例可变状态（invokeCalls、backendRows、listPlan/pendingList…、
//              rejectCmds、getArticlesPlan、feedCountsImpl、heldAi、settingsRaw 等）——
//              由本文件唯一拥有；域模块经 `const { S } = ctx` 显式读写。
//   - ctx.store：dist-test/store.js 的 useAppStore 单例（与 ui-loader 的 store 别名同一实例）。
//   - ctx.useMainBackend()：切换到主假后端（各主域入口调用；阶段一冒烟域用初始 smoke mock）。
//   - ctx.resetStore()/bootFixture()：夹具复位/规范化装载（跨域隔离的起点）。
//   - ctx.check/checkNew：断言记录（check=既有 / checkNew=新增，打印格式与原文件逐字一致）。
//   - ctx.overrideGlobal(target, key, value)：临时全局覆盖的统一入口——记录原属性
//     descriptor，返回 restore()；原本不存在 → restore 即 delete（不留影子属性）。
//     域内所有临时替换必须 `try { ... } finally { restore(); }`（成功/异常都保全）。
//   - ctx 的其余常量/助手/选择器：mkRow、iso、NOW、BASE_ROWS、queryRows、countsFromRows、
//     selectVisibleEntries…、bodyOf/bodyOfT122/getBodyEntry…/entryNeedsHydrationT122 等。
// 临时替换（__INVOKE__/window/console/process 监听等）的复位责任归各域自身，统一走
// ctx.overrideGlobal + try/finally（R1 起全量收口；成功/异常都保全原值/descriptor）。
import { register } from 'node:module';

/* 注册顺序与原文件一致：Node loader 链后注册者先执行；ui-loader 必须最后注册
   （先处理 src/ 下的解析，避免 test-loader 给无扩展名相对路径盲加 .js）。 */
register(new URL('../test-loader.mjs', import.meta.url).href, new URL('../../', import.meta.url).href);
register(new URL('../ui-loader.mjs', import.meta.url).href, new URL('../../', import.meta.url).href);

// ============================================================
// 阶段一环境与夹具（逐字节迁移自原文件；仅路径深度适配与 S. 归属重写）
// ============================================================

// ---- 伪造 Tauri 窗口 + 正文夹具 ----
// 1) 伪造 Tauri 窗口环境，使 isTauri()=true → dataMode 可进 'tauri'
globalThis.window = { __TAURI_INTERNALS__: {} };

// 2) 后端状态：一篇正文含 <script> 的文章 + 消毒后的译文缓存
const SANITIZED = '<p>安全译文，无脚本</p>';

// ---- 规范夹具（主假后端）----
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

// ---- 共享可变状态（域模块经 ctx.S 读写；初始化值逐字迁移自原文件）----
const S = {
  articleRow: {
  id: 1, feed_id: 1, title: '测试文章', author: 'a',
  summary: 'snippet', content_html: '<p>正文</p>', image_url: null,
  enclosure_url: null, enclosure_mime: null, duration_sec: null,
  ai_summary: null, translated_content: null, source: 'direct',
  published_at: '2026-09-04T10:00:00Z', is_read: false, is_starred: false,
  fulltext_extracted: false, url: 'https://example.com/a',
  },
// 批量水合（get_articles）的可控行为：S-2 用例按场景改写
  getArticlesBehavior: { rows: [], reject: false },
// S-3 可控行为：bootstrap 后端故障注入 / github 登录首调冲突
  failBootstrap: false,
// 3) invoke mock：按命令返回
  invokeCalls: [],
  /* ---------- 假后端的可变行为（每个用例显式设置，避免隐式耦合） ---------- */
  backendRows: BASE_ROWS,
  failReload: null,      // 非 null → folders/feeds/feed_counts 拒绝（bootstrap 失败注入）
  listPlan: null,        // { mode: 'reject' | 'defer', error } —— list_articles 行为
  pendingList: [],       // defer 模式挂起项 { args, resolve }
  indexPlan: null,       // { mode: 'defer' } —— article_index 行为
  pendingIndex: [],      // defer 模式挂起项 { articleId, value, resolve }
  detailImpl: (id) => mkRow({ id, content_html: '<p>详情</p>', translated_content: null }),
  aiSum: { deltas: [], error: null, reject: null, finish: true, holdIds: [] },
  aiTr: { deltas: [], error: null, reject: null, finish: true, holdIds: [] },
  rejectCmds: new Set(),   // (p) TASK-067 N10：按命令名注入 IPC 失败
  rejectWhen: null,        // (p3-f2) TASK-093：按 (cmd, args) 谓词注入失败——连点场景只拒第一次的置位写
  getArticlesPlan: null,   // TASK-103：批量水合行为——{ rows } | { mode:'defer' } | { mode:'reject', error }
  pendingGetArticles: [],  // defer 模式挂起项 { ids, resolve }
  /* TASK-107：feed_counts 行为。null = 静态 COUNTS（既有断言基线——夹具的计数
     与行级数据刻意不一致，大量断言依赖）；函数 = 按 backendRows 忠实聚合
     （计数对账用例：markCurrentViewAllRead 成功后重取的计数必须等于后端口径）。 */
  feedCountsImpl: null,
  /* TASK-119：feed_counts 挂起计划（{ mode: 'defer' }，一次性）+ 挂起项——
     计数竞态场景（对账请求在途窗口）需要手动放行响应 */
  feedCountsPlan: null,
  pendingCounts: [],
  heldAi: [],            // hold 模式挂起项 { cmd, id, ch }
  settingsRaw: null,     // get_setting('app_settings') 的返回值
  ghLoginStatus: null,   // github_login_status 的返回值：null | {login} | 'reject'
  extractResult: null,   // extract_fulltext 的返回值：ExtractFulltextResult | 'reject' | null
                              // TASK-076：后端改为结构化 { html, degraded, reason }

};

// ---- 假后端行为助手 ----
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
  let out = S.backendRows.slice();
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
  for (const r of S.backendRows) {
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

// ---- 两套 __INVOKE__：阶段一冒烟 / 主假后端 ----
function installSmokeBackend() {
globalThis.__INVOKE__ = (cmd, args) => {
S.invokeCalls.push({ cmd, args });
if (S.failBootstrap && ['list_folders', 'list_feeds', 'list_articles', 'sync_status'].includes(cmd)) {
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
  case 'list_articles': return Promise.resolve([S.articleRow]);
  case 'sync_status': return Promise.resolve({ connected: false });
  case 'get_articles': {
    // S-2 可控行为：成功返回 rows / 失败 reject（默认空）
    if (S.getArticlesBehavior.reject) return Promise.reject(S.getArticlesBehavior.error ?? { message: 'db busy' });
    return Promise.resolve(S.getArticlesBehavior.rows);
  }
  case 'get_article': return Promise.resolve(S.articleRow);

  case 'get_setting': return Promise.resolve(null);
  case 'set_setting': return Promise.resolve(null);
  // ai_translate / ai_summarize：args.onChannel 是 Channel mock
  case 'ai_translate': {
    const ch = args.onChannel;
    // 模拟未消毒流式 delta（含 <script>）
    ch.onmessage?.({ type: 'delta', data: '<p>译文<script>alert(1)</script></p>' });
    // 后端落库后返回消毒版（后续 get_article 会返回 SANITIZED）
    S.articleRow = { ...S.articleRow, translated_content: SANITIZED };
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
}

function installMainBackend() {
globalThis.__INVOKE__ = (cmd, args) => {
  S.invokeCalls.push({ cmd, args });
  if (S.rejectCmds.has(cmd) || (S.rejectWhen && S.rejectWhen(cmd, args))) return Promise.reject({ message: '注入失败:' + cmd });
  switch (cmd) {
    case 'list_folders': return S.failReload ? Promise.reject(S.failReload) : Promise.resolve(FOLDERS);
    case 'list_feeds': return S.failReload ? Promise.reject(S.failReload) : Promise.resolve(FEEDS);
    /* TASK-107：feedCountsImpl 非空时按 backendRows 忠实聚合（计数对账用例）；
       缺省仍返回静态 COUNTS——既有断言基线依赖它（与行级夹具刻意不一致） */
    case 'feed_counts': {
      if (S.failReload) return Promise.reject(S.failReload);
      /* TASK-119：一次性 defer——仅挂起当次请求，重取/后续调用照常返回 */
      if (S.feedCountsPlan && S.feedCountsPlan.mode === 'defer') {
        S.feedCountsPlan = null;
        return new Promise((resolve) => { S.pendingCounts.push({ resolve }); });
      }
      return Promise.resolve(S.feedCountsImpl ? S.feedCountsImpl() : COUNTS);
    }
    case 'sync_status': return Promise.resolve({ connected: false });
    case 'list_articles': {
      if (S.listPlan && S.listPlan.mode === 'reject') return Promise.reject(S.listPlan.error);
      const a = (args && args.args) || {};
      if (S.listPlan && S.listPlan.mode === 'defer') {
        return new Promise((resolve) => { S.pendingList.push({ args: a, resolve }); });
      }
      return Promise.resolve(queryRows(a));
    }
    case 'article_index': {
      const a = (args && args.args) || {};
      const full = queryRows({ ...a, limit: null, offset: 0 });
      const pos = full.findIndex((r) => r.id === args.articleId);
      const value = pos < 0 ? null : pos;
      if (S.indexPlan && S.indexPlan.mode === 'defer') {
        return new Promise((resolve) => { S.pendingIndex.push({ articleId: args.articleId, value, resolve }); });
      }
      return Promise.resolve(value);
    }
    case 'get_article': return Promise.resolve(S.detailImpl(Number(args.id)));
    case 'get_articles': {
      /* TASK-103 可控行为：成功返回 rows / defer 挂起（驱动在途窗口）/ 失败 reject */
      if (S.getArticlesPlan && S.getArticlesPlan.mode === 'reject') return Promise.reject(S.getArticlesPlan.error ?? { message: 'get_articles 注入失败' });
      if (S.getArticlesPlan && S.getArticlesPlan.mode === 'defer') {
        return new Promise((resolve) => { S.pendingGetArticles.push({ ids: args.ids, resolve }); });
      }
      return Promise.resolve(S.getArticlesPlan ? S.getArticlesPlan.rows : []);
    }
    case 'get_setting': return Promise.resolve(S.settingsRaw);
    case 'set_setting': return Promise.resolve(null);
    /* TASK-107：单条标读忠实落库（record_read_state 语义：仅该行自身，
       无本地同文副本传播）——单条计数一致性核查的根基 */
    case 'set_read': {
      const row = S.backendRows.find((r) => r.id === Number(args.id));
      /* TASK-107 R2/F3：mutation 用布尔——Rust Serialize 的 wire 格式是 bool，
         数字 1/0 会让 reload 后的 entry.isRead 变成 number，破坏 `=== true` 形态
         的守卫/断言（快照替换场景因此失真） */
      if (row) row.is_read = args.read === true;
      return Promise.resolve(null);
    }
    /* TASK-117：单条收藏忠实落库（record_star_state 语义：仅该行自身）——
       取消收藏场景的续拉回归（t117-2）需要后端集合真实收缩。 */
    case 'set_starred': {
      const row = S.backendRows.find((r) => r.id === Number(args.id));
      if (row) row.is_starred = args.starred === true;
      return Promise.resolve(null);
    }
    /* TASK-117：批量标读忠实落库（apply_read_bulk 语义：逐 id 翻转自身行）——
       阅读中连续翻页回归（t117-1）需要 is_read=0 的筛选集合真实收缩。 */
    case 'set_read_bulk': {
      for (const id of args.ids ?? []) {
        const row = S.backendRows.find((r) => r.id === Number(id));
        if (row) row.is_read = args.read === true;
      }
      return Promise.resolve(null);
    }
    /* TASK-107：mark_all_read 忠实落库（apply_mark_all_read 语义：整个
       范围×布局×视图口径的未读行置已读，返回受影响行数）——600/1 探针
       与「成功后计数以后端为准」对账的根基 */
    case 'mark_all_read': {
      let affected = 0;
      for (const r of S.backendRows) {
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
      return S.ghLoginStatus === 'reject'
        ? Promise.reject({ message: 'ipc down' })
        : Promise.resolve(S.ghLoginStatus);
    /* TASK-076：全文提取返回结构化结果（'reject' = 网络失败）。
       为兼容既有用例仍传裸字符串的写法，这里把字符串折算成「成功」形态：
       { html, degraded:false, reason:null }。 */
    case 'extract_fulltext':
      if (S.extractResult === 'reject') {
        return Promise.reject({ message: '网页拉取失败：HTTP 503' });
      }
      return Promise.resolve(
        typeof S.extractResult === 'string'
          ? { html: S.extractResult, degraded: false, reason: null }
          : S.extractResult,
      );
    case 'ai_summarize':
    case 'ai_translate': {
      const plan = cmd === 'ai_summarize' ? S.aiSum : S.aiTr;
      const id = Number(args.articleId);
      const ch = args.onChannel;
      if (plan.reject) return Promise.reject(plan.reject);
      if (plan.holdIds.includes(id)) { S.heldAi.push({ cmd, id, ch }); return new Promise(() => {}); }
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
}

installSmokeBackend(); // 原文件的初始假后端（冒烟域使用；主域入口调用 useMainBackend 切换）

// ---- E2 探针必须在 store.js 被求值（bindAppStore）之前执行 ----
// 4) import 编译后的 store（loader 会 mock @tauri-apps/api）
//    4a) E2 探针：必须在 store.js 被求值（其模块末尾 bindAppStore）之前调用
//        internals.appStore()，才能观察到「句柄尚未注入」这条路径的真实行为。
//        只是新增几行探测代码，不触碰上面任何一条既有断言。
const internalsBeforeBind = await import('../../dist-test/store/internals.js');
let appStoreUnbound = { threw: false, error: null, value: undefined };
try {
appStoreUnbound.value = internalsBeforeBind.appStore();
} catch (e) {
appStoreUnbound = { threw: true, error: e, value: undefined };
}
const { useAppStore, selectArticleBody: bodyOf } = await import('../../dist-test/store.js');
/* TASK-122：正文/AI 真值源 bodyById（模块级缓存，src/store/bodyCache.ts）——
 断言经 bodyOf（selectors.selectArticleBody）与 getBodyEntry 读取 */
const { getBodyEntry, dropBodyEntry } = await import('../../dist-test/store/bodyCache.js');

const store = useAppStore;

// ---- 断言记录与等待 ----
const results = [];      // 既有（check）
const newResults = [];   // 新增（checkNew）
let currentDomain = '';
function record(kind, name, cond) {
  const item = { domain: currentDomain, pass: !!cond, name };
  (kind === 'existing' ? results : newResults).push(item);
  console.log(((cond ? '✅' : '❌') + (kind === 'new' ? ' 🆕 ' : ' ') + name));
}
function check(name, cond) {
  record('existing', name, cond);
}
function checkNew(name, cond) {
  record('new', name, cond);
}
const nTick = (ms = 20) => new Promise((r) => setTimeout(r, ms));

/** OPT-016C R1：临时全局覆盖的异常安全恢复器。
 *  覆盖前记录原属性 descriptor；restore() 时：原本存在 → defineProperty 回原 descriptor
 *  （含 writable/enumerable/configurable 旗标）；原本不存在 → delete（不留影子属性）。
 *  用法（成功/异常都必须走 finally）：
 *    const restore = overrideGlobal(globalThis, '__INVOKE__', fn);
 *    try { ... } finally { restore(); }
 *  不支持访问器属性（本测试网只覆盖数据属性；遇到即抛错，不静默降级）。 */
function overrideGlobal(target, key, value) {
  const had = Object.prototype.hasOwnProperty.call(target, key);
  const prev = had ? Object.getOwnPropertyDescriptor(target, key) : null;
  if (prev && !('value' in prev)) {
    throw new Error(`overrideGlobal 不支持访问器属性：${String(key)}`);
  }
  Object.defineProperty(target, key, { value, writable: true, enumerable: true, configurable: true });
  let restored = false;
  return function restore() {
    if (restored) return;
    restored = true;
    if (had) Object.defineProperty(target, key, prev);
    else delete target[key];
  };
}

// ---- 复位 / 规范化装载（原文件逐字迁移，S. 归属重写）----
/* ---------- 状态复位 / 规范化装载 ---------- */
async function resetStore(extra) {
  /* TASK-107：行对象深拷贝——假后端的 set_read / mark_all_read 会就地翻转
     is_read（忠实模拟落库），浅引用会让突变跨用例残留（污染 BASE_ROWS） */
  S.backendRows = BASE_ROWS.map((r) => ({ ...r }));
  S.failReload = null;
  S.listPlan = null;
  S.pendingList = [];
  S.indexPlan = null;
  S.pendingIndex = [];
  S.settingsRaw = null;
  S.ghLoginStatus = null;
  S.extractResult = null;
  S.heldAi = [];
  S.aiSum = { deltas: [], error: null, reject: null, finish: true, holdIds: [] };
  S.aiTr = { deltas: [], error: null, reject: null, finish: true, holdIds: [] };
  S.rejectCmds = new Set();
  S.rejectWhen = null;
  S.getArticlesPlan = null;
  S.pendingGetArticles = [];
  S.feedCountsImpl = null;
  S.feedCountsPlan = null;
  S.pendingCounts = [];
  S.detailImpl = (id) => mkRow({ id, content_html: '<p>详情</p>', translated_content: null });
  /* TASK-063：视图缓存是模块级 Map，跨用例残留会让下一个用例的 selectFeed
     命中上一个夹具的快照（跨夹具污染）。每个用例独立起步（(s6) 此前已就地
     手工 clear，这里收口为公共 hygiene；动态导入与 (s) 块同一模块实例）。 */
  const { viewEntriesCache } = await import('../../dist-test/store/internals.js');
  viewEntriesCache.clear();
  /* TASK-122：bodyById 是模块级实体缓存，跨夹具残留会让后续用例命中上一夹具
     的正文/终态（与 viewEntriesCache 同型污染）——夹具复位一并清空 */
  const { resetBodyCacheForTests } = await import('../../dist-test/store/bodyCache.js');
  resetBodyCacheForTests();
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
  S.invokeCalls.length = 0;
}
async function bootFixture(extra) {
  await resetStore(extra);
  await store.getState().bootstrapFromBackend();
}

// ---- 主 store 派生面（同一 dist-test 模块图）----
const nSel = await import('../../dist-test/store.js');
const { selectVisibleEntries, selectScopeEntries, selectRawEntries, selectTreeCounts, selectViewCounts } = nSel;
const bodyOfT122 = nSel.selectArticleBody;
const entryNeedsHydrationT122 = nSel.entryNeedsHydration;
const { getBodyEntry: getBodyEntryT122, markBodyLoading: markBodyLoadingT122, dropBodyEntry: dropBodyEntryT122 } = await import('../../dist-test/store/bodyCache.js');
const { viewEntriesCache } = await import('../../dist-test/store/internals.js');

export function createHarness() {
  return {
    S,
    store,
    check,
    checkNew,
    nTick,
    overrideGlobal,
    resetStore,
    bootFixture,
    useMainBackend: installMainBackend,
    useSmokeBackend: installSmokeBackend,
    beginDomain: (id) => {
      currentDomain = id;
    },
    results,
    newResults,
    SANITIZED,
    FOLDERS,
    FEEDS,
    COUNTS,
    NOW,
    OLD,
    TODAY,
    iso,
    mkRow,
    BASE_ROWS,
    localDayKey,
    queryRows,
    countsFromRows,
    effectiveFeedLayout,
    emitAi,
    nSel,
    selectVisibleEntries,
    selectScopeEntries,
    selectRawEntries,
    selectTreeCounts,
    selectViewCounts,
    bodyOf,
    bodyOfT122,
    getBodyEntry,
    getBodyEntryT122,
    dropBodyEntry,
    markBodyLoadingT122,
    dropBodyEntryT122,
    entryNeedsHydrationT122,
    viewEntriesCache,
    appStoreUnbound,
    internalsBeforeBind,
  };
}
