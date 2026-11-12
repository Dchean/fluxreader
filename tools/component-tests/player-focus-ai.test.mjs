// 2026-11-07：F2–F7 的组件生命周期与真实消费者回归。媒体 API/IPC 受控，
// 不把 jsdom 的事件与 Promise 时序称为音频解码或原生宿主验证。
import assert from 'node:assert/strict';
import { componentTest, mount, flush, waitFor, resetStore, useAppStore, dom, getBodyEntry } from './harness.mjs';
import { installBackend, resetPlan, setPlan, seedFromRows, mkRow, clearInvokeCalls, invokeCount, deferred } from './fixtures.mjs';

async function playerFixture() {
  installBackend(); resetPlan(); resetStore();
  setPlan({ get_setting: () => Promise.resolve(null) });
  const media = dom.window.HTMLMediaElement.prototype;
  const original = { play: media.play, pause: media.pause };
  const plays = [];
  media.play = function () {
    const pending = deferred();
    plays.push({ el: this, url: this.src, ...pending });
    return pending.promise;
  };
  media.pause = function () {};
  const { createElement } = await import('react');
  const { PlayerBar } = await import('../../src/components/PlayerBar.tsx');
  const entry = await mount(createElement(PlayerBar));
  return { ...entry, plays, restore: () => { Object.assign(media, original); resetPlan(); } };
}

componentTest('F2：暂停后的 AbortError 与换集后的旧失败都不能改变当前目标态', { timeout: 20000 }, async () => {
  const fixture = await playerFixture();
  const { act } = await import('react');
  try {
    await act(async () => useAppStore.getState().playPodcastEpisode('A', 'Show', '', 'https://audio.invalid/a.mp3'));
    assert.equal(fixture.plays.length, 1);
    await act(async () => useAppStore.getState().applyMediaAction('pause'));
    await act(async () => fixture.plays[0].reject(new dom.window.DOMException('pause interrupted play', 'AbortError')));
    assert.equal(useAppStore.getState().player.isPlaying, false);
    assert.equal(fixture.plays.length, 1, '迟到拒绝不得触发新 play');
    assert.equal(useAppStore.getState().errors.length, 0, '主动暂停没有网络失败');
    await act(async () => useAppStore.getState().applyMediaAction('play'));
    await act(async () => useAppStore.getState().playPodcastEpisode('B', 'Show', '', 'https://audio.invalid/b.mp3'));
    await act(async () => fixture.plays[2].resolve());
    await act(async () => fixture.plays[1].reject(new Error('old A failure')));
    assert.equal(useAppStore.getState().player.audioUrl, 'https://audio.invalid/b.mp3');
    assert.equal(useAppStore.getState().player.isPlaying, true);
    assert.equal(useAppStore.getState().errors.length, 0);
  } finally { fixture.restore(); }
});

componentTest('F3：关闭重开同 URL 重装速率/元数据/进度/ended，旧元素事件失效', { timeout: 20000 }, async () => {
  const fixture = await playerFixture();
  const { act } = await import('react');
  const meta = (el, position) => {
    Object.defineProperty(el, 'duration', { value: 180, configurable: true });
    Object.defineProperty(el, 'readyState', { value: 1, configurable: true });
    el.currentTime = position;
    el.dispatchEvent(new Event('loadedmetadata'));
    el.dispatchEvent(new Event('timeupdate'));
  };
  try {
    await act(async () => useAppStore.getState().playPodcastEpisode('A', 'Show', '', 'https://audio.invalid/a.mp3'));
    const first = fixture.container.querySelector('audio');
    await act(async () => meta(first, 20));
    assert.equal(useAppStore.getState().player.positionSec, 20, '首次正常对照');
    await act(async () => useAppStore.getState().cyclePlaybackSpeed());
    await act(async () => useAppStore.getState().closePodcastBar());
    assert.equal(fixture.container.querySelector('audio'), null);
    await act(async () => useAppStore.getState().playPodcastEpisode('A', 'Show', '', 'https://audio.invalid/a.mp3'));
    const second = fixture.container.querySelector('audio');
    assert.notEqual(first, second);
    assert.equal(second.playbackRate, 1.25);
    await act(async () => meta(second, 47));
    assert.equal(useAppStore.getState().player.positionSec, 47);
    assert.equal(useAppStore.getState().player.durationSec, 180);
    await act(async () => { first.dispatchEvent(new Event('ended')); first.dispatchEvent(new Event('error')); });
    assert.equal(useAppStore.getState().player.isPlaying, true, '旧元素不能结束或暂停新元素');
    await act(async () => second.dispatchEvent(new Event('ended')));
    assert.equal(useAppStore.getState().player.isPlaying, false);
  } finally { fixture.restore(); }
});

