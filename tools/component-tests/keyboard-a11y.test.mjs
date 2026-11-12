import assert from 'node:assert/strict';
import { componentTest, mount, flush, waitFor, resetStore, useAppStore, pressKey, storeMod } from './harness.mjs';
import { installBackend, seedFromRows, mkRow } from './fixtures.mjs';

const { selectVisibleEntries } = storeMod;

const CARD_SELECTOR = {
  article: '.article-card',
  social: '.social-card',
  image: '.gallery-card',
  podcast: '.podcast-card',
  notification: '.notif-card',
};

function keyboardRows() {
  const base = Date.UTC(2026, 10, 4, 12, 0, 0);
  const feeds = [
    { feed: 10, layout: 'article', prefix: 'A' },
    { feed: 11, layout: 'social', prefix: 'S' },
    { feed: 12, layout: 'image', prefix: 'G' },
    { feed: 13, layout: 'podcast', prefix: 'P' },
    { feed: 14, layout: 'notification', prefix: 'N' },
  ];
  return feeds.flatMap(({ feed, layout, prefix }) => Array.from({ length: 3 }, (_, index) => mkRow({
    id: Number(`${feed}01`) + index,
    feed_id: feed,
    title: `${prefix}${index + 1} ${layout} 键盘条目`,
    snippet: `${prefix}${index + 1} 摘要`,
    published_at: new Date(base - index * 60_000).toISOString(),
    is_starred: true,
    image_url: layout === 'image' ? `https://img.example/${prefix}${index}.png` : null,
    enclosure_url: layout === 'podcast' ? `https://audio.example/${prefix}${index}.mp3` : null,
    enclosure_mime: layout === 'podcast' ? 'audio/mpeg' : null,
    duration_sec: layout === 'podcast' ? 600 + index : null,
    content_html: layout === 'social' ? `<p>${prefix}${index + 1} 正文</p>` : null,
  })));
}

function focusedCardId() {
  const active = document.activeElement;
  if (!active || active === document.body) return null;
  const owner = active.closest?.('[data-id]') ?? active;
  return owner?.getAttribute?.('data-id') ?? null;
}

function liveRegion(container) {
  const node = container.querySelector('[data-live-region="timeline"]');
  assert.ok(node, 'Timeline 应有 live region');
  assert.equal(node.getAttribute('role'), 'status', 'live region 应为 role=status');
  assert.equal(node.getAttribute('aria-live'), 'polite', 'live region 应为 aria-live=polite');
  assert.equal(node.getAttribute('aria-atomic'), 'true', 'live region 应 atomic=true');
  return node;
}

function dispatchKeyOn(target, key) {
  const event = new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true });
  target.dispatchEvent(event);
  return event;
}

componentTest('键盘与焦点：J/K 在五布局同步 activeArticleId、DOM focus、aria-current 与可见选中态', { timeout: 60000 }, async () => {
  installBackend();
  resetStore();
  seedFromRows(keyboardRows(), { layout: 'article' });

  const { createElement, act } = await import('react');
  const { default: App } = await import('../../src/App.tsx');
  const entry = await mount(createElement(App));
  await waitFor(() => entry.container.querySelectorAll('.article-card').length === 3,
    { label: 'App 启动后文章卡出现', timeout: 8000 });

  for (const layout of ['article', 'social', 'image', 'podcast', 'notification']) {
    await act(async () => { useAppStore.getState().selectLayout(layout); });
    await waitFor(() => entry.container.querySelectorAll(CARD_SELECTOR[layout]).length === 3,
      { label: `${layout} 三张卡出现`, timeout: 5000 });

    const visible = selectVisibleEntries(useAppStore.getState()).map((item) => item.id);
    assert.equal(visible.length, 3, `${layout}: 前置应有 3 条可见条目`);
    document.activeElement?.blur?.();

    await act(async () => { pressKey('j'); });
    await flush(4, 30);
    assert.equal(useAppStore.getState().activeArticleId, visible[0],
      `${layout}: J 无选中时应选中第一条`);
    assert.equal(focusedCardId(), visible[0], `${layout}: J 后 DOM 焦点应在目标卡`);
    assert.equal(document.activeElement?.getAttribute('aria-current'), 'true',
      `${layout}: 焦点卡应带 aria-current=true`);
    assert.ok(entry.container.querySelector(`.active-selected[data-id="${visible[0]}"]`),
      `${layout}: 目标卡应有 active-selected 可见选中态`);

    await act(async () => { pressKey('j'); });
    await flush(4, 30);
    assert.equal(useAppStore.getState().activeArticleId, visible[1],
      `${layout}: 第二次 J 应前进到第二条`);
    assert.equal(focusedCardId(), visible[1], `${layout}: 第二次 J 后焦点应跟随第二条`);

    await act(async () => { pressKey('k'); });
    await flush(4, 30);
    assert.equal(useAppStore.getState().activeArticleId, visible[0],
      `${layout}: K 应回到第一条`);
    assert.equal(focusedCardId(), visible[0], `${layout}: K 后焦点应回到第一条`);
  }
});

