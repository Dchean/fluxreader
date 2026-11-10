// 挂载真实 GalleryCard/Lightbox；只替换详情 IPC 的数据、时序和图片网络事件。
// jsdom 不能证明解码/布局；像素与真实键盘输入另由 Edge 验证。
import assert from 'node:assert/strict';
import { componentTest as harnessComponentTest, mount, unmountAll, flush, waitFor, resetStore, useAppStore } from './harness.mjs';
import { installBackend, seedFromRows, mkRow, resetPlan, setPlan, deferred, invokeCount, invokeCalls } from './fixtures.mjs';

const state = () => useAppStore.getState();

// 本域临时改变打开标读设置；先卸载本例组件，再恢复进入值，避免恢复动作触发仍挂载的 effect。
// harness 继续承担写前沿等待、错误账本和最终清理；本地清理失败不得覆盖原断言失败。
function componentTest(name, body) {
  return harnessComponentTest(name, async (t) => {
    const settings = state().settings;
    const errors = [];
    try {
      await body(t);
    } catch (error) {
      errors.push(error);
    } finally {
      try { await unmountAll(); } catch (error) { errors.push(error); }
      try { useAppStore.setState({ settings }); } catch (error) { errors.push(error); }
      resetPlan();
    }
    if (errors.length === 1) throw errors[0];
    if (errors.length > 1) throw new AggregateError(errors, '画廊用例与局部清理失败');
  });
}

const urls = ['https://img.example/first.png', 'https://img.example/second.png', 'https://img.example/third.png'];
const row = (id, images = urls, extra = {}) => mkRow({
  id, feed_id: 12, title: `图片条目 ${id}`, image_url: images[0] ?? null,
  gallery_first_image_url: images[0] ?? null, image_count: images.length,
  images: [...images], ...extra,
});

async function setup(rows, { timeline = true, markReadOnOpen = false } = {}) {
  installBackend(); resetPlan(); resetStore();
  seedFromRows(rows, { layout: 'image' });
  useAppStore.setState((s) => ({ settings: { ...s.settings, markReadOnOpen } }));
  const { createElement, act } = await import('react');
  const { Timeline } = await import('../../src/components/Timeline.tsx');
  const { Lightbox } = await import('../../src/components/Overlays.tsx');
  const mounted = await mount(createElement('div', null, timeline && createElement(Timeline), createElement(Lightbox)));
  await flush(2);
  const query = (selector) => mounted.container.querySelector(selector);
  const click = async (selector) => {
    const target = query(selector);
    assert.ok(target, `动作入口存在：${selector}`);
    await act(async () => { target.click(); });
    await flush(2);
  };
  const key = async (key, extra = {}) => {
    await act(async () => {
      query('.lightbox-overlay').dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true, ...extra }));
    });
    await flush(2);
  };
  const open = async (id) => {
    await act(async () => { state().selectArticle(String(id)); state().openGalleryLightbox(String(id)); });
    await flush(2);
  };
  return { ...mounted, act, query, click, key, open };
}

