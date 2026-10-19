// Note: 画廊有界渲染（列模型 + 垂直虚拟化）的窗口上界与取舍 — 见 .agents/notes/implemented/feature/2026-10-16-画廊有界渲染.md
/* ============================================================
   画廊（瀑布流）列模型的布局参数与纯函数（OPT-017C）

   背景：画廊此前用 CSS 多列（column-count 4/3/2 + break-inside: avoid）全量渲染
   items.map——DOM 随条目线性增长（5000 项 = 5000 张卡）。OPT-017C 改为「列模型 +
   垂直虚拟化」：列数由本模块按与旧 CSS 媒体查询一致的断点解析（浏览器不再分列），
   窗口内条目由 @tanstack/react-virtual 的 lanes 能力按贪心最短列分配并入窗。

   为什么仍用整数列数常量而不是容器宽度算列：旧观感的列数断点发生在**视口**宽度
   （base.css 媒体查询），容器宽度会额外受侧栏/阅读器列影响；用 matchMedia 对齐
   视口断点才能保持「窗口变窄 → 列数变少」的原观感（视觉列宽仍由容器宽度派生）。

   估算与真实高度的关系：estimateGalleryCardHeight 只服务**未挂载**条目的窗口定位
   与滚动总高；挂载后的真实高度由 measureElement（offsetHeight/ResizeObserver）
   修正。估偏方向的安全性由截断窗口上界 + overscan 缓冲承担（见
   galleryMountBound / GALLERY_MOUNT_BOUND_REF 推导）。

   窗口上界（对交付「最大同时挂载卡片数」的回答）：
   - 渲染集 = 虚拟化可见范围（lanes × 每列可见条数）+ 两端 overscan，
     每列可见条数由 视口高 / 卡片高 决定，与条目总数无关；
   - **通用式**（任何视口高/卡片高都成立，galleryMountBound 单点实现）：
     lanes × (⌈视口高 / 卡片高度⌉ + 3 行边界余量) + 2×overscan；
   - **参考口径上界** GALLERY_MOUNT_BOUND_REF = 48：参考视口（1024×768、四列）
     × 卡片高度 ≥ GALLERY_CARD_MIN_HEIGHT（150px）前提下通用式 = 44，取整到 48。
     前提不成立时（lazy 未加载、极扁图片 → 实测高 < 150）窗口会按通用式变宽——
     48 是**带前提的参考口径**，不是无前提的绝对上界；真实卡片通常 ≥200px，且
     模型层不强制高度下界（保持旧观感的自然比例，见归属 Note）。
   - 组件回归同时断言两条：夹具内的**通用式**（用实测卡高与视口高现算）与
     参考口径 48；另有 200 项 vs 5000 项挂载数相等（不随总数增长）。测量方法见
     docs/performance.md「画廊布局有界渲染」节。

   列分配与旧 CSS 多列的行为差异（如实记录）：CSS 多列按列优先填充并做高度平衡，
   lanes 是「填满一轮后放最短列」的贪心（行优先起步）——同一组条目在两套模型下的
   列归属不同，但列宽/列距/图片自然比例/错落观感一致；有界渲染无法对未挂载内容做
   浏览器列平衡，这是本卡裁决接受的取舍（见归属 Note）。
   ============================================================ */

/** 列间距（px）。与旧 CSS column-gap / .gallery-card margin-bottom 同值：
    列宽 = (容器宽 − (列数−1)×gap) / 列数，保持旧观感。 */
export const GALLERY_COLUMN_GAP = 14;

/** 列数：宽视口 / 中视口 / 窄视口——断点与旧 base.css 媒体查询逐字一致（≤800 → 2，≤1200 → 3）。 */
export const GALLERY_LANES_WIDE = 4;
export const GALLERY_LANES_MID = 3;
export const GALLERY_LANES_NARROW = 2;

/** react-virtual 的 overscan（以条目计，非列）：两端各多渲染一条，吸收估算误差与
    快速滚动。四列下一轮 = 4 条，取 4 = 一轮的缓冲。 */
export const GALLERY_OVERSCAN = 4;

/** 估算卡片高度下限（px）：估算函数的 floor，同时是窗口上界推导的分母口径。 */
export const GALLERY_CARD_ESTIMATE_FLOOR = 200;

/** 容器宽度未知（首帧）时的估算回落（px）。 */
export const GALLERY_CARD_ESTIMATE_FALLBACK = 240;

/** 参考视口高度（px）：窗口上界参考口径的视口分量（1024×768 桌面窗口）。 */
export const GALLERY_REF_VIEWPORT_HEIGHT = 768;

/** 卡片高度工程下界（px）：无图占位 min-height 120 + 元信息 ≈ 60 的下沿。
    只作**参考口径的前提**（GALLERY_MOUNT_BOUND_REF 的推导分母）；模型层不强制
    该下界（lazy 未加载、极扁图片的实测高可更小 → 窗口按通用式变宽，方向性见
    galleryMountBound）。 */
