// 2026-11-03：社交正文首帧折叠回归。
//
// 证据边界：jsdom 没有真实布局引擎，不能证明像素高度；这里锁的是生产组件
// 在正文落地时的**首帧类名策略**与 effect 时序：
// - 受控 ResizeObserver 不自动回调，排除「RO 事后补折叠」掩盖缺陷；
// - observe 自然高度内层时立即抓取 .social-text 的类名——长文落地后的第一次
//   commit 必须已经是 collapsed；旧实现先完整渲染、再由 effect/RO 折叠时，这里必红；
// - 真实几何由 tools/ui-layout-regression.mjs 的 Edge/CDP 量测承担。

import assert from 'node:assert/strict';
import { componentTest, mount, flush, waitFor, resetStore } from './harness.mjs';
import { installBackend, seedFromRows, mkRow, setPlan, deferred } from './fixtures.mjs';

const LONG_MARKER = 'LONG_SOCIAL_BODY';
const SHORT_MARKER = 'SHORT_SOCIAL_BODY';
const LONG_HTML = `<p>${LONG_MARKER}</p>${'<p>长文段落：用于验证首帧折叠与后续展开。</p>'.repeat(24)}`;
const SHORT_HTML = `<p>${SHORT_MARKER}</p>`;

const LONG_ROW = mkRow({
  id: 201,
  feed_id: 11,
  title: '社交首帧折叠',
  snippet: '长摘要用于判断正文区应预留到折叠上限。'.repeat(8),
});
const SHORT_ROW = mkRow({ id: 201, feed_id: 11, title: '社交短文', snippet: '短摘要' });
const contentRow = (row, html) => ({ ...row, content_html: html });

/** 用正文内容模拟 scrollHeight：jsdom 无布局，60/80 视为短文，900 视为长文。 */
function installScrollHeightByContent() {
  const proto = window.HTMLElement.prototype;
  const original = Object.getOwnPropertyDescriptor(proto, 'scrollHeight');
  Object.defineProperty(proto, 'scrollHeight', {
    configurable: true,
    get() {
      if (!this.classList?.contains('social-text-inner')) return 0;
      const text = this.textContent ?? '';
      if (text.includes(LONG_MARKER)) return 900;
      if (text.includes(SHORT_MARKER)) return 80;
      return 60;
    },
  });
  return () => {
    if (original) Object.defineProperty(proto, 'scrollHeight', original);
    else delete proto.scrollHeight;
  };
}

/**
 * 受控 RO：社交正文自然高度内层的 observe 只记录首帧类名，不自动回调；其余元素
 * （滚动容器、虚拟行）仍交给 harness 的同步 RO 替身，避免改变虚拟化环境。
 */
function installControlledSocialResizeObserver() {
  const NativeResizeObserver = globalThis.ResizeObserver;
  const socialObservations = [];
  class ControlledResizeObserver {
    constructor(callback) {
      this.callback = callback;
      this.native = null;
    }

    observe(target) {
      if (target.classList?.contains('social-text-inner')) {
        const text = target.closest('.social-text');
        socialObservations.push({
          target,
          className: text?.className ?? '',
          text: target.textContent ?? '',
          callback: this.callback,
          observer: this,
        });
        return;
      }
      if (!this.native) this.native = new NativeResizeObserver(this.callback);
      this.native.observe(target);
    }

    unobserve(target) {
      this.native?.unobserve?.(target);
    }

    disconnect() {
      this.native?.disconnect?.();
    }
  }
  globalThis.ResizeObserver = ControlledResizeObserver;
  window.ResizeObserver = ControlledResizeObserver;
  return {
    socialObservations,
    restore() {
      globalThis.ResizeObserver = NativeResizeObserver;
      window.ResizeObserver = NativeResizeObserver;
    },
  };
}