componentTest('画廊多图：挂载不预取，首图/计数打开有序集合，按钮与方向键首尾循环、Esc还焦', async () => {
  const view = await setup([row(7101), row(7102, [urls[0]])]);
  assert.equal(invokeCount('get_article'), 0, '仅挂载画廊不拉取每篇完整集合');
  const card = view.query('[data-id="7101"]');
  assert.equal(card.querySelector('img').getAttribute('src'), urls[0], '卡片只展示首图');
  const count = card.querySelector('.gallery-image-count');
  assert.equal(count.textContent, '3');
  assert.equal(count.getAttribute('aria-label'), '查看全部 3 张图片');
  const trigger = card.querySelector('[data-card-index]');
  trigger.focus();
  const scroller = view.query('#timelineContentScroll');
  const beforeScroll = scroller.scrollTop;
  await view.click('[data-id="7101"] .gallery-media-trigger');
  assert.equal(invokeCount('get_article'), 1);
  assert.deepEqual(state().lightboxGallery.urls, urls);
  assert.equal(view.query('.lightbox-position').textContent, '1 / 3');
  assert.equal(view.query('.lightbox-img').getAttribute('src'), urls[0]);
  const close = view.query('.lightbox-close');
  const next = view.query('.lightbox-next');
  next.focus();
  await view.key('Tab');
  assert.equal(document.activeElement, close, '末控件Tab回到首控件');
  await view.key('Tab', { shiftKey: true });
  assert.equal(document.activeElement, next, '首控件Shift+Tab回到末控件');
  await view.click('.lightbox-previous');
  assert.equal(state().lightboxUrl, urls[2], '首张往前回到最后');
  await view.click('.lightbox-next');
  assert.equal(state().lightboxUrl, urls[0], '最后往后回到首张');
  await view.key('ArrowRight');
  assert.equal(state().lightboxUrl, urls[1]);
  await view.key('ArrowLeft', { ctrlKey: true });
  assert.equal(state().lightboxUrl, urls[1], '组合键不能降级为单键切图');
  await view.key('Escape', { isComposing: true });
  assert.equal(state().lightboxUrl, urls[1], 'IME取消选词不关闭');
  await view.key('ArrowLeft');
  assert.equal(state().lightboxUrl, urls[0]);
  await view.key('Escape');
  assert.equal(state().lightboxUrl, null);
  assert.equal(document.activeElement, trigger, '关闭归还同一稳定卡片节点');
  assert.equal(scroller.scrollTop, beforeScroll, '打开、切换与关闭都不改列表位置');
  await view.click('[data-id="7101"] .gallery-image-count');
  await view.click('.lightbox-close');
  assert.equal(document.activeElement, count, '计数入口也能原样还焦');
});

componentTest('画廊多图：A晚到不能覆盖B，关闭/同条目重开/单图打开均失效旧响应', async () => {
  const a = row(7201); const b = row(7202, ['https://img.example/b.png']);
  const view = await setup([a, b], { timeline: false });
  const oldA = deferred(); const newB = deferred();
  setPlan({ get_article: ({ id }) => id === a.id ? oldA.promise : newB.promise });
  await view.open(a.id);
  const firstRequest = state().lightboxGallery.requestId;
  await view.open(b.id);
  await view.act(async () => { newB.resolve(b); });
  assert.equal(state().lightboxUrl, b.images[0]);
  await view.act(async () => { oldA.resolve(a); });
  assert.equal(state().lightboxGallery.entryId, String(b.id));
  assert.equal(state().lightboxUrl, b.images[0]);
  const closedRequest = deferred();
  setPlan({ get_article: () => closedRequest.promise });
  await view.open(a.id);
  await view.act(async () => { state().closeLightbox(); });
  const reopened = deferred();
  setPlan({ get_article: () => reopened.promise });
  await view.open(a.id);
  assert.ok(state().lightboxGallery.requestId > firstRequest, '生产代次始终递增');
  await view.act(async () => { closedRequest.resolve({ ...a, images: ['https://img.example/obsolete.png'] }); });
  assert.equal(state().lightboxGallery.status, 'loading', '关闭前同id回包不能接管重开会话');
  await view.act(async () => { reopened.resolve(a); });
  assert.deepEqual(state().lightboxGallery.urls, urls);
  const superseded = deferred();
  setPlan({ get_article: () => superseded.promise });
  await view.open(a.id);
  await view.act(async () => { state().openLightbox('https://img.example/reader.png'); superseded.resolve(a); });
  assert.equal(state().lightboxGallery, null);
  assert.equal(state().lightboxUrl, 'https://img.example/reader.png');
  assert.equal(view.query('.lightbox-navigation'), null, '正文单图继续沿原路径');
  await view.click('.lightbox-img');
  assert.equal(state().lightboxUrl, null, '单图点击关闭仍保留');
});