export const GALLERY_CARD_MIN_HEIGHT = 150;

/** 窗口上界**通用式**（任何视口高/卡片高都成立，与条目总数无关）：
    lanes × (⌈viewportHeight / cardHeight⌉ + 3 行边界余量) + 2 × overscan。
    3 = 跨视口上/下边界的行 + 贪心列错落再留 1 行；overscan 按条目计。
    这是「有界渲染」的可验证含义：上界随视口高与卡片高表达，条目总数不出现。 */
export function galleryMountBound(
  viewportHeight: number,
  cardHeight: number,
  lanes: number = GALLERY_LANES_WIDE,
  overscan: number = GALLERY_OVERSCAN,
): number {
  const rows = Math.ceil(viewportHeight / Math.max(1, cardHeight)) + 3;
  return lanes * rows + 2 * overscan;
}

/** **参考口径上界**（带前提）：参考视口 GALLERY_REF_VIEWPORT_HEIGHT(768px) ×
    卡片高度 ≥ GALLERY_CARD_MIN_HEIGHT(150px) 时，通用式
    galleryMountBound(768, 150) = 4×(⌈768/150⌉+3)+8 = 44，取整到 48 作断言口径
    （≥ 夹具实测最大值 32，测量高 260px）。
    前提不成立（卡片更矮）时通用式会给出更大的值——48 不是无前提绝对上界；
    域回归同时断言 galleryMountBound(768, 100) > 48 的方向性。 */
export const GALLERY_MOUNT_BOUND_REF = 48;

/** 与 matchMedia 结构兼容的最小宿主（浏览器 / 回归夹具 / Node 无 window 均可调用）。 */
export interface GalleryMatchMediaHost {
  matchMedia?: (query: string) => { matches: boolean };
}

/** 解析画廊列数：与旧 CSS 媒体查询同断点（≤800 → 2 列，≤1200 → 3 列，否则 4 列）。
    无 matchMedia（SSR / 测试替身缺省）→ 宽视口 4 列。 */
export function resolveGalleryLaneCount(
  host: GalleryMatchMediaHost | undefined = typeof window === 'undefined' ? undefined : window,
): number {
  if (!host || typeof host.matchMedia !== 'function') return GALLERY_LANES_WIDE;
  if (host.matchMedia('(max-width: 800px)').matches) return GALLERY_LANES_NARROW;
  if (host.matchMedia('(max-width: 1200px)').matches) return GALLERY_LANES_MID;
  return GALLERY_LANES_WIDE;
}

/** 估算未挂载卡片高度：图片按 ≈4:3 视窗（列宽 × 0.72）+ 元信息两行 ≈ 64px。
    只影响窗口定位与总高；挂载后的真实高度由测量修正。宽度未知时回落常量。 */
export function estimateGalleryCardHeight(containerWidth: number, lanes: number): number {
  if (!(containerWidth > 0) || !(lanes > 0)) return GALLERY_CARD_ESTIMATE_FALLBACK;
  const colWidth = (containerWidth - (lanes - 1) * GALLERY_COLUMN_GAP) / lanes;
  return Math.max(GALLERY_CARD_ESTIMATE_FLOOR, Math.round(colWidth * 0.72 + 64));
}

/** 条目在网格内的水平定位（CSS calc 字符串，四列示例 lane=1：
    left = ((100% − 42px) / 4) × 1 + 14px，width = (100% − 42px) / 4）——
    与旧 CSS 多列的列宽公式逐字等价（列宽 = (W − (n−1)·gap) / n）。 */
export function galleryItemPlacement(lane: number, lanes: number): { left: string; width: string } {
  const safeLanes = Math.max(1, lanes);
  const colWidth = `(100% - ${(safeLanes - 1) * GALLERY_COLUMN_GAP}px) / ${safeLanes}`;
  return {
    width: `calc(${colWidth})`,
    left: `calc(${colWidth} * ${lane} + ${lane * GALLERY_COLUMN_GAP}px)`,
  };
}

/* roving tabindex 的窗口内回落：成组卡片只保留一张 tabIndex=0，否则 Tab 会逐张
   走过全部卡片。虚拟化下焦点卡可能不在窗口内（用户用鼠标滚到别处后按 Tab）——
   此时回落给窗口首卡，保证「任何滚动位置下 Tab 都能进入列表」；焦点卡在窗口内
   则保持原语义（回落到它）。窗口空（无挂载）→ -1（无卡可 Tab）。 */
export function galleryTabbableIndex(mountedIndexes: readonly number[], tabbableIndex: number): number {
  if (mountedIndexes.length === 0) return -1;
  if (tabbableIndex >= 0 && mountedIndexes.includes(tabbableIndex)) return tabbableIndex;
  return mountedIndexes[0];
}
