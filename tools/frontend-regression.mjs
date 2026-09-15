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

// ---- 汇总 ----
const failed = results.filter((r) => !r.pass);
console.log(`\n=== 前端逻辑回归 ${results.length - failed.length}/${results.length} 通过 ===`);
if (failed.length) {
  console.error('失败项:', failed.map((f) => f.name).join('; '));
  process.exit(1);
}
process.exit(0);
