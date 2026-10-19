// OPT-016B：ReaderProse 源码/渲染/流式译文三条呈现路径的真实挂载回归。
//
// 取证方式：挂载**真实 Reader 组件**（生产消费路径——props 由 Reader 单点计算），
// 经真实 store action 切源码态，用同一挂载树前后对比 DOM。
// 「同一实例」不靠声称：状态经 store action 写入后挂载树立即以 DOM 反映，
// 且 bodyCache 记录与组件呈现对得上（组件与断言读同一份 dist-test 实例）。

import assert from 'node:assert/strict';
import { componentTest, mount, flush, waitFor, resetStore, getBodyEntry, useAppStore } from './harness.mjs';
import { installBackend, seedFromRows, mkRow, clearInvokeCalls } from './fixtures.mjs';
import { sourceModeChecks, renderModeChecks } from './reader-prose-checks.mjs';

/** 危险 + 合法混合的正文（与 F14 审计形态同构）。 */
const TEST_HTML = [
  '<p class="safe-p">安全段落</p>',
  '<script>window.__XSS_SCRIPT__=1</script>',
  '<img src="https://images.example.com/p.png" onerror="window.__XSS_ERR__=1" alt="p">',
  '<a href="https://example.com/link">链接</a>',
].join('');
const STREAM_TEXT = '<b>未消毒译文<b><script>window.__XSS_TRANS__=1</script>';

componentTest('OPT-016B ReaderProse：源码→渲染→源码同一挂载树，危险标签在源码态不创建、事件/样式/滚动宿主不丢', { timeout: 30000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();

  const row = mkRow({
    id: 101, title: '源码契约文章', url: 'https://example.com/a',
    /* 正文真值经真实 get_article 水合落 bodyById；译文列带上未消毒流式形态文本
       （后面 raw translated mode 用例消费）。 */
    content_html: TEST_HTML,
    translated_content: STREAM_TEXT,
  });
  seedFromRows([row]);
  useAppStore.setState({ settings: { ...useAppStore.getState().settings, fontSize: 20, lineHeight: 180, showReadTime: true } });

  const { createElement, act } = await import('react');
  const { Reader } = await import('../../src/components/Reader.tsx');
  const entry = await mount(createElement(Reader));

  /* 打开文章：走真实 selectArticle → get_article → bodyById 落记录。 */
  await act(async () => { useAppStore.getState().selectArticle('101'); });
  await flush(4);

  // ---- 同一实例（组件 ↔ 断言的 store/bodyCache 是同一份）----
  assert.equal(getBodyEntry('101')?.state, 'ready', '真实 get_article 应把正文落进 bodyById 记录');
  assert.equal(getBodyEntry('101')?.content, TEST_HTML, '记录正文与后端行一致');
  await waitFor(() => !!entry.container.querySelector('.article-prose'), { label: 'ReaderProse 挂载' });

  const proseRender = entry.container.querySelector('.article-prose');
  const scrollHost = entry.container.querySelector('.reader-scroll-content');
  assert.ok(proseRender && scrollHost, 'Reader 应渲染正文容器与滚动宿主');

  /* 记录滚动载体的位置与宿主引用：源码/渲染切换后必须还是同一个节点、
     位置不丢（「滚动宿主不丢」）。 */
  scrollHost.scrollTop = 123;

  // ---- 渲染态：合法元素存在（与源码态形成判别对）----
  for (const c of renderModeChecks(entry.container)) assert.ok(c.pass, `${c.name}（${c.detail}）`);
  assert.equal(window.__XSS_SCRIPT__, undefined, 'innerHTML 插入的 script 不执行（XSS 意图不成立）');

  // ---- 事件：渲染态正文点击代理（img→灯箱；a→外部打开）----
  await act(async () => { proseRender.querySelector('img').click(); });
  assert.equal(useAppStore.getState().lightboxUrl, 'https://images.example.com/p.png', 'img 点击应走灯箱（事件代理真实接线）');
  await act(async () => { useAppStore.getState().closeLightbox(); });
  await act(async () => { proseRender.querySelector('a').click(); });
  await flush(2);
  assert.ok((globalThis.__OPEN_URLS__ ?? []).includes('https://example.com/link'), 'a 点击应走 openExternal（Tauri opener 边界被真实调用）');

  // ---- 切到源码态（真实 action）----
  await act(async () => { useAppStore.getState().toggleReaderRenderMode(); });
  for (const c of sourceModeChecks(entry.container, TEST_HTML)) assert.ok(c.pass, `${c.name}（${c.detail}）`);
  assert.equal(window.__XSS_SCRIPT__, undefined, '源码态不得创建可执行 script（F14 契约）');
  assert.equal(window.__XSS_ERR__, undefined, '源码态不得创建带 onerror 的 img');

  // ---- 源码态 → 渲染态：同一节点复用，样式/滚动位置不丢 ----
  await act(async () => { useAppStore.getState().toggleReaderRenderMode(); });
  for (const c of renderModeChecks(entry.container)) assert.ok(c.pass, `${c.name}（${c.detail}）`);
  const proseAfter = entry.container.querySelector('.article-prose');
  assert.equal(proseAfter, proseRender, '源码↔渲染切换必须复用同一 DOM 节点（宿主不重建）');
  assert.equal(entry.container.querySelector('.reader-scroll-content'), scrollHost, '滚动宿主节点不丢');
  assert.equal(scrollHost.scrollTop, 123, '滚动位置在呈现切换后保留');
  assert.equal(proseAfter.style.fontSize, '20px', '排版样式（settings 驱动）不丢');

  // ---- raw translated mode：未消毒流式译文必须纯文本 ----
  await act(async () => {
    useAppStore.setState({ isShowingTranslatedProse: true, rawTranslatedIds: { '101': true } });
  });
  await flush(2);
  const streamingScope = entry.container;
  const proseStream = streamingScope.querySelector('.article-prose');
  const streamChecks = [
    { name: 'raw-translated: 未消毒译文不创建 <b>/<script>（纯文本路径）', pass: proseStream?.querySelector('b') == null && proseStream?.querySelector('script') == null },
    { name: 'raw-translated: 译文以字面文本可见（含未消毒标签原文）', pass: proseStream?.textContent.includes('<b>未消毒译文') === true },
    { name: 'raw-translated: 未执行注入脚本', pass: window.__XSS_TRANS__ === undefined },
    { name: 'raw-translated: 非 raw-render-mode 容器（走流式分支而非源码分支）', pass: proseStream?.classList.contains('raw-render-mode') === false },
  ];
  for (const c of streamChecks) assert.ok(c.pass, c.name);

  // 同一实例复核：写回 store 的译者态由同一挂载树实时反映
  assert.equal(useAppStore.getState().isShowingTranslatedProse, true);
  /* 未预期 console/jsdom/React 错误由 componentTest 的统一账本终检判红（R1），
     本用例不注册任何例外。 */
});