componentTest('F4：有效失败显式暂停且可重试；旧错误在换集/回到同 URL 后安全失效', { timeout: 20000 }, async () => {
  const fixture = await playerFixture();
  const { act } = await import('react');
  try {
    await act(async () => useAppStore.getState().playPodcastEpisode('A', 'Show', '', 'https://audio.invalid/a.mp3'));
    await act(async () => fixture.plays[0].reject(new Error('current failure')));
    assert.equal(useAppStore.getState().player.isPlaying, false);
    const failure = useAppStore.getState().errors[0];
    assert.ok(failure, '有效失败可在错误中心重试');
    await act(async () => useAppStore.getState().retryError(failure.id));
    assert.equal(fixture.plays.length, 2, '同一元素重试真的再次 play');
    assert.equal(useAppStore.getState().player.isPlaying, true);
    await act(async () => fixture.plays[1].reject(new Error('second failure')));
    const old = useAppStore.getState().errors[0];
    assert.ok(old, '重试自身再次失败必须保留新的错误入口');
    await act(async () => useAppStore.getState().playPodcastEpisode('B', 'Show', '', 'https://audio.invalid/b.mp3'));
    const before = fixture.plays.length;
    await act(async () => old.action.run());
    assert.equal(useAppStore.getState().player.isPlaying, true);
    assert.equal(fixture.plays.length, before);
    await act(async () => useAppStore.getState().playPodcastEpisode('A again', 'Show', '', 'https://audio.invalid/a.mp3'));
    await act(async () => useAppStore.getState().applyMediaAction('pause'));
    const beforeAgain = fixture.plays.length;
    await act(async () => old.action.run());
    assert.equal(useAppStore.getState().player.isPlaying, false, 'URL 相同也不是旧元素的播放会话');
    assert.equal(fixture.plays.length, beforeAgain);
  } finally { fixture.restore(); }
});

componentTest('F5：搜索和改名先保存触发按钮再移焦，关闭回到仍存活的按钮', { timeout: 20000 }, async () => {
  installBackend(); resetPlan(); resetStore({ categories: [{ id: 'c1', name: '分类', feeds: [] }] });
  const { createElement, Fragment, act } = await import('react');
  const { SearchModal, RenameCategoryModal } = await import('../../src/components/Overlays.tsx');
  const entry = await mount(createElement(Fragment, null,
    createElement('button', { id: 'search-trigger', onClick: () => useAppStore.getState().openSearch() }, 'Search'),
    createElement('button', { id: 'rename-trigger', onClick: () => useAppStore.getState().openRenameCatModal('c1') }, 'Rename'),
    createElement(SearchModal), createElement(RenameCategoryModal),
  ));
  for (const [triggerId, selector, close] of [
    ['search-trigger', '.search-modal-input', () => useAppStore.getState().closeSearch()],
    ['rename-trigger', '.mini-dialog input', () => useAppStore.getState().closeMiniModal('renameCat')],
  ]) {
    const trigger = entry.container.querySelector(`#${triggerId}`);
    trigger.focus();
    await act(async () => trigger.click());
    assert.equal(document.activeElement, entry.container.querySelector(selector));
    await act(async () => close());
    assert.equal(document.activeElement, trigger);
  }
});

