import assert from 'node:assert/strict';
import { componentTest, mount, flush, waitFor, resetStore, getBodyEntry, useAppStore } from './harness.mjs';
import { installBackend, seedFromRows, mkRow, setPlan, setCurrentRows, deferred, invokeCalls } from './fixtures.mjs';

componentTest('NF2 RSS 回切只在独立原文仍在缓存时出现；重开仅有全文时隐藏工具栏/菜单入口', async () => {
  installBackend(); resetStore();
  const row = mkRow({ id: 88101, content_id: 60, content_revision: 1, content_html: '<p>RSS 独立正文</p>' });
  seedFromRows([row]);
  const { createElement, act } = await import('react');
  const { Reader } = await import('../../src/components/Reader.tsx');
  const { ContextMenuHost } = await import('../../src/components/ContextMenu.tsx');
  const sourceStore = await import('../../src/store.ts');
  const compiledStore = await import('../../dist-test/store.js');
  assert.ok(sourceStore.selectArticleBody === compiledStore.selectArticleBody, '组件公开读取入口与harness必须是同一缓存selector实例');
  const entry = await mount(createElement('div', null, createElement(Reader), createElement(ContextMenuHost)));
  setPlan({ extract_fulltext: () => ({ html: '<p>提取后的完整正文</p>', degraded: false, contentId: 61, contentRevision: 1 }) });
  await act(async () => { useAppStore.getState().selectArticle('88101'); });
  await waitFor(() => getBodyEntry('88101')?.state === 'ready', { label: 'RSS hydrated' });
  await act(async () => { useAppStore.getState().extractCurrentArticle(); });
  await waitFor(() => getBodyEntry('88101')?.fulltextExtracted, { label: 'extracted' });
  const button = () => [...entry.container.querySelectorAll('.toggle-action-btn')].find((item) => /RSS 原文|显示全文/.test(item.textContent));
  assert.ok(button()?.textContent.includes('RSS 原文'), '本次提取后确实有 RSS 回切');
  assert.equal(sourceStore.selectArticleBody(useAppStore.getState(), '88101').rawContent, getBodyEntry('88101').rawContent, '公开selector实际读到生产store写入的独立RSS');
  await act(async () => { entry.container.querySelector('[data-ctx="reader"]').dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true })); });
  const rssMenu = [...document.querySelectorAll('[role="menuitem"]')].find((item) => item.textContent.trim() === 'RSS 原文');
  assert.ok(rssMenu, '菜单须与工具栏一致提供实际RSS，不能伪装成提取全文');
  assert.ok(![...document.querySelectorAll('[role="menuitem"]')].some((item) => item.textContent.trim() === '提取全文'));
  await act(async () => { rssMenu.click(); });
  assert.ok(entry.container.querySelector('.article-prose')?.textContent.includes('RSS 独立正文'), '回切真实显示独立 RSS');
  // 模拟正文缓存淘汰后重开：详情接口只持有已提取的当前正文。
  const { dropBodyEntry } = await import('../../dist-test/store/bodyCache.js');
  setCurrentRows([{ ...row, content_id: 61, content_html: '<p>提取后的完整正文</p>', fulltext_extracted: true }]);
  await act(async () => { useAppStore.getState().clearReaderSelection(); dropBodyEntry('88101'); });
  await act(async () => { useAppStore.getState().selectArticle('88101'); });
  await waitFor(() => getBodyEntry('88101')?.state === 'ready', { label: 'extracted detail reopened' });
  await flush(2);
  assert.equal(getBodyEntry('88101').rawContent, '', '不会把全文伪称 RSS 原文');
  assert.ok(entry.container.querySelector('.article-prose')?.textContent.includes('提取后的完整正文'), '唯一正文仍可正常阅读');
  assert.ok(!button(), '没有独立 RSS 时工具栏不提供两态切换');
  await act(async () => { entry.container.querySelector('[data-ctx="reader"]').dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true })); });
  assert.ok(![...document.querySelectorAll('[role="menuitem"]')].some((item) => /RSS 原文|显示全文|提取全文/.test(item.textContent)), '右键菜单相同边界');
});

componentTest('AF2 已提取文章loading时工具栏/菜单不露提取；其他入口新提取后旧详情不得覆盖正文身份', async () => {
  installBackend(); resetStore();
  const row = mkRow({ id: 88102, content_id: 601, content_revision: 1, fulltext_extracted: true, content_html: '<p>保存的旧全文</p>' });
  const detail = deferred();
  seedFromRows([row]);
  setPlan({
    get_article: () => detail.promise,
    extract_fulltext: () => ({ html: '<p>新提取全文</p>', degraded: false, contentId: 602, contentRevision: 1 }),
  });
  const { createElement, act } = await import('react');
  const { Reader } = await import('../../src/components/Reader.tsx');
  const { ContextMenuHost } = await import('../../src/components/ContextMenu.tsx');
  const entry = await mount(createElement('div', null, createElement(Reader), createElement(ContextMenuHost)));
  await act(async () => { useAppStore.getState().selectArticle('88102'); });
  await flush(1);
  const fulltextButtons = () => [...entry.container.querySelectorAll('.toggle-action-btn')].filter((item) => /RSS 原文|显示全文|提取全文/.test(item.textContent));
  assert.equal(getBodyEntry('88102').state, 'loading');
  assert.equal(fulltextButtons().length, 0, 'loading空记录不能临时冒充未提取');
  await act(async () => { entry.container.querySelector('[data-ctx="reader"]').dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true })); });
  assert.ok(![...document.querySelectorAll('[role="menuitem"]')].some((item) => /RSS 原文|显示全文|提取全文/.test(item.textContent)), '菜单相同边界');
  await act(async () => { (document.activeElement ?? document.body).dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true })); });
  assert.equal(invokeCalls.filter(({ cmd }) => cmd === 'extract_fulltext').length, 0, '并未因loading误发提取');
  const stamp = getBodyEntry('88102').contentRevision;
  // 独立于隐藏入口的资格保护：错误重试等直接store提取入口同样经过生产写入。
  await act(async () => { useAppStore.getState().extractCurrentArticle(); });
  await waitFor(() => getBodyEntry('88102')?.contentId === 602, { label: 'new extraction' });
  assert.notEqual(getBodyEntry('88102').contentRevision, stamp, '提取成功推进旧详情资格');
  await act(async () => { detail.resolve(row); });
  await flush(2);
  assert.equal(getBodyEntry('88102').contentId, 602, '旧详情不能回退内容身份');
  assert.ok(entry.container.querySelector('.article-prose')?.textContent.includes('新提取全文'), '实际DOM保持新全文');
  assert.equal(fulltextButtons().length, 0, '没有独立RSS仍不露切换');
});
