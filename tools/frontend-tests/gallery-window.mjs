// tools/frontend-tests/gallery-window.mjs
// 领域模块：画廊（瀑布流）列模型窗口与图片布局状态迁移 —— OPT-017C。
//
// 证据分层（与其他域同口径，如实说明）：
// - 纯函数真值表：列数断点解析、估算公式、列放置公式、roving tabindex 窗口兜底
//   （GalleryLayout.ts 是 Timeline 与回归网的同一份实现，不复制第二套）；
// - store 层行为：图片布局下的「打开灯箱即标读 + 未读筛选原地保留」「关闭清空」
//   「触底续拉追加/游标推进/已到底」状态迁移（真实动作序列 + 假后端 keyset 分页）；
// - 接线（源级反回退）：Timeline 经 lanes 虚拟化渲染窗口、DOM 挂载映射；base.css
//   不再用 CSS 多列（column-count）且画廊图片保持自然比例（width:100%、无裁切）。
// 未测边界：真实 WebView 的滚动/布局/解码性能（jsdom 无布局引擎），见
// docs/performance.md「画廊布局有界渲染」节与组件回归 gallery-virtual.test.mjs。
export const id = 'gallery-window';

export async function run(ctx) {
  const { S, store, checkNew, nTick, resetStore, NOW, iso, mkRow, selectVisibleEntries } = ctx;
  await ctx.useMainBackend();

  const {
    GALLERY_COLUMN_GAP,
    GALLERY_LANES_WIDE,
    GALLERY_OVERSCAN,
    GALLERY_CARD_ESTIMATE_FALLBACK,
    GALLERY_CARD_MIN_HEIGHT,
    GALLERY_MOUNT_BOUND_REF,
    GALLERY_REF_VIEWPORT_HEIGHT,
    resolveGalleryLaneCount,
    estimateGalleryCardHeight,
    galleryItemPlacement,
    galleryMountBound,
    galleryTabbableIndex,
  } = await import('../../src/components/GalleryLayout.ts');
  const { sentinelMode } = await import('../../src/components/timelineSentinel.ts');

  /* ============================================================
     (w1) 列数断点解析：与旧 CSS 媒体查询（≤800→2 / ≤1200→3 / 否则 4）逐字一致
     ============================================================ */
  const mmHost = ({ narrow = false, mid = false } = {}) => ({
    matchMedia: (q) => ({ matches: q.includes('800px') ? narrow : mid }),
  });
  checkNew('(w1) 列数断点与旧 CSS 媒体查询一致：≤800→2、≤1200→3、宽视口→4；无 matchMedia（SSR/替身缺省）回落宽视口',
    resolveGalleryLaneCount(mmHost({ narrow: true, mid: true })) === 2
    && resolveGalleryLaneCount(mmHost({ mid: true })) === 3
    && resolveGalleryLaneCount(mmHost()) === GALLERY_LANES_WIDE
    && resolveGalleryLaneCount(undefined) === GALLERY_LANES_WIDE
    && resolveGalleryLaneCount({}) === GALLERY_LANES_WIDE);

  /* ============================================================
     (w2) 估算公式与窗口上界常量（上界推导见 GalleryLayout.ts 头注）
     ============================================================ */
  checkNew('(w2) 估算高度 = 列宽 × 0.72 + 64（下限 200）；宽度未知回落常量；列数越小（列越宽）估算越高',
    estimateGalleryCardHeight(1024, 4) === Math.round(((1024 - 3 * GALLERY_COLUMN_GAP) / 4) * 0.72 + 64)
    && estimateGalleryCardHeight(1024, 4) === 241
    && estimateGalleryCardHeight(0, 4) === GALLERY_CARD_ESTIMATE_FALLBACK
    && estimateGalleryCardHeight(240, 4) === 200
    && estimateGalleryCardHeight(1024, 2) > estimateGalleryCardHeight(1024, 4));
  checkNew(`(w2) 窗口上界自洽（单一真值源 = galleryMountBound × GALLERY_CARD_MIN_HEIGHT）：参考口径(${GALLERY_REF_VIEWPORT_HEIGHT}px × ${GALLERY_CARD_MIN_HEIGHT}px) = 44、参考常量 ${GALLERY_MOUNT_BOUND_REF} ≥ 44；前提不成立/视口更大时通用式单调变大（上界随视口与卡高、与条目总数无关）`,
    galleryMountBound(GALLERY_REF_VIEWPORT_HEIGHT, GALLERY_CARD_MIN_HEIGHT) === 44
    && GALLERY_MOUNT_BOUND_REF === 48
    && GALLERY_MOUNT_BOUND_REF >= galleryMountBound(GALLERY_REF_VIEWPORT_HEIGHT, GALLERY_CARD_MIN_HEIGHT)
    && galleryMountBound(GALLERY_REF_VIEWPORT_HEIGHT, 100) > GALLERY_MOUNT_BOUND_REF /* 卡更矮（lazy/极扁图）→ 窗口变宽方向 */
    && galleryMountBound(2160, GALLERY_CARD_MIN_HEIGHT) > GALLERY_MOUNT_BOUND_REF /* 视口更高 → 窗口变宽方向 */
    && galleryMountBound(GALLERY_REF_VIEWPORT_HEIGHT, GALLERY_CARD_MIN_HEIGHT, GALLERY_LANES_WIDE, GALLERY_OVERSCAN) < 5000 / 10);

  /* ============================================================
     (w3) 列放置公式：列宽 =(W−(n−1)×gap)/n，左缘 = 列宽×lane + lane×gap
     （与旧 CSS 多列 column-width/column-gap 的等价式）
     ============================================================ */
  const p4 = galleryItemPlacement(0, 4);
  const p4l3 = galleryItemPlacement(3, 4);
  const p2 = galleryItemPlacement(1, 2);
  checkNew('(w3) 列放置 calc 公式与旧 CSS 多列等价：4 列 → (100%−42px)/4；lane=3 左缘 = 列宽×3+42px；2 列 → (100%−14px)/2 + 14px',
    p4.width === 'calc((100% - 42px) / 4)' && p4.left === 'calc((100% - 42px) / 4 * 0 + 0px)'
    && p4l3.left === 'calc((100% - 42px) / 4 * 3 + 42px)'
    && p2.width === 'calc((100% - 14px) / 2)' && p2.left === 'calc((100% - 14px) / 2 * 1 + 14px)');

  /* ============================================================
     (w4) roving tabindex 窗口兜底真值表（任何滚动位置下恰有一张卡可 Tab）
     ============================================================ */
  checkNew('(w4) tabbable 兜底：焦点卡在窗口内→原样；不在→窗口首卡；无挂载→-1；tabbable 越界(-1)→窗口首卡',
    galleryTabbableIndex([10, 11, 12], 11) === 11
    && galleryTabbableIndex([10, 11, 12], 5) === 10
    && galleryTabbableIndex([10, 11, 12], 999) === 10
    && galleryTabbableIndex([10, 11, 12], -1) === 10
    && galleryTabbableIndex([], 3) === -1);

  /* ============================================================
     (w5) 图片布局 store 状态迁移：打开灯箱（即标读 + 未读筛选原地保留）/ 关闭清空
     ============================================================ */
  await resetStore();
  const imgRows = Array.from({ length: 8 }, (_, i) => mkRow({
    id: 9100 + i, feed_id: 11, title: `画廊窗口 ${i}`, image_url: `https://img.example/${i}.png`,
    published_at: iso(NOW - i * 60000),
  }));
  S.backendRows = imgRows;
  await store.getState().bootstrapFromBackend();
  await store.getState().updateFeedLayout('cat-1', '11', 'image'); // feedIndex: 源B → image（真实 action）
  store.setState({ activeContentLayout: 'image', timelineFilter: 'unread', activeViewFilter: 'all' });
  const visibleImage = selectVisibleEntries(store.getState());
  checkNew('(w5) image 布局可见集合 = 绑定 image 的源条目（布局是派生结果：同一个源改绑定后条目即时迁移）',
    visibleImage.length === 8 && visibleImage.every((e) => e.feedId === '11'));

  const target = visibleImage[0];
  store.getState().markEntriesReadBulk([target.id]); // GalleryCard.openImage 的标读路径
  const afterRead = store.getState();
  checkNew('(w5) 画廊打开灯箱的标读路径：条目转已读且「打开过」保留——未读筛选下原地变灰不消失（焦点可归还前提）',
    afterRead.entries.find((e) => e.id === target.id)?.isRead === true
    && afterRead.openedReadIds[target.id] === true
    && selectVisibleEntries(afterRead).some((e) => e.id === target.id));
  store.getState().openLightbox('https://img.example/0.png', target.id);
  checkNew('(w5) 灯箱打开携带条目 id（画廊语义：失败可按条目上报）',
    store.getState().lightboxUrl === 'https://img.example/0.png' && store.getState().lightboxEntryId === target.id);
  store.getState().closeLightbox();
  checkNew('(w5) 灯箱关闭：url 与条目 id 一起清空（关闭返回原卡的焦点归还依据在此）',
    store.getState().lightboxUrl === null && store.getState().lightboxEntryId === null);

  /* ============================================================
     (w6) 图片布局触底加载更多：追加批次 / 游标推进 / 已到底 / 哨兵形态迁移
     ============================================================ */
  await resetStore();
  const manyRows = Array.from({ length: 503 }, (_, i) => mkRow({
    id: 9200 + i, feed_id: 11, title: `画廊分页 ${i}`, image_url: `https://img.example/p${i}.png`,
    published_at: iso(NOW - i * 60000),
  }));
  S.backendRows = manyRows;
  await store.getState().bootstrapFromBackend(); // 建立 categories/feedIndex（updateFeedLayout 的前置）
  await store.getState().updateFeedLayout('cat-1', '11', 'image');
  store.setState({ activeContentLayout: 'image' });
  await store.getState().bootstrapFromBackend(); // 以 image 口径重装首屏：游标键 image|all（窗口模型输入）
  const w6First = store.getState();
  checkNew('(w6) 图片布局首批 500 行：游标键 image|all 推进到 500、未到底（窗口模型输入 = 该集合）',
    w6First.entries.length === 500 && w6First.articlesExhausted === false
    && w6First.articlesCursor['image|all']?.loaded === 500);
  S.invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  const w6More = store.getState();
  const w6Call = S.invokeCalls.find((c) => c.cmd === 'list_articles');
  checkNew('(w6) 触底续拉：keyset 锚（最后一行 9699，无 offset）追加 3 行 → 503；追加批次在集合内可索引（窗口边界不错位的输入侧）',
    w6Call?.args.args.last_id === 9699 && w6Call?.args.args.offset === undefined
    && w6Call?.args.args.layout === 'image'
    && w6More.entries.length === 503 && w6More.entries[500].id === '9700' && w6More.entries[502].id === '9702'
    && new Set(w6More.entries.map((e) => e.id)).size === 503);
  checkNew('(w6) 不足一页 → 已到底；哨兵形态迁移 loading → end（「没有更多了」的判定单点）',
    w6More.articlesExhausted === true && w6More.articlesLoading === false
    && sentinelMode(w6More.entries.length, w6More.articlesExhausted, w6More.articlesLoading) === 'end'
    && sentinelMode(500, false, true) === 'loading'
    && sentinelMode(500, false, false) === 'idle'
    && sentinelMode(0, true, false) === 'hidden');
  S.invokeCalls.length = 0;
  await store.getState().loadMoreArticles();
  checkNew('(w6) 已到底后重复触底：零新 IPC、集合不变（追加批次不重复渲染的输入侧保证）',
    S.invokeCalls.length === 0 && store.getState().entries.length === 503);

  // 复原被改动的共享夹具（源B 布局回 social），避免残留给后续域
  await store.getState().updateFeedLayout('cat-1', '11', 'social');
  await nTick(0);

  /* ============================================================
     (w7) 接线（源级反回退）：Timeline 经 lanes 虚拟化渲染窗口；CSS 去多列、保比例
     ============================================================ */
  const fsW = await import('node:fs');
  const timelineW = fsW.readFileSync(new URL('../../src/components/Timeline.tsx', import.meta.url), 'utf8');
  const cssW = fsW.readFileSync(new URL('../../src/styles/base.css', import.meta.url), 'utf8');
  const galleryBlock = timelineW.slice(
    timelineW.indexOf("{/* 画廊（瀑布流多列）：lanes 列模型 + 垂直虚拟化"),
    timelineW.indexOf('{/* 其余 4 种单列布局'),
  );
  checkNew('(w7) Timeline 画廊分支：经 lanes 虚拟化实例渲染窗口（galleryVirtualItems.map + measureElement + lane 定位），不再 items.map 全量',
    galleryBlock.length > 0
    && galleryBlock.includes('galleryVirtualItems.map((vi) =>')
    && galleryBlock.includes('ref={galleryVirtualizer.measureElement}')
    && galleryBlock.includes('data-gallery-lane={vi.lane}')
    && galleryBlock.includes('galleryItemPlacement(vi.lane, galleryLanes)')
    && !galleryBlock.includes('items.map(')
    && timelineW.includes('lanes: galleryLanes')
    && timelineW.includes('useFlushSync: false'));
  checkNew('(w7) roving 焦点消费窗口兜底判定（galleryTabbableIndex 单点，不再直接 idx === tabbableIndex 全量口径）',
    timelineW.includes('galleryTabbableIndex(galleryVirtualItems.map((vi) => vi.index), tabbableIndex)'));
  const galleryCss = cssW.slice(cssW.indexOf('.gallery-masonry-grid'), cssW.indexOf('.gallery-meta {'));
  checkNew('(w7) CSS：画廊不再用 CSS 多列（column-count 清零）、卡片定位交给 .gallery-virtual-item；图片宽度 100% 且不裁切（自然比例保持）',
    !galleryCss.includes('column-count') && !galleryCss.includes('break-inside')
    && galleryCss.includes('.gallery-virtual-item') && galleryCss.includes('position: absolute')
    && cssW.includes('.gallery-card img {')
    && /\.gallery-card img \{[^}]*width: 100%/.test(cssW)
    && !/\.gallery-card img \{[^}]*object-fit/.test(cssW));
}
