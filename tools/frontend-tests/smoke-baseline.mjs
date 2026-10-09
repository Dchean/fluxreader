// tools/frontend-tests/smoke-baseline.mjs
// 领域模块：冒烟基线（原 26 条既有断言：S-1…S-5 / 缓存命中路径）
// OPT-016C 拆分自 tools/frontend-regression.mjs（旧行区间 101-277）；
// 断言名称/条件文本原样迁移，仅做路径深度适配（import.meta.url 与动态 import 深一层）与
// 共享可变状态的 S. 归属重写。数据所有权：共享假后端/夹具/记录归 harness（见 harness.mjs 头注），
// 本模块不 import 第二份 store；域内自带夹具（大行集等）仍在本模块内独立构造与复位。
export const id = 'smoke-baseline';

export async function run(ctx) {
  const { S, store, check, SANITIZED, bodyOf, getBodyEntry, dropBodyEntry, overrideGlobal } = ctx;

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

/* 【TASK-122 改动理由】真值源迁移：AI 产物落 bodyById 记录（选中文章已水合，
   记录存在），视图行字段不再是真值——断言改经 selectArticleBody 读取；
   保护意图（回读消毒版、无脚本残留）逐字保留。 */
const after = bodyOf(store.getState(), entryId);
check('S-1: 流式结束后 translatedContent 被回读为消毒版（真值源 bodyById，TASK-122）', after.translatedContent === SANITIZED);
check('S-1: 翻译后不再残留 <script>', !after.translatedContent.includes('<script>'));

// ---- C-3：全文提取失败可见（toast + 重试）----
// 把 settings.defaultOpenMode 置为 fulltext，重新水合一篇文章触发自动全文。
// 智能全文判定：正文须含截断标记（"…查看全文"）才触发提取。
store.getState().updateSettings({ defaultOpenMode: 'fulltext' });
S.articleRow = { ...S.articleRow, content_html: '<p>这是摘要正文，比较短…</p><a>…查看全文</a>' };
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
check('缓存命中：未新增 ai_translate 调用', S.invokeCalls.filter((c) => c.cmd === 'ai_translate').length === 1);

// ---- S-2：社交正文批量水合（REQ-001：加载失败/空正文不再永挂）----
const socialEntry = (id) => ({
  id, feedId: '1', title: '社交帖', publishedAt: Date.now(), isRead: false,
  isStarred: false, tags: [], source: 'direct', snippet: '摘要', author: 'a',
  content: '', rawContent: '', translatedContent: '', aiSummary: '',
});
const hydrCalls = () => S.invokeCalls.filter((c) => c.cmd === 'get_articles').length;

// 场景 1：正常行 → 正文填充 + ready 终态（TASK-122：终态/正文落 bodyById 记录）
store.setState({ entries: [socialEntry('11')], dataMode: 'tauri' });
S.getArticlesBehavior = { rows: [{ ...S.articleRow, id: 11, content_html: '<p>社交正文</p>' }], reject: false };
store.getState().hydrateArticleContent(['11']);
await new Promise((r) => setTimeout(r, 20));
const e11 = bodyOf(store.getState(), '11');
check('S-2: 水合成功填充正文并置终态（TASK-122：真值源 bodyById 记录）',
  e11.content === '<p>社交正文</p>' && getBodyEntry('11')?.state === 'ready');
check('S-2: 水合成功清除错误态', e11.state === 'ready' && e11.message === '');

// 场景 2：空正文（content_html 为 NULL）→ 终态「已水合」，再次挂载不再重复拉取
store.setState({ entries: [socialEntry('12')] });
S.getArticlesBehavior = { rows: [{ ...S.articleRow, id: 12, content_html: null }], reject: false };
store.getState().hydrateArticleContent(['12']);
await new Promise((r) => setTimeout(r, 20));
const e12 = bodyOf(store.getState(), '12');
const callsBefore = hydrCalls();
store.getState().ensureArticleContent('12'); // 挂载触发：应被 ready 终态（bodyById 记录）短路
check('S-2: 空正文条目置终态且不重复水合（TASK-122：终态=bodyById ready）',
  e12.content === '' && getBodyEntry('12')?.state === 'ready' && hydrCalls() === callsBefore);

// 场景 3：水合失败 → 错误态可见；重试收敛为成功
store.setState({ entries: [socialEntry('13')] });
S.getArticlesBehavior = { rows: [], reject: true, error: { message: 'IPC 超时' } };
store.getState().hydrateArticleContent(['13']);
await new Promise((r) => setTimeout(r, 20));
check('S-2: 水合失败记录错误态（不再静默假加载；TASK-122：failed 判别态）',
  getBodyEntry('13')?.state === 'failed' && getBodyEntry('13')?.message === 'IPC 超时');
S.getArticlesBehavior = { rows: [{ ...S.articleRow, id: 13, content_html: '<p>重试成功</p>' }], reject: false };
store.getState().retryHydration('13');
await new Promise((r) => setTimeout(r, 20));
const e13 = bodyOf(store.getState(), '13');
check('S-2: 重试后正文填充且错误态清除', e13.content === '<p>重试成功</p>' && getBodyEntry('13')?.state === 'ready' && e13.message === '');

// ---- S-3：启动失败不回退 mock（P0-2）+ WebDAV 冲突确认（P1-10）----
// S-3a：tauri 模式 bootstrap 失败 → 错误态 + 重试入口，绝不渲染 mock 演示数据
S.failBootstrap = true;
store.setState({ dataMode: 'tauri', dataLoading: false, bootstrapError: null, entries: [], categories: [] });
await store.getState().bootstrapFromBackend();
check('S-3a: tauri bootstrap 失败进入错误态', store.getState().bootstrapError?.includes('DB locked') === true);
check('S-3a: 失败时不回退 mock（dataMode 保持 tauri、无假数据）',
  store.getState().dataMode === 'tauri' && store.getState().entries.length === 0);
S.failBootstrap = false;
await store.getState().retryBootstrap();
check('S-3a: 重试后装载成功且错误态清除', store.getState().bootstrapError === null && store.getState().entries.length === 1);

// S-3b：WebDAV 冲突 → 结构化 code 识别 → 确认后 force 重发
let confirmCalls = 0;
// R1：临时覆盖必须 finally 复原——原 window 上无 confirm，复原即 delete（不泄漏影子属性）
const restoreConfirm = overrideGlobal(globalThis.window, 'confirm', () => { confirmCalls += 1; return true; });
try {
  await store.getState().githubLoginStart();
  const ghCalls = S.invokeCalls.filter((c) => c.cmd === 'github_login_start');
  check('S-3b: webdavConflict 弹确认并 force 重发', confirmCalls === 1 && ghCalls.length === 2 && ghCalls[1].args.force === true);
  check('S-3b: force 成功后进入授权流程', store.getState().githubFlow?.user_code === 'WDJB-MJHT');
} finally {
  restoreConfirm();
}

// ---- S-4：卡片级翻译接线（P1-7 空壳修复）----
/* 【TASK-122 改动理由】本用例模拟「通知卡对未水合条目就地翻译」：该文章在
   S-1/S-2 已被水合（bodyById 有记录），先 dropBodyEntry 回到未水合态重建场景；
   无记录时 AI 产物走视图行过渡位（ArticleEntry.translatedContent），读取仍经
   selectArticleBody（无记录回退视图行）。 */
const beforeAiCalls = S.invokeCalls.filter((c) => c.cmd === 'ai_translate').length;
const s4id = store.getState().entries[0].id;
dropBodyEntry(s4id);
store.setState((st) => ({
  entries: st.entries.map((a) => (a.id === s4id ? { ...a, translatedContent: '' } : a)),
}));
store.getState().translateEntry(s4id);
await new Promise((r) => setTimeout(r, 50));
const s4 = bodyOf(store.getState(), s4id);
check('S-4: 卡片级翻译流式生成并回读消毒版（无记录走视图行过渡位）', s4.translatedContent === SANITIZED);
check('S-4: 生成完成后按 id 状态清除', store.getState().translatingIds[s4id] === undefined);
// 缓存命中：已有译文直接返回，不新增 ai_translate（真值源 bodyById 生效值判定）
store.getState().translateEntry(s4id);
check('S-4: 已有译文时不再触发 ai_translate',
  S.invokeCalls.filter((c) => c.cmd === 'ai_translate').length === beforeAiCalls + 1);

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
S.invokeCalls.length = 0;
await store.getState().anchorToArticle('1');
await new Promise((r) => setTimeout(r, 10));
const readCall = S.invokeCalls.find((c) => c.cmd === 'set_read');
check(
  'S-5: 锚定打开按设置标已读',
  !!readCall && readCall.args.read === true && store.getState().entries.find((a) => a.id === '1')?.isRead === true,
);

// F8：全部已读的视图口径（收藏 → starredOnly；今天 → sinceMs）
store.setState({ activeViewFilter: 'starred', activeFeedFilter: 'all' });
S.invokeCalls.length = 0;
store.getState().markCurrentViewAllRead();
await new Promise((r) => setTimeout(r, 10)); // api.markAllRead 内部 await getInvoke()，需让出微任务
const starredCall = S.invokeCalls.find((c) => c.cmd === 'mark_all_read');
check(
  'S-5: 收藏视图全部已读带 starredOnly',
  !!starredCall && starredCall.args.starredOnly === true && (starredCall.args.sinceMs === null || starredCall.args.sinceMs === undefined),
);
store.setState({ activeViewFilter: 'today' });
S.invokeCalls.length = 0;
store.getState().markCurrentViewAllRead();
await new Promise((r) => setTimeout(r, 10));
const todayCall = S.invokeCalls.find((c) => c.cmd === 'mark_all_read');
check('S-5: 今天视图全部已读带 sinceMs', !!todayCall && typeof todayCall.args.sinceMs === 'number' && todayCall.args.sinceMs > 0);
}