componentTest('收藏会话取消收藏：当前卡与焦点原地保留，live region 宣告状态变化', { timeout: 40000 }, async () => {
  installBackend();
  resetStore();
  seedFromRows(keyboardRows().filter((row) => Number(row.feed_id) === 11), { layout: 'social' });
  useAppStore.setState({ activeViewFilter: 'starred', timelineFilter: 'all' });
  await useAppStore.getState().reloadFromBackend();

  const { createElement, act } = await import('react');
  const { Timeline } = await import('../../src/components/Timeline.tsx');
  const entry = await mount(createElement(Timeline));
  await waitFor(() => entry.container.querySelectorAll('.social-card').length === 3,
    { label: '社交卡出现', timeout: 5000 });

  const before = selectVisibleEntries(useAppStore.getState());
  const firstCard = entry.container.querySelector('.social-card');
  assert.ok(firstCard, '前置：第一张社交卡存在');
  await act(async () => {
    firstCard.focus();
    const star = firstCard.querySelector('.social-act-item.starred');
    assert.ok(star, '卡片应有取消收藏按钮');
    star.focus();
    star.click();
  });
  await flush(4, 30);

  assert.equal(selectVisibleEntries(useAppStore.getState())[0]?.id, before[0].id, '取消收藏不抽走当前会话成员');
  assert.equal(focusedCardId(), before[0].id, '焦点保留在原卡');
  const live = liveRegion(entry.container).textContent ?? '';
  assert.match(live, /已取消收藏/, 'live region 应宣告取消收藏');
  assert.ok(live.includes(before[0].title), 'live region 应包含状态变化的当前条目标题');
});

componentTest('画廊收藏会话取消收藏：跨 lane 的当前焦点与成员保持', { timeout: 40000 }, async () => {
  installBackend();
  resetStore();
  seedFromRows(keyboardRows().filter((row) => Number(row.feed_id) === 12), { layout: 'image' });
  useAppStore.setState({ activeViewFilter: 'starred', timelineFilter: 'all' });
  await useAppStore.getState().reloadFromBackend();

  const { createElement, act } = await import('react');
  const { Timeline } = await import('../../src/components/Timeline.tsx');
  const entry = await mount(createElement(Timeline));
  await waitFor(() => entry.container.querySelectorAll('.gallery-card').length === 3,
    { label: '画廊卡出现', timeout: 5000 });

  const before = selectVisibleEntries(useAppStore.getState());
  const firstCard = entry.container.querySelector('.gallery-card');
  const star = firstCard?.querySelector('.gallery-meta button[title="取消收藏"]');
  assert.ok(star, '前置：画廊卡应有取消收藏按钮');
  await act(async () => {
    star.focus();
    star.click();
  });
  await flush(4, 30);

  assert.equal(focusedCardId(), before[0].id, '画廊当前焦点不因收藏变化跨 lane 迁移');
  const live = liveRegion(entry.container).textContent ?? '';
  assert.match(live, /已取消收藏/);
});