componentTest('F5：设置内 Ctrl+K/Escape 返回设置触发点；触发点卸载后回到设置面板', { timeout: 25000 }, async () => {
  installBackend(); resetPlan(); resetStore(); seedFromRows([mkRow({ id: 101 })]);
  const { createElement, act } = await import('react');
  const { default: App } = await import('../../src/App.tsx');
  const entry = await mount(createElement(App));
  await flush(4);
  // 本用例需要一个随后可因切页卸载的真实input；明确建立通用页前置，
  // 不继承前面About用例的页签，也不依赖已退役OAuth套件偶然切到sync。
  await act(async () => useAppStore.getState().openSettingsTab('general'));
  const settingsPanel = entry.container.querySelector('[role="dialog"][aria-label="设置"]');
  const trigger = settingsPanel.querySelector('button');
  trigger.focus();
  const key = async (name, ctrlKey = false) => act(async () => {
    document.activeElement.dispatchEvent(new KeyboardEvent('keydown', { key: name, ctrlKey, bubbles: true, cancelable: true }));
  });
  await key('k', true);
  assert.equal(document.activeElement, entry.container.querySelector('.search-modal-input'));
  await key('Escape');
  assert.equal(useAppStore.getState().settingsOpen, true);
  assert.equal(document.activeElement, trigger);
  const disappearing = settingsPanel.querySelector('input');
  assert.ok(disappearing, `F5卸载目标前置：tab=${useAppStore.getState().settingsTab}; title=${settingsPanel.querySelector('.settings-pane-title')?.textContent}; pane=${settingsPanel.querySelector('.settings-content-pane')?.innerHTML.slice(0, 900)}`);
  disappearing.focus();
  await key('k', true);
  await act(async () => useAppStore.getState().switchSettingsTab('shortcuts'));
  assert.equal(disappearing.isConnected, false);
  await key('Escape');
  assert.equal(document.activeElement, settingsPanel, '原触发点失效时保留最上层存活浮层焦点');
  assert.equal(useAppStore.getState().settingsOpen, true);
});

componentTest('F6：AiTab 保存成功沿实际 API 装配使当前 Reader 重新只读验签，失败不改配置代次', { timeout: 25000 }, async () => {
  installBackend(); resetPlan(); resetStore(); clearInvokeCalls();
  seedFromRows([mkRow({ id: 101, content_id: 71, content_revision: 1, content_html: '<p>正文</p>', translated_content: '<p>旧译文</p>', translated_sig: 'old-sig' })]);
  let verified = true;
  let fail = false;
  setPlan({
    ai_cache_check: () => Promise.resolve({ summaryVerified: false, translationVerified: verified, translationSegments: { total: 1, completed: 0, discarded: 0 } }),
    save_ai_config: () => fail ? Promise.reject(new Error('save rejected')) : (verified = false, Promise.resolve(null)),
  });
  const { createElement, Fragment, act } = await import('react');
  const { Reader } = await import('../../src/components/Reader.tsx');
  const { AiTab } = await import('../../src/components/settings/AiTab.tsx');
  const { selectAiProductState } = await import('../../src/store.ts');
  const entry = await mount(createElement(Fragment, null, createElement(Reader), createElement(AiTab)));
  await act(async () => useAppStore.getState().selectArticle('101'));
  await waitFor(() => selectAiProductState(useAppStore.getState(), '101', 'translation') === 'complete');
  const save = [...entry.container.querySelectorAll('button')].find((b) => b.textContent === '保存提示词');
  assert.ok(save);
  const initial = useAppStore.getState().aiStatusRevision;
  fail = true;
  await act(async () => save.click()); await flush(3);
  assert.equal(useAppStore.getState().aiStatusRevision, initial);
  assert.equal(selectAiProductState(useAppStore.getState(), '101', 'translation'), 'complete');
  fail = false;
  await act(async () => save.click());
  await waitFor(() => selectAiProductState(useAppStore.getState(), '101', 'translation') === 'stale');
  assert.equal(useAppStore.getState().aiStatusRevision, initial + 1);
  assert.ok(invokeCount('ai_cache_check') >= 2);
  assert.equal(invokeCount('ai_translate') + invokeCount('ai_summarize'), 0, '显示失效不追加付费请求');
  assert.equal(getBodyEntry('101').translatedContent, '<p>旧译文</p>', '旧产物保留可读');
  resetPlan();
});