componentTest('画廊多图：列表版本变更挡旧详情，重排保留URL、移除后夹取下标', async () => {
  const original = row(7301);
  const view = await setup([original], { timeline: false });
  const old = deferred(); const updated = deferred();
  setPlan({ get_article: () => old.promise });
  await view.open(original.id);
  setPlan({ get_article: () => updated.promise });
  await view.act(async () => {
    useAppStore.setState((s) => ({ entries: s.entries.map((entry) => ({ ...entry, mediaRevision: 2, imageUrl: 'https://img.example/new-first.png' })) }));
  });
  assert.equal(invokeCount('get_article'), 2, '当前元数据变更触发新详情请求');
  assert.equal(view.query('.lightbox-img'), null, '新版未返回时不再显示旧版图片');
  await view.act(async () => { old.resolve(original); });
  assert.equal(state().lightboxGallery.status, 'loading');
  await view.act(async () => { updated.resolve({ ...original, media_revision: 2 }); });
  await view.key('ArrowRight');
  assert.equal(state().lightboxUrl, urls[1]);
  setPlan({ get_article: () => ({ ...original, media_revision: 3, images: [urls[2], urls[0], urls[1]] }) });
  await view.act(async () => { useAppStore.setState((s) => ({ entries: s.entries.map((entry) => ({ ...entry, mediaRevision: 3 })) })); });
  assert.equal(state().lightboxUrl, urls[1], '集合重排保留所看URL');
  assert.equal(state().lightboxGallery.index, 2);
  setPlan({ get_article: () => ({ ...original, media_revision: 4, images: [urls[2], urls[0]] }) });
  await view.act(async () => { useAppStore.setState((s) => ({ entries: s.entries.map((entry) => ({ ...entry, mediaRevision: 4, imageCount: 2 })) })); });
  assert.equal(state().lightboxGallery.index, 1, '删除原图后将旧下标2夹到最后1');
  assert.equal(state().lightboxUrl, urls[0]);
  assert.equal(invokeCount('get_article'), 4);
});

componentTest('画廊多图：详情身份/低版本拒绝，较新详情接受但不循环拉取', async () => {
  const original = row(7401, urls, { media_revision: 5 });
  const view = await setup([original], { timeline: false });
  for (const invalid of [{ id: 7402 }, { feed_id: 11 }, { media_revision: 4 }]) {
    setPlan({ get_article: () => ({ ...original, ...invalid }) });
    await view.open(original.id);
    assert.equal(state().lightboxGallery.status, 'error');
    assert.equal(view.query('.lightbox-img'), null, '错误身份与过期图片不可见');
  }
  setPlan({ get_article: () => ({ ...original, media_revision: 6 }) });
  await view.click('.lightbox-status button');
  assert.equal(state().lightboxGallery.status, 'ready');
  assert.equal(state().lightboxGallery.mediaRevision, 6);
  await flush(4);
  assert.equal(invokeCount('get_article'), 4, '列表尚未追上详情版本不导致无限重取');
});

componentTest('画廊多图：空集合与失败可重试，空详情不借首图伪装成有图', async () => {
  const original = row(7501);
  const view = await setup([original]);
  setPlan({ get_article: () => ({ ...original, images: [] }) });
  await view.click('.gallery-media-trigger');
  assert.equal(state().lightboxGallery.status, 'empty');
  assert.equal(view.query('.lightbox-img'), null);
  assert.match(view.query('.lightbox-status').textContent, /暂无图片/);
  setPlan({ get_article: () => Promise.reject(new Error('simulated IPC rejection')) });
  await view.click('.lightbox-status button');
  assert.equal(state().lightboxGallery.status, 'error');
  setPlan({ get_article: () => original });
  await view.click('.lightbox-status button');
  assert.equal(state().lightboxGallery.status, 'ready');
  assert.equal(view.query('.lightbox-img').getAttribute('src'), urls[0]);
});

componentTest('画廊多图：无首图先取集合；失败/空态卡片重试，取到真实图片才打开与标读', async () => {
  const original = row(7601, [], { image_count: 2 });
  const view = await setup([original], { markReadOnOpen: true });
  const pending = deferred();
  setPlan({ get_article: () => pending.promise });
  await view.click('.gallery-media-trigger');
  assert.equal(state().lightboxUrl, null, '不得发明空串/假URL作为打开信号');
  assert.equal(state().entries[0].isRead, false);
  assert.match(view.query('.gallery-no-image').textContent, /加载图片/);
  await view.act(async () => { pending.reject(new Error('expected detail failure')); });
  assert.match(view.query('.gallery-no-image').textContent, /点击重试/);
  setPlan({ get_article: () => original });
  await view.click('.gallery-media-trigger');
  assert.equal(state().lightboxGallery.status, 'empty');
  assert.equal(state().lightboxUrl, null);
  setPlan({ get_article: () => ({ ...original, images: urls }) });
  await view.click('.gallery-media-trigger');
  assert.equal(state().lightboxUrl, urls[0]);
  assert.equal(state().entries[0].isRead, true);
  assert.equal(invokeCount('set_read'), 1, '成功打开只标读一次');
  assert.equal(state().activeArticleId, String(original.id));
});

