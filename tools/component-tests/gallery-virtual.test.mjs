// OPT-017C：画廊（瀑布流）有界渲染回归——窗口内挂载数有界、键盘焦点、灯箱返回、
// 加载更多、列分配。
//
// 取证方式与证据边界（如实说明，不夸大）：
// - 挂载数 = 真实 React 树里的 DOM 节点计数（container.querySelectorAll('.gallery-card')），
//   在多档滚动偏移下扫描；不是 SSR，也不是「数组长度」自证。
// - jsdom 无布局引擎、无图片解码：本文件能证明「挂载节点数有界 + 列分配与索引映射正确 +
//   交互状态迁移」；**不能**证明真实 WebView 的滚动帧率、图片解码与布局性能——未测边界与
//   真实节点数测量方法见 docs/performance.md「画廊布局有界渲染」节。
// - 滚动模拟：jsdom 不派发真实 scroll 事件（scrollTop 赋值/scrollTo 桩均无事件），
//   故测试显式 dispatchEvent('scroll')；scrollHeight 用实例级 defineProperty 补齐
//   （jsdom 无布局，恒 0）。程序性 scrollToIndex 在真实浏览器会引发 scroll 事件，
//   测试同样手动补齐，否则虚拟化读不到新偏移（harness 边界，非产品行为）。
// - 高度口径：harness 的 offsetHeight 替身对 .gallery-virtual-item 返回统一 260px，
//   虚拟化据此测量/估算——列分配断言（index % 4）依赖「等高」前提，与真实图片高度
//   分布无关（真实分布的列分配由同一贪心算法决定，不在此自证）。
// - 列分配语义断言对照 @tanstack/react-virtual lanes（贪心最短列）：等高时退化为
//   轮转（index % lanes），列宽/列距公式与旧 CSS 多列逐字等价（GalleryLayout.ts）。

import assert from 'node:assert/strict';
import { componentTest, mount, unmount, flush, waitFor, resetStore, useAppStore } from './harness.mjs';
import { installBackend, seedFromRows, mkRow, setCurrentRows, setPlan, resetPlan } from './fixtures.mjs';
import {
  GALLERY_CARD_MIN_HEIGHT,
  GALLERY_COLUMN_GAP,
  GALLERY_LANES_WIDE,
  GALLERY_MOUNT_BOUND_REF,
  estimateGalleryCardHeight,
  galleryItemPlacement,
  galleryMountBound,
} from '../../src/components/GalleryLayout.ts';

const IMAGE_FEED = 12; // fixtures.mjs：源C 绑定 image 布局

/** 画廊行夹具：id 递增、发布时间递减（newest-first 下 index 0 = startId），带封面。 */
function galleryRows(n, startId = 10000) {
  const base = Date.UTC(2026, 9, 16, 12, 0, 0);
  return Array.from({ length: n }, (_, i) => mkRow({
    id: startId + i,
    feed_id: IMAGE_FEED,
    title: `画廊条目 ${startId + i}`,
    snippet: `摘要 ${startId + i}`,
    image_url: `https://img.example/${startId + i}.png`,
    published_at: new Date(base - i * 60000).toISOString(),
  }));
}

/** 补齐 jsdom 缺失的滚动几何：scrollHeight 可控（clientHeight 恒 0，够用）。 */
function stubScrollHeight(scroller, height) {
  Object.defineProperty(scroller, 'scrollHeight', { configurable: true, get: () => height });
}

/** 滚动到指定偏移并派发 scroll（全部在 act 内，避免「not wrapped in act」污染错误账本）。 */
async function scrollToOffset(act, scroller, top) {
  await act(async () => {
    scroller.scrollTop = top;
    scroller.dispatchEvent(new Event('scroll'));
  });
  await flush(3);
}

/** 窗口内挂载的画廊卡 wrapper（.gallery-virtual-item：带 data-index，供 measureElement）。 */
function mountedWrappers(entry) {
  return [...entry.container.querySelectorAll('.gallery-masonry-grid > .gallery-virtual-item')];
}

/** wrapper → 全局条目下标（虚拟化的 data-index 与 items 下标同口径）。 */
function wrapperIndex(wrap) {
  return Number(wrap.getAttribute('data-index'));
}

/** wrapper 内可聚焦的卡片元素（img 或「无图」占位 div——同一 data-card-index 协议）。 */
function focusableOf(wrap) {
  return wrap.querySelector('[data-card-index]');
}