componentTest('Sidebar：源行主选择与刷新/编辑为同级 button，active 状态暴露 aria-pressed/aria-current', { timeout: 30000 }, async () => {
  installBackend();
  resetStore();
  seedFromRows(keyboardRows(), { layout: 'article' });

  const { createElement, act } = await import('react');
  const { Sidebar } = await import('../../src/components/Sidebar.tsx');
  const entry = await mount(createElement(Sidebar));
  await flush(2);

  const search = entry.container.querySelector('.sidebar-search-pill');
  assert.ok(search, '搜索入口应存在');
  assert.equal(search.tagName, 'BUTTON', '搜索入口应为真实 button');
  assert.equal(search.getAttribute('aria-label'), null, '可见文字已提供名称，不重复加 aria-label');

  const feedRow = entry.container.querySelector('.feed-leaf-item[data-id="10"]');
  assert.ok(feedRow, '源 10 的侧栏行应存在');
  assert.equal(feedRow.getAttribute('role'), null, '源行容器不得再冒充 button');
  assert.equal(feedRow.getAttribute('tabindex'), null, '源行容器不得进入 Tab 序列');
  const selectButton = feedRow.querySelector('.feed-leaf-select');
  assert.ok(selectButton, '源行应有主选择 button');
  assert.equal(selectButton.tagName, 'BUTTON', '主选择区应为真实 button');
  assert.equal(selectButton.getAttribute('aria-current'), null, '未选中的源不应暴露 aria-current');

  const buttons = [...feedRow.querySelectorAll('button')];
  assert.equal(buttons.length, 3, '源行应有一个主选择按钮和刷新/编辑两个动作按钮');
  assert.deepEqual(buttons.map((button) => button.dataset.feedAction ?? 'select'),
    ['select', 'refresh', 'edit'], 'Tab/DOM 顺序应为主选择、刷新、编辑');
  for (const button of buttons) {
    assert.equal(button.parentElement?.closest('button, [role="button"]'), null,
      '源行按钮不得嵌套在 button/[role=button] 里');
  }

  const actionButtons = [...feedRow.querySelectorAll('.feed-leaf-actions button')];
  assert.equal(actionButtons.length, 2, '源行应有刷新与编辑动作');
  for (const action of actionButtons) {
    assert.equal(action.tagName, 'BUTTON', '动作应为真实 button');
    assert.ok(action.getAttribute('aria-label')?.includes('订阅源 源A'),
      '动作按钮应带源名稳定的可访问名');
  }

  await act(async () => {
    selectButton.click();
  });
  await flush(2);
  assert.equal(useAppStore.getState().activeFeedFilter, '10', '点击主选择区应激活订阅源行');
  assert.equal(selectButton.getAttribute('aria-current'), 'true', '当前订阅源应暴露 aria-current');

  const selectedBeforeAction = useAppStore.getState().activeFeedFilter;
  await act(async () => {
    actionButtons[1].click();
  });
  await flush(2);
  assert.equal(useAppStore.getState().editFeedModalOpen, true, '编辑动作应打开编辑框');
  assert.equal(useAppStore.getState().activeFeedFilter, selectedBeforeAction,
    '动作按钮的 click 应 stopPropagation，不得改选源');

  const pressed = [...entry.container.querySelectorAll('[aria-pressed="true"]')];
  assert.equal(pressed.length, 2, '当前视图与当前布局各有一个 aria-pressed=true');
});