componentTest('画廊多图：无首图等待时改选别条、删除条目或复位后，迟到响应不弹窗', async () => {
  const original = row(7701, []); const other = row(7702);
  const view = await setup([original, other], { timeline: false });
  const { articleRowToEntry } = await import('../../dist-test/lib/api.js');
  for (const transition of ['select', 'remove', 'reset']) {
    await view.act(async () => { useAppStore.setState({ entries: [original, other].map(articleRowToEntry) }); });
    const pending = deferred();
    setPlan({ get_article: () => pending.promise });
    await view.open(original.id);
    await view.act(async () => {
      if (transition === 'select') state().selectArticle(String(other.id));
      else if (transition === 'remove') useAppStore.setState((s) => ({ entries: s.entries.filter((entry) => entry.id !== String(original.id)) }));
      else resetStore();
    });
    await view.act(async () => { pending.resolve({ ...original, images: urls }); });
    assert.equal(state().lightboxUrl, null, `${transition}后旧响应不能打开`);
    assert.equal(state().lightboxGallery, null);
  }
});

componentTest('画廊多图：图片解码失败可独立重试，失败图不阻断切到下一张', async () => {
  const view = await setup([row(7801)]);
  await view.click('.gallery-media-trigger');
  const firstImage = view.query('.lightbox-img');
  await view.act(async () => { firstImage.dispatchEvent(new Event('error')); });
  assert.equal(view.query('.lightbox-img'), null);
  assert.match(view.query('.lightbox-status').textContent, /图片加载失败/);
  await view.click('.lightbox-status button');
  const retried = view.query('.lightbox-img');
  assert.notEqual(retried, firstImage, '重试重新创建真实图片请求节点');
  await view.act(async () => { retried.dispatchEvent(new Event('load')); });
  assert.equal(view.query('.lightbox-gallery-image').getAttribute('aria-busy'), 'false');
  await view.key('ArrowRight');
  assert.equal(view.query('.lightbox-img').getAttribute('src'), urls[1]);
  assert.equal(invokeCalls.some((call) => call.cmd === 'report_broken_cover'), false, '正文图失败不误报封面');
});

componentTest('画廊多图：代理失败重试、换源晚回包不显示另一图片', async () => {
  const proxyA = 'https://img.doubanio.com/a.jpg';
  const proxyB = 'https://img.doubanio.com/b.jpg';
  const original = row(7901, [proxyA, proxyB]);
  const view = await setup([original], { timeline: false });
  const pending = deferred();
  setPlan({ fetch_image: () => pending.promise });
  await view.open(original.id);
  assert.equal(view.query('.lightbox-img'), null, '代理未完成不泄露旧dataURL');
  setPlan({ fetch_image: () => [0x89, 0x50, 0x4e, 0x47, 5] });
  await view.key('ArrowRight');
  const second = view.query('.lightbox-img').getAttribute('src');
  await view.act(async () => { pending.resolve([0x89, 0x50, 0x4e, 0x47, 9]); });
  assert.equal(view.query('.lightbox-img').getAttribute('src'), second, 'A晚代理结果不可落在B节点');
  setPlan({ fetch_image: () => [0x3c, 0x68, 0x74, 0x6d, 0x6c] });
  await view.key('ArrowLeft');
  assert.match(view.query('.lightbox-status').textContent, /图片加载失败/);
  setPlan({ fetch_image: () => [0x89, 0x50, 0x4e, 0x47, 1] });
  await view.click('.lightbox-status button');
  assert.match(view.query('.lightbox-img').getAttribute('src'), /^data:image\/png;base64,/);
});

