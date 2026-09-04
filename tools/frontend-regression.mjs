// 前端逻辑回归（无浏览器）：用 node 驱动 Zustand 状态机验证 S-1 / C-3。
// 运行：先 npx tsc -p tsconfig.test.json，再
//   node --loader ./tools/test-loader.mjs ./tools/frontend-regression.mjs

import assert from 'node:assert';

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

// 3) invoke mock：按命令返回
const invokeCalls = [];
globalThis.__INVOKE__ = (cmd, args) => {
  invokeCalls.push({ cmd, args });
  switch (cmd) {
    case 'list_folders': return Promise.resolve([]);
    case 'list_feeds': return Promise.resolve([]);
    case 'list_articles': return Promise.resolve([articleRow]);
    case 'sync_status': return Promise.resolve({ connected: false });
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
const p = store.getState().toggleReaderTranslation();

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
store.getState().ensureArticleContent(entryId);
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
const beforeCalls = invokeCalls.length;
store.getState().toggleReaderTranslation();
await new Promise((r) => setTimeout(r, 20));
const cached = store.getState().entries.find((a) => a.id === cachedId);
check('缓存命中：已有译文直接展示，不触发 ai_translate',
  cached?.translatedContent === '已缓存译文' && store.getState().isShowingTranslatedProse === true);
check('缓存命中：未新增 ai_translate 调用', invokeCalls.filter((c) => c.cmd === 'ai_translate').length === 1);

// ---- 汇总 ----
const failed = results.filter((r) => !r.pass);
console.log(`\n=== 前端逻辑回归 ${results.length - failed.length}/${results.length} 通过 ===`);
if (failed.length) {
  console.error('失败项:', failed.map((f) => f.name).join('; '));
  process.exit(1);
}
process.exit(0);
