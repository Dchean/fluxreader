// OPT-016B：真实挂载测试的后端夹具（外部 IPC 边界的可控假后端）。
//
// 纪律：只 mock invoke 的**数据形状与时机**（延迟/缺行/失败），不 mock 应用逻辑。
// 所有命令都走真实路径 src/lib/api.ts → test-loader 的 @tauri-apps/api/core mock
// → globalThis.__INVOKE__；调用被记录，供「真的发了这次 IPC」类断言取证。

import { useAppStore } from './harness.mjs';

const { articleRowToEntry, folderRowsToCategories } = await import('../../dist-test/lib/api.js');
const { buildFeedIndex } = await import('../../dist-test/store/internals.js');

/* ------------------------------------------------------------------
   规范夹具：与 tools/frontend-regression.mjs 同形的后端行（snake_case）。
   五个订阅源分别绑定五种生效布局（feed 级覆盖 → 分类兜底），使五布局切换
   各自有真实数据源——列表快照按布局维度下发（与生产后端同一口径）。
   ------------------------------------------------------------------ */
export const FOLDERS = [
  { id: 1, name: '技术', layout: 'article', auto_summary: false, auto_translate: false, collapsed: false },
];
export const FEEDS = [
  { id: 10, folder_id: 1, feed_url: 'https://a.example/rss', site_url: null, title: '源A', favicon_url: null, layout: 'inherit', auto_summary: false, auto_translate: false, fetch_failed: false, fetch_error: null, last_fetched_at: null },
  { id: 11, folder_id: 1, feed_url: 'https://b.example/rss', site_url: null, title: '源B', favicon_url: null, layout: 'social', auto_summary: false, auto_translate: false, fetch_failed: false, fetch_error: null, last_fetched_at: null },
  { id: 12, folder_id: 1, feed_url: 'https://c.example/rss', site_url: null, title: '源C', favicon_url: null, layout: 'image', auto_summary: false, auto_translate: false, fetch_failed: false, fetch_error: null, last_fetched_at: null },
  { id: 13, folder_id: 1, feed_url: 'https://d.example/rss', site_url: null, title: '源D', favicon_url: null, layout: 'podcast', auto_summary: false, auto_translate: false, fetch_failed: false, fetch_error: null, last_fetched_at: null },
  { id: 14, folder_id: 1, feed_url: 'https://e.example/rss', site_url: null, title: '源E', favicon_url: null, layout: 'notification', auto_summary: false, auto_translate: false, fetch_failed: false, fetch_error: null, last_fetched_at: null },
];
export const COUNTS = [
  { feed_id: 10, total: 2, unread: 2, starred: 0, today: 0 },
  { feed_id: 11, total: 2, unread: 2, starred: 0, today: 0 },
  { feed_id: 12, total: 1, unread: 1, starred: 0, today: 0 },
  { feed_id: 13, total: 1, unread: 1, starred: 0, today: 0 },
  { feed_id: 14, total: 1, unread: 1, starred: 0, today: 0 },
];

/** feed → 生效布局（feed 显式布局优先，'inherit' 落分类 'article'）。 */
export function layoutOfFeed(feedId) {
  const feed = FEEDS.find((f) => f.id === Number(feedId));
  if (!feed || feed.layout === 'inherit') return 'article';
  return feed.layout;
}

export function mkRow(overrides = {}) {
  return {
    id: 0, feed_id: 10, title: '标题', author: '作者', snippet: '摘要片段',
    image_url: null, enclosure_url: null, enclosure_mime: null, duration_sec: null,
    ai_summary: null, source: 'direct', published_at: '2026-10-01T10:00:00Z',
    is_read: false, is_starred: false, url: 'https://example.com/a',
    content_html: null, translated_content: null, fulltext_extracted: false,
    ...overrides,
  };
}

/** 覆盖 5 布局所需的 8 行夹具（每布局 1–2 行）。 */
export function defaultRows() {
  return [
    mkRow({ id: 101, title: 'A1 文章卡一', snippet: 'A1 摘要' }),
    mkRow({ id: 102, title: 'A2 文章卡二', snippet: 'A2 摘要' }),
    mkRow({ id: 201, feed_id: 11, title: 'B1 社交卡', snippet: 'B1 摘要', image_url: 'https://img.example/1.png' }),
    mkRow({ id: 202, feed_id: 11, title: 'B2 社交卡', snippet: 'B2 摘要' }),
    mkRow({ id: 301, feed_id: 12, title: 'C1 画廊卡', snippet: 'C1 摘要', image_url: 'https://img.example/3.png' }),
    mkRow({ id: 401, feed_id: 13, title: 'D1 播客卡', snippet: 'D1 摘要', enclosure_url: 'https://audio.example/4.mp3', enclosure_mime: 'audio/mpeg', duration_sec: 1234 }),
    mkRow({ id: 501, feed_id: 14, title: 'E1 通知卡', snippet: 'E1 摘要' }),
    mkRow({ id: 103, title: 'A3 文章卡三', snippet: 'A3 摘要' }),
  ];
}