componentTest('F6：当前 Reader 提取全文后拒旧 complete，保留旧译文并只读重查', { timeout: 20000 }, async () => {
  installBackend(); resetPlan(); resetStore(); clearInvokeCalls();
  seedFromRows([mkRow({ id: 101, content_id: 71, content_revision: 1, content_html: '<p>RSS body</p>', translated_content: '<p>RSS translation</p>', translated_sig: 'rss-sig' })]);
  let extracted = false;
  setPlan({
    ai_cache_check: () => Promise.resolve({ summaryVerified: false, translationVerified: !extracted, translationSegments: { total: 1, completed: 0, discarded: 0 } }),
    extract_fulltext: () => { extracted = true; return Promise.resolve({ html: '<p>Fulltext body</p>', contentId: 72, contentRevision: 2, degraded: false }); },
  });
  const { createElement, act } = await import('react');
  const { Reader } = await import('../../src/components/Reader.tsx');
  const { selectAiProductState } = await import('../../src/store.ts');
  await mount(createElement(Reader));
  await act(async () => useAppStore.getState().selectArticle('101'));
  await waitFor(() => selectAiProductState(useAppStore.getState(), '101', 'translation') === 'complete');
  await act(async () => useAppStore.getState().extractCurrentArticle());
  await waitFor(() => selectAiProductState(useAppStore.getState(), '101', 'translation') === 'stale');
  assert.equal(getBodyEntry('101').content, '<p>Fulltext body</p>');
  assert.equal(getBodyEntry('101').translatedContent, '<p>RSS translation</p>');
  assert.equal(invokeCount('ai_translate') + invokeCount('ai_summarize'), 0);
  resetPlan();
});

for (const [layout, feedId, cardClass] of [['social', 11, 'social-card'], ['notification', 14, 'notif-card']]) {
  componentTest(`B-P2-2：${layout} 仅片段重启后卡片可见 partial/继续/清除，状态查询按当前窗口去重`, { timeout: 20000 }, async () => {
    installBackend(); resetPlan(); resetStore(); clearInvokeCalls();
    seedFromRows([mkRow({ id: 201, feed_id: feedId, content_id: 81, content_revision: 1, content_html: '<p>body</p>' })], { layout });
    setPlan({ ai_cache_check: () => Promise.resolve({ summaryVerified: false, translationVerified: false, translationSegments: { total: 4, completed: 2, discarded: 1 } }) });
    const { createElement, act } = await import('react');
    const { Timeline } = await import('../../src/components/Timeline.tsx');
    const entry = await mount(createElement(Timeline));
    await waitFor(() => entry.container.querySelector(`.${cardClass} .ai-partial-hint`));
    const block = entry.container.querySelector(layout === 'social' ? '.social-translated-block.show' : '.notif-translated-block.show');
    assert.ok(block, 'partial 区块实际展开，不只隐藏 DOM 含文案');
    assert.match(block.textContent, /2\/4/);
    assert.ok([...block.querySelectorAll('button')].some((b) => b.textContent === '继续'));
    assert.ok([...block.querySelectorAll('button')].some((b) => b.textContent === '清除'));
    const checks = invokeCount('ai_cache_check');
    await act(async () => useAppStore.getState().showToast('ordinary rerender'));
    await flush(3);
    assert.equal(invokeCount('ai_cache_check'), checks, '相同输入不因普通重渲染反复验签');
    assert.equal(invokeCount('ai_translate') + invokeCount('ai_summarize'), 0, '卡片挂载只读取状态');
    resetPlan();
  });
}