componentTest('社交长文水合：完整正文落地后的首次 commit 已折叠，受控 RO 不回调也不出现未折叠态', { timeout: 30000 }, async () => {
  installBackend();
  resetStore();
  seedFromRows([LONG_ROW], { layout: 'social' });
  const restoreScrollHeight = installScrollHeightByContent();
  const controlledResizeObserver = installControlledSocialResizeObserver();

  try {
    const batch = deferred();
    setPlan({ get_articles: () => batch.promise });

    const { createElement, act } = await import('react');
    const { Timeline } = await import('../../src/components/Timeline.tsx');
    const entry = await mount(createElement(Timeline));
    await flush(3);

    const card = entry.container.querySelector('.social-card[data-id="201"]');
    assert.ok(card, '社交卡应已挂载');
    const loadingText = card.querySelector('.social-text');
    assert.ok(loadingText.classList.contains('social-text-loading'),
      '占位阶段应显式标记，供 CSS 预留与折叠态同高的正文区');
    assert.equal(card.style.height, '310px',
      '折叠态外高必须是固定不变量，占位首帧即为 310px');
    assert.equal(loadingText.style.height, '150px',
      '占位与水合正文共用同一固定正文区高度');
    assert.ok(card.querySelector('.social-expand-slot'),
      '展开按钮槽位应始终存在，按钮显隐不得改变卡片高度');
    assert.ok(card.querySelector('.hydrate-placeholder')?.textContent.includes('加载正文…'),
      '正文未落地时应先显示加载占位');

    await act(async () => {
      batch.resolve([contentRow(LONG_ROW, LONG_HTML)]);
      await Promise.resolve();
    });
    await waitFor(
      () => entry.container.querySelector('.social-text')?.textContent.includes(LONG_MARKER),
      { label: '长文正文落地' },
    );

    const text = card.querySelector('.social-text');
    const inner = text.querySelector('.social-text-inner');
    const firstLongObservation = controlledResizeObserver.socialObservations
      .find((observation) => observation.target === inner && observation.text.includes(LONG_MARKER));
    assert.ok(firstLongObservation, '长文落地后应重新 observe 正文元素以接管后续图片高度校正');
    assert.match(firstLongObservation.className, /\bcollapsed\b/,
      '长文落地后的首次 commit/observe 必须已带 collapsed——不得先以完整正文高度进入布局');
    assert.match(text.className, /\bcollapsed\b/, '长文在无 RO 回调时也应保持折叠');
    assert.match(text.className, /\bsocial-text-clipped\b/,
      '长文应在测量后加裁切渐变标记（不改变折叠高度）');
    assert.doesNotMatch(text.className, /\bsocial-text-loading\b/,
      '正文落地后应移除占位高度标记');
    const expandButton = card.querySelector('.social-expand-btn');
    assert.ok(expandButton?.textContent.includes('展开更多'),
      '长文折叠后应提供展开入口');
    assert.equal(expandButton.getAttribute('aria-expanded'), 'false',
      '折叠态展开按钮必须暴露 aria-expanded=false');
    assert.equal(expandButton.getAttribute('aria-controls'), 'social-text-201',
      '展开按钮必须通过 aria-controls 指向正文节点');

    const latestObservation = controlledResizeObserver.socialObservations
      .filter((observation) => observation.target === inner)
      .at(-1);
    await act(async () => {
      latestObservation.callback([{ target: text, contentRect: { width: 800, height: 900 } }], latestObservation.observer);
      await Promise.resolve();
    });
    assert.match(text.className, /\bcollapsed\b/, '后续 RO 校正长文后仍应保持折叠');

    await act(async () => {
      expandButton.click();
    });
    assert.doesNotMatch(text.className, /\bcollapsed\b/, '用户点击展开后应移除折叠');
    assert.doesNotMatch(text.className, /\bsocial-text-clipped\b/,
      '展开后应移除底部裁切渐变');
    assert.equal(card.style.height, '', '展开后必须退出固定外高，按真实内容增长');
    assert.equal(text.style.height, '', '展开后正文区必须退出固定高度');
    assert.ok(card.querySelector('.social-expand-btn')?.textContent.includes('收起内容'));
    assert.equal(card.querySelector('.social-expand-btn').getAttribute('aria-expanded'), 'true',
      '展开态按钮必须暴露 aria-expanded=true');
  } finally {
    controlledResizeObserver.restore();
    restoreScrollHeight();
  }
});

componentTest('社交短文水合：始终保持折叠类但天然低于上限，不出现展开按钮', { timeout: 30000 }, async () => {
  installBackend();
  resetStore();
  seedFromRows([SHORT_ROW], { layout: 'social' });
  const restoreScrollHeight = installScrollHeightByContent();
  const controlledResizeObserver = installControlledSocialResizeObserver();

  try {
    const batch = deferred();
    setPlan({ get_articles: () => batch.promise });

    const { createElement, act } = await import('react');
    const { Timeline } = await import('../../src/components/Timeline.tsx');
    const entry = await mount(createElement(Timeline));
    await flush(3);

    const card = entry.container.querySelector('.social-card[data-id="201"]');
    const placeholderText = card.querySelector('.social-text');
    assert.equal(card.style.height, '310px',
      '短文同样保持统一折叠外高');
    assert.equal(placeholderText.style.height, '150px',
      '短文正文区同样保持统一固定高度');
    assert.match(placeholderText.className, /\bcollapsed\b/,
      '占位首帧即使用统一 collapsed CSS');

    await act(async () => {
      batch.resolve([contentRow(SHORT_ROW, SHORT_HTML)]);
      await Promise.resolve();
    });
    await waitFor(
      () => entry.container.querySelector('.social-text')?.textContent.includes(SHORT_MARKER),
      { label: '短文正文落地' },
    );

    const text = card.querySelector('.social-text');
    assert.match(text.className, /\bcollapsed\b/,
      '短文也保留 collapsed；实际高度天然低于固定上限，无需 JS 移除');
    assert.doesNotMatch(text.className, /\bsocial-text-clipped\b/,
      '短文没有溢出，不应出现底部裁切渐变');
    assert.equal(card.querySelector('.social-expand-btn'), null, '短文不应出现展开按钮');
    assert.ok(card.querySelector('.social-expand-slot'), '短文仍保留固定高度按钮槽位');
  } finally {
    controlledResizeObserver.restore();
    restoreScrollHeight();
  }
});