/** 某布局的行集合（后端 list_articles 的布局过滤口径）。 */
export function rowsForLayout(rows, layout) {
  return rows.filter((r) => layoutOfFeed(r.feed_id) === layout);
}

/* ------------------------------------------------------------------
   假后端：默认覆盖启动装载所需的全部命令；用例可经 plan 覆写命令行为。
   plan 的键 = 命令名；值 = 函数 (args, ctx) => 结果（可返回 Promise/deferred）。
   ------------------------------------------------------------------ */
export const invokeCalls = [];
let plan = {};
let installed = null;

export function clearInvokeCalls() { invokeCalls.length = 0; }

/** 覆写命令行为；未覆写的命令回落默认实现。 */
export function setPlan(next) { plan = { ...plan, ...next }; }

export function resetPlan() { plan = {}; }

/** 默认行为：完美行——但 hydrate 场景由用例显式覆写 get_articles（延迟/缺行/失败）。 */
function defaultHandler(cmd, args) {
  switch (cmd) {
    case 'list_folders': return Promise.resolve(FOLDERS);
    case 'list_feeds': return Promise.resolve(FEEDS);
    case 'feed_counts': return Promise.resolve(COUNTS);
    case 'list_articles': {
      /* 后端布局过滤口径：list_articles 带 layout 维度（QueryScope.args），
         只回该布局的源的行——切布局后列表快照必须换数据（不是 css 类变化）。 */
      const layout = args?.args?.layout;
      return Promise.resolve(layout ? rowsForLayout(currentRows, layout) : currentRows);
    }
    case 'get_articles': {
      const ids = new Set((args?.ids ?? []).map(String));
      return Promise.resolve(currentRows.filter((r) => ids.has(String(r.id))));
    }
    case 'get_article': {
      const row = currentRows.find((r) => String(r.id) === String(args?.id));
      return Promise.resolve(row ?? null);
    }
    case 'set_read': case 'set_starred': case 'set_setting': case 'resolve_close': return Promise.resolve(null);
    case 'set_read_bulk': return Promise.resolve(null);
    case 'get_setting': return Promise.resolve(null);
    case 'sync_status': return Promise.resolve({ connected: false, endpoint: null, account: null, last_sync: 0 });
    case 'sync_queue_stats': return Promise.resolve({ waiting: 0, failed: 0, last_error: null });
    case 'github_login_status': return Promise.resolve(null);
    default: return Promise.resolve(null);
  }
}

/* currentRows：当前后端行快照（用例可替换——例如 hydration 的「第二次重试才给行」）。 */
export let currentRows = defaultRows();
export function setCurrentRows(rows) { currentRows = rows; }

/** 安装假后端（幂等：重复调用先复位调用记录）。 */
export function installBackend() {
  clearInvokeCalls();
  if (installed) return installed;
  installed = (cmd, args) => {
    invokeCalls.push({ cmd, args });
    const custom = plan[cmd];
    if (custom) return custom(args, { calls: invokeCalls });
    return defaultHandler(cmd, args);
  };
  globalThis.__INVOKE__ = installed;
  return installed;
}

export function invokeCount(cmd) { return invokeCalls.filter((c) => c.cmd === cmd).length; }

/* ------------------------------------------------------------------
   store 播种：把后端行装进真实 store（同 dist-test 实例），走真实适配函数。
   ------------------------------------------------------------------ */
export function seedFromRows(rows, { layout = 'article', dataMode = 'tauri' } = {}) {
  setCurrentRows(rows);
  const categories = folderRowsToCategories(FOLDERS, FEEDS);
  useAppStore.setState({
    categories,
    feedIndex: buildFeedIndex(categories),
    feedCounts: new Map(COUNTS.map((c) => [String(c.feed_id), { total: c.total, unread: c.unread, starred: c.starred, today: c.today }])),
    /* 快照只装当前布局的条目（与后端下发口径一致；切布局由真实 selectLayout
       重拉替换）。 */
    entries: rowsForLayout(rows, layout).map(articleRowToEntry),
    activeContentLayout: layout,
    activeFeedFilter: 'all',
    activeViewFilter: 'all',
    dataMode,
    dataLoading: false,
  });
}

/** 手动 deferred（延迟/失败注入用；resolve/reject 由用例在 act 内驱动）。 */
export function deferred() {
  let resolve; let reject;
  const promise = new Promise((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}