/** jsdom 的 cssstyle 会把 calc() 归一化（如 "calc((100% - 42px) / 4 * 1 + 14px)" →
    "calc(14px + 0.25 * (100% - 42px))"），故断言用「同宿主归一化后的串」比对——
    公式等价性由 GalleryLayout 的纯函数真值表承担，这里证明组件真的用了该公式。 */
function canonCssValue(prop, value) {
  const probe = document.createElement('div');
  probe.style[prop] = value;
  return probe.style[prop];
}

componentTest('OPT-017C 画廊有界渲染：5000 项时同时挂载数有上限、远小于总数，且不随总数线性增长', { timeout: 150000 }, async () => {
  installBackend();
  resetStore();
  const rows = galleryRows(5000, 10000);
  seedFromRows(rows, { layout: 'image' });

  const { createElement, act } = await import('react');
  const { Timeline } = await import('../../src/components/Timeline.tsx');
  const entry = await mount(createElement(Timeline));
  await flush(3);

  const scroller = entry.container.querySelector('#timelineContentScroll');
  assert.ok(scroller, '列表滚动容器应存在');
  stubScrollHeight(scroller, 500000);

  assert.equal(useAppStore.getState().entries.length, 5000, '前置：store 内有 5000 条画廊条目（全量数据入口）');

  const mounted = () => entry.container.querySelectorAll('.gallery-card').length;
  /* 通用有界性（**不依赖「卡片高度 ≥150px」前提**的加强断言，复审 P2）：
     上界 = lanes × (⌈视口高 / 卡高⌉ + 3) + 2×overscan（galleryMountBound 单点），
     卡高取夹具的 min(估算, 实测最小)——估算偏小（未挂载条目）与实测偏小
     （矮卡）两个方向都用更小的高度让上界变大，公式里不出现条目总数。
     夹具实测卡高 ≥ GALLERY_CARD_MIN_HEIGHT 时，参考口径 GALLERY_MOUNT_BOUND_REF
     对该夹具成立（参考口径是带前提的，见 GalleryLayout.ts）。 */
  const fixtureBound = () => {
    const measured = Math.min(...mountedWrappers(entry).map((w) => w.offsetHeight).filter((h) => h > 0));
    const cardH = Math.min(estimateGalleryCardHeight(scroller.offsetWidth, GALLERY_LANES_WIDE), measured);
    return { bound: galleryMountBound(scroller.offsetHeight, cardH), measured, cardH };
  };
  const assertGenericBound = (label) => {
    const { bound, measured, cardH } = fixtureBound();
    assert.ok(Number.isFinite(cardH) && cardH > 0, `${label}：夹具应能测出卡高（实测最小 ${measured}）`);
    assert.ok(measured >= GALLERY_CARD_MIN_HEIGHT,
      `${label}：夹具实测最小卡高 ${measured} 应 ≥ 参考前提 ${GALLERY_CARD_MIN_HEIGHT}（否则参考口径 48 不适用）`);
    const n = mounted();
    assert.ok(n <= bound,
      `${label}：挂载 ${n} 应 ≤ 通用上界 ${bound}（4×(⌈${scroller.offsetHeight}/${cardH}⌉+3)+2×4，公式不含条目总数）`);
  };
  /* 反例 1（先红后绿）：5000 项下同时挂载的 GalleryCard 数有上限且远小于总数。
     基线（全量 items.map）此断言以 5000 失败；有界渲染后挂载数由窗口（列数 × 可见
     条数 + overscan）决定，与总数无关。 */
  const initial5000 = mounted();
  assert.ok(initial5000 <= GALLERY_MOUNT_BOUND_REF,
    `初始挂载 ${initial5000} 应 ≤ 参考口径 ${GALLERY_MOUNT_BOUND_REF}（参考视口 × 卡高 ≥150px 前提）`);
  assert.ok(initial5000 < 5000 / 10, `初始挂载 ${initial5000} 应远小于总数 5000`);
  assert.ok(initial5000 >= GALLERY_LANES_WIDE, `初始挂载 ${initial5000} 至少应铺满一屏（四列各≥1 张）`);
  assertGenericBound('初始');

  /* 跨滚动位置扫描：窗口上界在整个列表范围内保持（不随滚动到深部而增长）。 */
  const scanMax = { value: initial5000, at: 0 };
  for (const top of [0, 30000, 90000, 180000, 270000, 340000]) {
    await scrollToOffset(act, scroller, top);
    const n = mounted();
    assert.ok(n <= GALLERY_MOUNT_BOUND_REF && n > 0,
      `scrollTop=${top} 挂载 ${n} 应落入 (0, ${GALLERY_MOUNT_BOUND_REF}]`);
    assertGenericBound(`scrollTop=${top}`);
    if (n > scanMax.value) { scanMax.value = n; scanMax.at = top; }
  }

  /* 挂载数与总数无关的直接对照：同为 image 布局、同一视口、同一初始滚动位置，
     200 项与 5000 项的挂载数应一致（等高夹具下窗口覆盖的条数只由视口与卡片高决定）。 */
  await unmount(entry);
  resetStore();
  seedFromRows(galleryRows(200, 500), { layout: 'image' });
  const entry2 = await mount(createElement(Timeline));
  await flush(3);
  const scroller2 = entry2.container.querySelector('#timelineContentScroll');
  stubScrollHeight(scroller2, 500000);
  const count200 = entry2.container.querySelectorAll('.gallery-card').length;
  assert.equal(count200, initial5000,
    `200 项的挂载数（${count200}）应与 5000 项初始（${initial5000}）一致——不随总数线性增长`);
  assert.ok(count200 <= GALLERY_MOUNT_BOUND_REF, `200 项挂载 ${count200} 应 ≤ 参考口径下界`);
});

