import assert from 'node:assert/strict';
import { componentTest, mount, flush, waitFor, resetStore, useAppStore } from './harness.mjs';
import { installBackend, seedFromRows, mkRow } from './fixtures.mjs';

const TRANSLATED_HTML = '<p><a href="https://example.com/plain">普通链接</a> '
  + '<a href="https://example.com/news/relative"><strong>净化后相对链接</strong></a></p>';

for (const [layout, feedId, selector] of [
  ['social', 11, '.social-translated-block'],
  ['notification', 14, '.notif-translated-block'],
]) {
  componentTest(`译文外链：${layout} 普通/嵌套点击只外开一次，默认导航阻止，失败与拒绝 scheme 可见，流式保持文本`, { timeout: 30000 }, async (test) => {
    installBackend();
    resetStore();
    globalThis.__OPEN_URLS__ = [];
    test.after(() => { delete globalThis.__OPEN_URL_ERROR__; });
    const row = mkRow({
      id: 771,
      feed_id: feedId,
      content_html: '<p>正文</p>',
      translated_content: TRANSLATED_HTML,
    });
    seedFromRows([row], { layout });
    const feedIndex = new Map(useAppStore.getState().feedIndex);
    const binding = feedIndex.get(String(feedId));
    feedIndex.set(String(feedId), {
      ...binding,
      feed: { ...binding.feed, autoTranslate: true, translateMode: 'on' },
    });
    useAppStore.setState({ feedIndex });

    const { createElement, act } = await import('react');
    const { Timeline } = await import('../../src/components/Timeline.tsx');
    const mounted = await mount(createElement(Timeline));
    await waitFor(() => !!mounted.container.querySelector(`${selector}.show a strong`), { label: `${layout} 完成态译文可见` });
    const block = mounted.container.querySelector(selector);
    const click = async (target) => {
      const event = new window.MouseEvent('click', { bubbles: true, cancelable: true, button: 0 });
      await act(async () => { target.dispatchEvent(event); });
      await flush(2);
      assert.equal(event.defaultPrevented, true, '正文/译文链接必须在浏览器默认导航前被接管');
    };
    await click(block.querySelector('a'));
    assert.deepEqual(globalThis.__OPEN_URLS__, ['https://example.com/plain']);
    await click(block.querySelector('strong'));
    assert.deepEqual(globalThis.__OPEN_URLS__, ['https://example.com/plain', 'https://example.com/news/relative']);

    globalThis.__OPEN_URL_ERROR__ = new Error('default browser rejected');
    await click(block.querySelector('strong'));
    assert.equal(globalThis.__OPEN_URLS__.length, 2, 'opener 失败不走第二次外开');
    assert.ok(useAppStore.getState().toasts.some((toast) => toast.text.includes('无法在浏览器中打开此链接')));
    delete globalThis.__OPEN_URL_ERROR__;

    for (const href of ['javascript:alert(1)', 'file:///C:/Windows/win.ini', 'mailto:test@example.com', 'data:text/html,hello']) {
      await act(async () => { useAppStore.setState({ toasts: [] }); });
      const anchor = block.querySelector('a');
      anchor.setAttribute('href', href);
      await click(anchor);
      assert.equal(globalThis.__OPEN_URLS__.length, 2, `${href} 不得交给系统 opener`);
      assert.ok(useAppStore.getState().toasts.some((toast) => toast.text.includes('无法在浏览器中打开此链接')), `${href} 拒绝不能冒充成功`);
    }

    await act(async () => { useAppStore.setState({ rawTranslatedIds: { '771': true } }); });
    assert.equal(block.querySelector('a'), null, '流式未净化译文不创建链接');
    assert.ok(block.textContent.includes(TRANSLATED_HTML), '流式译文按原始字面文本显示');
  });
}