componentTest('收藏会话末项取消收藏：末项焦点保留，live region 保持 polite/atomic', { timeout: 40000 }, async () => {
  installBackend();
  resetStore();
  seedFromRows(keyboardRows().filter((row) => Number(row.feed_id) === 11), { layout: 'social' });
  useAppStore.setState({ activeViewFilter: 'starred', timelineFilter: 'all' });
  await useAppStore.getState().reloadFromBackend();

  const { createElement, act } = await import('react');
  const { Timeline } = await import('../../src/components/Timeline.tsx');
  const entry = await mount(createElement(Timeline));
  await waitFor(() => entry.container.querySelectorAll('.social-card').length === 3,
    { label: '三条社交卡出现', timeout: 5000 });

  const before = selectVisibleEntries(useAppStore.getState());
  const cards = [...entry.container.querySelectorAll('.social-card')];
  const lastCard = cards.at(-1);
  await act(async () => {
    lastCard.focus();
    const star = lastCard.querySelector('.social-act-item.starred');
    assert.ok(star, '末项应有取消收藏按钮');
    star.focus();
    star.click();
  });
  await flush(4, 30);

  assert.equal(selectVisibleEntries(useAppStore.getState()).length, 3, '末项仍属于当前会话');
  assert.equal(focusedCardId(), before[2].id, '末项焦点保留');
  const node = liveRegion(entry.container);
  assert.ok(node.textContent?.includes(before[2].title), 'live region 点名当前末项');
});

componentTest('收藏会话最后一项取消收藏：当前列表不空、不移动焦点', { timeout: 40000 }, async () => {
  installBackend();
  resetStore();
  seedFromRows(keyboardRows().filter((row) => Number(row.feed_id) === 11).slice(0, 1), { layout: 'social' });
  useAppStore.setState({ activeViewFilter: 'starred', timelineFilter: 'all' });
  await useAppStore.getState().reloadFromBackend();

  const { createElement, act } = await import('react');
  const { Timeline } = await import('../../src/components/Timeline.tsx');
  const entry = await mount(createElement(Timeline));
  await waitFor(() => entry.container.querySelectorAll('.social-card').length === 1,
    { label: '单条社交卡出现', timeout: 5000 });

  const card = entry.container.querySelector('.social-card');
  await act(async () => {
    card.focus();
    const star = card.querySelector('.social-act-item.starred');
    assert.ok(star, '单条卡应有取消收藏按钮');
    star.focus();
    star.click();
  });
  await flush(4, 30);

  assert.equal(selectVisibleEntries(useAppStore.getState()).length, 1, '最后一项仍在当前会话');
  assert.equal(focusedCardId(), card.dataset.id, '最后一项焦点保留在原卡');
  const node = liveRegion(entry.container);
  assert.match(node.textContent ?? '', /已取消收藏/, 'live region 宣告状态变化');
});

componentTest('焦点迁移：焦点在筛选器时不抢走焦点', { timeout: 40000 }, async () => {
  installBackend();
  resetStore();
  seedFromRows(keyboardRows().filter((row) => Number(row.feed_id) === 11), { layout: 'social' });
  useAppStore.setState({ activeViewFilter: 'starred', timelineFilter: 'all' });
  await useAppStore.getState().reloadFromBackend();

  const { createElement, act } = await import('react');
  const { Timeline } = await import('../../src/components/Timeline.tsx');
  const entry = await mount(createElement(Timeline));
  await waitFor(() => entry.container.querySelectorAll('.social-card').length === 3,
    { label: '三条社交卡出现', timeout: 5000 });

  const before = selectVisibleEntries(useAppStore.getState());
  const firstCard = entry.container.querySelector('.social-card');
  const filterButton = entry.container.querySelector('.filter-sort-group button');
  assert.ok(filterButton, '时间流筛选按钮应存在');

  await act(async () => {
    firstCard.focus();
    filterButton.focus();
    await useAppStore.getState().toggleEntryFlag(before[0].id, 'isStarred');
  });
  await flush(4, 30);

  assert.equal(selectVisibleEntries(useAppStore.getState()).length, 3, '属性变化不重筛当前会话');
  assert.equal(document.activeElement, filterButton,
    '用户焦点在筛选器时，条目移除不得抢焦点');
});