componentTest('OPT-017C 画廊键盘焦点：跨窗口滚动后仍有可 Tab 卡；方向键相邻移动不跳卡、不丢焦点', { timeout: 60000 }, async () => {
  installBackend();
  resetStore();
  seedFromRows(galleryRows(600, 20000), { layout: 'image' });

  const { createElement, act } = await import('react');
  const { Timeline } = await import('../../src/components/Timeline.tsx');
  const entry = await mount(createElement(Timeline));
  await flush(3);

  const scroller = entry.container.querySelector('#timelineContentScroll');
  stubScrollHeight(scroller, 200000);

  /* 滚到深部（首窗已卸载：tabbable 焦点卡 index 0 不在窗口内）。 */
  await scrollToOffset(act, scroller, 80000);
  const wrappers = mountedWrappers(entry);
  assert.ok(wrappers.length > 0, '深部滚动后应有挂载窗口');
  const firstIndex = wrapperIndex(wrappers[0]);
  assert.ok(firstIndex > 0, `深部滚动后窗口首卡下标 ${firstIndex} 应 > 0（首窗确已卸载）`);

  /* 反例 2a：跨窗口滚动后 Tab 焦点仍可达——roving 兜底：窗口内恰有一张 tabIndex=0。 */
  const tabbables = wrappers
    .map((w) => focusableOf(w))
    .filter((el) => el && el.getAttribute('tabindex') === '0');
  assert.equal(tabbables.length, 1,
    `窗口内应恰有 1 张可 Tab 卡片（实际 ${tabbables.length}）——焦点卡不在窗口时兜底到窗口首卡`);
  assert.equal(Number(tabbables[0].getAttribute('data-card-index')), firstIndex,
    '兜底可 Tab 卡应为窗口首卡（保证 Tab 进入的是当前能看到的区域）');

  /* 反例 2b：方向键从窗口首卡向下移动 → 精确到相邻卡（index + 1），焦点不丢。 */
  const startEl = focusableOf(wrappers[0]);
  startEl.focus();
  assert.equal(document.activeElement, startEl, '前置：焦点落在窗口首卡');

  const pressArrow = async (key) => {
    /* 事件派发在**当前聚焦元素**上（roving：方向键语义属于持焦卡片本身；
       在旧元素上派发会按旧卡下标移动，不是产品路径）。 */
    const target = document.activeElement ?? startEl;
    await act(async () => {
      target.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true }));
    });
    /* 程序性 scrollToIndex 在真实浏览器会引发 scroll 事件，jsdom 手动补齐；
       随后等待 rAF 聚焦回调执行。 */
    await act(async () => { scroller.dispatchEvent(new Event('scroll')); });
    await flush(4, 30);
  };

  await pressArrow('ArrowDown');
  const afterDown = document.activeElement;
  assert.ok(afterDown && afterDown !== document.body, 'ArrowDown 后焦点不应丢失到 body');
  assert.equal(Number(afterDown.getAttribute('data-card-index')), firstIndex + 1,
    `ArrowDown 应停在相邻下一卡（${firstIndex + 1}），实际 ${afterDown?.getAttribute?.('data-card-index')}`);
  assert.ok(document.contains(afterDown), 'ArrowDown 后焦点元素应仍在挂载树内（目标卡已入窗）');

  await pressArrow('ArrowUp');
  const afterUp = document.activeElement;
  assert.equal(Number(afterUp.getAttribute('data-card-index')), firstIndex,
    `ArrowUp 应回到原卡（${firstIndex}），实际 ${afterUp?.getAttribute?.('data-card-index')}`);
});