componentTest('画廊多图：卡片代理结果绑定媒体版本，替换源后首帧与晚回包不串图', async () => {
  const original = row(7951, ['https://img.doubanio.com/old.jpg']);
  const view = await setup([original]);
  const oldProxy = deferred(); const freshProxy = deferred();
  setPlan({ fetch_image: ({ url }) => url.includes('new') ? freshProxy.promise : oldProxy.promise });
  await view.act(async () => { useAppStore.setState((s) => ({ entries: s.entries.map((entry) => ({ ...entry, mediaRevision: 2 })) })); });
  await view.act(async () => {
    useAppStore.setState((s) => ({ entries: s.entries.map((entry) => ({ ...entry, imageUrl: 'https://img.doubanio.com/new.jpg', mediaRevision: 3 })) }));
  });
  assert.equal(view.query('.gallery-card img').getAttribute('src'), 'https://img.doubanio.com/new.jpg');
  await view.act(async () => { oldProxy.resolve([0x89, 0x50, 0x4e, 0x47, 2]); });
  assert.equal(view.query('.gallery-card img').getAttribute('src'), 'https://img.doubanio.com/new.jpg');
  await view.act(async () => { freshProxy.resolve([0x89, 0x50, 0x4e, 0x47, 3]); });
  await waitFor(() => view.query('.gallery-card img').getAttribute('src').startsWith('data:'), { label: '新版代理结果落地' });
  assert.equal(invokeCount('get_article'), 0);
});

componentTest('画廊多图：标读设置关闭时点击/切图不写已读；单图集合禁用切换仍可关闭', async () => {
  const view = await setup([row(7961, [urls[0]])]);
  await view.click('.gallery-media-trigger');
  assert.equal(view.query('.lightbox-previous').getAttribute('aria-disabled'), 'true');
  assert.equal(view.query('.lightbox-next').getAttribute('aria-disabled'), 'true');
  await view.key('ArrowRight');
  assert.equal(state().lightboxUrl, urls[0]);
  assert.equal(state().entries[0].isRead, false);
  assert.equal(invokeCount('set_read'), 0);
  await view.click('.lightbox-overlay');
  assert.equal(state().lightboxUrl, null);
});

componentTest('画廊多图：卡片图片失败/无图共用稳定Enter与Space入口，不污染同URL封面缓存', async () => {
  const unique = 'https://img.example/local-gallery-failure.png';
  const original = row(7971, [unique]);
  const empty = row(7972, []);
  const view = await setup([original, empty]);
  const { getCoverState } = await import('../../src/lib/coverImage.ts');
  const trigger = view.query('[data-id="7971"] .gallery-media-trigger');
  await view.act(async () => {
    trigger.focus();
    trigger.querySelector('img').dispatchEvent(new Event('error'));
  });
  assert.equal(view.query('[data-id="7971"] .gallery-media-trigger'), trigger);
  assert.equal(trigger.querySelector('[data-cover-state="failed"]') !== null, true);
  assert.equal(getCoverState(unique).status, 'idle', '首图失败不能将同URL的封面永久拉黑');
  await view.act(async () => { trigger.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })); });
  assert.equal(state().lightboxUrl, unique, '失败占位仍能键盘打开可重试图片');
  await view.key('Escape');
  assert.equal(document.activeElement, trigger);
  const emptyTrigger = view.query('[data-id="7972"] .gallery-media-trigger');
  setPlan({ get_article: () => ({ ...empty, images: [urls[2]] }) });
  await view.act(async () => { emptyTrigger.dispatchEvent(new KeyboardEvent('keydown', { key: ' ', bubbles: true, cancelable: true })); });
  assert.equal(state().lightboxUrl, urls[2], '无图占位也能Space读取并打开真实集合');
  assert.equal(invokeCount('report_broken_cover'), 0);
  await view.key('Escape');
  assert.equal(document.activeElement, emptyTrigger);
});