componentTest('焦点迁移：焦点在侧栏时不抢走焦点', { timeout: 50000 }, async () => {
  installBackend();
  resetStore();
  seedFromRows(keyboardRows().filter((row) => Number(row.feed_id) === 11), { layout: 'social' });
  useAppStore.setState({ activeViewFilter: 'starred', timelineFilter: 'all' });
  await useAppStore.getState().reloadFromBackend();

  const { createElement, act } = await import('react');
  const { default: App } = await import('../../src/App.tsx');
  const entry = await mount(createElement(App));
  await waitFor(() => entry.container.querySelectorAll('.social-card').length === 3,
    { label: 'App 启动后三条社交卡出现', timeout: 8000 });

  const before = selectVisibleEntries(useAppStore.getState());
  const firstCard = entry.container.querySelector('.social-card');
  const sidebarSelect = entry.container.querySelector('.sidebar .feed-leaf-select');
  assert.ok(sidebarSelect, '侧栏源行主选择按钮应存在');

  await act(async () => {
    firstCard.focus();
    sidebarSelect.focus();
    await useAppStore.getState().toggleEntryFlag(before[0].id, 'isStarred');
  });
  await flush(4, 30);

  assert.equal(selectVisibleEntries(useAppStore.getState()).length, 3, '属性变化不重筛当前会话');
  assert.equal(document.activeElement, sidebarSelect,
    '用户焦点在侧栏时，条目移除不得抢焦点');
});

componentTest('Space 让路清单覆盖原生交互控件，排除卡片容器 role', { timeout: 20000 }, async () => {
  const { isInteractiveSpaceTarget } = await import('../../src/components/shortcutYield.ts');
  const probe = document.createElement('div');
  const cases = [
    ['<button><span>按钮</span></button>', true],
    ['<a href="#"><span>链接</span></a>', true],
    ['<input>', true],
    ['<select><option>选项</option></select>', true],
    ['<textarea></textarea>', true],
    ['<div contenteditable="true"><span>可编辑</span></div>', true],
    ['<div role="menuitem">菜单项</div>', true],
    ['<div role="combobox">下拉</div>', true],
    ['<article role="button" tabindex="0">卡片</article>', false],
  ];
  for (const [html, expected] of cases) {
    probe.innerHTML = html;
    const target = probe.firstElementChild?.querySelector('span')
      ?? probe.firstElementChild;
    assert.equal(isInteractiveSpaceTarget(target), expected, `${html} 的 Space 让路判定错误`);
  }
});

componentTest('播放器激活时 Space 让路真实按钮，但卡片仍切换播放', { timeout: 40000 }, async () => {
  installBackend();
  resetStore();
  seedFromRows(keyboardRows().filter((row) => Number(row.feed_id) === 10), { layout: 'article' });

  const { createElement, act } = await import('react');
  const { default: App } = await import('../../src/App.tsx');
  const entry = await mount(createElement(App));
  await waitFor(() => entry.container.querySelectorAll('.article-card').length === 3,
    { label: 'App 启动后三条文章卡出现', timeout: 8000 });

  /* jsdom 不实现 HTMLMediaElement.play/pause；这里只补齐环境，不替身播放器逻辑。 */
  const audio = document.createElement('audio');
  let mediaProto = Object.getPrototypeOf(audio);
  while (mediaProto && !Object.prototype.hasOwnProperty.call(mediaProto, 'play')) {
    mediaProto = Object.getPrototypeOf(mediaProto);
  }
  assert.ok(mediaProto, '应找到 HTMLMediaElement.play 的原型');
  const playDescriptor = Object.getOwnPropertyDescriptor(mediaProto, 'play');
  const pauseDescriptor = Object.getOwnPropertyDescriptor(mediaProto, 'pause');
  assert.ok(playDescriptor && pauseDescriptor, '媒体原型应同时定义 play/pause');
  Object.defineProperty(mediaProto, 'play', {
    configurable: true,
    writable: true,
    value: () => Promise.resolve(),
  });
  Object.defineProperty(mediaProto, 'pause', {
    configurable: true,
    writable: true,
    value: () => {},
  });

  try {
    await act(async () => {
      useAppStore.setState((state) => ({
        player: {
          ...state.player,
          isActive: true,
          isPlaying: false,
          title: '测试剧集',
          audioUrl: 'https://audio.example/test.mp3',
        },
      }));
    });

    const search = entry.container.querySelector('.sidebar-search-pill');
    assert.ok(search, '搜索按钮应存在');
    await act(async () => {
      search.focus();
      dispatchKeyOn(search, ' ');
    });
    assert.equal(useAppStore.getState().player.isPlaying, false,
      'Space 在按钮上不得被播放器全局快捷键吞掉');

    const feedSelect = entry.container.querySelector('.feed-leaf-select');
    assert.ok(feedSelect, '侧栏源行主选择按钮应存在');
    await act(async () => {
      feedSelect.focus();
      dispatchKeyOn(feedSelect, ' ');
    });
    assert.equal(useAppStore.getState().player.isPlaying, false,
      'Space 在源行主选择按钮上不得切换播放');

    const card = entry.container.querySelector('.article-card');
    assert.ok(card, '文章卡应存在');
    await act(async () => {
      card.focus();
      dispatchKeyOn(card, ' ');
    });
    assert.equal(useAppStore.getState().player.isPlaying, true,
      'Space 在卡片上仍应切换播放器播放状态');

    await act(async () => {
      dispatchKeyOn(card, ' ');
    });
    assert.equal(useAppStore.getState().player.isPlaying, false,
      '卡片上的 Space 应可再次暂停播放');
  } finally {
    Object.defineProperty(mediaProto, 'play', playDescriptor);
    Object.defineProperty(mediaProto, 'pause', pauseDescriptor);
  }
});