componentTest('OPT-017C 画廊灯箱：打开→关闭后滚动位置不动、焦点回到原卡（跨窗口滚动后成立）', { timeout: 60000 }, async () => {
  installBackend();
  resetStore();
  seedFromRows(galleryRows(600, 30000), { layout: 'image' });

  const { createElement, act } = await import('react');
  const { Timeline } = await import('../../src/components/Timeline.tsx');
  const { Lightbox } = await import('../../src/components/Overlays.tsx');
  const entry = await mount(createElement('div', null, createElement(Timeline), createElement(Lightbox)));
  await flush(3);

  const scroller = entry.container.querySelector('#timelineContentScroll');
  stubScrollHeight(scroller, 200000);

  /* 跨窗口滚动：到深部后取窗口内的一张卡打开灯箱。 */
  await scrollToOffset(act, scroller, 90000);
  const wrappers = mountedWrappers(entry);
  assert.ok(wrappers.length > 0, '深部滚动后应有挂载窗口');
  const wrap = wrappers[1] ?? wrappers[0];
  const targetIdx = wrapperIndex(wrap);
  const targetId = wrap.querySelector('.gallery-card').getAttribute('data-id');
  const img = focusableOf(wrap);
  img.focus();

  const scrollBefore = scroller.scrollTop;
  await act(async () => {
    img.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  });
  await flush(3);

  /* 打开：灯箱带条目 id（画廊语义——失败可按条目上报），滚动位置不动。 */
  assert.ok(useAppStore.getState().lightboxUrl, 'Enter 应打开灯箱（真实 GalleryCard openImage 路径）');
  assert.equal(useAppStore.getState().lightboxEntryId, targetId, '灯箱应携带被点条目的 id');
  assert.equal(scroller.scrollTop, scrollBefore, '打开灯箱不应改变滚动位置');
  /* 标读后原卡仍在窗口（openedReadIds 保留语义）——焦点归还的前提。 */
  assert.equal(mountedWrappers(entry).some((w) => wrapperIndex(w) === targetIdx), true,
    '打开灯箱（标已读）后原卡应仍在挂载窗口内');

  /* 关闭：真实关闭路径（点击遮罩）→ 焦点回到原卡、滚动不动。 */
  const overlay = entry.container.querySelector('.lightbox-overlay');
  assert.ok(overlay, '灯箱遮罩应存在');
  await act(async () => { overlay.click(); });
  await flush(3);

  assert.equal(useAppStore.getState().lightboxUrl, null, '点击遮罩应关闭灯箱');
  assert.equal(document.activeElement, img, '关闭后焦点应回到原卡的可聚焦元素');
  assert.equal(scroller.scrollTop, scrollBefore, '关闭灯箱不应改变滚动位置');
});