componentTest('N-5：NotifCard 译文含媒体，重复渲染不重建译文 DOM（渲染计数断言）', { timeout: 30000 }, async () => {
  installBackend();
  resetStore();
  const MEDIA_TRANSLATED = '<p>译文段落 <img src="https://example.com/pic.png" alt="媒体探针"> '
    + '<video src="https://example.com/clip.mp4"></video></p>';
  const row = mkRow({
    id: 772,
    feed_id: 14,
    content_html: '<p>正文</p>',
    translated_content: MEDIA_TRANSLATED,
  });
  seedFromRows([row], { layout: 'notification' });
  const feedIndex = new Map(useAppStore.getState().feedIndex);
  const binding = feedIndex.get(String(14));
  feedIndex.set(String(14), {
    ...binding,
    feed: { ...binding.feed, autoTranslate: true, translateMode: 'on' },
  });
  useAppStore.setState({ feedIndex });

  const { createElement, act } = await import('react');
  const { Timeline } = await import('../../src/components/Timeline.tsx');
  const mounted = await mount(createElement(Timeline));
  await waitFor(() => !!mounted.container.querySelector('.notif-translated-block.show video'), { label: 'NotifCard 译文媒体可见' });

  /* N-5 验收：重复渲染时译文里的媒体节点不重建（同 SocialCard translatedHtml
     范式——内联 {__html} 对象每次 render 新建会触发 React 重设 innerHTML）。
     重渲染由 NotifCard 自己订阅的 translatingIds 翻转驱动；每次翻转都以
     「翻译中…」文本出现/消失证明该次 render 真实发生（渲染计数），
     防止「根本没重渲染」导致的空绿。 */
  const mediaImg = mounted.container.querySelector('.notif-translated-block img');
  const mediaVideo = mounted.container.querySelector('.notif-translated-block video');
  assert.ok(mediaImg && mediaVideo, '译文媒体 img/video 必须真实进入 DOM');

  let confirmedRenders = 0;
  const confirmRerender = async (on) => {
    await act(async () => { useAppStore.setState({ translatingIds: { '772': on } }); });
    await flush(2);
    const block = mounted.container.querySelector('.notif-translated-block');
    assert.equal(block.textContent.includes('翻译中…'), on,
      `第 ${confirmedRenders + 1} 次重渲染未发生（翻译中…文本未按预期切换）`);
    confirmedRenders++;
  };
  await confirmRerender(true);
  await confirmRerender(false);
  await confirmRerender(true);
  await confirmRerender(false);
  assert.ok(confirmedRenders >= 4, `至少确认 4 次重渲染，实际 ${confirmedRenders}`);

  const imgAfter = mounted.container.querySelector('.notif-translated-block img');
  const videoAfter = mounted.container.querySelector('.notif-translated-block video');
  assert.strictEqual(imgAfter, mediaImg, '译文 <img> 节点跨重复渲染被重建（translatedHtml 未按 SocialCard 范式 memo 化）');
  assert.strictEqual(videoAfter, mediaVideo, '译文 <video> 节点跨重复渲染被重建（translatedHtml 未按 SocialCard 范式 memo 化）');
});

componentTest('外链委托：嵌套代理容器只处理一次，已阻止事件不再触发 opener', { timeout: 30000 }, async () => {
  resetStore();
  globalThis.__OPEN_URLS__ = [];
  const { createElement, act } = await import('react');
  const { handleArticleLinkClick } = await import('../../dist-test/lib/external.js');
  const mounted = await mount(createElement('div', { onClick: handleArticleLinkClick },
    createElement('div', { onClick: handleArticleLinkClick },
      createElement('a', { href: 'https://example.com/once', target: '_blank' },
        createElement('strong', null, '只开一次')))));
  const nested = mounted.container.querySelector('strong');
  const event = new window.MouseEvent('click', { bubbles: true, cancelable: true });
  await act(async () => { nested.dispatchEvent(event); });
  await flush(2);
  assert.equal(event.defaultPrevented, true);
  assert.deepEqual(globalThis.__OPEN_URLS__, ['https://example.com/once']);
  const prevented = new window.MouseEvent('click', { bubbles: true, cancelable: true });
  prevented.preventDefault();
  await act(async () => { nested.dispatchEvent(prevented); });
  await flush(2);
  assert.equal(globalThis.__OPEN_URLS__.length, 1);
});
