// 前端纯逻辑回归（无浏览器）：布局继承 / 视图筛选 / 计数一致性 / 时间工具。
// 直接 import 编译后的 selectors.js + format.js（无 DOM、无状态、无 IPC）。
// 运行：先 npx tsc -p tsconfig.test.json，再
//   node --loader ./tools/test-loader.mjs ./tools/logic-regression.mjs

import assert from 'node:assert';

const { resolveFeedLayout, matchesViewFilter, selectRawEntries, selectScopeEntries,
        selectVisibleEntries, selectViewCounts, numericId, selectTreeCounts, selectFeedConfig } =
  await import('../dist-test/store/selectors.js');
const { isSameLocalDay, formatRelativeTime, formatDuration } = await import('../dist-test/lib/format.js');

const results = [];
function check(name, cond) {
  results.push({ name, pass: !!cond });
  console.log(`${cond ? '✅' : '❌'} ${name}`);
}

/* ============================================================
   布局继承（resolveFeedLayout）：feed 覆盖 → 分类 → inherit 继承
   ============================================================ */

check('布局继承：feed 明确布局覆盖分类', resolveFeedLayout({ layout: 'podcast' }, 'article') === 'podcast');
check('布局继承：feed=inherit 回落分类布局', resolveFeedLayout({ layout: 'inherit' }, 'social') === 'social');
check('布局继承：feed=inherit 回落分类=article', resolveFeedLayout({ layout: 'inherit' }, 'article') === 'article');

/* ============================================================
   视图筛选（matchesViewFilter）：全部/今天/未读/收藏
   ============================================================ */

const now = new Date('2026-09-09T12:00:00').getTime();
const todayEntry = { publishedAt: new Date('2026-09-09T08:00:00').getTime(), isRead: false, isStarred: false };
const yesterdayEntry = { publishedAt: new Date('2026-09-08T08:00:00').getTime(), isRead: false, isStarred: false };
const readEntry = { publishedAt: todayEntry.publishedAt, isRead: true, isStarred: false };
const starredEntry = { publishedAt: todayEntry.publishedAt, isRead: true, isStarred: true };

check('视图筛选：全部 恒真', matchesViewFilter(todayEntry, 'all', now) === true);
check('视图筛选：今天 命中今天', matchesViewFilter(todayEntry, 'today', now) === true);
check('视图筛选：今天 排除昨天', matchesViewFilter(yesterdayEntry, 'today', now) === false);
check('视图筛选：未读 命中未读', matchesViewFilter(todayEntry, 'unread', now) === true);
check('视图筛选：未读 排除已读', matchesViewFilter(readEntry, 'unread', now) === false);
check('视图筛选：收藏 命中收藏', matchesViewFilter(starredEntry, 'starred', now) === true);
check('视图筛选：收藏 排除非收藏', matchesViewFilter(readEntry, 'starred', now) === false);

/* ============================================================
   时间工具（isSameLocalDay / formatRelativeTime / formatDuration）
   ============================================================ */

check('时间：同一本地日历日', isSameLocalDay(
  new Date('2026-09-09T00:30:00').getTime(),
  new Date('2026-09-09T23:30:00').getTime(),
) === true);
check('时间：跨日不相等', isSameLocalDay(
  new Date('2026-09-08T23:59:00').getTime(),
  new Date('2026-09-09T00:01:00').getTime(),
) === false);
check('时间：相对时间 刚刚', formatRelativeTime(now - 30_000, now) === '刚刚');
check('时间：相对时间 N 分钟前', formatRelativeTime(now - 5 * 60_000, now) === '5 分钟前');
check('时间：未来时间戳 clamp 为刚刚（不出现负数）', formatRelativeTime(now + 3600_000, now) === '刚刚');
check('时间：时长 m:ss', formatDuration(65) === '1:05');
check('时间：时长 h:mm:ss', formatDuration(3661) === '1:01:01');
check('时间：时长负值 clamp 0', formatDuration(-5) === '0:00');

/* ============================================================
   numericId：统一 ID 解析（纯数字 / feed- 前缀 / cat- 前缀）
   ============================================================ */

check('numericId：纯数字', numericId('42') === 42);
check('numericId：feed- 前缀', numericId('feed-42') === 42);
check('numericId：cat- 前缀', numericId('cat-7') === 7);
check('numericId：非数字 NaN', Number.isNaN(numericId('abc')));

/* ============================================================
   可见列表与计数（selectVisibleEntries / selectViewCounts / selectTreeCounts）
   用同一份 mock 状态，验证「角标 = 列表条数」口径一致
   ============================================================ */