componentTest('OPT-017C 画廊加载更多：追加批次可渲染可交互、窗口下标不错位、无重复挂载', { timeout: 60000 }, async () => {
  installBackend();
  /* 本用例覆写 list_articles 的分页行为（keyset 续页）——先复位再覆写，
     结束（含断言失败路径）必须还原，避免污染后续 App 级用例的启动装载。 */
  resetPlan();
  resetStore();

  const pool = galleryRows(120, 40000); // 40000..40119，newest-first 下 index 0 = 40000
  const firstBatch = pool.slice(0, 60);
  seedFromRows(firstBatch, { layout: 'image' });
  setCurrentRows(pool);
  /* 假后端按 keyset 续页（与后端 ORDER BY published_at, id 的 newest 口径一致）。 */
  setPlan({
    list_articles: (args) => {
      const a = args?.args ?? {};
      const sorted = [...pool].filter((r) => r.feed_id === IMAGE_FEED)
        .sort((x, y) => (Date.parse(y.published_at) - Date.parse(x.published_at)) || (y.id - x.id));
      const rest = a.last_published != null && a.last_id != null
        ? sorted.filter((r) => r.published_at < a.last_published
          || (r.published_at === a.last_published && r.id < Number(a.last_id)))
        : sorted;
      return Promise.resolve(rest.slice(0, a.limit ?? rest.length));
    },
  });

  try {
    const { createElement, act } = await import('react');
    const { Timeline } = await import('../../src/components/Timeline.tsx');
    const entry = await mount(createElement(Timeline));
    await flush(3);

    const scroller = entry.container.querySelector('#timelineContentScroll');
    stubScrollHeight(scroller, 200000);
    /* 可滚动测量就绪后再放行续拉：挂载期容器不可滚动（scrollHeight 未替身）时
       自动续拉会先拉一页，使「首批 60 → 触底追加 120」的判别窗口消失。 */
    const anchorRow = firstBatch[firstBatch.length - 1];
    await act(async () => {
      useAppStore.setState({
        articlesExhausted: false,
        articlesLimit: 60,
        articlesCursor: {
          'image|all': { loaded: 60, lastPublished: anchorRow.published_at, lastId: anchorRow.id },
        },
      });
    });
    await flush(2);
    assert.equal(useAppStore.getState().entries.length, 60, '前置：首批 60 条（触底前无额外续拉）');

    /* 触底：进入「距底 600px」触发区 → 真实 loadMoreArticles 追加第二批。 */
    await scrollToOffset(act, scroller, 199500);
    await waitFor(() => useAppStore.getState().entries.length === 120,
      { label: '触底后追加第二批（60 → 120）', timeout: 8000 });
    assert.equal(useAppStore.getState().articlesExhausted, true, '第二批不足一页 → 已到底');

    /* 滚到追加区：新项真实渲染、下标与 store 一致、可交互（Enter 打开灯箱）。 */
    const items = useAppStore.getState().entries;
    assert.equal(items[119].id, '40119', '末条应来自追加批次（40119）');
    await scrollToOffset(act, scroller, 199500);
    const wrappers = mountedWrappers(entry);
    assert.ok(wrappers.length > 0, '追加后窗口应有挂载');
    /* 窗口边界不错位：每个挂载 wrapper 的 data-index 与其卡片 data-id 都和 store items 同下标。 */
    for (const w of wrappers) {
      const idx = wrapperIndex(w);
      const card = w.querySelector('.gallery-card');
      assert.ok(Number.isInteger(idx) && idx >= 0 && idx < items.length, `wrapper data-index ${idx} 应在 [0, ${items.length})`);
      assert.equal(card.getAttribute('data-id'), items[idx].id,
        `下标 ${idx} 的卡片 data-id 应与 store items[${idx}] 一致（窗口边界不错位）`);
    }
    /* 不重复渲染：挂载 id 集合无重复。 */
    const ids = wrappers.map((w) => w.querySelector('.gallery-card').getAttribute('data-id'));
    assert.equal(new Set(ids).size, ids.length, `挂载 id 不应重复（实际 ${ids.join(',')}）`);

    /* 新项可交互：聚焦追加批次中的卡，Enter 打开灯箱。 */
    const lastWrap = wrappers.find((w) => wrapperIndex(w) === 119) ?? wrappers[wrappers.length - 1];
    const lastIdx = wrapperIndex(lastWrap);
    const lastImg = focusableOf(lastWrap);
    lastImg.focus();
    await act(async () => {
      lastImg.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    });
    await flush(3);
    assert.equal(useAppStore.getState().lightboxEntryId, items[lastIdx].id,
      `追加批次的卡可交互：Enter 打开灯箱应携带 ${items[lastIdx].id}`);
    useAppStore.getState().closeLightbox();

    /* 已到底文案保持（哨兵形态迁移：追加中 → 没有更多了）。 */
    assert.ok(entry.container.textContent.includes('没有更多了'), '已到底后应显示「没有更多了」');
  } finally {
    resetPlan();
  }
});