componentTest('画廊多图F1：Next聚焦后版本加载aria禁用动作，双向Tab留在灯箱且完成后可继续切图/还焦', async () => {
  const original = row(7981);
  const view = await setup([original]);
  const trigger = view.query('.gallery-media-trigger');
  const scroller = view.query('#timelineContentScroll');
  const beforeScroll = scroller.scrollTop;
  await view.click('.gallery-media-trigger');
  const overlay = view.query('.lightbox-overlay');
  const next = view.query('.lightbox-next');
  await view.act(async () => { next.focus(); });
  const pending = deferred();
  setPlan({ get_article: () => pending.promise });
  await view.act(async () => {
    useAppStore.setState((s) => ({ entries: s.entries.map((entry) => ({ ...entry, mediaRevision: 2 })) }));
  });
  assert.equal(state().lightboxGallery.status, 'loading');
  assert.equal(next.getAttribute('aria-disabled'), 'true', '加载中向读屏标示动作不可用');
  assert.equal(next.disabled, false, '保留原生焦点节点，不让Edge禁用blur到BODY');
  assert.equal(document.activeElement, next, '进入加载态时保持原Next焦点');
  const loadingIndex = state().lightboxGallery.index;
  await view.act(async () => { next.click(); });
  await view.key('ArrowRight');
  assert.equal(state().lightboxGallery.index, loadingIndex);
  for (const shiftKey of [false, true]) {
    const event = new KeyboardEvent('keydown', { key: 'Tab', shiftKey, bubbles: true, cancelable: true });
    await view.act(async () => { document.activeElement.dispatchEvent(event); });
    assert.equal(event.defaultPrevented, true, `加载期${shiftKey ? 'Shift+Tab' : 'Tab'}在首尾控件循环`);
    assert.equal(overlay.contains(document.activeElement), true);
  }
  await view.act(async () => { pending.resolve({ ...original, media_revision: 2 }); });
  assert.equal(next.getAttribute('aria-disabled'), 'false');
  await view.act(async () => { view.query('.lightbox-close').focus(); });
  await view.key('Tab', { shiftKey: true });
  assert.equal(document.activeElement, next, '详情恢复后Shift+Tab从Close回到Next');
  await view.click('.lightbox-next');
  assert.equal(state().lightboxUrl, urls[1]);
  await view.key('Escape');
  assert.equal(document.activeElement, trigger);
  assert.equal(scroller.scrollTop, beforeScroll);
});

componentTest('画廊多图E2：公开单图action替换当前集合后移除控件，焦点留在容器且点击关闭仍还原入口', async () => {
  const view = await setup([row(7982)]);
  const trigger = view.query('.gallery-media-trigger');
  await view.click('.gallery-media-trigger');
  await view.act(async () => { view.query('.lightbox-next').focus(); state().openLightbox(urls[0]); });
  const overlay = view.query('.lightbox-overlay');
  assert.equal(state().lightboxGallery, null);
  assert.equal(document.activeElement, overlay, '公开action替换分支仍保dialog焦点（不声称普通UI可达）');
  for (const shiftKey of [false, true]) {
    const event = new KeyboardEvent('keydown', { key: 'Tab', shiftKey, bubbles: true, cancelable: true });
    await view.act(async () => { document.activeElement.dispatchEvent(event); });
    assert.equal(event.defaultPrevented, true);
    assert.equal(document.activeElement, overlay);
  }
  await view.click('.lightbox-img');
  assert.equal(state().lightboxUrl, null);
  assert.equal(document.activeElement, trigger);
});

componentTest('画廊多图F1边界：焦点已交给外部控件时媒体更新不得抢回灯箱', async () => {
  const view = await setup([row(7983)]);
  const outside = document.createElement('button');
  outside.textContent = '外部弹层或portal入口';
  view.container.append(outside);
  await view.click('.gallery-media-trigger');
  await view.act(async () => { view.query('.lightbox-next').focus(); outside.focus(); });
  const pending = deferred();
  setPlan({ get_article: () => pending.promise });
  await view.act(async () => {
    useAppStore.setState((s) => ({ entries: s.entries.map((entry) => ({ ...entry, mediaRevision: 2 })) }));
  });
  assert.equal(document.activeElement, outside, '局部交接只修本灯箱失去的焦点');
  const event = new KeyboardEvent('keydown', { key: 'Tab', bubbles: true, cancelable: true });
  await view.act(async () => { outside.dispatchEvent(event); });
  assert.equal(event.defaultPrevented, false, '不建立全局Tab拦截');
});