// 构造：两个分类（article / podcast 布局），article 分类下两个 feed
const feedArticleA = { layout: 'inherit', autoSummary: false, autoTranslate: false };
const feedArticleB = { layout: 'inherit', autoSummary: false, autoTranslate: false };
const feedPodcast = { layout: 'inherit', autoSummary: false, autoTranslate: false };
const catArticle = { id: 'cat-1', layout: 'article', collapsed: false, settingsCollapsed: false, name: '文章', autoSummary: false, autoTranslate: false, feeds: [] };
const catPodcast = { id: 'cat-2', layout: 'podcast', collapsed: false, settingsCollapsed: false, name: '播客', autoSummary: false, autoTranslate: false, feeds: [] };

const feedIndex = new Map([
  ['1', { feed: feedArticleA, cat: catArticle }],
  ['2', { feed: feedArticleB, cat: catArticle }],
  ['3', { feed: feedPodcast, cat: catPodcast }],
]);

const entries = [
  { id: 'a1', feedId: '1', publishedAt: todayEntry.publishedAt, isRead: false, isStarred: false },
  { id: 'a2', feedId: '1', publishedAt: todayEntry.publishedAt, isRead: true, isStarred: false },
  { id: 'b1', feedId: '2', publishedAt: todayEntry.publishedAt, isRead: false, isStarred: true },
  { id: 'p1', feedId: '3', publishedAt: todayEntry.publishedAt, isRead: false, isStarred: false },
];
const openedReadIds = {};

function makeState(overrides = {}) {
  return {
    activeContentLayout: 'article',
    activeViewFilter: 'all',
    activeFeedFilter: 'all',
    timelineFilter: 'all',
    timelineSort: 'newest',
    feedIndex,
    entries,
    openedReadIds,
    feedCounts: new Map([
      ['1', { total: 2, unread: 1, starred: 0, today: 2 }],
      ['2', { total: 1, unread: 1, starred: 1, today: 1 }],
      ['3', { total: 1, unread: 1, starred: 0, today: 1 }],
    ]),
    ...overrides,
  };
}

// selectRawEntries：article 布局只含 feed 1/2 的条目（feed 3 是 podcast，被排除）
const raw = selectRawEntries(makeState());
check('布局过滤：article 布局排除 podcast 源条目', raw.length === 3 && raw.every((e) => e.feedId !== '3'));

// selectScopeEntries：单 feed 范围
const scoped = selectScopeEntries(makeState({ activeFeedFilter: '1' }));
check('范围过滤：单 feed 只含该 feed 条目', scoped.length === 2 && scoped.every((e) => e.feedId === '1'));

// selectVisibleEntries：全部视图 + 时间流全部
const visibleAll = selectVisibleEntries(makeState());
check('可见列表：全部视图 = 3 条（article 布局）', visibleAll.length === 3);

// 未读视图（article 布局含 feed 1/2 的 a1、a2、b1；未读 = a1 + b1 = 2 条）
const visibleUnread = selectVisibleEntries(makeState({ activeViewFilter: 'unread' }));
check('可见列表：未读视图只含未读', visibleUnread.length === 2 && visibleUnread.every((e) => !e.isRead));

// 收藏视图
const visibleStarred = selectVisibleEntries(makeState({ activeViewFilter: 'starred' }));
check('可见列表：收藏视图只含收藏', visibleStarred.length === 1 && visibleStarred[0].id === 'b1');

// 计数一致性：selectViewCounts 的「未读」数字 == selectVisibleEntries(未读视图) 的条数
const counts = selectViewCounts(makeState());
const unreadCount = counts.unread;
check('计数一致：侧栏未读角标 == 未读视图条数', unreadCount === visibleUnread.length);
check('计数一致：全部角标 == 全部视图条数', counts.all === visibleAll.length);

// selectTreeCounts：feed 级角标（默认 view=all + timeline=all → 返回 total）
const tree = selectTreeCounts(makeState());
check('树计数：feed 1 total = 2', tree.get('1') === 2);
check('树计数：feed 2 total = 1', tree.get('2') === 1);
check('树计数：分类聚合 = 各 feed 之和', tree.get('cat-1') === 3);

// timelineFilter=unread 时树角标切换为未读口径
const treeUnread = selectTreeCounts(makeState({ timelineFilter: 'unread' }));
check('树计数：timeline=unread 时 feed 1 未读 = 1', treeUnread.get('1') === 1);

// selectFeedConfig：feed 级 AI 开关
const cfg = selectFeedConfig(makeState(), '1');
check('AI 配置：feed 默认关闭', cfg.autoSummary === false && cfg.autoTranslate === false);

/* ============================================================
   汇总
   ============================================================ */

const failed = results.filter((r) => !r.pass);
console.log(`\n=== 前端纯逻辑回归 ${results.length - failed.length}/${results.length} 通过 ===`);
if (failed.length) {
  console.error('失败项:', failed.map((f) => f.name).join('; '));
  process.exit(1);
}
process.exit(0);