componentTest('OPT-017C 画廊列布局：列分配与列宽/列距公式一致、同列同左缘、纵向不重叠', { timeout: 60000 }, async () => {
  installBackend();
  resetStore();
  seedFromRows(galleryRows(300, 50000), { layout: 'image' });

  const { createElement, act } = await import('react');
  const { Timeline } = await import('../../src/components/Timeline.tsx');
  const entry = await mount(createElement(Timeline));
  await flush(3);

  const scroller = entry.container.querySelector('#timelineContentScroll');
  stubScrollHeight(scroller, 100000);
  await scrollToOffset(act, scroller, 0);

  const lanes = GALLERY_LANES_WIDE; // harness 的 matchMedia 替身恒不命中 → 宽视口 4 列
  const wrappers = mountedWrappers(entry);
  assert.ok(wrappers.length >= lanes * 2, `首屏应挂载至少两轮（实际 ${wrappers.length}）`);

  /* 列分配断言（等高夹具 → 贪心最短列退化为轮转）：index % 4 = lane。
     同时钉住列宽/列距公式（与旧 CSS 多列 (W − (n−1)·gap) / n 等价）。 */
  const expectedWidth = canonCssValue('width', galleryItemPlacement(0, lanes).width);
  const byLane = new Map();
  for (const w of wrappers) {
    const idx = wrapperIndex(w);
    const lane = Number(w.dataset.galleryLane);
    assert.equal(lane, idx % lanes, `下标 ${idx} 应分配到列 ${idx % lanes}（实际 ${lane}）`);
    assert.equal(w.style.width, expectedWidth,
      `下标 ${idx} 的列宽应为 (100% − ${(lanes - 1) * GALLERY_COLUMN_GAP}px) / ${lanes}`);
    const expectedLeft = canonCssValue('left', galleryItemPlacement(lane, lanes).left);
    assert.equal(w.style.left, expectedLeft, `下标 ${idx}（列 ${lane}）的左缘应与列公式一致`);
    if (!byLane.has(lane)) byLane.set(lane, []);
    byLane.get(lane).push(w);
  }
  assert.equal(byLane.size, lanes, `首屏应覆盖全部 ${lanes} 列（实际 ${byLane.size}）`);

  /* 同列内：左缘完全一致，纵向 top（translateY）严格递增且间隔 = 卡片高 + gap（不重叠）。 */
  const ys = (w) => Number((w.style.transform.match(/translateY\(([-0-9.]+)px\)/) ?? [])[1]);
  for (const [lane, list] of byLane) {
    for (let i = 1; i < list.length; i++) {
      assert.equal(list[i].style.left, list[i - 1].style.left, `列 ${lane} 内左缘应一致`);
      assert.ok(ys(list[i]) > ys(list[i - 1]),
        `列 ${lane} 内条目应按序向下排列（${ys(list[i - 1])} → ${ys(list[i])}）`);
    }
  }

  /* 图片比例保持：卡片 img 不设固定尺寸/裁切（自然比例由图片自身决定，宽度走 CSS 100%）。 */
  const imgs = [...entry.container.querySelectorAll('.gallery-card img')];
  assert.ok(imgs.length > 0, '画廊卡应渲染真实 img');
  for (const img of imgs) {
    assert.equal(img.getAttribute('height'), null, '画廊 img 不应带 height 属性（固定高会破坏比例）');
    assert.equal(img.style.height, '', '画廊 img 不应内联 height（比例由自然尺寸决定）');
    assert.equal(img.style.objectFit, '', '画廊 img 不应内联 object-fit（裁切会破坏比例）');
  }

  /* 网格容器高度 = 虚拟化总高（有界窗口的滚动替身），而不是全量卡片高度之和。 */
  const grid = entry.container.querySelector('.gallery-masonry-grid');
  const gridHeight = Number((grid.style.height.match(/^([0-9.]+)px$/) ?? [])[1]);
  assert.ok(Number.isFinite(gridHeight) && gridHeight > 0, `网格容器应有确定高度（实际 ${grid.style.height}）`);
  assert.ok(gridHeight < 300 * (260 + GALLERY_COLUMN_GAP),
    '网格高度应为虚拟化总高（估算/测量），不是全量条目的朴素叠加');

  /* 空态文案保持（条目清空 → 无卡片、网格归零、空态文案照旧）。 */
  await act(async () => { useAppStore.setState({ entries: [] }); });
  await flush(2);
  assert.equal(entry.container.querySelectorAll('.gallery-card').length, 0, '空集合不应残留挂载卡片');
  assert.equal(entry.container.querySelector('.gallery-masonry-grid')?.style.height, '0px',
    '空集合时网格高度应为 0');
  assert.ok(entry.container.textContent.includes('暂无匹配内容'), '空态文案应照旧显示');
});