componentTest('设置页表单可访问名：checkbox/slider/textbox/combobox 均有稳定名称，combobox 不再以当前选项值命名', { timeout: 40000 }, async () => {
  installBackend();
  resetStore();
  seedFromRows(keyboardRows(), { layout: 'article' });

  const { createElement, act } = await import('react');
  const { SettingsModal } = await import('../../src/components/SettingsModal.tsx');
  const entry = await mount(createElement(SettingsModal));
  await act(async () => { useAppStore.getState().openSettings(); });
  await flush(2);

  const nameOf = (el) => {
    const aria = el.getAttribute('aria-label')?.trim();
    if (aria) return aria;
    const labelledBy = el.getAttribute('aria-labelledby');
    if (labelledBy) {
      const parts = labelledBy.split(/\s+/).map((id) => document.getElementById(id)?.textContent?.trim()).filter(Boolean);
      if (parts.length) return parts.join(' ');
    }
    const labels = [...(el.labels ?? [])].map((label) => label.textContent?.trim()).filter(Boolean);
    return labels.join(' ') || null;
  };
  const currentValueOf = (el) => {
    if (el.getAttribute('role') === 'combobox') {
      return el.querySelector('.flux-dropdown-label')?.textContent?.trim()
        ?? el.textContent?.trim()
        ?? '';
    }
    if (el.tagName === 'SELECT') return el.selectedOptions[0]?.textContent?.trim() ?? '';
    return el.textContent?.trim() ?? '';
  };

  for (const tab of ['general', 'appearance', 'reading', 'feeds', 'ai', 'sync', 'shortcuts', 'about']) {
    await act(async () => { useAppStore.getState().switchSettingsTab(tab); });
    await flush(2);
    const pane = entry.container.querySelector('.settings-content-pane');
    assert.ok(pane, `${tab}: 设置内容区应存在`);
    const controls = [...pane.querySelectorAll('input:not([type="hidden"]):not([type="file"]), textarea, [role="combobox"]')];
    for (const control of controls) {
      const name = nameOf(control);
      assert.ok(name, `${tab}: 控件 ${control.tagName.toLowerCase()} ${control.getAttribute('type') ?? ''} 应有可访问名（class=${control.className}）`);
      if (control.getAttribute('role') === 'combobox') {
        const currentValue = currentValueOf(control);
        assert.ok(currentValue, `${tab}: combobox 应能读取当前选项值`);
        assert.notEqual(name, currentValue,
          `${tab}: combobox 名称不得只是当前选项值「${currentValue}」`);
      }
    }
  }
});
